// LonasConfig の中で使う型（§3・§11）。LonasConfig 本体は段階 4 で作る

// 3D の手系（§11 座標系）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Handedness {
    // カメラは -Z を向く
    RightHanded,
    // カメラは +Z を向く
    LeftHanded,
}

// 3D の上方向（§11 座標系）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpAxis {
    Y,
    // Lonas の内部では Y が上の座標に回してから扱う（Matrix4f::up_axis_to_y）
    Z,
}
