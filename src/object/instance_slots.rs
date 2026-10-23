// インスタンスのスロット（§7 高頻度更新への対応）
// 削除したスロットは非表示にして再利用する。インスタンスは移動しないため、InstanceId から位置への対応表は要らない

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::Range;

use crate::instance::{Instance2D, Instance3D, Visibility};

// Object* の識別子（InstanceId の発行元の検査に使う、§2）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ObjectId(pub u64);

// インスタンスの型（Instance2D / Instance3D）を型引数に持つ。Object2D の ID を Object3D に渡すとコンパイルエラー（§11 インスタンス API）
pub struct InstanceId<I> {
    slot: u32,
    generation: u32,
    // 同じ型の別の Object* の ID は型では区別できないため、実行時に検査する
    owner: ObjectId,
    _marker: PhantomData<fn() -> I>,
}

// derive は I に Copy などを求めてしまうため、手で実装する
impl<I> Clone for InstanceId<I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for InstanceId<I> {}

impl<I> PartialEq for InstanceId<I> {
    fn eq(&self, other: &Self) -> bool {
        (self.slot, self.generation, self.owner) == (other.slot, other.generation, other.owner)
    }
}

impl<I> Eq for InstanceId<I> {}

impl<I> Hash for InstanceId<I> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.slot, self.generation, self.owner).hash(state);
    }
}

impl<I> fmt::Debug for InstanceId<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceId").field("slot", &self.slot).field("generation", &self.generation).finish()
    }
}

// GPU に送るインスタンスの型（§11 シェーダーとの約束事）
pub(crate) trait InstanceData: Copy {
    // GPU 上の 1 つあたりの大きさ（バイト）
    const GPU_SIZE: usize;
    // インスタンスを置くメモリ空間
    const SPACE: crate::draw::SpaceKind;

    fn with_visibility(self, visibility: Visibility) -> Self;

    // texture_slot / uniform_index を付け足して dst に書く。dst の大きさは GPU_SIZE
    fn write_gpu(&self, texture_slot: u32, uniform_index: u32, dst: &mut [u8]);
}

impl InstanceData for Instance2D {
    const GPU_SIZE: usize = crate::wgsl::INSTANCE_2D_SIZE as usize;
    const SPACE: crate::draw::SpaceKind = crate::draw::SpaceKind::Instance2D;

    fn with_visibility(self, visibility: Visibility) -> Self {
        Self { visibility, ..self }
    }

    fn write_gpu(&self, texture_slot: u32, uniform_index: u32, dst: &mut [u8]) {
        dst.copy_from_slice(bytemuck::bytes_of(&self.to_gpu(texture_slot, uniform_index)));
    }
}

impl InstanceData for Instance3D {
    const GPU_SIZE: usize = crate::wgsl::INSTANCE_3D_SIZE as usize;
    const SPACE: crate::draw::SpaceKind = crate::draw::SpaceKind::Instance3D;

    fn with_visibility(self, visibility: Visibility) -> Self {
        Self { visibility, ..self }
    }

    fn write_gpu(&self, texture_slot: u32, uniform_index: u32, dst: &mut [u8]) {
        dst.copy_from_slice(bytemuck::bytes_of(&self.to_gpu(texture_slot, uniform_index)));
    }
}

pub(crate) struct InstanceSlots<I> {
    owner: ObjectId,
    // GPU に送るインスタンスの CPU 側の写し。削除したスロットは Hidden
    slots: Vec<I>,
    // スロットごとの世代。削除のたびに増やす
    generations: Vec<u32>,
    alive: Vec<bool>,
    free_slots: Vec<u32>,
    live_count: usize,
    // そのフレームで変わったスロットの最小〜最大
    dirty_range: Option<Range<u32>>,
}

impl<I: InstanceData> InstanceSlots<I> {
    pub(crate) fn new(owner: ObjectId) -> Self {
        Self {
            owner,
            slots: Vec::new(),
            generations: Vec::new(),
            alive: Vec::new(),
            free_slots: Vec::new(),
            live_count: 0,
            dirty_range: None,
        }
    }

    // 空いているスロットがあれば再利用し、なければ末尾に追加する
    pub(crate) fn add(&mut self, instance: I) -> InstanceId<I> {
        let slot = match self.free_slots.pop() {
            Some(slot) => {
                self.slots[slot as usize] = instance;
                self.alive[slot as usize] = true;
                slot
            }
            None => {
                self.slots.push(instance);
                self.generations.push(0);
                self.alive.push(true);
                (self.slots.len() - 1) as u32
            }
        };

        self.live_count += 1;
        self.mark_dirty(slot);

        InstanceId { slot, generation: self.generations[slot as usize], owner: self.owner, _marker: PhantomData }
    }

    pub(crate) fn update(&mut self, id: InstanceId<I>, instance: I) {
        let slot = self.check(id);

        self.slots[slot] = instance;
        self.mark_dirty(slot as u32);
    }

    // スロットに Hidden を書き込み、空きに戻す。穴は Compute Shader が非表示として省く（§10）
    pub(crate) fn remove(&mut self, id: InstanceId<I>) {
        let slot = self.check(id);

        self.slots[slot] = self.slots[slot].with_visibility(Visibility::Hidden);
        self.alive[slot] = false;
        self.generations[slot] += 1;
        self.free_slots.push(slot as u32);
        self.live_count -= 1;
        self.mark_dirty(slot as u32);
        self.trim_tail();
    }

    // GPU に送るスロットの数（穴を含む）
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    // 生きているインスタンスの数（§7 インスタンス数の増加の警告）
    pub(crate) fn live_count(&self) -> usize {
        self.live_count
    }

    pub(crate) fn slots(&self) -> &[I] {
        &self.slots
    }

    // そのフレームで変わった範囲を取り出す（転送したら空にする）
    pub(crate) fn take_dirty_range(&mut self) -> Option<Range<u32>> {
        let range = self.dirty_range.take()?;

        // 末尾を切り詰めた後は、範囲が今のスロット数を超えていることがある
        let end = range.end.min(self.slots.len() as u32);

        (range.start < end).then_some(range.start..end)
    }

    // テクスチャを変えたときなど、全体を書き直す
    pub(crate) fn mark_all_dirty(&mut self) {
        if !self.slots.is_empty() {
            self.dirty_range = Some(0..self.slots.len() as u32);
        }
    }

    // 末尾のスロットが空いた場合は切り詰める。途中の穴は詰め直さない（InstanceId の位置が変わってしまうため）
    fn trim_tail(&mut self) {
        while self.alive.last() == Some(&false) {
            let slot = (self.slots.len() - 1) as u32;

            self.slots.pop();
            self.alive.pop();
            self.generations.pop();
            self.free_slots.retain(|&free| free != slot);
        }
    }

    fn mark_dirty(&mut self, slot: u32) {
        self.dirty_range = Some(match self.dirty_range.take() {
            Some(range) => range.start.min(slot)..range.end.max(slot + 1),
            None => slot..slot + 1,
        });
    }

    fn check(&self, id: InstanceId<I>) -> usize {
        let slot = id.slot as usize;

        assert_eq!(id.owner, self.owner, "別の Object* の InstanceId です");
        assert!(
            slot < self.slots.len() && self.alive[slot] && self.generations[slot] == id.generation,
            "存在しない（削除済みの）InstanceId です",
        );

        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector2f;

    fn instance(x: f32) -> Instance2D {
        Instance2D { position: Vector2f::new(x, 0.0), ..Instance2D::default() }
    }

    #[test]
    fn removed_slot_is_hidden_and_reused_with_new_generation() {
        let mut slots = InstanceSlots::new(ObjectId(0));
        let a = slots.add(instance(1.0));
        let b = slots.add(instance(2.0));
        slots.add(instance(3.0));

        slots.remove(b);
        assert_eq!(slots.slots()[1].visibility, Visibility::Hidden);
        assert_eq!((slots.len(), slots.live_count()), (3, 2));

        let c = slots.add(instance(4.0));
        // 穴を再利用するため、他のインスタンスは動かない
        assert_eq!(slots.slots()[1].position.x, 4.0);
        assert_eq!(slots.slots()[0].position.x, 1.0);
        assert_ne!(b, c);

        slots.update(a, instance(5.0));
        assert_eq!(slots.slots()[0].position.x, 5.0);
    }

    #[test]
    #[should_panic(expected = "存在しない（削除済みの）InstanceId です")]
    fn stale_id_panics() {
        let mut slots = InstanceSlots::new(ObjectId(0));
        let a = slots.add(instance(1.0));
        slots.add(instance(2.0));

        slots.remove(a);
        slots.add(instance(3.0)); // 同じスロットを再利用する
        slots.update(a, instance(4.0));
    }

    #[test]
    #[should_panic(expected = "別の Object* の InstanceId です")]
    fn id_from_other_object_panics() {
        let mut first = InstanceSlots::new(ObjectId(0));
        let mut second = InstanceSlots::<Instance2D>::new(ObjectId(1));
        let id = first.add(instance(1.0));

        second.add(instance(1.0));
        second.update(id, instance(2.0));
    }

    #[test]
    fn tail_is_trimmed_but_middle_holes_remain() {
        let mut slots = InstanceSlots::new(ObjectId(0));
        let ids: Vec<_> = (0..4).map(|i| slots.add(instance(i as f32))).collect();

        slots.remove(ids[1]);
        slots.remove(ids[3]);
        // 末尾の 3 は切り詰めるが、途中の 1 は穴のまま
        assert_eq!(slots.len(), 3);
        slots.remove(ids[2]);
        // 2 を削除すると末尾が続けて空くため、1 まで切り詰める
        assert_eq!(slots.len(), 1);

        // 切り詰めたスロットは空きの一覧からも消えている
        let new = slots.add(instance(9.0));
        assert_eq!(slots.len(), 2);
        assert_eq!(slots.slots()[1].position.x, 9.0);
        slots.update(new, instance(10.0));
    }

    #[test]
    fn dirty_range_covers_changed_slots() {
        let mut slots = InstanceSlots::new(ObjectId(0));
        let ids: Vec<_> = (0..10).map(|i| slots.add(instance(i as f32))).collect();

        assert_eq!(slots.take_dirty_range(), Some(0..10));
        assert_eq!(slots.take_dirty_range(), None);

        slots.update(ids[7], instance(0.0));
        slots.update(ids[3], instance(0.0));
        assert_eq!(slots.take_dirty_range(), Some(3..8));
    }

    #[test]
    fn gpu_bytes_include_added_fields() {
        let mut bytes = vec![0; Instance2D::GPU_SIZE];
        instance(1.0).write_gpu(5, 6, &mut bytes);

        assert_eq!(&bytes[40..48], bytemuck::bytes_of(&[5u32, 6u32]));
    }
}
