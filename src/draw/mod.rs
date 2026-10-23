// DrawManager2D / DrawManager3D（§5・§6 DrawManager 経由のアクセス）
// Object*::end から受け取った要求と描画コマンドを溜め、end_frame で GpuContext が処理する

pub(crate) mod camera;
pub(crate) mod staging;

use std::collections::HashSet;
use std::ops::Range;

pub use camera::{Camera2D, Camera3D, CameraId, ProjectionMode};
pub(crate) use camera::GpuCamera;
use camera::CameraSet;
use staging::StagingBelt;

use crate::config::{Blend, CoordinateSystemConfig, CullMode};
use crate::math::{Matrix4f, Vector2f, Vector3f};
use crate::gpu::{GpuContext, sealed};
use crate::memory::{GpuAllocation, ManagerId, MemoryUpdateType};
use crate::object::instance_slots::ObjectId;
use crate::resource::{ShaderId, TextureId};
use crate::vertex::VertexLayout;

// 用途ごとのメモリ空間（§6 用途ごとのメモリ空間）
// 要素の数を単位に管理する。描画で base_vertex・インスタンスの番号として位置を指定するため、
// 領域の先頭は要素の大きさの倍数でなければならない（段階 6 で変更）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SpaceKind {
    // 頂点は VertexLayout ごとに大きさが違うため、空間を分ける
    Vertex(VertexLayout),
    Index,
    Instance2D,
    Instance3D,
}

impl SpaceKind {
    // 1 要素の大きさ（バイト）
    pub(crate) fn unit(self) -> u64 {
        match self {
            SpaceKind::Vertex(layout) => layout.stride() as u64,
            SpaceKind::Index => 4,
            SpaceKind::Instance2D => crate::wgsl::INSTANCE_2D_SIZE as u64,
            SpaceKind::Instance3D => crate::wgsl::INSTANCE_3D_SIZE as u64,
        }
    }
}

// end_frame で GpuContext がヒープに対して行うこと（§6 DrawManager 経由のアクセス）。大きさと位置は要素の数
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HeapRequest {
    Allocate { handle: GpuAllocation, space: SpaceKind, count: u64, update: MemoryUpdateType },
    // 大きさや memory_update_type が変わった。中身は Write で書き直す
    Reallocate { handle: GpuAllocation, space: SpaceKind, count: u64, update: MemoryUpdateType },
    // staging バッファの chunk の range を、領域の先頭から offset 要素目の位置へコピーする
    Write { handle: GpuAllocation, space: SpaceKind, offset: u64, chunk: u32, range: Range<u64> },
}

// 段階 6 の描画で使う設定。*_when_unset は end の時点で解決済み
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct DrawSettings {
    pub shader: Option<ShaderId>,
    pub texture: Option<TextureId>,
    pub blend: Blend,
    pub cull_mode: CullMode,
    pub camera: CameraId,
    // 2D のみ
    pub layer: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DrawCommand {
    pub object: ObjectId,
    pub vertex: GpuAllocation,
    pub index: GpuAllocation,
    pub instance: GpuAllocation,
    pub index_count: u32,
    pub instance_count: u32,
    pub settings: DrawSettings,
    // 視界の判定に使う、ローカル空間の境界（中心 xyz と半径。§10 視界の判定に使う大きさ）
    pub bounds: [f32; 4],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Dimension {
    Two,
    Three,
}

// 視界の判定と GPU に送るカメラ。削除したカメラの番号は None
pub struct CameraFrame {
    pub(crate) camera: GpuCamera,
    // 3D の不透明の奥行きの並べ替えに使う（near, far）
    pub(crate) depth_range: [f32; 2],
}

// Object*::end が end_frame まで使う値（*_when_unset など）。LonasConfig から写す
#[derive(Copy, Clone, Debug)]
pub(crate) struct ObjectDefaults {
    pub blend: Blend,
    pub cull_mode: CullMode,
    pub layer: i32,
}

// 2D / 3D で共通の部分（§5 2D / 3D の型）
pub struct DrawManagerCore {
    pub(crate) id: ManagerId,
    pub(crate) dimension: Dimension,
    pub(crate) coordinate_system: CoordinateSystemConfig,
    // 視界の外を省く処理を行うか（2D は LonasConfig::culling_2d、3D は常に行う）
    pub(crate) culling: bool,
    // GPU に送るためのバッファ（render が作り、使い回す）
    pub(crate) resources: Option<crate::render::frame::DrawResources>,
    pub(crate) layout: VertexLayout,
    pub(crate) defaults: ObjectDefaults,
    pub(crate) staging: StagingBelt,
    pub(crate) heap_requests: Vec<HeapRequest>,
    pub(crate) draw_commands: Vec<DrawCommand>,
    // そのフレームで end された Object* の領域（GC に知らせる、§6 解放）
    pub(crate) touched: Vec<(SpaceKind, GpuAllocation)>,
    // 溜まっている要求とコマンドが積まれたフレーム
    pending_frame: Option<u64>,
    // GC で回収されたハンドル（§6 回収の通知）
    reclaimed: HashSet<GpuAllocation>,
    next_serial: u32,
    // 詰めた頂点などを staging バッファへ書く前に置く場所。容量は使い回す
    pub(crate) scratch: Vec<u8>,
}

impl DrawManagerCore {
    fn new(gpu: &mut GpuContext, layout: VertexLayout, dimension: Dimension) -> Self {
        let config = gpu.config();
        let coordinate_system = config.coordinate_system;
        let culling = dimension == Dimension::Three || config.culling_2d;
        let defaults = ObjectDefaults {
            blend: config.blend_when_unset,
            cull_mode: config.cull_mode_when_unset,
            layer: config.layer_when_unset,
        };
        let staging = StagingBelt::new(gpu.device().clone(), config.staging_chunk_size);

        Self {
            id: gpu.issue_manager_id(),
            dimension,
            coordinate_system,
            culling,
            resources: None,
            layout,
            defaults,
            staging,
            heap_requests: Vec::new(),
            draw_commands: Vec::new(),
            touched: Vec::new(),
            pending_frame: None,
            reclaimed: HashSet::new(),
            next_serial: 0,
            scratch: Vec::new(),
        }
    }

    pub(crate) fn issue_handle(&mut self) -> GpuAllocation {
        self.next_serial += 1;

        GpuAllocation { manager: self.id, serial: self.next_serial - 1 }
    }

    // Object*::end が要求やコマンドを積む前に呼ぶ
    // 前のフレームの要求が残っていれば、end_frame に渡し忘れている（§9 誤用時は panic）
    pub(crate) fn prepare_for_end(&mut self, frame: u64) {
        match self.pending_frame {
            Some(pending) if pending != frame => panic!(
                "前のフレームで Object の end を呼んだ DrawManager が end_frame に渡されていません"
            ),
            _ => self.pending_frame = Some(frame),
        }
    }

    // GC で回収されたなら、その記録を消して true（Object* は確保し直してすべてを転送する）
    pub(crate) fn take_reclaimed(&mut self, handle: GpuAllocation) -> bool {
        self.reclaimed.remove(&handle)
    }

    // ---- end_frame（GpuContext）が使う ----

    pub(crate) fn receive_reclaimed(&mut self, handles: impl IntoIterator<Item = GpuAllocation>) {
        self.reclaimed.extend(handles);
    }

    // end_frame で処理した後に、コマンドと要求を消す
    pub(crate) fn finish_frame(&mut self) {
        self.heap_requests.clear();
        self.draw_commands.clear();
        self.touched.clear();
        self.pending_frame = None;
    }

    // staging バッファに bytes を書き、Write の要求を積む
    pub(crate) fn push_write(&mut self, handle: GpuAllocation, space: SpaceKind, offset: u64, bytes: &[u8]) {
        let (chunk, range) = self.staging.write(bytes);

        self.heap_requests.push(HeapRequest::Write { handle, space, offset, chunk, range });
    }
}

macro_rules! draw_manager {
    ($name:ident, $camera:ty, $dimension:expr) => {
        pub struct $name {
            pub(crate) core: DrawManagerCore,
            pub(crate) cameras: CameraSet<$camera>,
        }

        impl $name {
            // 渡したカメラが既定カメラになる（§5 カメラの指定）
            pub fn new(gpu: &mut GpuContext, camera: $camera, vertex_layout: VertexLayout) -> Self {
                let core = DrawManagerCore::new(gpu, vertex_layout, $dimension);
                let cameras = CameraSet::new(core.id, camera);

                Self { core, cameras }
            }

            pub fn default_camera_id(&self) -> CameraId {
                self.cameras.default_id()
            }

            pub fn add_camera(&mut self, camera: $camera) -> CameraId {
                self.cameras.add(camera)
            }

            // 削除済みの CameraId を使った場合は panic（§11 Camera）
            pub fn remove_camera(&mut self, id: CameraId) {
                self.cameras.remove(id);
            }

            pub fn camera(&self, id: CameraId) -> &$camera {
                self.cameras.get(id)
            }

            pub fn camera_mut(&mut self, id: CameraId) -> &mut $camera {
                self.cameras.get_mut(id)
            }

            pub fn vertex_layout(&self) -> VertexLayout {
                self.core.layout
            }
        }

        impl sealed::Sealed for $name {
            fn core(&mut self) -> &mut DrawManagerCore {
                &mut self.core
            }

            fn camera_frames(&self, viewport_size: Vector2f) -> Vec<Option<CameraFrame>> {
                self.cameras.slots().map(|camera| camera.map(|camera| self.camera_frame(camera, viewport_size))).collect()
            }

            fn camera_slot(&self, id: CameraId) -> u32 {
                self.cameras.slot(id)
            }
        }

        impl crate::gpu::DrawManager for $name {}
    };
}

draw_manager!(DrawManager2D, Camera2D, Dimension::Two);
draw_manager!(DrawManager3D, Camera3D, Dimension::Three);

impl DrawManager2D {
    // 描く大きさ（ピクセル）を渡して、ビュー・プロジェクション行列を求める。マウス位置の逆変換など（§11 Camera）
    pub fn view_projection(&self, id: CameraId, viewport_size: Vector2f) -> Matrix4f {
        let camera = self.cameras.get(id);

        Camera2D::projection(self.core.coordinate_system.origin_2d, viewport_size) * camera.view()
    }

    fn camera_frame(&self, camera: &Camera2D, viewport_size: Vector2f) -> CameraFrame {
        let projection = Camera2D::projection(self.core.coordinate_system.origin_2d, viewport_size);

        CameraFrame {
            camera: GpuCamera::new(camera.view(), projection, camera.position.extend(0.0), viewport_size),
            depth_range: [0.0, 1.0],
        }
    }
}

impl DrawManager3D {
    pub fn view_projection(&self, id: CameraId, viewport_size: Vector2f) -> Matrix4f {
        let camera = self.cameras.get(id);

        camera.projection(self.core.coordinate_system.handedness_3d, viewport_size) * camera.view()
    }

    // target の方を向くように、そのカメラの rotation を設定する（§11 Camera の look_at）
    // 呼んだ時点の position から計算する。追いかけるカメラは毎フレーム呼ぶ
    pub fn look_at(&mut self, id: CameraId, target: Vector3f, up: Vector3f) {
        let handedness = self.core.coordinate_system.handedness_3d;

        self.cameras.get_mut(id).look_at(target, up, handedness);
    }

    fn camera_frame(&self, camera: &Camera3D, viewport_size: Vector2f) -> CameraFrame {
        let projection = camera.projection(self.core.coordinate_system.handedness_3d, viewport_size);

        CameraFrame {
            camera: GpuCamera::new(camera.view(), projection, camera.position, viewport_size),
            depth_range: [camera.near, camera.far],
        }
    }
}
