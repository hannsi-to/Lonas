// サーフェス（ウィンドウ）ごとの状態（§8・§9・§11 複数ウィンドウ）

use crate::config::{AntiAliasing, PresentMode};
use crate::error::LonasError;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub(crate) u32);

pub(crate) struct SurfaceState {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    supported_present_modes: Vec<wgpu::PresentMode>,
    // resize で通知された最新の大きさ。反映は次の begin_frame（§11 深度バッファのリサイズ時）
    requested_size: (u32, u32),
    pending_present_mode: Option<wgpu::PresentMode>,
    pub anti_aliasing: AntiAliasing,
    pending_anti_aliasing: Option<AntiAliasing>,
    // TAA の履歴を捨てるか（§11 カメラのカット）。TAA は段階 8 で使う
    pub history_invalidated: bool,
    needs_configure: bool,
}

impl SurfaceState {
    pub(crate) fn new(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        width: u32,
        height: u32,
        present_mode: PresentMode,
        anti_aliasing: AntiAliasing,
    ) -> Result<Self, LonasError> {
        let capabilities = surface.get_capabilities(adapter);
        // 常に sRGB のフォーマットにする（§11 サーフェスのフォーマット）
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .ok_or(LonasError::UnsupportedSurfaceFormat)?;
        let present_mode = resolve_present_mode(present_mode, &capabilities.present_modes)?;
        let mut config = surface
            .get_default_config(adapter, width.max(1), height.max(1))
            .ok_or(LonasError::UnsupportedSurfaceFormat)?;

        config.format = format;
        config.present_mode = present_mode;

        Ok(Self {
            surface,
            config,
            supported_present_modes: capabilities.present_modes,
            requested_size: (width, height),
            pending_present_mode: None,
            anti_aliasing,
            pending_anti_aliasing: None,
            history_invalidated: true,
            needs_configure: true,
        })
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.requested_size
    }

    // 幅か高さが 0（最小化など）なら描かない（§9 リサイズとフレームスキップ）
    pub(crate) fn is_drawable(&self) -> bool {
        self.requested_size.0 > 0 && self.requested_size.1 > 0
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.requested_size = (width, height);
    }

    pub(crate) fn set_present_mode(&mut self, mode: PresentMode) -> Result<(), LonasError> {
        self.pending_present_mode = Some(resolve_present_mode(mode, &self.supported_present_modes)?);

        Ok(())
    }

    pub(crate) fn set_anti_aliasing(&mut self, anti_aliasing: AntiAliasing) {
        self.pending_anti_aliasing = Some(anti_aliasing);
    }

    // 描画先の作り直しが必要になったとき（Suboptimal / Outdated）
    pub(crate) fn mark_outdated(&mut self) {
        self.needs_configure = true;
    }

    // begin_frame で、溜まった変更を反映する。1 フレームに何度 resize されても作り直しは 1 回
    pub(crate) fn apply_pending(&mut self, device: &wgpu::Device) {
        if let Some(mode) = self.pending_present_mode.take() {
            self.config.present_mode = mode;
            self.needs_configure = true;
        }

        if let Some(anti_aliasing) = self.pending_anti_aliasing.take() {
            // 切り替えた直後は履歴がないため、無効化した状態から始める（§11 実行中の切り替え）
            self.history_invalidated |= anti_aliasing != self.anti_aliasing;
            self.anti_aliasing = anti_aliasing;
        }

        let (width, height) = self.requested_size;

        if (width, height) != (self.config.width, self.config.height) {
            self.needs_configure = true;
            // 解像度の変更では TAA の履歴を無効化する（§11 TAA の処理の流れ）
            self.history_invalidated = true;
        }

        if self.needs_configure && self.is_drawable() {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(device, &self.config);
            self.needs_configure = false;
        }
    }
}

// Lonas の PresentMode を wgpu のものにする。Auto* は wgpu が使えるものを選ぶため必ず成功する（§8）
fn resolve_present_mode(mode: PresentMode, supported: &[wgpu::PresentMode]) -> Result<wgpu::PresentMode, LonasError> {
    let resolved = match mode {
        PresentMode::AutoVsync => return Ok(wgpu::PresentMode::AutoVsync),
        PresentMode::AutoNoVsync => return Ok(wgpu::PresentMode::AutoNoVsync),
        PresentMode::Fifo => wgpu::PresentMode::Fifo,
        PresentMode::FifoRelaxed => wgpu::PresentMode::FifoRelaxed,
        PresentMode::Mailbox => wgpu::PresentMode::Mailbox,
        PresentMode::Immediate => wgpu::PresentMode::Immediate,
    };

    // Fifo はすべての環境で使える
    if resolved == wgpu::PresentMode::Fifo || supported.contains(&resolved) {
        Ok(resolved)
    } else {
        Err(LonasError::UnsupportedPresentMode(mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_modes_always_resolve() {
        assert_eq!(resolve_present_mode(PresentMode::AutoVsync, &[]).unwrap(), wgpu::PresentMode::AutoVsync);
        assert_eq!(resolve_present_mode(PresentMode::AutoNoVsync, &[]).unwrap(), wgpu::PresentMode::AutoNoVsync);
        assert_eq!(resolve_present_mode(PresentMode::Fifo, &[]).unwrap(), wgpu::PresentMode::Fifo);
    }

    #[test]
    fn unsupported_manual_mode_is_error() {
        let supported = [wgpu::PresentMode::Fifo, wgpu::PresentMode::Mailbox];

        assert_eq!(resolve_present_mode(PresentMode::Mailbox, &supported).unwrap(), wgpu::PresentMode::Mailbox);
        assert!(matches!(
            resolve_present_mode(PresentMode::Immediate, &supported),
            Err(LonasError::UnsupportedPresentMode(PresentMode::Immediate)),
        ));
    }
}
