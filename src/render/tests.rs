// 描画のテスト（段階 6 の終わりの条件）。ウィンドウのない描画先に描き、画素を読み戻して確かめる
// GPU がない環境では飛ばす

use crate::color::Color;
use crate::config::tests::sample_config;
use crate::config::{Blend, CullMode};
use crate::draw::{Camera2D, Camera3D, DrawManager2D, DrawManager3D, ProjectionMode};
use crate::gpu::{GpuContext, SurfaceDraw, SurfaceId};
use crate::instance::{Instance2D, Instance3D, Visibility};
use crate::math::{Matrix4f, Quaternion, Vector2f, Vector3f};
use crate::memory::MemoryUpdateType;
use crate::object::{Object2D, Object3D, Topology};
use crate::resource::{ResourceManager, ShaderId};
use crate::vertex::VertexLayout;
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

fn scene(shader: &str) -> Option<Scene> {
    let mut gpu = match GpuContext::new_headless(sample_config()) {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("GPU を使うテストを飛ばします: {error}");
            return None;
        }
    };
    let surface = gpu.add_offscreen(SIZE, SIZE);
    let mut resources = ResourceManager::new();
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
        object.vertex_color(x, y, 1.0, 1.0, 1.0, 1.0);
    }
}

fn square_object_2d(scene: &Scene, color: Color, position: Vector2f) -> Object2D {
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).topology(Topology::TriangleFan).shader(scene.shader).cull_mode(CullMode::None);
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

    scene.gpu.end_frame(&mut scene.resources, &mut [SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        draw_managers: &mut [manager],
    }]);

    scene.gpu.read_pixels(scene.surface)
}

fn manager_2d(scene: &mut Scene) -> DrawManager2D {
    DrawManager2D::new(&mut scene.gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColor)
}

// DrawManager の、そのフレームの描画の引数の instance_count
fn instance_counts(scene: &Scene, manager: &mut DrawManager2D) -> Vec<u32> {
    let resources = manager.core.resources.as_ref().unwrap();
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
    assert_eq!(manager.core.resources.as_ref().unwrap().last_batches, vec![(0, 5)]);
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
        .topology(Topology::TriangleFan)
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
            object.vertex_color(x, y, 0.0, 1.0, 1.0, 1.0, 1.0);
        }
        object.end(manager);
    }

    scene.gpu.end_frame(&mut scene.resources, &mut [SurfaceDraw {
        surface: scene.surface,
        clear_color: Some(Color::BLACK),
        draw_managers: &mut [manager],
    }]);

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
    assert_eq!(manager.core.resources.as_ref().unwrap().last_batches, vec![(0, 2)]);
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
