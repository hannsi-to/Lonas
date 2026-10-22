#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
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