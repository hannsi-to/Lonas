// フォントの登録と整形のテスト（段階 9）。フォントは tests/fonts（OFL）
// Cantarell はラテン文字だけ、Noto Sans JP の部分集合は日本語の字だけを持つ

use super::layout::segments_for_tests;
use super::*;
use crate::error::LonasError;
use crate::resource::{FontId, ResourceConfig, ResourceManager};

const LATIN: &[u8] = include_bytes!("../../tests/fonts/Cantarell-Regular.ttf");
const JAPANESE: &[u8] = include_bytes!("../../tests/fonts/NotoSansJP-Subset.ttf");

struct Fonts {
    resources: ResourceManager,
    latin: FontId,
    japanese: FontId,
}

fn fonts() -> Fonts {
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());
    let latin = resources.register_font(LATIN).unwrap();
    let japanese = resources.register_font(JAPANESE).unwrap();

    Fonts { resources, latin, japanese }
}

fn style(families: Vec<FontFamily>, max_width: Option<f32>) -> TextStyle {
    TextStyle {
        families,
        size: 10.0,
        line_height: LineHeight::Font(1.0),
        letter_spacing: 0.0,
        max_width,
        align: TextAlign::Left,
        bold_weight: 0.03,
        outline_width: 0.05,
    }
}

fn parse(text: &str) -> Formatted<'_> {
    Formatted::parse(text).unwrap()
}

// 行ごとに、置いたグリフの元の字を並べる
fn row_texts(layout: &TextLayout, plain: &str) -> Vec<String> {
    let mut rows = vec![String::new(); layout.rows().len()];
    let mut previous = None;

    for glyph in layout.glyphs() {
        if previous == Some((glyph.row, glyph.cluster)) {
            continue;
        }
        previous = Some((glyph.row, glyph.cluster));

        let character = plain[glyph.cluster..].chars().next().unwrap();
        rows[glyph.row].push(character);
    }

    rows
}

fn assert_near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 1e-3, "{actual} と {expected} が一致しません");
}

// ---- フォントの登録 ----

#[test]
fn registers_and_unregisters_fonts() {
    let Fonts { mut resources, latin, japanese } = fonts();

    assert!(resources.is_font_valid(latin));
    assert!(resources.is_font_valid(japanese));

    resources.unregister_font(latin);
    assert!(!resources.is_font_valid(latin));

    // 番号は再利用されるが、世代で古い ID と区別できる
    let again = resources.register_font(LATIN.to_vec()).unwrap();
    assert_ne!(again, latin);
    assert!(!resources.is_font_valid(latin));
    assert!(resources.is_font_valid(again));

    let other = ResourceManager::new(ResourceConfig::for_tests());
    assert!(!other.is_font_valid(again));
}

#[test]
fn rejects_bytes_that_are_not_a_font() {
    let mut resources = ResourceManager::new(ResourceConfig::for_tests());

    assert!(matches!(resources.register_font([1u8, 2, 3].as_slice()), Err(LonasError::InvalidFont)));
}

#[test]
#[should_panic(expected = "登録解除済みの FontId を解除しました")]
fn double_unregister_panics() {
    let Fonts { mut resources, latin, .. } = fonts();

    resources.unregister_font(latin);
    resources.unregister_font(latin);
}

#[test]
#[should_panic(expected = "sdf_range")]
fn invalid_text_config_panics() {
    let mut config = ResourceConfig::for_tests();
    config.text.sdf_range = 0.0;

    ResourceManager::new(config);
}

#[test]
#[should_panic(expected = "max_new_glyphs_per_frame")]
fn zero_glyphs_per_frame_panics() {
    let mut config = ResourceConfig::for_tests();
    config.text.max_new_glyphs_per_frame = Some(0);

    ResourceManager::new(config);
}

// ---- 書体の選び方 ----

#[test]
fn selects_faces_and_marks_synthesis() {
    let Fonts { latin: regular, japanese: bold, .. } = fonts();
    let only_regular = FontFamily::new(regular);

    let face = only_regular.select(true, true);
    assert_eq!(face.font, regular);
    assert!(face.synthetic_bold && face.synthetic_italic);

    // bold_italic がなければ bold に傾きを合成する
    let face = FontFamily::new(regular).bold(bold).select(true, true);
    assert_eq!(face.font, bold);
    assert!(!face.synthetic_bold && face.synthetic_italic);

    // それもなければ italic に太さを合成する
    let face = FontFamily::new(regular).italic(bold).select(true, true);
    assert_eq!(face.font, bold);
    assert!(face.synthetic_bold && !face.synthetic_italic);

    let face = FontFamily::new(regular).bold(bold).select(true, false);
    assert_eq!(face.font, bold);
    assert!(!face.synthetic_bold);
}

#[test]
fn bold_without_bold_face_is_synthesized() {
    let Fonts { resources, latin, .. } = fonts();
    let layout = resources.layout_text(&style(vec![FontFamily::new(latin)], None), &parse("a§[bold]b§[italic]c"));
    let styles: Vec<_> = layout.glyphs().iter().map(|glyph| glyph.style).collect();

    assert!(!styles[0].synthetic_bold);
    assert!(styles[1].synthetic_bold);
    assert!(styles[2].synthetic_bold);
    assert_eq!(styles[2].synthetic_italic, format::DEFAULT_ITALIC);
    // 合成の太字は送り幅を変えない
    let plain = resources.layout_text(&style(vec![FontFamily::new(latin)], None), &parse("abc"));
    assert_near(layout.size().x, plain.size().x);
}

// ---- 区間の分け方 ----

#[test]
fn color_does_not_split_runs_but_font_size_does() {
    let Fonts { resources, latin, .. } = fonts();
    let style = style(vec![FontFamily::new(latin)], None);

    // 色・下線・Ghost が変わっても区間は 1 つ
    let segments = segments_for_tests(&resources, &style, &parse("ab§[red]cd§[u]ef§[ghost]gh"));
    assert_eq!(segments, vec![0..8]);

    // FontSize が変わる位置で分かれる
    let segments = segments_for_tests(&resources, &style, &parse("ab§[size 20]cd§[/size]ef"));
    assert_eq!(segments, vec![0..2, 2..4, 4..6]);

    // 改行の位置で分かれる
    let segments = segments_for_tests(&resources, &style, &parse("ab\ncd"));
    assert_eq!(segments, vec![0..2, 3..5]);
}

#[test]
fn bold_splits_runs_only_when_the_face_changes() {
    let Fonts { resources, latin, japanese } = fonts();
    let text = parse("ab§[bold]cd");

    // 太字の書体がない（合成する）なら分けない
    let synthesized = style(vec![FontFamily::new(latin)], None);
    assert_eq!(segments_for_tests(&resources, &synthesized, &text), vec![0..4]);

    // 太字の書体があれば分ける
    let with_bold = style(vec![FontFamily::new(latin).bold(japanese)], None);
    assert_eq!(segments_for_tests(&resources, &with_bold, &text), vec![0..2, 2..4]);
}

// ---- フォールバック ----

#[test]
fn falls_back_per_cluster_and_uses_notdef_of_the_first_family() {
    let Fonts { resources, latin, japanese } = fonts();
    let style = style(vec![FontFamily::new(latin), FontFamily::new(japanese)], None);
    // A（ラテン）・あ（日本語）・ก（タイ文字。どちらにもない）
    let layout = resources.layout_text(&style, &Formatted::plain("Aあก"));
    let glyphs = layout.glyphs();

    assert_eq!(glyphs.len(), 3);
    assert_eq!(glyphs[0].font, latin);
    assert_ne!(glyphs[0].glyph_id, 0);
    assert_eq!(glyphs[1].font, japanese);
    assert_ne!(glyphs[1].glyph_id, 0);
    // どれにもない字は先頭のフォントの .notdef
    assert_eq!(glyphs[2].font, latin);
    assert_eq!(glyphs[2].glyph_id, 0);

    // 字の順に左から並ぶ
    assert!(glyphs[0].position.x < glyphs[1].position.x && glyphs[1].position.x < glyphs[2].position.x);
}

#[test]
fn fallback_order_follows_families() {
    let Fonts { resources, latin, japanese } = fonts();
    // 日本語のフォントを先にしても、ラテン文字は持たないため次の Cantarell で描く
    let style = style(vec![FontFamily::new(japanese), FontFamily::new(latin)], None);
    let layout = resources.layout_text(&style, &Formatted::plain("あA"));

    assert_eq!(layout.glyphs()[0].font, japanese);
    assert_eq!(layout.glyphs()[1].font, latin);
}

// ---- 折り返し ----

#[test]
fn japanese_wraps_between_characters() {
    let Fonts { resources, japanese, .. } = fonts();
    let text = "日本語の文章を折り返して表示する";
    // 1 字が 1 em（10）。7 字分の幅
    let layout = resources.layout_text(&style(vec![FontFamily::new(japanese)], Some(70.5)), &Formatted::plain(text));

    assert_eq!(row_texts(&layout, text), vec!["日本語の文章を", "折り返して表示", "する"]);
    for row in layout.rows() {
        assert!(row.width <= 70.5);
    }
}

#[test]
fn punctuation_does_not_start_a_line() {
    let Fonts { resources, japanese, .. } = fonts();
    let text = "あいうえおかきくけこ。さしす";
    // 10 字ちょうどの幅。字の間で切ると「。」が行頭に来る
    let layout = resources.layout_text(&style(vec![FontFamily::new(japanese)], Some(100.5)), &Formatted::plain(text));
    let rows = row_texts(&layout, text);

    assert_eq!(rows, vec!["あいうえおかきくけ", "こ。さしす"]);
    for row in &rows {
        assert!(!row.starts_with(['。', '、']));
    }
}

#[test]
fn long_word_is_cut_at_cluster_boundaries() {
    let Fonts { resources, latin, .. } = fonts();
    let text = "abcdefghijklmnopqrstuvwxyz";
    let max_width = 40.0;
    let layout = resources.layout_text(&style(vec![FontFamily::new(latin)], Some(max_width)), &Formatted::plain(text));
    let rows = row_texts(&layout, text);

    assert!(rows.len() > 2, "{rows:?}");
    // 字を落とさず、順に並ぶ
    assert_eq!(rows.concat(), text);
    for row in layout.rows() {
        assert!(row.width <= max_width, "{} > {max_width}", row.width);
    }
}

#[test]
fn breaks_at_spaces_and_trailing_space_is_not_counted() {
    let Fonts { resources, latin, .. } = fonts();
    let text = "aaa aaa";
    let single = resources.layout_text(&style(vec![FontFamily::new(latin)], None), &Formatted::plain("aaa"));
    // "aaa" と少しの幅。後ろの "aaa" は次の行へ
    let max_width = single.size().x + 1.0;
    let layout = resources.layout_text(&style(vec![FontFamily::new(latin)], Some(max_width)), &Formatted::plain(text));

    assert_eq!(row_texts(&layout, text), vec!["aaa ", "aaa"]);
    // 行末の空白は幅に含めない
    assert_near(layout.rows()[0].width, single.size().x);
}

#[test]
fn newlines_make_rows() {
    let Fonts { resources, latin, .. } = fonts();
    let style = style(vec![FontFamily::new(latin)], None);

    assert_eq!(resources.layout_text(&style, &parse("a\nb§[ln]c")).rows().len(), 3);
    // \r\n は 1 つの改行
    assert_eq!(resources.layout_text(&style, &parse("a\r\nb")).rows().len(), 2);
    // 末尾の改行の後ろに空の行がある
    let layout = resources.layout_text(&style, &parse("a\n"));
    assert_eq!(layout.rows().len(), 2);
    assert_eq!(layout.glyphs().len(), 1);
    // 空の文字列も 1 行
    assert_eq!(resources.layout_text(&style, &Formatted::new()).rows().len(), 1);
}

// ---- 行の高さ・揃え ----

#[test]
fn row_height_uses_the_largest_font_size() {
    let Fonts { mut resources, latin, .. } = fonts();
    let metrics = resources.font(latin).metrics;
    let style = style(vec![FontFamily::new(latin)], None);
    let pitch = metrics.ascender - metrics.descender + metrics.line_gap;
    let layout = resources.layout_text(&style, &parse("a\nb§[size 20]c§[/size]d\ne"));
    let rows = layout.rows();

    assert_near(rows[0].top, 0.0);
    assert_near(rows[0].baseline, metrics.ascender * 10.0);
    assert_near(rows[0].height, pitch * 10.0);
    // 2 行目は一番大きい FontSize（20）の行送り
    assert_near(rows[1].top, pitch * 10.0);
    assert_near(rows[1].height, pitch * 20.0);
    assert_near(rows[2].top, pitch * 30.0);
    // 高さは最後の行の下端まで
    assert_near(layout.size().y, pitch * 30.0 + (metrics.ascender - metrics.descender) * 10.0);

    // 使い終わったフォントを解除しても、作った TextLayout はそのまま使える
    resources.unregister_font(latin);
    assert_eq!(layout.rows().len(), 3);
}

#[test]
fn space_y_is_added_below_the_row_and_space_x_between_characters() {
    let Fonts { resources, latin, .. } = fonts();
    let style = style(vec![FontFamily::new(latin)], None);
    let plain = resources.layout_text(&style, &parse("ab\nc"));
    let spaced = resources.layout_text(&style, &parse("§[spacey 5]ab\nc"));

    assert_near(spaced.rows()[1].top, plain.rows()[1].top + 5.0);

    // その行の最後の字の時点の SpaceY を使う。最後の字より後に変えても効かない
    let late = resources.layout_text(&style, &parse("ab§[spacey 5]\nc"));
    assert_near(late.rows()[1].top, plain.rows()[1].top);

    let wide = resources.layout_text(&style, &parse("§[spacex 3]ab"));
    let narrow = resources.layout_text(&style, &parse("ab"));
    assert_near(wide.glyphs()[1].position.x, narrow.glyphs()[1].position.x + 3.0);
}

#[test]
fn align_shifts_each_row() {
    let Fonts { resources, latin, .. } = fonts();
    let text = parse("aaaa\naa");
    let mut style = style(vec![FontFamily::new(latin)], None);

    let left = resources.layout_text(&style, &text);
    let [wide, narrow] = [left.rows()[0].width, left.rows()[1].width];
    assert_near(left.size().x, wide);
    assert_near(left.rows()[1].x, 0.0);

    style.align = TextAlign::Center;
    let center = resources.layout_text(&style, &text);
    assert_near(center.rows()[0].x, 0.0);
    assert_near(center.rows()[1].x, (wide - narrow) / 2.0);
    assert_near(center.glyphs()[4].position.x, left.glyphs()[4].position.x + (wide - narrow) / 2.0);

    style.align = TextAlign::Right;
    let right = resources.layout_text(&style, &text);
    assert_near(right.rows()[1].x, wide - narrow);
    assert_near(right.glyphs()[5].position.x, left.glyphs()[5].position.x + wide - narrow);
}

// ---- 書式の積み上げ・線 ----

#[test]
fn formats_stack_and_reset() {
    let Fonts { resources, latin, .. } = fonts();
    let layout = resources.layout_text(
        &style(vec![FontFamily::new(latin)], None),
        &parse("§[red]§[hidebox]a§[skippush]b§[skippop]c§[/hidebox]d§[/]e"),
    );
    let red = Some(NamedColor::Red.color());
    let styles: Vec<_> = layout.glyphs().iter().map(|glyph| (glyph.style.color, glyph.style.hide_box)).collect();

    assert_eq!(styles, vec![(red, true), (None, false), (red, true), (red, false), (None, false)]);
}

#[test]
fn decorations_merge_while_the_style_continues() {
    let Fonts { resources, latin, .. } = fonts();
    let metrics = resources.font(latin).metrics;
    let layout = resources.layout_text(&style(vec![FontFamily::new(latin)], None), &parse("§[u]ab§[red]c§[/u]d§[s]e"));
    let glyphs = layout.glyphs();
    let lines = layout.lines();

    // a・b で 1 本、色が変わる c で 1 本、e の打消し線で 1 本
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0].kind, LineKind::Underline);
    assert_near(lines[0].start.x, 0.0);
    assert_near(lines[0].length, glyphs[2].position.x);
    assert_eq!(lines[1].color, Some(NamedColor::Red.color()));
    assert_near(lines[1].start.x, glyphs[2].position.x);
    assert_near(lines[1].start.x + lines[1].length, glyphs[3].position.x);
    assert_eq!(lines[2].kind, LineKind::Strikethrough);

    // 位置はフォントの値、太さは LineDecoration::line_width
    let baseline = layout.rows()[0].baseline;
    assert_near(lines[0].start.y, baseline - metrics.underline_position * 10.0);
    assert_near(lines[2].start.y, baseline - metrics.strikeout_position * 10.0);
    assert_near(lines[0].thickness, format::DEFAULT_LINE_WIDTH * 10.0);
}

#[test]
fn decorations_are_cut_at_row_ends() {
    let Fonts { resources, japanese, .. } = fonts();
    let layout = resources.layout_text(&style(vec![FontFamily::new(japanese)], Some(30.5)), &parse("§[u]あいうえお"));

    assert_eq!(layout.rows().len(), 2);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(layout.lines()[1].row, 1);
    assert_near(layout.lines()[0].length, 30.0);
}

#[test]
fn plain_does_not_interpret_formats() {
    let Fonts { resources, latin, .. } = fonts();
    let text = "§[red]a";
    let layout = resources.layout_text(&style(vec![FontFamily::new(latin)], None), &Formatted::plain(text));

    assert_eq!(row_texts(&layout, text), vec![text]);
    assert!(layout.glyphs().iter().all(|glyph| glyph.style.color.is_none()));
}

// ---- 誤用 ----

#[test]
#[should_panic(expected = "TextStyle::families が空です")]
fn empty_families_panics() {
    let Fonts { resources, .. } = fonts();

    resources.layout_text(&style(Vec::new(), None), &parse("a"));
}

#[test]
#[should_panic(expected = "登録解除済みの FontId です")]
fn unregistered_font_panics() {
    let Fonts { mut resources, latin, .. } = fonts();

    resources.unregister_font(latin);
    resources.layout_text(&style(vec![FontFamily::new(latin)], None), &parse("a"));
}
