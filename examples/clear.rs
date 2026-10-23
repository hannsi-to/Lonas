// 段階 4 の確認用：ウィンドウを時間とともに色が変わる色で塗りつぶす
// cargo run --example clear

use std::sync::Arc;

mod common;

use Lonas::lonas::{Color, GpuContext, ResourceManager, SurfaceDraw, SurfaceId};
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
        let (gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, common::config()).unwrap();

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
