// Sdf / Msdf：fdsm で作る符号付き距離（§13 グリフの作り方・Sdf / Msdf の細部）。msdf feature
// 画素の値 0〜1 が距離 −sdf_range / 2〜+sdf_range / 2（画素、内側が正）を表す

use fdsm::bezier::scanline::FillRule;
use fdsm::bezier::{Point, Segment as FdsmSegment};
use fdsm::correct_error::{ErrorCorrectionConfig, correct_error_mtsdf};
use fdsm::generate::{generate_mtsdf, generate_sdf};
use fdsm::render::{correct_sign_mtsdf, correct_sign_sdf};
use fdsm::shape::{Contour, Shape};
use image::{GrayImage, Rgba32FImage};

use crate::text::bitmap::RasterGlyph;
use crate::text::vector::Segment;

// 辺の色分けで角とみなす角度の sin（fdsm・msdfgen の例と同じ値）
const CORNER_SIN_ALPHA: f64 = 0.03;
// 辺の色分けの乱数の種。同じグリフからは同じ距離の場を作る
const COLORING_SEED: u64 = 0x004c_6f6e_6173;

// 輪郭（フォントの単位、y は上向き）から距離の場を作る。scale はフォントの単位 → 画素
// multi_channel なら MTSDF（4 チャンネル）、そうでなければ SDF（1 チャンネル）
pub(crate) fn generate(segments: &[Segment], scale: f64, range: f64, multi_channel: bool) -> RasterGlyph {
    let Some((min_x, min_y, max_x, max_y)) = bounds(segments) else {
        return RasterGlyph::default();
    };

    // 太らせた境目も収まるよう、距離の幅の半分の余白を付ける
    let margin = (range / 2.0).ceil() as i32;
    let left = (min_x * scale).floor() as i32 - margin;
    let top = (-max_y * scale).floor() as i32 - margin;
    let right = (max_x * scale).ceil() as i32 + margin;
    let bottom = (-min_y * scale).ceil() as i32 + margin;
    let (width, height) = ((right - left) as u32, (bottom - top) as u32);

    // 画素の座標（y は下向き、左上が 0）に直して、fdsm の形にする
    let at = |x: f32, y: f32| Point::new(x as f64 * scale - left as f64, -(y as f64) * scale - top as f64);
    let shape = shape(segments, at);

    if shape.contours.is_empty() {
        return RasterGlyph::default();
    }

    let pixels = if multi_channel {
        let colored = Shape::edge_coloring_simple(shape, CORNER_SIN_ALPHA, COLORING_SEED);
        let prepared = colored.prepare();
        let mut field = Rgba32FImage::new(width, height);

        generate_mtsdf(&prepared, range, &mut field);
        // 重なった輪郭のため、nonzero で符号を補正する（§13 Sdf / Msdf の細部）
        correct_sign_mtsdf(&mut field, &prepared, FillRule::Nonzero);
        correct_error_mtsdf(&mut field, &colored, &prepared, range, &ErrorCorrectionConfig::default());

        field.into_raw().into_iter().map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8).collect()
    } else {
        let prepared = shape.prepare();
        let mut field = GrayImage::new(width, height);

        generate_sdf(&prepared, range, &mut field);
        correct_sign_sdf(&mut field, &prepared, FillRule::Nonzero);

        field.into_raw()
    };

    RasterGlyph { left, top, width, height, bytes_per_pixel: if multi_channel { 4 } else { 1 }, pixels, distance: None }
}

// 制御点を含む外接矩形（フォントの単位）
fn bounds(segments: &[Segment]) -> Option<(f64, f64, f64, f64)> {
    let mut points = Vec::new();

    for &segment in segments {
        match segment {
            Segment::Move(x, y) | Segment::Line(x, y) => points.push((x, y)),
            Segment::Quad(cx, cy, x, y) => points.extend([(cx, cy), (x, y)]),
            Segment::Cubic(cx0, cy0, cx1, cy1, x, y) => points.extend([(cx0, cy0), (cx1, cy1), (x, y)]),
            Segment::Close => {}
        }
    }

    let first = *points.first()?;

    Some(points.iter().fold((first.0 as f64, first.1 as f64, first.0 as f64, first.1 as f64), |(x0, y0, x1, y1), &(x, y)| {
        (x0.min(x as f64), y0.min(y as f64), x1.max(x as f64), y1.max(y as f64))
    }))
}

// 輪郭ごとに fdsm の Contour にする。閉じていない輪郭は始点へ線で閉じる。長さ 0 の線は捨てる
fn shape(segments: &[Segment], at: impl Fn(f32, f32) -> Point) -> Shape<Contour> {
    let mut contours = Vec::new();
    let mut current = Contour::default();
    let mut start = None;
    let mut pen = Point::origin();

    let close = |current: &mut Contour, start: Option<Point>, pen: Point, contours: &mut Vec<Contour>| {
        if let Some(start) = start
            && pen != start
        {
            current.segments.push(FdsmSegment::line(pen, start));
        }
        if !current.segments.is_empty() {
            contours.push(std::mem::take(current));
        }
    };

    for &segment in segments {
        match segment {
            Segment::Move(x, y) => {
                close(&mut current, start, pen, &mut contours);
                pen = at(x, y);
                start = Some(pen);
            }
            Segment::Line(x, y) if start.is_some() => {
                let end = at(x, y);
                if end != pen {
                    current.segments.push(FdsmSegment::line(pen, end));
                }
                pen = end;
            }
            Segment::Quad(cx, cy, x, y) if start.is_some() => {
                let end = at(x, y);
                current.segments.push(FdsmSegment::quad(pen, at(cx, cy), end));
                pen = end;
            }
            Segment::Cubic(cx0, cy0, cx1, cy1, x, y) if start.is_some() => {
                let end = at(x, y);
                current.segments.push(FdsmSegment::cubic(pen, at(cx0, cy0), at(cx1, cy1), end));
                pen = end;
            }
            Segment::Close if start.is_some() => {
                close(&mut current, start, pen, &mut contours);
                start = None;
            }
            _ => {}
        }
    }

    close(&mut current, start, pen, &mut contours);

    Shape { contours }
}
