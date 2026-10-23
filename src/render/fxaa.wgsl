// FXAA（§11 アンチエイリアス）：描き終えた画像の輪郭をぼかす後処理
// 明るさの差から輪郭の向きを求め、その向きに沿って混ぜる（FXAA 3.11 の簡易版）

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3(0.299, 0.587, 0.114));
}

fn sample(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, source_sampler, uv, 0.0).rgb;
}

const SPAN_MAX: f32 = 8.0;
const REDUCE_MUL: f32 = 1.0 / 8.0;
const REDUCE_MIN: f32 = 1.0 / 128.0;

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    let uv = position.xy * texel;
    let center = textureSampleLevel(source, source_sampler, uv, 0.0);
    let luma_m = luma(center.rgb);
    let luma_nw = luma(sample(uv + vec2(-1.0, -1.0) * texel));
    let luma_ne = luma(sample(uv + vec2(1.0, -1.0) * texel));
    let luma_sw = luma(sample(uv + vec2(-1.0, 1.0) * texel));
    let luma_se = luma(sample(uv + vec2(1.0, 1.0) * texel));
    let luma_min = min(luma_m, min(min(luma_nw, luma_ne), min(luma_sw, luma_se)));
    let luma_max = max(luma_m, max(max(luma_nw, luma_ne), max(luma_sw, luma_se)));

    var direction = vec2(-((luma_nw + luma_ne) - (luma_sw + luma_se)), (luma_nw + luma_sw) - (luma_ne + luma_se));
    let reduce = max((luma_nw + luma_ne + luma_sw + luma_se) * 0.25 * REDUCE_MUL, REDUCE_MIN);
    let scale = 1.0 / (min(abs(direction.x), abs(direction.y)) + reduce);
    direction = clamp(direction * scale, vec2(-SPAN_MAX), vec2(SPAN_MAX)) * texel;

    let near = 0.5 * (sample(uv + direction * (1.0 / 3.0 - 0.5)) + sample(uv + direction * (2.0 / 3.0 - 0.5)));
    let far = near * 0.5 + 0.25 * (sample(uv - direction * 0.5) + sample(uv + direction * 0.5));
    let luma_far = luma(far);

    // 遠くまで混ぜた色が周りの明るさの範囲を外れたら、輪郭を越えているため近くだけを混ぜる
    if (luma_far < luma_min || luma_far > luma_max) {
        return vec4(near, center.a);
    }

    return vec4(far, center.a);
}
