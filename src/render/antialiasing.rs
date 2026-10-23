// アンチエイリアス（§11）：方式ごとの描画先と、後処理（FXAA・TAA）
//
// | 方式 | DrawManager* が描く先 | 後処理 |
// |---|---|---|
// | None | サーフェス | なし |
// | MSAA | サンプル数の多い描画先（サーフェスへ resolve） | なし |
// | FXAA | 中間の画像 | 中間の画像 → サーフェス |
// | TAA | 中間の画像と動きのベクトル | 中間の画像・履歴・動きのベクトル・深度 → サーフェスと次の履歴 |

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::pipeline::DEPTH_FORMAT;
use crate::config::{AntiAliasing, MotionVectorFormat, TaaConfig};
use crate::math::{Matrix4f, Vector2f};

// 履歴は精度のため半精度の浮動小数点で持つ
const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

type Target = (wgpu::Texture, wgpu::TextureView);

// サーフェスごとの描画先。大きさ・方式・フォーマットが変わったら作り直す（§11 実行中の切り替え）
pub(crate) struct FrameTargets {
    key: (u32, u32, AntiAliasing, wgpu::TextureFormat),
    pub depth: Target,
    msaa_color: Option<Target>,
    intermediate: Option<Target>,
    pub motion: Option<Target>,
    history: Option<[Target; 2]>,
    previous_depth: Option<Target>,
    history_index: usize,
}

// DrawManager* の描画のパスの描画先
pub(crate) struct PassTargets<'a> {
    pub color: &'a wgpu::TextureView,
    pub resolve: Option<&'a wgpu::TextureView>,
    pub motion: Option<&'a wgpu::TextureView>,
}

impl FrameTargets {
    pub(crate) fn ensure(
        targets: &mut Option<FrameTargets>,
        device: &wgpu::Device,
        size: (u32, u32),
        anti_aliasing: AntiAliasing,
        format: wgpu::TextureFormat,
        motion_format: MotionVectorFormat,
    ) -> bool {
        let key = (size.0.max(1), size.1.max(1), anti_aliasing, format);

        if targets.as_ref().is_some_and(|targets| targets.key == key) {
            return false;
        }

        let (width, height) = (key.0, key.1);
        let samples = sample_count(anti_aliasing);
        let texture = |label, format, samples, usage| create(device, label, width, height, format, samples, usage);
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let is_taa = anti_aliasing == AntiAliasing::Taa;

        *targets = Some(FrameTargets {
            key,
            // TAA は深度を読み、次のフレーム用にコピーする
            depth: texture(
                "Lonas depth",
                DEPTH_FORMAT,
                samples,
                if is_taa { sampled | wgpu::TextureUsages::COPY_SRC } else { attachment },
            ),
            msaa_color: (samples > 1).then(|| texture("Lonas msaa", format, samples, attachment)),
            intermediate: matches!(anti_aliasing, AntiAliasing::Fxaa | AntiAliasing::Taa)
                .then(|| texture("Lonas intermediate", format, 1, sampled)),
            motion: is_taa.then(|| texture("Lonas motion", motion_texture_format(motion_format), 1, sampled)),
            history: is_taa.then(|| [texture("Lonas history", HISTORY_FORMAT, 1, sampled), texture("Lonas history", HISTORY_FORMAT, 1, sampled)]),
            previous_depth: is_taa.then(|| {
                texture("Lonas previous depth", DEPTH_FORMAT, 1, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST)
            }),
            history_index: 0,
        });

        // 作り直した直後は履歴がない
        true
    }

    pub(crate) fn pass_targets<'a>(&'a self, surface: &'a wgpu::TextureView) -> PassTargets<'a> {
        match (&self.msaa_color, &self.intermediate) {
            (Some((_, msaa)), _) => PassTargets { color: msaa, resolve: Some(surface), motion: None },
            (None, Some((_, intermediate))) => {
                PassTargets { color: intermediate, resolve: None, motion: self.motion.as_ref().map(|(_, view)| view) }
            }
            (None, None) => PassTargets { color: surface, resolve: None, motion: None },
        }
    }
}

pub(crate) fn sample_count(anti_aliasing: AntiAliasing) -> u32 {
    match anti_aliasing {
        AntiAliasing::Msaa { sample_count } => sample_count,
        _ => 1,
    }
}

pub(crate) fn motion_texture_format(format: MotionVectorFormat) -> wgpu::TextureFormat {
    match format {
        MotionVectorFormat::Rg16Float => wgpu::TextureFormat::Rg16Float,
        MotionVectorFormat::Rg32Float => wgpu::TextureFormat::Rg32Float,
    }
}

// TAA のカメラのずらし（ピクセル、-0.5〜0.5）。Halton 列（2, 3）の 8 個を繰り返す
pub(crate) fn jitter(frame: u64) -> Vector2f {
    let index = (frame % 8) as u32 + 1;

    Vector2f::new(halton(index, 2) - 0.5, halton(index, 3) - 0.5)
}

fn halton(mut index: u32, base: u32) -> f32 {
    let (mut result, mut fraction) = (0.0, 1.0);

    while index > 0 {
        fraction /= base as f32;
        result += fraction * (index % base) as f32;
        index /= base;
    }

    result
}

// 投影の後にかけ、画面上で jitter ピクセルずらす行列
pub(crate) fn jitter_matrix(jitter: Vector2f, viewport_size: Vector2f) -> Matrix4f {
    let x = 2.0 * jitter.x / viewport_size.x;
    let y = -2.0 * jitter.y / viewport_size.y;

    // 平行移動の列は clip の w に掛かるため、正規化した後の位置が (x, y) ずれる
    Matrix4f::from_translation(crate::math::Vector3f::new(x, y, 0.0))
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct TaaParams {
    history_blend_min: f32,
    history_blend_max: f32,
    depth_rejection_threshold: f32,
    color_rejection_threshold: f32,
    history_valid: u32,
    _pad: [u32; 3],
}

pub(crate) struct PostProcess {
    fxaa_layout: wgpu::BindGroupLayout,
    taa_layout: wgpu::BindGroupLayout,
    fxaa_module: wgpu::ShaderModule,
    taa_module: wgpu::ShaderModule,
    fxaa_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    taa_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
}

impl PostProcess {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let fxaa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lonas fxaa"),
            entries: &[texture(0, float), sampler(1)],
        });
        let taa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lonas taa"),
            entries: &[
                texture(0, float),
                texture(1, float),
                // 動きのベクトルは textureLoad で読むため、補間しない型でよい
                texture(2, wgpu::TextureSampleType::Float { filterable: false }),
                texture(3, wgpu::TextureSampleType::Depth),
                texture(4, wgpu::TextureSampleType::Depth),
                sampler(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let module = |label, source: &'static str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) })
        };

        Self {
            fxaa_layout,
            taa_layout,
            fxaa_module: module("Lonas fxaa", include_str!("fxaa.wgsl")),
            taa_module: module("Lonas taa", include_str!("taa.wgsl")),
            fxaa_pipelines: HashMap::new(),
            taa_pipelines: HashMap::new(),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Lonas post"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    // 後処理を描画先のサーフェスに記録する。MSAA と None は何もしない
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &mut FrameTargets,
        surface: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        taa: &TaaConfig,
        history_valid: bool,
    ) {
        let Some((_, intermediate)) = &targets.intermediate else { return };

        if let Some(history) = &targets.history {
            let (motion, previous_depth) = (&targets.motion.as_ref().unwrap().1, &targets.previous_depth.as_ref().unwrap());
            let (read, write) = (&history[targets.history_index].1, &history[1 - targets.history_index].1);
            let params = TaaParams {
                history_blend_min: taa.history_blend_min,
                history_blend_max: taa.history_blend_max,
                depth_rejection_threshold: taa.depth_rejection_threshold,
                color_rejection_threshold: taa.color_rejection_threshold,
                history_valid: history_valid as u32,
                _pad: [0; 3],
            };

            // 同じフレームの他のサーフェスと値が混ざらないよう、サーフェスごとに専用のバッファを作る
            let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Lonas taa params"),
                size: size_of::<TaaParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

            let depth_view = |texture: &wgpu::Texture| {
                texture.create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() })
            };
            let depth = depth_view(&targets.depth.0);
            let previous = depth_view(&previous_depth.0);
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lonas taa"),
                layout: &self.taa_layout,
                entries: &[
                    view_entry(0, intermediate),
                    view_entry(1, read),
                    view_entry(2, motion),
                    view_entry(3, &depth),
                    view_entry(4, &previous),
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 6, resource: params_buffer.as_entire_binding() },
                ],
            });
            let pipeline = self.taa_pipelines.entry(format).or_insert_with(|| {
                create_post_pipeline(device, &self.taa_layout, &self.taa_module, &[Some(format.into()), Some(HISTORY_FORMAT.into())])
            });

            run_pass(encoder, &[surface, write], pipeline, &bind_group);

            // 5. 保存：現在の深度を次のフレーム用にコピーし、履歴を入れ替える
            encoder.copy_texture_to_texture(
                targets.depth.0.as_image_copy(),
                previous_depth.0.as_image_copy(),
                targets.depth.0.size(),
            );
            targets.history_index = 1 - targets.history_index;
        } else {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Lonas fxaa"),
                layout: &self.fxaa_layout,
                entries: &[view_entry(0, intermediate), wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
            });
            let pipeline = self
                .fxaa_pipelines
                .entry(format)
                .or_insert_with(|| create_post_pipeline(device, &self.fxaa_layout, &self.fxaa_module, &[Some(format.into())]));

            run_pass(encoder, &[surface], pipeline, &bind_group);
        }
    }
}

fn view_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}

fn run_pass(encoder: &mut wgpu::CommandEncoder, targets: &[&wgpu::TextureView], pipeline: &wgpu::RenderPipeline, bind_group: &wgpu::BindGroup) {
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = targets
        .iter()
        .map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })
        })
        .collect();
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Lonas post"),
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

fn create_post_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    targets: &[Option<wgpu::ColorTargetState>],
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Lonas post"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Lonas post"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState { module, entry_point: Some("fs"), compilation_options: Default::default(), targets }),
        multiview_mask: None,
        cache: None,
    })
}

fn create(device: &wgpu::Device, label: &str, width: u32, height: u32, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_half_pixel_and_repeats() {
        let offsets: Vec<Vector2f> = (0..16).map(jitter).collect();

        assert!(offsets.iter().all(|j| j.x.abs() <= 0.5 && j.y.abs() <= 0.5));
        assert_eq!(offsets[0], offsets[8]);
        // 8 個はすべて違う位置
        for i in 0..8 {
            for k in i + 1..8 {
                assert_ne!(offsets[i], offsets[k]);
            }
        }
    }

    #[test]
    fn halton_matches_known_values() {
        assert_eq!(halton(1, 2), 0.5);
        assert_eq!(halton(2, 2), 0.25);
        assert!((halton(1, 3) - 1.0 / 3.0).abs() < 1e-6);
    }
}
