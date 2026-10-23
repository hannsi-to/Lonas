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

gpu.end_frame(&mut resource_manager, &mut [
    SurfaceDraw { surface: main_surface, clear_color: Some(color), draw_managers: &mut [&mut draw_manager_3d, &mut draw_manager_2d] },
]);
```

**高頻度ではないが、描画中でも呼ばれる関数群**

```rust
object.camera(camera_id);
object.memory_update_type(MemoryUpdateType::Dynamic);
object.shader(shader_id);
object.texture(texture_id);
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
3. `end` : 変更があった場合のみ GPU への転送を要求し、描画コマンドを `DrawManager*` に積む（下記の更新条件）

### begin / end の対応

**決定：`begin` と `end` は、描画する `Object*` ごとに毎フレーム 1 回ずつ対で呼ぶ。**

| 呼び方 | 扱い |
|---|---|
| そのフレームで `begin` を呼ばずに `end` を呼ぶ | `end` で **panic** |
| 同じ `Object*` を 1 フレームに 2 回以上 `end` する | 2 回目の `end` で **panic**。`Object*` の GPU 上の領域は 1 つのため、2 つの内容を同時には描けない。同じ形を何度も描く場合はインスタンスを使う |
| 同じ `Object*` で 1 フレームに 2 回以上 `begin` する | 2 回目の `begin` で **panic** |
| `begin` したが `end` しない | そのフレームは描かない（`end` されなかった `Object*` と同じ。§6 GC） |

- `begin` / `end` を呼んだフレーム番号（§9）を `Object*` が記録し、それと比べて判定する
- フレームの外（`begin_frame` の前 / `end_frame` の後）での `begin` も `begin` の時点で **panic**

**決定：`Object*` は `Object2D::new(&gpu)` / `Object3D::new(&gpu)` で作り、そのときにフレーム番号を受け取る。**

```rust
impl Object2D { // Object3D も同じ
    pub fn new(gpu: &GpuContext) -> Self;
}

let mut rock = Object3D::new(&gpu);

// 毎フレーム
rock.begin();         // フレームの外や、同じフレームで 2 回目なら panic
rock.vertex(...); ...
rock.end(&mut dm3d);  // そのフレームで begin していなければ panic
```

- `new` で `GpuContext` からフレーム番号の `Arc<AtomicU64>`（§9）の複製と、`Object*` の識別子（`InstanceId` の検査に使う、§7）を受け取る。`&GpuContext` で済むよう、識別子は `GpuContext` の中のアトミックな数で発行する
- `begin` は引数を取らないが、受け取ったフレーム番号で判定できる
- 却下案：`Object2D::new()` を引数なしにし、フラグだけで判定する（「`begin` だけして `end` しなかった」次のフレームの `begin` が 2 回目と区別できず、§2 の表どおりに判定できない）
- 動かない物体でも毎フレーム頂点を書く。書いた値は前フレームの値と比べるだけで、変わらなければ転送しない（§1）

### end 時の更新条件

**決定：`end` は次の変化を判定し、必要な転送・確保だけを要求として `DrawManager*` に積む（§6 DrawManager 経由のアクセス）。**どの変化もなければ、転送も確保もせず、描画コマンドだけを積む。

| 変化 | 判定のしかた | `end` で行うこと |
|---|---|---|
| 初回の `end` | 領域が未確保 | すべての領域を確保し、すべてを転送 |
| GC で回収されていた | `DrawManager*` の回収済みの一覧（§6 回収の通知） | 初回と同じ |
| 頂点の値が変わった（数は同じ） | `vertex*` での比較（§4） | 変わった位置の最小〜最大の範囲だけを転送 |
| 頂点の数が変わった | `end` 時の書き込み位置と前の数を比べる | Static は **panic**、Dynamic は確保し直してすべてを転送（§6） |
| インデックスの値が変わった（数は同じ） | `index` での比較 | 変わった位置の最小〜最大の範囲だけを転送 |
| インデックスの数が変わった | 頂点と同じ | 頂点と同じ |
| 自動で作るインデックスの作り直し | `index()` を呼んでいない `Object*` で、`topology` が変わった / 頂点の数が変わった / `Polygon` で頂点の位置が変わった（§10） | インデックスを作り直し、上の 2 行と同じく扱う。Static で数が変わる場合も **panic**（変えたい場合は先に `memory_update_type(Dynamic)` にする旨をメッセージで伝える） |
| `index()` を使う / 使わないが切り替わった | そのフレームで `index()` が呼ばれたか | インデックスの値・数の変化として扱う |
| 頂点の位置が変わった | 頂点の比較 | 3D は `bounding_sphere`、2D は `bounding_circle` を指定していなければ計算し直す（2D は `culling_2d` が true のときのみ、§10） |
| `memory_update_type` が変わった | 設定の比較（§3） | 新しい側の領域へ移す（§6 確保後の memory_update_type の変更） |
| `shader` が変わった | 設定の比較 | 古いシェーダーの配列の要素を空きに戻し、新しいシェーダーの配列に要素を割り当てて書き込む（`DrawManager*` がその場で割り当てる、§6 uniform の領域）。全インスタンスの `uniform_index` を書き直す |
| `texture` が変わった | 設定の比較 | 全インスタンスの `texture_slot` を書き直す（§11 TextureAtlas） |
| `blend` / `cull_mode` / `camera` / `layer` / `topology`（`index()` 使用時）が変わった | 設定の比較 | 転送なし。積む描画コマンドの内容（パイプラインのキー・カメラ・描画順）が変わるだけ |
| uniform の値が変わった | バイト列の比較（§11 uniform） | その `Object*` の要素だけを転送 |
| インスタンスの追加・更新・削除 | `InstanceSlots` の変更の範囲（§7） | 変わったスロットの最小〜最大の範囲だけを転送。容量を超えたら確保し直してすべてを転送 |
| 使っている `CameraId` が無効 | `DrawManager*` の記録と比べる | **panic**（§11） |
| 使っている `ShaderId` / `TextureId` が無効 | `end` では判定できない（`Object*::end` は `ResourceManager` を受け取らない） | `end_frame` で `ResourceManager` と比べて **panic**（§11 登録解除） |

- 判定はすべて `Object*` の中で完結し、`GpuContext` には問い合わせない。GC の回収だけは `DrawManager*` に問い合わせる
- 描画コマンドは変化の有無に関わらず毎フレーム積む（`end_frame` で実行した後に消えるため、§9）
- 転送はすべて staging バッファ経由（§6 転送）
- 頂点・インデックスの変更の範囲は、`vertex*` / `index` で比較したときに、変わった位置の最小と最大を記録して求める（整数 2 つ。インスタンスと同じ方式、§7）
- TAA を使うサーフェスに描く場合、インスタンスの前のフレームの変換（§11 アンチエイリアス）は、転送の前に GPU 上で現在の値を前のフレーム用の領域へコピーして作る（`end_frame` 内）

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

### 同値で呼ばれたときの警告の設定

**決定：同値で呼ばれたときに警告を出すかどうかを、`LonasConfig::redundant_call_warning` で指定する。**同値の判定と無視は設定に関係なく常に行う。

- 設定の現在の値は描画に使うため必ず保持しており（§4）、同値の判定はその値と比べるだけで済む。判定のために余分なメモリは要らない
- 警告は開発中に「毎フレーム呼ぶ必要がない呼び出し」に気付くためのもの。リリース時やログを抑えたい場合に切れるようにする
- 却下案：同値検知用のキャッシュを無効にできる設定（`change_detection_cache`）。無効にしても減るメモリがない

### LonasConfig

**決定：`LonasConfig` のすべての項目に既定値を設けない。ユーザーが必ず全項目を指定する。**
`LonasConfig` / `MemoryConfig` / `MemoryCapacity` はいずれも `Default` を実装せず、既定値を埋めるコンストラクタも用意しない（構造体リテラルで全項目を書く）。
アプリごとに適切な値が大きく異なり、ライブラリ側で妥当な既定値を決められないため。

`GpuContext::new(window, width, height, config)` で受け取る（§9）。

```rust
pub struct LonasConfig {
    /// 設定の関数が同値で呼ばれたときに log で警告するか（§3 参照）
    pub redundant_call_warning: bool,
    /// インスタンス数がこのフレーム数だけ連続で増加したら警告する（§7 参照）
    /// None の場合は警告しない
    pub instance_growth_warning_frames: Option<u32>,
    /// 用途ごとのメモリ空間の割り当て量（§6 参照）
    pub memory: MemoryConfig,
    /// staging バッファのチャンクの大きさ（バイト、§6 転送）
    pub staging_chunk_size: u64,
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
    /// 各サーフェスの最初のアンチエイリアスの方式（§11 参照）
    /// GpuContext::new と add_surface で作るすべてのサーフェスがこの値で始まる
    /// 実行中は set_anti_aliasing でサーフェスごとに変更できる
    pub initial_anti_aliasing: AntiAliasing,
    /// 使う GPU の API（§8）
    pub backend: Backend,
    /// 各サーフェスの最初の Present mode（§8）
    /// 実行中は set_present_mode でサーフェスごとに変更できる
    pub initial_present_mode: PresentMode,
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
    /// uniform の配列（DrawManager × シェーダーごと）の最初の要素数（§6 uniform の領域）
    pub uniform_initial_elements: u32,
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
- `staging_chunk_size` が 0、または 4 の倍数でない（`copy_buffer_to_buffer` の制約）


---

## 4. Object 内部構造

**決定：`Object2D` / `Object3D` は共通部分（`ObjectCore`）を内部に持ち、型ごとの違いはインスタンスの型・`end` に渡せる `DrawManager*` の型・型固有の設定のみ。**

```rust
struct ObjectCore<I> { // I = Instance2D / Instance3D
    // 設定。None は未指定で、end の時点で LonasConfig の *_when_unset に解決する（§3）。DrawManager* が値を写して持つ
    shader: Option<ShaderId>,
    texture: Option<TextureId>,               // None ならテクスチャ未指定の画素（§11 TextureAtlas）
    camera_id: Option<CameraId>,              // None なら DrawManager の既定カメラ
    blend: Option<Blend>,
    topology: Option<Topology>,
    cull_mode: Option<CullMode>,
    memory_update_type: Option<MemoryUpdateType>, // None のまま end したら panic（§6）
    manager_id: Option<ManagerId>,            // 初回 end で紐づいた DrawManager

    // 頂点・インデックス。GPU に載っている内容の CPU 側の写し。vertex* / index で比較しながら上書き（§1）
    vertices: Vec<Vertex>,
    vertex_cursor: usize,                     // begin で 0 に戻す書き込み位置
    indices: Vec<u32>,                        // index() を呼ばなければ topology から生成（§10）
    index_cursor: usize,

    instances: InstanceSlots<I>,              // §7
    uniform: Vec<u8>,                         // 前の値（常に保持、§11 uniform）


    vertex_dirty_range: Option<Range<u32>>,   // そのフレームで変わった頂点の最小〜最大（§2）
    index_dirty_range: Option<Range<u32>>,    // インデックスも同じ
    uses_explicit_indices: bool,              // そのフレームで index() を呼んだか（§2・§10）

    instance_id_owner: ObjectId,              // InstanceId に入れる、この Object* の識別子（§7）
    uniform_slot: Option<u32>,                // uniform の配列の要素の番号。DrawManager が end で割り当てる。None なら要素 0（§6 uniform の領域）
    frame: Arc<AtomicU64>,                    // GpuContext と共有するフレーム番号（§9）。new で受け取る
    frame_state: FrameState,                  // 最後に begin / end を呼んだフレーム番号（§2）

    allocations: ObjectAllocations,           // 頂点 / インデックス / インスタンスの GpuAllocation（§6）
    dirty: DirtyFlags,                        // 頂点 / インデックス / 設定 / インスタンス / uniform の変更
    warned: WarnedFlags,                      // 一度だけ出す警告を出したか（§10 topology・VertexLayout、§7）
    instance_growth: InstanceGrowth,          // 前フレームのインスタンス数と連続増加の回数（§7）
}

pub struct Object2D {
    core: ObjectCore<Instance2D>,
    layer: Option<i32>,                       // None なら LonasConfig::layer_when_unset
    bounding_circle: Option<BoundingCircle>,  // None なら頂点から自動で計算（culling_2d が true のとき、§10）
}

pub struct Object3D {
    core: ObjectCore<Instance3D>,
    bounding_sphere: Option<BoundingSphere>,  // None なら頂点から自動で計算（§10）
}
```

- `vertex*` は `vertices[vertex_cursor]` と比較して異なれば上書きし dirty を立てる。`vertex_cursor == vertices.len()` なら push。`index` も同じ
- `end` 時に `vertex_cursor != vertices.len()` なら頂点数が変わったので truncate して dirty。インデックスも同じ
- `allocations` が未確保、または GC で回収済み（§6 解放）なら、初回とみなして領域を確保し、すべてを転送する
- `Object*` は GPU リソースの実体を持たない（§6）
- 設定を `Option` で持つことで、「一度も呼ばれていない」と「既定値と同じ値で呼ばれた」を区別できる

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
impl DrawManager2D { // DrawManager3D も同じ（Camera3D を使う）
    pub fn new(gpu: &mut GpuContext, camera: Camera2D, vertex_layout: VertexLayout) -> Self; // 渡したカメラが既定カメラになる（VertexLayout は §10）
    pub fn default_camera_id(&self) -> CameraId;
    pub fn add_camera(&mut self, camera: Camera2D) -> CameraId;
    pub fn remove_camera(&mut self, id: CameraId);
    pub fn camera(&self, id: CameraId) -> &Camera2D;
    pub fn camera_mut(&mut self, id: CameraId) -> &mut Camera2D;
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
    Dynamic,
}

impl Object2D { // Object3D も同じ
    pub fn memory_update_type(&mut self, memory_update_type: MemoryUpdateType) -> &mut Self;
}
```

- **既定値なし。**`memory_update_type()` を一度も呼ばずに `end` した場合は panic。Static / Dynamic で振る舞い（サイズ変更の可否）が大きく異なるため、ユーザーに必ず選ばせる
- **決定（段階 5）：Static 領域の先頭に積めなくても、穴を詰めれば収まる場合は、panic せずに先に詰め直してから確保する**

| | Static | Dynamic |
|---|---|---|
| 割り当て方式 | **スタック方式**（手前から順に積む） | **Free List Allocator** |
| 容量を超えた場合 | **panic**（`Object` を追加し続けてメモリを圧迫するのを防ぐ） | 必要なサイズが容量を超えた場合 **だけ** バッファを拡張 |
| 確保後の編集 | 確保した領域のサイズが **変わらない** 変更のみ可能。増える場合も減る場合も **不可（panic）** | 領域のサイズが変わることを前提とする |

- wgpu のバッファはサイズを変えられないため、Dynamic の拡張は「より大きなバッファを作成 → `copy_buffer_to_buffer` で既存の内容をコピー → 差し替え」で行う

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

**決定：頂点 / インデックス / インスタンスの用途ごとに、上記の「Static + Dynamic」のメモリ空間を持つ。**uniform はこのメモリ空間に置かず、別に持つ（§6 uniform の領域）。

**決定（段階 6）：メモリ空間は要素の数を単位に管理し、要素の大きさが違うものは空間を分ける。**頂点は `VertexLayout` ごと、インデックス、`Instance2D`、`Instance3D` の空間を持つ。
- 描画では、頂点の位置を「何番目の頂点か」（`base_vertex`）で、インスタンスを要素の番号（`instances[instance_order[i]]`）で指定する。そのため領域の先頭は要素の大きさの倍数でなければならず、バイト単位の割り当て（4 バイトに揃えるだけ）では満たせない
- `LonasConfig::memory` の容量はバイトのまま。空間ごとに要素の大きさで割って要素の数にする。頂点の空間は、その `VertexLayout` の `DrawManager*` が初めて使ったときに作り、それぞれが設定の容量を持つ（`VertexLayout` を何種類も使うと、その数だけ容量を使う）

| 用途 | Static 領域 | Dynamic 領域 |
|---|---|---|
| 頂点 | あり | あり |
| インデックス | あり | あり |
| インスタンス | **なし**（常に Dynamic） | あり |

- wgpu の `BufferUsages`（`VERTEX` / `INDEX` / `STORAGE` など）を用途ごとに分けられる
- 割り当て量も用途ごとに `LonasConfig` で指定する（§3）

### インスタンスの領域

**決定：インスタンスの領域は、`Object*` の `memory_update_type` に関係なく常に Dynamic 領域を使う。**

- インスタンスは追加・削除で領域のサイズが変わるため（§7）
- これにより Static の `Object*` でも `add_instance` / `remove_instance` が使える（頂点などは Static、インスタンスだけ Dynamic）

### uniform の領域

**決定：uniform は `DrawManager*` × `ShaderId` ごとに 1 本の storage バッファを持ち、その中に配列として置く。要素の番号は `DrawManager*` が `Object*::end` の時点で割り当てる。**

```
DrawManager A × ShaderId X のバッファ: [0][1][2][3]...
DrawManager A × ShaderId Y のバッファ: [0][1]...
DrawManager B × ShaderId X のバッファ: [0][1][2]...
  └ 要素 0 はすべて 0 の予約済みの要素
```

- バッチング（§10）は 1 つの `DrawManager*` の中の、同じシェーダーの `Object*` の間でしか起きない。そのため、1 回の DrawCall で読む uniform は必ず 1 本のバッファに収まる。バッファはシェーダーごとのバインドグループで指定し、パイプラインを切り替えるときに一緒に切り替える
- **要素の番号は `DrawManager*` が持つ空き番号の一覧から、`Object*::end` の時点で割り当てる。**インスタンスに書き込む `uniform_index`（§11 uniform）を `end` の時点で決められる
  - `GpuContext` が割り当てる形では、`Object*` が `GpuContext` に触れないため `end` の時点で番号が分からない
- GPU 上のバッファの実体はこれまでどおり `GpuContext` が持ち、`end_frame` で作る・大きくする。要素が足りなくなったら、より大きなバッファを新しく作り、`copy_buffer_to_buffer` で既存の内容をコピーして差し替える（別のバッファ同士のためコピーは 1 回。増やす量は `dynamic_growth_factor`、最初の要素数は `MemoryConfig::uniform_initial_elements`）
- `Object*` の要素の番号は、バッファを作り直しても変わらない。値が変わったときは、その要素だけを staging バッファ経由で書き込む（§6 転送）
- **要素 0 は予約し、すべて 0 で埋める。**`uniform()` を一度も呼ばなかった `Object*` は要素 0 を指す
- 要素の間隔は WGSL の配列の間隔（stride）。シェーダーから求める（§11 uniform）
- GC（§6 解放）で回収された `Object*` の要素は、回収の通知を受けた `DrawManager*` が空きに戻す（§6 回収の通知）
- 頂点などのメモリ空間（TLSF）には置かないため、uniform に TLSF・オフセットの制約（`min_storage_buffer_offset_alignment`）は関係しない
- 却下案：uniform のメモリ空間に `ShaderId` ごとの配列を TLSF で並べる（配列を大きくするときに同じバッファ内でのコピーになり、`copy_buffer_to_buffer` が使えない。また番号を `end` の時点で決められない）
- 却下案：`ShaderId` ごとに `DrawManager*` をまたいで 1 本にし、番号を共有の割り当て器で配る（割り当て器の同時アクセスにロックかアトミックな空き管理が必要になる）

### 転送：map 済みの staging バッファ

**決定：GPU への転送は、`DrawManager*` が持つ map 済みの staging バッファに `Object*::end` が直接書き込み、`end_frame` で `copy_buffer_to_buffer` によってヒープへコピーする。**

```
Object*::end
  └ 変更があった分を、VertexLayout に合わせて詰めながら staging バッファ（map 済み）へ直接書き込む
GpuContext::end_frame
  └ staging バッファの map を解除 → copy_buffer_to_buffer でヒープの領域へコピー → 描画 → submit
  └ submit 後、使い終わった staging バッファを map_async で map し直す（次以降のフレームで再利用）
```

- `queue.write_buffer` は、呼び出しのたびに wgpu の内部で一時領域を確保する。staging バッファを使い回せば、この確保がなくなり、複数の書き込みを同じチャンクにまとめられる
  - **段階 5 で分かったこと：**wgpu 30 では map したメモリを `&mut [u8]` として直接書けない（書き込み専用の型 `WriteOnly` を経由し、`copy_from_slice` で写す）。`VertexLayout` に合わせて詰める作業用のバイト列（`DrawManager*` が使い回す）から 1 回写すため、CPU でのコピーの回数は `write_buffer` と同じ。速くなるのは一時領域の確保がなくなる分
- `copy_buffer_to_buffer` は queue に積んだ順に実行されるため、前のフレームの描画が読んでいる領域へ書いても競合しない。そのため、書き込み先の領域をフレームごとに切り替えるリングバッファは持たない
- staging バッファは一定の大きさ（`LonasConfig::staging_chunk_size`）の塊（チャンク）に分けて持つ
  - 空いた map 済みのチャンクがなければ、`mapped_at_creation: true` で新しく作る（作った時点で map 済みのため待ちが起きない）
  - `map_async` で map し直したチャンクは、次以降の `end_frame` で完了を確認（`device.poll` で待たずに確認）してから空きに戻す。GPU がまだ読んでいるチャンクに書き込むことはない
  - 1 回の書き込みがチャンクより大きい場合は、その大きさの専用のチャンクを作る
- `DrawManager*` は `DrawManager*::new(&mut gpu, ...)` で device の複製を受け取り、チャンクを自分で作る（wgpu の `Device` は内部で参照を数えているため、複製は安い）
- 却下案：`Object*` ごとのリングバッファ（`MemoryUpdateType::Dynamic { ring_count }`）。転送をコピーで行う限り、前のフレームの描画とは競合しないため効果がない。効果が出るのはヒープのバッファ自体を map して書き込む場合だが、wgpu では頂点などのバッファを map するのに `MAPPABLE_PRIMARY_BUFFERS` が必要で、単体の GPU では GPU から読む速度が落ちる
- 却下案：`DrawManager*` の `Vec<u8>` に書き写し、`end_frame` で `queue.write_buffer` する（呼び出しのたびに wgpu の内部で一時領域を確保する）
- map し直しが終わったチャンクは、`end_frame` の最初と、新しいチャンクを作る前に空きに戻す。GPU が使っている間は別のチャンクに書くため、画面に出す場合は 2〜3 つに落ち着く（段階 5 で確認）

### Dynamic の割り当て：TLSF

**決定：Dynamic 領域の Free List Allocator は TLSF（Two-Level Segregated Fit）の考え方で設計する。構造は下記のとおり。**
Blender の Vulkan バックエンドが使う VMA（Vulkan Memory Allocator）の既定アルゴリズムと同じ方式。

#### 選んだ理由

- 確保・解放とも **O(1)**（空き領域の探索がリストの長さに依存しない）。毎フレーム確保・解放が起きる Dynamic 領域に向く
- 近いサイズの空き領域を選ぶ（good-fit）ため断片化しにくい
- 解放時に隣接する空き領域と結合する

#### メタデータは CPU 側に持つ

GPU バッファの中にはブロックのヘッダを書き込めないため、VMA の仮想割り当てと同様に、**ブロック情報はすべて CPU 側に持つ**。
GPU バッファは「オフセットとサイズで区切られた領域」としてのみ扱う。

```rust
struct TlsfAllocator {
    blocks: Vec<Block>,             // ブロックの実体。インデックスで参照する
    unused_block_slots: Vec<u32>,   // 再利用できる blocks のインデックス
    first_level_bitmap: u32,        // 空きブロックがある第 1 レベルのビット
    second_level_bitmaps: [u32; FIRST_LEVEL_COUNT], // 第 1 レベルごとの第 2 レベルのビット
    free_heads: [[Option<u32>; SECOND_LEVEL_COUNT]; FIRST_LEVEL_COUNT], // サイズクラスごとの空きリストの先頭
    last_physical_block: u32,       // 領域の末尾のブロック（拡張時に使う）
    alignment: u64,                 // アラインメント（4 バイト）
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

- 第 1 レベル `fl` = `floor(log2(size)) - 7`（`SMALL_SIZE` 以上の場合）。2 の累乗ごとの大きな区分。`SMALL_SIZE` 未満を `fl = 0` に使うため 7 ずらす
- 第 2 レベル `sl` = 第 1 レベルの範囲を `SECOND_LEVEL_COUNT` 等分した区分。`sl = (size >> (fl - SLI)) - SECOND_LEVEL_COUNT`
- 小さいサイズ（`SMALL_SIZE` 未満）は `fl = 0` にまとめ、線形に等分する（VMA と同様）
- 各サイズクラスに空きブロックの双方向リストを持ち、空きがあるかをビットマップで管理する

| 定数 | 値 | 意味 |
|---|---|---|
| `SLI` | 5（`SECOND_LEVEL_COUNT` = 32） | VMA と同じ。サイズクラス内の無駄は最大約 1/32（約 3%） |
| `SMALL_SIZE` | 256 バイト | これ未満は線形に分類 |
| `FIRST_LEVEL_COUNT` | 32 | 2^39 バイト（512GB）未満のブロックを扱える |
| `MIN_BLOCK_SIZE` | 16 バイト | 分割した残りがこれ未満なら分割せず、確保したブロックに含める |

#### 確保

1. サイズを 4 バイトの倍数に切り上げる（`copy_buffer_to_buffer` の制約）
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

#### サイズ変更

- サイズが変わった場合は「新しいブロックを確保 → 新しいデータを staging バッファから書き込む（§6 転送）→ 古いブロックを解放」の順で行う
  - サイズ変更は新しいデータの書き込みを伴うため、古いブロックから GPU 上でコピーする必要はない（`Object*` が CPU 側に頂点を持っている、§4）
  - 古いブロックを同じフレームで別の `Object*` に再利用しても、コピーは queue に積んだ順に実行されるため、前のフレームの描画とは競合しない

### 確保後の memory_update_type の変更

**決定：初期化後（初回 `end` で領域を確保した後）に `memory_update_type()` が呼ばれた場合、新しい側の領域へ内容をコピーする。**

- Static → Dynamic：Dynamic 領域に確保して書き込み、Static 側の領域は不要になる
- Dynamic → Static：Static 領域のスタックに積んで書き込み、Dynamic 側の領域をフリーリストへ返す
- **決定：書き込みは `Object*` が CPU 側に持っているデータ（§4）から行う。**Static と Dynamic は同じバッファ内にあり GPU 上で直接コピーできないが、CPU 側のデータを staging バッファ経由で書き込めば（§6 転送）、ヒープ内でのコピーも一時バッファも不要。Dynamic のサイズ変更時と同じ処理になる
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
- 回収は `Object*` 単位で行い、その `Object*` の頂点 / インデックス / インスタンスの領域と uniform の要素をまとめて回収する

#### 回収の通知

**決定：回収したことは、`DrawManager*` を経由して `Object*` に知らせる。**

`Object*` は頂点が変わらなければ転送しないため、回収されたことを知らないと、中身のない領域を描いてしまう。

- ハンドル（`GpuAllocation`）には発行元の `ManagerId` が入っている。`GpuContext` は回収したハンドルを `ManagerId` ごとに溜めておき、その `DrawManager*` が `end_frame` に渡されたときに渡す
- `Object*::end` は、自分のハンドルが回収済みかを `DrawManager*` に問い合わせる。回収済みなら初回と同じく確保し直し、CPU 側の写し（§4）からすべてを転送する
- `DrawManager*` は、回収済みのハンドルが再び `end` されるか、ハンドルが再利用されない期間を過ぎるまで、回収済みの一覧を持つ
- 却下案：ハンドルに世代を持たせ、`Arc` で共有する表で判定する（同時アクセスのためにロックかアトミックの配列が必要になる）

### DrawManager 経由のアクセス

`gpu.end_frame(&mut [&mut dm])` で `GpuContext` を可変借用するため、`DrawManager*` が `&mut GpuContext` を保持し続けることはできない。
そこで `Object*::end` は確保・転送・解放を **要求として `DrawManager*` に積み**、`GpuContext::end_frame` でまとめて実行する。

```
Object*::end
  └ DrawManager* に「領域確保 / 転送 / 描画」の要求を積む
GpuContext::end_frame
  └ 各 DrawManager* の要求を処理：ヒープから確保 → staging バッファからコピー（§6 転送）→ DrawCall
```

- ハンドル（`GpuAllocation`）は `DrawManager*` が `ManagerId` + 連番で即座に発行し、実際のヒープ上の位置との対応は `GpuContext` が持つ
- 転送が `end_frame` にまとまるため、複数 `Object*` の転送を 1 回の書き込みにまとめやすい
- `Arc` / ロックを使わずに済む

**決定：要求は列挙型で積み、転送するデータは `DrawManager*` が持つ map 済みの staging バッファに直接書き込む（§6 転送）。**

```rust
enum HeapRequest {
    Allocate   { handle: GpuAllocation, space: SpaceKind, size: u64, update: MemoryUpdateType },
    // 大きさや memory_update_type が変わった。中身は Write で書き直す（段階 5 で追加）
    Reallocate { handle: GpuAllocation, space: SpaceKind, size: u64, update: MemoryUpdateType },
    Write      { handle: GpuAllocation, space: SpaceKind, offset: u64, chunk: u32, range: Range<u64> }, // staging バッファのチャンクと範囲
    WriteUniform { shader: ShaderId, slot: u32, chunk: u32, range: Range<u64> }, // uniform の要素（§6 uniform の領域）
}

struct DrawManagerCore {
    staging: StagingBelt,            // map 済みのチャンクの集まり（§6 転送）
    heap_requests: Vec<HeapRequest>,
    draw_commands: Vec<DrawCommand>,
    reclaimed: HashSet<GpuAllocation>, // GC で回収されたハンドル（§6 回収の通知）
    touched: Vec<(SpaceKind, GpuAllocation)>, // そのフレームで end された Object* の領域。描かないフレームでも GC に回収されないように知らせる（段階 5 で追加）
    uniform_slots: HashMap<ShaderId, UniformSlots>, // シェーダーごとの uniform の要素の空き番号（§6 uniform の領域）
}
```

- `MemorySpace` は頂点 / インデックス / インスタンス（§6 用途ごとのメモリ空間）。uniform は `WriteUniform` で書き、`end_frame` で必要な要素数までバッファを作る・大きくする
- `Object*::end` は変更があったときだけ、`VertexLayout`（§10）に合わせて詰めながら staging バッファへ書き込み、その範囲を `Write` に入れる。変更がなければ何も書かない
- `end_frame` は、コピー元とコピー先がともに連続する `Write` をまとめて 1 回の `copy_buffer_to_buffer` にする
- 却下案：要求に `Object*` のデータへの参照を持たせる（`end_frame` まで `Object*` を借用し続けることになり、`begin` で `Object*` を書き換える使い方と両立しない）

---

## 7. インスタンス API

インスタンスの追加・更新・削除は **毎フレーム呼ばれるとは限らないが、高頻度の更新に対応できる** ようにする。

```rust
// 決定（§11）
impl Object2D { // Object3D も同じ（Instance3D を受け取る）
    pub fn add_instance(&mut self, instance: Instance2D) -> InstanceId<Instance2D>;
    pub fn update_instance(&mut self, id: InstanceId<Instance2D>, instance: Instance2D);
    pub fn remove_instance(&mut self, id: InstanceId<Instance2D>);
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
- 必要な状態は「前フレームのインスタンス数」と「連続増加カウンタ」のみで、`redundant_call_warning` の設定に依存しない

### 高頻度更新への対応

**決定：インスタンスはスロットの配列で持ち、削除したスロットは非表示にして再利用する。インスタンスは移動しない。**

```rust
pub struct InstanceId<I> {         // I = Instance2D / Instance3D
    slot: u32,
    generation: u32,
    owner: ObjectId,                  // 発行元の Object* の識別子。別の Object* の InstanceId を検知する（§11）
    _marker: PhantomData<fn() -> I>,  // I の性質によらず Copy / Send にするため fn() -> I
}

struct InstanceSlots<I> {
    slots: Vec<I>,                  // GPU に送るインスタンスの CPU 側の写し
    generations: Vec<u32>,          // スロットごとの世代。削除のたびに増やす
    free_slots: Vec<u32>,           // 空いているスロット
    dirty_range: Option<Range<u32>>, // そのフレームで変わったスロットの最小〜最大
}
```

- `add_instance` は空いているスロットがあれば再利用し、なければ末尾に追加する
- `remove_instance` はそのスロットに `Visibility::Hidden` を書き込み、空きに戻す。穴は Compute Shader が非表示として省く（§10）ため、描画には影響しない
- スロットの位置が変わらないため、`InstanceId` から位置への対応表は不要。TAA の前のフレームの変換（§11）もずれない
- 末尾のスロットが空いた場合は切り詰める。途中の穴を詰め直すことはしない（`InstanceId` の位置が変わってしまうため）
- 世代が合わない `InstanceId`（削除済み）を `update_instance` / `remove_instance` に渡した場合は panic（§11）
- 転送は、そのフレームで変わったスロットの最小〜最大の範囲を 1 回で行う
- インスタンスの領域は §6 の Dynamic 領域に、容量に余裕を持たせて確保し、超過時のみ再確保する
- 却下案：`swap_remove` で常に詰める（GPU が読む量は最小になるが、`InstanceId` から位置への対応表が必要で、移動のたびに TAA の前のフレームの変換も動かす必要がある）

---

## 8. 描画バックエンド

**決定：gueiz と同じ方式をとる。**

- `wgpu` を使用（gueiz は wgpu 30）
- 頂点データの変換は `bytemuck`、ログは `log`
- バックエンド選択は gueiz の `renderer.rs` と同様に `Auto / Vulkan / DirectX12 / Metal / OpenGl` から指定可能にする
- Present mode も gueiz の `SurfacePresentMode` と同様に指定可能にする
- **決定：バックエンドは `LonasConfig::backend` で指定する。Present mode はサーフェスごとの値とし、最初の値を `LonasConfig::initial_present_mode`、以降は `set_present_mode` で変更する。**

```rust
pub enum Backend {
    Auto, // wgpu が環境に合わせて選ぶ
    Vulkan,
    DirectX12,
    Metal,
    OpenGl,
}

pub enum PresentMode {
    AutoVsync,   // 垂直同期あり。使えるものを Lonas が選ぶ（FifoRelaxed → Fifo）
    AutoNoVsync, // 垂直同期なし。使えるものを Lonas が選ぶ（Immediate → Mailbox → Fifo）
    Fifo,
    FifoRelaxed,
    Mailbox,
    Immediate,
}

impl GpuContext {
    pub fn set_present_mode(&mut self, surface: SurfaceId, mode: PresentMode) -> Result<(), LonasError>;
}
```

  - バックエンドは `GpuContext` 全体で 1 つのため設定に置く。Present mode はサーフェスごとに作るため、アンチエイリアス（§11）と同じ形にする。設定画面で垂直同期を切り替えるような使い方に対応できる
  - 指定したバックエンドのアダプターがなければ `Err(AdapterNotFound)`。`Fifo` 以外の Present mode にサーフェスが対応していなければ `Err(UnsupportedPresentMode)`（`Auto*` は必ず何かを選べるため `Err` にならない）
  - `set_present_mode` の反映は次の `begin_frame`（サーフェスの設定し直し）
  - 却下案：Present mode も設定に固定する（垂直同期を切り替えるのに `GpuContext` の作り直しが必要になる）
- **決定：WebGPU 環境は対象としない。**Compute Shader の有無などの環境の分岐は、ネイティブのバックエンド（Vulkan / DirectX12 / Metal / OpenGL）の間でのみ考える
- **決定：頂点シェーダーで storage バッファを読めない環境（OpenGL ES 3.0、OpenGL 4.3 未満など）は対象としない。**
  - インスタンス（§11 Instance）・インスタンス番号のリスト（§10）・uniform（§11 uniform）をすべて storage バッファで渡し、ユーザーのシェーダーを環境によらず 1 つにするため
  - wgpu の `DownlevelFlags::VERTEX_STORAGE` が使えることを条件とし、使えないアダプターでは `GpuContext::new` が `Err(UnsupportedAdapter)` を返す（環境に起因、§11 エラーの方針）
  - 対象となるのは Vulkan / DirectX12 / Metal / OpenGL 4.3 以上
- **決定：Compute Shader も必須とする。**
  - 頂点シェーダーで storage バッファを読める環境は、実際にはすべて Compute Shader も使える。Compute Shader がない場合の道（CPU による並べ替え）は通る環境がないため持たない
  - wgpu の `DownlevelFlags::COMPUTE_SHADERS` / `INDIRECT_EXECUTION` と `Features::INDIRECT_FIRST_INSTANCE`（間接描画で 0 以外の `first_instance` を使う、§10 DrawCall の単位）が使えることを条件とし、使えないアダプターでは `GpuContext::new` が `Err(UnsupportedAdapter)` を返す
  - 視界の外を省く処理と奥行きの並べ替えは常に GPU で行う（§10 描画順）。CPU が決めるのは、パイプラインをまたぐ半透明の順番だけ
- **決定：使う wgpu のバージョンで `MULTI_DRAW_INDIRECT` が使えない環境は対象としない。**
  - バッチング（§10）は常に `multi_draw_indexed_indirect` で行い、`Object*` ごとに描画を呼ぶ道は持たない
  - **wgpu 30 では `MULTI_DRAW_INDIRECT` という機能はない。**`multi_draw_indexed_indirect` は `DownlevelFlags::INDIRECT_EXECUTION` があれば使え、使えない環境では wgpu が内部で代わりに処理する。そのため判定は `INDIRECT_EXECUTION` に含まれる（段階 4 で確認）
  - 却下案：インスタンスを頂点バッファ（インスタンス単位）で渡す道を別に用意する（環境ごとにユーザーのシェーダーを書き分ける必要があり、§10 の「シェーダーを 1 つにする」決定と衝突する）

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
    // window は GpuContext が持ち続けるため、所有権ごと渡す（winit なら Arc<Window>）
    pub fn new(
        window: impl HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
        width: u32,
        height: u32,
        config: LonasConfig,
    ) -> Result<(Self, SurfaceId), LonasError>;
    pub fn add_surface(
        &mut self,
        window: impl HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
        width: u32,
        height: u32,
    ) -> Result<SurfaceId, LonasError>;
    pub fn resize(&mut self, surface: SurfaceId, width: u32, height: u32);
    pub fn surface_size(&self, surface: SurfaceId) -> Vector2f; // resize で通知された大きさ（ピクセル）
    pub fn begin_frame(&mut self);
    pub fn end_frame(&mut self, resource_manager: &mut ResourceManager, surfaces: &mut [SurfaceDraw<'_, '_>]);
}

pub struct SurfaceDraw<'a, 'b> {
    pub surface: SurfaceId,
    pub clear_color: Option<Color>, // None ならクリアしない（§10）
    pub draw_managers: &'a mut [&'b mut dyn DrawManager], // 渡した順に描画する
}
```

```rust
gpu.begin_frame();

object_3d.end(&mut draw_manager_3d);
object_ui.end(&mut draw_manager_2d);

// サーフェスごとに、渡した順に描画する（3D → 2D）
gpu.end_frame(&mut resource_manager, &mut [
    SurfaceDraw { surface: main_surface, clear_color: Some(color), draw_managers: &mut [&mut draw_manager_3d, &mut draw_manager_2d] },
]);
```

- **決定：`new` / `add_surface` は、ウィンドウの最初の大きさ（ピクセル）を受け取る。**`raw-window-handle` からは大きさが分からず、サーフェスの設定に必要なため。以降の変化は `resize` で知らせる（段階 4 で決定）
- **決定：ウィンドウは `Send + Sync + 'static` の値として所有権ごと受け取る。**wgpu のサーフェスはウィンドウより長く生きてはならないため、`GpuContext` が持ち続ける（winit なら `Arc<Window>` を複製して渡す）（段階 4 で決定）
  - OpenGL のバックエンドで、表示（ディスプレイ）の情報が要る環境には未対応。必要になったら `new` で表示の情報も受け取る
- `DrawManager` は `DrawManager2D` / `DrawManager3D` が実装するトレイト。`end_frame` に 2D / 3D を混ぜて渡すために使う
- 配列の借用（`'a`）と各 `DrawManager*` の借用（`'b`）の寿命を分ける。同じ寿命にすると、Rust の可変参照の性質（寿命を縮められない）により、`DrawManager*` の借用が必要以上に長く続くことがある

| 関数 | 処理 |
|---|---|
| `GpuContext::begin_frame` | フレームの開始（フレーム番号を進める）。描画先の取得は行わない |
| `GpuContext::end_frame` | 下記の順で処理する |

`end_frame` の処理の順：

1. **前のフレームの後片付け**：map し直しが完了した staging バッファのチャンクを、各 `DrawManager*` の空きに戻す（§6 転送）
2. **リソースの反映**：`ResourceManager` の未反映の登録・解除を GPU に反映する（シェーダーの作成、アトラスへの配置・転送・ミップマップ、解除したものとそのパイプラインの破棄、§11）
3. **ヒープの処理**（渡された全 `DrawManager*` をまとめて 1 回）：
   1. 各 `DrawManager*` の staging バッファの map を解除する
   2. `HeapRequest` を処理する：解放 → 確保（必要なら Dynamic 領域の拡張、uniform のバッファの作成・拡張）→ Static 領域のコンパクション（しきい値を超えた場合、§6）
   3. TAA のサーフェスに描く `DrawManager*` について、インスタンスの現在の値を前のフレーム用の領域へコピーする（§2）
   4. staging バッファからヒープへコピーする（`copy_buffer_to_buffer`、§6 転送）
4. **カメラ**：各 `DrawManager*` のカメラの行列を計算して転送する（TAA のジッターを含む、§11 Camera）
5. **Compute**：視界の外・非表示のインスタンスを省き、並べ替え、インスタンス番号のリストと描画の引数を書き込む（§10）
6. **描画**（渡されたサーフェスごと）：描画先の取得（取得できなければ飛ばす、§9 リサイズとフレームスキップ）→ `DrawManager*` ごとに RenderPass で実行 → アンチエイリアスの後処理（resolve / FXAA / TAA、§11）
7. **送信**：`submit` を 1 回行い、各サーフェスを `present` する
8. **後片付け**：各 `DrawManager*` のコマンドと要求をクリアする。使った staging バッファのチャンクを `map_async` で map し直す。GC で期限切れの領域を回収し、回収したハンドルを `DrawManager*` ごとに溜める（§6 回収の通知）。使われなくなったパイプラインを捨てる（§10）

責務の分担：

| 型 | 持つもの |
|---|---|
| `GpuContext` | device / queue / surface、GPU メモリのヒープ（2D / 3D 共通）、シェーダー・テクスチャアトラスの GPU 上の実体（§11）、パイプラインのキャッシュ（§10）、フレーム境界、`ManagerId` の発行、`LonasConfig` |
| `ResourceManager` | `ShaderId` / `TextureId` の発行、WGSL の検査、画像の読み込み。GPU にはアクセスしない（§11） |
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
| フレーム外（`begin_frame` 前 / `end_frame` 後）の `Object*::begin` / `Object*::end` | `Object*::begin` / `Object*::end` で panic（§2） |
| `Object*` を別の `DrawManager*` に渡す | `Object*::end` で panic（§5） |
| 別の `DrawManager*` の `CameraId` を使う | `Object*::end` で panic（§5） |
| `memory_update_type()` を呼ばずに `end` | `Object*::end` で panic（§6） |
| そのフレームで `begin` を呼ばずに `end` / 1 フレームに 2 回 `end` / 1 フレームに 2 回 `begin` | `Object*::end` / `Object*::begin` で panic（§2） |
| Static の `Object*` で頂点・インデックスの数が変わる（`topology` の変更による自動のインデックスを含む） | `Object*::end` で panic（§2・§6） |
| 削除済みの `CameraId` を使う | panic（§11） |
| 存在しない / 別の `Object*` の `InstanceId` を `update_instance` / `remove_instance` に渡す | panic（§11） |
| 登録解除済みの `ShaderId` / `TextureId` を使う | `end_frame` で panic（§11 登録解除） |
| `uniform()` に渡した型の大きさがシェーダーの uniform の要素と違う、またはシェーダーに uniform の配列がない | `end_frame` で panic（§11 uniform） |
| 別の `ResourceManager` の `ShaderId` / `TextureId` を使う、または `Object*` の ID と別の `ResourceManager` を `end_frame` に渡す | `end_frame` で panic（§11） |
| 同じサーフェスを `end_frame` の `SurfaceDraw` に 2 回以上渡す | `end_frame` で panic（§11 複数ウィンドウ） |
| `index()` も `topology()` も呼ばずに `end` | `Object*::end` で panic（§10） |
| `DrawManager*` に積んだのに `end_frame` に渡さなかった | 次のフレームでその `DrawManager*` に `Object*::end` が呼ばれた時点で panic（前のフレームのコマンドが残っている） |

- フレームの状態は `GpuContext` と各 `DrawManager*`・各 `Object*` で共有する **フレーム番号**（`Arc<AtomicU64>`）で判定する。フレーム中かどうかも同じ値に入れる（例：最下位ビット）。フレーム中かどうかに加え、`DrawManager*` に残っているコマンドがどのフレームのものかも分かる。`DrawManager*` が `GpuContext` を借用し続けずに済む
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
  - **決定（段階 5）：2D は入力の向きによらず xy 平面で反時計回りに揃える。3D は入力の頂点の並びと同じ向きにする。**3D は多角形の平面がどちらを向くかで表裏が決まり、表にする側はユーザーが頂点の順で選ぶため

#### Polygon の分割方法

**決定：`Polygon` の三角形化は CPU で行い、単調多角形分割（monotone polygon decomposition）を使う。計算量は O(n log n)。**

- **段階 5 の実装：**走査線に交わる辺の一覧は配列で持ち、頂点の左の辺を線形に探す。計算量は並べ替えの O(n log n) と、走査線に同時に交わる辺の数 k に比例する O(n·k)。k は多くの形で小さいため、木構造（O(log n) で探せる）にするのは、遅さが問題になってから
- 3D の多角形は、Newell の方法で求めた法線の最も大きい軸を落として 2D にしてから分割する

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
- **決定：間接描画の引数を GPU バッファに並べ、`multi_draw_indexed_indirect` で 1 回にまとめて描く。**
  - `MULTI_DRAW_INDIRECT` が使えない環境は対象としない（§8）。環境による分岐は持たない
- **決定：引数の単位は次のとおり。**

| 描くもの | 引数の単位 | `instance_count` | 引数を書くもの |
|---|---|---|---|
| 2D | `Object*` ごと | Compute Shader が書く（非表示・視界外を省いた数） | 位置は CPU、`instance_count` は GPU（§10 2D の Compute のパス） |
| 3D の不透明 | `Object*` ごと | Compute Shader が書く（視界内の数） | 位置（`first_index` など）は CPU、`instance_count` は GPU |
| 3D の半透明 | **インスタンスごと** | 1（非表示・視界外は 0） | すべて GPU（奥から手前へ並べた順に書く） |

  - 3D の半透明は、違う `Object*` のインスタンスどうしを奥から手前へ交互に描く必要があるため、インスタンスごとの引数にする。半透明だけインスタンシングが効かなくなるが、`Object*` 同士が前後に重なっても正しく重なる
  - 3D の半透明の引数の数は、そのパイプラインの半透明のインスタンスの総数で CPU にも分かる。そのため `MULTI_DRAW_INDIRECT_COUNT` は不要（§10 視界内の数が GPU で決まる場合の描き方）
  - 却下案：3D の半透明も `Object*` ごとの引数にし、並べ替えは `Object*` の中だけにする（`Object*` 同士が前後に重なると重なり方が間違う）
- **決定：`first_instance` は「インスタンス番号のリスト」の読み出し位置に使う。**頂点シェーダーは `instances[instance_order[instance_index]]` で読む（`instance_index` は `first_instance` から始まる）
  - 不透明・2D：その `Object*` のリストの先頭
  - 3D の半透明：並べた順のリストの、そのインスタンスの位置
  - 間接描画で 0 以外の `first_instance` を使うには wgpu の `INDIRECT_FIRST_INSTANCE` が必要。必須とする（§8）
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
並べ替えは Compute Shader で行う（Compute Shader は必須、§8）。

| | 並べ替え |
|---|---|
| 不透明 | Compute Shader で **視界の外を省く処理（フラスタムカリング）と、奥行きの大まかな並べ替え** を `Object*` の中で行う。`Object*` 間はパイプラインごとにまとめる |
| 半透明 | **同じパイプラインの中** のすべてのインスタンスを、`Object*` をまたいで Compute Shader で奥から手前へ並べ替え、その順に引数を書く。**パイプラインをまたぐ順番は `end` の順**（下記） |

**決定：パイプラインをまたぐ半透明の順番は、`end` の順で決める。**
- 各パイプラインの半透明の `Object*` のうち、最初に `end` されたものの順にパイプラインを描く
- CPU は奥行きを計算しない。パイプラインの異なる半透明が前後に重なる場合は、ユーザーが `end` の順で調整する（2D の `layer` と同じ考え方）
- 却下案：CPU がインスタンスの写しからパイプラインごとの代表の奥行きを計算する（半透明のインスタンスの数だけ CPU の計算が増え、代表の値では交差する場合に正しくならない）

#### Compute Shader による並べ替え

インスタンスのデータは GPU 上の Dynamic 領域にある（§6）ため、CPU に読み戻さずに GPU 上で完結する。

```
end_frame
  1. Compute：各インスタンスの位置をカメラ空間に変換し、奥行きを計算。視界の外のインスタンスを省く
  2. Compute：奥行きでインスタンス番号を並べ替え、「描く順のインスタンス番号のリスト」と間接描画の引数を書き込む
     - 不透明：Object* ごとの引数の instance_count = 視界内の数
     - 半透明：パイプラインごとに、並べた順にインスタンスごとの引数（instance_count = 1）
  3. 描画：頂点シェーダーはリストを経由してインスタンスを読む
     instance = instances[instance_order[instance_index]]
```

- 並べ替えの結果は「インスタンス番号のリスト」として持つため、不透明は `Object*` ごとの引数が 1 つのまま。インスタンシングとバッチングを崩さない
- 不透明の並べ替えは速度のため（深度テストで奥のピクセルを早く捨てる）であり、正確さは不要。奥行きを段階に分けて振り分ける **大まかな並べ替え（バケットソート）** で行う
- 半透明は正しく重ねるために正確な並べ替えが必要。同じパイプラインの中で、`Object*` をまたいで GPU で並べ替える。パイプラインの切り替えは描画中にしかできないため、パイプラインをまたぐ順番は `end` の順（上記）
- CPU は奥行きの計算も並べ替えもしない

#### 2D の Compute のパス

**決定：2D も 3D と同じ Compute のパスを通す。並べ替えはせず、スロットの順のまま詰める。**

```
end_frame（2D の DrawManager）
  1. Compute：各インスタンスについて、Hidden（削除したスロットを含む、§7）なら省く。
     culling_2d が true なら、境界円（下記）が視界の外のものも省く
  2. Compute：残ったインスタンスを、累積和でスロットの順のまま詰めて「インスタンス番号のリスト」に書き、
     Object* ごとの引数の instance_count を書く
```

- 2D は深度を使わず描いた順に上書きする（§10 深度バッファとクリア）ため、同じ `Object*` の中でもスロットの順を保つ。累積和で詰めれば順番は変わらない
- `culling_2d` が false でも、Hidden を省くためにこのパスは通す
- ユーザーのシェーダーは 3D と同じく `instances[instance_order[instance_index]]` で読む
- 却下案：Compute を使わず CPU がリストを作る（視界の外を省く場合、カメラが動くたびに CPU で全インスタンスを判定することになる）

#### 段階 6 で決めた描画の細部

- **`DrawManager*` ごとに描画のパスを分ける。**3D の `DrawManager3D` は自分のパスの最初に深度をクリアする（§10 深度バッファとクリア）。画面の色のクリアは、そのサーフェスの最初のパスだけ
- **ユーザーのシェーダーは、頂点とフラグメントのエントリーポイントを 1 つずつ持つ。**パイプラインはエントリーポイントの名前を指定せずに作る（wgpu が 1 つだけあるものを使う）
- **`shader()` を呼んでいない `Object*` は描かない。**`Object*` ごとに 1 回だけ `log` で警告する
- インスタンスの視界の判定は、カメラの行列から求めた 6 つの平面と境界の球で行う。2D も同じ処理（z は 0）
- 3D の不透明の大まかな並べ替えは、近い〜遠いを 16 段に分けて振り分ける（`near` / `far` で正規化）
- 3D の半透明の並べ替えは、パイプラインとカメラのまとまりごとにバイトニックソートで行う（要素数を 2 の累乗に切り上げる）

#### シェーダーでのインスタンスの読み方

**決定：頂点シェーダーが「インスタンス番号のリスト」を経由してインスタンスを読む部分は、ユーザーが書く。**Lonas はシェーダーを書き換えない。

- Lonas は、インスタンスのデータと「インスタンス番号のリスト」をどのバインドグループ・バインディング番号に置くかを **決まりとして公開** する
- 対象とする環境はすべて Compute Shader が使えるため（§8）、「インスタンス番号のリスト」は常に Compute Shader が書き込む。ユーザーのシェーダーは環境によらず 1 つ

#### 視界の判定に使う大きさ

**決定：`Object*` の頂点から自動で計算する方法と、ユーザーが指定する方法の両方を使えるようにする。3D は境界球、2D は境界円。**

```rust
impl Object3D {
    pub fn bounding_sphere(&mut self, center: Vector3f, radius: f32) -> &mut Self; // 任意
}

impl Object2D {
    pub fn bounding_circle(&mut self, center: Vector2f, radius: f32) -> &mut Self; // 任意。culling_2d が true のときに使う
}
```

- 指定した場合はその値を使う。指定しない場合は、頂点から自動で計算する（頂点が変わったフレームでのみ計算し直す）
- 境界球・境界円は `Object*` のローカル空間の値。各インスタンスの変換（位置と最大の拡大率）を掛けて判定する。円・球のため、インスタンスの回転を考えなくてよい
- 2D と 3D で判定の形が同じため、Compute Shader の処理を共通にできる
- 2D で `culling_2d` が false の場合は、境界円を計算しない
- 却下案：2D は矩形（AABB）にする（スプライトや UI にはぴったり合うが、インスタンスの回転で矩形が傾くため、判定で 4 隅を変換する必要がある）
- 自動計算は手軽だが、頂点が多いと計算に時間がかかる。また、シェーダーで頂点を動かす場合は実際の範囲と合わないため、その場合はユーザーが指定する

#### 視界内の数が GPU で決まる場合の描き方

視界に入るインスタンス数は Compute Shader が決めるため、CPU は描く数を知らない。

- **決定：**不透明の `Object*` ごとの描画の引数は、視界に入るかどうかに関わらず **常に 1 つずつ用意** し、Compute Shader が `instance_count` だけを書き込む。視界に 1 つも入らない `Object*` は `instance_count = 0` になり、描画は実質的に何もしない
- 半透明のインスタンスごとの引数も、非表示・視界外を含めて **インスタンスの数だけ用意** し、省いたものは `instance_count = 0` にする（並べた順の後ろに寄せる）
- これにより、引数の数は CPU 側で分かっているため、`MULTI_DRAW_INDIRECT_COUNT` 機能（描く数そのものを GPU から読む機能）は **不要**
  - `multi_draw_indexed_indirect` で 1 回にまとめて描く（§10 DrawCall の単位）
- `MULTI_DRAW_INDIRECT_COUNT` が必要になるのは、空の引数（`instance_count = 0`）も詰めて数を減らしたい場合のみ。`Object*` の数が非常に多く、その大半が視界外になる場合に効果があるが、現時点では使わない

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
| 描画先の構成 | サーフェスのフォーマット・MSAA のサンプル数・TAA の動きのベクトルを出力するか（§11） |

**決定：キャッシュは `GpuContext` が 1 つ持つ。**

- 描画先の構成はサーフェスごとの値で、同じ `DrawManager*` でもフレームによって渡されるサーフェスが変わり得る。そのため、`DrawManager*` ではなく描画先の構成まで含めたキーで、全体で共有する
- シェーダーの実体と登録解除（§11 ResourceManager）も `GpuContext` にあるため、登録解除されたシェーダーのパイプラインを 1 か所で捨てられる
- 同じ `VertexLayout` の `DrawManager*` の間でパイプラインを共有できる
- `gc_unused_frames`（§3）のフレーム数だけ使われなかったパイプラインは捨てる。アンチエイリアスを切り替えて使われなくなったパイプラインもこれで消える
- 却下案：`DrawManager*` ごとに持つ（登録解除やアンチエイリアスの切り替えのたびにすべての `DrawManager*` のキャッシュを捨てる必要があり、`end_frame` に渡されなかった `DrawManager*` には届かない）

### 頂点の並び方

**決定：`DrawManager*` ごとに、必要な頂点の要素を選べるようにする。**

```rust
pub enum VertexLayout {
    Position,              // 12 バイト
    PositionColor,         // 28 バイト
    PositionColorUv,       // 36 バイト
    PositionColorUvNormal, // 48 バイト
}

impl DrawManager2D { // DrawManager3D も同じ（Camera3D を受け取る）
    pub fn new(gpu: &mut GpuContext, camera: Camera2D, vertex_layout: VertexLayout) -> Self;
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
}

pub struct Camera3D {
    pub position: Vector3f,
    pub rotation: Quaternion,
    pub projection_mode: ProjectionMode, // 透視投影 / 正射影
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

pub enum ProjectionMode {
    Perspective,
    // 写す範囲の高さ（ワールドの単位）。幅は描くサーフェスの縦横比から求める（段階 5 で決定）
    Orthographic { height: f32 },
}

// 手系・2D の原点（LonasConfig）が必要なため、DrawManager* のメソッドにする（段階 6 で決定）
impl DrawManager2D { // DrawManager3D も同じ
    pub fn view_projection(&self, id: CameraId, viewport_size: Vector2f) -> Matrix4f; // 現在の値と描く大きさから計算して返す
}

impl DrawManager3D {
    pub fn look_at(&mut self, id: CameraId, target: Vector3f, up: Vector3f); // rotation を計算して設定する
}
```

- **決定（段階 6）：`look_at` と `view_projection` は `DrawManager*` のメソッドにする。**計算に手系・2D の原点が必要で、`DrawManager*` は作るときに `LonasConfig` からその値を写して持つ。カメラは値だけの構造体のまま（構造体リテラルで作れる）
  - 却下案：カメラのメソッドで、設定を引数で受け取る（呼ぶたびに座標系を渡す必要があり、`LonasConfig` と違う値を渡すと描画と食い違う）
  - 却下案：カメラが座標系を持つ（非公開の項目が増え、構造体リテラルで作れなくなる）
- **決定（段階 6）：2D のカメラは、`position` のワールド座標を画面の原点（`Origin2D`）に置き、拡大（`zoom`）と回転（`rotation`）は画面の原点を中心に行う。**2D は深度を使わないため、near / far は -1 / 1（z = 0 が深度の範囲の中に入る）

- **決定：`Camera3D` の向きは `rotation` だけで表す。**注視点で指定したい場合は `look_at(target, up)` が `rotation` を計算して設定する
  - 向きを表す値が 1 つだけなので、どちらを優先するかという問題が起きない
  - `look_at` は呼んだ時点の `position` から計算する。後から `position` を動かしても `target` を向き続けるわけではないため、追いかけるカメラは毎フレーム `look_at` を呼ぶ
  - 却下案：`target` / `up` も項目として持つ（片方を変えたとき、もう片方が古い値のまま残る）
  - 却下案：`Orientation::Rotation` / `Orientation::LookAt` の列挙型で切り替える（向きを読むたびに分岐が必要）
- **決定：ビュー・プロジェクション行列は項目として持たず、Lonas が計算する。**
  - `end_frame` で各 `DrawManager*` のカメラごとに 1 フレーム 1 回計算して GPU に送る（§9）。TAA のずらし（ジッター）もここで加える
  - ユーザーが行列を必要とする場合（マウス位置からの逆変換など）は `view_projection()` で取得する。ジッターは含まない
  - 項目と行列が食い違う状態が起きない
- **決定：カメラは描く大きさ（`viewport_size`）と縦横比（`aspect_ratio`）を持たない。`end_frame` で、そのカメラを描くサーフェスの大きさから求める。**
  - 同じ `DrawManager*` でもフレームによって描くサーフェスが変わり得るため、大きさはカメラではなくサーフェスの値として扱う
  - ユーザーはリサイズのときに `gpu.resize` だけを呼べばよい
  - `view_projection()` は描く大きさを引数に取る。`gpu.surface_size(surface)` で取得した値を渡す
  - シェーダーに渡す `Camera::viewport_size`（§11 シェーダーとの約束事）は、描くサーフェスの大きさ
  - 却下案：`viewport_size: Option<Vector2f>` にし、`None` ならサーフェスに合わせる（画面の一部に描く用途に使えそうだが、大きさだけでは描く位置を指定できず、その機能は別に必要になる）
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
- **決定：`Visibility` は列挙型にする。**構造体リテラルで意味が読み取れるため。GPU 上では `u32`（`Visible = 1` / `Hidden = 0`）として書き込む

```rust
pub enum Visibility {
    Visible,
    Hidden,
}
```

- 非表示のインスタンスは、視界の外を省く処理（§10）と同じ段階で描画対象から外す
- **決定：インスタンスの `Color` は頂点の色と掛け算で合成する**（`最終の色 = 頂点の色 × インスタンスの色`）

### インスタンス API

**決定：`add_instance` は `InstanceId` を返す（§7 の案を確定）。**`add_instance` だけはメソッドチェーンにならない。

- 存在しない / 別の `Object*` の `InstanceId` を `update_instance` / `remove_instance` に渡した場合は **panic**
- **決定：`InstanceId` はインスタンスの型を型引数に持つ（`InstanceId<Instance2D>` / `InstanceId<Instance3D>`）。**`Object2D` の ID を `Object3D` に渡すと **コンパイルエラー** になる
  - 同じ型の別の `Object*` の ID（例：`Object2D` 同士）は型では区別できないため、これまでどおり `owner` で実行時に検査して panic する
  - 却下案：寿命で `Object*` ごとに印を付ける（GhostCell と同じ手法。`Object*` をクロージャの外に出せなくなり、構造体に持たせられない）
  - 却下案：ユーザーが印の型を付ける（`Object2D<Player>`）。同じ印の `Object*` 同士は区別できず実行時の検査が残るうえ、型の表記が長くなる。既定値付きの型引数（`Object2D<M = ()>`）で後から足せるため、今は入れない

### 数学型

**決定：`Matrix4f` / `Vector2f` などの数学型は自前で作る。**外部の数学クレートに依存しない。

**決定：作り方は次のとおり。**名前は一般的な数学クレート（glam など）に合わせ、使い慣れた書き方で書けるようにする。

```rust
Vector2f::new(x, y)        Vector2f::ZERO   Vector2f::ONE   Vector2f::from([x, y])
Vector3f::new(x, y, z)     Vector3f::ZERO   Vector3f::ONE   Vector3f::from([x, y, z])
Quaternion::IDENTITY       Quaternion::from_axis_angle(axis: Vector3f, radians: f32)
Matrix4f::IDENTITY
Matrix4f::from_translation(v: Vector3f)
Matrix4f::from_rotation(q: Quaternion)
Matrix4f::from_scale(v: Vector3f)
Matrix4f::from_trs(translation: Vector3f, rotation: Quaternion, scale: Vector3f)
```

- 演算子（`+` / `-` / `*` など）や他の関数は実装時に必要なものを足す
- 角度はすべてラジアン

### ResourceManager（シェーダー・テクスチャ）

**決定：シェーダーとテクスチャは `ResourceManager` に登録し、`Object*` からは登録時に返る ID で参照する。**

```rust
// 決定：ResourceManager は GpuContext とは別の型としてユーザーが持つ。GPU にはアクセスしない
let mut resource_manager = ResourceManager::new();
let shader_id: ShaderId = resource_manager.register_shader(wgsl_source)?;          // naga で検査し、エラーなら Err
let texture_id: TextureId = resource_manager.register_texture(encoded_bytes)?;     // PNG / JPEG など
let texture_id: TextureId = resource_manager.register_texture_rgba(width, height, &rgba_bytes)?; // RGBA の生データ
resource_manager.unregister_shader(shader_id);
resource_manager.unregister_texture(texture_id);

object.shader(shader_id);

gpu.end_frame(&mut resource_manager, &mut [ /* SurfaceDraw ... */ ]);
```

#### GPU との分担

**決定：`ResourceManager` は CPU 側の処理だけを行い、GPU 上の実体（シェーダー・アトラスのテクスチャ）は `GpuContext` が持つ。登録・解除は `end_frame` で GPU に反映する。**

| | `ResourceManager`（CPU のみ） | `GpuContext`（`end_frame` 内） |
|---|---|---|
| シェーダー | ID の発行、WGSL の保持、**naga で WGSL を検査**して `Result` を返す | 未作成のものから `create_shader_module` で作る |
| テクスチャ | ID の発行、画像の読み込み（RGBA への変換） | アトラス上の配置を決め、ピクセルを `write_texture` で転送し、ミップマップを作る。転送後、`ResourceManager` 側の画像のデータは捨てる |
| 解除 | ID を無効にし、解除の要求を積む | GPU 上の実体を破棄し、その ID を使うパイプラインのキャッシュを捨てる |

- `ResourceManager` は登録・解除の要求を溜めておき、`end_frame(&mut resource_manager, ...)` で `GpuContext` が取り出して反映する。そのため引数は `&mut`
- §6 の「GPU リソースの実体は `GpuContext` に集約」と同じ形になる
- アトラス上の配置は `GpuContext` が決める。アトラスの設定（`LonasConfig::texture_atlas`）を `ResourceManager` が知らなくて済む
- `ResourceManager` が GPU に依存しないため、`GpuContext` の作成前に登録したり、別のスレッドで読み込んだりできる（`Send`）
- WGSL のエラーは、wgpu が内部で使う naga で CPU 側から検査できるため、登録の時点で `Result` を返せる（`LonasError` に WGSL のエラーを含める）
- 却下案：`ResourceManager::new(&gpu)` で device / queue の複製を持ち、登録時に GPU 上の実体を作る（`ResourceManager` が GPU に依存し、§6 の集約の方針から外れる）
- 却下案：登録時に `&mut GpuContext` を渡す（呼び出しのたびに `gpu` を借用する必要がある）
- 却下案：`DrawManager*` が `Arc<RwLock<ResourceManager>>` を共有する（§5 で却下したパターン）

#### ID

- `ShaderId` / `TextureId` は **発行元の `ResourceManager` の識別子・番号・世代** を持つ。番号を再利用しても、世代が違えば古い ID と区別できる
- 別の `ResourceManager` の ID が使われた場合は `end_frame` で **panic**（`CameraId` と同じ方式、§5）

#### 登録解除

**決定：`unregister_*` を呼んだ時点で ID を無効にする。無効な ID を持つ `Object*` を描こうとしたら、`end_frame` で panic。**

- `Object*::end` は `ResourceManager` を受け取らないため、`end` の時点では判定できない。`end_frame` が積まれた描画コマンドの ID を `ResourceManager` と比べる

- 削除済みの `CameraId` と同じ扱い（§11 Camera）
- GPU 上の実体は次の `end_frame` で破棄する。wgpu は送信済みのコマンドが使っているリソースを実行が終わるまで生かしておくため、すぐに破棄しても前のフレームの描画とは競合しない
- 却下案：使われなくなるまで解除を遅らせる（`Object*` が `GpuContext` を知らないため、参照の数を正確に数えられない）
- 却下案：`unregister_*` の時点で使用中なら panic（どの `Object*` が使っているかを `ResourceManager` が知る必要がある）

#### 画像の形式

**決定：符号化済みの画像（PNG / JPEG など）と RGBA の生データの両方から登録できる。**

- `register_texture(bytes)`：`image` クレートで読み込む。`image` クレートは Cargo の feature で外せるようにする（外した場合は `register_texture_rgba` のみ）
- `register_texture_rgba(width, height, bytes)`：RGBA 各 8 ビット。`bytes.len() != width * height * 4` なら panic（誤用）
- 読み込みに失敗した場合は `Err`

### TextureAtlas

**決定：登録したテクスチャは `TextureAtlas` のレイヤーに詰め込んで格納する。**

- **構成**：1 枚の画像（アトラス）に複数のテクスチャを詰め込み、さらにそのアトラスを z 方向に重ねたテクスチャ配列（`texture_2d_array`）の各レイヤーとして持つ。x・y でアトラス内の位置、z でレイヤーを表す
- アトラスにまとめることでテクスチャの切り替えが不要になり、レイヤーで枚数を増やせるため、大量のテクスチャを 1 つのバインディングで効率よく扱える（バッチング（§10）を崩さない）
- **アトラスの大きさ・レイヤー数などはすべて `LonasConfig` で指定する**（下記 `TextureAtlasConfig`）
- アトラス・サンプラーは `GpuContext` が 1 つ持ち、サーフェス・`DrawManager*` 間で共有する

#### Object* への指定

**決定：`Object*` に指定できるテクスチャは 1 つのみ。**複数必要な場合は `Object*` を分ける。

```rust
impl Object2D { // Object3D も同じ
    pub fn texture(&mut self, id: TextureId) -> &mut Self; // 「高頻度ではないが描画中でも呼ばれる」区分
}
```

- 同値の扱いは `shader()` などと同じ（§11 同値の設定の扱い）
- 将来、法線マップなどで複数が必要になった場合は、スロットを持つ形に拡張する

#### UV の変換

**決定：Lonas は GPU に書き込むインスタンスのデータに、その `Object*` のテクスチャの番号（`texture_slot`）を自動で付ける。番号からアトラス上の範囲とレイヤーを引く表（`texture_rects`）を `GpuContext` が持ち、UV の変換はユーザーのシェーダーで行う。**

```wgsl
// GPU 上のインスタンスのデータに Lonas が付け足す項目
texture_slot: u32,    // テクスチャの番号。0 は未指定（下記）
uniform_index: u32,   // uniform の配列の番号（§11 uniform）

// Lonas が用意する表（番号は BindingConfig::texture_rects）
struct TextureRect {
    rect: vec4<f32>,  // アトラス上の範囲（xy = 左上の UV、zw = 大きさの UV）。余白は含まない
    layer: u32,       // レイヤー番号
}
@group(G) @binding(B)
var<storage, read> texture_rects: array<TextureRect>;

// ユーザーのシェーダー
let t = texture_rects[inst.texture_slot];
let uv = t.rect.xy + in.uv * t.rect.zw;
let color = textureSample(atlas, atlas_sampler, uv, t.layer);
```

- `texture_slot` は `TextureId` の番号（§11 ResourceManager の ID）。`ResourceManager` が登録の時点で発行するため、`Object*::end` の時点で分かる
- アトラス上の配置は `end_frame` で決まる（§11 GPU との分担）。`GpuContext` は配置を決めたときに `texture_rects` の該当する番号を書き込む。そのため、インスタンスにアトラス上の範囲を直接入れることはできない
- `texture_rects` は番号の最大値まで大きくする（大きくするときは新しいバッファにコピー）。登録を解除した番号は、再利用されるまで古い値を残す（無効な `TextureId` を使う `Object*` は `end` で panic するため、読まれない、§11 登録解除）
- バッチング（§10）で別々のテクスチャの `Object*` が 1 回の DrawCall にまとまっても、インスタンスごとに区別できる
- 頂点には手を加えないため、テクスチャを変えても頂点（Static を含む）を書き直さなくてよい。書き直すのはインスタンス（常に Dynamic、§6）の `texture_slot` のみ
- インスタンス 1 つあたり 8 バイト増える（`texture_slot` と `uniform_index`）
- アトラス・サンプラー・`texture_rects` のバインディング番号は `BindingConfig` で公開する（§11 バインドグループ・バインディング番号）
- 却下案：インスタンスに範囲（`tex_rect` / `tex_layer`）を直接入れる（シェーダーでの参照は一段少ないが、配置が `end_frame` で決まるため、`end` の時点では範囲が分からない）
- 却下案：頂点の転送時に Lonas が UV を書き換える（レイヤー番号を頂点に追加する必要があり、テクスチャを変えると Static の頂点まで書き直すことになる）

#### texture() を呼ばなかった Object*

**決定：アトラスの先頭に「テクスチャ未指定」を表すピンク紫（`#FF00FF`）の 1 画素を予約する。`texture_rects` の番号 0 をその画素の範囲とし、`texture()` を呼ばなかった `Object*` のインスタンスは `texture_slot = 0` にする。**

- `TextureId` の番号は 1 から発行する

- テクスチャを使うシェーダーで `texture()` を指定し忘れた場合に、画面上ですぐ気付ける
- テクスチャを使わない `Object*` は、テクスチャをサンプルしないシェーダーを使う
- 予約した画素の周りにも通常のテクスチャと同じ余白を付ける（下記）

#### 詰め込み方

**決定：棚（shelf）方式で詰め込む。**

- アトラスを高さごとの棚に分け、各棚に左から順にテクスチャを置く。登録するテクスチャの高さに合う棚がなければ、新しい棚を下に作る
- 登録解除で空いた場所は同じ棚の中で再利用する。棚が空になったら棚ごと解放する
- アトラスの詰め直しは行わない（配置済みのテクスチャの位置は変わらない）
- 配置は `end_frame` で `GpuContext` が決める（§11 GPU との分担）
- 却下案：ギロチン方式（大きさがばらばらなときに詰め込み効率はよいが、解除時の結合が複雑）

#### にじみ対策：余白

**決定：各テクスチャの周りに `TextureAtlasConfig::padding` 画素の余白を付け、余白には端の画素を複製して埋める。**

- 線形補間で隣のテクスチャの色がにじむのを防ぐ（端で `ClampToEdge` と同じ見た目になる）
- `texture_rects` の範囲は余白を含まない

#### ミップマップ

**決定：ミップマップに対応し、段数は `TextureAtlasConfig::mip_level_count` で指定する。**`1` ならミップマップなし。

- 縮小したときに隣のテクスチャとにじまないよう、配置位置と余白を `2^(mip_level_count - 1)` 画素の倍数に切り上げる
- ミップマップは `end_frame` で、登録したテクスチャの範囲（余白を含む）だけ GPU で作る
  - **段階 7 の実装：**1 つ上の段を線形補間で読む描画のパスで作る。作る段の画素の中心は 1 つ上の段の 2×2 の画素の境目に当たるため、線形補間でちょうど 4 つの平均になる。sRGB のフォーマットのため、平均は線形の値で取られる
  - **段階 7 で決めたこと：**画像の転送とミップマップの作成は、テクスチャごとに自分の encoder ですぐに送る。同じフレームの後の登録でレイヤーを増やしたとき（新しいテクスチャへのコピー）に、作り終えた内容をコピーできるようにするため
- 段数が多いほど余白が増え、アトラスに詰め込める量が減る

#### 容量を超えた場合

**決定：レイヤーが足りなくなったら `max_layers` まで増やし、それを超えたら panic。**

- 最初は `initial_layers` 枚で作る。足りなくなったら、より多いレイヤーのテクスチャ配列を作り直し、既存の内容を GPU 上でコピーして差し替える（§6 の Dynamic の拡張と同じ考え方）。増やす量は `LonasConfig::dynamic_growth_factor` と同じく倍率で決める
- `max_layers` を超える場合は **panic**（Static 領域の容量超過と同じ扱い、§6）
- 1 つのテクスチャが余白を含めて 1 レイヤーに収まらない場合も **panic**

#### TextureAtlasConfig

```rust
pub struct TextureAtlasConfig {
    /// 1 レイヤーの幅・高さ（画素、正方形）
    pub size: u32,
    /// 最初に作るレイヤー数
    pub initial_layers: u32,
    /// レイヤー数の上限。超えたら panic
    pub max_layers: u32,
    /// 各テクスチャの周りの余白（画素）。mip_level_count に合わせて切り上げる
    pub padding: u32,
    /// ミップマップの段数。1 ならミップマップなし
    pub mip_level_count: u32,
    /// 拡大・縮小時の補間
    pub filter: FilterMode, // Nearest / Linear
}
```

- フォーマットは `Rgba8UnormSrgb` で固定（`register_texture_rgba` も sRGB の値として受け取る。§11 色）
- 値の検査（`GpuContext::new`）：
  - `size` / `max_layers` がアダプターの上限（`max_texture_dimension_2d` / `max_texture_array_layers`）を超える → `Err`（環境に起因、§11 エラーの方針）
  - `initial_layers` が 0、または `max_layers` より大きい → panic
  - `mip_level_count` が 0、または `size` から作れる段数（`floor(log2(size)) + 1`）より多い → panic

### uniform

**決定：uniform を渡す関数を用意する。**高頻度で呼ばれる可能性があるため、前の値を保持して比較し、**同値の場合は無視する**。

- 同値の場合に警告は出さない（高頻度で呼ぶことを想定した関数のため）
- uniform は用途別メモリ空間の uniform 領域に、`ShaderId` ごとの配列として置く（§6 uniform の領域）
- **決定：uniform は `Object*` ごとに渡す。渡せる型は `bytemuck` の `Pod` を満たす型。**

```rust
impl Object2D { // Object3D も同じ
    pub fn uniform<T: bytemuck::Pod>(&mut self, value: &T) -> &mut Self;
}
```

- 前の値とはバイト列として比較する（`Pod` のため可能）

#### バッチングとの両立

**決定：uniform は storage バッファの配列としてシェーダーに渡し、Lonas がインスタンスのデータに配列の番号（`uniform_index`）を付け足す。**

`Object*` ごとに uniform のオフセットを切り替える方式では、1 回の DrawCall（`multi_draw_indexed_indirect`）にまとめた `Object*` の間で uniform を切り替えられず、バッチング（§10）が崩れる。
そこで、テクスチャの `texture_slot`（§11 TextureAtlas）と同じく、インスタンスのデータに番号を付けて区別する。

```wgsl
// ユーザーのシェーダー（番号は BindingConfig::uniform）
struct MyUniform { ... }
@group(G) @binding(B)
var<storage, read> uniforms: array<MyUniform>;

let u = uniforms[inst.uniform_index];
```

- `uniform_index` は `Object*` に割り当てた配列の要素の番号で、`DrawManager*` が `end` の時点で割り当てる（§6 uniform の領域）。uniform の値が変わっても番号は変わらないため、インスタンスを書き直さなくてよい
- 却下案：uniform を持つ `Object*` はまとめない（`var<uniform>` のまま書けて、どの環境でも動くが、uniform を使う `Object*` の数だけ DrawCall が増える）
- 却下案：uniform を `ShaderId` ごとに 1 つだけ共有する（`Object*` ごとの値を持てない）
- 頂点シェーダーで storage バッファを読めない環境（OpenGL ES 3.0 など）は対象外（§8）

#### 型の検査

- 要素の間隔と大きさは、シェーダーを naga で解析して求める（§11 ResourceManager）。`BindingConfig::uniform` の位置にある変数の配列の要素の型を見る
- `T` の大きさ（`size_of::<T>()`）が WGSL の要素の型の大きさと違う場合は **panic**。Lonas は要素の間隔に合わせて詰め物を入れて書き込む
  - **段階 7 の実装：**判定は `end_frame` で行う（`Object*::end` はシェーダーの解析結果を持たないため）。コピーの制約のため、どちらの大きさも 4 バイトの倍数に切り上げて比べる
  - uniform を使わないシェーダーの uniform のバインディングには、すべて 0 の小さなバッファを置く
- シェーダーの `BindingConfig::uniform` の位置に変数がない、または `var<storage, read>` の配列でないのに `uniform()` を呼んだ場合は `end_frame` で **panic**（段階 7 の実装）
- シェーダーを変えた場合（`shader()`）、uniform の要素を新しいシェーダーの配列に割り当て直す（§2 end 時の更新条件）。大きさが違えば上記と同じく panic

### バインドグループ・バインディング番号

**決定：シェーダーに公開するバインドグループ・バインディング番号（カメラ / インスタンス / インスタンス番号のリスト / テクスチャ / uniform）は `LonasConfig` で指定する。**ユーザーが書くシェーダー（§10）と番号を合わせるため。

**決定：公開するものごとに、グループと番号の組（`BindingSlot`）で指定する。**

```rust
pub struct BindingSlot {
    pub group: u32,
    pub binding: u32,
}

pub struct BindingConfig {
    pub camera: BindingSlot,             // var<uniform> の Camera（§11 シェーダーとの約束事）
    pub instances: BindingSlot,
    pub instance_order: BindingSlot,     // インスタンス番号のリスト（§10）
    pub atlas_texture: BindingSlot,      // §11 TextureAtlas
    pub atlas_sampler: BindingSlot,
    pub texture_rects: BindingSlot,      // テクスチャの番号 → アトラス上の範囲の表（§11 TextureAtlas）
    pub uniform: BindingSlot,            // var<storage, read> の配列（§11 uniform）
    pub previous_camera: BindingSlot,    // TAA（§11 アンチエイリアス）。前のフレームの Camera
    pub previous_instances: BindingSlot, // TAA
}
```

- 更新の頻度ごとにグループを分ける（例：カメラはフレームごと、uniform は `DrawManager*` × シェーダーごと）といった最適化をユーザーが選べる
- 値の検査（`GpuContext::new`）：
  - 同じ `BindingSlot` が 2 つ以上ある → panic
  - `group` がアダプターの上限（`max_bind_groups`）以上 → `Err(LimitExceeded)`
- TAA 用の 2 つは、TAA を使わない場合もシェーダーと番号を合わせるために指定する（TAA 以外のサーフェスでは前のフレームと同じ値を置く）
- 却下案：グループは Lonas が 1 つに固定し、番号だけを指定する（更新の頻度ごとにグループを分けられない）

### シェーダーとの約束事

ユーザーが書くシェーダー（§10）と Lonas の間で、次の並び方を固定する。

#### WGSL の struct の提供

**決定：Lonas は下記の struct の WGSL の定義を文字列 `lonas::wgsl::STRUCTS` として公開する。ユーザーは自分の WGSL の先頭につなげて登録する。**

```rust
let src = format!("{}{}", lonas::wgsl::STRUCTS, MY_WGSL);
let shader = resource_manager.register_shader(&src)?;
```

- バインディングの宣言（`@group` / `@binding`）は番号が `BindingConfig` 次第のため、ユーザーが書く
- **決定：`end_frame` でシェーダーを作るとき、naga で各バインディングの型の大きさを検査し、下記と合わなければ panic する。**`BindingConfig` の位置にある変数の型（`instances` の配列の要素、`camera` の struct など）の大きさを見る。書き間違いを、表示が崩れる前に見つけられる
- 却下案：DESIGN.md に並び方を書くだけにする（ユーザーが struct を書き間違えても表示が崩れるだけで、原因が分かりにくい）
- 却下案：`register_shader` が自動で先頭につなげる（ユーザーの WGSL だけを読んでも定義の場所が分からず、naga のエラーの行番号もずれる。ユーザーが同じ名前を使うと衝突する）

#### インスタンス

**決定：GPU 上のインスタンスの並び方は次のとおり。**WGSL の storage バッファの決まり（`vec4` / `mat4x4` は 16 バイト、`vec2` は 8 バイトの境界）に合わせ、大きいものから並べて詰め物を最小にする。

```wgsl
struct Instance2D {          // 48 バイト
    color: vec4<f32>,        //  0 線形の色（§11 色）
    position: vec2<f32>,     // 16
    scale: vec2<f32>,        // 24
    rotation: f32,           // 32 ラジアン
    visibility: u32,         // 36 1 = Visible、0 = Hidden
    texture_slot: u32,       // 40 Lonas が付け足す（§11 UV の変換）
    uniform_index: u32,      // 44 Lonas が付け足す（§11 uniform）
}

struct Instance3D {          // 96 バイト
    transform: mat4x4<f32>,  //  0
    color: vec4<f32>,        // 64 線形の色
    visibility: u32,         // 80
    texture_slot: u32,       // 84
    uniform_index: u32,      // 88
    _pad: u32,               // 92
}

struct TextureRect {         // 32 バイト（§11 UV の変換）
    rect: vec4<f32>,         //  0 xy = 左上の UV、zw = 大きさの UV
    layer: u32,              // 16
}
```

- Rust 側の `Instance2D` / `Instance3D`（§11 Instance）とは別の並び方。Lonas が転送するときに詰め直す（色の線形への変換も同時に行う）
- 前のフレームのインスタンス（`previous_instances`、TAA）も同じ並び方
- インスタンス番号のリスト（`instance_order`）は `array<u32>`

#### カメラ

**決定：カメラは 2D / 3D 共通の struct とし、`var<uniform>` で渡す。**

```wgsl
struct Camera {                              // 224 バイト
    view_projection: mat4x4<f32>,            //   0 ジッター込み。画面上の位置に使う
    view_projection_no_jitter: mat4x4<f32>,  //  64 動きのベクトル用（TAA）
    view: mat4x4<f32>,                       // 128 光の計算など
    position: vec4<f32>,                     // 192 xyz。w = 0
    viewport_size: vec2<f32>,                // 208 ピクセル
    jitter: vec2<f32>,                       // 216 ピクセル。TAA 以外は 0
}
```

- カメラは 1 回の DrawCall の中で変わらない（バッチングのキーに `CameraId` を含む、§10）ため、uniform で足りる
- `DrawManager*` のすべてのカメラを 1 本の uniform バッファに並べ、動的オフセットで切り替える。1 つあたりの間隔は `min_uniform_buffer_offset_alignment`（多くの環境で 256 バイト）に切り上げる
- 2D では `view` は 2D のカメラの変換、`position` は `(x, y, 0, 0)`
- 前のフレームのカメラ（`previous_camera`、TAA）も同じ struct。動きのベクトルは、現在と前のフレームの `view_projection_no_jitter` で求める
- 却下案：`view_projection` だけにする（光の計算にカメラの位置が要る場合や、TAA の動きのベクトルを正確に出す場合に、ユーザーが uniform で別に渡すことになる）

#### 頂点

**決定：頂点の要素の `@location` は要素ごとに固定する。**`VertexLayout`（§10）は要素を前から順に足していく形のため、どの `VertexLayout` でも連番になる。

| 要素 | `@location` | 形式 | 含まれる `VertexLayout` |
|---|---|---|---|
| 位置 | 0 | `Float32x3`（`vec3<f32>`） | すべて |
| 色 | 1 | `Float32x4`（`vec4<f32>`）。線形の色（§11 色） | `PositionColor` 以降 |
| UV | 2 | `Float32x2`（`vec2<f32>`） | `PositionColorUv` 以降 |
| 法線 | 3 | `Float32x3`（`vec3<f32>`） | `PositionColorUvNormal` |

- 2D の頂点（`vertex(x, y)`、§11 2D の頂点の入力）も位置は `vec3<f32>` で、z は 0
- 色は精度のため `f32` のまま送る（8 ビットに詰めると、線形の値では暗い色の階調が失われる）

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

### 同値の設定の扱いと redundant_call_warning

**決定：`shader()` 以外の設定（`texture` / `blend` / `topology` / `camera` / `layer` / `memory_update_type`）も、毎フレーム同じ値で呼ばれた場合は `shader()` と同じく無視して警告する。**

| 対象 | 同値の判定と無視 | 警告 |
|---|---|---|
| `shader` / `texture` / `blend` / `topology` / `camera` / `layer` / `memory_update_type` | 常に行う | `redundant_call_warning = true` のときのみ。毎フレームのログを避けるため、`Object*` ごと・設定の関数ごとに 1 回だけ（段階 5 で決定） |
| 頂点・インデックスのその場比較（§4） | 常に行う | 出さない（毎フレーム呼ぶ関数のため） |
| uniform | 常に行う | 出さない（高頻度で呼ぶことを想定した関数のため） |

### エラーの方針

**決定：環境に起因するエラーは `Result` で返す。**

- 例：アダプター / デバイスが取得できない、サーフェスが作れない、デバイスが失われた
- ユーザーのコードの誤用（§9 の表）は従来どおり panic
- **決定：エラーの型は列挙型 `LonasError` とする。**環境に起因するエラーと、登録時の入力（WGSL・画像）のエラーを返す関数はすべて `Result<T, LonasError>` を返す

```rust
#[non_exhaustive]
pub enum LonasError {
    // 環境
    AdapterNotFound(wgpu::RequestAdapterError),
    UnsupportedAdapter { missing: &'static str }, // 必須の機能がない（§8。VERTEX_STORAGE / COMPUTE_SHADERS / INDIRECT_EXECUTION / INDIRECT_FIRST_INSTANCE）
    RequestDevice(wgpu::RequestDeviceError),
    CreateSurface(wgpu::CreateSurfaceError),
    DeviceLost { reason: String },
    UnsupportedSurfaceFormat,          // sRGB のフォーマットに対応していない（§11 サーフェスのフォーマット）
    UnsupportedSampleCount { requested: u32, supported: Vec<u32> },
    UnsupportedPresentMode(PresentMode),  // §8
    LimitExceeded { limit: &'static str, requested: u64, max: u64 }, // アトラスの大きさ・レイヤー数など
    // 登録時の入力（§11 ResourceManager）
    InvalidShader { message: String },  // naga の検査結果
    ImageDecode(image::ImageError),     // image の feature が有効な場合のみ
}
```

- `std::error::Error` と `Display` を実装する
- `#[non_exhaustive]` にし、後から種類を足してもユーザーのコードが壊れないようにする
- **`GpuContext::new` の非同期処理（wgpu の `request_adapter` / `request_device`）は内部で待つ**（gueiz と同じく `pollster`）。`GpuContext::new` 自体は同期関数

### 座標系

**決定：2D の原点と 3D の座標系は `LonasConfig` で指定する。**

```rust
pub struct CoordinateSystemConfig {
    pub origin_2d: Origin2D,       // 2D の原点
    pub handedness_3d: Handedness, // 3D の手系
    pub up_axis_3d: UpAxis,        // 3D の上方向
}

pub enum Origin2D {
    TopLeft,    // 左上が原点、y は下向き
    BottomLeft, // 左下が原点、y は上向き
    Center,     // 中央が原点、y は上向き
}

pub enum Handedness {
    RightHanded,
    LeftHanded,
}

pub enum UpAxis {
    Y,
    Z,
}
```

- **決定：2D の単位はピクセル。**`Camera2D::zoom = 1.0` のとき、1 単位がサーフェスの 1 ピクセルになる
- 3D の手系と上方向は `Camera3D` のビュー・プロジェクション行列と `look_at` の計算に使う（§11 Camera）。深度は手系によらず Reversed-Z（§11 深度バッファ）
- 背面カリングの表裏（どちら回りを表とするか）は手系に合わせて Lonas が決める（§11 背面カリング）
- **決定：`UpAxis::Z` の場合、Lonas の内部では Y が上の座標に回してから扱う。**回転は (x, y, z) → (x, z, −y)（X 軸まわりに −90 度）。右手系では +Y が前、左手系では −Y が前になる
- 却下案：2D を正規化座標（-1〜1）にする（UI をピクセルで置きたい場合にユーザーが変換する必要がある）

### 2D の視界外の省略

**決定：2D で視界の外を省く処理を行うかどうかは `LonasConfig` で指定する。**

- 判定には `Object2D` の境界円を使う（§10 視界の判定に使う大きさ）。処理は Compute Shader で行う（§10 2D の Compute のパス）

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
- **決定（段階 6）：画面上で反時計回りに見える面を表にする。**3D の右手系・2D の原点が `BottomLeft` / `Center` では反時計回り、3D の左手系・2D の原点が `TopLeft`（y が下向き）では時計回りの頂点の順を表にする（2D の `Polygon` は xy 平面で反時計回りに作るため、`TopLeft` では表になる）

### 色

**決定：`Color` は sRGB の値として受け取る。色の計算・ブレンドは常に線形で行う。**

```rust
Color::new(r, g, b, a)            // sRGB の 0.0〜1.0
Color::rgb(r, g, b)               // a = 1.0
Color::from_rgba8(255, 128, 0, 255)
Color::WHITE   Color::BLACK   Color::TRANSPARENT
```

- 頂点の色（`vertex_color*`）・インスタンスの `Color`・`clear_color` は、転送するときに線形に変換する
- テクスチャはアトラスが `Rgba8UnormSrgb` のため、サンプルした時点で線形になる（§11 TextureAtlas）
- サーフェスは常に sRGB のフォーマットのため（§11 サーフェスのフォーマット）、書き込むときに GPU が sRGB に戻す
- これにより、頂点の色・インスタンスの色・テクスチャの色の扱いがすべて揃う

### インデックスの型

**決定：インデックスは `u32` で固定する。**

### サーフェスのフォーマット

**決定：サーフェスは常に sRGB のフォーマット（`Bgra8UnormSrgb` / `Rgba8UnormSrgb` など）にする。設定の項目は持たない。**具体的なフォーマットは、サーフェスの対応フォーマットのうち sRGB のものから Lonas が選ぶ。

- 色の計算を常に線形で行い、テクスチャ（`Rgba8UnormSrgb`）・頂点の色・インスタンスの色の扱いを揃えるため（§11 色）
- sRGB のフォーマットに対応していなければ `GpuContext::new` / `add_surface` が `Err(UnsupportedSurfaceFormat)` を返す
- プラットフォームごとの差（Bgra / Rgba）をユーザーが意識しなくて済む。wgpu の型が公開 API に出ない
- HDR（`Rgba16Float` など）が必要になったら、そのときに `LonasConfig` に設定の項目を追加する
- 却下案：`SurfaceFormat::Srgb` / `Linear` を選べるようにする（`Linear` ではアトラスの `Rgba8UnormSrgb` と色の扱いが揃わない。揃えるには `Linear` のときだけアトラスのフォーマットを変え、ガンマ空間で描くことになり、ブレンドや光の計算が物理的に正しくなくなる）
- 却下案：`wgpu::TextureFormat` をそのまま受け取る（環境ごとに対応しているフォーマットが違い、ユーザーが環境を意識する必要がある）

### 環境の判定

**決定：必須の機能が使えるかどうかを、`GpuContext::new` でアダプターから判定する。**機能の有無で描き方を切り替えることはしない。

- 頂点シェーダーでの storage バッファ（`VERTEX_STORAGE`）・Compute Shader（`COMPUTE_SHADERS`）・間接描画（`INDIRECT_EXECUTION`）・`INDIRECT_FIRST_INSTANCE` は必須（`MULTI_DRAW_INDIRECT` は wgpu 30 では `INDIRECT_EXECUTION` に含まれる、§8）。使えなければ `GpuContext::new` が `Err(UnsupportedAdapter)`（§8）

### 複数ウィンドウ

**決定：`GpuContext` 1 つで複数のサーフェス（ウィンドウ）に対応する。**

**決定：フレームは全体で 1 つとし、`begin_frame` は全体で 1 回、`end_frame` にサーフェスごとの描画をまとめて渡す。**

```rust
let (mut gpu, main_surface) = GpuContext::new(window, width, height, config)?;
let sub_surface: SurfaceId = gpu.add_surface(other_window)?;

gpu.begin_frame();
// 各 Object* の end ...
gpu.end_frame(&mut resource_manager, &mut [
    SurfaceDraw { surface: main_surface, clear_color: Some(color), draw_managers: &mut [&mut dm_3d, &mut dm_2d] },
    SurfaceDraw { surface: sub_surface,  clear_color: None,        draw_managers: &mut [&mut dm_ui] },
]);

gpu.resize(main_surface, width, height);
gpu.set_anti_aliasing(main_surface, AntiAliasing::Msaa { sample_count: 4 })?;
```

- データの転送・GC のフレームの数え方・`queue.submit` は **1 フレームに 1 回**。ウィンドウの数が増えても変わらない
- 描画先の取得（`get_current_texture`）と `present` は、`end_frame` に渡されたサーフェスについてのみ行う。そのフレームで描かないサーフェスは触らない
- 画面の色のクリアは `SurfaceDraw::clear_color` でサーフェスごとに指定する（`None` ならクリアしない）
- 却下案：サーフェスごとに `begin_frame` / `end_frame` を呼ぶ形（ウィンドウの数だけ転送・GC の数え方・`submit` が増える）

- **決定：同じサーフェスを `end_frame` に 2 回以上渡した場合は panic。**1 つのサーフェスに複数の `DrawManager*` を描く場合は、1 つの `SurfaceDraw` の `draw_managers` に並べる
- 同じ `DrawManager*` を複数のサーフェスに渡すことは、`&mut` を同時に 2 つ作れないため **コンパイルエラー** になる
- ヒープ（§6）・`ResourceManager`・パイプラインはサーフェス間で共有する
- 描画先の取得・`present`・深度バッファはサーフェスごと

### スレッド

**決定：`Object*` / `DrawManager*` を別スレッドで扱えるようにする（`Send`）。**

- `Object*::end` は `&mut DrawManager*` を取るため、別々の `DrawManager*` であれば複数のスレッドから同時に `end` できる
- フレーム番号は `Arc<AtomicU64>` で共有しており（§9）、スレッドをまたいでも判定できる

### アンチエイリアス

**決定：None / MSAA / FXAA / TAA のすべてを選べるようにする。**各サーフェスの最初の値は `LonasConfig::initial_anti_aliasing`、以降は `set_anti_aliasing` でサーフェスごとに変更する。

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
  - **決定（段階 8）：動きのベクトルは、フラグメントシェーダーの `@location(1)` に「現在の UV − 前のフレームの UV」を書く。**現在と前のフレームの位置は、ジッターなしの行列（`view_projection_no_jitter`）で求める。`examples/antialiasing.rs` が書き方の例
    - `@location(1)` を書かないシェーダーの部分は 0（動いていない）として扱う。Lonas はシェーダーの出力を naga で調べ、書かないシェーダーのパイプラインでは動きのベクトルの描画先に書き込まない
- 動きの速いものに残像のようなぼやけが出やすいため、履歴をどの程度使うかの調整が必要
- **決定：TAA の細かい値は `LonasConfig` で指定する。**

```rust
pub struct TaaConfig {
    /// 履歴を混ぜる割合の範囲（信頼度が最低のとき〜最高のとき）
    pub history_blend_min: f32,
    pub history_blend_max: f32,
    /// この深度差を超えたら履歴の信頼度を下げる
    pub depth_rejection_threshold: f32,
    /// 分散クリップの範囲の広さ（周囲の色の標準偏差の倍率。下記）
    pub color_rejection_threshold: f32,
    /// 動きのベクトルを書き込む画像のフォーマット
    pub motion_vector_format: MotionVectorFormat,
}

pub enum MotionVectorFormat {
    Rg16Float, // 半精度。多くの場合これで足りる
    Rg32Float, // 単精度。大きな画面で精度が足りない場合
}
```

- **決定：動きのベクトルのフォーマットは Lonas の列挙型 `MotionVectorFormat` で指定する。**wgpu の型を公開 API に出さない（§11 サーフェスのフォーマット）
- **決定：カメラのカットは `gpu.invalidate_history(surface_id)` で知らせる。**そのサーフェスの TAA の履歴を無効化する

```rust
impl GpuContext {
    pub fn invalidate_history(&mut self, surface: SurfaceId);
}
```

  - 履歴はサーフェスごとに持ち、1 つのサーフェスには複数の `DrawManager*` やカメラが描かれるため、履歴を持つ単位で無効化する
  - 次の `end_frame` で反映する。TAA 以外の方式のサーフェスでは何もしない
  - 却下案：カメラ側の `camera.cut()`（そのカメラが描いたサーフェスをたどる必要があり、同じサーフェスの他のカメラの履歴まで消える）

#### TAA の処理の流れ

**決定：次の流れで行う。**

1. **取得**：現在のフレームの色・深度・動きのベクトル（モーションベクトル）を取得する
2. **再投影**：動きのベクトルを使い、前のフレームの履歴画像から、各ピクセルが前のフレームで対応する位置の色を読み出す
3. **信頼度の評価**：次の判定から、読み出した履歴をどれだけ信頼できるかを求める
   - 画面外判定：再投影した位置が前のフレームの画面の外なら信頼しない
   - 深度差：前のフレームの深度と現在の深度が大きく違えば、別の物が見えていたとみなし信頼度を下げる
   - 色の不一致：履歴の色が現在のピクセルの周囲の色から大きく外れていれば信頼度を下げる
     - **決定：YCoCg の色空間での分散クリップで判定する。**周囲 3×3 ピクセルの色を YCoCg（輝度と色差）に変換し、平均 `μ` と標準偏差 `σ` から範囲 `μ ± color_rejection_threshold × σ` を作る。履歴の色が範囲の外なら、平均に向かう線分上で範囲の境界まで切り詰め、切り詰めた量に応じて信頼度を下げる
     - 却下案：周囲の色の最小・最大の範囲に収める（外れ値 1 つで範囲が広がり、残像が出やすい）
4. **合成**：信頼度に応じて履歴のブレンド率を調整し、現在の画像と合成する（信頼度が高いほど履歴を多く使い、低いほど現在の画像を多く使う）
5. **保存**：合成結果を次のフレーム用の履歴画像として保存する。深度差の判定のため、現在の深度も次のフレーム用に保存する
6. **無効化**：カメラのカット（場面の切り替え）や解像度の変更などでは履歴を無効化し、現在の画像から履歴を作り直す

- 解像度の変更（`resize`）では自動で無効化する
- 履歴の画像・前のフレームの深度はサーフェスごとに持つ

#### 段階 8 の実装で決めた細部

| 方式 | DrawManager* が描く先 | 後処理 |
|---|---|---|
| None | サーフェス | なし |
| MSAA | サンプル数の多い描画先（各パスでサーフェスへ resolve） | なし |
| FXAA | 中間の画像 | FXAA（FXAA 3.11 の簡易版）で中間の画像からサーフェスへ |
| TAA | 中間の画像と動きのベクトル | TAA の合成でサーフェスと次の履歴へ |

- 描画先（深度バッファを含む）はサーフェスごとに持ち、大きさ・方式・フォーマットが変わったときに作り直す。作り直した直後は TAA の履歴を使わない
- TAA のずらしは Halton 列（2, 3）の 8 個を繰り返す（-0.5〜0.5 ピクセル）。`view_projection` だけに加え、2D のカメラにも加える
- 履歴は `Rgba16Float` で 2 枚を交互に使う。深度は合成の後に、次のフレーム用へコピーする
- 前のフレームのカメラは、前のフレームに送った値を CPU 側に残しておき、そのまま送る
- 前のフレームのインスタンスは、TAA を使うサーフェスがあるフレームだけ、転送の前にインスタンスのヒープ全体を GPU 上でコピーして作る。そのフレームで確保し直したインスタンスは位置が変わるため、そのフレームだけ前の値が合わない
- 深度差による信頼度の低下は ×0.25

#### 実行中の切り替え

**決定：アンチエイリアスの方式を実行中に変更できるようにする。**

```rust
// 決定：サーフェスごとに 1 つ設定する
gpu.set_anti_aliasing(surface_id, AntiAliasing::Taa)?;
```

- 変更は次の `begin_frame` で反映する（フレームの途中では切り替えない）
- 反映時に作り直すもの：描画先（MSAA 用の描画先・後処理用の画像・TAA の履歴）、深度バッファ（サンプル数が変わるため）、パイプライン（MSAA のサンプル数はパイプラインのキーに含まれるため、新しい構成のパイプラインを作る。古いものは使われなくなれば捨てられる、§10 パイプラインのキャッシュ）
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

- [ ] `UpAxis`（3D の上方向）の使い道（段階 6 で見つかった）。カメラの向きは `rotation` と、`look_at` に渡す `up` で完全に決まるため、今の実装では `UpAxis` は描画に影響しない。`look_at` の `up` を省いたときの既定値にする、項目を削除する、などを決める（§11 座標系）
