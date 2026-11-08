// 同じ DrawManager* を複数のサーフェスに描くときの、サーフェスごとの見え方（§11 同じ DrawManager* を複数のサーフェスに描く）
// 借用を持たない値。end_frame の SurfaceDraw::views に並べる

use std::marker::PhantomData;

use super::camera::{Camera2D, Camera3D, CameraId};
use crate::gpu::DrawManager;
use crate::math::{Matrix4f, Vector2f};
use crate::memory::ManagerId;

// 差し替えるカメラの値（sealed のメソッドが返すため pub。mod view が pub(crate) なので外からは使えない）
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ViewCamera {
    Two(Camera2D),
    Three(Camera3D),
}

// 大きさ full_size の全体のうち、offset から size の範囲だけを映す
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Crop {
    pub full_size: Vector2f,
    pub offset: Vector2f,
    pub size: Vector2f,
}

impl Crop {
    // full_size で作った投影の後に掛け、範囲をクリップ空間の全体へ広げる（中心をずらした視錐台と同じ）
    // 範囲は左上が原点。クリップ空間の y は上向き
    pub(crate) fn matrix(&self) -> Matrix4f {
        let (full, offset, size) = (self.full_size, self.offset, self.size);
        let sx = full.x / size.x;
        let sy = full.y / size.y;
        let tx = (full.x - 2.0 * offset.x - size.x) / size.x;
        let ty = (size.y - full.y + 2.0 * offset.y) / size.y;

        Matrix4f::from_cols_array([
            sx, 0.0, 0.0, 0.0, //
            0.0, sy, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            tx, ty, 0.0, 1.0,
        ])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DrawView {
    pub(crate) manager: ManagerId,
    pub(crate) cameras: Vec<(CameraId, ViewCamera)>,
    pub(crate) crop: Option<Crop>,
}

impl DrawView {
    // カメラを差し替えない
    pub fn new(manager: &(impl DrawManager + ?Sized)) -> Self {
        Self { manager: manager.core_ref().id, cameras: Vec::new(), crop: None }
    }

    pub(crate) fn camera(&self, id: CameraId) -> Option<ViewCamera> {
        self.cameras.iter().find(|(camera, _)| *camera == id).map(|&(_, camera)| camera)
    }
}

// 差し替えられるカメラの型（Camera2D / Camera3D）
pub trait ViewCameraType: view_sealed::Sealed {}

mod view_sealed {
    pub trait Sealed: Copy {
        fn into_view_camera(self) -> super::ViewCamera;
    }
}

impl view_sealed::Sealed for Camera2D {
    fn into_view_camera(self) -> ViewCamera {
        ViewCamera::Two(self)
    }
}

impl view_sealed::Sealed for Camera3D {
    fn into_view_camera(self) -> ViewCamera {
        ViewCamera::Three(self)
    }
}

impl ViewCameraType for Camera2D {}
impl ViewCameraType for Camera3D {}

// DrawManager2D::view() / DrawManager3D::view() が返す。C でカメラの型を決め、2D / 3D の取り違えをコンパイルエラーにする
#[derive(Clone, Debug)]
pub struct DrawViewBuilder<C> {
    view: DrawView,
    _camera: PhantomData<C>,
}

impl<C: ViewCameraType> DrawViewBuilder<C> {
    pub(crate) fn new(view: DrawView) -> Self {
        Self { view, _camera: PhantomData }
    }

    // そのサーフェスでは、id のカメラの代わりに camera を使う
    // 別の DrawManager* の CameraId、同じ CameraId の 2 回目は panic。削除済みかは end_frame で確かめる
    pub fn camera(mut self, id: CameraId, camera: C) -> Self {
        assert_eq!(id.manager, self.view.manager, "別の DrawManager の CameraId です");
        assert!(self.view.camera(id).is_none(), "DrawView で同じ CameraId が 2 回差し替えられました");

        self.view.cameras.push((id, camera.into_view_camera()));
        self
    }

    // 大きさ full_size の全体のうち、offset から size の範囲（左上が原点）だけをこのサーフェスに映す
    pub fn crop(mut self, full_size: Vector2f, offset: Vector2f, size: Vector2f) -> Self {
        assert!(
            full_size.x > 0.0 && full_size.y > 0.0 && size.x > 0.0 && size.y > 0.0,
            "crop の full_size と size は正の値にしてください",
        );

        self.view.crop = Some(Crop { full_size, offset, size });
        self
    }

}

impl<C> From<DrawViewBuilder<C>> for DrawView {
    fn from(builder: DrawViewBuilder<C>) -> Self {
        builder.view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector4f;

    fn ndc(matrix: Matrix4f, x: f32, y: f32) -> (f32, f32) {
        let p = matrix * Vector4f::new(x, y, 0.0, 1.0);
        (p.x / p.w, p.y / p.w)
    }

    #[test]
    fn crop_maps_range_to_full_clip_space() {
        // 全体 400×100 の、左から 2 つ目の 100×100
        let crop = Crop { full_size: Vector2f::new(400.0, 100.0), offset: Vector2f::new(100.0, 0.0), size: Vector2f::new(100.0, 100.0) };
        let m = crop.matrix();
        // 全体のクリップ空間で x = -0.5（左から 100 ピクセル）は、範囲の左端 -1 になる
        let (x, y) = ndc(m, -0.5, 1.0);
        assert!((x + 1.0).abs() < 1e-5 && (y - 1.0).abs() < 1e-5, "{x} {y}");
        let (x, y) = ndc(m, 0.0, -1.0);
        assert!((x - 1.0).abs() < 1e-5 && (y + 1.0).abs() < 1e-5, "{x} {y}");
    }

    #[test]
    fn crop_offset_y_is_from_top() {
        // 全体 100×200 の下半分
        let crop = Crop { full_size: Vector2f::new(100.0, 200.0), offset: Vector2f::new(0.0, 100.0), size: Vector2f::new(100.0, 100.0) };
        let (_, top) = ndc(crop.matrix(), 0.0, 0.0);
        let (_, bottom) = ndc(crop.matrix(), 0.0, -1.0);
        assert!((top - 1.0).abs() < 1e-5 && (bottom + 1.0).abs() < 1e-5, "{top} {bottom}");
    }
}
