// 描画（§10）：シェーダー・パイプライン・Compute のパス・描画のパス・テクスチャアトラス・uniform

pub(crate) mod antialiasing;
pub(crate) mod frame;
pub(crate) mod pipeline;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::config::{BindingConfig, BindingSlot, LonasConfig};
use crate::draw::{CameraFrame, Dimension, DrawManagerCore, HeapRequest, SpaceKind};
use crate::gpu::heap::Heap;
use crate::memory::ManagerId;
use crate::resource::shader::BindingKind;
use crate::resource::{ResourceChange, ResourceManager, ShaderId};
use crate::texture::atlas::Atlas;
use frame::{DrawResources, FramePlan, PlanInput, Strides};
pub(crate) use frame::TargetFormat;
use pipeline::{PipelineCache, ShaderEntry};

// cull.wgsl の Compute のパイプライン
pub(crate) struct Culling {
    pub layout: wgpu::BindGroupLayout,
    pub cull_compact: wgpu::ComputePipeline,
    pub transparent_keys: wgpu::ComputePipeline,
    pub bitonic_step: wgpu::ComputePipeline,
    pub transparent_emit: wgpu::ComputePipeline,
}

// DrawManager* × シェーダーごとの uniform の配列（§6 uniform の領域）
struct UniformBuffer {
    buffer: wgpu::Buffer,
    // 要素の数
    capacity: u32,
    // 要素の間隔（WGSL の配列の stride）
    stride: u64,
}

pub(crate) struct Renderer {
    bindings: BindingConfig,
    user_layouts: Vec<wgpu::BindGroupLayout>,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: HashMap<ShaderId, ShaderEntry>,
    pipelines: PipelineCache,
    culling: Culling,
    atlas: Atlas,
    uniforms: HashMap<(ManagerId, ShaderId), UniformBuffer>,
    // uniform を使わないシェーダーのバインディングに置く、すべて 0 のバッファ
    empty_uniform: wgpu::Buffer,
    uniform_initial_elements: u32,
    growth_factor: f32,
    post: antialiasing::PostProcess,
    strides: Strides,
    // シェーダーを指定していない Object を描かない警告を、一度だけ出す
    warned_missing_shader: HashSet<crate::object::instance_slots::ObjectId>,
}

// 1 つの DrawManager* の、そのフレームの描画（end_frame の 5 と 6 の間で持ち回る）
pub(crate) struct PreparedDraw {
    plan: FramePlan,
    layout_space: SpaceKind,
    // シェーダーごとのバインドグループ（uniform の配列がシェーダーごとのため）
    user_bind_groups: HashMap<ShaderId, Vec<wgpu::BindGroup>>,
}

impl Renderer {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, config: &LonasConfig) -> Self {
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
            atlas: Atlas::new(device, queue, config),
            uniforms: HashMap::new(),
            empty_uniform: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Lonas empty uniform"),
                size: 256,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            uniform_initial_elements: config.memory.uniform_initial_elements.max(1),
            growth_factor: config.dynamic_growth_factor,
            post: antialiasing::PostProcess::new(device),
            strides: Strides {
                camera: align(crate::wgsl::CAMERA_SIZE as u64),
                params: align(size_of::<frame::GpuParams>() as u64),
            },
            warned_missing_shader: HashSet::new(),
        }
    }

    // end_frame の「2. リソースの反映」
    pub(crate) fn apply_resource_changes(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, resources: &mut ResourceManager) {
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
                    self.uniforms.retain(|&(_, shader), _| shader != id);
                }
                ResourceChange::TextureAdded(id) => {
                    // 転送した後、ResourceManager 側の画像は捨てる（§11 GPU との分担）
                    if let Some(data) = resources.take_texture_data(id) {
                        self.atlas.add(device, queue, id.slot(), &data);
                    }
                }
                ResourceChange::TextureRemoved(id) => self.atlas.remove(id.slot()),
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
        target: TargetFormat,
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

        self.write_uniforms(device, encoder, manager);

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
            .map(|(command, &shader)| frame::pipeline_key(command, manager, shader, target))
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

        let manager_id = manager.id;
        let mut user_bind_groups = HashMap::new();

        for batch in &plan.batches {
            user_bind_groups
                .entry(batch.key.shader)
                .or_insert_with(|| self.user_bind_groups(device, resources, heap, instance_space, manager_id, batch.key.shader));
        }

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

            for (group, bind_group) in prepared.user_bind_groups[&batch.key.shader].iter().enumerate() {
                let offsets = self.camera_offsets(group as u32, batch.camera);
                pass.set_bind_group(group as u32, bind_group, &offsets);
            }

            pass.multi_draw_indexed_indirect(&resources.args.buffer, frame::args_offset(batch.args_start), batch.args_count);
        }
    }

    // アンチエイリアスの後処理（FXAA・TAA）を記録する（§11 アンチエイリアス）
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn post_process(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        targets: &mut antialiasing::FrameTargets,
        surface: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        taa: &crate::config::TaaConfig,
        history_valid: bool,
    ) {
        self.post.record(device, queue, encoder, targets, surface, format, taa, history_valid);
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

    // uniform の値を、DrawManager* × シェーダーのバッファの要素へコピーする（§6 uniform の領域）
    // 要素が足りなければ、より大きなバッファを作り、既存の内容をコピーして差し替える（別のバッファ同士のためコピーは 1 回）
    fn write_uniforms(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, manager: &DrawManagerCore) {
        for request in &manager.heap_requests {
            let HeapRequest::WriteUniform { shader, slot, chunk, ref range } = *request else { continue };
            let entry = self.shaders.get(&shader).expect("登録されていないシェーダーの uniform です");
            let binding = self.bindings.uniform;
            let element = entry
                .reflection
                .binding(binding.group, binding.binding)
                .filter(|info| matches!(info.kind, BindingKind::Storage { read_only: true }))
                .and_then(|info| info.array_element)
                .unwrap_or_else(|| {
                    panic!(
                        "uniform() を呼んだ Object のシェーダーに、uniform の配列（group {}, binding {} の var<storage, read> の配列）がありません",
                        binding.group, binding.binding,
                    )
                });
            let size = range.end - range.start;

            // T の大きさが WGSL の要素の大きさと違う場合は panic（§11 uniform の型の検査）。4 バイトへの詰め物は許す
            assert!(
                size == (element.size as u64).div_ceil(4) * 4,
                "uniform() に渡した型の大きさ（{size} バイト）が、シェーダーの uniform の要素の大きさ（{} バイト）と違います",
                element.size,
            );

            let key = (manager.id, shader);
            let initial = self.uniform_initial_elements;
            let growth = self.growth_factor;
            let stride = element.stride as u64;
            let uniform = self.uniforms.entry(key).or_insert_with(|| UniformBuffer {
                buffer: create_uniform_buffer(device, initial as u64 * stride),
                capacity: initial,
                stride,
            });

            if slot >= uniform.capacity {
                let capacity = ((uniform.capacity as f32 * growth).ceil() as u32).max(slot + 1);
                let buffer = create_uniform_buffer(device, capacity as u64 * stride);

                encoder.copy_buffer_to_buffer(&uniform.buffer, 0, &buffer, 0, uniform.buffer.size());
                *uniform = UniformBuffer { buffer, capacity, stride };
            }

            encoder.copy_buffer_to_buffer(manager.staging.buffer(chunk), range.start, &uniform.buffer, slot as u64 * uniform.stride, size);
        }
    }

    fn user_bind_groups(
        &self,
        device: &wgpu::Device,
        resources: &DrawResources,
        heap: &Heap,
        instance_space: SpaceKind,
        manager: ManagerId,
        shader: ShaderId,
    ) -> Vec<wgpu::BindGroup> {
        let instances = heap.buffer(instance_space).expect("インスタンスのヒープがありません");
        // 前のフレームのインスタンス。TAA を使わないフレームは写していないため、現在と同じもの
        let previous_instances = heap.previous_buffer(instance_space).unwrap_or(instances);
        let previous_camera = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &resources.previous_cameras.buffer,
            offset: 0,
            size: wgpu::BufferSize::new(crate::wgsl::CAMERA_SIZE as u64),
        });
        let b = &self.bindings;
        let camera = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &resources.cameras.buffer,
            offset: 0,
            size: wgpu::BufferSize::new(crate::wgsl::CAMERA_SIZE as u64),
        });
        let uniform = self.uniforms.get(&(manager, shader)).map_or(&self.empty_uniform, |uniform| &uniform.buffer);
        let all = [
            (b.camera, camera),
            (b.instances, instances.as_entire_binding()),
            (b.instance_order, resources.order.buffer.as_entire_binding()),
            (b.atlas_texture, wgpu::BindingResource::TextureView(self.atlas.view())),
            (b.atlas_sampler, wgpu::BindingResource::Sampler(self.atlas.sampler())),
            (b.texture_rects, self.atlas.rects().as_entire_binding()),
            (b.uniform, uniform.as_entire_binding()),
            (b.previous_camera, previous_camera),
            (b.previous_instances, previous_instances.as_entire_binding()),
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
    pub(crate) fn atlas(&self) -> &Atlas {
        &self.atlas
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

fn create_uniform_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    // 作った時点ですべて 0。要素 0 は予約（uniform() を呼ばなかった Object* が指す、§6 uniform の領域）
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Lonas uniform"),
        size: size.max(256).div_ceil(4) * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}
