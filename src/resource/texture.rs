// 登録した画像（§11 画像の形式）。RGBA 各 8 ビット、sRGB の値

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextureData {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl TextureData {
    // 大きさとバイト列の長さが合わなければ panic（誤用）
    pub(crate) fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        assert!(width > 0 && height > 0, "大きさ 0 の画像は登録できません");
        assert_eq!(
            pixels.len() as u64,
            width as u64 * height as u64 * 4,
            "画像の大きさとバイト列の長さが合いません（{width}×{height} の RGBA には {} バイト必要）",
            width as u64 * height as u64 * 4,
        );

        Self { width, height, pixels }
    }

    #[cfg(feature = "image")]
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, image::ImageError> {
        let image = image::load_from_memory(bytes)?.into_rgba8();
        let (width, height) = image.dimensions();

        Ok(Self::from_rgba(width, height, image.into_raw()))
    }
}
