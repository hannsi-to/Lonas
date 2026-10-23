// 段階 8 の確認用：回る細い三角形をアンチエイリアスの方式を切り替えて描く
// cargo run --example antialiasing（1：なし、2：MSAA 4x、3：FXAA、4：TAA。最初の方式は引数でも選べる：-- 4）
// シェーダーは TAA の動きのベクトルを @location(1) に書く例も兼ねる（§11 シェーダーとの約束事）

use std::sync::Arc;

mod common;

use lonas::lonas::{
    AntiAliasing, Camera3D, Color, CullMode, DrawManager3D, GpuContext, Instance3D, InstanceId, Matrix4f, MemoryUpdateType,
    Object3D, ProjectionMode, Quaternion, ResourceManager, SurfaceDraw, SurfaceId, Topology, Vector3f, VertexLayout, wgsl,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::Key;
use winit::window::{Window, WindowId};

// 番号は common::config() の BindingConfig と合わせる（前のフレームのカメラ・インスタンスは group 3）
const SHADER: &str = "
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance3D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;
@group(3) @binding(0) var<uniform> previous_camera: Camera;
@group(3) @binding(1) var<storage, read> previous_instances: array<Instance3D>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) current: vec4<f32>,
    @location(2) previous: vec4<f32>,
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @location(1) motion: vec2<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let slot = instance_order[index];
    let inst = instances[slot];
    let local = vec4(position, 1.0);

    var out: VertexOut;
    out.clip = camera.view_projection * inst.transform * local;
    out.color = color * inst.color;
    // 動きのベクトルはジッターなしの行列で求める
    out.current = camera.view_projection_no_jitter * inst.transform * local;
    out.previous = previous_camera.view_projection_no_jitter * previous_instances[slot].transform * local;
    return out;
}

@fragment
fn fs(in: VertexOut) -> FragmentOut {
    // 動きのベクトルは UV の差（現在の UV − 前のフレームの UV）
    let motion = (in.current.xy / in.current.w - in.previous.xy / in.previous.w) * vec2(0.5, -0.5);
    return FragmentOut(in.color, motion);
}
";

const MODES: [(&str, AntiAliasing); 4] = [
    ("1", AntiAliasing::None),
    ("2", AntiAliasing::Msaa { sample_count: 4 }),
    ("3", AntiAliasing::Fxaa),
    ("4", AntiAliasing::Taa),
];

fn main() {
    EventLoop::new().unwrap().run_app(&mut App::default()).unwrap();
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

struct State {
    window: Arc<Window>,
    gpu: GpuContext,
    surface: SurfaceId,
    resources: ResourceManager,
    manager: DrawManager3D,
    blades: Object3D,
    blade_ids: Vec<InstanceId<Instance3D>>,
    time: f32,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, common::config()).unwrap();
        let mut resources = ResourceManager::new();
        let shader = resources.register_shader(&format!("{}{}", wgsl::STRUCTS, SHADER)).unwrap();
        let camera = Camera3D {
            position: Vector3f::new(0.0, 0.0, 4.0),
            rotation: Quaternion::IDENTITY,
            projection_mode: ProjectionMode::Perspective,
            fov_y: 50f32.to_radians(),
            near: 0.1,
            far: 100.0,
        };
        let manager = DrawManager3D::new(&mut gpu, camera, VertexLayout::PositionColor);
        let mut blades = Object3D::new(&gpu);
        blades.memory_update_type(MemoryUpdateType::Static).topology(Topology::TriangleList).shader(shader).cull_mode(CullMode::None);
        let blade_ids = (0..24).map(|_| blades.add_instance(Instance3D::default())).collect();

        // 引数で最初の方式を選べる（例：cargo run --example antialiasing -- 4）
        let initial = std::env::args().nth(1).and_then(|arg| MODES.iter().find(|(name, _)| *name == arg)).map_or(AntiAliasing::None, |&(_, mode)| mode);
        gpu.set_anti_aliasing(surface, initial).unwrap();
        window.set_title(&title(initial));

        Self { window, gpu, surface, resources, manager, blades, blade_ids, time: 0.0 }
    }

    fn draw(&mut self) {
        self.time += 1.0 / 60.0;
        self.gpu.begin_frame();

        // 細長い三角形を放射状に並べて回す（輪郭のギザギザが目立つ形）
        self.blades.begin();
        self.blades.vertex_color(0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0);
        self.blades.vertex_color(1.6, 0.03, 0.0, 1.0, 0.8, 0.3, 1.0);
        self.blades.vertex_color(1.6, -0.03, 0.0, 0.3, 0.8, 1.0, 1.0);

        for (i, &id) in self.blade_ids.iter().enumerate() {
            let angle = self.time * 0.3 + i as f32 * std::f32::consts::TAU / self.blade_ids.len() as f32;
            self.blades.update_instance(id, Instance3D {
                transform: Matrix4f::from_rotation(Quaternion::from_axis_angle(Vector3f::Z, angle)),
                ..Instance3D::default()
            });
        }

        self.blades.end(&mut self.manager);

        self.gpu.end_frame(&mut self.resources, &mut [SurfaceDraw {
            surface: self.surface,
            clear_color: Some(Color::rgb(0.05, 0.05, 0.07)),
            draw_managers: &mut [&mut self.manager],
        }]);
    }
}

fn title(mode: AntiAliasing) -> String {
    format!("Lonas: antialiasing — {mode:?}（1：なし / 2：MSAA / 3：FXAA / 4：TAA）")
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let window = Arc::new(event_loop.create_window(Window::default_attributes()).unwrap());
            self.state = Some(State::new(window));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.gpu.resize(state.surface, size.width, size.height),
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key: Key::Character(key), state: ElementState::Pressed, .. }, .. } => {
                if let Some(&(_, mode)) = MODES.iter().find(|(name, _)| key == *name) {
                    // 反映は次の begin_frame（§11 実行中の切り替え）
                    state.gpu.set_anti_aliasing(state.surface, mode).unwrap();
                    state.window.set_title(&title(mode));
                }
            }
            WindowEvent::RedrawRequested => {
                state.draw();
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}
