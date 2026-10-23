// DrawManager* ごとの 1 フレームの描画（§10）
// 描画コマンドから Object の表と描画の順を作り、Compute で引数とインスタンス番号のリストを書かせ、描く

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::pipeline::{DepthMode, PipelineKey};
use crate::config::Blend;
use crate::draw::{CameraFrame, Dimension, DrawCommand, DrawManagerCore, SpaceKind};
use crate::gpu::heap::Heap;
use crate::resource::ShaderId;

const FLAG_CULL: u32 = 1;
const FLAG_BUCKET_SORT: u32 = 2;
const FLAG_TRANSPARENT: u32 = 4;
const WORKGROUP_SIZE: u32 = 256;
// DrawIndexedIndirectArgs の大きさ
const ARGS_SIZE: u64 = 20;

// cull.wgsl の ObjectData（64 バイト）
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuObject {
    pub instance_base: u32,
    pub instance_count: u32,
    pub order_base: u32,
    pub args_index: u32,
    pub index_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub camera: u32,
    pub bounds: [f32; 4],
    pub flags: u32,
    pub sort_base: u32,
    pub _pad: [u32; 2],
}

// cull.wgsl の CameraCull（80 バイト）
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct GpuCameraCull {
    view_projection: [f32; 16],
    depth_range: [f32; 4],
}

// cull.wgsl の Params（48 バイト）
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub(crate) struct GpuParams {
    instance_words: u32,
    visibility_word: u32,
    is_3d: u32,
    object_count: u32,
    group_object_start: u32,
    group_object_count: u32,
    item_count: u32,
    padded_count: u32,
    args_base: u32,
    order_base: u32,
    k: u32,
    j: u32,
}

const _: () = assert!(size_of::<GpuObject>() == 64);
const _: () = assert!(size_of::<GpuCameraCull>() == 80);
const _: () = assert!(size_of::<GpuParams>() == 48);

// 1 回の multi_draw_indexed_indirect
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Batch {
    pub key: PipelineKey,
    pub camera: u32,
    pub args_start: u32,
    pub args_count: u32,
}

// 3D の半透明の、パイプラインとカメラごとのまとまり
#[derive(Copy, Clone, Debug)]
struct TransparentGroup {
    object_start: u32,
    object_count: u32,
    item_count: u32,
    padded_count: u32,
    args_base: u32,
    order_base: u32,
}

pub(crate) struct FramePlan {
    pub batches: Vec<Batch>,
    pub objects: Vec<GpuObject>,
    groups: Vec<TransparentGroup>,
    has_opaque: bool,
    order_len: u32,
    args_len: u32,
    padded_max: u32,
}

// 描画の順と、それぞれの描画の設定（パイプラインのキー以外）
pub(crate) struct PlanInput<'a> {
    pub commands: &'a [DrawCommand],
    pub keys: &'a [Option<PipelineKey>],
    pub camera_slots: &'a [u32],
}

impl FramePlan {
    // 描画の順を決める（§10 描画順）
    // 2D：layer の小さい順、同じ layer は end の順。続いている同じ設定をまとめる
    // 3D：不透明をパイプラインとカメラでまとめ、続いて半透明をパイプラインとカメラごとに、最初に end された順に描く
    pub(crate) fn build(manager: &DrawManagerCore, heap: &Heap, input: PlanInput<'_>) -> Self {
        let instance_space = match manager.dimension {
            Dimension::Two => SpaceKind::Instance2D,
            Dimension::Three => SpaceKind::Instance3D,
        };
        let object_of = |index: usize, flags: u32| {
            let command = &input.commands[index];

            GpuObject {
                instance_base: heap.offset(instance_space, command.instance) as u32,
                instance_count: command.instance_count,
                index_count: command.index_count,
                first_index: heap.offset(SpaceKind::Index, command.index) as u32,
                base_vertex: heap.offset(SpaceKind::Vertex(manager.layout), command.vertex) as i32,
                camera: input.camera_slots[index],
                bounds: command.bounds,
                flags: flags | if manager.culling { FLAG_CULL } else { 0 },
                ..GpuObject::default()
            }
        };
        let drawable: Vec<usize> = (0..input.commands.len()).filter(|&i| input.keys[i].is_some()).collect();
        let mut plan = FramePlan {
            batches: Vec::new(),
            objects: Vec::new(),
            groups: Vec::new(),
            has_opaque: false,
            order_len: 0,
            args_len: 0,
            padded_max: 0,
        };

        // 2D と 3D の不透明：Object ごとに引数を 1 つ
        let opaque_order: Vec<usize> = match manager.dimension {
            Dimension::Two => {
                let mut order = drawable.clone();
                order.sort_by_key(|&i| input.commands[i].settings.layer);
                order
            }
            Dimension::Three => group_in_first_order(
                drawable.iter().copied().filter(|&i| input.commands[i].settings.blend == Blend::Opaque),
                |i| (input.keys[i].unwrap(), input.camera_slots[i]),
            )
            .into_iter()
            .flatten()
            .collect(),
        };
        let bucket = if manager.dimension == Dimension::Three { FLAG_BUCKET_SORT } else { 0 };

        for &i in &opaque_order {
            let key = input.keys[i].unwrap();
            let camera = input.camera_slots[i];
            let mut object = object_of(i, bucket);

            object.order_base = plan.order_len;
            object.args_index = plan.args_len;
            plan.order_len += object.instance_count;
            plan.objects.push(object);

            match plan.batches.last_mut() {
                Some(batch) if batch.key == key && batch.camera == camera => batch.args_count += 1,
                _ => plan.batches.push(Batch { key, camera, args_start: plan.args_len, args_count: 1 }),
            }

            plan.args_len += 1;
        }

        plan.has_opaque = !plan.objects.is_empty();

        // 3D の半透明：インスタンスごとに引数を 1 つ。Object をまたいで奥から手前へ並べる
        if manager.dimension == Dimension::Three {
            let transparent = drawable.iter().copied().filter(|&i| input.commands[i].settings.blend != Blend::Opaque);

            for group in group_in_first_order(transparent, |i| (input.keys[i].unwrap(), input.camera_slots[i])) {
                let object_start = plan.objects.len() as u32;
                let mut item_count = 0;

                for &i in &group {
                    let mut object = object_of(i, FLAG_TRANSPARENT);
                    object.sort_base = item_count;
                    item_count += object.instance_count;
                    plan.objects.push(object);
                }

                let first = group[0];
                let padded_count = item_count.next_power_of_two().max(1);

                plan.groups.push(TransparentGroup {
                    object_start,
                    object_count: group.len() as u32,
                    item_count,
                    padded_count,
                    args_base: plan.args_len,
                    order_base: plan.order_len,
                });
                plan.batches.push(Batch {
                    key: input.keys[first].unwrap(),
                    camera: input.camera_slots[first],
                    args_start: plan.args_len,
                    args_count: item_count,
                });
                plan.args_len += item_count;
                plan.order_len += item_count;
                plan.padded_max = plan.padded_max.max(padded_count);
            }
        }

        plan
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    // Compute の各ディスパッチの Params。最初の 1 つが cull_compact 用
    fn params(&self, dimension: Dimension) -> Vec<(GpuParams, Dispatch)> {
        let (instance_words, visibility_word, is_3d) = match dimension {
            Dimension::Two => (12, 9, 0),
            Dimension::Three => (24, 20, 1),
        };
        let base = GpuParams { instance_words, visibility_word, is_3d, object_count: self.objects.len() as u32, ..Default::default() };
        let mut params = Vec::new();

        if self.has_opaque {
            params.push((base, Dispatch::CullCompact(self.objects.len() as u32)));
        }

        for group in &self.groups {
            let group_params = GpuParams {
                group_object_start: group.object_start,
                group_object_count: group.object_count,
                item_count: group.item_count,
                padded_count: group.padded_count,
                args_base: group.args_base,
                order_base: group.order_base,
                ..base
            };
            let workgroups = group.padded_count.div_ceil(WORKGROUP_SIZE);

            params.push((group_params, Dispatch::TransparentKeys(workgroups)));

            let mut k = 2;
            while k <= group.padded_count {
                let mut j = k / 2;
                while j > 0 {
                    params.push((GpuParams { k, j, ..group_params }, Dispatch::BitonicStep(workgroups)));
                    j /= 2;
                }
                k *= 2;
            }

            params.push((group_params, Dispatch::TransparentEmit(group.item_count.div_ceil(WORKGROUP_SIZE))));
        }

        params
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum Dispatch {
    CullCompact(u32),
    TransparentKeys(u32),
    BitonicStep(u32),
    TransparentEmit(u32),
}

// key で、最初に現れた順にまとめる
fn group_in_first_order<K: std::hash::Hash + Eq>(items: impl Iterator<Item = usize>, key: impl Fn(usize) -> K) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut index: HashMap<K, usize> = HashMap::new();

    for item in items {
        let position = *index.entry(key(item)).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[position].push(item);
    }

    groups
}

// 大きさが足りなければ作り直すバッファ。中身は毎フレーム書き直す
pub(crate) struct GrowBuffer {
    pub buffer: wgpu::Buffer,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl GrowBuffer {
    fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Self { buffer: create(device, label, 256, usage), usage, label }
    }

    fn ensure(&mut self, device: &wgpu::Device, size: u64) {
        if self.buffer.size() < size {
            self.buffer = create(device, self.label, size.next_power_of_two(), self.usage);
        }
    }
}

fn create(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(256), usage, mapped_at_creation: false })
}

// DrawManager* が持ち、使い回す GPU のバッファ
pub(crate) struct DrawResources {
    pub objects: GrowBuffer,
    pub cameras: GrowBuffer,
    pub camera_culls: GrowBuffer,
    pub order: GrowBuffer,
    pub args: GrowBuffer,
    pub keys: GrowBuffer,
    pub values: GrowBuffer,
    pub params: GrowBuffer,
    // 前のフレームのカメラ（TAA の動きのベクトル用、§11 シェーダーとの約束事）
    pub previous_cameras: GrowBuffer,
    last_camera_bytes: Vec<u8>,
    // テストで、そのフレームのまとまり（引数の先頭と数）を確かめる
    #[cfg(test)]
    pub last_batches: Vec<(u32, u32)>,
}

impl DrawResources {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        use wgpu::BufferUsages as U;

        Self {
            objects: GrowBuffer::new(device, "Lonas objects", U::STORAGE | U::COPY_DST),
            cameras: GrowBuffer::new(device, "Lonas cameras", U::UNIFORM | U::COPY_DST),
            camera_culls: GrowBuffer::new(device, "Lonas camera culls", U::STORAGE | U::COPY_DST),
            order: GrowBuffer::new(device, "Lonas instance order", U::STORAGE),
            args: GrowBuffer::new(device, "Lonas draw args", U::STORAGE | U::INDIRECT | U::COPY_SRC),
            keys: GrowBuffer::new(device, "Lonas sort keys", U::STORAGE),
            values: GrowBuffer::new(device, "Lonas sort values", U::STORAGE),
            params: GrowBuffer::new(device, "Lonas cull params", U::UNIFORM | U::COPY_DST),
            previous_cameras: GrowBuffer::new(device, "Lonas previous cameras", U::UNIFORM | U::COPY_DST),
            last_camera_bytes: Vec::new(),
            #[cfg(test)]
            last_batches: Vec::new(),
        }
    }
}

pub(crate) struct Strides {
    pub camera: u64,
    pub params: u64,
}

// バッファに書き込み、Compute を記録する（end_frame の「4. カメラ」「5. Compute」）
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_compute(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    culling: &super::Culling,
    resources: &mut DrawResources,
    plan: &FramePlan,
    cameras: &[Option<CameraFrame>],
    instance_buffer: &wgpu::Buffer,
    dimension: Dimension,
    strides: &Strides,
) {
    // カメラ：描画用（uniform、動的オフセット）と視界の判定用
    let mut camera_bytes = vec![0u8; cameras.len() * strides.camera as usize];
    let mut culls = vec![GpuCameraCull::default(); cameras.len()];

    for (slot, frame) in cameras.iter().enumerate() {
        if let Some(frame) = frame {
            let start = slot * strides.camera as usize;
            camera_bytes[start..start + size_of::<crate::draw::GpuCamera>()].copy_from_slice(bytemuck::bytes_of(&frame.camera));
            culls[slot] = GpuCameraCull {
                view_projection: frame.camera.view_projection_no_jitter,
                depth_range: [frame.depth_range[0], frame.depth_range[1], 0.0, 0.0],
            };
        }
    }

    let params = plan.params(dimension);
    let mut params_bytes = vec![0u8; params.len().max(1) * strides.params as usize];

    for (i, (param, _)) in params.iter().enumerate() {
        let start = i * strides.params as usize;
        params_bytes[start..start + size_of::<GpuParams>()].copy_from_slice(bytemuck::bytes_of(param));
    }

    resources.objects.ensure(device, (plan.objects.len() * size_of::<GpuObject>()) as u64);
    resources.cameras.ensure(device, camera_bytes.len() as u64);
    resources.camera_culls.ensure(device, (culls.len() * size_of::<GpuCameraCull>()) as u64);
    resources.order.ensure(device, plan.order_len as u64 * 4);
    resources.args.ensure(device, plan.args_len as u64 * ARGS_SIZE);
    resources.keys.ensure(device, plan.padded_max as u64 * 4);
    resources.values.ensure(device, plan.padded_max as u64 * 4);
    resources.params.ensure(device, params_bytes.len() as u64);

    queue.write_buffer(&resources.objects.buffer, 0, bytemuck::cast_slice(&plan.objects));
    queue.write_buffer(&resources.cameras.buffer, 0, &camera_bytes);

    // 前のフレームのカメラ。初めてのフレームは現在と同じ
    let previous = if resources.last_camera_bytes.len() == camera_bytes.len() { &resources.last_camera_bytes } else { &camera_bytes };
    resources.previous_cameras.ensure(device, previous.len() as u64);
    queue.write_buffer(&resources.previous_cameras.buffer, 0, previous);
    resources.last_camera_bytes = camera_bytes;
    queue.write_buffer(&resources.camera_culls.buffer, 0, bytemuck::cast_slice(&culls));
    queue.write_buffer(&resources.params.buffer, 0, &params_bytes);

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Lonas cull"),
        layout: &culling.layout,
        entries: &[
            entry(0, &resources.objects.buffer),
            entry(1, instance_buffer),
            entry(2, &resources.camera_culls.buffer),
            entry(3, &resources.order.buffer),
            entry(4, &resources.args.buffer),
            entry(5, &resources.keys.buffer),
            entry(6, &resources.values.buffer),
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &resources.params.buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<GpuParams>() as u64),
                }),
            },
        ],
    });

    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Lonas cull"), timestamp_writes: None });

    for (i, (_, dispatch)) in params.iter().enumerate() {
        let (pipeline, workgroups) = match *dispatch {
            Dispatch::CullCompact(n) => (&culling.cull_compact, n),
            Dispatch::TransparentKeys(n) => (&culling.transparent_keys, n),
            Dispatch::BitonicStep(n) => (&culling.bitonic_step, n),
            Dispatch::TransparentEmit(n) => (&culling.transparent_emit, n),
        };

        if workgroups == 0 {
            continue;
        }

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[(i as u64 * strides.params) as u32]);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
}

// 描画コマンドのパイプラインのキー。シェーダーが未指定なら None（描かない）
// 描画先の構成（§10 パイプラインのキャッシュ）
#[derive(Copy, Clone, Debug)]
pub(crate) struct TargetFormat {
    pub color: wgpu::TextureFormat,
    pub sample_count: u32,
    pub motion: Option<wgpu::TextureFormat>,
}

pub(crate) fn pipeline_key(command: &DrawCommand, manager: &DrawManagerCore, shader: Option<ShaderId>, target: TargetFormat) -> Option<PipelineKey> {
    let shader = shader?;
    let depth = match (manager.dimension, command.settings.blend) {
        (Dimension::Two, _) => DepthMode::None,
        (Dimension::Three, Blend::Opaque) => DepthMode::Opaque,
        (Dimension::Three, _) => DepthMode::Transparent,
    };

    Some(PipelineKey {
        shader,
        dimension: manager.dimension,
        blend: command.settings.blend,
        cull_mode: command.settings.cull_mode,
        depth,
        layout: manager.layout,
        front_face: front_face(manager),
        color_format: target.color,
        sample_count: target.sample_count,
        motion: target.motion,
    })
}

// 表の向き（§11 背面カリング）。画面上で反時計回りに見える面を表にする
// 3D の右手系・2D の y が上向きの原点では反時計回り、3D の左手系・2D の左上の原点では時計回りが表
fn front_face(manager: &DrawManagerCore) -> wgpu::FrontFace {
    use crate::config::{Handedness, Origin2D};

    let clockwise = match manager.dimension {
        Dimension::Two => manager.coordinate_system.origin_2d == Origin2D::TopLeft,
        Dimension::Three => manager.coordinate_system.handedness_3d == Handedness::LeftHanded,
    };

    if clockwise { wgpu::FrontFace::Cw } else { wgpu::FrontFace::Ccw }
}

pub(crate) const fn args_offset(index: u32) -> u64 {
    index as u64 * ARGS_SIZE
}
