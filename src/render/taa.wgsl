// TAA（§11 TAA の処理の流れ）
// 1. 取得：現在のフレームの色・深度・動きのベクトル
// 2. 再投影：動きのベクトルで、前のフレームの履歴から対応する位置の色を読む
// 3. 信頼度の評価：画面外・深度差・色の不一致（YCoCg での分散クリップ）
// 4. 合成：信頼度に応じて履歴を混ぜる
// 5. 保存：合成結果を次のフレーム用の履歴として書く（深度の保存は CPU 側でコピーする）
//
// 動きのベクトルは UV の差（現在の UV − 前のフレームの UV）。ユーザーのシェーダーが @location(1) に書く。
// 書かないシェーダーの部分は 0（動いていない）として扱う

struct Params {
    history_blend_min: f32,
    history_blend_max: f32,
    depth_rejection_threshold: f32,
    color_rejection_threshold: f32,
    history_valid: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var current: texture_2d<f32>;
@group(0) @binding(1) var history: texture_2d<f32>;
@group(0) @binding(2) var motion: texture_2d<f32>;
@group(0) @binding(3) var depth: texture_depth_2d;
@group(0) @binding(4) var previous_depth: texture_depth_2d;
@group(0) @binding(5) var linear_sampler: sampler;
@group(0) @binding(6) var<uniform> params: Params;

struct Out {
    @location(0) color: vec4<f32>,
    @location(1) history: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn from_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> Out {
    let size = vec2<i32>(textureDimensions(current));
    let coord = vec2<i32>(position.xy);
    let uv = position.xy / vec2<f32>(size);
    let color = textureLoad(current, coord, 0);

    // カットや解像度の変更の直後は、現在の画像から履歴を作り直す（§11 無効化）
    if (params.history_valid == 0u) {
        return Out(color, color);
    }

    // 周囲 3×3 の色の平均と標準偏差（YCoCg）
    var sum = vec3(0.0);
    var sum_squared = vec3(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let c = to_ycocg(textureLoad(current, clamp(coord + vec2(x, y), vec2(0), size - 1), 0).rgb);
            sum += c;
            sum_squared += c * c;
        }
    }
    let mean = sum / 9.0;
    let sigma = sqrt(max(sum_squared / 9.0 - mean * mean, vec3(0.0)));

    // 再投影
    let previous_uv = uv - textureLoad(motion, coord, 0).xy;
    var confidence = 1.0;

    // 画面外判定：前のフレームの画面の外なら信頼しない
    if (any(previous_uv < vec2(0.0)) || any(previous_uv > vec2(1.0))) {
        confidence = 0.0;
    }

    let history_color = textureSampleLevel(history, linear_sampler, clamp(previous_uv, vec2(0.0), vec2(1.0)), 0.0);

    // 深度差：前のフレームと大きく違えば、別の物が見えていたとみなす
    let previous_coord = clamp(vec2<i32>(previous_uv * vec2<f32>(size)), vec2(0), size - 1);
    if (abs(textureLoad(depth, coord, 0) - textureLoad(previous_depth, previous_coord, 0)) > params.depth_rejection_threshold) {
        confidence *= 0.25;
    }

    // 色の不一致：履歴の色を μ ± γσ の範囲へ、平均に向かう線分上で切り詰める
    let h = to_ycocg(history_color.rgb);
    let extent = params.color_rejection_threshold * sigma + vec3(1e-4);
    let offset = h - mean;
    let ratio = max(max(abs(offset.x) / extent.x, abs(offset.y) / extent.y), abs(offset.z) / extent.z);
    var clipped = h;
    if (ratio > 1.0) {
        clipped = mean + offset / ratio;
        confidence *= 1.0 / ratio;
    }

    // 合成：信頼度が高いほど履歴を多く使う
    let blend = mix(params.history_blend_min, params.history_blend_max, clamp(confidence, 0.0, 1.0));
    let result = vec4(mix(color.rgb, from_ycocg(clipped), blend), color.a);

    return Out(result, result);
}
