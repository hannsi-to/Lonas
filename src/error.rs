// 環境に起因するエラーと、登録時の入力のエラー（§11 エラーの方針）。誤用は panic

use std::fmt;

use crate::config::PresentMode;

#[non_exhaustive]
#[derive(Debug)]
pub enum LonasError {
    // ---- 環境 ----
    // 指定したバックエンドのアダプターがない（§8）
    AdapterNotFound(wgpu::RequestAdapterError),
    // 必須の機能がない（§8・§11 環境の判定）
    UnsupportedAdapter { missing: &'static str },
    RequestDevice(wgpu::RequestDeviceError),
    CreateSurface(wgpu::CreateSurfaceError),
    DeviceLost { reason: String },
    // sRGB のフォーマットに対応していない（§11 サーフェスのフォーマット）
    UnsupportedSurfaceFormat,
    UnsupportedSampleCount { requested: u32, supported: Vec<u32> },
    UnsupportedPresentMode(PresentMode),
    // アトラスの大きさ・レイヤー数・バインドグループの数などがアダプターの上限を超えている
    LimitExceeded { limit: &'static str, requested: u64, max: u64 },

    // ---- 登録時の入力（§11 ResourceManager）----
    // naga の検査結果。message はエラーの位置を含む
    InvalidShader { message: String },
    #[cfg(feature = "image")]
    ImageDecode(image::ImageError),
    // フォントとして読めない（§13 フォントの登録）
    #[cfg(feature = "text")]
    InvalidFont,
}

impl fmt::Display for LonasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LonasError::AdapterNotFound(error) => write!(f, "GPU のアダプターが見つかりません: {error}"),
            LonasError::UnsupportedAdapter { missing } => write!(f, "GPU のアダプターに必須の機能がありません: {missing}"),
            LonasError::RequestDevice(error) => write!(f, "GPU のデバイスを作れません: {error}"),
            LonasError::CreateSurface(error) => write!(f, "サーフェスを作れません: {error}"),
            LonasError::DeviceLost { reason } => write!(f, "GPU のデバイスが失われました: {reason}"),
            LonasError::UnsupportedSurfaceFormat => write!(f, "サーフェスが sRGB のフォーマットに対応していません"),
            LonasError::UnsupportedSampleCount { requested, supported } => {
                write!(f, "MSAA のサンプル数 {requested} に対応していません（対応しているのは {supported:?}）")
            }
            LonasError::UnsupportedPresentMode(mode) => write!(f, "サーフェスが Present mode {mode:?} に対応していません"),
            LonasError::LimitExceeded { limit, requested, max } => {
                write!(f, "{limit} がアダプターの上限を超えています（{requested} > {max}）")
            }
            LonasError::InvalidShader { message } => write!(f, "WGSL のシェーダーが正しくありません:\n{message}"),
            #[cfg(feature = "image")]
            LonasError::ImageDecode(error) => write!(f, "画像を読み込めません: {error}"),
            #[cfg(feature = "text")]
            LonasError::InvalidFont => write!(f, "フォントを読み込めません"),
        }
    }
}

impl std::error::Error for LonasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LonasError::AdapterNotFound(error) => Some(error),
            LonasError::RequestDevice(error) => Some(error),
            LonasError::CreateSurface(error) => Some(error),
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
