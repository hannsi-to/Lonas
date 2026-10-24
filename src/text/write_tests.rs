// ベクターの書き込みのテスト（段階 10）。Object* を使うテストは GPU がない環境では飛ばす

use super::vector::{ShapeKey, signed_area, size_stage, tessellate_glyph};
use super::*;
use crate::color::Color;
use crate::config::{Blend, CullMode, LonasConfig};
use crate::config::tests::sample_config;
use crate::draw::{Camera2D, DrawManager2D, HeapRequest};
use crate::gpu::{GpuContext, SurfaceDraw, SurfaceId};
use crate::instance::Instance2D;
use crate::math::{Vector2f, Vector3f};
use crate::memory::MemoryUpdateType;
use crate::object::Object2D;
use crate::resource::{FontId, ResourceConfig, ResourceManager};
use crate::vertex::VertexLayout;
use crate::wgsl::{Dimension, text_shader};

const LATIN: &[u8] = include_bytes!("../../tests/fonts/Cantarell-Regular.ttf");
const JAPANESE: &[u8] = include_bytes!("../../tests/fonts/NotoSansJP-Subset.ttf");

fn resources_with(config: ResourceConfig) -> (ResourceManager, FontId, FontId) {
    let mut resources = ResourceManager::new(config);
    let latin = resources.register_font(LATIN).unwrap();
    let japanese = resources.register_font(JAPANESE).unwrap();

    (resources, latin, japanese)
}

fn style(latin: FontId, japanese: FontId) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::new(latin), FontFamily::new(japanese)],
        size: 24.0,
        line_height: LineHeight::Font(1.0),
        letter_spacing: 0.0,
        max_width: None,
        align: TextAlign::Left,
        bold_weight: 0.03,
        outline_width: 0.05,
    }
}

fn text_write(y_axis: YAxis) -> TextWrite {
    TextWrite {
        position: Vector3f::new(10.0, 20.0, 0.0),
        color: Color::WHITE,
        y_axis,
        pixel_scale: 1.0,
        obfuscation_seed: 0,
        method: GlyphMethod::Vector,
    }
}

fn headless(config: LonasConfig) -> Option<(GpuContext, SurfaceId)> {
    match GpuContext::new_headless(config) {
        Ok(mut gpu) => {
            let surface = gpu.add_offscreen(4, 4);
            Some((gpu, surface))
        }
        Err(error) => {
            eprintln!("GPU を使うテストを飛ばします: {error}");
            None
        }
    }
}

fn manager(gpu: &mut GpuContext) -> DrawManager2D {
    DrawManager2D::new(gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColor)
}

fn text_object(gpu: &GpuContext) -> Object2D {
    // テキストの Object* は Dynamic にする。paint_type() は呼ばない（§13）
    let mut object = Object2D::new(gpu);
    object.memory_update_type(MemoryUpdateType::Dynamic);
    object.add_instance(Instance2D::default());
    object
}

fn end_frame(gpu: &mut GpuContext, surface: SurfaceId, resources: &mut ResourceManager, manager: &mut DrawManager2D) {
    gpu.end_frame(resources, &mut [SurfaceDraw { surface, clear_color: None, draw_managers: &mut [manager] }]).unwrap();
}

// 書き込んだ三角形の符号付き面積
fn triangle_areas(object: &Object2D) -> Vec<f32> {
    let vertices = object.get_vertices();

    object
        .get_indices()
        .chunks_exact(3)
        .map(|triangle| {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| {
                let vertex = vertices[index as usize];
                [vertex.x, vertex.y]
            });
            signed_area(a, b, c)
        })
        .collect()
}

// ---- GPU を使わないもの ----

#[test]
fn synthetic_bold_glyph_has_no_double_counted_overlap() {
    let (resources, latin, _) = resources_with(ResourceConfig::for_tests());
    let font = resources.font(latin);
    let glyph = font.font.charmap().map_unicode('A').unwrap().to_u32();
    let stage = size_stage(24.0);

    let regular = tessellate_glyph(font, &ShapeKey::new(latin, glyph, stage, 0.0, 0.0));
    let bold = tessellate_glyph(font, &ShapeKey::new(latin, glyph, stage, 0.05, 0.0));
    let area = |shape: &vector::GlyphShape| -> f32 {
        shape
            .indices
            .chunks_exact(3)
            .map(|t| signed_area(shape.vertices[t[0] as usize], shape.vertices[t[1] as usize], shape.vertices[t[2] as usize]))
            .sum()
    };
    let bounds = |shape: &vector::GlyphShape| {
        let (xs, ys): (Vec<f32>, Vec<f32>) = shape.vertices.iter().map(|&[x, y]| (x, y)).unzip();
        let span = |values: &[f32]| values.iter().copied().fold(f32::MIN, f32::max) - values.iter().copied().fold(f32::MAX, f32::min);
        span(&xs) * span(&ys)
    };

    assert!(area(&regular) > 0.0);
    // 太らせた分だけ広く、複製の重なりを 2 回数えないため、外接する四角形を超えない
    assert!(area(&bold) > area(&regular));
    assert!(area(&bold) <= bounds(&bold));
    assert!(bold.indices.chunks_exact(3).all(|t| {
        signed_area(bold.vertices[t[0] as usize], bold.vertices[t[1] as usize], bold.vertices[t[2] as usize]) >= 0.0
    }));
}

#[test]
fn cache_evicts_shapes_unused_in_the_frame_oldest_first() {
    let mut config = ResourceConfig::for_tests();
    // どの形も上限を超える大きさ
    config.text.vector_cache_max_bytes = 1;
    let (mut resources, latin, japanese) = resources_with(config);
    let style = style(latin, japanese);
    let ab = resources.layout_text(&style, &Formatted::plain("AB"));
    let c = resources.layout_text(&style, &Formatted::plain("C"));

    resources.prepare_glyphs(&ab, GlyphMethod::Vector, 1.0);
    resources.finish_text_frame();
    // そのフレームに使った形は、上限を超えても捨てない
    assert_eq!(resources.vector_cache().len(), 2);

    resources.prepare_glyphs(&c, GlyphMethod::Vector, 1.0);
    resources.finish_text_frame();
    // 使わなかった A・B を捨て、使った C は残る
    assert_eq!(resources.vector_cache().len(), 1);
    let key = ShapeKey::new(c.glyphs()[0].font, c.glyphs()[0].glyph_id, size_stage(24.0), 0.0, 0.0);
    assert!(resources.vector_cache().contains(&key));
}

#[test]
fn cache_keeps_shapes_within_the_limit_and_drops_unregistered_fonts() {
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let layout = resources.layout_text(&style(latin, japanese), &Formatted::plain("Aあ"));

    resources.prepare_glyphs(&layout, GlyphMethod::Vector, 1.0);
    resources.finish_text_frame();
    resources.finish_text_frame();
    // 上限（64 MiB）に収まっていれば、使わなくても捨てない
    assert_eq!(resources.vector_cache().len(), 2);

    resources.unregister_font(latin);
    assert_eq!(resources.vector_cache().len(), 1);
}

#[test]
fn text_shaders_pass_validation() {
    let bindings = sample_config().bindings;
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());

    for dimension in [Dimension::D2, Dimension::D3] {
        let source = text_shader(&bindings, GlyphMethod::Vector, dimension);
        assert!(resources.register_shader(&source).is_ok(), "{source}");
    }
}

// ---- Object* に書くもの ----

#[test]
fn triangles_are_counter_clockwise_for_both_y_axes() {
    let Some((mut gpu, surface)) = headless(sample_config()) else { return };
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let layout = resources.layout_text(
        &style(latin, japanese),
        &Formatted::parse("§[bold]§[outline #000]§[shadow]Aあ§[/]§[italic]§[u 0.05 round 2]§[s]xy§[/]§[hidebox]z").unwrap(),
    );
    let mut manager = manager(&mut gpu);

    for y_axis in [YAxis::Down, YAxis::Up] {
        let mut object = text_object(&gpu);

        gpu.begin_frame();
        object.begin();
        write_text(&mut object, &mut resources, &layout, &text_write(y_axis));
        object.end(&mut manager);
        end_frame(&mut gpu, surface, &mut resources, &mut manager);

        let areas = triangle_areas(&object);
        assert!(!areas.is_empty());
        assert!(areas.iter().all(|&area| area >= -1e-4), "{y_axis:?}: {:?}", areas.iter().copied().fold(0.0, f32::min));
    }
}

#[test]
fn y_axis_mirrors_vertices_around_the_position() {
    let Some((mut gpu, surface)) = headless(sample_config()) else { return };
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let layout = resources.layout_text(&style(latin, japanese), &Formatted::plain("A"));
    let mut manager = manager(&mut gpu);
    let mut down = text_object(&gpu);
    let mut up = text_object(&gpu);

    gpu.begin_frame();
    down.begin();
    write_text(&mut down, &mut resources, &layout, &text_write(YAxis::Down));
    down.end(&mut manager);
    up.begin();
    write_text(&mut up, &mut resources, &layout, &text_write(YAxis::Up));
    up.end(&mut manager);
    end_frame(&mut gpu, surface, &mut resources, &mut manager);

    let position = text_write(YAxis::Down).position;
    for (a, b) in down.get_vertices().iter().zip(up.get_vertices()) {
        assert_eq!(a.x, b.x);
        assert!(((a.y - position.y) + (b.y - position.y)).abs() < 1e-4);
        // YAxis::Down では字が position より下（y が大きい側）にある
        assert!(a.y >= position.y - 1e-4);
    }
}

#[test]
fn same_text_in_consecutive_frames_is_not_transferred_again() {
    let Some((mut gpu, surface)) = headless(sample_config()) else { return };
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let layout = resources.layout_text(&style(latin, japanese), &Formatted::parse("§[red]日本語§[/]と English").unwrap());
    let mut manager = manager(&mut gpu);
    let mut object = text_object(&gpu);

    for frame in 0..3 {
        gpu.begin_frame();
        object.begin();
        write_text(&mut object, &mut resources, &layout, &text_write(YAxis::Down));
        object.end(&mut manager);

        let writes = manager.core.heap_requests.iter().filter(|request| matches!(request, HeapRequest::Write { .. })).count();
        if frame == 0 {
            assert!(writes > 0);
        } else {
            assert_eq!(writes, 0, "{frame} フレーム目に転送が起きました");
        }
        assert_eq!(manager.core.draw_commands.len(), 1);
        end_frame(&mut gpu, surface, &mut resources, &mut manager);
    }
}

#[test]
fn glyphs_over_the_frame_limit_appear_in_later_frames() {
    let Some((mut gpu, surface)) = headless(sample_config()) else { return };
    let mut config = ResourceConfig::for_tests();
    config.text.max_new_glyphs_per_frame = Some(2);
    let (mut resources, latin, japanese) = resources_with(config);
    let (mut unlimited, unlimited_latin, unlimited_japanese) = resources_with(ResourceConfig::for_tests());
    let text = Formatted::plain("ABCDE");
    let layout = resources.layout_text(&style(latin, japanese), &text);
    let full_layout = unlimited.layout_text(&style(unlimited_latin, unlimited_japanese), &text);
    let mut manager = manager(&mut gpu);
    let mut object = text_object(&gpu);
    let mut full = text_object(&gpu);
    let mut shapes_per_frame = Vec::new();

    for _ in 0..3 {
        gpu.begin_frame();
        object.begin();
        write_text(&mut object, &mut resources, &layout, &text_write(YAxis::Down));
        object.end(&mut manager);
        shapes_per_frame.push(object.get_written_vertex_count());
        // 比べるための、上限のない書き込み
        full.begin();
        write_text(&mut full, &mut unlimited, &full_layout, &text_write(YAxis::Down));
        full.end(&mut manager);
        end_frame(&mut gpu, surface, &mut resources, &mut manager);
    }

    let full_count = full.get_written_vertex_count();
    // 1 フレームに 2 字ずつ作り、先頭から現れる。3 フレーム目には全部そろう
    assert!(shapes_per_frame[0] > 0 && shapes_per_frame[0] < shapes_per_frame[1]);
    assert!(shapes_per_frame[1] < full_count);
    assert_eq!(shapes_per_frame[2], full_count);
    assert_eq!(object.get_vertices(), full.get_vertices());
}

#[test]
fn empty_text_writes_nothing_without_paint_type() {
    let Some((mut gpu, surface)) = headless(sample_config()) else { return };
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let layout = resources.layout_text(&style(latin, japanese), &Formatted::new());
    let mut manager = manager(&mut gpu);
    let mut object = text_object(&gpu);

    gpu.begin_frame();
    object.begin();
    write_text(&mut object, &mut resources, &layout, &text_write(YAxis::Down));
    object.end(&mut manager);
    end_frame(&mut gpu, surface, &mut resources, &mut manager);

    assert!(object.get_vertices().is_empty());
    assert!(manager.core.draw_commands.is_empty());
}

#[test]
fn hide_box_and_obfuscated_follow_the_layout() {
    let Some((mut gpu, surface)) = headless(sample_config()) else { return };
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let style = style(latin, japanese);
    let hidden = resources.layout_text(&style, &Formatted::parse("§[hidebox]A").unwrap());
    let obfuscated = resources.layout_text(&style, &Formatted::parse("§[obf]AAAA").unwrap());
    let mut manager = manager(&mut gpu);
    let mut boxed = text_object(&gpu);
    let mut scrambled = [text_object(&gpu), text_object(&gpu)];

    gpu.begin_frame();
    boxed.begin();
    write_text(&mut boxed, &mut resources, &hidden, &text_write(YAxis::Down));
    boxed.end(&mut manager);
    for (seed, object) in scrambled.iter_mut().enumerate() {
        object.begin();
        write_text(object, &mut resources, &obfuscated, &TextWrite { obfuscation_seed: seed as u64, ..text_write(YAxis::Down) });
        object.end(&mut manager);
    }
    end_frame(&mut gpu, surface, &mut resources, &mut manager);

    // 箱は 4 頂点・2 三角形。幅は送り幅、高さは行の高さ
    let vertices = boxed.get_vertices();
    assert_eq!(vertices.len(), 4);
    let width = vertices.iter().map(|v| v.x).fold(f32::MIN, f32::max) - vertices.iter().map(|v| v.x).fold(f32::MAX, f32::min);
    let height = vertices.iter().map(|v| v.y).fold(f32::MIN, f32::max) - vertices.iter().map(|v| v.y).fold(f32::MAX, f32::min);
    assert!((width - hidden.glyphs()[0].advance).abs() < 1e-4);
    assert!((height - hidden.rows()[0].height).abs() < 1e-4);

    // 種が違えば選ぶ字が変わる
    assert_ne!(scrambled[0].get_vertices(), scrambled[1].get_vertices());
}

// ---- 描いた画素 ----

#[test]
fn text_shader_draws_glyphs_and_boxes_with_back_face_culling() {
    const SIZE: u32 = 64;
    let mut config = sample_config();
    config.cull_mode_when_unset = crate::config::CullMode::Back;
    let bindings = config.bindings;
    let Ok(mut gpu) = GpuContext::new_headless(config) else {
        eprintln!("GPU を使うテストを飛ばします");
        return;
    };
    let surface = gpu.add_offscreen(SIZE, SIZE);
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let shader = resources.register_shader(&text_shader(&bindings, GlyphMethod::Vector, Dimension::D2)).unwrap();
    // 左上に 24×24 ほどの箱、その右に大きな「l」
    let layout = resources.layout_text(
        &TextStyle { size: 40.0, ..style(latin, japanese) },
        &Formatted::parse("§[hidebox]A§[/]§[red]l").unwrap(),
    );
    let mut manager = manager(&mut gpu);
    // cull_mode は既定（Back）のまま。三角形が反時計回りでなければ消える
    let mut object = text_object(&gpu);
    object.shader(shader);

    gpu.begin_frame();
    object.begin();
    write_text(&mut object, &mut resources, &layout, &TextWrite { position: Vector3f::ZERO, ..text_write(YAxis::Down) });
    object.end(&mut manager);
    gpu.end_frame(&mut resources, &mut [SurfaceDraw { surface, clear_color: Some(Color::BLACK), draw_managers: &mut [&mut manager] }])
        .unwrap();
    let pixels = gpu.read_pixels(surface);
    let at = |x: f32, y: f32| pixels[(y as u32 * SIZE + x as u32) as usize];

    // 箱の中は白
    let hidden = layout.glyphs()[0];
    let row = layout.rows()[0];
    assert_eq!(at(hidden.advance / 2.0, row.top + row.height / 2.0)[..3], [255, 255, 255]);

    // 「l」の縦線のどこかが赤い（ベースラインの少し上を横に探す）
    let glyph = layout.glyphs()[1];
    let y = glyph.position.y - glyph.size * 0.3;
    let red = (glyph.position.x as u32..(glyph.position.x + glyph.advance) as u32)
        .filter(|&x| x < SIZE)
        .any(|x| at(x as f32, y)[..3] == [255, 0, 0]);
    assert!(red, "「l」が描かれていません");

    // 字のない場所は背景のまま
    assert_eq!(at(SIZE as f32 - 1.0, SIZE as f32 - 1.0)[..3], [0, 0, 0]);
}

#[test]
fn shadow_is_drawn_behind_and_ghost_needs_alpha_blending() {
    const SIZE: u32 = 64;
    let config = sample_config();
    let bindings = config.bindings;
    let Ok(mut gpu) = GpuContext::new_headless(config) else {
        eprintln!("GPU を使うテストを飛ばします");
        return;
    };
    let surface = gpu.add_offscreen(SIZE, SIZE);
    let (mut resources, latin, japanese) = resources_with(ResourceConfig::for_tests());
    let shader = resources.register_shader(&text_shader(&bindings, GlyphMethod::Vector, Dimension::D2)).unwrap();
    let style = TextStyle { size: 40.0, ..style(latin, japanese) };
    let mut draw = |text: &str, blend: Blend| {
        let layout = resources.layout_text(&style, &Formatted::parse(text).unwrap());
        let mut manager = manager(&mut gpu);
        let mut object = text_object(&gpu);
        object.shader(shader).cull_mode(CullMode::None).blend(blend);

        gpu.begin_frame();
        object.begin();
        write_text(&mut object, &mut resources, &layout, &TextWrite { position: Vector3f::ZERO, ..text_write(YAxis::Down) });
        object.end(&mut manager);
        gpu.end_frame(&mut resources, &mut [SurfaceDraw { surface, clear_color: Some(Color::BLACK), draw_managers: &mut [&mut manager] }])
            .unwrap();
        let pixels = gpu.read_pixels(surface);
        // ベースラインの少し上の 1 行の rgb
        let y = (layout.rows()[0].baseline - 10.0) as u32;
        (0..SIZE).map(|x| {
            let [r, g, b, _] = pixels[(y * SIZE + x) as usize];
            [r, g, b]
        })
        .collect::<Vec<[u8; 3]>>()
    };

    // 影：字（255）の右に、字の色の rgb を 0.25 倍した影（sRGB で 64 前後）が出る
    let white = [255, 255, 255];
    let shadowed = draw("§[shadow 0.3 0]l", Blend::Opaque);
    let glyph_end = shadowed.iter().rposition(|&pixel| pixel == white).expect("字が描かれていません");
    assert!(shadowed[glyph_end + 1..].iter().any(|pixel| pixel.iter().all(|&value| (60..=68).contains(&value))), "{shadowed:?}");

    // 色を指定した影は、その色で出る
    let colored = draw("§[shadow 0.3 0 red]l", Blend::Opaque);
    let glyph_end = colored.iter().rposition(|&pixel| pixel == white).expect("字が描かれていません");
    assert!(colored[glyph_end + 1..].contains(&[255, 0, 0]), "{colored:?}");

    // Ghost：Blend::Alpha なら透け、Opaque では不透明度が捨てられて透けない（§13 書き込みの細部）
    let alpha = draw("§[ghost 0.5]§[hidebox]A", Blend::Alpha);
    assert!((180..=195).contains(&alpha[2][0]), "{alpha:?}");
    let opaque = draw("§[ghost 0.5]§[hidebox]A", Blend::Opaque);
    assert_eq!(opaque[2], white);
}
