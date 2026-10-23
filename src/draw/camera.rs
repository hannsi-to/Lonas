// カメラ（§5 Camera の保持・§11 Camera）。DrawManager* が持ち、CameraId で参照する
// 行列の計算は手系・2D の原点（LonasConfig）が必要なため、DrawManager* のメソッドから行う（段階 6 で決定）

use bytemuck::{Pod, Zeroable};

use crate::config::{Handedness, Origin2D};
use crate::math::{Matrix4f, Quaternion, Vector2f, Vector3f};
use crate::memory::ManagerId;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Camera2D {
    pub position: Vector2f,
    pub rotation: f32,
    pub zoom: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ProjectionMode {
    Perspective,
    // 写す範囲の高さ（ワールドの単位）。幅は描くサーフェスの縦横比から求める
    Orthographic { height: f32 },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Camera3D {
    pub position: Vector3f,
    pub rotation: Quaternion,
    pub projection_mode: ProjectionMode,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

// 2D は深度を使わないため、z = 0 が深度の範囲の中に入る値にする
const DEPTH_RANGE_2D: (f32, f32) = (-1.0, 1.0);

impl Camera2D {
    // ビュー行列。拡大と回転は 2D の原点（Origin2D）を中心に行う
    pub(crate) fn view(&self) -> Matrix4f {
        Matrix4f::from_scale(Vector3f::new(self.zoom, self.zoom, 1.0))
            * Matrix4f::from_rotation(Quaternion::from_axis_angle(Vector3f::Z, -self.rotation))
            * Matrix4f::from_translation(-self.position.extend(0.0))
    }

    // 単位はピクセル。zoom = 1.0 で 1 単位が 1 ピクセル（§11 座標系）
    pub(crate) fn projection(origin: Origin2D, viewport_size: Vector2f) -> Matrix4f {
        let (w, h) = (viewport_size.x, viewport_size.y);
        let (left, right, bottom, top) = match origin {
            Origin2D::TopLeft => (0.0, w, h, 0.0),
            Origin2D::BottomLeft => (0.0, w, 0.0, h),
            Origin2D::Center => (-w * 0.5, w * 0.5, -h * 0.5, h * 0.5),
        };
        let (near, far) = DEPTH_RANGE_2D;

        Matrix4f::orthographic_off_center(left, right, bottom, top, near, far, Handedness::RightHanded)
    }
}

impl Camera3D {
    pub(crate) fn view(&self) -> Matrix4f {
        Matrix4f::view(self.position, self.rotation)
    }

    pub(crate) fn projection(&self, handedness: Handedness, viewport_size: Vector2f) -> Matrix4f {
        let aspect_ratio = viewport_size.x / viewport_size.y.max(1.0);

        match self.projection_mode {
            ProjectionMode::Perspective => Matrix4f::perspective(self.fov_y, aspect_ratio, self.near, self.far, handedness),
            ProjectionMode::Orthographic { height } => {
                Matrix4f::orthographic(height * aspect_ratio, height, self.near, self.far, handedness)
            }
        }
    }

    // target の方を向くように rotation を設定する（§11 Camera の look_at）
    pub(crate) fn look_at(&mut self, target: Vector3f, up: Vector3f, handedness: Handedness) {
        self.rotation = Quaternion::look_rotation(target - self.position, up, handedness);
    }
}

// GPU に送るカメラ（WGSL の Camera、224 バイト、§11 シェーダーとの約束事）
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub(crate) struct GpuCamera {
    pub view_projection: [f32; 16],
    pub view_projection_no_jitter: [f32; 16],
    pub view: [f32; 16],
    pub position: [f32; 4],
    pub viewport_size: [f32; 2],
    pub jitter: [f32; 2],
}

const _: () = assert!(size_of::<GpuCamera>() == crate::wgsl::CAMERA_SIZE as usize);

impl GpuCamera {
    // ジッター（TAA）は段階 8 で加える
    pub(crate) fn new(view: Matrix4f, projection: Matrix4f, position: Vector3f, viewport_size: Vector2f) -> Self {
        let view_projection = (projection * view).to_cols_array();

        Self {
            view_projection,
            view_projection_no_jitter: view_projection,
            view: view.to_cols_array(),
            position: position.extend(0.0).to_array(),
            viewport_size: viewport_size.to_array(),
            jitter: [0.0; 2],
        }
    }
}

// 発行元の ManagerId を持ち、別の DrawManager* の CameraId を検知する（§5 カメラの指定）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CameraId {
    pub(crate) manager: ManagerId,
    index: u32,
    generation: u32,
}

pub(crate) struct CameraSet<C> {
    manager: ManagerId,
    cameras: Vec<(u32, Option<C>)>,
    free: Vec<u32>,
}

impl<C> CameraSet<C> {
    // 渡したカメラが既定カメラ（番号 0）になる
    pub(crate) fn new(manager: ManagerId, default_camera: C) -> Self {
        Self { manager, cameras: vec![(0, Some(default_camera))], free: Vec::new() }
    }

    pub(crate) fn default_id(&self) -> CameraId {
        CameraId { manager: self.manager, index: 0, generation: self.cameras[0].0 }
    }

    pub(crate) fn add(&mut self, camera: C) -> CameraId {
        let index = match self.free.pop() {
            Some(index) => {
                self.cameras[index as usize].1 = Some(camera);
                index
            }
            None => {
                self.cameras.push((0, Some(camera)));
                (self.cameras.len() - 1) as u32
            }
        };

        CameraId { manager: self.manager, index, generation: self.cameras[index as usize].0 }
    }

    pub(crate) fn remove(&mut self, id: CameraId) {
        self.check(id);
        assert_ne!(id.index, 0, "既定カメラは削除できません");

        let slot = &mut self.cameras[id.index as usize];
        slot.0 += 1;
        slot.1 = None;
        self.free.push(id.index);
    }

    pub(crate) fn get(&self, id: CameraId) -> &C {
        self.check(id);
        self.cameras[id.index as usize].1.as_ref().unwrap()
    }

    // GPU に送るときの番号（削除したカメラの番号は空きのまま）
    pub(crate) fn slot(&self, id: CameraId) -> u32 {
        self.check(id);
        id.index
    }

    // 番号の順に、カメラ（削除済みは None）をたどる
    pub(crate) fn slots(&self) -> impl Iterator<Item = Option<&C>> {
        self.cameras.iter().map(|(_, camera)| camera.as_ref())
    }

    pub(crate) fn get_mut(&mut self, id: CameraId) -> &mut C {
        self.check(id);
        self.cameras[id.index as usize].1.as_mut().unwrap()
    }

    // 別の DrawManager* の CameraId・削除済みの CameraId は panic（§5・§11）
    pub(crate) fn check(&self, id: CameraId) {
        assert_eq!(id.manager, self.manager, "別の DrawManager の CameraId です");

        let alive = self.cameras.get(id.index as usize).is_some_and(|(generation, camera)| {
            *generation == id.generation && camera.is_some()
        });
        assert!(alive, "削除済みの CameraId です");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_camera_id_is_rejected_after_reuse() {
        let mut cameras = CameraSet::new(ManagerId(0), 1);
        let a = cameras.add(2);

        cameras.remove(a);
        let b = cameras.add(3);
        assert_eq!(*cameras.get(b), 3);
        assert!(std::panic::catch_unwind(|| cameras.check(a)).is_err());
    }

    #[test]
    #[should_panic(expected = "別の DrawManager の CameraId です")]
    fn camera_id_from_other_manager_panics() {
        let first = CameraSet::new(ManagerId(0), 1);
        let second = CameraSet::new(ManagerId(1), 1);

        second.check(first.default_id());
    }
}
