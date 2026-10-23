// 線の三角形化：頂点を線の中心として、太さのある線の三角形を作る（§10 線：Stroke）
// 頂点の xy 平面で広げ、z は頂点ごとの値を保つ。色・UV・法線は元の頂点の値を写す（破線の切れ目では線形に補間する）
// 三角形は xy 平面で反時計回りに揃える。角の内側では辺の四角形どうしが重なる

use std::f32::consts::PI;

use super::topology::{self, Dash, JointType};
use crate::math::Vector2f;
use crate::vertex::Vertex;

// Miter の尖りの長さ（角の点から尖りの先まで）が線の太さの半分のこの倍を超えたら Bevel にする（SVG の stroke-miterlimit の既定値と同じ）
const MITER_LIMIT: f32 = 4.0;
// 丸い角・端の扇形で、1 つの三角形が受け持つ角度の上限
const ROUND_STEP: f32 = PI / 16.0;
// これより短い辺は長さ 0 とみなして捨てる
const MIN_LENGTH: f32 = 1e-6;

#[derive(Debug, Default)]
pub(crate) struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

// holes は穴の最初の頂点の位置（§10 穴のある多角形）。外周と穴の輪郭を、それぞれ別の線として描く
pub(crate) fn tessellate(points: &[Vertex], holes: &[usize], line_width: f32, joint: JointType, strip: bool, dash: Option<Dash>) -> Mesh {
    let mut builder = Builder { mesh: Mesh::default(), half: line_width * 0.5, joint };

    for ring in topology::rings(points.len(), holes) {
        builder.ring(&points[ring], strip, dash);
    }

    builder.mesh
}

struct Builder {
    mesh: Mesh,
    half: f32,
    joint: JointType,
}

impl Builder {
    // 1 つの輪（外周か穴）を、開いた折れ線か閉じた輪郭として描く
    fn ring(&mut self, points: &[Vertex], strip: bool, dash: Option<Dash>) {
        let points = dedup(points, !strip);
        // 頂点が 2 個の閉じた輪郭は、1 本の線分として描く
        let closed = !strip && points.len() >= 3;

        if points.len() < 2 {
            return;
        }

        match dash {
            None => self.polyline(&points, closed, !closed),
            Some(dash) => {
                for piece in dash_pieces(&points, closed, dash) {
                    self.polyline(&dedup(&piece, false), false, true);
                }
            }
        }
    }

    // caps が true なら、JointType に応じて両端を丸める
    fn polyline(&mut self, points: &[Vertex], closed: bool, caps: bool) {
        let count = points.len();

        if count < 2 {
            return;
        }

        let segments = if closed { count } else { count - 1 };
        let direction = |i: usize| (xy(&points[(i + 1) % count]) - xy(&points[i])).normalize();

        for i in 0..segments {
            let (a, b) = (&points[i], &points[(i + 1) % count]);
            let normal = perpendicular(direction(i)) * self.half;
            let [a_left, a_right, b_left, b_right] = [
                self.vertex(a, xy(a) + normal),
                self.vertex(a, xy(a) - normal),
                self.vertex(b, xy(b) + normal),
                self.vertex(b, xy(b) - normal),
            ];

            self.triangle(a_left, a_right, b_left);
            self.triangle(a_right, b_right, b_left);
        }

        // 開いた折れ線は内側の頂点だけ、閉じた輪郭はすべての頂点でつなぐ
        let joints = if closed { 0..count } else { 1..count - 1 };

        for i in joints {
            let previous = (i + count - 1) % count;
            self.joint(&points[i], direction(previous), direction(i));
        }

        let (round_start, round_end) = self.joint.round_caps();

        if caps && round_start {
            // 左の法線から、進む向きの逆を通って右の法線まで
            let normal = perpendicular(direction(0)) * self.half;
            self.arc(&points[0], normal, PI);
        }

        if caps && round_end {
            // 右の法線から、進む向きを通って左の法線まで
            let normal = perpendicular(direction(count - 2)) * self.half;
            self.arc(&points[count - 1], -normal, PI);
        }
    }

    // d0 の向きで point に来て、d1 の向きへ出ていく角の外側を塞ぐ
    fn joint(&mut self, point: &Vertex, d0: Vector2f, d1: Vector2f) {
        let cross = d0.x * d1.y - d0.y * d1.x;
        let dot = d0.dot(d1);
        let u_turn = cross.abs() <= MIN_LENGTH && dot < 0.0;

        // まっすぐ続くなら、辺の四角形どうしが隙間なくつながる
        if cross.abs() <= MIN_LENGTH && !u_turn {
            return;
        }

        // 左に曲がるなら右側が外側。折り返しでは左側を外側にする
        let side = if cross > 0.0 { -1.0 } else { 1.0 };
        let (n0, n1) = (perpendicular(d0) * (side * self.half), perpendicular(d1) * (side * self.half));
        let center = xy(point);

        match self.joint {
            JointType::None => {}
            JointType::Bevel => self.bevel(point, center, n0, n1),
            JointType::Miter => {
                // 尖りの先は 2 つの法線の間の向きにあり、距離は half / cos(曲がる角の半分)
                let tip = (n0 + n1).try_normalize().and_then(|bisector| {
                    let cos = bisector.dot(n0) / self.half;
                    (cos > 0.0 && 1.0 / cos <= MITER_LIMIT).then(|| center + bisector * (self.half / cos))
                });

                match tip {
                    Some(tip) => {
                        let [c, a, t, b] =
                            [self.vertex(point, center), self.vertex(point, center + n0), self.vertex(point, tip), self.vertex(point, center + n1)];
                        self.triangle(c, a, t);
                        self.triangle(c, t, b);
                    }
                    None => self.bevel(point, center, n0, n1),
                }
            }
            JointType::Round | JointType::RoundStart | JointType::RoundEnd | JointType::RoundStartEnd => {
                // 曲がる角だけ回す。折り返しでは、進む向きの側を通るよう右回りに半周する
                let sweep = if u_turn { -PI } else { cross.atan2(dot) };
                self.arc(point, n0, sweep);
            }
        }
    }

    fn bevel(&mut self, point: &Vertex, center: Vector2f, n0: Vector2f, n1: Vector2f) {
        let [c, a, b] = [self.vertex(point, center), self.vertex(point, center + n0), self.vertex(point, center + n1)];
        self.triangle(c, a, b);
    }

    // point を中心に、start（中心からの向きと長さ）から sweep だけ回す扇形（正は反時計回り）
    fn arc(&mut self, point: &Vertex, start: Vector2f, sweep: f32) {
        let center = xy(point);
        let steps = (sweep.abs() / ROUND_STEP).ceil().max(1.0) as u32;
        let c = self.vertex(point, center);
        let mut previous = self.vertex(point, center + start);

        for step in 1..=steps {
            let (sin, cos) = (sweep * step as f32 / steps as f32).sin_cos();
            let rotated = Vector2f::new(start.x * cos - start.y * sin, start.x * sin + start.y * cos);
            let next = self.vertex(point, center + rotated);

            self.triangle(c, previous, next);
            previous = next;
        }
    }

    // source の位置を xy だけ置き換えた頂点を足し、その番号を返す
    fn vertex(&mut self, source: &Vertex, position: Vector2f) -> u32 {
        self.mesh.vertices.push(Vertex { x: position.x, y: position.y, ..*source });

        (self.mesh.vertices.len() - 1) as u32
    }

    // xy 平面で反時計回りになるよう並べて足す。面積 0 の三角形は捨てる
    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        let [pa, pb, pc] = [a, b, c].map(|i| xy(&self.mesh.vertices[i as usize]));
        let area = (pb - pa).x * (pc - pa).y - (pb - pa).y * (pc - pa).x;

        if area > 0.0 {
            self.mesh.indices.extend([a, b, c]);
        } else if area < 0.0 {
            self.mesh.indices.extend([a, c, b]);
        }
    }
}

// ---- 破線 ----

// 破線の 1 本ずつを、開いた折れ線として返す
fn dash_pieces(points: &[Vertex], closed: bool, dash: Dash) -> Vec<Vec<Vertex>> {
    let count = points.len();
    let segments = if closed { count } else { count - 1 };
    // 頂点ごとの、始点からの線に沿った長さ。閉じた輪郭では最後に始点へ戻った長さが入る
    let mut distances = vec![0.0];

    for i in 0..segments {
        let length = (xy(&points[(i + 1) % count]) - xy(&points[i])).length();
        distances.push(distances[i] + length);
    }

    let total = distances[segments];
    let period = dash.length + dash.gap;
    // 始点での、模様の中の位置
    let base = -dash.offset.rem_euclid(period);
    let mut intervals = Vec::new();

    // start += period を繰り返すと、start が大きいとき f32 の精度で進まなくなるため、番号から求める
    for k in 0u64.. {
        let start = base + k as f32 * period;

        if start >= total {
            break;
        }

        let (from, to) = (start.max(0.0), (start + dash.length).min(total));

        if to - from > MIN_LENGTH {
            intervals.push((from, to));
        }
    }

    let along = |from: f32, to: f32| path_between(points, &distances, from, to);

    // 閉じた輪郭では、始点をまたぐ破線を 1 本につなぐ
    if closed && intervals.len() >= 2 && intervals[0].0 <= 0.0 && intervals[intervals.len() - 1].1 >= total {
        let (first, last) = (intervals.remove(0), intervals.pop().unwrap());
        let mut joined = along(last.0, total);

        joined.extend(along(0.0, first.1).into_iter().skip(1));

        return std::iter::once(joined).chain(intervals.into_iter().map(|(from, to)| along(from, to))).collect();
    }

    intervals.into_iter().map(|(from, to)| along(from, to)).collect()
}

// 線に沿った長さ from から to までの折れ線。両端は辺の途中を補間した頂点になる
fn path_between(points: &[Vertex], distances: &[f32], from: f32, to: f32) -> Vec<Vertex> {
    let count = points.len();
    let point_at = |distance: f32| {
        // distance を含む辺を探す（distances は増えていく）
        let i = distances.partition_point(|&d| d <= distance).clamp(1, distances.len() - 1) - 1;
        let length = distances[i + 1] - distances[i];
        let t = if length > 0.0 { ((distance - distances[i]) / length).clamp(0.0, 1.0) } else { 0.0 };

        lerp(&points[i % count], &points[(i + 1) % count], t)
    };
    let mut path = vec![point_at(from)];

    path.extend((1..distances.len() - 1).filter(|&i| distances[i] > from && distances[i] < to).map(|i| points[i % count]));
    path.push(point_at(to));
    path
}

// ---- 補助 ----

// 位置が同じ頂点が続く場合は 1 つにまとめる。閉じた輪郭では、最後が始点と同じなら最後を捨てる
fn dedup(points: &[Vertex], closed: bool) -> Vec<Vertex> {
    let mut result: Vec<Vertex> = Vec::with_capacity(points.len());

    for point in points {
        if result.last().is_none_or(|last| (xy(point) - xy(last)).length() > MIN_LENGTH) {
            result.push(*point);
        }
    }

    if closed && result.len() >= 2 && (xy(&result[0]) - xy(&result[result.len() - 1])).length() <= MIN_LENGTH {
        result.pop();
    }

    result
}

fn xy(vertex: &Vertex) -> Vector2f {
    Vector2f::new(vertex.x, vertex.y)
}

// 左向きの法線（反時計回りに 90 度）
fn perpendicular(direction: Vector2f) -> Vector2f {
    Vector2f::new(-direction.y, direction.x)
}

fn lerp(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let mix = |a: f32, b: f32| a + (b - a) * t;

    Vertex {
        x: mix(a.x, b.x),
        y: mix(a.y, b.y),
        z: mix(a.z, b.z),
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
        u: mix(a.u, b.u),
        v: mix(a.v, b.v),
        n_x: mix(a.n_x, b.n_x),
        n_y: mix(a.n_y, b.n_y),
        n_z: mix(a.n_z, b.n_z),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::topology::Topology;

    fn point(x: f32, y: f32) -> Vertex {
        Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0)
    }

    fn points(xy: &[(f32, f32)]) -> Vec<Vertex> {
        xy.iter().map(|&(x, y)| point(x, y)).collect()
    }

    fn solid(xy: &[(f32, f32)], width: f32, joint: JointType, strip: bool) -> Mesh {
        tessellate(&points(xy), &[], width, joint, strip, None)
    }

    fn triangles(mesh: &Mesh) -> Vec<[Vector2f; 3]> {
        mesh.indices.chunks_exact(3).map(|t| [0, 1, 2].map(|i| xy(&mesh.vertices[t[i] as usize]))).collect()
    }

    fn signed_area([a, b, c]: [Vector2f; 3]) -> f32 {
        ((b - a).x * (c - a).y - (b - a).y * (c - a).x) * 0.5
    }

    fn has_vertex_near(mesh: &Mesh, target: Vector2f) -> bool {
        mesh.vertices.iter().any(|v| (xy(v) - target).length() < 1e-4)
    }

    const ALL_JOINTS: [JointType; 7] = [
        JointType::None,
        JointType::Miter,
        JointType::Bevel,
        JointType::Round,
        JointType::RoundStart,
        JointType::RoundEnd,
        JointType::RoundStartEnd,
    ];

    #[test]
    fn straight_segment_is_one_quad() {
        let mesh = solid(&[(0.0, 0.0), (10.0, 0.0)], 2.0, JointType::Miter, true);

        assert_eq!((mesh.vertices.len(), mesh.indices.len()), (4, 6));
        for (x, y) in [(0.0, 1.0), (0.0, -1.0), (10.0, 1.0), (10.0, -1.0)] {
            assert!(has_vertex_near(&mesh, Vector2f::new(x, y)), "({x}, {y}) がありません");
        }
        let area: f32 = triangles(&mesh).into_iter().map(signed_area).sum();
        assert!((area - 20.0).abs() < 1e-4);
    }

    #[test]
    fn every_triangle_is_counter_clockwise() {
        let zigzag = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 5.0), (8.0, -6.0)];

        for joint in ALL_JOINTS {
            for strip in [true, false] {
                for dash in [None, Some(Dash { length: 3.0, gap: 1.5, offset: 0.7 })] {
                    let mesh = tessellate(&points(&zigzag), &[], 2.0, joint, strip, dash);

                    assert!(!mesh.indices.is_empty());
                    assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.vertices.len()));
                    for triangle in triangles(&mesh) {
                        assert!(signed_area(triangle) > 0.0, "{joint:?} strip={strip} dash={dash:?}: {triangle:?} が反時計回りではありません");
                    }
                }
            }
        }
    }

    #[test]
    fn miter_extends_the_outer_corner() {
        // 90 度の角。外側の尖りは (11, -1)
        let corner = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)];
        let miter = solid(&corner, 2.0, JointType::Miter, true);
        let bevel = solid(&corner, 2.0, JointType::Bevel, true);
        let none = solid(&corner, 2.0, JointType::None, true);

        assert!(has_vertex_near(&miter, Vector2f::new(11.0, -1.0)));
        assert!(!has_vertex_near(&bevel, Vector2f::new(11.0, -1.0)));
        // None は辺の四角形 2 つだけ、Bevel は三角形 1 つ、Miter は 2 つ足す
        assert_eq!(none.indices.len() / 3, 4);
        assert_eq!(bevel.indices.len() / 3, 5);
        assert_eq!(miter.indices.len() / 3, 6);
    }

    #[test]
    fn sharp_miter_falls_back_to_bevel() {
        // ほぼ折り返す角。尖れば x が 10 から大きく飛び出すが、Bevel なら線の太さの半分（1）までに収まる
        let corner = [(0.0, 0.0), (10.0, 0.0), (0.0, 0.5)];
        let mesh = solid(&corner, 2.0, JointType::Miter, true);
        let max_x = mesh.vertices.iter().map(|v| v.x).fold(f32::MIN, f32::max);

        assert!(max_x <= 11.0 + 1e-4, "尖りが長すぎます: {max_x}");
        assert_eq!(mesh.indices.len() / 3, 5);
    }

    #[test]
    fn round_joint_stays_on_the_circle() {
        let mesh = solid(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 2.0, JointType::Round, true);
        let corner = Vector2f::new(10.0, 0.0);
        let outer: Vec<Vector2f> = mesh.vertices.iter().map(xy).filter(|p| p.x > 10.0 + 1e-4 && p.y < -1e-4).collect();

        // 外側の角に、半径 1 の円の上の頂点が並ぶ
        assert!(!outer.is_empty());
        assert!(outer.iter().all(|p| ((*p - corner).length() - 1.0).abs() < 1e-4));
    }

    #[test]
    fn round_caps_follow_joint_type() {
        let line = [(0.0, 0.0), (10.0, 0.0)];
        let reaches = |joint| {
            let mesh = solid(&line, 2.0, joint, true);
            let min = mesh.vertices.iter().map(|v| v.x).fold(f32::MAX, f32::min);
            let max = mesh.vertices.iter().map(|v| v.x).fold(f32::MIN, f32::max);
            (min < -0.5, max > 10.5)
        };

        assert_eq!(reaches(JointType::Round), (false, false));
        assert_eq!(reaches(JointType::RoundStart), (true, false));
        assert_eq!(reaches(JointType::RoundEnd), (false, true));
        assert_eq!(reaches(JointType::RoundStartEnd), (true, true));
    }

    #[test]
    fn closed_outline_joins_every_corner() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let closed = solid(&square, 2.0, JointType::Miter, false);
        let open = solid(&square, 2.0, JointType::Miter, true);

        // 閉じた輪郭は 4 辺と 4 つの角。開いた折れ線は 3 辺と内側の 2 つの角
        assert_eq!(closed.indices.len() / 3, 4 * 2 + 4 * 2);
        assert_eq!(open.indices.len() / 3, 3 * 2 + 2 * 2);
        for (x, y) in [(11.0, -1.0), (11.0, 11.0), (-1.0, 11.0), (-1.0, -1.0)] {
            assert!(has_vertex_near(&closed, Vector2f::new(x, y)), "({x}, {y}) がありません");
        }
        // 閉じた輪郭に端の丸めは付かない
        let rounded = solid(&square, 2.0, JointType::RoundStartEnd, false);
        assert!(rounded.vertices.iter().all(|v| v.x >= -1.0 - 1e-4 && v.y >= -1.0 - 1e-4));
    }

    #[test]
    fn dash_pattern_and_offset() {
        let line = points(&[(0.0, 0.0), (10.0, 0.0)]);
        let ranges = |offset| {
            dash_pieces(&line, false, Dash { length: 2.0, gap: 1.0, offset })
                .iter()
                .map(|piece| (piece[0].x, piece[piece.len() - 1].x))
                .collect::<Vec<_>>()
        };

        assert_eq!(ranges(0.0), [(0.0, 2.0), (3.0, 5.0), (6.0, 8.0), (9.0, 10.0)]);
        // offset だけ模様を進めた位置から始まる。負の値は逆にずらす
        assert_eq!(ranges(1.0), [(0.0, 1.0), (2.0, 4.0), (5.0, 7.0), (8.0, 10.0)]);
        assert_eq!(ranges(-1.0), [(1.0, 3.0), (4.0, 6.0), (7.0, 9.0)]);
        assert_eq!(ranges(3.0), ranges(0.0));
    }

    #[test]
    fn dash_follows_corners_and_joins_across_the_start_of_a_closed_outline() {
        // 1 周 40 の正方形。offset 1 なら破線は -1, 4, 9, …, 39 から始まる 9 本で、
        // 始点の 1 手前から始まる最初の破線と、39 から始まる最後の破線がつながって 8 本になる
        let square = points(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
        let pieces = dash_pieces(&square, true, Dash { length: 3.0, gap: 2.0, offset: 1.0 });
        let length = |piece: &[Vertex]| piece.windows(2).map(|w| (xy(&w[1]) - xy(&w[0])).length()).sum::<f32>();
        let coordinates = |piece: &[Vertex]| piece.iter().map(|v| (v.x, v.y)).collect::<Vec<_>>();

        assert_eq!(pieces.len(), 8);
        assert!(pieces.iter().all(|piece| (length(piece) - 3.0).abs() < 1e-4));
        // つないだ破線は (0, 1) → (0, 0) → (2, 0) で、始点の角を内側の頂点として含む
        assert_eq!(coordinates(&pieces[0]), [(0.0, 1.0), (0.0, 0.0), (2.0, 0.0)]);
        // 角をまたぐ破線は角の頂点を含む
        assert!(pieces.iter().any(|piece| coordinates(piece) == [(9.0, 0.0), (10.0, 0.0), (10.0, 2.0)]));
    }

    #[test]
    fn dash_ends_interpolate_vertex_attributes() {
        let mut line = points(&[(0.0, 0.0), (10.0, 0.0)]);
        line[0].r = 0.0;
        line[1].r = 1.0;

        let pieces = dash_pieces(&line, false, Dash { length: 2.0, gap: 3.0, offset: 0.0 });
        let reds: Vec<(f32, f32)> = pieces.iter().map(|piece| (piece[0].r, piece[piece.len() - 1].r)).collect();

        assert_eq!(reds, [(0.0, 0.2), (0.5, 0.7)]);
    }

    #[test]
    fn degenerate_input_draws_nothing_or_skips_duplicates() {
        assert!(solid(&[], 2.0, JointType::Miter, true).indices.is_empty());
        assert!(solid(&[(1.0, 1.0)], 2.0, JointType::Miter, true).indices.is_empty());
        assert!(solid(&[(1.0, 1.0), (1.0, 1.0)], 2.0, JointType::Round, true).indices.is_empty());

        // 同じ位置の点が続いても、まとめて 1 本の線分として扱う（NaN にならない）
        let mesh = solid(&[(0.0, 0.0), (0.0, 0.0), (10.0, 0.0), (10.0, 0.0)], 2.0, JointType::Miter, true);
        assert_eq!(mesh.indices.len(), 6);
        assert!(mesh.vertices.iter().all(|v| v.x.is_finite() && v.y.is_finite()));

        // 閉じた輪郭で最後に始点を繰り返しても、同じ形になる
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let repeated = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)];
        assert_eq!(solid(&square, 2.0, JointType::Miter, false).indices, solid(&repeated, 2.0, JointType::Miter, false).indices);
    }

    #[test]
    fn holes_are_outlined_as_separate_rings() {
        // 外周 0〜10 の正方形と、穴 4〜6 の正方形
        let rings = points(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)]);
        let closed = tessellate(&rings, &[4], 1.0, JointType::Miter, false, None);
        let open = tessellate(&rings, &[4], 1.0, JointType::Miter, true, None);

        // 閉じた輪郭は、どちらの輪も 4 辺と 4 つの角（三角形 16 個ずつ）。外周と穴の間をつなぐ線はない
        assert_eq!(closed.indices.len() / 3, 16 * 2);
        for (x, y) in [(10.5, 10.5), (-0.5, -0.5), (6.5, 6.5), (3.5, 3.5)] {
            assert!(has_vertex_near(&closed, Vector2f::new(x, y)), "({x}, {y}) がありません");
        }
        assert!(!closed.vertices.iter().any(|v| (v.x - 7.0).abs() < 1e-4 || (v.y - 7.0).abs() < 1e-4));
        // 開いた折れ線は、輪ごとに 3 辺と内側の 2 つの角
        assert_eq!(open.indices.len() / 3, (3 * 2 + 2 * 2) * 2);
        // 空の穴は何も描かない
        assert_eq!(tessellate(&rings, &[4, 8], 1.0, JointType::Miter, false, None).indices, closed.indices);
    }

    #[test]
    fn invalid_settings_panic() {
        let stroke = |line_width, dash| Topology::Stroke { line_width, joint_type: JointType::Miter, strip: true, dash };
        let panics = |topology: Topology| std::panic::catch_unwind(|| topology.validate()).is_err();
        let dash = |length, gap, offset| Some(Dash { length, gap, offset });

        assert!(!panics(stroke(1.0, None)));
        assert!(!panics(stroke(1.0, dash(1.0, 1.0, -5.0))));
        assert!(panics(stroke(0.0, None)));
        assert!(panics(stroke(-1.0, None)));
        assert!(panics(stroke(f32::NAN, None)));
        assert!(panics(stroke(1.0, dash(0.0, 1.0, 0.0))));
        assert!(panics(stroke(1.0, dash(1.0, 0.0, 0.0))));
        assert!(panics(stroke(1.0, dash(1.0, 1.0, f32::INFINITY))));
    }
}
