// 登録したフォントと、太字・斜体の書体の組（§13 フォントの登録・フォントの組とフォールバック）

use std::sync::Arc;

use harfrust::Font;
use skrifa::raw::types::F48Dot16;

use crate::resource::FontId;

// 行送り・線の位置。em を 1 とした単位で、y は上向き（フォントの座標のまま）
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct FontMetrics {
    pub ascender: f32,
    // ベースラインより下は負
    pub descender: f32,
    pub line_gap: f32,
    pub underline_position: f32,
    pub strikeout_position: f32,
}

// TextFormat::Obfuscated の候補の字（gueiz と同じ）
const OBFUSCATION_POOL: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789#$%&@?!";

pub(crate) struct FontData {
    // skrifa で輪郭を取り出すために、元のバイト列も持つ（HarfRust の Font と共有する）
    pub bytes: Arc<Vec<u8>>,
    pub font: Font,
    pub units_per_em: f32,
    pub metrics: FontMetrics,
    // OBFUSCATION_POOL のうち、このフォントにある字のグリフ
    pub obfuscation_glyphs: Vec<u32>,
}

impl FontData {
    // 読めない、または 1 em の大きさが 0 なら None。コレクションは先頭のフォントだけを使う
    pub(crate) fn new(bytes: Vec<u8>) -> Option<Self> {
        let bytes = Arc::new(bytes);
        let font = Font::new(Arc::clone(&bytes) as Arc<dyn AsRef<[u8]> + Send + Sync>, 0)?;
        let units_per_em = font.units_per_em();

        if units_per_em == 0 {
            return None;
        }

        let metrics = read_metrics(&font, units_per_em as f32);
        let charmap = font.charmap();
        let obfuscation_glyphs =
            OBFUSCATION_POOL.chars().filter_map(|character| charmap.map_unicode(character)).map(|glyph| glyph.to_u32()).collect();

        Some(Self { bytes, font, units_per_em: units_per_em as f32, metrics, obfuscation_glyphs })
    }

    // cmap にある字か。Default_Ignorable の文字は見ない（§13 フォントの組とフォールバック）
    pub(crate) fn has_char(&self, character: char) -> bool {
        is_default_ignorable(character) || self.font.charmap().map_unicode(character).is_some()
    }
}

fn read_metrics(font: &Font, units_per_em: f32) -> FontMetrics {
    let global = font.metrics();
    let style = font.style_metrics();
    let em = |value: F48Dot16| value.to_f64() as f32 / units_per_em;

    // USE_TYPO_METRICS なら typo、なければ hhea（0 なら typo）、それもなければ win（§13 フォントの登録）
    let typo = global.typo_line.filter(|_| global.use_typo_metrics);
    let hhea = global.hhea_line.filter(|line| line.ascender.to_f64() != 0.0 || line.descender.to_f64() != 0.0);
    let (ascender, descender, line_gap) = match typo.or(hhea).or(global.typo_line) {
        Some(line) => (em(line.ascender), em(line.descender), em(line.line_gap)),
        None => match global.win_line {
            Some(line) => (em(line.ascender), em(line.descender), 0.0),
            // 行送りの値がないフォント。1 em をそのまま使う
            None => (0.8, -0.2, 0.0),
        },
    };

    FontMetrics {
        ascender,
        descender,
        line_gap,
        // 値がなければ、下線は −0.1 em、打消し線は 0.3 em（§13 整形の結果）
        underline_position: style.underline.map_or(-0.1, |line| em(line.position)),
        strikeout_position: style.strike_through.map_or(0.3, |line| em(line.position)),
    }
}

// Unicode の Default_Ignorable_Code_Point（異体字セレクター・ZWJ など）
fn is_default_ignorable(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

// 太字・斜体の書体をまとめたもの。TextStyle::families に優先する順に並べる
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFamily {
    pub regular: FontId,
    pub bold: Option<FontId>,
    pub italic: Option<FontId>,
    pub bold_italic: Option<FontId>,
}

// 実際に使う書体と、合成で足すもの
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Face {
    pub font: FontId,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
}

impl FontFamily {
    pub fn new(regular: FontId) -> Self {
        Self { regular, bold: None, italic: None, bold_italic: None }
    }

    pub fn bold(mut self, font: FontId) -> Self {
        self.bold = Some(font);
        self
    }

    pub fn italic(mut self, font: FontId) -> Self {
        self.italic = Some(font);
        self
    }

    pub fn bold_italic(mut self, font: FontId) -> Self {
        self.bold_italic = Some(font);
        self
    }

    pub(crate) fn fonts(&self) -> impl Iterator<Item = FontId> {
        [Some(self.regular), self.bold, self.italic, self.bold_italic].into_iter().flatten()
    }

    // 太字・斜体の状態に合う書体を選ぶ。なければ近い書体に合成の印を付ける
    // bold_italic → bold に傾き → italic に太さ → regular に両方 の順
    pub(crate) fn select(&self, bold: bool, italic: bool) -> Face {
        let face = |font, synthetic_bold, synthetic_italic| Face { font, synthetic_bold, synthetic_italic };

        match (bold, italic) {
            (false, false) => face(self.regular, false, false),
            (true, false) => match self.bold {
                Some(font) => face(font, false, false),
                None => face(self.regular, true, false),
            },
            (false, true) => match self.italic {
                Some(font) => face(font, false, false),
                None => face(self.regular, false, true),
            },
            (true, true) => match (self.bold_italic, self.bold, self.italic) {
                (Some(font), _, _) => face(font, false, false),
                (None, Some(font), _) => face(font, false, true),
                (None, None, Some(font)) => face(font, true, false),
                (None, None, None) => face(self.regular, true, true),
            },
        }
    }
}
