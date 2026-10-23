// 数学型（§11 数学型）。外部の数学クレートに依存しない。名前は glam などに合わせる。角度はすべてラジアン

mod matrix;
mod quaternion;
mod vector;

pub use matrix::Matrix4f;
pub use quaternion::Quaternion;
pub use vector::{Vector2f, Vector3f, Vector4f};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const TOLERANCE: f32 = 1e-4;

    pub fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= TOLERANCE, "{actual} != {expected}");
    }

    pub fn assert_vector3_eq(actual: Vector3f, expected: Vector3f) {
        for (a, e) in actual.to_array().into_iter().zip(expected.to_array()) {
            assert!((a - e).abs() <= TOLERANCE, "{actual:?} != {expected:?}");
        }
    }

    pub fn assert_matrix_eq(actual: Matrix4f, expected: Matrix4f) {
        for (a, e) in actual.to_cols_array().into_iter().zip(expected.to_cols_array()) {
            assert!((a - e).abs() <= TOLERANCE, "{actual:?} != {expected:?}");
        }
    }
}
