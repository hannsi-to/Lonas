// GPU 上のヒープ（§6）。メモリ空間ごとに 1 本の GPU バッファを持つ
// 領域の割り当ての計算は memory（要素の数が単位）、GPU のバッファの作成・コピー（バイトが単位）はここで行う

use std::collections::HashMap;

use crate::config::{LonasConfig, MemoryCapacity};
use crate::draw::{DrawManagerCore, HeapRequest, SpaceKind};
use crate::memory::gc::GcTracker;
use crate::memory::space::MemorySpace;
use crate::memory::static_stack::Move;
use crate::memory::{GpuAllocation, ManagerId};

struct HeapSpace {
    memory: MemorySpace,
    gc: GcTracker,
    buffer: wgpu::Buffer,
    usage: wgpu::BufferUsages,
    // 1 要素の大きさ（バイト）
    unit: u64,
}

pub(crate) struct Heap {
    // 頂点は VertexLayout ごとに、使われたときに作る
    spaces: HashMap<SpaceKind, HeapSpace>,
    vertex_capacity: (u64, u64),
    index_capacity: (u64, u64),
    instance_capacity: u64,
    growth_factor: f32,
    // Static 領域の詰め直しに使う一時バッファ。同じバッファ内ではコピーできないため（§6）
    temporary: Option<wgpu::Buffer>,
    compaction_threshold: f32,
}

impl Heap {
    pub(crate) fn new(config: &LonasConfig) -> Self {
        let memory = &config.memory;
        let capacity = |c: &MemoryCapacity| (c.static_capacity, c.dynamic_initial_capacity);

        Self {
            spaces: HashMap::new(),
            vertex_capacity: capacity(&memory.vertex),
            index_capacity: capacity(&memory.index),
            instance_capacity: memory.instance_dynamic_initial_capacity,
            growth_factor: config.dynamic_growth_factor,
            temporary: None,
            compaction_threshold: config.static_compaction_threshold,
        }
    }

    pub(crate) fn buffer(&self, kind: SpaceKind) -> Option<&wgpu::Buffer> {
        self.spaces.get(&kind).map(|space| &space.buffer)
    }

    // 領域の先頭の位置（要素の数）
    pub(crate) fn offset(&self, kind: SpaceKind, handle: GpuAllocation) -> u64 {
        self.spaces[&kind].memory.offset(handle)
    }

    // LonasConfig の容量はバイトのため、要素の数に直してメモリ空間を作る
    fn space(&mut self, device: &wgpu::Device, kind: SpaceKind) -> &mut HeapSpace {
        let ((static_capacity, dynamic_capacity), usage) = match kind {
            SpaceKind::Vertex(_) => (self.vertex_capacity, wgpu::BufferUsages::VERTEX),
            SpaceKind::Index => (self.index_capacity, wgpu::BufferUsages::INDEX),
            // インスタンスは常に Dynamic（§6 インスタンスの領域）
            SpaceKind::Instance2D | SpaceKind::Instance3D => ((0, self.instance_capacity), wgpu::BufferUsages::STORAGE),
        };
        let growth = self.growth_factor;

        self.spaces.entry(kind).or_insert_with(|| {
            let unit = kind.unit();
            let memory = MemorySpace::new(static_capacity / unit, dynamic_capacity / unit, growth);
            let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
            let buffer = create_buffer(device, "Lonas heap", memory.buffer_size() * unit, usage);

            HeapSpace { memory, gc: GcTracker::new(), buffer, usage, unit }
        })
    }

    // end_frame の「3. ヒープの処理」（§9）
    // 確保・解放 → Dynamic の拡張（バッファの作り直し）→ Static の詰め直し → staging からのコピー
    pub(crate) fn process(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        managers: &mut [&mut DrawManagerCore],
        frame: u64,
    ) {
        for manager in managers.iter_mut() {
            for request in &manager.heap_requests {
                match *request {
                    HeapRequest::Allocate { handle, space, count, update } => {
                        let space = self.space(device, space);
                        space.memory.allocate(handle, count, update);
                        space.gc.touch(handle, frame);
                    }
                    HeapRequest::Reallocate { handle, space, count, update } => {
                        let space = self.space(device, space);
                        space.memory.reallocate(handle, count, update);
                        space.gc.touch(handle, frame);
                    }
                    HeapRequest::Write { .. } => {}
                }
            }

            for &(space, handle) in &manager.touched {
                self.space(device, space).gc.touch(handle, frame);
            }

            // GPU のコピーの前に map を解除する
            manager.staging.unmap_all();
        }

        for space in self.spaces.values_mut() {
            space.memory.compact_if_needed(self.compaction_threshold);
            self.temporary = grow_buffer(device, encoder, space, self.temporary.take());
        }

        for manager in managers.iter() {
            for request in &manager.heap_requests {
                if let HeapRequest::Write { handle, space, offset, chunk, ref range } = *request {
                    let space = &self.spaces[&space];
                    let destination = (space.memory.offset(handle) + offset) * space.unit;

                    encoder.copy_buffer_to_buffer(
                        manager.staging.buffer(chunk),
                        range.start,
                        &space.buffer,
                        destination,
                        range.end - range.start,
                    );
                }
            }
        }
    }

    // end_frame の「8. 後片付け」の GC（§6 解放：GC 方式）。回収したハンドルは DrawManager* ごとに溜まる
    pub(crate) fn collect_garbage(&mut self, frame: u64, unused_frames: u32) {
        for space in self.spaces.values_mut() {
            for handle in space.gc.collect(frame, unused_frames) {
                space.memory.free(handle);
            }
        }
    }

    pub(crate) fn take_reclaimed(&mut self, manager: ManagerId) -> Vec<GpuAllocation> {
        self.spaces.values_mut().flat_map(|space| space.gc.take_reclaimed(manager)).collect()
    }

    // テストで GPU 上の中身を読み戻す。offset と count は要素の数
    #[cfg(test)]
    pub(crate) fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue, kind: SpaceKind, offset: u64, count: u64) -> Vec<u8> {
        let (offset, size) = (offset * kind.unit(), count * kind.unit());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lonas readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());

        encoder.copy_buffer_to_buffer(self.buffer(kind).unwrap(), offset, &readback, 0, size);
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

        let bytes = readback.get_mapped_range(..).unwrap().to_vec();
        bytes
    }
}

// メモリ空間が広がっていれば、より大きなバッファを作って既存の内容をコピーし、差し替える（§6 Dynamic 領域の拡張）
// その後、詰め直しの範囲を一時バッファ経由でコピーする（§6 Static 領域のコンパクション）
fn grow_buffer(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    space: &mut HeapSpace,
    temporary: Option<wgpu::Buffer>,
) -> Option<wgpu::Buffer> {
    let unit = space.unit;
    let needed = buffer_size(space.memory.buffer_size() * unit);

    if needed > space.buffer.size() {
        let buffer = create_buffer(device, "Lonas heap", needed, space.usage);

        encoder.copy_buffer_to_buffer(&space.buffer, 0, &buffer, 0, space.buffer.size());
        space.buffer = buffer;
    }

    let mut temporary = temporary;

    for moves in space.memory.take_moves() {
        let total: u64 = moves.iter().map(|m| m.size * unit).sum();

        // 一時バッファは使い回し、移す量が容量を超えた場合だけ作り直す
        if temporary.as_ref().is_none_or(|buffer| buffer.size() < total) {
            let usage = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
            temporary = Some(create_buffer(device, "Lonas compaction", total, usage));
        }

        let temporary = temporary.as_ref().unwrap();
        let mut cursor = 0;

        for &Move { from, size, .. } in &moves {
            encoder.copy_buffer_to_buffer(&space.buffer, from * unit, temporary, cursor, size * unit);
            cursor += size * unit;
        }

        cursor = 0;

        for &Move { to, size, .. } in &moves {
            encoder.copy_buffer_to_buffer(temporary, cursor, &space.buffer, to * unit, size * unit);
            cursor += size * unit;
        }
    }

    temporary
}

// wgpu のバッファは 4 バイトの倍数で、大きさ 0 にはしない
fn buffer_size(size: u64) -> u64 {
    size.max(wgpu::COPY_BUFFER_ALIGNMENT).div_ceil(wgpu::COPY_BUFFER_ALIGNMENT) * wgpu::COPY_BUFFER_ALIGNMENT
}

fn create_buffer(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: buffer_size(size), usage, mapped_at_creation: false })
}
