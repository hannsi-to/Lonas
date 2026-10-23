// 段階 4 の確認用：ウィンドウを時間とともに色が変わる色で塗りつぶす
// cargo run --example clear

use std::sync::Arc;

use Lonas::color::Color;
use Lonas::config::{
    AntiAliasing, Backend, BindingConfig, BindingSlot, Blend, CoordinateSystemConfig, CullMode, FilterMode, Handedness,
    LonasConfig, MemoryCapacity, MemoryConfig, MotionVectorFormat, Origin2D, PresentMode, TaaConfig, TextureAtlasConfig,
    UpAxis,
};
use Lonas::gpu::{GpuContext, SurfaceDraw, SurfaceId};
use Lonas::resource::ResourceManager;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

fn main() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.run_app(&mut App::default()).unwrap();
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    gpu: GpuContext,
    surface: SurfaceId,
    resource_manager: ResourceManager,
    frame: u32,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window = Arc::new(event_loop.create_window(Window::default_attributes().with_title("Lonas: clear")).unwrap());
        let size = window.inner_size();
        // ウィンドウは GpuContext が持ち続けるため、Arc を複製して渡す
        let (gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, config()).unwrap();

        self.state = Some(State { window, gpu, surface, resource_manager: ResourceManager::new(), frame: 0 });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.gpu.resize(state.surface, size.width, size.height),
            WindowEvent::RedrawRequested => {
                let t = state.frame as f32 / 120.0;
                let color = Color::rgb(0.5 + 0.5 * t.sin(), 0.5 + 0.5 * (t * 1.3).sin(), 0.5 + 0.5 * (t * 1.7).sin());

                state.gpu.begin_frame();
                state.gpu.end_frame(&mut state.resource_manager, &mut [SurfaceDraw {
                    surface: state.surface,
                    clear_color: Some(color),
                    draw_managers: &mut [],
                }]);

                state.frame += 1;
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn config() -> LonasConfig {
    LonasConfig {
        redundant_call_warning: true,
        instance_growth_warning_frames: Some(60),
        memory: MemoryConfig {
            vertex: MemoryCapacity { static_capacity: 1 << 20, dynamic_initial_capacity: 1 << 20 },
            index: MemoryCapacity { static_capacity: 1 << 20, dynamic_initial_capacity: 1 << 20 },
            instance_dynamic_initial_capacity: 1 << 20,
            uniform_initial_elements: 64,
        },
        staging_chunk_size: 1 << 20,
        dynamic_growth_factor: 1.5,
        gc_unused_frames: 120,
        static_compaction_threshold: 0.25,
        blend_when_unset: Blend::Opaque,
        layer_when_unset: 0,
        cull_mode_when_unset: CullMode::Back,
        bindings: BindingConfig {
            camera: BindingSlot { group: 0, binding: 0 },
            instances: BindingSlot { group: 0, binding: 1 },
            instance_order: BindingSlot { group: 0, binding: 2 },
            atlas_texture: BindingSlot { group: 1, binding: 0 },
            atlas_sampler: BindingSlot { group: 1, binding: 1 },
            texture_rects: BindingSlot { group: 1, binding: 2 },
            uniform: BindingSlot { group: 2, binding: 0 },
            previous_camera: BindingSlot { group: 3, binding: 0 },
            previous_instances: BindingSlot { group: 3, binding: 1 },
        },
        coordinate_system: CoordinateSystemConfig {
            origin_2d: Origin2D::TopLeft,
            handedness_3d: Handedness::RightHanded,
            up_axis_3d: UpAxis::Y,
        },
        culling_2d: true,
        initial_anti_aliasing: AntiAliasing::None,
        backend: Backend::Auto,
        initial_present_mode: PresentMode::AutoVsync,
        taa: TaaConfig {
            history_blend_min: 0.1,
            history_blend_max: 0.95,
            depth_rejection_threshold: 0.01,
            color_rejection_threshold: 1.25,
            motion_vector_format: MotionVectorFormat::Rg16Float,
        },
        texture_atlas: TextureAtlasConfig {
            size: 2048,
            initial_layers: 1,
            max_layers: 4,
            padding: 2,
            mip_level_count: 4,
            filter: FilterMode::Linear,
        },
    }
}
