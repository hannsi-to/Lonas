// 文字列の中に埋める書式（§13 書式：TextFormat）。gueiz の format.rs を元にし、色を Color にした
//
// 書き方：目印（MARKER、`§`）に続けて `[...]` に書式を書く
//   §[red]赤い字§[/]ふつうの字
//   §[bold]§[size 48]大きな太字
//   §[underline 0.05 round 2]二重下線
//   §[color #ff8800]16 進でも指定できる
//   §§        ← 目印そのものを出したいとき
// 書式は積み上がる。`§[/bold]` のように `/` を付ければその書式だけを、`§[/]` なら全部を元に戻す
//
// 書式は Formatted::parse を通したときだけ解釈する。Formatted::plain は解釈しない。
// 外から来た文字列（利用者の入力など）に書式を差し込まれないようにするため

use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::color::Color;
use crate::object::JointType;

/// 書式の始まりを表す文字。
pub const MARKER: char = '§';

/// 傾きを指定しなかったときの斜体の量。
pub const DEFAULT_ITALIC: f32 = 0.21;

/// 距離を指定しなかったときの影の長さ。em に対する割合。
pub const DEFAULT_SHADOW_DISTANCE: f32 = 0.06;

/// 向きを指定しなかったときの影の角度。度。
pub const DEFAULT_SHADOW_ANGLE: f32 = 45.0;

/// 太さを指定しなかったときの線の太さ。em に対する割合。
pub const DEFAULT_LINE_WIDTH: f32 = 0.05;

// --- 色 ---

/// 名前の付いた色。
///
/// 値は sRGB として扱う（§11 色）。頂点の色と同じ。
#[derive(Clone, Copy)]
#[derive(Eq, PartialEq)]
#[derive(Hash)]
#[derive(Debug)]
pub enum NamedColor {
    Black,
    White,
    Red,
    Green,
    Blue,
    Yellow,
    Cyan,
    Magenta,
    Gray,
    LightGray,
    DarkGray,
    Orange,
    Pink,
    Purple,
    Brown,
    Lime,
    Navy,
    Teal,
    Olive,
    Maroon,
    Gold,
    Silver,
    /// 何も描かれない。書式を消さずに字だけ伏せたいときに。
    Transparent,
}

impl NamedColor {
    /// 名前から。大文字小文字は問わず、`_` は無視する。
    pub fn from_name(name: &str) -> Option<Self> {
        // `light_gray` と `lightgray` を同じに扱いたいので、`_` を落として比べる。
        let mut normalized = String::with_capacity(name.len());

        for character in name.chars() {
            if character != '_' && character != '-' {
                normalized.extend(character.to_lowercase());
            }
        }

        Some(match normalized.as_str() {
            "black" => Self::Black,
            "white" => Self::White,
            "red" => Self::Red,
            "green" => Self::Green,
            "blue" => Self::Blue,
            "yellow" => Self::Yellow,
            "cyan" | "aqua" => Self::Cyan,
            "magenta" | "fuchsia" => Self::Magenta,
            "gray" | "grey" => Self::Gray,
            "lightgray" | "lightgrey" => Self::LightGray,
            "darkgray" | "darkgrey" => Self::DarkGray,
            "orange" => Self::Orange,
            "pink" => Self::Pink,
            "purple" => Self::Purple,
            "brown" => Self::Brown,
            "lime" => Self::Lime,
            "navy" => Self::Navy,
            "teal" => Self::Teal,
            "olive" => Self::Olive,
            "maroon" => Self::Maroon,
            "gold" => Self::Gold,
            "silver" => Self::Silver,
            "transparent" | "none" => Self::Transparent,
            _ => return None,
        })
    }

    pub const fn color(self) -> Color {
        const fn hex(red: u8, green: u8, blue: u8) -> Color {
            Color::new(red as f32 / 255.0, green as f32 / 255.0, blue as f32 / 255.0, 1.0)
        }

        match self {
            Self::Black => hex(0x00, 0x00, 0x00),
            Self::White => hex(0xff, 0xff, 0xff),
            Self::Red => hex(0xff, 0x00, 0x00),
            Self::Green => hex(0x00, 0xff, 0x00),
            Self::Blue => hex(0x00, 0x00, 0xff),
            Self::Yellow => hex(0xff, 0xff, 0x00),
            Self::Cyan => hex(0x00, 0xff, 0xff),
            Self::Magenta => hex(0xff, 0x00, 0xff),
            Self::Gray => hex(0x80, 0x80, 0x80),
            Self::LightGray => hex(0xc0, 0xc0, 0xc0),
            Self::DarkGray => hex(0x40, 0x40, 0x40),
            Self::Orange => hex(0xff, 0xa5, 0x00),
            Self::Pink => hex(0xff, 0xc0, 0xcb),
            Self::Purple => hex(0x80, 0x00, 0x80),
            Self::Brown => hex(0x8b, 0x45, 0x13),
            Self::Lime => hex(0x32, 0xcd, 0x32),
            Self::Navy => hex(0x00, 0x00, 0x80),
            Self::Teal => hex(0x00, 0x80, 0x80),
            Self::Olive => hex(0x80, 0x80, 0x00),
            Self::Maroon => hex(0x80, 0x00, 0x00),
            Self::Gold => hex(0xff, 0xd7, 0x00),
            Self::Silver => hex(0xc0, 0xc0, 0xc0),
            Self::Transparent => Color::TRANSPARENT,
        }
    }
}

// --- 書式 ---

/// 線を 1 本引くときの指定。下線・打消し線・上線で共通。
#[derive(Clone, Copy)]
#[derive(PartialEq)]
#[derive(Debug)]
pub struct LineDecoration {
    /// 線の太さ。em に対する割合。
    pub line_width: f32,
    /// 端と角の処理。まっすぐな線なので、効くのは端だけ。
    pub joint_type: JointType,
    /// 何本引くか。2 なら二重線。
    pub lines: u32,
}

impl Default for LineDecoration {
    fn default() -> Self {
        Self {
            line_width: DEFAULT_LINE_WIDTH,
            joint_type: JointType::Miter,
            lines: 1,
        }
    }
}

impl LineDecoration {
    pub fn new(line_width: f32, joint_type: JointType, lines: u32) -> Self {
        Self {
            line_width,
            joint_type,
            lines,
        }
    }
}

/// 影の付け方。
#[derive(Clone, Copy)]
#[derive(PartialEq)]
#[derive(Debug)]
pub struct Shadow {
    /// どれだけずらすか。em に対する割合。
    pub distance: f32,
    /// どちらにずらすか。度。**右向きが 0 で、時計回り**。
    /// 45 なら右下に落ちる。
    pub angle: f32,
    /// 影の色。`None` なら字の色の rgb を 0.25 倍したもの（不透明度は字と同じ）。
    pub color: Option<Color>,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            distance: DEFAULT_SHADOW_DISTANCE,
            angle: DEFAULT_SHADOW_ANGLE,
            color: None,
        }
    }
}

impl Shadow {
    pub fn new(distance: f32, angle: f32) -> Self {
        Self { distance, angle, color: None }
    }

    /// 影の色を指定する。
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// ずらし幅。em 単位。y は**下向きが正**。
    pub fn offset(self) -> [f32; 2] {
        let radians = self.angle.to_radians();

        [self.distance * radians.cos(), self.distance * radians.sin()]
    }
}

/// どの書式を元に戻すか。
#[derive(Clone, Copy)]
#[derive(Eq, PartialEq)]
#[derive(Hash)]
#[derive(Debug)]
pub enum ResetTarget {
    /// 全部。
    All,
    Color,
    Obfuscated,
    Bold,
    FontSize,
    Strikethrough,
    Underline,
    Overline,
    Italic,
    Ghost,
    HideBox,
    Shadow,
    Outline,
    SpaceX,
    SpaceY,
}

impl ResetTarget {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "" | "all" | "reset" => Self::All,
            "color" | "colour" | "c" => Self::Color,
            "obfuscated" | "obf" | "k" => Self::Obfuscated,
            "bold" | "b" => Self::Bold,
            "size" | "fontsize" => Self::FontSize,
            "strike" | "strikethrough" | "s" => Self::Strikethrough,
            "underline" | "u" => Self::Underline,
            "overline" | "o" => Self::Overline,
            "italic" | "i" => Self::Italic,
            "ghost" => Self::Ghost,
            "hidebox" | "hide" => Self::HideBox,
            "shadow" => Self::Shadow,
            "outline" => Self::Outline,
            "spacex" | "sx" => Self::SpaceX,
            "spacey" | "sy" => Self::SpaceY,
            _ => return None,
        })
    }
}

/// 文字列の途中で切り替える書式。
///
/// 名前の付いた色は [`TextFormat::RED`] のような定数で置いてあります。
/// 中身は [`TextFormat::Color`] なので、状態がふたつに分かれません。
#[derive(Clone, Copy)]
#[derive(PartialEq)]
#[derive(Debug)]
pub enum TextFormat {
    /// これ以降の字の色を上書きする。
    Color(Color),
    /// 色の上書きをやめて、`TextWrite::color` に戻す。
    DefaultColor,
    /// これ以降の字を出鱈目な字に置き換える。送り幅は元のまま。
    Obfuscated,
    /// これ以降の字を太くする。
    Bold,
    /// これ以降の字の大きさ（1 em）。頂点と同じ単位。
    FontSize(f32),
    /// これ以降の字に打消し線。
    Strikethrough(LineDecoration),
    /// これ以降の字に下線。
    Underline(LineDecoration),
    /// これ以降の字に上線。
    Overline(LineDecoration),
    /// これ以降の字を傾ける。値は傾きの量。
    Italic(f32),
    /// これ以降の字を透かす。値は**透ける割合**で、1 なら消える。
    Ghost(f32),
    /// これ以降の字を、字の大きさに合った箱で隠す。
    HideBox,
    /// これ以降の字に影を落とす。
    Shadow(Shadow),
    /// これ以降の字に縁取りを付ける。
    Outline(Color),
    /// 改行する。
    NewLine,
    /// これ以降の字間。`TextStyle::letter_spacing` を置き換える。頂点と同じ単位。
    SpaceX(f32),
    /// これ以降の行間に足す量。頂点と同じ単位。その行の最後の字の時点の値を使う。
    SpaceY(f32),
    /// 積み上げた書式を退避して、素の状態に戻す。
    SkipPush,
    /// [`TextFormat::SkipPush`] で退避した書式に戻す。
    SkipPop,
    /// 書式を元に戻す。
    Reset(ResetTarget),
}

impl TextFormat {
    pub const BLACK: Self = Self::Color(NamedColor::Black.color());
    pub const WHITE: Self = Self::Color(NamedColor::White.color());
    pub const RED: Self = Self::Color(NamedColor::Red.color());
    pub const GREEN: Self = Self::Color(NamedColor::Green.color());
    pub const BLUE: Self = Self::Color(NamedColor::Blue.color());
    pub const YELLOW: Self = Self::Color(NamedColor::Yellow.color());
    pub const CYAN: Self = Self::Color(NamedColor::Cyan.color());
    pub const MAGENTA: Self = Self::Color(NamedColor::Magenta.color());
    pub const GRAY: Self = Self::Color(NamedColor::Gray.color());
    pub const LIGHT_GRAY: Self = Self::Color(NamedColor::LightGray.color());
    pub const DARK_GRAY: Self = Self::Color(NamedColor::DarkGray.color());
    pub const ORANGE: Self = Self::Color(NamedColor::Orange.color());
    pub const PINK: Self = Self::Color(NamedColor::Pink.color());
    pub const PURPLE: Self = Self::Color(NamedColor::Purple.color());
    pub const BROWN: Self = Self::Color(NamedColor::Brown.color());
    pub const LIME: Self = Self::Color(NamedColor::Lime.color());
    pub const NAVY: Self = Self::Color(NamedColor::Navy.color());
    pub const TEAL: Self = Self::Color(NamedColor::Teal.color());
    pub const OLIVE: Self = Self::Color(NamedColor::Olive.color());
    pub const MAROON: Self = Self::Color(NamedColor::Maroon.color());
    pub const GOLD: Self = Self::Color(NamedColor::Gold.color());
    pub const SILVER: Self = Self::Color(NamedColor::Silver.color());
    pub const TRANSPARENT: Self = Self::Color(NamedColor::Transparent.color());

    /// 全部を元に戻す。
    pub const RESET: Self = Self::Reset(ResetTarget::All);

    /// 名前の付いた色から。
    pub const fn named(color: NamedColor) -> Self {
        Self::Color(color.color())
    }

    /// 赤緑青と不透明度から。
    pub const fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self::Color(Color::new(red, green, blue, alpha))
    }

    /// 縁取りの色を、赤緑青と不透明度から。
    pub const fn outline_rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Self {
        Self::Outline(Color::new(red, green, blue, alpha))
    }

    /// 下線。太さ・端の処理・本数を並べて渡す。
    pub const fn underline(line_width: f32, joint_type: JointType, lines: u32) -> Self {
        Self::Underline(LineDecoration {
            line_width,
            joint_type,
            lines,
        })
    }

    /// 打消し線。
    pub const fn strikethrough(line_width: f32, joint_type: JointType, lines: u32) -> Self {
        Self::Strikethrough(LineDecoration {
            line_width,
            joint_type,
            lines,
        })
    }

    /// 上線。
    pub const fn overline(line_width: f32, joint_type: JointType, lines: u32) -> Self {
        Self::Overline(LineDecoration {
            line_width,
            joint_type,
            lines,
        })
    }

    /// 既定の傾きで斜体にする。
    pub const fn italic() -> Self {
        Self::Italic(DEFAULT_ITALIC)
    }

    /// 既定の落とし方で影を付ける。
    pub fn shadow() -> Self {
        Self::Shadow(Shadow::default())
    }
}

// --- 解釈した文字列 ---

/// 文字列を書式と字に切り分けたもの。
#[derive(Clone, Copy)]
#[derive(PartialEq)]
#[derive(Debug)]
pub enum Token<'a> {
    /// そのまま出す字。
    Text(&'a str),
    /// ここから切り替わる書式。
    Format(TextFormat),
}

/// 書式を解釈し終えた文字列。
///
/// 元の文字列を**借りている**ので、元のほうが長生きする必要があります。
/// 複製はしません。
#[derive(Clone)]
#[derive(PartialEq)]
#[derive(Debug, Default)]
pub struct Formatted<'a> {
    tokens: Vec<Token<'a>>,
}

impl<'a> Formatted<'a> {
    /// 空。[`Formatted::text`] と [`Formatted::format`] で組み立てる。
    pub fn new() -> Self {
        Self::default()
    }

    /// 書式を解釈せず、丸ごと字として扱う。
    ///
    /// 外から来た文字列を安全に流し込むときに。
    pub fn plain(text: &'a str) -> Self {
        Self {
            tokens: if text.is_empty() {
                Vec::new()
            } else {
                vec![Token::Text(text)]
            },
        }
    }

    /// [`MARKER`] で始まる書式コードを解釈する。
    pub fn parse(text: &'a str) -> Result<Self, TextFormatError> {
        Self::parse_with(MARKER, text)
    }

    /// 目印を替えて解釈する。`§` が打ちにくいときに。
    pub fn parse_with(marker: char, text: &'a str) -> Result<Self, TextFormatError> {
        let mut tokens = Vec::new();
        // まだ字として積んでいない範囲の先頭。
        let mut pending = 0_usize;
        let mut cursor = 0_usize;

        while let Some(found) = text[cursor..].find(marker) {
            let at = cursor + found;
            let after = at + marker.len_utf8();

            // 空の字は積まない。後ろの処理を無駄に回すだけ。
            if pending < at {
                tokens.push(Token::Text(&text[pending..at]));
            }

            match text[after..].chars().next() {
                // 目印を 2 つ続けたら、目印そのもの。
                Some(character) if character == marker => {
                    tokens.push(Token::Text(&text[after..after + marker.len_utf8()]));
                    pending = after + marker.len_utf8();
                    cursor = pending;
                }

                Some('[') => {
                    let body_start = after + 1;

                    let Some(offset) = text[body_start..].find(']') else {
                        return Err(TextFormatError {
                            position: at,
                            kind: TextFormatErrorKind::Unterminated,
                        });
                    };

                    let body = &text[body_start..body_start + offset];
                    tokens.push(Token::Format(parse_code(body, at)?));

                    pending = body_start + offset + 1;
                    cursor = pending;
                }

                _ => {
                    return Err(TextFormatError {
                        position: at,
                        kind: TextFormatErrorKind::Unterminated,
                    });
                }
            }
        }

        if pending < text.len() {
            tokens.push(Token::Text(&text[pending..]));
        }

        Ok(Self { tokens })
    }

    /// 字を足す。
    pub fn text(&mut self, text: &'a str) -> &mut Self {
        if !text.is_empty() {
            self.tokens.push(Token::Text(text));
        }

        self
    }

    /// 書式を足す。
    pub fn format(&mut self, format: TextFormat) -> &mut Self {
        self.tokens.push(Token::Format(format));
        self
    }

    pub fn tokens(&self) -> &[Token<'a>] {
        &self.tokens
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// 書式を取り除いた、字だけの並び。
    ///
    /// 幅を測る前の確認や、読み上げに渡すときに。
    /// [`TextFormat::NewLine`] は改行として残す。
    pub fn plain_text(&self) -> String {
        let mut plain = String::new();

        for token in &self.tokens {
            match token {
                Token::Text(text) => plain.push_str(text),
                Token::Format(TextFormat::NewLine) => plain.push('\n'),
                Token::Format(_) => {}
            }
        }

        plain
    }
}

// --- コードの解釈 ---

fn parse_code(body: &str, position: usize) -> Result<TextFormat, TextFormatError> {
    let trimmed = body.trim();

    if let Some(target) = trimmed.strip_prefix('/') {
        let name = target.trim().to_ascii_lowercase();

        return ResetTarget::from_name(&name).map(TextFormat::Reset).ok_or(
            TextFormatError {
                position,
                kind: TextFormatErrorKind::UnknownReset(name),
            },
        );
    }

    let mut words = trimmed.split_whitespace();

    let Some(name) = words.next() else {
        return Err(TextFormatError {
            position,
            kind: TextFormatErrorKind::Empty,
        });
    };

    let arguments: Vec<&str> = words.collect();
    let lowered = name.to_ascii_lowercase();

    let fail = |kind| TextFormatError { position, kind };

    match lowered.as_str() {
        "color" | "colour" | "c" => {
            Ok(TextFormat::Color(parse_color(&arguments).map_err(fail)?))
        }

        "defaultcolor" | "defaultcolour" | "default" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::DefaultColor)
        }

        "obfuscated" | "obf" | "k" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::Obfuscated)
        }

        "bold" | "b" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::Bold)
        }

        "size" | "fontsize" => {
            let size = number(&arguments, 0, &lowered).map_err(fail)?;

            if !size.is_finite() || size <= 0.0 {
                return Err(fail(TextFormatErrorKind::BadArgument {
                    format: lowered,
                    argument: String::from("size"),
                    reason: String::from("字の大きさは 0 より大きい必要がある"),
                }));
            }

            Ok(TextFormat::FontSize(size))
        }

        "strike" | "strikethrough" | "s" => {
            Ok(TextFormat::Strikethrough(parse_line(&arguments, &lowered).map_err(fail)?))
        }

        "underline" | "u" => {
            Ok(TextFormat::Underline(parse_line(&arguments, &lowered).map_err(fail)?))
        }

        "overline" | "o" => {
            Ok(TextFormat::Overline(parse_line(&arguments, &lowered).map_err(fail)?))
        }

        "italic" | "i" => {
            let value = optional_number(&arguments, 0, &lowered).map_err(fail)?;
            Ok(TextFormat::Italic(value.unwrap_or(DEFAULT_ITALIC)))
        }

        "ghost" => {
            let value = optional_number(&arguments, 0, &lowered).map_err(fail)?;
            Ok(TextFormat::Ghost(value.unwrap_or(0.5).clamp(0.0, 1.0)))
        }

        "hidebox" | "hide" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::HideBox)
        }

        "shadow" => Ok(TextFormat::Shadow(parse_shadow(&arguments, &lowered).map_err(fail)?)),

        "outline" => {
            Ok(TextFormat::Outline(parse_color(&arguments).map_err(fail)?))
        }

        "ln" | "newline" | "br" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::NewLine)
        }

        "spacex" | "sx" => {
            Ok(TextFormat::SpaceX(number(&arguments, 0, &lowered).map_err(fail)?))
        }

        "spacey" | "sy" => {
            Ok(TextFormat::SpaceY(number(&arguments, 0, &lowered).map_err(fail)?))
        }

        "skippush" | "skip" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::SkipPush)
        }

        "skippop" | "unskip" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::SkipPop)
        }

        "reset" => {
            expect_none(&arguments, &lowered).map_err(fail)?;
            Ok(TextFormat::RESET)
        }

        // 名前の付いた色は、引数の要らない書式として通す。
        _ => match NamedColor::from_name(&lowered) {
            Some(color) => {
                expect_none(&arguments, &lowered).map_err(fail)?;
                Ok(TextFormat::named(color))
            }

            None => Err(fail(TextFormatErrorKind::UnknownFormat(lowered))),
        },
    }
}

/// 色は `#rrggbb` / `#rrggbbaa` / `r g b` / `r g b a` / 色の名前。
fn parse_color(arguments: &[&str]) -> Result<Color, TextFormatErrorKind> {
    match arguments {
        [] => Err(TextFormatErrorKind::MissingArgument {
            format: String::from("color"),
            argument: String::from("色"),
        }),

        [single] => {
            if let Some(digits) = single.strip_prefix('#') {
                return parse_hex(digits);
            }

            NamedColor::from_name(single)
                .map(NamedColor::color)
                .ok_or_else(|| TextFormatErrorKind::UnknownColor(String::from(*single)))
        }

        [red, green, blue] => Ok(Color::new(
            parse_channel(red)?,
            parse_channel(green)?,
            parse_channel(blue)?,
            1.0,
        )),

        [red, green, blue, alpha] => Ok(Color::new(
            parse_channel(red)?,
            parse_channel(green)?,
            parse_channel(blue)?,
            parse_channel(alpha)?,
        )),

        _ => Err(TextFormatErrorKind::TooManyArguments {
            format: String::from("color"),
            expected: 4,
            found: arguments.len(),
        }),
    }
}

fn parse_channel(text: &str) -> Result<f32, TextFormatErrorKind> {
    text.parse::<f32>()
        .map(|value| value.clamp(0.0, 1.0))
        .map_err(|_| TextFormatErrorKind::BadArgument {
            format: String::from("color"),
            argument: String::from(text),
            reason: String::from("0 から 1 の小数で書く"),
        })
}

fn parse_hex(digits: &str) -> Result<Color, TextFormatErrorKind> {
    let bad = || TextFormatErrorKind::BadArgument {
        format: String::from("color"),
        argument: format!("#{}", digits),
        reason: String::from("#rgb / #rgba / #rrggbb / #rrggbbaa のどれかで書く"),
    };

    // 短い形は各桁を 2 倍に伸ばす。`#f80` は `#ff8800`。
    let expanded: String = match digits.len() {
        3 | 4 => digits.chars().flat_map(|digit| [digit, digit]).collect(),
        6 | 8 => String::from(digits),
        _ => return Err(bad()),
    };

    let mut channels = [0.0_f32, 0.0, 0.0, 1.0];

    for (index, pair) in expanded.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| bad())?;
        let value = u8::from_str_radix(text, 16).map_err(|_| bad())?;

        channels[index] = value as f32 / 255.0;
    }

    let [red, green, blue, alpha] = channels;

    Ok(Color::new(red, green, blue, alpha))
}

/// 影は `距離 角度 色`。数は前から距離・角度の順で、色は数の後ろに 1 つだけ（16 進か名前）。
/// `r g b` の数の並びは距離・角度と見分けられないため使えない。
fn parse_shadow(arguments: &[&str], format: &str) -> Result<Shadow, TextFormatErrorKind> {
    let numbers = arguments.iter().take(2).take_while(|argument| argument.parse::<f32>().is_ok()).count();
    let rest = &arguments[numbers..];

    if rest.len() > 1 {
        return Err(TextFormatErrorKind::TooManyArguments {
            format: String::from(format),
            expected: numbers + 1,
            found: arguments.len(),
        });
    }

    let numeric = &arguments[..numbers];
    let mut shadow = Shadow::new(
        optional_number(numeric, 0, format)?.unwrap_or(DEFAULT_SHADOW_DISTANCE),
        optional_number(numeric, 1, format)?.unwrap_or(DEFAULT_SHADOW_ANGLE),
    );

    if let Some(color) = rest.first() {
        shadow = shadow.color(parse_color(&[color])?);
    }

    Ok(shadow)
}

/// 線の指定。省略したぶんは既定のまま。
fn parse_line(
    arguments: &[&str],
    format: &str,
) -> Result<LineDecoration, TextFormatErrorKind> {
    if arguments.len() > 3 {
        return Err(TextFormatErrorKind::TooManyArguments {
            format: String::from(format),
            expected: 3,
            found: arguments.len(),
        });
    }

    let mut decoration = LineDecoration::default();

    if let Some(width) = optional_number(arguments, 0, format)? {
        if !width.is_finite() || width <= 0.0 {
            return Err(TextFormatErrorKind::BadArgument {
                format: String::from(format),
                argument: String::from("line_width"),
                reason: String::from("線の太さは 0 より大きい必要がある"),
            });
        }

        decoration.line_width = width;
    }

    if let Some(joint) = arguments.get(1) {
        decoration.joint_type = parse_joint(joint)?;
    }

    if let Some(count) = arguments.get(2) {
        decoration.lines = count.parse::<u32>().map_err(|_| {
            TextFormatErrorKind::BadArgument {
                format: String::from(format),
                argument: String::from(*count),
                reason: String::from("線の本数は 0 以上の整数で書く"),
            }
        })?;
    }

    Ok(decoration)
}

fn parse_joint(name: &str) -> Result<JointType, TextFormatErrorKind> {
    let normalized: String = name
        .chars()
        .filter(|character| *character != '_' && *character != '-')
        .flat_map(char::to_lowercase)
        .collect();

    Ok(match normalized.as_str() {
        "none" => JointType::None,
        "miter" => JointType::Miter,
        "bevel" => JointType::Bevel,
        "round" => JointType::Round,
        "roundstart" => JointType::RoundStart,
        "roundend" => JointType::RoundEnd,
        "roundstartend" | "roundboth" => JointType::RoundStartEnd,
        _ => return Err(TextFormatErrorKind::UnknownJoint(String::from(name))),
    })
}

fn number(
    arguments: &[&str],
    index: usize,
    format: &str,
) -> Result<f32, TextFormatErrorKind> {
    optional_number(arguments, index, format)?.ok_or_else(|| {
        TextFormatErrorKind::MissingArgument {
            format: String::from(format),
            argument: format!("{} 番目", index + 1),
        }
    })
}

fn optional_number(
    arguments: &[&str],
    index: usize,
    format: &str,
) -> Result<Option<f32>, TextFormatErrorKind> {
    let Some(text) = arguments.get(index) else {
        return Ok(None);
    };

    text.parse::<f32>()
        .map(Some)
        .map_err(|_| TextFormatErrorKind::BadArgument {
            format: String::from(format),
            argument: String::from(*text),
            reason: String::from("数で書く"),
        })
}

fn expect_none(arguments: &[&str], format: &str) -> Result<(), TextFormatErrorKind> {
    if arguments.is_empty() {
        return Ok(());
    }

    Err(TextFormatErrorKind::TooManyArguments {
        format: String::from(format),
        expected: 0,
        found: arguments.len(),
    })
}

// --- 誤り ---

/// 書式コードを読めなかった。
#[derive(Clone)]
#[derive(PartialEq)]
#[derive(Debug)]
pub struct TextFormatError {
    /// 元の文字列の何バイト目から始まるコードか。
    pub position: usize,
    pub kind: TextFormatErrorKind,
}

#[derive(Clone)]
#[derive(PartialEq)]
#[derive(Debug)]
pub enum TextFormatErrorKind {
    /// 目印のあとが `[...]` でも目印でもない。
    Unterminated,
    /// `§[]` のように中身が無い。
    Empty,
    UnknownFormat(String),
    UnknownColor(String),
    UnknownJoint(String),
    UnknownReset(String),
    MissingArgument {
        format: String,
        argument: String,
    },
    TooManyArguments {
        format: String,
        expected: usize,
        found: usize,
    },
    BadArgument {
        format: String,
        argument: String,
        reason: String,
    },
}

impl Display for TextFormatError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{} バイト目: ", self.position))?;

        match &self.kind {
            TextFormatErrorKind::Unterminated => f.write_fmt(format_args!(
                "目印のあとは `[...]` か、目印をもう 1 つ（`{MARKER}{MARKER}`）",
            )),

            TextFormatErrorKind::Empty => f.write_str("書式が空"),

            TextFormatErrorKind::UnknownFormat(name) => {
                f.write_fmt(format_args!("`{}` という書式は無い", name))
            }

            TextFormatErrorKind::UnknownColor(name) => {
                f.write_fmt(format_args!("`{}` という色は無い", name))
            }

            TextFormatErrorKind::UnknownJoint(name) => f.write_fmt(format_args!(
                "`{}` という角の処理は無い（none / miter / bevel / round / round_start / round_end / round_start_end）",
                name,
            )),

            TextFormatErrorKind::UnknownReset(name) => {
                f.write_fmt(format_args!("`{}` は元に戻せる書式ではない", name))
            }

            TextFormatErrorKind::MissingArgument { format, argument } => {
                f.write_fmt(format_args!("`{}` に {} が足りない", format, argument))
            }

            TextFormatErrorKind::TooManyArguments { format, expected, found } => f.write_fmt(
                format_args!("`{}` は引数 {} 個までだが {} 個ある", format, expected, found),
            ),

            TextFormatErrorKind::BadArgument { format, argument, reason } => f.write_fmt(
                format_args!("`{}` の `{}` が読めない: {}", format, argument, reason),
            ),
        }
    }
}

impl Error for TextFormatError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn formats(text: &str) -> Vec<TextFormat> {
        Formatted::parse(text)
            .expect("読める")
            .tokens()
            .iter()
            .filter_map(|token| match token {
                Token::Format(format) => Some(*format),
                Token::Text(_) => None,
            })
            .collect()
    }

    #[test]
    fn plain_text_has_no_formats() {
        let parsed = Formatted::parse("ただの字").expect("読める");

        assert_eq!(parsed.tokens(), &[Token::Text("ただの字")]);
    }

    /// 書式を読ませたくない文字列は、丸ごと字として扱える。
    /// 外から来た文に書式を差し込まれないための入口。
    #[test]
    fn plain_keeps_the_marker_as_text() {
        let parsed = Formatted::plain("§[red]これは字");

        assert_eq!(parsed.tokens(), &[Token::Text("§[red]これは字")]);
    }

    #[test]
    fn text_and_formats_alternate() {
        let parsed = Formatted::parse("あ§[bold]い§[/]う").expect("読める");

        assert_eq!(
            parsed.tokens(),
            &[
                Token::Text("あ"),
                Token::Format(TextFormat::Bold),
                Token::Text("い"),
                Token::Format(TextFormat::RESET),
                Token::Text("う"),
            ],
        );
    }

    /// 目印を 2 つ続けたら目印そのもの。これが無いと `§` を書けない。
    #[test]
    fn a_doubled_marker_is_the_marker_itself() {
        let parsed = Formatted::parse("値段は 100§§").expect("読める");

        assert_eq!(parsed.plain_text(), "値段は 100§");
    }

    #[test]
    fn a_different_marker_can_be_used() {
        let parsed = Formatted::parse_with('&', "&[red]赤&&").expect("読める");

        assert_eq!(parsed.tokens()[0], Token::Format(TextFormat::RED));
        assert_eq!(parsed.plain_text(), "赤&");
    }

    /// 名前の付いた色は引数なしの書式として通る。
    #[test]
    fn named_colours_parse() {
        assert_eq!(formats("§[red]"), vec![TextFormat::RED]);
        assert_eq!(formats("§[Cyan]"), vec![TextFormat::CYAN]);
        assert_eq!(formats("§[light_gray]"), vec![TextFormat::LIGHT_GRAY]);
        // 別名も同じ色を指す。
        assert_eq!(formats("§[aqua]"), formats("§[cyan]"));
        assert_eq!(formats("§[grey]"), formats("§[gray]"));
    }

    #[test]
    fn colours_can_be_written_three_ways() {
        let expected = TextFormat::Color(Color::new(1.0, 0.0, 0.0, 1.0));

        assert_eq!(formats("§[red]"), vec![expected]);
        assert_eq!(formats("§[color 1 0 0 1]"), vec![expected]);
        assert_eq!(formats("§[color #ff0000]"), vec![expected]);
    }

    /// 短い 16 進は各桁を 2 倍に伸ばす。`#f80` は `#ff8800`。
    #[test]
    fn short_hex_expands() {
        assert_eq!(formats("§[color #f80]"), formats("§[color #ff8800]"));
        assert_eq!(formats("§[color #f808]"), formats("§[color #ff880088]"));
    }

    #[test]
    fn hex_can_carry_alpha() {
        let TextFormat::Color(color) = formats("§[color #00000080]")[0] else {
            panic!("色のはず");
        };

        assert!((color.a - 128.0 / 255.0).abs() < 1e-6, "{:?}", color);
    }

    /// 3 つだけ書いたら不透明。毎回 1 を書かせるのは面倒。
    #[test]
    fn three_channels_mean_opaque() {
        assert_eq!(formats("§[color 1 0 0]"), vec![TextFormat::RED]);
    }

    #[test]
    fn the_size_is_taken_as_pixels() {
        assert_eq!(formats("§[size 48]"), vec![TextFormat::FontSize(48.0)]);
    }

    /// 線は全部省略できる。既定は 1 本・マイター。
    #[test]
    fn a_line_falls_back_to_defaults() {
        assert_eq!(
            formats("§[underline]"),
            vec![TextFormat::Underline(LineDecoration::default())],
        );
    }

    #[test]
    fn a_line_takes_width_joint_and_count() {
        assert_eq!(
            formats("§[underline 0.08 round_start_end 2]"),
            vec![TextFormat::Underline(LineDecoration::new(
                0.08,
                JointType::RoundStartEnd,
                2,
            ))],
        );
    }

    #[test]
    fn strike_and_overline_use_the_same_shape() {
        let decoration = LineDecoration::new(0.04, JointType::Bevel, 3);

        assert_eq!(
            formats("§[strike 0.04 bevel 3]"),
            vec![TextFormat::Strikethrough(decoration)],
        );
        assert_eq!(
            formats("§[overline 0.04 bevel 3]"),
            vec![TextFormat::Overline(decoration)],
        );
    }

    #[test]
    fn italic_and_ghost_have_defaults() {
        assert_eq!(formats("§[italic]"), vec![TextFormat::Italic(DEFAULT_ITALIC)]);
        assert_eq!(formats("§[italic 0.5]"), vec![TextFormat::Italic(0.5)]);
        assert_eq!(formats("§[ghost]"), vec![TextFormat::Ghost(0.5)]);
        assert_eq!(formats("§[ghost 0.25]"), vec![TextFormat::Ghost(0.25)]);
    }

    /// 透ける割合は 0 から 1 に収める。外れた値は効き方が想像できない。
    #[test]
    fn ghost_is_clamped() {
        assert_eq!(formats("§[ghost 3]"), vec![TextFormat::Ghost(1.0)]);
        assert_eq!(formats("§[ghost -1]"), vec![TextFormat::Ghost(0.0)]);
    }

    #[test]
    fn shadow_takes_distance_then_angle() {
        assert_eq!(
            formats("§[shadow 0.1 90]"),
            vec![TextFormat::Shadow(Shadow::new(0.1, 90.0))],
        );
        assert_eq!(formats("§[shadow]"), vec![TextFormat::shadow()]);
    }

    /// 影の色は数の後ろに 1 つだけ。数を省いて色だけでも書ける。
    #[test]
    fn shadow_takes_a_color_after_the_numbers() {
        let red = NamedColor::Red.color();

        assert_eq!(formats("§[shadow 0.1 90 red]"), vec![TextFormat::Shadow(Shadow::new(0.1, 90.0).color(red))]);
        assert_eq!(formats("§[shadow 0.1 #ff0000]"), vec![TextFormat::Shadow(Shadow::new(0.1, DEFAULT_SHADOW_ANGLE).color(red))]);
        assert_eq!(formats("§[shadow #f00]"), vec![TextFormat::Shadow(Shadow::default().color(red))]);

        assert!(matches!(
            Formatted::parse("§[shadow 0.1 red blue]").expect_err("色は 1 つだけ").kind,
            TextFormatErrorKind::TooManyArguments { .. },
        ));
        assert!(matches!(
            Formatted::parse("§[shadow 0.1 90 そら]").expect_err("そんな色は無い").kind,
            TextFormatErrorKind::UnknownColor(_),
        ));
        // 3 つ目の数は色として読めない
        assert!(Formatted::parse("§[shadow 0.1 90 0.5]").is_err());
    }

    /// 角度は右向きが 0 の時計回り。45 度なら右下。
    #[test]
    fn a_shadow_angle_points_where_it_says() {
        let [x, y] = Shadow::new(1.0, 0.0).offset();
        assert!((x - 1.0).abs() < 1e-6 && y.abs() < 1e-6, "{x} {y}");

        let [x, y] = Shadow::new(1.0, 90.0).offset();
        assert!(x.abs() < 1e-6 && (y - 1.0).abs() < 1e-6, "{x} {y}");

        let [x, y] = Shadow::new(1.0, 45.0).offset();
        assert!(x > 0.0 && y > 0.0, "右下に落ちるはず: {x} {y}");
    }

    #[test]
    fn the_remaining_formats_parse() {
        assert_eq!(formats("§[obfuscated]"), vec![TextFormat::Obfuscated]);
        assert_eq!(formats("§[hidebox]"), vec![TextFormat::HideBox]);
        assert_eq!(formats("§[ln]"), vec![TextFormat::NewLine]);
        assert_eq!(formats("§[spacex 3]"), vec![TextFormat::SpaceX(3.0)]);
        assert_eq!(formats("§[spacey -2]"), vec![TextFormat::SpaceY(-2.0)]);
        assert_eq!(formats("§[skippush]"), vec![TextFormat::SkipPush]);
        assert_eq!(formats("§[skippop]"), vec![TextFormat::SkipPop]);
        assert_eq!(formats("§[defaultcolor]"), vec![TextFormat::DefaultColor]);
        assert_eq!(
            formats("§[outline 0 0 0 1]"),
            vec![TextFormat::Outline(Color::new(0.0, 0.0, 0.0, 1.0))],
        );
    }

    /// `/` は「その書式だけ戻す」。`§[/]` は全部。
    #[test]
    fn slash_resets_one_format_or_all() {
        assert_eq!(formats("§[/]"), vec![TextFormat::RESET]);
        assert_eq!(formats("§[/all]"), vec![TextFormat::RESET]);
        assert_eq!(
            formats("§[/bold]"),
            vec![TextFormat::Reset(ResetTarget::Bold)],
        );
        assert_eq!(
            formats("§[/underline]"),
            vec![TextFormat::Reset(ResetTarget::Underline)],
        );
    }

    #[test]
    fn short_names_work_too() {
        assert_eq!(formats("§[b]"), vec![TextFormat::Bold]);
        assert_eq!(formats("§[i]"), vec![TextFormat::Italic(DEFAULT_ITALIC)]);
        assert_eq!(formats("§[u]"), formats("§[underline]"));
        assert_eq!(formats("§[/b]"), vec![TextFormat::Reset(ResetTarget::Bold)]);
    }

    #[test]
    fn whitespace_inside_a_code_is_ignored() {
        assert_eq!(formats("§[  bold  ]"), vec![TextFormat::Bold]);
        assert_eq!(formats("§[ color  1 0 0 ]"), vec![TextFormat::RED]);
    }

    // --- 誤り ---

    #[test]
    fn an_unclosed_code_is_an_error() {
        let error = Formatted::parse("§[red").expect_err("閉じていない");

        assert_eq!(error.kind, TextFormatErrorKind::Unterminated);
    }

    #[test]
    fn a_lone_marker_is_an_error() {
        // 黙って字として出すと、書き間違いに気づけない。
        let error = Formatted::parse("100§ 円").expect_err("目印だけ");

        assert_eq!(error.kind, TextFormatErrorKind::Unterminated);
        assert_eq!(error.position, "100".len());
    }

    #[test]
    fn an_unknown_format_is_an_error() {
        let error = Formatted::parse("§[sparkle]").expect_err("そんな書式は無い");

        assert_eq!(
            error.kind,
            TextFormatErrorKind::UnknownFormat(String::from("sparkle")),
        );
    }

    #[test]
    fn arguments_are_checked() {
        assert!(matches!(
            Formatted::parse("§[bold 3]").expect_err("引数は取らない").kind,
            TextFormatErrorKind::TooManyArguments { .. },
        ));

        assert!(matches!(
            Formatted::parse("§[size]").expect_err("大きさが要る").kind,
            TextFormatErrorKind::MissingArgument { .. },
        ));

        assert!(matches!(
            Formatted::parse("§[size ふとい]").expect_err("数ではない").kind,
            TextFormatErrorKind::BadArgument { .. },
        ));

        assert!(matches!(
            Formatted::parse("§[size 0]").expect_err("0 は大きさにならない").kind,
            TextFormatErrorKind::BadArgument { .. },
        ));
    }

    #[test]
    fn an_unknown_joint_is_an_error() {
        assert!(matches!(
            Formatted::parse("§[underline 0.05 ぐるぐる]").expect_err("角の名前が違う").kind,
            TextFormatErrorKind::UnknownJoint(_),
        ));
    }

    /// 誤りの場所は元の文字列のバイト位置。多バイト文字があっても合うこと。
    #[test]
    fn the_error_points_at_the_code() {
        let text = "日本語§[nope]";
        let error = Formatted::parse(text).expect_err("そんな書式は無い");

        assert_eq!(error.position, "日本語".len());
        assert!(text[error.position..].starts_with(MARKER));
    }

    // --- 組み立て ---

    #[test]
    fn a_formatted_string_can_be_built_by_hand() {
        let mut built = Formatted::new();
        built
            .format(TextFormat::RED)
            .text("赤")
            .format(TextFormat::RESET)
            .text("素");

        assert_eq!(built, Formatted::parse("§[red]赤§[/]素").expect("読める"));
    }

    #[test]
    fn empty_pieces_are_not_kept() {
        let mut built = Formatted::new();
        built.text("");

        assert!(built.is_empty());
        assert!(Formatted::plain("").is_empty());
        // 書式が続いても空の字は挟まらない。
        assert_eq!(Formatted::parse("§[b]§[i]").expect("読める").tokens().len(), 2);
    }

    #[test]
    fn plain_text_drops_formats_but_keeps_newlines() {
        let parsed = Formatted::parse("あ§[red]い§[ln]う").expect("読める");

        assert_eq!(parsed.plain_text(), "あい\nう");
    }

    /// 定数と `Color` は同じもの。状態がふたつに分かれていないこと。
    #[test]
    fn the_named_constants_are_plain_colours() {
        assert_eq!(TextFormat::RED, TextFormat::Color(Color::new(1.0, 0.0, 0.0, 1.0)));
        assert_eq!(TextFormat::named(NamedColor::Blue), TextFormat::BLUE);
    }

    /// 手で書く近道と、コードで書いたものが一致すること。
    #[test]
    fn the_shorthands_match_what_the_codes_produce() {
        assert_eq!(TextFormat::rgba(1.0, 0.0, 0.0, 1.0), formats("§[red]")[0]);
        assert_eq!(
            TextFormat::outline_rgba(0.0, 0.0, 0.0, 1.0),
            formats("§[outline 0 0 0 1]")[0],
        );
        assert_eq!(
            TextFormat::underline(0.08, JointType::Round, 2),
            formats("§[underline 0.08 round 2]")[0],
        );
        assert_eq!(
            TextFormat::strikethrough(0.04, JointType::Bevel, 1),
            formats("§[strike 0.04 bevel 1]")[0],
        );
        assert_eq!(
            TextFormat::overline(0.04, JointType::Miter, 3),
            formats("§[overline 0.04 miter 3]")[0],
        );
    }
}
