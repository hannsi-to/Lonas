// 段階 11 の確認用：小さい UI の文字を Bitmap で、Hinting::None（左）と Hinting::Light（右）で並べて比べる
// 一番下は同じ文をベクター（MSAA なし）で描いたもの
// cargo run --example text_bitmap
// フォントはテスト用のもの（tests/fonts、OFL）

use std::sync::Arc;

mod common;

use lonas::lonas::{
    Blend, Camera2D, Color, CullMode, DrawManager2D, FontFamily, Formatted, GlyphMethod, GpuContext, Hinting, Instance2D,
    LineHeight, MemoryUpdateType, Object2D, ResourceManager, SurfaceDraw, SurfaceId, TextAlign, TextLayout, TextStyle, TextWrite,
    Vector2f, Vector3f, VertexLayout, YAxis, write_text,
};
use lonas::wgsl::{Dimension, text_shader};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const TEXT: &str = "The quick brown fox jumps over the lazy dog. 0123456789";
const SIZES: [f32; 5] = [10.0, 11.0, 12.0, 14.0, 16.0];

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
    // Bitmap は normal.x にグリフの番号を書くため PositionColorUvNormal（§13 頂点からの参照）
    bitmap_manager: DrawManager2D,
    vector_manager: DrawManager2D,
    layouts: Vec<TextLayout>,
    none: Object2D,
    light: Object2D,
    vector: Object2D,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let config = common::config();
        let bindings = config.bindings;
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, config).unwrap();
        let mut resources = ResourceManager::new(common::resource_config());

        let font = resources.register_font(include_bytes!("../tests/fonts/Cantarell-Regular.ttf")).unwrap();
        let bitmap_shader =
            resources.register_shader(&text_shader(&bindings, GlyphMethod::Bitmap { hinting: Hinting::None }, Dimension::D2)).unwrap();
        let vector_shader = resources.register_shader(&text_shader(&bindings, GlyphMethod::Vector, Dimension::D2)).unwrap();

        let layouts = SIZES
            .iter()
            .map(|&size| {
                let style = TextStyle {
                    families: vec![FontFamily::new(font)],
                    size,
                    line_height: LineHeight::Font(1.2),
                    letter_spacing: 0.0,
                    max_width: Some(380.0),
                    align: TextAlign::Left,
                    bold_weight: 0.03,
                    outline_width: 0.05,
                };
                resources.layout_text(&style, &Formatted::plain(TEXT))
            })
            .collect();

        let camera = Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 };
        let bitmap_manager = DrawManager2D::new(&mut gpu, camera, VertexLayout::PositionColorUvNormal);
        let vector_manager = DrawManager2D::new(&mut gpu, camera, VertexLayout::PositionColor);

        // 覆い率を不透明度に掛けるため Blend::Alpha（§13 Bitmap の書き込み）
        let text_object = |gpu: &GpuContext, shader| {
            let mut object = Object2D::new(gpu);
            object.memory_update_type(MemoryUpdateType::Dynamic).shader(shader).cull_mode(CullMode::None).blend(Blend::Alpha);
            object.add_instance(Instance2D::default());
            object
        };
        let none = text_object(&gpu, bitmap_shader);
        let light = text_object(&gpu, bitmap_shader);
        let vector = text_object(&gpu, vector_shader);

        Self { window, gpu, surface, resources, bitmap_manager, vector_manager, layouts, none, light, vector }
    }

    fn draw(&mut self) {
        self.gpu.begin_frame();

        let columns = [(&mut self.none, 20.0, GlyphMethod::Bitmap { hinting: Hinting::None }), (&mut self.light, 420.0, GlyphMethod::Bitmap { hinting: Hinting::Light })];

        for (object, x, method) in columns {
            object.begin();
            let mut y = 20.0;

            for layout in &self.layouts {
                write_text(object, &mut self.resources, layout, &TextWrite {
                    position: Vector3f::new(x, y, 0.0),
                    color: Color::WHITE,
                    y_axis: YAxis::Down,
                    pixel_scale: 1.0,
                    obfuscation_seed: 0,
                    method,
                });
                y += layout.size().y + 12.0;
            }

            object.end(&mut self.bitmap_manager);
        }

        self.vector.begin();
        let mut y = 300.0;
        for layout in &self.layouts {
            write_text(&mut self.vector, &mut self.resources, layout, &TextWrite {
                position: Vector3f::new(20.0, y, 0.0),
                color: Color::WHITE,
                y_axis: YAxis::Down,
                pixel_scale: 1.0,
                obfuscation_seed: 0,
                method: GlyphMethod::Vector,
            });
            y += layout.size().y + 12.0;
        }
        self.vector.end(&mut self.vector_manager);

        self.gpu
            .end_frame(&mut self.resources, &mut [SurfaceDraw {
                surface: self.surface,
                clear_color: Some(Color::rgb(0.1, 0.1, 0.12)),
                draw_managers: &mut [&mut self.bitmap_manager, &mut self.vector_manager],
            }])
            .expect("GPU のデバイスが失われました");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let attributes = Window::default_attributes().with_title("Lonas: text_bitmap（左：Hinting::None、右：Hinting::Light、下：Vector）");
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
