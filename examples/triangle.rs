// 段階 6 の確認用：3D の回る三角形の手前に、2D の三角形を 100 個（インスタンス）並べて動かす
// cargo run --example triangle

use std::sync::Arc;

mod common;

use lonas::lonas::{
    Camera2D, Camera3D, Color, CullMode, DrawManager2D, DrawManager3D, GpuContext, Instance2D, Instance3D, InstanceId,
    Matrix4f, MemoryUpdateType, Object2D, Object3D, ProjectionMode, Quaternion, ResourceManager, SurfaceDraw, SurfaceId,
    Topology, Vector2f, Vector3f, VertexLayout, wgsl,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

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

const SHADER_3D: &str = "
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance3D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;

struct VertexOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[instance_order[index]];
    return VertexOut(camera.view_projection * inst.transform * vec4(position, 1.0), color * inst.color);
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
    manager_3d: DrawManager3D,
    triangle_2d: Object2D,
    instances_2d: Vec<InstanceId<Instance2D>>,
    triangle_3d: Object3D,
    instance_3d: InstanceId<Instance3D>,
    time: f32,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, common::config()).unwrap();
        let mut resources = ResourceManager::new();
        let shader_2d = resources.register_shader(&format!("{}{}", wgsl::STRUCTS, SHADER_2D)).unwrap();
        let shader_3d = resources.register_shader(&format!("{}{}", wgsl::STRUCTS, SHADER_3D)).unwrap();

        let manager_2d = DrawManager2D::new(
            &mut gpu,
            Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 },
            VertexLayout::PositionColor,
        );
        let camera_3d = Camera3D {
            position: Vector3f::new(0.0, 1.0, 3.0),
            rotation: Quaternion::IDENTITY,
            projection_mode: ProjectionMode::Perspective,
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 100.0,
        };
        let mut manager_3d = DrawManager3D::new(&mut gpu, camera_3d, VertexLayout::PositionColor);
        let camera = manager_3d.default_camera_id();
        manager_3d.look_at(camera, Vector3f::ZERO, Vector3f::Y);

        // 2D：頂点は変えず、インスタンスだけを動かす
        let mut triangle_2d = Object2D::new(&gpu);
        triangle_2d
            .memory_update_type(MemoryUpdateType::Static)
            .topology(Topology::TriangleList)
            .shader(shader_2d)
            .cull_mode(CullMode::None);
        let instances_2d = (0..100)
            .map(|i| {
                triangle_2d.add_instance(Instance2D {
                    position: Vector2f::new(40.0 + (i % 10) as f32 * 60.0, 40.0 + (i / 10) as f32 * 50.0),
                    color: Color::rgb((i % 10) as f32 / 9.0, 0.5, (i / 10) as f32 / 9.0),
                    ..Instance2D::default()
                })
            })
            .collect();

        let mut triangle_3d = Object3D::new(&gpu);
        triangle_3d
            .memory_update_type(MemoryUpdateType::Static)
            .topology(Topology::TriangleList)
            .shader(shader_3d)
            .cull_mode(CullMode::None);
        let instance_3d = triangle_3d.add_instance(Instance3D::default());

        Self {
            window,
            gpu,
            surface,
            resources,
            manager_2d,
            manager_3d,
            triangle_2d,
            instances_2d,
            triangle_3d,
            instance_3d,
            time: 0.0,
        }
    }

    fn draw(&mut self) {
        self.time += 1.0 / 60.0;
        self.gpu.begin_frame();

        // 3D：頂点は毎フレーム同じ値を書く（変わらなければ転送しない）。回転はインスタンスで
        self.triangle_3d.begin();
        self.triangle_3d.vertex_color(-1.0, -0.8, 0.0, 1.0, 0.0, 0.0, 1.0);
        self.triangle_3d.vertex_color(1.0, -0.8, 0.0, 0.0, 1.0, 0.0, 1.0);
        self.triangle_3d.vertex_color(0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0);
        self.triangle_3d.update_instance(self.instance_3d, Instance3D {
            transform: Matrix4f::from_rotation(Quaternion::from_axis_angle(Vector3f::Y, self.time)),
            ..Instance3D::default()
        });
        self.triangle_3d.end(&mut self.manager_3d);

        // 2D：インスタンスを回す
        self.triangle_2d.begin();
        self.triangle_2d.vertex_color(-15.0, 12.0, 1.0, 1.0, 1.0, 1.0);
        self.triangle_2d.vertex_color(15.0, 12.0, 1.0, 1.0, 1.0, 1.0);
        self.triangle_2d.vertex_color(0.0, -15.0, 1.0, 1.0, 1.0, 1.0);

        for (i, &id) in self.instances_2d.iter().enumerate() {
            self.triangle_2d.update_instance(id, Instance2D {
                position: Vector2f::new(40.0 + (i % 10) as f32 * 60.0, 40.0 + (i / 10) as f32 * 50.0),
                rotation: self.time * (1.0 + i as f32 * 0.05),
                color: Color::rgb((i % 10) as f32 / 9.0, 0.5, (i / 10) as f32 / 9.0),
                ..Instance2D::default()
            });
        }

        self.triangle_2d.end(&mut self.manager_2d);

        // 3D の上に 2D を重ねる（§9）
        self.gpu.end_frame(&mut self.resources, &mut [SurfaceDraw {
            surface: self.surface,
            clear_color: Some(Color::rgb(0.1, 0.1, 0.12)),
            draw_managers: &mut [&mut self.manager_3d, &mut self.manager_2d],
        }]);
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
