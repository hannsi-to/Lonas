// 番号を再利用し、世代で古い ID と区別する表（§11 ID）。owner で発行元も区別する
// ResourceManager の ShaderId / TextureId と、GpuContext の SurfaceId が使う

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SlotId {
    pub owner: u32,
    pub index: u32,
    pub generation: u32,
}

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

pub(crate) struct Slots<T> {
    owner: u32,
    // first_index より前の番号は使わない（TextureId の 0 の予約など）
    first_index: u32,
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Slots<T> {
    pub(crate) fn new(owner: u32, first_index: u32) -> Self {
        Self { owner, first_index, slots: Vec::new(), free: Vec::new() }
    }

    pub(crate) fn insert(&mut self, value: T) -> SlotId {
        let position = match self.free.pop() {
            Some(position) => position,
            None => {
                self.slots.push(Slot { generation: 0, value: None });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[position as usize];
        slot.value = Some(value);

        SlotId { owner: self.owner, index: position + self.first_index, generation: slot.generation }
    }

    // 世代を進めて番号を空ける。すでに無効な ID なら None
    pub(crate) fn remove(&mut self, id: SlotId) -> Option<T> {
        let position = self.position(id)?;
        let slot = &mut self.slots[position];
        let value = slot.value.take()?;

        slot.generation += 1;
        self.free.push(position as u32);

        Some(value)
    }

    pub(crate) fn get(&self, id: SlotId) -> Option<&T> {
        self.slots[self.position(id)?].value.as_ref()
    }

    pub(crate) fn get_mut(&mut self, id: SlotId) -> Option<&mut T> {
        let position = self.position(id)?;

        self.slots[position].value.as_mut()
    }

    // 発行元が違う ID か。違う場合の panic の文言を分けるために使う
    pub(crate) fn owns(&self, id: SlotId) -> bool {
        id.owner == self.owner
    }

    #[cfg(test)]
    pub(crate) fn owner(&self) -> u32 {
        self.owner
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.slots.iter_mut().filter_map(|slot| slot.value.as_mut())
    }

    fn position(&self, id: SlotId) -> Option<usize> {
        let position = id.index.checked_sub(self.first_index)? as usize;
        let slot = self.slots.get(position)?;

        (id.owner == self.owner && slot.generation == id.generation).then_some(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_id_is_rejected_after_reuse() {
        let mut slots = Slots::new(0, 0);
        let first = slots.insert("a");
        let second = slots.insert("b");

        assert_eq!(slots.remove(first), Some("a"));
        assert_eq!(slots.get(first), None);
        // 2 回目の削除は何もしない
        assert_eq!(slots.remove(first), None);

        // 空いた番号を再利用しても、古い ID では取れない
        let third = slots.insert("c");
        assert_eq!(third.index, first.index);
        assert_ne!(third, first);
        assert_eq!(slots.get(first), None);
        assert_eq!(slots.get(third), Some(&"c"));
        assert_eq!(slots.get(second), Some(&"b"));
        assert_eq!(slots.iter_mut().count(), 2);
    }

    #[test]
    fn other_owner_is_rejected() {
        let mut slots = Slots::new(0, 0);
        let mut other = Slots::new(1, 0);
        let id = slots.insert("a");
        let other_id = other.insert("b");

        // 番号と世代が同じでも、発行元が違えば取れない
        assert_eq!((other_id.index, other_id.generation), (id.index, id.generation));
        assert!(!slots.owns(other_id));
        assert_eq!(slots.get(other_id), None);
        assert_eq!(slots.remove(other_id), None);
        assert_eq!(slots.get(id), Some(&"a"));
    }
}
