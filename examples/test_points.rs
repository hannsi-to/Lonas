use std::sync::Arc;
use log::{error, info};

mod common;

use lonas::lonas::{Camera2D, Camera3D, Color, CullMode, DrawManager2D, DrawManager3D, GpuContext, Instance2D, Instance3D, InstanceId, Matrix4f, MemoryUpdateType, Object2D, Object3D, ProjectionMode, Quaternion, ResourceManager, SurfaceDraw, SurfaceId, Vector2f, Vector3f, VertexLayout, wgsl, JointType, PaintType};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};
use lonas::util_2d::point::{build_point, PointType};
use lonas::vertex::Vertex;

// 番号は common::config() の BindingConfig と合わせる
const SHADER_2D: &str = "
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance2D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;

struct VertexOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[instance_order[index]];
    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let local = position.xy * inst.scale;
    let world = vec2(local.x * c - local.y * s, local.x * s + local.y * c) + inst.position;
    return VertexOut(camera.view_projection * vec4(world, 0.0, 1.0), color * inst.color);
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> { return in.color; }
";

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
    resources: ResourceManager,
    manager_2d: DrawManager2D,
    point_2d: Object2D,
    time: f32,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, common::config()).unwrap();
        let mut resources = ResourceManager::new(common::resource_config());
        let shader_2d = resources.register_shader(&format!("{}{}", wgsl::STRUCTS, SHADER_2D)).unwrap();

        let manager_2d = DrawManager2D::new(
            &mut gpu,
            Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 },
            VertexLayout::PositionColor,
        );

        // 2D：頂点は変えず、インスタンスだけを動かす
        let mut point_2d = Object2D::new(&gpu);
        point_2d
            .memory_update_type(MemoryUpdateType::Static)
            .paint_type(PaintType::Stroke {
                line_width: 1.0,
                joint_type: JointType::Miter,
                strip: false,
                dash: None,
            })
            // .paint_type(PaintType::Fill)
            .shader(shader_2d)
            .cull_mode(CullMode::None)
            .add_instance(Instance2D::default());

        Self {
            window,
            gpu,
            surface,
            resources,
            manager_2d,
            point_2d,
            time: 0.0,
        }
    }

    fn draw(&mut self) {
        self.time += 1.0 / 60.0;
        self.gpu.begin_frame();

        self.point_2d.begin();

        self.point_2d.vertex(Vertex::new_position_color(0.0,0.0,0.0, 1.0, 1.0, 1.0, 1.0));
        self.point_2d.vertex(Vertex::new_position_color(100.0,0.0,0.0, 1.0, 1.0, 1.0, 1.0));
        self.point_2d.vertex(Vertex::new_position_color(100.0,100.0,0.0, 1.0, 1.0, 1.0, 1.0));
        self.point_2d.vertex(Vertex::new_position_color(0.0,100.0,0.0, 1.0, 1.0, 1.0, 1.0));

        // self.point_2d.index(0);
        // self.point_2d.index(1);
        // self.point_2d.index(2);
        // self.point_2d.index(2);
        // self.point_2d.index(3);
        // self.point_2d.index(0);
        //
        // self.point_2d.begin_shape();
        //
        // self.point_2d.vertex(Vertex::new_position_color(200.0,200.0,0.0, 1.0, 1.0, 1.0, 1.0));
        // self.point_2d.vertex(Vertex::new_position_color(300.0,200.0,0.0, 1.0, 1.0, 1.0, 1.0));
        // self.point_2d.vertex(Vertex::new_position_color(300.0,300.0,0.0, 1.0, 1.0, 1.0, 1.0));
        // self.point_2d.vertex(Vertex::new_position_color(200.0,300.0,0.0, 1.0, 1.0, 1.0, 1.0));

        let base_vertex = self.point_2d.get_written_vertex_count() as u32;

        self.point_2d.index(base_vertex + 0);
        self.point_2d.index(base_vertex + 1);
        self.point_2d.index(base_vertex + 2);
        self.point_2d.index(base_vertex + 2);
        self.point_2d.index(base_vertex + 3);
        self.point_2d.index(base_vertex + 0);

        // self.point_2d.begin_shape();

        // build_point(&mut self.point_2d, PointType::Rect { width: 10.0, height: 10.0 }, Vertex::new_position_color(500.0, 500.0, 0.0, 1.0, 1.0, 1.0, 1.0));
        // build_point_vertex(&mut self.point_2d, PointType::Circle { radius: 20.0 }, Vertex::new_position_color(500.0, 500.0, 0.0, 1.0, 1.0, 1.0, 1.0));

        self.point_2d.end(&mut self.manager_2d);

        self.gpu.end_frame(&mut self.resources, &mut [SurfaceDraw {
            surface: self.surface,
            clear_color: Some(Color::rgb(0.1, 0.1, 0.12)),
            draw_managers: &mut [&mut self.manager_2d],
        }])
            .expect("GPU のデバイスが失われました"); // 作り直す例は test1.rs（§12 デバイスの喪失）
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let attributes = Window::default_attributes().with_title("Lonas: triangle");
            self.state = Some(State::new(Arc::new(event_loop.create_window(attributes).unwrap())));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.gpu.resize(state.surface, size.width, size.height),
            WindowEvent::RedrawRequested => {
                state.draw();
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}
