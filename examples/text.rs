// 段階 10 の確認用：書式（色・太字・斜体・下線・影・縁取り）を混ぜた日本語の文を、2D と 3D にベクターで描く
// cargo run --example text
// フォントはテスト用のもの（tests/fonts、OFL）。Noto Sans JP の部分集合のため、使える漢字は限られる

use std::sync::Arc;

mod common;

use lonas::lonas::{
    AntiAliasing, Blend, Camera2D, Camera3D, Color, CullMode, DrawManager2D, DrawManager3D, FontFamily, Formatted,
    GlyphMethod, GpuContext, Instance2D, Instance3D, InstanceId, LineHeight, Matrix4f, MemoryUpdateType, Object2D,
    Object3D, ProjectionMode, Quaternion, ResourceManager, SurfaceDraw, SurfaceId, TextAlign, TextLayout, TextStyle,
    TextWrite, Vector2f, Vector3f, VertexLayout, YAxis, write_text,
};
use lonas::wgsl::{Dimension, text_shader};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

const TEXT: &str = "§[size 40]書式の例§[/size]§[ln]\
§[red]赤い字§[/]と§[bold]太い字§[/]と§[italic]斜体§[/]、\
§[u]下線§[/]と§[u 0.04 round 2]2 本の下線§[/]と§[s]打消し§[/]、\
§[shadow 0.12]影§[/]と§[outline #000]§[yellow]縁取り§[/]と§[ghost 0.6]ghost§[/]。\
日本語の文章は、字の間で折り返して表示する。句読点は行頭に来ない。";

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
    layout_2d: TextLayout,
    layout_3d: TextLayout,
    text_2d: Object2D,
    text_3d: Object3D,
    instance_3d: InstanceId<Instance3D>,
    time: f32,
}

impl State {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let mut config = common::config();
        // ベクターの文字の縁は MSAA に頼る（§13 小さい文字）
        config.initial_anti_aliasing = AntiAliasing::Msaa { sample_count: 4 };
        let bindings = config.bindings;
        let (mut gpu, surface) = GpuContext::new(window.clone(), size.width, size.height, config).unwrap();
        let mut resources = ResourceManager::new(common::resource_config());

        let latin = resources.register_font(include_bytes!("../tests/fonts/Cantarell-Regular.ttf")).unwrap();
        let japanese = resources.register_font(include_bytes!("../tests/fonts/NotoSansJP-Subset.ttf")).unwrap();
        let shader_2d = resources.register_shader(&text_shader(&bindings, GlyphMethod::Vector, Dimension::D2)).unwrap();
        let shader_3d = resources.register_shader(&text_shader(&bindings, GlyphMethod::Vector, Dimension::D3)).unwrap();

        // 文字列か書式が変わった時だけ整形する（§13 整形と書き込みの分け方）
        let style = TextStyle {
            families: vec![FontFamily::new(latin), FontFamily::new(japanese)],
            size: 24.0,
            line_height: LineHeight::Font(1.2),
            letter_spacing: 0.0,
            max_width: Some(560.0),
            align: TextAlign::Left,
            bold_weight: 0.03,
            outline_width: 0.06,
        };
        let formatted = Formatted::parse(TEXT).unwrap();
        let layout_2d = resources.layout_text(&style, &formatted);
        let layout_3d = resources.layout_text(&TextStyle { size: 0.1, max_width: Some(2.4), align: TextAlign::Center, ..style }, &formatted);

        let manager_2d = DrawManager2D::new(
            &mut gpu,
            Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 },
            VertexLayout::PositionColor,
        );
        let camera_3d = Camera3D {
            position: Vector3f::new(0.0, 0.5, 3.0),
            rotation: Quaternion::IDENTITY,
            projection_mode: ProjectionMode::Perspective,
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 100.0,
        };
        let mut manager_3d = DrawManager3D::new(&mut gpu, camera_3d, VertexLayout::PositionColor);
        let camera = manager_3d.default_camera_id();
        manager_3d.look_at(camera, Vector3f::ZERO, Vector3f::Y);

        // テキストの Object* は Dynamic。paint_type() は呼ばない（すべての形を index() で書く）
        // Ghost で透かすため Blend::Alpha（Opaque では不透明度が捨てられる）
        let mut text_2d = Object2D::new(&gpu);
        text_2d.memory_update_type(MemoryUpdateType::Dynamic).shader(shader_2d).cull_mode(CullMode::None).blend(Blend::Alpha);
        text_2d.add_instance(Instance2D::default());

        // 3D で影・縁取りを描くため Blend::Alpha（§13 3D の影・縁取り）
        let mut text_3d = Object3D::new(&gpu);
        text_3d.memory_update_type(MemoryUpdateType::Dynamic).shader(shader_3d).cull_mode(CullMode::None).blend(Blend::Alpha);
        let instance_3d = text_3d.add_instance(Instance3D::default());

        Self {
            window,
            gpu,
            surface,
            resources,
            manager_2d,
            manager_3d,
            layout_2d,
            layout_3d,
            text_2d,
            text_3d,
            instance_3d,
            time: 0.0,
        }
    }

    fn draw(&mut self) {
        self.time += 1.0 / 60.0;
        self.gpu.begin_frame();

        // 3D：xy 平面に置き、インスタンスで回す。中央揃えの幅の半分だけ左にずらす
        let half = self.layout_3d.size();
        self.text_3d.begin();
        write_text(&mut self.text_3d, &mut self.resources, &self.layout_3d, &TextWrite {
            position: Vector3f::new(-half.x / 2.0, half.y / 2.0, 0.0),
            color: Color::WHITE,
            y_axis: YAxis::Up,
            // 画面上でおよそ 1 単位 = 300px の見込み
            pixel_scale: 300.0,
            obfuscation_seed: 0,
            method: GlyphMethod::Vector,
        });
        self.text_3d.update_instance(self.instance_3d, Instance3D {
            transform: Matrix4f::from_rotation(Quaternion::from_axis_angle(Vector3f::Y, self.time.sin() * 0.6)),
            ..Instance3D::default()
        });
        self.text_3d.end(&mut self.manager_3d);

        // 2D：左上が原点（Origin2D::TopLeft）なので YAxis::Down
        self.text_2d.begin();
        write_text(&mut self.text_2d, &mut self.resources, &self.layout_2d, &TextWrite {
            position: Vector3f::new(20.0, 20.0, 0.0),
            color: Color::WHITE,
            y_axis: YAxis::Down,
            pixel_scale: 1.0,
            obfuscation_seed: 0,
            method: GlyphMethod::Vector,
        });
        self.text_2d.end(&mut self.manager_2d);

        self.gpu
            .end_frame(&mut self.resources, &mut [SurfaceDraw {
                surface: self.surface,
                // 影は字の色の 0.25 倍で暗くなるため、暗すぎない背景にする
                clear_color: Some(Color::rgb(0.45, 0.5, 0.55)),
                draw_managers: &mut [&mut self.manager_3d, &mut self.manager_2d],
            }])
            .expect("GPU のデバイスが失われました");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            let attributes = Window::default_attributes().with_title("Lonas: text");
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
