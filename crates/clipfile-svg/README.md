# clipfile-svg

`clipfile-svg` is a loss-aware, streaming SVG adapter for vector data read by
[`clipfile`](https://crates.io/crates/clipfile).

```toml
[dependencies]
clipfile-svg = "0.2"
```

It maps straight, quadratic Bezier, and cubic Bezier geometry directly to SVG
paths, and fits CLIP spline samples to cubic Bezier paths with a configurable
geometric tolerance. Per-point width becomes a fitted filled outline;
per-point opacity becomes gradient-filled segments instead of a stroke-wide
average. A valid one-point stroke becomes a visible `<circle>` using its
primary color, effective width, and opacity.

Brush shapes, secondary-color behavior, and frame fills do not always have a
lossless mapping to plain SVG. The currently supported brush scope is a plain,
hard round tip. Call `write_vector_with_brush_styles` with styles read from
`Database::brush_styles` to confirm that scope. Textured, multi-image, and
spray brushes are deliberately left as a reported `BrushToRoundStroke`
approximation and can be rejected through the loss policy. The default policy
returns an `SvgExportReport` with counts for every remaining conversion and
skip. Individual loss kinds can instead be configured as errors.

The adapter writes XML incrementally through `quick-xml`. It does not build a
complete SVG DOM or retain all converted strokes in memory; only the current
stroke's path data is allocated.

```rust,no_run
use std::io::Cursor;

use clipfile::VectorData;
use clipfile_svg::{SvgCanvas, SvgOptions, SvgViewport, write_svg};

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
            .with_spline_tolerance(0.5),
    )?;
    eprintln!(
        "wrote {} mark(s): {} paths and {} dots",
        report.marks_written(),
        report.paths_written(),
        report.dots_written(),
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
and optional accessible title. They deliberately do not guess mappings for
CLIP blend modes, masks, clipping, or transforms.

`SvgLossKind::severity`, `SvgOptions::reject_omissions`, and
`SvgExportReport::merge` support application-level policy and multi-file
summaries. Successful reports maintain both invariants:

```text
strokes_seen    == strokes_written + strokes_skipped
strokes_skipped == sum(skip_count)
```

The bundled `export_vector_svg` example writes to a same-directory temporary
file, flushes and syncs it, and only then publishes a new destination. It never
overwrites an existing output and removes partial temporary output on failure
when possible.

`clipfile-svg` is versioned independently from the core format crate. Its
`0.2.x` releases require `clipfile 1.2` or newer within the compatible `1.x`
series.
