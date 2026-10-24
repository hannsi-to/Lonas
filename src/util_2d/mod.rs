use log::error;
use crate::lonas::PaintType;
use crate::object::Object2D;

pub mod point;
pub mod triangle;

pub(crate) fn check_object_type(object2d: &Object2D) -> Result<PaintType, ()> {
    object2d.get_paint_type().ok_or_else(|| {
        error!(
            "引数に渡されたオブジェクトの PaintType が設定されていません。処理を飛ばします。"
        );
    })
}