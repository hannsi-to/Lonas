use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

macro_rules! impl_vector {
    ($name:ident, $len:literal, $($field:ident),+) => {
        impl $name {
            pub const ZERO: Self = Self { $($field: 0.0),+ };
            pub const ONE: Self = Self { $($field: 1.0),+ };

            pub const fn new($($field: f32),+) -> Self {
                Self { $($field),+ }
            }

            pub fn dot(self, other: Self) -> f32 {
                0.0 $(+ self.$field * other.$field)+
            }

            pub fn length_squared(self) -> f32 {
                self.dot(self)
            }

            pub fn length(self) -> f32 {
                self.length_squared().sqrt()
            }

            // 長さ 0 のベクトルは正規化できないため None
            pub fn try_normalize(self) -> Option<Self> {
                let length = self.length();

                if length > f32::EPSILON {
                    Some(self / length)
                } else {
                    None
                }
            }

            pub fn normalize(self) -> Self {
                self.try_normalize().expect("長さ 0 のベクトルは正規化できません")
            }

            pub fn to_array(self) -> [f32; $len] {
                [$(self.$field),+]
            }
        }

        impl From<[f32; $len]> for $name {
            fn from([$($field),+]: [f32; $len]) -> Self {
                Self { $($field),+ }
            }
        }

        impl From<$name> for [f32; $len] {
            fn from(vector: $name) -> Self {
                vector.to_array()
            }
        }

        impl Add for $name {
            type Output = Self;

            fn add(self, other: Self) -> Self {
                Self { $($field: self.$field + other.$field),+ }
            }
        }

        impl Sub for $name {
            type Output = Self;

            fn sub(self, other: Self) -> Self {
                Self { $($field: self.$field - other.$field),+ }
            }
        }

        impl Mul<f32> for $name {
            type Output = Self;

            fn mul(self, scalar: f32) -> Self {
                Self { $($field: self.$field * scalar),+ }
            }
        }

        impl Mul<$name> for f32 {
            type Output = $name;

            fn mul(self, vector: $name) -> $name {
                vector * self
            }
        }

        // 要素ごとの掛け算（拡大率の適用など）
        impl Mul for $name {
            type Output = Self;

            fn mul(self, other: Self) -> Self {
                Self { $($field: self.$field * other.$field),+ }
            }
        }

        impl Div<f32> for $name {
            type Output = Self;

            fn div(self, scalar: f32) -> Self {
                Self { $($field: self.$field / scalar),+ }
            }
        }

        impl Neg for $name {
            type Output = Self;

            fn neg(self) -> Self {
                Self { $($field: -self.$field),+ }
            }
        }

        impl AddAssign for $name {
            fn add_assign(&mut self, other: Self) {
                *self = *self + other;
            }
        }

        impl SubAssign for $name {
            fn sub_assign(&mut self, other: Self) {
                *self = *self - other;
            }
        }

        impl MulAssign<f32> for $name {
            fn mul_assign(&mut self, scalar: f32) {
                *self = *self * scalar;
            }
        }

        impl DivAssign<f32> for $name {
            fn div_assign(&mut self, scalar: f32) {
                *self = *self / scalar;
            }
        }
    };
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vector2f {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vector4f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl_vector!(Vector2f, 2, x, y);
impl_vector!(Vector3f, 3, x, y, z);
impl_vector!(Vector4f, 4, x, y, z, w);

impl Vector3f {
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn extend(self, w: f32) -> Vector4f {
        Vector4f::new(self.x, self.y, self.z, w)
    }
}

impl Vector2f {
    pub fn extend(self, z: f32) -> Vector3f {
        Vector3f::new(self.x, self.y, z)
    }
}

impl Vector4f {
    pub fn truncate(self) -> Vector3f {
        Vector3f::new(self.x, self.y, self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let a = Vector3f::new(1.0, 2.0, 3.0);
        let b = Vector3f::new(4.0, 5.0, 6.0);

        assert_eq!(a + b, Vector3f::new(5.0, 7.0, 9.0));
        assert_eq!(b - a, Vector3f::new(3.0, 3.0, 3.0));
        assert_eq!(a * 2.0, Vector3f::new(2.0, 4.0, 6.0));
        assert_eq!(2.0 * a, a * 2.0);
        assert_eq!(a * b, Vector3f::new(4.0, 10.0, 18.0));
        assert_eq!(-a, Vector3f::new(-1.0, -2.0, -3.0));
        assert_eq!(a.dot(b), 32.0);
    }

    #[test]
    fn cross_follows_right_hand_rule() {
        assert_eq!(Vector3f::X.cross(Vector3f::Y), Vector3f::Z);
        assert_eq!(Vector3f::Y.cross(Vector3f::Z), Vector3f::X);
    }

    #[test]
    fn normalize() {
        let v = Vector2f::new(3.0, 4.0);

        assert_eq!(v.length(), 5.0);
        assert_eq!(v.normalize(), Vector2f::new(0.6, 0.8));
        assert_eq!(Vector2f::ZERO.try_normalize(), None);
    }

    #[test]
    fn array_conversion() {
        assert_eq!(Vector2f::from([1.0, 2.0]), Vector2f::new(1.0, 2.0));
        assert_eq!(<[f32; 3]>::from(Vector3f::new(1.0, 2.0, 3.0)), [1.0, 2.0, 3.0]);
    }
}
