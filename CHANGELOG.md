# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `BrushStyle::is_eraser` identifies the observed destination-out brush
  composite mode used by vector eraser strokes.
- `clipfile-svg` exposes `SvgOptions::with_nominal_width_adjustment` for
  renderer-specific absolute stroke-width calibration while retaining stored
  per-point width factors.

### Fixed

- Image-cel tracks with no `ImageCelName` curve are now exposed as empty cel
  tracks, allowing valid animation folders with no assigned timeline cels.
- Animation readers now select only the `FirstTrack` chain for the requested
  timeline when multiple timelines share one track bank, instead of rejecting
  the other timeline chains as unreachable tracks.
- Default animation reads now follow `FirstTimeLine` links and honor the
  animation bank's zero-based `CurrentIndex`, rather than treating the head of
  the timeline list as the selected timeline.
- Discontinuous cel animation loops now retain every `ActionNodeClip` segment;
  later loops are evaluated with their own local `TimeClip` instead of being
  discarded after the first curve.
- `clipfile-svg` now applies vector eraser strokes as order-preserving SVG
  masks instead of rendering their stored color as an ordinary thick line.
- Connected one-point vector runs now preserve per-point width and opacity
  variation when appearance-preserving SVG output is selected, instead of
  averaging the factors across the complete run.

## [clipfile-svg 0.2.2] - 2026-08-07

### Added

- `SvgGroupOptions::with_transform_matrix` writes a validated finite SVG
  affine matrix on a nested group.

## [1.2.2] - 2026-08-07

### Added

- Typed half-open `AnimationTimeClip` metadata is exposed on generic and
  image-cel tracks.

### Fixed

- `CelTrack::cel_at_frame` now returns no cel outside the track's active
  `TimeClip`, instead of retaining the final preceding cel indefinitely.
- `CelTrack::cel_at_frame` now evaluates curve keys relative to the start of
  their `TimeClip`, fixing frozen cel selection on timelines whose start is
  not frame zero.
- Timeline end-frame documentation now reflects the stored exclusive bound.

## [clipfile-svg 0.2.1] - 2026-08-04

### Added

- `clipfile-svg` now exposes independent appearance/editable-centerline and
  polyline/guarded-cubic spline modes through `SvgOptions`, plus report counts
  for centerline and filled-outline paths.
- A local spline-fit line fallback has its own policy-rejectable
  `SplineFitSegmentToLine` loss category.
- `clipfile-svg` can optionally join consecutive, style-compatible nearby
  one-point spline strokes into an editable centerline while preserving dots
  by default for backward compatibility.

### Changed

- Spline cubic fitting now splits at corner flags and cusps, handles closed
  strokes with periodic neighbors, rejects non-finite values, loops,
  backtracking, oversized handles, and tolerance-corridor overshoot, and falls
  back only for unsafe local segments.
- Variable-width outlines now use bounded joins, adaptive width-transition
  samples, guarded sides, and cap-free closed contours. Variable-opacity
  segments use spacing-aware tangents with guarded line fallback.
- Width and opacity variation are calculated after drawable-point
  normalization, so outline/average losses match the mark actually written.

## [1.2.1] - 2026-08-04

### Fixed

- Older `BrushStyle` schemas that omit the optional `MipmapIndexToPlot`
  column now return `None` for that field instead of failing the complete
  style query. Required columns remain errors.

## [clipfile-svg 0.2.0] - 2026-08-04

### Changed

- `clipfile-svg` now fits CLIP spline samples to cubic Bezier paths, preserves
  variable width as a smooth filled outline, and preserves variable opacity
  through per-segment linear gradients instead of averaging the point values.
- The default spline fitting tolerance is calibrated to `0.5` SVG user units
  after comparing stored points and CLIP STUDIO's exported cubic paths.
- The SVG example resolves core brush styles and suppresses the round-brush
  approximation warning for confirmed hard, circular, untextured tips.
- Plain hard round tips are the supported brush scope. Textured, multi-image,
  and spray brushes remain explicit, policy-rejectable approximations.

## [1.2.0] - 2026-08-04

### Added

- Typed brush-style, dynamics graph, brush-tip image/style, fixed-spray, and
  frame-fill metadata APIs, including `VectorStrokeStyle::brush_style_id` and
  bounded brush-specific parser limits.
- Native byte-aligned raster tile decoding based on `PixelPacking`, preserving
  16-bit and 32-bit channel bytes without converting through an 8-bit
  `PixelFormat`.

## [clipfile-svg 0.1.1] - 2026-08-04

### Added

- `clipfile-svg` viewport origins independent from display size, balanced
  nested layer groups, loss severity and omission policies, report merging,
  one-point dot counts, and complete skip-reason counts.

### Fixed

- `clipfile-svg` now emits semantically valid one-point strokes as visible
  circles instead of silently skipping them, while preserving the meaning of
  `paths_written` and avoiding a false spline-approximation count for dots.

### Changed

- The SVG export example now writes beside the destination and atomically
  publishes a complete new file without overwriting an existing output.

## [1.1.0] - 2026-08-04

### Added

- Lossless typed vector-data reading across observed variable header and point
  record layouts, exposing geometry, curve controls, colors, brush sizing,
  stroke opacity, point width/opacity factors, and raw forward-compatible
  fields.
- A separate workspace crate, `clipfile-svg`, with streaming `quick-xml`
  output, configurable loss policies, and an aggregate approximation report
  for splines, variable factors, brush rendering, colors, and frame fills.

## [1.0.0] - 2026-07-25

### Added

- Format-independent and format-specific immutable/mutable
  RGBA8/Gray8/GrayAlpha8 pixel iterators with named channels, channel bounds,
  common inversion, and explicit checked or saturating arithmetic helpers.
- Validated GrayAlpha8 raster decoding and writing for the observed planar
  alpha/value tile layout.
- Optional zero-copy consuming conversion from `RasterImage` to image-rs
  `DynamicImage`, plus checked conversion back from supported 8-bit image-rs
  buffers, without making image-rs a default dependency.
- High-level ruler-layer discovery that hides schema probing and SQL while
  validating every returned layer.
- High-level correction-layer discovery and SQL-free raster discovery in
  examples.
- Semantic raster replacement helpers that retain the image format and select
  CSP-compatible checksums automatically.
- Animation helpers for explicit timeline loading, template-complete key
  insertion, internal image-cel value assignment, and known track-kind names.
- A public API-level audit documenting typed semantic entry points and the
  intentional forward-compatibility escape hatches.
- Template-based plain raster-layer cloning with regenerated row identities,
  layer UUID, external IDs, tree links, CSP-compatible base pixels, and fresh
  non-stale derived-cache references.
- Text-object removal with synchronized primary/additional arrays and safe
  primary-object promotion while retaining at least one object.
- Conservative vector-stroke template cloning and removal for the validated
  92-byte-header/88-byte-point layout, preserving all unknown fields.
- Primary/secondary animation-curve key insertion and removal, normalized
  image-cel Track cloning from a complete kind-2000 template, and Track
  unlinking with timeline-chain repair.
- Purpose-specific examples and safety documentation for each new semantic
  write operation.

### Changed

- Document render-cache boundaries explicitly: raster clones invalidate
  derived references, while vector edits and canvas previews are not
  regenerated automatically.

### Fixed

- Enable all crate features for rust-analyzer in VS Code so feature-gated
  examples receive complete type analysis and inlay hints.

## [0.5.0] - 2026-07-24

### Added

- Typed constants and stable names for all 29 observed layer blend modes,
  while retaining lossless raw-value construction for future modes.
- Conservative write APIs for per-character-width-preserving text content,
  atomic text-object template cloning with fresh document-wide IDs, validated
  vector translation and opaque bodies, typed animation values, existing
  curve keys, and cel tags.
- CSP-compatible `BlockCheckSum` generation using Adler-32 over the
  little-endian compressed-size prefix and zlib payload, while retaining the
  explicit zero-checksum compatibility mode.
- Safe staging of new external objects before SQLite, including generated
  index rows, repaired offsets, duplicate and limit checks, rollback helpers,
  and strict reopened-output validation.
- Conservative animation Track cloning with regenerated row/track/external
  identities, independent primary and secondary mixer bodies, complete
  unknown-column preservation, and validated timeline-chain insertion.

### Fixed

- Preserve the observed SQLite storage class for newly indexed external
  identifiers so generated files remain readable by CLIP STUDIO PAINT.
- Keep the Track allocation high-water mark in `ElemScheme` synchronized when
  cloning animation tracks.

## [0.4.0] - 2026-07-24

### Added

- Opt-in `write` support with an editable in-memory SQLite database, exact
  no-change round trips, external-body replacement, offset repair, and
  create-new output validation.
- Validated re-encoding of one existing block-data tile with an explicit
  zero-checksum compatibility opt-in and a small raster-editing example.
- Purpose-specific document, preview, text, vector, raster, animation, and
  time-lapse examples with an examples guide and annotated processing steps.

### Fixed

- Keep animation and page-management validation compatible with the declared
  Rust 1.85 minimum version.

### Documentation

- Define full-image, vector, text, and animation semantic writing as the
  priority before a stable 1.0 release, with time-lapse and 3D following later.

## [0.3.0] - 2026-07-24

### Added

- Bounded decoding of inline animation `TrackValueMap` entries, including
  observed floating-point and indexed-text values with opaque fallback.
- Secondary action-mixer presence reporting on `AnimationTrack`.
- Streaming time-lapse frame indexing with internal record, sequence,
  RIFF/WebP boundary, and observed VP8 dimension validation.
- Verified time-lapse full key-frame and positioned delta-patch helpers.
- Validated animation `FirstTrack` / `TrackNextIndex` chains and helpers for
  observed folder, static-image, paper, play-time, and audio track kinds.
- Sparse double-precision `FCurve` decoding for validated secondary
  action-mixer value records.
- Raster-state helpers for distinguishing default-filled images from decoded
  external block data.
- Common opaque block-status reporting through `BlockData::uniform_status`
  when every block in an external object agrees.
- Bounded standalone `.cmc` page-management loading with validated node trees,
  raw link preservation, and traversal-safe page-path resolution.
- Bounded decoding of all nine observed correction-layer kinds, including
  fixed-point raw values, gradient curves, and opaque future-kind fallback.
- Validated vector-ruler references and typed, bounded metadata for all nine
  observed special-ruler table kinds and perspective vanishing-point chains.
- Validated 2D-camera layer snapshots, typed kind-`2005` saved-position values,
  and axis-qualified primary and secondary position/center curves.
- Verified 2D-camera rotation in degrees and scale/opacity percentages.
- Secondary-curve decoding across observed metadata-header variants, including
  double-precision `PlayTime` records.

### Documentation

- Clarify that observed time-lapse streams encode frame order but no
  recoverable wall-clock timestamp.

## [0.2.0] - 2026-07-23

### Added

- Direct resolution of a layer-mask raster source through `Database::layer_mask_raster_source`.
- Bounded `CanvasPreview` extraction with PNG IHDR dimension validation.
- Bounded raw access to external vector-layer data through
  `Database::vector_data_sources` and `ClipFile::read_vector_data`.
- Bounded read and streaming copy APIs for complete external-object bodies.
- Bounded UTF-8 text-layer extraction with opaque attribute preservation and
  checked parsing of additional-object arrays.
- Optional timeline and image-cel selection decoding, including bounded BINC
  mixer parsing and layer-UUID resolution.
- Generic primary action-mixer `FCurve` access with raw track kinds,
  interpolation, slopes, optional tags, and audio/play-time curve support.
- Optional validated time-lapse manager, record, and blob chains with bounded
  allocation and streaming zlib decoding.
- Indexed external-object resolution through `ClipFile::resolve_external_object`.

### Documentation

- Record real-file validation of 8-bit grayscale layer masks and opaque vector
  references, UTF-8 text storage, and image-cel animation curves; anonymize
  local-corpus reporting.

## [0.1.0] - 2026-07-22

### Added

- Initial crate and project infrastructure.
- Streaming parser for the CLIP top-level container.
- Structural validation and an `inspect` example.
- Configurable parsing safety limits.
- External-body classification and block-data indexing.
- Optional bundled-SQLite access, schema discovery, and external-index validation.
- Optional offscreen metadata parsing, bounded zlib tile decoding, and raster assembly.
- Project, canvas, layer, and cycle-checked layer-tree models.

[Unreleased]: https://github.com/Aodaruma/clipfile-rs/compare/clipfile-svg-v0.2.1...HEAD
[clipfile-svg 0.2.1]: https://github.com/Aodaruma/clipfile-rs/compare/clipfile-svg-v0.2.0...clipfile-svg-v0.2.1
[1.2.1]: https://github.com/Aodaruma/clipfile-rs/compare/v1.2.0...v1.2.1
[clipfile-svg 0.2.0]: https://github.com/Aodaruma/clipfile-rs/compare/clipfile-svg-v0.1.1...clipfile-svg-v0.2.0
[1.2.0]: https://github.com/Aodaruma/clipfile-rs/compare/v1.1.0...v1.2.0
[clipfile-svg 0.1.1]: https://github.com/Aodaruma/clipfile-rs/compare/clipfile-svg-v0.1.0...clipfile-svg-v0.1.1
[1.1.0]: https://github.com/Aodaruma/clipfile-rs/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.5.0...v1.0.0
[0.5.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Aodaruma/clipfile-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Aodaruma/clipfile-rs/releases/tag/v0.1.0
