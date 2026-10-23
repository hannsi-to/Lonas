// GPU 上のヒープ（§6）。用途ごとのメモリ空間を 1 本の GPU バッファとして持つ
// 領域の割り当ての計算は memory、GPU のバッファの作成・コピーはここで行う

use crate::config::LonasConfig;
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
    label: &'static str,
}

pub(crate) struct Heap {
    spaces: [HeapSpace; 3],
    // Static 領域の詰め直しに使う一時バッファ。同じバッファ内ではコピーできないため（§6）
    temporary: Option<wgpu::Buffer>,
    compaction_threshold: f32,
}

impl Heap {
    pub(crate) fn new(device: &wgpu::Device, config: &LonasConfig) -> Self {
        let memory = &config.memory;
        let growth = config.dynamic_growth_factor;
        let space = |memory: MemorySpace, usage: wgpu::BufferUsages, label: &'static str| {
            let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
            let buffer = create_buffer(device, label, memory.buffer_size(), usage);

            HeapSpace { memory, gc: GcTracker::new(), buffer, usage, label }
        };

        Self {
            spaces: [
                space(
                    MemorySpace::new(memory.vertex.static_capacity, memory.vertex.dynamic_initial_capacity, growth),
                    wgpu::BufferUsages::VERTEX,
                    "Lonas vertex heap",
                ),
                space(
                    MemorySpace::new(memory.index.static_capacity, memory.index.dynamic_initial_capacity, growth),
                    wgpu::BufferUsages::INDEX,
                    "Lonas index heap",
                ),
                // インスタンスは常に Dynamic（§6 インスタンスの領域）
                space(
                    MemorySpace::new(0, memory.instance_dynamic_initial_capacity, growth),
                    wgpu::BufferUsages::STORAGE,
                    "Lonas instance heap",
                ),
            ],
            temporary: None,
            compaction_threshold: config.static_compaction_threshold,
        }
    }

    #[allow(dead_code)] // 段階 6（描画）で使う
    pub(crate) fn buffer(&self, kind: SpaceKind) -> &wgpu::Buffer {
        &self.spaces[kind.index()].buffer
    }

    #[allow(dead_code)] // 段階 6（描画）で使う
    pub(crate) fn offset(&self, kind: SpaceKind, handle: GpuAllocation) -> u64 {
        self.spaces[kind.index()].memory.offset(handle)
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
                    HeapRequest::Allocate { handle, space, size, update } => {
                        let space = &mut self.spaces[space.index()];
                        space.memory.allocate(handle, size, update);
                        space.gc.touch(handle, frame);
                    }
                    HeapRequest::Reallocate { handle, space, size, update } => {
                        let space = &mut self.spaces[space.index()];
                        space.memory.reallocate(handle, size, update);
                        space.gc.touch(handle, frame);
                    }
                    HeapRequest::Write { .. } => {}
                }
            }

            for &(space, handle) in &manager.touched {
                self.spaces[space.index()].gc.touch(handle, frame);
            }

            // GPU のコピーの前に map を解除する
            manager.staging.unmap_all();
        }

        for space in &mut self.spaces {
            space.memory.compact_if_needed(self.compaction_threshold);
            self.temporary = grow_buffer(device, encoder, space, self.temporary.take());
        }

        for manager in managers.iter() {
            for request in &manager.heap_requests {
                if let HeapRequest::Write { handle, space, offset, chunk, ref range } = *request {
                    let space = &self.spaces[space.index()];
                    let destination = space.memory.offset(handle) + offset;

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
        for space in &mut self.spaces {
            for handle in space.gc.collect(frame, unused_frames) {
                space.memory.free(handle);
            }
        }
    }

    pub(crate) fn take_reclaimed(&mut self, manager: ManagerId) -> Vec<GpuAllocation> {
        self.spaces.iter_mut().flat_map(|space| space.gc.take_reclaimed(manager)).collect()
    }

    // テストで GPU 上の中身を読み戻す
    #[cfg(test)]
    pub(crate) fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue, kind: SpaceKind, offset: u64, size: u64) -> Vec<u8> {
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lonas readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());

        encoder.copy_buffer_to_buffer(self.buffer(kind), offset, &readback, 0, size);
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
    let needed = buffer_size(space.memory.buffer_size());

    if needed > space.buffer.size() {
        let buffer = create_buffer(device, space.label, needed, space.usage);

        encoder.copy_buffer_to_buffer(&space.buffer, 0, &buffer, 0, space.buffer.size());
        space.buffer = buffer;
    }

    let mut temporary = temporary;

    for moves in space.memory.take_moves() {
        let total: u64 = moves.iter().map(|m| m.size).sum();

        // 一時バッファは使い回し、移す量が容量を超えた場合だけ作り直す
        if temporary.as_ref().is_none_or(|buffer| buffer.size() < total) {
            temporary = Some(create_buffer(device, "Lonas compaction", total, wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST));
        }

        let temporary = temporary.as_ref().unwrap();
        let mut cursor = 0;

        for &Move { from, size, .. } in &moves {
            encoder.copy_buffer_to_buffer(&space.buffer, from, temporary, cursor, size);
            cursor += size;
        }

        cursor = 0;

        for &Move { to, size, .. } in &moves {
            encoder.copy_buffer_to_buffer(temporary, cursor, &space.buffer, to, size);
            cursor += size;
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
