// シェーダー・テクスチャ・フォントの登録（§11 ResourceManager・§13 フォントの登録）
// CPU 側の処理だけを行い、GPU には触れない。GPU 上の実体は end_frame で GpuContext が作る（§11 GPU との分担）
#![allow(dead_code)] // GpuContext 向けの関数は段階 4〜7 で使う

pub(crate) mod shader;
mod texture;

use std::sync::atomic::{AtomicU32, Ordering};

use crate::error::LonasError;
use shader::{CheckedShader, ShaderReflection, check_wgsl};
use crate::slots::{SlotId, Slots};
#[cfg(feature = "text")]
use crate::text::vector::{GlyphShape, ShapeKey, VectorCache, tessellate_glyph};
#[cfg(feature = "text")]
use crate::text::{FontData, TextConfig};
pub(crate) use texture::TextureData;

// ResourceManager ごとの識別子。別の ResourceManager の ID を見分ける（§11 ID）
static NEXT_RESOURCE_MANAGER_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderId(SlotId);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(SlotId);

#[cfg(feature = "text")]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontId(SlotId);

// ResourceManager::new に渡す設定（§13 CPU 側の設定）。LonasConfig と同じく既定値を設けない（§3）
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceConfig {
    #[cfg(feature = "text")]
    pub text: TextConfig,
}

impl ShaderId {
    pub(crate) fn index(self) -> u32 {
        self.0.index
    }
}

impl TextureId {
    // texture_rects の番号（§11 UV の変換）。0 は未指定のために予約し、1 から発行する
    pub(crate) fn slot(self) -> u32 {
        self.0.index
    }
}

// end_frame で GPU に反映する登録・解除
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResourceChange {
    ShaderAdded(ShaderId),
    ShaderRemoved(ShaderId),
    TextureAdded(TextureId),
    TextureRemoved(TextureId),
}

struct ShaderEntry {
    source: String,
    module: naga::Module,
    reflection: ShaderReflection,
}

struct TextureEntry {
    // GPU へ転送するまでの画像。転送後は捨てる
    pending: Option<TextureData>,
    width: u32,
    height: u32,
}

pub struct ResourceManager {
    id: u32,
    shaders: Slots<ShaderEntry>,
    textures: Slots<TextureEntry>,
    #[cfg(feature = "text")]
    fonts: Slots<FontData>,
    #[cfg(feature = "text")]
    text_config: TextConfig,
    // 三角形化したグリフ（§13 ベクター）
    #[cfg(feature = "text")]
    vector_cache: VectorCache,
    // ResourceManager のフレームの番号。end_frame で進める（§13 書き込み：write_text の細部）
    #[cfg(feature = "text")]
    text_frame: u64,
    // このフレームに write_text が作ったグリフの数（§13 グリフを作る負担を分ける）
    #[cfg(feature = "text")]
    new_glyphs: u32,
    changes: Vec<ResourceChange>,
}

impl ResourceManager {
    // 値が不正なら panic（§13 CPU 側の設定）
    #[cfg_attr(not(feature = "text"), allow(unused_variables))] // text feature を外すと設定の項目がない
    pub fn new(config: ResourceConfig) -> Self {
        #[cfg(feature = "text")]
        config.text.validate();

        let id = NEXT_RESOURCE_MANAGER_ID.fetch_add(1, Ordering::Relaxed);

        Self {
            id,
            shaders: Slots::new(id, 0),
            // 番号 0 はテクスチャ未指定（ピンク紫の画素）のために予約する
            textures: Slots::new(id, 1),
            #[cfg(feature = "text")]
            fonts: Slots::new(id, 0),
            #[cfg(feature = "text")]
            text_config: config.text,
            #[cfg(feature = "text")]
            vector_cache: VectorCache::default(),
            #[cfg(feature = "text")]
            text_frame: 0,
            #[cfg(feature = "text")]
            new_glyphs: 0,
            changes: Vec::new(),
        }
    }

    // WGSL を naga で検査する。誤りがあれば Err（位置を含むメッセージ）
    // Lonas の struct を使う場合は lonas::wgsl::STRUCTS を先頭につなげる（§11 シェーダーとの約束事）
    pub fn register_shader(&mut self, source: &str) -> Result<ShaderId, LonasError> {
        let CheckedShader { module, reflection } = check_wgsl(source)?;
        let id = ShaderId(self.shaders.insert(ShaderEntry { source: source.to_owned(), module, reflection }));

        self.changes.push(ResourceChange::ShaderAdded(id));

        Ok(id)
    }

    // PNG / JPEG などの符号化済みの画像から登録する（§11 画像の形式）
    #[cfg(feature = "image")]
    pub fn register_texture(&mut self, bytes: &[u8]) -> Result<TextureId, LonasError> {
        Ok(self.insert_texture(TextureData::decode(bytes)?))
    }

    // RGBA 各 8 ビットの生データから登録する。sRGB の値として扱う（§11 色）
    pub fn register_texture_rgba(&mut self, width: u32, height: u32, bytes: &[u8]) -> Result<TextureId, LonasError> {
        Ok(self.insert_texture(TextureData::from_rgba(width, height, bytes.to_vec())))
    }

    // 呼んだ時点で ID を無効にする。無効な ID を使う Object* は end_frame で panic（§11 登録解除）
    pub fn unregister_shader(&mut self, id: ShaderId) {
        self.check_owner(id.0, "ShaderId");
        self.shaders.remove(id.0).expect("登録解除済みの ShaderId を解除しました");

        // まだ GPU に反映していなければ、登録ごと取り消す
        if !remove_pending(&mut self.changes, ResourceChange::ShaderAdded(id)) {
            self.changes.push(ResourceChange::ShaderRemoved(id));
        }
    }

    pub fn unregister_texture(&mut self, id: TextureId) {
        self.check_owner(id.0, "TextureId");
        self.textures.remove(id.0).expect("登録解除済みの TextureId を解除しました");

        if !remove_pending(&mut self.changes, ResourceChange::TextureAdded(id)) {
            self.changes.push(ResourceChange::TextureRemoved(id));
        }
    }

    pub fn is_shader_valid(&self, id: ShaderId) -> bool {
        id.0.owner == self.id && self.shaders.get(id.0).is_some()
    }

    pub fn is_texture_valid(&self, id: TextureId) -> bool {
        id.0.owner == self.id && self.textures.get(id.0).is_some()
    }

    pub fn texture_size(&self, id: TextureId) -> (u32, u32) {
        let entry = self.texture_entry(id);

        (entry.width, entry.height)
    }

    // TrueType / OpenType のフォントを登録する。読めなければ Err（§13 フォントの登録）
    #[cfg(feature = "text")]
    pub fn register_font(&mut self, bytes: impl Into<Vec<u8>>) -> Result<FontId, LonasError> {
        let font = FontData::new(bytes.into()).ok_or(LonasError::InvalidFont)?;

        Ok(FontId(self.fonts.insert(font)))
    }

    // 呼んだ時点で ID を無効にする。無効な ID を使う TextStyle は layout_text で panic
    #[cfg(feature = "text")]
    pub fn unregister_font(&mut self, id: FontId) {
        self.check_owner(id.0, "FontId");
        self.fonts.remove(id.0).expect("登録解除済みの FontId を解除しました");
        self.vector_cache.remove_font(id);
    }

    #[cfg(feature = "text")]
    pub fn is_font_valid(&self, id: FontId) -> bool {
        id.0.owner == self.id && self.fonts.get(id.0).is_some()
    }

    #[cfg(feature = "text")]
    pub(crate) fn font(&self, id: FontId) -> &FontData {
        self.check_owner(id.0, "FontId");
        self.fonts.get(id.0).expect("登録解除済みの FontId です")
    }

    #[cfg(feature = "text")]
    pub(crate) fn text_config(&self) -> &TextConfig {
        &self.text_config
    }

    // キャッシュになければ三角形化して入れる。limited なら max_new_glyphs_per_frame を超えた分は作らず false
    #[cfg(feature = "text")]
    pub(crate) fn ensure_vector_shape(&mut self, key: ShapeKey, limited: bool) -> bool {
        if self.vector_cache.get(&key, self.text_frame).is_some() {
            return true;
        }

        if limited && self.text_config.max_new_glyphs_per_frame.is_some_and(|max| self.new_glyphs >= max) {
            return false;
        }

        let shape = tessellate_glyph(self.font(key.font), &key);
        self.vector_cache.insert(key, shape, self.text_frame);

        if limited {
            self.new_glyphs += 1;
        }

        true
    }

    // ensure_vector_shape の後に呼ぶ
    #[cfg(feature = "text")]
    pub(crate) fn vector_shape(&self, key: &ShapeKey) -> &GlyphShape {
        self.vector_cache.peek(key).expect("キャッシュにない形です")
    }

    #[cfg(feature = "text")]
    pub(crate) fn vector_cache(&self) -> &VectorCache {
        &self.vector_cache
    }

    // end_frame の最後に呼ぶ。キャッシュの追い出し → 作った数を 0 に → フレームの番号を進める
    #[cfg(feature = "text")]
    pub(crate) fn finish_text_frame(&mut self) {
        self.vector_cache.evict(self.text_config.vector_cache_max_bytes, self.text_frame);
        self.new_glyphs = 0;
        self.text_frame += 1;
    }

    // ---- end_frame（GpuContext）が使う ----

    pub(crate) fn owns_shader(&self, id: ShaderId) -> bool {
        id.0.owner == self.id
    }

    pub(crate) fn owns_texture(&self, id: TextureId) -> bool {
        id.0.owner == self.id
    }

    // 前回から溜まった登録・解除を、起きた順に取り出す
    pub(crate) fn take_changes(&mut self) -> Vec<ResourceChange> {
        std::mem::take(&mut self.changes)
    }

    pub(crate) fn shader_source(&self, id: ShaderId) -> &str {
        &self.shader_entry(id).source
    }

    pub(crate) fn shader_module(&self, id: ShaderId) -> &naga::Module {
        &self.shader_entry(id).module
    }

    pub(crate) fn shader_reflection(&self, id: ShaderId) -> &ShaderReflection {
        &self.shader_entry(id).reflection
    }

    // GPU へ転送する画像を受け取る。受け取った後、ResourceManager 側の画像は捨てる
    pub(crate) fn take_texture_data(&mut self, id: TextureId) -> Option<TextureData> {
        self.check_owner(id.0, "TextureId");
        self.textures.get_mut(id.0).expect("登録解除済みの TextureId です").pending.take()
    }

    fn insert_texture(&mut self, data: TextureData) -> TextureId {
        let (width, height) = (data.width, data.height);
        let id = TextureId(self.textures.insert(TextureEntry { pending: Some(data), width, height }));

        self.changes.push(ResourceChange::TextureAdded(id));

        id
    }

    fn shader_entry(&self, id: ShaderId) -> &ShaderEntry {
        self.check_owner(id.0, "ShaderId");
        self.shaders.get(id.0).expect("登録解除済みの ShaderId です")
    }

    fn texture_entry(&self, id: TextureId) -> &TextureEntry {
        self.check_owner(id.0, "TextureId");
        self.textures.get(id.0).expect("登録解除済みの TextureId です")
    }

    fn check_owner(&self, id: SlotId, name: &str) {
        assert_eq!(id.owner, self.id, "別の ResourceManager の {name} です");
    }
}

fn remove_pending(changes: &mut Vec<ResourceChange>, change: ResourceChange) -> bool {
    match changes.iter().position(|&pending| pending == change) {
        Some(index) => {
            changes.remove(index);
            true
        }
        None => false,
    }
}

#[cfg(test)]
impl ResourceConfig {
    // テスト用の設定
    pub(crate) fn for_tests() -> Self {
        Self {
            #[cfg(feature = "text")]
            text: TextConfig {
                sdf_em_size: 48,
                sdf_range: 8.0,
                max_new_glyphs_per_frame: Some(64),
                vector_cache_max_bytes: 64 << 20,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wgsl::STRUCTS;

    const VALID: &str = "@vertex fn main() -> @builtin(position) vec4<f32> { return vec4(0.0); }";

    fn pixels(width: u32, height: u32) -> Vec<u8> {
        vec![255; (width * height * 4) as usize]
    }

    #[test]
    fn registers_valid_shader_and_rejects_invalid() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());

        assert!(resources.register_shader("fn main( {").is_err());

        let source = format!("{STRUCTS}{VALID}");
        let id = resources.register_shader(&source).unwrap();
        assert!(resources.is_shader_valid(id));
        assert_eq!(resources.shader_source(id), source);
        assert_eq!(resources.take_changes(), vec![ResourceChange::ShaderAdded(id)]);
    }

    #[test]
    fn texture_slots_start_at_one() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let id = resources.register_texture_rgba(2, 2, &pixels(2, 2)).unwrap();

        assert_eq!(id.slot(), 1);
        assert_eq!(resources.texture_size(id), (2, 2));
    }

    #[test]
    #[should_panic(expected = "画像の大きさとバイト列の長さが合いません")]
    fn rgba_length_mismatch_panics() {
        ResourceManager::new(ResourceConfig::for_tests()).register_texture_rgba(2, 2, &[0; 15]).unwrap();
    }

    #[test]
    fn unregister_invalidates_and_reuses_slot_with_new_generation() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let old = resources.register_texture_rgba(1, 1, &pixels(1, 1)).unwrap();
        resources.take_changes();

        resources.unregister_texture(old);
        assert!(!resources.is_texture_valid(old));

        let new = resources.register_texture_rgba(1, 1, &pixels(1, 1)).unwrap();
        // 番号は再利用されるが、世代で古い ID と区別できる
        assert_eq!(new.slot(), old.slot());
        assert_ne!(new, old);
        assert!(!resources.is_texture_valid(old));
        assert!(resources.is_texture_valid(new));
        assert_eq!(
            resources.take_changes(),
            vec![ResourceChange::TextureRemoved(old), ResourceChange::TextureAdded(new)],
        );
    }

    #[test]
    fn unregister_before_end_frame_cancels_registration() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let id = resources.register_shader(VALID).unwrap();

        resources.unregister_shader(id);
        // GPU に反映する前に取り消したため、何も反映しない
        assert!(resources.take_changes().is_empty());
    }

    #[test]
    #[should_panic(expected = "登録解除済みの ShaderId を解除しました")]
    fn double_unregister_panics() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let id = resources.register_shader(VALID).unwrap();

        resources.unregister_shader(id);
        resources.unregister_shader(id);
    }

    #[test]
    fn ids_from_other_manager_are_distinguished() {
        let mut a = ResourceManager::new(ResourceConfig::for_tests());
        let b = ResourceManager::new(ResourceConfig::for_tests());
        let id = a.register_shader(VALID).unwrap();

        assert!(a.owns_shader(id));
        assert!(!b.owns_shader(id));
        assert!(!b.is_shader_valid(id));
    }

    #[test]
    fn texture_data_is_taken_once() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let id = resources.register_texture_rgba(1, 1, &pixels(1, 1)).unwrap();

        assert!(resources.take_texture_data(id).is_some());
        // 転送した後は捨てられている
        assert!(resources.take_texture_data(id).is_none());
        assert_eq!(resources.texture_size(id), (1, 1));
    }

    #[cfg(feature = "image")]
    #[test]
    fn decodes_png() {
        // 1×1 の赤い画素の PNG を作って読み込む
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let id = resources.register_texture(&png).unwrap();

        assert_eq!(resources.take_texture_data(id).unwrap().pixels, vec![255, 0, 0, 255]);
        assert!(resources.register_texture(&[1, 2, 3]).is_err());
    }
}
