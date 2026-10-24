// アトラスに置くグリフの画素（§13 グリフのアトラス・グリフの作り方・ヒンティング）
// Bitmap は画面上の大きさでラスタライズした覆い率、Sdf / Msdf は sdf_em_size の大きさの距離の場（sdf.rs、msdf feature）
// 画素は ResourceManager が持ち、アトラス上の配置は end_frame で GpuContext が決める

use std::collections::HashMap;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, SmoothMode, Target};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use zeno::{Command, Fill, Format, Mask, Point};

use crate::resource::FontId;
use crate::text::FontData;
use crate::text::vector::{Recorder, Segment, copy_offsets};

// ヒンティング（§13 ヒンティング）。Bitmap だけで使う
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Hinting {
    // 輪郭をそのまま使う
    None,
    // skrifa の自動ヒンティングを縦方向だけに使う
    Light,
}

// 画素の種類
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RasterKind {
    // 覆い率（R8Unorm）
    Coverage,
    // 1 チャンネルの距離（R8Unorm）
    Sdf,
    // MTSDF（Rgba8Unorm）
    Msdf,
}

// Bitmap は (FontId, グリフ, 画素の大きさ, ヒンティング, 太らせる量, 縁取りの太さ)。量は em に対する割合の f32 のビット列
// Sdf / Msdf は (FontId, グリフ) だけで決まる（太字・縁取りはシェーダーで作る）。残りは 0
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BitmapKey {
    pub kind: RasterKind,
    pub font: FontId,
    pub glyph: u32,
    pub pixel_size: u32,
    pub hinting: Hinting,
    pub bold: u32,
    pub outline: u32,
}

impl BitmapKey {
    pub(crate) fn new(font: FontId, glyph: u32, pixel_size: u32, hinting: Hinting, bold: f32, outline: f32) -> Self {
        Self {
            kind: RasterKind::Coverage,
            font,
            glyph,
            pixel_size,
            hinting,
            bold: bold.max(0.0).to_bits(),
            outline: outline.max(0.0).to_bits(),
        }
    }

    pub(crate) fn distance(font: FontId, glyph: u32, multi_channel: bool) -> Self {
        let kind = if multi_channel { RasterKind::Msdf } else { RasterKind::Sdf };

        Self { kind, font, glyph, pixel_size: 0, hinting: Hinting::None, bold: 0, outline: 0 }
    }
}

// Sdf / Msdf を作る大きさ（TextConfig の値）
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DistanceSettings {
    pub em_size: u32,
    pub range: f32,
}

// グリフの画素。left・top はグリフの原点（ベースライン上）からの位置（画素、y は下向き）
// bytes_per_pixel は 1（R8Unorm）か 4（Rgba8Unorm）
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RasterGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub bytes_per_pixel: u32,
    pub pixels: Vec<u8>,
    // Sdf / Msdf の距離の幅と 1 em の大きさ。glyph_rects に書く（§13 Sdf / Msdf の細部）
    pub distance: Option<DistanceSettings>,
}

impl RasterGlyph {
    pub(crate) fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

pub(crate) struct BitmapGlyph {
    // グリフの番号（glyph_rects の要素）。空のグリフ（空白など）は 0 で、書かない
    pub slot: u32,
    pub raster: RasterGlyph,
    // 最後に使った ResourceManager のフレームの番号
    pub last_used: u64,
    // アトラスに置かれているか。追い出されたら false にし、次に使った時に置き直す
    pub resident: bool,
    // 置く要求を溜めたか（同じフレームに 2 回溜めない）
    pub queued: bool,
}

// Bitmap のグリフのキャッシュと、番号の発行（§13 グリフの番号と、追い出した後）
#[derive(Default)]
pub(crate) struct BitmapCache {
    glyphs: HashMap<BitmapKey, BitmapGlyph>,
    // 番号 → キー。番号 0 は塗りつぶしの領域のため使わない
    slots: Vec<Option<BitmapKey>>,
    free_slots: Vec<u32>,
    // end_frame で置く番号（新しく作ったもの・追い出されていたのに使ったもの）
    uploads: Vec<u32>,
    // end_frame でアトラスから外す番号（フォントの登録解除）
    removals: Vec<u32>,
    // (FontId, 画素の大きさ) ごとのヒンティング
    hinting: HashMap<(FontId, u32), HintingInstance>,
}

impl BitmapCache {
    // あれば使ったフレームを記録し、アトラスになければ置く要求を溜める
    pub(crate) fn touch(&mut self, key: &BitmapKey, frame: u64) -> Option<&BitmapGlyph> {
        let glyph = self.glyphs.get_mut(key)?;
        glyph.last_used = frame;

        if glyph.slot != 0 && !glyph.resident && !glyph.queued {
            glyph.queued = true;
            self.uploads.push(glyph.slot);
        }

        Some(&*glyph)
    }

    pub(crate) fn peek(&self, key: &BitmapKey) -> Option<&BitmapGlyph> {
        self.glyphs.get(key)
    }

    // 画素を作って入れる。空でなければ番号を発行し、置く要求を溜める
    pub(crate) fn insert(&mut self, font: &FontData, key: BitmapKey, frame: u64, distance: DistanceSettings) {
        let raster = match key.kind {
            RasterKind::Coverage => self.rasterize(font, &key),
            RasterKind::Sdf | RasterKind::Msdf => generate_distance(font, &key, distance),
        };
        let slot = if raster.is_empty() { 0 } else { self.issue_slot(key) };

        if slot != 0 {
            self.uploads.push(slot);
        }

        self.glyphs.insert(key, BitmapGlyph { slot, raster, last_used: frame, resident: false, queued: slot != 0 });
    }

    fn issue_slot(&mut self, key: BitmapKey) -> u32 {
        match self.free_slots.pop() {
            Some(slot) => {
                self.slots[slot as usize] = Some(key);
                slot
            }
            None => {
                if self.slots.is_empty() {
                    // 番号 0 は塗りつぶしの領域
                    self.slots.push(None);
                }
                self.slots.push(Some(key));
                (self.slots.len() - 1) as u32
            }
        }
    }

    // ---- end_frame（GpuContext）が使う ----

    pub(crate) fn take_uploads(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.uploads)
    }

    pub(crate) fn take_removals(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.removals)
    }

    pub(crate) fn glyph(&self, slot: u32) -> Option<&BitmapGlyph> {
        let key = self.slots.get(slot as usize).copied().flatten()?;
        self.glyphs.get(&key)
    }

    // アトラスに置いた・追い出した、を記録する
    pub(crate) fn set_resident(&mut self, slot: u32, resident: bool) {
        if let Some(key) = self.slots.get(slot as usize).copied().flatten()
            && let Some(glyph) = self.glyphs.get_mut(&key)
        {
            glyph.resident = resident;
            glyph.queued = false;
        }
    }

    // 登録を解除したフォントのグリフを捨て、番号を空ける
    pub(crate) fn remove_font(&mut self, font: FontId) {
        let removed: Vec<BitmapKey> = self.glyphs.keys().filter(|key| key.font == font).copied().collect();

        for key in removed {
            let glyph = self.glyphs.remove(&key).expect("キーから取り出したグリフです");

            if glyph.slot != 0 {
                self.slots[glyph.slot as usize] = None;
                self.free_slots.push(glyph.slot);
                self.uploads.retain(|&slot| slot != glyph.slot);
                self.removals.push(glyph.slot);
            }
        }

        self.hinting.retain(|&(hinted, _), _| hinted != font);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.glyphs.len()
    }

    // zeno で、nonzero の規則でラスタライズする（§13 グリフの作り方）
    fn rasterize(&mut self, font: &FontData, key: &BitmapKey) -> RasterGlyph {
        let Ok(font_ref) = FontRef::from_index(font.bytes.as_slice(), 0) else {
            return RasterGlyph::default();
        };
        let outlines = font_ref.outline_glyphs();
        let Some(glyph) = outlines.get(GlyphId::new(key.glyph)) else {
            return RasterGlyph::default();
        };
        let size = Size::new(key.pixel_size as f32);
        let mut recorder = Recorder::default();

        let drawn = match key.hinting {
            Hinting::None => glyph.draw(DrawSettings::unhinted(size, LocationRef::default()), &mut recorder),
            Hinting::Light => {
                let instance = match self.hinting.entry((key.font, key.pixel_size)) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let Ok(instance) = HintingInstance::new(&outlines, size, LocationRef::default(), light_hinting()) else {
                            return RasterGlyph::default();
                        };
                        entry.insert(instance)
                    }
                };
                glyph.draw(&*instance, &mut recorder)
            }
        };

        if drawn.is_err() {
            return RasterGlyph::default();
        }

        let pixels = key.pixel_size as f32;
        let offsets: Vec<[f32; 2]> =
            copy_offsets(f32::from_bits(key.bold), f32::from_bits(key.outline)).into_iter().map(|[x, y]| [x * pixels, y * pixels]).collect();
        let commands = commands(&recorder.0, &offsets);

        if commands.is_empty() {
            return RasterGlyph::default();
        }

        let (coverage, placement) = Mask::new(&commands).style(Fill::NonZero).format(Format::Alpha).render();

        RasterGlyph {
            left: placement.left,
            top: placement.top,
            width: placement.width,
            height: placement.height,
            bytes_per_pixel: 1,
            pixels: coverage,
            distance: None,
        }
    }
}

// Sdf / Msdf の距離の場（§13 Sdf / Msdf の細部）。msdf feature がなければ write_text などで先に panic している
#[cfg(feature = "msdf")]
fn generate_distance(font: &FontData, key: &BitmapKey, distance: DistanceSettings) -> RasterGlyph {
    let Ok(font_ref) = FontRef::from_index(font.bytes.as_slice(), 0) else {
        return RasterGlyph::default();
    };
    let Some(glyph) = font_ref.outline_glyphs().get(GlyphId::new(key.glyph)) else {
        return RasterGlyph::default();
    };
    let mut recorder = Recorder::default();

    if glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut recorder).is_err() {
        return RasterGlyph::default();
    }

    let scale = distance.em_size as f64 / font.units_per_em as f64;

    RasterGlyph {
        distance: Some(distance),
        ..crate::text::sdf::generate(&recorder.0, scale, distance.range as f64, key.kind == RasterKind::Msdf)
    }
}

#[cfg(not(feature = "msdf"))]
fn generate_distance(_: &FontData, _: &BitmapKey, _: DistanceSettings) -> RasterGlyph {
    unreachable!("Sdf / Msdf の画素は msdf feature がなければ作らない")
}

// 横方向は変えず、縦方向だけを画素の格子に合わせる（§13 ヒンティング）
fn light_hinting() -> HintingOptions {
    HintingOptions {
        engine: Engine::Auto(None),
        target: Target::Smooth { mode: SmoothMode::Light, symmetric_rendering: true, preserve_linear_metrics: true },
    }
}

// 輪郭の複製（合成の太字・縁取り）を zeno の命令に並べる。y は下向きにする（画像の行の順）
fn commands(segments: &[Segment], offsets: &[[f32; 2]]) -> Vec<Command> {
    let mut commands = Vec::new();

    for &[dx, dy] in offsets {
        let at = |x: f32, y: f32| Point::new(x + dx, -(y + dy));
        let mut open = false;

        for &segment in segments {
            match segment {
                Segment::Move(x, y) => {
                    if open {
                        commands.push(Command::Close);
                    }
                    commands.push(Command::MoveTo(at(x, y)));
                    open = true;
                }
                Segment::Line(x, y) if open => commands.push(Command::LineTo(at(x, y))),
                Segment::Quad(cx, cy, x, y) if open => commands.push(Command::QuadTo(at(cx, cy), at(x, y))),
                Segment::Cubic(cx0, cy0, cx1, cy1, x, y) if open => commands.push(Command::CurveTo(at(cx0, cy0), at(cx1, cy1), at(x, y))),
                Segment::Close if open => {
                    commands.push(Command::Close);
                    open = false;
                }
                _ => {}
            }
        }

        if open {
            commands.push(Command::Close);
        }
    }

    commands
}
