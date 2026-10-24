// util_2d の circle と polygon_rounded を目で確かめる example
// cargo run --example test_circle_polygon
//
// 1 段目：円（Fill）  … 白い円 / 円周のグラデーション / 楕円 / 270° の扇形 / 6 分割（六角形）
// 2 段目：円（Stroke）… 同じ並び。270° の弧は strip: false のため弦で閉じる
// 3 段目：角（Fill）  … None / Circle / Ellipse / Bevel / Chamfer / Inset / Concave
// 4 段目：角（Stroke）… 同じ並び
// 5 段目：へこんだ角を丸めた星（Fill・Stroke）と、角ごとに形の違う五角形（Fill・Stroke）

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

mod common;

use lonas::lonas::{
    Camera2D, Color, CullMode, DrawManager2D, GpuContext, Instance2D, JointType, MemoryUpdateType, Object2D, PaintType,
    ResourceManager, SurfaceDraw, SurfaceId, Vector2f, Vertex, VertexLayout, wgsl,
};
use lonas::util_2d::circle::{build_circle, build_circle_other_colors, CircleData};
use lonas::util_2d::polygon_rounded::build_polygon_rounded;
use lonas::util_2d::{Corner, CornerType};
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

// 1 段の中の 1 つの枠の左端（7 個並べる）
const COLUMN: [f32; 7] = [15.0, 125.0, 235.0, 345.0, 455.0, 565.0, 675.0];
const CELL: f32 = 90.0;

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
    fill: Object2D,
    stroke: Object2D,
}

fn color_vertex(x: f32, y: f32, color: Color) -> Vertex {
    Vertex::new_position_color(x, y, 0.0, color.r, color.g, color.b, color.a)
}

// 角の形ごとの色
fn corner_color(index: usize) -> Color {
    [
        Color::rgb(0.8, 0.8, 0.8),
        Color::rgb(1.0, 0.45, 0.45),
        Color::rgb(1.0, 0.75, 0.3),
        Color::rgb(0.95, 0.95, 0.35),
        Color::rgb(0.45, 0.9, 0.45),
        Color::rgb(0.4, 0.8, 1.0),
        Color::rgb(0.75, 0.55, 1.0),
    ][index]
}

const CORNER_TYPES: [CornerType; 7] = [
    CornerType::None {},
    CornerType::Circle { radius: 20.0 },
    CornerType::Ellipse { radius_x: 30.0, radius_y: 12.0 },
    CornerType::Bevel { radius: 20.0 },
    CornerType::Chamfer { radius_x: 30.0, radius_y: 12.0 },
    CornerType::Inset { radius_x: 20.0, radius_y: 20.0 },
    CornerType::Concave { radius_x: 20.0, radius_y: 20.0 },
];

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, common::config()).unwrap();
        let mut resources = ResourceManager::new();
        let shader_2d = resources.register_shader(&format!("{}{}", wgsl::STRUCTS, SHADER_2D)).unwrap();

        let manager_2d = DrawManager2D::new(
            &mut gpu,
            Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 },
            VertexLayout::PositionColor,
        );

        let new_object = |gpu: &GpuContext, paint_type: PaintType| {
            let mut object = Object2D::new(gpu);
            object
                .memory_update_type(MemoryUpdateType::Dynamic)
                .paint_type(paint_type)
                .shader(shader_2d)
                .cull_mode(CullMode::None)
                .add_instance(Instance2D::default());
            object
        };
        let fill = new_object(&gpu, PaintType::Fill);
        let stroke = new_object(&gpu, PaintType::Stroke { line_width: 2.0, joint_type: JointType::Miter, strip: false, dash: None });

        Self { window, gpu, surface, resources, manager_2d, fill, stroke }
    }

    fn draw(&mut self) {
        self.gpu.begin_frame();

        for (object, row_offset) in [(&mut self.fill, 0.0), (&mut self.stroke, 1.0)] {
            object.begin();
            draw_circles(object, 15.0 + row_offset * 110.0);
            draw_corner_types(object, 245.0 + row_offset * 110.0);
            draw_mixed(object, 475.0, row_offset);
            object.end(&mut self.manager_2d);
        }

        self.gpu.end_frame(&mut self.resources, &mut [SurfaceDraw {
            surface: self.surface,
            clear_color: Some(Color::rgb(0.1, 0.1, 0.12)),
            draw_managers: &mut [&mut self.manager_2d],
        }])
            .expect("GPU のデバイスが失われました"); // 作り直す例は test1.rs（§12 デバイスの喪失）
    }
}

// 1・2 段目：円。top は段の上端
fn draw_circles(object: &mut Object2D, top: f32) {
    let center = |column: usize| color_vertex(COLUMN[column] + CELL / 2.0, top + CELL / 2.0, Color::WHITE);
    let rainbow = [
        Color::rgb(1.0, 0.2, 0.2),
        Color::rgb(1.0, 0.9, 0.2),
        Color::rgb(0.2, 1.0, 0.3),
        Color::rgb(0.2, 0.9, 1.0),
        Color::rgb(0.3, 0.3, 1.0),
        Color::rgb(1.0, 0.3, 1.0),
    ];

    // 白い円
    build_circle(object, CircleData::new(40.0).segments(48), center(0));
    // 円周のグラデーション（中心は白）
    object.begin_shape();
    build_circle_other_colors(object, CircleData::new(40.0).segments(48), &rainbow, center(1));
    // 楕円
    object.begin_shape();
    build_circle(object, CircleData::new_xy(44.0, 22.0).segments(48), color_vertex(COLUMN[2] + CELL / 2.0, top + CELL / 2.0, Color::rgb(1.0, 0.6, 0.2)));
    // 0° から 270° の扇形（y が下向きなので時計回りに見える）
    object.begin_shape();
    build_circle(object, CircleData::new(40.0).end_angle(270.0f32.to_radians()).segments(36), color_vertex(COLUMN[3] + CELL / 2.0, top + CELL / 2.0, Color::rgb(0.5, 0.8, 1.0)));
    // 6 分割（六角形）
    object.begin_shape();
    build_circle(object, CircleData::new(40.0).segments(6), color_vertex(COLUMN[4] + CELL / 2.0, top + CELL / 2.0, Color::rgb(0.7, 0.7, 0.7)));
    object.begin_shape();
}

// 3・4 段目：角の形ごとの正方形
fn draw_corner_types(object: &mut Object2D, top: f32) {
    for (index, &corner_type) in CORNER_TYPES.iter().enumerate() {
        let (left, color) = (COLUMN[index], corner_color(index));
        let corners: Vec<Corner> = [(0.0, 0.0), (CELL, 0.0), (CELL, CELL), (0.0, CELL)]
            .iter()
            .map(|&(x, y)| Corner::new(corner_type, color_vertex(left + x, top + y, color)))
            .collect();

        build_polygon_rounded(object, &corners);
        object.begin_shape();
    }
}

// 5 段目：星と、角ごとに形の違う五角形。stroke_column は Stroke なら 1（右にずらす）
fn draw_mixed(object: &mut Object2D, top: f32, stroke_column: f32) {
    let middle = top + 55.0;

    // へこんだ角も丸めた星（10 個の角）
    let star_center = Vector2f::new(COLUMN[0] + 55.0 + stroke_column * 200.0, middle);
    let star: Vec<Corner> = (0..10)
        .map(|i| {
            let radius = if i % 2 == 0 { 50.0 } else { 22.0 };
            let angle = -PI / 2.0 + TAU * i as f32 / 10.0;
            Corner::new(
                CornerType::Circle { radius: 6.0 },
                color_vertex(star_center.x + radius * angle.cos(), star_center.y + radius * angle.sin(), Color::rgb(1.0, 0.85, 0.3)),
            )
        })
        .collect();
    build_polygon_rounded(object, &star);
    object.begin_shape();

    // 角ごとに形の違う五角形（None / Circle / Bevel / Inset / Concave）
    let pentagon_center = Vector2f::new(COLUMN[4] + 55.0 + stroke_column * 200.0, middle);
    let kinds = [CORNER_TYPES[0], CORNER_TYPES[1], CORNER_TYPES[3], CORNER_TYPES[5], CORNER_TYPES[6]];
    let pentagon: Vec<Corner> = kinds
        .iter()
        .enumerate()
        .map(|(i, &corner_type)| {
            let angle = -PI / 2.0 + TAU * i as f32 / 5.0;
            Corner::new(
                corner_type,
                color_vertex(pentagon_center.x + 52.0 * angle.cos(), pentagon_center.y + 52.0 * angle.sin(), Color::rgb(0.6, 0.9, 1.0)),
            )
        })
        .collect();
    build_polygon_rounded(object, &pentagon);
    object.begin_shape();
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let attributes = Window::default_attributes().with_title("Lonas: circle / polygon_rounded");
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
