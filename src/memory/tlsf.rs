// Dynamic 領域の割り当て器（§6 Dynamic の割り当て：TLSF）
// GPU のバッファにはヘッダを書けないため、ブロックの情報はすべて CPU 側に持つ。オフセットは Dynamic 領域の先頭からの位置

// 第 2 レベルの分割数の log2（VMA と同じ。サイズクラス内の無駄は最大約 1/32）
const SECOND_LEVEL_INDEX: u32 = 5;
const SECOND_LEVEL_COUNT: usize = 1 << SECOND_LEVEL_INDEX;
const FIRST_LEVEL_COUNT: usize = 32;
// これ未満は fl = 0 にまとめ、線形に分ける
const SMALL_SIZE: u64 = 256;
const SMALL_STEP: u64 = SMALL_SIZE / SECOND_LEVEL_COUNT as u64;
// fl >= 1 は 2^(fl + FIRST_LEVEL_SHIFT) 以上 2^(fl + FIRST_LEVEL_SHIFT + 1) 未満
const FIRST_LEVEL_SHIFT: u32 = SMALL_SIZE.trailing_zeros() - 1;
// 分割した残りがこれ未満なら分割せず、確保したブロックに含める
const MIN_BLOCK_SIZE: u64 = 16;

// ブロックの番号。解放後は再利用される
pub(crate) type BlockId = u32;

#[derive(Copy, Clone, Debug)]
struct Block {
    offset: u64,
    size: u64,
    is_free: bool,
    prev_physical: Option<BlockId>, // メモリ上で手前に隣接するブロック（結合用）
    next_physical: Option<BlockId>, // メモリ上で後ろに隣接するブロック（結合用）
    prev_free: Option<BlockId>,     // 同じサイズクラスの空きリスト（双方向）
    next_free: Option<BlockId>,
}

#[derive(Debug)]
pub(crate) struct TlsfAllocator {
    blocks: Vec<Block>,
    unused_block_slots: Vec<BlockId>,
    first_level_bitmap: u32,
    second_level_bitmaps: [u32; FIRST_LEVEL_COUNT],
    free_heads: [[Option<BlockId>; SECOND_LEVEL_COUNT]; FIRST_LEVEL_COUNT],
    last_physical_block: Option<BlockId>,
    capacity: u64,
    alignment: u64,
}

impl TlsfAllocator {
    pub(crate) fn new(capacity: u64, alignment: u64) -> Self {
        assert!(alignment.is_power_of_two(), "アラインメントは 2 の累乗であること");

        let mut allocator = Self {
            blocks: Vec::new(),
            unused_block_slots: Vec::new(),
            first_level_bitmap: 0,
            second_level_bitmaps: [0; FIRST_LEVEL_COUNT],
            free_heads: [[None; SECOND_LEVEL_COUNT]; FIRST_LEVEL_COUNT],
            last_physical_block: None,
            capacity: 0,
            alignment,
        };
        allocator.grow(align_down(capacity, alignment));

        allocator
    }

    pub(crate) fn capacity(&self) -> u64 {
        self.capacity
    }

    pub(crate) fn alignment(&self) -> u64 {
        self.alignment
    }

    pub(crate) fn offset(&self, id: BlockId) -> u64 {
        self.blocks[id as usize].offset
    }

    // 分割の残りが小さい場合は要求より大きくなることがある
    pub(crate) fn size(&self, id: BlockId) -> u64 {
        self.blocks[id as usize].size
    }

    // 空きがなければ None。呼び出し側が grow してから再試行する（§6 Dynamic 領域の拡張）
    pub(crate) fn allocate(&mut self, size: u64) -> Option<BlockId> {
        let size = self.aligned_size(size);
        let (fl, sl) = mapping(round_up_to_class(size));
        let (fl, sl) = self.find_free_class(fl, sl)?;
        let id = self.free_heads[fl][sl].expect("ビットマップと空きリストが食い違っています");

        self.remove_free(id);
        self.split(id, size);
        self.blocks[id as usize].is_free = false;

        Some(id)
    }

    pub(crate) fn free(&mut self, id: BlockId) {
        assert!(!self.blocks[id as usize].is_free, "解放済みのブロックを解放しました");

        let mut id = id;
        self.blocks[id as usize].is_free = true;

        if let Some(next) = self.blocks[id as usize].next_physical
            && self.blocks[next as usize].is_free
        {
            self.remove_free(next);
            self.absorb_next(id);
        }

        if let Some(prev) = self.blocks[id as usize].prev_physical
            && self.blocks[prev as usize].is_free
        {
            self.remove_free(prev);
            self.absorb_next(prev);
            id = prev;
        }

        self.insert_free(id);
    }

    // 容量を additional だけ末尾に足す（§6 Dynamic 領域の拡張）。既存のブロックのオフセットは変わらない
    pub(crate) fn grow(&mut self, additional: u64) {
        let additional = align_down(additional, self.alignment);

        if additional == 0 {
            return;
        }

        match self.last_physical_block {
            Some(last) if self.blocks[last as usize].is_free => {
                self.remove_free(last);
                self.blocks[last as usize].size += additional;
                self.insert_free(last);
            }
            last => {
                let id = self.new_block(Block {
                    offset: self.capacity,
                    size: additional,
                    is_free: true,
                    prev_physical: last,
                    next_physical: None,
                    prev_free: None,
                    next_free: None,
                });

                if let Some(last) = last {
                    self.blocks[last as usize].next_physical = Some(id);
                }

                self.last_physical_block = Some(id);
                self.insert_free(id);
            }
        }

        self.capacity += additional;
    }

    // allocate が空きを探すときの大きさ（サイズクラスの境界まで切り上げた値）。広げる量の計算に使う
    pub(crate) fn request_size(&self, size: u64) -> u64 {
        round_up_to_class(self.aligned_size(size))
    }

    // 確保に必要な大きさ（アラインメントに切り上げた値）
    pub(crate) fn aligned_size(&self, size: u64) -> u64 {
        assert!(size > 0, "大きさ 0 の領域は確保できません");

        align_up(size, self.alignment)
    }

    fn find_free_class(&self, fl: usize, sl: usize) -> Option<(usize, usize)> {
        assert!(fl < FIRST_LEVEL_COUNT, "確保できる大きさを超えています");

        let second_level = self.second_level_bitmaps[fl] & (u32::MAX << sl);

        if second_level != 0 {
            return Some((fl, second_level.trailing_zeros() as usize));
        }

        let first_level = self.first_level_bitmap & u32::MAX.checked_shl(fl as u32 + 1).unwrap_or(0);

        if first_level == 0 {
            return None;
        }

        let fl = first_level.trailing_zeros() as usize;

        Some((fl, self.second_level_bitmaps[fl].trailing_zeros() as usize))
    }

    // 使う大きさより後ろを空きブロックとして切り出す
    fn split(&mut self, id: BlockId, size: u64) {
        let block = self.blocks[id as usize];
        let remaining = block.size - size;

        if remaining < MIN_BLOCK_SIZE {
            return;
        }

        let rest = self.new_block(Block {
            offset: block.offset + size,
            size: remaining,
            is_free: true,
            prev_physical: Some(id),
            next_physical: block.next_physical,
            prev_free: None,
            next_free: None,
        });

        match block.next_physical {
            Some(next) => self.blocks[next as usize].prev_physical = Some(rest),
            None => self.last_physical_block = Some(rest),
        }

        let block = &mut self.blocks[id as usize];
        block.size = size;
        block.next_physical = Some(rest);
        self.insert_free(rest);
    }

    // id のすぐ後ろのブロックを id に結合する。後ろのブロックは空きリストから外してあること
    fn absorb_next(&mut self, id: BlockId) {
        let next = self.blocks[id as usize].next_physical.expect("後ろのブロックがありません");
        let next_block = self.blocks[next as usize];

        self.blocks[id as usize].size += next_block.size;
        self.blocks[id as usize].next_physical = next_block.next_physical;

        match next_block.next_physical {
            Some(after) => self.blocks[after as usize].prev_physical = Some(id),
            None => self.last_physical_block = Some(id),
        }

        self.unused_block_slots.push(next);
    }

    fn insert_free(&mut self, id: BlockId) {
        let (fl, sl) = mapping(self.blocks[id as usize].size);
        let head = self.free_heads[fl][sl];
        let block = &mut self.blocks[id as usize];

        block.is_free = true;
        block.prev_free = None;
        block.next_free = head;

        if let Some(head) = head {
            self.blocks[head as usize].prev_free = Some(id);
        }

        self.free_heads[fl][sl] = Some(id);
        self.first_level_bitmap |= 1 << fl;
        self.second_level_bitmaps[fl] |= 1 << sl;
    }

    fn remove_free(&mut self, id: BlockId) {
        let block = self.blocks[id as usize];
        let (fl, sl) = mapping(block.size);

        match block.prev_free {
            Some(prev) => self.blocks[prev as usize].next_free = block.next_free,
            None => self.free_heads[fl][sl] = block.next_free,
        }

        if let Some(next) = block.next_free {
            self.blocks[next as usize].prev_free = block.prev_free;
        }

        if self.free_heads[fl][sl].is_none() {
            self.second_level_bitmaps[fl] &= !(1 << sl);

            if self.second_level_bitmaps[fl] == 0 {
                self.first_level_bitmap &= !(1 << fl);
            }
        }

        let block = &mut self.blocks[id as usize];
        block.prev_free = None;
        block.next_free = None;
    }

    fn new_block(&mut self, block: Block) -> BlockId {
        match self.unused_block_slots.pop() {
            Some(id) => {
                self.blocks[id as usize] = block;
                id
            }
            None => {
                self.blocks.push(block);
                (self.blocks.len() - 1) as BlockId
            }
        }
    }
}

// サイズクラス（fl, sl）を求める
fn mapping(size: u64) -> (usize, usize) {
    if size < SMALL_SIZE {
        return (0, (size / SMALL_STEP) as usize);
    }

    let msb = 63 - size.leading_zeros();
    let fl = (msb - FIRST_LEVEL_SHIFT) as usize;
    let sl = ((size >> (msb - SECOND_LEVEL_INDEX)) as usize) - SECOND_LEVEL_COUNT;

    (fl, sl)
}

// 次のサイズクラスの境界まで切り上げる。見つかったクラスのブロックは必ず要求以上になる（リストを探索しない）
fn round_up_to_class(size: u64) -> u64 {
    let step = if size < SMALL_SIZE { SMALL_STEP } else { 1 << (63 - size.leading_zeros() - SECOND_LEVEL_INDEX) };

    align_up(size, step)
}

pub(crate) fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

#[cfg(test)]
impl TlsfAllocator {
    // ブロックが重ならず隙間なく並び、空きリストとビットマップが一致することを確かめる
    pub(crate) fn check_invariants(&self) {
        let mut chain = Vec::new();
        let mut cursor = self.last_physical_block;

        while let Some(id) = cursor {
            chain.push(id);
            cursor = self.blocks[id as usize].prev_physical;
        }

        chain.reverse();

        let mut offset = 0;
        let mut previous_free = false;

        for &id in &chain {
            let block = &self.blocks[id as usize];

            assert_eq!(block.offset, offset, "ブロックの間に隙間か重なりがあります");
            assert!(!(previous_free && block.is_free), "隣り合う空きブロックが結合されていません");
            assert_eq!(block.offset % self.alignment, 0);
            offset += block.size;
            previous_free = block.is_free;
        }

        assert_eq!(offset, self.capacity, "ブロックの合計が容量と一致しません");

        let mut listed = 0;

        for fl in 0..FIRST_LEVEL_COUNT {
            for sl in 0..SECOND_LEVEL_COUNT {
                let mut cursor = self.free_heads[fl][sl];
                let has_bit = self.second_level_bitmaps[fl] & (1 << sl) != 0;

                assert_eq!(cursor.is_some(), has_bit, "ビットマップと空きリストが食い違っています");

                while let Some(id) = cursor {
                    let block = &self.blocks[id as usize];

                    assert!(block.is_free);
                    assert_eq!(mapping(block.size), (fl, sl), "空きブロックが違うサイズクラスにあります");
                    listed += 1;
                    cursor = block.next_free;
                }
            }

            assert_eq!(self.first_level_bitmap & (1 << fl) != 0, self.second_level_bitmaps[fl] != 0);
        }

        let free_in_chain = chain.iter().filter(|&&id| self.blocks[id as usize].is_free).count();
        assert_eq!(listed, free_in_chain, "空きリストにないか、余分な空きブロックがあります");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // 再現できる簡単な乱数（xorshift）
    pub(crate) struct Rng(u64);

    impl Rng {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed)
        }

        pub(crate) fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        pub(crate) fn below(&mut self, max: u64) -> u64 {
            self.next() % max
        }
    }

    #[test]
    fn mapping_classes() {
        assert_eq!(mapping(0), (0, 0));
        assert_eq!(mapping(255), (0, 31));
        assert_eq!(mapping(256), (1, 0));
        assert_eq!(mapping(511), (1, 31));
        assert_eq!(mapping(512), (2, 0));
        assert_eq!(round_up_to_class(9), 16);
        assert_eq!(round_up_to_class(257), 264);
        assert_eq!(mapping(round_up_to_class(511)), (2, 0));
    }

    #[test]
    fn allocates_from_front_and_splits() {
        let mut tlsf = TlsfAllocator::new(1024, 4);
        let a = tlsf.allocate(100).unwrap();
        let b = tlsf.allocate(30).unwrap();

        assert_eq!((tlsf.offset(a), tlsf.size(a)), (0, 100));
        assert_eq!((tlsf.offset(b), tlsf.size(b)), (100, 32)); // 4 バイトに切り上げ
        tlsf.check_invariants();
    }

    #[test]
    fn returns_none_when_full() {
        let mut tlsf = TlsfAllocator::new(256, 4);

        assert!(tlsf.allocate(256).is_some());
        assert!(tlsf.allocate(4).is_none());
        tlsf.check_invariants();
    }

    #[test]
    fn free_merges_with_neighbors() {
        let mut tlsf = TlsfAllocator::new(1024, 4);
        let a = tlsf.allocate(100).unwrap();
        let b = tlsf.allocate(100).unwrap();
        let c = tlsf.allocate(100).unwrap();

        tlsf.free(a);
        tlsf.free(c);
        tlsf.check_invariants();
        // b を解放すると前後の空きと結合し、全体が 1 つの空きブロックになる
        tlsf.free(b);
        tlsf.check_invariants();

        let all = tlsf.allocate(1024).unwrap();
        assert_eq!((tlsf.offset(all), tlsf.size(all)), (0, 1024));
    }

    #[test]
    fn grow_keeps_existing_offsets() {
        let mut tlsf = TlsfAllocator::new(256, 4);
        let a = tlsf.allocate(200).unwrap();

        assert!(tlsf.allocate(200).is_none());
        tlsf.grow(256);
        tlsf.check_invariants();

        let b = tlsf.allocate(200).unwrap();
        assert_eq!(tlsf.offset(a), 0);
        assert_eq!(tlsf.offset(b), 200); // 末尾の空きブロックが伸びている
        assert_eq!(tlsf.capacity(), 512);
    }

    #[test]
    fn grow_from_zero_capacity() {
        let mut tlsf = TlsfAllocator::new(0, 4);

        assert!(tlsf.allocate(4).is_none());
        tlsf.grow(64);
        let id = tlsf.allocate(64).unwrap();
        assert_eq!(tlsf.offset(id), 0);
        tlsf.check_invariants();
    }

    #[test]
    fn random_allocate_and_free_keeps_invariants() {
        let mut rng = Rng::new(0x1234_5678_9abc_def0);
        let mut tlsf = TlsfAllocator::new(64 * 1024, 4);
        let mut live: Vec<(BlockId, u64, u64)> = Vec::new();

        for _ in 0..5000 {
            if live.is_empty() || rng.below(3) != 0 {
                let max = if rng.below(10) == 0 { 8192 } else { 300 };
                let size = 1 + rng.below(max);

                match tlsf.allocate(size) {
                    Some(id) => {
                        assert!(tlsf.size(id) >= size);
                        live.push((id, tlsf.offset(id), tlsf.size(id)));
                    }
                    None => tlsf.grow(size.max(4096)),
                }
            } else {
                let index = rng.below(live.len() as u64) as usize;
                let (id, offset, size) = live.swap_remove(index);

                // 確保中のブロックは動かない
                assert_eq!((tlsf.offset(id), tlsf.size(id)), (offset, size));
                tlsf.free(id);
            }

            tlsf.check_invariants();
        }

        // 確保中のブロック同士が重ならない
        live.sort_by_key(|&(_, offset, _)| offset);

        for pair in live.windows(2) {
            assert!(pair[0].1 + pair[0].2 <= pair[1].1);
        }
    }
}
