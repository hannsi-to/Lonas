// Bitmap とグリフのアトラスのテスト（段階 11）。GPU がない環境では飛ばす

use super::bitmap::BitmapKey;
use super::*;
use crate::color::Color;
use crate::config::tests::sample_config;
use crate::config::{Blend, CullMode, LonasConfig};
use crate::draw::{Camera2D, DrawManager2D, HeapRequest};
use crate::gpu::{GpuContext, SurfaceDraw, SurfaceId};
use crate::instance::Instance2D;
use crate::math::{Vector2f, Vector3f};
use crate::memory::MemoryUpdateType;
use crate::object::Object2D;
use crate::resource::{FontId, ResourceConfig, ResourceManager, ShaderId};
use crate::vertex::VertexLayout;
use crate::wgsl::{Dimension, text_shader};

const LATIN: &[u8] = include_bytes!("../../tests/fonts/Cantarell-Regular.ttf");
const SIZE: u32 = 64;

struct Scene {
    gpu: GpuContext,
    surface: SurfaceId,
    resources: ResourceManager,
    font: FontId,
    shader: ShaderId,
    manager: DrawManager2D,
}

fn scene(config: LonasConfig) -> Option<Scene> {
    let bindings = config.bindings;
    let mut gpu = match GpuContext::new_headless(config) {
        Ok(gpu) => gpu,
        Err(error) => {
            eprintln!("GPU を使うテストを飛ばします: {error}");
            return None;
        }
    };
    let surface = gpu.add_offscreen(SIZE, SIZE);
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());
    let font = resources.register_font(LATIN).unwrap();
    let shader = resources.register_shader(&text_shader(&bindings, GlyphMethod::Bitmap { hinting: Hinting::None }, Dimension::D2)).unwrap();
    // Bitmap は normal.x にグリフの番号を書くため PositionColorUvNormal（§13 頂点からの参照）
    let manager = DrawManager2D::new(&mut gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColorUvNormal);

    Some(Scene { gpu, surface, resources, font, shader, manager })
}

fn style(font: FontId, size: f32) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::new(font)],
        size,
        line_height: LineHeight::Font(1.0),
        letter_spacing: 0.0,
        max_width: None,
        align: TextAlign::Left,
        bold_weight: 0.03,
        outline_width: 0.05,
    }
}

fn bitmap_write(position: Vector3f) -> TextWrite {
    TextWrite {
        position,
        color: Color::WHITE,
        y_axis: YAxis::Down,
        pixel_scale: 1.0,
        obfuscation_seed: 0,
        method: GlyphMethod::Bitmap { hinting: Hinting::None },
    }
}

fn text_object(scene: &Scene) -> Object2D {
    let mut object = Object2D::new(&scene.gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic).shader(scene.shader).cull_mode(CullMode::None).blend(Blend::Alpha);
    object.add_instance(Instance2D::default());
    object
}

// begin〜end で書き、end_frame まで進める。objects は (Object, TextLayout) の組
fn draw(scene: &mut Scene, objects: &mut [(&mut Object2D, &TextLayout)], position: Vector3f) -> Vec<[u8; 4]> {
    scene.gpu.begin_frame();

    for (object, layout) in objects.iter_mut() {
        object.begin();
        write_text(*object, &mut scene.resources, layout, &bitmap_write(position));
        object.end(&mut scene.manager);
    }

    scene
        .gpu
        .end_frame(&mut scene.resources, &mut [SurfaceDraw { surface: scene.surface, clear_color: Some(Color::BLACK), draw_managers: &mut [&mut scene.manager] }])
        .unwrap();

    scene.gpu.read_pixels(scene.surface)
}

// 線形の値を sRGB の 8 ビットに直す（描画先は sRGB のフォーマット）
fn srgb_byte(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
    (encoded * 255.0).round() as u8
}

#[test]
fn bitmap_shader_passes_validation() {
    let bindings = sample_config().bindings;
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());

    for dimension in [Dimension::D2, Dimension::D3] {
        let source = text_shader(&bindings, GlyphMethod::Bitmap { hinting: Hinting::Light }, dimension);
        assert!(resources.register_shader(&source).is_ok(), "{source}");
    }
}

#[test]
fn rendered_pixels_follow_the_coverage() {
    let Some(mut scene) = scene(sample_config()) else { return };
    let layout = scene.resources.layout_text(&style(scene.font, 32.0), &Formatted::plain("e"));
    let mut object = text_object(&scene);
    let position = Vector3f::new(10.0, 6.0, 0.0);
    let pixels = draw(&mut scene, &mut [(&mut object, &layout)], position);

    let glyph = layout.glyphs()[0];
    let key = BitmapKey::new(glyph.font, glyph.glyph_id, 32, Hinting::None, 0.0, 0.0);
    let raster = &scene.resources.bitmap_glyph(&key).raster;
    assert!(!raster.is_empty());
    // 原点は画素の格子にそろえる（§13 Bitmap の書き込み）
    let origin_x = (position.x + glyph.position.x).round() as i32 + raster.left;
    let origin_y = (position.y + glyph.position.y).round() as i32 + raster.top;
    let mut covered = 0;

    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let (inner_x, inner_y) = (x - origin_x, y - origin_y);
            let coverage = if (0..raster.width as i32).contains(&inner_x) && (0..raster.height as i32).contains(&inner_y) {
                raster.pixels[(inner_y * raster.width as i32 + inner_x) as usize]
            } else {
                0
            };
            let actual = pixels[(y as u32 * SIZE + x as u32) as usize][0];
            let expected = srgb_byte(coverage as f32 / 255.0);

            assert!(
                (actual as i32 - expected as i32).abs() <= 3,
                "({x}, {y}) の画素 {actual} が覆い率 {coverage}（sRGB で {expected}）と合いません",
            );
            covered += (coverage > 0) as u32;
        }
    }

    assert!(covered > 20);
}

#[test]
fn hide_box_uses_the_fill_slot() {
    let Some(mut scene) = scene(sample_config()) else { return };
    let layout = scene.resources.layout_text(&style(scene.font, 32.0), &Formatted::parse("§[hidebox]A").unwrap());
    let mut object = text_object(&scene);
    let pixels = draw(&mut scene, &mut [(&mut object, &layout)], Vector3f::ZERO);
    let row = layout.rows()[0];

    // 番号 0 は覆い率 1。箱の中は白
    let center = ((row.top + row.height / 2.0) as u32 * SIZE + (layout.glyphs()[0].advance / 2.0) as u32) as usize;
    assert_eq!(pixels[center][..3], [255, 255, 255]);
    assert!(object.get_vertices().iter().all(|vertex| vertex.n_x == 0.0));
}

#[test]
fn evicted_glyph_is_placed_again_without_rewriting_vertices() {
    let mut config = sample_config();
    // グリフが数個しか入らない小さなアトラス。レイヤーは増やせない
    config.glyph_atlas.size = 48;
    config.glyph_atlas.initial_layers = 1;
    config.glyph_atlas.max_layers = 1;
    let Some(mut scene) = scene(config) else { return };
    // 32px の字は余白を含めて 17×19 画素ほどで、48 画素のレイヤーには 4 個ほどしか入らない
    let style = style(scene.font, 32.0);
    let first = scene.resources.layout_text(&style, &Formatted::plain("e"));
    // 「e」より高くも広くもない字。棚方式では、追い出した「e」の場所に入る大きさでないと、追い出しても置けない
    let others = "acnrsuxz";
    let mut object = text_object(&scene);
    let mut filler = text_object(&scene);
    let position = Vector3f::new(4.0, 4.0, 0.0);

    let before = draw(&mut scene, &mut [(&mut object, &first)], position);
    let glyph = first.glyphs()[0];
    let slot = scene.resources.bitmap_glyph(&BitmapKey::new(glyph.font, glyph.glyph_id, 32, Hinting::None, 0.0, 0.0)).slot;
    let vertices = object.get_vertices().to_vec();
    assert!(scene.gpu.renderer().glyph_atlas().placement(slot).is_some());

    // 「e」を使わないフレームで、ほかの字を 1 つずつ増やしていく。いつか「e」が追い出される
    let mut evicted = false;
    for count in 1..=others.len() {
        let layout = scene.resources.layout_text(&style, &Formatted::plain(&others[..count]));
        draw(&mut scene, &mut [(&mut filler, &layout)], position);

        if scene.gpu.renderer().glyph_atlas().placement(slot).is_none() {
            evicted = true;
            break;
        }
    }
    assert!(evicted, "「e」が追い出されませんでした");
    assert_eq!(scene.gpu.renderer().glyph_atlas().layer_count(), 1);

    // また使うと、同じ番号のまま置き直される。頂点は前と同じで、転送も起きない
    scene.gpu.begin_frame();
    object.begin();
    write_text(&mut object, &mut scene.resources, &first, &bitmap_write(position));
    object.end(&mut scene.manager);
    let vertex_writes = scene
        .manager
        .core
        .heap_requests
        .iter()
        .filter(|request| matches!(request, HeapRequest::Write { space: crate::draw::SpaceKind::Vertex(_), .. }))
        .count();
    scene
        .gpu
        .end_frame(&mut scene.resources, &mut [SurfaceDraw { surface: scene.surface, clear_color: Some(Color::BLACK), draw_managers: &mut [&mut scene.manager] }])
        .unwrap();
    let after = scene.gpu.read_pixels(scene.surface);

    assert_eq!(vertex_writes, 0);
    assert_eq!(object.get_vertices(), vertices);
    assert!(scene.gpu.renderer().glyph_atlas().placement(slot).is_some());
    // 置き直した後も同じ画素で描かれる
    assert_eq!(after, before);
}

#[test]
fn prepare_glyphs_rasterizes_without_the_frame_limit() {
    let mut config = ResourceConfig::for_tests();
    config.text.max_new_glyphs_per_frame = Some(1);
    let mut resources = ResourceManager::new(config);
    let font = resources.register_font(LATIN).unwrap();
    let layout = resources.layout_text(&style(font, 16.0), &Formatted::parse("§[outline #000]abc").unwrap());

    resources.prepare_glyphs(&layout, GlyphMethod::Bitmap { hinting: Hinting::Light }, 1.0);

    // 字と縁取りで 2 つずつ
    assert_eq!(resources.bitmap_cache_mut().len(), 6);
    // 空でないグリフは、置く要求が溜まっている
    assert_eq!(resources.bitmap_cache_mut().take_uploads().len(), 6);
}

#[test]
fn light_hinting_keeps_advances_and_snaps_horizontal_edges() {
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());
    let font = resources.register_font(LATIN).unwrap();
    let layout = resources.layout_text(&style(font, 13.0), &Formatted::plain("H"));
    let glyph = layout.glyphs()[0];
    let raster = |resources: &mut ResourceManager, hinting| {
        let key = BitmapKey::new(glyph.font, glyph.glyph_id, 13, hinting, 0.0, 0.0);
        resources.ensure_bitmap(key, false);
        resources.bitmap_glyph(&key).raster.clone()
    };
    let none = raster(&mut resources, Hinting::None);
    let light = raster(&mut resources, Hinting::Light);

    // 縦方向を画素に合わせるため、「H」の横線は半端な覆い率の行が減る
    let partial_rows = |raster: &super::bitmap::RasterGlyph| {
        (0..raster.height)
            .filter(|&y| {
                let row = &raster.pixels[(y * raster.width) as usize..((y + 1) * raster.width) as usize];
                let max = row.iter().copied().max().unwrap_or(0);
                max > 0 && max < 200
            })
            .count()
    };
    assert!(partial_rows(&light) <= partial_rows(&none), "{} > {}", partial_rows(&light), partial_rows(&none));
    // 横方向は変えない（幅はほぼ同じ）
    assert!((light.width as i32 - none.width as i32).abs() <= 1);
}

