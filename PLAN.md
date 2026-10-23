# Lonas 実装計画

DESIGN.md の設計を実装する順番と、各段階で終わったと判断する条件をまとめる。
依存の少ないものから作り、各段階の終わりで動作を確かめられるようにする。§ は DESIGN.md の章を指す。

---

## 全体の方針

- **段階 1〜3 は GPU を使わない。**単体テストだけで固め、段階 4 以降で GPU の問題と切り分けやすくする
- **各段階の終わりで `cargo test` がすべて通る状態にする。**途中の段階では、まだ作っていない部分を `todo!()` にしない（呼ばれない形にする）
- **今の `src/lonas.rs` は仮のもの。**段階 4・5 で下記の構成に置き換え、今あるテスト（フレーム境界・誤用の panic）は新しい構成に移す
- **`examples/test1.rs` は `api-sample` feature で止めている。**段階 6 でコンパイルできるようにし、feature の制限を外す
- **GPU を使うテストは、ウィンドウなしで作れる内部向けの `GpuContext`（サーフェスを持たない）で行う。**`GpuContext::new` はウィンドウを必須とするため、テスト用の作り方を `pub(crate)` で用意する。アダプターがない環境（CI など）では飛ばす

### モジュールの構成

```
src/
  lib.rs            公開する型をまとめて出す。`pub mod lonas` は公開 API の入り口として残す（test1.rs が使う）
  config.rs         LonasConfig とその中の型（§3）
  error.rs          LonasError（§11 エラーの方針）
  math/             Vector2f / Vector3f / Quaternion / Matrix4f（§11 数学型）
  color.rs          Color（§11 色）
  vertex.rs         Vertex と VertexLayout ごとの詰め方（§1・§10）
  memory/           TLSF・Static のスタック・ヒープの対応表・コンパクション・GC の記録（§6）
  resource/         ResourceManager・WGSL の検査と解析・画像の読み込み（§11 ResourceManager）
  gpu/              GpuContext・サーフェス・フレーム境界・必須の機能の判定（§8・§9）
  object/           ObjectCore・Object2D / Object3D・InstanceSlots・三角形化（§4・§7・§10）
  draw/             DrawManagerCore・DrawManager2D / 3D・カメラ・staging バッファ・HeapRequest（§5・§6）
  render/           パイプラインのキャッシュ・Compute のパス・描画（§10）
  texture/          TextureAtlas（棚方式・余白・ミップマップ）（§11 TextureAtlas）
  aa/               MSAA・FXAA・TAA（§11 アンチエイリアス）
  wgsl.rs           STRUCTS と Lonas 内部のシェーダー（§11 シェーダーとの約束事）
```

### 追加する依存

| 段階 | クレート | 用途 |
|---|---|---|
| 1 | `bytemuck` | 頂点・インスタンスのバイト列への変換、uniform の `Pod`（§8） |
| 1 | `log` | 警告（§3） |
| 3 | `naga`（wgsl-in） | WGSL の検査と解析（§11 ResourceManager） |
| 3 | `image`（feature で外せる） | PNG / JPEG の読み込み（§11 画像の形式） |
| 4 | `wgpu` | 描画（§8。バージョンは段階 4 の最初に決める） |
| 4 | `pollster` | `GpuContext::new` の中で非同期処理を待つ（§11 エラーの方針） |
| 4 | `raw-window-handle` | ウィンドウの受け取り（§8） |
| 6 | `winit`（dev-dependencies のみ） | 画面で確かめる example |

---

## 段階 1：数学型・Color・Vertex

**目的**：他のすべての段階が使う値の型を作る。

- [x] `Vector2f` / `Vector3f`：`new`・`ZERO`・`ONE`・配列からの `From`・基本の演算子
- [x] `Quaternion`：`IDENTITY`・`from_axis_angle`・掛け算・ベクトルの回転
- [x] `Matrix4f`：`IDENTITY`・`from_translation` / `from_rotation` / `from_scale` / `from_trs`・掛け算・逆行列
- [x] 投影行列：透視投影・正射影。Reversed-Z（§11 深度バッファ）、手系と上方向（§11 座標系）に対応する
- [x] `look_at` の計算（§11 Camera）
- [x] `Color`：`new`・`rgb`・`from_rgba8`・定数・sRGB → 線形の変換（§11 色）
- [x] `Vertex`：今の `src/vertex.rs` を移す。`VertexLayout` ごとにバイト列へ詰める関数（§10 頂点の並び方）
- [x] `Instance2D` / `Instance3D` を GPU の並び方（48 / 96 バイト）へ詰める関数（§11 シェーダーとの約束事）

**終わりの条件**
- 行列・クォータニオンの計算が既知の値と一致するテスト（単位行列、90 度回転、逆行列 × 元 = 単位行列など）
- Reversed-Z の投影で、`near` が深度 1.0、`far` が 0.0 になるテスト
- 詰めたバイト列の大きさとオフセットが DESIGN.md の表と一致するテスト

---

## 段階 2：TLSF とヒープの CPU 側

**目的**：GPU のバッファを使わずに、領域の割り当てと移動の計算だけを完成させる。§6 で最も細かい部分。

- [x] `TlsfAllocator`：サイズクラス・ビットマップ・確保・解放・隣接の結合（§6 Dynamic の割り当て：TLSF）
- [x] Dynamic 領域の拡張：末尾の空きブロックを伸ばす / 足す。拡張後の容量の式（§6 Dynamic 領域の拡張）
- [x] Static のスタック：手前から積む・容量超過で panic・穴の記録（§6 Static / Dynamic の区別）
- [x] コンパクションの計画：しきい値の判定と、「どこからどこへ何バイト移すか」の一覧を作る（§6 Static 領域のコンパクション）。GPU のコピーは段階 5 で、この一覧から行う
- [x] ハンドル（`GpuAllocation`）と実際のオフセットの対応表。コンパクションしてもハンドルが変わらないこと
- [x] GC の記録：ハンドルごとの最後の `end` のフレーム、期限切れの回収、`ManagerId` ごとの回収済みの一覧（§6 解放・回収の通知）
- [x] Static ↔ Dynamic の移動（§6 確保後の memory_update_type の変更）

**終わりの条件**
- 確保と解放を乱数で大量に繰り返し、毎回「ブロックが重ならない・隙間なく並ぶ・空きリストとビットマップが一致する」を検査するテスト
- 解放したブロックが前後の空きと結合されるテスト
- 拡張後に既存のブロックのオフセットが変わらないテスト
- コンパクションの一覧どおりに移すと、穴がなくなり、すべてのハンドルが正しいオフセットを指すテスト
- GC が `gc_unused_frames` ちょうどで回収するテスト

---

## 段階 3：ResourceManager

**目的**：GPU に触れない登録・解除・検査を完成させる（§11 ResourceManager）。

- [x] `ShaderId` / `TextureId`：発行元の識別子・番号・世代。`TextureId` の番号は 1 から（§11 ID・texture() を呼ばなかった Object*）
- [x] `register_shader`：naga で WGSL を検査し、失敗なら `Err(InvalidShader)`
- [x] シェーダーの解析結果を保持する：`BindingConfig` の位置にある変数の型の大きさを、段階 6 で検査できる形で取り出せるようにする（§11 シェーダーとの約束事・uniform の型の検査）
- [x] `wgsl::STRUCTS`：Instance2D / Instance3D / Camera / TextureRect の定義（§11 シェーダーとの約束事）
- [x] `register_texture`（`image` feature）・`register_texture_rgba`（大きさの不一致で panic）
- [x] `unregister_*`：ID を即座に無効化し、解除の要求を積む
- [x] 未反映の登録・解除を取り出す関数（段階 4 以降の `end_frame` が使う）

**終わりの条件**
- 誤った WGSL が `Err` になり、正しい WGSL と `STRUCTS` をつないだものが通るテスト
- `STRUCTS` の各 struct の大きさを naga で求め、48 / 96 / 224 / 32 バイトになるテスト（段階 1 の詰め方と食い違わないことの確認）
- 解除後に同じ番号が再利用されても、古い ID と世代で区別できるテスト
- 別の `ResourceManager` の ID を判別できるテスト

---

## 段階 4：GpuContext とフレーム境界

**目的**：wgpu を初期化し、何も描かないフレームを正しく回す（§8・§9）。

- [x] **最初に wgpu のバージョンを決める。**（段階 3 で naga のバージョンを揃えるために 30 系に決定。gueiz と同じ）そのバージョンでの `MULTI_DRAW_INDIRECT` と `INDIRECT_FIRST_INSTANCE` の表し方（`Features` / `DownlevelFlags`）を確認する（§8）
- [ ] `LonasConfig` の値の検査（§3・§11 の各「値の検査」）。不正なら panic、アダプターの上限を超えるなら `Err`
- [ ] 必須の機能の判定：`VERTEX_STORAGE`・`COMPUTE_SHADERS`・`INDIRECT_EXECUTION`・`INDIRECT_FIRST_INSTANCE`・`MULTI_DRAW_INDIRECT`（§11 環境の判定）
- [ ] `GpuContext::new` / `add_surface`：sRGB のフォーマットを選ぶ（§11 サーフェスのフォーマット）、Present mode を選ぶ（§8）
- [ ] `resize`・`surface_size`・`set_present_mode`・`set_anti_aliasing`（反映は次の `begin_frame`）
- [ ] フレーム番号の `Arc<AtomicU64>`（フレーム中かどうかを含む）と、`begin_frame` / `end_frame` の誤用の panic（§9）
- [ ] `end_frame` の骨組み：§9 の 8 段階の順に関数を並べ、この段階では「描画先の取得 → クリア → present」だけを行う
- [ ] 幅か高さが 0、または描画先を取得できない場合に、そのサーフェスだけ飛ばす（§9 リサイズとフレームスキップ）
- [ ] 同じサーフェスを 2 回渡したら panic（§11 複数ウィンドウ）

**終わりの条件**
- ウィンドウなしの `GpuContext` で `begin_frame` / `end_frame` を繰り返せるテスト。今の `src/lonas.rs` のフレーム境界のテストをここへ移す
- 必須の機能が欠けたアダプターを想定し、`Err(UnsupportedAdapter)` を返す経路のテスト（判定の関数を単体で）
- `clear_color` で塗りつぶしたウィンドウが出る example（`winit` を使う。段階 6 の example の土台）

---

## 段階 5：Object と DrawManager

**目的**：`Object*::end` から `end_frame` のヒープの処理まで、データが正しく GPU に載るところまで作る。まだ描かない。

- [ ] `Object2D::new(&gpu)` / `Object3D::new(&gpu)`、`begin` / `end` の対応の判定（§2 begin / end の対応）
- [ ] `vertex*` / `index` のその場比較と変更の範囲（§2・§4）
- [ ] 三角形化：`TriangleList` / `TriangleFan` / `TriangleStrip` / `Polygon`（単調多角形分割）。頂点数が合わない場合の警告（§10 三角形化）
- [ ] 設定の関数と同値の判定・警告（§3・§11 同値の設定の扱い）
- [ ] `InstanceSlots`：スロットの再利用・世代・`InstanceId<I>`・変更の範囲（§7）。インスタンス数の増加の警告（§7）
- [ ] `DrawManager2D` / `DrawManager3D`：`ManagerId`・カメラ・`CameraId` の検査（§5）
- [ ] staging バッファ：チャンク・`mapped_at_creation`・`map_async` での再利用（§6 転送）
- [ ] `HeapRequest` と `end` 時の更新条件の表のすべての行（§2 end 時の更新条件）
- [ ] `end_frame` の段階 1〜3 と 8：チャンクの回収・ヒープの処理（確保・拡張・コンパクションのコピー・staging からのコピー）・GC と回収の通知（§9）
- [ ] 誤用の panic（§9 の表のうち `Object*` / `DrawManager*` に関するもの）

**終わりの条件**
- 頂点を変えたフレームだけ `Write` が積まれ、範囲が変わった位置の最小〜最大になるテスト
- 三角形化の各方式が期待どおりのインデックスを作るテスト。`Polygon` はへこみのある形・時計回り / 反時計回りの両方
- GC で回収された `Object*` を再び `end` すると、すべてを転送し直すテスト
- **GPU 上のヒープを読み戻し**（テスト用に `map_read` で）、`Object*` の写しと一致するテスト。Static のコンパクションと Dynamic の拡張の後でも一致すること
- 今の `src/lonas.rs` の誤用のテストをここへ移す

---

## 段階 6：Compute のパスと描画

**目的**：画面に描く。ここで初めて三角形が出る（§10）。

- [ ] パイプラインのキャッシュ（キー・`gc_unused_frames` での破棄、§10 パイプラインのキャッシュ）
- [ ] シェーダーのバインディングの型の大きさの検査（段階 3 の解析結果を使う、§11 シェーダーとの約束事）
- [ ] カメラの行列の計算と転送（描くサーフェスの大きさから、§11 Camera）。カメラの uniform バッファと動的オフセット
- [ ] Compute のパス：
  - [ ] 2D：Hidden を省き、`culling_2d` なら境界円で視界外を省き、累積和でスロットの順のまま詰める（§10 2D の Compute のパス）
  - [ ] 3D 不透明：フラスタムカリングと奥行きの大まかな並べ替え（バケットソート）
  - [ ] 3D 半透明：パイプラインごとに `Object*` をまたいで奥から手前へ並べ替え、インスタンスごとの引数を書く
- [ ] 境界球・境界円の自動計算（§10 視界の判定に使う大きさ）
- [ ] バッチングと `multi_draw_indexed_indirect`（§10 DrawCall の単位）
- [ ] 描画順：2D の `layer`、3D の不透明 → 半透明、パイプラインをまたぐ半透明は `end` の順（§10 描画順）
- [ ] 深度バッファ：`Depth32Float`・Reversed-Z・各 `DrawManager3D` の最初にクリア（§11 深度バッファ）
- [ ] `examples/test1.rs` をコンパイルできるようにし、`api-sample` feature の制限を外す

**終わりの条件**
- テクスチャなし・uniform なしの三角形が、2D と 3D の両方で画面に出る example
- 描いた画像を読み戻して画素を確かめるテスト（ウィンドウなしの描画先で）：
  - 2D の `layer` と `end` の順どおりに重なる
  - 削除したインスタンス・視界外のインスタンスが描かれない
  - 3D の半透明が `Object*` をまたいで奥から手前に重なる
- 同じ設定の `Object*` が 1 回の `multi_draw_indexed_indirect` にまとまっていることの確認（DrawCall の数を数えるテスト用の記録）

---

## 段階 7：テクスチャアトラスと uniform

**目的**：シェーダーとの約束事の残り（`texture_rects`・uniform）を完成させる（§11 TextureAtlas・uniform）。

- [ ] 棚方式の詰め込みと解除（CPU 側。GPU なしでテストできる）
- [ ] 余白（端の画素の複製）・ミップマップの段数に合わせた切り上げ
- [ ] アトラスへの転送とミップマップの作成（`end_frame` の段階 2）
- [ ] レイヤーの追加（`max_layers` まで）と、超えた場合の panic
- [ ] `texture_rects` の表と、未指定のピンク紫の画素（番号 0）
- [ ] uniform：`DrawManager` × シェーダーごとのバッファ、`DrawManager` による番号の割り当て、要素 0 の予約、拡張（§6 uniform の領域）
- [ ] uniform の型の検査（§11 uniform の型の検査）
- [ ] `shader()` / `texture()` を変えたときのインスタンスの書き直し（§2 end 時の更新条件）

**終わりの条件**
- 棚方式の詰め込みで、配置が重ならず余白が保たれるテスト（乱数で登録と解除を繰り返す）
- テクスチャを貼った 2D のスプライトと、`texture()` を呼ばなかったピンク紫の表示が出る example
- uniform の値を変えた `Object*` だけ見た目が変わり、同じシェーダーの複数の `Object*` が 1 回の DrawCall のままであることのテスト
- `examples/test1.rs` が画面で動く（`assets/player.png` を用意する）

---

## 段階 8：アンチエイリアス

**目的**：MSAA → FXAA → TAA の順に作る（§11 アンチエイリアス）。

- [ ] MSAA：描画先・深度バッファ・パイプラインのサンプル数を揃え、resolve する
- [ ] FXAA：後処理のパス
- [ ] TAA：
  - [ ] カメラのジッターと `view_projection_no_jitter`（§11 シェーダーとの約束事）
  - [ ] 前のフレームのカメラ・インスタンスの保持（`end_frame` でのコピー、§2）
  - [ ] 再投影・信頼度の評価（画面外・深度差・YCoCg の分散クリップ）・合成・保存（§11 TAA の処理の流れ）
  - [ ] `invalidate_history` と、リサイズ・切り替え直後の無効化
- [ ] `set_anti_aliasing` による実行中の切り替え（§11 実行中の切り替え）

**終わりの条件**
- 各方式で同じ場面を描き、斜めの辺がなめらかになることを画像で確かめる example
- 方式を実行中に切り替えても、エラーや表示の崩れが起きないこと
- TAA で、カメラを動かしたときの残像が `TaaConfig` の値で調整できること

---

## 進め方

- 段階ごとに作業を分け、段階の終わりの条件を満たしたらコミットする
- 実装中に設計の穴が見つかった場合は、先に DESIGN.md を直してから実装を続ける（設計と実装を食い違わせない）
- 段階 1〜3 は互いにほぼ独立しているため、並行して進めてもよい（段階 3 の `STRUCTS` の大きさのテストだけは段階 1 の詰め方と合わせる）
