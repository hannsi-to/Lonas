// 塗り方：index() を呼ばない Object* のインデックスを作る（§10 塗り方：PaintType）
// GPU に送るプリミティブは常に三角形。Fill はインデックスだけを作り、Stroke は頂点も作る（stroke.rs）

use std::ops::Range;

use crate::math::{Vector2f, Vector3f};

// Stroke が f32 を持つため Eq / Hash は実装しない
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PaintType {
    // 頂点を外周とする多角形を塗る（単調多角形分割）。へこみ・穴（begin_hole）があってもよい
    Fill,
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

impl PaintType {
    // 値の検査。不正なら panic（§3 設定の値の検査）
    pub(crate) fn validate(&self) {
        let PaintType::Stroke { line_width, dash, .. } = *self else { return };

        assert!(line_width.is_finite() && line_width > 0.0, "Stroke の line_width は 0 より大きい有限の値にしてください（{line_width}）");

        if let Some(Dash { length, gap, offset }) = dash {
            assert!(length.is_finite() && length > 0.0, "Dash の length は 0 より大きい有限の値にしてください（{length}）");
            assert!(gap.is_finite() && gap > 0.0, "Dash の gap は 0 より大きい有限の値にしてください（{gap}）");
            assert!(offset.is_finite(), "Dash の offset は有限の値にしてください（{offset}）");
        }
    }
}

// 生成したインデックスと、描かなかった形があるか
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Generated {
    pub indices: Vec<u32>,
    // 外周の頂点が 3 個未満で描かなかった形がある（log で警告する、§10）
    pub has_leftover: bool,
}

// 生成する三角形の向き（§10 Fill の頂点の向き）
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Winding {
    // 2D：入力の向きによらず、xy 平面で反時計回りに揃える
    CounterClockwiseXy,
    // 3D：入力の頂点の並びと同じ向き（どちらを表にするかはユーザーの頂点の順で決まる）
    FollowInput,
}

// begin_shape() / begin_hole() で区切った位置（その時点の頂点数、§10 形の区切り・穴のある多角形）。呼んだ順に並ぶ
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Break {
    Shape(usize),
    Hole(usize),
}

// 1 つの形の頂点の範囲。外周の後に穴が続けて並ぶ。どれも空のことがある
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    pub outer: Range<usize>,
    pub holes: Vec<Range<usize>>,
}

impl Shape {
    // 外周と穴（最初が外周）
    pub(crate) fn rings(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        std::iter::once(self.outer.clone()).chain(self.holes.iter().cloned())
    }

    // 形の最後の頂点の次の位置
    fn end(&self) -> usize {
        self.holes.last().map_or(self.outer.end, |hole| hole.end)
    }
}

// 頂点を形に分ける。最初の形は頂点 0 から始まる。頂点数を超える区切りは頂点数にする
pub(crate) fn shapes(count: usize, breaks: &[Break]) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let mut current = Shape { outer: 0..0, holes: Vec::new() };
    let mut start = 0;
    let mut in_hole = false;

    for &split in breaks {
        let (at, hole_follows) = match split {
            Break::Shape(at) => (at, false),
            Break::Hole(at) => (at, true),
        };
        let at = at.clamp(start, count);

        close_ring(&mut current, start..at, in_hole);

        if !hole_follows {
            shapes.push(std::mem::replace(&mut current, Shape { outer: at..at, holes: Vec::new() }));
        }

        start = at;
        in_hole = hole_follows;
    }

    close_ring(&mut current, start..count, in_hole);
    shapes.push(current);
    shapes
}

fn close_ring(shape: &mut Shape, ring: Range<usize>, is_hole: bool) {
    if is_hole {
        shape.holes.push(ring);
    } else {
        shape.outer = ring;
    }
}

// Fill のインデックスを、形ごとに別々に作る。breaks は begin_shape() / begin_hole() の区切り
pub(crate) fn fill(positions: &[Vector3f], breaks: &[Break], winding: Winding) -> Generated {
    let mut generated = Generated { indices: Vec::new(), has_leftover: false };

    for shape in shapes(positions.len(), breaks) {
        if shape.outer.len() < 3 {
            // 外周の頂点が 3 個未満の形は描かない
            generated.has_leftover |= !shape.outer.is_empty();
            continue;
        }

        let base = shape.outer.start;
        let rings: Vec<Range<usize>> = shape.rings().map(|ring| ring.start - base..ring.end - base).collect();

        generated
            .indices
            .extend(triangulate_polygon(&positions[base..shape.end()], &rings, winding).into_iter().map(|i| (base + i as usize) as u32));
    }

    generated
}

// ---- Fill：単調多角形分割（§10 Fill の分割方法）----

// 輪ごとの前後の頂点。辺 i は頂点 i → next[i]
struct Links {
    prev: Vec<usize>,
    next: Vec<usize>,
}

impl Links {
    // 輪の頂点が続けて並んでいる前提
    fn new(rings: &[Range<usize>]) -> Self {
        let count = rings.last().map_or(0, |ring| ring.end);
        let mut links = Links { prev: vec![0; count], next: vec![0; count] };

        for ring in rings {
            for i in ring.clone() {
                links.next[i] = if i + 1 == ring.end { ring.start } else { i + 1 };
                links.prev[i] = if i == ring.start { ring.end - 1 } else { i - 1 };
            }
        }

        links
    }
}

// rings は外周と穴の頂点の範囲（最初が外周）。positions はその形の頂点だけで、戻り値の番号も positions の中の位置
fn triangulate_polygon(positions: &[Vector3f], rings: &[Range<usize>], winding: Winding) -> Vec<u32> {
    let points = project_to_plane(positions, rings[0].clone(), winding);
    let outer_area = signed_area(&points[rings[0].clone()]);

    if outer_area.abs() <= f32::EPSILON {
        return Vec::new();
    }

    // 内部では外周を反時計回り、穴を時計回りに揃え、輪ごとに続けて並べる。どの辺でも多角形の内部が左になる
    // 頂点が 3 個未満の穴・面積のない穴は使わない
    let mut order: Vec<usize> = Vec::new();
    let mut internal_rings = Vec::new();

    for (index, ring) in rings.iter().enumerate() {
        let area = signed_area(&points[ring.clone()]);

        if ring.len() < 3 || area.abs() <= f32::EPSILON {
            continue;
        }

        let start = order.len();
        let counter_clockwise = index == 0;

        if (area > 0.0) == counter_clockwise {
            order.extend(ring.clone());
        } else {
            order.extend(ring.clone().rev());
        }

        internal_rings.push(start..order.len());
    }

    let ccw: Vec<Vector2f> = order.iter().map(|&i| points[i]).collect();
    let links = Links::new(&internal_rings);
    let mut triangles = Vec::new();

    for face in monotone_faces(&ccw, &links) {
        triangulate_monotone(&ccw, &face, &mut triangles);
    }

    // 3D は入力と同じ向きにする（外周の入力が時計回りなら裏返す）
    let flip = winding == Winding::FollowInput && outer_area < 0.0;

    triangles
        .into_iter()
        .flat_map(|[a, b, c]| {
            let [a, b, c] = [order[a], order[b], order[c]].map(|i| i as u32);
            if flip { [a, c, b] } else { [a, b, c] }
        })
        .collect()
}

// 多角形の平面上の 2D 座標にする。2D はそのまま xy、3D は外周の法線の最も大きい軸を落とす
fn project_to_plane(positions: &[Vector3f], outer: Range<usize>, winding: Winding) -> Vec<Vector2f> {
    if winding == Winding::CounterClockwiseXy {
        return positions.iter().map(|p| Vector2f::new(p.x, p.y)).collect();
    }

    // Newell の方法で法線を求める
    let ring = &positions[outer];
    let normal = ring.iter().enumerate().fold(Vector3f::ZERO, |n, (i, a)| {
        let b = ring[(i + 1) % ring.len()];
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
    edge: usize, // 辺 i は頂点 i → links.next[i]
    helper: usize,
}

// へこみの原因になる頂点に対角線を引き、y 方向に単調な多角形に分ける。各多角形の頂点を反時計回りで返す
// 穴があっても同じ手順で分けられる（穴の頂点も、内部が辺の左になる向きで並んでいるため）
fn monotone_faces(points: &[Vector2f], links: &Links) -> Vec<Vec<usize>> {
    let n = points.len();
    let prev = |i: usize| links.prev[i];
    let next = |i: usize| links.next[i];
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

    trace_faces(points, links, &diagonals)
}

// 外周の辺と対角線でできる面を、内部を左に見ながらたどる
fn trace_faces(points: &[Vector2f], links: &Links, diagonals: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let n = points.len();
    let mut neighbors: Vec<Vec<usize>> = (0..n).map(|i| vec![links.prev[i], links.next[i]]).collect();
    // たどる辺：外周と穴は内部が左になる向きだけ、対角線は両方の向き
    let mut edges: Vec<(usize, usize)> = (0..n).map(|i| (i, links.next[i])).collect();

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
    fn fewer_than_three_vertices_draw_nothing() {
        for count in 1..3 {
            let generated = fill(&points(&[(0.0, 0.0), (1.0, 0.0)][..count]), &[], Winding::CounterClockwiseXy);
            assert!(generated.indices.is_empty() && generated.has_leftover);
        }

        let empty = fill(&[], &[], Winding::CounterClockwiseXy);
        assert!(empty.indices.is_empty() && !empty.has_leftover);
    }

    // 三角形の面積の合計が多角形の面積と一致し、どの三角形も反時計回り
    fn check_polygon(xy: &[(f32, f32)]) {
        let positions = points(xy);
        let projected: Vec<Vector2f> = positions.iter().map(|p| Vector2f::new(p.x, p.y)).collect();
        let indices = fill(&positions, &[], Winding::CounterClockwiseXy).indices;

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
    fn fill_3d_follows_input_winding() {
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

        let forward = fill(&square, &[], Winding::FollowInput).indices;
        let reversed_square: Vec<Vector3f> = square.iter().rev().copied().collect();
        let backward = fill(&reversed_square, &[], Winding::FollowInput).indices;

        assert_eq!(forward.len(), 6);
        // 入力の向きが逆なら、三角形の表も逆になる
        assert!(normal_of(&forward, &square).y > 0.0);
        assert!(normal_of(&backward, &reversed_square).y < 0.0);
    }

    // ---- 穴 ----

    // 左下 (x, y)、一辺 size の正方形。clockwise なら時計回り
    fn square(x: f32, y: f32, size: f32, clockwise: bool) -> Vec<(f32, f32)> {
        let corners = vec![(x, y), (x + size, y), (x + size, y + size), (x, y + size)];
        if clockwise { corners.into_iter().rev().collect() } else { corners }
    }

    // 外周と穴をつなげた頂点と、穴の最初の頂点の位置
    fn with_holes(outer: &[(f32, f32)], holes: &[Vec<(f32, f32)>]) -> (Vec<Vector3f>, Vec<Break>) {
        let mut all = outer.to_vec();
        let mut starts = Vec::new();

        for hole in holes {
            starts.push(Break::Hole(all.len()));
            all.extend(hole);
        }

        (points(&all), starts)
    }

    // 三角形がすべて反時計回りで、面積が expected_area、数が頂点数 + 2 × 穴の数 - 2 で、
    // どの三角形の重心も穴（正方形 (x, y, size)）の外にあることを確かめる
    fn assert_holes_triangulated(positions: &[Vector3f], starts: &[Break], hole_squares: &[(f32, f32, f32)], expected_area: f32) {
        let indices = fill(positions, starts, Winding::CounterClockwiseXy).indices;
        let xy = |i: u32| Vector2f::new(positions[i as usize].x, positions[i as usize].y);

        assert_eq!(indices.len() / 3, positions.len() + 2 * hole_squares.len() - 2);
        assert!((area_of_triangles(positions, &indices) - expected_area).abs() < 1e-3);

        for t in indices.chunks(3) {
            let [a, b, c] = [xy(t[0]), xy(t[1]), xy(t[2])];
            assert!(orient(a, b, c) > 0.0, "{t:?} が反時計回りではありません");

            let center = (a + b + c) / 3.0;
            for &(x, y, size) in hole_squares {
                let inside = center.x > x && center.x < x + size && center.y > y && center.y < y + size;
                assert!(!inside, "{t:?} が穴 ({x}, {y}, {size}) の中にあります");
            }
        }
    }

    #[test]
    fn shapes_split_vertices_at_breaks() {
        // 範囲は (start, end) の組で書く
        let shape = |outer: Range<usize>, holes: &[(usize, usize)]| Shape { outer, holes: holes.iter().map(|&(a, b)| a..b).collect() };

        assert_eq!(shapes(5, &[]), [shape(0..5, &[])]);
        assert_eq!(shapes(10, &[Break::Hole(4), Break::Hole(7)]), [shape(0..4, &[(4, 7), (7, 10)])]);
        // 穴は呼んだ時点の形に属する
        assert_eq!(
            shapes(12, &[Break::Hole(3), Break::Shape(6), Break::Hole(9)]),
            [shape(0..3, &[(3, 6)]), shape(6..9, &[(9, 12)])],
        );
        // 続けて呼んだ空の区切り、頂点数を超える位置
        assert_eq!(shapes(6, &[Break::Shape(3), Break::Shape(3), Break::Hole(9)]), [
            shape(0..3, &[]),
            shape(3..3, &[]),
            shape(3..6, &[(6, 6)]),
        ]);
    }

    #[test]
    fn fill_with_one_hole() {
        // 穴の向きは時計回りでも反時計回りでも同じ結果になる
        for clockwise in [true, false] {
            let (positions, starts) = with_holes(&square(0.0, 0.0, 10.0, false), &[square(4.0, 4.0, 2.0, clockwise)]);
            assert_holes_triangulated(&positions, &starts, &[(4.0, 4.0, 2.0)], 100.0 - 4.0);
        }

        // 外周が時計回りでも、2D は反時計回りの三角形にする
        let (positions, starts) = with_holes(&square(0.0, 0.0, 10.0, true), &[square(4.0, 4.0, 2.0, true)]);
        assert_holes_triangulated(&positions, &starts, &[(4.0, 4.0, 2.0)], 96.0);
    }

    #[test]
    fn concave_polygon_with_holes_on_the_same_rows() {
        // L 字の外周に、同じ高さに並ぶ穴を 2 つ（走査線で同じ y の頂点が多い）
        let outer = [(0.0, 0.0), (20.0, 0.0), (20.0, 8.0), (8.0, 8.0), (8.0, 20.0), (0.0, 20.0)];
        let holes = [(2.0, 2.0, 2.0), (10.0, 2.0, 2.0), (2.0, 12.0, 3.0)];
        let (positions, starts) =
            with_holes(&outer, &holes.iter().map(|&(x, y, size)| square(x, y, size, false)).collect::<Vec<_>>());

        assert_holes_triangulated(&positions, &starts, &holes, 20.0 * 8.0 + 8.0 * 12.0 - 4.0 - 4.0 - 9.0);
    }

    #[test]
    fn grid_of_holes() {
        // 3 × 3 に並んだ穴。行と列で同じ x・y の頂点が並ぶ
        let holes: Vec<(f32, f32, f32)> = (0..3).flat_map(|i| (0..3).map(move |j| (2.0 + i as f32 * 6.0, 2.0 + j as f32 * 6.0, 3.0))).collect();
        let (positions, starts) =
            with_holes(&square(0.0, 0.0, 20.0, false), &holes.iter().map(|&(x, y, size)| square(x, y, size, true)).collect::<Vec<_>>());

        assert_holes_triangulated(&positions, &starts, &holes, 400.0 - 9.0 * 9.0);
    }

    #[test]
    fn degenerate_holes_are_ignored() {
        // 頂点が 2 個の穴・面積のない穴・空の穴は使わない
        let (positions, mut starts) = with_holes(
            &square(0.0, 0.0, 10.0, false),
            &[vec![(1.0, 1.0), (2.0, 2.0)], vec![(3.0, 3.0), (4.0, 4.0), (5.0, 5.0)]],
        );
        starts.push(Break::Hole(positions.len()));
        let indices = fill(&positions, &starts, Winding::CounterClockwiseXy).indices;

        assert_eq!(indices.len(), 6);
        assert!((area_of_triangles(&positions, &indices) - 100.0).abs() < 1e-4);
    }

    // ---- 形の区切り ----

    #[test]
    fn each_shape_is_filled_separately() {
        // 正方形を 2 つ並べ、begin_shape() で区切る。どの三角形も 1 つの形の頂点だけを使う
        let mut corners = square(0.0, 0.0, 1.0, false);
        corners.extend(square(5.0, 0.0, 1.0, false));
        let positions = points(&corners);
        let indices = fill(&positions, &[Break::Shape(4)], Winding::CounterClockwiseXy).indices;

        assert_eq!(indices.len() / 3, 2 * 2);
        assert!(indices.chunks(3).all(|t| t.iter().all(|&i| i < 4) || t.iter().all(|&i| i >= 4)));
        assert!((area_of_triangles(&positions, &indices) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn short_shapes_are_skipped() {
        // 三角形・2 頂点・三角形。2 頂点の形だけ描かない
        let positions = points(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (3.0, 0.0), (4.0, 0.0), (5.0, 0.0), (6.0, 0.0), (5.0, 1.0)]);
        let generated = fill(&positions, &[Break::Shape(3), Break::Shape(5)], Winding::CounterClockwiseXy);
        let mut used: Vec<u32> = generated.indices.clone();
        used.sort_unstable();

        assert_eq!(used, [0, 1, 2, 5, 6, 7]);
        assert!(generated.has_leftover);
    }

    #[test]
    fn fills_with_holes_per_shape() {
        // 穴のある正方形を 2 つ。それぞれの穴は、その形にだけ空く
        let mut corners = square(0.0, 0.0, 10.0, false);
        corners.extend(square(4.0, 4.0, 2.0, true));
        corners.extend(square(20.0, 0.0, 10.0, false));
        corners.extend(square(24.0, 4.0, 2.0, true));
        let positions = points(&corners);
        let breaks = [Break::Hole(4), Break::Shape(8), Break::Hole(12)];
        let indices = fill(&positions, &breaks, Winding::CounterClockwiseXy).indices;

        assert_eq!(indices.len() / 3, 2 * (8 + 2 - 2));
        assert!((area_of_triangles(&positions, &indices) - 2.0 * 96.0).abs() < 1e-3);
        // 形をまたぐ三角形はない
        assert!(indices.chunks(3).all(|t| t.iter().all(|&i| i < 8) || t.iter().all(|&i| i >= 8)));
    }

    #[test]
    fn fill_3d_with_hole_follows_input_winding() {
        // xz 平面（y = 1）の外周と穴。外周の入力の向きで表が決まる
        let outer = [(0.0, 0.0), (0.0, 10.0), (10.0, 10.0), (10.0, 0.0)];
        let hole = [(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)];
        let to_xz = |(x, z): (f32, f32)| Vector3f::new(x, 1.0, z);
        let positions: Vec<Vector3f> = outer.iter().chain(&hole).copied().map(to_xz).collect();
        let indices = fill(&positions, &[Break::Hole(4)], Winding::FollowInput).indices;

        assert_eq!(indices.len() / 3, 8 + 2 - 2);
        for t in indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize]);
            assert!((b - a).cross(c - a).y > 0.0, "{t:?} の表が外周と逆です");
        }
    }
}
