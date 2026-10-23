# Lonas

wgpu の上に作った、2D / 3D の描画ライブラリです。

`begin` / `end` で毎フレーム頂点を渡す、即時モードに近い API にしています。ただし内部では毎フレームすべてを作り直さず、前のフレームと値を比べて、変わったところだけを GPU に送ります。

## 特徴

- **差分だけを転送する。** 頂点は関数の引数として受け取り、内部の写しとその場で比べながら書き込みます。変わった範囲だけを GPU に送ります（DESIGN.md §1・§4）
- **GPU メモリを自前で管理する。** 用途ごとの大きなバッファをヒープとして使います。Static 領域はスタックとコンパクションで、Dynamic 領域は TLSF で割り当て、使われなくなった領域はフレーム単位の GC で回収します（§6）
- **Compute Shader で描画を準備する。** カリング・並べ替え・間接描画の引数の書き込みを GPU で行い、同じ設定の Object をまとめて `multi_draw_indexed_indirect` で描きます（§10）
- **2D と 3D を別の型で扱う。** `Object2D` / `Object3D`、`DrawManager2D` / `DrawManager3D` に分けています。2D は `layer` の順に、3D は不透明なものの後に半透明なものを奥から手前の順に描きます
- **アンチエイリアス。** MSAA・FXAA・TAA を実行中に切り替えられます（§11）
- **複数ウィンドウ。** 1 つの `GpuContext` で複数のサーフェスに描けます（§11）
- **誤用は panic、環境に起因するものは `Result`。** フレームの外での呼び出しや、削除済みの ID の使用などは panic で知らせます（§9・§11）

## 使い方の概略

ウィンドウとイベントループは使う側（winit など）が持ちます。

```rust
use lonas::lonas::*;

let (mut gpu, surface) = GpuContext::new(window, width, height, config)?;
let mut resources = ResourceManager::new();
let shader = resources.register_shader(&format!("{}{}", wgsl::STRUCTS, MY_WGSL))?;
let mut draw_manager = DrawManager2D::new(&mut gpu, camera, VertexLayout::PositionColor);
let mut object = Object2D::new(&gpu);
object.shader(shader);
object.add_instance(Instance2D::default());

// 毎フレーム
gpu.begin_frame();

object.begin();
object.vertex(0.0, 0.0);
object.vertex(100.0, 0.0);
object.vertex(0.0, 100.0);
object.end(&mut draw_manager);

gpu.end_frame(&mut resources, &mut [
    SurfaceDraw { surface, clear_color: Some(Color::BLACK), draw_managers: &mut [&mut draw_manager] },
])?;
```

`end_frame` は、ドライバーのリセットなどで GPU のデバイスが失われると `Err(LonasError::DeviceLost)` を返します。Lonas の中では復旧しないので、`GpuContext` などをすべて作り直してください（DESIGN.md §12）。

`LonasConfig` には既定値がなく、すべての項目を指定する必要があります（§3）。動く全体の例は [examples/test1.rs](examples/test1.rs) を見てください。

## 動作環境

- Rust 2024 edition
- wgpu 30 が対応するバックエンド（Vulkan / DirectX 12 / Metal / OpenGL）
- アダプターに次の機能が必要です。足りない場合、`GpuContext::new` は `LonasError::UnsupportedAdapter` を返します
  - `VERTEX_STORAGE`（頂点シェーダーでの storage バッファ）
  - `COMPUTE_SHADERS`
  - `INDIRECT_EXECUTION`（間接描画）
  - `INDIRECT_FIRST_INSTANCE`

### feature

| feature | 既定 | 内容 |
|---|---|---|
| `image` | 有効 | PNG / JPEG の読み込み（`ResourceManager::register_texture`）。無効にすると `register_texture_rgba` だけになります |

## example

```bash
cargo run --example test1
```

| example | 内容 |
|---|---|
| `clear` | ウィンドウを、時間とともに変わる色で塗りつぶす |
| `triangle` | 3D の回る三角形の手前に、2D の三角形を 100 個（インスタンス）並べて動かす |
| `antialiasing` | 回る細い三角形を、アンチエイリアスの方式を切り替えて描く（1：なし、2：MSAA 4x、3：FXAA、4：TAA） |
| `test1` | DESIGN.md の API をひととおり使うサンプル（T：TAA への切り替えとカメラのカット、V：垂直同期の切り替え） |

## テスト

```bash
cargo test
```

GPU を使うテストは、ウィンドウなしの描画先に描いて画素を読み戻して確かめます。アダプターが見つからない環境では、自動で飛ばします。環境変数 `LONAS_SKIP_GPU_TESTS` を設定すると、アダプターがあっても GPU を使うテストを飛ばします（CI ではこれを使っています）。

## ドキュメント

- [DESIGN.md](DESIGN.md)：設計の決定とその理由。文中やコードのコメントの「§」はこの文書の章を指します
- [PLAN.md](PLAN.md)：実装の順番と、各段階を終えたと判断する条件
