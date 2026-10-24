// Object2D / Object3D（§1・§4・§5・§7・§10）
// 内部は ObjectCore で共通化し、型ごとの差はインスタンスの型・end に渡せる DrawManager* の型・型固有の設定のみ

pub(crate) mod core;
pub(crate) mod instance_slots;
mod stroke;
#[cfg(test)]
mod tests;
pub(crate) mod paint_type;

pub use instance_slots::InstanceId;
pub use paint_type::{Dash, JointType, PaintType};

use crate::config::{Blend, CullMode};
use crate::draw::{CameraId, DrawManager2D, DrawManager3D};
use crate::gpu::GpuContext;
use crate::instance::{Instance2D, Instance3D};
use crate::math::{Vector2f, Vector3f};
use crate::memory::MemoryUpdateType;
use crate::resource::{ShaderId, TextureId};
use crate::vertex::Vertex;
use core::{EndContext, ObjectCore};
use paint_type::Winding;

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

            // 頂点（§1 頂点の入力）。前のフレームの同じ位置の頂点と比べながら書き込む。2D でも z は渡したとおりに使う
            // VertexLayout に含まれない要素に 0 以外の値があれば、Object ごとに一度だけ警告して捨てる（§10 頂点の並び方）
            pub fn vertex(&mut self, vertex: Vertex) -> &mut Self {
                self.core.write_vertex(vertex);
                self
            }

            // 以降の vertex() を、新しい形にする（§10 形の区切り）。Fill でも Stroke でも、形ごとに別々に三角形を作る
            // index() を呼んだ形はそのインデックスを使い、呼ばなかった形だけを paint_type で作る（§10 インデックス）
            pub fn begin_shape(&mut self) -> &mut Self {
                self.core.begin_shape();
                self
            }

            // 以降の vertex() を、次の begin_hole() か begin_shape() か end まで、今の形の 1 つの穴の輪郭にする（§10 穴のある多角形）
            // Fill は穴を空けて塗り、Stroke は穴の輪郭も描く
            pub fn begin_hole(&mut self) -> &mut Self {
                self.core.begin_hole();
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

            // Stroke の line_width・Dash の値が不正なら panic（§10 線：Stroke）
            pub fn paint_type(&mut self, paint_type: PaintType) -> &mut Self {
                paint_type.validate();
                self.core.set("paint_type", |core| &mut core.paint_type, paint_type);
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

            // ---- 状態の取得（§11 Object の状態の取得）----
            // 設定は、設定の関数で指定した値を返す。None は一度も指定していない（end の時点で LonasConfig の *_when_unset などを使う）
            // 設定の関数と名前が重なるため、get_ を付ける

            pub fn get_shader(&self) -> Option<ShaderId> {
                self.core.shader
            }

            pub fn get_texture(&self) -> Option<TextureId> {
                self.core.texture
            }

            // None なら end で DrawManager* の既定のカメラを使う
            pub fn get_camera(&self) -> Option<CameraId> {
                self.core.camera
            }

            pub fn get_blend(&self) -> Option<Blend> {
                self.core.blend
            }

            pub fn get_paint_type(&self) -> Option<PaintType> {
                self.core.paint_type
            }

            pub fn get_cull_mode(&self) -> Option<CullMode> {
                self.core.cull_mode
            }

            pub fn get_memory_update_type(&self) -> Option<MemoryUpdateType> {
                self.core.memory_update_type
            }

            // uniform() で渡した値。None は一度も渡していない。T の大きさが渡した値と違えば panic
            pub fn get_uniform<T: bytemuck::Pod>(&self) -> Option<T> {
                if self.core.uniform.is_empty() {
                    return None;
                }

                assert_eq!(
                    self.core.uniform.len(),
                    size_of::<T>(),
                    "get_uniform の型の大きさが、uniform() で渡した値の大きさと違います",
                );
                Some(bytemuck::pod_read_unaligned(&self.core.uniform))
            }

            // 別の Object* の InstanceId・削除済みの InstanceId は panic（update_instance と同じ、§7）
            pub fn get_instance(&self, id: InstanceId<$instance>) -> $instance {
                self.core.instances.get(id)
            }

            // 削除していないインスタンスの数
            pub fn get_instance_count(&self) -> usize {
                self.core.instances.live_count()
            }

            // このフレームの begin から vertex() で書いた頂点の数。begin で 0 に戻り、end の後も次の begin までは同じ値
            // index() の番号は Object 全体の通し番号のため、begin_shape() の後の形の番号の起点に使える
            pub fn get_written_vertex_count(&self) -> usize {
                self.core.written_vertex_count()
            }

            // このフレームの begin から index() で積んだ数。begin で 0 に戻り、end の後も次の begin までは同じ値
            pub fn get_written_index_count(&self) -> usize {
                self.core.written_index_count()
            }

            // 最後の end の時点の頂点（vertex() で書いたもの）。begin〜end の間は書きかけ
            // Stroke では線の中心の点で、線として作った頂点ではない
            pub fn get_vertices(&self) -> &[Vertex] {
                self.core.vertices()
            }

            // 最後の end の時点で GPU に送るインデックス。index() で書いた形はそのまま、書かなかった形は paint_type で作ったもの
            // Stroke では、線として作った頂点を指す
            pub fn get_indices(&self) -> &[u32] {
                self.core.index_values()
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

    // None なら end で LonasConfig::layer_when_unset を使う
    pub fn get_layer(&self) -> Option<i32> {
        self.layer
    }

    // None なら頂点から自動で計算する
    pub fn get_bounding_circle(&self) -> Option<(Vector2f, f32)> {
        self.bounding_circle
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

    // 任意。指定しなければ頂点から自動で計算する（§10 視界の判定に使う大きさ）
    pub fn bounding_sphere(&mut self, center: Vector3f, radius: f32) -> &mut Self {
        self.bounding_sphere = Some((center, radius));
        self
    }

    // None なら頂点から自動で計算する
    pub fn get_bounding_sphere(&self) -> Option<(Vector3f, f32)> {
        self.bounding_sphere
    }

    fn layer_setting(&self) -> Option<i32> {
        None
    }

    fn bounds_setting(&self) -> Option<[f32; 4]> {
        self.bounding_sphere.map(|(center, radius)| [center.x, center.y, center.z, radius])
    }
}
