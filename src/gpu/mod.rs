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
use crate::draw::DrawManagerCore;
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

        let heap = Heap::new(&device, &config);

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            config,
            surfaces: Vec::new(),
            heap,
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

    // §9 の end_frame の処理の順に行う。この段階ではヒープの処理と、描画先のクリアまで
    pub fn end_frame(&mut self, _resource_manager: &mut ResourceManager, surfaces: &mut [SurfaceDraw<'_, '_>]) {
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

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Lonas frame") });

        {
            let mut managers: Vec<&mut DrawManagerCore> =
                surfaces.iter_mut().flat_map(|draw| draw.draw_managers.iter_mut().map(|manager| manager.core())).collect();

            // 1. 前のフレームの後片付け：map し直しが終わった staging バッファを空きに戻す
            let _ = self.device.poll(wgpu::PollType::Poll);

            for manager in managers.iter_mut() {
                manager.staging.reclaim();
            }

            // 3. ヒープの処理
            self.heap.process(&self.device, &mut encoder, &mut managers, state.number);
        }

        // 6. 描画（渡されたサーフェスごと）
        let mut acquired = Vec::new();

        for draw in surfaces.iter() {
            let Some(texture) = self.acquire(draw.surface) else { continue };
            let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
            let load = match draw.clear_color {
                Some(color) => wgpu::LoadOp::Clear(to_wgpu_color(color)),
                None => wgpu::LoadOp::Load,
            };

            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Lonas surface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            acquired.push(texture);
        }

        // 7. 送信：submit を 1 回行い、各サーフェスを present する
        self.queue.submit([encoder.finish()]);

        for texture in acquired {
            self.queue.present(texture);
        }

        // 8. 後片付け
        self.heap.collect_garbage(state.number, self.config.gc_unused_frames);

        for manager in surfaces.iter_mut().flat_map(|draw| draw.draw_managers.iter_mut().map(|manager| manager.core())) {
            manager.staging.recall();
            manager.finish_frame();
            manager.receive_reclaimed(self.heap.take_reclaimed(manager.id));
        }

        self.frame.end();
    }

    // 描画先を取得する。取得できない場合はそのサーフェスだけ飛ばす（§9 リサイズとフレームスキップ）
    fn acquire(&mut self, surface: SurfaceId) -> Option<wgpu::SurfaceTexture> {
        // テストのウィンドウなしの GpuContext はサーフェスを持たないため、描かずにヒープの処理だけを行う
        #[cfg(test)]
        if self.surfaces.is_empty() {
            return None;
        }

        let state = self.surface_mut(surface);

        if !state.is_drawable() {
            return None;
        }

        match state.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => Some(texture),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                state.mark_outdated();
                Some(texture)
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => None,
            wgpu::CurrentSurfaceTexture::Outdated => {
                state.mark_outdated();
                None
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("サーフェスが失われました。次のフレームで設定し直します");
                state.mark_outdated();
                None
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                log::warn!("描画先の取得で検証エラーが起きました。このフレームは描きません");
                None
            }
        }
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
