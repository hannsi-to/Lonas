# テスト用のフォント

`src/text/` のテスト（`text` feature）が使う。どちらも SIL Open Font License 1.1。ライセンス文は同じフォルダーの `OFL-*.txt`。

| ファイル | 元 | 手を加えた点 |
|---|---|---|
| `Cantarell-Regular.ttf` | Cantarell 0.0.5（crates.io の `sctk-adwaita` 0.10.1 に含まれるもの） | なし |
| `NotoSansJP-Subset.ttf` | Noto Sans JP（[google/fonts](https://github.com/google/fonts/tree/main/ofl/notosansjp) の `NotoSansJP[wght].ttf`） | fontTools で wght=400 に固定し、テストで使う字だけに絞った（下記） |

- Cantarell はラテン文字だけを持ち、日本語の字を持たない。Noto Sans JP の部分集合はラテン文字を持たない。フォールバックのテストはこの違いを使う
- Noto Sans JP の部分集合に含めた字：U+3000〜303F（句読点など）、ひらがな、カタカナ、`！（）？`、テストの文に使う漢字

作り直す場合（fontTools）：

```
fonttools varLib.instancer "NotoSansJP[wght].ttf" wght=400 --update-name-table -o NotoSansJP-400.ttf
pyftsubset NotoSansJP-400.ttf --unicodes="U+3000-303F,U+3041-3096,U+30A1-30FA,U+30FC,U+FF01,U+FF08,U+FF09,U+FF1F" --text="日本語文章折返表示句読点行頭来規則字間長単語赤太細斜体影縁取下線上打消色大小書式描画整形配置揃中央右左端幅高送改段落試験例何度繰東京都私今天気良雨雪春夏秋冬山川海空花木森林人名前後年月時分秒上下内外出入見聞話読書食飲行来帰" --name-IDs="*" --output-file=NotoSansJP-Subset.ttf
```
