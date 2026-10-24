// ベクター：輪郭の三角形化とキャッシュ（§13 ベクター：輪郭の三角形化）
// 輪郭は skrifa で取り出し、lyon の FillTessellator（nonzero）で三角形化する。em を 1 とした単位、y は上向き（フォントの座標）のまま持つ

use std::collections::HashMap;
use std::f32::consts::FRAC_1_SQRT_2;

use lyon_tessellation::math::point;
use lyon_tessellation::path::Path;
use lyon_tessellation::{BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::resource::FontId;
use crate::text::FontData;

// 画面上の許容誤差（px）。util_2d の円と同じ
const SCREEN_TOLERANCE: f32 = 0.25;
// 大きさの段階の指数の範囲（1〜2^16 px）
const MAX_STAGE: i32 = 16;

// (FontId, グリフ, 大きさの段階, 太らせる量, 縁取りの太さ)。量は f32 のビット列で、0 なら合成しない
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShapeKey {
    pub font: FontId,
    pub glyph: u32,
    pub stage: i32,
    pub bold: u32,
    pub outline: u32,
}

impl ShapeKey {
    pub(crate) fn new(font: FontId, glyph: u32, stage: i32, bold: f32, outline: f32) -> Self {
        Self { font, glyph, stage, bold: bold.max(0.0).to_bits(), outline: outline.max(0.0).to_bits() }
    }
}

// 三角形化した形。三角形は xy 平面（y 上向き）で反時計回り
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GlyphShape {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl GlyphShape {
    fn bytes(&self) -> usize {
        self.vertices.len() * size_of::<[f32; 2]>() + self.indices.len() * size_of::<u32>() + size_of::<CachedShape>()
    }
}

struct CachedShape {
    shape: GlyphShape,
    // 最後に使った ResourceManager のフレームの番号
    last_used: u64,
}

#[derive(Default)]
pub(crate) struct VectorCache {
    shapes: HashMap<ShapeKey, CachedShape>,
    bytes: usize,
}

impl VectorCache {
    // あれば使ったフレームを記録して返す
    pub(crate) fn get(&mut self, key: &ShapeKey, frame: u64) -> Option<&GlyphShape> {
        let cached = self.shapes.get_mut(key)?;
        cached.last_used = frame;

        Some(&cached.shape)
    }

    pub(crate) fn contains(&self, key: &ShapeKey) -> bool {
        self.shapes.contains_key(key)
    }

    // 使ったフレームを記録せずに返す
    pub(crate) fn peek(&self, key: &ShapeKey) -> Option<&GlyphShape> {
        self.shapes.get(key).map(|cached| &cached.shape)
    }

    pub(crate) fn insert(&mut self, key: ShapeKey, shape: GlyphShape, frame: u64) {
        self.bytes += shape.bytes();

        if let Some(old) = self.shapes.insert(key, CachedShape { shape, last_used: frame }) {
            self.bytes -= old.shape.bytes();
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn len(&self) -> usize {
        self.shapes.len()
    }

    // 上限を超えていれば、そのフレームに使わなかった形を、最後に使ったフレームが古い順に捨てる
    // そのフレームに使った形だけで超える場合は、超えたままにする（§13 ベクター）
    pub(crate) fn evict(&mut self, max_bytes: usize, frame: u64) {
        if self.bytes <= max_bytes {
            return;
        }

        let mut candidates: Vec<(u64, ShapeKey)> =
            self.shapes.iter().filter(|(_, cached)| cached.last_used < frame).map(|(key, cached)| (cached.last_used, *key)).collect();
        candidates.sort_unstable_by_key(|&(last_used, _)| last_used);

        for (_, key) in candidates {
            if self.bytes <= max_bytes {
                break;
            }

            if let Some(removed) = self.shapes.remove(&key) {
                self.bytes -= removed.shape.bytes();
            }
        }
    }

    // 登録を解除したフォントの形を捨てる
    pub(crate) fn remove_font(&mut self, font: FontId) {
        let bytes = &mut self.bytes;

        self.shapes.retain(|key, cached| {
            let keep = key.font != font;
            if !keep {
                *bytes -= cached.shape.bytes();
            }
            keep
        });
    }
}

// 画面上の大きさ（px）を 2 倍ごとの段階に切り上げた指数
pub(crate) fn size_stage(screen_size: f32) -> i32 {
    (screen_size.max(1.0).log2().ceil() as i32).clamp(0, MAX_STAGE)
}

// 輪郭の 1 つの要素（フォントの単位か、ヒンティングした画素の単位。y は上向き）
#[derive(Copy, Clone)]
pub(crate) enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Default)]
pub(crate) struct Recorder(pub Vec<Segment>);

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Segment::Move(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(Segment::Line(x, y));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.push(Segment::Quad(cx0, cy0, x, y));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(Segment::Cubic(cx0, cy0, cx1, cy1, x, y));
    }

    fn close(&mut self) {
        self.0.push(Segment::Close);
    }
}

// グリフの輪郭を三角形化する。輪郭のないグリフ（空白など）・読めないグリフは空の形
// bold・outline は em に対する割合（ShapeKey と同じ値）
pub(crate) fn tessellate_glyph(font: &FontData, key: &ShapeKey) -> GlyphShape {
    let Ok(font_ref) = FontRef::from_index(font.bytes.as_slice(), 0) else {
        return GlyphShape::default();
    };
    let Some(glyph) = font_ref.outline_glyphs().get(GlyphId::new(key.glyph)) else {
        return GlyphShape::default();
    };

    let mut recorder = Recorder::default();
    if glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut recorder).is_err() {
        return GlyphShape::default();
    }

    let scale = 1.0 / font.units_per_em;
    let tolerance = SCREEN_TOLERANCE / (1u32 << key.stage) as f32;
    let offsets = copy_offsets(f32::from_bits(key.bold), f32::from_bits(key.outline));

    tessellate_segments(&recorder.0, scale, &offsets, tolerance)
}

// 合成の太字・縁取りでずらす量（em）。元の形（0, 0）を含む
pub(crate) fn copy_offsets(bold: f32, outline: f32) -> Vec<[f32; 2]> {
    let mut offsets = vec![[0.0, 0.0]];

    // 合成の太字：上下左右に bold ずらした複製
    if bold > 0.0 {
        offsets.extend([[bold, 0.0], [-bold, 0.0], [0.0, bold], [0.0, -bold]]);
    }

    // 縁取り：(太字を含む) 形を 8 方向へ outline ずらした複製
    if outline > 0.0 {
        let diagonal = outline * FRAC_1_SQRT_2;
        let directions = [
            [outline, 0.0],
            [-outline, 0.0],
            [0.0, outline],
            [0.0, -outline],
            [diagonal, diagonal],
            [diagonal, -diagonal],
            [-diagonal, diagonal],
            [-diagonal, -diagonal],
        ];
        let base = offsets.clone();

        offsets.extend(base.iter().flat_map(|&[x, y]| directions.map(|[dx, dy]| [x + dx, y + dy])));
    }

    offsets
}

// 輪郭の複製を 1 つのパスに入れ、nonzero で三角形化する（§13 書式の作り方の表：Bold・Outline）
fn tessellate_segments(segments: &[Segment], scale: f32, offsets: &[[f32; 2]], tolerance: f32) -> GlyphShape {
    let mut builder = Path::builder();

    for &[dx, dy] in offsets {
        let at = |x: f32, y: f32| point(x * scale + dx, y * scale + dy);
        let mut open = false;

        for &segment in segments {
            match segment {
                Segment::Move(x, y) => {
                    if open {
                        builder.end(true);
                    }
                    builder.begin(at(x, y));
                    open = true;
                }
                Segment::Line(x, y) if open => {
                    builder.line_to(at(x, y));
                }
                Segment::Quad(cx, cy, x, y) if open => {
                    builder.quadratic_bezier_to(at(cx, cy), at(x, y));
                }
                Segment::Cubic(cx0, cy0, cx1, cy1, x, y) if open => {
                    builder.cubic_bezier_to(at(cx0, cy0), at(cx1, cy1), at(x, y));
                }
                Segment::Close if open => {
                    builder.end(true);
                    open = false;
                }
                // move_to の前の要素は捨てる（壊れた輪郭）
                _ => {}
            }
        }

        if open {
            builder.end(true);
        }
    }

    let path = builder.build();
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let options = FillOptions::tolerance(tolerance).with_fill_rule(FillRule::NonZero);
    let result = FillTessellator::new().tessellate_path(
        &path,
        &options,
        &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| vertex.position().to_array()),
    );

    if result.is_err() {
        return GlyphShape::default();
    }

    let mut shape = GlyphShape { vertices: buffers.vertices, indices: buffers.indices };
    make_counter_clockwise(&mut shape);

    shape
}

// lyon の出力の向きに頼らず、三角形ごとの符号付き面積で反時計回りに揃える（§13 ベクター）
fn make_counter_clockwise(shape: &mut GlyphShape) {
    for triangle in shape.indices.chunks_exact_mut(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| shape.vertices[index as usize]);

        if signed_area(a, b, c) < 0.0 {
            triangle.swap(1, 2);
        }
    }
}

pub(crate) fn signed_area(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, y: f32, size: f32, counter_clockwise: bool) -> Vec<Segment> {
        let mut corners = [(x, y), (x + size, y), (x + size, y + size), (x, y + size)];
        if !counter_clockwise {
            corners.reverse();
        }

        let mut segments = vec![Segment::Move(corners[0].0, corners[0].1)];
        segments.extend(corners[1..].iter().map(|&(x, y)| Segment::Line(x, y)));
        segments.push(Segment::Close);
        segments
    }

    fn area(shape: &GlyphShape) -> f32 {
        shape
            .indices
            .chunks_exact(3)
            .map(|triangle| signed_area(shape.vertices[triangle[0] as usize], shape.vertices[triangle[1] as usize], shape.vertices[triangle[2] as usize]))
            .sum()
    }

    #[test]
    fn overlapping_contours_fill_without_holes() {
        // 同じ向きと逆の向きの、重なった 2 つの四角形。nonzero ではどちらも和集合になる
        for counter_clockwise in [true, false] {
            let mut segments = square(0.0, 0.0, 2.0, true);
            segments.extend(square(1.0, 1.0, 2.0, counter_clockwise));
            let shape = tessellate_segments(&segments, 1.0, &[[0.0, 0.0]], 0.01);

            if counter_clockwise {
                // 和集合：4 + 4 − 1（重なり）。重なりが 2 回数えられたり、穴になったりしない
                assert!((area(&shape) - 7.0).abs() < 1e-4, "{}", area(&shape));
            } else {
                // 逆向きの重なりは巻き数 0 で穴になる（nonzero の規則どおり）
                assert!((area(&shape) - 6.0).abs() < 1e-4, "{}", area(&shape));
            }
        }
    }

    #[test]
    fn triangles_are_counter_clockwise() {
        let shape = tessellate_segments(&square(0.0, 0.0, 1.0, false), 1.0, &[[0.0, 0.0]], 0.01);

        assert!(!shape.indices.is_empty());
        for triangle in shape.indices.chunks_exact(3) {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| shape.vertices[index as usize]);
            assert!(signed_area(a, b, c) >= 0.0);
        }
    }

    #[test]
    fn copies_are_merged_into_one_surface() {
        // 太字の複製は重なり合うが、nonzero で 1 つの面になり、面積は複製の数倍にならない
        let shape = tessellate_segments(&square(0.0, 0.0, 1.0, true), 1.0, &copy_offsets(0.1, 0.0), 0.01);

        // 元の四角形を上下左右に 0.1 広げた十字の形：1.2 × 1.0 + 1.0 × 1.2 − 1.0 × 1.0
        assert!((area(&shape) - 1.4).abs() < 1e-4, "{}", area(&shape));
    }

    #[test]
    fn size_stages_round_up_by_powers_of_two() {
        assert_eq!(size_stage(0.5), 0);
        assert_eq!(size_stage(16.0), 4);
        assert_eq!(size_stage(17.0), 5);
        assert_eq!(size_stage(1.0e9), MAX_STAGE);
    }
}
