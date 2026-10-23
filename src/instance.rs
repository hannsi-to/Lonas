// インスタンスの型（§11 Instance）と、GPU 上の並び方への詰め直し（§11 シェーダーとの約束事）

use bytemuck::{Pod, Zeroable};

use crate::color::Color;
use crate::math::{Matrix4f, Vector2f};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}

impl Visibility {
    // GPU 上では u32（1 = Visible、0 = Hidden）
    const fn to_gpu(self) -> u32 {
        match self {
            Visibility::Visible => 1,
            Visibility::Hidden => 0,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Instance2D {
    pub position: Vector2f,
    pub rotation: f32,
    pub scale: Vector2f,
    pub color: Color,
    pub visibility: Visibility,
}

impl Default for Instance2D {
    // 先に確保して後から値を設定する使い方のため（§7 add_instance の同値の扱い）
    fn default() -> Self {
        Self {
            position: Vector2f::ZERO,
            rotation: 0.0,
            scale: Vector2f::ONE,
            color: Color::WHITE,
            visibility: Visibility::Visible,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Instance3D {
    pub transform: Matrix4f,
    pub color: Color,
    pub visibility: Visibility,
}

impl Default for Instance3D {
    fn default() -> Self {
        Self {
            transform: Matrix4f::IDENTITY,
            color: Color::WHITE,
            visibility: Visibility::Visible,
        }
    }
}

// WGSL の Instance2D と同じ並び（48 バイト）
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuInstance2D {
    pub color: [f32; 4],
    pub position: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
    pub visibility: u32,
    pub texture_slot: u32,
    pub uniform_index: u32,
}

// WGSL の Instance3D と同じ並び（96 バイト）
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuInstance3D {
    pub transform: [f32; 16],
    pub color: [f32; 4],
    pub visibility: u32,
    pub texture_slot: u32,
    pub uniform_index: u32,
    pub _pad: u32,
}

// Rust 側の並び方が WGSL の STRUCTS と食い違えばコンパイルエラーにする
const _: () = assert!(size_of::<GpuInstance2D>() == crate::wgsl::INSTANCE_2D_SIZE as usize);
const _: () = assert!(size_of::<GpuInstance3D>() == crate::wgsl::INSTANCE_3D_SIZE as usize);

impl Instance2D {
    // texture_slot / uniform_index は Lonas が付け足す（§11 UV の変換・uniform）
    pub(crate) fn to_gpu(&self, texture_slot: u32, uniform_index: u32) -> GpuInstance2D {
        GpuInstance2D {
            color: self.color.to_linear(),
            position: self.position.to_array(),
            scale: self.scale.to_array(),
            rotation: self.rotation,
            visibility: self.visibility.to_gpu(),
            texture_slot,
            uniform_index,
        }
    }
}

impl Instance3D {
    pub(crate) fn to_gpu(&self, texture_slot: u32, uniform_index: u32) -> GpuInstance3D {
        GpuInstance3D {
            transform: self.transform.to_cols_array(),
            color: self.color.to_linear(),
            visibility: self.visibility.to_gpu(),
            texture_slot,
            uniform_index,
            _pad: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Quaternion, Vector3f};
    use std::mem::offset_of;

    #[test]
    fn gpu_instance_2d_layout_matches_design() {
        assert_eq!(size_of::<GpuInstance2D>(), 48);
        assert_eq!(offset_of!(GpuInstance2D, color), 0);
        assert_eq!(offset_of!(GpuInstance2D, position), 16);
        assert_eq!(offset_of!(GpuInstance2D, scale), 24);
        assert_eq!(offset_of!(GpuInstance2D, rotation), 32);
        assert_eq!(offset_of!(GpuInstance2D, visibility), 36);
        assert_eq!(offset_of!(GpuInstance2D, texture_slot), 40);
        assert_eq!(offset_of!(GpuInstance2D, uniform_index), 44);
    }

    #[test]
    fn gpu_instance_3d_layout_matches_design() {
        assert_eq!(size_of::<GpuInstance3D>(), 96);
        assert_eq!(offset_of!(GpuInstance3D, transform), 0);
        assert_eq!(offset_of!(GpuInstance3D, color), 64);
        assert_eq!(offset_of!(GpuInstance3D, visibility), 80);
        assert_eq!(offset_of!(GpuInstance3D, texture_slot), 84);
        assert_eq!(offset_of!(GpuInstance3D, uniform_index), 88);
    }

    #[test]
    fn to_gpu_converts_values() {
        let instance = Instance2D {
            position: Vector2f::new(1.0, 2.0),
            rotation: 0.5,
            scale: Vector2f::new(3.0, 4.0),
            color: Color::new(1.0, 0.5, 0.0, 0.25),
            visibility: Visibility::Hidden,
        };
        let gpu = instance.to_gpu(7, 9);

        assert_eq!(gpu.position, [1.0, 2.0]);
        assert_eq!(gpu.scale, [3.0, 4.0]);
        assert_eq!(gpu.color, instance.color.to_linear());
        assert_eq!(gpu.visibility, 0);
        assert_eq!((gpu.texture_slot, gpu.uniform_index), (7, 9));

        let transform = Matrix4f::from_trs(Vector3f::new(1.0, 2.0, 3.0), Quaternion::IDENTITY, Vector3f::ONE);
        let gpu = Instance3D { transform, ..Instance3D::default() }.to_gpu(0, 0);

        // 列優先のため、平行移動は 12〜14 番目
        assert_eq!(&gpu.transform[12..15], &[1.0, 2.0, 3.0]);
        assert_eq!(gpu.visibility, 1);
    }
}
