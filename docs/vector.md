# ベクターデータAPI

`sqlite` featureでは、レイヤーが参照するvector外部本体を構造検証し、stroke・point単位の借用viewとして読める。

```rust,no_run
use std::fs::File;

use clipfile::{ClipFile, Limits};

let limits = Limits::default();
let mut clip = ClipFile::open(File::open("drawing.clip")?)?;
let database = clip.open_database()?;

for source in database.vector_data_sources(42, limits)? {
    let vector = clip.read_vector(&database, &source, limits)?;
    for stroke in vector.strokes() {
        let style = stroke.style();
        for point in stroke.points() {
            let position = point.position();
            let controls = point.control_points().collect::<Vec<_>>();
            let factors = point.factors();
            let _ = (&style, position, controls, factors);
        }
    }
}

Ok::<(), Box<dyn std::error::Error>>(())
```

## 公開する境界

- `VectorData` は外部本体を1回だけ所有し、stroke境界だけを索引する。
- `VectorStroke` / `VectorPoint` / iteratorは元bodyを借用するため、全点を別の意味モデルへ複製しない。
- `VectorLayout` はheader、point-section、point、default-pointの4サイズを元値のまま返す。
- `VectorStrokeFlags` / `VectorPointFlags` は未知bitを保持する。
- `raw()`、`raw_header()`、`raw_points()` は未解釈fieldと将来layoutの調査用に元byteを返す。
- `ClipFile::read_vector_data` は構造解析もしない完全なraw escape hatchとして維持する。

構造走査にはheader sizeとpoint size/countだけを使う。宣言境界がbody末尾と一致する未知layoutは拒否せず、意味getterを`None`にしてraw recordを保持する。既知layoutでは座標、制御点、style、point factorの有限性と範囲も`read_vector`時に検証する。

stroke数は`Limits::max_vector_strokes`、全point数は`Limits::max_vector_points`、外部本体は`Limits::max_vector_data_bytes`で制限する。

## 観測済みlayout

ローカル・参考コーパス2,757 stroke / 23,655 pointでは、default point sizeはすべて88 byteで、次の9組を確認した。

| stroke header | point section | point record | stroke kind |
|---:|---:|---:|---|
| 76 | 72 | 88 / 104 / 120 | ruler |
| 88 | 72 | 88 / 104 | normal |
| 92 | 76 | 88 | normal（現行生成例） |
| 96 | 72 | 88 / 104 / 120 | frame |

`VectorLayout::is_observed`はこの完全な4サイズ組を判定する。一方、`VectorStroke::is_semantically_supported`は、独立に確認したheader/kind相関とpoint-size/curve相関を合成する。このため、意味上安全に合成できる組がコーパス内の完全一致を持たない場合もある。

## geometryと曲線

`VectorPoint::position`は倍精度のcanvas座標を返す。curve kindはstroke flagから`Straight`、`Quadratic`、`Cubic`、`Spline`を判定する。104-byte pointには1点、120-byte pointには2点の倍精度制御座標があり、`control_points`で保存順に取得できる。

二次・三次Bezierの制御点は、次のpointではなく区間の始点側pointに属する。つまりpoint `i` から `i + 1` への区間ではpoint `i` のcontrolsを使う。closed strokeの最終区間も最後のpointのcontrolsから最初のpointへ接続する。

CLIP splineの補間規則はSVG pathへ直接対応付けていないため、ライブラリは座標とkindを返し、描画方式を決めない。

## style、太さ、不透明度

`VectorStroke::style`は対応layoutで次を返す。

- losslessなmain/sub color。各32-bit channel wordと観測済みhigh-byte 8-bit値の両方
- stroke opacity
- common/effective brush ID
- 保存されているbrush radiusと、その2倍を返すnominal diameter helper
- frame fill-style ID
- 92-byte normal headerの未命名追加word

これは完全なbrush rendererではない。brush IDが参照する形状、texture、混色などはstyle値だけでは復元できない。

各pointの`width_factor`と`opacity_factor`は、それぞれstrokeのnominal width・opacityへ作用する確認済み係数である。これらを入力機器の生の「筆圧」とは呼ばない。元のtablet pressure値またはその変換規則は独立に確認できていないためである。

## SVGとの境界

workspace内の独立crate [`clipfile-svg`](../crates/clipfile-svg/) が、公開APIからSVG要素を逐次出力する。直線、二次Bezier、三次Bezierをpathへ直接写像し、有効な1点strokeは実効幅・主色・opacityを持つcircleへ写像する。1点strokeには補間区間がないためspline近似には数えない。次を明示的なlossとして扱う。

- splineは直線区間
- pointごとの可変幅・不透明度はstroke内の一定平均値
- brush形状・textureはround linecap/join
- sub color metadataとframe fill styleは未反映
- nominal widthがない場合は1 SVG user unit
- semantic layoutが未知のstrokeはskip

既定では近似を実行して`SvgExportReport`に種類別のstroke数を集計する。`paths_written`はpath数のまま維持し、circle数は`dots_written`、両者の合計は`marks_written`で取得する。描画しないstrokeは`SvgSkipKind`で空、未知semantic、未知curve、不完全geometry、policy拒否に分類される。成功したreportでは`strokes_seen == marks_written + strokes_skipped`かつskip総数が`strokes_skipped`と一致する。

`SvgLossKind::severity`は近似・欠落・fallbackを分類し、`SvgOptions::reject_omissions`で近似を許しながら欠落だけを拒否できる。`SvgExportReport::merge`は複数ファイルの集計に使える。`SvgViewport`は表示width/heightと非0または負のviewBox原点を分離する。`SvgWriter::write_group` / `write_vectors_group`はopacity、id、title付きのgroupをcallback内で対にする。blend mode、mask、clipping、transformは推測して変換しない。

XMLは`quick-xml`へ逐次書き込み、全SVG DOMや全strokeの変換結果を保持しない。割り当てる主な一時領域は現在の1 strokeのpath dataである。公式exampleは同一directoryの一時ファイルへ書き、finish・flush・sync後だけ新規出力として確定する。

core crateはSVG型や出力policyに依存せず、format modelとadapterの責務を分離する。adapterは`clipfile-svg`として独立versionで公開し、`clipfile 1.1`の公開vector APIへversion付きで依存する。
