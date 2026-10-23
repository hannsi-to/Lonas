// 視界の外・非表示のインスタンスを省き、並べ替え、インスタンス番号のリストと描画の引数を書く（§10 描画順）
//
// cull_compact          : 2D と 3D の不透明。Object ごとに 1 つのワークグループ
//                         2D はスロットの順のまま詰める。3D の不透明は奥行きの大まかな並べ替え（バケット）
// transparent_keys      : 3D の半透明。パイプラインごとに、Object をまたいだ全インスタンスの奥行きを鍵にする
// bitonic_step          : 鍵の並べ替え（バイトニックソートの 1 段）
// transparent_emit      : 並べた順にインスタンスごとの描画の引数を書く

struct DrawArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

struct ObjectData {
    instance_base: u32,   // インスタンスのヒープ上の先頭（要素の番号）
    instance_count: u32,  // スロットの数（穴を含む）
    order_base: u32,      // インスタンス番号のリストの先頭
    args_index: u32,      // 描画の引数の番号（不透明・2D）
    index_count: u32,
    first_index: u32,
    base_vertex: i32,
    camera: u32,
    bounds: vec4<f32>,    // ローカル空間の境界（中心 xyz と半径）
    flags: u32,
    sort_base: u32,       // 半透明：パイプラインの中での、この Object の最初のインスタンスの位置
    _pad0: u32,
    _pad1: u32,
}

struct CameraCull {
    view_projection: mat4x4<f32>,
    depth_range: vec4<f32>,  // near, far
}

struct Params {
    instance_words: u32,     // 1 インスタンスの u32 の数（2D は 12、3D は 24）
    visibility_word: u32,    // visibility の位置（2D は 9、3D は 20）
    is_3d: u32,
    object_count: u32,
    // 半透明のグループ
    group_object_start: u32,
    group_object_count: u32,
    item_count: u32,
    padded_count: u32,
    args_base: u32,
    order_base: u32,
    // バイトニックソートの段
    k: u32,
    j: u32,
}

const FLAG_CULL: u32 = 1u;
const FLAG_BUCKET_SORT: u32 = 2u;
const FLAG_TRANSPARENT: u32 = 4u;
const NO_KEY: u32 = 0xffffffffu;
const BUCKETS: u32 = 16u;

@group(0) @binding(0) var<storage, read> objects: array<ObjectData>;
@group(0) @binding(1) var<storage, read> instances: array<u32>;
@group(0) @binding(2) var<storage, read> cameras: array<CameraCull>;
@group(0) @binding(3) var<storage, read_write> order: array<u32>;
@group(0) @binding(4) var<storage, read_write> args: array<DrawArgs>;
@group(0) @binding(5) var<storage, read_write> keys: array<u32>;
@group(0) @binding(6) var<storage, read_write> values: array<u32>;
@group(0) @binding(7) var<uniform> params: Params;

fn load(base: u32, word: u32) -> f32 {
    return bitcast<f32>(instances[base + word]);
}

// インスタンスの位置に置いた境界の中心（ワールド空間）と半径
fn world_bounds(object: ObjectData, slot: u32) -> vec4<f32> {
    let base = (object.instance_base + slot) * params.instance_words;

    if (params.is_3d == 1u) {
        // Instance3D：transform は 0〜15
        let c0 = vec4(load(base, 0u), load(base, 1u), load(base, 2u), load(base, 3u));
        let c1 = vec4(load(base, 4u), load(base, 5u), load(base, 6u), load(base, 7u));
        let c2 = vec4(load(base, 8u), load(base, 9u), load(base, 10u), load(base, 11u));
        let c3 = vec4(load(base, 12u), load(base, 13u), load(base, 14u), load(base, 15u));
        let center = mat4x4(c0, c1, c2, c3) * vec4(object.bounds.xyz, 1.0);
        let scale = max(length(c0.xyz), max(length(c1.xyz), length(c2.xyz)));

        return vec4(center.xyz, object.bounds.w * scale);
    }

    // Instance2D：position は 4〜5、scale は 6〜7、rotation は 8
    let position = vec2(load(base, 4u), load(base, 5u));
    let scale = vec2(load(base, 6u), load(base, 7u));
    let rotation = load(base, 8u);
    let local = object.bounds.xy * scale;
    let c = cos(rotation);
    let s = sin(rotation);
    let center = position + vec2(local.x * c - local.y * s, local.x * s + local.y * c);

    return vec4(center, 0.0, object.bounds.w * max(abs(scale.x), abs(scale.y)));
}

fn row(m: mat4x4<f32>, i: u32) -> vec4<f32> {
    return vec4(m[0][i], m[1][i], m[2][i], m[3][i]);
}

// 球が視界（6 つの平面の内側）に少しでも入るか。深度は 0〜w（Reversed-Z）
fn sphere_in_frustum(m: mat4x4<f32>, sphere: vec4<f32>) -> bool {
    let r0 = row(m, 0u);
    let r1 = row(m, 1u);
    let r2 = row(m, 2u);
    let r3 = row(m, 3u);
    let planes = array<vec4<f32>, 6>(r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2);

    for (var i = 0u; i < 6u; i++) {
        let plane = planes[i];
        let distance = dot(plane.xyz, sphere.xyz) + plane.w;

        if (distance < -sphere.w * length(plane.xyz)) {
            return false;
        }
    }

    return true;
}

fn is_visible(object: ObjectData, slot: u32) -> bool {
    let base = (object.instance_base + slot) * params.instance_words;

    if (instances[base + params.visibility_word] == 0u) {
        return false;
    }

    if ((object.flags & FLAG_CULL) == 0u) {
        return true;
    }

    return sphere_in_frustum(cameras[object.camera].view_projection, world_bounds(object, slot));
}

// カメラからの奥行き（ビュー空間の距離。透視投影では clip の w）
fn view_depth(object: ObjectData, slot: u32) -> f32 {
    let m = cameras[object.camera].view_projection;
    let center = world_bounds(object, slot);

    return dot(row(m, 3u).xyz, center.xyz) + m[3][3];
}

// ---- cull_compact ----

var<workgroup> scan: array<u32, 256>;
var<workgroup> histogram: array<atomic<u32>, BUCKETS>;
var<workgroup> bucket_offsets: array<atomic<u32>, BUCKETS>;
var<workgroup> running: u32;
var<workgroup> shared_flags: u32;
var<workgroup> shared_count: u32;

@compute @workgroup_size(256)
fn cull_compact(@builtin(workgroup_id) workgroup: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let object = objects[workgroup.x];

    if (lid == 0u) {
        shared_flags = object.flags;
        shared_count = object.instance_count;
        running = 0u;
    }

    // ワークグループ全体で同じ値として読む（バリアの前の分岐を揃えるため）
    let flags = workgroupUniformLoad(&shared_flags);
    let count = workgroupUniformLoad(&shared_count);

    if ((flags & FLAG_TRANSPARENT) != 0u) {
        return;
    }

    let chunks = (count + 255u) / 256u;

    if ((flags & FLAG_BUCKET_SORT) != 0u) {
        // 3D の不透明：奥行きで 16 段に振り分ける（手前から奥へ。正確さは要らない、§10）
        let depth_range = cameras[object.camera].depth_range;

        if (lid < BUCKETS) {
            atomicStore(&histogram[lid], 0u);
            atomicStore(&bucket_offsets[lid], 0u);
        }
        workgroupBarrier();

        for (var chunk = 0u; chunk < chunks; chunk++) {
            let slot = chunk * 256u + lid;
            if (slot < count && is_visible(object, slot)) {
                atomicAdd(&histogram[bucket_of(object, slot, depth_range)], 1u);
            }
        }
        workgroupBarrier();

        if (lid == 0u) {
            var total = 0u;
            for (var b = 0u; b < BUCKETS; b++) {
                atomicStore(&bucket_offsets[b], total);
                total += atomicLoad(&histogram[b]);
            }
            running = total;
        }
        workgroupBarrier();

        for (var chunk = 0u; chunk < chunks; chunk++) {
            let slot = chunk * 256u + lid;
            if (slot < count && is_visible(object, slot)) {
                let position = atomicAdd(&bucket_offsets[bucket_of(object, slot, depth_range)], 1u);
                order[object.order_base + position] = object.instance_base + slot;
            }
        }
    } else {
        // 2D：スロットの順のまま詰める（累積和）。2D は描いた順に上書きするため、順番を保つ（§10）
        for (var chunk = 0u; chunk < chunks; chunk++) {
            let slot = chunk * 256u + lid;
            var flag = 0u;
            if (slot < count && is_visible(object, slot)) {
                flag = 1u;
            }

            scan[lid] = flag;
            workgroupBarrier();

            for (var offset = 1u; offset < 256u; offset *= 2u) {
                var add = 0u;
                if (lid >= offset) {
                    add = scan[lid - offset];
                }
                workgroupBarrier();
                scan[lid] += add;
                workgroupBarrier();
            }

            if (flag == 1u) {
                order[object.order_base + running + scan[lid] - 1u] = object.instance_base + slot;
            }
            workgroupBarrier();

            if (lid == 255u) {
                running += scan[255];
            }
            workgroupBarrier();
        }
    }

    workgroupBarrier();

    if (lid == 0u) {
        args[object.args_index] = DrawArgs(object.index_count, running, object.first_index, object.base_vertex, object.order_base);
    }
}

fn bucket_of(object: ObjectData, slot: u32, depth_range: vec4<f32>) -> u32 {
    let t = clamp((view_depth(object, slot) - depth_range.x) / max(depth_range.y - depth_range.x, 1e-6), 0.0, 0.9999);
    return u32(t * f32(BUCKETS));
}

// ---- 3D の半透明 ----

// 並べ替えの位置 t が、グループの中のどの Object のどのスロットか（sort_base で二分探索する）
fn locate(t: u32) -> vec2<u32> {
    var low = params.group_object_start;
    var high = params.group_object_start + params.group_object_count - 1u;

    while (low < high) {
        let middle = (low + high + 1u) / 2u;
        if (objects[middle].sort_base <= t) {
            low = middle;
        } else {
            high = middle - 1u;
        }
    }

    return vec2(low, t - objects[low].sort_base);
}

// 浮動小数点数を、符号なし整数の大小が同じ順になる値にする
fn float_key(value: f32) -> u32 {
    let bits = bitcast<u32>(value);
    if ((bits & 0x80000000u) != 0u) {
        return ~bits;
    }
    return bits | 0x80000000u;
}

@compute @workgroup_size(256)
fn transparent_keys(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = id.x;

    if (t >= params.padded_count) {
        return;
    }

    values[t] = t;

    if (t >= params.item_count) {
        keys[t] = NO_KEY;
        return;
    }

    let located = locate(t);
    let object = objects[located.x];

    if (is_visible(object, located.y)) {
        // 奥から手前へ描くため、遠いほど小さい鍵にする
        keys[t] = min(float_key(-view_depth(object, located.y)), NO_KEY - 1u);
    } else {
        // 非表示・視界外は最後に回し、描く数を 0 にする
        keys[t] = NO_KEY;
    }
}

@compute @workgroup_size(256)
fn bitonic_step(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    let partner = i ^ params.j;

    if (i >= params.padded_count || partner <= i) {
        return;
    }

    let ascending = (i & params.k) == 0u;
    let a = keys[i];
    let b = keys[partner];

    if ((a > b) == ascending) {
        keys[i] = b;
        keys[partner] = a;
        let value = values[i];
        values[i] = values[partner];
        values[partner] = value;
    }
}

@compute @workgroup_size(256)
fn transparent_emit(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.x;

    if (p >= params.item_count) {
        return;
    }

    let located = locate(values[p]);
    let object = objects[located.x];
    var count = 1u;

    if (keys[p] == NO_KEY) {
        count = 0u;
    }

    args[params.args_base + p] = DrawArgs(object.index_count, count, object.first_index, object.base_vertex, params.order_base + p);
    order[params.order_base + p] = object.instance_base + located.y;
}
