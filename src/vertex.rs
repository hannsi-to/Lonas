use crate::color::srgb_to_linear;

// 頂点。Object*::vertex に渡す（§1 頂点の入力）。DrawManager* の VertexLayout に含まれない要素は 0.0 のままにする
// Default はすべて 0.0。Vertex { x, y, ..Default::default() } のように、使う要素だけを書ける
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    pub u: f32,
    pub v: f32,
    pub n_x: f32,
    pub n_y: f32,
    pub n_z: f32,
}

impl Vertex {
    // 頂点の要素を引数で直接受け取る（§1 頂点の入力）
    #[allow(clippy::too_many_arguments)]
    pub fn new_position_color_uv_normal(x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32, n_x: f32, n_y: f32, n_z: f32) -> Self {
        Self {
            x,
            y,
            z,
            r,
            g,
            b,
            a,
            u,
            v,
            n_x,
            n_y,
            n_z
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_position_color_uv(x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32) -> Self {
        Vertex::new_position_color_uv_normal(
            x,
            y,
            z,
            r,
            g,
            b,
            a,
            u,
            v,
            0.0,
            0.0,
            0.0,
        )
    }

    pub fn new_position_color(x: f32, y: f32, z: f32, r: f32, g: f32, b: f32, a: f32) -> Self {
        Vertex::new_position_color_uv(
            x,
            y,
            z,
            r,
            g,
            b,
            a,
            0.0,
            0.0,
        )
    }

    pub fn new_position(x: f32, y: f32, z: f32) -> Self {
        Vertex::new_position_color(
            x,
            y,
            z,
            0.0,
            0.0,
            0.0,
            0.0,
        )
    }
}

// DrawManager* ごとに選ぶ頂点の要素（§10 頂点の並び方）。要素は前から順に足していく形
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum VertexLayout {
    Position,              // 12 バイト
    PositionColor,         // 28 バイト
    PositionColorUv,       // 36 バイト
    PositionColorUvNormal, // 48 バイト
}

impl VertexLayout {
    // GPU 上の 1 頂点の大きさ（バイト）
    pub const fn stride(self) -> usize {
        self.float_count() * size_of::<f32>()
    }

    const fn float_count(self) -> usize {
        match self {
            VertexLayout::Position => 3,
            VertexLayout::PositionColor => 7,
            VertexLayout::PositionColorUv => 9,
            VertexLayout::PositionColorUvNormal => 12,
        }
    }
}

impl Vertex {
    // VertexLayout に含まれる要素だけを詰めて dst に書く。色は線形に変換する（§10 頂点の並び方・§11 色）
    // dst の大きさは layout.stride() と同じであること
    pub(crate) fn write_packed(&self, layout: VertexLayout, dst: &mut [u8]) {
        let values = [
            self.x,
            self.y,
            self.z,
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
            self.a,
            self.u,
            self.v,
            self.n_x,
            self.n_y,
            self.n_z,
        ];

        dst.copy_from_slice(bytemuck::cast_slice(&values[..layout.float_count()]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(vertex: &Vertex, layout: VertexLayout) -> Vec<f32> {
        let mut bytes = vec![0; layout.stride()];
        vertex.write_packed(layout, &mut bytes);

        bytes.as_chunks::<4>().0.iter().map(|&chunk| f32::from_ne_bytes(chunk)).collect()
    }

    #[test]
    fn strides_match_design() {
        assert_eq!(VertexLayout::Position.stride(), 12);
        assert_eq!(VertexLayout::PositionColor.stride(), 28);
        assert_eq!(VertexLayout::PositionColorUv.stride(), 36);
        assert_eq!(VertexLayout::PositionColorUvNormal.stride(), 48);
    }

    #[test]
    fn packs_only_layout_elements_with_linear_color() {
        let vertex = Vertex::new_position_color_uv_normal(1.0, 2.0, 3.0, 1.0, 0.0, 1.0, 0.5, 0.25, 0.75, 0.0, 1.0, 0.0);

        assert_eq!(packed(&vertex, VertexLayout::Position), [1.0, 2.0, 3.0]);
        assert_eq!(packed(&vertex, VertexLayout::PositionColor), [1.0, 2.0, 3.0, 1.0, 0.0, 1.0, 0.5]);
        assert_eq!(
            packed(&vertex, VertexLayout::PositionColorUvNormal),
            [1.0, 2.0, 3.0, 1.0, 0.0, 1.0, 0.5, 0.25, 0.75, 0.0, 1.0, 0.0],
        );

        // 0.0 と 1.0 以外の色は線形に変換される
        let gray = Vertex::new_position_color(0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5);
        let color = packed(&gray, VertexLayout::PositionColor);
        assert!((color[3] - 0.21404).abs() < 1e-4);
        assert_eq!(color[6], 0.5);
    }
}
