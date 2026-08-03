# clipfile-svg

`clipfile-svg` is a loss-aware, streaming SVG adapter for vector data read by
[`clipfile`](https://crates.io/crates/clipfile).

```toml
[dependencies]
clipfile-svg = "0.1"
```

It maps straight, quadratic Bezier, and cubic Bezier geometry directly to SVG
paths. CLIP splines, per-point width/opacity, brush shapes, secondary colors,
and frame fills do not have a lossless mapping to a plain SVG stroke. The
default policy uses documented approximations and returns an `SvgExportReport`
with counts for every kind of loss. Individual loss kinds can instead be
configured as errors.

The adapter writes XML incrementally through `quick-xml`. It does not build a
complete SVG DOM or retain all converted strokes in memory; only the current
stroke's path data is allocated.

```rust,no_run
use std::io::Cursor;

use clipfile::VectorData;
use clipfile_svg::{SvgCanvas, SvgOptions, write_svg};

fn export(vector: &VectorData) -> Result<Vec<u8>, clipfile_svg::Error> {
    let output = Cursor::new(Vec::new());
    let canvas = SvgCanvas::new(1920.0, 1080.0)?;
    let (output, report) = write_svg(
        output,
        canvas,
        [vector],
        SvgOptions::default().with_title("CLIP vector layer"),
    )?;
    eprintln!("wrote {} path(s)", report.paths_written());
    Ok(output.into_inner())
}
```

`clipfile-svg` is versioned independently from the core format crate. Its
`0.1.x` releases require `clipfile 1.1` or newer within the compatible `1.x`
series.
