use std::f32::consts::PI;
use crate::lonas::{Object2D, PaintType};
use crate::util_2d::check_object_type;
use crate::vertex::Vertex;

pub fn build_equilateral_triangle_radius (
    object2d: &mut Object2D,
    center_vertex: Vertex,
    radius: f32,
) {
    assert!(radius.is_finite() && radius > 0.0, "半径は 0 より大きい有限の値にしてください。({radius})");

    let start_angle = -PI / 2.0;
    let vertices: [Vertex; 3] = std::array::from_fn(|i| {
        let angle = start_angle + 2.0 * PI * i as f32 / 3.0;
        center_vertex.translate(radius * angle.cos(), radius * angle.sin(), 0.0)
    });

    build_triangle_3p(object2d, vertices[0], vertices[1], vertices[2]);
}

pub fn build_equilateral_triangle_side (
    object2d: &mut Object2D,
    center_vertex: Vertex,
    side: f32,
) {
    assert!(side.is_finite() && side > 0.0, "辺の長さは 0 より大きい有限の値にしてください。({side})");

    build_equilateral_triangle_radius(object2d, center_vertex, side / 3.0f32.sqrt());
}

pub fn build_isosceles_triangle (
    object2d: &mut Object2D,
    midpoint_base_vertex: Vertex,
    base: f32,
    side: f32,
) {
    assert!(base.is_finite() && base > 0.0, "底辺の長さは 0 より大きい有限の値にしてください。({base})");
    assert!(side.is_finite() && side > 0.0, "等辺の長さは 0 より大きい有限の値にしてください。({side})");
    let half_base = base / 2.0;
    assert!(side >= base / half_base, "等辺の長さは底辺の半分より大きい値にしてください。(side: {side}, base / 2.0: {half_base})");

    let height = (side * side - half_base * half_base).sqrt();
    build_triangle_3p(
        object2d,
        midpoint_base_vertex.translate(0.0, -height,0.0),
        midpoint_base_vertex.translate(half_base, 0.0,0.0),
        midpoint_base_vertex.translate(-half_base, 0.0,0.0),
    )
}

pub fn build_scalene_triangle (
    object2d: &mut Object2D,
    center_vertex: Vertex,
    side1: f32,
    side2: f32,
    side3: f32,
) {
    for (name, side) in [("side1", side1), ("side2", side2), ("side3", side3)] {
        assert!(side.is_finite() && side > 0.0, "辺の長さは 0 より大きい有限の値にしてください。({name}: {side})");
    }
    assert!(side1 + side2 >= side3 && side1 + side3 >= side2 && side2 + side3 >= side1, "三角形の成立条件(side1 + side2 >= side3 && side1 + side3 >= side2 && side2 + side3 >= side1)を満たしていません");

    let x = (side2 * side2 + side3 * side3 - side1 * side1) / (2.0 * side3);
    let y_squared = side2 * side2 - x * x;
    let y = y_squared.max(0.0).sqrt();

    let centroid_x = side3 / 3.0 + x / 3.0;
    let centroid_y = y / 3.0;

    build_triangle_3p(
        object2d,
        center_vertex.translate(-centroid_x, -centroid_y, 0.0),
        center_vertex.translate(side3 - centroid_x, -centroid_y, 0.0),
        center_vertex.translate(x - centroid_x, y - centroid_y, 0.0),
    )
}

pub fn build_triangle_3p(
    object2d: &mut Object2D,
    p1: Vertex,
    p2: Vertex,
    p3: Vertex,
) {
    let Ok(paint_type) = check_object_type(object2d) else {
        return;
    };

    let base_vertex = object2d.get_written_vertex_count() as u32;
    object2d.vertex(p1);
    object2d.vertex(p2);
    object2d.vertex(p3);
    match paint_type {
        PaintType::Fill => {
            object2d.index(base_vertex);
            object2d.index(base_vertex + 1);
            object2d.index(base_vertex + 2);
        }
        PaintType::Stroke { .. } => {

        }
    }
}