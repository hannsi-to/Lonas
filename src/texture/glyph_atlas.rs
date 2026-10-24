// グリフのアトラス（§13 グリフのアトラス）。テクスチャの TextureAtlas とは別に、R8Unorm と Rgba8Unorm の 2 つを持つ
// 配置は棚方式（packer）、満杯の時はそのフレームに使わなかったグリフを古い順に追い出す（§13 満杯の時）
// ミップマップは作らない。番号 0 は塗りつぶしの領域（覆い率 1）
// text feature を外すとグリフを置く処理（apply）がなく、番号 0 だけを持つ
#![cfg_attr(not(feature = "text"), allow(dead_code))]

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::packer::{Placement, ShelfPacker};
use crate::config::LonasConfig;

// Sdf / Msdf の距離の幅と 1 em の大きさ。text feature を外すと使わないため、ここで同じ形を持つ
#[cfg(feature = "text")]
use crate::text::bitmap::DistanceSettings;
#[cfg(not(feature = "text"))]
#[derive(Copy, Clone)]
struct DistanceSettings {
    range: f32,
    em_size: u32,
}

// WGSL の GlyphRect（32 バイト、§13 グリフの番号と、追い出した後・Sdf / Msdf の細部）
// distance_range・em_size は Sdf / Msdf のときだけ使う（Bitmap では 0）
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuGlyphRect {
    pub rect: [f32; 4],
    pub layer: u32,
    pub distance_range: f32,
    pub em_size: f32,
    pub _reserved: u32,
}

const _: () = assert!(size_of::<GpuGlyphRect>() == crate::wgsl::GLYPH_RECT_SIZE as usize);

// 配置と追い出しの計算（GPU なしでテストできる部分）
pub(crate) struct GlyphPacking {
    packer: ShelfPacker,
    placements: HashMap<u32, Placement>,
}

impl GlyphPacking {
    pub(crate) fn new(size: u32, padding: u32, initial_layers: u32, max_layers: u32) -> Self {
        Self { packer: ShelfPacker::new(size, padding, 1, initial_layers, max_layers), placements: HashMap::new() }
    }

    pub(crate) fn try_place(&mut self, slot: u32, width: u32, height: u32) -> Option<Placement> {
        let placement = self.packer.allocate(width, height)?;
        self.placements.insert(slot, placement);
        Some(placement)
    }

    // candidates の順に追い出しながら置く。置けたら (配置, 追い出した番号)。候補を使い切っても置けなければ None
    pub(crate) fn place_evicting(&mut self, slot: u32, width: u32, height: u32, candidates: &[u32]) -> Option<(Placement, Vec<u32>)> {
        let mut evicted = Vec::new();

        for &candidate in candidates {
            if let Some(placement) = self.try_place(slot, width, height) {
                return Some((placement, evicted));
            }

            if self.remove(candidate) {
                evicted.push(candidate);
            }
        }

        self.try_place(slot, width, height).map(|placement| (placement, evicted))
    }

    pub(crate) fn remove(&mut self, slot: u32) -> bool {
        match self.placements.remove(&slot) {
            Some(placement) => {
                self.packer.deallocate(placement);
                true
            }
            None => false,
        }
    }

    pub(crate) fn can_grow(&self) -> bool {
        self.packer.layer_count() < self.packer.max_layers()
    }

    pub(crate) fn layer_count(&self) -> u32 {
        self.packer.layer_count()
    }

    pub(crate) fn placed(&self) -> impl Iterator<Item = (u32, Placement)> + '_ {
        self.placements.iter().map(|(&slot, &placement)| (slot, placement))
    }

    pub(crate) fn placement(&self, slot: u32) -> Option<Placement> {
        self.placements.get(&slot).copied()
    }

    fn add_layers(&mut self, count: u32) {
        self.packer.add_layers(count);
    }
}

// 1 つのフォーマットのテクスチャ配列
struct GlyphTexture {
    format: wgpu::TextureFormat,
    bytes_per_pixel: u32,
    size: u32,
    packing: GlyphPacking,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl GlyphTexture {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, bytes_per_pixel: u32, config: &LonasConfig) -> Self {
        let atlas = &config.glyph_atlas;
        let texture = create_texture(device, format, atlas.size, atlas.initial_layers);

        Self {
            format,
            bytes_per_pixel,
            size: atlas.size,
            packing: GlyphPacking::new(atlas.size, atlas.padding, atlas.initial_layers, atlas.max_layers),
            view: array_view(&texture),
            texture,
        }
    }

    // 余白を含む範囲に、余白を fill で埋めた画素を送る
    fn write(&self, queue: &wgpu::Queue, placement: &Placement, pixels: &[u8], fill: u8) {
        let bpp = self.bytes_per_pixel;
        let (left, top) = (placement.inner_x - placement.outer_x, placement.inner_y - placement.outer_y);
        let mut padded = vec![fill; (placement.outer_width * placement.outer_height * bpp) as usize];

        for y in 0..placement.height {
            let source = (y * placement.width * bpp) as usize;
            let target = (((y + top) * placement.outer_width + left) * bpp) as usize;
            let length = (placement.width * bpp) as usize;
            padded[target..target + length].copy_from_slice(&pixels[source..source + length]);
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: placement.outer_x, y: placement.outer_y, z: placement.layer },
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(placement.outer_width * bpp),
                rows_per_image: Some(placement.outer_height),
            },
            wgpu::Extent3d { width: placement.outer_width, height: placement.outer_height, depth_or_array_layers: 1 },
        );
    }

    // max_layers まで増やす。より多いレイヤーのテクスチャを作り、既存の内容をコピーして差し替える
    fn add_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, growth_factor: f32) {
        let current = self.packing.layer_count();
        let target = ((current as f32 * growth_factor).ceil() as u32).max(current + 1).min(self.packing.packer.max_layers());

        self.packing.add_layers(target - current);

        let texture = create_texture(device, self.format, self.size, target);
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: self.size, height: self.size, depth_or_array_layers: current },
        );
        queue.submit([encoder.finish()]);

        self.view = array_view(&texture);
        self.texture = texture;
    }
}

pub(crate) struct GlyphAtlas {
    r8: GlyphTexture,
    // Msdf 用（段階 12）
    rgba: GlyphTexture,
    sampler: wgpu::Sampler,
    // 番号 → アトラス上の範囲とレイヤー。表を大きくする時に書き直すため、CPU にも写しを持つ
    rects: Vec<GpuGlyphRect>,
    rects_buffer: wgpu::Buffer,
    growth_factor: f32,
}

impl GlyphAtlas {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, config: &LonasConfig) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Lonas glyph"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut atlas = Self {
            r8: GlyphTexture::new(device, wgpu::TextureFormat::R8Unorm, 1, config),
            rgba: GlyphTexture::new(device, wgpu::TextureFormat::Rgba8Unorm, 4, config),
            sampler,
            rects: Vec::new(),
            rects_buffer: create_rects(device, 64),
            growth_factor: config.dynamic_growth_factor,
        };

        // 番号 0 は塗りつぶしの領域。余白も 1 で埋め、どこを読んでも 1（距離は内側の最大）になる（§13 頂点からの参照）
        // R8Unorm と Rgba8Unorm の両方の同じ位置に置き、Msdf のシェーダーが読んでも同じ範囲になるようにする
        let placement = atlas.r8.packing.try_place(0, 1, 1).expect("空のアトラスに 1 画素を置けません");
        let rgba_placement = atlas.rgba.packing.try_place(0, 1, 1).expect("空のアトラスに 1 画素を置けません");
        assert_eq!(placement, rgba_placement, "2 つのアトラスで番号 0 の位置が食い違いました");
        atlas.r8.write(queue, &placement, &[255], 255);
        atlas.rgba.write(queue, &rgba_placement, &[255; 4], 255);
        atlas.set_rect(device, queue, 0, &placement, None);

        atlas
    }

    pub(crate) fn r8_view(&self) -> &wgpu::TextureView {
        &self.r8.view
    }

    pub(crate) fn rgba_view(&self) -> &wgpu::TextureView {
        &self.rgba.view
    }

    pub(crate) fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    pub(crate) fn rects(&self) -> &wgpu::Buffer {
        &self.rects_buffer
    }

    // end_frame の「2. リソースの反映」：登録を解除したフォントのグリフを外し、新しいグリフと追い出されていたグリフを置く
    #[cfg(feature = "text")]
    pub(crate) fn apply(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cache: &mut crate::text::bitmap::BitmapCache, frame: u64) {
        // 番号はどちらか一方のアトラスにだけ置かれている
        for slot in cache.take_removals() {
            self.r8.packing.remove(slot);
            self.rgba.packing.remove(slot);
        }

        for slot in cache.take_uploads() {
            let Some(glyph) = cache.glyph(slot) else { continue };
            let (width, height, distance) = (glyph.raster.width, glyph.raster.height, glyph.raster.distance);
            let growth_factor = self.growth_factor;
            // Bitmap・Sdf は R8Unorm、Msdf は Rgba8Unorm（§13 グリフのアトラス）
            let texture = if glyph.raster.bytes_per_pixel == 4 { &mut self.rgba } else { &mut self.r8 };

            if texture.packing.placement(slot).is_some() {
                cache.set_resident(slot, true);
                continue;
            }

            let placement = match texture.packing.try_place(slot, width, height) {
                Some(placement) => placement,
                None if texture.packing.can_grow() => {
                    texture.add_layers(device, queue, growth_factor);
                    texture.packing.try_place(slot, width, height).expect("レイヤーを増やした後もグリフを置けません")
                }
                None => {
                    // そのフレームに使わなかったグリフを、最後に使ったフレームが古い順に追い出す（§13 満杯の時）
                    let mut candidates: Vec<(u64, u32)> = texture
                        .packing
                        .placed()
                        .filter(|&(placed, _)| placed != 0)
                        .filter_map(|(placed, _)| cache.glyph(placed).map(|glyph| (glyph.last_used, placed)))
                        .filter(|&(last_used, _)| last_used < frame)
                        .collect();
                    candidates.sort_unstable();
                    let candidates: Vec<u32> = candidates.into_iter().map(|(_, placed)| placed).collect();

                    let (placement, evicted) = texture.packing.place_evicting(slot, width, height, &candidates).unwrap_or_else(|| {
                        panic!(
                            "そのフレームに使うグリフだけでグリフのアトラスが溢れました。LonasConfig::glyph_atlas の size か max_layers を増やしてください"
                        )
                    });

                    for evicted in evicted {
                        cache.set_resident(evicted, false);
                    }

                    placement
                }
            };

            let glyph = cache.glyph(slot).expect("置いている途中のグリフです");
            // 余白は 0（覆い率 0・距離は外側の最大）
            texture.write(queue, &placement, &glyph.raster.pixels, 0);
            self.set_rect(device, queue, slot, &placement, distance);
            cache.set_resident(slot, true);
        }
    }

    // 余白を含まない範囲の UV。表を大きくする時は、すべての番号を書き直す
    #[cfg_attr(not(feature = "text"), allow(unused_variables))]
    fn set_rect(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, slot: u32, placement: &Placement, distance: Option<DistanceSettings>) {
        let size = self.r8.size as f32;
        let rect = GpuGlyphRect {
            rect: [placement.inner_x as f32 / size, placement.inner_y as f32 / size, placement.width as f32 / size, placement.height as f32 / size],
            layer: placement.layer,
            distance_range: distance.map_or(0.0, |distance| distance.range),
            em_size: distance.map_or(0.0, |distance| distance.em_size as f32),
            _reserved: 0,
        };
        let index = slot as usize;

        if index >= self.rects.len() {
            self.rects.resize(index + 1, GpuGlyphRect::default());
        }
        self.rects[index] = rect;

        let needed = self.rects.len() as u64 * size_of::<GpuGlyphRect>() as u64;
        if needed > self.rects_buffer.size() {
            self.rects_buffer = create_rects(device, (self.rects.len() as u64).next_power_of_two());
            queue.write_buffer(&self.rects_buffer, 0, bytemuck::cast_slice(&self.rects));
        } else {
            queue.write_buffer(&self.rects_buffer, index as u64 * size_of::<GpuGlyphRect>() as u64, bytemuck::bytes_of(&rect));
        }
    }

    #[cfg(test)]
    pub(crate) fn placement(&self, slot: u32) -> Option<Placement> {
        self.r8.packing.placement(slot)
    }

    #[cfg(test)]
    pub(crate) fn layer_count(&self) -> u32 {
        self.r8.packing.layer_count()
    }
}

fn create_texture(device: &wgpu::Device, format: wgpu::TextureFormat, size: u32, layers: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Lonas glyph atlas"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // レイヤーを増やす時のコピーのため COPY_SRC
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
}

fn create_rects(device: &wgpu::Device, entries: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Lonas glyph rects"),
        size: entries * size_of::<GpuGlyphRect>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &Placement, b: &Placement) -> bool {
        a.layer == b.layer
            && a.outer_x < b.outer_x + b.outer_width
            && b.outer_x < a.outer_x + a.outer_width
            && a.outer_y < b.outer_y + b.outer_height
            && b.outer_y < a.outer_y + a.outer_height
    }

    // 乱数（xorshift）
    struct Random(u64);

    impl Random {
        fn next(&mut self, range: u32) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % range as u64) as u32
        }
    }

    #[test]
    fn random_placement_and_eviction_keep_padding_without_overlap() {
        let padding = 2;
        let mut packing = GlyphPacking::new(128, padding, 1, 1);
        let mut random = Random(0x1234_5678_9abc_def0);
        // 最後に使ったフレーム
        let mut last_used: HashMap<u32, u64> = HashMap::new();

        for frame in 0..400u64 {
            let slot = frame as u32 + 1;
            let (width, height) = (1 + random.next(24), 1 + random.next(24));
            let mut candidates: Vec<(u64, u32)> = packing.placed().map(|(placed, _)| (last_used[&placed], placed)).collect();
            candidates.sort_unstable();
            let candidates: Vec<u32> = candidates.into_iter().map(|(_, placed)| placed).collect();

            let (placement, evicted) = packing.place_evicting(slot, width, height, &candidates).expect("追い出せば置けるはず");
            // 古いものから追い出す：追い出したものは、残ったどれよりも前に使われた
            if let Some(newest_evicted) = evicted.iter().map(|evicted| last_used[evicted]).max() {
                assert!(packing.placed().filter(|&(placed, _)| placed != slot).all(|(placed, _)| last_used[&placed] >= newest_evicted));
            }
            for evicted in &evicted {
                last_used.remove(evicted);
            }
            last_used.insert(slot, frame);

            // 余白を含めて大きさが合い、余白の分だけ内側に置かれる
            assert_eq!((placement.width, placement.height), (width, height));
            assert_eq!(placement.inner_x - placement.outer_x, padding);
            assert_eq!(placement.outer_width, width + padding * 2);

            // 置いてあるものどうしは重ならない
            let placed: Vec<Placement> = packing.placed().map(|(_, placement)| placement).collect();
            for (i, a) in placed.iter().enumerate() {
                for b in &placed[i + 1..] {
                    assert!(!overlaps(a, b), "{a:?} と {b:?} が重なっています");
                }
            }

            // ときどき使ったことにする（追い出されにくくなる）
            if let Some((placed, _)) = packing.placed().nth(random.next(4) as usize) {
                last_used.insert(placed, frame);
            }
        }
    }

    #[test]
    fn placing_fails_when_no_candidate_can_be_evicted() {
        let mut packing = GlyphPacking::new(32, 0, 1, 1);

        assert!(packing.try_place(1, 32, 32).is_some());
        // そのフレームに使ったものは候補に入らない
        assert!(packing.place_evicting(2, 8, 8, &[]).is_none());
        assert_eq!(packing.place_evicting(2, 8, 8, &[1]).map(|(_, evicted)| evicted), Some(vec![1]));
    }
}
