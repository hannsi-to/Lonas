// 番号を再利用し、世代で古い ID と区別する表（§11 ID）

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

    fn position(&self, id: SlotId) -> Option<usize> {
        let position = id.index.checked_sub(self.first_index)? as usize;
        let slot = self.slots.get(position)?;

        (id.owner == self.owner && slot.generation == id.generation).then_some(position)
    }
}
