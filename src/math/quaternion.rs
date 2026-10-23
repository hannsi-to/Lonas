use std::ops::Mul;

use super::Vector3f;

// 回転を表す単位クォータニオン。角度はラジアン
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Quaternion {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quaternion {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quaternion {
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub const fn from_xyzw(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    // axis は正規化してから使う
    pub fn from_axis_angle(axis: Vector3f, radians: f32) -> Self {
        let axis = axis.normalize();
        let (sin, cos) = (radians * 0.5).sin_cos();

        Self::from_xyzw(axis.x * sin, axis.y * sin, axis.z * sin, cos)
    }

    // 回転行列の 3 つの軸（列）から作る。軸は正規直交であること
    pub fn from_rotation_axes(x_axis: Vector3f, y_axis: Vector3f, z_axis: Vector3f) -> Self {
        let (m00, m01, m02) = (x_axis.x, y_axis.x, z_axis.x);
        let (m10, m11, m12) = (x_axis.y, y_axis.y, z_axis.y);
        let (m20, m21, m22) = (x_axis.z, y_axis.z, z_axis.z);
        let trace = m00 + m11 + m22;

        // 対角成分のうち最も大きいものを基準にして、0 に近い値での割り算を避ける
        let quaternion = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Self::from_xyzw((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
        } else if m00 > m11 && m00 > m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            Self::from_xyzw(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
        } else if m11 > m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            Self::from_xyzw((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            Self::from_xyzw((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
        };

        quaternion.normalize()
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }

    pub fn normalize(self) -> Self {
        let length = self.length();

        Self::from_xyzw(self.x / length, self.y / length, self.z / length, self.w / length)
    }

    // 単位クォータニオンの逆回転
    pub fn conjugate(self) -> Self {
        Self::from_xyzw(-self.x, -self.y, -self.z, self.w)
    }

    pub fn rotate(self, vector: Vector3f) -> Vector3f {
        // v' = v + 2w(q × v) + 2(q × (q × v))
        let q = Vector3f::new(self.x, self.y, self.z);
        let t = q.cross(vector) * 2.0;

        vector + t * self.w + q.cross(t)
    }
}

impl Mul for Quaternion {
    type Output = Self;

    // self * other は「other で回してから self で回す」
    fn mul(self, other: Self) -> Self {
        Self::from_xyzw(
            self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
            self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
        )
    }
}

impl Mul<Vector3f> for Quaternion {
    type Output = Vector3f;

    fn mul(self, vector: Vector3f) -> Vector3f {
        self.rotate(vector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tests::assert_vector3_eq;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn rotates_90_degrees_around_z() {
        let q = Quaternion::from_axis_angle(Vector3f::Z, FRAC_PI_2);

        assert_vector3_eq(q * Vector3f::X, Vector3f::Y);
        assert_vector3_eq(q * Vector3f::Y, -Vector3f::X);
    }

    #[test]
    fn multiplication_composes_rotations() {
        let a = Quaternion::from_axis_angle(Vector3f::Z, FRAC_PI_2);
        let b = Quaternion::from_axis_angle(Vector3f::X, FRAC_PI_2);
        let v = Vector3f::new(1.0, 2.0, 3.0);

        assert_vector3_eq((a * b) * v, a * (b * v));
    }

    #[test]
    fn conjugate_undoes_rotation() {
        let q = Quaternion::from_axis_angle(Vector3f::new(1.0, 1.0, 0.0), 0.7);
        let v = Vector3f::new(1.0, 2.0, 3.0);

        assert_vector3_eq(q.conjugate() * (q * v), v);
    }

    #[test]
    fn from_rotation_axes_round_trips() {
        for q in [
            Quaternion::IDENTITY,
            Quaternion::from_axis_angle(Vector3f::X, 3.0),
            Quaternion::from_axis_angle(Vector3f::Y, -2.5),
            Quaternion::from_axis_angle(Vector3f::new(1.0, -2.0, 0.5), 1.2),
        ] {
            let restored = Quaternion::from_rotation_axes(q * Vector3f::X, q * Vector3f::Y, q * Vector3f::Z);
            let v = Vector3f::new(0.3, -1.0, 2.0);

            assert_vector3_eq(restored * v, q * v);
        }
    }
}
