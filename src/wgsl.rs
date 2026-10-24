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

// text_shader が作るシェーダーの次元（§13 シェーダー）
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Dimension {
    D2,
    D3,
}

// テキスト用の完成品のシェーダー（§13 シェーダー）。バインディング番号が BindingConfig 次第のため、設定から WGSL を作る
// Vector 用は頂点の色（× インスタンスの色）をそのまま出す。STRUCTS を含むため、そのまま register_shader に渡せる
//
// let src = lonas::wgsl::text_shader(&config.bindings, GlyphMethod::Vector, Dimension::D2);
// let shader = resource_manager.register_shader(&src)?;
#[cfg(feature = "text")]
pub fn text_shader(bindings: &crate::config::BindingConfig, method: crate::text::GlyphMethod, dimension: Dimension) -> String {
    use crate::text::GlyphMethod;

    let (instance, world) = match dimension {
        Dimension::D2 => (
            "Instance2D",
            "let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let local = position.xy * inst.scale;
    let world = vec4(vec2(local.x * c - local.y * s, local.x * s + local.y * c) + inst.position, position.z, 1.0);",
        ),
        Dimension::D3 => ("Instance3D", "let world = inst.transform * vec4(position, 1.0);"),
    };
    let fragment = match method {
        GlyphMethod::Vector => "return in.color;",
    };
    let (camera, instances, order) = (bindings.camera, bindings.instances, bindings.instance_order);

    format!(
        "{STRUCTS}
@group({}) @binding({}) var<uniform> camera: Camera;
@group({}) @binding({}) var<storage, read> instances: array<{instance}>;
@group({}) @binding({}) var<storage, read> instance_order: array<u32>;

struct LonasTextOut {{ @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @builtin(instance_index) index: u32) -> LonasTextOut {{
    let inst = instances[instance_order[index]];
    {world}
    return LonasTextOut(camera.view_projection * world, color * inst.color);
}}

@fragment
fn fs(in: LonasTextOut) -> @location(0) vec4<f32> {{ {fragment} }}
",
        camera.group, camera.binding, instances.group, instances.binding, order.group, order.binding,
    )
}
