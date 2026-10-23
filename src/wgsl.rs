// ユーザーのシェーダーと Lonas の間の約束事（§11 シェーダーとの約束事）

// ユーザーが自分の WGSL の先頭につなげて登録する struct の定義
// バインディングの宣言は番号が BindingConfig 次第のため、ユーザーが書く
//
// let src = format!("{}{}", lonas::wgsl::STRUCTS, MY_WGSL);
// let shader = resource_manager.register_shader(&src)?;
pub const STRUCTS: &str = r#"
struct Instance2D {
    color: vec4<f32>,
    position: vec2<f32>,
    scale: vec2<f32>,
    rotation: f32,
    visibility: u32,
    texture_slot: u32,
    uniform_index: u32,
}

struct Instance3D {
    transform: mat4x4<f32>,
    color: vec4<f32>,
    visibility: u32,
    texture_slot: u32,
    uniform_index: u32,
    _pad: u32,
}

struct Camera {
    view_projection: mat4x4<f32>,
    view_projection_no_jitter: mat4x4<f32>,
    view: mat4x4<f32>,
    position: vec4<f32>,
    viewport_size: vec2<f32>,
    jitter: vec2<f32>,
}

struct TextureRect {
    rect: vec4<f32>,
    layer: u32,
}
"#;

// GPU 上の大きさ（バイト）。Rust 側の詰め方（instance.rs）と、シェーダーの検査（段階 6）で使う
pub(crate) const INSTANCE_2D_SIZE: u32 = 48;
pub(crate) const INSTANCE_3D_SIZE: u32 = 96;
#[allow(dead_code)] // 段階 6（カメラの転送）で使う
pub(crate) const CAMERA_SIZE: u32 = 224;
#[allow(dead_code)] // 段階 7（texture_rects）で使う
pub(crate) const TEXTURE_RECT_SIZE: u32 = 32;
