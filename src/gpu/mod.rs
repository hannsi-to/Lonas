// GpuContext：device / queue / サーフェス・ヒープ・フレーム境界（§6・§8・§9）
// パイプライン・Compute のパスは段階 6 以降で足す

mod features;
pub(crate) mod frame;
pub(crate) mod heap;
mod surface;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::config::{AntiAliasing, Backend, LonasConfig, PresentMode};
use crate::error::LonasError;
use crate::math::Vector2f;
use crate::resource::ResourceManager;
use crate::draw::{Dimension, DrawManagerCore};
use crate::render::{self, Renderer, TargetFormat};
use crate::slots::Slots;
use crate::{color::Color, memory::ManagerId};
use frame::FrameClock;
use heap::Heap;
pub use surface::SurfaceId;
use surface::SurfaceState;

// end_frame に 2D / 3D の DrawManager* を混ぜて渡すためのトレイト（§9）。DrawManager2D / DrawManager3D が実装する
pub trait DrawManager: sealed::Sealed {}

pub(crate) mod sealed {
    pub trait Sealed {
        fn core(&mut self) -> &mut crate::draw::DrawManagerCore;
        // 描くサーフェスの大きさから求めた、カメラの番号ごとの値
        // jitter は TAA のカメラのずらし（ピクセル）。TAA 以外は 0
        fn camera_frames(&self, viewport_size: crate::math::Vector2f, jitter: crate::math::Vector2f) -> Vec<Option<crate::draw::CameraFrame>>;
        fn camera_slot(&self, id: crate::draw::CameraId) -> u32;
    }
}

// サーフェスごとの描画（§9・§11 複数ウィンドウ）
pub struct SurfaceDraw<'a, 'b> {
    pub surface: SurfaceId,
    // None ならクリアしない（§10 深度バッファとクリア）
    pub clear_color: Option<Color>,
    // 渡した順に描画する
    pub draw_managers: &'a mut [&'b mut dyn DrawManager],
}

// SurfaceId の発行元を区別するための番号（ResourceManager と同じ考え方、§11 ID）
static NEXT_GPU_CONTEXT_ID: AtomicU32 = AtomicU32::new(0);

pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: LonasConfig,
    surfaces: Slots<SurfaceState>,
    heap: Heap,
    renderer: Renderer,
    frame: FrameClock,
    next_manager_id: u32,
    // Object*::new(&gpu) で発行するため、&self で増やせるようにする（§2）
    next_object_id: AtomicU64,
    // デバイスが失われた理由。wgpu のコールバックが別のスレッドから書き込むことがある（§12 デバイスの喪失）
    device_lost: Arc<Mutex<Option<String>>>,
    // ウィンドウなしの GpuContext が生きている間持つ。device より後に drop するため最後に置く
    #[cfg(test)]
    _gpu_test_lock: Option<GpuTestLock>,
}

// GPU を使うテストを 1 つずつ動かすためのロック。並列にデバイスを作ると止まる環境があるため
// GpuContext を Send に保つため、MutexGuard ではなく Condvar で待つ
#[cfg(test)]
struct GpuTestLock;

#[cfg(test)]
static GPU_TEST_LOCKED: (std::sync::Mutex<bool>, std::sync::Condvar) = (std::sync::Mutex::new(false), std::sync::Condvar::new());

#[cfg(test)]
impl GpuTestLock {
    fn acquire() -> Self {
        let (locked, released) = &GPU_TEST_LOCKED;
        let mut locked = locked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        while *locked {
            locked = released.wait(locked).unwrap_or_else(std::sync::PoisonError::into_inner);
        }

        *locked = true;
        GpuTestLock
    }
}

#[cfg(test)]
impl Drop for GpuTestLock {
    fn drop(&mut self) {
        let (locked, released) = &GPU_TEST_LOCKED;

        *locked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        released.notify_one();
    }
}

impl GpuContext {
    // 最初のサーフェスと設定を受け取り、そのサーフェスの SurfaceId も返す（§9・§11 複数ウィンドウ）
    // 環境に起因するエラーは Result、config の値が不正なら panic（§3）
    // window は GpuContext が持ち続けるため、所有権ごと渡す（winit なら Arc<Window>）
    pub fn new(
        window: impl HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
        width: u32,
        height: u32,
        config: LonasConfig,
    ) -> Result<(Self, SurfaceId), LonasError> {
        config.validate();

        let instance = create_instance(config.backend);
        let surface = instance.create_surface(window).map_err(LonasError::CreateSurface)?;
        let mut context = pollster::block_on(Self::with_instance(instance, Some(&surface), config))?;
        let id = context.push_surface(surface, width, height)?;

        Ok((context, id))
    }

    pub fn add_surface(
        &mut self,
        window: impl HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
        width: u32,
        height: u32,
    ) -> Result<SurfaceId, LonasError> {
        let surface = self.instance.create_surface(window).map_err(LonasError::CreateSurface)?;

        self.push_surface(surface, width, height)
    }

    // サーフェスを持たない GpuContext。GPU を使うテストで使う（PLAN.md 全体の方針）
    #[cfg(test)]
    // 作ってから drop するまで、他の GPU を使うテストを待たせる
    // 環境変数 LONAS_SKIP_GPU_TESTS があれば GPU を使わずに Err を返し、GPU を使うテストを飛ばさせる（CI 用）
    pub(crate) fn new_headless(config: LonasConfig) -> Result<Self, LonasError> {
        config.validate();

        if std::env::var_os("LONAS_SKIP_GPU_TESTS").is_some() {
            // テストだけで使うため、専用の種類は作らずに UnsupportedAdapter を借りる
            return Err(LonasError::UnsupportedAdapter { missing: "調べていません（LONAS_SKIP_GPU_TESTS が設定されています）" });
        }

        let lock = GpuTestLock::acquire();
        let mut context = pollster::block_on(Self::with_instance(create_instance(config.backend), None, config))?;
        context._gpu_test_lock = Some(lock);

        Ok(context)
    }

    async fn with_instance(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        config: LonasConfig,
    ) -> Result<Self, LonasError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                ..Default::default()
            })
            .await
            .map_err(LonasError::AdapterNotFound)?;

        if let Some(missing) = features::find_missing(adapter.features(), adapter.get_downlevel_capabilities().flags) {
            return Err(LonasError::UnsupportedAdapter { missing });
        }

        check_limits(&config, &adapter.limits())?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Lonas"),
                required_features: features::REQUIRED_FEATURES,
                // アトラスの大きさなどをアダプターの上限まで使えるようにする
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(LonasError::RequestDevice)?;

        let device_lost = Arc::new(Mutex::new(None));
        let record = device_lost.clone();
        device.set_device_lost_callback(move |reason, message| record_device_lost(&record, reason, message));

        let heap = Heap::new(&config);
        let renderer = Renderer::new(&device, &queue, &config);

        let id = NEXT_GPU_CONTEXT_ID.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            config,
            surfaces: Slots::new(id, 0),
            heap,
            renderer,
            frame: FrameClock::new(),
            next_manager_id: 0,
            next_object_id: AtomicU64::new(0),
            device_lost,
            #[cfg(test)]
            _gpu_test_lock: None,
        })
    }

    fn push_surface(&mut self, surface: wgpu::Surface<'static>, width: u32, height: u32) -> Result<SurfaceId, LonasError> {
        let anti_aliasing = self.config.initial_anti_aliasing;

        self.check_anti_aliasing(anti_aliasing)?;

        let state =
            SurfaceState::new(surface, &self.adapter, width, height, self.config.initial_present_mode, anti_aliasing)?;

        Ok(SurfaceId(self.surfaces.insert(state)))
    }

    // サーフェスを取り除き、new / add_surface で渡したウィンドウを手放す（§11 複数ウィンドウ）
    // 以降、その SurfaceId を渡すと panic。番号は再利用されるが、世代で古い ID と区別する
    pub fn remove_surface(&mut self, surface: SurfaceId) {
        self.check_surface(surface);
        self.surfaces.remove(surface.0);
    }

    // 大きさを記録するだけにし、次の begin_frame で反映する（§11 深度バッファのリサイズ時）
    pub fn resize(&mut self, surface: SurfaceId, width: u32, height: u32) {
        self.surface_mut(surface).resize(width, height);
    }

    // resize で通知された大きさ（ピクセル）
    pub fn surface_size(&self, surface: SurfaceId) -> Vector2f {
        let (width, height) = self.surface(surface).size();

        Vector2f::new(width as f32, height as f32)
    }

    // 反映は次の begin_frame（§8）
    pub fn set_present_mode(&mut self, surface: SurfaceId, mode: PresentMode) -> Result<(), LonasError> {
        self.surface_mut(surface).set_present_mode(mode)
    }

    // 反映は次の begin_frame（§11 実行中の切り替え）。描画先の作り直しは段階 8 で行う
    pub fn set_anti_aliasing(&mut self, surface: SurfaceId, anti_aliasing: AntiAliasing) -> Result<(), LonasError> {
        self.check_anti_aliasing(anti_aliasing)?;
        self.surface_mut(surface).set_anti_aliasing(anti_aliasing);

        Ok(())
    }

    // カメラのカット。そのサーフェスの TAA の履歴を無効化する（§11）
    pub fn invalidate_history(&mut self, surface: SurfaceId) {
        self.surface_mut(surface).history_invalidated = true;
    }

    // フレームの開始。フレーム番号を進め、溜まったサーフェスの変更を反映する。描画先の取得は行わない（§9）
    pub fn begin_frame(&mut self) {
        self.frame.begin();

        for surface in self.surfaces.iter_mut() {
            surface.apply_pending(&self.device);
        }
    }

    // §9 の end_frame の処理の順に行う
    // デバイスが失われていたら GPU の処理をせずに Err(DeviceLost) を返す。以降のフレームも毎回返す（§12 デバイスの喪失）
    pub fn end_frame(&mut self, resource_manager: &mut ResourceManager, surfaces: &mut [SurfaceDraw<'_, '_>]) -> Result<(), LonasError> {
        let state = self.frame.state();

        if !state.in_frame {
            panic!("begin_frame の前に end_frame が呼ばれました");
        }

        let mut seen = HashSet::new();

        // 別の GpuContext の、または削除済みの SurfaceId は、GPU の処理を始める前に panic する（§9 誤用時は panic）
        for draw in surfaces.iter() {
            self.check_surface(draw.surface);
            assert!(
                seen.insert(draw.surface),
                "同じサーフェスが end_frame に 2 回以上渡されました。1 つの SurfaceDraw の draw_managers に並べてください",
            );
        }

        if let Err(error) = self.check_device() {
            self.abandon_frame(surfaces);
            return Err(error);
        }

        let frame = state.number;
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lonas frame") });

        {
            let mut managers: Vec<&mut DrawManagerCore> =
                surfaces.iter_mut().flat_map(|draw| draw.draw_managers.iter_mut().map(|manager| manager.core())).collect();

            // 1. 前のフレームの後片付け：map し直しが終わった staging バッファを空きに戻す
            // wgpu 30 の poll の失敗（Timeout / WrongSubmissionIndex）は待つ場合のもので、Poll では起きないはず。起きたら知らせる
            // デバイスの喪失は poll ではなく、set_device_lost_callback で知る（§12 デバイスの喪失）
            if let Err(error) = self.device.poll(wgpu::PollType::Poll) {
                log::warn!("device.poll が失敗しました: {error}");
            }

            for manager in managers.iter_mut() {
                manager.staging.reclaim();
            }

            // 2. リソースの反映
            self.renderer.apply_resource_changes(&self.device, &self.queue, resource_manager);
            // このフレームの write_text は終わっている。グリフのキャッシュの追い出しとフレームの区切り（§13）
            #[cfg(feature = "text")]
            resource_manager.finish_text_frame();

            // 3. ヒープの処理。TAA を使うサーフェスがあれば、インスタンスの前のフレームの値を残す
            let keep_previous = surfaces_use_taa(&self.surfaces, &seen);
            self.heap.process(&self.device, &mut encoder, &mut managers, frame, keep_previous);
        }

        // 4. カメラ・5. Compute：DrawManager* ごとに描画の順を決め、引数とインスタンス番号のリストを書かせる
        let motion_format = self.config.taa.motion_vector_format;
        let mut prepared = Vec::new();

        for draw in surfaces.iter_mut() {
            let surface = self.surface(draw.surface);
            let (width, height) = surface.size();
            let viewport = Vector2f::new(width.max(1) as f32, height.max(1) as f32);
            let anti_aliasing = surface.anti_aliasing;
            let target = TargetFormat {
                color: surface.format(),
                sample_count: render::antialiasing::sample_count(anti_aliasing),
                motion: (anti_aliasing == AntiAliasing::Taa).then(|| render::antialiasing::motion_texture_format(motion_format)),
            };
            // TAA はフレームごとにカメラを 1 ピクセル未満ずらす（§11 TAA で必要になるもの）
            let jitter = if anti_aliasing == AntiAliasing::Taa { render::antialiasing::jitter(frame) } else { Vector2f::ZERO };
            let mut per_surface = Vec::new();

            for manager in draw.draw_managers.iter_mut() {
                let cameras = manager.camera_frames(viewport, jitter);
                let commands = &manager.core().draw_commands;
                let camera_ids: Vec<_> = commands.iter().map(|command| command.settings.camera).collect();
                let camera_slots: Vec<u32> = camera_ids.into_iter().map(|id| manager.camera_slot(id)).collect();
                let core = manager.core();

                per_surface.push(self.renderer.prepare(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &self.heap,
                    resource_manager,
                    core,
                    &cameras,
                    &camera_slots,
                    target,
                    frame,
                ));
            }

            prepared.push(per_surface);
        }

        // 6. 描画（渡されたサーフェスごと）→ アンチエイリアスの後処理
        let mut presented = Vec::new();

        for (draw, per_surface) in surfaces.iter_mut().zip(&prepared) {
            // end_frame の最初に確かめたため、必ずある
            let surface = self.surfaces.get_mut(draw.surface.0).expect("存在しない SurfaceId です");
            let Some(acquired) = surface.acquire() else { continue };
            let recreated = surface.prepare_targets(&self.device, motion_format);
            let history_valid = !recreated && !surface.history_invalidated;
            let format = surface.format();
            let targets = surface.targets();
            let mut color_load = match draw.clear_color {
                Some(color) => wgpu::LoadOp::Clear(to_wgpu_color(color)),
                None => wgpu::LoadOp::Load,
            };
            let mut motion_load = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);

            // DrawManager* ごとに描画のパスを分ける。3D は各 DrawManager3D の最初に深度をクリアする（§10）
            let pass_count = draw.draw_managers.len().max(1);

            for index in 0..pass_count {
                let manager = draw.draw_managers.get_mut(index).map(|manager| manager.core());
                let pass_targets = targets.pass_targets(&acquired.view);
                let depth_attachment = match &manager {
                    Some(manager) if manager.dimension == Dimension::Three => Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &targets.depth.1,
                        // Reversed-Z のため、奥（0.0）でクリアする（§11 深度バッファ）
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    _ => None,
                };
                let mut attachments = vec![Some(wgpu::RenderPassColorAttachment {
                    view: pass_targets.color,
                    depth_slice: None,
                    resolve_target: pass_targets.resolve,
                    ops: wgpu::Operations { load: color_load, store: wgpu::StoreOp::Store },
                })];

                // TAA：動きのベクトル。書かないシェーダーの部分は 0（動いていない）
                if let Some(motion) = pass_targets.motion {
                    attachments.push(Some(wgpu::RenderPassColorAttachment {
                        view: motion,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: motion_load, store: wgpu::StoreOp::Store },
                    }));
                }

                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Lonas draw"),
                    color_attachments: &attachments,
                    depth_stencil_attachment: depth_attachment,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                // 画面の色のクリアは、そのサーフェスの最初の DrawManager* の描画の前だけ
                color_load = wgpu::LoadOp::Load;
                motion_load = wgpu::LoadOp::Load;

                if let (Some(manager), Some(Some(prepared))) = (manager, per_surface.get(index)) {
                    self.renderer.draw(&mut pass, &self.heap, manager, prepared);
                }
            }

            self.renderer.post_process(&self.device, &self.queue, &mut encoder, targets, &acquired.view, format, &self.config.taa, history_valid);
            surface.history_invalidated = false;
            presented.extend(acquired.surface_texture);
        }

        // フレームの途中で失われた場合は、記録した命令を送らずに捨てる
        if let Err(error) = self.check_device() {
            self.abandon_frame(surfaces);
            return Err(error);
        }

        // 7. 送信：submit を 1 回行い、各サーフェスを present する
        self.queue.submit([encoder.finish()]);

        for texture in presented {
            self.queue.present(texture);
        }

        // 8. 後片付け
        self.heap.collect_garbage(frame, self.config.gc_unused_frames);
        self.renderer.collect_unused(frame, self.config.gc_unused_frames);

        for manager in surfaces.iter_mut().flat_map(|draw| draw.draw_managers.iter_mut().map(|manager| manager.core())) {
            manager.staging.recall();
            manager.finish_frame();
            manager.receive_reclaimed(self.heap.take_reclaimed(manager.id));

            for (kind, handle) in manager.take_dropped() {
                self.heap.release(kind, handle);
            }
        }

        self.frame.end();

        Ok(())
    }

    fn check_device(&self) -> Result<(), LonasError> {
        match &*self.device_lost.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(reason) => Err(LonasError::DeviceLost { reason: reason.clone() }),
            None => Ok(()),
        }
    }

    // デバイスが失われたフレームを終える。GPU には触れず、次の begin_frame と Object* の end が使える状態にだけ戻す
    fn abandon_frame(&mut self, surfaces: &mut [SurfaceDraw<'_, '_>]) {
        for manager in surfaces.iter_mut().flat_map(|draw| draw.draw_managers.iter_mut().map(|manager| manager.core())) {
            manager.finish_frame();
        }

        self.frame.end();
    }

    // テストで、デバイスが失われた状態にする（wgpu には喪失を起こす公開の手段がない）
    #[cfg(test)]
    pub(crate) fn simulate_device_lost(&self, message: &str) {
        record_device_lost(&self.device_lost, wgpu::DeviceLostReason::Unknown, message.to_string());
    }

    fn check_anti_aliasing(&self, anti_aliasing: AntiAliasing) -> Result<(), LonasError> {
        let AntiAliasing::Msaa { sample_count } = anti_aliasing else { return Ok(()) };
        // サーフェスは sRGB のフォーマットのため、その代表で調べる
        let flags = self.adapter.get_texture_format_features(wgpu::TextureFormat::Bgra8UnormSrgb).flags;

        if flags.sample_count_supported(sample_count) {
            Ok(())
        } else {
            Err(LonasError::UnsupportedSampleCount { requested: sample_count, supported: flags.supported_sample_counts() })
        }
    }

    // 別の GpuContext の ID と削除済みの ID を、別の文言で panic する（§9 誤用時は panic）
    fn check_surface(&self, id: SurfaceId) {
        assert!(self.surfaces.owns(id.0), "別の GpuContext の SurfaceId です");
        assert!(self.surfaces.get(id.0).is_some(), "削除済みの SurfaceId です");
    }

    fn surface(&self, id: SurfaceId) -> &SurfaceState {
        self.check_surface(id);
        self.surfaces.get(id.0).expect("check_surface で確かめた")
    }

    fn surface_mut(&mut self, id: SurfaceId) -> &mut SurfaceState {
        self.check_surface(id);
        self.surfaces.get_mut(id.0).expect("check_surface で確かめた")
    }

    // ---- DrawManager* / Object* が使う ----

    pub(crate) fn issue_manager_id(&mut self) -> ManagerId {
        self.next_manager_id += 1;

        ManagerId(self.next_manager_id - 1)
    }

    pub(crate) fn issue_object_id(&self) -> u64 {
        self.next_object_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn frame_clock(&self) -> FrameClock {
        self.frame.clone()
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[cfg(test)]
    pub(crate) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    #[cfg(test)]
    pub(crate) fn heap(&self) -> &Heap {
        &self.heap
    }

    #[cfg(test)]
    pub(crate) fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    // ウィンドウのない描画先を足す。描いた画像を read_pixels で読み戻せる
    #[cfg(test)]
    pub(crate) fn add_offscreen(&mut self, width: u32, height: u32) -> SurfaceId {
        SurfaceId(self.surfaces.insert(SurfaceState::new_offscreen(&self.device, width, height)))
    }

    // テストで、この GpuContext が発行した形の SurfaceId を作る
    #[cfg(test)]
    pub(crate) fn surface_id_for_test(&self, index: u32) -> SurfaceId {
        SurfaceId(crate::slots::SlotId { owner: self.surfaces.owner(), index, generation: 0 })
    }

    #[cfg(test)]
    pub(crate) fn read_pixels(&self, surface: SurfaceId) -> Vec<[u8; 4]> {
        self.surface(surface).read_pixels(&self.device, &self.queue)
    }

    pub(crate) fn config(&self) -> &LonasConfig {
        &self.config
    }
}

fn create_instance(backend: Backend) -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();

    descriptor.backends = match backend {
        Backend::Auto => wgpu::Backends::all(),
        Backend::Vulkan => wgpu::Backends::VULKAN,
        Backend::DirectX12 => wgpu::Backends::DX12,
        Backend::Metal => wgpu::Backends::METAL,
        Backend::OpenGl => wgpu::Backends::GL,
    };

    wgpu::Instance::new(descriptor)
}

// アダプターの上限による設定の検査。超えていれば Err（環境に起因、§11 エラーの方針）
fn check_limits(config: &LonasConfig, limits: &wgpu::Limits) -> Result<(), LonasError> {
    let checks = [
        ("texture_atlas.size（max_texture_dimension_2d）", config.texture_atlas.size as u64, limits.max_texture_dimension_2d as u64),
        ("texture_atlas.max_layers（max_texture_array_layers）", config.texture_atlas.max_layers as u64, limits.max_texture_array_layers as u64),
        // group は 0 から数えるため、使う数は最大の番号 + 1
        ("BindingConfig の group（max_bind_groups）", config.bindings.max_group() as u64 + 1, limits.max_bind_groups as u64),
    ];

    match checks.into_iter().find(|&(_, requested, max)| requested > max) {
        Some((limit, requested, max)) => Err(LonasError::LimitExceeded { limit, requested, max }),
        None => Ok(()),
    }
}

// sRGB の Color を、線形の値として wgpu に渡す（§11 色）
fn to_wgpu_color(color: Color) -> wgpu::Color {
    let [r, g, b, a] = color.to_linear();

    wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }
}

// デバイスの喪失を記録する。最初の理由を残す
// Destroyed は GpuContext を drop したときのもので、喪失として扱わない（Lonas 以外は device.destroy を呼べない）
fn record_device_lost(state: &Mutex<Option<String>>, reason: wgpu::DeviceLostReason, message: String) {
    if reason == wgpu::DeviceLostReason::Destroyed {
        return;
    }

    let message = if message.is_empty() { "原因は分かりません".to_string() } else { message };
    state.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(message);
}

// 渡されたサーフェスのどれかが TAA を使うか
fn surfaces_use_taa(surfaces: &Slots<SurfaceState>, used: &HashSet<SurfaceId>) -> bool {
    used.iter().any(|id| surfaces.get(id.0).is_some_and(|surface| surface.anti_aliasing == AntiAliasing::Taa))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ResourceConfig;
    use crate::config::tests::sample_config;

    // GPU のアダプターがない環境（CI など）では飛ばす
    fn headless() -> Option<GpuContext> {
        match GpuContext::new_headless(sample_config()) {
            Ok(context) => Some(context),
            Err(error) => {
                eprintln!("GPU を使うテストを飛ばします: {error}");
                None
            }
        }
    }

    #[test]
    fn frames_run_repeatedly_without_surface() {
        let Some(mut gpu) = headless() else { return };
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());

        for frame in 1..=3 {
            gpu.begin_frame();
            assert_eq!(gpu.frame.state().number, frame);
            gpu.end_frame(&mut resources, &mut []).unwrap();
        }
    }

    // begin_frame / end_frame の誤用の panic は frame.rs のテストで確かめる（GPU を使わずに済むため）

    fn panic_message(f: impl FnOnce()) -> String {
        let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("panic しませんでした");

        error.downcast_ref::<String>().cloned().or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
    }

    fn draw(surface: SurfaceId) -> SurfaceDraw<'static, 'static> {
        SurfaceDraw { surface, clear_color: Some(Color::new(1.0, 0.0, 0.0, 1.0)), draw_managers: &mut [] }
    }

    #[test]
    fn other_context_surface_panics() {
        let Some(mut gpu) = headless() else { return };
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let own = gpu.add_offscreen(4, 4);
        // 番号と世代は同じで、発行元だけが違う ID（2 つ目の GpuContext はテストのロックで同時に作れないため、直接作る）
        let other = SurfaceId(crate::slots::SlotId { owner: own.0.owner.wrapping_add(1), ..own.0 });

        assert!(panic_message(|| { gpu.surface_size(other); }).contains("別の GpuContext"));
        assert!(panic_message(|| gpu.remove_surface(other)).contains("別の GpuContext"));

        gpu.begin_frame();
        assert!(panic_message(|| { let _ = gpu.end_frame(&mut resources, &mut [draw(other)]); }).contains("別の GpuContext"));
    }

    #[test]
    fn removed_surface_id_is_rejected_after_reuse() {
        let Some(mut gpu) = headless() else { return };
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let old = gpu.add_offscreen(4, 4);

        gpu.remove_surface(old);
        assert!(panic_message(|| { gpu.surface_size(old); }).contains("削除済み"));
        assert!(panic_message(|| gpu.remove_surface(old)).contains("削除済み"));

        // 番号は再利用されるが、古い ID は使えない
        let new = gpu.add_offscreen(8, 8);
        assert_eq!(new.0.index, old.0.index);
        assert_eq!(gpu.surface_size(new), Vector2f::new(8.0, 8.0));

        gpu.begin_frame();
        gpu.end_frame(&mut resources, &mut [draw(new)]).unwrap();
        assert!(gpu.read_pixels(new).iter().all(|&pixel| pixel == [255, 0, 0, 255]));

        gpu.begin_frame();
        assert!(panic_message(|| { let _ = gpu.end_frame(&mut resources, &mut [draw(old)]); }).contains("削除済み"));
    }

    #[test]
    fn device_lost_is_reported_every_frame() {
        let Some(mut gpu) = headless() else { return };
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let surface = gpu.add_offscreen(4, 4);

        gpu.begin_frame();
        gpu.end_frame(&mut resources, &mut [draw(surface)]).unwrap();
        gpu.simulate_device_lost("テストで失わせた");

        // 一度だけでなく、以降のフレームも毎回 Err を返す。begin_frame はそのまま使える
        for _ in 0..2 {
            gpu.begin_frame();
            let green = SurfaceDraw { surface, clear_color: Some(Color::new(0.0, 1.0, 0.0, 1.0)), draw_managers: &mut [] };

            match gpu.end_frame(&mut resources, &mut [green]) {
                Err(LonasError::DeviceLost { reason }) => assert_eq!(reason, "テストで失わせた"),
                other => panic!("DeviceLost になりませんでした: {other:?}"),
            }
        }

        // 失われた後のフレームは GPU に送らないため、画面は最初の赤のまま
        assert!(gpu.read_pixels(surface).iter().all(|&pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn device_lost_records_first_reason_and_ignores_destroy() {
        let state = Mutex::new(None);

        // GpuContext の drop による Destroyed は喪失ではない
        record_device_lost(&state, wgpu::DeviceLostReason::Destroyed, "dropped".to_string());
        assert_eq!(*state.lock().unwrap(), None);

        record_device_lost(&state, wgpu::DeviceLostReason::Unknown, String::new());
        record_device_lost(&state, wgpu::DeviceLostReason::Unknown, "2 回目".to_string());
        assert_eq!(state.lock().unwrap().as_deref(), Some("原因は分かりません"));
    }

    #[test]
    fn limits_are_checked() {
        let limits = wgpu::Limits::default();
        let mut config = sample_config();

        assert!(check_limits(&config, &limits).is_ok());

        config.texture_atlas.size = limits.max_texture_dimension_2d + 1;
        assert!(matches!(check_limits(&config, &limits), Err(LonasError::LimitExceeded { .. })));

        let mut config = sample_config();
        config.bindings.previous_instances.group = limits.max_bind_groups;
        assert!(matches!(check_limits(&config, &limits), Err(LonasError::LimitExceeded { .. })));
    }

    #[test]
    fn clear_color_is_linear() {
        let color = to_wgpu_color(Color::new(0.5, 0.0, 1.0, 0.5));

        assert!((color.r - 0.21404).abs() < 1e-4);
        assert_eq!((color.g, color.b, color.a), (0.0, 1.0, 0.5));
    }
}
