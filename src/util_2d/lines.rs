// 制御点から線（曲線）を作り、Object2D に 1 つの形として書く補助
// begin〜end の間で呼ぶ。色・UV・法線は、位置と同じ重みで制御点の値を補間する
// Stroke は線にする（開いた折れ線にするには strip: true。strip: false では始点と終点を結んで閉じる）。Fill は線で囲んだ形を塗る
// begin_shape() は呼ばない（形の区切りは呼び出す側で行う）

use crate::lonas::{Object2D, Vector3f, Vertex};
use crate::util_2d::{check_object_type, combine, lerp_vertex, write_outline};

#[derive(Clone, Copy)]
#[derive(PartialEq)]
#[derive(Debug, Default)]
pub enum CurveRepresentationType {
    /// 曲げない。置かれた制御点をそのまま結ぶ折れ線。
    /// 刻まないので `segment_count` は効かない。
    #[default]
    Normal,
    /// 制御点全部で 1 本のベジェ曲線。両端だけを通る。
    BezierCurve,
    /// 一様な B スプライン。`degree` で滑らかさが決まる。両端だけを通る。
    BSpline,
    /// 重み付きの B スプライン。`point_weighted` で重みを指定する。
    NURBS,
    /// 制御点をすべて通る。`tension` で張りを決める。
    CatmullRomSpline,
    /// 区間ごとの 3 次エルミート。接線は `point_tangent` で指定できる。
    HermiteCurve,
    /// 制御点をすべて通る 1 本の多項式（ラグランジュ補間）。
    /// 点を増やすと端が激しく波打つ。
    PolynomialCurve,
    /// 曲率が長さに比例して変わるクロソイド曲線。
    /// 最初の点から、2 番目の点の向きに、折れ線の総長ぶん伸びる。
    /// 形は `curvature` と `curvature_rate` で決まる。
    ClothoidCurve,
}

/// 曲線の作り方。`CurveData::new(種類)` から、必要な値だけを変える。
#[derive(Clone, Debug, PartialEq)]
pub struct CurveData {
    curve_representation_type: CurveRepresentationType,
    segment_count: u32,
    degree: u32,
    point_weighted: Vec<f32>,
    tension: f32,
    point_tangent: Vec<Vector3f>,
    curvature: f32,
    curvature_rate: f32,
}

impl CurveData {
    pub fn new(curve_representation_type: CurveRepresentationType) -> Self {
        Self {
            curve_representation_type,
            segment_count: 32,
            degree: 3,
            point_weighted: Vec::new(),
            tension: 0.0,
            point_tangent: Vec::new(),
            curvature: 0.0,
            curvature_rate: 0.0,
        }
    }

    /// 曲線全体を分ける数（既定 32）。区間ごとの曲線（CatmullRomSpline・HermiteCurve）は区間の数で等分する。
    pub fn segment_count(mut self, segment_count: u32) -> Self {
        self.segment_count = segment_count;
        self
    }

    /// B スプライン・NURBS の次数（既定 3）。制御点の数 - 1 より大きければ、制御点の数 - 1 にする。
    pub fn degree(mut self, degree: u32) -> Self {
        self.degree = degree;
        self
    }

    /// NURBS の制御点ごとの重み（0 より大きい値）。空なら全部 1。空でなければ制御点と同じ数にする。
    pub fn point_weighted(mut self, point_weighted: Vec<f32>) -> Self {
        self.point_weighted = point_weighted;
        self
    }

    /// Catmull-Rom の張り（既定 0）。0 で Catmull-Rom、1 で制御点を直線で結ぶ形に近づく。
    pub fn tension(mut self, tension: f32) -> Self {
        self.tension = tension;
        self
    }

    /// エルミート曲線の制御点ごとの接線（位置の変化の向きと大きさ）。空なら Catmull-Rom と同じ接線を使う。空でなければ制御点と同じ数にする。
    pub fn point_tangent(mut self, point_tangent: Vec<Vector3f>) -> Self {
        self.point_tangent = point_tangent;
        self
    }

    /// クロソイド曲線の始点の曲率（既定 0）。正なら xy 平面で反時計回りに曲がる。
    pub fn curvature(mut self, curvature: f32) -> Self {
        self.curvature = curvature;
        self
    }

    /// クロソイド曲線の、長さあたりの曲率の変化（既定 0）。
    pub fn curvature_rate(mut self, curvature_rate: f32) -> Self {
        self.curvature_rate = curvature_rate;
        self
    }

    fn validate(&self, point_count: usize) {
        assert!(self.segment_count >= 1, "segment_count は 1 以上にしてください。({})", self.segment_count);
        assert!(self.degree >= 1, "degree は 1 以上にしてください。({})", self.degree);
        assert!(self.tension.is_finite(), "tension は有限の値にしてください。({})", self.tension);
        assert!(
            self.curvature.is_finite() && self.curvature_rate.is_finite(),
            "curvature・curvature_rate は有限の値にしてください。({}, {})",
            self.curvature,
            self.curvature_rate,
        );
        assert!(
            self.point_weighted.is_empty() || self.point_weighted.len() == point_count,
            "point_weighted は空か、制御点と同じ数にしてください。({} 個、制御点 {} 個)",
            self.point_weighted.len(),
            point_count,
        );
        assert!(
            self.point_weighted.iter().all(|w| w.is_finite() && *w > 0.0),
            "point_weighted の重みは 0 より大きい有限の値にしてください。",
        );
        assert!(
            self.point_tangent.is_empty() || self.point_tangent.len() == point_count,
            "point_tangent は空か、制御点と同じ数にしてください。({} 個、制御点 {} 個)",
            self.point_tangent.len(),
            point_count,
        );
    }
}

/// 既定の値（`CurveData::new`）で線を書く。
pub fn build_line(
    object2d: &mut Object2D,
    vertices: &[Vertex],
    curve_representation_type: CurveRepresentationType,
) {
    build_curve(object2d, vertices, &CurveData::new(curve_representation_type));
}

/// `curve_data` の作り方で線を書く。制御点が 2 個未満なら何も書かない。
pub fn build_curve(object2d: &mut Object2D, vertices: &[Vertex], curve_data: &CurveData) {
    curve_data.validate(vertices.len());

    // PaintType が設定されていなければ書かない
    if check_object_type(object2d).is_err() || vertices.len() < 2 {
        return;
    }

    write_outline(object2d, &sample_curve(vertices, curve_data));
}

// 曲線の上の点を、始点から終点の順に返す
pub(crate) fn sample_curve(vertices: &[Vertex], data: &CurveData) -> Vec<Vertex> {
    let count = vertices.len();
    let segments = data.segment_count as usize;

    if count < 2 {
        return vertices.to_vec();
    }

    let parameters = |count: usize| (0..=count).map(move |i| i as f32 / count as f32);

    match data.curve_representation_type {
        CurveRepresentationType::Normal => vertices.to_vec(),
        CurveRepresentationType::BezierCurve => parameters(segments).map(|t| de_casteljau(vertices, t)).collect(),
        CurveRepresentationType::BSpline => {
            parameters(segments).map(|t| b_spline(vertices, &[], data.degree as usize, t)).collect()
        }
        CurveRepresentationType::NURBS => {
            parameters(segments).map(|t| b_spline(vertices, &data.point_weighted, data.degree as usize, t)).collect()
        }
        CurveRepresentationType::CatmullRomSpline => piecewise(count, segments, |i, t| cardinal(vertices, i, t, data.tension)),
        CurveRepresentationType::HermiteCurve => piecewise(count, segments, |i, t| hermite(vertices, &data.point_tangent, i, t)),
        CurveRepresentationType::PolynomialCurve => parameters(segments).map(|t| lagrange(vertices, t)).collect(),
        CurveRepresentationType::ClothoidCurve => clothoid(vertices, segments, data.curvature, data.curvature_rate),
    }
}

// 区間ごとの曲線。区間 i（制御点 i〜i + 1）の t（0〜1）の点を evaluate で求め、全体の分割数を区間の数で等分する
fn piecewise(count: usize, segments: usize, evaluate: impl Fn(usize, f32) -> Vertex) -> Vec<Vertex> {
    let intervals = count - 1;
    let steps = segments.div_ceil(intervals).max(1);
    let mut points: Vec<Vertex> =
        (0..intervals).flat_map(|i| (0..steps).map(move |step| (i, step as f32 / steps as f32))).map(|(i, t)| evaluate(i, t)).collect();

    points.push(evaluate(intervals - 1, 1.0));
    points
}

fn de_casteljau(vertices: &[Vertex], t: f32) -> Vertex {
    let mut points = vertices.to_vec();

    for level in (1..points.len()).rev() {
        for i in 0..level {
            points[i] = lerp_vertex(&points[i], &points[i + 1], t);
        }
    }

    points[0]
}

// 端を通る（clamped）一様な B スプライン。weights が空でなければ NURBS
fn b_spline(vertices: &[Vertex], weights: &[f32], degree: usize, t: f32) -> Vertex {
    let count = vertices.len();
    let p = degree.min(count - 1);
    // ノット：両端に p + 1 個ずつ 0 と 1、間は等間隔
    let knots: Vec<f32> = (0..count + p + 1)
        .map(|j| {
            if j <= p {
                0.0
            } else if j >= count {
                1.0
            } else {
                (j - p) as f32 / (count - p) as f32
            }
        })
        .collect();

    // Cox-de Boor の漸化式。t = 1 は最後の区間に含める
    let mut basis: Vec<f32> = (0..knots.len() - 1)
        .map(|i| {
            let inside = knots[i] <= t && t < knots[i + 1];
            let last = t >= 1.0 && knots[i] < knots[i + 1] && knots[i + 1] >= 1.0;
            if inside || last { 1.0 } else { 0.0 }
        })
        .collect();
    let ratio = |numerator: f32, denominator: f32| if denominator.abs() <= f32::EPSILON { 0.0 } else { numerator / denominator };

    for k in 1..=p {
        basis = (0..basis.len() - 1)
            .map(|i| {
                ratio(t - knots[i], knots[i + k] - knots[i]) * basis[i]
                    + ratio(knots[i + k + 1] - t, knots[i + k + 1] - knots[i + 1]) * basis[i + 1]
            })
            .collect();
    }

    let mut combined: Vec<f32> = basis.iter().enumerate().map(|(i, n)| n * weights.get(i).copied().unwrap_or(1.0)).collect();
    let sum: f32 = combined.iter().sum();
    for w in &mut combined {
        *w /= sum;
    }

    combine(vertices, &combined)
}

// 3 次エルミートの基底
fn hermite_basis(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2]
}

// カーディナルスプライン（tension 0 で Catmull-Rom）。接線 m_i = (1 - tension) / 2 × (P_{i+1} - P_{i-1})。端は端の点を繰り返す
// 接線も制御点の差のため、4 つの制御点の重み付きの和（重みの合計は 1）になり、色なども同じ重みで補間する
fn cardinal(vertices: &[Vertex], i: usize, t: f32, tension: f32) -> Vertex {
    let last = vertices.len() - 1;
    let at = |index: isize| vertices[index.clamp(0, last as isize) as usize];
    let s = (1.0 - tension) / 2.0;
    let [h00, h10, h01, h11] = hermite_basis(t);
    let i = i as isize;

    combine(&[at(i - 1), at(i), at(i + 1), at(i + 2)], &[-h10 * s, h00 - h11 * s, h01 + h10 * s, h11 * s])
}

// 3 次エルミート。接線は位置だけに使い、色などは区間の両端の値を h00・h01 の重みで補間する
fn hermite(vertices: &[Vertex], tangents: &[Vector3f], i: usize, t: f32) -> Vertex {
    let last = vertices.len() - 1;
    let position = |v: &Vertex| Vector3f::new(v.x, v.y, v.z);
    let tangent = |index: usize| match tangents.get(index) {
        Some(&tangent) => tangent,
        // 指定がなければ Catmull-Rom と同じ接線
        None => (position(&vertices[(index + 1).min(last)]) - position(&vertices[index.saturating_sub(1)])) * 0.5,
    };
    let [h00, h10, h01, h11] = hermite_basis(t);
    let p = position(&vertices[i]) * h00 + tangent(i) * h10 + position(&vertices[i + 1]) * h01 + tangent(i + 1) * h11;

    combine(&[vertices[i], vertices[i + 1]], &[h00, h01]).set_position(p.x, p.y, p.z)
}

// ラグランジュ補間。制御点 j を t_j = j / (制御点の数 - 1) で通る
fn lagrange(vertices: &[Vertex], t: f32) -> Vertex {
    let last = (vertices.len() - 1) as f32;
    let node = |j: usize| j as f32 / last;
    let weights: Vec<f32> = (0..vertices.len())
        .map(|j| (0..vertices.len()).filter(|&k| k != j).map(|k| (t - node(k)) / (node(j) - node(k))).product())
        .collect();

    combine(vertices, &weights)
}

// クロソイド曲線。最初の点から 2 番目の点の向きに、制御点の折れ線の総長ぶん伸ばす（xy 平面）
// 曲率 κ(s) = curvature + curvature_rate × s。z と色などは、制御点の折れ線を同じ長さだけ進んだ位置の値を使う
fn clothoid(vertices: &[Vertex], segments: usize, curvature: f32, curvature_rate: f32) -> Vec<Vertex> {
    // 長さの累積（xy 平面）
    let mut distances = vec![0.0f32];
    for pair in vertices.windows(2) {
        let length = ((pair[1].x - pair[0].x).powi(2) + (pair[1].y - pair[0].y).powi(2)).sqrt();
        distances.push(distances[distances.len() - 1] + length);
    }

    let total = distances[distances.len() - 1];
    let Some(direction) = vertices.windows(2).find(|pair| (pair[1].x - pair[0].x).abs() + (pair[1].y - pair[0].y).abs() > 0.0) else {
        // すべて同じ位置なら曲げられない
        return vertices.to_vec();
    };
    let heading = (direction[1].y - direction[0].y).atan2(direction[1].x - direction[0].x);
    let angle = |s: f32| heading + curvature * s + 0.5 * curvature_rate * s * s;
    // 折れ線の長さ s の位置の頂点（z・色など）
    let along = |s: f32| {
        let i = distances.partition_point(|&d| d <= s).clamp(1, distances.len() - 1) - 1;
        let length = distances[i + 1] - distances[i];
        let t = if length > 0.0 { ((s - distances[i]) / length).clamp(0.0, 1.0) } else { 0.0 };
        lerp_vertex(&vertices[i], &vertices[i + 1], t)
    };

    // 1 区間を細かく分けた中点法で、向きを積分する
    const SUBSTEPS: usize = 8;
    let step = total / segments as f32;
    let (mut x, mut y) = (vertices[0].x, vertices[0].y);
    let mut points = vec![vertices[0]];

    for i in 0..segments {
        for sub in 0..SUBSTEPS {
            let s = step * (i as f32 + (sub as f32 + 0.5) / SUBSTEPS as f32);
            let (sin, cos) = angle(s).sin_cos();
            x += cos * step / SUBSTEPS as f32;
            y += sin * step / SUBSTEPS as f32;
        }

        let base = along(step * (i + 1) as f32);
        points.push(base.set_position(x, y, base.z));
    }

    points
}
