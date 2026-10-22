# Lonas 設計メモ

gueiz の処理が複雑化したため、ユーザー API を先に固めてから実装する方針で新規作成したプロジェクト。
ユーザーが使用する関数のサンプルは `examples/test1.rs` を参照。

---

## 1. ユーザー API 概要

### 主要な型

| 型 | 役割 |
|---|---|
| `Object2D` / `Object3D` | 描画単位。頂点・シェーダー・インスタンスを持つ。2D / 3D で別型（§5） |
| `GpuContext` | device / queue / surface、GPU メモリのヒープ管理（§6）、フレーム境界（§9） |
| `DrawManager2D` / `DrawManager3D` | 描画コマンドの蓄積と DrawCall。`Camera` を保持し、`Object*::end` に渡される。いくつでも作成可能 |
| `Camera2D` / `Camera3D` | `DrawManager*` が所有し、`CameraId` で参照する（§5・§11） |
| `ResourceManager` | シェーダーとテクスチャを登録し、`ShaderId` / `TextureId` を返す（§11） |
| `Instance2D` / `Instance3D` | インスタンスごとの変換・色・表示 / 非表示（§11） |
| `Vertex` | 内部の頂点表現。位置 / 色 / UV / 法線 (`#[repr(C)]`, `PartialEq`) |

### 呼び出し区分

**毎フレーム必ず呼ばれる関数群**

```rust
gpu.begin_frame();

object.begin();
object.vertex(x, 100.0); // Object2D は vertex(x, y)（§11）
object.vertex(x, 100.0);
object.vertex(x, 300.0);
object.index(0);
object.index(1);
object.index(2);
object.end(&mut draw_manager_2d);

gpu.end_frame(&mut [
    SurfaceDraw { surface: main_surface, clear_color: Some(color), draw_managers: &mut [&mut draw_manager_3d, &mut draw_manager_2d] },
]);
```

**高頻度ではないが、描画中でも呼ばれる関数群**

```rust
object.camera(camera_id);
object.memory_update_type(MemoryUpdateType::Dynamic { ring_count: 3 });
object.shader(shader_id);
object.blend(Blend::Alpha);
object.topology(Topology::TriangleFan);
object.layer(1); // Object2D のみ。任意
object.cull_mode(CullMode::Back);
object.uniform(...); // 高頻度で呼んでよい。同値なら無視（§11）
let instance_id = object.add_instance(Instance2D { ... }); // InstanceId を返す（§11）
```

各メソッドは `&mut Self` を返すため、メソッドチェーンも可能。ただし `add_instance` は `InstanceId` を返す（§11）。

### 頂点の入力

毎フレーム `Vertex` をユーザー側で作成するのはコストが高いため、**頂点の要素を関数の引数として直接受け取る**方式とする。
`Vertex` はユーザーが組み立てるものではなく、ライブラリ内部の表現として扱う。

段階的に省略可能（省略された要素は 0.0）。

- `vertex(x, y, z)`
- `vertex_color(x, y, z, r, g, b, a)`
- `vertex_color_uv(x, y, z, r, g, b, a, u, v)`
- `vertex_color_uv_normal(x, y, z, r, g, b, a, u, v, n_x, n_y, n_z)`

この方式の利点として、受け取った値を内部バッファの該当位置と **その場で比較しながら書き込める**。
収集用バッファと GPU 反映済みバッファを二重に持たなくても変更検出ができる（§4 参照）。

---

## 2. begin / end のライフサイクル

`begin` / `end` は毎フレーム呼ぶ設計。ただし内部では毎フレーム全てを作り直さない。

### 初回フレーム

1. `begin` : 書き込み位置を先頭に戻す
2. `vertex*` : 頂点を書き込む
3. `end` : `DrawManager*` 経由で `GpuContext` のヒープに領域を確保し（§6）、描画コマンドを `DrawManager*` に積む

### 2 フレーム目以降

1. `begin` : 書き込み位置を先頭に戻す（`Vec` の容量は再利用）
2. `vertex*` : 前フレームの値と比較しながら書き込み、差分があれば dirty
3. `end` : 変更があった場合のみ GPU への転送を要求し、描画コマンドを `DrawManager*` に積む
   - 頂点が前フレームと同一 → 転送なし。既存の領域で描画
   - 頂点が変更された → サイズが同じなら既存の領域に上書き転送。サイズが変わる場合、Static は panic、Dynamic は再確保（§6）
   - 頂点数が変わった → dirty 扱い
   - その他の更新条件 → **随時検討する**（§12 参照）

---

## 3. 変更の遅延反映と同値検知

### 遅延反映

`shader()` / `add_instance()` は即時に GPU リソースへ反映せず、dirty フラグを立てるのみ。
実際の反映は次の `end` でまとめて行う。これにより描画中に呼ばれても安全に扱える。

### 同値時の扱い

`shader()` が毎フレーム呼ばれた場合、**現在の値と同値なら無視する**。
その際、ロガー（`log` クレート）で **警告** を出す（毎フレーム呼ぶ必要がない呼び出しであることをユーザーに知らせるため）。

`add_instance()` は **同値無視の対象外**（§7 参照）。`shader()` は「置き換え」だが `add_instance()` は「追加」であり、
同値のインスタンスを意図的に複数追加するケースがあるため。

### 同値検知用キャッシュの設定

同値検知には比較用のキャッシュ（直前の値やハッシュ）を保持する可能性が高い。
メモリ最適化のため、**ライブラリの設定でキャッシュを無効化できる** ようにする。

### LonasConfig

**決定：`LonasConfig` のすべての項目に既定値を設けない。ユーザーが必ず全項目を指定する。**
`LonasConfig` / `MemoryConfig` / `MemoryCapacity` はいずれも `Default` を実装せず、既定値を埋めるコンストラクタも用意しない（構造体リテラルで全項目を書く）。
アプリごとに適切な値が大きく異なり、ライブラリ側で妥当な既定値を決められないため。

`GpuContext::new(window, config)` で受け取る（§9）。

```rust
pub struct LonasConfig {
    /// false の場合、同値検知用のキャッシュを保持しない
    pub change_detection_cache: bool,
    /// インスタンス数がこのフレーム数だけ連続で増加したら警告する（§7 参照）
    /// None の場合は警告しない
    pub instance_growth_warning_frames: Option<u32>,
    /// 用途ごとのメモリ空間の割り当て量（§6 参照）
    pub memory: MemoryConfig,
    /// Dynamic 領域が不足したときの拡張倍率（§6 参照）
    /// 拡張後の容量 = max(現在の容量 × この値, 現在の容量 + 必要なサイズ)
    pub dynamic_growth_factor: f32,
    /// このフレーム数だけ end されなかった Object の領域を回収する（§6 参照）
    pub gc_unused_frames: u32,
    /// Static 領域の穴の合計が Static 領域に対してこの割合（0.0〜1.0）を超えたらコンパクションする（§6 参照）
    pub static_compaction_threshold: f32,
    /// blend() を一度も呼ばなかった Object が使うブレンド（§10 参照）
    pub blend_when_unset: Blend,
    /// layer() を一度も呼ばなかった Object2D が属する層（§10 参照）
    pub layer_when_unset: i32,
    /// cull_mode() を一度も呼ばなかった Object が使う背面カリング（§11 参照）
    pub cull_mode_when_unset: CullMode,
    /// シェーダーに公開するバインドグループ・バインディング番号（§11 参照）
    pub bindings: BindingConfig,
    /// 2D の原点・3D の座標系（§11 参照）
    pub coordinate_system: CoordinateSystemConfig,
    /// 2D で視界の外を省く処理を行うか（§11 参照）
    pub culling_2d: bool,
    /// サーフェスのフォーマット（§11 参照）
    pub surface_format: SurfaceFormat,
    /// アンチエイリアスの方式（§11 参照）
    /// 実行中は set_anti_aliasing でサーフェスごとに変更できる（§11 参照）
    pub anti_aliasing: AntiAliasing,
    /// TAA の細かい値（§11 参照）
    pub taa: TaaConfig,
    /// TextureAtlas の大きさ・レイヤー数など（§11 参照）
    pub texture_atlas: TextureAtlasConfig,
}

pub struct MemoryConfig {
    pub vertex: MemoryCapacity,
    pub index: MemoryCapacity,
    /// インスタンスは常に Dynamic のため、Dynamic の初期量のみ
    pub instance_dynamic_initial_capacity: u64,
    pub uniform: MemoryCapacity,
}

pub struct MemoryCapacity {
    /// Static 領域の割り当て量（バイト）。超えると panic
    pub static_capacity: u64,
    /// Dynamic 領域の初期の割り当て量（バイト）。不足時のみ拡張
    pub dynamic_initial_capacity: u64,
}
```

値の検査（`GpuContext::new` で行い、不正なら panic）：
- `static_compaction_threshold` が 0.0〜1.0 の範囲外
- `dynamic_growth_factor` が 1.0 以下（拡張しても容量が増えない）

- キャッシュ無効時は同値検知を行わず、呼ばれた値をそのまま反映する（警告も出ない）

---

## 4. Object 内部構造（案）

`Object2D` / `Object3D` は共通部分（`ObjectCore`）を内部に持ち、型ごとに異なるのは `end` に渡せる `DrawManager*` の型のみ。

```rust
struct ObjectCore {
    instances: Vec<Instance>,
    shader: Shader,
    camera_id: Option<CameraId>, // None なら DrawManager の既定カメラ
    manager_id: Option<ManagerId>, // 初回 end で紐づいた DrawManager
    memory_update_type: MemoryUpdateType, // Static / Dynamic（§6）

    vertices: Vec<Vertex>,       // GPU に載っている内容の CPU 側の写し。vertex* で比較しながら上書き
    cursor: usize,               // begin で 0 に戻す書き込み位置
    allocation: Option<GpuAllocation>, // GpuContext のヒープ上の領域を指すハンドル（実体は GpuContext）
    dirty: DirtyFlags,           // vertex / shader / instance の変更フラグ
}

pub struct Object2D { core: ObjectCore }
pub struct Object3D { core: ObjectCore }
```

- `vertex*` は `vertices[cursor]` と比較して異なれば上書きし dirty を立てる。`cursor == vertices.len()` なら push
- `end` 時に `cursor != vertices.len()` なら頂点数が変わったので truncate して dirty
- `allocation` が `None` なら初回とみなしてヒープに領域を確保
- `Object*` は GPU リソースの実体を持たない（§6）

---

## 5. Camera と 2D / 3D

### Camera の保持

**決定：`Camera` は `DrawManager*` が所有する。**`Object*` はカメラを持たない。

- カメラの変更は `draw_manager.camera_mut(id)` 経由で行う。`Arc` / ロック不要
- `Object*::end(&mut DrawManager*)` の時点で `DrawManager*` からカメラを参照できるため、`Object*` 側で持つ必要がない
- カメラの行列（ビュー・プロジェクション）は `DrawManager*` 側で 1 フレームに 1 回だけ GPU へ反映すればよく、`Object*` ごとの更新判定からカメラ変更を外せる

却下案：
- `Arc<Camera>` を各 `Object` が共有 → 不変なのでカメラを動かせない
- `Arc<RwLock<Camera>>` → 動かせるが、描画中のロックとアクセス経路が複雑化する（gueiz の反省点）

### 2D / 3D の共通化方針

- 2D と 3D で **主要な機能は共通化** する（GPU メモリ管理、インスタンスの仕組みなど）。共通化できる部分は可能な限り共通化する
- **DrawCall だけは 2D と 3D で分ける**

### 2D / 3D の型：別型にする

**決定：`DrawManager2D` / `DrawManager3D`、`Object2D` / `Object3D` をそれぞれ別型にする。**

```rust
impl Object2D {
    pub fn end(&mut self, draw_manager: &mut DrawManager2D) -> &mut Self;
}

impl Object3D {
    pub fn end(&mut self, draw_manager: &mut DrawManager3D) -> &mut Self;
}
```

- `Object2D` を `DrawManager3D` に渡すような 2D / 3D の取り違えは **コンパイルエラー** になる
- 内部は `ObjectCore` / `DrawManagerCore` で共通化し、型ごとの差は `end` の引数と DrawCall のみ
- 却下案：`DrawManager<D>` のジェネリクス、`new_2d` / `new_3d` の実行時モード

### DrawManager は複数作成可能

**決定：`DrawManager2D` / `DrawManager3D` はいくつでも作成できる。**

- 各 `DrawManager*` は生成時に `GpuContext` から一意な `ManagerId` を受け取る
- 同じ型の別インスタンス（例：`DrawManager2D` が 2 つ）への取り違えは型では防げないため、実行時に検査する
  - `Object*` は初回 `end` で渡された `DrawManager*` の `ManagerId` を記録し、以降別の `DrawManager*` に渡されたら **panic**

### カメラの指定：CameraId

**決定：`CameraId` は `DrawManager*` ごとに持つ。1 つの `DrawManager*` が複数のカメラを持ち、`Object*` は `CameraId` で使用するカメラを指定する。**

```rust
impl DrawManager2D { // DrawManager3D も同じ
    pub fn new(gpu: &mut GpuContext, camera: Camera, vertex_layout: VertexLayout) -> Self; // 渡したカメラが既定カメラになる（VertexLayout は §10）
    pub fn default_camera_id(&self) -> CameraId;
    pub fn add_camera(&mut self, camera: Camera) -> CameraId;
    pub fn camera(&self, id: CameraId) -> &Camera;
    pub fn camera_mut(&mut self, id: CameraId) -> &mut Camera;
}

impl Object2D { // Object3D も同じ
    pub fn camera(&mut self, id: CameraId) -> &mut Self; // 未指定なら既定カメラ
}
```

- `Object*::camera(id)` は `shader()` と同じく「高頻度ではないが描画中でも呼ばれる」区分。指定は保持され、毎フレーム呼ぶ必要はない
- `CameraId` は発行元の `ManagerId` を内部に持つ。別の `DrawManager*` の `CameraId` が使われたら **panic**

---

## 6. GPU メモリ管理

**決定：GPU リソース（頂点バッファ、インスタンスバッファ等）の実体は `GpuContext` に集約し、アプリケーションのヒープのように管理する。**
**`Object*` から `GpuContext` へのアクセスは `DrawManager*` 経由で行う。**

### ヒープとしての管理

- 大きな GPU バッファをまとめて確保し、その中から各 `Object*` に領域を **部分割り当て** する
- `Object*` が持つのは領域を指すハンドル（`GpuAllocation`）のみ
- 2D / 3D で同じヒープを共有する（§5 の共通化方針）

### Static / Dynamic の区別

**決定：変更がほとんど起こらないものと、毎フレーム変更が行われるもので管理の仕方を分ける。**
`Object*` ごとに「高頻度ではないが描画中でも呼ばれる」区分の `memory_update_type()` で指定する。

```rust
pub enum MemoryUpdateType {
    Static,
    Dynamic { ring_count: u32 },
}

impl Object2D { // Object3D も同じ
    pub fn memory_update_type(&mut self, memory_update_type: MemoryUpdateType) -> &mut Self;
}
```

- **既定値なし。**`memory_update_type()` を一度も呼ばずに `end` した場合は panic。Static / Dynamic で振る舞い（サイズ変更の可否）が大きく異なるため、ユーザーに必ず選ばせる

### メモリ空間の配置

**決定：全体のメモリ空間の手前を Static 用、後ろを Dynamic 用にする。**

```
0                       static_capacity                   static_capacity + dynamic_capacity
|------ Static 領域 ------|----------- Dynamic 領域 -----------|
  スタック方式で手前から積む     Free List Allocator
```

- 初期の割り当て量は `LonasConfig` で Static / Dynamic それぞれ指定する（§3）
- Dynamic 領域を拡張する際は、後ろ側だけが伸びるため **Static 領域のオフセットは変わらない**

### 用途ごとのメモリ空間

**決定：頂点 / インデックス / インスタンス / uniform の用途ごとに、上記の「Static + Dynamic」のメモリ空間を持つ。**

| 用途 | Static 領域 | Dynamic 領域 |
|---|---|---|
| 頂点 | あり | あり |
| インデックス | あり | あり |
| インスタンス | **なし**（常に Dynamic） | あり |
| uniform | あり | あり |

- wgpu の `BufferUsages`（`VERTEX` / `INDEX` / `UNIFORM` など）を用途ごとに分けられる
- 割り当て量も用途ごとに `LonasConfig` で指定する（§3）

### インスタンスの領域

**決定：インスタンスの領域は、`Object*` の `memory_update_type` に関係なく常に Dynamic 領域を使う。**

- インスタンスは追加・削除で領域のサイズが変わるため（§7）
- これにより Static の `Object*` でも `add_instance` / `remove_instance` が使える（頂点などは Static、インスタンスだけ Dynamic）

| | Static | Dynamic |
|---|---|---|
| 割り当て方式 | **スタック方式**（手前から順に積む） | **Free List Allocator** |
| 容量を超えた場合 | **panic**（`Object` を追加し続けてメモリを圧迫するのを防ぐ） | 必要なサイズが容量を超えた場合 **だけ** バッファを拡張 |
| 確保後の編集 | 確保した領域のサイズが **変わらない** 変更のみ可能。増える場合も減る場合も **不可（panic）** | 領域のサイズが変わることを前提とする |
| リングバッファ | なし | `ring_count` 個の領域を持ち、フレームごとに順に書き込む |

- wgpu のバッファはサイズを変えられないため、Dynamic の拡張は「より大きなバッファを作成 → `copy_buffer_to_buffer` で既存の内容をコピー → 差し替え」で行う

### リングバッファ（Dynamic）

- `ring_count` は `Object*` ごとに指定する
- 領域を `ring_count` 個持ち、フレームごとに書き込み先を順に切り替える。GPU が前のフレームの領域を読んでいる間に、CPU が次の領域へ書き込めるため、同期待ちを避けられる
- `ring_count: 1` はリングなし（単一の領域を上書き）と同じ
- `ring_count: 0` は `memory_update_type()` の時点で panic

### Dynamic の割り当て：TLSF

**決定：Dynamic 領域の Free List Allocator は TLSF（Two-Level Segregated Fit）の考え方で設計する。**
Blender の Vulkan バックエンドが使う VMA（Vulkan Memory Allocator）の既定アルゴリズムと同じ方式。

#### 選んだ理由

- 確保・解放とも **O(1)**（空き領域の探索がリストの長さに依存しない）。毎フレーム確保・解放が起きる Dynamic 領域に向く
- 近いサイズの空き領域を選ぶ（good-fit）ため断片化しにくい
- 解放時に隣接する空き領域と結合する

#### メタデータは CPU 側に持つ

GPU バッファの中にはブロックのヘッダを書き込めないため、VMA の仮想割り当てと同様に、**ブロック情報はすべて CPU 側に持つ**。
GPU バッファは「オフセットとサイズで区切られた領域」としてのみ扱う。

```rust
// 案
struct TlsfAllocator {
    blocks: Vec<Block>,             // ブロックの実体。インデックスで参照する
    unused_block_slots: Vec<u32>,   // 再利用できる blocks のインデックス
    first_level_bitmap: u32,        // 空きブロックがある第 1 レベルのビット
    second_level_bitmaps: [u32; FIRST_LEVEL_COUNT], // 第 1 レベルごとの第 2 レベルのビット
    free_heads: [[Option<u32>; SECOND_LEVEL_COUNT]; FIRST_LEVEL_COUNT], // サイズクラスごとの空きリストの先頭
    last_physical_block: u32,       // 領域の末尾のブロック（拡張時に使う）
    alignment: u64,                 // 用途ごとのアラインメント（§6 用途ごとのメモリ空間）
}

struct Block {
    offset: u64,
    size: u64,
    is_free: bool,
    prev_physical: Option<u32>,     // メモリ上で手前に隣接するブロック（結合用）
    next_physical: Option<u32>,     // メモリ上で後ろに隣接するブロック（結合用）
    prev_free: Option<u32>,         // 同じサイズクラスの空きリスト（双方向）
    next_free: Option<u32>,
}
```

#### サイズクラス（2 段階の分類）

- 第 1 レベル `fl` = `floor(log2(size))`。2 の累乗ごとの大きな区分
- 第 2 レベル `sl` = 第 1 レベルの範囲を `SECOND_LEVEL_COUNT` 等分した区分。`sl = (size >> (fl - SLI)) - SECOND_LEVEL_COUNT`
- 小さいサイズ（`SMALL_SIZE` 未満）は `fl = 0` にまとめ、線形に等分する（VMA と同様）
- 各サイズクラスに空きブロックの双方向リストを持ち、空きがあるかをビットマップで管理する

| 定数 | 値 | 意味 |
|---|---|---|
| `SLI` | 5（`SECOND_LEVEL_COUNT` = 32） | VMA と同じ。サイズクラス内の無駄は最大約 1/32（約 3%） |
| `SMALL_SIZE` | 256 バイト | これ未満は線形に分類 |
| `FIRST_LEVEL_COUNT` | 32 | 4GB までのブロックを扱える |

#### 確保

1. サイズを用途ごとのアラインメント（uniform なら `min_uniform_buffer_offset_alignment`、それ以外は 4 バイト）の倍数に切り上げる
2. サイズを **次のサイズクラスの境界まで切り上げて** `fl` / `sl` を求める。これにより、見つかったクラスのブロックは必ず要求サイズ以上になる（リストを探索しない）
3. `second_level_bitmaps[fl]` の `sl` 以上のビットから最下位ビットを探す（`trailing_zeros`）。なければ `first_level_bitmap` の `fl + 1` 以上から探す
4. 見つかったブロックを空きリストから外す
5. 残りが最小ブロックサイズ以上なら分割し、残りを空きリストへ入れる
6. どのクラスにも空きがなければ **Dynamic 領域を拡張**（下記）してから再試行

#### 解放

1. ブロックを空きにする
2. `prev_physical` / `next_physical` が空きなら、それらを空きリストから外して結合する
3. 結合後のブロックを該当するサイズクラスの空きリストへ入れ、ビットマップを更新する

#### Dynamic 領域の拡張

- Dynamic 領域はメモリ空間の **末尾** にあるため（§6 メモリ空間の配置）、拡張は末尾に空きブロックを足すだけで済む
  - `last_physical_block` が空きなら、そのブロックを伸ばす
  - そうでなければ、追加分を新しい空きブロックとして末尾につなぐ
- 拡張後の容量は `max(現在の容量 × LonasConfig::dynamic_growth_factor, 現在の容量 + 必要なサイズ)`（§3）。拡張の回数を減らすため倍率で伸ばし、1 回の要求が大きい場合でも必ず足りるようにする
- GPU バッファの作り直しとコピーは前述のとおり（Static 領域のオフセットは変わらない）

#### サイズ変更とリングバッファ

- `ring_count` 個の領域は **1 つのブロックとして連続で確保** する（サイズ = 1 個分 × `ring_count`）。フレームごとの書き込み先は `offset + (frame_index % ring_count) * 1 個分`
- サイズが変わった場合は「新しいブロックを確保 → 新しいデータを `queue.write_buffer` で書き込む → 古いブロックを解放」の順で行う
  - サイズ変更は新しいデータの書き込みを伴うため、古いブロックから GPU 上でコピーする必要はない（`Object*` が CPU 側に頂点を持っている、§4）
  - 古いブロックを同じフレームで別の `Object*` に再利用しても、wgpu は queue に積んだ順に実行するため、前のフレームの描画とは競合しない

### 確保後の memory_update_type の変更

**決定：初期化後（初回 `end` で領域を確保した後）に `memory_update_type()` が呼ばれた場合、新しい側の領域へ内容をコピーする。**

- Static → Dynamic：Dynamic 領域に確保して書き込み、Static 側の領域は不要になる
- Dynamic → Static：Static 領域のスタックに積んで書き込み、Dynamic 側の領域をフリーリストへ返す
- `ring_count` だけを変えた場合も、領域を確保し直して書き込む
- **決定：書き込みは `Object*` が CPU 側に持っているデータ（§4）から行う。**Static と Dynamic は同じバッファ内にあり GPU 上で直接コピーできないが、CPU 側のデータを `queue.write_buffer` で書き込めば GPU 上のコピーも一時バッファも不要。Dynamic のサイズ変更時と同じ処理になる
- インスタンスは常に Dynamic のため（§6 インスタンスの領域）、この移動の対象外

### Static 領域のコンパクション

**決定：Static 領域の途中が不要になった場合（GC での回収、Static → Dynamic への変更）、Static 領域を詰め直す。**

- 不要になった領域がスタックの一番上なら、スタックの先頭を下げるだけでよい（コンパクション不要）
- 途中に穴ができた場合は、それより上の領域を手前へ詰める
- **タイミングはしきい値方式**：穴の合計が Static 領域に対して `LonasConfig::static_compaction_threshold`（割合）を超えたときだけ行う（§3）。それまでは穴を残したまま使う
- 各領域の実際のオフセットは `GpuContext` 側の対応表で持つため、詰め直しても `Object*` のハンドル（`GpuAllocation`）は変わらない
- 詰め直しは `end_frame` 内で描画コマンドの実行より前に行う。GPU のコマンドは queue に積んだ順に実行されるため、前のフレームの描画と競合しない
- **決定：コピーは一時バッファを経由し、移す部分だけをコピーする。**wgpu の `copy_buffer_to_buffer` は同じバッファ内でのコピーができないため
  1. 穴より上の移す部分を一時バッファへコピー
  2. 一時バッファから元のバッファの穴の位置へコピー
  - 一時バッファは使い回し、移す量が一時バッファの容量を超えた場合だけ作り直す
  - Dynamic 領域はコピーしない
  - 却下案：新しいバッファへ全体をコピーして差し替える（Dynamic 部分まで毎回コピーし、作業中にメモリが一時的に 2 倍必要になる）

### 解放：GC 方式

**決定：一定フレーム `end` されなかった `Object*` の領域を回収する。**

- `Object*` は `GpuContext` への参照を持たないため、`Drop` では直接解放できない。そこで `GpuContext` が各領域の最終 `end` フレームを記録し、`end_frame` で期限切れの領域を回収する
- 回収までのフレーム数は `LonasConfig` で指定する（§3）
- 回収された `Object*` が再び `end` された場合は、初回と同じく領域を確保し直す

### DrawManager 経由のアクセス（案）

`gpu.end_frame(&mut [&mut dm])` で `GpuContext` を可変借用するため、`DrawManager*` が `&mut GpuContext` を保持し続けることはできない。
そこで `Object*::end` は確保・転送・解放を **要求として `DrawManager*` に積み**、`GpuContext::end_frame` でまとめて実行する。

```
Object*::end
  └ DrawManager* に「領域確保 / 転送 / 描画」の要求を積む
GpuContext::end_frame
  └ 各 DrawManager* の要求を処理：ヒープから確保 → queue.write_buffer → DrawCall
```

- ハンドル（`GpuAllocation`）は `DrawManager*` が `ManagerId` + 連番で即座に発行し、実際のヒープ上の位置との対応は `GpuContext` が持つ
- 転送が `end_frame` にまとまるため、複数 `Object*` の転送を 1 回の書き込みにまとめやすい
- `Arc` / ロックを使わずに済む

---

## 7. インスタンス API

インスタンスの追加・更新・削除は **毎フレーム呼ばれるとは限らないが、高頻度の更新に対応できる** ようにする。

```rust
// 決定（§11）
impl Object2D { // Object3D も同じ（Instance3D を受け取る）
    pub fn add_instance(&mut self, instance: Instance2D) -> InstanceId;
    pub fn update_instance(&mut self, id: InstanceId, instance: Instance2D);
    pub fn remove_instance(&mut self, id: InstanceId);
}
```

### add_instance の同値の扱い

**決定：`add_instance()` は同値でも無視せず、常に追加して新しい `InstanceId` を返す。**

同値のインスタンスを意図的に複数追加するケースがあるため：
- 同じ位置から同一フレームに複数生成し、その後 `update_instance` で個別に動かす（弾、パーティクル）
- `Instance::default()` で先に確保し、後から値を設定する（プール、レベル読み込み）
- 加算合成で意図的に重ねる
- 将来 `Instance` に項目が増えても「同値で追加 → 後から変更」の流れは残る

### 誤用（毎フレーム add_instance）の検知

描画ループ内で毎フレーム `add_instance()` を呼び続ける誤用は、値の比較ではなく
**インスタンス数が増え続けていること** で検知し、`log` で警告する。

- `end` 時にインスタンス数を前フレームと比較し、連続して増加したフレーム数を数える
- `LonasConfig::instance_growth_warning_frames`で指定したフレーム数だけ連続で増加したら **一度だけ** 警告する。`None` で警告を無効化（§3 参照）
- 必要な状態は「前フレームのインスタンス数」と「連続増加カウンタ」のみで、同値検知用キャッシュは不要（`change_detection_cache` の設定に依存しない）

### 高頻度更新への対応案
- `InstanceId` → 配列インデックスの対応表を持ち、削除は `swap_remove` で O(1)
- 変更されたインスタンスの範囲（dirty range）だけを GPU に部分転送する
- インスタンスバッファは §6 のヒープ上に容量に余裕を持たせて確保し、超過時のみ再確保

---

## 8. 描画バックエンド

**決定：gueiz と同じ方式をとる。**

- `wgpu` を使用（gueiz は wgpu 30）
- 頂点データの変換は `bytemuck`、ログは `log`
- バックエンド選択は gueiz の `renderer.rs` と同様に `Auto / Vulkan / DirectX12 / Metal / OpenGl` から指定可能にする
- Present mode も gueiz の `SurfacePresentMode` と同様に指定可能にする
- **決定：WebGPU 環境は対象としない。**Compute Shader の有無などの環境の分岐は、ネイティブのバックエンド（Vulkan / DirectX12 / Metal / OpenGL）の間でのみ考える

### ウィンドウとイベントループ

**決定：ウィンドウとイベントループは Lonas が持たない。`winit` などの外部ライブラリ（ユーザー側）が所有する。**

- Lonas は描画先として `raw-window-handle` の `HasWindowHandle + HasDisplayHandle` を受け取り、wgpu の Surface を作る
- 特定のウィンドウライブラリに依存しないため、`winit` 以外（SDL など）でも使える
- イベントループはユーザーが回し、その中で `begin_frame` / `end_frame`（§9）を呼ぶ
- リサイズはユーザーがイベントを受けて `GpuContext::resize(surface, width, height)` で通知する（§9）

---

## 9. フレーム境界

### 必要性

`Object*::end` は描画コマンドを **積むだけ** のため、それを GPU で実行して画面に出す処理を 1 フレームに 1 回どこかで行う必要がある。

- 描画先の取得（`surface.get_current_texture()`）
- 全 `Object*` の `end` 後に描画順を決める（`DrawManager*` の順、シェーダー単位のまとめ）
- カメラ行列の転送など、フレームに 1 回だけ行う処理
- `queue.submit()` と `present()`
- リサイズや最小化で描画先が取得できない場合のフレームスキップ

### 決定：ユーザーが明示的に呼ぶ

- 却下案：ライブラリが `draw_loop` の前後で暗黙に管理 / クロージャ形式（`frame(|dm| { ... })`）

### 決定：フレーム境界は GpuContext が持つ

描画先の取得と `present` は 1 フレームに 1 回しかできないため、各 `DrawManager*` にフレーム境界を持たせることはできない。
共有の **`GpuContext`** がフレーム境界を持つ。

```rust
impl GpuContext {
    // 最初のサーフェスと設定を受け取り、そのサーフェスの SurfaceId も返す（§11 複数ウィンドウ）
    // 環境に起因するエラーは Result、config の値が不正なら panic（§3）
    pub fn new(window: impl HasWindowHandle + HasDisplayHandle, config: LonasConfig) -> Result<(Self, SurfaceId), LonasError>;
    pub fn add_surface(&mut self, window: impl HasWindowHandle + HasDisplayHandle) -> Result<SurfaceId, LonasError>;
    pub fn resize(&mut self, surface: SurfaceId, width: u32, height: u32);
    pub fn begin_frame(&mut self);
    pub fn end_frame(&mut self, surfaces: &mut [SurfaceDraw]);
}

pub struct SurfaceDraw<'a> {
    pub surface: SurfaceId,
    pub clear_color: Option<Color>, // None ならクリアしない（§10）
    pub draw_managers: &'a mut [&'a mut dyn DrawManager], // 渡した順に描画する
}
```
```

```rust
gpu.begin_frame();

object_3d.end(&mut draw_manager_3d);
object_ui.end(&mut draw_manager_2d);

// サーフェスごとに、渡した順に描画する（3D → 2D）
gpu.end_frame(&mut [
    SurfaceDraw { surface: main_surface, clear_color: Some(color), draw_managers: &mut [&mut draw_manager_3d, &mut draw_manager_2d] },
]);
```

- `DrawManager` は `DrawManager2D` / `DrawManager3D` が実装するトレイト。`end_frame` に 2D / 3D を混ぜて渡すために使う

| 関数 | 処理 |
|---|---|
| `GpuContext::begin_frame` | フレームの開始（フレーム番号を進める）。描画先の取得は行わない |
| `GpuContext::end_frame` | 全サーフェスの `DrawManager*` の要求の処理（ヒープ確保・転送、§6）を 1 回まとめて行った後、渡されたサーフェスごとに：描画先の取得 → `DrawManager*` ごとに（カメラ行列の転送 → コマンドの並べ替え → RenderPass で実行 → コマンドのクリア）。最後に `submit` を 1 回行い、各サーフェスを `present` |

責務の分担：

| 型 | 持つもの |
|---|---|
| `GpuContext` | device / queue / surface、GPU メモリのヒープ（2D / 3D 共通）、フレーム境界、`ManagerId` の発行、`LonasConfig` |
| `DrawManager2D` / `DrawManager3D` | 2D / 3D 固有の DrawCall、カメラ（`CameraId`）、そのフレームに積まれた描画コマンドとヒープへの要求 |

- `DrawManager*` 間の描画順は `end_frame` に渡す配列の順で決まる。ライブラリ側で 3D / 2D の順を固定しないため、2D の背景 → 3D → 2D の UI のような構成もできる
- コマンドのクリアは `end_frame` で実行した直後に行う。`begin_frame` は `DrawManager*` を受け取らない

### 決定：リサイズとフレームスキップ

- ウィンドウのリサイズはユーザーがイベントを受けて `GpuContext::resize(surface, width, height)` で通知する
- 幅か高さが 0（最小化など）の場合や、描画先（`get_current_texture`）を取得できなかった場合、`GpuContext` の内部でそのフレームの **そのサーフェスの描画だけを飛ばす**（他のサーフェスは描く）
  - その間も `Object*::end` と `end_frame` はそのまま呼べる。`end_frame` は描画と `present` を行わず、ヒープの確保・転送とコマンドのクリアだけを行う
  - ユーザー側で「描画できるか」の分岐を書かなくて済む
- 却下案：`begin_frame` が描画するかどうかを返し、ユーザーが判断する

### 決定：誤用時は panic

| 誤用 | 検知 |
|---|---|
| `begin_frame` の二重呼び出し | `begin_frame` で panic |
| `end_frame` の呼び忘れ | 次の `begin_frame` が二重呼び出しとなり panic |
| `begin_frame` 前の `end_frame` | `end_frame` で panic |
| フレーム外（`begin_frame` 前 / `end_frame` 後）の `Object*::end` | `Object*::end` で panic |
| `Object*` を別の `DrawManager*` に渡す | `Object*::end` で panic（§5） |
| 別の `DrawManager*` の `CameraId` を使う | `Object*::end` で panic（§5） |
| `memory_update_type()` を呼ばずに `end` | `Object*::end` で panic（§6） |
| 削除済みの `CameraId` を使う | panic（§11） |
| 存在しない / 別の `Object*` の `InstanceId` を `update_instance` / `remove_instance` に渡す | panic（§11） |
| `ring_count` が 0 または 5 以上 | `memory_update_type()` で panic（§11） |
| `index()` も `topology()` も呼ばずに `end` | `Object*::end` で panic（§10） |
| `DrawManager*` に積んだのに `end_frame` に渡さなかった | 次のフレームでその `DrawManager*` に `Object*::end` が呼ばれた時点で panic（前のフレームのコマンドが残っている） |

- フレームの状態は `GpuContext` と各 `DrawManager*` で共有する **フレーム番号**（`Arc<AtomicU64>`）で判定する。フレーム中かどうかに加え、`DrawManager*` に残っているコマンドがどのフレームのものかも分かる。`DrawManager*` が `GpuContext` を借用し続けずに済む
- 表示したくない `DrawManager*` は、その `Object*` の `end` を呼ばなければよい（コマンドが積まれないので panic しない）

---

## 10. DrawCall

2D / 3D の `DrawManager*` で異なるのはこの章の内容のみ（§5）。

### 描画の設定（Object 側）

「高頻度ではないが描画中でも呼ばれる」区分に、描画の設定を追加する。

```rust
impl Object2D { // Object3D も同じ（layer は 2D のみ）
    pub fn blend(&mut self, blend: Blend) -> &mut Self;
    pub fn layer(&mut self, layer: i32) -> &mut Self; // Object2D のみ。任意
}

pub enum Blend {
    Opaque,
    Alpha,
    Additive,
}
```

- **プリミティブは三角形のみ**（`TriangleList`）。線・点は扱わない。四角形や多角形は `topology()` で三角形に分ける（下記）
- `Alpha` / `Additive` を半透明として扱う（3D の描画順で使う）

### インデックス

**決定：`index(i)` を追加する。**`begin`〜`end` の間で、頂点と同じく前フレームの値と比較しながら書き込む（§4 と同じ方式）。

```rust
object.begin();
object.vertex(0.0, 0.0, 0.0);
object.vertex(1.0, 0.0, 0.0);
object.vertex(0.0, 1.0, 0.0);
object.index(0);
object.index(1);
object.index(2);
object.end(&mut draw_manager);
```

- インデックスは用途別メモリ空間のインデックス領域に置く（§6）
- **決定：`index()` を一度も呼ばなかった `Object*` は、`topology()` で指定した分け方に従って Lonas がインデックスを自動で生成する（三角形化）。**
  - これにより、すべての `Object*` をインデックス付きの描画として扱え、バッチングの引数が `indexed` に統一される
  - `index()` を呼んだ場合はそちらを優先し、`topology()` は使わない

### 三角形化：topology

GPU に送るプリミティブは常に三角形（`TriangleList`）のまま、**インデックスの作り方だけ** を `topology()` で選ぶ。
「高頻度ではないが描画中でも呼ばれる」区分。

```rust
pub enum Topology {
    TriangleList,  // 3 つずつで 1 つの三角形
    TriangleFan,   // 凸多角形の外周
    TriangleStrip, // 帯状に続く三角形
    Polygon,       // へこみのある多角形の外周
}

impl Object2D { // Object3D も同じ
    pub fn topology(&mut self, topology: Topology) -> &mut Self;
}
```

| 指定 | 生成するインデックス | 作り直す時 |
|---|---|---|
| `TriangleList` | 0,1,2, 3,4,5, … | 頂点数が変わった時 |
| `TriangleFan` | 0,1,2, 0,2,3, 0,3,4, … | 頂点数が変わった時 |
| `TriangleStrip` | 0,1,2, 2,1,3, 2,3,4, …（向きを揃えるため 1 つおきに入れ替える） | 頂点数が変わった時 |
| `Polygon` | 単調多角形分割で分割（O(n log n)、下記） | 頂点の位置が変わった時 |

- パイプラインもバッチングも変わらない（GPU 側は常に三角形）
- `TriangleList` / `TriangleFan` / `TriangleStrip` は頂点数だけで決まるため、計算はほぼ不要で使い回せる
- `Polygon` は頂点の位置が変わるたびに計算し直す必要がある（計算量 O(n log n)、下記）。形が頻繁に変わり、頂点数が多いものでは負担になる
- `Polygon` は 2D の多角形（3D では 1 つの平面上にある多角形）を対象とする
- 扱わないもの：穴のある多角形、自分と交差する多角形、線・点（線は細長い四角形としてユーザーが作る）
- **決定：頂点数が分け方に合わない場合、余った頂点を捨てて `log` で警告する。**
  - `TriangleList` で 3 の倍数でない場合：最後の余り（1〜2 個）を三角形に含めない
  - どの分け方でも頂点が 3 個未満の場合：何も描かない
  - 警告は、頂点数が合わない状態になった時に 1 回だけ出す（毎フレーム出さない）
- **決定：`index()` を呼ばず、`topology()` も一度も呼ばずに `end` した場合は panic。**`index()` を呼んでいる場合は `topology()` を使わないため、呼ばなくてよい
- **決定：`Polygon` の頂点の向き（時計回り / 反時計回り）は自動で判定する。**符号付き面積で向きを求め、分割はその向きに合わせて行う。生成する三角形の向きは一定に揃える

#### Polygon の分割方法

**決定：`Polygon` の三角形化は CPU で行い、単調多角形分割（monotone polygon decomposition）を使う。計算量は O(n log n)。**

1. 頂点を y 座標（3D では多角形の平面上の座標）で並べ替える
2. 上から順に走査し（sweep line）、へこみの原因になる頂点（split / merge 頂点）に対角線を引いて、多角形を y 方向に単調な多角形に分ける
3. 単調な多角形はそれぞれ O(n) で三角形に分ける

- 却下案：耳切り法（O(n²)。頂点が多いと遅い）
- 却下案：Compute Shader での三角形化。1 つの多角形の中では処理を並列にできず、効果がない
- 却下案：三角形に分けずに塗る方法（ステンシルで塗る技法）。2 回の描画とステンシルバッファが必要で、パイプラインとバッチングの方式に合わない

### インスタンスが 0 個の Object

**決定：インスタンスが 0 個の `Object*` は何も描かない。**
インスタンスを使わない `Object*` でも、描くには少なくとも 1 つ `add_instance` する必要がある。

### DrawCall の単位：バッチング

**決定：同じ設定の複数の `Object*` を 1 回の DrawCall にまとめる。**

- 「同じ設定」＝ パイプラインのキー（下記）と、使用するカメラ（`CameraId`）が同じ
- 頂点・インデックス・インスタンスはすべて共有のヒープ上にあるため、`Object*` ごとの違いは読み出し位置だけ。間接描画の引数で表せる
  - `first_index` / `base_vertex`：インデックス・頂点の位置
  - `first_instance`：インスタンスの位置
- **決定：最も高速な方法を環境に応じて選ぶ。**
  - `MULTI_DRAW_INDIRECT` 機能が使える環境：`Object*`（3D ではインスタンス）ごとの間接描画の引数を GPU バッファに並べ、`multi_draw_indexed_indirect` で 1 回にまとめて描く
  - 使えない環境（古い OpenGL / OpenGL ES 3.0 など。間接描画自体が使えない場合もある）：引数は CPU 側で分かっているため、間接描画ではなく **直接の `draw_indexed` を順に呼ぶ**。同じ `Object*` で連続するインスタンスは `instance_count` をまとめて 1 回にする
- まとめられるのは、描画順（下記）で **連続している** 同じ設定の `Object*` のみ。描画順は崩さない

### 描画順

**2D：`end` を呼んだ順を基本とし、`layer` で任意に順番を指定できる。**
- `layer` が小さいものから描く。同じ `layer` の中では `end` を呼んだ順
- `layer` を一度も呼ばない `Object2D` は `LonasConfig::layer_when_unset` の層に属する（§3）
- `blend` を一度も呼ばない `Object*` は `LonasConfig::blend_when_unset` を使う（§3）

**3D：不透明と半透明で分ける。**
1. 不透明（`Opaque`）：パイプラインごとにまとめ、手前から奥へ描く（深度テストで奥のピクセルを早く捨てられる）
2. 半透明（`Alpha` / `Additive`）：奥から手前へ描く（正しく重ねるため）。深度テストは行うが深度の書き込みはしない

**3D の奥行きはインスタンスごとに測る。**
`Object*` は位置を持たず、位置はインスタンスの変換にあるため、インスタンスの位置をカメラ空間に変換した奥行きで並べる。
並べ替えの方法は、Compute Shader が使えるかどうかで分ける。

| | Compute Shader が使える環境（Vulkan / DirectX12 / Metal / OpenGL 4.3 以上） | 使えない環境（古い OpenGL / OpenGL ES 3.0 など） |
|---|---|---|
| 不透明 | Compute Shader で **視界の外を省く処理（フラスタムカリング）と、奥行きの大まかな並べ替え** を行う。`Object*` 間はパイプラインごとにまとめる | インスタンスごとに手前から奥へ並べる（CPU、下記） |
| 半透明 | **同じパイプラインの中** を Compute Shader で奥から手前へ並べ替える。**パイプラインをまたぐ順番は CPU** で決める | インスタンスごとに奥から手前へ並べる（CPU、下記） |

#### Compute Shader による並べ替え

インスタンスのデータは GPU 上の Dynamic 領域にある（§6）ため、CPU に読み戻さずに GPU 上で完結する。

```
end_frame
  1. Compute：各インスタンスの位置をカメラ空間に変換し、奥行きを計算。視界の外のインスタンスを省く
  2. Compute：奥行きでインスタンス番号を並べ替え、「描く順のインスタンス番号のリスト」と
     間接描画の引数（instance_count = 視界内の数）を書き込む
  3. 描画：頂点シェーダーはリストを経由してインスタンスを読む
     instance = instances[sorted_ids[instance_index]]
```

- 並べ替えの結果は「インスタンス番号のリスト」として持つため、`Object*` ごとの DrawCall は 1 回のまま。インスタンシングとバッチングを崩さない
- 不透明の並べ替えは速度のため（深度テストで奥のピクセルを早く捨てる）であり、正確さは不要。奥行きを段階に分けて振り分ける **大まかな並べ替え（バケットソート）** で行う
- 半透明は正しく重ねるために正確な並べ替えが必要。同じパイプラインの中でのみ GPU で並べ替える。描画中のパイプラインの切り替え順は CPU が決めるため、パイプラインをまたぐ順番は CPU で決める
- CPU は奥行きの計算も並べ替えもしない（パイプラインをまたぐ半透明の順番を除く）

#### シェーダーでのインスタンスの読み方

**決定：頂点シェーダーが「インスタンス番号のリスト」を経由してインスタンスを読む部分は、ユーザーが書く。**Lonas はシェーダーを書き換えない。

- Lonas は、インスタンスのデータと「インスタンス番号のリスト」をどのバインドグループ・バインディング番号に置くかを **決まりとして公開** する
- **決定：Compute Shader が使えない環境でも「インスタンス番号のリスト」を用意し、CPU で並べた順（並べ替えない場合は 0, 1, 2, …）を書き込む。これにより、ユーザーのシェーダーを環境によらず 1 つにできる**

#### 視界の判定に使う大きさ

**決定：`Object*` の頂点から自動で計算する方法と、ユーザーが指定する方法の両方を使えるようにする。**

```rust
impl Object3D {
    pub fn bounding_sphere(&mut self, center: [f32; 3], radius: f32) -> &mut Self; // 任意
}
```

- `bounding_sphere` を指定した場合はその値を使う。指定しない場合は、頂点から境界球を自動で計算する（頂点が変わったフレームでのみ計算し直す）
- 境界球は `Object*` のローカル空間の値。各インスタンスの変換（位置と最大の拡大率）を掛けて判定する
- 自動計算は手軽だが、頂点が多いと計算に時間がかかる。また、シェーダーで頂点を動かす場合は実際の範囲と合わないため、その場合はユーザーが指定する

#### 視界内の数が GPU で決まる場合の描き方

視界に入るインスタンス数は Compute Shader が決めるため、CPU は描く数を知らない。

- **決定：**`Object*` ごとの描画の引数は、視界に入るかどうかに関わらず **常に 1 つずつ用意** し、Compute Shader が `instance_count` だけを書き込む。視界に 1 つも入らない `Object*` は `instance_count = 0` になり、描画は実質的に何もしない
- これにより、引数の数は CPU 側で分かっているため、`MULTI_DRAW_INDIRECT_COUNT` 機能（描く数そのものを GPU から読む機能）は **不要**
  - `MULTI_DRAW_INDIRECT` が使える環境：`multi_draw_indexed_indirect` で 1 回にまとめて描く
  - 使えない環境：`Object*` ごとに `draw_indexed_indirect` を呼ぶ
- `MULTI_DRAW_INDIRECT_COUNT` が必要になるのは、空の引数（`instance_count = 0`）も詰めて数を減らしたい場合のみ。`Object*` の数が非常に多く、その大半が視界外になる場合に効果があるが、現時点では使わない

#### CPU による並べ替え（Compute Shader が使えない環境）

- 並べる単位は「インスタンス 1 つ」。描画の引数を `instance_count = 1`、`first_instance = そのインスタンスの位置` で 1 つずつ作り、並べた順に配置する
- 並べた後も、連続する同じ設定の引数はまとめて描く（§10 DrawCall の単位）
- インスタンスのデータ自体は並べ替えない（引数の `first_instance` で参照するだけ）

### 深度バッファとクリア

| | 2D | 3D |
|---|---|---|
| 深度バッファ | **使わない**（描いた順に上書き） | 使う |
| 深度のクリア | なし | **各 `DrawManager3D` の描画の最初にクリア** |

- `DrawManager3D` が複数ある場合、それぞれの深度は独立する（例：ワールドの 3D の上に、別の 3D の UI を重ねられる）
- **画面の色のクリアは毎フレーム、サーフェスごとに指定する**：`end_frame` に渡す `SurfaceDraw::clear_color: Option<Color>`（§11 複数ウィンドウ）。そのサーフェスの最初の `DrawManager*` の描画の前にクリアする。`None` ならクリアしない（前のフレームの内容に重ねて描く）

### パイプラインのキャッシュ

**決定：次の組み合わせごとにパイプラインを作り、使い回す。**

| キー | 内容 |
|---|---|
| シェーダー | `ShaderId`（§11） |
| プリミティブの種類 | 三角形のみのため現状は固定 |
| ブレンド | `Blend` |
| 背面カリング | `CullMode`（§11） |
| 深度 | 2D / 3D、不透明 / 半透明で決まる |
| 頂点の並び方 | `DrawManager*` ごとの `VertexLayout`（下記） |

- キャッシュの持ち主は `DrawManager*`（案）。2D / 3D でパイプラインの中身が異なるため

### 頂点の並び方

**決定：`DrawManager*` ごとに、必要な頂点の要素を選べるようにする。**

```rust
pub enum VertexLayout {
    Position,              // 12 バイト
    PositionColor,         // 28 バイト
    PositionColorUv,       // 36 バイト
    PositionColorUvNormal, // 48 バイト
}

impl DrawManager2D { // DrawManager3D も同じ
    pub fn new(gpu: &mut GpuContext, camera: Camera, vertex_layout: VertexLayout) -> Self;
}
```

- GPU へ転送する際に、`VertexLayout` に含まれる要素だけを詰めて書き込む。2D で位置と色だけ使う場合などに GPU メモリを節約できる
- **決定：`VertexLayout` に含まれない要素を `vertex*` 関数で書いた場合（例：`Position` の `DrawManager*` に `vertex_color` で書いた `Object*`）、余分な要素を捨てて `log` で警告する。**
  - 警告は `Object*` ごとに 1 回だけ出す（毎フレーム出さない）
  - 逆に `VertexLayout` の要素が足りない場合は、§1 の頂点の入力と同じく 0.0 で埋める
- `Object*` の CPU 側は従来どおり `Vertex`（全要素）で持つ

---

## 11. 型・リソース・環境の詳細

### Camera

**決定：2D と 3D で別の型にする。**`DrawManager2D` は `Camera2D`、`DrawManager3D` は `Camera3D` を持つ。

```rust
pub struct Camera2D {
    pub position: Vector2f,
    pub rotation: f32,
    pub zoom: f32,
    pub viewport_size: Vector2f,
    pub view_projection: Matrix4f,
}

pub struct Camera3D {
    pub position: Vector3f,
    pub rotation: Quaternion,
    pub target: Vector3f,
    pub up: Vector3f,
    pub projection_mode: ProjectionMode, // 透視投影 / 正射影
    pub fov_y: f32,
    pub aspect_ratio: f32,
    pub near: f32,
    pub far: f32,
    pub viewport_size: Vector2f,
}
```

- カメラの削除：**`remove_camera(id)` を用意する。**削除済みの `CameraId` を使った場合は **panic**

### Instance

**決定：2D 用に小さい型を分ける。2D / 3D とも色（`Color`）と表示 / 非表示（`Visibility`）を持つ。**

```rust
pub struct Instance2D {
    pub position: Vector2f,
    pub rotation: f32,
    pub scale: Vector2f,
    pub color: Color,
    pub visibility: Visibility,
}

pub struct Instance3D {
    pub transform: Matrix4f,
    pub color: Color,
    pub visibility: Visibility,
}
```

- `Object2D` は `Instance2D`、`Object3D` は `Instance3D` を受け取る
- 非表示のインスタンスは、視界の外を省く処理（§10）と同じ段階で描画対象から外す
- **決定：インスタンスの `Color` は頂点の色と掛け算で合成する**（`最終の色 = 頂点の色 × インスタンスの色`）

### インスタンス API

**決定：`add_instance` は `InstanceId` を返す（§7 の案を確定）。**`add_instance` だけはメソッドチェーンにならない。

- 存在しない / 別の `Object*` の `InstanceId` を `update_instance` / `remove_instance` に渡した場合は **panic**

### 数学型

**決定：`Matrix4f` / `Vector2f` などの数学型は自前で作る。**外部の数学クレートに依存しない。

### ResourceManager（シェーダー・テクスチャ）

**決定：シェーダーとテクスチャは `ResourceManager` に登録し、`Object*` からは登録時に返る ID で参照する。**

```rust
// 決定：ResourceManager は GpuContext とは別の型としてユーザーが持つ
let mut resource_manager = ResourceManager::new(...);
let shader_id: ShaderId = resource_manager.register_shader(wgsl_source)?;
let texture_id: TextureId = resource_manager.register_texture(image_bytes)?;
resource_manager.unregister_shader(shader_id);
resource_manager.unregister_texture(texture_id);

object.shader(shader_id);
```

- **決定：シェーダーの比較・ハッシュは `ShaderId` を比較・ハッシュする。**ID は登録ごとに一意な整数のため、比較は整数 1 つで済み、同値検知（§3）とパイプラインのキー（§10）にそのまま使える。シェーダーの中身（WGSL の文字列）を毎回比べる必要がない
- **テクスチャ：`ResourceManager` に登録した後、`TextureAtlas` のレイヤーごとに格納する。**
  - **`TextureAtlas` の構成**：1 枚の画像（アトラス）に複数のテクスチャを詰め込み、さらにそのアトラスを z 方向に重ねたテクスチャ配列（`texture_2d_array`）の各レイヤーとして持つ。x・y でアトラス内の位置、z でレイヤーを表す
  - アトラスにまとめることでテクスチャの切り替えが不要になり、レイヤーで枚数を増やせるため、大量のテクスチャを 1 つのバインディングで効率よく扱える（バッチング（§10）を崩さない）
  - **アトラスの大きさ・レイヤー数などはすべて `LonasConfig` で指定する**
- **テクスチャの登録：画像のバイト列から登録する。**`Object*` へは `object.texture(texture_id)` で指定する
- **シェーダーの登録：WGSL の文字列から登録する。登録の解除も用意する**（テクスチャも同様に解除を用意する）

### uniform

**決定：uniform を渡す関数を用意する。**高頻度で呼ばれる可能性があるため、前の値を保持して比較し、**同値の場合は無視する**。

- 同値の場合に警告は出さない（高頻度で呼ぶことを想定した関数のため）
- 前の値の保持は `change_detection_cache` の設定に関係なく常に行う（下記）
- uniform は用途別メモリ空間の uniform 領域に置く（§6）
- **決定：uniform は `Object*` ごとに渡す。渡せる型は `bytemuck` の `Pod` を満たす型。**

```rust
impl Object2D { // Object3D も同じ
    pub fn uniform<T: bytemuck::Pod>(&mut self, value: &T) -> &mut Self;
}
```

- 前の値とはバイト列として比較する（`Pod` のため可能）

### バインドグループ・バインディング番号

**決定：シェーダーに公開するバインドグループ・バインディング番号（カメラ / インスタンス / インスタンス番号のリスト / テクスチャ / uniform）は `LonasConfig` で指定する。**ユーザーが書くシェーダー（§10）と番号を合わせるため。

### 2D の頂点の入力

**決定：`Object2D` には 2D 用の `vertex(x, y)` を用意する。**

```rust
impl Object2D {
    pub fn vertex(&mut self, x: f32, y: f32) -> &mut Self;
    pub fn vertex_color(&mut self, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32) -> &mut Self;
    pub fn vertex_color_uv(&mut self, x: f32, y: f32, r: f32, g: f32, b: f32, a: f32, u: f32, v: f32) -> &mut Self;
}
```

- `Object3D` は従来どおり `vertex(x, y, z)` など

### 同値の設定の扱いと change_detection_cache

**決定：`shader()` 以外の設定（`blend` / `topology` / `camera` / `layer` / `memory_update_type`）も、毎フレーム同じ値で呼ばれた場合は `shader()` と同じく無視して警告する。**

| 対象 | `change_detection_cache = false` の場合 |
|---|---|
| `shader` / `blend` / `topology` / `camera` / `layer` / `memory_update_type` | 前の値を保持せず、同値検知をしない（呼ばれた値をそのまま反映、警告なし） |
| 頂点・インデックスのその場比較（§4） | **対象外**（常に比較する） |
| uniform | **対象外**（常に前の値と比較する） |

### エラーの方針

**決定：環境に起因するエラーは `Result` で返す。**

- 例：アダプター / デバイスが取得できない、サーフェスが作れない、デバイスが失われた
- ユーザーのコードの誤用（§9 の表）は従来どおり panic
- **決定：エラーの型は列挙型 `LonasError` とする。**環境に起因するエラーを返す関数はすべて `Result<T, LonasError>` を返す
- **`GpuContext::new` の非同期処理（wgpu の `request_adapter` / `request_device`）は内部で待つ**（gueiz と同じく `pollster`）。`GpuContext::new` 自体は同期関数

### 座標系

**決定：2D の原点と 3D の座標系は `LonasConfig` で指定する。**

### 2D の視界外の省略

**決定：2D で視界の外を省く処理を行うかどうかは `LonasConfig` で指定する。**

### 背面カリング

**決定：`Object*` ごとに設定できるようにする。呼ばなかった場合の値は `LonasConfig` で指定する。**

```rust
pub enum CullMode {
    None,
    Back,
    Front,
}

impl Object2D { // Object3D も同じ
    pub fn cull_mode(&mut self, cull_mode: CullMode) -> &mut Self;
}
```

- `cull_mode` はパイプラインのキー（§10）に含める

### 色

**決定：`Color` は sRGB の値として受け取る。**サーフェスが sRGB のフォーマットの場合、シェーダーへ渡す前に線形に変換する。

### インデックスの型

**決定：インデックスは `u32` で固定する。**

### サーフェスのフォーマット

**決定：`LonasConfig` で指定する。**

### 環境の判定

**決定：Compute Shader / `MULTI_DRAW_INDIRECT` が使えるかどうかは、アダプターから自動で判定する。**

### 複数ウィンドウ

**決定：`GpuContext` 1 つで複数のサーフェス（ウィンドウ）に対応する。**

**決定：フレームは全体で 1 つとし、`begin_frame` は全体で 1 回、`end_frame` にサーフェスごとの描画をまとめて渡す。**

```rust
let (mut gpu, main_surface) = GpuContext::new(window, config)?;
let sub_surface: SurfaceId = gpu.add_surface(other_window)?;

gpu.begin_frame();
// 各 Object* の end ...
gpu.end_frame(&mut [
    SurfaceDraw { surface: main_surface, clear_color: Some(color), draw_managers: &mut [&mut dm_3d, &mut dm_2d] },
    SurfaceDraw { surface: sub_surface,  clear_color: None,        draw_managers: &mut [&mut dm_ui] },
]);

gpu.resize(main_surface, width, height);
gpu.set_anti_aliasing(main_surface, AntiAliasing::Msaa { sample_count: 4 })?;
```

- データの転送・GC のフレームの数え方・リングバッファの切り替え・`queue.submit` は **1 フレームに 1 回**。ウィンドウの数が増えても変わらない
- 描画先の取得（`get_current_texture`）と `present` は、`end_frame` に渡されたサーフェスについてのみ行う。そのフレームで描かないサーフェスは触らない
- 画面の色のクリアは `SurfaceDraw::clear_color` でサーフェスごとに指定する（`None` ならクリアしない）
- 却下案：サーフェスごとに `begin_frame` / `end_frame` を呼ぶ形（ウィンドウの数だけ転送・GC の数え方・`submit` が増える）
gpu.resize(main_surface, width, height);
```

- ヒープ（§6）・`ResourceManager`・パイプラインはサーフェス間で共有する
- 描画先の取得・`present`・深度バッファはサーフェスごと

### スレッド

**決定：`Object*` / `DrawManager*` を別スレッドで扱えるようにする（`Send`）。**

- `Object*::end` は `&mut DrawManager*` を取るため、別々の `DrawManager*` であれば複数のスレッドから同時に `end` できる
- フレーム番号は `Arc<AtomicU64>` で共有しており（§9）、スレッドをまたいでも判定できる

### ring_count の上限

**決定：`ring_count` は 1〜4。**5 以上は `memory_update_type()` の時点で panic（0 は従来どおり panic）。

### アンチエイリアス

**決定：None / MSAA / FXAA / TAA のすべてを `LonasConfig` で選べるようにする。**

```rust
pub enum AntiAliasing {
    None,
    Msaa { sample_count: u32 },
    Fxaa,
    Taa,
}
```

| 方式 | 仕組み | Lonas 側で必要になるもの |
|---|---|---|
| `None` | なし | なし |
| `Msaa` | GPU の機能で 1 ピクセルを複数の点で塗る | パイプライン・深度バッファ・描画先のサンプル数を揃える。描き終えた後に 1 サンプルへまとめる（resolve） |
| `Fxaa` | 描き終えた画像の輪郭をぼかす後処理 | 後処理のパスを 1 つ追加 |
| `Taa` | 毎フレームカメラを少しずらし、前のフレームの結果と混ぜる | 前のフレームの画像（履歴）・動きのベクトル・カメラのずらし（下記） |

- `sample_count` はアダプターが対応していない値なら、`GpuContext::new` が `Result` のエラーを返す（環境に起因するため、§11 エラーの方針）。wgpu で必ず使えるのは 1 と 4
- 2D / 3D の `DrawManager*` で共通の設定（サーフェス単位で効く）

#### TAA で必要になるもの

- **カメラのずらし（ジッター）**：`Camera3D` / `Camera2D` の投影行列にフレームごとに 1 ピクセル未満のずれを加える
- **履歴の画像**：サーフェスごとに前のフレームの結果を 1 枚保持する
- **動きのベクトル**：各ピクセルが前のフレームでどこにあったかを描画時に出力する
  - そのため、インスタンスの **前のフレームの変換** も保持する（インスタンスの領域が TAA 使用時のみ約 2 倍になる）
  - シェーダーはユーザーが書く（§10）ため、動きのベクトルの出力もユーザーのシェーダーで行う。必要な値（前のフレームの変換・カメラ）はバインディング番号で公開する
- 動きの速いものに残像のようなぼやけが出やすいため、履歴をどの程度使うかの調整が必要
- **決定：TAA の細かい値は `LonasConfig` で指定する。**

```rust
pub struct TaaConfig {
    /// 履歴を混ぜる割合の範囲（信頼度が最低のとき〜最高のとき）
    pub history_blend_min: f32,
    pub history_blend_max: f32,
    /// この深度差を超えたら履歴の信頼度を下げる
    pub depth_rejection_threshold: f32,
    /// 履歴の色が周囲の色からこの量を超えて外れたら信頼度を下げる
    pub color_rejection_threshold: f32,
    /// 動きのベクトルを書き込む画像のフォーマット
    pub motion_vector_format: TextureFormat,
}
```

- **決定：カメラのカットを Lonas に知らせる API を作成する。**呼ばれたサーフェスの TAA の履歴を無効化する（API の形は未決 §12）

#### TAA の処理の流れ

**決定：次の流れで行う。**

1. **取得**：現在のフレームの色・深度・動きのベクトル（モーションベクトル）を取得する
2. **再投影**：動きのベクトルを使い、前のフレームの履歴画像から、各ピクセルが前のフレームで対応する位置の色を読み出す
3. **信頼度の評価**：次の判定から、読み出した履歴をどれだけ信頼できるかを求める
   - 画面外判定：再投影した位置が前のフレームの画面の外なら信頼しない
   - 深度差：前のフレームの深度と現在の深度が大きく違えば、別の物が見えていたとみなし信頼度を下げる
   - 色の不一致：履歴の色が現在のピクセルの周囲の色から大きく外れていれば信頼度を下げる
4. **合成**：信頼度に応じて履歴のブレンド率を調整し、現在の画像と合成する（信頼度が高いほど履歴を多く使い、低いほど現在の画像を多く使う）
5. **保存**：合成結果を次のフレーム用の履歴画像として保存する。深度差の判定のため、現在の深度も次のフレーム用に保存する
6. **無効化**：カメラのカット（場面の切り替え）や解像度の変更などでは履歴を無効化し、現在の画像から履歴を作り直す

- 解像度の変更（`resize`）では自動で無効化する
- 履歴の画像・前のフレームの深度はサーフェスごとに持つ

#### 実行中の切り替え

**決定：アンチエイリアスの方式を実行中に変更できるようにする。**

```rust
// 決定：サーフェスごとに 1 つ設定する
gpu.set_anti_aliasing(surface_id, AntiAliasing::Taa)?;
```

- 変更は次の `begin_frame` で反映する（フレームの途中では切り替えない）
- 反映時に作り直すもの：描画先（MSAA 用の描画先・後処理用の画像・TAA の履歴）、深度バッファ（サンプル数が変わるため）、パイプライン（MSAA のサンプル数はパイプラインに含まれるため、キャッシュを作り直す）
- 対応していない `sample_count` を指定した場合は `Result` のエラー（`GpuContext::new` と同じ）
- TAA に切り替えた直後は履歴がないため、無効化した状態から始める
### 深度バッファ

**決定：次のとおりとする。**

| 項目 | 決定 | 理由 |
|---|---|---|
| フォーマット | `Depth32Float` | どの環境でも使え、精度が最も高い。ステンシルは使わない（§10 でステンシルの技法は却下）ため不要 |
| 深度の向き | **Reversed-Z**（手前を 1.0、奥を 0.0） | 浮動小数点の精度が奥で落ちる問題を打ち消し、遠くのちらつき（Z ファイティング）を大幅に減らせる。`Camera3D` の投影行列と比較関数（`Greater`）を合わせるだけで、追加の負担はない |
| 持ち方 | サーフェスごとに 1 つ。同じサーフェスの `DrawManager3D` 間で共有し、各 `DrawManager3D` の最初にクリア（§10） | `DrawManager3D` ごとに持つよりメモリが少ない |
| リサイズ時 | `resize()` では大きさを記録するだけにし、次の `begin_frame` で大きさが変わっていれば作り直す | 1 フレームにリサイズのイベントが何度来ても、作り直しは 1 回で済む |
| 2D | 持たない（§10） | |

---

## 12. 未決事項

- [ ] `end` 時の更新条件の全容（頂点変更以外に何をトリガーとするか）→ 随時検討

### 確認事項

- [ ] 同じサーフェスを `end_frame` に 2 回渡した場合、同じ `DrawManager*` を複数のサーフェスに渡した場合の扱い（panic か、複数のウィンドウに同じ内容を描けるようにするか）
- [ ] カメラのカットを知らせる API の形（`gpu.invalidate_history(surface_id)` / カメラ側の `cut()` など）
- [ ] `Camera3D` の `rotation` と `target` の関係：両方を持つ場合、どちらを優先するか（`target` を使う「注視点」方式と、`rotation` を使う方式を切り替えるか）
- [ ] `Camera2D` の `view_projection`：ユーザーが設定する値か、他の値から Lonas が計算して保持する値か。`Camera3D` も同様に計算した行列を持つか
- [ ] `LonasConfig::anti_aliasing` は、各サーフェスの最初の値として使うか（サーフェスごとに切り替えられるため）
- [ ] TAA の色の不一致の判定方法（周囲のピクセルの色の範囲に収める方法など）

### ResourceManager・テクスチャ

- [ ] `ResourceManager` が GPU のデバイスにどうアクセスするか（`ResourceManager::new(&gpu)` で受け取る / 登録時に `&mut GpuContext` を渡す）
- [ ] `end_frame` で `ShaderId` / `TextureId` から実体を引くために `ResourceManager` をどう渡すか（`end_frame` の引数に追加するか）
- [ ] 使用中のシェーダー / テクスチャを登録解除した場合の扱い（panic か、使われなくなるまで遅らせるか）
- [ ] 画像のバイト列の形式：PNG / JPEG などの符号化済みデータか、RGBA の生データか（gueiz は `image` クレートで PNG / JPEG を読む）
- [ ] UV をアトラス上の位置に変換するのは誰か（Lonas がシェーダーに各テクスチャのアトラス上の範囲を渡し、ユーザーのシェーダーで変換する、など）。`Object*` 1 つに複数のテクスチャを指定できるか
- [ ] アトラスへの詰め込み方、ミップマップ、隣のテクスチャのにじみ対策（余白）
- [ ] `TextureAtlasConfig` の中身（大きさ・レイヤー数・余白など）

### 型の中身

- [ ] `Visibility` の型（`bool` か列挙型か）
- [ ] `LonasConfig` の `BindingConfig` / `CoordinateSystemConfig` / `SurfaceFormat` の中身。2D の単位（ピクセルか）
- [ ] `LonasError` の種類

### 案のまま残っている箇所

- [ ] §4 Object 内部構造、§6 DrawManager 経由のアクセス、§6 `TlsfAllocator` の構造、§7 高頻度更新への対応、§10 パイプラインのキャッシュの持ち主
