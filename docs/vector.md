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

coreはCLIP splineの保存座標・point flag・kindを返し、描画方式を決めない。`clipfile-svg`は`SvgSplineConversion::Polyline`でdrawableな保存点をM/Lへそのまま写像するか、`CubicFit`で公式アプリのSVG出力と実作業ファイルを基準にしたcurve fittingを選べる。既定fitは0.5 SVG user unitの許容誤差を使い、corner/cuspで区間を分割し、閉線を周期的に扱い、overshoot・loop・非有限値・過大handleを検査する。安全性を証明できない局所区間だけLへfallbackし、`SplineFitSegmentToLine`を記録する。許容誤差は`SvgOptions::with_spline_tolerance`で変更できる。公式SVGの649 anchor中498件は保存点と丸め誤差内で一致したが、151件は点間の仮想anchorだったため、公式処理は保存点の選択だけでなく平滑化または再分割も行うと考えられる。現在のfitはこの非公開処理のsample-to-cubic近似であり、NURBS→Bezier変換や制御点の完全一致を意味しない。

## style、太さ、不透明度

`VectorStroke::style`は対応layoutで次を返す。

- losslessなmain/sub color。各32-bit channel wordと観測済みhigh-byte 8-bit値の両方
- stroke opacity
- common/effective brush ID
- 保存されているbrush radiusと、その2倍を返すnominal diameter helper
- frame fill-style ID
- 92-byte normal headerが参照する`BrushStyle.MainId`

`Database::brush_style` / `brush_styles`は、このIDからsize・opacity・flow・interval・thickness等のeffector、hardness、tip/texture/spray参照、sub color量を取得する。graph-backed effectorは`brush_effector_graph`でbig-endian `f64`入出力点へ展開できる。`BrushPatternImage` / `BrushPatternStyle` / `BrushFixedSpray`は先端画像・並び順・spray payloadを公開し、未知の配列suffixはraw byteを保持する。frameの`fill_style_id`は`Database::fill_style`へ解決できる。

各pointの`width_factor`と`opacity_factor`は、それぞれstrokeのnominal width・opacityへ作用する確認済み係数である。これらを入力機器の生の「筆圧」とは呼ばない。元のtablet pressure値またはその変換規則は独立に確認できていないためである。

## SVGとの境界

workspace内の独立crate [`clipfile-svg`](../crates/clipfile-svg/) が、公開APIからSVG要素を逐次出力する。直線、二次Bezier、三次Bezierをpathへ直接写像する。既定の`SvgOnePointStrokeHandling::PreserveDots`では有効な1点strokeを実効幅・主色・opacityを持つcircleへ写像する。任意の`ConnectNearby`では、同じstyleを持つ2件以上の連続した1点spline strokeについて、各点間距離が小さい側の実効線幅の半分以内なら中心線pathへ結合する。互換性のある直前・直後splineの端点も同じ距離条件でbridgeへ含める。単独点は変換しない。この判定にはstroke順序が必要なため`write_vector` / `write_vector_with_brush_styles`でだけ行い、単独の`write_stroke`はcircleを維持する。

- splineは選択した保存点polylineまたは許容誤差付きcubic Bezier fitting
- 見た目優先modeではpointごとの可変幅を左右輪郭、可変不透明度を区間gradient
- 編集可能中心線modeでは各factorの単純算術平均（点間距離の重み付けは行わない）
- plain hard round以外のbrush形状・textureはround linecap/joinへの明示的近似
- sub color metadataとframe fill styleは未反映
- nominal widthがない場合は1 SVG user unit
- 近接する1点strokeを中心線へ結合した場合は`AdjacentOnePointStrokesConnected`
- semantic layoutが未知のstrokeはskip

既定では近似を実行して`SvgExportReport`に種類別のstroke数を集計する。`paths_written`は実際のpath要素数、内訳は`centerline_paths_written`と`filled_outline_paths_written`、circle数は`dots_written`、pathとcircleの合計は`marks_written`で取得する。可変不透明度では1 strokeが複数pathになり得るため、入力stroke数は`strokes_written`で別に取得する。描画しないstrokeは`SvgSkipKind`で空、未知semantic、未知curve、不完全geometry、policy拒否に分類される。成功したreportでは`strokes_seen == strokes_written + strokes_skipped`、skip総数が`strokes_skipped`、path内訳の和が`paths_written`と一致する。

`SvgLossKind::severity`は近似・欠落・fallbackを分類し、`SvgOptions::reject_omissions`で近似を許しながら欠落だけを拒否できる。`SvgExportReport::merge`は複数ファイルの集計に使える。`SvgViewport`は表示width/heightと非0または負のviewBox原点を分離する。`SvgWriter::write_group` / `write_vectors_group`はopacity、id、title付きのgroupをcallback内で対にする。blend mode、mask、clipping、transformは推測して変換しない。

XMLは`quick-xml`へ逐次書き込み、全SVG DOMや全strokeの変換結果を保持しない。割り当てる主な一時領域は現在の1 strokeのpath dataである。公式exampleは同一directoryの一時ファイルへ書き、finish・flush・sync後だけ新規出力として確定する。

`SvgStrokeRepresentation::PreserveAppearance`、`SvgSplineConversion::CubicFit`、`SvgOnePointStrokeHandling::PreserveDots`が既定である。`EditableCenterline`は2点以上を原則1本の`fill="none"` pathにし、round cap/joinを明示する。1点strokeの保持・近接結合はこの可変幅表現とは独立して選択する。保持された1点strokeは区間がないためSpline変換・outline/average lossを持たない。factor変動はsentinelと重複点を除いた実drawable点だけから判定する。

## 高ビット深度raster

`PixelPacking::native_bits_per_pixel` / `native_bytes_per_pixel`は8-bit `PixelFormat`とは独立に保存深度を返す。`ClipFile::decode_native_tile`はbyte-alignedな16-bit/32-bit channelを`DecodedTile`のnative planar/interleaved byte列として返し、8-bitへ変換しない。bit-packed layoutはbit順序・row paddingが未確定なため対象外である。今回の実作業ファイルに含まれる131 raster sourceはすべて8-bit/channelだったため、高ビット深度のend-to-end fixture検証は引き続き必要となる。

core crateはSVG型や出力policyに依存せず、format modelとadapterの責務を分離する。adapterは`clipfile-svg`として独立versionで公開し、`clipfile 1.2`の公開vector APIへversion付きで依存する。
