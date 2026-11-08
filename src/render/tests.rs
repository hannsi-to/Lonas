// 描画のテスト（段階 6 の終わりの条件）。ウィンドウのない描画先に描き、画素を読み戻して確かめる
// GPU がない環境では飛ばす

use crate::color::Color;
use crate::config::tests::sample_config;
use crate::config::{Blend, CullMode};
use crate::draw::{Camera2D, Camera3D, DrawManager2D, DrawManager3D, ProjectionMode};
use crate::draw::DrawView;
use crate::gpu::{GpuContext, SurfaceDraw, SurfaceId};
use crate::instance::{Instance2D, Instance3D, Visibility};
use crate::math::{Matrix4f, Quaternion, Vector2f, Vector3f};
use crate::memory::MemoryUpdateType;
use crate::object::{Dash, JointType, Object2D, Object3D, PaintType};
use crate::resource::{ResourceConfig, ResourceManager, ShaderId};
use crate::vertex::{Vertex, VertexLayout};
use crate::wgsl::STRUCTS;

const SIZE: u32 = 64;

const SHADER_2D: &str = "
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance2D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;

struct VertexOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[instance_order[index]];
    let world = position.xy * inst.scale + inst.position;
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

struct Scene {
    gpu: GpuContext,
    surface: SurfaceId,
    resources: ResourceManager,
    shader: ShaderId,
}

// 画素の色を確かめるため、テクスチャは補間しない（Nearest）
fn test_config() -> crate::config::LonasConfig {
    let mut config = sample_config();
    config.texture_atlas.filter = crate::config::FilterMode::Nearest;
    config
}

fn scene(shader: &str) -> Option<Scene> {
    let mut gpu = match GpuContext::new_headless(test_config()) {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("GPU を使うテストを飛ばします: {error}");
            return None;
        }
    };
    let surface = gpu.add_offscreen(SIZE, SIZE);
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());
    let shader = resources.register_shader(&format!("{STRUCTS}{shader}")).unwrap();

    Some(Scene { gpu, surface, resources, shader })
}

fn pixel(pixels: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    pixels[(y * SIZE + x) as usize]
}

// 色が近いか（sRGB の 8 ビットで ±3）
fn assert_color(actual: [u8; 4], expected: [u8; 3], message: &str) {
    let close = actual.iter().zip(expected).all(|(&a, e)| (a as i32 - e as i32).abs() <= 3);
    assert!(close, "{message}: {actual:?} は {expected:?} に近くありません");
}

// 0〜size の正方形（2D は左上が原点、y が下向き）
fn square_2d(object: &mut Object2D, size: f32) {
    for (x, y) in [(0.0, 0.0), (size, 0.0), (size, size), (0.0, size)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
}

fn square_object_2d(scene: &Scene, color: Color, position: Vector2f) -> Object2D {
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(PaintType::Fill).shader(scene.shader).cull_mode(CullMode::None);
    object.add_instance(Instance2D { position, color, ..Instance2D::default() });
    object
}

fn draw_2d(scene: &mut Scene, manager: &mut DrawManager2D, objects: &mut [&mut Object2D], size: f32) -> Vec<[u8; 4]> {
    scene.gpu.begin_frame();

    for object in objects.iter_mut() {
        object.begin();
        square_2d(object, size);
        object.end(manager);
    }

    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&*manager)],
    }], &mut [manager])
    .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

// ---- 線：Stroke（§10）----

// (8, 32) から (56, 32) への太さ 4 の白い横線を描く。cull_mode は既定（Back）のままにし、三角形の向きも確かめる
fn draw_stroke(scene: &mut Scene, joint_type: JointType, dash: Option<Dash>) -> Vec<[u8; 4]> {
    let mut manager = manager_2d(scene);
    let mut object = Object2D::new(&scene.gpu);
    object
        .memory_update_type(MemoryUpdateType::Dynamic)
        .paint_type(PaintType::Stroke { line_width: 4.0, joint_type, strip: true, dash })
        .shader(scene.shader);
    object.add_instance(Instance2D::default());

    scene.gpu.begin_frame();
    object.begin();
    object.vertex(Vertex::new_position_color(8.0, 32.0, 0.0, 1.0, 1.0, 1.0, 1.0));
    object.vertex(Vertex::new_position_color(56.0, 32.0, 0.0, 1.0, 1.0, 1.0, 1.0));
    object.end(&mut manager);
    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&manager)],
    }], &mut [&mut manager])
    .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

#[test]
fn stroke_draws_thick_line_with_caps() {
    let Some(mut scene) = scene(SHADER_2D) else { return };

    let butt = draw_stroke(&mut scene, JointType::Miter, None);
    // 太さ 4 なので y = 30〜33 の画素に入り、y = 35 には出ない
    assert_color(pixel(&butt, 32, 30), [255, 255, 255], "線の上側");
    assert_color(pixel(&butt, 32, 33), [255, 255, 255], "線の下側");
    assert_color(pixel(&butt, 32, 35), [0, 0, 0], "線の外");
    // 端を丸めなければ、始点より手前には出ない
    assert_color(pixel(&butt, 7, 32), [0, 0, 0], "始点の手前（丸めなし）");

    let round = draw_stroke(&mut scene, JointType::RoundStartEnd, None);
    // 半径 2 の丸めで、始点・終点の 1 画素外まで塗られる
    assert_color(pixel(&round, 7, 32), [255, 255, 255], "始点の手前（丸め）");
    assert_color(pixel(&round, 56, 32), [255, 255, 255], "終点の先（丸め）");
}

#[test]
fn stroke_draws_dashes() {
    let Some(mut scene) = scene(SHADER_2D) else { return };

    // 始点 x = 8 から、長さ 8 の線と 8 の隙間をくり返す：8〜16, 24〜32, 40〜48
    let pixels = draw_stroke(&mut scene, JointType::Miter, Some(Dash { length: 8.0, gap: 8.0, offset: 0.0 }));

    for (x, white) in [(12, true), (20, false), (28, true), (36, false), (44, true), (52, false)] {
        let expected = if white { [255, 255, 255] } else { [0, 0, 0] };
        assert_color(pixel(&pixels, x, 32), expected, &format!("x = {x}"));
    }
}

// ---- 形の区切り（§10）----

// 8〜24 と 40〜56 の正方形を、begin_shape() で区切った 2 つの形として Fill で描く
fn draw_two_squares(scene: &mut Scene) -> Vec<[u8; 4]> {
    let mut manager = manager_2d(scene);
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(PaintType::Fill).shader(scene.shader);
    object.add_instance(Instance2D::default());

    scene.gpu.begin_frame();
    object.begin();
    for (i, left) in [8.0, 40.0].into_iter().enumerate() {
        if i == 1 {
            object.begin_shape();
        }
        for (x, y) in [(left, 24.0), (left + 16.0, 24.0), (left + 16.0, 40.0), (left, 40.0)] {
            object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
        }
    }
    object.end(&mut manager);
    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&manager)],
    }], &mut [&mut manager])
    .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

#[test]
fn begin_shape_draws_separate_shapes() {
    let Some(mut scene) = scene(SHADER_2D) else { return };

    let split = draw_two_squares(&mut scene);
    assert_color(pixel(&split, 16, 32), [255, 255, 255], "1 つ目の正方形");
    assert_color(pixel(&split, 48, 32), [255, 255, 255], "2 つ目の正方形");
    assert_color(pixel(&split, 32, 32), [0, 0, 0], "正方形の間");
}

#[test]
fn index_shape_and_fill_shape_are_both_drawn() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Static).paint_type(PaintType::Fill).shader(scene.shader).cull_mode(CullMode::None);
    object.add_instance(Instance2D::default());

    // 1 つ目の正方形は index() で、2 つ目は index() を呼ばずに Fill で描く（examples/test_points.rs と同じ形）
    scene.gpu.begin_frame();
    object.begin();
    for (x, y) in [(8.0, 24.0), (24.0, 24.0), (24.0, 40.0), (8.0, 40.0)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    object.index(0).index(1).index(2).index(2).index(3).index(0);
    object.begin_shape();
    for (x, y) in [(40.0, 24.0), (56.0, 24.0), (56.0, 40.0), (40.0, 40.0)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    object.end(&mut manager);
    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&manager)],
    }], &mut [&mut manager])
    .unwrap();
    let pixels = scene.gpu.read_pixels(scene.surface);

    assert_color(pixel(&pixels, 16, 32), [255, 255, 255], "index() の正方形");
    assert_color(pixel(&pixels, 48, 32), [255, 255, 255], "Fill の正方形");
    assert_color(pixel(&pixels, 32, 32), [0, 0, 0], "正方形の間");
}

#[test]
fn stroke_object_draws_index_shape_and_line_shape() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut object = Object2D::new(&scene.gpu);
    let stroke = PaintType::Stroke { line_width: 2.0, joint_type: JointType::Miter, strip: false, dash: None };
    object.memory_update_type(MemoryUpdateType::Static).paint_type(stroke).shader(scene.shader).cull_mode(CullMode::None);
    object.add_instance(Instance2D::default());

    // 1 つ目の正方形は index() で塗り、2 つ目は index() を呼ばずに線で描く（examples/test_points.rs と同じ形）
    scene.gpu.begin_frame();
    object.begin();
    for (x, y) in [(8.0, 24.0), (24.0, 24.0), (24.0, 40.0), (8.0, 40.0)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    object.index(0).index(1).index(2).index(2).index(3).index(0);
    object.begin_shape();
    for (x, y) in [(36.0, 20.0), (56.0, 20.0), (56.0, 44.0), (36.0, 44.0)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    object.end(&mut manager);
    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&manager)],
    }], &mut [&mut manager])
    .unwrap();
    let pixels = scene.gpu.read_pixels(scene.surface);

    assert_color(pixel(&pixels, 16, 32), [255, 255, 255], "index() で塗った正方形");
    assert_color(pixel(&pixels, 36, 32), [255, 255, 255], "線の正方形の左の辺");
    assert_color(pixel(&pixels, 46, 20), [255, 255, 255], "線の正方形の上の辺");
    assert_color(pixel(&pixels, 46, 32), [0, 0, 0], "線の正方形の中");
}

// ---- 穴のある多角形（§10）----

// 16〜48 の白い正方形に、28〜36 の正方形の穴を空けて描く。cull_mode は既定（Back）
fn draw_with_hole(scene: &mut Scene, paint_type: PaintType) -> Vec<[u8; 4]> {
    let mut manager = manager_2d(scene);
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(paint_type).shader(scene.shader);
    object.add_instance(Instance2D::default());

    scene.gpu.begin_frame();
    object.begin();
    for (x, y) in [(16.0, 16.0), (48.0, 16.0), (48.0, 48.0), (16.0, 48.0)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    object.begin_hole();
    for (x, y) in [(28.0, 28.0), (36.0, 28.0), (36.0, 36.0), (28.0, 36.0)] {
        object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    object.end(&mut manager);
    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&manager)],
    }], &mut [&mut manager])
    .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

#[test]
fn polygon_hole_is_not_filled() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let pixels = draw_with_hole(&mut scene, PaintType::Fill);

    assert_color(pixel(&pixels, 20, 20), [255, 255, 255], "穴の外");
    assert_color(pixel(&pixels, 44, 40), [255, 255, 255], "穴の外");
    assert_color(pixel(&pixels, 32, 32), [0, 0, 0], "穴の中");
    assert_color(pixel(&pixels, 8, 8), [0, 0, 0], "外周の外");
}

#[test]
fn stroke_outlines_holes() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let stroke = PaintType::Stroke { line_width: 2.0, joint_type: JointType::Miter, strip: false, dash: None };
    let pixels = draw_with_hole(&mut scene, stroke);

    assert_color(pixel(&pixels, 16, 32), [255, 255, 255], "外周の線");
    assert_color(pixel(&pixels, 28, 32), [255, 255, 255], "穴の線");
    assert_color(pixel(&pixels, 22, 32), [0, 0, 0], "外周と穴の間");
    assert_color(pixel(&pixels, 32, 32), [0, 0, 0], "穴の中");
}

fn manager_2d(scene: &mut Scene) -> DrawManager2D {
    DrawManager2D::new(&mut scene.gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColor)
}

// DrawManager の、そのフレームの描画の引数の instance_count
fn instance_counts(scene: &Scene, manager: &mut DrawManager2D) -> Vec<u32> {
    let resources = manager.core.single_resources();
    let count: u32 = resources.last_batches.iter().map(|&(_, n)| n).sum();
    let device = scene.gpu.device();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: count as u64 * 20,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());

    encoder.copy_buffer_to_buffer(&resources.args.buffer, 0, &readback, 0, count as u64 * 20);
    scene.gpu.queue().submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

    let data = readback.get_mapped_range(..).unwrap();
    let words: &[u32] = bytemuck::cast_slice(&data);
    words.chunks(5).map(|args| args[1]).collect()
}

#[test]
fn draws_2d_square_at_top_left() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut square = square_object_2d(&scene, Color::rgb(1.0, 0.0, 0.0), Vector2f::ZERO);

    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut square], 32.0);

    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "左上は赤");
    assert_color(pixel(&pixels, 48, 48), [0, 0, 0], "右下はクリアの色");
    // 同じ値なら次のフレームも同じに描ける（転送なし）
    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut square], 32.0);
    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "2 フレーム目も赤");
}

#[test]
fn layer_decides_2d_order() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut red = square_object_2d(&scene, Color::rgb(1.0, 0.0, 0.0), Vector2f::ZERO);
    let mut blue = square_object_2d(&scene, Color::rgb(0.0, 0.0, 1.0), Vector2f::ZERO);

    // 後に end した blue が上に描かれる
    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut red, &mut blue], 32.0);
    assert_color(pixel(&pixels, 8, 8), [0, 0, 255], "end の順");

    // layer が大きい red が上に描かれる
    red.layer(1);
    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut red, &mut blue], 32.0);
    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "layer の順");
}

#[test]
fn removed_hidden_and_offscreen_instances_are_skipped() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut object = square_object_2d(&scene, Color::rgb(1.0, 0.0, 0.0), Vector2f::ZERO);
    let right = object.add_instance(Instance2D { position: Vector2f::new(32.0, 0.0), color: Color::rgb(0.0, 1.0, 0.0), ..Instance2D::default() });
    let hidden = Instance2D { position: Vector2f::new(0.0, 32.0), visibility: Visibility::Hidden, ..Instance2D::default() };
    object.add_instance(hidden);
    // 視界の外
    object.add_instance(Instance2D { position: Vector2f::new(1000.0, 1000.0), ..Instance2D::default() });

    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut object], 32.0);
    assert_color(pixel(&pixels, 40, 8), [0, 255, 0], "右のインスタンス");
    assert_color(pixel(&pixels, 8, 40), [0, 0, 0], "非表示のインスタンス");
    // 4 つのうち、描くのは見えている 2 つだけ
    assert_eq!(instance_counts(&scene, &mut manager), vec![2]);

    object.remove_instance(right);
    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut object], 32.0);
    assert_color(pixel(&pixels, 40, 8), [0, 0, 0], "削除したインスタンス");
    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "残ったインスタンス");
}

#[test]
fn same_settings_are_batched() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut objects: Vec<Object2D> =
        (0..5).map(|i| square_object_2d(&scene, Color::WHITE, Vector2f::new(i as f32 * 10.0, 0.0))).collect();
    let mut refs: Vec<&mut Object2D> = objects.iter_mut().collect();

    draw_2d(&mut scene, &mut manager, &mut refs, 8.0);

    // 5 つの Object が 1 回の multi_draw_indexed_indirect にまとまる
    assert_eq!(manager.core.single_resources().last_batches, vec![(0, 5)]);
    assert_eq!(scene.gpu.renderer().pipeline_count(), 1);
}

// ---- 3D ----

fn manager_3d(scene: &mut Scene) -> DrawManager3D {
    let camera = Camera3D {
        position: Vector3f::new(0.0, 0.0, 5.0),
        rotation: Quaternion::IDENTITY,
        projection_mode: ProjectionMode::Perspective,
        fov_y: 1.0,
        near: 0.1,
        far: 100.0,
    };

    DrawManager3D::new(&mut scene.gpu, camera, VertexLayout::PositionColor)
}

// 画面の中央を覆う、z の位置にある正方形
fn quad_3d(scene: &Scene, color: Color, z: f32, blend: Blend) -> Object3D {
    let mut object = Object3D::new(&scene.gpu);
    object
        .memory_update_type(MemoryUpdateType::Static)
        .paint_type(PaintType::Fill)
        .shader(scene.shader)
        .blend(blend)
        .cull_mode(CullMode::None);
    object.add_instance(Instance3D { transform: Matrix4f::from_translation(Vector3f::new(0.0, 0.0, z)), color, ..Instance3D::default() });
    object
}

fn draw_3d(scene: &mut Scene, manager: &mut DrawManager3D, objects: &mut [&mut Object3D]) -> Vec<[u8; 4]> {
    scene.gpu.begin_frame();

    for object in objects.iter_mut() {
        object.begin();
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            object.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
        }
        object.end(manager);
    }

    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&*manager)],
    }], &mut [manager])
    .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

#[test]
fn opaque_3d_uses_depth() {
    let Some(mut scene) = scene(SHADER_3D) else { return };
    let mut manager = manager_3d(&mut scene);
    let mut near = quad_3d(&scene, Color::rgb(1.0, 0.0, 0.0), 0.0, Blend::Opaque);
    let mut far = quad_3d(&scene, Color::rgb(0.0, 0.0, 1.0), -1.0, Blend::Opaque);

    // 奥の物を後に描いても、手前の物が見える
    let pixels = draw_3d(&mut scene, &mut manager, &mut [&mut near, &mut far]);
    assert_color(pixel(&pixels, SIZE / 2, SIZE / 2), [255, 0, 0], "手前が見える");
}

#[test]
fn transparent_3d_is_sorted_across_objects() {
    let Some(mut scene) = scene(SHADER_3D) else { return };
    let mut manager = manager_3d(&mut scene);
    let mut near = quad_3d(&scene, Color::new(1.0, 0.0, 0.0, 0.5), 0.0, Blend::Alpha);
    let mut far = quad_3d(&scene, Color::new(0.0, 0.0, 1.0, 0.5), -1.0, Blend::Alpha);

    // end の順が逆でも、奥から手前へ重なる（手前の赤が上）
    let first = draw_3d(&mut scene, &mut manager, &mut [&mut near, &mut far]);
    let second = draw_3d(&mut scene, &mut manager, &mut [&mut far, &mut near]);
    let center = pixel(&first, SIZE / 2, SIZE / 2);

    assert_eq!(center, pixel(&second, SIZE / 2, SIZE / 2), "end の順によらない");
    // 線形で 赤 0.5、青 0.25（奥の青の上に手前の赤を半分重ねる）→ sRGB で約 (188, 0, 137)
    assert_color(center, [188, 0, 137], "奥から手前への重なり");
    // 2 つの Object の半透明は、インスタンスごとの引数（2 つ）を 1 回で描く
    assert_eq!(manager.core.single_resources().last_batches, vec![(0, 2)]);
}

#[test]
#[should_panic(expected = "登録解除済みの ShaderId")]
fn unregistered_shader_panics_at_end_frame() {
    let Some(mut scene) = scene(SHADER_2D) else { panic!("登録解除済みの ShaderId（GPU なし）") };
    let mut manager = manager_2d(&mut scene);
    let mut object = square_object_2d(&scene, Color::WHITE, Vector2f::ZERO);

    scene.resources.unregister_shader(scene.shader);
    draw_2d(&mut scene, &mut manager, &mut [&mut object], 8.0);
}

// ---- テクスチャアトラスと uniform（段階 7）----

const SHADER_TEXTURED: &str = "
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance2D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;
@group(1) @binding(0) var atlas: texture_2d_array<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;
@group(1) @binding(2) var<storage, read> texture_rects: array<TextureRect>;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) texture_slot: u32,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @location(2) uv: vec2<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[instance_order[index]];
    let world = position.xy * inst.scale + inst.position;
    return VertexOut(camera.view_projection * vec4(world, 0.0, 1.0), uv, inst.texture_slot);
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let t = texture_rects[in.texture_slot];
    return textureSampleLevel(atlas, atlas_sampler, t.rect.xy + in.uv * t.rect.zw, t.layer, 0.0);
}
";

const SHADER_UNIFORM: &str = "
struct Tint { color: vec4<f32> }

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance2D>;
@group(0) @binding(2) var<storage, read> instance_order: array<u32>;
@group(2) @binding(0) var<storage, read> uniforms: array<Tint>;

struct VertexOut { @builtin(position) clip: vec4<f32>, @location(0) @interpolate(flat) uniform_index: u32 }

@vertex
fn vs(@location(0) position: vec3<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[instance_order[index]];
    let world = position.xy * inst.scale + inst.position;
    return VertexOut(camera.view_projection * vec4(world, 0.0, 1.0), inst.uniform_index);
}

@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> { return uniforms[in.uniform_index].color; }
";

fn textured_square(scene: &Scene, position: Vector2f) -> Object2D {
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Static).paint_type(PaintType::Fill).shader(scene.shader).cull_mode(CullMode::None);
    object.add_instance(Instance2D { position, ..Instance2D::default() });
    object
}

fn draw_textured(scene: &mut Scene, manager: &mut DrawManager2D, objects: &mut [&mut Object2D]) -> Vec<[u8; 4]> {
    scene.gpu.begin_frame();

    for object in objects.iter_mut() {
        object.begin();
        for (x, y, u, v) in [(0.0, 0.0, 0.0, 0.0), (32.0, 0.0, 1.0, 0.0), (32.0, 32.0, 1.0, 1.0), (0.0, 32.0, 0.0, 1.0)] {
            object.vertex(Vertex::new_position_color_uv(x, y, 0.0, 1.0, 1.0, 1.0, 1.0, u, v));
        }
        object.end(manager);
    }

    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&*manager)],
    }], &mut [manager])
    .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

fn manager_uv(scene: &mut Scene) -> DrawManager2D {
    DrawManager2D::new(&mut scene.gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColorUv)
}

const QUADRANTS: [u8; 16] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];

#[test]
fn textured_sprite_and_missing_texture() {
    let Some(mut scene) = scene(SHADER_TEXTURED) else { return };
    let mut manager = manager_uv(&mut scene);
    // 2×2：左上 赤、右上 緑、左下 青、右下 白
    let texture = scene.resources.register_texture_rgba(2, 2, &QUADRANTS).unwrap();
    let mut sprite = textured_square(&scene, Vector2f::ZERO);
    let mut missing = textured_square(&scene, Vector2f::new(32.0, 32.0));
    sprite.texture(texture);

    let pixels = draw_textured(&mut scene, &mut manager, &mut [&mut sprite, &mut missing]);

    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "左上");
    assert_color(pixel(&pixels, 24, 8), [0, 255, 0], "右上");
    assert_color(pixel(&pixels, 8, 24), [0, 0, 255], "左下");
    assert_color(pixel(&pixels, 24, 24), [255, 255, 255], "右下");
    // texture() を呼ばなかった Object はピンク紫
    assert_color(pixel(&pixels, 48, 48), [255, 0, 255], "テクスチャ未指定");
}

#[test]
fn atlas_grows_layers_and_keeps_contents() {
    let mut config = test_config();
    config.texture_atlas.size = 64;
    config.texture_atlas.padding = 0;
    config.texture_atlas.mip_level_count = 1;
    config.texture_atlas.initial_layers = 1;
    config.texture_atlas.max_layers = 4;

    let Ok(mut gpu) = GpuContext::new_headless(config) else { return };
    let surface = gpu.add_offscreen(SIZE, SIZE);
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());
    let shader = resources.register_shader(&format!("{STRUCTS}{SHADER_TEXTURED}")).unwrap();
    let mut scene = Scene { gpu, surface, resources, shader };
    let mut manager = manager_uv(&mut scene);
    let first = scene.resources.register_texture_rgba(2, 2, &QUADRANTS).unwrap();
    let mut sprite = textured_square(&scene, Vector2f::ZERO);
    sprite.texture(first);
    draw_textured(&mut scene, &mut manager, &mut [&mut sprite]);

    // 1 レイヤーに 48×48 は 1 つしか入らないため、レイヤーが増える
    let big = vec![255; 48 * 48 * 4];
    for _ in 0..3 {
        scene.resources.register_texture_rgba(48, 48, &big).unwrap();
    }

    let pixels = draw_textured(&mut scene, &mut manager, &mut [&mut sprite]);
    assert!(scene.gpu.renderer().atlas().layer_count() >= 3);
    // レイヤーを増やすときにコピーしたため、最初のテクスチャもそのまま
    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "コピー後の左上");
    assert_color(pixel(&pixels, 24, 24), [255, 255, 255], "コピー後の右下");
}

#[test]
fn mipmaps_average_the_level_above() {
    let Some(mut scene) = scene(SHADER_TEXTURED) else { return };
    let mut manager = manager_uv(&mut scene);
    // 縦縞（赤と青が 1 画素ごと）。1 段下では平均の紫になる
    let stripes: Vec<u8> = (0..8 * 8).flat_map(|i| if i % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] }).collect();
    let texture = scene.resources.register_texture_rgba(8, 8, &stripes).unwrap();
    draw_textured(&mut scene, &mut manager, &mut []);

    let placement = scene.gpu.renderer().atlas().placement(texture.slot()).unwrap();
    let atlas = scene.gpu.renderer().atlas().texture();
    let device = scene.gpu.device();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: atlas,
            mip_level: 1,
            origin: wgpu::Origin3d { x: (placement.inner_x >> 1) + 1, y: (placement.inner_y >> 1) + 1, z: placement.layer },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: None } },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    scene.gpu.queue().submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

    let data = readback.get_mapped_range(..).unwrap();
    // 線形で 0.5 ずつ混ぜた色を sRGB にすると約 188
    assert_color([data[0], data[1], data[2], data[3]], [188, 0, 188], "ミップマップの 1 段目");
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Tint {
    color: [f32; 4],
}

fn uniform_square(scene: &Scene, position: Vector2f, color: [f32; 4]) -> Object2D {
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Static).paint_type(PaintType::Fill).shader(scene.shader).cull_mode(CullMode::None);
    object.uniform(&Tint { color });
    object.add_instance(Instance2D { position, ..Instance2D::default() });
    object
}

#[test]
fn uniforms_are_per_object_and_batched() {
    let Some(mut scene) = scene(SHADER_UNIFORM) else { return };
    let mut manager = manager_2d(&mut scene);
    let mut red = uniform_square(&scene, Vector2f::ZERO, [1.0, 0.0, 0.0, 1.0]);
    let mut blue = uniform_square(&scene, Vector2f::new(32.0, 0.0), [0.0, 0.0, 1.0, 1.0]);

    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut red, &mut blue], 32.0);
    assert_color(pixel(&pixels, 8, 8), [255, 0, 0], "赤の uniform");
    assert_color(pixel(&pixels, 40, 8), [0, 0, 255], "青の uniform");
    // uniform が違っても、同じシェーダーなら 1 回の DrawCall にまとまる
    assert_eq!(manager.core.single_resources().last_batches, vec![(0, 2)]);

    // 値を変えた Object だけ見た目が変わる
    red.uniform(&Tint { color: [0.0, 1.0, 0.0, 1.0] });
    let pixels = draw_2d(&mut scene, &mut manager, &mut [&mut red, &mut blue], 32.0);
    assert_color(pixel(&pixels, 8, 8), [0, 255, 0], "変えた uniform");
    assert_color(pixel(&pixels, 40, 8), [0, 0, 255], "変えていない uniform");
}

#[test]
#[should_panic(expected = "uniform の要素の大きさ")]
fn uniform_size_mismatch_panics() {
    let Some(mut scene) = scene(SHADER_UNIFORM) else { panic!("uniform の要素の大きさ（GPU なし）") };
    let mut manager = manager_2d(&mut scene);
    let mut object = square_object_2d(&scene, Color::WHITE, Vector2f::ZERO);
    // シェーダーの Tint は 16 バイト
    object.uniform(&[1.0f32, 2.0]);

    draw_2d(&mut scene, &mut manager, &mut [&mut object], 8.0);
}

// ---- アンチエイリアス（段階 8）----

use crate::config::AntiAliasing;

// 斜めの辺を持つ白い三角形を描き、輪郭で中間の色になった画素の数を返す
fn partial_pixels(scene: &mut Scene, manager: &mut DrawManager2D, object: &mut Object2D) -> usize {
    scene.gpu.begin_frame();
    object.begin();
    object.vertex(Vertex::new_position_color(0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0));
    object.vertex(Vertex::new_position_color(SIZE as f32, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0));
    object.vertex(Vertex::new_position_color(0.0, SIZE as f32 * 0.7, 0.0, 1.0, 1.0, 1.0, 1.0));
    object.end(manager);
    scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        views: &[DrawView::new(&*manager)],
    }], &mut [manager])
    .unwrap();

    // 履歴を 95% 使うフレームでは白や黒に近い値になるため、ほぼ白・黒も数える
    scene.gpu.read_pixels(scene.surface).iter().filter(|p| p[0] > 2 && p[0] < 253).count()
}

fn edge_scene() -> Option<(Scene, DrawManager2D, Object2D)> {
    let mut scene = scene(SHADER_2D)?;
    let manager = manager_2d(&mut scene);
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Static).paint_type(PaintType::Fill).shader(scene.shader).cull_mode(CullMode::None);
    object.add_instance(Instance2D::default());

    Some((scene, manager, object))
}

#[test]
fn each_anti_aliasing_smooths_edges() {
    let Some((mut scene, mut manager, mut object)) = edge_scene() else { return };

    // アンチエイリアスなしでは、輪郭は白か黒のどちらか
    assert_eq!(partial_pixels(&mut scene, &mut manager, &mut object), 0, "None");

    for mode in [AntiAliasing::Msaa { sample_count: 4 }, AntiAliasing::Fxaa, AntiAliasing::Taa] {
        // 実行中に切り替える。反映は次の begin_frame（§11 実行中の切り替え）
        scene.gpu.set_anti_aliasing(scene.surface, mode).unwrap();

        // TAA は履歴が貯まるまで数フレームかかる
        let mut count = 0;
        for _ in 0..8 {
            count = partial_pixels(&mut scene, &mut manager, &mut object);
        }

        assert!(count > 20, "{mode:?} で輪郭がなめらかになっていません（{count} 画素）");
    }

    // なしに戻すと、また白か黒になる
    scene.gpu.set_anti_aliasing(scene.surface, AntiAliasing::None).unwrap();
    assert_eq!(partial_pixels(&mut scene, &mut manager, &mut object), 0, "None に戻した後");
}

#[test]
fn taa_history_is_discarded_on_cut() {
    let Some((mut scene, mut manager, mut object)) = edge_scene() else { return };

    scene.gpu.set_anti_aliasing(scene.surface, AntiAliasing::Taa).unwrap();
    for _ in 0..8 {
        partial_pixels(&mut scene, &mut manager, &mut object);
    }

    // カメラのカット：そのフレームは履歴を使わず、ずらしたカメラの 1 枚だけになる（輪郭は白か黒）
    scene.gpu.invalidate_history(scene.surface);
    assert_eq!(partial_pixels(&mut scene, &mut manager, &mut object), 0, "カットの直後");
    // 次のフレームからまた混ぜる
    assert!(partial_pixels(&mut scene, &mut manager, &mut object) > 0, "カットの次のフレーム");
}

#[test]
fn unsupported_sample_count_is_error() {
    let Some((mut scene, _, _)) = edge_scene() else { return };

    assert!(matches!(
        scene.gpu.set_anti_aliasing(scene.surface, AntiAliasing::Msaa { sample_count: 3 }),
        Err(crate::error::LonasError::UnsupportedSampleCount { requested: 3, .. }),
    ));
}

// ---- 同じ DrawManager* を複数のサーフェスに描く（§11）----

// 赤の正方形（ワールドの 0〜16）と緑の正方形（64〜80）を持つ Object2D
fn red_and_green(scene: &Scene) -> Object2D {
    let mut object = square_object_2d(scene, Color::rgb(1.0, 0.0, 0.0), Vector2f::ZERO);
    object.add_instance(Instance2D { position: Vector2f::new(64.0, 0.0), color: Color::rgb(0.0, 1.0, 0.0), ..Instance2D::default() });
    object
}

// object を 1 回だけ end し、views の各サーフェスに描く。各サーフェスの画素と、ヒープへのコピー（Write）の数を返す
fn draw_views(scene: &mut Scene, manager: &mut DrawManager2D, object: &mut Object2D, views: &[(SurfaceId, DrawView)]) -> (Vec<Vec<[u8; 4]>>, u64, u64) {
    scene.gpu.begin_frame();
    object.begin();
    square_2d(object, 16.0);
    object.end(manager);

    let requested = manager.core.heap_requests.iter().filter(|request| matches!(request, crate::draw::HeapRequest::Write { .. })).count() as u64;
    let before = scene.gpu.heap().copied_writes;
    let draws: Vec<SurfaceDraw<'_>> = views
        .iter()
        .map(|(surface, view)| SurfaceDraw { surface: *surface, clear_color: Some(Color::BLACK), views: std::slice::from_ref(view) })
        .collect();

    scene.gpu.end_frame(&mut scene.resources, &draws, &mut [manager]).unwrap();

    let copied = scene.gpu.heap().copied_writes - before;
    let pixels = views.iter().map(|(surface, _)| scene.gpu.read_pixels(*surface)).collect();
    (pixels, requested, copied)
}

fn camera_at(x: f32) -> Camera2D {
    Camera2D { position: Vector2f::new(x, 0.0), rotation: 0.0, zoom: 1.0 }
}

#[test]
fn one_manager_draws_to_two_surfaces_with_different_cameras() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let second = scene.gpu.add_offscreen(SIZE, SIZE);
    let mut manager = manager_2d(&mut scene);
    let mut object = red_and_green(&scene);
    let id = manager.default_camera_id();
    let views = [(scene.surface, DrawView::new(&manager)), (second, manager.view().camera(id, camera_at(64.0)).into())];

    for frame in 0..2 {
        let (pixels, requested, copied) = draw_views(&mut scene, &mut manager, &mut object, &views);

        assert_color(pixel(&pixels[0], 8, 8), [255, 0, 0], "1 枚目は DrawManager のカメラ（赤が左上）");
        assert_color(pixel(&pixels[1], 8, 8), [0, 255, 0], "2 枚目は差し替えたカメラ（緑が左上）");
        // ヒープへの転送は、サーフェスの数によらず要求の数だけ（1 回ずつ）
        assert_eq!(copied, requested, "{frame} フレーム目の転送");
        if frame == 0 {
            assert!(requested > 0, "初回は転送がある");
        }
    }

    // DrawManager のカメラは変わらない。カメラ・引数などのバッファはサーフェスごと
    assert_eq!(*manager.camera(id), camera_at(0.0));
    assert_eq!(manager.core.resources.len(), 2);
    let cameras: Vec<&Vec<u8>> = manager.core.resources.values().map(|(resources, _)| &resources.last_camera_bytes).collect();
    assert_ne!(cameras[0], cameras[1], "前のフレームのカメラ（TAA）はサーフェスごと");
}

#[test]
fn uniforms_reach_both_surfaces() {
    let Some(mut scene) = scene(SHADER_UNIFORM) else { return };
    let second = scene.gpu.add_offscreen(SIZE, SIZE);
    let mut manager = manager_2d(&mut scene);
    let mut object = uniform_square(&scene, Vector2f::ZERO, [0.0, 0.0, 1.0, 1.0]);
    let id = manager.default_camera_id();
    let views = [(scene.surface, DrawView::new(&manager)), (second, manager.view().camera(id, camera_at(-32.0)).into())];

    let (pixels, _, _) = draw_views(&mut scene, &mut manager, &mut object, &views);

    assert_color(pixel(&pixels[0], 8, 8), [0, 0, 255], "1 枚目");
    assert_color(pixel(&pixels[1], 40, 8), [0, 0, 255], "2 枚目は右へずれる");
    assert_color(pixel(&pixels[1], 8, 8), [0, 0, 0], "2 枚目の左上は空");
}

#[test]
fn crop_splits_one_view_across_surfaces_2d() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let second = scene.gpu.add_offscreen(SIZE, SIZE);
    let mut manager = manager_2d(&mut scene);
    let mut object = red_and_green(&scene);
    let full = Vector2f::new(128.0, 64.0);
    let size = Vector2f::new(64.0, 64.0);
    let views = [
        (scene.surface, manager.view().crop(full, Vector2f::ZERO, size).into()),
        (second, manager.view().crop(full, Vector2f::new(64.0, 0.0), size).into()),
    ];

    let (pixels, _, _) = draw_views(&mut scene, &mut manager, &mut object, &views);

    assert_color(pixel(&pixels[0], 8, 8), [255, 0, 0], "左半分");
    assert_color(pixel(&pixels[1], 8, 8), [0, 255, 0], "右半分");
    assert_color(pixel(&pixels[0], 40, 8), [0, 0, 0], "左半分の右側は空");
    // 切り取った後の行列で視界を判定するため、範囲の外の正方形は省かれる
    for (surface, _) in &views {
        let resources = &manager.core.resources[surface].0;
        let count: u32 = resources.last_batches.iter().map(|&(_, n)| n).sum();
        assert_eq!(count, 1, "引数は Object ごとに 1 つ");
    }
}

#[test]
fn crop_cuts_off_center_frustum_3d() {
    let Some(mut scene) = scene(SHADER_3D) else { return };
    let second = scene.gpu.add_offscreen(SIZE, SIZE);
    let mut manager = manager_3d(&mut scene);
    // 全体（128×64、縦横比 2）の右半分に入る位置の正方形
    let mut quad = Object3D::new(&scene.gpu);
    quad.memory_update_type(MemoryUpdateType::Static).paint_type(PaintType::Fill).shader(scene.shader).cull_mode(CullMode::None);
    quad.add_instance(Instance3D {
        transform: Matrix4f::from_translation(Vector3f::new(1.5, 0.0, 0.0)) * Matrix4f::from_scale(Vector3f::new(0.5, 0.5, 1.0)),
        color: Color::rgb(1.0, 0.0, 0.0),
        ..Instance3D::default()
    });
    let full = Vector2f::new(128.0, 64.0);
    let size = Vector2f::new(64.0, 64.0);
    let left: DrawView = manager.view().crop(full, Vector2f::ZERO, size).into();
    let right: DrawView = manager.view().crop(full, Vector2f::new(64.0, 0.0), size).into();

    scene.gpu.begin_frame();
    quad.begin();
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        quad.vertex(Vertex::new_position_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0));
    }
    quad.end(&mut manager);
    scene.gpu.end_frame(&mut scene.resources, &[
        SurfaceDraw { surface: scene.surface, clear_color: Some(Color::BLACK), views: std::slice::from_ref(&left) },
        SurfaceDraw { surface: second, clear_color: Some(Color::BLACK), views: std::slice::from_ref(&right) },
    ], &mut [&mut manager])
    .unwrap();

    // 全体の行列での x の位置を、右半分のピクセルに直して確かめる
    let id = manager.default_camera_id();
    let whole = manager.view_projection(id, full) * crate::math::Vector4f::new(1.5, 0.0, 0.0, 1.0);
    let full_x = (whole.x / whole.w + 1.0) * 0.5 * full.x;
    let right_x = (full_x - 64.0) as u32;
    let left_pixels = scene.gpu.read_pixels(scene.surface);
    let right_pixels = scene.gpu.read_pixels(second);

    assert!(right_x < SIZE, "正方形は右半分にある: {full_x}");
    assert_color(pixel(&right_pixels, right_x, SIZE / 2), [255, 0, 0], "右半分に映る");
    assert_color(pixel(&right_pixels, SIZE - 2, SIZE / 2), [0, 0, 0], "右半分の右端は空");
    assert!(left_pixels.iter().all(|p| p[..3] == [0, 0, 0]), "左半分には映らない");

    // view_projection_in は切り取りを含む
    let cropped = manager.view_projection_in(&right, id, size) * crate::math::Vector4f::new(1.5, 0.0, 0.0, 1.0);
    let cropped_x = (cropped.x / cropped.w + 1.0) * 0.5 * size.x;
    assert!((cropped_x - (full_x - 64.0)).abs() < 1e-3, "{cropped_x} と {full_x}");
}

#[test]
fn surface_resources_are_dropped_after_unused_frames() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let second = scene.gpu.add_offscreen(SIZE, SIZE);
    let mut manager = manager_2d(&mut scene);
    let mut object = red_and_green(&scene);
    let both = [(scene.surface, DrawView::new(&manager)), (second, DrawView::new(&manager))];
    let only_first = [(scene.surface, DrawView::new(&manager))];

    draw_views(&mut scene, &mut manager, &mut object, &both);
    assert_eq!(manager.core.resources.len(), 2);

    for _ in 0..=scene.gpu.config().gc_unused_frames {
        draw_views(&mut scene, &mut manager, &mut object, &only_first);
    }
    assert_eq!(manager.core.resources.len(), 1, "描かなくなったサーフェスのバッファは捨てる");
}

#[test]
fn view_projection_in_uses_replaced_camera() {
    let Some(mut scene) = scene(SHADER_2D) else { return };
    let mut manager = manager_2d(&mut scene);
    let id = manager.default_camera_id();
    let other = manager.add_camera(camera_at(10.0));
    let view: DrawView = manager.view().camera(id, camera_at(64.0)).into();
    let viewport = Vector2f::new(64.0, 64.0);

    assert_eq!(manager.view_projection_in(&DrawView::new(&manager), id, viewport), manager.view_projection(id, viewport));
    let replaced = manager.add_camera(camera_at(64.0));
    assert_eq!(manager.view_projection_in(&view, id, viewport), manager.view_projection(replaced, viewport));
    // 差し替えていないカメラは DrawManager のまま
    assert_eq!(manager.view_projection_in(&view, other, viewport), manager.view_projection(other, viewport));
}

fn misuse_message(f: impl FnOnce(&mut Scene, &mut DrawManager2D, &mut DrawManager2D)) -> String {
    let Some(mut scene) = scene(SHADER_2D) else { return "GPU なし".into() };
    let mut first = manager_2d(&mut scene);
    let mut second = manager_2d(&mut scene);
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut scene, &mut first, &mut second))).unwrap_err();

    error.downcast_ref::<String>().cloned().or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string())).unwrap()
}

#[test]
fn draw_view_misuse_panics() {
    if scene(SHADER_2D).is_none() {
        return;
    }

    // draw_managers に同じ DrawManager* を 2 回渡すのは、&mut が 2 つになるためコンパイルエラー（テストは書けない）

    let message = misuse_message(|scene, first, second| {
        scene.gpu.begin_frame();
        let view = DrawView::new(second);
        let _ = scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw { surface: scene.surface, clear_color: None, views: &[view] }], &mut [first]);
    });
    assert!(message.contains("draw_managers に渡されていません"), "{message}");

    let message = misuse_message(|scene, first, _| {
        scene.gpu.begin_frame();
        let views = [DrawView::new(first), DrawView::new(first)];
        let _ = scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw { surface: scene.surface, clear_color: None, views: &views }], &mut [first]);
    });
    assert!(message.contains("同じ DrawManager の DrawView が 2 つ以上"), "{message}");

    let message = misuse_message(|scene, first, _| {
        let removed = first.add_camera(camera_at(0.0));
        let view: DrawView = first.view().camera(removed, camera_at(1.0)).into();
        first.remove_camera(removed);
        scene.gpu.begin_frame();
        let _ = scene.gpu.end_frame(&mut scene.resources, &[SurfaceDraw { surface: scene.surface, clear_color: None, views: &[view] }], &mut [first]);
    });
    assert!(message.contains("削除済みの CameraId"), "{message}");

    let message = misuse_message(|_, first, second| {
        let _ = first.view().camera(second.default_camera_id(), camera_at(0.0));
    });
    assert!(message.contains("別の DrawManager の CameraId"), "{message}");

    let message = misuse_message(|_, first, _| {
        let id = first.default_camera_id();
        let _ = first.view().camera(id, camera_at(0.0)).camera(id, camera_at(1.0));
    });
    assert!(message.contains("2 回差し替えられました"), "{message}");
}
