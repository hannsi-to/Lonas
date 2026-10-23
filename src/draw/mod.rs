// DrawManager2D / DrawManager3D（§5・§6 DrawManager 経由のアクセス）
// Object*::end から受け取った要求と描画コマンドを溜め、end_frame で GpuContext が処理する

pub(crate) mod camera;
pub(crate) mod staging;

use std::collections::HashSet;
use std::ops::Range;

pub use camera::{Camera2D, Camera3D, CameraId, ProjectionMode};
use camera::CameraSet;
use staging::StagingBelt;

use crate::config::{Blend, CullMode};
use crate::gpu::{GpuContext, sealed};
use crate::memory::{GpuAllocation, ManagerId, MemoryUpdateType};
use crate::object::instance_slots::ObjectId;
use crate::resource::{ShaderId, TextureId};
use crate::vertex::VertexLayout;

// 用途ごとのメモリ空間（§6 用途ごとのメモリ空間）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SpaceKind {
    Vertex,
    Index,
    Instance,
}

impl SpaceKind {
    pub(crate) const ALL: [SpaceKind; 3] = [SpaceKind::Vertex, SpaceKind::Index, SpaceKind::Instance];

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

// end_frame で GpuContext がヒープに対して行うこと（§6 DrawManager 経由のアクセス）
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HeapRequest {
    Allocate { handle: GpuAllocation, space: SpaceKind, size: u64, update: MemoryUpdateType },
    // 大きさや memory_update_type が変わった。中身は Write で書き直す
    Reallocate { handle: GpuAllocation, space: SpaceKind, size: u64, update: MemoryUpdateType },
    // staging バッファの chunk の range を、領域の先頭から offset の位置へコピーする
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
    fn new(gpu: &mut GpuContext, layout: VertexLayout) -> Self {
        let config = gpu.config();
        let defaults = ObjectDefaults {
            blend: config.blend_when_unset,
            cull_mode: config.cull_mode_when_unset,
            layer: config.layer_when_unset,
        };
        let staging = StagingBelt::new(gpu.device().clone(), config.staging_chunk_size);

        Self {
            id: gpu.issue_manager_id(),
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
    ($name:ident, $camera:ty) => {
        pub struct $name {
            pub(crate) core: DrawManagerCore,
            pub(crate) cameras: CameraSet<$camera>,
        }

        impl $name {
            // 渡したカメラが既定カメラになる（§5 カメラの指定）
            pub fn new(gpu: &mut GpuContext, camera: $camera, vertex_layout: VertexLayout) -> Self {
                let core = DrawManagerCore::new(gpu, vertex_layout);
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
        }

        impl crate::gpu::DrawManager for $name {}
    };
}

draw_manager!(DrawManager2D, Camera2D);
draw_manager!(DrawManager3D, Camera3D);
