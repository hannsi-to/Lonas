use std::ops::Mul;

use super::{Quaternion, Vector2f, Vector3f, Vector4f};
use crate::config::Handedness;

// 列優先の 4×4 行列。WGSL の mat4x4<f32> と同じ並びのため、そのまま GPU に送れる
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Matrix4f {
    pub cols: [Vector4f; 4],
}

impl Default for Matrix4f {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix4f {
    pub const IDENTITY: Self = Self::from_cols(
        Vector4f::new(1.0, 0.0, 0.0, 0.0),
        Vector4f::new(0.0, 1.0, 0.0, 0.0),
        Vector4f::new(0.0, 0.0, 1.0, 0.0),
        Vector4f::new(0.0, 0.0, 0.0, 1.0),
    );

    pub const fn from_cols(x: Vector4f, y: Vector4f, z: Vector4f, w: Vector4f) -> Self {
        Self { cols: [x, y, z, w] }
    }

    pub fn from_translation(translation: Vector3f) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.cols[3] = translation.extend(1.0);

        matrix
    }

    pub fn from_rotation(rotation: Quaternion) -> Self {
        Self::from_cols(
            (rotation * Vector3f::X).extend(0.0),
            (rotation * Vector3f::Y).extend(0.0),
            (rotation * Vector3f::Z).extend(0.0),
            Vector4f::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    pub fn from_scale(scale: Vector3f) -> Self {
        Self::from_cols(
            Vector4f::new(scale.x, 0.0, 0.0, 0.0),
            Vector4f::new(0.0, scale.y, 0.0, 0.0),
            Vector4f::new(0.0, 0.0, scale.z, 0.0),
            Vector4f::new(0.0, 0.0, 0.0, 1.0),
        )
    }

    // 拡大 → 回転 → 平行移動の順に掛ける（T * R * S）
    pub fn from_trs(translation: Vector3f, rotation: Quaternion, scale: Vector3f) -> Self {
        let rotation = Self::from_rotation(rotation);

        Self::from_cols(
            rotation.cols[0] * scale.x,
            rotation.cols[1] * scale.y,
            rotation.cols[2] * scale.z,
            translation.extend(1.0),
        )
    }

    // 位置と向きからビュー行列（ワールド → カメラ）を作る。from_trs(position, rotation, ONE) の逆行列
    pub fn view(position: Vector3f, rotation: Quaternion) -> Self {
        Self::from_rotation(rotation.conjugate()) * Self::from_translation(-position)
    }

    // eye から target を見るビュー行列（§11 Camera の look_at）
    pub fn look_at(eye: Vector3f, target: Vector3f, up: Vector3f, handedness: Handedness) -> Self {
        Self::view(eye, Quaternion::look_rotation(target - eye, up, handedness))
    }

    // 透視投影。深度は Reversed-Z（near が 1.0、far が 0.0、§11 深度バッファ）
    pub fn perspective(fov_y: f32, aspect_ratio: f32, near: f32, far: f32, handedness: Handedness) -> Self {
        let f = 1.0 / (fov_y * 0.5).tan();
        let depth = near / (far - near);
        // カメラが向く方向の奥行きを w に入れる。右手系は -Z を向くため符号が逆
        let sign = match handedness {
            Handedness::RightHanded => -1.0,
            Handedness::LeftHanded => 1.0,
        };

        Self::from_cols(
            Vector4f::new(f / aspect_ratio, 0.0, 0.0, 0.0),
            Vector4f::new(0.0, f, 0.0, 0.0),
            Vector4f::new(0.0, 0.0, -sign * depth, sign),
            Vector4f::new(0.0, 0.0, far * depth, 0.0),
        )
    }

    // 中心を原点とする正射影。深度は Reversed-Z
    pub fn orthographic(width: f32, height: f32, near: f32, far: f32, handedness: Handedness) -> Self {
        let (half_width, half_height) = (width * 0.5, height * 0.5);

        Self::orthographic_off_center(-half_width, half_width, -half_height, half_height, near, far, handedness)
    }

    // 範囲を指定する正射影（2D の原点の位置に使う、§11 座標系）。深度は Reversed-Z
    pub fn orthographic_off_center(
        left: f32,
        right: f32,
        bottom: f32,
        top: f32,
        near: f32,
        far: f32,
        handedness: Handedness,
    ) -> Self {
        let (width, height, depth) = (right - left, top - bottom, far - near);
        let sign = match handedness {
            Handedness::RightHanded => -1.0,
            Handedness::LeftHanded => 1.0,
        };

        Self::from_cols(
            Vector4f::new(2.0 / width, 0.0, 0.0, 0.0),
            Vector4f::new(0.0, 2.0 / height, 0.0, 0.0),
            Vector4f::new(0.0, 0.0, -sign / depth, 0.0),
            Vector4f::new(-(right + left) / width, -(top + bottom) / height, far / depth, 1.0),
        )
    }

    pub fn row(&self, index: usize) -> Vector4f {
        let [x, y, z, w] = self.cols.map(|col| col.to_array()[index]);

        Vector4f::new(x, y, z, w)
    }

    pub fn transpose(&self) -> Self {
        Self::from_cols(self.row(0), self.row(1), self.row(2), self.row(3))
    }

    // 逆行列がない（行列式が 0）なら None
    pub fn inverse(&self) -> Option<Self> {
        let m = self.to_cols_array();
        // 2×2 の小行列式から余因子を求める
        let s0 = m[0] * m[5] - m[4] * m[1];
        let s1 = m[0] * m[6] - m[4] * m[2];
        let s2 = m[0] * m[7] - m[4] * m[3];
        let s3 = m[1] * m[6] - m[5] * m[2];
        let s4 = m[1] * m[7] - m[5] * m[3];
        let s5 = m[2] * m[7] - m[6] * m[3];
        let c5 = m[10] * m[15] - m[14] * m[11];
        let c4 = m[9] * m[15] - m[13] * m[11];
        let c3 = m[9] * m[14] - m[13] * m[10];
        let c2 = m[8] * m[15] - m[12] * m[11];
        let c1 = m[8] * m[14] - m[12] * m[10];
        let c0 = m[8] * m[13] - m[12] * m[9];
        let determinant = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;

        if determinant.abs() <= f32::EPSILON * f32::EPSILON {
            return None;
        }

        let inv = 1.0 / determinant;
        let r = [
            (m[5] * c5 - m[6] * c4 + m[7] * c3) * inv,
            (-m[1] * c5 + m[2] * c4 - m[3] * c3) * inv,
            (m[13] * s5 - m[14] * s4 + m[15] * s3) * inv,
            (-m[9] * s5 + m[10] * s4 - m[11] * s3) * inv,
            (-m[4] * c5 + m[6] * c2 - m[7] * c1) * inv,
            (m[0] * c5 - m[2] * c2 + m[3] * c1) * inv,
            (-m[12] * s5 + m[14] * s2 - m[15] * s1) * inv,
            (m[8] * s5 - m[10] * s2 + m[11] * s1) * inv,
            (m[4] * c4 - m[5] * c2 + m[7] * c0) * inv,
            (-m[0] * c4 + m[1] * c2 - m[3] * c0) * inv,
            (m[12] * s4 - m[13] * s2 + m[15] * s0) * inv,
            (-m[8] * s4 + m[9] * s2 - m[11] * s0) * inv,
            (-m[4] * c3 + m[5] * c1 - m[6] * c0) * inv,
            (m[0] * c3 - m[1] * c1 + m[2] * c0) * inv,
            (-m[12] * s3 + m[13] * s1 - m[14] * s0) * inv,
            (m[8] * s3 - m[9] * s1 + m[10] * s0) * inv,
        ];

        Some(Self::from_cols_array(r))
    }

    // 点を変換する（w = 1 として掛け、w で割る）。投影行列にも使える
    pub fn transform_point3(&self, point: Vector3f) -> Vector3f {
        let result = *self * point.extend(1.0);

        result.truncate() / result.w
    }

    // 2D の点を変換する（z = 0 として扱う）。マウス位置の逆変換など
    pub fn transform_point2(&self, point: Vector2f) -> Vector2f {
        let result = self.transform_point3(point.extend(0.0));

        Vector2f::new(result.x, result.y)
    }

    // 向きを変換する（平行移動を含めない）
    pub fn transform_vector3(&self, vector: Vector3f) -> Vector3f {
        (*self * vector.extend(0.0)).truncate()
    }

    pub fn to_cols_array(&self) -> [f32; 16] {
        let [x, y, z, w] = self.cols.map(Vector4f::to_array);

        [
            x[0], x[1], x[2], x[3], y[0], y[1], y[2], y[3], z[0], z[1], z[2], z[3], w[0], w[1], w[2], w[3],
        ]
    }

    pub fn from_cols_array(m: [f32; 16]) -> Self {
        Self::from_cols(
            Vector4f::new(m[0], m[1], m[2], m[3]),
            Vector4f::new(m[4], m[5], m[6], m[7]),
            Vector4f::new(m[8], m[9], m[10], m[11]),
            Vector4f::new(m[12], m[13], m[14], m[15]),
        )
    }
}

impl Mul for Matrix4f {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self { cols: other.cols.map(|col| self * col) }
    }
}

impl Mul<Vector4f> for Matrix4f {
    type Output = Vector4f;

    fn mul(self, vector: Vector4f) -> Vector4f {
        self.cols[0] * vector.x + self.cols[1] * vector.y + self.cols[2] * vector.z + self.cols[3] * vector.w
    }
}

impl Quaternion {
    // forward の方向を向く回転（§11 Camera の look_at）。右手系はカメラの -Z、左手系は +Z を forward に合わせる
    pub fn look_rotation(forward: Vector3f, up: Vector3f, handedness: Handedness) -> Self {
        let forward = forward.normalize();

        match handedness {
            Handedness::RightHanded => {
                let right = forward.cross(up).normalize();
                let up = right.cross(forward);

                Self::from_rotation_axes(right, up, -forward)
            }
            Handedness::LeftHanded => {
                let right = up.cross(forward).normalize();
                let up = forward.cross(right);

                Self::from_rotation_axes(right, up, forward)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tests::{assert_matrix_eq, assert_near, assert_vector3_eq};
    use std::f32::consts::FRAC_PI_2;

    fn sample() -> Matrix4f {
        Matrix4f::from_trs(
            Vector3f::new(1.0, -2.0, 3.0),
            Quaternion::from_axis_angle(Vector3f::new(1.0, 2.0, 3.0), 0.8),
            Vector3f::new(2.0, 0.5, 1.5),
        )
    }

    #[test]
    fn identity_does_nothing() {
        let v = Vector3f::new(1.0, 2.0, 3.0);

        assert_eq!(Matrix4f::IDENTITY.transform_point3(v), v);
        assert_eq!(Matrix4f::IDENTITY * sample(), sample());
    }

    #[test]
    fn trs_applies_scale_then_rotation_then_translation() {
        let translation = Vector3f::new(1.0, 2.0, 3.0);
        let rotation = Quaternion::from_axis_angle(Vector3f::Z, FRAC_PI_2);
        let scale = Vector3f::new(2.0, 2.0, 2.0);
        let expected = Matrix4f::from_translation(translation) * Matrix4f::from_rotation(rotation) * Matrix4f::from_scale(scale);

        assert_matrix_eq(Matrix4f::from_trs(translation, rotation, scale), expected);
        // X 方向の 1 は、2 倍 → Z 軸まわりに 90 度で Y 方向の 2 → 平行移動
        assert_vector3_eq(expected.transform_point3(Vector3f::X), Vector3f::new(1.0, 4.0, 3.0));
    }

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m = sample();
        let inverse = m.inverse().unwrap();

        assert_matrix_eq(inverse * m, Matrix4f::IDENTITY);
        assert_matrix_eq(m * inverse, Matrix4f::IDENTITY);
    }

    #[test]
    fn singular_matrix_has_no_inverse() {
        assert_eq!(Matrix4f::from_scale(Vector3f::new(1.0, 0.0, 1.0)).inverse(), None);
    }

    #[test]
    fn transpose_swaps_rows_and_columns() {
        let m = sample();

        assert_eq!(m.transpose().row(0), m.cols[0]);
        assert_eq!(m.transpose().transpose(), m);
    }

    #[test]
    fn perspective_is_reversed_z() {
        for (handedness, forward) in [(Handedness::RightHanded, -1.0), (Handedness::LeftHanded, 1.0)] {
            let projection = Matrix4f::perspective(1.0, 16.0 / 9.0, 0.1, 100.0, handedness);

            assert_near(projection.transform_point3(Vector3f::new(0.0, 0.0, 0.1 * forward)).z, 1.0);
            assert_near(projection.transform_point3(Vector3f::new(0.0, 0.0, 100.0 * forward)).z, 0.0);
        }
    }

    #[test]
    fn orthographic_is_reversed_z_and_maps_edges() {
        for (handedness, forward) in [(Handedness::RightHanded, -1.0), (Handedness::LeftHanded, 1.0)] {
            let projection = Matrix4f::orthographic_off_center(0.0, 800.0, 600.0, 0.0, 0.1, 100.0, handedness);

            assert_vector3_eq(projection.transform_point3(Vector3f::new(0.0, 0.0, 0.1 * forward)), Vector3f::new(-1.0, 1.0, 1.0));
            assert_vector3_eq(projection.transform_point3(Vector3f::new(800.0, 600.0, 100.0 * forward)), Vector3f::new(1.0, -1.0, 0.0));
        }
    }

    #[test]
    fn look_at_puts_target_in_front_of_camera() {
        let eye = Vector3f::new(3.0, 4.0, 5.0);
        let target = Vector3f::new(-1.0, 0.0, 2.0);
        let distance = (target - eye).length();

        for (handedness, forward) in [(Handedness::RightHanded, -1.0), (Handedness::LeftHanded, 1.0)] {
            let view = Matrix4f::look_at(eye, target, Vector3f::Y, handedness);

            assert_vector3_eq(view.transform_point3(eye), Vector3f::ZERO);
            assert_vector3_eq(view.transform_point3(target), Vector3f::new(0.0, 0.0, distance * forward));
            // 上方向はカメラの +Y 側に残る
            assert!(view.transform_vector3(Vector3f::Y).y > 0.0);
        }
    }

    #[test]
    fn look_at_keeps_right_handed_axes() {
        let view = Matrix4f::look_at(Vector3f::ZERO, -Vector3f::Z, Vector3f::Y, Handedness::RightHanded);

        // 既定の向き（-Z）を向くなら単位行列
        assert_matrix_eq(view, Matrix4f::IDENTITY);
    }
}
