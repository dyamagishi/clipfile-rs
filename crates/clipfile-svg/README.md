# clipfile-svg

`clipfile-svg` is a loss-aware, streaming SVG adapter for vector data read by
[`clipfile`](https://crates.io/crates/clipfile).

```toml
[dependencies]
clipfile-svg = "0.2"
```

It maps straight, quadratic Bezier, and cubic Bezier geometry directly to SVG
paths. CLIP spline samples can either remain an exact stored-point polyline or
be fitted to guarded cubic Bezier segments with a configurable geometric
tolerance. The fitter splits at corner flags and cusps, uses periodic neighbors
for closed strokes, and locally falls back to `L` when it cannot prove a
segment safe.

`SvgStrokeRepresentation::PreserveAppearance` keeps per-point width as a
filled outline and per-point opacity as gradient-filled segments.
`EditableCenterline` instead guarantees one `fill="none"` centerline path for
each multi-point stroke and uses the arithmetic mean width and opacity factors.
`SvgSplineConversion::CubicFit` and `PreserveAppearance` are the defaults for
compatibility with the 0.2 policy. Independently,
`SvgOnePointStrokeHandling::PreserveDots` keeps valid one-point strokes as
visible `<circle>` elements. `ConnectNearby` joins a run of two or more
consecutive one-point spline strokes when their styles match and each gap is
within half of the smaller effective stroke diameter. A nearby endpoint from
the preceding or following compatible spline is included in the bridge. A
single dot is never converted automatically. This option requires vector-level
writing (`write_vector` or `write_vector_with_brush_styles`); `write_stroke`
has no neighboring-stroke context and therefore preserves a circle.

`SvgOptions::with_nominal_width_adjustment` applies a finite absolute offset to
resolved nominal stroke diameters before per-point pressure factors are
evaluated. The default is `0.0`, so ordinary SVG output is unchanged. Raster
adapters can use a small negative adjustment to calibrate crisp SVG outlines
against another renderer; adjusted widths are clamped at zero.

Brush shapes, secondary-color behavior, and frame fills do not always have a
lossless mapping to plain SVG. The currently supported brush scope is a plain,
hard round tip. Call `write_vector_with_brush_styles` with styles read from
`Database::brush_styles` to confirm that scope. Textured, multi-image, and
spray brushes are deliberately left as a reported `BrushToRoundStroke`
approximation and can be rejected through the loss policy. The default policy
returns an `SvgExportReport` with counts for every remaining conversion and
skip. Individual loss kinds can instead be configured as errors.

When resolved brush metadata identifies CLIP's destination-out eraser mode,
vector-level writing preserves stroke order with SVG luminance masks. Each
paint segment is affected only by eraser strokes that follow it; later paint
strokes remain visible. This behavior requires
`write_vector_with_brush_styles`, because unresolved brush IDs do not expose
the composite mode.

The adapter writes XML incrementally through `quick-xml` and does not build a
complete SVG DOM. Bodies containing eraser strokes retain only lightweight
operation references until their mask definitions and ordered `<use>` nodes
have been emitted; path data is still allocated one stroke at a time.

```rust,no_run
use std::io::Cursor;

use clipfile::VectorData;
use clipfile_svg::{
    SvgCanvas, SvgOnePointStrokeHandling, SvgOptions, SvgSplineConversion,
    SvgStrokeRepresentation, SvgViewport, write_svg,
};

fn export(vector: &VectorData) -> Result<Vec<u8>, clipfile_svg::Error> {
    let output = Cursor::new(Vec::new());
    let viewport = SvgViewport::new(-100.0, 50.0, 1920.0, 1080.0)?;
    let canvas = SvgCanvas::new(960.0, 540.0)?.with_viewport(viewport);
    let (output, report) = write_svg(
        output,
        canvas,
        [vector],
        SvgOptions::default()
            .with_title("CLIP vector layer")
            .with_stroke_representation(SvgStrokeRepresentation::EditableCenterline)
            .with_one_point_stroke_handling(SvgOnePointStrokeHandling::ConnectNearby)
            .with_spline_conversion(SvgSplineConversion::CubicFit)
            .with_spline_tolerance(0.5),
    )?;
    eprintln!(
        "wrote {} mark(s): {} paths and {} dots",
        report.marks_written(),
        report.paths_written(),
        report.dots_written(),
    );
    eprintln!(
        "{} centerline path(s), {} filled outline path(s)",
        report.centerline_paths_written(),
        report.filled_outline_paths_written(),
    );
    assert!(report.is_consistent());
    Ok(output.into_inner())
}
```

`SvgCanvas::new(width, height)` remains the shorthand for
`viewBox="0 0 width height"`. `SvgViewport` separates a non-zero or negative
source origin and extent from the displayed SVG width and height.

For cells containing several vector layers, `SvgWriter::write_group` and
`write_vectors_group` emit balanced nested groups with opacity, optional `id`,
optional accessible title, and an optional validated affine matrix supplied by
the caller. Apart from the resolved vector-brush eraser mode described above,
they deliberately do not guess mappings for CLIP blend modes, masks, clipping,
or transforms.

`SvgLossKind::severity`, `SvgOptions::reject_omissions`, and
`SvgExportReport::merge` support application-level policy and multi-file
summaries. Successful reports maintain both invariants:

```text
strokes_seen    == strokes_written + strokes_skipped
strokes_skipped == sum(skip_count)
paths_written   == centerline_paths_written + filled_outline_paths_written
```

The arithmetic mean in `EditableCenterline` intentionally matches the
existing factor accumulator; it is not weighted by sample spacing. Missing
factor fallbacks remain separately reportable. `SplineToPolyline` versus
`SplineToCubicApproximation`, and `Variable*Average` versus the outline loss
kinds, always describe the representation actually selected for a written
path. Dots have no interpolation segment and receive none of those losses.

The bundled `export_vector_svg` example writes to a same-directory temporary
file, flushes and syncs it, and only then publishes a new destination. It never
overwrites an existing output and removes partial temporary output on failure
when possible.

`clipfile-svg` is versioned independently from the core format crate. Its
`0.2.x` releases require `clipfile 1.2` or newer within the compatible `1.x`
series.
