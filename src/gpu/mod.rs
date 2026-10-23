// GpuContext：device / queue / サーフェス・ヒープ・フレーム境界（§6・§8・§9）
// パイプライン・Compute のパスは段階 6 以降で足す

mod features;
pub(crate) mod frame;
pub(crate) mod heap;
mod surface;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::config::{AntiAliasing, Backend, LonasConfig, PresentMode};
use crate::error::LonasError;
use crate::math::Vector2f;
use crate::resource::ResourceManager;
use crate::draw::{Dimension, DrawManagerCore};
use crate::render::Renderer;
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
        fn camera_frames(&self, viewport_size: crate::math::Vector2f) -> Vec<Option<crate::draw::CameraFrame>>;
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

pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: LonasConfig,
    surfaces: Vec<SurfaceState>,
    heap: Heap,
    renderer: Renderer,
    frame: FrameClock,
    next_manager_id: u32,
    // Object*::new(&gpu) で発行するため、&self で増やせるようにする（§2）
    next_object_id: AtomicU64,
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
    pub(crate) fn new_headless(config: LonasConfig) -> Result<Self, LonasError> {
        config.validate();

        pollster::block_on(Self::with_instance(create_instance(config.backend), None, config))
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

        let heap = Heap::new(&config);
        let renderer = Renderer::new(&device, &config);

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            config,
            surfaces: Vec::new(),
            heap,
            renderer,
            frame: FrameClock::new(),
            next_manager_id: 0,
            next_object_id: AtomicU64::new(0),
        })
    }

    fn push_surface(&mut self, surface: wgpu::Surface<'static>, width: u32, height: u32) -> Result<SurfaceId, LonasError> {
        let anti_aliasing = self.config.initial_anti_aliasing;

        self.check_anti_aliasing(anti_aliasing)?;

        let state =
            SurfaceState::new(surface, &self.adapter, width, height, self.config.initial_present_mode, anti_aliasing)?;
        self.surfaces.push(state);

        Ok(SurfaceId((self.surfaces.len() - 1) as u32))
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

        for surface in &mut self.surfaces {
            surface.apply_pending(&self.device);
        }
    }

    // §9 の end_frame の処理の順に行う
    pub fn end_frame(&mut self, resource_manager: &mut ResourceManager, surfaces: &mut [SurfaceDraw<'_, '_>]) {
        let state = self.frame.state();

        if !state.in_frame {
            panic!("begin_frame の前に end_frame が呼ばれました");
        }

        let mut seen = HashSet::new();

        for draw in surfaces.iter() {
            assert!(
                seen.insert(draw.surface),
                "同じサーフェスが end_frame に 2 回以上渡されました。1 つの SurfaceDraw の draw_managers に並べてください",
            );
        }

        let frame = state.number;
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lonas frame") });

        {
            let mut managers: Vec<&mut DrawManagerCore> =
                surfaces.iter_mut().flat_map(|draw| draw.draw_managers.iter_mut().map(|manager| manager.core())).collect();

            // 1. 前のフレームの後片付け：map し直しが終わった staging バッファを空きに戻す
            let _ = self.device.poll(wgpu::PollType::Poll);

            for manager in managers.iter_mut() {
                manager.staging.reclaim();
            }

            // 2. リソースの反映
            self.renderer.apply_resource_changes(&self.device, resource_manager);

            // 3. ヒープの処理
            self.heap.process(&self.device, &mut encoder, &mut managers, frame);
        }

        // 4. カメラ・5. Compute：DrawManager* ごとに描画の順を決め、引数とインスタンス番号のリストを書かせる
        let mut prepared = Vec::new();

        for draw in surfaces.iter_mut() {
            let surface = self.surfaces.get(draw.surface.0 as usize);
            let (width, height) = surface.map_or((1, 1), |surface| surface.size());
            let viewport = Vector2f::new(width.max(1) as f32, height.max(1) as f32);
            let format = surface.map_or(wgpu::TextureFormat::Rgba8UnormSrgb, |surface| surface.format());
            let mut per_surface = Vec::new();

            for manager in draw.draw_managers.iter_mut() {
                let cameras = manager.camera_frames(viewport);
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
                    format,
                    frame,
                ));
            }

            prepared.push(per_surface);
        }

        // 6. 描画（渡されたサーフェスごと）
        let mut presented = Vec::new();

        for (draw, per_surface) in surfaces.iter_mut().zip(&prepared) {
            let Some(surface) = self.surfaces.get_mut(draw.surface.0 as usize) else {
                // テストのウィンドウなしの GpuContext はサーフェスを持たないため、描かない
                #[cfg(test)]
                continue;
                #[cfg(not(test))]
                panic!("存在しない SurfaceId です");
            };
            let Some(target) = surface.acquire() else { continue };
            let needs_depth = draw.draw_managers.iter_mut().any(|manager| manager.core().dimension == Dimension::Three);
            let depth = needs_depth.then(|| surface.depth_view(&self.device));
            let mut color_load = match draw.clear_color {
                Some(color) => wgpu::LoadOp::Clear(to_wgpu_color(color)),
                None => wgpu::LoadOp::Load,
            };

            // DrawManager* ごとに描画のパスを分ける。3D は各 DrawManager3D の最初に深度をクリアする（§10）
            let pass_count = draw.draw_managers.len().max(1);

            for index in 0..pass_count {
                let manager = draw.draw_managers.get_mut(index).map(|manager| manager.core());
                let depth_attachment = match (&manager, &depth) {
                    (Some(manager), Some(view)) if manager.dimension == Dimension::Three => Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        // Reversed-Z のため、奥（0.0）でクリアする（§11 深度バッファ）
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    _ => None,
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Lonas draw"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: color_load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: depth_attachment,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

                // 画面の色のクリアは、そのサーフェスの最初の DrawManager* の描画の前だけ
                color_load = wgpu::LoadOp::Load;

                if let (Some(manager), Some(Some(prepared))) = (manager, per_surface.get(index)) {
                    self.renderer.draw(&mut pass, &self.heap, manager, prepared);
                }
            }

            presented.extend(target.surface_texture);
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
        }

        self.frame.end();
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

    fn surface(&self, id: SurfaceId) -> &SurfaceState {
        self.surfaces.get(id.0 as usize).expect("存在しない SurfaceId です")
    }

    fn surface_mut(&mut self, id: SurfaceId) -> &mut SurfaceState {
        self.surfaces.get_mut(id.0 as usize).expect("存在しない SurfaceId です")
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
        self.surfaces.push(SurfaceState::new_offscreen(&self.device, width, height));
        SurfaceId((self.surfaces.len() - 1) as u32)
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

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut resources = ResourceManager::new();

        for frame in 1..=3 {
            gpu.begin_frame();
            assert_eq!(gpu.frame.state().number, frame);
            gpu.end_frame(&mut resources, &mut []);
        }
    }

    // begin_frame / end_frame の誤用の panic は frame.rs のテストで確かめる（GPU を使わずに済むため）

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
