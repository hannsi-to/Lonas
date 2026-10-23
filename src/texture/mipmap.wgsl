// ミップマップを 1 段作る（§11 ミップマップ）。1 つ上の段を線形補間で読み、2×2 の平均にする
// 描く範囲はビューポートで、登録したテクスチャの範囲（余白を含む）に絞る

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // 画面全体を覆う三角形
    let uv = vec2(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // 作る段の画素の中心は、1 つ上の段の 2×2 の画素の境目に当たる。そこを線形補間で読めば 4 つの平均になる
    let size = vec2<f32>(textureDimensions(source)) * 0.5;
    return textureSampleLevel(source, source_sampler, position.xy / size, 0.0);
}
