// 環境に起因するエラーと、登録時の入力のエラー（§11 エラーの方針）。誤用は panic
// 環境のエラー（アダプター・サーフェスなど）は段階 4 で足す

use std::fmt;

#[non_exhaustive]
#[derive(Debug)]
pub enum LonasError {
    // 登録時の入力（§11 ResourceManager）
    // naga の検査結果。message はエラーの位置を含む
    InvalidShader { message: String },
    #[cfg(feature = "image")]
    ImageDecode(image::ImageError),
}

impl fmt::Display for LonasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LonasError::InvalidShader { message } => write!(f, "WGSL のシェーダーが正しくありません:\n{message}"),
            #[cfg(feature = "image")]
            LonasError::ImageDecode(error) => write!(f, "画像を読み込めません: {error}"),
        }
    }
}

impl std::error::Error for LonasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(feature = "image")]
            LonasError::ImageDecode(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(feature = "image")]
impl From<image::ImageError> for LonasError {
    fn from(error: image::ImageError) -> Self {
        LonasError::ImageDecode(error)
    }
}
