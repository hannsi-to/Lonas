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
pub(crate) const GLYPH_RECT_SIZE: u32 = 32;

// グリフのアトラスを読む関数（§13 シェーダー）。STRUCTS と同じく、自分の WGSL の先頭につなげる
// アトラス・サンプラー・表の要素を引数で受け取る（WGSL ではストレージの配列を関数に渡しにくいため）
//
// let rect = glyph_rects[slot];
// let coverage = lonas_glyph_bitmap(glyph_atlas_r8, glyph_sampler, rect, in.uv);
pub const GLYPH: &str = r#"
struct GlyphRect {
    rect: vec4<f32>,
    layer: u32,
    // Sdf / Msdf：距離の幅（画素）と 1 em の大きさ（画素）
    distance_range: f32,
    em_size: f32,
    _reserved: u32,
}

// Bitmap：覆い率（0〜1）
fn lonas_glyph_bitmap(atlas: texture_2d_array<f32>, atlas_sampler: sampler, rect: GlyphRect, uv: vec2<f32>) -> f32 {
    return textureSample(atlas, atlas_sampler, rect.rect.xy + uv * rect.rect.zw, rect.layer).r;
}

// 距離（画素、内側が正）を、画面上の 1 画素の幅でぼかして覆い率にする
fn lonas_glyph_distance_coverage(value: f32, rect: GlyphRect, thickening: f32) -> f32 {
    // 番号 0（塗りつぶし）は距離の幅を持たない。値は 1 のため内側
    let range = select(rect.distance_range, 1.0, rect.distance_range == 0.0);
    let distance = (value - 0.5) * range + thickening * rect.em_size;
    let width = max(fwidth(distance), 1e-4);
    return clamp(distance / width + 0.5, 0.0, 1.0);
}

// Sdf：1 チャンネルの距離。thickening は境目を外にずらす量（em、normal.y）
fn lonas_glyph_sdf(atlas: texture_2d_array<f32>, atlas_sampler: sampler, rect: GlyphRect, uv: vec2<f32>, thickening: f32) -> f32 {
    let value = textureSample(atlas, atlas_sampler, rect.rect.xy + uv * rect.rect.zw, rect.layer).r;
    return lonas_glyph_distance_coverage(value, rect, thickening);
}

fn lonas_median(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

// Msdf：MTSDF の rgb の中央値で角を保つ。thickening は境目を外にずらす量（em、normal.y）
fn lonas_glyph_msdf(atlas: texture_2d_array<f32>, atlas_sampler: sampler, rect: GlyphRect, uv: vec2<f32>, thickening: f32) -> f32 {
    let sample = textureSample(atlas, atlas_sampler, rect.rect.xy + uv * rect.rect.zw, rect.layer);
    return lonas_glyph_distance_coverage(lonas_median(sample.r, sample.g, sample.b), rect, thickening);
}
"#;

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
    let (camera, instances, order) = (bindings.camera, bindings.instances, bindings.instance_order);
    let common = format!(
        "{STRUCTS}
@group({}) @binding({}) var<uniform> camera: Camera;
@group({}) @binding({}) var<storage, read> instances: array<{instance}>;
@group({}) @binding({}) var<storage, read> instance_order: array<u32>;
",
        camera.group, camera.binding, instances.group, instances.binding, order.group, order.binding,
    );

    match method {
        GlyphMethod::Vector => format!(
            "{common}
struct LonasTextOut {{ @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> }}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>, @builtin(instance_index) index: u32) -> LonasTextOut {{
    let inst = instances[instance_order[index]];
    {world}
    return LonasTextOut(camera.view_projection * world, color * inst.color);
}}

@fragment
fn fs(in: LonasTextOut) -> @location(0) vec4<f32> {{ return in.color; }}
"
        ),
        // 頂点の色 × 覆い率。VertexLayout::PositionColorUvNormal で描く（normal.x がグリフの番号、normal.y が太らせる量）
        GlyphMethod::Bitmap { .. } | GlyphMethod::Sdf | GlyphMethod::Msdf => {
            method.check_feature();

            let (sampler, rects) = (bindings.glyph_sampler, bindings.glyph_rects);
            let (atlas, coverage) = match method {
                GlyphMethod::Msdf => (bindings.glyph_atlas_rgba, "lonas_glyph_msdf(glyph_atlas, glyph_sampler, glyph_rects[in.slot], in.uv, in.thickening)"),
                GlyphMethod::Sdf => (bindings.glyph_atlas_r8, "lonas_glyph_sdf(glyph_atlas, glyph_sampler, glyph_rects[in.slot], in.uv, in.thickening)"),
                _ => (bindings.glyph_atlas_r8, "lonas_glyph_bitmap(glyph_atlas, glyph_sampler, glyph_rects[in.slot], in.uv)"),
            };

            format!(
                "{common}{GLYPH}
@group({}) @binding({}) var glyph_atlas: texture_2d_array<f32>;
@group({}) @binding({}) var glyph_sampler: sampler;
@group({}) @binding({}) var<storage, read> glyph_rects: array<GlyphRect>;

struct LonasTextOut {{
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) slot: u32,
    @location(3) thickening: f32,
}}

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
    @builtin(instance_index) index: u32,
) -> LonasTextOut {{
    let inst = instances[instance_order[index]];
    {world}
    return LonasTextOut(camera.view_projection * world, color * inst.color, uv, u32(normal.x + 0.5), normal.y);
}}

@fragment
fn fs(in: LonasTextOut) -> @location(0) vec4<f32> {{
    let coverage = {coverage};
    return vec4(in.color.rgb, in.color.a * coverage);
}}
",
                atlas.group, atlas.binding, sampler.group, sampler.binding, rects.group, rects.binding,
            )
        }
    }
}
