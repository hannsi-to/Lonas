// 三角形化：index() を呼ばない Object* のインデックスを作る（§10 三角形化：topology）
// GPU に送るプリミティブは常に三角形。Stroke 以外はインデックスの作り方だけを選ぶ。Stroke は頂点も作る（stroke.rs）

use crate::math::{Vector2f, Vector3f};

// Stroke が f32 を持つため Eq / Hash は実装しない
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Topology {
    // 3 つずつで 1 つの三角形
    TriangleList,
    // 凸多角形の外周
    TriangleFan,
    // 帯状に続く三角形
    TriangleStrip,
    // へこみのある多角形の外周（単調多角形分割）
    Polygon,
    // 頂点を線の中心として、太さのある線にする（§10 線：Stroke）。xy 平面で広げ、z は頂点ごとの値を保つ
    Stroke {
        /// 線の太さ。頂点と同じ単位（ピクセル座標なら px）。
        line_width: f32,
        /// 角のつなぎ方。
        joint_type: JointType,
        /// `true` なら開いた折れ線、`false` なら閉じた輪郭。
        strip: bool,
        /// `Some` なら破線にする。`None` なら実線。
        dash: Option<Dash>,
    },
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum JointType {
    /// 何も足さない。角の外側に隙間が空く。いちばん軽い。
    None,
    /// 外側の辺を延長して尖らせる。角が鋭すぎるときは
    /// [`JointType::Bevel`] に落ちる。
    #[default]
    Miter,
    /// 外側の角を三角形 1 枚で塞ぐ。
    Bevel,
    /// 外側の角を扇形で丸める。
    Round,
    /// 丸い角 + 始点を丸く閉じる。
    RoundStart,
    /// 丸い角 + 終点を丸く閉じる。
    RoundEnd,
    /// 丸い角 + 両端を丸く閉じる。
    RoundStartEnd,
}

impl JointType {
    // 始点・終点を丸く閉じるか。閉じた輪郭（strip: false）には端がないため使わない
    pub(crate) fn round_caps(self) -> (bool, bool) {
        match self {
            JointType::RoundStart => (true, false),
            JointType::RoundEnd => (false, true),
            JointType::RoundStartEnd => (true, true),
            _ => (false, false),
        }
    }
}

/// 破線の模様。長さはすべて頂点と同じ単位。
/// 破線の 1 本 1 本を短い線として扱うため、[`JointType::RoundStart`] などの端の丸めは各破線の端に付く。
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Dash {
    /// 線を描く部分の長さ。0 より大きいこと。
    pub length: f32,
    /// 描かない部分の長さ。0 より大きいこと。
    pub gap: f32,
    /// 模様をずらす量。線の始点では、模様の先頭から `offset` だけ進んだ位置から始まる。負の値も使える。
    pub offset: f32,
}

impl Topology {
    // 値の検査。不正なら panic（§3 設定の値の検査）
    pub(crate) fn validate(&self) {
        let Topology::Stroke { line_width, dash, .. } = *self else { return };

        assert!(line_width.is_finite() && line_width > 0.0, "Stroke の line_width は 0 より大きい有限の値にしてください（{line_width}）");

        if let Some(Dash { length, gap, offset }) = dash {
            assert!(length.is_finite() && length > 0.0, "Dash の length は 0 より大きい有限の値にしてください（{length}）");
            assert!(gap.is_finite() && gap > 0.0, "Dash の gap は 0 より大きい有限の値にしてください（{gap}）");
            assert!(offset.is_finite(), "Dash の offset は有限の値にしてください（{offset}）");
        }
    }
}

// 生成したインデックスと、頂点数が分け方に合わなかったか
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Generated {
    pub indices: Vec<u32>,
    // 三角形に含めなかった頂点がある（log で警告する、§10）
    pub has_leftover: bool,
}

// 生成する三角形の向き（§10 Polygon の頂点の向き）
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Winding {
    // 2D：入力の向きによらず、xy 平面で反時計回りに揃える
    CounterClockwiseXy,
    // 3D：入力の頂点の並びと同じ向き（どちらを表にするかはユーザーの頂点の順で決まる）
    FollowInput,
}

pub(crate) fn generate(topology: Topology, positions: &[Vector3f], winding: Winding) -> Generated {
    let count = positions.len() as u32;

    if count < 3 {
        // どの分け方でも、頂点が 3 個未満なら何も描かない
        return Generated { indices: Vec::new(), has_leftover: count > 0 };
    }

    match topology {
        Topology::TriangleList => Generated { indices: (0..count / 3 * 3).collect(), has_leftover: !count.is_multiple_of(3) },
        Topology::TriangleFan => Generated {
            indices: (1..count - 1).flat_map(|i| [0, i, i + 1]).collect(),
            has_leftover: false,
        },
        Topology::TriangleStrip => Generated {
            // 向きを揃えるため 1 つおきに入れ替える
            indices: (0..count - 2).flat_map(|i| if i % 2 == 0 { [i, i + 1, i + 2] } else { [i + 1, i, i + 2] }).collect(),
            has_leftover: false,
        },
        Topology::Polygon => Generated { indices: triangulate_polygon(positions, winding), has_leftover: false },
        Topology::Stroke { .. } => unreachable!("Stroke は頂点も作るため stroke::tessellate で三角形化する"),
    }
}

// ---- Polygon：単調多角形分割（§10 Polygon の分割方法）----

fn triangulate_polygon(positions: &[Vector3f], winding: Winding) -> Vec<u32> {
    let points = project_to_plane(positions, winding);
    let area = signed_area(&points);

    if area.abs() <= f32::EPSILON {
        return Vec::new();
    }

    // 内部では反時計回りに揃えて処理する
    let order: Vec<usize> = if area > 0.0 { (0..points.len()).collect() } else { (0..points.len()).rev().collect() };
    let ccw: Vec<Vector2f> = order.iter().map(|&i| points[i]).collect();
    let mut triangles = Vec::new();

    for face in monotone_faces(&ccw) {
        triangulate_monotone(&ccw, &face, &mut triangles);
    }

    // 3D は入力と同じ向きにする（入力が時計回りなら裏返す）
    let flip = winding == Winding::FollowInput && area < 0.0;

    triangles
        .into_iter()
        .flat_map(|[a, b, c]| {
            let [a, b, c] = [order[a], order[b], order[c]].map(|i| i as u32);
            if flip { [a, c, b] } else { [a, b, c] }
        })
        .collect()
}

// 多角形の平面上の 2D 座標にする。2D はそのまま xy、3D は法線の最も大きい軸を落とす
fn project_to_plane(positions: &[Vector3f], winding: Winding) -> Vec<Vector2f> {
    if winding == Winding::CounterClockwiseXy {
        return positions.iter().map(|p| Vector2f::new(p.x, p.y)).collect();
    }

    // Newell の方法で法線を求める
    let normal = positions.iter().enumerate().fold(Vector3f::ZERO, |n, (i, a)| {
        let b = positions[(i + 1) % positions.len()];
        n + Vector3f::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y))
    });
    let (x, y, z) = (normal.x.abs(), normal.y.abs(), normal.z.abs());

    positions
        .iter()
        .map(|p| {
            if z >= x && z >= y {
                Vector2f::new(p.x, p.y)
            } else if x >= y {
                Vector2f::new(p.y, p.z)
            } else {
                Vector2f::new(p.z, p.x)
            }
        })
        .collect()
}

fn signed_area(points: &[Vector2f]) -> f32 {
    let sum: f32 = points
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = points[(i + 1) % points.len()];
            a.x * b.y - b.x * a.y
        })
        .sum();

    sum * 0.5
}

// a → b → c が左に曲がれば正
fn orient(a: Vector2f, b: Vector2f, c: Vector2f) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

// 走査線は上から下へ進む。y が大きいほど上、同じ y なら x が小さいほど上
fn is_above(a: Vector2f, b: Vector2f) -> bool {
    a.y > b.y || (a.y == b.y && a.x < b.x)
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum VertexKind {
    Start,
    End,
    Split,
    Merge,
    Regular,
}

// 走査線に交わる辺（左側に多角形の内部がない辺だけ）と、その helper
struct StatusEdge {
    edge: usize, // 辺 i は頂点 i → i + 1
    helper: usize,
}

// へこみの原因になる頂点に対角線を引き、y 方向に単調な多角形に分ける。各多角形の頂点を反時計回りで返す
fn monotone_faces(points: &[Vector2f]) -> Vec<Vec<usize>> {
    let n = points.len();
    let prev = |i: usize| (i + n - 1) % n;
    let next = |i: usize| (i + 1) % n;
    let kind = |i: usize| {
        let (p, v, q) = (points[prev(i)], points[i], points[next(i)]);
        let convex = orient(p, v, q) > 0.0;

        match (is_above(v, p), is_above(v, q)) {
            (true, true) if convex => VertexKind::Start,
            (true, true) => VertexKind::Split,
            (false, false) if convex => VertexKind::End,
            (false, false) => VertexKind::Merge,
            _ => VertexKind::Regular,
        }
    };
    // 走査線の高さ y での辺の x
    let x_at = |edge: usize, y: f32| {
        let (a, b) = (points[edge], points[next(edge)]);

        if (a.y - b.y).abs() <= f32::EPSILON { a.x.min(b.x) } else { a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y) }
    };

    let mut events: Vec<usize> = (0..n).collect();
    events.sort_by(|&a, &b| {
        if is_above(points[a], points[b]) {
            std::cmp::Ordering::Less
        } else if is_above(points[b], points[a]) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });

    let mut status: Vec<StatusEdge> = Vec::new();
    let mut diagonals: Vec<(usize, usize)> = Vec::new();

    // v のすぐ左にある辺
    let left_of = |status: &Vec<StatusEdge>, v: usize| -> usize {
        let p = points[v];

        status
            .iter()
            .enumerate()
            .filter(|(_, s)| x_at(s.edge, p.y) <= p.x)
            .max_by(|(_, a), (_, b)| x_at(a.edge, p.y).total_cmp(&x_at(b.edge, p.y)))
            .map(|(index, _)| index)
            .expect("頂点の左に辺がありません（自分と交差する多角形は扱わない）")
    };
    let position = |status: &Vec<StatusEdge>, edge: usize| status.iter().position(|s| s.edge == edge);

    for v in events {
        match kind(v) {
            VertexKind::Start => status.push(StatusEdge { edge: v, helper: v }),
            VertexKind::End => {
                if let Some(index) = position(&status, prev(v)) {
                    let helper = status.remove(index).helper;
                    if kind(helper) == VertexKind::Merge {
                        diagonals.push((v, helper));
                    }
                }
            }
            VertexKind::Split => {
                let left = left_of(&status, v);
                diagonals.push((v, status[left].helper));
                status[left].helper = v;
                status.push(StatusEdge { edge: v, helper: v });
            }
            VertexKind::Merge => {
                if let Some(index) = position(&status, prev(v)) {
                    let helper = status.remove(index).helper;
                    if kind(helper) == VertexKind::Merge {
                        diagonals.push((v, helper));
                    }
                }

                let left = left_of(&status, v);
                if kind(status[left].helper) == VertexKind::Merge {
                    diagonals.push((v, status[left].helper));
                }
                status[left].helper = v;
            }
            VertexKind::Regular => {
                // 前の頂点が上にあるなら、多角形の内部は v の右側
                if is_above(points[prev(v)], points[v]) {
                    if let Some(index) = position(&status, prev(v)) {
                        let helper = status.remove(index).helper;
                        if kind(helper) == VertexKind::Merge {
                            diagonals.push((v, helper));
                        }
                    }
                    status.push(StatusEdge { edge: v, helper: v });
                } else {
                    let left = left_of(&status, v);
                    if kind(status[left].helper) == VertexKind::Merge {
                        diagonals.push((v, status[left].helper));
                    }
                    status[left].helper = v;
                }
            }
        }
    }

    trace_faces(points, &diagonals)
}

// 外周の辺と対角線でできる面を、内部を左に見ながらたどる
fn trace_faces(points: &[Vector2f], diagonals: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let n = points.len();
    let mut neighbors: Vec<Vec<usize>> = (0..n).map(|i| vec![(i + n - 1) % n, (i + 1) % n]).collect();
    // たどる辺：外周は反時計回りの向きだけ、対角線は両方の向き
    let mut edges: Vec<(usize, usize)> = (0..n).map(|i| (i, (i + 1) % n)).collect();

    for &(a, b) in diagonals {
        neighbors[a].push(b);
        neighbors[b].push(a);
        edges.push((a, b));
        edges.push((b, a));
    }

    let angle = |from: usize, to: usize| {
        let d = points[to] - points[from];
        d.y.atan2(d.x)
    };
    let mut used = std::collections::HashSet::new();
    let mut faces = Vec::new();

    for &start in &edges {
        if used.contains(&start) {
            continue;
        }

        let mut face = Vec::new();
        let (mut from, mut to) = start;

        while used.insert((from, to)) {
            face.push(from);
            // 戻る向きから時計回りに最初の辺へ進む（最も鋭く左に曲がる）
            let back = angle(to, from);
            let next = neighbors[to]
                .iter()
                .copied()
                .filter(|&w| w != from)
                .min_by(|&a, &b| {
                    let turn = |w: usize| (back - angle(to, w)).rem_euclid(std::f32::consts::TAU);
                    turn(a).total_cmp(&turn(b))
                })
                .expect("面をたどれません");

            (from, to) = (to, next);
        }

        faces.push(face);
    }

    faces
}

// y 方向に単調な多角形（頂点は反時計回り）を三角形に分ける。O(n)
fn triangulate_monotone(points: &[Vector2f], face: &[usize], triangles: &mut Vec<[usize; 3]>) {
    let n = face.len();

    if n == 3 {
        push_ccw(points, triangles, [face[0], face[1], face[2]]);
        return;
    }

    // 一番上から反時計回りに一番下までが左の鎖、残りが右の鎖
    let top = (0..n).min_by(|&a, &b| above_order(points, face[a], face[b])).unwrap();
    let bottom = (0..n).max_by(|&a, &b| above_order(points, face[a], face[b])).unwrap();
    let mut on_left = vec![false; n];
    let mut i = top;

    while i != bottom {
        on_left[i] = true;
        i = (i + 1) % n;
    }

    on_left[bottom] = true;

    let mut sorted: Vec<usize> = (0..n).collect();
    sorted.sort_by(|&a, &b| above_order(points, face[a], face[b]));

    let mut stack = vec![sorted[0], sorted[1]];

    for &u in &sorted[2..n - 1] {
        let top_of_stack = *stack.last().unwrap();

        if on_left[u] != on_left[top_of_stack] {
            // 反対の鎖：スタックのすべてと結ぶ
            while stack.len() > 1 {
                let a = stack.pop().unwrap();
                push_ccw(points, triangles, [face[u], face[a], face[*stack.last().unwrap()]]);
            }

            stack = vec![top_of_stack, u];
        } else {
            // 同じ鎖：内部を通る対角線だけを結ぶ
            let mut last = stack.pop().unwrap();

            while let Some(&candidate) = stack.last() {
                let turn = orient(points[face[candidate]], points[face[last]], points[face[u]]);
                let inside = if on_left[u] { turn > 0.0 } else { turn < 0.0 };

                if !inside {
                    break;
                }

                push_ccw(points, triangles, [face[u], face[last], face[candidate]]);
                last = stack.pop().unwrap();
            }

            stack.push(last);
            stack.push(u);
        }
    }

    let lowest = sorted[n - 1];

    for pair in stack.windows(2) {
        push_ccw(points, triangles, [face[lowest], face[pair[0]], face[pair[1]]]);
    }
}

fn above_order(points: &[Vector2f], a: usize, b: usize) -> std::cmp::Ordering {
    if is_above(points[a], points[b]) {
        std::cmp::Ordering::Less
    } else if is_above(points[b], points[a]) {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

// 面積のない三角形は捨て、反時計回りに揃えて加える
fn push_ccw(points: &[Vector2f], triangles: &mut Vec<[usize; 3]>, [a, b, c]: [usize; 3]) {
    let turn = orient(points[a], points[b], points[c]);

    if turn > 0.0 {
        triangles.push([a, b, c]);
    } else if turn < 0.0 {
        triangles.push([a, c, b]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(xy: &[(f32, f32)]) -> Vec<Vector3f> {
        xy.iter().map(|&(x, y)| Vector3f::new(x, y, 0.0)).collect()
    }

    fn area_of_triangles(positions: &[Vector3f], indices: &[u32]) -> f32 {
        indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vector2f::new(positions[i as usize].x, positions[i as usize].y));
                orient(a, b, c) * 0.5
            })
            .sum()
    }

    #[test]
    fn simple_topologies() {
        let six = points(&[(0.0, 0.0); 6]);

        assert_eq!(generate(Topology::TriangleList, &six, Winding::CounterClockwiseXy).indices, [0, 1, 2, 3, 4, 5]);
        assert_eq!(
            generate(Topology::TriangleFan, &six, Winding::CounterClockwiseXy).indices,
            [0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 5],
        );
        assert_eq!(
            generate(Topology::TriangleStrip, &six[..5], Winding::CounterClockwiseXy).indices,
            [0, 1, 2, 2, 1, 3, 2, 3, 4],
        );
    }

    #[test]
    fn leftover_vertices_are_dropped() {
        let seven = points(&[(0.0, 0.0); 7]);
        let generated = generate(Topology::TriangleList, &seven, Winding::CounterClockwiseXy);

        assert_eq!(generated.indices.len(), 6);
        assert!(generated.has_leftover);

        let two = generate(Topology::TriangleFan, &seven[..2], Winding::CounterClockwiseXy);
        assert!(two.indices.is_empty() && two.has_leftover);
    }

    // 三角形の面積の合計が多角形の面積と一致し、どの三角形も反時計回り
    fn check_polygon(xy: &[(f32, f32)]) {
        let positions = points(xy);
        let projected: Vec<Vector2f> = positions.iter().map(|p| Vector2f::new(p.x, p.y)).collect();
        let indices = generate(Topology::Polygon, &positions, Winding::CounterClockwiseXy).indices;

        assert_eq!(indices.len(), (xy.len() - 2) * 3, "三角形の数は頂点数 - 2");
        assert!((area_of_triangles(&positions, &indices) - signed_area(&projected).abs()).abs() < 1e-3);

        for t in indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| projected[i as usize]);
            assert!(orient(a, b, c) > 0.0, "三角形が反時計回りではありません");
        }
    }

    #[test]
    fn convex_polygon() {
        check_polygon(&[(0.0, 0.0), (2.0, 0.0), (3.0, 1.0), (2.0, 2.0), (0.0, 2.0)]);
    }

    #[test]
    fn concave_polygons_in_both_orientations() {
        // へこみのある形（split / merge 頂点を含む）
        let shapes: [&[(f32, f32)]; 3] = [
            &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (2.0, 1.0), (0.0, 4.0)],                         // 上がへこんだ形
            &[(0.0, 0.0), (2.0, 3.0), (4.0, 0.0), (4.0, 5.0), (0.0, 5.0)],                         // 下がへこんだ形
            &[(0.0, 0.0), (6.0, 0.0), (6.0, 6.0), (4.0, 2.0), (3.0, 6.0), (2.0, 2.0), (0.0, 6.0)], // くし形
        ];

        for shape in shapes {
            check_polygon(shape);

            let reversed: Vec<(f32, f32)> = shape.iter().rev().copied().collect();
            check_polygon(&reversed);
        }
    }

    #[test]
    fn star_polygon() {
        let star: Vec<(f32, f32)> = (0..10)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::TAU / 10.0;
                let radius = if i % 2 == 0 { 5.0 } else { 2.0 };
                (radius * angle.cos(), radius * angle.sin())
            })
            .collect();

        check_polygon(&star);
    }

    #[test]
    fn random_star_shaped_polygons() {
        let mut rng = crate::memory::tlsf::tests::Rng::new(99);

        for _ in 0..300 {
            let count = 3 + rng.below(30) as usize;
            // 中心から見て角度の順に並べた点は、隣り合う角度の間隔がすべて 180 度未満なら自分と交差しない（へこみは多い）
            let mut angles: Vec<f32> = (0..count).map(|_| rng.below(10_000) as f32 / 10_000.0 * std::f32::consts::TAU).collect();
            angles.sort_by(f32::total_cmp);
            angles.dedup_by(|a, b| (*a - *b).abs() < 1e-3);

            let wrap_gap = angles.first().map_or(0.0, |first| first + std::f32::consts::TAU) - angles.last().copied().unwrap_or(0.0);
            let max_gap = angles.windows(2).map(|w| w[1] - w[0]).fold(wrap_gap, f32::max);

            if angles.len() < 3 || max_gap >= std::f32::consts::PI - 1e-3 {
                continue;
            }

            let shape: Vec<(f32, f32)> = angles
                .iter()
                .map(|&angle| {
                    let radius = 1.0 + rng.below(1000) as f32 / 100.0;
                    (radius * angle.cos(), radius * angle.sin())
                })
                .collect();

            check_polygon(&shape);
        }
    }

    #[test]
    fn polygon_3d_follows_input_winding() {
        // xz 平面上の四角形（y = 1）
        let square = [
            Vector3f::new(0.0, 1.0, 0.0),
            Vector3f::new(0.0, 1.0, 1.0),
            Vector3f::new(1.0, 1.0, 1.0),
            Vector3f::new(1.0, 1.0, 0.0),
        ];
        let normal_of = |indices: &[u32], positions: &[Vector3f]| {
            let [a, b, c] = [indices[0], indices[1], indices[2]].map(|i| positions[i as usize]);
            (b - a).cross(c - a)
        };

        let forward = generate(Topology::Polygon, &square, Winding::FollowInput).indices;
        let reversed_square: Vec<Vector3f> = square.iter().rev().copied().collect();
        let backward = generate(Topology::Polygon, &reversed_square, Winding::FollowInput).indices;

        assert_eq!(forward.len(), 6);
        // 入力の向きが逆なら、三角形の表も逆になる
        assert!(normal_of(&forward, &square).y > 0.0);
        assert!(normal_of(&backward, &reversed_square).y < 0.0);
    }
}
