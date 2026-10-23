// Object2D / Object3D（§1・§4・§5・§7・§10）
// 内部は ObjectCore で共通化し、型ごとの差はインスタンスの型・end に渡せる DrawManager* の型・型固有の設定のみ

pub(crate) mod core;
pub(crate) mod instance_slots;
#[cfg(test)]
mod tests;
pub(crate) mod topology;

pub use instance_slots::InstanceId;
pub use topology::Topology;

use crate::config::{Blend, CullMode};
use crate::draw::{CameraId, DrawManager2D, DrawManager3D};
use crate::gpu::GpuContext;
use crate::instance::{Instance2D, Instance3D};
use crate::math::{Vector2f, Vector3f};
use crate::memory::MemoryUpdateType;
use crate::resource::{ShaderId, TextureId};
use crate::vertex::Vertex;
use core::{EndContext, ObjectCore, VertexElements};
use topology::Winding;

// 2D / 3D で共通のメソッド
macro_rules! object_common {
    ($name:ident, $instance:ty, $draw_manager:ty) => {
        impl $name {
            pub fn begin(&mut self) -> &mut Self {
                self.core.begin();
                self
            }

            // 前フレームの値と比較しながら書き込む（§10 インデックス）
            pub fn index(&mut self, index: u32) -> &mut Self {
                self.core.write_index(index);
                self
            }

            pub fn end(&mut self, draw_manager: &mut $draw_manager) -> &mut Self {
                let camera = self.core.camera.unwrap_or_else(|| draw_manager.default_camera_id());

                // 別の DrawManager* の CameraId・削除済みの CameraId は panic（§5・§11）
                draw_manager.cameras.check(camera);
                let context = EndContext { camera, layer: self.layer_setting(), bounds: self.bounds_setting() };
                self.core.end(&mut draw_manager.core, context);
                self
            }

            // ---- 高頻度ではないが、描画中でも呼ばれる関数群（§1）----

            pub fn camera(&mut self, id: CameraId) -> &mut Self {
                self.core.set("camera", |core| &mut core.camera, id);
                self
            }

            pub fn memory_update_type(&mut self, memory_update_type: MemoryUpdateType) -> &mut Self {
                self.core.set("memory_update_type", |core| &mut core.memory_update_type, memory_update_type);
                self
            }

            pub fn shader(&mut self, shader: ShaderId) -> &mut Self {
                self.core.set("shader", |core| &mut core.shader, shader);
                self
            }

            pub fn texture(&mut self, texture: TextureId) -> &mut Self {
                self.core.set_texture(texture);
                self
            }

            pub fn blend(&mut self, blend: Blend) -> &mut Self {
                self.core.set("blend", |core| &mut core.blend, blend);
                self
            }

            pub fn topology(&mut self, topology: Topology) -> &mut Self {
                self.core.set("topology", |core| &mut core.topology, topology);
                self
            }

            pub fn cull_mode(&mut self, cull_mode: CullMode) -> &mut Self {
                self.core.set("cull_mode", |core| &mut core.cull_mode, cull_mode);
                self
            }

            // 高頻度で呼んでよい。同値なら無視し、警告は出さない（§11 uniform）
            pub fn uniform<T: bytemuck::Pod>(&mut self, value: &T) -> &mut Self {
                self.core.set_uniform(bytemuck::bytes_of(value));
                self
            }

            // ---- インスタンス（§7）----

            // 同値でも無視せず、常に追加する（§7 add_instance の同値の扱い）
            pub fn add_instance(&mut self, instance: $instance) -> InstanceId<$instance> {
                self.core.instances.add(instance)
            }

            pub fn update_instance(&mut self, id: InstanceId<$instance>, instance: $instance) {
                self.core.instances.update(id, instance);
            }

            pub fn remove_instance(&mut self, id: InstanceId<$instance>) {
                self.core.instances.remove(id);
            }
        }
    };
}

pub struct Object2D {
    pub(crate) core: ObjectCore<Instance2D>,
    layer: Option<i32>,
    bounding_circle: Option<(Vector2f, f32)>,
}

pub struct Object3D {
    pub(crate) core: ObjectCore<Instance3D>,
    bounding_sphere: Option<(Vector3f, f32)>,
}

object_common!(Object2D, Instance2D, DrawManager2D);
object_common!(Object3D, Instance3D, DrawManager3D);

impl Object2D {
    // フレーム番号と Object の識別子を受け取る（§2 begin / end の対応）
    pub fn new(gpu: &GpuContext) -> Self {
        Self { core: ObjectCore::new(gpu, Winding::CounterClockwiseXy), layer: None, bounding_circle: None }
    }

    // 2D の頂点（§11 2D の頂点の入力）。省略された要素は 0.0
    pub fn vertex(&mut self, x: f32, y: f32) -> &mut Self {
        self.core.write_vertex(Vertex::new_position(x, y, 0.0), VertexElements::Position);
        self
    }

    pub fn vertex_color(&mut self, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32) -> &mut Self {
        self.core.write_vertex(Vertex::new_position_color(x, y, 0.0, r, g, b, a), VertexElements::Color);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn vertex_color_uv(&mut self, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32) -> &mut Self {
        self.core.write_vertex(Vertex::new_position_color_uv(x, y, 0.0, r, g, b, a, u, v), VertexElements::Uv);
        self
    }

    // 2D のみ。未指定なら LonasConfig::layer_when_unset（§10 描画順）
    pub fn layer(&mut self, layer: i32) -> &mut Self {
        if self.layer == Some(layer) {
            self.core.warn_redundant("layer");
        }

        self.layer = Some(layer);
        self
    }

    // 任意。指定しなければ頂点から自動で計算する（§10 視界の判定に使う大きさ）
    pub fn bounding_circle(&mut self, center: Vector2f, radius: f32) -> &mut Self {
        self.bounding_circle = Some((center, radius));
        self
    }

    fn layer_setting(&self) -> Option<i32> {
        self.layer
    }

    fn bounds_setting(&self) -> Option<[f32; 4]> {
        self.bounding_circle.map(|(center, radius)| [center.x, center.y, 0.0, radius])
    }
}

impl Object3D {
    pub fn new(gpu: &GpuContext) -> Self {
        Self { core: ObjectCore::new(gpu, Winding::FollowInput), bounding_sphere: None }
    }

    pub fn vertex(&mut self, x: f32, y: f32, z: f32) -> &mut Self {
        self.core.write_vertex(Vertex::new_position(x, y, z), VertexElements::Position);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn vertex_color(&mut self, x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32) -> &mut Self {
        self.core.write_vertex(Vertex::new_position_color(x, y, z, r, g, b, a), VertexElements::Color);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn vertex_color_uv(&mut self, x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32) -> &mut Self {
        self.core.write_vertex(Vertex::new_position_color_uv(x, y, z, r, g, b, a, u, v), VertexElements::Uv);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn vertex_color_uv_normal(
        &mut self,
        x: f32,
        y: f32,
        z: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
        u: f32,
        v: f32,
        n_x: f32,
        n_y: f32,
        n_z: f32,
    ) -> &mut Self {
        self.core.write_vertex(
            Vertex::new_position_color_uv_normal(x, y, z, r, g, b, a, u, v, n_x, n_y, n_z),
            VertexElements::Normal,
        );
        self
    }

    // 任意。指定しなければ頂点から自動で計算する（§10 視界の判定に使う大きさ）
    pub fn bounding_sphere(&mut self, center: Vector3f, radius: f32) -> &mut Self {
        self.bounding_sphere = Some((center, radius));
        self
    }

    fn layer_setting(&self) -> Option<i32> {
        None
    }

    fn bounds_setting(&self) -> Option<[f32; 4]> {
        self.bounding_sphere.map(|(center, radius)| [center.x, center.y, center.z, radius])
    }
}
