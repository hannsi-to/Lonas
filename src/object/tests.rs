// Object と DrawManager のテスト（段階 5 の終わりの条件）。GPU がない環境では飛ばす

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;
use crate::config::LonasConfig;
use crate::config::tests::sample_config;
use crate::draw::{Camera2D, Camera3D, HeapRequest, SpaceKind};
use crate::vertex::{Vertex, VertexLayout};
use crate::gpu::{SurfaceDraw, SurfaceId, heap::Heap};
use crate::math::Quaternion;
use crate::resource::ResourceManager;

// headless() が最初に足す描画先。end_frame に渡す
fn surface(gpu: &GpuContext) -> SurfaceId {
    gpu.surface_id_for_test(0)
}

fn headless(config: LonasConfig) -> Option<GpuContext> {
    match GpuContext::new_headless(config) {
        Ok(mut gpu) => {
            let added = gpu.add_offscreen(4, 4);
            assert_eq!(added, surface(&gpu));
            Some(gpu)
        }
        Err(error) => {
            eprintln!("GPU を使うテストを飛ばします: {error}");
            None
        }
    }
}

fn camera_2d() -> Camera2D {
    Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }
}

fn camera_3d() -> Camera3D {
    Camera3D {
        position: Vector3f::ZERO,
        rotation: Quaternion::IDENTITY,
        projection_mode: crate::draw::ProjectionMode::Perspective,
        fov_y: 1.0,
        near: 0.1,
        far: 100.0,
    }
}

fn end_frame(gpu: &mut GpuContext, resources: &mut ResourceManager, manager: &mut DrawManager2D) {
    let surface = surface(gpu);
    gpu.end_frame(resources, &mut [SurfaceDraw { surface, clear_color: None, draw_managers: &mut [manager] }]).unwrap();
}

fn triangle(object: &mut Object2D, x: f32) {
    object.vertex(Vertex::new_position_color(x, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0));
    object.vertex(Vertex::new_position_color(x + 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0));
    object.vertex(Vertex::new_position_color(x, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0));
}

fn new_triangle(gpu: &GpuContext, update: MemoryUpdateType) -> Object2D {
    let mut object = Object2D::new(gpu);
    object.memory_update_type(update).paint_type(PaintType::Fill);
    object.add_instance(Instance2D::default());
    object
}

// Write の要求の（位置、バイト数）。位置は要素の数
fn writes(manager: &DrawManager2D, space: SpaceKind) -> Vec<(u64, u64)> {
    manager
        .core
        .heap_requests
        .iter()
        .filter_map(|request| match request {
            HeapRequest::Write { space: s, offset, range, .. } if *s == space => Some((*offset, range.end - range.start)),
            _ => None,
        })
        .collect()
}

// GPU 上のヒープから、その Object の頂点の領域を読み戻す
fn read_vertices(gpu: &GpuContext, heap: &Heap, object: &Object2D, layout: VertexLayout) -> Vec<u8> {
    let [vertex, ..] = object.core.handles().expect("領域が確保されていません");
    let kind = SpaceKind::Vertex(layout);

    heap.read(gpu.device(), gpu.queue(), kind, heap.offset(kind, vertex), object.core.vertices().len() as u64)
}

fn expected_vertices(object: &Object2D, layout: VertexLayout) -> Vec<u8> {
    let mut bytes = vec![0; object.core.vertices().len() * layout.stride()];

    for (vertex, dst) in object.core.vertices().iter().zip(bytes.chunks_exact_mut(layout.stride())) {
        vertex.write_packed(layout, dst);
    }

    bytes
}

fn panic_message(f: impl FnOnce()) -> String {
    let error = catch_unwind(AssertUnwindSafe(f)).expect_err("panic しませんでした");

    error
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| error.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

// Vertex をそのまま受け取り、2D でも z は渡したとおりに使う（§1 頂点の入力）
#[test]
fn vertex_struct_is_stored_as_given() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::PositionColorUv);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    let vertices = [
        Vertex { x: 1.0, y: 2.0, z: 0.5, r: 1.0, a: 1.0, u: 0.25, ..Default::default() },
        Vertex::new_position_color_uv(3.0, 4.0, -0.5, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0),
        Vertex { x: 5.0, y: 6.0, a: 1.0, ..Default::default() },
    ];

    gpu.begin_frame();
    object.begin();
    for vertex in vertices {
        object.vertex(vertex);
    }
    object.end(&mut manager);

    assert_eq!(object.core.vertices(), vertices);
}

#[test]
fn only_changed_vertices_are_written() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::PositionColor);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    let stride = VertexLayout::PositionColor.stride() as u64;
    let vertices = SpaceKind::Vertex(VertexLayout::PositionColor);

    // 初回はすべてを転送する
    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    assert_eq!(writes(&manager, vertices), vec![(0, 3 * stride)]);
    assert_eq!(writes(&manager, SpaceKind::Index).len(), 1);
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 同じ値なら転送しない
    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    assert!(manager.core.heap_requests.is_empty());
    assert_eq!(manager.core.draw_commands.len(), 1);
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 2 番目の頂点だけ変えると、その範囲だけ転送する
    gpu.begin_frame();
    object.begin();
    object.vertex(Vertex::new_position_color(0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0));
    object.vertex(Vertex::new_position_color(5.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0));
    object.vertex(Vertex::new_position_color(0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0));
    object.end(&mut manager);
    assert_eq!(writes(&manager, vertices), vec![(1, stride)]);
    // Fill は位置が変わると作り直すが、三角形のつなぎ方が同じならインデックスは送らない
    assert!(writes(&manager, SpaceKind::Index).is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);
}

#[test]
fn heap_contents_match_cpu_copy_after_growth_and_compaction() {
    let mut config = sample_config();
    // Dynamic を小さくして拡張を起こし、しきい値 0 で穴ができるたびに詰め直す
    config.memory.vertex.dynamic_initial_capacity = 64;
    config.memory.vertex.static_capacity = 4096;
    config.static_compaction_threshold = 0.0;

    let Some(mut gpu) = headless(config) else { return };
    let mut resources = ResourceManager::new();
    let layout = VertexLayout::PositionColor;
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), layout);
    let mut statics: Vec<Object2D> = (0..3).map(|_| new_triangle(&gpu, MemoryUpdateType::Static)).collect();
    let mut dynamic = new_triangle(&gpu, MemoryUpdateType::Dynamic);

    for frame in 0..6 {
        gpu.begin_frame();

        for (i, object) in statics.iter_mut().enumerate() {
            // 2 フレーム目に真ん中を Dynamic にし、Static 領域に穴を作る（次の end_frame で詰め直す）
            if frame == 2 && i == 1 {
                object.memory_update_type(MemoryUpdateType::Dynamic);
            }

            object.begin();
            triangle(object, i as f32 * 10.0);
            object.end(&mut manager);
        }

        // Dynamic の頂点数を毎フレーム増やし、確保し直しと拡張を起こす
        dynamic.begin();
        // 円周上に並べ、どのフレームでも凸多角形にする
        let count = 3 + frame * 6;
        for i in 0..count {
            let angle = std::f32::consts::TAU * i as f32 / count as f32;
            dynamic.vertex(Vertex::new_position_color(10.0 * angle.cos(), 10.0 * angle.sin() + frame as f32, 0.0, 0.5, 0.5, 0.5, 1.0));
        }
        dynamic.end(&mut manager);

        end_frame(&mut gpu, &mut resources, &mut manager);

        for object in statics.iter().chain([&dynamic]) {
            assert_eq!(read_vertices(&gpu, gpu.heap(), object, layout), expected_vertices(object, layout), "フレーム {frame}");
        }
    }
}

#[test]
fn reclaimed_object_is_uploaded_again() {
    let mut config = sample_config();
    config.gc_unused_frames = 2;

    let Some(mut gpu) = headless(config) else { return };
    let mut resources = ResourceManager::new();
    let layout = VertexLayout::PositionColor;
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), layout);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Static);

    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    end_frame(&mut gpu, &mut resources, &mut manager);
    let first = object.core.handles().unwrap();

    // 2 フレーム end しないと回収される
    for _ in 0..2 {
        gpu.begin_frame();
        end_frame(&mut gpu, &mut resources, &mut manager);
    }

    // 頂点は変わっていないが、確保し直してすべてを転送する
    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);

    let allocations = manager.core.heap_requests.iter().filter(|r| matches!(r, HeapRequest::Allocate { .. })).count();
    assert_eq!(allocations, 3);
    assert_ne!(object.core.handles().unwrap(), first);
    end_frame(&mut gpu, &mut resources, &mut manager);
    assert_eq!(read_vertices(&gpu, gpu.heap(), &object, layout), expected_vertices(&object, layout));
}

#[test]
fn dropped_object_is_freed_at_end_frame() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let layout = VertexLayout::PositionColor;
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), layout);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Static);

    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    let [vertex, index, instance] = object.core.handles().unwrap();
    // end したフレームのうちに捨てても、そのフレームの描画の後で解放する
    drop(object);
    end_frame(&mut gpu, &mut resources, &mut manager);

    let heap = gpu.heap();
    assert!(!heap.contains(SpaceKind::Vertex(layout), vertex));
    assert!(!heap.contains(SpaceKind::Index, index));
    assert!(!heap.contains(SpaceKind::Instance2D, instance));
}

#[test]
fn dropping_reclaimed_object_clears_the_record() {
    let mut config = sample_config();
    config.gc_unused_frames = 2;

    let Some(mut gpu) = headless(config) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::PositionColor);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Static);

    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    end_frame(&mut gpu, &mut resources, &mut manager);

    for _ in 0..2 {
        gpu.begin_frame();
        end_frame(&mut gpu, &mut resources, &mut manager);
    }

    assert_eq!(manager.core.reclaimed_len(), 3);
    // 回収済みの Object を捨てると、回収の記録が消える（ヒープは二重に解放しない）
    drop(object);
    gpu.begin_frame();
    end_frame(&mut gpu, &mut resources, &mut manager);
    assert_eq!(manager.core.reclaimed_len(), 0);
}

#[test]
fn indices_come_from_paint_type_or_index_calls() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(PaintType::Fill);
    object.add_instance(Instance2D::default());

    gpu.begin_frame();
    object.begin();
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        object.vertex(Vertex::new_position(x, y, 0.0));
    }
    object.end(&mut manager);
    // Fill は正方形を三角形 2 つに分け、4 頂点をすべて使う
    let mut used = object.core.index_values().to_vec();
    used.sort_unstable();
    used.dedup();
    assert_eq!((object.core.index_values().len(), used), (6, vec![0, 1, 2, 3]));
    end_frame(&mut gpu, &mut resources, &mut manager);

    // index() を呼んだフレームはそちらを使う
    gpu.begin_frame();
    object.begin();
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        object.vertex(Vertex::new_position(x, y, 0.0));
    }
    object.index(3).index(2).index(1);
    object.end(&mut manager);
    assert_eq!(object.core.index_values(), [3, 2, 1]);
    end_frame(&mut gpu, &mut resources, &mut manager);
}

#[test]
fn staging_chunks_are_reused() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::PositionColor);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);

    for frame in 0..4 {
        gpu.begin_frame();
        object.begin();
        triangle(&mut object, frame as f32);
        object.end(&mut manager);
        end_frame(&mut gpu, &mut resources, &mut manager);
        // map し直しを待つ（実際のアプリでは、次以降のフレームの end_frame で終わったことが分かる）
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    }

    // map し直しが終わっていれば、毎フレーム書いてもチャンクは 1 つを使い回す
    let (active, free, in_flight) = manager.core.staging.chunk_counts();
    assert_eq!(active + free + in_flight, 1);

    // 待たない場合は、CPU が GPU より先に進んだフレームの数だけチャンクが要る（GPU の混み具合で変わる）
    for frame in 0..300 {
        gpu.begin_frame();
        object.begin();
        triangle(&mut object, frame as f32);
        object.end(&mut manager);
        end_frame(&mut gpu, &mut resources, &mut manager);
    }

    // GPU が追いついた後は、新しいチャンクを作らずに使い回す（増え続けない）
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    let (active, free, in_flight) = manager.core.staging.chunk_counts();
    let total = active + free + in_flight;

    for frame in 0..50 {
        gpu.begin_frame();
        object.begin();
        triangle(&mut object, frame as f32);
        object.end(&mut manager);
        end_frame(&mut gpu, &mut resources, &mut manager);
        let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    }

    let (active, free, in_flight) = manager.core.staging.chunk_counts();
    assert_eq!(active + free + in_flight, total, "GPU が追いついた後にチャンクが増えました");
}

// 形ごとに、index() を呼んだ形はそのインデックス、呼ばなかった形は paint_type で作る（§10 インデックス）
#[test]
fn index_and_automatic_shapes_can_be_mixed() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(PaintType::Fill);
    object.add_instance(Instance2D::default());

    // 正方形 2 つ。first / second が true の形は index() で指定する（番号は Object 全体の通し番号）
    let draw = |gpu: &mut GpuContext, object: &mut Object2D, manager: &mut DrawManager2D, first: bool, second: bool| {
        gpu.begin_frame();
        object.begin();
        for (shape, left) in [(0, 0.0), (1, 200.0)] {
            if shape == 1 {
                object.begin_shape();
            }
            for (x, y) in [(left, 0.0), (left + 100.0, 0.0), (left + 100.0, 100.0), (left, 100.0)] {
                object.vertex(Vertex::new_position(x, y, 0.0));
            }
            if [first, second][shape] {
                let base = shape as u32 * 4;
                for i in [0, 1, 2, 2, 3, 0] {
                    object.index(base + i);
                }
            }
        }
        object.end(manager);
    };
    let uses_only = |indices: &[u32], range: std::ops::Range<u32>| indices.iter().all(|i| range.contains(i));

    // 1 つ目は index()、2 つ目は Fill で自動
    draw(&mut gpu, &mut object, &mut manager, true, false);
    let indices = object.core.index_values().to_vec();
    assert_eq!(indices[..6], [0, 1, 2, 2, 3, 0]);
    assert_eq!(indices.len(), 12);
    assert!(uses_only(&indices[6..], 4..8));
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 同じなら何も送らない
    draw(&mut gpu, &mut object, &mut manager, true, false);
    assert!(manager.core.heap_requests.is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 2 つ目も index() にすると、書いたとおりになる
    draw(&mut gpu, &mut object, &mut manager, true, true);
    assert_eq!(object.core.index_values(), [0, 1, 2, 2, 3, 0, 4, 5, 6, 6, 7, 4]);
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 1 つ目の index() をやめると、1 つ目を自動で作る
    draw(&mut gpu, &mut object, &mut manager, false, true);
    let indices = object.core.index_values().to_vec();
    assert!(uses_only(&indices[..6], 0..4));
    assert_eq!(indices[6..], [4, 5, 6, 6, 7, 4]);
    end_frame(&mut gpu, &mut resources, &mut manager);
}

// Stroke は index() と混ぜられない。index() を呼んだフレームは、呼ばなかった形を描かない（§10 インデックス）
#[test]
fn stroke_shapes_are_not_drawn_with_index() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = Object2D::new(&gpu);
    let stroke = PaintType::Stroke { line_width: 2.0, joint_type: crate::object::JointType::Miter, strip: false, dash: None };
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(stroke);
    object.add_instance(Instance2D::default());

    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.index(0).index(1).index(2);
    object.begin_shape();
    triangle(&mut object, 5.0);
    object.end(&mut manager);

    // 元の頂点を送り、index() の三角形だけを描く
    assert_eq!(object.core.uploaded_vertices(), object.core.vertices());
    assert_eq!(object.core.index_values(), [0, 1, 2]);
}

// 形の区切りが変われば、頂点が同じでもインデックスを作り直す（§10 形の区切り）
#[test]
fn indices_follow_shape_breaks() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(PaintType::Fill);
    object.add_instance(Instance2D::default());

    // 凸六角形。区切らなければ 1 つの形、3 頂点目で区切れば三角形 2 つ
    let hexagon = [(0.0, 0.0), (2.0, 0.0), (3.0, 1.0), (2.0, 2.0), (0.0, 2.0), (-1.0, 1.0)];
    let draw = |gpu: &mut GpuContext, object: &mut Object2D, manager: &mut DrawManager2D, split: Option<usize>| {
        gpu.begin_frame();
        object.begin();
        for (i, &(x, y)) in hexagon.iter().enumerate() {
            if Some(i) == split {
                object.begin_shape();
            }
            object.vertex(Vertex::new_position(x, y, 0.0));
        }
        object.end(manager);
    };
    let sorted = |object: &Object2D| {
        let mut indices: Vec<Vec<u32>> = object.core.index_values().chunks(3).map(|t| { let mut t = t.to_vec(); t.sort_unstable(); t }).collect();
        indices.sort();
        indices
    };

    // 区切らなければ 6 頂点の 1 つの形（三角形 4 つ）
    draw(&mut gpu, &mut object, &mut manager, None);
    assert_eq!(object.core.index_values().len(), 4 * 3);
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 3 頂点ずつに区切ると、三角形 2 つ。頂点は送り直さない
    draw(&mut gpu, &mut object, &mut manager, Some(3));
    assert_eq!(sorted(&object), [vec![0, 1, 2], vec![3, 4, 5]]);
    assert!(!writes(&manager, SpaceKind::Index).is_empty());
    assert!(writes(&manager, SpaceKind::Vertex(VertexLayout::Position)).is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 同じ区切りなら作り直さない
    draw(&mut gpu, &mut object, &mut manager, Some(3));
    assert!(manager.core.heap_requests.is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);
}

// 穴の区切りが変われば、頂点が同じでもインデックスを作り直す（§10 穴のある多角形）
#[test]
fn polygon_indices_follow_hole_boundaries() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(PaintType::Fill);
    object.add_instance(Instance2D::default());

    // 外周 0〜10 の正方形と、穴 4〜6 の正方形。hole_start 個目の頂点から穴にする
    let corners = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)];
    let draw = |gpu: &mut GpuContext, object: &mut Object2D, manager: &mut DrawManager2D, hole_start: usize| {
        gpu.begin_frame();
        object.begin();
        for (i, &(x, y)) in corners.iter().enumerate() {
            if i == hole_start {
                object.begin_hole();
            }
            object.vertex(Vertex::new_position(x, y, 0.0));
        }
        object.end(manager);
    };

    draw(&mut gpu, &mut object, &mut manager, 4);
    // 頂点 8 個・穴 1 つなら三角形は 8 + 2 - 2 = 8 個
    assert_eq!(object.core.index_values().len(), 8 * 3);
    end_frame(&mut gpu, &mut resources, &mut manager);

    draw(&mut gpu, &mut object, &mut manager, 4);
    assert!(manager.core.heap_requests.is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);

    // (4, 4) を外周に移す（外周 5 点のへこんだ形と、三角形の穴）。頂点は同じでもインデックスを作り直す
    let before = object.core.index_values().to_vec();
    draw(&mut gpu, &mut object, &mut manager, 5);
    assert!(!writes(&manager, SpaceKind::Index).is_empty());
    assert_ne!(object.core.index_values(), before);
    assert!(writes(&manager, SpaceKind::Vertex(VertexLayout::Position)).is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);
}

// Stroke は作った頂点を送り、変わった位置だけを送り直す。Stroke をやめたら元の頂点をすべて送り直す（§10 線：Stroke）
#[test]
fn stroke_uploads_generated_vertices_and_switches_back() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let layout = VertexLayout::PositionColor;
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), layout);
    let vertices = SpaceKind::Vertex(layout);
    let stride = layout.stride() as u64;
    let stroke = |line_width| PaintType::Stroke { line_width, joint_type: crate::object::JointType::Miter, strip: true, dash: None };
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).paint_type(stroke(2.0));
    object.add_instance(Instance2D::default());

    // 直角に曲がる 3 点。辺の四角形 2 つ（頂点 8 個）と Miter の角（頂点 4 個）
    let draw = |gpu: &mut GpuContext, object: &mut Object2D, manager: &mut DrawManager2D, last_red: f32| {
        gpu.begin_frame();
        object.begin();
        object.vertex(Vertex::new_position_color(0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0));
        object.vertex(Vertex::new_position_color(10.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0));
        object.vertex(Vertex::new_position_color(10.0, 10.0, 0.0, last_red, 1.0, 1.0, 1.0));
        object.end(manager);
    };
    let read_back = |gpu: &GpuContext, object: &Object2D| {
        let [vertex, ..] = object.core.handles().unwrap();
        gpu.heap().read(gpu.device(), gpu.queue(), vertices, gpu.heap().offset(vertices, vertex), object.core.uploaded_vertices().len() as u64)
    };
    let expected = |object: &Object2D| {
        let uploaded = object.core.uploaded_vertices();
        let mut bytes = vec![0; uploaded.len() * layout.stride()];
        for (vertex, dst) in uploaded.iter().zip(bytes.chunks_exact_mut(layout.stride())) {
            vertex.write_packed(layout, dst);
        }
        bytes
    };

    draw(&mut gpu, &mut object, &mut manager, 1.0);
    assert_eq!(object.core.uploaded_vertices().len(), 12);
    assert_eq!(writes(&manager, vertices), vec![(0, 12 * stride)]);
    end_frame(&mut gpu, &mut resources, &mut manager);
    assert_eq!(read_back(&gpu, &object), expected(&object));

    // 同じ点なら作り直さず、何も送らない
    draw(&mut gpu, &mut object, &mut manager, 1.0);
    assert!(manager.core.heap_requests.is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);

    // 終点の色だけ変えると、2 本目の辺の終点側の 2 頂点だけを送る
    draw(&mut gpu, &mut object, &mut manager, 0.0);
    assert_eq!(writes(&manager, vertices), vec![(6, 2 * stride)]);
    assert!(writes(&manager, SpaceKind::Index).is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);
    assert_eq!(read_back(&gpu, &object), expected(&object));

    // 太さを変えると位置がすべて変わるが、三角形のつなぎ方は同じなのでインデックスは送らない
    object.paint_type(stroke(4.0));
    draw(&mut gpu, &mut object, &mut manager, 0.0);
    assert_eq!(writes(&manager, vertices), vec![(0, 12 * stride)]);
    assert!(writes(&manager, SpaceKind::Index).is_empty());
    end_frame(&mut gpu, &mut resources, &mut manager);

    // Stroke をやめたら、元の 3 頂点をすべて送り直す
    object.paint_type(PaintType::Fill);
    draw(&mut gpu, &mut object, &mut manager, 0.0);
    assert_eq!(object.core.uploaded_vertices(), object.core.vertices());
    assert_eq!(writes(&manager, vertices), vec![(0, 3 * stride)]);
    end_frame(&mut gpu, &mut resources, &mut manager);
    assert_eq!(read_back(&gpu, &object), expected(&object));
}

// デバイスが失われたフレームでも、DrawManager* の描画コマンドは片付け、次のフレームで使えるようにする
#[test]
fn draw_commands_are_cleared_when_device_is_lost() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);

    gpu.simulate_device_lost("テストで失わせた");

    for _ in 0..2 {
        gpu.begin_frame();
        object.begin();
        triangle(&mut object, 0.0);
        object.end(&mut manager);
        assert_eq!(manager.core.draw_commands.len(), 1);

        let surface = surface(&gpu);
        let result = gpu.end_frame(&mut resources, &mut [SurfaceDraw { surface, clear_color: None, draw_managers: &mut [&mut manager] }]);
        assert!(matches!(result, Err(crate::error::LonasError::DeviceLost { .. })));
        assert!(manager.core.draw_commands.is_empty());
    }
}

#[test]
fn misuse_panics() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut other_manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);

    // フレーム外の begin
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    assert!(panic_message(|| { object.begin(); }).contains("フレーム外"));

    gpu.begin_frame();

    // begin を呼ばずに end
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    assert!(panic_message(|| { object.end(&mut manager); }).contains("begin を呼ばずに"));

    // 1 フレームに 2 回 begin
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    object.begin();
    assert!(panic_message(|| { object.begin(); }).contains("begin が 1 フレームに 2 回"));

    // 1 フレームに 2 回 end
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    assert!(panic_message(|| { object.end(&mut manager); }).contains("end が 1 フレームに 2 回"));

    // memory_update_type を呼ばずに end
    let mut object = Object2D::new(&gpu);
    object.begin();
    assert!(panic_message(|| { object.end(&mut manager); }).contains("memory_update_type()"));

    // index も paint_type も呼ばずに end
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).begin();
    object.vertex(Vertex::new_position(0.0, 0.0, 0.0));
    assert!(panic_message(|| { object.end(&mut manager); }).contains("index() も paint_type() も"));

    // 別の DrawManager の CameraId
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    object.camera(other_manager.default_camera_id()).begin();
    assert!(panic_message(|| { object.end(&mut manager); }).contains("別の DrawManager の CameraId"));

    // 削除済みの CameraId
    let removed = manager.add_camera(camera_2d());
    manager.remove_camera(removed);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    object.camera(removed).begin();
    assert!(panic_message(|| { object.end(&mut manager); }).contains("削除済みの CameraId"));

    // 初回とは別の DrawManager に渡す
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut other_manager);
    end_frame(&mut gpu, &mut resources, &mut manager);
    gpu.begin_frame();
    object.begin();
    assert!(panic_message(|| { object.end(&mut manager); }).contains("別の DrawManager に渡されました"));

    // 前のフレームで使った DrawManager を end_frame に渡さなかった（other_manager は上で渡していない）
    let mut object = new_triangle(&gpu, MemoryUpdateType::Dynamic);
    object.begin();
    triangle(&mut object, 0.0);
    assert!(panic_message(|| { object.end(&mut other_manager); }).contains("end_frame に渡されていません"));
}

#[test]
fn static_object_cannot_change_vertex_count() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = new_triangle(&gpu, MemoryUpdateType::Static);

    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.end(&mut manager);
    end_frame(&mut gpu, &mut resources, &mut manager);

    gpu.begin_frame();
    object.begin();
    triangle(&mut object, 0.0);
    object.begin_shape();
    triangle(&mut object, 5.0);
    assert!(panic_message(|| { object.end(&mut manager); }).contains("memory_update_type(Dynamic)"));
}

#[test]
fn draw_manager_3d_accepts_object_3d() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager3D::new(&mut gpu, camera_3d(), VertexLayout::PositionColorUvNormal);
    let mut object = Object3D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Static).paint_type(PaintType::Fill);
    let id = object.add_instance(Instance3D::default());

    gpu.begin_frame();
    object.begin();
    for (x, z) in [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)] {
        object.vertex(Vertex::new_position(x, 0.0, z));
    }
    object.end(&mut manager);
    assert_eq!(object.core.index_values().len(), 6);
    assert_eq!(manager.core.draw_commands[0].instance_count, 1);
    let surface = surface(&gpu);
    gpu.end_frame(&mut resources, &mut [SurfaceDraw { surface, clear_color: None, draw_managers: &mut [&mut manager] }]).unwrap();

    object.remove_instance(id);
    gpu.begin_frame();
    object.begin();
    for (x, z) in [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)] {
        object.vertex(Vertex::new_position(x, 0.0, z));
    }
    object.end(&mut manager);
    // インスタンスが 0 個なら描かない
    assert!(manager.core.draw_commands.is_empty());
}
