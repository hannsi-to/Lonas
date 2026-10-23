// アトラスへの詰め込み（§11 TextureAtlas の詰め込み方）。棚（shelf）方式
// アトラスを高さごとの棚に分け、各棚に左から順に置く。解除で空いた場所は同じ棚の中で再利用する

// 配置した場所（画素）。outer は余白を含む範囲、inner はテクスチャ自体の範囲
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Placement {
    pub layer: u32,
    pub outer_x: u32,
    pub outer_y: u32,
    pub outer_width: u32,
    pub outer_height: u32,
    pub inner_x: u32,
    pub inner_y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
struct Shelf {
    y: u32,
    height: u32,
    // 空いている横の区間（x, 幅）。x の順
    free: Vec<(u32, u32)>,
}

#[derive(Debug)]
struct Layer {
    shelves: Vec<Shelf>,
    // 次の棚を作る位置
    next_y: u32,
    // 棚の幅（アトラスの幅）
    size: u32,
}

#[derive(Debug)]
pub(crate) struct ShelfPacker {
    size: u32,
    padding: u32,
    // 配置位置と大きさの単位。ミップマップで隣とにじまないよう 2^(段数 - 1)（§11 ミップマップ）
    alignment: u32,
    layers: Vec<Layer>,
    max_layers: u32,
}

// 棚の高さが要求の何倍までなら、その棚に置くか（低いものを高い棚に置いて無駄にしないため）
const SHELF_FIT: f32 = 1.5;

impl ShelfPacker {
    pub(crate) fn new(size: u32, padding: u32, mip_level_count: u32, initial_layers: u32, max_layers: u32) -> Self {
        let alignment = 1 << (mip_level_count - 1);
        let mut packer = Self { size, padding: align_up(padding, alignment), alignment, layers: Vec::new(), max_layers };

        packer.add_layers(initial_layers);
        packer
    }

    pub(crate) fn layer_count(&self) -> u32 {
        self.layers.len() as u32
    }

    // 置く場所を探す。どのレイヤーにも入らなければ None（呼び出し側がレイヤーを増やして再試行する）
    // 1 つのテクスチャが余白を含めて 1 レイヤーに収まらない場合は panic（§11 容量を超えた場合）
    pub(crate) fn allocate(&mut self, width: u32, height: u32) -> Option<Placement> {
        let outer_width = align_up(width + self.padding * 2, self.alignment);
        let outer_height = align_up(height + self.padding * 2, self.alignment);

        assert!(
            outer_width <= self.size && outer_height <= self.size,
            "テクスチャ（{width}×{height}、余白を含めて {outer_width}×{outer_height}）がアトラスの 1 レイヤー（{0}×{0}）に収まりません",
            self.size,
        );

        for (index, layer) in self.layers.iter_mut().enumerate() {
            if let Some((x, y)) = layer.allocate(outer_width, outer_height) {
                return Some(Placement {
                    layer: index as u32,
                    outer_x: x,
                    outer_y: y,
                    outer_width,
                    outer_height,
                    inner_x: x + self.padding,
                    inner_y: y + self.padding,
                    width,
                    height,
                });
            }
        }

        None
    }

    pub(crate) fn deallocate(&mut self, placement: Placement) {
        self.layers[placement.layer as usize].deallocate(placement);
    }

    // レイヤーを足す。max_layers を超える場合は panic（Static 領域の容量超過と同じ扱い、§11）
    pub(crate) fn add_layers(&mut self, count: u32) {
        let total = self.layers.len() as u32 + count;

        assert!(
            total <= self.max_layers,
            "テクスチャアトラスのレイヤー数が max_layers（{}）を超えます。LonasConfig::texture_atlas の size か max_layers を増やしてください",
            self.max_layers,
        );

        let size = self.size;
        self.layers.extend((0..count).map(|_| Layer { shelves: Vec::new(), next_y: 0, size }));
    }

    pub(crate) fn max_layers(&self) -> u32 {
        self.max_layers
    }
}

impl Layer {
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let size = self.size;

        // 高さが合う棚の空いた区間に置く
        for shelf in &mut self.shelves {
            let fits = shelf.height >= height && (shelf.height as f32) <= height as f32 * SHELF_FIT;
            // 空になった棚は、高さが収まれば何でも置ける
            let empty = shelf.free == [(0, size)];

            if !(fits || (empty && shelf.height >= height)) {
                continue;
            }

            if let Some(index) = shelf.free.iter().position(|&(_, free)| free >= width) {
                let (x, free) = shelf.free[index];

                if free == width {
                    shelf.free.remove(index);
                } else {
                    shelf.free[index] = (x + width, free - width);
                }

                return Some((x, shelf.y));
            }
        }

        // 合う棚がなければ、新しい棚を下に作る
        if self.next_y + height > size {
            return None;
        }

        let y = self.next_y;
        self.next_y += height;
        self.shelves.push(Shelf { y, height, free: vec![(width, size - width)] });

        if width == size {
            self.shelves.last_mut().unwrap().free.clear();
        }

        Some((0, y))
    }

    fn deallocate(&mut self, placement: Placement) {
        let index = self.shelves.iter().position(|shelf| shelf.y == placement.outer_y).expect("存在しない棚です");
        let shelf = &mut self.shelves[index];
        let segment = (placement.outer_x, placement.outer_width);
        let position = shelf.free.partition_point(|&(x, _)| x < segment.0);

        shelf.free.insert(position, segment);

        // 隣り合う空き区間を結合する
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(shelf.free.len());

        for &(x, width) in &shelf.free {
            match merged.last_mut() {
                Some((last_x, last_width)) if *last_x + *last_width == x => *last_width += width,
                _ => merged.push((x, width)),
            }
        }

        shelf.free = merged;

        // 一番下の棚が空になったら、棚ごと解放する
        while self.shelves.last().is_some_and(|last| last.free == [(0, self.size)]) {
            self.next_y = self.shelves.pop().unwrap().y;
        }
    }
}

fn align_up(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tlsf::tests::Rng;

    fn overlaps(a: &Placement, b: &Placement) -> bool {
        a.layer == b.layer
            && a.outer_x < b.outer_x + b.outer_width
            && b.outer_x < a.outer_x + a.outer_width
            && a.outer_y < b.outer_y + b.outer_height
            && b.outer_y < a.outer_y + a.outer_height
    }

    #[test]
    fn padding_and_alignment_follow_mip_levels() {
        // ミップマップ 3 段：配置と余白を 4 画素に揃える。余白 1 は 4 に切り上げる
        let mut packer = ShelfPacker::new(256, 1, 3, 1, 1);
        let a = packer.allocate(10, 6).unwrap();
        let b = packer.allocate(5, 5).unwrap();

        assert_eq!((a.outer_x, a.outer_y, a.outer_width, a.outer_height), (0, 0, 20, 16));
        assert_eq!((a.inner_x, a.inner_y), (4, 4));
        // 同じ棚の右に置く
        assert_eq!((b.outer_x, b.outer_y), (20, 0));
        assert!([a, b].iter().all(|p| p.outer_x % 4 == 0 && p.outer_y % 4 == 0));
    }

    #[test]
    fn freed_space_is_reused_and_empty_bottom_shelf_is_released() {
        let mut packer = ShelfPacker::new(64, 0, 1, 1, 1);
        let a = packer.allocate(32, 32).unwrap();
        let b = packer.allocate(32, 32).unwrap();
        let c = packer.allocate(64, 16).unwrap();

        assert_eq!((c.outer_x, c.outer_y), (0, 32));
        packer.deallocate(a);
        // 空いた区間を同じ棚で再利用する
        assert_eq!(packer.allocate(32, 30).unwrap().outer_x, 0);

        // 一番下の棚を解放すると、そこに別の高さの棚を作れる
        packer.deallocate(c);
        let d = packer.allocate(64, 32).unwrap();
        assert_eq!((d.outer_x, d.outer_y), (0, 32));
        let _ = b;
    }

    #[test]
    fn full_layer_returns_none_until_layers_are_added() {
        let mut packer = ShelfPacker::new(32, 0, 1, 1, 2);

        packer.allocate(32, 32).unwrap();
        assert_eq!(packer.allocate(8, 8), None);
        packer.add_layers(1);
        assert_eq!(packer.allocate(8, 8).unwrap().layer, 1);
    }

    #[test]
    #[should_panic(expected = "max_layers")]
    fn too_many_layers_panics() {
        ShelfPacker::new(32, 0, 1, 1, 1).add_layers(1);
    }

    #[test]
    #[should_panic(expected = "1 レイヤー")]
    fn texture_larger_than_layer_panics() {
        ShelfPacker::new(32, 1, 1, 1, 1).allocate(31, 4);
    }

    #[test]
    fn random_allocate_and_free_never_overlaps() {
        let mut rng = Rng::new(5);
        let mut packer = ShelfPacker::new(512, 2, 2, 1, 64);
        let mut live: Vec<Placement> = Vec::new();

        for _ in 0..3000 {
            if live.is_empty() || rng.below(3) != 0 {
                let (w, h) = (1 + rng.below(60) as u32, 1 + rng.below(60) as u32);
                let placement = match packer.allocate(w, h) {
                    Some(p) => p,
                    None => {
                        packer.add_layers(1);
                        packer.allocate(w, h).unwrap()
                    }
                };

                assert!(placement.outer_x + placement.outer_width <= 512 && placement.outer_y + placement.outer_height <= 512);
                assert!(live.iter().all(|other| !overlaps(other, &placement)), "配置が重なりました");
                live.push(placement);
            } else {
                packer.deallocate(live.swap_remove(rng.below(live.len() as u64) as usize));
            }
        }
    }
}
