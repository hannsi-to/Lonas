use std::f32::consts::TAU;

use log::error;
use crate::lonas::PaintType;
use crate::object::Object2D;
use crate::vertex::Vertex;

pub mod point;
pub mod triangle;
pub mod rect;
pub mod rect_rounded;
pub mod polygon_rounded;
pub mod lines;
pub mod circle;

// 角の形（polygon_rounded・rect_rounded）。長さは頂点と同じ単位
// radius_x は前の角へ向かう辺に沿った長さ、radius_y は次の角へ向かう辺に沿った長さ
// （build_rect_rounded_* では、x が横・y が縦になるように入れ替えて渡す）
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum CornerType {
    // 尖ったまま
    None{},
    // 半径 radius の円弧で丸める（両方の辺に接する）
    Circle{
        radius: f32,
    },
    // 楕円弧で丸める（両方の辺に接する）
    Ellipse{
        radius_x: f32,
        radius_y: f32,
    },
    // 両方の辺で radius の位置を結んで、まっすぐ切り落とす
    Bevel {
        radius: f32,
    },
    // それぞれの辺で radius_x・radius_y の位置を結んで、まっすぐ切り落とす
    Chamfer {
        radius_x: f32,
        radius_y: f32,
    },
    // 内側へ四角く切り欠く
    Inset {
        radius_x: f32,
        radius_y: f32,
    },
    // 角を中心にした弧で、内側へえぐる
    Concave {
        radius_x: f32,
        radius_y: f32,
    }
}

impl CornerType {
    // radius_x と radius_y を入れ替える（radius が 1 つのものはそのまま）
    pub(crate) fn swapped_xy(self) -> Self {
        match self {
            CornerType::Ellipse { radius_x, radius_y } => CornerType::Ellipse { radius_x: radius_y, radius_y: radius_x },
            CornerType::Chamfer { radius_x, radius_y } => CornerType::Chamfer { radius_x: radius_y, radius_y: radius_x },
            CornerType::Inset { radius_x, radius_y } => CornerType::Inset { radius_x: radius_y, radius_y: radius_x },
            CornerType::Concave { radius_x, radius_y } => CornerType::Concave { radius_x: radius_y, radius_y: radius_x },
            other => other,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Corner {
    pub corner_type: CornerType,
    pub vertex: Vertex,
}

impl Corner {
    pub fn new(corner_type: CornerType, vertex: Vertex) -> Self {
        Self { corner_type, vertex }
    }
}

pub(crate) fn check_object_type(object2d: &Object2D) -> Result<PaintType, ()> {
    object2d.get_paint_type().ok_or_else(|| {
        error!(
            "引数に渡されたオブジェクトの PaintType が設定されていません。処理を飛ばします。"
        );
    })
}

// ---- 補助で共通に使うもの ----

// 円周を折れ線で近づけたときの、円からのずれ（頂点と同じ単位）の上限
const ARC_TOLERANCE: f32 = 0.25;
const MAX_CIRCLE_SEGMENTS: usize = 128;

// 半径 radius・中心角 sweep（ラジアン、正の値）の弧の分割数。1 つの辺と弧のずれが ARC_TOLERANCE 以下になる数
// 中心角 θ の辺と円のずれは radius × (1 - cos(θ / 2)) のため、θ ≤ 2 × acos(1 - ARC_TOLERANCE / radius)
pub(crate) fn arc_segments(radius: f32, sweep: f32) -> usize {
    if radius <= ARC_TOLERANCE {
        return 1;
    }

    let step = 2.0 * (1.0 - ARC_TOLERANCE / radius).acos();
    let full = (TAU / step).ceil().min(MAX_CIRCLE_SEGMENTS as f32);

    ((full * sweep.abs() / TAU).ceil() as usize).max(1)
}

// 外周の頂点を書く。三角形の分け方は Object の paint_type に任せる（Fill は自動の三角形化、Stroke は線）
// begin_shape() は呼ばない（形の区切りは呼び出す側で行う）。Stroke の閉じ方は Object の strip のとおり
pub(crate) fn write_outline(object2d: &mut Object2D, outline: &[Vertex]) {
    for &vertex in outline {
        object2d.vertex(vertex);
    }
}

// 頂点の要素（位置・色・UV・法線）を、重み付きで足し合わせる。重みの合計は 1 にすること
pub(crate) fn combine(points: &[Vertex], weights: &[f32]) -> Vertex {
    let mut result = Vertex::default();

    for (point, &w) in points.iter().zip(weights) {
        result.x += point.x * w;
        result.y += point.y * w;
        result.z += point.z * w;
        result.r += point.r * w;
        result.g += point.g * w;
        result.b += point.b * w;
        result.a += point.a * w;
        result.u += point.u * w;
        result.v += point.v * w;
        result.n_x += point.n_x * w;
        result.n_y += point.n_y * w;
        result.n_z += point.n_z * w;
    }

    result
}

pub(crate) fn lerp_vertex(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    combine(&[*a, *b], &[1.0 - t, t])
}
