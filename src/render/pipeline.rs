// パイプラインのキャッシュ（§10 パイプラインのキャッシュ）。GpuContext が 1 つ持ち、全体で共有する

use std::collections::HashMap;

use crate::config::{BindingConfig, BindingSlot, Blend, CullMode};
use crate::draw::Dimension;
use crate::resource::ShaderId;
use crate::resource::shader::{BindingKind, ShaderReflection};
use crate::vertex::VertexLayout;
use crate::wgsl::{CAMERA_SIZE, INSTANCE_2D_SIZE, INSTANCE_3D_SIZE, TEXTURE_RECT_SIZE};

pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

// 深度の使い方。2D / 3D、不透明 / 半透明で決まる
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DepthMode {
    None,
    Opaque,
    // 深度テストは行うが書き込まない（§10 描画順）
    Transparent,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub shader: ShaderId,
    pub dimension: Dimension,
    pub blend: Blend,
    pub cull_mode: CullMode,
    pub depth: DepthMode,
    pub layout: VertexLayout,
    pub front_face: wgpu::FrontFace,
    // 描画先の構成（§10）
    pub color_format: wgpu::TextureFormat,
    pub sample_count: u32,
}

struct CachedPipeline {
    pipeline: wgpu::RenderPipeline,
    last_used_frame: u64,
}

pub(crate) struct ShaderEntry {
    pub module: wgpu::ShaderModule,
    pub reflection: ShaderReflection,
}

pub(crate) struct PipelineCache {
    pipelines: HashMap<PipelineKey, CachedPipeline>,
}

impl PipelineCache {
    pub(crate) fn new() -> Self {
        Self { pipelines: HashMap::new() }
    }

    // なければ作る。作るときにシェーダーのバインディングの型の大きさを検査する（§11 シェーダーとの約束事）
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        bindings: &BindingConfig,
        shader: &ShaderEntry,
        key: PipelineKey,
        frame: u64,
    ) {
        if let Some(cached) = self.pipelines.get_mut(&key) {
            cached.last_used_frame = frame;
            return;
        }

        check_reflection(&shader.reflection, bindings, key.dimension);

        let pipeline = create_pipeline(device, layout, &shader.module, key);
        self.pipelines.insert(key, CachedPipeline { pipeline, last_used_frame: frame });
    }

    pub(crate) fn get(&self, key: &PipelineKey) -> &wgpu::RenderPipeline {
        &self.pipelines[key].pipeline
    }

    // 登録解除されたシェーダーのパイプラインを捨てる（§11 GPU との分担）
    pub(crate) fn remove_shader(&mut self, shader: ShaderId) {
        self.pipelines.retain(|key, _| key.shader != shader);
    }

    // 使われなくなったパイプラインを捨てる。アンチエイリアスの切り替えで使われなくなったものもこれで消える（§10）
    pub(crate) fn collect_unused(&mut self, frame: u64, unused_frames: u32) {
        self.pipelines.retain(|_, cached| frame - cached.last_used_frame < unused_frames as u64);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pipelines.len()
    }
}

// BindingConfig の位置にある変数の型が、Lonas の並び方と合うかを確かめる。合わなければ panic
fn check_reflection(reflection: &ShaderReflection, bindings: &BindingConfig, dimension: Dimension) {
    let instance_size = match dimension {
        Dimension::Two => INSTANCE_2D_SIZE,
        Dimension::Three => INSTANCE_3D_SIZE,
    };
    let check_struct = |name: &str, slot: BindingSlot, size: u32| {
        if let Some(info) = reflection.binding(slot.group, slot.binding) {
            assert!(
                info.kind == BindingKind::Uniform && info.size == size,
                "シェーダーの {name}（group {}, binding {}）は var<uniform> の {size} バイトの struct にしてください（lonas::wgsl::STRUCTS を使う）",
                slot.group,
                slot.binding,
            );
        }
    };
    let check_array = |name: &str, slot: BindingSlot, element: u32| {
        if let Some(info) = reflection.binding(slot.group, slot.binding) {
            let size = info.array_element.map(|e| e.size);
            assert!(
                matches!(info.kind, BindingKind::Storage { read_only: true }) && size == Some(element),
                "シェーダーの {name}（group {}, binding {}）は要素が {element} バイトの var<storage, read> の配列にしてください（lonas::wgsl::STRUCTS を使う）",
                slot.group,
                slot.binding,
            );
        }
    };

    check_struct("camera", bindings.camera, CAMERA_SIZE);
    check_struct("previous_camera", bindings.previous_camera, CAMERA_SIZE);
    check_array("instances", bindings.instances, instance_size);
    check_array("previous_instances", bindings.previous_instances, instance_size);
    check_array("instance_order", bindings.instance_order, 4);
    check_array("texture_rects", bindings.texture_rects, TEXTURE_RECT_SIZE);
}

fn create_pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, module: &wgpu::ShaderModule, key: PipelineKey) -> wgpu::RenderPipeline {
    let attributes = vertex_attributes(key.layout);
    let buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: key.layout.stride() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attributes,
    })];
    let depth_stencil = match key.depth {
        DepthMode::None => None,
        // Reversed-Z のため、手前ほど大きい（§11 深度バッファ）
        DepthMode::Opaque | DepthMode::Transparent => Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(key.depth == DepthMode::Opaque),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
    };

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Lonas object"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: None,
            compilation_options: Default::default(),
            buffers: &buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: key.front_face,
            cull_mode: match key.cull_mode {
                CullMode::None => None,
                CullMode::Back => Some(wgpu::Face::Back),
                CullMode::Front => Some(wgpu::Face::Front),
            },
            ..Default::default()
        },
        depth_stencil,
        multisample: wgpu::MultisampleState { count: key.sample_count, ..Default::default() },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: None,
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: key.color_format,
                blend: blend_state(key.blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn blend_state(blend: Blend) -> Option<wgpu::BlendState> {
    match blend {
        Blend::Opaque => None,
        Blend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
        Blend::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
    }
}

// 頂点の要素の @location は要素ごとに固定（§11 シェーダーとの約束事）
fn vertex_attributes(layout: VertexLayout) -> Vec<wgpu::VertexAttribute> {
    let all = [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 12, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 28, shader_location: 2 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 36, shader_location: 3 },
    ];
    let count = match layout {
        VertexLayout::Position => 1,
        VertexLayout::PositionColor => 2,
        VertexLayout::PositionColorUv => 3,
        VertexLayout::PositionColorUvNormal => 4,
    };

    all[..count].to_vec()
}

// BindingConfig から、ユーザーのシェーダーと共有するバインドグループのレイアウトを作る（§11）
pub(crate) fn bind_group_layouts(device: &wgpu::Device, bindings: &BindingConfig) -> Vec<wgpu::BindGroupLayout> {
    let stages = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
    let storage = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only: true },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let camera = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: true,
        min_binding_size: wgpu::BufferSize::new(CAMERA_SIZE as u64),
    };
    let entries = [
        (bindings.camera, camera),
        (bindings.instances, storage),
        (bindings.instance_order, storage),
        (
            bindings.atlas_texture,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
        ),
        (bindings.atlas_sampler, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        (bindings.texture_rects, storage),
        (bindings.uniform, storage),
        (bindings.previous_camera, camera),
        (bindings.previous_instances, storage),
    ];

    (0..=bindings.max_group())
        .map(|group| {
            let entries: Vec<wgpu::BindGroupLayoutEntry> = entries
                .iter()
                .filter(|(slot, _)| slot.group == group)
                .map(|&(slot, ty)| wgpu::BindGroupLayoutEntry { binding: slot.binding, visibility: stages, ty, count: None })
                .collect();

            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Lonas user"), entries: &entries })
        })
        .collect()
}
