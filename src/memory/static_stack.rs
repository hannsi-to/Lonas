// Static 領域の割り当て（§6 Static / Dynamic の区別・Static 領域のコンパクション）
// 手前から順に積み、途中が不要になったら穴として残す。穴が増えたら詰め直す

use std::collections::BTreeMap;

pub(crate) type StaticId = u32;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Region {
    offset: u64,
    size: u64,
}

// 詰め直しで移す範囲。from / to はメモリ空間の先頭からの位置
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Move {
    pub from: u64,
    pub to: u64,
    pub size: u64,
}

#[derive(Debug)]
pub(crate) struct StaticStack {
    capacity: u64,
    alignment: u64,
    // 番号ごとの領域。None は解放済み（番号は再利用する）
    regions: Vec<Option<Region>>,
    free_ids: Vec<StaticId>,
    // オフセット → 番号。オフセットの順にたどるため
    order: BTreeMap<u64, StaticId>,
    top: u64,
    hole_total: u64,
}

impl StaticStack {
    pub(crate) fn new(capacity: u64, alignment: u64) -> Self {
        Self {
            capacity,
            alignment,
            regions: Vec::new(),
            free_ids: Vec::new(),
            order: BTreeMap::new(),
            top: 0,
            hole_total: 0,
        }
    }

    pub(crate) fn capacity(&self) -> u64 {
        self.capacity
    }

    pub(crate) fn hole_total(&self) -> u64 {
        self.hole_total
    }

    // 容量を超える場合は panic（Object を追加し続けてメモリを圧迫するのを防ぐ、§6）
    pub(crate) fn allocate(&mut self, size: u64) -> StaticId {
        assert!(size > 0, "大きさ 0 の領域は確保できません");

        let size = super::tlsf::align_up(size, self.alignment);

        if self.top + size > self.capacity {
            panic!(
                "Static 領域の容量を超えました（容量 {} バイト、使用中 {} バイト、要求 {} バイト）。\
                 LonasConfig::memory の static_capacity を増やすか、memory_update_type(Dynamic) にしてください",
                self.capacity, self.top, size,
            );
        }

        let region = Region { offset: self.top, size };
        let id = match self.free_ids.pop() {
            Some(id) => {
                self.regions[id as usize] = Some(region);
                id
            }
            None => {
                self.regions.push(Some(region));
                (self.regions.len() - 1) as StaticId
            }
        };

        self.order.insert(region.offset, id);
        self.top += size;

        id
    }

    pub(crate) fn free(&mut self, id: StaticId) {
        let region = self.regions[id as usize].take().expect("存在しない Static の領域です");
        self.order.remove(&region.offset);
        self.free_ids.push(id);

        if region.offset + region.size == self.top {
            // 一番上なら先頭を下げる。下にある穴もまとめて捨てる
            let new_top = self.order.values().next_back().map_or(0, |&last| {
                let last = self.region(last);
                last.offset + last.size
            });
            self.hole_total -= region.offset - new_top;
            self.top = new_top;
        } else {
            self.hole_total += region.size;
        }
    }

    pub(crate) fn offset(&self, id: StaticId) -> u64 {
        self.region(id).offset
    }

    #[cfg(test)]
    pub(crate) fn size(&self, id: StaticId) -> u64 {
        self.region(id).size
    }

    // 今の先頭に積めば容量に収まるか
    pub(crate) fn fits(&self, size: u64) -> bool {
        self.top + super::tlsf::align_up(size, self.alignment) <= self.capacity
    }

    // 穴の合計が容量に対して threshold（0.0〜1.0）を超えたか（§6 Static 領域のコンパクション）
    pub(crate) fn needs_compaction(&self, threshold: f32) -> bool {
        self.capacity > 0 && self.hole_total as f64 / self.capacity as f64 > threshold as f64
    }

    // 穴を詰め、移す範囲の一覧を返す。各領域の番号は変わらない
    // GPU のコピーは返した一覧から行う（同じバッファ内ではコピーできないため一時バッファを経由する）
    pub(crate) fn compact(&mut self) -> Vec<Move> {
        let mut moves: Vec<Move> = Vec::new();
        let mut cursor = 0;

        let ids: Vec<StaticId> = self.order.values().copied().collect();
        self.order.clear();

        for id in ids {
            let region = self.regions[id as usize].as_mut().expect("存在しない Static の領域です");

            if region.offset != cursor {
                match moves.last_mut() {
                    // 移す元と先がともに続いていれば 1 つにまとめる
                    Some(last) if last.from + last.size == region.offset && last.to + last.size == cursor => {
                        last.size += region.size;
                    }
                    _ => moves.push(Move { from: region.offset, to: cursor, size: region.size }),
                }

                region.offset = cursor;
            }

            self.order.insert(cursor, id);
            cursor += region.size;
        }

        self.top = cursor;
        self.hole_total = 0;

        moves
    }

    fn region(&self, id: StaticId) -> Region {
        self.regions.get(id as usize).copied().flatten().expect("存在しない Static の領域です")
    }
}

#[cfg(test)]
impl StaticStack {
    pub(crate) fn check_invariants(&self) {
        let mut cursor = 0;
        let mut holes = 0;

        for (&offset, &id) in &self.order {
            let region = self.region(id);

            assert_eq!(region.offset, offset, "順番の表と領域が一致しません");
            assert!(region.offset >= cursor, "領域が重なっています");
            holes += region.offset - cursor;
            cursor = region.offset + region.size;
        }

        assert_eq!(self.order.len(), self.regions.iter().flatten().count());

        assert_eq!(cursor, self.top, "先頭の位置が一番上の領域と一致しません");
        assert_eq!(holes, self.hole_total, "穴の合計が一致しません");
        assert!(self.top <= self.capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tlsf::tests::Rng;

    #[test]
    fn stacks_from_front() {
        let mut stack = StaticStack::new(1024, 4);
        let a = stack.allocate(10);
        let b = stack.allocate(20);

        assert_eq!((stack.offset(a), stack.size(a)), (0, 12));
        assert_eq!((stack.offset(b), stack.size(b)), (12, 20));
    }

    #[test]
    #[should_panic(expected = "Static 領域の容量を超えました")]
    fn panics_when_full() {
        let mut stack = StaticStack::new(64, 4);
        stack.allocate(60);
        stack.allocate(8);
    }

    #[test]
    fn freeing_top_lowers_top_and_drops_holes_below() {
        let mut stack = StaticStack::new(1024, 4);
        let a = stack.allocate(100);
        let b = stack.allocate(100);
        let c = stack.allocate(100);

        stack.free(b);
        assert_eq!(stack.hole_total(), 100);
        // 一番上を解放すると、その下の穴もまとめて捨てる
        stack.free(c);
        assert_eq!(stack.hole_total(), 0);
        stack.check_invariants();

        let d = stack.allocate(4);
        assert_eq!(stack.offset(d), 100);
        assert_eq!(stack.offset(a), 0);
    }

    #[test]
    fn threshold_decides_compaction() {
        let mut stack = StaticStack::new(1000, 4);
        let a = stack.allocate(200);
        stack.allocate(100);

        stack.free(a);
        assert!(stack.needs_compaction(0.1));
        assert!(!stack.needs_compaction(0.25));
    }

    #[test]
    fn compaction_merges_contiguous_moves() {
        let mut stack = StaticStack::new(1024, 4);
        let a = stack.allocate(100);
        let b = stack.allocate(100);
        let c = stack.allocate(100);
        let d = stack.allocate(100);

        stack.free(a);
        let moves = stack.compact();

        // b〜d は続いているため 1 回のコピーにまとまる
        assert_eq!(moves, vec![Move { from: 100, to: 0, size: 300 }]);
        assert_eq!([stack.offset(b), stack.offset(c), stack.offset(d)], [0, 100, 200]);
        stack.check_invariants();
    }

    #[test]
    fn random_free_and_compact_keeps_contents() {
        let mut rng = Rng::new(42);
        let mut stack = StaticStack::new(1 << 20, 4);
        // 各領域の「中身」として、先頭の位置に置いた値を追いかける
        let mut memory = vec![0u32; (1 << 20) / 4];
        let mut live: Vec<(StaticId, u32)> = Vec::new();

        for value in 1..3000u32 {
            if live.is_empty() || rng.below(3) != 0 {
                let id = stack.allocate(4 + rng.below(200));
                memory[(stack.offset(id) / 4) as usize] = value;
                live.push((id, value));
            } else {
                let (id, _) = live.swap_remove(rng.below(live.len() as u64) as usize);
                stack.free(id);
            }

            if stack.needs_compaction(0.1) {
                // 一時バッファを経由するのと同じく、移す前の内容を写してから書き込む
                let before = memory.clone();

                for Move { from, to, size } in stack.compact() {
                    let (from, to, len) = ((from / 4) as usize, (to / 4) as usize, (size / 4) as usize);
                    memory[to..to + len].copy_from_slice(&before[from..from + len]);
                }
            }

            stack.check_invariants();

            for &(id, value) in &live {
                assert_eq!(memory[(stack.offset(id) / 4) as usize], value, "詰め直しで中身がずれました");
            }
        }
    }
}
