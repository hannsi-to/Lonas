// カメラ（§5 Camera の保持・§11 Camera）。DrawManager* が持ち、CameraId で参照する
// 行列の計算（view_projection・look_at）は段階 6 で作る

use crate::math::{Quaternion, Vector2f, Vector3f};
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
