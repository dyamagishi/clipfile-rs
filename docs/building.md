# Template-free raster documents

The optional `build` feature creates a `.clip` without an existing file. It
enables `write`, `image`, `raster`, and SQLite. The core builder accepts typed
RGBA images; PNG discovery and metadata JSON handling live in the example.

```toml
[dependencies]
clipfile = { version = "1.1", features = ["build"] }
```

`build` is introduced after the 1.1.0 release: until it is released, use this
checkout or a pinned Git dependency instead of the published 1.1.0 crate.

## API

```rust,no_run
use clipfile::{Limits, PixelFormat, RasterDocumentBuilder, RasterImage, RasterLayerOptions};

let mut builder = RasterDocumentBuilder::new(320, 180, Limits::default())?;
let folder = builder.add_folder("Paint", None, 256)?;
let image = RasterImage::from_pixels(32, 24, PixelFormat::Rgba8, vec![255; 32 * 24 * 4])?;
let mut options = RasterLayerOptions::new("Patch");
options.parent = Some(folder);
options.offset = (10, -5);
options.opacity = 154;
builder.add_layer(image, options)?;
builder.write_to_path("new-document.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Supported:

- explicit positive canvas dimensions, including a transparent empty document;
- multiple RGBA8 raster layers, with mixed input sizes;
- positive/negative layer offsets and `0..=256` opacity;
- nested normal-blend folders and insertion-ordered sibling stacks;
- independent layer UUIDs and external IDs even for duplicate names;
- five linked mipmap metadata levels, 512×512 thumbnails, and a PNG preview;
- generated native alpha + B/G/R/X tiles and CSP-compatible Adler-32 checksums;
- explicit preview replacement, including a different preview image size;
- bounded generation using `Limits`, structural/database/tree validation, and
  create-new output semantics.

`None` as a parent means the root; folder IDs must come from the same builder.
Children are stacked bottom-to-top in insertion order. Images are placed in a
storage region matching clip-file's offset conventions. The canvas clips the
displayed result; negative offsets retain the needed extra storage extent.

Lower mipmap levels use absent external IDs (derived cache metadata), following
the Python implementation. Thumbnails use image-rs Lanczos resizing. Transparent
native pixels use the Python builder's white color/padding convention, so hidden
RGB at alpha zero is normalized rather than preserved byte-for-byte.

The default preview uses normal alpha-over and folder-group opacity. It is a
convenience preview, not a full CSP renderer; supply a reference preview with
`set_preview` when exact application rendering is required.

## PNG example

```console
cargo run --features build --example build_raster -- input.png new-output.clip
cargo run --features build --example build_raster -- layers/ new-output.clip --order natural
cargo run --features build --example build_raster -- layers/ new-output.clip --metadata layers/metadata.json --canvas-size 1024x768 --preview reference.png
```

Options: `--canvas-size auto|WIDTHxHEIGHT`, `--metadata`, `--preview`,
`--order name|natural`, `--pattern GLOB`.

Single PNGs infer their canvas from input dimensions. Unannotated directories
use the maximum width/height. Explicit dimensions override metadata; metadata
without canvas size follows clip-file's 1024×768 default. The example discovers
`metadata.json` automatically. It supports legacy filename-keyed entries and
version-2 `layers`/`folders` metadata, including `source_opacity`, nested folder
paths, and optional declared image sizes. JSON dependencies are example-only.

Default directory discovery skips `combine*.png` and preserves clip-file's
legacy ordering for `layer_tiles`, `layer_grid`, `layer_labels`, `layer_markers`.
Remaining files use name or natural order. Output paths must be new and their
parent directory must already exist.

## Provenance and verification

The initial SQLite schema, row defaults, object relationships, offset storage,
tile wrappers, and cache layout are based on
[clip-file's builder](https://github.com/olduvai-jp/clip-file/blob/63b0c250e847d042e68a2347ce0b8fc4891ec212/clip_parser/builder.py)
and its `builder_constants.py` and `test_build_clip.py`. These are
reverse-engineered conventions, not an official CELSYS specification.

Synthetic Rust regression tests cover multi-tile partial-alpha rasters, edge
padding, positive/negative offsets, checksums, duplicate names, nested folders,
empty documents, preview overrides, limits, parent validity, non-overwriting
output, and subsequent editing with `ClipWriter`.

The optional independent oracle creates documents with both builders and decodes
both using the existing Python reconstructor. It compares layer/tree/position/
opacity metadata and every reconstructed RGBA byte across 10 cases: single PNG,
flat stack, positioned layers, opacity, nested folders, mixed dimensions/natural
ordering, negative partial-alpha offsets, explicit dimensions, preview override,
and glob filtering.

```console
cargo build --all-features --examples
uv run --project ../clip-file python tests/compare_python_builder.py --python-repo ../clip-file
```

This checks generated raster-document interoperability. The new Rust-generated
files have not yet been manually reopened/resaved in CSP as part of this change.
It does not establish arbitrary vector/text/animation object creation or full
CSP compositing parity.
