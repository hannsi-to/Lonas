// Object と DrawManager のテスト（段階 5 の終わりの条件）。GPU がない環境では飛ばす

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::*;
use crate::config::LonasConfig;
use crate::config::tests::sample_config;
use crate::draw::{Camera2D, Camera3D, HeapRequest, SpaceKind};
use crate::vertex::VertexLayout;
use crate::gpu::{SurfaceDraw, heap::Heap};
use crate::math::Quaternion;
use crate::resource::ResourceManager;

fn headless(config: LonasConfig) -> Option<GpuContext> {
    match GpuContext::new_headless(config) {
        Ok(gpu) => Some(gpu),
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
    gpu.end_frame(resources, &mut [SurfaceDraw { surface: crate::gpu::SurfaceId(0), clear_color: None, draw_managers: &mut [manager] }]);
}

fn triangle(object: &mut Object2D, x: f32) {
    object.vertex_color(x, 0.0, 1.0, 0.0, 0.0, 1.0);
    object.vertex_color(x + 1.0, 0.0, 0.0, 1.0, 0.0, 1.0);
    object.vertex_color(x, 1.0, 0.0, 0.0, 1.0, 1.0);
}

fn new_triangle(gpu: &GpuContext, update: MemoryUpdateType) -> Object2D {
    let mut object = Object2D::new(gpu);
    object.memory_update_type(update).topology(Topology::TriangleList);
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
    object.vertex_color(0.0, 0.0, 1.0, 0.0, 0.0, 1.0);
    object.vertex_color(5.0, 0.0, 0.0, 1.0, 0.0, 1.0);
    object.vertex_color(0.0, 1.0, 0.0, 0.0, 1.0, 1.0);
    object.end(&mut manager);
    assert_eq!(writes(&manager, vertices), vec![(1, stride)]);
    // TriangleList は頂点数が変わらなければインデックスを作り直さない
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
        for i in 0..(3 + frame * 6) {
            dynamic.vertex_color(i as f32, frame as f32, 0.5, 0.5, 0.5, 1.0);
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
fn indices_come_from_topology_or_index_calls() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager2D::new(&mut gpu, camera_2d(), VertexLayout::Position);
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).topology(Topology::TriangleFan);
    object.add_instance(Instance2D::default());

    gpu.begin_frame();
    object.begin();
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        object.vertex(x, y);
    }
    object.end(&mut manager);
    assert_eq!(object.core.index_values(), [0, 1, 2, 0, 2, 3]);
    end_frame(&mut gpu, &mut resources, &mut manager);

    // index() を呼んだフレームはそちらを使う
    gpu.begin_frame();
    object.begin();
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        object.vertex(x, y);
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

    // index も topology も呼ばずに end
    let mut object = Object2D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).begin();
    object.vertex(0.0, 0.0);
    assert!(panic_message(|| { object.end(&mut manager); }).contains("index() も topology() も"));

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
    triangle(&mut object, 5.0);
    assert!(panic_message(|| { object.end(&mut manager); }).contains("memory_update_type(Dynamic)"));
}

#[test]
fn draw_manager_3d_accepts_object_3d() {
    let Some(mut gpu) = headless(sample_config()) else { return };
    let mut resources = ResourceManager::new();
    let mut manager = DrawManager3D::new(&mut gpu, camera_3d(), VertexLayout::PositionColorUvNormal);
    let mut object = Object3D::new(&gpu);
    object.memory_update_type(MemoryUpdateType::Static).topology(Topology::Polygon);
    let id = object.add_instance(Instance3D::default());

    gpu.begin_frame();
    object.begin();
    for (x, z) in [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)] {
        object.vertex(x, 0.0, z);
    }
    object.end(&mut manager);
    assert_eq!(object.core.index_values().len(), 6);
    assert_eq!(manager.core.draw_commands[0].instance_count, 1);
    gpu.end_frame(&mut resources, &mut [SurfaceDraw { surface: crate::gpu::SurfaceId(0), clear_color: None, draw_managers: &mut [&mut manager] }]);

    object.remove_instance(id);
    gpu.begin_frame();
    object.begin();
    for (x, z) in [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)] {
        object.vertex(x, 0.0, z);
    }
    object.end(&mut manager);
    // インスタンスが 0 個なら描かない
    assert!(manager.core.draw_commands.is_empty());
}
