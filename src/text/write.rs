// Object* への書き込み（§13 整形と書き込みの分け方・書き込み：write_text の細部）
// begin〜end の間で毎フレーム呼ぶ。キャッシュした形を写すだけで、キャッシュにない形だけを三角形化する

use crate::color::Color;
use crate::config::Blend;
use crate::math::Vector3f;
use crate::object::{Object2D, Object3D, stroke};
use crate::resource::ResourceManager;
use crate::text::layout::{PlacedGlyph, PlacedLine, TextLayout, TextRow};
use crate::text::vector::{GlyphShape, ShapeKey, signed_area, size_stage};
use crate::vertex::Vertex;

// TextLayout の y（下向き）を、書き込む座標のどちらに向けるか
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum YAxis {
    // 2D の Origin2D::TopLeft
    Down,
    // それ以外（3D を含む）
    Up,
}

// 描き方（§13 描き方の選択）。Bitmap は段階 11、Sdf / Msdf は段階 12 で足す
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GlyphMethod {
    // 輪郭を三角形化する
    Vector,
}

// 書き込みの設定。変えても整形し直さない
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextWrite {
    // 1 行目の上端の左端（§13 位置と揃え）
    pub position: Vector3f,
    // 書式で色を上書きしていない字の色
    pub color: Color,
    pub y_axis: YAxis,
    // 1 単位が画面の何ピクセルか。曲線の細かさに使う。0 以下か有限でなければ panic
    pub pixel_scale: f32,
    // TextFormat::Obfuscated で選ぶ字の種
    pub obfuscation_seed: u64,
    pub method: GlyphMethod,
}

// write_text を使える Object*。crate の外から実装できない
pub trait TextObject: sealed::Target {}

impl TextObject for Object2D {}
impl TextObject for Object3D {}

pub(crate) mod sealed {
    use crate::vertex::Vertex;

    pub trait Target {
        fn written_vertex_count(&self) -> usize;
        fn push_vertex(&mut self, vertex: Vertex);
        fn push_index(&mut self, index: u32);
        fn new_shape(&mut self);
        // 影・縁取りのあるテキストを書く時に呼ぶ（§13 3D の影・縁取り）
        fn check_effects(&mut self);
        // Ghost で透かす字を書く時に呼ぶ（§13 書き込みの細部）
        fn check_ghost(&mut self);
    }
}

macro_rules! text_target {
    ($name:ident, $check:expr) => {
        impl sealed::Target for $name {
            fn written_vertex_count(&self) -> usize {
                self.get_written_vertex_count()
            }

            fn push_vertex(&mut self, vertex: Vertex) {
                self.vertex(vertex);
            }

            fn push_index(&mut self, index: u32) {
                self.index(index);
            }

            fn new_shape(&mut self) {
                self.begin_shape();
            }

            fn check_effects(&mut self) {
                let check: fn(&mut $name) = $check;
                check(self);
            }

            // blend() を呼んでいない場合は LonasConfig の値が使われ、Object* からは分からないため警告しない
            fn check_ghost(&mut self) {
                if self.core.blend == Some(Blend::Opaque) {
                    self.core.warn_once(
                        "text_opaque_ghost",
                        "Ghost のあるテキストを Blend::Opaque の Object に書きました。不透明度が捨てられるため透けません。Blend::Alpha で描いてください",
                    );
                }
            }
        }
    };
}

text_target!(Object2D, |_| {});
text_target!(Object3D, |object| {
    // blend() を呼んでいない場合は LonasConfig の値が使われ、Object3D からは分からないため警告しない
    if object.core.blend == Some(Blend::Opaque) {
        object.core.warn_once(
            "text_opaque_effects",
            "影・縁取りのあるテキストを Blend::Opaque の Object3D に書きました。同じ深さの字が消えるため、Blend::Alpha で描いてください（§13 3D の影・縁取り）",
        );
    }
});

// TextLayout を Object* に書く（§13 整形と書き込みの分け方）
// グリフごとに begin_shape() で区切り、index() で書く。paint_type() は要らない
pub fn write_text<T: TextObject>(object: &mut T, resources: &mut ResourceManager, layout: &TextLayout, write: &TextWrite) {
    assert!(
        write.pixel_scale.is_finite() && write.pixel_scale > 0.0,
        "TextWrite::pixel_scale は 0 より大きい有限の値にしてください（{}）",
        write.pixel_scale,
    );

    match write.method {
        GlyphMethod::Vector => {}
    }

    if layout.glyphs().iter().any(|glyph| glyph.style.shadow.is_some() || glyph.style.outline.is_some()) {
        object.check_effects();
    }

    if layout.glyphs().iter().any(|glyph| glyph.style.ghost > 0.0) || layout.lines().iter().any(|line| line.ghost > 0.0) {
        object.check_ghost();
    }

    let writer = Writer { write, layout };

    for (index, glyph) in layout.glyphs().iter().enumerate() {
        writer.glyph(object, resources, index, glyph);
    }

    for line in layout.lines() {
        writer.line(object, line);
    }
}

impl ResourceManager {
    // TextLayout に出てくるグリフを、書き込まずに作ってキャッシュに入れる（§13 グリフを作る負担を分ける）
    // max_new_glyphs_per_frame の上限は受けない。Obfuscated で選ばれる字は種で決まるため、元の字だけを作る
    pub fn prepare_glyphs(&mut self, layout: &TextLayout, method: GlyphMethod, pixel_scale: f32) {
        assert!(pixel_scale.is_finite() && pixel_scale > 0.0, "pixel_scale は 0 より大きい有限の値にしてください（{pixel_scale}）");

        match method {
            GlyphMethod::Vector => {}
        }

        for glyph in layout.glyphs().iter().filter(|glyph| !glyph.style.hide_box) {
            let (body, outline) = shape_keys(layout, glyph, glyph.glyph_id, pixel_scale);

            self.ensure_vector_shape(body, false);
            if let Some(outline) = outline {
                self.ensure_vector_shape(outline, false);
            }
        }
    }
}

// 字の形と縁取りの形のキー
fn shape_keys(layout: &TextLayout, glyph: &PlacedGlyph, glyph_id: u32, pixel_scale: f32) -> (ShapeKey, Option<ShapeKey>) {
    let stage = size_stage(glyph.size * pixel_scale);
    let bold = if glyph.style.synthetic_bold { layout.bold_weight() } else { 0.0 };
    let body = ShapeKey::new(glyph.font, glyph_id, stage, bold, 0.0);
    let outline = glyph.style.outline.map(|_| ShapeKey::new(glyph.font, glyph_id, stage, bold, layout.outline_width()));

    (body, outline)
}

struct Writer<'a> {
    write: &'a TextWrite,
    layout: &'a TextLayout,
}

// フォントの座標（em、y 上向き）の形を TextLayout の座標に置く量
#[derive(Copy, Clone)]
struct Placement {
    origin_x: f32,
    origin_y: f32,
    size: f32,
    italic: f32,
}

impl Writer<'_> {
    fn glyph<T: TextObject>(&self, object: &mut T, resources: &mut ResourceManager, index: usize, glyph: &PlacedGlyph) {
        let style = glyph.style;
        let alpha = 1.0 - style.ghost;
        let color = with_alpha(style.color.unwrap_or(self.write.color), alpha);

        if style.hide_box {
            self.hide_box(object, glyph, &self.layout.rows()[glyph.row], color);
            return;
        }

        let glyph_id = if style.obfuscated { obfuscated_glyph(resources, glyph, self.write.obfuscation_seed, index) } else { glyph.glyph_id };
        let (body_key, outline_key) = shape_keys(self.layout, glyph, glyph_id, self.write.pixel_scale);

        // 上限で作れない形があれば、このフレームはそのグリフを書かない（§13 グリフを作る負担を分ける）
        if !resources.ensure_vector_shape(body_key, true) {
            return;
        }
        if let Some(key) = outline_key
            && !resources.ensure_vector_shape(key, true)
        {
            return;
        }

        let body = resources.vector_shape(&body_key);
        let outline = outline_key.map(|key| resources.vector_shape(&key));
        let placement = Placement { origin_x: glyph.position.x, origin_y: glyph.position.y, size: glyph.size, italic: style.synthetic_italic };

        // 影 → 縁取り → 字 の順（§13 書式の作り方）
        if let Some(shadow) = style.shadow {
            let [dx, dy] = shadow.offset();
            let shadowed = Placement { origin_x: placement.origin_x + dx * glyph.size, origin_y: placement.origin_y + dy * glyph.size, ..placement };
            // 指定がなければ字の色の rgb を 0.25 倍（§13 書き込み：write_text の細部）
            let shadow_color = match shadow.color {
                Some(shadow_color) => with_alpha(shadow_color, alpha),
                None => Color::new(color.r * 0.25, color.g * 0.25, color.b * 0.25, color.a),
            };

            if let Some(outline) = outline {
                self.shape(object, outline, shadowed, shadow_color);
            }
            self.shape(object, body, shadowed, shadow_color);
        }

        if let (Some(outline), Some(outline_color)) = (outline, style.outline) {
            self.shape(object, outline, placement, with_alpha(outline_color, alpha));
        }

        self.shape(object, body, placement, color);
    }

    // キャッシュの形を 1 つの形として書く。YAxis::Down では y の反転で向きが逆になるため、インデックスの順を入れ替える
    fn shape<T: TextObject>(&self, object: &mut T, shape: &GlyphShape, placement: Placement, color: Color) {
        if shape.indices.is_empty() {
            return;
        }

        object.new_shape();
        let base = object.written_vertex_count() as u32;

        for &[x, y] in &shape.vertices {
            let layout_x = placement.origin_x + (x + placement.italic * y) * placement.size;
            let layout_y = placement.origin_y - y * placement.size;
            object.push_vertex(self.vertex(layout_x, layout_y, color));
        }

        let flip = self.write.y_axis == YAxis::Down;
        for triangle in shape.indices.chunks_exact(3) {
            let order = if flip { [triangle[0], triangle[2], triangle[1]] } else { [triangle[0], triangle[1], triangle[2]] };
            for index in order {
                object.push_index(base + index);
            }
        }
    }

    // 送り幅 × 行の高さの四角形（§13 書き込み：write_text の細部）
    fn hide_box<T: TextObject>(&self, object: &mut T, glyph: &PlacedGlyph, row: &TextRow, color: Color) {
        let (left, right) = (glyph.position.x, glyph.position.x + glyph.advance);
        let (top, bottom) = (row.top, row.top + row.height);
        let vertices = [(left, top), (right, top), (right, bottom), (left, bottom)].map(|(x, y)| self.vertex(x, y, color));

        write_counter_clockwise(object, &vertices, &[0, 1, 2, 0, 2, 3]);
    }

    // 下線・打消し線・上線。lines 本を太さの 2 倍の間隔で並べる（§13 書き込み：write_text の細部）
    fn line<T: TextObject>(&self, object: &mut T, line: &PlacedLine) {
        let color = with_alpha(line.color.unwrap_or(self.write.color), 1.0 - line.ghost);
        let count = line.decoration.lines;

        for number in 0..count {
            let y = line.start.y + (number as f32 - (count as f32 - 1.0) / 2.0) * line.thickness * 2.0;
            let ends = [self.vertex(line.start.x, y, color), self.vertex(line.start.x + line.length, y, color)];
            let mesh = stroke::tessellate(&ends, &[], &[], line.thickness, line.decoration.joint_type, true, None);

            write_counter_clockwise(object, &mesh.vertices, &mesh.indices);
        }
    }

    // TextLayout の座標から、書き込む座標の頂点を作る
    fn vertex(&self, layout_x: f32, layout_y: f32, color: Color) -> Vertex {
        let position = self.write.position;
        let y = match self.write.y_axis {
            YAxis::Down => position.y + layout_y,
            YAxis::Up => position.y - layout_y,
        };

        Vertex::new_position_color(position.x + layout_x, y, position.z, color.r, color.g, color.b, color.a)
    }
}

// 書き込む座標で、三角形ごとに反時計回りに揃えて書く
fn write_counter_clockwise<T: TextObject>(object: &mut T, vertices: &[Vertex], indices: &[u32]) {
    if indices.is_empty() {
        return;
    }

    object.new_shape();
    let base = object.written_vertex_count() as u32;

    for &vertex in vertices {
        object.push_vertex(vertex);
    }

    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| {
            let vertex = vertices[index as usize];
            [vertex.x, vertex.y]
        });
        let order = if signed_area(a, b, c) < 0.0 { [triangle[0], triangle[2], triangle[1]] } else { [triangle[0], triangle[1], triangle[2]] };

        for index in order {
            object.push_index(base + index);
        }
    }
}

fn with_alpha(color: Color, scale: f32) -> Color {
    Color { a: color.a * scale, ..color }
}

// 候補の字から、(種, グリフの番号) の hash で選ぶ。候補がなければ元の字
fn obfuscated_glyph(resources: &ResourceManager, glyph: &PlacedGlyph, seed: u64, index: usize) -> u32 {
    let candidates = &resources.font(glyph.font).obfuscation_glyphs;

    if candidates.is_empty() {
        return glyph.glyph_id;
    }

    candidates[(split_mix(seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)) % candidates.len() as u64) as usize]
}

fn split_mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
