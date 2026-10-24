// 段階 12 の確認用：同じ字を Bitmap（左）・Sdf（中）・Msdf（右）で描き、カメラを拡大して縁を比べる
// Bitmap は 24px で作った画素を引き伸ばすためぼやけ、Sdf / Msdf は拡大しても縁がくっきりする。Msdf は角も尖ったまま
// 下の段は太字・縁取りを付けたもの（Sdf / Msdf はシェーダーで境目をずらす）
// cargo run --example text_sdf --features msdf
// フォントはテスト用のもの（tests/fonts、OFL）

use std::sync::Arc;

mod common;

use lonas::lonas::{
    Blend, Camera2D, Color, CullMode, DrawManager2D, FontFamily, Formatted, GlyphMethod, GpuContext, Hinting, Instance2D, LineHeight,
    MemoryUpdateType, Object2D, ResourceManager, SurfaceDraw, SurfaceId, TextAlign, TextLayout, TextStyle, TextWrite, Vector2f,
    Vector3f, VertexLayout, YAxis, write_text,
};
use lonas::wgsl::{Dimension, text_shader};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const METHODS: [GlyphMethod; 3] = [GlyphMethod::Bitmap { hinting: Hinting::None }, GlyphMethod::Sdf, GlyphMethod::Msdf];

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
    // Bitmap / Sdf / Msdf は normal にグリフの番号と太らせる量を書くため PositionColorUvNormal（§13 頂点からの参照）
    manager: DrawManager2D,
    plain: TextLayout,
    styled: TextLayout,
    objects: Vec<Object2D>,
    time: f32,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let config = common::config();
        let bindings = config.bindings;
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, config).unwrap();
        let mut resources = ResourceManager::new(common::resource_config());

        let font = resources.register_font(include_bytes!("../tests/fonts/Cantarell-Regular.ttf")).unwrap();
        let style = TextStyle {
            families: vec![FontFamily::new(font)],
            size: 24.0,
            line_height: LineHeight::Font(1.2),
            letter_spacing: 0.0,
            max_width: None,
            align: TextAlign::Left,
            bold_weight: 0.04,
            outline_width: 0.06,
        };
        let plain = resources.layout_text(&style, &Formatted::plain("AMag"));
        let styled = resources.layout_text(&style, &Formatted::parse("§[bold]§[outline #d04040]AMag").unwrap());

        let manager = DrawManager2D::new(&mut gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColorUvNormal);

        // 覆い率を不透明度に掛けるため Blend::Alpha（§13 Bitmap の書き込み）
        let objects = METHODS
            .iter()
            .map(|&method| {
                let shader = resources.register_shader(&text_shader(&bindings, method, Dimension::D2)).unwrap();
                let mut object = Object2D::new(&gpu);
                object.memory_update_type(MemoryUpdateType::Dynamic).shader(shader).cull_mode(CullMode::None).blend(Blend::Alpha);
                object.add_instance(Instance2D::default());
                object
            })
            .collect();

        Self { window, gpu, surface, resources, manager, plain, styled, objects, time: 0.0 }
    }

    fn draw(&mut self) {
        self.time += 1.0 / 60.0;
        self.gpu.begin_frame();

        // 1〜6 倍にゆっくり拡大・縮小する。拡大は 2D の原点（左上）を中心に行う（§11 座標系）
        let zoom = 1.0 + 5.0 * (0.5 - 0.5 * (self.time * 0.5).cos());
        let camera = self.manager.default_camera_id();
        *self.manager.camera_mut(camera) = Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom };

        for (column, (object, &method)) in self.objects.iter_mut().zip(&METHODS).enumerate() {
            object.begin();
            let x = 10.0 + column as f32 * 70.0;

            for (row, layout) in [&self.plain, &self.styled].into_iter().enumerate() {
                write_text(object, &mut self.resources, layout, &TextWrite {
                    position: Vector3f::new(x, 10.0 + row as f32 * 40.0, 0.0),
                    color: Color::WHITE,
                    y_axis: YAxis::Down,
                    // Bitmap は拡大前の大きさ（24px）で作る。拡大するとぼやけるのが比べどころ
                    pixel_scale: 1.0,
                    obfuscation_seed: 0,
                    method,
                });
            }

            object.end(&mut self.manager);
        }

        self.gpu
            .end_frame(&mut self.resources, &mut [SurfaceDraw {
                surface: self.surface,
                clear_color: Some(Color::rgb(0.12, 0.12, 0.14)),
                draw_managers: &mut [&mut self.manager],
            }])
            .expect("GPU のデバイスが失われました");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let attributes = Window::default_attributes().with_title("Lonas: text_sdf（左：Bitmap、中：Sdf、右：Msdf）");
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
