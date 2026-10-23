// 描画（§10）：シェーダー・パイプライン・Compute のパス・描画のパス
// テクスチャアトラスと uniform は段階 7 で作る。それまでは中身のない代わりのものをバインドする

pub(crate) mod frame;
pub(crate) mod pipeline;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::config::{BindingConfig, BindingSlot, LonasConfig};
use crate::draw::{CameraFrame, Dimension, DrawManagerCore, SpaceKind};
use crate::gpu::heap::Heap;
use crate::resource::{ResourceChange, ResourceManager, ShaderId};
use frame::{DrawResources, FramePlan, PlanInput, Strides};
use pipeline::{PipelineCache, ShaderEntry};

// cull.wgsl の Compute のパイプライン
pub(crate) struct Culling {
    pub layout: wgpu::BindGroupLayout,
    pub cull_compact: wgpu::ComputePipeline,
    pub transparent_keys: wgpu::ComputePipeline,
    pub bitonic_step: wgpu::ComputePipeline,
    pub transparent_emit: wgpu::ComputePipeline,
}

// 段階 7 までの代わりのアトラス・サンプラー・texture_rects・uniform
struct Placeholders {
    atlas_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    texture_rects: wgpu::Buffer,
    uniform: wgpu::Buffer,
}

pub(crate) struct Renderer {
    bindings: BindingConfig,
    user_layouts: Vec<wgpu::BindGroupLayout>,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: HashMap<ShaderId, ShaderEntry>,
    pipelines: PipelineCache,
    culling: Culling,
    placeholders: Placeholders,
    strides: Strides,
    // シェーダーを指定していない Object を描かない警告を、一度だけ出す
    warned_missing_shader: HashSet<crate::object::instance_slots::ObjectId>,
}

// 1 つの DrawManager* の、そのフレームの描画（end_frame の 5 と 6 の間で持ち回る）
pub(crate) struct PreparedDraw {
    plan: FramePlan,
    layout_space: SpaceKind,
    user_bind_groups: Vec<wgpu::BindGroup>,
}

impl Renderer {
    pub(crate) fn new(device: &wgpu::Device, config: &LonasConfig) -> Self {
        let bindings = config.bindings;
        let user_layouts = pipeline::bind_group_layouts(device, &bindings);
        let layout_refs: Vec<Option<&wgpu::BindGroupLayout>> = user_layouts.iter().map(Some).collect();
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Lonas user"),
            bind_group_layouts: &layout_refs,
            immediate_size: 0,
        });
        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let align = |size: u64| size.div_ceil(alignment) * alignment;

        Self {
            bindings,
            user_layouts,
            pipeline_layout,
            shaders: HashMap::new(),
            pipelines: PipelineCache::new(),
            culling: create_culling(device),
            placeholders: create_placeholders(device),
            strides: Strides {
                camera: align(crate::wgsl::CAMERA_SIZE as u64),
                params: align(size_of::<frame::GpuParams>() as u64),
            },
            warned_missing_shader: HashSet::new(),
        }
    }

    // end_frame の「2. リソースの反映」。テクスチャは段階 7 で反映する
    pub(crate) fn apply_resource_changes(&mut self, device: &wgpu::Device, resources: &mut ResourceManager) {
        for change in resources.take_changes() {
            match change {
                ResourceChange::ShaderAdded(id) => {
                    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("Lonas user shader"),
                        source: wgpu::ShaderSource::Wgsl(resources.shader_source(id).into()),
                    });
                    let reflection = resources.shader_reflection(id).clone();

                    self.shaders.insert(id, ShaderEntry { module, reflection });
                }
                ResourceChange::ShaderRemoved(id) => {
                    self.shaders.remove(&id);
                    self.pipelines.remove_shader(id);
                }
                ResourceChange::TextureAdded(_) | ResourceChange::TextureRemoved(_) => {}
            }
        }
    }

    // 描画の順を決め、パイプラインを用意し、Compute を記録する（end_frame の 4・5）
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        heap: &Heap,
        resources: &ResourceManager,
        manager: &mut DrawManagerCore,
        cameras: &[Option<CameraFrame>],
        camera_slots: &[u32],
        color_format: wgpu::TextureFormat,
        frame: u64,
    ) -> Option<PreparedDraw> {
        let mut shaders = Vec::with_capacity(manager.draw_commands.len());

        for command in &manager.draw_commands {
            match command.settings.shader {
                Some(id) => {
                    // 無効な ID は end では判定できないため、ここで判定する（§11 登録解除）
                    assert!(resources.owns_shader(id), "別の ResourceManager の ShaderId です");
                    assert!(resources.is_shader_valid(id), "登録解除済みの ShaderId を使う Object があります");
                    shaders.push(Some(id));
                }
                None => {
                    if self.warned_missing_shader.insert(command.object) {
                        log::warn!("shader() を呼んでいない Object は描きません");
                    }
                    shaders.push(None);
                }
            }
        }

        if let Some(texture) = manager.draw_commands.iter().find_map(|command| command.settings.texture) {
            assert!(resources.owns_texture(texture), "別の ResourceManager の TextureId です");
        }

        for command in &manager.draw_commands {
            if let Some(texture) = command.settings.texture {
                assert!(resources.is_texture_valid(texture), "登録解除済みの TextureId を使う Object があります");
            }
        }

        let keys: Vec<_> = manager
            .draw_commands
            .iter()
            .zip(&shaders)
            .map(|(command, &shader)| frame::pipeline_key(command, manager, shader, color_format))
            .collect();
        let plan = FramePlan::build(manager, heap, PlanInput { commands: &manager.draw_commands, keys: &keys, camera_slots });

        if plan.is_empty() {
            return None;
        }

        for batch in &plan.batches {
            let shader = &self.shaders[&batch.key.shader];
            self.pipelines.prepare(device, &self.pipeline_layout, &self.bindings, shader, batch.key, frame);
        }

        let dimension = manager.dimension;
        let instance_space = match dimension {
            Dimension::Two => SpaceKind::Instance2D,
            Dimension::Three => SpaceKind::Instance3D,
        };
        let instance_buffer = heap.buffer(instance_space).expect("インスタンスのヒープがありません");
        let resources = manager.resources.get_or_insert_with(|| DrawResources::new(device));

        frame::record_compute(device, queue, encoder, &self.culling, resources, &plan, cameras, instance_buffer, dimension, &self.strides);

        #[cfg(test)]
        {
            resources.last_batches = plan.batches.iter().map(|batch| (batch.args_start, batch.args_count)).collect();
        }

        let user_bind_groups = self.user_bind_groups(device, resources, instance_buffer);

        Some(PreparedDraw { plan, layout_space: SpaceKind::Vertex(manager.layout), user_bind_groups })
    }

    // 準備した描画を、描画のパスに記録する（end_frame の 6）
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, heap: &Heap, manager: &DrawManagerCore, prepared: &PreparedDraw) {
        let resources = manager.resources.as_ref().expect("描画のバッファがありません");
        let (Some(vertices), Some(indices)) = (heap.buffer(prepared.layout_space), heap.buffer(SpaceKind::Index)) else { return };

        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);

        for batch in &prepared.plan.batches {
            pass.set_pipeline(self.pipelines.get(&batch.key));

            for (group, bind_group) in prepared.user_bind_groups.iter().enumerate() {
                let offsets = self.camera_offsets(group as u32, batch.camera);
                pass.set_bind_group(group as u32, bind_group, &offsets);
            }

            pass.multi_draw_indexed_indirect(&resources.args.buffer, frame::args_offset(batch.args_start), batch.args_count);
        }
    }

    pub(crate) fn collect_unused(&mut self, frame: u64, unused_frames: u32) {
        self.pipelines.collect_unused(frame, unused_frames);
    }

    // カメラは動的オフセットで切り替える。動的オフセットは binding の番号の順に並べる
    fn camera_offsets(&self, group: u32, camera: u32) -> Vec<u32> {
        let mut slots: Vec<BindingSlot> =
            [self.bindings.camera, self.bindings.previous_camera].into_iter().filter(|slot| slot.group == group).collect();
        slots.sort_by_key(|slot| slot.binding);

        slots.iter().map(|_| (camera as u64 * self.strides.camera) as u32).collect()
    }

    fn user_bind_groups(&self, device: &wgpu::Device, resources: &DrawResources, instances: &wgpu::Buffer) -> Vec<wgpu::BindGroup> {
        let b = &self.bindings;
        let camera = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &resources.cameras.buffer,
            offset: 0,
            size: wgpu::BufferSize::new(crate::wgsl::CAMERA_SIZE as u64),
        });
        let placeholders = &self.placeholders;
        let all = [
            (b.camera, camera.clone()),
            (b.instances, instances.as_entire_binding()),
            (b.instance_order, resources.order.buffer.as_entire_binding()),
            (b.atlas_texture, wgpu::BindingResource::TextureView(&placeholders.atlas_view)),
            (b.atlas_sampler, wgpu::BindingResource::Sampler(&placeholders.sampler)),
            (b.texture_rects, placeholders.texture_rects.as_entire_binding()),
            (b.uniform, placeholders.uniform.as_entire_binding()),
            // 前のフレームのカメラ・インスタンスは段階 8（TAA）で作る。それまでは現在と同じもの
            (b.previous_camera, camera),
            (b.previous_instances, instances.as_entire_binding()),
        ];

        self.user_layouts
            .iter()
            .enumerate()
            .map(|(group, layout)| {
                let entries: Vec<wgpu::BindGroupEntry> = all
                    .iter()
                    .filter(|(slot, _)| slot.group == group as u32)
                    .map(|(slot, resource)| wgpu::BindGroupEntry { binding: slot.binding, resource: resource.clone() })
                    .collect();

                device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Lonas user"), layout, entries: &entries })
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }
}

fn create_culling(device: &wgpu::Device) -> Culling {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Lonas cull"),
        source: wgpu::ShaderSource::Wgsl(include_str!("cull.wgsl").into()),
    });
    let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Lonas cull"),
        entries: &[
            storage(0, true),
            storage(1, true),
            storage(2, true),
            storage(3, false),
            storage(4, false),
            storage(5, false),
            storage(6, false),
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<frame::GpuParams>() as u64),
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Lonas cull"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = |entry_point: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry_point),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            cache: None,
        })
    };

    Culling {
        cull_compact: pipeline("cull_compact"),
        transparent_keys: pipeline("transparent_keys"),
        bitonic_step: pipeline("bitonic_step"),
        transparent_emit: pipeline("transparent_emit"),
        layout,
    }
}

fn create_placeholders(device: &wgpu::Device) -> Placeholders {
    let atlas = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lonas atlas placeholder"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let buffer = |label| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 256,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        })
    };

    Placeholders {
        atlas_view,
        sampler: device.create_sampler(&wgpu::SamplerDescriptor::default()),
        texture_rects: buffer("Lonas texture rects placeholder"),
        uniform: buffer("Lonas uniform placeholder"),
    }
}
