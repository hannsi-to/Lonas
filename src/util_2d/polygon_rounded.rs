// 角を丸めた（切り落とした・えぐった）多角形を Object2D に 1 つの形として書く補助
// begin〜end の間で呼ぶ。角の形は Corner の corner_type、角に作る頂点の色などは Corner の vertex のものを使う
// 角の形は xy 平面で計算し、z は角の頂点の値を使う

use std::f32::consts::PI;

use crate::lonas::{Object2D, Vector2f, Vertex};
use crate::util_2d::{arc_segments, check_object_type, write_outline, Corner, CornerType};

// 多角形の角を、順に（時計回りでも反時計回りでもよい）並べて渡す。3 個以上
// 隣り合う角の丸め（辺に沿った長さ）の合計が辺の長さを超える場合は、すべての角を同じ割合で小さくする（CSS の border-radius と同じ）
// Fill は自動の三角形化で塗り（Inset・Concave でへこんでもよい）、Stroke は輪郭を線にする（閉じ方は Object の strip のとおり）
// begin_shape() は呼ばない（形の区切りは呼び出す側で行う）
pub fn build_polygon_rounded(
    object2d: &mut Object2D,
    corners: &[Corner],
) {
    assert!(corners.len() >= 3, "角は 3 個以上にしてください。({})", corners.len());
    for corner in corners {
        validate(corner.corner_type);
    }

    // PaintType が設定されていなければ書かない
    if check_object_type(object2d).is_err() {
        return;
    }

    write_outline(object2d, &rounded_outline(corners));
}

// 角の形の値の検査。長さは 0 以上の有限の値
fn validate(corner_type: CornerType) {
    let lengths: &[f32] = match &corner_type {
        CornerType::None {} => &[],
        CornerType::Circle { radius } | CornerType::Bevel { radius } => std::slice::from_ref(radius),
        CornerType::Ellipse { radius_x, radius_y }
        | CornerType::Chamfer { radius_x, radius_y }
        | CornerType::Inset { radius_x, radius_y }
        | CornerType::Concave { radius_x, radius_y } => &[*radius_x, *radius_y],
    };

    for &length in lengths {
        assert!(length.is_finite() && length >= 0.0, "角の長さは 0 以上の有限の値にしてください。({corner_type:?})");
    }
}

// 1 つの角の向き。前の角・次の角への単位ベクトルと、2 つの辺の間の角度（0〜π）
struct CornerFrame {
    center: Vector2f,
    to_prev: Vector2f,
    to_next: Vector2f,
    angle: f32,
}

pub(crate) fn rounded_outline(corners: &[Corner]) -> Vec<Vertex> {
    let count = corners.len();
    let position = |i: usize| Vector2f::new(corners[i].vertex.x, corners[i].vertex.y);
    let frames: Vec<Option<CornerFrame>> = (0..count)
        .map(|i| {
            let center = position(i);
            let to_prev = (position((i + count - 1) % count) - center).try_normalize()?;
            let to_next = (position((i + 1) % count) - center).try_normalize()?;
            let angle = to_prev.dot(to_next).clamp(-1.0, 1.0).acos();

            // 辺がまっすぐ続く・折り返す角は丸めない
            (angle > 1e-4 && angle < PI - 1e-4).then_some(CornerFrame { center, to_prev, to_next, angle })
        })
        .collect();

    // 角ごとの、前の辺・次の辺に沿った長さ
    let lengths: Vec<(f32, f32)> = (0..count)
        .map(|i| match &frames[i] {
            Some(frame) => edge_lengths(corners[i].corner_type, frame.angle),
            None => (0.0, 0.0),
        })
        .collect();

    // 1 本の辺の両端の長さの合計が辺の長さを超えるなら、すべての角を同じ割合で小さくする
    let scale = (0..count)
        .map(|i| {
            let next = (i + 1) % count;
            let used = lengths[i].1 + lengths[next].0;
            let edge = (position(next) - position(i)).length();
            if used > edge { edge / used } else { 1.0 }
        })
        .fold(1.0f32, f32::min);

    let mut outline = Vec::new();

    for (i, corner) in corners.iter().enumerate() {
        match &frames[i] {
            Some(frame) => corner_points(&mut outline, corner, frame, scale),
            None => outline.push(corner.vertex),
        }
    }

    outline
}

// 前の辺・次の辺に沿って、角から何だけ離れた位置から形が始まるか
fn edge_lengths(corner_type: CornerType, angle: f32) -> (f32, f32) {
    match corner_type {
        CornerType::None {} => (0.0, 0.0),
        // 半径 r の円が両方の辺に接する位置は、角から r / tan(角度 / 2)
        CornerType::Circle { radius } => {
            let length = radius / (angle / 2.0).tan();
            (length, length)
        }
        CornerType::Bevel { radius } => (radius, radius),
        CornerType::Ellipse { radius_x, radius_y }
        | CornerType::Chamfer { radius_x, radius_y }
        | CornerType::Inset { radius_x, radius_y }
        | CornerType::Concave { radius_x, radius_y } => (radius_x, radius_y),
    }
}

// 角の形の頂点を、前の辺の側から次の辺の側へ順に足す
fn corner_points(outline: &mut Vec<Vertex>, corner: &Corner, frame: &CornerFrame, scale: f32) {
    let (prev_length, next_length) = edge_lengths(corner.corner_type, frame.angle);
    let (a, b) = (prev_length * scale, next_length * scale);
    let at = |p: Vector2f| corner.vertex.set_position(p.x, p.y, corner.vertex.z);
    let c = frame.center;
    let (u, v) = (frame.to_prev, frame.to_next);
    let start = c + u * a;
    let end = c + v * b;

    if a <= 0.0 && b <= 0.0 {
        outline.push(corner.vertex);
        return;
    }

    match corner.corner_type {
        CornerType::None {} => outline.push(corner.vertex),
        CornerType::Bevel { .. } | CornerType::Chamfer { .. } => outline.extend([at(start), at(end)]),
        CornerType::Inset { .. } => outline.extend([at(start), at(c + u * a + v * b), at(end)]),
        CornerType::Circle { radius } => {
            // 円の中心は、2 つの辺の間の二等分線の上、角から r / sin(角度 / 2)
            let radius = radius * scale;
            let bisector = (u + v).normalize();
            let center = c + bisector * (radius / (frame.angle / 2.0).sin());
            let from = (start - center).y.atan2((start - center).x);
            let to = (end - center).y.atan2((end - center).x);
            // 弧の中心角は π - 角度。短い側を回る
            let mut sweep = to - from;
            if sweep > PI {
                sweep -= 2.0 * PI;
            } else if sweep < -PI {
                sweep += 2.0 * PI;
            }
            let steps = arc_segments(radius, sweep);

            for step in 0..=steps {
                let (sin, cos) = (from + sweep * step as f32 / steps as f32).sin_cos();
                outline.push(at(center + Vector2f::new(cos, sin) * radius));
            }
        }
        CornerType::Ellipse { .. } => {
            // 2 つの辺を軸にした楕円の 4 分の 1。start と end で辺に接する
            // t = π/2 で origin - v·b = start、t = 0 で origin - u·a = end
            let origin = c + u * a + v * b;
            let steps = arc_segments(a.max(b), PI / 2.0);

            for step in 0..=steps {
                let t = PI / 2.0 * (1.0 - step as f32 / steps as f32);
                outline.push(at(origin - u * (a * t.cos()) - v * (b * t.sin())));
            }
        }
        CornerType::Concave { .. } => {
            let steps = arc_segments(a.max(b), frame.angle);

            if (a - b).abs() <= 1e-4 {
                // 同じ長さなら、角を中心にした円弧で u から v まで回す
                let turn = if u.x * v.y - u.y * v.x >= 0.0 { frame.angle } else { -frame.angle };
                for step in 0..=steps {
                    let (sin, cos) = (turn * step as f32 / steps as f32).sin_cos();
                    let direction = Vector2f::new(u.x * cos - u.y * sin, u.x * sin + u.y * cos);
                    outline.push(at(c + direction * a));
                }
            } else {
                // 長さが違えば、2 つの辺を軸にした楕円の 4 分の 1
                for step in 0..=steps {
                    let t = PI / 2.0 * step as f32 / steps as f32;
                    outline.push(at(c + u * (a * t.cos()) + v * (b * t.sin())));
                }
            }
        }
    }
}
