// DESIGN.md の API に沿ったユーザー側のサンプル
// cargo run --example test1（T：TAA への切り替えとカメラのカット、V：垂直同期の切り替え）

use lonas::lonas::{
    AntiAliasing, Backend, BindingConfig, BindingSlot, Blend, Camera2D, Camera3D, Color, CoordinateSystemConfig, CullMode,
    DrawManager2D, DrawManager3D, FilterMode, GpuContext, Handedness, Instance2D, Instance3D, InstanceId,
    LonasConfig, LonasError, Matrix4f, MemoryCapacity, MemoryConfig, MemoryUpdateType, MotionVectorFormat, Object2D,
    Object3D, Origin2D, PresentMode, ProjectionMode, Quaternion, ResourceManager, ShaderId, SurfaceDraw, SurfaceId, TaaConfig,
    TextureAtlasConfig, TextureId, Topology, Vector2f, Vector3f, VertexLayout, Visibility,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

fn main() {
    // ウィンドウとイベントループはユーザー側（winit など）が持つ（§8）
    EventLoop::new().unwrap().run_app(&mut App::default()).unwrap();
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    application: Option<TestApplication>,
    vsync: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = Arc::new(event_loop.create_window(Window::default_attributes().with_title("Lonas: test1")).unwrap());
        let size = window.inner_size();

        // ウィンドウは GpuContext が持ち続けるため、Arc を複製して渡す
        self.application = Some(TestApplication::init(window.clone(), size.width, size.height).unwrap());
        self.window = Some(window);
        self.vsync = true;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let (Some(window), Some(application)) = (&self.window, &mut self.application) else { return };

        match event {
            WindowEvent::CloseRequested => {
                application.finish();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => application.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key, state: ElementState::Pressed, .. }, .. } => {
                match logical_key {
                    Key::Character(c) if c == "t" => application.switch_scene().unwrap(),
                    Key::Character(c) if c == "v" => {
                        self.vsync = !self.vsync;
                        application.set_vsync(self.vsync).unwrap();
                    }
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                window.request_redraw();

                // デバイスが失われたら、GPU のものをすべて作り直す（§12 デバイスの喪失）
                if let Err(error) = application.draw_loop() {
                    eprintln!("{error}。作り直します");
                    self.rebuild(event_loop);
                }
            }
            _ => {}
        }
    }
}

impl App {
    fn rebuild(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else { return };
        let size = window.inner_size();

        // 古い GpuContext を先に drop し、ウィンドウのサーフェスを手放してから作り直す
        self.application = None;

        match TestApplication::init(window, size.width, size.height) {
            Ok(application) => {
                self.application = Some(application);
                self.vsync = true;
            }
            Err(error) => {
                eprintln!("作り直せませんでした: {error}");
                event_loop.exit();
            }
        }
    }
}

// 2D のシェーダーの例。Camera / Instance2D / TextureRect は lonas::wgsl::STRUCTS が定義する（§11 シェーダーとの約束事）
// 番号は下の BindingConfig と合わせる（§11 バインドグループ・バインディング番号）
const SPRITE_WGSL: &str = r#"
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance2D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;
@group(1) @binding(0) var atlas: texture_2d_array<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;
@group(1) @binding(2) var<storage, read> texture_rects: array<TextureRect>;

// @location は要素ごとに固定（§11 シェーダーとの約束事）
struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @builtin(instance_index) instance_index: u32,
}
struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) texture_slot: u32,
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    // インスタンス番号のリストを経由して読む（§10 シェーダーでのインスタンスの読み方）
    let inst = instances[instance_order[in.instance_index]];
    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let local = in.position.xy * inst.scale;
    let world = vec2(local.x * c - local.y * s, local.x * s + local.y * c) + inst.position;

    var out: VertexOut;
    out.clip = camera.view_projection * vec4(world, 0.0, 1.0);
    out.color = in.color * inst.color; // 頂点の色 × インスタンスの色（§11 Instance）
    out.uv = in.uv;
    out.texture_slot = inst.texture_slot;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let t = texture_rects[in.texture_slot];
    let uv = t.rect.xy + in.uv * t.rect.zw;
    return textureSample(atlas, atlas_sampler, uv, t.layer) * in.color;
}
"#;

// 3D のシェーダー。uniform を storage の配列で受け取る（§11 uniform）
const MESH_WGSL: &str = r#"
struct Wave { time: f32, amplitude: f32, _pad: vec2<f32> }

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance3D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;
@group(2) @binding(0) var<storage, read> uniforms: array<Wave>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @builtin(instance_index) instance_index: u32,
}
struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let inst = instances[instance_order[in.instance_index]];
    let wave = uniforms[inst.uniform_index];
    // uniform の値で床を上下に揺らし、明るさを変える
    let y = in.position.y + sin(in.position.x * 0.3 + wave.time) * wave.amplitude;
    let light = 0.6 + 0.4 * in.normal.y * (0.75 + 0.25 * sin(wave.time));

    var out: VertexOut;
    out.clip = camera.view_projection * inst.transform * vec4(in.position.x, y, in.position.z, 1.0);
    out.color = vec4(in.color.rgb * inst.color.rgb * light, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

// uniform に渡す型（bytemuck の Pod、§11 uniform）
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Wave {
    time: f32,
    amplitude: f32,
    _pad: [f32; 2],
}

pub struct TestApplication {
    gpu: GpuContext,
    main_surface: SurfaceId,
    resource_manager: ResourceManager,
    draw_manager_2d: DrawManager2D,
    draw_manager_3d: DrawManager3D,

    sprite_shader: ShaderId,
    mesh_shader: ShaderId,
    player_texture: TextureId,

    // 頂点を毎フレーム動かす 2D の三角形
    triangle: Object2D,
    // 頂点は動かさず、インスタンスだけを動かす 2D のスプライト
    sprites: Object2D,
    sprite_ids: Vec<InstanceId<Instance2D>>,
    // 3D の床
    floor: Object3D,

    x: f32,
    reverse: bool,
    time: f32,
}

impl TestApplication {
    pub fn init(
        window: impl HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
        width: u32,
        height: u32,
    ) -> Result<Self, LonasError> {
        // LonasConfig は既定値を持たないため、すべての項目を書く（§3）
        let config = LonasConfig {
            redundant_call_warning: true,
            instance_growth_warning_frames: Some(60),
            memory: MemoryConfig {
                vertex: MemoryCapacity { static_capacity: 16 << 20, dynamic_initial_capacity: 4 << 20 },
                index: MemoryCapacity { static_capacity: 8 << 20, dynamic_initial_capacity: 2 << 20 },
                instance_dynamic_initial_capacity: 4 << 20,
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
            },
            culling_2d: true,
            initial_anti_aliasing: AntiAliasing::Msaa { sample_count: 4 },
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
                size: 4096,
                initial_layers: 1,
                max_layers: 8,
                padding: 2,
                mip_level_count: 4,
                filter: FilterMode::Linear,
            },
        };

        // 環境に起因するエラーは Result（§11 エラーの方針）
        let (mut gpu, main_surface) = GpuContext::new(window, width, height, config)?;

        // シェーダー・テクスチャの登録は GPU に触れない。反映は end_frame（§11 ResourceManager）
        let mut resource_manager = ResourceManager::new();
        // Lonas が公開する struct の定義を先頭につなげる（§11 シェーダーとの約束事）
        let sprite_shader = resource_manager.register_shader(&format!("{}{}", lonas::wgsl::STRUCTS, SPRITE_WGSL))?; // WGSL の誤りはここで Err
        let mesh_shader = resource_manager.register_shader(&format!("{}{}", lonas::wgsl::STRUCTS, MESH_WGSL))?;
        let player_texture = resource_manager.register_texture(include_bytes!("assets/player.png"))?;

        // カメラは描く大きさを持たない。描くサーフェスの大きさから Lonas が求める（§11 Camera）
        let draw_manager_2d = DrawManager2D::new(
            &mut gpu,
            Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 },
            VertexLayout::PositionColorUv,
        );
        let camera_3d = Camera3D {
            position: Vector3f::new(0.0, 5.0, 10.0),
            rotation: Quaternion::IDENTITY,
            projection_mode: ProjectionMode::Perspective,
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 1000.0,
        };
        let mut draw_manager_3d = DrawManager3D::new(&mut gpu, camera_3d, VertexLayout::PositionColorUvNormal);
        // 手系が要るため、DrawManager のメソッドで rotation を設定する（§11 Camera）
        let camera = draw_manager_3d.default_camera_id();
        draw_manager_3d.look_at(camera, Vector3f::new(0.0, 0.0, 0.0), Vector3f::new(0.0, 1.0, 0.0));

        // 設定は「高頻度ではないが描画中でも呼ばれる」区分。毎フレーム呼ぶ必要はない（§1・§3）
        let mut triangle = Object2D::new(&gpu);
        triangle
            .memory_update_type(MemoryUpdateType::Dynamic) // 頂点を毎フレーム動かす
            .shader(sprite_shader)
            .topology(Topology::TriangleList)
            .blend(Blend::Alpha)
            .layer(1);
        // インスタンスが 0 個だと描かれないため、1 つ追加する（§10）
        triangle.add_instance(Instance2D {
            position: Vector2f::ZERO,
            rotation: 0.0,
            scale: Vector2f::ONE,
            color: Color::WHITE,
            visibility: Visibility::Visible,
        });

        let mut sprites = Object2D::new(&gpu);
        sprites
            .memory_update_type(MemoryUpdateType::Static) // 四角形の頂点は変えない。インスタンスは常に Dynamic（§6）
            .shader(sprite_shader)
            .texture(player_texture)
            .topology(Topology::TriangleFan)
            .blend(Blend::Alpha)
            .bounding_circle(Vector2f::new(16.0, 16.0), 23.0); // 任意。省略すると頂点から自動で計算（§10）
        let sprite_ids = (0..100)
            .map(|i| {
                sprites.add_instance(Instance2D {
                    position: Vector2f::new((i % 10) as f32 * 40.0, (i / 10) as f32 * 40.0),
                    rotation: 0.0,
                    scale: Vector2f::ONE,
                    color: Color::WHITE,
                    visibility: Visibility::Visible,
                })
            })
            .collect();

        let mut floor = Object3D::new(&gpu);
        floor
            .memory_update_type(MemoryUpdateType::Static)
            .shader(mesh_shader)
            .cull_mode(CullMode::None);
        floor.add_instance(Instance3D {
            transform: Matrix4f::IDENTITY,
            color: Color::new(0.6, 0.6, 0.6, 1.0),
            visibility: Visibility::Visible,
        });

        Ok(Self {
            gpu,
            main_surface,
            resource_manager,
            draw_manager_2d,
            draw_manager_3d,
            sprite_shader,
            mesh_shader,
            player_texture,
            triangle,
            sprites,
            sprite_ids,
            floor,
            x: 100.0,
            reverse: false,
            time: 0.0,
        })
    }

    // デバイスが失われたら Err（§12 デバイスの喪失）
    pub fn draw_loop(&mut self) -> Result<(), LonasError> {
        if self.x > 100.0 {
            self.reverse = true;
        }
        if self.x < 0.0 {
            self.reverse = false;
        }
        self.x += if self.reverse { -1.0 } else { 1.0 };
        self.time += 1.0 / 60.0;

        self.gpu.begin_frame();

        // 毎フレーム必ず呼ばれる関数群。begin / end は Object* ごとに毎フレーム 1 回ずつ（§2）
        // 頂点は前フレームの値と比べられ、変わった範囲だけ転送される
        self.triangle.begin();
        self.triangle.vertex_color(self.x, 100.0, 1.0, 0.0, 0.0, 1.0);
        self.triangle.vertex_color(self.x + 50.0, 100.0, 0.0, 1.0, 0.0, 1.0);
        self.triangle.vertex_color(self.x, 300.0, 0.0, 0.0, 1.0, 1.0);
        self.triangle.end(&mut self.draw_manager_2d);

        // 動かない物体も毎フレーム頂点を書く。変わらなければ転送されない
        self.sprites.begin();
        self.sprites.vertex_color_uv(0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0);
        self.sprites.vertex_color_uv(32.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0);
        self.sprites.vertex_color_uv(32.0, 32.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
        self.sprites.vertex_color_uv(0.0, 32.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0);
        // インスタンスの更新は変わったスロットの範囲だけ転送される（§7）
        let id = self.sprite_ids[0];
        self.sprites.update_instance(id, Instance2D {
            position: Vector2f::new(self.x, 0.0),
            rotation: self.time,
            scale: Vector2f::ONE,
            color: Color::WHITE,
            visibility: Visibility::Visible,
        });
        self.sprites.end(&mut self.draw_manager_2d);

        self.floor.begin();
        for (x, z) in [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)] {
            self.floor.vertex_color_uv_normal(x, 0.0, z, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0);
        }
        self.floor.index(0).index(1).index(2).index(0).index(2).index(3);
        // uniform は高頻度で呼んでよい。同値なら無視され、警告も出ない（§11 uniform）
        self.floor.uniform(&Wave { time: self.time, amplitude: 0.2, _pad: [0.0; 2] });
        self.floor.end(&mut self.draw_manager_3d);

        // サーフェスごとに、渡した順に描画する（3D → 2D、§9）
        self.gpu.end_frame(&mut self.resource_manager, &mut [SurfaceDraw {
            surface: self.main_surface,
            clear_color: Some(Color::new(0.1, 0.1, 0.12, 1.0)),
            draw_managers: &mut [&mut self.draw_manager_3d, &mut self.draw_manager_2d],
        }])
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        // カメラは描くサーフェスの大きさに自動で合う（§11 Camera）
        self.gpu.resize(self.main_surface, width, height);
    }

    pub fn mouse_to_world(&self, mouse: Vector2f) -> Vector2f {
        // 行列が必要な場合は、描く大きさを渡して求める（§11 Camera）
        let size = self.gpu.surface_size(self.main_surface);
        let id = self.draw_manager_2d.default_camera_id();
        let inverse = self.draw_manager_2d.view_projection(id, size).inverse().expect("カメラの行列は逆行列を持つ");
        inverse.transform_point2(mouse)
    }

    pub fn set_vsync(&mut self, enabled: bool) -> Result<(), LonasError> {
        // Present mode はサーフェスごとに変えられる。反映は次の begin_frame（§8）
        let mode = if enabled { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
        self.gpu.set_present_mode(self.main_surface, mode)
    }

    pub fn switch_scene(&mut self) -> Result<(), LonasError> {
        // 実行中にアンチエイリアスを変える。反映は次の begin_frame（§11）
        self.gpu.set_anti_aliasing(self.main_surface, AntiAliasing::Taa)?;
        // カメラのカット。TAA の履歴を捨てる（§11）
        self.gpu.invalidate_history(self.main_surface);
        Ok(())
    }

    pub fn finish(&mut self) {
        // 登録解除した ID を持つ Object* を次に end すると panic（§11 登録解除）
        self.resource_manager.unregister_texture(self.player_texture);
        self.resource_manager.unregister_shader(self.sprite_shader);
        self.resource_manager.unregister_shader(self.mesh_shader);
    }
}
