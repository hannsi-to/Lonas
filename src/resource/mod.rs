// シェーダーとテクスチャの登録（§11 ResourceManager）
// CPU 側の処理だけを行い、GPU には触れない。GPU 上の実体は end_frame で GpuContext が作る（§11 GPU との分担）
#![allow(dead_code)] // GpuContext 向けの関数は段階 4〜7 で使う

pub(crate) mod shader;
mod slots;
mod texture;

use std::sync::atomic::{AtomicU32, Ordering};

use crate::error::LonasError;
use shader::{CheckedShader, ShaderReflection, check_wgsl};
use slots::{SlotId, Slots};
pub(crate) use texture::TextureData;

// ResourceManager ごとの識別子。別の ResourceManager の ID を見分ける（§11 ID）
static NEXT_RESOURCE_MANAGER_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderId(SlotId);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(SlotId);

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
    changes: Vec<ResourceChange>,
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceManager {
    pub fn new() -> Self {
        let id = NEXT_RESOURCE_MANAGER_ID.fetch_add(1, Ordering::Relaxed);

        Self {
            id,
            shaders: Slots::new(id, 0),
            // 番号 0 はテクスチャ未指定（ピンク紫の画素）のために予約する
            textures: Slots::new(id, 1),
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
mod tests {
    use super::*;
    use crate::wgsl::STRUCTS;

    const VALID: &str = "@vertex fn main() -> @builtin(position) vec4<f32> { return vec4(0.0); }";

    fn pixels(width: u32, height: u32) -> Vec<u8> {
        vec![255; (width * height * 4) as usize]
    }

    #[test]
    fn registers_valid_shader_and_rejects_invalid() {
        let mut resources = ResourceManager::new();

        assert!(resources.register_shader("fn main( {").is_err());

        let source = format!("{STRUCTS}{VALID}");
        let id = resources.register_shader(&source).unwrap();
        assert!(resources.is_shader_valid(id));
        assert_eq!(resources.shader_source(id), source);
        assert_eq!(resources.take_changes(), vec![ResourceChange::ShaderAdded(id)]);
    }

    #[test]
    fn texture_slots_start_at_one() {
        let mut resources = ResourceManager::new();
        let id = resources.register_texture_rgba(2, 2, &pixels(2, 2)).unwrap();

        assert_eq!(id.slot(), 1);
        assert_eq!(resources.texture_size(id), (2, 2));
    }

    #[test]
    #[should_panic(expected = "画像の大きさとバイト列の長さが合いません")]
    fn rgba_length_mismatch_panics() {
        ResourceManager::new().register_texture_rgba(2, 2, &[0; 15]).unwrap();
    }

    #[test]
    fn unregister_invalidates_and_reuses_slot_with_new_generation() {
        let mut resources = ResourceManager::new();
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
        let mut resources = ResourceManager::new();
        let id = resources.register_shader(VALID).unwrap();

        resources.unregister_shader(id);
        // GPU に反映する前に取り消したため、何も反映しない
        assert!(resources.take_changes().is_empty());
    }

    #[test]
    #[should_panic(expected = "登録解除済みの ShaderId を解除しました")]
    fn double_unregister_panics() {
        let mut resources = ResourceManager::new();
        let id = resources.register_shader(VALID).unwrap();

        resources.unregister_shader(id);
        resources.unregister_shader(id);
    }

    #[test]
    fn ids_from_other_manager_are_distinguished() {
        let mut a = ResourceManager::new();
        let b = ResourceManager::new();
        let id = a.register_shader(VALID).unwrap();

        assert!(a.owns_shader(id));
        assert!(!b.owns_shader(id));
        assert!(!b.is_shader_valid(id));
    }

    #[test]
    fn texture_data_is_taken_once() {
        let mut resources = ResourceManager::new();
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

        let mut resources = ResourceManager::new();
        let id = resources.register_texture(&png).unwrap();

        assert_eq!(resources.take_texture_data(id).unwrap().pixels, vec![255, 0, 0, 255]);
        assert!(resources.register_texture(&[1, 2, 3]).is_err());
    }
}
