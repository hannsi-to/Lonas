// Object* への書き込み（§13 整形と書き込みの分け方・書き込み：write_text の細部）
// begin〜end の間で毎フレーム呼ぶ。キャッシュした形を写すだけで、キャッシュにない形だけを三角形化する

use crate::color::Color;
use crate::config::Blend;
use crate::math::Vector3f;
use crate::object::{Object2D, Object3D, stroke};
use crate::resource::ResourceManager;
use crate::text::bitmap::{BitmapKey, Hinting};
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

// 描き方（§13 描き方の選択）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GlyphMethod {
    // 輪郭を三角形化する
    Vector,
    // 画面上の大きさでラスタライズした画素を、グリフのアトラスから読む。VertexLayout::PositionColorUvNormal・Blend::Alpha で描く
    Bitmap { hinting: Hinting },
    // 1 チャンネルの符号付き距離（msdf feature）。Bitmap と同じく PositionColorUvNormal・Blend::Alpha で描く
    Sdf,
    // MTSDF（msdf feature）。Bitmap と同じく PositionColorUvNormal・Blend::Alpha で描く
    Msdf,
}

impl GlyphMethod {
    // Sdf / Msdf を msdf feature なしで使うと panic（§13 描き方の選択）
    pub(crate) fn check_feature(self) {
        if matches!(self, GlyphMethod::Sdf | GlyphMethod::Msdf) && !cfg!(feature = "msdf") {
            panic!("GlyphMethod::{self:?} を使うには、lonas の msdf feature を有効にしてください");
        }
    }
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

    write.method.check_feature();

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
        method.check_feature();

        for glyph in layout.glyphs().iter().filter(|glyph| !glyph.style.hide_box) {
            match method {
                GlyphMethod::Vector => {
                    let (body, outline) = shape_keys(layout, glyph, glyph.glyph_id, pixel_scale);

                    self.ensure_vector_shape(body, false);
                    if let Some(outline) = outline {
                        self.ensure_vector_shape(outline, false);
                    }
                }
                GlyphMethod::Bitmap { hinting } => {
                    let (body, outline) = bitmap_keys(layout, glyph, glyph.glyph_id, pixel_scale, hinting);

                    self.ensure_bitmap(body, false);
                    if let Some(outline) = outline {
                        self.ensure_bitmap(outline, false);
                    }
                }
                // 太字・縁取りはシェーダーで作るため、字ごとに 1 つ
                GlyphMethod::Sdf | GlyphMethod::Msdf => {
                    self.ensure_bitmap(BitmapKey::distance(glyph.font, glyph.glyph_id, method == GlyphMethod::Msdf), false);
                }
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

// Bitmap：字の画素と縁取りの画素のキー。画素の大きさは size × pixel_scale を整数に丸めた値
fn bitmap_keys(layout: &TextLayout, glyph: &PlacedGlyph, glyph_id: u32, pixel_scale: f32, hinting: Hinting) -> (BitmapKey, Option<BitmapKey>) {
    let pixel_size = (glyph.size * pixel_scale).round().max(1.0) as u32;
    let bold = if glyph.style.synthetic_bold { layout.bold_weight() } else { 0.0 };
    let body = BitmapKey::new(glyph.font, glyph_id, pixel_size, hinting, bold, 0.0);
    let outline = glyph.style.outline.map(|_| BitmapKey::new(glyph.font, glyph_id, pixel_size, hinting, bold, layout.outline_width()));

    (body, outline)
}

// アトラスの中のグリフの四角形（画素、原点からの位置。y は下向き）
// scale は TextLayout の 1 単位あたりの画素（Bitmap は pixel_scale、Sdf / Msdf は sdf_em_size / size）
// snap なら原点を画素の格子にそろえる（Bitmap だけ）。thickening は normal.y に入れる、境目を外にずらす量（em）
#[derive(Copy, Clone)]
struct GlyphQuad {
    slot: u32,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    scale: f32,
    snap: bool,
    thickening: f32,
}

impl GlyphQuad {
    fn of(resources: &ResourceManager, key: &BitmapKey) -> Self {
        let glyph = resources.bitmap_glyph(key);
        let raster = &glyph.raster;

        Self { slot: glyph.slot, left: raster.left, top: raster.top, width: raster.width, height: raster.height, scale: 1.0, snap: false, thickening: 0.0 }
    }

    fn bitmap(resources: &ResourceManager, key: &BitmapKey, pixel_scale: f32) -> Self {
        Self { scale: pixel_scale, snap: true, ..Self::of(resources, key) }
    }

    fn distance(resources: &ResourceManager, key: &BitmapKey, glyph: &PlacedGlyph, thickening: f32) -> Self {
        let em_size = resources.text_config().sdf_em_size as f32;

        Self { scale: em_size / glyph.size, thickening, ..Self::of(resources, key) }
    }
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

        match self.write.method {
            GlyphMethod::Vector => {}
            GlyphMethod::Bitmap { hinting } => {
                self.bitmap_glyph(object, resources, glyph, glyph_id, hinting, color);
                return;
            }
            GlyphMethod::Sdf | GlyphMethod::Msdf => {
                self.distance_glyph(object, resources, glyph, glyph_id, color);
                return;
            }
        }

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

    // Bitmap：影 → 縁取り → 字 の順に、アトラスの画素を指す四角形を書く（§13 Bitmap の書き込み）
    fn bitmap_glyph<T: TextObject>(&self, object: &mut T, resources: &mut ResourceManager, glyph: &PlacedGlyph, glyph_id: u32, hinting: Hinting, color: Color) {
        let (body_key, outline_key) = bitmap_keys(self.layout, glyph, glyph_id, self.write.pixel_scale, hinting);

        // 上限で作れない画素があれば、このフレームはそのグリフを書かない
        if !resources.ensure_bitmap(body_key, true) {
            return;
        }
        if let Some(key) = outline_key
            && !resources.ensure_bitmap(key, true)
        {
            return;
        }

        let scale = self.write.pixel_scale;
        let body = GlyphQuad::bitmap(resources, &body_key, scale);
        let outline = outline_key.map(|key| GlyphQuad::bitmap(resources, &key, scale));

        self.glyph_quads(object, glyph, body, outline, color);
    }

    // Sdf / Msdf：字ごとに 1 つの距離の場から、太字・縁取りを normal.y で作る（§13 グリフの作り方）
    fn distance_glyph<T: TextObject>(&self, object: &mut T, resources: &mut ResourceManager, glyph: &PlacedGlyph, glyph_id: u32, color: Color) {
        let key = BitmapKey::distance(glyph.font, glyph_id, self.write.method == GlyphMethod::Msdf);

        if !resources.ensure_bitmap(key, true) {
            return;
        }

        let style = glyph.style;
        let bold = if style.synthetic_bold { self.layout.bold_weight() } else { 0.0 };
        let body = GlyphQuad::distance(resources, &key, glyph, resources.clamp_thickening(bold));
        // 縁取りは同じ四角形を、太字と縁取りの分だけ太らせて字の後ろに書く
        let outline =
            style.outline.map(|_| GlyphQuad::distance(resources, &key, glyph, resources.clamp_thickening(bold + self.layout.outline_width())));

        self.glyph_quads(object, glyph, body, outline, color);
    }

    // 影 → 縁取り → 字 の順に四角形を書く（Bitmap / Sdf / Msdf で共通）
    fn glyph_quads<T: TextObject>(&self, object: &mut T, glyph: &PlacedGlyph, body: GlyphQuad, outline: Option<GlyphQuad>, color: Color) {
        let style = glyph.style;
        let (x, y, italic) = (glyph.position.x, glyph.position.y, style.synthetic_italic);

        if let Some(shadow) = style.shadow {
            let [dx, dy] = shadow.offset();
            let (shadow_x, shadow_y) = (x + dx * glyph.size, y + dy * glyph.size);
            let shadow_color = match shadow.color {
                Some(shadow_color) => with_alpha(shadow_color, 1.0 - style.ghost),
                None => Color::new(color.r * 0.25, color.g * 0.25, color.b * 0.25, color.a),
            };

            if let Some(outline) = outline {
                self.quad(object, outline, shadow_x, shadow_y, italic, shadow_color);
            }
            self.quad(object, body, shadow_x, shadow_y, italic, shadow_color);
        }

        if let (Some(outline), Some(outline_color)) = (outline, style.outline) {
            self.quad(object, outline, x, y, italic, with_alpha(outline_color, 1.0 - style.ghost));
        }

        self.quad(object, body, x, y, italic, color);
    }

    // グリフの原点（TextLayout の座標）から、画素の位置だけずらした四角形を書く。Bitmap は原点を 1 / pixel_scale の格子に丸める
    // uv は左上が (0, 0)。normal.x はグリフの番号、normal.y は境目を外にずらす量（em）
    fn quad<T: TextObject>(&self, object: &mut T, quad: GlyphQuad, origin_x: f32, origin_y: f32, italic: f32, color: Color) {
        if quad.slot == 0 {
            return;
        }

        let scale = quad.scale;
        let origin = self.vertex(origin_x, origin_y, color);
        let (snapped_x, snapped_y) = if quad.snap {
            let pixel = self.write.pixel_scale;
            ((origin.x * pixel).round() / pixel, (origin.y * pixel).round() / pixel)
        } else {
            (origin.x, origin.y)
        };
        let (left, top) = (quad.left as f32, quad.top as f32);
        let (right, bottom) = (left + quad.width as f32, top + quad.height as f32);
        let corners = [(left, top, 0.0, 0.0), (right, top, 1.0, 0.0), (right, bottom, 1.0, 1.0), (left, bottom, 0.0, 1.0)];
        let vertices = corners.map(|(x, y, u, v)| {
            // y は画素の下向き。ベースラインからの高さは -y
            let layout_x = (x - italic * y) / scale;
            let layout_y = y / scale;
            let vertex_y = match self.write.y_axis {
                YAxis::Down => snapped_y + layout_y,
                YAxis::Up => snapped_y - layout_y,
            };

            Vertex { x: snapped_x + layout_x, y: vertex_y, u, v, n_x: quad.slot as f32, n_y: quad.thickening, ..origin }
        });

        write_counter_clockwise(object, &vertices, &[0, 1, 2, 0, 2, 3]);
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
