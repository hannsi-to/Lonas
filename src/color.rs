// 色（§11 色）。sRGB の 0.0〜1.0 で受け取り、GPU に送るときに線形へ変換する
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }

    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
    }

    // GPU に送る線形の値。透明度は変換しない
    pub fn to_linear(self) -> [f32; 4] {
        [srgb_to_linear(self.r), srgb_to_linear(self.g), srgb_to_linear(self.b), self.a]
    }
}

// sRGB の規格どおりの変換（0.04045 以下は直線、それより上は 2.4 乗）
pub(crate) fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tests::assert_near;

    #[test]
    fn from_rgba8_scales_to_unit_range() {
        assert_eq!(Color::from_rgba8(255, 0, 255, 255), Color::new(1.0, 0.0, 1.0, 1.0));
        assert_eq!(Color::rgb(0.5, 0.5, 0.5).a, 1.0);
    }

    #[test]
    fn to_linear_matches_srgb_curve() {
        let [r, g, b, a] = Color::new(0.0, 0.5, 1.0, 0.5).to_linear();

        assert_near(r, 0.0);
        assert_near(g, 0.21404);
        assert_near(b, 1.0);
        // 透明度は変換しない
        assert_eq!(a, 0.5);
        // 直線の部分
        assert_near(srgb_to_linear(0.02), 0.02 / 12.92);
    }
}
