// Object2D / Object3D の共通部分（§4 Object 内部構造・§2 end 時の更新条件）

use std::collections::HashSet;
use std::ops::Range;

use super::instance_slots::{InstanceData, InstanceSlots, ObjectId};
use super::topology::{self, Topology, Winding};
use crate::config::{Blend, CullMode};
use crate::draw::{CameraId, DrawCommand, DrawManagerCore, DrawSettings, HeapRequest, SpaceKind};
use crate::gpu::GpuContext;
use crate::gpu::frame::FrameClock;
use crate::math::Vector3f;
use crate::memory::{GpuAllocation, ManagerId, MemoryUpdateType};
use crate::resource::{ShaderId, TextureId};
use crate::vertex::{Vertex, VertexLayout};

// 頂点の要素をどこまで書いたか（§10 頂点の並び方の警告）
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum VertexElements {
    Position,
    Color,
    Uv,
    Normal,
}

impl VertexElements {
    fn of(layout: VertexLayout) -> Self {
        match layout {
            VertexLayout::Position => VertexElements::Position,
            VertexLayout::PositionColor => VertexElements::Color,
            VertexLayout::PositionColorUv => VertexElements::Uv,
            VertexLayout::PositionColorUvNormal => VertexElements::Normal,
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct Region {
    handle: GpuAllocation,
    // 頂点・インデックスは数、インスタンスは確保した容量（スロットの数）
    count: usize,
}

#[derive(Copy, Clone, Debug)]
struct Allocations {
    vertex: Region,
    index: Region,
    instance: Region,
    update: MemoryUpdateType,
}

// 型ごとに違う end の値
pub(crate) struct EndContext {
    pub camera: CameraId,
    pub layer: Option<i32>,
    // bounding_circle / bounding_sphere で指定した境界（中心 xyz と半径）。None なら頂点から求める
    pub bounds: Option<[f32; 4]>,
}

pub(crate) struct ObjectCore<I> {
    id: ObjectId,
    frame: FrameClock,
    redundant_call_warning: bool,
    instance_growth_warning_frames: Option<u32>,
    winding: Winding,

    // 設定。None は未指定（§3・§4）
    pub shader: Option<ShaderId>,
    pub texture: Option<TextureId>,
    pub camera: Option<CameraId>,
    pub blend: Option<Blend>,
    pub topology: Option<Topology>,
    pub cull_mode: Option<CullMode>,
    pub memory_update_type: Option<MemoryUpdateType>,
    manager: Option<ManagerId>,

    // 頂点・インデックス。GPU に載っている内容の CPU 側の写し（§1・§4）
    vertices: Vec<Vertex>,
    vertex_cursor: usize,
    vertex_dirty: Option<Range<u32>>,
    positions_changed: bool,
    elements_written: VertexElements,
    indices: Vec<u32>,
    index_cursor: usize,
    index_dirty: Option<Range<u32>>,
    explicit_indices: bool,
    // 最後に自動で作ったインデックスの元（topology と頂点数）。None は作っていない
    generated_for: Option<(Topology, usize)>,

    pub instances: InstanceSlots<I>,
    texture_changed: bool,
    pub uniform: Vec<u8>,
    uniform_changed: bool,
    // uniform の配列の要素（シェーダーと番号）。DrawManager* が end の時点で割り当てる（§6 uniform の領域）
    uniform_slot: Option<(ShaderId, u32)>,

    allocations: Option<Allocations>,
    // 頂点から求めた境界（中心 xyz と半径）。頂点の位置が変わったフレームでのみ求め直す（§10）
    auto_bounds: [f32; 4],

    // 一度だけ出す警告（§3・§7・§10）
    warned: HashSet<&'static str>,
    had_leftover: bool,
    previous_live_instances: usize,
    growth_streak: u32,

    // 最後に begin / end を呼んだフレーム番号（§2 begin / end の対応）
    begun_frame: Option<u64>,
    ended_frame: Option<u64>,
}

impl<I: InstanceData> ObjectCore<I> {
    pub(crate) fn new(gpu: &GpuContext, winding: Winding) -> Self {
        let id = ObjectId(gpu.issue_object_id());
        let config = gpu.config();

        Self {
            id,
            frame: gpu.frame_clock(),
            redundant_call_warning: config.redundant_call_warning,
            instance_growth_warning_frames: config.instance_growth_warning_frames,
            winding,
            shader: None,
            texture: None,
            camera: None,
            blend: None,
            topology: None,
            cull_mode: None,
            memory_update_type: None,
            manager: None,
            vertices: Vec::new(),
            vertex_cursor: 0,
            vertex_dirty: None,
            positions_changed: false,
            elements_written: VertexElements::Position,
            indices: Vec::new(),
            index_cursor: 0,
            index_dirty: None,
            explicit_indices: false,
            generated_for: None,
            instances: InstanceSlots::new(id),
            texture_changed: false,
            uniform: Vec::new(),
            uniform_changed: false,
            uniform_slot: None,
            allocations: None,
            auto_bounds: [0.0; 4],
            warned: HashSet::new(),
            had_leftover: false,
            previous_live_instances: 0,
            growth_streak: 0,
            begun_frame: None,
            ended_frame: None,
        }
    }

    // ---- 毎フレーム呼ばれる関数 ----

    pub(crate) fn begin(&mut self) {
        let frame = self.current_frame("begin");

        assert_ne!(self.begun_frame, Some(frame), "同じ Object の begin が 1 フレームに 2 回呼ばれました");

        self.begun_frame = Some(frame);
        self.vertex_cursor = 0;
        self.index_cursor = 0;
        self.explicit_indices = false;
    }

    // 前フレームの値と比較しながら書き込み、変わった位置を記録する（§1・§4）
    pub(crate) fn write_vertex(&mut self, vertex: Vertex, elements: VertexElements) {
        let position = self.vertex_cursor;
        self.vertex_cursor += 1;
        self.elements_written = self.elements_written.max(elements);

        match self.vertices.get_mut(position) {
            Some(old) if *old == vertex => {}
            Some(old) => {
                self.positions_changed |= (old.x, old.y, old.z) != (vertex.x, vertex.y, vertex.z);
                *old = vertex;
                extend_range(&mut self.vertex_dirty, position);
            }
            None => {
                self.vertices.push(vertex);
                self.positions_changed = true;
                extend_range(&mut self.vertex_dirty, position);
            }
        }
    }

    pub(crate) fn write_index(&mut self, index: u32) {
        let position = self.index_cursor;
        self.index_cursor += 1;
        self.explicit_indices = true;

        match self.indices.get_mut(position) {
            Some(old) if *old == index => {}
            Some(old) => {
                *old = index;
                extend_range(&mut self.index_dirty, position);
            }
            None => {
                self.indices.push(index);
                extend_range(&mut self.index_dirty, position);
            }
        }
    }

    // ---- 設定（§3 同値時の扱い）----

    // 同値なら無視して false。redundant_call_warning なら、その設定の関数ごとに一度だけ警告する
    pub(crate) fn set<T: PartialEq>(&mut self, name: &'static str, field: fn(&mut Self) -> &mut Option<T>, value: T) -> bool {
        if field(self).as_ref() == Some(&value) {
            self.warn_redundant(name);
            return false;
        }

        *field(self) = Some(value);
        true
    }

    pub(crate) fn warn_redundant(&mut self, name: &'static str) {
        if self.redundant_call_warning && self.warned.insert(name) {
            log::warn!("{name}() が同じ値で呼ばれました。毎フレーム呼ぶ必要はありません（この Object では以降警告しません）");
        }
    }

    pub(crate) fn set_texture(&mut self, texture: TextureId) {
        if self.set("texture", |core| &mut core.texture, texture) {
            // texture_slot を全インスタンスで書き直す（§2 end 時の更新条件）
            self.texture_changed = true;
        }
    }

    // 前の値とバイト列で比較し、同値なら無視する。警告は出さない（§11 uniform）
    pub(crate) fn set_uniform(&mut self, bytes: &[u8]) {
        if self.uniform != bytes {
            self.uniform.clear();
            self.uniform.extend_from_slice(bytes);
            self.uniform_changed = true;
        }
    }

    // ---- end（§2 end 時の更新条件）----

    pub(crate) fn end(&mut self, manager: &mut DrawManagerCore, context: EndContext) {
        let frame = self.current_frame("end");

        assert_eq!(self.begun_frame, Some(frame), "このフレームで begin を呼ばずに Object の end が呼ばれました");
        assert_ne!(self.ended_frame, Some(frame), "同じ Object の end が 1 フレームに 2 回呼ばれました");
        self.ended_frame = Some(frame);

        match self.manager {
            None => self.manager = Some(manager.id),
            Some(id) => assert_eq!(id, manager.id, "Object が初回とは別の DrawManager に渡されました"),
        }

        let update = self.memory_update_type.expect("memory_update_type() を呼ばずに Object の end が呼ばれました");

        manager.prepare_for_end(frame);
        self.finish_vertices();

        // 指定がなく、culling を行う場合だけ求める（2D で culling_2d が false なら求めない）
        if self.positions_changed && context.bounds.is_none() && manager.culling {
            self.auto_bounds = bounding_sphere(&self.vertices);
        }

        self.finish_indices();
        self.warn_vertex_elements(manager.layout);
        self.warn_instance_growth();

        if self.texture_changed {
            self.instances.mark_all_dirty();
            self.texture_changed = false;
        }

        // GC で回収されていたら、初回と同じく確保し直す（§6 回収の通知）
        if let Some(allocations) = self.allocations {
            let regions = [allocations.vertex, allocations.index, allocations.instance];
            let spaces = [SpaceKind::Vertex(manager.layout), SpaceKind::Index, I::SPACE];
            let reclaimed = regions.iter().fold(false, |any, region| manager.take_reclaimed(region.handle) | any);

            if reclaimed {
                self.allocations = None;
                // uniform の要素も DrawManager* が空きに戻している
                self.uniform_slot = None;
            } else {
                // end されたことを GC に知らせる（描かないフレームでも回収されないように）
                manager.touched.extend(spaces.into_iter().zip(regions.map(|region| region.handle)));
            }
        }

        // 頂点・インデックス・インスタンスのどれかが空なら描かない（§10 インスタンスが 0 個の Object）
        if self.vertices.is_empty() || self.indices.is_empty() || self.instances.live_count() == 0 {
            return;
        }

        let allocations = match self.allocations {
            Some(allocations) => self.update_regions(manager, allocations, update),
            None => self.allocate_all(manager, update),
        };

        self.allocations = Some(allocations);
        self.update_uniform(manager, allocations.instance.handle);
        self.write_dirty(manager, allocations);

        manager.draw_commands.push(DrawCommand {
            object: self.id,
            vertex: allocations.vertex.handle,
            index: allocations.index.handle,
            instance: allocations.instance.handle,
            index_count: self.indices.len() as u32,
            instance_count: self.instances.len() as u32,
            settings: DrawSettings {
                shader: self.shader,
                texture: self.texture,
                blend: self.blend.unwrap_or(manager.defaults.blend),
                cull_mode: self.cull_mode.unwrap_or(manager.defaults.cull_mode),
                camera: context.camera,
                layer: context.layer.unwrap_or(manager.defaults.layer),
            },
            bounds: context.bounds.unwrap_or(self.auto_bounds),
        });
    }

    fn current_frame(&self, function: &str) -> u64 {
        let state = self.frame.state();

        assert!(state.in_frame, "フレーム外（begin_frame の前 / end_frame の後）で Object の {function} が呼ばれました");

        state.number
    }

    // 書き込み位置より後ろは、頂点数が減ったので捨てる
    fn finish_vertices(&mut self) {
        if self.vertex_cursor < self.vertices.len() {
            self.vertices.truncate(self.vertex_cursor);
            self.positions_changed = true;
        }
    }

    fn finish_indices(&mut self) {
        if self.explicit_indices {
            self.indices.truncate(self.index_cursor);
            self.generated_for = None;
            return;
        }

        // index() を呼ばなければ topology から作る（§10 インデックス）
        let topology = self.topology.expect("index() も topology() も呼ばずに Object の end が呼ばれました");
        let key = (topology, self.vertices.len());
        let needs_generation =
            self.generated_for != Some(key) || (topology == Topology::Polygon && self.positions_changed);

        if needs_generation {
            let positions: Vec<Vector3f> = self.vertices.iter().map(|v| Vector3f::new(v.x, v.y, v.z)).collect();
            let generated = topology::generate(topology, &positions, self.winding);

            // 頂点数が合わない状態になった時に 1 回だけ警告する（§10）
            if generated.has_leftover && !self.had_leftover {
                log::warn!(
                    "頂点数（{}）が {topology:?} の分け方に合いません。余った頂点は描きません",
                    self.vertices.len(),
                );
            }

            self.had_leftover = generated.has_leftover;
            replace_tracking(&mut self.indices, generated.indices, &mut self.index_dirty);
            self.generated_for = Some(key);
        }

        self.positions_changed = false;
    }

    fn allocate_all(&mut self, manager: &mut DrawManagerCore, update: MemoryUpdateType) -> Allocations {
        let layout = manager.layout;
        let capacity = instance_capacity(self.instances.len(), 0);
        let mut region = |space: SpaceKind, count: usize, update: MemoryUpdateType| {
            let handle = manager.issue_handle();

            manager.heap_requests.push(HeapRequest::Allocate { handle, space, count: count as u64, update });
            Region { handle, count }
        };

        let allocations = Allocations {
            vertex: region(SpaceKind::Vertex(layout), self.vertices.len(), update),
            index: region(SpaceKind::Index, self.indices.len(), update),
            // インスタンスは常に Dynamic（§6 インスタンスの領域）
            instance: region(I::SPACE, capacity, MemoryUpdateType::Dynamic),
            update,
        };

        self.mark_everything_dirty();
        allocations
    }

    // 大きさ・memory_update_type が変わった領域を確保し直す（§2・§6）
    fn update_regions(&mut self, manager: &mut DrawManagerCore, mut allocations: Allocations, update: MemoryUpdateType) -> Allocations {
        let type_changed = allocations.update != update;
        let counts = [self.vertices.len(), self.indices.len()];

        for (region, (count, space)) in [&mut allocations.vertex, &mut allocations.index]
            .into_iter()
            .zip(counts.into_iter().zip([SpaceKind::Vertex(manager.layout), SpaceKind::Index]))
        {
            let size_changed = region.count != count;

            if size_changed && update == MemoryUpdateType::Static && !type_changed {
                panic!(
                    "Static の Object の{}の数が変わりました（{} → {}）。数を変える場合は先に memory_update_type(Dynamic) にしてください",
                    if space == SpaceKind::Index { "インデックス（topology の変更による自動のインデックスを含む）" } else { "頂点" },
                    region.count,
                    count,
                );
            }

            if size_changed || type_changed {
                manager.heap_requests.push(HeapRequest::Reallocate { handle: region.handle, space, count: count as u64, update });
                region.count = count;

                match space {
                    SpaceKind::Index => self.index_dirty = full_range(count),
                    _ => self.vertex_dirty = full_range(count),
                }
            }
        }

        if self.instances.len() > allocations.instance.count {
            let capacity = instance_capacity(self.instances.len(), allocations.instance.count);

            manager.heap_requests.push(HeapRequest::Reallocate {
                handle: allocations.instance.handle,
                space: I::SPACE,
                count: capacity as u64,
                update: MemoryUpdateType::Dynamic,
            });
            allocations.instance.count = capacity;
            self.instances.mark_all_dirty();
        }

        allocations.update = update;
        allocations
    }

    fn mark_everything_dirty(&mut self) {
        self.vertex_dirty = full_range(self.vertices.len());
        self.index_dirty = full_range(self.indices.len());
        self.instances.mark_all_dirty();
    }

    // 変わった範囲だけを staging バッファに書き、Write の要求を積む
    fn write_dirty(&mut self, manager: &mut DrawManagerCore, allocations: Allocations) {
        let mut scratch = std::mem::take(&mut manager.scratch);
        let layout = manager.layout;
        let stride = layout.stride();

        if let Some(range) = self.vertex_dirty.take() {
            scratch.clear();
            scratch.resize((range.end - range.start) as usize * stride, 0);

            for (vertex, dst) in self.vertices[range.start as usize..range.end as usize].iter().zip(scratch.chunks_exact_mut(stride)) {
                vertex.write_packed(layout, dst);
            }

            manager.push_write(allocations.vertex.handle, SpaceKind::Vertex(layout), range.start as u64, &scratch);
        }

        if let Some(range) = self.index_dirty.take() {
            let indices = &self.indices[range.start as usize..range.end as usize];

            manager.push_write(allocations.index.handle, SpaceKind::Index, range.start as u64, bytemuck::cast_slice(indices));
        }

        if let Some(range) = self.instances.take_dirty_range() {
            let texture_slot = self.texture.map_or(0, TextureId::slot);
            // uniform() を呼んでいなければ要素 0（すべて 0、§6 uniform の領域）
            let uniform_index = self.uniform_slot.map_or(0, |(_, slot)| slot);

            scratch.clear();
            scratch.resize((range.end - range.start) as usize * I::GPU_SIZE, 0);

            for (instance, dst) in self.instances.slots()[range.start as usize..range.end as usize]
                .iter()
                .zip(scratch.chunks_exact_mut(I::GPU_SIZE))
            {
                instance.write_gpu(texture_slot, uniform_index, dst);
            }

            manager.push_write(allocations.instance.handle, I::SPACE, range.start as u64, &scratch);
        }

        manager.scratch = scratch;
    }

    // uniform の要素を割り当て、変わった値を書く（§6 uniform の領域・§2 end 時の更新条件）
    // シェーダーを変えた場合は、古いシェーダーの要素を空きに戻し、新しいシェーダーの配列に割り当て直す
    fn update_uniform(&mut self, manager: &mut DrawManagerCore, owner: GpuAllocation) {
        let Some(shader) = self.shader else { return };

        if self.uniform.is_empty() {
            return;
        }

        if self.uniform_slot.map(|(current, _)| current) != Some(shader) {
            if let Some((old_shader, old_slot)) = self.uniform_slot.take() {
                manager.release_uniform_slot(old_shader, old_slot);
            }

            self.uniform_slot = Some((shader, manager.allocate_uniform_slot(shader, owner)));
            self.uniform_changed = true;
            // uniform_index を全インスタンスで書き直す
            self.instances.mark_all_dirty();
        }

        if self.uniform_changed {
            let (shader, slot) = self.uniform_slot.unwrap();
            manager.push_uniform_write(shader, slot, &self.uniform);
            self.uniform_changed = false;
        }
    }

    // VertexLayout に含まれない要素を書いた場合、Object ごとに 1 回だけ警告する（§10 頂点の並び方）
    fn warn_vertex_elements(&mut self, layout: VertexLayout) {
        if self.elements_written > VertexElements::of(layout) && self.warned.insert("vertex_elements") {
            log::warn!("VertexLayout::{layout:?} に含まれない頂点の要素が書かれました。余分な要素は捨てます");
        }
    }

    // インスタンス数が連続で増え続けたら一度だけ警告する（§7 誤用の検知）
    fn warn_instance_growth(&mut self) {
        let live = self.instances.live_count();

        self.growth_streak = if live > self.previous_live_instances { self.growth_streak + 1 } else { 0 };
        self.previous_live_instances = live;

        if let Some(frames) = self.instance_growth_warning_frames
            && self.growth_streak >= frames
            && self.warned.insert("instance_growth")
        {
            log::warn!("インスタンス数が {frames} フレーム連続で増えています。描画ループで毎フレーム add_instance を呼んでいませんか");
        }
    }

    #[cfg(test)]
    pub(crate) fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    #[cfg(test)]
    pub(crate) fn index_values(&self) -> &[u32] {
        &self.indices
    }

    // 頂点 / インデックス / インスタンスのハンドル
    #[cfg(test)]
    pub(crate) fn handles(&self) -> Option<[GpuAllocation; 3]> {
        self.allocations.map(|a| [a.vertex.handle, a.index.handle, a.instance.handle])
    }
}

// 頂点を囲む球（中心は頂点の範囲の中心、半径は最も遠い頂点まで）。最小の球ではないが、1 回の走査で求まる
fn bounding_sphere(vertices: &[Vertex]) -> [f32; 4] {
    let Some(first) = vertices.first() else { return [0.0; 4] };
    let position = |v: &Vertex| Vector3f::new(v.x, v.y, v.z);
    let (min, max) = vertices.iter().fold((position(first), position(first)), |(min, max), v| {
        let p = position(v);
        (Vector3f::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z)), Vector3f::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z)))
    });
    let center = (min + max) * 0.5;
    let radius = vertices.iter().map(|v| (position(v) - center).length()).fold(0.0, f32::max);

    [center.x, center.y, center.z, radius]
}

// インスタンスの領域は余裕を持たせて確保し、超過時のみ確保し直す（§7）
fn instance_capacity(needed: usize, current: usize) -> usize {
    needed.max(current * 2).max(4)
}

fn extend_range(range: &mut Option<Range<u32>>, position: usize) {
    let position = position as u32;

    *range = Some(match range.take() {
        Some(range) => range.start.min(position)..range.end.max(position + 1),
        None => position..position + 1,
    });
}

fn full_range(count: usize) -> Option<Range<u32>> {
    (count > 0).then_some(0..count as u32)
}

// 新しい値に置き換え、変わった位置の最小〜最大を記録する
fn replace_tracking(old: &mut Vec<u32>, new: Vec<u32>, dirty: &mut Option<Range<u32>>) {
    for (position, (a, b)) in old.iter().zip(&new).enumerate() {
        if a != b {
            extend_range(dirty, position);
        }
    }

    for position in old.len().min(new.len())..new.len() {
        extend_range(dirty, position);
    }

    *old = new;
}
