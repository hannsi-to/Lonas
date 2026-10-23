use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::vertex::Vertex;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ManagerId(u32);

// device / queue / surface / GPU メモリのヒープ。2D / 3D の DrawManager で共有し、フレーム境界を持つ
pub struct GpuContext {
    in_frame: Arc<AtomicBool>,
    next_manager_id: u32,
}

impl GpuContext {
    pub fn new() -> Self {
        Self {
            in_frame: Arc::new(AtomicBool::new(false)),
            next_manager_id: 0,
        }
    }

    fn issue_manager_id(&mut self) -> ManagerId {
        let id = ManagerId(self.next_manager_id);
        self.next_manager_id += 1;

        id
    }

    // 描画先の取得
    pub fn begin_frame(&mut self) -> &mut Self {
        if self.in_frame.swap(true, Ordering::Relaxed) {
            panic!("begin_frame が二重に呼ばれました（end_frame の呼び忘れ）");
        }

        self
    }

    // DrawManager ごとに要求の処理 → カメラ行列の転送 → 並べ替え → 実行 → コマンドのクリア。最後に submit と present
    // 渡した順に描画する
    pub fn end_frame(&mut self, draw_managers: &mut [&mut dyn DrawManager]) -> &mut Self {
        if !self.in_frame.load(Ordering::Relaxed) {
            panic!("begin_frame の前に end_frame が呼ばれました");
        }

        for draw_manager in draw_managers.iter_mut() {
            draw_manager.core_mut().clear_commands();
        }

        self.in_frame.store(false, Ordering::Relaxed);

        self
    }
}

mod sealed {
    pub trait Sealed {
        fn core_mut(&mut self) -> &mut super::DrawManagerCore;
    }
}

// GpuContext::end_frame に 2D / 3D の DrawManager を混ぜて渡すためのトレイト
pub trait DrawManager: sealed::Sealed {}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CameraId {
    manager_id: ManagerId,
    index: usize,
}

// DrawManager2D / DrawManager3D の共通部分
pub struct DrawManagerCore {
    id: ManagerId,
    cameras: Vec<Camera>,
    in_frame: Arc<AtomicBool>,
}

impl DrawManagerCore {
    // 渡したカメラが既定カメラになる
    fn new(gpu: &mut GpuContext, camera: Camera) -> Self {
        Self {
            id: gpu.issue_manager_id(),
            cameras: vec![camera],
            in_frame: Arc::clone(&gpu.in_frame),
        }
    }

    fn default_camera_id(&self) -> CameraId {
        CameraId {
            manager_id: self.id,
            index: 0,
        }
    }

    fn add_camera(&mut self, camera: Camera) -> CameraId {
        self.cameras.push(camera);

        CameraId {
            manager_id: self.id,
            index: self.cameras.len() - 1,
        }
    }

    fn check_camera_id(&self, id: CameraId) {
        if id.manager_id != self.id {
            panic!("別の DrawManager の CameraId が使われました");
        }
    }

    fn camera(&self, id: CameraId) -> &Camera {
        self.check_camera_id(id);

        &self.cameras[id.index]
    }

    fn camera_mut(&mut self, id: CameraId) -> &mut Camera {
        self.check_camera_id(id);

        &mut self.cameras[id.index]
    }

    fn clear_commands(&mut self) {

    }
}

macro_rules! impl_draw_manager {
    ($name:ident) => {
        pub struct $name {
            core: DrawManagerCore,
        }

        impl $name {
            pub fn new(gpu: &mut GpuContext, camera: Camera) -> Self {
                Self {
                    core: DrawManagerCore::new(gpu, camera),
                }
            }

            pub fn default_camera_id(&self) -> CameraId {
                self.core.default_camera_id()
            }

            pub fn add_camera(&mut self, camera: Camera) -> CameraId {
                self.core.add_camera(camera)
            }

            pub fn camera(&self, id: CameraId) -> &Camera {
                self.core.camera(id)
            }

            pub fn camera_mut(&mut self, id: CameraId) -> &mut Camera {
                self.core.camera_mut(id)
            }
        }

        impl sealed::Sealed for $name {
            fn core_mut(&mut self) -> &mut DrawManagerCore {
                &mut self.core
            }
        }

        impl DrawManager for $name {}
    };
}

impl_draw_manager!(DrawManager2D);
impl_draw_manager!(DrawManager3D);

// Static はメモリ空間の手前にスタック方式、Dynamic は後ろに Free List Allocator 方式で確保する
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemoryUpdateType {
    // 確保した領域のサイズが変わる変更は不可
    Static,
    // ring_count 個の領域をフレームごとに切り替えて使う
    Dynamic { ring_count: u32 },
}

// Object2D / Object3D の共通部分
struct ObjectCore {
    instances: Vec<Instance>,
    shader: Shader,
    camera_id: Option<CameraId>, // None なら DrawManager の既定カメラ
    manager_id: Option<ManagerId>, // 初回 end で紐づいた DrawManager
    memory_update_type: Option<MemoryUpdateType>, // 既定値なし。未指定のまま end すると panic
    vertices: Vec<Vertex>,
    cursor: usize,
    vertices_dirty: bool,
}

impl ObjectCore {
    fn new() -> Self {
        Self {
            instances: Vec::new(),
            shader: Shader::default(),
            camera_id: None,
            manager_id: None,
            memory_update_type: None,
            vertices: Vec::new(),
            cursor: 0,
            vertices_dirty: false,
        }
    }

    fn begin(&mut self) {
        self.cursor = 0;
    }

    // 前フレームの値と比較しながら書き込み、差分があれば dirty にする
    #[inline]
    fn write_vertex(&mut self, vertex: Vertex) {
        match self.vertices.get_mut(self.cursor) {
            Some(current) => {
                if *current != vertex {
                    *current = vertex;
                    self.vertices_dirty = true;
                }
            }
            None => {
                self.vertices.push(vertex);
                self.vertices_dirty = true;
            }
        }
        self.cursor += 1;
    }

    fn end(&mut self, draw_manager: &mut DrawManagerCore) {
        if !draw_manager.in_frame.load(Ordering::Relaxed) {
            panic!("フレーム外（begin_frame の前 / end_frame の後）で end が呼ばれました");
        }

        if self.memory_update_type.is_none() {
            panic!("memory_update_type() を呼ばずに end が呼ばれました");
        }

        match self.manager_id {
            Some(manager_id) if manager_id != draw_manager.id => {
                panic!("Object が初回とは別の DrawManager の end に渡されました");
            }
            Some(_) => {}
            None => self.manager_id = Some(draw_manager.id),
        }

        if let Some(camera_id) = self.camera_id {
            draw_manager.check_camera_id(camera_id);
        }

        // 頂点数が減った場合
        if self.cursor != self.vertices.len() {
            self.vertices.truncate(self.cursor);
            self.vertices_dirty = true;
        }
    }
}

macro_rules! impl_object {
    ($name:ident, $draw_manager:ident) => {
        pub struct $name {
            core: ObjectCore,
        }

        impl $name {
            pub fn new() -> Self {
                Self {
                    core: ObjectCore::new(),
                }
            }

            pub fn begin(&mut self) -> &mut Self {
                self.core.begin();

                self
            }

            pub fn vertex(&mut self, x: f32, y: f32, z: f32) -> &mut Self {
                self.core.write_vertex(Vertex::new_position(x, y, z));

                self
            }

            pub fn vertex_color(&mut self, x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32) -> &mut Self {
                self.core.write_vertex(Vertex::new_position_color(x, y, z, r, g, b, a));

                self
            }

            pub fn vertex_color_uv(&mut self, x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32) -> &mut Self {
                self.core.write_vertex(Vertex::new_position_color_uv(x, y, z, r, g, b, a, u, v));

                self
            }

            pub fn vertex_color_uv_normal(&mut self, x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32, n_x: f32, n_y: f32, n_z: f32) -> &mut Self {
                self.core.write_vertex(Vertex::new_position_color_uv_normal(x, y, z, r, g, b, a, u, v, n_x, n_y, n_z));

                self
            }

            pub fn end(&mut self, draw_manager: &mut $draw_manager) -> &mut Self {
                self.core.end(&mut draw_manager.core);

                self
            }

            pub fn camera(&mut self, id: CameraId) -> &mut Self {
                self.core.camera_id = Some(id);

                self
            }

            // 領域の確保後に呼ばれた場合は、新しい側の領域へコピーする
            pub fn memory_update_type(&mut self, memory_update_type: MemoryUpdateType) -> &mut Self {
                if let MemoryUpdateType::Dynamic { ring_count: 0 } = memory_update_type {
                    panic!("ring_count には 1 以上を指定してください");
                }
                self.core.memory_update_type = Some(memory_update_type);

                self
            }

            pub fn shader(&mut self, shader: Shader) -> &mut Self {
                self.core.shader = shader;

                self
            }

            pub fn add_instance(&mut self, instance: Instance) -> &mut Self {
                self.core.instances.push(instance);

                self
            }
        }
    };
}

impl_object!(Object2D, DrawManager2D);
impl_object!(Object3D, DrawManager3D);

#[derive(Default)]
pub struct Camera {

}

#[derive(Default)]
pub struct Instance {
    pub transform: Matrix4f,
}

#[derive(Default)]
pub struct Matrix4f {

}

#[derive(Default)]
pub struct Shader {
    vertex_shader: VertexShader,
    fragment_shader: FragmentShader,
}

#[derive(Default)]
pub struct VertexShader {

}

#[derive(Default)]
pub struct FragmentShader {

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_runs() {
        let mut gpu = GpuContext::new();
        let mut draw_manager_2d = DrawManager2D::new(&mut gpu, Camera::default());
        let mut draw_manager_3d = DrawManager3D::new(&mut gpu, Camera::default());
        let mut object = Object2D::new();
        object.memory_update_type(MemoryUpdateType::Dynamic { ring_count: 1 });

        for _ in 0..2 {
            gpu.begin_frame();
            object.begin().vertex(0.0, 0.0, 0.0).end(&mut draw_manager_2d);
            gpu.end_frame(&mut [&mut draw_manager_3d, &mut draw_manager_2d]);
        }
    }

    #[test]
    #[should_panic(expected = "フレーム外")]
    fn end_outside_frame_panics() {
        let mut gpu = GpuContext::new();
        let mut draw_manager = DrawManager2D::new(&mut gpu, Camera::default());
        Object2D::new().begin().end(&mut draw_manager);
    }

    #[test]
    #[should_panic(expected = "別の DrawManager の end")]
    fn other_draw_manager_panics() {
        let mut gpu = GpuContext::new();
        let mut draw_manager_a = DrawManager2D::new(&mut gpu, Camera::default());
        let mut draw_manager_b = DrawManager2D::new(&mut gpu, Camera::default());
        let mut object = Object2D::new();
        object.memory_update_type(MemoryUpdateType::Dynamic { ring_count: 1 });

        gpu.begin_frame();
        object.begin().end(&mut draw_manager_a);
        object.begin().end(&mut draw_manager_b);
    }

    #[test]
    #[should_panic(expected = "CameraId")]
    fn other_camera_id_panics() {
        let mut gpu = GpuContext::new();
        let mut draw_manager_a = DrawManager2D::new(&mut gpu, Camera::default());
        let draw_manager_b = DrawManager2D::new(&mut gpu, Camera::default());

        gpu.begin_frame();
        Object2D::new().memory_update_type(MemoryUpdateType::Static).camera(draw_manager_b.default_camera_id()).begin().end(&mut draw_manager_a);
    }

    #[test]
    #[should_panic(expected = "memory_update_type()")]
    fn end_without_memory_update_type_panics() {
        let mut gpu = GpuContext::new();
        let mut draw_manager = DrawManager2D::new(&mut gpu, Camera::default());

        gpu.begin_frame();
        Object2D::new().begin().end(&mut draw_manager);
    }
}
