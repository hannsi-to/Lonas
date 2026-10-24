// 整形・折り返し・揃え（§13 整形と書き込みの分け方・整形・折り返し・位置と揃え・整形の結果）
// 文字列か書式が変わった時だけ呼ぶ。毎フレームの書き込み（write_text）は、ここで作った TextLayout を写すだけ

use std::ops::Range;

use harfrust::{Buffer, BufferFlags, Direction, ShapeOptions, ShaperFont};
use unicode_linebreak::{BreakOpportunity, linebreaks};

use crate::color::Color;
use crate::math::Vector2f;
use crate::resource::{FontId, ResourceManager};
use crate::text::font::{Face, FontFamily, FontMetrics};
use crate::text::format::{Formatted, LineDecoration, ResetTarget, Shadow, TextFormat, Token};

// 整形の設定。書き込みの設定（色・位置など）は TextWrite に置く
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    // 先頭から字を探す（§13 フォントの組とフォールバック）。空なら layout_text で panic
    pub families: Vec<FontFamily>,
    // 1 em の大きさ。頂点と同じ単位（2D ならピクセル）
    pub size: f32,
    pub line_height: LineHeight,
    // 字間に足す量。TextFormat::SpaceX で置き換えられる
    pub letter_spacing: f32,
    // None なら改行文字と TextFormat::NewLine でだけ改行する
    pub max_width: Option<f32>,
    pub align: TextAlign,
    // TextFormat::Bold で太らせる量（em に対する割合）。太字の書体がない場合に使う
    pub bold_weight: f32,
    // TextFormat::Outline の太さ（em に対する割合）
    pub outline_width: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LineHeight {
    // 先頭の FontFamily の regular の推奨の行送り × 倍率 × その行で一番大きい FontSize
    Font(f32),
    // FontSize によらず、この値
    Fixed(f32),
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

// 書き込みで使う書式（§13 書式の作り方の表のうち、書き込み・キャッシュの層のもの）
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GlyphStyle {
    // None なら TextWrite::color
    pub color: Option<Color>,
    // 透ける割合。不透明度に 1 − ghost を掛ける
    pub ghost: f32,
    // 太字の書体がなく、合成で太らせる
    pub synthetic_bold: bool,
    // 斜体の書体がなく、傾ける量。0 なら傾けない
    pub synthetic_italic: f32,
    pub obfuscated: bool,
    pub hide_box: bool,
    pub shadow: Option<Shadow>,
    pub outline: Option<Color>,
}

// 置いたグリフ。座標は y が下向きで、1 行目の上端の左端が原点（§13 整形の結果）
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlacedGlyph {
    // 実際に使った書体（フォールバック・太字・斜体の書体を選んだ後）
    pub font: FontId,
    // フォントの中のグリフの番号。0 は .notdef
    pub glyph_id: u32,
    // グリフの原点（ベースライン上）。揃えを反映した位置
    pub position: Vector2f,
    // 1 em の大きさ（FontSize を反映）
    pub size: f32,
    // 送り幅（SpaceX を含まない）
    pub advance: f32,
    // 書式を除いた文字列（Formatted::plain_text）の中のバイト位置
    pub cluster: usize,
    pub row: usize,
    pub style: GlyphStyle,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    Underline,
    Strikethrough,
    Overline,
}

// 下線・打消し線・上線。同じ線が続く字をまとめて 1 本にしたもの
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlacedLine {
    pub kind: LineKind,
    // 線の中心の左端
    pub start: Vector2f,
    pub length: f32,
    // LineDecoration::line_width × FontSize
    pub thickness: f32,
    pub decoration: LineDecoration,
    pub color: Option<Color>,
    pub ghost: f32,
    pub row: usize,
}

// 1 行分の位置と大きさ。空の行も 1 行
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextRow {
    pub top: f32,
    pub baseline: f32,
    // ベースライン − descender × その行で一番大きい FontSize
    pub bottom: f32,
    // 行送り（LineHeight の値。SpaceY を含まない）
    pub height: f32,
    // 揃えを反映した行の左端
    pub x: f32,
    // 行の幅（行末の空白を除く）
    pub width: f32,
}

// 整形の結果（§13 整形の結果：TextLayout）。FontId を持つだけで、フォントのデータを借りない
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    glyphs: Vec<PlacedGlyph>,
    lines: Vec<PlacedLine>,
    rows: Vec<TextRow>,
    size: Vector2f,
    bold_weight: f32,
    outline_width: f32,
}

impl TextLayout {
    // (幅, 高さ)。幅は一番長い行の幅、高さは最後の行の下端まで（§13 位置と揃え）
    pub fn size(&self) -> Vector2f {
        self.size
    }

    // 書く順（行の順、行の中は左から右）
    pub fn glyphs(&self) -> &[PlacedGlyph] {
        &self.glyphs
    }

    pub fn lines(&self) -> &[PlacedLine] {
        &self.lines
    }

    pub fn rows(&self) -> &[TextRow] {
        &self.rows
    }

    pub fn bold_weight(&self) -> f32 {
        self.bold_weight
    }

    pub fn outline_width(&self) -> f32 {
        self.outline_width
    }
}

// ---- 書式の積み上げ ----

// 積み上がった書式。どれも「指定されていない」が既定（gueiz の FormatState と同じ）
#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct FormatState {
    color: Option<Color>,
    size: Option<f32>,
    ghost: f32,
    italic: f32,
    bold: bool,
    obfuscated: bool,
    hidden: bool,
    shadow: Option<Shadow>,
    outline: Option<Color>,
    underline: Option<LineDecoration>,
    strikethrough: Option<LineDecoration>,
    overline: Option<LineDecoration>,
    space_x: Option<f32>,
    space_y: Option<f32>,
}

impl FormatState {
    // 状態だけで決まる書式を積む。行やスタックに関わるもの（NewLine・SkipPush・SkipPop）はそのまま返す
    fn apply(&mut self, format: TextFormat) -> Option<TextFormat> {
        match format {
            TextFormat::Color(color) => self.color = Some(color),
            TextFormat::DefaultColor => self.color = None,
            TextFormat::Obfuscated => self.obfuscated = true,
            TextFormat::Bold => self.bold = true,
            TextFormat::FontSize(size) => self.size = Some(size),
            TextFormat::Strikethrough(line) => self.strikethrough = Some(line),
            TextFormat::Underline(line) => self.underline = Some(line),
            TextFormat::Overline(line) => self.overline = Some(line),
            TextFormat::Italic(value) => self.italic = value,
            TextFormat::Ghost(value) => self.ghost = value.clamp(0.0, 1.0),
            TextFormat::HideBox => self.hidden = true,
            TextFormat::Shadow(shadow) => self.shadow = Some(shadow),
            TextFormat::Outline(color) => self.outline = Some(color),
            TextFormat::SpaceX(value) => self.space_x = Some(value),
            TextFormat::SpaceY(value) => self.space_y = Some(value),
            TextFormat::Reset(target) => self.reset(target),

            other => return Some(other),
        }

        None
    }

    fn reset(&mut self, target: ResetTarget) {
        match target {
            ResetTarget::All => *self = Self::default(),
            ResetTarget::Color => self.color = None,
            ResetTarget::Obfuscated => self.obfuscated = false,
            ResetTarget::Bold => self.bold = false,
            ResetTarget::FontSize => self.size = None,
            ResetTarget::Strikethrough => self.strikethrough = None,
            ResetTarget::Underline => self.underline = None,
            ResetTarget::Overline => self.overline = None,
            ResetTarget::Italic => self.italic = 0.0,
            ResetTarget::Ghost => self.ghost = 0.0,
            ResetTarget::HideBox => self.hidden = false,
            ResetTarget::Shadow => self.shadow = None,
            ResetTarget::Outline => self.outline = None,
            ResetTarget::SpaceX => self.space_x = None,
            ResetTarget::SpaceY => self.space_y = None,
        }
    }

    fn italic(&self) -> bool {
        self.italic != 0.0
    }
}

// 積み上げた書式と、SkipPush で退避した分
#[derive(Clone, Debug, Default)]
struct FormatStack {
    state: FormatState,
    skipped: Vec<FormatState>,
}

impl FormatStack {
    // 書式を 1 つ積む。改行なら true
    fn apply(&mut self, format: TextFormat) -> bool {
        let Some(rest) = self.state.apply(format) else {
            return false;
        };

        match rest {
            TextFormat::NewLine => return true,
            TextFormat::SkipPush => {
                self.skipped.push(self.state);
                self.state = FormatState::default();
            }
            // 退避がなければ何もしない
            TextFormat::SkipPop => {
                if let Some(previous) = self.skipped.pop() {
                    self.state = previous;
                }
            }
            unexpected => unreachable!("{unexpected:?} は FormatState が引き受けるはず"),
        }

        false
    }
}

// 書式を除いた文字列と、バイト位置ごとの書式。NewLine は '\n' にする
struct FlatText {
    text: String,
    // (始まりのバイト位置, 書式)。次の要素の始まりまで続く
    spans: Vec<(usize, FormatState)>,
    // 最後の書式。文字列の末尾の空の行に使う
    end_state: FormatState,
}

impl FlatText {
    fn new(formatted: &Formatted) -> Self {
        let mut stack = FormatStack::default();
        let mut flat = Self { text: String::new(), spans: Vec::new(), end_state: FormatState::default() };

        for token in formatted.tokens() {
            match *token {
                Token::Text(text) => flat.push(text, stack.state),
                Token::Format(format) => {
                    if stack.apply(format) {
                        flat.push("\n", stack.state);
                    }
                }
            }
        }

        flat.end_state = stack.state;

        flat
    }

    fn push(&mut self, text: &str, state: FormatState) {
        if self.spans.last().is_none_or(|&(_, last)| last != state) {
            self.spans.push((self.text.len(), state));
        }

        self.text.push_str(text);
    }

    fn state_at(&self, position: usize) -> &FormatState {
        if position >= self.text.len() {
            return &self.end_state;
        }

        let index = self.spans.partition_point(|&(start, _)| start <= position) - 1;

        &self.spans[index].1
    }
}

// ---- 整形 ----

// 整形したグリフ。x はクラスターの左端からの位置
#[derive(Copy, Clone, Debug)]
struct ShapedGlyph {
    // TextStyle::families の番号
    family: usize,
    face: Face,
    glyph_id: u32,
    x_offset: f32,
    y_offset: f32,
    advance: f32,
}

// クラスター（HarfRust の cluster）。折り返しの最小の単位
#[derive(Clone, Debug)]
struct Cluster {
    range: Range<usize>,
    glyphs: Vec<ShapedGlyph>,
    // グリフの送り幅の和
    advance: f32,
    // 後ろに足す字間（SpaceX / letter_spacing）
    spacing: f32,
    size: f32,
    whitespace: bool,
}

impl Cluster {
    fn width(&self) -> f32 {
        self.advance + self.spacing
    }
}

struct Layouter<'a> {
    resources: &'a ResourceManager,
    style: &'a TextStyle,
    flat: FlatText,
    // 行送り・線の位置に使う（先頭の FontFamily の regular、§13 フォントの組とフォールバック）
    metrics: FontMetrics,
}

impl ResourceManager {
    // 文字列を整形し、グリフの位置と書式を求める（§13 整形と書き込みの分け方）
    // TextStyle の値が不正なら panic（§13 CPU 側の設定）
    pub fn layout_text(&self, style: &TextStyle, formatted: &Formatted) -> TextLayout {
        validate_style(self, style);

        let layouter = Layouter {
            resources: self,
            style,
            flat: FlatText::new(formatted),
            metrics: self.font(style.families[0].regular).metrics,
        };

        layouter.layout()
    }
}

fn validate_style(resources: &ResourceManager, style: &TextStyle) {
    assert!(!style.families.is_empty(), "TextStyle::families が空です");
    assert!(style.size.is_finite() && style.size > 0.0, "TextStyle::size は 0 より大きい有限の値にしてください（{}）", style.size);

    if let Some(width) = style.max_width {
        assert!(width.is_finite() && width > 0.0, "TextStyle::max_width は 0 より大きい有限の値にしてください（{width}）");
    }

    let (LineHeight::Font(value) | LineHeight::Fixed(value)) = style.line_height;
    assert!(value.is_finite() && value > 0.0, "TextStyle::line_height は 0 より大きい有限の値にしてください（{value}）");

    for (name, value) in [
        ("letter_spacing", style.letter_spacing),
        ("bold_weight", style.bold_weight),
        ("outline_width", style.outline_width),
    ] {
        assert!(value.is_finite(), "TextStyle::{name} は有限の値にしてください（{value}）");
    }

    // 登録解除済み・別の ResourceManager の FontId なら panic
    for font in style.families.iter().flat_map(FontFamily::fonts) {
        resources.font(font);
    }
}

// UAX #14 で必ず改行する文字（§13 整形・折り返し）
fn is_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

// 線 1 本分の見た目。ここが変わったら線を切る
#[derive(Copy, Clone, PartialEq)]
struct LineRun {
    decoration: LineDecoration,
    color: Option<Color>,
    ghost: f32,
    size: f32,
}

const LINE_KINDS: [LineKind; 3] = [LineKind::Underline, LineKind::Strikethrough, LineKind::Overline];

// 並べる前の行
struct PendingRow {
    clusters: Vec<Cluster>,
    // 行末の空白を除いたクラスターの数
    visible: usize,
    width: f32,
    size: f32,
    space_y: f32,
}

impl Layouter<'_> {
    fn layout(self) -> TextLayout {
        let mut rows = Vec::new();

        for paragraph in self.paragraphs() {
            let clusters = self.shape_paragraph(paragraph.clone());
            self.break_lines(paragraph, clusters, &mut rows);
        }

        self.place(rows)
    }

    // 改行で分けた段落。改行の文字は含めない。末尾が改行なら、その後ろに空の段落を足す
    fn paragraphs(&self) -> Vec<Range<usize>> {
        let text = &self.flat.text;
        let mut paragraphs = Vec::new();
        let mut start = 0;

        for (position, opportunity) in linebreaks(text) {
            if opportunity != BreakOpportunity::Mandatory {
                continue;
            }

            let end = text[start..position].trim_end_matches(is_line_terminator).len() + start;
            paragraphs.push(start..end);
            start = position;
        }

        if text.is_empty() || text.ends_with(is_line_terminator) {
            paragraphs.push(text.len()..text.len());
        }

        paragraphs
    }

    // 書体の大きさと選ぶ書体が変わる位置で区間に分け、区間ごとに整形する（§13 整形・折り返しの 1・2）
    fn shape_paragraph(&self, paragraph: Range<usize>) -> Vec<Cluster> {
        let mut clusters = Vec::new();

        for segment in self.segments(paragraph.clone()) {
            self.shape_segment(&paragraph, segment, &mut clusters);
        }

        clusters
    }

    // 段落を区間に分ける。フォールバックで分ける前の区間
    fn segments(&self, paragraph: Range<usize>) -> Vec<Range<usize>> {
        let mut segments = Vec::new();
        let mut segment_start = paragraph.start;
        let mut segment_key = None;

        for (offset, _) in self.flat.text[paragraph.clone()].char_indices() {
            let position = paragraph.start + offset;
            let key = self.segment_key(self.flat.state_at(position));

            if segment_key.as_ref().is_some_and(|current| *current != key) {
                segments.push(segment_start..position);
                segment_start = position;
            }

            segment_key = Some(key);
        }

        if segment_start < paragraph.end {
            segments.push(segment_start..paragraph.end);
        }

        segments
    }

    // 区間を分ける値：字の大きさと、FontFamily ごとに選ぶ書体。合成するだけなら書体は変わらない
    fn segment_key(&self, state: &FormatState) -> (u32, Vec<FontId>) {
        let size = state.size.unwrap_or(self.style.size);
        let fonts = self.style.families.iter().map(|family| family.select(state.bold, state.italic()).font).collect();

        (size.to_bits(), fonts)
    }

    // 区間を先頭の FontFamily で整形してクラスターを求め、クラスターごとに FontFamily を選び、
    // 同じ FontFamily が続く範囲ごとに整形し直す（§13 フォントの組とフォールバック）
    fn shape_segment(&self, paragraph: &Range<usize>, segment: Range<usize>, clusters: &mut Vec<Cluster>) {
        let state = *self.flat.state_at(segment.start);
        let families = &self.style.families;
        let first = self.shape(paragraph, segment.clone(), 0, &state);

        let mut choices: Vec<(Range<usize>, usize)> = Vec::new();
        for range in cluster_ranges(&first, segment.end) {
            let family = self.family_for(&self.flat.text[range.clone()], &state);

            match choices.last_mut() {
                Some((last, last_family)) if *last_family == family => last.end = range.end,
                _ => choices.push((range, family)),
            }
        }

        let glyphs = if families.len() == 1 || matches!(choices.as_slice(), [(_, 0)]) {
            first
        } else {
            choices.into_iter().flat_map(|(range, family)| self.shape(paragraph, range, family, &state)).collect()
        };

        let size = state.size.unwrap_or(self.style.size);
        let ranges = cluster_ranges(&glyphs, segment.end);
        let mut glyphs = glyphs.into_iter().peekable();

        for range in ranges {
            let cluster_state = self.flat.state_at(range.start);
            let mut cluster_glyphs = Vec::new();
            while let Some((_, mut glyph)) = glyphs.next_if(|(cluster, _)| *cluster < range.end) {
                // 区間の中で太字・斜体が変わっても書体は同じ（segment_key）。合成の印だけをクラスターの書式で決め直す
                glyph.face = families[glyph.family].select(cluster_state.bold, cluster_state.italic());
                cluster_glyphs.push(glyph);
            }

            let text = &self.flat.text[range.clone()];

            clusters.push(Cluster {
                advance: cluster_glyphs.iter().map(|glyph| glyph.advance).sum(),
                spacing: cluster_state.space_x.unwrap_or(self.style.letter_spacing),
                glyphs: cluster_glyphs,
                size,
                whitespace: text.chars().all(char::is_whitespace),
                range,
            });
        }
    }

    // クラスターのすべての文字を持つ最初の FontFamily。どれにもなければ先頭（.notdef を描く）
    fn family_for(&self, text: &str, state: &FormatState) -> usize {
        self.style
            .families
            .iter()
            .position(|family| {
                let font = self.resources.font(family.select(state.bold, state.italic()).font);
                text.chars().all(|character| font.has_char(character))
            })
            .unwrap_or(0)
    }

    // HarfRust で整形する。返すのは (クラスターの始まりのバイト位置, グリフ)
    fn shape(&self, paragraph: &Range<usize>, range: Range<usize>, family: usize, state: &FormatState) -> Vec<(usize, ShapedGlyph)> {
        let text = &self.flat.text;
        let face = self.style.families[family].select(state.bold, state.italic());
        let font = self.resources.font(face.font);
        let scale = state.size.unwrap_or(self.style.size) / font.units_per_em;

        let mut buffer = Buffer::new();
        for (offset, character) in text[range.clone()].char_indices() {
            buffer.push(character as u32, (range.start + offset) as u32);
        }
        // 区間の前後の文字は文脈として渡す。段落の端なら BOT / EOT
        buffer.set_pre_context(&text[paragraph.start..range.start]);
        buffer.set_post_context(&text[range.end..paragraph.end]);
        let mut flags = BufferFlags::empty();
        flags.set(BufferFlags::BEGINNING_OF_TEXT, range.start == paragraph.start);
        flags.set(BufferFlags::END_OF_TEXT, range.end == paragraph.end);
        buffer.set_flags(flags);
        buffer.guess_segment_properties();
        // 左から右の横書きだけを扱う（§13 整形・折り返し）
        buffer.set_direction(Direction::LeftToRight);

        let shaper = ShaperFont::new(&font.font);
        harfrust::shape(&shaper, &mut buffer, ShapeOptions::new()).expect("HarfRust の整形に失敗しました");

        buffer
            .glyph_infos()
            .iter()
            .zip(buffer.glyph_positions())
            .map(|(info, position)| {
                let glyph = ShapedGlyph {
                    family,
                    face,
                    glyph_id: info.glyph_id,
                    x_offset: position.x_offset as f32 * scale,
                    y_offset: position.y_offset as f32 * scale,
                    advance: position.x_advance as f32 * scale,
                };

                (info.cluster as usize, glyph)
            })
            .collect()
    }

    // UAX #14 の折り返せる場所のうち、幅に収まる最後の場所で改行する。なければクラスターの境目で切る
    fn break_lines(&self, paragraph: Range<usize>, clusters: Vec<Cluster>, rows: &mut Vec<PendingRow>) {
        let allowed: Vec<usize> = linebreaks(&self.flat.text[paragraph.clone()])
            .filter(|&(_, opportunity)| opportunity == BreakOpportunity::Allowed)
            .map(|(position, _)| paragraph.start + position)
            .collect();
        let mut breaks = Vec::new();

        if let Some(max_width) = self.style.max_width {
            let mut line_start = 0;
            let mut last_break = None;
            let mut width = 0.0;
            let mut index = 0;

            while index < clusters.len() {
                let cluster = &clusters[index];

                if index > line_start {
                    if allowed.binary_search(&cluster.range.start).is_ok() {
                        last_break = Some(index);
                    }

                    // 空白は行末に残してよい（幅に数えない）
                    if !cluster.whitespace && width + cluster.advance > max_width {
                        let at = last_break.unwrap_or(index);
                        breaks.push(at);
                        line_start = at;
                        last_break = None;
                        width = 0.0;
                        index = at;
                        continue;
                    }
                }

                width += cluster.width();
                index += 1;
            }
        }

        let paragraph_state = *self.flat.state_at(paragraph.start);
        let mut clusters = clusters.into_iter();
        let mut previous = 0;

        for end in breaks.into_iter().chain([usize::MAX]) {
            let line: Vec<Cluster> = clusters.by_ref().take(end.saturating_sub(previous)).collect();
            previous = end;

            let visible = line.iter().rposition(|cluster| !cluster.whitespace).map_or(0, |index| index + 1);
            let state = line.last().map_or(paragraph_state, |cluster| *self.flat.state_at(cluster.range.start));

            rows.push(PendingRow {
                width: line[..visible].iter().map(Cluster::width).sum(),
                size: line
                    .iter()
                    .map(|cluster| cluster.size)
                    .reduce(f32::max)
                    .unwrap_or(state.size.unwrap_or(self.style.size)),
                space_y: state.space_y.unwrap_or(0.0),
                visible,
                clusters: line,
            });
        }
    }

    // 行の高さ・揃えを決め、グリフと線を置く（§13 位置と揃え）
    fn place(&self, pending: Vec<PendingRow>) -> TextLayout {
        let metrics = self.metrics;
        let width = pending.iter().map(|row| row.width).fold(0.0, f32::max);
        let mut layout = TextLayout {
            glyphs: Vec::new(),
            lines: Vec::new(),
            rows: Vec::new(),
            size: Vector2f::ZERO,
            bold_weight: self.style.bold_weight,
            outline_width: self.style.outline_width,
        };
        let mut top = 0.0;

        for (row_index, row) in pending.iter().enumerate() {
            let height = match self.style.line_height {
                LineHeight::Font(scale) => (metrics.ascender - metrics.descender + metrics.line_gap) * scale * row.size,
                LineHeight::Fixed(height) => height,
            };
            let baseline = top + metrics.ascender * row.size;
            let x = match self.style.align {
                TextAlign::Left => 0.0,
                TextAlign::Center => (width - row.width) / 2.0,
                TextAlign::Right => width - row.width,
            };

            layout.rows.push(TextRow {
                top,
                baseline,
                bottom: baseline - metrics.descender * row.size,
                height,
                x,
                width: row.width,
            });

            let mut pen = x;
            let mut open: [Option<(LineRun, f32)>; 3] = [None; 3];

            for (cluster_index, cluster) in row.clusters.iter().enumerate() {
                let state = self.flat.state_at(cluster.range.start);

                // 線は行末の空白の下には引かない
                if cluster_index < row.visible {
                    let desired = [state.underline, state.strikethrough, state.overline].map(|decoration| {
                        decoration.map(|decoration| LineRun {
                            decoration,
                            color: state.color,
                            ghost: state.ghost,
                            size: cluster.size,
                        })
                    });

                    for kind in 0..LINE_KINDS.len() {
                        if open[kind].map(|(run, _)| run) != desired[kind] {
                            self.close_line(&mut layout, kind, open[kind].take(), pen, baseline, row_index);
                            open[kind] = desired[kind].map(|run| (run, pen));
                        }
                    }
                }

                let mut glyph_pen = pen;
                for glyph in &cluster.glyphs {
                    layout.glyphs.push(PlacedGlyph {
                        font: glyph.face.font,
                        glyph_id: glyph.glyph_id,
                        position: Vector2f::new(glyph_pen + glyph.x_offset, baseline - glyph.y_offset),
                        size: cluster.size,
                        advance: glyph.advance,
                        cluster: cluster.range.start,
                        row: row_index,
                        style: GlyphStyle {
                            color: state.color,
                            ghost: state.ghost,
                            synthetic_bold: glyph.face.synthetic_bold,
                            synthetic_italic: if glyph.face.synthetic_italic { state.italic } else { 0.0 },
                            obfuscated: state.obfuscated,
                            hide_box: state.hidden,
                            shadow: state.shadow,
                            outline: state.outline,
                        },
                    });
                    glyph_pen += glyph.advance;
                }

                pen += cluster.width();

                if cluster_index + 1 == row.visible {
                    for (kind, run) in open.iter_mut().enumerate() {
                        self.close_line(&mut layout, kind, run.take(), pen, baseline, row_index);
                    }
                }
            }

            top += height + row.space_y;
        }

        let last = layout.rows.last().expect("行は常に 1 つ以上ある");
        layout.size = Vector2f::new(width, last.bottom);

        layout
    }

    fn close_line(&self, layout: &mut TextLayout, kind: usize, open: Option<(LineRun, f32)>, end: f32, baseline: f32, row: usize) {
        let Some((run, start)) = open else {
            return;
        };

        let metrics = &self.metrics;
        let kind = LINE_KINDS[kind];
        // フォントの座標は y が上向き。TextLayout は下向き
        let position = match kind {
            LineKind::Underline => metrics.underline_position,
            LineKind::Strikethrough => metrics.strikeout_position,
            LineKind::Overline => metrics.ascender,
        };

        layout.lines.push(PlacedLine {
            kind,
            start: Vector2f::new(start, baseline - position * run.size),
            length: end - start,
            thickness: run.decoration.line_width * run.size,
            decoration: run.decoration,
            color: run.color,
            ghost: run.ghost,
            row,
        });
    }
}

// テストで区間の分け方を確かめる
#[cfg(test)]
pub(crate) fn segments_for_tests(resources: &ResourceManager, style: &TextStyle, formatted: &Formatted) -> Vec<Range<usize>> {
    validate_style(resources, style);

    let layouter = Layouter {
        resources,
        style,
        flat: FlatText::new(formatted),
        metrics: resources.font(style.families[0].regular).metrics,
    };

    layouter.paragraphs().into_iter().flat_map(|paragraph| layouter.segments(paragraph)).collect()
}

// グリフの並び（クラスターの順）から、クラスターごとのバイト範囲を求める
fn cluster_ranges(glyphs: &[(usize, ShapedGlyph)], end: usize) -> Vec<Range<usize>> {
    let mut starts: Vec<usize> = glyphs.iter().map(|&(cluster, _)| cluster).collect();
    starts.dedup();

    starts.iter().enumerate().map(|(index, &start)| start..starts.get(index + 1).copied().unwrap_or(end)).collect()
}
