// テキスト（§13）。text feature
// 段階 9 では、フォントの登録と整形（文字列から TextLayout を作る）までを持つ。GPU は使わない
#![allow(dead_code)] // TextConfig の値とフォントの輪郭は段階 10 以降で使う

mod font;
pub mod format;
mod layout;

pub use font::FontFamily;
pub(crate) use font::FontData;
pub use format::{
    Formatted, LineDecoration, MARKER, NamedColor, ResetTarget, Shadow, TextFormat, TextFormatError,
    TextFormatErrorKind, Token,
};
pub use layout::{GlyphStyle, LineHeight, LineKind, PlacedGlyph, PlacedLine, TextAlign, TextLayout, TextRow, TextStyle};

// グリフを作る側の設定（§13 CPU 側の設定）。ResourceConfig::text で受け取る
#[derive(Clone, Debug, PartialEq)]
pub struct TextConfig {
    // Sdf / Msdf を作るときの 1 em の大きさ（画素）
    pub sdf_em_size: u32,
    // Sdf / Msdf の距離の幅（画素）。太字・縁取りで太らせられる量の上限になる
    pub sdf_range: f32,
    // write_text が 1 フレームに作るグリフの上限。None なら上限なし
    pub max_new_glyphs_per_frame: Option<u32>,
    // ベクターのキャッシュの上限（バイト）
    pub vector_cache_max_bytes: usize,
}

impl TextConfig {
    // ResourceManager::new で検査する。不正なら panic
    pub(crate) fn validate(&self) {
        assert!(self.sdf_em_size > 0, "TextConfig::sdf_em_size は 1 以上にしてください");
        assert!(
            self.sdf_range.is_finite() && self.sdf_range > 0.0,
            "TextConfig::sdf_range は 0 より大きい有限の値にしてください（{}）",
            self.sdf_range,
        );
        assert_ne!(
            self.max_new_glyphs_per_frame,
            Some(0),
            "TextConfig::max_new_glyphs_per_frame は Some(1) 以上か None にしてください",
        );
    }
}

#[cfg(test)]
mod tests;
