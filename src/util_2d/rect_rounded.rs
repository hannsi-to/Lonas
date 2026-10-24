// 角を丸めた（切り落とした・えぐった）四角形を Object2D に 1 つの形として書く補助
// begin〜end の間で呼ぶ。角の形は Corner の corner_type、角に作る頂点の色などは Corner の vertex のものを使う
// 角の形の radius_x は上下の辺（left_top〜right_top・right_bottom〜left_bottom）に沿った長さ、radius_y は左右の辺に沿った長さ
// begin_shape() は呼ばない（形の区切りは呼び出す側で行う）

use crate::lonas::Object2D;
use crate::util_2d::polygon_rounded::build_polygon_rounded;
use crate::util_2d::Corner;

// right_bottom の位置は left_top からの差（幅, 高さ, 奥行き）
pub fn build_rect_rounded_2p_wh(
    object2d: &mut Object2D,
    left_top: Corner,
    right_bottom: Corner,
) {
    build_rect_rounded_2p(object2d, left_top, offset_from(left_top, right_bottom))
}

// 2 つの角から四角形を書く。right_top は left_top の、left_bottom は right_bottom の角の形と色などを使う
// right_top・left_bottom の z は、left_top と right_bottom の z の中間
pub fn build_rect_rounded_2p(
    object2d: &mut Object2D,
    left_top: Corner,
    right_bottom: Corner,
) {
    let (lt, rb) = (left_top.vertex, right_bottom.vertex);
    let middle_z = (lt.z + rb.z) / 2.0;
    let right_top = Corner { vertex: lt.set_position(rb.x, lt.y, middle_z), ..left_top };
    let left_bottom = Corner { vertex: rb.set_position(lt.x, rb.y, middle_z), ..right_bottom };

    build_rect_rounded_4p(
        object2d,
        left_top,
        right_top,
        right_bottom,
        left_bottom,
    )
}

// right_top・right_bottom・left_bottom の位置は left_top からの差
// （例：right_top = (幅, 0)、right_bottom = (幅, 高さ)、left_bottom = (0, 高さ)）
pub fn build_rect_rounded_4p_wh(
    object2d: &mut Object2D,
    left_top: Corner,
    right_top: Corner,
    right_bottom: Corner,
    left_bottom: Corner,
) {
    build_rect_rounded_4p(
        object2d,
        left_top,
        offset_from(left_top, right_top),
        offset_from(left_top, right_bottom),
        offset_from(left_top, left_bottom),
    )
}

// 4 つの角から四角形を書く。left_top → right_top → right_bottom → left_bottom の順につなぐ
pub fn build_rect_rounded_4p(
    object2d: &mut Object2D,
    left_top: Corner,
    right_top: Corner,
    right_bottom: Corner,
    left_bottom: Corner,
) {
    // build_polygon_rounded の radius_x は前の角へ向かう辺に沿った長さのため、
    // 前の辺が縦の辺になる left_top と right_bottom は入れ替え、radius_x が上下の辺に沿うようにする
    let swapped = |corner: Corner| Corner { corner_type: corner.corner_type.swapped_xy(), ..corner };

    build_polygon_rounded(object2d, &[swapped(left_top), right_top, swapped(right_bottom), left_bottom]);
}

// corner の位置を origin からの差として、実際の位置にする。色などは corner のもの
fn offset_from(origin: Corner, corner: Corner) -> Corner {
    let (o, c) = (origin.vertex, corner.vertex);

    Corner { vertex: c.set_position(o.x + c.x, o.y + c.y, o.z + c.z), ..corner }
}
