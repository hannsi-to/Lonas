// Sdf / Msdf のテスト（段階 12）。描画のテストは GPU がない環境では飛ばす

use super::*;
use crate::resource::{ResourceConfig, ResourceManager};

const LATIN: &[u8] = include_bytes!("../../tests/fonts/Cantarell-Regular.ttf");

#[cfg(not(feature = "msdf"))]
mod without_feature {
    use super::*;
    use crate::config::tests::sample_config;
    use crate::wgsl::{Dimension, text_shader};

    #[test]
    #[should_panic(expected = "msdf feature")]
    fn text_shader_panics_without_msdf() {
        text_shader(&sample_config().bindings, GlyphMethod::Msdf, Dimension::D2);
    }

    #[test]
    #[should_panic(expected = "msdf feature")]
    fn prepare_glyphs_panics_without_msdf() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let font = resources.register_font(LATIN).unwrap();
        let layout = resources.layout_text(
            &TextStyle {
                families: vec![FontFamily::new(font)],
                size: 16.0,
                line_height: LineHeight::Font(1.0),
                letter_spacing: 0.0,
                max_width: None,
                align: TextAlign::Left,
                bold_weight: 0.03,
                outline_width: 0.05,
            },
            &Formatted::plain("a"),
        );

        resources.prepare_glyphs(&layout, GlyphMethod::Sdf, 1.0);
    }
}

#[cfg(feature = "msdf")]
mod with_feature {
    use super::super::bitmap::BitmapKey;
    use super::*;
    use crate::color::Color;
    use crate::config::tests::sample_config;
    use crate::config::{Blend, CullMode};
    use crate::draw::{Camera2D, DrawManager2D};
    use crate::gpu::{GpuContext, SurfaceDraw};
    use crate::instance::Instance2D;
    use crate::math::{Vector2f, Vector3f};
    use crate::memory::MemoryUpdateType;
    use crate::object::Object2D;
    use crate::vertex::VertexLayout;
    use crate::wgsl::{Dimension, text_shader};

    const SIZE: u32 = 96;
    const FONT_SIZE: f32 = 48.0;
    const BOLD: f32 = 0.05;
    const OUTLINE: f32 = 0.04;

    fn style(font: crate::resource::FontId) -> TextStyle {
        TextStyle {
            families: vec![FontFamily::new(font)],
            size: FONT_SIZE,
            line_height: LineHeight::Font(1.0),
            letter_spacing: 0.0,
            max_width: None,
            align: TextAlign::Left,
            bold_weight: BOLD,
            outline_width: OUTLINE,
        }
    }

    #[test]
    fn distance_shaders_pass_validation() {
        let bindings = sample_config().bindings;
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());

        for method in [GlyphMethod::Sdf, GlyphMethod::Msdf] {
            for dimension in [Dimension::D2, Dimension::D3] {
                let source = text_shader(&bindings, method, dimension);
                assert!(resources.register_shader(&source).is_ok(), "{source}");
            }
        }
    }

    #[test]
    fn distance_fields_are_positive_inside() {
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let font = resources.register_font(LATIN).unwrap();
        let layout = resources.layout_text(&style(font), &Formatted::plain("l"));
        let glyph = layout.glyphs()[0];

        for multi_channel in [false, true] {
            let key = BitmapKey::distance(glyph.font, glyph.glyph_id, multi_channel);
            resources.ensure_bitmap(key, false);
            let raster = &resources.bitmap_glyph(&key).raster;
            let channels = raster.bytes_per_pixel as usize;
            let value = |x: u32, y: u32| {
                let index = ((y * raster.width + x) as usize) * channels;
                let pixel = &raster.pixels[index..index + channels];
                if channels == 4 {
                    // rgb の中央値
                    let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
                    r.max(g).min(r.min(g).max(b))
                } else {
                    pixel[0]
                }
            };

            assert!(!raster.is_empty());
            assert_eq!(channels, if multi_channel { 4 } else { 1 });
            // 角は余白の外側（遠い）、真ん中の行のどこか（縦線の中）は内側
            assert!(value(0, 0) < 64, "{}", value(0, 0));
            let inside = (0..raster.width).map(|x| value(x, raster.height / 2)).max().unwrap();
            assert!(inside > 160, "{inside}");
        }
    }

    #[test]
    fn thickening_is_clamped_to_the_range() {
        let resources = ResourceManager::new(ResourceConfig::for_tests());
        // sdf_range 8・sdf_em_size 48：太らせられるのは 4 / 48 em まで
        let max = 4.0 / 48.0;

        assert_eq!(resources.clamp_thickening(0.05), 0.05);
        assert!((resources.clamp_thickening(1.0) - max).abs() < 1e-6);
    }

    // 1 行の、しきい値より明るい画素の数（左端から右端まで）
    fn span(row: &[[u8; 4]], channel: usize, threshold: u8) -> f32 {
        let lit: Vec<usize> = row.iter().enumerate().filter(|(_, pixel)| pixel[channel] > threshold).map(|(x, _)| x).collect();

        match (lit.first(), lit.last()) {
            (Some(&first), Some(&last)) => (last - first + 1) as f32,
            _ => 0.0,
        }
    }

    // 太字・縁取りの境目が normal.y の量だけ外に出る（段階 12 の終わりの条件）
    #[test]
    fn bold_and_outline_move_the_edge_by_the_thickening() {
        let config = sample_config();
        let bindings = config.bindings;
        let Ok(mut gpu) = GpuContext::new_headless(config) else {
            eprintln!("GPU を使うテストを飛ばします");
            return;
        };
        let surface = gpu.add_offscreen(SIZE, SIZE);
        let mut resources = ResourceManager::new(ResourceConfig::for_tests());
        let font = resources.register_font(LATIN).unwrap();
        let mut manager = DrawManager2D::new(&mut gpu, Camera2D { position: Vector2f::ZERO, rotation: 0.0, zoom: 1.0 }, VertexLayout::PositionColorUvNormal);

        for method in [GlyphMethod::Sdf, GlyphMethod::Msdf] {
            let shader = resources.register_shader(&text_shader(&bindings, method, Dimension::D2)).unwrap();
            let mut draw = |text: &str| {
                let layout = resources.layout_text(&style(font), &Formatted::parse(text).unwrap());
                let mut object = Object2D::new(&gpu);
                object.memory_update_type(MemoryUpdateType::Dynamic).shader(shader).cull_mode(CullMode::None).blend(Blend::Alpha);
                object.add_instance(Instance2D::default());

                gpu.begin_frame();
                object.begin();
                write_text(&mut object, &mut resources, &layout, &TextWrite {
                    position: Vector3f::new(30.0, 10.0, 0.0),
                    color: Color::WHITE,
                    y_axis: YAxis::Down,
                    pixel_scale: 1.0,
                    obfuscation_seed: 0,
                    method,
                });
                object.end(&mut manager);
                gpu.end_frame(&mut resources, &mut [SurfaceDraw { surface, clear_color: Some(Color::BLACK), draw_managers: &mut [&mut manager] }])
                    .unwrap();

                // 「l」の縦線の真ん中あたりの 1 行
                let y = (10.0 + layout.rows()[0].baseline - FONT_SIZE * 0.3) as u32;
                let pixels = gpu.read_pixels(surface);
                pixels[(y * SIZE) as usize..((y + 1) * SIZE) as usize].to_vec()
            };

            // 覆い率 0.5 は sRGB で 188。それより明るい画素を字の中とする
            let plain = span(&draw("l"), 1, 188);
            let bold = span(&draw("§[bold]l"), 1, 188);
            // 縁取りは赤。赤の範囲は、字と縁取りを合わせた幅
            let outlined = draw("§[outline #f00]l");
            let outline = span(&outlined, 0, 188);
            let inner = span(&outlined, 1, 188);

            assert!(plain > 2.0, "{method:?}: 字が描かれていません");
            // 左右に BOLD × size ずつ広がる
            let expected = 2.0 * BOLD * FONT_SIZE;
            assert!((bold - plain - expected).abs() <= 1.5, "{method:?}: 太字 {bold} − 元 {plain} が {expected} と合いません");
            let expected = 2.0 * OUTLINE * FONT_SIZE;
            assert!((outline - inner - expected).abs() <= 1.5, "{method:?}: 縁取り {outline} − 字 {inner} が {expected} と合いません");
            assert!((inner - plain).abs() <= 1.0, "{method:?}: 縁取りを付けても字の幅は変わらないはず（{inner} / {plain}）");
        }
    }
}
