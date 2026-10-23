// 用途ごとのメモリ空間（§6 メモリ空間の配置・用途ごとのメモリ空間）
// 手前が Static、後ろが Dynamic。ハンドルから実際の位置への対応表を持ち、詰め直しや移動をしてもハンドルは変わらない

use std::collections::HashMap;

use super::static_stack::{Move, StaticId, StaticStack};
use super::tlsf::{BlockId, TlsfAllocator};
use super::{GpuAllocation, MemoryUpdateType};

// GPU のコピー（copy_buffer_to_buffer）の制約に合わせる
const ALIGNMENT: u64 = 4;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Placement {
    Static(StaticId),
    Dynamic(BlockId),
}

// Dynamic 領域を広げたときの知らせ。GPU のバッファを作り直してコピーする（§6 Dynamic 領域の拡張）
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Grown {
    pub old_buffer_size: u64,
    pub new_buffer_size: u64,
}

#[derive(Debug)]
pub(crate) struct MemorySpace {
    static_stack: StaticStack,
    dynamic: TlsfAllocator,
    growth_factor: f32,
    placements: HashMap<GpuAllocation, Placement>,
}

impl MemorySpace {
    pub(crate) fn new(static_capacity: u64, dynamic_initial_capacity: u64, growth_factor: f32) -> Self {
        assert!(growth_factor > 1.0, "dynamic_growth_factor は 1.0 より大きいこと");

        Self {
            static_stack: StaticStack::new(static_capacity, ALIGNMENT),
            dynamic: TlsfAllocator::new(dynamic_initial_capacity, ALIGNMENT),
            growth_factor,
            placements: HashMap::new(),
        }
    }

    // GPU のバッファの大きさ（Static + Dynamic）
    pub(crate) fn buffer_size(&self) -> u64 {
        self.static_stack.capacity() + self.dynamic.capacity()
    }

    pub(crate) fn contains(&self, handle: GpuAllocation) -> bool {
        self.placements.contains_key(&handle)
    }

    // Static は容量を超えると panic。Dynamic は足りなければ広げ、広げた場合は Grown を返す
    pub(crate) fn allocate(&mut self, handle: GpuAllocation, size: u64, update: MemoryUpdateType) -> Option<Grown> {
        assert!(!self.contains(handle), "確保済みのハンドルを確保しました");

        let (placement, grown) = self.place(size, update);
        self.placements.insert(handle, placement);

        grown
    }

    pub(crate) fn free(&mut self, handle: GpuAllocation) {
        let placement = self.placements.remove(&handle).expect("確保されていないハンドルを解放しました");

        self.release(placement);
    }

    // 大きさや memory_update_type が変わったときに確保し直す（§6 サイズ変更・確保後の memory_update_type の変更）
    // 中身は Object* の CPU 側の写しから書き直すため、古い領域からはコピーしない
    pub(crate) fn reallocate(&mut self, handle: GpuAllocation, size: u64, update: MemoryUpdateType) -> Option<Grown> {
        let old = self.placements.remove(&handle).expect("確保されていないハンドルを確保し直しました");
        let (placement, grown) = self.place(size, update);

        self.placements.insert(handle, placement);
        self.release(old);

        grown
    }

    // メモリ空間の先頭からの位置
    pub(crate) fn offset(&self, handle: GpuAllocation) -> u64 {
        match self.placement(handle) {
            Placement::Static(id) => self.static_stack.offset(id),
            Placement::Dynamic(id) => self.static_stack.capacity() + self.dynamic.offset(id),
        }
    }

    pub(crate) fn size(&self, handle: GpuAllocation) -> u64 {
        match self.placement(handle) {
            Placement::Static(id) => self.static_stack.size(id),
            Placement::Dynamic(id) => self.dynamic.size(id),
        }
    }

    pub(crate) fn update_type(&self, handle: GpuAllocation) -> MemoryUpdateType {
        match self.placement(handle) {
            Placement::Static(_) => MemoryUpdateType::Static,
            Placement::Dynamic(_) => MemoryUpdateType::Dynamic,
        }
    }

    // 穴がしきい値を超えていれば Static 領域を詰め直し、移す範囲を返す（§6 Static 領域のコンパクション）
    // Static 領域はメモリ空間の先頭にあるため、Move の位置はそのままメモリ空間の位置
    pub(crate) fn compact_if_needed(&mut self, threshold: f32) -> Vec<Move> {
        if self.static_stack.needs_compaction(threshold) {
            self.static_stack.compact()
        } else {
            Vec::new()
        }
    }

    fn place(&mut self, size: u64, update: MemoryUpdateType) -> (Placement, Option<Grown>) {
        match update {
            MemoryUpdateType::Static => (Placement::Static(self.static_stack.allocate(size)), None),
            MemoryUpdateType::Dynamic => {
                if let Some(id) = self.dynamic.allocate(size) {
                    return (Placement::Dynamic(id), None);
                }

                let old_buffer_size = self.buffer_size();
                let capacity = self.dynamic.capacity();
                // サイズクラスの境界まで切り上げた分を足せば、広げた後の確保は必ず成功する
                let needed = self.dynamic.request_size(size);
                let new_capacity = ((capacity as f64 * self.growth_factor as f64) as u64).max(capacity + needed);

                self.dynamic.grow(new_capacity - capacity);

                let id = self.dynamic.allocate(size).expect("Dynamic 領域を広げた後の確保に失敗しました");
                let grown = Grown { old_buffer_size, new_buffer_size: self.buffer_size() };

                (Placement::Dynamic(id), Some(grown))
            }
        }
    }

    fn release(&mut self, placement: Placement) {
        match placement {
            Placement::Static(id) => self.static_stack.free(id),
            Placement::Dynamic(id) => self.dynamic.free(id),
        }
    }

    fn placement(&self, handle: GpuAllocation) -> Placement {
        *self.placements.get(&handle).expect("確保されていないハンドルです")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ManagerId;
    use crate::memory::tlsf::tests::Rng;

    fn handle(serial: u32) -> GpuAllocation {
        GpuAllocation { manager: ManagerId(0), serial }
    }

    #[test]
    fn dynamic_offsets_start_after_static_region() {
        let mut space = MemorySpace::new(1024, 1024, 2.0);

        space.allocate(handle(0), 100, MemoryUpdateType::Static);
        space.allocate(handle(1), 100, MemoryUpdateType::Dynamic);

        assert_eq!(space.offset(handle(0)), 0);
        assert_eq!(space.offset(handle(1)), 1024);
        assert_eq!(space.buffer_size(), 2048);
    }

    #[test]
    fn dynamic_grows_by_factor_or_needed_size() {
        let mut space = MemorySpace::new(0, 1024, 1.5);

        space.allocate(handle(0), 1000, MemoryUpdateType::Dynamic);
        // 倍率で広げれば足りる
        let grown = space.allocate(handle(1), 100, MemoryUpdateType::Dynamic).unwrap();
        assert_eq!(grown, Grown { old_buffer_size: 1024, new_buffer_size: 1536 });
        // 倍率では足りないため、必要な分だけ広げる
        let grown = space.allocate(handle(2), 10_000, MemoryUpdateType::Dynamic).unwrap();
        assert!(grown.new_buffer_size >= 1536 + 10_000);
        // 既存の位置は変わらない
        assert_eq!(space.offset(handle(0)), 0);
    }

    #[test]
    fn reallocate_moves_between_static_and_dynamic() {
        let mut space = MemorySpace::new(1024, 1024, 2.0);

        space.allocate(handle(0), 100, MemoryUpdateType::Static);
        space.reallocate(handle(0), 100, MemoryUpdateType::Dynamic);
        assert_eq!(space.update_type(handle(0)), MemoryUpdateType::Dynamic);
        assert!(space.offset(handle(0)) >= 1024);

        space.reallocate(handle(0), 200, MemoryUpdateType::Static);
        assert_eq!(space.update_type(handle(0)), MemoryUpdateType::Static);
        assert_eq!((space.offset(handle(0)), space.size(handle(0))), (0, 200));
    }

    #[test]
    fn compaction_keeps_handles_valid() {
        let mut space = MemorySpace::new(1000, 0, 2.0);

        for serial in 0..4 {
            space.allocate(handle(serial), 100, MemoryUpdateType::Static);
        }

        space.free(handle(1));
        assert!(space.compact_if_needed(0.5).is_empty()); // 穴は 10%
        assert_eq!(space.compact_if_needed(0.05), vec![Move { from: 200, to: 100, size: 200 }]);
        assert_eq!([space.offset(handle(0)), space.offset(handle(2)), space.offset(handle(3))], [0, 100, 200]);
    }

    #[test]
    fn random_mixed_usage_never_overlaps() {
        let mut rng = Rng::new(7);
        let mut space = MemorySpace::new(1 << 20, 4 * 1024, 1.5); // Static は容量を超えると panic するため大きめにする
        let mut live: Vec<GpuAllocation> = Vec::new();

        for serial in 0..1500 {
            match rng.below(4) {
                0 | 1 => {
                    let update = if rng.below(2) == 0 { MemoryUpdateType::Static } else { MemoryUpdateType::Dynamic };
                    space.allocate(handle(serial), 4 + rng.below(256), update);
                    live.push(handle(serial));
                }
                2 if !live.is_empty() => {
                    space.free(live.swap_remove(rng.below(live.len() as u64) as usize));
                }
                _ if !live.is_empty() => {
                    let target = live[rng.below(live.len() as u64) as usize];
                    let update = if rng.below(2) == 0 { MemoryUpdateType::Static } else { MemoryUpdateType::Dynamic };
                    space.reallocate(target, 4 + rng.below(256), update);
                }
                _ => {}
            }

            space.compact_if_needed(0.2);

            let mut ranges: Vec<(u64, u64)> = live.iter().map(|&h| (space.offset(h), space.size(h))).collect();
            ranges.sort();

            for pair in ranges.windows(2) {
                assert!(pair[0].0 + pair[0].1 <= pair[1].0, "領域が重なっています");
            }

            assert!(ranges.last().is_none_or(|&(offset, size)| offset + size <= space.buffer_size()));
        }
    }
}
