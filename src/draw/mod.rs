// DrawManager2D / DrawManager3D（§5・§6 DrawManager 経由のアクセス）
// Object*::end から受け取った要求と描画コマンドを溜め、end_frame で GpuContext が処理する

pub(crate) mod camera;
pub(crate) mod staging;
pub(crate) mod view;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};

pub use camera::{Camera2D, Camera3D, CameraId, ProjectionMode};
pub(crate) use camera::GpuCamera;
use camera::CameraSet;
use staging::StagingBelt;
pub use view::{DrawView, DrawViewBuilder, ViewCameraType};
use view::{Crop, ViewCamera};

use crate::config::{Blend, CoordinateSystemConfig, CullMode};
use crate::math::{Matrix4f, Vector2f, Vector3f};
use crate::gpu::{GpuContext, SurfaceId, sealed};
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
    // uniform の要素（§6 uniform の領域）。この DrawManager* と shader のバッファの slot 番目へコピーする
    WriteUniform { shader: ShaderId, slot: u32, chunk: u32, range: Range<u64> },
}

// uniform の要素の番号の割り当て（§6 uniform の領域）。番号 0 は予約（すべて 0）
#[derive(Debug, Default)]
struct UniformSlots {
    next: u32,
    free: Vec<u32>,
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

// 捨てられた Object* の領域（§6 解放：Drop の知らせ）
// Object* が初回の end で受け取り、Drop で自分の領域を入れる。end_frame の後片付けで取り出して解放する
pub(crate) type DroppedRegions = Arc<Mutex<Vec<(SpaceKind, GpuAllocation)>>>;

// 2D / 3D で共通の部分（§5 2D / 3D の型）
pub struct DrawManagerCore {
    pub(crate) id: ManagerId,
    pub(crate) dimension: Dimension,
    pub(crate) coordinate_system: CoordinateSystemConfig,
    // 視界の外を省く処理を行うか（2D は LonasConfig::culling_2d、3D は常に行う）
    pub(crate) culling: bool,
    // GPU に送るためのバッファ（render が作り、使い回す）。カメラと視界の判定の結果はサーフェスごとに違うため、
    // サーフェスごとに持つ（§11 同じ DrawManager* を複数のサーフェスに描く）。値は最後に使ったフレーム
    pub(crate) resources: HashMap<SurfaceId, (crate::render::frame::DrawResources, u64)>,
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
    dropped: DroppedRegions,
    next_serial: u32,
    // 詰めた頂点などを staging バッファへ書く前に置く場所。容量は使い回す
    pub(crate) scratch: Vec<u8>,
    // シェーダーごとの uniform の要素の空き番号（§6 uniform の領域）
    uniform_slots: HashMap<ShaderId, UniformSlots>,
    // GC で回収されたときに uniform の要素を空きに戻すため、インスタンスの領域のハンドルから引く
    uniform_owners: HashMap<GpuAllocation, (ShaderId, u32)>,
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
            resources: HashMap::new(),
            layout,
            defaults,
            staging,
            heap_requests: Vec::new(),
            draw_commands: Vec::new(),
            touched: Vec::new(),
            pending_frame: None,
            reclaimed: HashSet::new(),
            dropped: DroppedRegions::default(),
            next_serial: 0,
            scratch: Vec::new(),
            uniform_slots: HashMap::new(),
            uniform_owners: HashMap::new(),
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

    // Object* が初回の end で受け取り、Drop で使う
    pub(crate) fn dropped_regions(&self) -> DroppedRegions {
        self.dropped.clone()
    }

    #[cfg(test)]
    pub(crate) fn reclaimed_len(&self) -> usize {
        self.reclaimed.len()
    }

    // ---- end_frame（GpuContext）が使う ----

    // 捨てられた Object* の領域のうち、まだヒープにあるものを返す。uniform の要素は空きに戻す
    // GC で回収済みのものは、回収の記録を消すだけにする（receive_reclaimed の後に呼ぶ）
    pub(crate) fn take_dropped(&mut self) -> Vec<(SpaceKind, GpuAllocation)> {
        let dropped = std::mem::take(&mut *self.dropped.lock().unwrap_or_else(PoisonError::into_inner));

        dropped
            .into_iter()
            .filter(|&(_, handle)| {
                if let Some((shader, slot)) = self.uniform_owners.remove(&handle) {
                    self.release_uniform_slot(shader, slot);
                }

                !self.reclaimed.remove(&handle)
            })
            .collect()
    }

    pub(crate) fn receive_reclaimed(&mut self, handles: impl IntoIterator<Item = GpuAllocation>) {
        for handle in handles {
            // その Object* の uniform の要素も空きに戻す（§6 uniform の領域）
            if let Some((shader, slot)) = self.uniform_owners.remove(&handle) {
                self.release_uniform_slot(shader, slot);
            }

            self.reclaimed.insert(handle);
        }
    }

    // uniform の要素の番号を、end の時点で割り当てる（§6 uniform の領域）。owner はインスタンスの領域のハンドル
    pub(crate) fn allocate_uniform_slot(&mut self, shader: ShaderId, owner: GpuAllocation) -> u32 {
        let slots = self.uniform_slots.entry(shader).or_default();
        let slot = slots.free.pop().unwrap_or_else(|| {
            slots.next += 1;
            slots.next
        });

        self.uniform_owners.insert(owner, (shader, slot));
        slot
    }

    pub(crate) fn release_uniform_slot(&mut self, shader: ShaderId, slot: u32) {
        self.uniform_owners.retain(|_, &mut owned| owned != (shader, slot));
        self.uniform_slots.entry(shader).or_default().free.push(slot);
    }

    // uniform の値を staging バッファに書き、WriteUniform の要求を積む。コピーのため 4 バイトの倍数に詰め物をする
    pub(crate) fn push_uniform_write(&mut self, shader: ShaderId, slot: u32, bytes: &[u8]) {
        let mut padded = std::mem::take(&mut self.scratch);
        padded.clear();
        padded.extend_from_slice(bytes);
        padded.resize(bytes.len().div_ceil(4) * 4, 0);

        let (chunk, range) = self.staging.write(&padded);
        self.heap_requests.push(HeapRequest::WriteUniform { shader, slot, chunk, range });
        self.scratch = padded;
    }

    // そのサーフェスに描くためのバッファ。なければ作る
    pub(crate) fn surface_resources(&mut self, device: &wgpu::Device, surface: SurfaceId, frame: u64) -> &mut crate::render::frame::DrawResources {
        let entry = self.resources.entry(surface).or_insert_with(|| (crate::render::frame::DrawResources::new(device), frame));
        entry.1 = frame;
        &mut entry.0
    }

    // unused_frames のフレーム数だけ描かなかったサーフェスのバッファを捨てる（取り除いたサーフェスもこれで消える）
    pub(crate) fn collect_unused_resources(&mut self, frame: u64, unused_frames: u32) {
        self.resources.retain(|_, (_, last)| frame.saturating_sub(*last) <= unused_frames as u64);
    }

    // テストで、1 つのサーフェスにだけ描いた場合のバッファを取り出す
    #[cfg(test)]
    pub(crate) fn single_resources(&self) -> &crate::render::frame::DrawResources {
        assert_eq!(self.resources.len(), 1, "サーフェスごとのバッファが 1 つではありません");
        &self.resources.values().next().unwrap().0
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

            // サーフェスごとにカメラを差し替える・切り取るための DrawView を作る（§11 同じ DrawManager* を複数のサーフェスに描く）
            pub fn view(&self) -> DrawViewBuilder<$camera> {
                DrawViewBuilder::new(DrawView::new(self))
            }

            // DrawView を通したときのビュー・プロジェクション行列（差し替えと切り取りを含む。ジッターは含まない）
            pub fn view_projection_in(&self, view: &DrawView, id: CameraId, viewport_size: Vector2f) -> Matrix4f {
                assert_eq!(view.manager, self.core.id, "別の DrawManager の DrawView です");
                let camera = self.view_camera(view, id);
                let frame = self.camera_frame(&camera, viewport_size, Vector2f::ZERO, view.crop);

                Matrix4f::from_cols_array(frame.camera.view_projection_no_jitter)
            }

            // DrawView で差し替えたカメラ。なければ DrawManager* のカメラ
            fn view_camera(&self, view: &DrawView, id: CameraId) -> $camera {
                let camera = *self.cameras.get(id);

                match view.camera(id) {
                    Some(override_camera) => Self::unwrap_view_camera(override_camera),
                    None => camera,
                }
            }
        }

        impl sealed::Sealed for $name {
            fn core(&mut self) -> &mut DrawManagerCore {
                &mut self.core
            }

            fn core_ref(&self) -> &DrawManagerCore {
                &self.core
            }

            fn camera_frames(&self, viewport_size: Vector2f, jitter: Vector2f, view: &DrawView) -> Vec<Option<CameraFrame>> {
                let mut cameras: Vec<Option<$camera>> = self.cameras.slots().map(|camera| camera.copied()).collect();

                // 削除済みの CameraId の差し替えは panic（§9 誤用時は panic）
                for &(id, camera) in &view.cameras {
                    cameras[self.cameras.slot(id) as usize] = Some(Self::unwrap_view_camera(camera));
                }

                cameras.iter().map(|camera| camera.map(|camera| self.camera_frame(&camera, viewport_size, jitter, view.crop))).collect()
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
    fn unwrap_view_camera(camera: ViewCamera) -> Camera2D {
        match camera {
            ViewCamera::Two(camera) => camera,
            ViewCamera::Three(_) => unreachable!("DrawViewBuilder<Camera2D> は Camera2D だけを受け取る"),
        }
    }

    // 描く大きさ（ピクセル）を渡して、ビュー・プロジェクション行列を求める。マウス位置の逆変換など（§11 Camera）
    pub fn view_projection(&self, id: CameraId, viewport_size: Vector2f) -> Matrix4f {
        let camera = self.cameras.get(id);

        Camera2D::projection(self.core.coordinate_system.origin_2d, viewport_size) * camera.view()
    }

    fn camera_frame(&self, camera: &Camera2D, viewport_size: Vector2f, jitter: Vector2f, crop: Option<Crop>) -> CameraFrame {
        let origin = self.core.coordinate_system.origin_2d;
        // 切り取る場合は、全体の大きさで投影を作ってから範囲を切り出す
        let projection = match crop {
            Some(crop) => crop.matrix() * Camera2D::projection(origin, crop.full_size),
            None => Camera2D::projection(origin, viewport_size),
        };

        CameraFrame {
            camera: GpuCamera::new(camera.view(), projection, camera.position.extend(0.0), viewport_size, jitter),
            depth_range: [0.0, 1.0],
        }
    }
}

impl DrawManager3D {
    fn unwrap_view_camera(camera: ViewCamera) -> Camera3D {
        match camera {
            ViewCamera::Three(camera) => camera,
            ViewCamera::Two(_) => unreachable!("DrawViewBuilder<Camera3D> は Camera3D だけを受け取る"),
        }
    }

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

    fn camera_frame(&self, camera: &Camera3D, viewport_size: Vector2f, jitter: Vector2f, crop: Option<Crop>) -> CameraFrame {
        let handedness = self.core.coordinate_system.handedness_3d;
        let projection = match crop {
            Some(crop) => crop.matrix() * camera.projection(handedness, crop.full_size),
            None => camera.projection(handedness, viewport_size),
        };

        CameraFrame {
            camera: GpuCamera::new(camera.view(), projection, camera.position, viewport_size, jitter),
            depth_range: [camera.near, camera.far],
        }
    }
}
