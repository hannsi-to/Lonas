// GPU 上のテクスチャアトラス（§11 TextureAtlas）
// 配置の計算は packer、画像の転送・ミップマップ・レイヤーの追加・texture_rects の表はここで行う

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::packer::{Placement, ShelfPacker};
use crate::config::{FilterMode, LonasConfig};
use crate::resource::TextureData;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
// テクスチャ未指定を表す色（ピンク紫、§11 texture() を呼ばなかった Object*）
const MISSING: [u8; 4] = [255, 0, 255, 255];

// WGSL の TextureRect（32 バイト、§11 シェーダーとの約束事）
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct GpuTextureRect {
    rect: [f32; 4],
    layer: u32,
    _pad: [u32; 3],
}

const _: () = assert!(size_of::<GpuTextureRect>() == crate::wgsl::TEXTURE_RECT_SIZE as usize);

pub(crate) struct Atlas {
    packer: ShelfPacker,
    size: u32,
    mip_level_count: u32,
    growth_factor: f32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    // テクスチャの番号 → アトラス上の範囲とレイヤー（§11 UV の変換）
    rects: wgpu::Buffer,
    placements: HashMap<u32, Placement>,
    mipmap: MipmapGenerator,
}

impl Atlas {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, config: &LonasConfig) -> Self {
        let atlas = &config.texture_atlas;
        let filter = match atlas.filter {
            FilterMode::Nearest => wgpu::FilterMode::Nearest,
            FilterMode::Linear => wgpu::FilterMode::Linear,
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lonas atlas"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: if atlas.mip_level_count > 1 { wgpu::MipmapFilterMode::Linear } else { wgpu::MipmapFilterMode::Nearest },
            ..Default::default()
        });
        let texture = create_texture(device, atlas.size, atlas.initial_layers, atlas.mip_level_count);
        let mut result = Self {
            packer: ShelfPacker::new(atlas.size, atlas.padding, atlas.mip_level_count, atlas.initial_layers, atlas.max_layers),
            size: atlas.size,
            mip_level_count: atlas.mip_level_count,
            growth_factor: config.dynamic_growth_factor,
            view: array_view(&texture),
            texture,
            sampler,
            rects: create_rects(device, 64),
            placements: HashMap::new(),
            mipmap: MipmapGenerator::new(device),
        };

        // 番号 0 はテクスチャ未指定のピンク紫の 1 画素
        result.add(device, queue, 0, &TextureData::from_rgba(1, 1, MISSING.to_vec()));

        result
    }

    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub(crate) fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    pub(crate) fn rects(&self) -> &wgpu::Buffer {
        &self.rects
    }

    // 配置を決め、余白を付けて転送し、ミップマップを作る（end_frame の「2. リソースの反映」）
    // ミップマップは自分の encoder ですぐに送る。同じフレームの後の登録でレイヤーを増やしても、作り終えた内容をコピーできるように
    pub(crate) fn add(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, slot: u32, data: &TextureData) {
        let placement = match self.packer.allocate(data.width, data.height) {
            Some(placement) => placement,
            None => {
                self.add_layers(device, queue);
                self.packer.allocate(data.width, data.height).expect("レイヤーを増やした後も配置できません")
            }
        };
        let padded = pad_with_edges(data, &placement);

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: placement.outer_x, y: placement.outer_y, z: placement.layer },
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(placement.outer_width * 4),
                rows_per_image: Some(placement.outer_height),
            },
            wgpu::Extent3d { width: placement.outer_width, height: placement.outer_height, depth_or_array_layers: 1 },
        );

        if self.mip_level_count > 1 {
            let mut encoder = device.create_command_encoder(&Default::default());

            for level in 1..self.mip_level_count {
                self.mipmap.generate(device, &mut encoder, &self.texture, &placement, level);
            }

            queue.submit([encoder.finish()]);
        }

        self.placements.insert(slot, placement);
        self.write_rect(device, queue, slot);
    }

    // 空いた場所は同じ棚の中で再利用する。表の値は番号が再利用されるまで残す（§11 UV の変換）
    pub(crate) fn remove(&mut self, slot: u32) {
        if let Some(placement) = self.placements.remove(&slot) {
            self.packer.deallocate(placement);
        }
    }

    #[cfg(test)]
    pub(crate) fn layer_count(&self) -> u32 {
        self.packer.layer_count()
    }

    #[cfg(test)]
    pub(crate) fn placement(&self, slot: u32) -> Option<Placement> {
        self.placements.get(&slot).copied()
    }

    #[cfg(test)]
    pub(crate) fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    fn write_rect(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, slot: u32) {
        let needed = (slot as u64 + 1) * size_of::<GpuTextureRect>() as u64;

        if needed > self.rects.size() {
            // 表を大きくするときは、すべての番号を書き直す
            let entries = (slot as u64 + 1).next_power_of_two();
            self.rects = create_rects(device, entries);

            for (&slot, _) in &self.placements {
                queue.write_buffer(&self.rects, slot as u64 * size_of::<GpuTextureRect>() as u64, bytemuck::bytes_of(&self.rect(slot)));
            }
        }

        queue.write_buffer(&self.rects, slot as u64 * size_of::<GpuTextureRect>() as u64, bytemuck::bytes_of(&self.rect(slot)));
    }

    // 余白を含まない範囲の UV（§11 にじみ対策）
    fn rect(&self, slot: u32) -> GpuTextureRect {
        let p = &self.placements[&slot];
        let size = self.size as f32;

        GpuTextureRect {
            rect: [p.inner_x as f32 / size, p.inner_y as f32 / size, p.width as f32 / size, p.height as f32 / size],
            layer: p.layer,
            _pad: [0; 3],
        }
    }

    // max_layers まで増やす。より多いレイヤーのテクスチャを作り、既存の内容をコピーして差し替える（§11 容量を超えた場合）
    fn add_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let current = self.packer.layer_count();
        let target = ((current as f32 * self.growth_factor).ceil() as u32).max(current + 1).min(self.packer.max_layers());

        // max_layers を超えるなら、ここで panic する
        self.packer.add_layers(target.max(current + 1) - current);

        let texture = create_texture(device, self.size, self.packer.layer_count(), self.mip_level_count);
        // 新しいテクスチャへの書き込み（write_texture）より先に、コピーを終えておく
        let mut encoder = device.create_command_encoder(&Default::default());

        for level in 0..self.mip_level_count {
            let size = (self.size >> level).max(1);
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: current },
            );
        }

        queue.submit([encoder.finish()]);
        self.view = array_view(&texture);
        self.texture = texture;
    }
}

// 余白を、テクスチャの端の画素を複製して埋める（端で ClampToEdge と同じ見た目になる、§11 にじみ対策）
fn pad_with_edges(data: &TextureData, placement: &Placement) -> Vec<u8> {
    let (left, top) = (placement.inner_x - placement.outer_x, placement.inner_y - placement.outer_y);
    let mut padded = Vec::with_capacity((placement.outer_width * placement.outer_height * 4) as usize);

    for y in 0..placement.outer_height {
        let source_y = (y as i64 - top as i64).clamp(0, data.height as i64 - 1) as u32;

        for x in 0..placement.outer_width {
            let source_x = (x as i64 - left as i64).clamp(0, data.width as i64 - 1) as u32;
            let index = ((source_y * data.width + source_x) * 4) as usize;
            padded.extend_from_slice(&data.pixels[index..index + 4]);
        }
    }

    padded
}

fn create_texture(device: &wgpu::Device, size: u32, layers: u32, mip_level_count: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lonas atlas"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
        mip_level_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        // ミップマップを描画で作るため RENDER_ATTACHMENT、レイヤーを増やすときのコピーのため COPY_SRC
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
}

fn create_rects(device: &wgpu::Device, entries: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Lonas texture rects"),
        size: entries * size_of::<GpuTextureRect>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

struct MipmapGenerator {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl MipmapGenerator {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lonas mipmap"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mipmap.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Lonas mipmap"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lonas mipmap"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Lonas mipmap"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lonas mipmap"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self { pipeline, layout, sampler }
    }

    // level の段の、登録したテクスチャの範囲（余白を含む）だけを、1 つ上の段から作る
    fn generate(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, placement: &Placement, level: u32) {
        let view = |mip: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: mip,
                mip_level_count: Some(1),
                base_array_layer: placement.layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let source = view(level - 1);
        let target = view(level);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Lonas mipmap"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Lonas mipmap"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // 配置位置と大きさは 2^(段数 - 1) の倍数のため、どの段でも割り切れる（§11 ミップマップ）
        let (x, y, w, h) = (placement.outer_x >> level, placement.outer_y >> level, placement.outer_width >> level, placement.outer_height >> level);

        pass.set_viewport(0.0, 0.0, (texture.width() >> level) as f32, (texture.height() >> level) as f32, 0.0, 1.0);
        pass.set_scissor_rect(x, y, w.max(1), h.max(1));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
