// 円・楕円・扇形・弧を Object2D に 1 つの形として書く補助
// begin〜end の間で呼ぶ。色以外の要素（UV・法線など）は center_vertex のものを使う

use std::f32::consts::TAU;

use crate::lonas::{Color, Object2D, PaintType};
use crate::util_2d::{check_object_type, write_outline};
use crate::vertex::Vertex;

// 円の形。長さは頂点と同じ単位、角度はラジアン
// 角度は xy 平面で x 軸から測る（y が下向きの画面では、正の向きは時計回りに見える）
// start_angle から end_angle まで描く。差が 2π 以上なら一周（円・楕円）、それより小さければ扇形・弧
pub struct CircleData {
    radius_x: f32,
    radius_y: f32,
    start_angle: f32,
    end_angle: f32,
    // start_angle から end_angle までを分ける数
    segments: u32,
}

impl CircleData {
    pub fn new(radius: f32) -> Self{
        Self::new_xy(radius, radius)
    }

    pub fn new_xy(radius_x: f32, radius_y: f32) -> Self {
        Self {
            radius_x,
            radius_y,
            start_angle: 0.0,
            end_angle: 360.0f32.to_radians(),
            segments: 16,
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius_x = radius;
        self.radius_y = radius;
        self
    }

    pub fn radius_x(mut self, radius_x: f32) -> Self {
        self.radius_x = radius_x;
        self
    }

    pub fn radius_y(mut self, radius_y: f32) -> Self {
        self.radius_y = radius_y;
        self
    }

    pub fn start_angle(mut self, start_angle: f32) -> Self {
        self.start_angle = start_angle;
        self
    }

    pub fn end_angle(mut self, end_angle: f32) -> Self {
        self.end_angle = end_angle;
        self
    }

    pub fn segments(mut self, segments: u32) -> Self {
        self.segments = segments;
        self
    }

    fn is_full(&self) -> bool {
        (self.end_angle - self.start_angle).abs() >= TAU - 1e-4
    }

    fn validate(&self) {
        assert!(self.radius_x.is_finite() && self.radius_x > 0.0, "radius_x は 0 より大きい有限の値にしてください。({})", self.radius_x);
        assert!(self.radius_y.is_finite() && self.radius_y > 0.0, "radius_y は 0 より大きい有限の値にしてください。({})", self.radius_y);
        assert!(self.start_angle.is_finite() && self.end_angle.is_finite(), "角度は有限の値にしてください。");
        assert!(self.start_angle != self.end_angle, "start_angle と end_angle が同じです。({})", self.start_angle);
        let min_segments = if self.is_full() { 3 } else { 1 };
        assert!(self.segments >= min_segments, "segments は {min_segments} 以上にしてください。({})", self.segments);
    }
}

// center_vertex を中心に、center_vertex の色で円を書く
pub fn build_circle(
    object2d: &mut Object2D,
    circle_data: CircleData,
    center_vertex: Vertex,
) {
    build_circle_other_colors(object2d, circle_data, &[], center_vertex);
}

// center_vertex を中心に円を書く。中心は center_vertex の色、円周は other_colors のグラデーション
// other_colors は円周（弧）に沿って等間隔に並べ、その間を補間する。一周の場合は最後の色から最初の色へつながる。空なら円周も center_vertex の色
// Fill は中心から扇形に塗り（index() で指定）、Stroke は円周（弧）だけを線にする
// Stroke の閉じ方は Object の strip のとおり。閉じた輪郭（strip: false）では一周の円は閉じ、弧は弦で閉じる。
// 開いた折れ線（strip: true）では、一周の円も最後の頂点から最初の頂点へはつながらない
// begin_shape() は呼ばない（形の区切りは呼び出す側で行う）
pub fn build_circle_other_colors (
    object2d: &mut Object2D,
    circle_data: CircleData,
    other_colors: &[Color],
    center_vertex: Vertex,
) {
    circle_data.validate();

    let Ok(paint_type) = check_object_type(object2d) else {
        return;
    };

    let full = circle_data.is_full();
    let sweep = if full { TAU.copysign(circle_data.end_angle - circle_data.start_angle) } else { circle_data.end_angle - circle_data.start_angle };
    let segments = circle_data.segments as usize;
    // 一周なら最後の頂点は最初の頂点と同じなので書かない
    let ring_count = if full { segments } else { segments + 1 };
    let ring: Vec<Vertex> = (0..ring_count)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let (sin, cos) = (circle_data.start_angle + sweep * t).sin_cos();
            let color = ring_color(other_colors, t, full).unwrap_or(Color::new(center_vertex.r, center_vertex.g, center_vertex.b, center_vertex.a));
            let vertex = center_vertex.translate(circle_data.radius_x * cos, circle_data.radius_y * sin, 0.0);

            Vertex { r: color.r, g: color.g, b: color.b, a: color.a, ..vertex }
        })
        .collect();

    match paint_type {
        PaintType::Fill => {
            let center = object2d.get_written_vertex_count() as u32;
            object2d.vertex(center_vertex);
            for &vertex in &ring {
                object2d.vertex(vertex);
            }

            // 中心からの扇形。xy 平面で反時計回りにそろえる（sweep が負なら入れ替える）
            for i in 0..segments as u32 {
                let a = center + 1 + i;
                let b = center + 1 + (i + 1) % ring_count as u32;
                let [a, b] = if sweep > 0.0 { [a, b] } else { [b, a] };

                object2d.index(center);
                object2d.index(a);
                object2d.index(b);
            }
        }
        PaintType::Stroke { .. } => write_outline(object2d, &ring),
    }
}

// 円周の t（0〜1、弧の始まりから終わり）の位置の色。colors が空なら None
fn ring_color(colors: &[Color], t: f32, full: bool) -> Option<Color> {
    let count = colors.len();

    if count <= 1 {
        return colors.first().copied();
    }

    // 一周なら色は count 個の区間（最後の色から最初の色へつながる）、弧なら count - 1 個の区間
    let position = t * if full { count as f32 } else { (count - 1) as f32 };
    let index = (position.floor() as usize).min(if full { count - 1 } else { count - 2 });
    let frac = position - index as f32;
    let (a, b) = (colors[index], colors[(index + 1) % count]);
    let mix = |x: f32, y: f32| x + (y - x) * frac;

    Some(Color::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a)))
}
