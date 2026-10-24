use crate::lonas::{Object2D, PaintType, Vertex};
use crate::util_2d::check_object_type;

pub fn build_rect_2p_wh(
    object2d: &mut Object2D,
    left_top: Vertex,
    right_bottom: Vertex,
) {
    build_rect_2p(
        object2d,
        left_top,
        left_top.translate(right_bottom.x, right_bottom.y, right_bottom.z),
    )
}

pub fn build_rect_2p(
    object2d: &mut Object2D,
    left_top: Vertex,
    right_bottom: Vertex,
) {
    let delta_z = (right_bottom.z - left_top.z).abs();

    build_rect_4p(
        object2d,
        left_top,
        left_top.set_position(right_bottom.x, left_top.y, left_top.z + delta_z / 2.0),
        right_bottom,
        right_bottom.set_position(left_top.x, right_bottom.y, right_bottom.z - delta_z / 2.0),
    )
}

pub fn build_rect_4p_wh(
    object2d: &mut Object2D,
    left_top: Vertex,
    right_top: Vertex,
    right_bottom: Vertex,
    left_bottom: Vertex,
) {
    build_rect_4p(
        object2d,
        left_top,
        right_top.translate(left_top.x, 0.0, left_top.z),
        right_bottom.translate(left_top.x, left_top.y, left_top.z),
        left_bottom.translate(0.0, left_top.y, left_top.z)
    )
}

pub fn build_rect_4p(
    object2d: &mut Object2D,
    left_top: Vertex,
    right_top: Vertex,
    right_bottom: Vertex,
    left_bottom: Vertex,
) {
    let Ok(paint_type) = check_object_type(object2d) else {
        return;
    };

    let base_vertex = object2d.get_written_vertex_count() as u32;
    object2d.vertex(left_top);
    object2d.vertex(right_top);
    object2d.vertex(right_bottom);
    object2d.vertex(left_bottom);
    match paint_type {
        PaintType::Fill => {
            object2d.index(base_vertex + 0);
            object2d.index(base_vertex + 1);
            object2d.index(base_vertex + 2);
            object2d.index(base_vertex + 2);
            object2d.index(base_vertex + 3);
            object2d.index(base_vertex + 0);
        }
        PaintType::Stroke { .. } => {

        }
    }
}