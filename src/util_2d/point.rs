use std::marker::PhantomData;
use image::imageops::interpolate_bilinear;
use log::error;
use std::f32::consts::TAU;
use crate::lonas::{Object2D, PaintType};
use crate::util_2d::check_object_type;
use crate::vertex::Vertex;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PointType {
    Circle { radius: f32 },
    Rect { width: f32, height: f32 },
}

const CIRCLE_TOLERANCE: f32 = 0.25;
const MIN_CIRCLE_SEGMENTS: usize = 8;
const MAX_CIRCLE_SEGMENTS: usize = 128;

pub fn build_point(object2d: &mut Object2D, point_type: PointType, vertex: Vertex) {
    match point_type {
        PointType::Circle { radius } => {
            build_circle(object2d, vertex, radius);
        }
        PointType::Rect { width, height } => {
            build_rect(object2d, vertex, width, height);
        }
    }
}

fn build_rect(object2d: &mut Object2D, vertex: Vertex, width: f32, height: f32) {
    assert!(width.is_finite() && width > 0.0, "点の width は 0 より大きい有限の値にしてください（{width}）");
    assert!(height.is_finite() && height > 0.0, "点の height は 0 より大きい有限の値にしてください（{height}）");

    let Ok(paint_type) = check_object_type(object2d) else {
        return;
    };

    let outline = [
        vertex.translate(-width / 2.0, -height / 2.0, 0.0),
        vertex.translate(width / 2.0, -height / 2.0, 0.0),
        vertex.translate(width / 2.0, height / 2.0, 0.0),
        vertex.translate(-width / 2.0, height / 2.0, 0.0),
    ];

    write_shape(object2d, paint_type, &outline);
}

fn build_circle(object2d: &mut Object2D, vertex: Vertex, radius: f32) {
    assert!(radius.is_finite() && radius > 0.0, "点の radius は 0 より大きい有限の値にしてください（{radius}）");

    let Ok(paint_type) = check_object_type(object2d) else {
        return;
    };

    let segments = circle_segments(radius);
    let outline: Vec<Vertex> = (0..segments)
        .map(|i| {
            let (sin, cos) = (TAU * i as f32 / segments as f32).sin_cos();
            vertex.translate(radius * cos, radius * sin, 0.0)
        })
        .collect();

    write_shape(object2d, paint_type, &outline);
}

fn circle_segments(radius: f32) -> usize {
    if radius <= CIRCLE_TOLERANCE {
        return MIN_CIRCLE_SEGMENTS;
    }

    let step = 2.0 * (1.0 - CIRCLE_TOLERANCE / radius).acos();

    ((TAU / step).ceil() as usize).clamp(MIN_CIRCLE_SEGMENTS, MAX_CIRCLE_SEGMENTS)
}

fn write_shape(object2d: &mut Object2D, paint_type: PaintType, outline: &[Vertex]) {
    let base_vertex = object2d.get_written_vertex_count() as u32;

    for &vertex in outline {
        object2d.vertex(vertex);
    }

    match paint_type {
        PaintType::Fill => {
            for i in 1..outline.len() as u32 - 1 {
                object2d.index(base_vertex);
                object2d.index(base_vertex + i);
                object2d.index(base_vertex + i + 1);
            }
        }
        PaintType::Stroke { strip, .. } => {
            if strip {
                object2d.vertex(outline[0]);
            }
        }
    }
}