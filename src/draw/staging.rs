// map 済みの staging バッファ（§6 転送）
// Object*::end が書き込み、end_frame で copy_buffer_to_buffer によってヒープへコピーする

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Chunk {
    buffer: wgpu::Buffer,
    size: u64,
    used: u64,
}

// 使い終わったチャンクを map し直している間、完了を待つ
struct InFlight {
    chunk: Chunk,
    ready: Arc<AtomicBool>,
}

pub(crate) struct StagingBelt {
    device: wgpu::Device,
    chunk_size: u64,
    // そのフレームで書き込み中のチャンク。Write の chunk はこの番号
    active: Vec<Chunk>,
    // map 済みで空いているチャンク
    free: Vec<Chunk>,
    in_flight: Vec<InFlight>,
}

impl StagingBelt {
    pub(crate) fn new(device: wgpu::Device, chunk_size: u64) -> Self {
        Self { device, chunk_size, active: Vec::new(), free: Vec::new(), in_flight: Vec::new() }
    }

    // bytes を書き込み、そのチャンクの番号と範囲を返す
    pub(crate) fn write(&mut self, bytes: &[u8]) -> (u32, Range<u64>) {
        let size = bytes.len() as u64;
        assert!(size > 0 && size.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT), "転送の大きさは 4 の倍数であること");

        let index = self.chunk_with_space(size);
        let chunk = &mut self.active[index];
        let start = chunk.used;
        let range = start..start + size;

        // 次の書き込み位置は map の境界（8 バイト）に揃える
        chunk.used = align_up(range.end, wgpu::MAP_ALIGNMENT);
        chunk
            .buffer
            .slice(range.clone())
            .get_mapped_range_mut()
            .expect("staging バッファが map されていません")
            .slice(..)
            .copy_from_slice(bytes);

        (index as u32, range)
    }

    pub(crate) fn buffer(&self, chunk: u32) -> &wgpu::Buffer {
        &self.active[chunk as usize].buffer
    }

    // GPU のコピーの前に map を解除する（end_frame）
    pub(crate) fn unmap_all(&mut self) {
        for chunk in &self.active {
            chunk.buffer.unmap();
        }
    }

    // submit の後、使ったチャンクを map し直す。完了は次以降の end_frame で確かめる
    pub(crate) fn recall(&mut self) {
        for chunk in self.active.drain(..) {
            let ready = Arc::new(AtomicBool::new(false));
            let signal = ready.clone();

            chunk.buffer.map_async(wgpu::MapMode::Write, .., move |result| {
                if result.is_ok() {
                    signal.store(true, Ordering::Release);
                }
            });

            self.in_flight.push(InFlight { chunk, ready });
        }
    }

    // map し直しが終わったチャンクを空きに戻す（device.poll の後に呼ぶ）
    pub(crate) fn reclaim(&mut self) {
        let (ready, waiting): (Vec<InFlight>, Vec<InFlight>) =
            self.in_flight.drain(..).partition(|flight| flight.ready.load(Ordering::Acquire));

        self.in_flight = waiting;
        self.free.extend(ready.into_iter().map(|flight| Chunk { used: 0, ..flight.chunk }));
    }

    // テストで、書き込んだチャンクの数を確かめる
    #[cfg(test)]
    pub(crate) fn chunk_counts(&self) -> (usize, usize, usize) {
        (self.active.len(), self.free.len(), self.in_flight.len())
    }

    fn chunk_with_space(&mut self, size: u64) -> usize {
        if let Some(index) = self.active.iter().position(|chunk| chunk.size - chunk.used >= size) {
            return index;
        }

        // map し直しが終わっていれば、新しく作る前に空きに戻す
        self.reclaim();

        let chunk = match self.free.iter().position(|chunk| chunk.size >= size) {
            Some(index) => self.free.swap_remove(index),
            None => self.create_chunk(size),
        };

        self.active.push(chunk);
        self.active.len() - 1
    }

    // 空いた map 済みのチャンクがなければ新しく作る。作った時点で map 済みのため待ちが起きない
    // 1 回の書き込みがチャンクより大きい場合は、その大きさの専用のチャンクを作る
    fn create_chunk(&self, size: u64) -> Chunk {
        let size = align_up(size.max(self.chunk_size), wgpu::MAP_ALIGNMENT);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Lonas staging"),
            size,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });

        Chunk { buffer, size, used: 0 }
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}
