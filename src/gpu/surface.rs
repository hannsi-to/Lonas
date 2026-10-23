// サーフェス（ウィンドウ）ごとの状態（§8・§9・§11 複数ウィンドウ・深度バッファ）

use crate::config::{AntiAliasing, PresentMode};
use crate::error::LonasError;
use crate::render::pipeline::DEPTH_FORMAT;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub(crate) u32);

enum Target {
    Window {
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        supported_present_modes: Vec<wgpu::PresentMode>,
        pending_present_mode: Option<wgpu::PresentMode>,
        needs_configure: bool,
    },
    // ウィンドウのない描画先。GPU を使うテストで、描いた画像を読み戻すのに使う
    #[cfg(test)]
    Offscreen { texture: wgpu::Texture, format: wgpu::TextureFormat },
}

// そのフレームの描画先
pub(crate) struct Acquired {
    pub view: wgpu::TextureView,
    pub surface_texture: Option<wgpu::SurfaceTexture>,
}

pub(crate) struct SurfaceState {
    target: Target,
    // resize で通知された最新の大きさ。反映は次の begin_frame（§11 深度バッファのリサイズ時）
    requested_size: (u32, u32),
    // 描画先の大きさ（反映済み）
    current_size: (u32, u32),
    // サーフェスごとに 1 つ。同じサーフェスの DrawManager3D 間で共有する（§11 深度バッファ）
    depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub anti_aliasing: AntiAliasing,
    pending_anti_aliasing: Option<AntiAliasing>,
    // TAA の履歴を捨てるか（§11 カメラのカット）。TAA は段階 8 で使う
    pub history_invalidated: bool,
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

        Ok(Self::with_target(
            Target::Window {
                surface,
                config,
                supported_present_modes: capabilities.present_modes,
                pending_present_mode: None,
                needs_configure: true,
            },
            width,
            height,
            anti_aliasing,
        ))
    }

    #[cfg(test)]
    pub(crate) fn new_offscreen(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut state = Self::with_target(
            Target::Offscreen { texture: create_offscreen(device, width, height, format), format },
            width,
            height,
            AntiAliasing::None,
        );

        state.current_size = (width, height);
        state
    }

    fn with_target(target: Target, width: u32, height: u32, anti_aliasing: AntiAliasing) -> Self {
        Self {
            target,
            requested_size: (width, height),
            current_size: (0, 0),
            depth: None,
            anti_aliasing,
            pending_anti_aliasing: None,
            history_invalidated: true,
        }
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        self.requested_size
    }

    pub(crate) fn format(&self) -> wgpu::TextureFormat {
        match &self.target {
            Target::Window { config, .. } => config.format,
            #[cfg(test)]
            Target::Offscreen { format, .. } => *format,
        }
    }

    // 幅か高さが 0（最小化など）なら描かない（§9 リサイズとフレームスキップ）
    pub(crate) fn is_drawable(&self) -> bool {
        self.requested_size.0 > 0 && self.requested_size.1 > 0
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.requested_size = (width, height);
    }

    pub(crate) fn set_present_mode(&mut self, mode: PresentMode) -> Result<(), LonasError> {
        match &mut self.target {
            Target::Window { supported_present_modes, pending_present_mode, .. } => {
                *pending_present_mode = Some(resolve_present_mode(mode, supported_present_modes)?);
            }
            #[cfg(test)]
            Target::Offscreen { .. } => {}
        }

        Ok(())
    }

    pub(crate) fn set_anti_aliasing(&mut self, anti_aliasing: AntiAliasing) {
        self.pending_anti_aliasing = Some(anti_aliasing);
    }

    // begin_frame で、溜まった変更を反映する。1 フレームに何度 resize されても作り直しは 1 回
    pub(crate) fn apply_pending(&mut self, device: &wgpu::Device) {
        if let Some(anti_aliasing) = self.pending_anti_aliasing.take() {
            // 切り替えた直後は履歴がないため、無効化した状態から始める（§11 実行中の切り替え）
            self.history_invalidated |= anti_aliasing != self.anti_aliasing;
            self.anti_aliasing = anti_aliasing;
        }

        let size = self.requested_size;
        let resized = size != self.current_size;

        if resized {
            // 解像度の変更では TAA の履歴を無効化する（§11 TAA の処理の流れ）
            self.history_invalidated = true;
        }

        match &mut self.target {
            Target::Window { surface, config, pending_present_mode, needs_configure, .. } => {
                if let Some(mode) = pending_present_mode.take() {
                    config.present_mode = mode;
                    *needs_configure = true;
                }

                if (*needs_configure || resized) && self.requested_size.0 > 0 && self.requested_size.1 > 0 {
                    (config.width, config.height) = size;
                    surface.configure(device, config);
                    *needs_configure = false;
                    self.current_size = size;
                }
            }
            #[cfg(test)]
            Target::Offscreen { texture, format } => {
                if resized && size.0 > 0 && size.1 > 0 {
                    *texture = create_offscreen(device, size.0, size.1, *format);
                    self.current_size = size;
                }
            }
        }
    }

    // 描画先を取得する。取得できない場合はそのサーフェスだけ飛ばす（§9 リサイズとフレームスキップ）
    pub(crate) fn acquire(&mut self) -> Option<Acquired> {
        if !self.is_drawable() {
            return None;
        }

        match &mut self.target {
            Target::Window { surface, needs_configure, .. } => {
                let texture = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture) => texture,
                    wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                        *needs_configure = true;
                        texture
                    }
                    wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return None,
                    wgpu::CurrentSurfaceTexture::Outdated => {
                        *needs_configure = true;
                        return None;
                    }
                    wgpu::CurrentSurfaceTexture::Lost => {
                        log::warn!("サーフェスが失われました。次のフレームで設定し直します");
                        *needs_configure = true;
                        return None;
                    }
                    wgpu::CurrentSurfaceTexture::Validation => {
                        log::warn!("描画先の取得で検証エラーが起きました。このフレームは描きません");
                        return None;
                    }
                };
                let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());

                Some(Acquired { view, surface_texture: Some(texture) })
            }
            #[cfg(test)]
            Target::Offscreen { texture, .. } => {
                Some(Acquired { view: texture.create_view(&wgpu::TextureViewDescriptor::default()), surface_texture: None })
            }
        }
    }

    // 描画先と同じ大きさの深度バッファ。大きさが変わっていれば作り直す（§11 深度バッファ）
    pub(crate) fn depth_view(&mut self, device: &wgpu::Device) -> wgpu::TextureView {
        let (width, height) = self.current_size;
        let outdated = self.depth.as_ref().is_none_or(|(texture, _)| (texture.width(), texture.height()) != (width, height));

        if outdated {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Lonas depth"),
                size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            self.depth = Some((texture, view));
        }

        self.depth.as_ref().unwrap().1.clone()
    }

    // テストで、描いた画像を RGBA の行の並びとして読み戻す
    #[cfg(test)]
    pub(crate) fn read_pixels(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<[u8; 4]> {
        let Target::Offscreen { texture, .. } = &self.target else { panic!("ウィンドウのサーフェスは読み戻せません") };
        let (width, height) = (texture.width(), texture.height());
        let row_bytes = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lonas readback"),
            size: (row_bytes * height) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());

        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes), rows_per_image: None },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

        let data = buffer.get_mapped_range(..).unwrap();
        (0..height)
            .flat_map(|y| {
                let row = &data[(y * row_bytes) as usize..];
                (0..width as usize).map(move |x| [row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]])
            })
            .collect()
    }
}

#[cfg(test)]
fn create_offscreen(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lonas offscreen"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
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
