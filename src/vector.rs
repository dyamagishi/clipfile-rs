use std::{
    io::{Read, Seek},
    iter::FusedIterator,
};

#[cfg(feature = "write")]
use rusqlite::OptionalExtension;
use rusqlite::{params, types::ValueRef};

use crate::vector_semantics::{
    DecodedColor, DecodedPointFactors, DecodedStrokeKind, DecodedStrokeStyle, SemanticLayout,
    decode_control_points, decode_point_factors, decode_stroke_style, is_supported_semantic_layout,
};
use crate::{ClipFile, Database, Error, Limits, Result};
#[cfg(feature = "write")]
use crate::{ClipWriter, DatabaseSchema};

#[cfg(feature = "write")]
const SUPPORTED_VECTOR_HEADER_SIZE: usize = 92;
#[cfg(feature = "write")]
const SUPPORTED_VECTOR_POINT_SECTION_SIZE: u32 = 76;
#[cfg(feature = "write")]
const SUPPORTED_VECTOR_POINT_SIZE: usize = 88;

const VECTOR_LAYOUT_PREFIX_SIZE: usize = 20;
const VECTOR_STROKE_FLAGS_OFFSET: usize = 20;
const VECTOR_STROKE_BOUNDS_OFFSET: usize = 24;
const VECTOR_POINT_BOUNDS_OFFSET: usize = 16;
const VECTOR_POINT_FLAGS_OFFSET: usize = 32;
const VECTOR_POINT_BASE_SIZE: usize = 88;
const VECTOR_CONTROL_POINT_SIZE: usize = 16;

/// Sizes declared by one vector stroke record.
///
/// The four values are preserved even when the combination has not yet been
/// assigned a semantic meaning. This lets callers inspect new CLIP revisions
/// without discarding their original bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VectorLayout {
    header_size: u32,
    point_section_size: u32,
    point_size: u32,
    default_point_size: u32,
}

impl VectorLayout {
    /// Declared byte length of the stroke header.
    #[must_use]
    pub const fn header_size(self) -> u32 {
        self.header_size
    }

    /// Declared size of the point-section metadata used by CLIP.
    #[must_use]
    pub const fn point_section_size(self) -> u32 {
        self.point_section_size
    }

    /// Declared byte length of each point record in this stroke.
    #[must_use]
    pub const fn point_size(self) -> u32 {
        self.point_size
    }

    /// Declared default point-record size stored in the stroke header.
    #[must_use]
    pub const fn default_point_size(self) -> u32 {
        self.default_point_size
    }

    /// Whether this exact four-size tuple has been observed in the test corpus.
    ///
    /// An unobserved result does not make the record invalid: its raw bytes
    /// remain available when its declared boundaries can be validated.
    #[must_use]
    pub const fn is_observed(self) -> bool {
        self.default_point_size == 88
            && matches!(
                (self.header_size, self.point_section_size, self.point_size),
                (76, 72, 88 | 104 | 120)
                    | (88, 72, 88 | 104)
                    | (92, 76, 88)
                    | (96, 72, 88 | 104 | 120)
            )
    }

    const fn semantic(self) -> SemanticLayout {
        SemanticLayout {
            header_size: self.header_size,
            point_section_size: self.point_section_size,
            point_size: self.point_size,
            default_point_size: self.default_point_size,
        }
    }
}

/// A finite two-dimensional coordinate from a vector point record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorPosition {
    x: f64,
    y: f64,
}

impl VectorPosition {
    /// Horizontal coordinate in canvas units.
    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    /// Vertical coordinate in canvas units.
    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

/// An integer bounding rectangle stored by CLIP for a stroke or point.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VectorBounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl VectorBounds {
    /// Left edge in canvas units.
    #[must_use]
    pub const fn left(self) -> i32 {
        self.left
    }

    /// Top edge in canvas units.
    #[must_use]
    pub const fn top(self) -> i32 {
        self.top
    }

    /// Right edge in canvas units.
    #[must_use]
    pub const fn right(self) -> i32 {
        self.right
    }

    /// Bottom edge in canvas units.
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.bottom
    }
}

/// Raw bit flags from a vector stroke header.
///
/// Unknown bits are deliberately retained for forward compatibility.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VectorStrokeFlags(u32);

impl VectorStrokeFlags {
    /// A normal drawable stroke.
    pub const NORMAL: Self = Self(0x0001);
    /// A vector-ruler stroke.
    pub const RULER: Self = Self(0x0004);
    /// A frame-border stroke.
    pub const FRAME: Self = Self(0x0008);
    /// Straight-line geometry.
    pub const STRAIGHT: Self = Self(0x0010);
    /// Quadratic Bezier geometry.
    pub const QUADRATIC: Self = Self(0x0020);
    /// Cubic Bezier geometry.
    pub const CUBIC: Self = Self(0x0040);
    /// CLIP spline geometry.
    pub const SPLINE: Self = Self(0x0080);
    /// A closed stroke.
    pub const CLOSED: Self = Self(0x0100);
    /// An observed frame-related flag whose exact semantics remain unknown.
    pub const FRAME_FEATURE: Self = Self(0x1000);
    /// An observed freehand-pen flag.
    pub const FREE_PEN: Self = Self(0x2000);

    const KNOWN_BITS: u32 = Self::NORMAL.0
        | Self::RULER.0
        | Self::FRAME.0
        | Self::STRAIGHT.0
        | Self::QUADRATIC.0
        | Self::CUBIC.0
        | Self::SPLINE.0
        | Self::CLOSED.0
        | Self::FRAME_FEATURE.0
        | Self::FREE_PEN.0;

    /// Creates flags while retaining every bit from the file.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Complete unmodified flag value.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Whether all bits in `flag` are present.
    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    /// Bits not currently assigned a public meaning.
    #[must_use]
    pub const fn unknown_bits(self) -> u32 {
        self.0 & !Self::KNOWN_BITS
    }

    /// Known stroke category, or `None` when it is absent or ambiguous.
    #[must_use]
    pub const fn kind(self) -> Option<VectorStrokeKind> {
        match self.0 & (Self::NORMAL.0 | Self::RULER.0 | Self::FRAME.0) {
            value if value == Self::NORMAL.0 => Some(VectorStrokeKind::Normal),
            value if value == Self::RULER.0 => Some(VectorStrokeKind::Ruler),
            value if value == Self::FRAME.0 => Some(VectorStrokeKind::Frame),
            _ => None,
        }
    }

    /// Known curve representation, or `None` when it is absent or ambiguous.
    #[must_use]
    pub const fn curve_kind(self) -> Option<VectorCurveKind> {
        match self.0 & (Self::STRAIGHT.0 | Self::QUADRATIC.0 | Self::CUBIC.0 | Self::SPLINE.0) {
            value if value == Self::STRAIGHT.0 => Some(VectorCurveKind::Straight),
            value if value == Self::QUADRATIC.0 => Some(VectorCurveKind::Quadratic),
            value if value == Self::CUBIC.0 => Some(VectorCurveKind::Cubic),
            value if value == Self::SPLINE.0 => Some(VectorCurveKind::Spline),
            _ => None,
        }
    }

    /// Whether the closed-stroke bit is present.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        self.contains(Self::CLOSED)
    }
}

/// Confirmed high-level category encoded in stroke flags.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum VectorStrokeKind {
    /// A normal drawable vector stroke.
    Normal,
    /// Vector geometry owned by a ruler.
    Ruler,
    /// A frame-border stroke.
    Frame,
}

/// Confirmed curve representation encoded in stroke flags.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum VectorCurveKind {
    /// Straight segments between positions.
    Straight,
    /// Quadratic Bezier segments.
    Quadratic,
    /// Cubic Bezier segments.
    Cubic,
    /// CLIP's spline representation.
    Spline,
}

/// A lossless RGB color stored in a vector stroke header.
///
/// CLIP stores each channel as a 32-bit word. The high byte is the observed
/// 8-bit color component, while the complete word remains available so that
/// lower-bit patterns are not discarded.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VectorColor {
    red_raw: u32,
    green_raw: u32,
    blue_raw: u32,
}

impl VectorColor {
    /// Complete stored red-channel word.
    #[must_use]
    pub const fn red_raw(self) -> u32 {
        self.red_raw
    }

    /// Complete stored green-channel word.
    #[must_use]
    pub const fn green_raw(self) -> u32 {
        self.green_raw
    }

    /// Complete stored blue-channel word.
    #[must_use]
    pub const fn blue_raw(self) -> u32 {
        self.blue_raw
    }

    /// Observed 8-bit red component from the high byte.
    #[must_use]
    pub const fn red_8bit(self) -> u8 {
        (self.red_raw >> 24) as u8
    }

    /// Observed 8-bit green component from the high byte.
    #[must_use]
    pub const fn green_8bit(self) -> u8 {
        (self.green_raw >> 24) as u8
    }

    /// Observed 8-bit blue component from the high byte.
    #[must_use]
    pub const fn blue_8bit(self) -> u8 {
        (self.blue_raw >> 24) as u8
    }

    /// Observed 8-bit RGB triplet.
    #[must_use]
    pub const fn rgb_8bit(self) -> [u8; 3] {
        [self.red_8bit(), self.green_8bit(), self.blue_8bit()]
    }
}

impl From<DecodedColor> for VectorColor {
    fn from(value: DecodedColor) -> Self {
        Self {
            red_raw: value.red_raw,
            green_raw: value.green_raw,
            blue_raw: value.blue_raw,
        }
    }
}

/// Confirmed style fields from one supported vector stroke header.
///
/// This describes stored stroke metadata, not a full CLIP brush renderer.
/// Texture, shape, blending, and other brush behavior can require additional
/// application data and remain available only through raw records.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VectorStrokeStyle {
    kind: VectorStrokeKind,
    main_color: VectorColor,
    sub_color: VectorColor,
    opacity: f64,
    base_brush_id: u32,
    brush_id: u32,
    brush_radius: Option<f64>,
    fill_style_id: Option<u32>,
    modern_normal_extra: Option<u32>,
}

impl VectorStrokeStyle {
    /// Stroke category used to select this header interpretation.
    #[must_use]
    pub const fn kind(self) -> VectorStrokeKind {
        self.kind
    }

    /// Primary drawing color.
    #[must_use]
    pub const fn main_color(self) -> VectorColor {
        self.main_color
    }

    /// Secondary drawing color.
    #[must_use]
    pub const fn sub_color(self) -> VectorColor {
        self.sub_color
    }

    /// Stored stroke opacity in the inclusive range `0.0..=1.0`.
    #[must_use]
    pub const fn opacity(self) -> f64 {
        self.opacity
    }

    /// Brush identifier in the common header field.
    ///
    /// For frame strokes, [`Self::brush_id`] returns the effective frame-brush
    /// identifier while this method retains the common field separately.
    #[must_use]
    pub const fn base_brush_id(self) -> u32 {
        self.base_brush_id
    }

    /// Effective brush identifier for this stroke.
    #[must_use]
    pub const fn brush_id(self) -> u32 {
        self.brush_id
    }

    /// Stored brush radius in canvas units, when this header has one.
    #[must_use]
    pub const fn brush_radius(self) -> Option<f64> {
        self.brush_radius
    }

    /// Finite nominal round-stroke diameter in canvas units.
    ///
    /// This is useful as an SVG `stroke-width` approximation, but it does not
    /// reproduce the selected CLIP brush shape or texture. `None` means the
    /// radius is absent or doubling it cannot be represented as a finite `f64`.
    #[must_use]
    pub fn brush_diameter(self) -> Option<f64> {
        self.brush_radius.and_then(|radius| {
            let diameter = radius * 2.0;
            diameter.is_finite().then_some(diameter)
        })
    }

    /// Frame fill-style identifier, when this is a frame stroke.
    #[must_use]
    pub const fn fill_style_id(self) -> Option<u32> {
        self.fill_style_id
    }

    /// Unnamed extra word present in the observed 92-byte normal header.
    #[must_use]
    pub const fn modern_normal_extra(self) -> Option<u32> {
        self.modern_normal_extra
    }
}

impl From<DecodedStrokeKind> for VectorStrokeKind {
    fn from(value: DecodedStrokeKind) -> Self {
        match value {
            DecodedStrokeKind::Normal => Self::Normal,
            DecodedStrokeKind::Ruler => Self::Ruler,
            DecodedStrokeKind::Frame => Self::Frame,
        }
    }
}

impl From<DecodedStrokeStyle> for VectorStrokeStyle {
    fn from(value: DecodedStrokeStyle) -> Self {
        Self {
            kind: value.kind.into(),
            main_color: value.main_color.into(),
            sub_color: value.sub_color.into(),
            opacity: value.stroke_opacity,
            base_brush_id: value.base_brush_id,
            brush_id: value.brush_id,
            brush_radius: value.brush_radius,
            fill_style_id: value.fill_style_id,
            modern_normal_extra: value.modern_normal_extra,
        }
    }
}

/// Confirmed per-point factors from a supported vector record.
///
/// These stored values affect width and opacity. They are deliberately not
/// named tablet pressure because the source pressure itself has not been
/// independently identified in the format.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct VectorPointFactors {
    width_factor: f32,
    opacity_factor: f32,
}

impl VectorPointFactors {
    /// Non-negative multiplier applied to the stroke's nominal width.
    #[must_use]
    pub const fn width_factor(self) -> f32 {
        self.width_factor
    }

    /// Opacity multiplier in the inclusive range `0.0..=1.0`.
    #[must_use]
    pub const fn opacity_factor(self) -> f32 {
        self.opacity_factor
    }
}

impl From<DecodedPointFactors> for VectorPointFactors {
    fn from(value: DecodedPointFactors) -> Self {
        Self {
            width_factor: value.width_factor,
            opacity_factor: value.opacity_factor,
        }
    }
}

/// Raw bit flags from one vector point record.
///
/// Unknown bits are deliberately retained for forward compatibility.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct VectorPointFlags(u32);

impl VectorPointFlags {
    /// The observed corner-point bit.
    pub const CORNER: Self = Self(0x0001);

    /// Creates flags while retaining every bit from the file.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Complete unmodified flag value.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Whether all bits in `flag` are present.
    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    /// Whether the observed corner-point bit is present.
    #[must_use]
    pub const fn is_corner(self) -> bool {
        self.contains(Self::CORNER)
    }

    /// Bits whose meaning is not currently known.
    #[must_use]
    pub const fn unknown_bits(self) -> u32 {
        self.0 & !Self::CORNER.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedVectorStroke {
    start: usize,
    end: usize,
    point_count: u32,
    layout: VectorLayout,
}

/// An owned, structurally validated vector external body.
///
/// Parsing records their declared boundaries but preserves the complete body,
/// including fields and layouts whose meaning is not yet known.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VectorData {
    raw: Vec<u8>,
    strokes: Vec<ParsedVectorStroke>,
}

impl VectorData {
    /// Complete unmodified external-body bytes.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Number of validated stroke records.
    #[must_use]
    pub fn stroke_count(&self) -> usize {
        self.strokes.len()
    }

    /// Whether the body contains no stroke records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    /// Returns one stroke by external-body order.
    #[must_use]
    pub fn stroke(&self, index: usize) -> Option<VectorStroke<'_>> {
        self.strokes
            .get(index)
            .map(|range| VectorStroke::from_parsed(&self.raw, range))
    }

    /// Iterates over strokes in external-body order.
    #[must_use]
    pub fn strokes(&self) -> VectorStrokes<'_> {
        VectorStrokes {
            raw: &self.raw,
            ranges: self.strokes.iter(),
        }
    }
}

impl<'a> IntoIterator for &'a VectorData {
    type Item = VectorStroke<'a>;
    type IntoIter = VectorStrokes<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.strokes()
    }
}

/// Borrowed iterator over the strokes in [`VectorData`].
#[derive(Clone, Debug)]
pub struct VectorStrokes<'a> {
    raw: &'a [u8],
    ranges: std::slice::Iter<'a, ParsedVectorStroke>,
}

impl<'a> Iterator for VectorStrokes<'a> {
    type Item = VectorStroke<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.ranges
            .next()
            .map(|range| VectorStroke::from_parsed(self.raw, range))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ranges.size_hint()
    }
}

impl DoubleEndedIterator for VectorStrokes<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.ranges
            .next_back()
            .map(|range| VectorStroke::from_parsed(self.raw, range))
    }
}

impl ExactSizeIterator for VectorStrokes<'_> {}
impl FusedIterator for VectorStrokes<'_> {}

/// A borrowed view of one validated vector stroke record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VectorStroke<'a> {
    raw: &'a [u8],
    layout: VectorLayout,
    point_count: u32,
}

impl<'a> VectorStroke<'a> {
    fn from_parsed(body: &'a [u8], parsed: &ParsedVectorStroke) -> Self {
        Self {
            raw: &body[parsed.start..parsed.end],
            layout: parsed.layout,
            point_count: parsed.point_count,
        }
    }

    /// Sizes declared by this stroke header.
    #[must_use]
    pub const fn layout(&self) -> VectorLayout {
        self.layout
    }

    /// Number of point records declared by this stroke.
    #[must_use]
    pub const fn point_count(&self) -> u32 {
        self.point_count
    }

    /// Complete unmodified stroke bytes, including its point records.
    #[must_use]
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// Complete unmodified stroke-header bytes.
    #[must_use]
    pub fn raw_header(&self) -> &'a [u8] {
        &self.raw[..self.layout.header_size as usize]
    }

    /// Complete unmodified point-section bytes.
    #[must_use]
    pub fn raw_points(&self) -> &'a [u8] {
        &self.raw[self.layout.header_size as usize..]
    }

    /// Raw candidate stroke flags when the header contains the observed field.
    ///
    /// This returns the stored word even for an unsupported layout. Use
    /// [`Self::is_semantically_supported`] before assigning the known flag
    /// meanings to the surrounding record.
    #[must_use]
    pub fn flags(&self) -> Option<VectorStrokeFlags> {
        read_be_u32_at(self.raw_header(), VECTOR_STROKE_FLAGS_OFFSET)
            .map(VectorStrokeFlags::from_raw)
    }

    fn semantic_flags(&self) -> Option<u32> {
        let flags = self.flags()?.raw();
        is_supported_semantic_layout(self.layout.semantic(), flags).then_some(flags)
    }

    /// Whether this record's header, point layout, kind, and curve flags have
    /// a supported semantic interpretation.
    ///
    /// Support composes independently observed header/kind and point/curve
    /// correlations, so it is not the same question as whether the exact size
    /// tuple was seen by [`VectorLayout::is_observed`].
    ///
    /// `false` does not make the stroke invalid. Its declared boundaries and
    /// raw bytes remain available without guessing at field meanings.
    #[must_use]
    pub fn is_semantically_supported(&self) -> bool {
        self.semantic_flags().is_some()
    }

    /// Known stroke category for a supported layout, or `None` otherwise.
    #[must_use]
    pub fn kind(&self) -> Option<VectorStrokeKind> {
        self.semantic_flags()
            .map(VectorStrokeFlags::from_raw)
            .and_then(VectorStrokeFlags::kind)
    }

    /// Known curve representation for a supported layout, or `None` otherwise.
    #[must_use]
    pub fn curve_kind(&self) -> Option<VectorCurveKind> {
        self.semantic_flags()
            .map(VectorStrokeFlags::from_raw)
            .and_then(VectorStrokeFlags::curve_kind)
    }

    /// Whether a supported layout has the closed-stroke bit.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.semantic_flags()
            .map(VectorStrokeFlags::from_raw)
            .is_some_and(VectorStrokeFlags::is_closed)
    }

    /// Stored stroke bounding rectangle for a supported semantic layout.
    #[must_use]
    pub fn bounds(&self) -> Option<VectorBounds> {
        self.semantic_flags()?;
        read_vector_bounds(self.raw_header(), VECTOR_STROKE_BOUNDS_OFFSET)
    }

    /// Confirmed colors, opacity, and brush metadata for a supported layout.
    ///
    /// Unsupported or ambiguous records return `None` and remain accessible
    /// through [`Self::raw_header`]. All returned numeric values were validated
    /// when the owning [`VectorData`] was read.
    #[must_use]
    pub fn style(&self) -> Option<VectorStrokeStyle> {
        let flags = self.semantic_flags()?;
        let decoded = decode_stroke_style(self.raw_header(), self.layout.semantic(), flags)
            .expect("validated vector stroke style must remain valid")
            .expect("supported vector stroke layout must have a style");
        Some(decoded.into())
    }

    /// Returns one point by record order.
    #[must_use]
    pub fn point(&self, index: usize) -> Option<VectorPoint<'a>> {
        let index = u32::try_from(index).ok()?;
        if index >= self.point_count {
            return None;
        }
        let point_size = self.layout.point_size as usize;
        let start = index as usize * point_size;
        Some(VectorPoint {
            raw: &self.raw_points()[start..start + point_size],
            layout: self.layout,
            stroke_flags: self.semantic_flags(),
        })
    }

    /// Iterates over point records in their stored order.
    #[must_use]
    pub fn points(&self) -> VectorPoints<'a> {
        VectorPoints {
            raw: self.raw_points(),
            layout: self.layout,
            stroke_flags: self.semantic_flags(),
            front: 0,
            back: self.point_count,
        }
    }
}

/// Borrowed iterator over the points in one [`VectorStroke`].
#[derive(Clone, Debug)]
pub struct VectorPoints<'a> {
    raw: &'a [u8],
    layout: VectorLayout,
    stroke_flags: Option<u32>,
    front: u32,
    back: u32,
}

impl<'a> Iterator for VectorPoints<'a> {
    type Item = VectorPoint<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        let index = self.front;
        self.front += 1;
        Some(self.point_at(index))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.back - self.front) as usize;
        (remaining, Some(remaining))
    }
}

impl DoubleEndedIterator for VectorPoints<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        Some(self.point_at(self.back))
    }
}

impl ExactSizeIterator for VectorPoints<'_> {}
impl FusedIterator for VectorPoints<'_> {}

impl<'a> VectorPoints<'a> {
    fn point_at(&self, index: u32) -> VectorPoint<'a> {
        let point_size = self.layout.point_size as usize;
        let start = index as usize * point_size;
        VectorPoint {
            raw: &self.raw[start..start + point_size],
            layout: self.layout,
            stroke_flags: self.stroke_flags,
        }
    }
}

/// A borrowed view of one validated vector point record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VectorPoint<'a> {
    raw: &'a [u8],
    layout: VectorLayout,
    stroke_flags: Option<u32>,
}

impl<'a> VectorPoint<'a> {
    /// Complete unmodified point-record bytes.
    #[must_use]
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// Layout inherited from the owning stroke.
    #[must_use]
    pub const fn layout(&self) -> VectorLayout {
        self.layout
    }

    /// Whether the owning stroke has a supported semantic interpretation.
    #[must_use]
    pub const fn is_semantically_supported(&self) -> bool {
        self.stroke_flags.is_some()
    }

    /// Finite point position for a supported semantic layout.
    #[must_use]
    pub fn position(&self) -> Option<VectorPosition> {
        self.stroke_flags?;
        read_vector_position(self.raw, 0)
    }

    /// Stored point bounding rectangle for a supported semantic layout.
    #[must_use]
    pub fn bounds(&self) -> Option<VectorBounds> {
        self.stroke_flags?;
        read_vector_bounds(self.raw, VECTOR_POINT_BOUNDS_OFFSET)
    }

    /// Raw point flags for a supported semantic layout.
    #[must_use]
    pub fn flags(&self) -> Option<VectorPointFlags> {
        self.stroke_flags?;
        read_be_u32_at(self.raw, VECTOR_POINT_FLAGS_OFFSET).map(VectorPointFlags::from_raw)
    }

    /// Confirmed width and opacity factors for a supported semantic layout.
    ///
    /// These values should not be interpreted as raw tablet pressure.
    #[must_use]
    pub fn factors(&self) -> Option<VectorPointFactors> {
        let flags = self.stroke_flags?;
        let decoded = decode_point_factors(self.raw, self.layout.semantic(), flags)
            .expect("validated vector point factors must remain valid")
            .expect("supported vector point layout must have factors");
        Some(decoded.into())
    }

    /// Confirmed non-negative point width factor.
    #[must_use]
    pub fn width_factor(&self) -> Option<f32> {
        self.factors().map(VectorPointFactors::width_factor)
    }

    /// Confirmed point opacity factor in the inclusive range `0.0..=1.0`.
    #[must_use]
    pub fn opacity_factor(&self) -> Option<f32> {
        self.factors().map(VectorPointFactors::opacity_factor)
    }

    /// Iterates over the Bezier controls stored for this point.
    ///
    /// A quadratic point has one control and a cubic point has two. These
    /// controls belong to the segment starting at this point and ending at the
    /// next point; a closed stroke's final point controls the closing segment.
    /// Straight and spline records return an empty iterator.
    #[must_use]
    pub fn control_points(&self) -> VectorControlPoints<'a> {
        let count = match (self.stroke_flags, self.layout.point_size) {
            (Some(_), 104) => 1,
            (Some(_), 120) => 2,
            _ => 0,
        };
        VectorControlPoints {
            raw: self.raw,
            front: 0,
            back: count,
        }
    }
}

/// Borrowed iterator over the Bezier controls of a supported point.
#[derive(Clone, Debug)]
pub struct VectorControlPoints<'a> {
    raw: &'a [u8],
    front: u8,
    back: u8,
}

impl Iterator for VectorControlPoints<'_> {
    type Item = VectorPosition;

    fn next(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        let index = self.front;
        self.front += 1;
        Some(self.position_at(index))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.back - self.front);
        (remaining, Some(remaining))
    }
}

impl DoubleEndedIterator for VectorControlPoints<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        Some(self.position_at(self.back))
    }
}

impl ExactSizeIterator for VectorControlPoints<'_> {}
impl FusedIterator for VectorControlPoints<'_> {}

impl VectorControlPoints<'_> {
    fn position_at(&self, index: u8) -> VectorPosition {
        let offset = VECTOR_POINT_BASE_SIZE + usize::from(index) * VECTOR_CONTROL_POINT_SIZE;
        read_vector_position(self.raw, offset)
            .expect("validated control-point coordinates must remain present and finite")
    }
}

#[cfg(feature = "write")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VectorStrokeRange {
    start: usize,
    end: usize,
    point_count: u32,
}

/// One `VectorObjectList` row and its opaque external-object identifier.
///
/// This preserves the ownership metadata needed to retrieve either a typed
/// [`VectorData`] value or its bounded raw external body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VectorDataSource {
    id: i64,
    canvas_id: i64,
    layer_id: i64,
    external_identifier: Box<[u8]>,
}

/// Result of translating every point in an existing supported vector body.
#[cfg(feature = "write")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VectorTranslationSummary {
    strokes: u32,
    points: u32,
    delta_x: i32,
    delta_y: i32,
}

#[cfg(feature = "write")]
impl VectorTranslationSummary {
    /// Number of validated stroke records.
    #[must_use]
    pub const fn strokes(self) -> u32 {
        self.strokes
    }

    /// Number of translated points.
    #[must_use]
    pub const fn points(self) -> u32 {
        self.points
    }

    /// Horizontal translation in canvas units.
    #[must_use]
    pub const fn delta_x(self) -> i32 {
        self.delta_x
    }

    /// Vertical translation in canvas units.
    #[must_use]
    pub const fn delta_y(self) -> i32 {
        self.delta_y
    }
}

impl VectorDataSource {
    /// `VectorObjectList.MainId`.
    #[must_use]
    pub const fn id(&self) -> i64 {
        self.id
    }

    /// Owning canvas ID.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Owning layer ID.
    #[must_use]
    pub const fn layer_id(&self) -> i64 {
        self.layer_id
    }

    /// Opaque identifier used by the SQLite external-object index.
    #[must_use]
    pub fn external_identifier(&self) -> &[u8] {
        &self.external_identifier
    }
}

impl Database {
    /// Reads all vector-data references owned by one layer.
    ///
    /// Files without a `VectorObjectList` table return an empty vector. The
    /// raw external body can be obtained with [`ClipFile::read_vector_data`].
    pub fn vector_data_sources(
        &self,
        layer_id: i64,
        limits: Limits,
    ) -> Result<Vec<VectorDataSource>> {
        if self.schema().table("VectorObjectList").is_none() {
            return Ok(Vec::new());
        }
        for column in ["MainId", "CanvasId", "LayerId", "VectorData"] {
            self.require_column("VectorObjectList", column)?;
        }
        let mut statement = self.connection().prepare(
            "SELECT MainId, CanvasId, LayerId, VectorData \
             FROM VectorObjectList WHERE LayerId = ?1 ORDER BY MainId",
        )?;
        let mut rows = statement.query(params![layer_id])?;
        let mut sources = Vec::new();
        while let Some(row) = rows.next()? {
            let next_count = u64::try_from(sources.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            if next_count > limits.max_vector_objects() {
                return Err(Error::LimitExceeded {
                    resource: "vector objects per layer",
                    value: next_count,
                    limit: limits.max_vector_objects(),
                });
            }
            sources.push(VectorDataSource {
                id: row.get(0)?,
                canvas_id: row.get(1)?,
                layer_id: row.get(2)?,
                external_identifier: value_bytes(
                    row.get_ref(3)?,
                    3,
                    "VectorData",
                    limits.max_identifier_size(),
                )?,
            });
        }
        Ok(sources)
    }
}

impl<R: Read + Seek> ClipFile<R> {
    /// Resolves and reads one opaque vector-data external body under a limit.
    pub fn read_vector_data(
        &mut self,
        database: &Database,
        source: &VectorDataSource,
        limits: Limits,
    ) -> Result<Vec<u8>> {
        let identifier = source.external_identifier();
        let object = self
            .resolve_external_object(database, identifier)?
            .ok_or_else(|| Error::InvalidDatabase {
                reason: format!(
                    "VectorObjectList row {} references a missing external object",
                    source.id()
                ),
            })?;
        self.read_external_body(&object, limits.max_vector_data_bytes())
    }

    /// Resolves and structurally validates one vector-data external body.
    ///
    /// The parser bounds every declared stroke and point record, validates
    /// exposed semantic values for supported layouts, and applies the vector
    /// stroke and point limits. An empty external body is valid and has no
    /// strokes. Unknown layouts remain available through the raw accessors on
    /// [`VectorData`], [`VectorStroke`], and [`VectorPoint`].
    pub fn read_vector(
        &mut self,
        database: &Database,
        source: &VectorDataSource,
        limits: Limits,
    ) -> Result<VectorData> {
        let raw = self.read_vector_data(database, source, limits)?;
        parse_vector_data(raw, limits)
    }
}

#[cfg(feature = "write")]
impl<R: Read + Seek> ClipWriter<'_, R> {
    /// Replaces the complete opaque body referenced by a validated vector row.
    ///
    /// The `VectorObjectList` row and external identifier must still match the
    /// supplied [`VectorDataSource`]. The body format is not interpreted, so
    /// this is intentionally an opaque boundary rather than a stroke encoder.
    /// Container offsets are repaired by [`Self::write_to`](ClipWriter::write_to).
    pub fn replace_vector_data_body(
        &mut self,
        source: &VectorDataSource,
        body: impl Into<Vec<u8>>,
        limits: Limits,
    ) -> Result<Option<Vec<u8>>> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = body.into();
        if body.len() as u64 > limits.max_vector_data_bytes() {
            return Err(Error::LimitExceeded {
                resource: "replacement vector data bytes",
                value: body.len() as u64,
                limit: limits.max_vector_data_bytes(),
            });
        }
        self.source_external_object(source.external_identifier())?;
        self.replace_external_body(source.external_identifier(), body)
    }

    /// Translates every point and bounding box in an existing vector body.
    ///
    /// This semantic edit is intentionally limited to the strictly validated
    /// 92-byte stroke header and 88-byte point layout observed in current
    /// generated documents. Unsupported layouts are rejected without
    /// installing a replacement, while brush data, width/opacity factors,
    /// flags, and all unknown bytes are preserved exactly. Parsed records are
    /// bounded by [`Limits::max_vector_objects`].
    pub fn translate_vector_data(
        &mut self,
        source: &VectorDataSource,
        delta_x: i32,
        delta_y: i32,
        limits: Limits,
    ) -> Result<VectorTranslationSummary> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = self.external_body_for_update(
            source.external_identifier(),
            limits.max_vector_data_bytes(),
        )?;
        if body.len() as u64 > limits.max_vector_data_bytes() {
            return Err(Error::LimitExceeded {
                resource: "vector data bytes",
                value: body.len() as u64,
                limit: limits.max_vector_data_bytes(),
            });
        }
        let (translated, summary) =
            translate_supported_vector_body(&body, delta_x, delta_y, limits)?;
        if translated != body {
            self.replace_external_body(source.external_identifier(), translated)?;
        }
        Ok(summary)
    }

    /// Appends a translated clone of one existing supported vector stroke.
    ///
    /// `template_stroke_index` addresses a stroke in external-body order. The
    /// complete 92-byte header and every complete 88-byte point record are
    /// cloned, then only point positions and stroke/point bounding boxes are
    /// translated. Brush attributes, width/opacity factors, flags, and all
    /// opaque fields are retained byte-for-byte.
    ///
    /// The clone is appended to the same [`VectorDataSource`]. This does not
    /// synthesize a brush or a stroke header from zero. The returned index is
    /// the appended stroke's external-body order. Parsed stroke records are
    /// bounded by [`Limits::max_vector_objects`].
    pub fn clone_vector_stroke(
        &mut self,
        source: &VectorDataSource,
        template_stroke_index: usize,
        delta_x: i32,
        delta_y: i32,
        limits: Limits,
    ) -> Result<(usize, VectorTranslationSummary)> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = self.external_body_for_update(
            source.external_identifier(),
            limits.max_vector_data_bytes(),
        )?;
        ensure_vector_body_within_limit(&body, limits, "vector data bytes")?;
        let (replacement, appended_index, summary) =
            clone_supported_vector_stroke(&body, template_stroke_index, delta_x, delta_y, limits)?;
        self.replace_external_body(source.external_identifier(), replacement)?;
        Ok((appended_index, summary))
    }

    /// Removes one existing supported vector stroke from external-body order.
    ///
    /// All bytes before and after the selected record are retained exactly.
    /// Removing the final stroke produces an empty external body. Saved render
    /// caches are separate external objects and are intentionally unchanged.
    /// Further semantic stroke operations reject that empty body until it is
    /// replaced with a supported body. Parsed stroke records are bounded by
    /// [`Limits::max_vector_objects`].
    pub fn remove_vector_stroke(
        &mut self,
        source: &VectorDataSource,
        stroke_index: usize,
        limits: Limits,
    ) -> Result<u32> {
        validate_vector_source(
            self.database().connection(),
            self.database().schema(),
            source,
        )?;
        let body = self.external_body_for_update(
            source.external_identifier(),
            limits.max_vector_data_bytes(),
        )?;
        ensure_vector_body_within_limit(&body, limits, "vector data bytes")?;
        let (replacement, removed_points) =
            remove_supported_vector_stroke(&body, stroke_index, limits)?;
        self.replace_external_body(source.external_identifier(), replacement)?;
        Ok(removed_points)
    }
}

#[cfg(feature = "write")]
fn translate_supported_vector_body(
    body: &[u8],
    delta_x: i32,
    delta_y: i32,
    limits: Limits,
) -> Result<(Vec<u8>, VectorTranslationSummary)> {
    let ranges = supported_vector_stroke_ranges(body, limits)?;
    let mut output = body.to_vec();
    let mut points = 0_u32;
    for range in &ranges {
        translate_vector_bbox(&mut output, range.start + 24, delta_x, delta_y)?;
        for point_index in 0..range.point_count {
            let point_offset = range
                .start
                .checked_add(SUPPORTED_VECTOR_HEADER_SIZE)
                .and_then(|value| {
                    value.checked_add(
                        usize::try_from(point_index)
                            .ok()?
                            .checked_mul(SUPPORTED_VECTOR_POINT_SIZE)?,
                    )
                })
                .ok_or(Error::OffsetOverflow)?;
            translate_vector_f64(&mut output, point_offset, f64::from(delta_x))?;
            translate_vector_f64(&mut output, point_offset + 8, f64::from(delta_y))?;
            translate_vector_bbox(&mut output, point_offset + 16, delta_x, delta_y)?;
        }
        points = points
            .checked_add(range.point_count)
            .ok_or(Error::OffsetOverflow)?;
    }
    let strokes = u32::try_from(ranges.len()).map_err(|_| Error::OffsetOverflow)?;
    Ok((
        output,
        VectorTranslationSummary {
            strokes,
            points,
            delta_x,
            delta_y,
        },
    ))
}

#[cfg(feature = "write")]
fn supported_vector_stroke_ranges(body: &[u8], limits: Limits) -> Result<Vec<VectorStrokeRange>> {
    let mut ranges = Vec::new();
    let mut offset = 0_usize;
    while offset < body.len() {
        let header_size = read_vector_u32(body, offset)?;
        let point_section_size = read_vector_u32(body, offset + 4)?;
        let point_size = read_vector_u32(body, offset + 8)?;
        let default_point_size = read_vector_u32(body, offset + 12)?;
        if header_size != SUPPORTED_VECTOR_HEADER_SIZE as u32
            || point_section_size != SUPPORTED_VECTOR_POINT_SECTION_SIZE
            || point_size != SUPPORTED_VECTOR_POINT_SIZE as u32
            || default_point_size != SUPPORTED_VECTOR_POINT_SIZE as u32
        {
            return Err(Error::InvalidWrite {
                reason: format!(
                    "unsupported vector layout at byte {offset}: header={header_size}, point-section={point_section_size}, point={point_size}, default-point={default_point_size}"
                ),
            });
        }
        let point_count = read_vector_u32(body, offset + 16)?;
        let points_size = usize::try_from(point_count)
            .map_err(|_| Error::OffsetOverflow)?
            .checked_mul(SUPPORTED_VECTOR_POINT_SIZE)
            .ok_or(Error::OffsetOverflow)?;
        let end = offset
            .checked_add(SUPPORTED_VECTOR_HEADER_SIZE)
            .and_then(|value| value.checked_add(points_size))
            .ok_or(Error::OffsetOverflow)?;
        if end > body.len() {
            return Err(Error::InvalidWrite {
                reason: format!("vector stroke at byte {offset} exceeds its external body"),
            });
        }
        let next_count = u64::try_from(ranges.len())
            .map_err(|_| Error::OffsetOverflow)?
            .checked_add(1)
            .ok_or(Error::OffsetOverflow)?;
        if next_count > limits.max_vector_objects() {
            return Err(Error::LimitExceeded {
                resource: "vector stroke records",
                value: next_count,
                limit: limits.max_vector_objects(),
            });
        }
        ranges.push(VectorStrokeRange {
            start: offset,
            end,
            point_count,
        });
        offset = end;
    }
    if ranges.is_empty() {
        return Err(Error::InvalidWrite {
            reason: "vector body contains no supported stroke records".to_owned(),
        });
    }
    Ok(ranges)
}

#[cfg(feature = "write")]
fn clone_supported_vector_stroke(
    body: &[u8],
    template_stroke_index: usize,
    delta_x: i32,
    delta_y: i32,
    limits: Limits,
) -> Result<(Vec<u8>, usize, VectorTranslationSummary)> {
    let ranges = supported_vector_stroke_ranges(body, limits)?;
    let appended_index = ranges.len();
    let template = ranges
        .get(template_stroke_index)
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!(
                "vector template stroke index {template_stroke_index} is out of range for {} strokes",
                ranges.len()
            ),
        })?;
    let resulting_count = u64::try_from(ranges.len())
        .map_err(|_| Error::OffsetOverflow)?
        .checked_add(1)
        .ok_or(Error::OffsetOverflow)?;
    if resulting_count > limits.max_vector_objects() {
        return Err(Error::LimitExceeded {
            resource: "vector stroke records after clone",
            value: resulting_count,
            limit: limits.max_vector_objects(),
        });
    }
    if template.point_count == 0 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "vector template stroke index {template_stroke_index} contains no points"
            ),
        });
    }
    let template_bytes = body
        .get(template.start..template.end)
        .ok_or(Error::OffsetOverflow)?;
    let replacement_size = body
        .len()
        .checked_add(template_bytes.len())
        .ok_or(Error::OffsetOverflow)?;
    let replacement_size_u64 =
        u64::try_from(replacement_size).map_err(|_| Error::OffsetOverflow)?;
    if replacement_size_u64 > limits.max_vector_data_bytes() {
        return Err(Error::LimitExceeded {
            resource: "replacement vector data bytes",
            value: replacement_size_u64,
            limit: limits.max_vector_data_bytes(),
        });
    }
    let (translated, summary) =
        translate_supported_vector_body(template_bytes, delta_x, delta_y, limits)?;
    let mut replacement = Vec::with_capacity(replacement_size);
    replacement.extend_from_slice(body);
    replacement.extend_from_slice(&translated);
    Ok((replacement, appended_index, summary))
}

#[cfg(feature = "write")]
fn remove_supported_vector_stroke(
    body: &[u8],
    stroke_index: usize,
    limits: Limits,
) -> Result<(Vec<u8>, u32)> {
    let ranges = supported_vector_stroke_ranges(body, limits)?;
    let removed = ranges
        .get(stroke_index)
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!(
                "vector stroke index {stroke_index} is out of range for {} strokes",
                ranges.len()
            ),
        })?;
    let removed_size = removed
        .end
        .checked_sub(removed.start)
        .ok_or(Error::OffsetOverflow)?;
    let replacement_size = body
        .len()
        .checked_sub(removed_size)
        .ok_or(Error::OffsetOverflow)?;
    let mut replacement = Vec::with_capacity(replacement_size);
    replacement.extend_from_slice(&body[..removed.start]);
    replacement.extend_from_slice(&body[removed.end..]);
    Ok((replacement, removed.point_count))
}

#[cfg(feature = "write")]
fn ensure_vector_body_within_limit(
    body: &[u8],
    limits: Limits,
    resource: &'static str,
) -> Result<()> {
    if body.len() as u64 > limits.max_vector_data_bytes() {
        return Err(Error::LimitExceeded {
            resource,
            value: body.len() as u64,
            limit: limits.max_vector_data_bytes(),
        });
    }
    Ok(())
}

#[cfg(feature = "write")]
fn read_vector_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset.checked_add(4).ok_or(Error::OffsetOverflow)?)
        .ok_or_else(|| Error::InvalidWrite {
            reason: format!("vector record is truncated at byte {offset}"),
        })?;
    Ok(u32::from_be_bytes(
        value.try_into().expect("four-byte slice"),
    ))
}

#[cfg(feature = "write")]
fn translate_vector_f64(bytes: &mut [u8], offset: usize, delta: f64) -> Result<()> {
    let end = offset.checked_add(8).ok_or(Error::OffsetOverflow)?;
    let value = bytes.get(offset..end).ok_or_else(|| Error::InvalidWrite {
        reason: format!("vector coordinate is truncated at byte {offset}"),
    })?;
    let original = f64::from_be_bytes(value.try_into().expect("eight-byte slice"));
    let replacement = original + delta;
    if !original.is_finite() || !replacement.is_finite() {
        return Err(Error::InvalidWrite {
            reason: format!("vector coordinate at byte {offset} is not finite"),
        });
    }
    bytes[offset..end].copy_from_slice(&replacement.to_be_bytes());
    Ok(())
}

#[cfg(feature = "write")]
fn translate_vector_bbox(
    bytes: &mut [u8],
    offset: usize,
    delta_x: i32,
    delta_y: i32,
) -> Result<()> {
    for (index, delta) in [delta_x, delta_y, delta_x, delta_y].into_iter().enumerate() {
        let value_offset = offset
            .checked_add(index.checked_mul(4).ok_or(Error::OffsetOverflow)?)
            .ok_or(Error::OffsetOverflow)?;
        let original = i32::from_be_bytes(
            bytes
                .get(value_offset..value_offset + 4)
                .ok_or_else(|| Error::InvalidWrite {
                    reason: format!("vector bounding box is truncated at byte {value_offset}"),
                })?
                .try_into()
                .expect("four-byte slice"),
        );
        let replacement = original
            .checked_add(delta)
            .ok_or_else(|| Error::InvalidWrite {
                reason: "vector bounding-box translation overflows i32".to_owned(),
            })?;
        bytes[value_offset..value_offset + 4].copy_from_slice(&replacement.to_be_bytes());
    }
    Ok(())
}

#[cfg(feature = "write")]
fn validate_vector_source(
    connection: &rusqlite::Connection,
    schema: &DatabaseSchema,
    source: &VectorDataSource,
) -> Result<()> {
    for column in ["MainId", "CanvasId", "LayerId", "VectorData"] {
        if !schema.has_column("VectorObjectList", column) {
            return Err(Error::InvalidWrite {
                reason: format!("VectorObjectList.{column} is required to edit vector data"),
            });
        }
    }
    let row_count: i64 = connection.query_row(
        "SELECT count(*) FROM VectorObjectList WHERE MainId = ?1",
        params![source.id()],
        |row| row.get(0),
    )?;
    if row_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "expected one vector row with ID {}, found {row_count}",
                source.id()
            ),
        });
    }
    let stored = connection
        .query_row(
            "SELECT CanvasId, LayerId, VectorData FROM VectorObjectList \
             WHERE MainId = ?1 LIMIT 1",
            params![source.id()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    match row.get_ref(2)? {
                        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes.to_vec(),
                        value => {
                            return Err(rusqlite::Error::InvalidColumnType(
                                2,
                                "VectorData".to_owned(),
                                value.data_type(),
                            ));
                        }
                    },
                ))
            },
        )
        .optional()?;
    let Some((canvas_id, layer_id, identifier)) = stored else {
        return Err(Error::InvalidWrite {
            reason: format!("vector row {} does not exist", source.id()),
        });
    };
    if canvas_id != source.canvas_id()
        || layer_id != source.layer_id()
        || identifier != source.external_identifier()
    {
        return Err(Error::InvalidWrite {
            reason: format!(
                "vector row {} changed after VectorDataSource was read",
                source.id()
            ),
        });
    }
    let alias_count: i64 = connection.query_row(
        "SELECT count(*) FROM VectorObjectList WHERE CAST(VectorData AS BLOB) = ?1",
        params![&identifier],
        |row| row.get(0),
    )?;
    if alias_count != 1 {
        return Err(Error::InvalidWrite {
            reason: format!(
                "vector external identifier is shared by {alias_count} VectorObjectList rows"
            ),
        });
    }
    Ok(())
}

fn parse_vector_data(raw: Vec<u8>, limits: Limits) -> Result<VectorData> {
    let mut strokes = Vec::new();
    let mut offset = 0_usize;
    let mut total_points = 0_u64;

    while offset < raw.len() {
        let remaining = raw.len() - offset;
        if remaining < VECTOR_LAYOUT_PREFIX_SIZE {
            return Err(invalid_vector_data(format!(
                "stroke header at byte {offset} is truncated: {remaining} bytes remain"
            )));
        }

        let layout = VectorLayout {
            header_size: read_required_vector_u32(&raw, offset, "stroke header size")?,
            point_section_size: read_required_vector_u32(&raw, offset + 4, "point-section size")?,
            point_size: read_required_vector_u32(&raw, offset + 8, "point-record size")?,
            default_point_size: read_required_vector_u32(
                &raw,
                offset + 12,
                "default point-record size",
            )?,
        };
        let point_count = read_required_vector_u32(&raw, offset + 16, "point count")?;

        let header_size = usize::try_from(layout.header_size).map_err(|_| Error::OffsetOverflow)?;
        if header_size < VECTOR_LAYOUT_PREFIX_SIZE {
            return Err(invalid_vector_data(format!(
                "stroke at byte {offset} declares a {}-byte header below the {VECTOR_LAYOUT_PREFIX_SIZE}-byte common prefix",
                layout.header_size
            )));
        }

        let next_stroke_count = u64::try_from(strokes.len())
            .map_err(|_| Error::OffsetOverflow)?
            .checked_add(1)
            .ok_or(Error::OffsetOverflow)?;
        if next_stroke_count > limits.max_vector_strokes() {
            return Err(Error::LimitExceeded {
                resource: "vector strokes",
                value: next_stroke_count,
                limit: limits.max_vector_strokes(),
            });
        }

        total_points = total_points
            .checked_add(u64::from(point_count))
            .ok_or(Error::OffsetOverflow)?;
        if total_points > limits.max_vector_points() {
            return Err(Error::LimitExceeded {
                resource: "vector points",
                value: total_points,
                limit: limits.max_vector_points(),
            });
        }

        let point_size = usize::try_from(layout.point_size).map_err(|_| Error::OffsetOverflow)?;
        if point_count != 0 && point_size == 0 {
            return Err(invalid_vector_data(format!(
                "stroke at byte {offset} declares {point_count} zero-byte point records"
            )));
        }
        let point_bytes = usize::try_from(point_count)
            .map_err(|_| Error::OffsetOverflow)?
            .checked_mul(point_size)
            .ok_or(Error::OffsetOverflow)?;
        let end = offset
            .checked_add(header_size)
            .and_then(|value| value.checked_add(point_bytes))
            .ok_or(Error::OffsetOverflow)?;
        if end > raw.len() {
            return Err(invalid_vector_data(format!(
                "stroke at byte {offset} declares an end at byte {end}, beyond the {}-byte body",
                raw.len()
            )));
        }

        let header = &raw[offset..offset + header_size];
        let semantic_flags = read_be_u32_at(header, VECTOR_STROKE_FLAGS_OFFSET)
            .filter(|flags| is_supported_semantic_layout(layout.semantic(), *flags));
        if let Some(flags) = semantic_flags {
            decode_stroke_style(header, layout.semantic(), flags)?
                .expect("supported vector stroke layout must decode a style");
            validate_vector_points(
                &raw[offset + header_size..end],
                point_size,
                point_count,
                offset + header_size,
                layout,
                flags,
            )?;
        }
        strokes.push(ParsedVectorStroke {
            start: offset,
            end,
            point_count,
            layout,
        });
        offset = end;
    }

    Ok(VectorData { raw, strokes })
}

fn validate_vector_points(
    point_bytes: &[u8],
    point_size: usize,
    point_count: u32,
    body_offset: usize,
    layout: VectorLayout,
    stroke_flags: u32,
) -> Result<()> {
    for point_index in 0..point_count {
        let point_start = usize::try_from(point_index)
            .map_err(|_| Error::OffsetOverflow)?
            .checked_mul(point_size)
            .ok_or(Error::OffsetOverflow)?;
        let point = &point_bytes[point_start..point_start + point_size];
        validate_finite_vector_position(point, 0, body_offset + point_start, "point position")?;
        decode_point_factors(point, layout.semantic(), stroke_flags)?
            .expect("supported vector point layout must decode factors");
        decode_control_points(point, layout.semantic(), stroke_flags)?
            .expect("supported vector point layout must decode controls");
    }
    Ok(())
}

fn validate_finite_vector_position(
    bytes: &[u8],
    offset: usize,
    body_offset: usize,
    name: &str,
) -> Result<()> {
    if read_vector_position(bytes, offset).is_none() {
        return Err(invalid_vector_data(format!(
            "{name} at byte {body_offset} is truncated or not finite"
        )));
    }
    Ok(())
}

fn read_required_vector_u32(bytes: &[u8], offset: usize, name: &str) -> Result<u32> {
    read_be_u32_at(bytes, offset)
        .ok_or_else(|| invalid_vector_data(format!("{name} is truncated at byte {offset}")))
}

fn read_be_u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    Some(u32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn read_be_i32_at(bytes: &[u8], offset: usize) -> Option<i32> {
    let end = offset.checked_add(4)?;
    Some(i32::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn read_be_f64_at(bytes: &[u8], offset: usize) -> Option<f64> {
    let end = offset.checked_add(8)?;
    Some(f64::from_be_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn read_vector_position(bytes: &[u8], offset: usize) -> Option<VectorPosition> {
    let x = read_be_f64_at(bytes, offset)?;
    let y = read_be_f64_at(bytes, offset.checked_add(8)?)?;
    (x.is_finite() && y.is_finite()).then_some(VectorPosition { x, y })
}

fn read_vector_bounds(bytes: &[u8], offset: usize) -> Option<VectorBounds> {
    Some(VectorBounds {
        left: read_be_i32_at(bytes, offset)?,
        top: read_be_i32_at(bytes, offset.checked_add(4)?)?,
        right: read_be_i32_at(bytes, offset.checked_add(8)?)?,
        bottom: read_be_i32_at(bytes, offset.checked_add(12)?)?,
    })
}

fn invalid_vector_data(reason: String) -> Error {
    Error::InvalidVectorData { reason }
}

fn value_bytes(value: ValueRef<'_>, column: usize, name: &str, limit: u64) -> Result<Box<[u8]>> {
    let bytes = match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes,
        value => {
            return Err(rusqlite::Error::InvalidColumnType(
                column,
                name.to_owned(),
                value.data_type(),
            )
            .into());
        }
    };
    let size = bytes.len() as u64;
    if size > limit {
        return Err(Error::LimitExceeded {
            resource: "vector external identifier size",
            value: size,
            limit,
        });
    }
    Ok(Box::from(bytes))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use rusqlite::Connection;
    #[cfg(feature = "write")]
    use rusqlite::MAIN_DB;

    use super::*;

    const IDENTIFIER: &[u8] = b"extrnlid0123456789ABCDEF0123456789ABCDEF";

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn push_chunk(bytes: &mut Vec<u8>, tag: &[u8; 8], payload: &[u8]) -> u64 {
        let offset = bytes.len() as u64;
        bytes.extend_from_slice(tag);
        push_u64(bytes, payload.len() as u64);
        bytes.extend_from_slice(payload);
        offset
    }

    fn sample(body: &[u8]) -> (Vec<u8>, u64) {
        let mut bytes = Vec::from(b"CSFCHUNK".as_slice());
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 24);

        let mut header = Vec::new();
        push_u64(&mut header, 256);
        let database_offset_position = header.len();
        push_u64(&mut header, 0);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);
        push_chunk(&mut bytes, b"CHNKHead", &header);

        let mut external = Vec::new();
        push_u64(&mut external, IDENTIFIER.len() as u64);
        external.extend_from_slice(IDENTIFIER);
        push_u64(&mut external, body.len() as u64);
        external.extend_from_slice(body);
        let external_offset = push_chunk(&mut bytes, b"CHNKExta", &external);

        let database_offset = push_chunk(&mut bytes, b"CHNKSQLi", b"db!");
        push_chunk(&mut bytes, b"CHNKFoot", b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        let database_field = 24 + 16 + database_offset_position;
        bytes[database_field..database_field + 8].copy_from_slice(&database_offset.to_be_bytes());
        (bytes, external_offset)
    }

    fn database(external_offset: u64) -> Database {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE VectorObjectList (
                    MainId INTEGER,
                    CanvasId INTEGER,
                    LayerId INTEGER,
                    VectorData BLOB
                 );
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO VectorObjectList VALUES (1, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ExternalChunk VALUES (?1, ?2)",
                params![IDENTIFIER, external_offset as i64],
            )
            .unwrap();
        Database::from_connection(connection).unwrap()
    }

    #[cfg(feature = "write")]
    fn writable_sample(body: &[u8]) -> Vec<u8> {
        let external_offset = 24 + 16 + 40;
        let external_chunk_size = 16 + 16 + IDENTIFIER.len() as u64 + body.len() as u64;
        let database_offset = external_offset + external_chunk_size;

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE VectorObjectList (
                    MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
                    VectorData BLOB
                 );
                 CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO VectorObjectList VALUES (1, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ExternalChunk VALUES (?1, ?2)",
                params![IDENTIFIER, external_offset as i64],
            )
            .unwrap();
        let database = connection.serialize(MAIN_DB).unwrap().to_vec();

        let mut header = Vec::new();
        push_u64(&mut header, 256);
        push_u64(&mut header, database_offset);
        push_u64(&mut header, 16);
        header.extend_from_slice(&[0x42; 16]);
        let mut external = Vec::new();
        push_u64(&mut external, IDENTIFIER.len() as u64);
        external.extend_from_slice(IDENTIFIER);
        push_u64(&mut external, body.len() as u64);
        external.extend_from_slice(body);

        let mut bytes = Vec::from(b"CSFCHUNK".as_slice());
        push_u64(&mut bytes, 0);
        push_u64(&mut bytes, 24);
        assert_eq!(push_chunk(&mut bytes, b"CHNKHead", &header), 24);
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKExta", &external),
            external_offset
        );
        assert_eq!(
            push_chunk(&mut bytes, b"CHNKSQLi", &database),
            database_offset
        );
        push_chunk(&mut bytes, b"CHNKFoot", b"");
        let file_size = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&file_size.to_be_bytes());
        bytes
    }

    fn synthetic_vector_stroke(
        header_size: u32,
        point_section_size: u32,
        point_size: u32,
        point_count: u32,
        flags: u32,
    ) -> Vec<u8> {
        let mut body =
            vec![0_u8; header_size as usize + point_size as usize * point_count as usize];
        for (offset, value) in [
            (0, header_size),
            (4, point_section_size),
            (8, point_size),
            (12, 88),
            (16, point_count),
            (20, flags),
        ] {
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        for (index, value) in [-3_i32, 4, 105, 206].into_iter().enumerate() {
            let offset = VECTOR_STROKE_BOUNDS_OFFSET + index * 4;
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        for (offset, value) in [
            (40, 0x4faf_4faf_u32),
            (44, 0x56d1_56d1),
            (48, 0xcb1f_cb1f),
            (52, 0xffff_ffff),
            (56, 0x8000_0000),
            (60, 0),
        ] {
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        body[64..72].copy_from_slice(&0.75_f64.to_be_bytes());
        body[72..76].copy_from_slice(&41_u32.to_be_bytes());
        match flags & 0x000d {
            0x0001 if header_size == 88 => {
                body[76..84].copy_from_slice(&5.0_f64.to_be_bytes());
            }
            0x0001 if header_size == 92 => {
                body[76..80].copy_from_slice(&1_u32.to_be_bytes());
                body[80..88].copy_from_slice(&5.0_f64.to_be_bytes());
            }
            0x0008 if header_size == 96 => {
                body[76..80].copy_from_slice(&7_u32.to_be_bytes());
                body[80..84].copy_from_slice(&11_u32.to_be_bytes());
                body[84..92].copy_from_slice(&1.5_f64.to_be_bytes());
            }
            _ => {}
        }
        for point_index in 0..point_count as usize {
            let offset = header_size as usize + point_index * point_size as usize;
            let x = 10.25 + point_index as f64;
            let y = 20.5 + point_index as f64;
            body[offset..offset + 8].copy_from_slice(&x.to_be_bytes());
            body[offset + 8..offset + 16].copy_from_slice(&y.to_be_bytes());
            for (index, value) in [-1_i32, 2, 30, 40].into_iter().enumerate() {
                let bounds_offset = offset + VECTOR_POINT_BOUNDS_OFFSET + index * 4;
                body[bounds_offset..bounds_offset + 4].copy_from_slice(&value.to_be_bytes());
            }
            body[offset + VECTOR_POINT_FLAGS_OFFSET..offset + VECTOR_POINT_FLAGS_OFFSET + 4]
                .copy_from_slice(&0x4000_0001_u32.to_be_bytes());
            body[offset + 56..offset + 60].copy_from_slice(&1.25_f32.to_be_bytes());
            body[offset + 60..offset + 64].copy_from_slice(&0.5_f32.to_be_bytes());
            let control_count = match point_size {
                104 => 1,
                120 => 2,
                _ => 0,
            };
            for control_index in 0..control_count {
                let control_offset =
                    offset + VECTOR_POINT_BASE_SIZE + control_index * VECTOR_CONTROL_POINT_SIZE;
                let control_x = 100.0 + control_index as f64;
                let control_y = 200.0 + control_index as f64;
                body[control_offset..control_offset + 8].copy_from_slice(&control_x.to_be_bytes());
                body[control_offset + 8..control_offset + 16]
                    .copy_from_slice(&control_y.to_be_bytes());
            }
        }
        body
    }

    #[cfg(feature = "write")]
    fn supported_vector_body() -> Vec<u8> {
        let mut body = vec![0_u8; 92 + 2 * 88];
        for (offset, value) in [(0, 92_u32), (4, 76), (8, 88), (12, 88), (16, 2)] {
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        body[20..24].copy_from_slice(&0x2081_u32.to_be_bytes());
        for (index, value) in [5_i32, 148, 76, 197].into_iter().enumerate() {
            let offset = 24 + index * 4;
            body[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        for (point, (x, y, bbox)) in [
            (10.0_f64, 152.5_f64, [5_i32, 148, 76, 197]),
            (70.0_f64, 191.0_f64, [65_i32, 186, 75, 196]),
        ]
        .into_iter()
        .enumerate()
        {
            let offset = 92 + point * 88;
            body[offset..offset + 8].copy_from_slice(&x.to_be_bytes());
            body[offset + 8..offset + 16].copy_from_slice(&y.to_be_bytes());
            for (index, value) in bbox.into_iter().enumerate() {
                let bbox_offset = offset + 16 + index * 4;
                body[bbox_offset..bbox_offset + 4].copy_from_slice(&value.to_be_bytes());
            }
        }
        body
    }

    #[test]
    fn parses_all_observed_vector_record_sizes() {
        let layouts = [
            (
                76_u32,
                72_u32,
                88_u32,
                VectorStrokeFlags::RULER,
                VectorStrokeFlags::STRAIGHT,
            ),
            (
                76,
                72,
                104,
                VectorStrokeFlags::RULER,
                VectorStrokeFlags::QUADRATIC,
            ),
            (
                76,
                72,
                120,
                VectorStrokeFlags::RULER,
                VectorStrokeFlags::CUBIC,
            ),
            (
                88,
                72,
                88,
                VectorStrokeFlags::NORMAL,
                VectorStrokeFlags::STRAIGHT,
            ),
            (
                88,
                72,
                104,
                VectorStrokeFlags::NORMAL,
                VectorStrokeFlags::QUADRATIC,
            ),
            (
                92,
                76,
                88,
                VectorStrokeFlags::NORMAL,
                VectorStrokeFlags::SPLINE,
            ),
            (
                96,
                72,
                88,
                VectorStrokeFlags::FRAME,
                VectorStrokeFlags::STRAIGHT,
            ),
            (
                96,
                72,
                104,
                VectorStrokeFlags::FRAME,
                VectorStrokeFlags::QUADRATIC,
            ),
            (
                96,
                72,
                120,
                VectorStrokeFlags::FRAME,
                VectorStrokeFlags::CUBIC,
            ),
        ];
        let mut body = Vec::new();
        for (header, section, point, kind, curve) in layouts {
            body.extend_from_slice(&synthetic_vector_stroke(
                header,
                section,
                point,
                2,
                kind.raw() | curve.raw() | 0x8000_0000,
            ));
        }

        let data = parse_vector_data(body.clone(), Limits::default()).unwrap();
        assert_eq!(data.raw(), body);
        assert_eq!(data.stroke_count(), layouts.len());
        assert!(!data.is_empty());
        assert_eq!(data.strokes().len(), layouts.len());

        for (stroke, (header, section, point_size, kind_flag, curve_flag)) in
            data.strokes().zip(layouts)
        {
            assert_eq!(stroke.layout().header_size(), header);
            assert_eq!(stroke.layout().point_section_size(), section);
            assert_eq!(stroke.layout().point_size(), point_size);
            assert_eq!(stroke.layout().default_point_size(), 88);
            assert!(stroke.layout().is_observed());
            assert_eq!(stroke.raw_header().len(), header as usize);
            assert_eq!(stroke.raw_points().len(), point_size as usize * 2);
            assert_eq!(stroke.point_count(), 2);
            assert!(stroke.is_semantically_supported());
            assert_eq!(stroke.kind(), kind_flag.kind());
            assert_eq!(stroke.curve_kind(), curve_flag.curve_kind());
            assert_eq!(stroke.flags().unwrap().unknown_bits(), 0x8000_0000);
            assert!(!stroke.is_closed());
            assert_eq!(
                stroke.bounds(),
                Some(VectorBounds {
                    left: -3,
                    top: 4,
                    right: 105,
                    bottom: 206,
                })
            );
            let style = stroke.style().unwrap();
            assert_eq!(style.kind(), stroke.kind().unwrap());
            assert_eq!(style.main_color().rgb_8bit(), [0x4f, 0x56, 0xcb]);
            assert_eq!(style.main_color().red_raw(), 0x4faf_4faf);
            assert_eq!(style.sub_color().rgb_8bit(), [0xff, 0x80, 0]);
            assert_eq!(style.opacity(), 0.75);
            assert_eq!(style.base_brush_id(), 41);
            assert_eq!(style.brush_id(), if header == 96 { 7 } else { 41 });
            assert_eq!(
                style.brush_radius(),
                match header {
                    76 => None,
                    96 => Some(1.5),
                    _ => Some(5.0),
                }
            );
            assert_eq!(
                style.brush_diameter(),
                style.brush_radius().map(|radius| radius * 2.0)
            );
            assert_eq!(style.fill_style_id(), (header == 96).then_some(11));
            assert_eq!(style.modern_normal_extra(), (header == 92).then_some(1));

            let first = stroke.point(0).unwrap();
            assert_eq!(first, stroke.points().next().unwrap());
            assert!(first.is_semantically_supported());
            assert_eq!(first.position(), Some(VectorPosition { x: 10.25, y: 20.5 }));
            assert_eq!(
                first.bounds(),
                Some(VectorBounds {
                    left: -1,
                    top: 2,
                    right: 30,
                    bottom: 40,
                })
            );
            let point_flags = first.flags().unwrap();
            assert!(point_flags.is_corner());
            assert_eq!(point_flags.unknown_bits(), 0x4000_0000);
            assert_eq!(first.width_factor(), Some(1.25));
            assert_eq!(first.opacity_factor(), Some(0.5));
            assert_eq!(
                first.factors(),
                Some(VectorPointFactors {
                    width_factor: 1.25,
                    opacity_factor: 0.5,
                })
            );
            let controls = first.control_points().collect::<Vec<_>>();
            assert_eq!(
                controls.len(),
                match point_size {
                    104 => 1,
                    120 => 2,
                    _ => 0,
                }
            );
            if let Some(control) = controls.first() {
                assert_eq!(*control, VectorPosition { x: 100.0, y: 200.0 });
            }
        }
    }

    #[test]
    fn accepts_empty_and_boundary_traversable_unknown_vector_layouts() {
        let empty = parse_vector_data(Vec::new(), Limits::default()).unwrap();
        assert!(empty.is_empty());
        assert!(empty.raw().is_empty());
        assert!(empty.strokes().next().is_none());

        let mut unknown = vec![0_u8; 24 + 2 * 4];
        for (offset, value) in [
            (0, 24_u32),
            (4, 7),
            (8, 4),
            (12, 3),
            (16, 2),
            (20, 0xDEAD_BEEF),
        ] {
            unknown[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        unknown[24..].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let data = parse_vector_data(unknown.clone(), Limits::default()).unwrap();
        let stroke = data.stroke(0).unwrap();
        assert!(!stroke.layout().is_observed());
        assert_eq!(stroke.raw(), unknown);
        assert_eq!(stroke.raw_header(), &unknown[..24]);
        assert_eq!(stroke.flags().unwrap().raw(), 0xDEAD_BEEF);
        assert_eq!(stroke.kind(), None);
        assert_eq!(stroke.bounds(), None);
        assert_eq!(stroke.points().len(), 2);
        let point = stroke.point(1).unwrap();
        assert_eq!(point.raw(), &[5, 6, 7, 8]);
        assert_eq!(point.position(), None);
        assert_eq!(point.bounds(), None);
        assert_eq!(point.flags(), None);
        assert_eq!(point.factors(), None);
        assert_eq!(point.control_points().len(), 0);

        let mut unknown_with_point = synthetic_vector_stroke(80, 99, 88, 1, 0x11);
        unknown_with_point[80..88].copy_from_slice(&f64::NAN.to_be_bytes());
        let data = parse_vector_data(unknown_with_point, Limits::default()).unwrap();
        let stroke = data.stroke(0).unwrap();
        assert!(!stroke.layout().is_observed());
        assert!(!stroke.is_semantically_supported());
        assert_eq!(stroke.style(), None);
        let point = stroke.point(0).unwrap();
        assert!(!point.is_semantically_supported());
        assert_eq!(point.position(), None);

        for layout in [
            VectorLayout {
                header_size: 88,
                point_section_size: 76,
                point_size: 88,
                default_point_size: 88,
            },
            VectorLayout {
                header_size: 88,
                point_section_size: 72,
                point_size: 120,
                default_point_size: 88,
            },
            VectorLayout {
                header_size: 92,
                point_section_size: 76,
                point_size: 88,
                default_point_size: 104,
            },
        ] {
            assert!(!layout.is_observed());
        }
    }

    #[test]
    fn rejects_truncated_or_non_advancing_vector_records() {
        assert!(matches!(
            parse_vector_data(vec![0; VECTOR_LAYOUT_PREFIX_SIZE - 1], Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));

        let mut short_header = vec![0_u8; VECTOR_LAYOUT_PREFIX_SIZE];
        short_header[..4].copy_from_slice(&16_u32.to_be_bytes());
        assert!(matches!(
            parse_vector_data(short_header, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));

        let mut zero_sized_points = vec![0_u8; VECTOR_LAYOUT_PREFIX_SIZE];
        zero_sized_points[..4].copy_from_slice(&(VECTOR_LAYOUT_PREFIX_SIZE as u32).to_be_bytes());
        zero_sized_points[16..20].copy_from_slice(&1_u32.to_be_bytes());
        assert!(matches!(
            parse_vector_data(zero_sized_points, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));

        let mut truncated = synthetic_vector_stroke(92, 76, 88, 1, 0x11);
        truncated.pop();
        assert!(matches!(
            parse_vector_data(truncated, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));
    }

    #[test]
    fn enforces_vector_stroke_and_point_limits() {
        let stroke = synthetic_vector_stroke(92, 76, 88, 2, 0x11);
        let two_strokes = [&stroke[..], &stroke[..]].concat();
        assert!(matches!(
            parse_vector_data(two_strokes, Limits::default().with_max_vector_strokes(1)),
            Err(Error::LimitExceeded {
                resource: "vector strokes",
                value: 2,
                limit: 1,
            })
        ));
        assert!(matches!(
            parse_vector_data(stroke, Limits::default().with_max_vector_points(1)),
            Err(Error::LimitExceeded {
                resource: "vector points",
                value: 2,
                limit: 1,
            })
        ));
    }

    #[test]
    fn rejects_non_finite_vector_positions_and_control_points() {
        let mut position = synthetic_vector_stroke(92, 76, 88, 1, 0x11);
        position[92..100].copy_from_slice(&f64::NAN.to_be_bytes());
        assert!(matches!(
            parse_vector_data(position, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));

        let mut control = synthetic_vector_stroke(76, 72, 104, 1, 0x24);
        let control_offset = 76 + VECTOR_POINT_BASE_SIZE;
        control[control_offset..control_offset + 8].copy_from_slice(&f64::INFINITY.to_be_bytes());
        assert!(matches!(
            parse_vector_data(control, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));

        let mut opacity = synthetic_vector_stroke(92, 76, 88, 1, 0x11);
        opacity[64..72].copy_from_slice(&f64::NAN.to_be_bytes());
        assert!(matches!(
            parse_vector_data(opacity, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));

        let mut width_factor = synthetic_vector_stroke(92, 76, 88, 1, 0x11);
        width_factor[92 + 56..92 + 60].copy_from_slice(&f32::NAN.to_be_bytes());
        assert!(matches!(
            parse_vector_data(width_factor, Limits::default()),
            Err(Error::InvalidVectorData { .. })
        ));
    }

    #[test]
    fn resolves_and_reads_typed_vector_data() {
        let body = synthetic_vector_stroke(92, 76, 88, 1, 0x11);
        let (bytes, offset) = sample(&body);
        let database = database(offset);
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        let vector = clip
            .read_vector(&database, &source, Limits::default())
            .unwrap();
        assert_eq!(vector.raw(), body);
        assert_eq!(vector.stroke_count(), 1);
        assert_eq!(vector.stroke(0).unwrap().point_count(), 1);
    }

    #[test]
    fn resolves_and_reads_raw_vector_data() {
        let body = b"opaque vector body";
        let (bytes, offset) = sample(body);
        let database = database(offset);
        let sources = database.vector_data_sources(3, Limits::default()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id(), 1);
        assert_eq!(sources[0].canvas_id(), 2);
        assert_eq!(sources[0].layer_id(), 3);
        assert_eq!(sources[0].external_identifier(), IDENTIFIER);

        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert_eq!(
            clip.read_vector_data(&database, &sources[0], Limits::default())
                .unwrap(),
            body
        );
    }

    #[test]
    fn enforces_vector_limits_and_tolerates_missing_table() {
        let connection = Connection::open_in_memory().unwrap();
        let empty = Database::from_connection(connection).unwrap();
        assert!(
            empty
                .vector_data_sources(3, Limits::default())
                .unwrap()
                .is_empty()
        );

        let body = b"1234";
        let (bytes, offset) = sample(body);
        let database = database(offset);
        assert!(matches!(
            database.vector_data_sources(3, Limits::default().with_max_vector_objects(0)),
            Err(Error::LimitExceeded { .. })
        ));
        assert!(matches!(
            database.vector_data_sources(
                3,
                Limits::default().with_max_identifier_size((IDENTIFIER.len() - 1) as u64)
            ),
            Err(Error::LimitExceeded { .. })
        ));
        let source = database.vector_data_sources(3, Limits::default()).unwrap();
        let mut clip = ClipFile::open(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            clip.read_vector_data(
                &database,
                &source[0],
                Limits::default().with_max_vector_data_bytes(3)
            ),
            Err(Error::PayloadTooLarge { size: 4, limit: 3 })
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn replaces_a_validated_vector_body_and_reads_it_back() {
        let mut clip = ClipFile::open(Cursor::new(writable_sample(b"old vector"))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        assert!(
            writer
                .replace_vector_data_body(&source, b"new opaque vector".to_vec(), Limits::default())
                .unwrap()
                .is_none()
        );

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        assert_eq!(
            rewritten
                .read_vector_data(&database, &source, Limits::default())
                .unwrap(),
            b"new opaque vector"
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn writes_a_vector_identifier_stored_with_the_text_storage_class() {
        let mut clip = ClipFile::open(Cursor::new(writable_sample(b"old vector"))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "UPDATE VectorObjectList \
                 SET VectorData = CAST(VectorData AS TEXT) WHERE MainId = 1",
                [],
            )
            .unwrap();
        assert_eq!(
            writer
                .database()
                .connection()
                .query_row(
                    "SELECT typeof(VectorData) FROM VectorObjectList WHERE MainId = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "text"
        );

        assert!(writer
            .replace_vector_data_body(&source, b"new opaque vector".to_vec(), Limits::default(),)
            .is_ok());
        assert_eq!(writer.replacement_count(), 1);
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_a_vector_identifier_shared_by_multiple_rows() {
        let mut clip = ClipFile::open(Cursor::new(writable_sample(b"old vector"))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "INSERT INTO VectorObjectList VALUES (2, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();

        assert!(matches!(
            writer.replace_vector_data_body(
                &source,
                b"new opaque vector".to_vec(),
                Limits::default()
            ),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn translates_supported_vector_points_and_bounding_boxes() {
        let body = supported_vector_body();
        let (translated, summary) =
            translate_supported_vector_body(&body, 3, -4, Limits::default()).unwrap();
        assert_eq!(summary.strokes(), 1);
        assert_eq!(summary.points(), 2);
        assert_eq!(summary.delta_x(), 3);
        assert_eq!(summary.delta_y(), -4);
        assert_eq!(
            f64::from_be_bytes(translated[92..100].try_into().unwrap()),
            13.0
        );
        assert_eq!(
            f64::from_be_bytes(translated[100..108].try_into().unwrap()),
            148.5
        );
        assert_eq!(
            i32::from_be_bytes(translated[24..28].try_into().unwrap()),
            8
        );
        assert_eq!(
            i32::from_be_bytes(translated[28..32].try_into().unwrap()),
            144
        );
        for index in 0..body.len() {
            let is_coordinate = matches!(
                index,
                24..=39 | 92..=123 | 180..=211
            );
            if !is_coordinate {
                assert_eq!(translated[index], body[index], "byte {index} changed");
            }
        }
    }

    #[cfg(feature = "write")]
    #[test]
    fn clones_one_supported_vector_stroke_and_preserves_opaque_bytes() {
        let mut body = supported_vector_body();
        body[40..52].copy_from_slice(&[0xA5; 12]);
        body[72..92].copy_from_slice(&[0x5A; 20]);
        body[124..180].copy_from_slice(&[0xC3; 56]);
        body[212..268].copy_from_slice(&[0x3C; 56]);

        let (replacement, appended_index, summary) =
            clone_supported_vector_stroke(&body, 0, 3, -4, Limits::default()).unwrap();
        assert_eq!(appended_index, 1);
        assert_eq!(summary.strokes(), 1);
        assert_eq!(summary.points(), 2);
        assert_eq!(&replacement[..body.len()], body.as_slice());
        let clone = &replacement[body.len()..];
        assert_eq!(clone.len(), body.len());
        assert_eq!(f64::from_be_bytes(clone[92..100].try_into().unwrap()), 13.0);
        assert_eq!(
            f64::from_be_bytes(clone[100..108].try_into().unwrap()),
            148.5
        );
        assert_eq!(i32::from_be_bytes(clone[24..28].try_into().unwrap()), 8);
        assert_eq!(i32::from_be_bytes(clone[28..32].try_into().unwrap()), 144);
        for index in 0..body.len() {
            let is_coordinate = matches!(index, 24..=39 | 92..=123 | 180..=211);
            if !is_coordinate {
                assert_eq!(clone[index], body[index], "byte {index} changed");
            }
        }
    }

    #[cfg(feature = "write")]
    #[test]
    fn removes_only_the_selected_supported_vector_record() {
        let mut first = supported_vector_body();
        first[40] = 0x11;
        let mut second = supported_vector_body();
        second[40] = 0x22;
        let mut third = supported_vector_body();
        third[40] = 0x33;
        let body = [&first[..], &second[..], &third[..]].concat();

        let (replacement, removed_points) =
            remove_supported_vector_stroke(&body, 1, Limits::default()).unwrap();
        assert_eq!(removed_points, 2);
        assert_eq!(replacement, [&first[..], &third[..]].concat());
        assert_eq!(
            supported_vector_stroke_ranges(&replacement, Limits::default())
                .unwrap()
                .len(),
            2
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn removes_final_vector_stroke_and_rejects_missing_index() {
        let mut clip =
            ClipFile::open(Cursor::new(writable_sample(&supported_vector_body()))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 1, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(
            writer
                .remove_vector_stroke(&source, 0, Limits::default())
                .unwrap(),
            2
        );
        assert_eq!(writer.replacement_count(), 1);
        assert!(matches!(
            writer.clone_vector_stroke(&source, 0, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.remove_vector_stroke(&source, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.translate_vector_data(&source, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(writer.addition_count(), 0);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        assert!(
            rewritten
                .read_vector_data(&database, &source, Limits::default())
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_vector_stroke_count_limit_without_a_replacement() {
        let stroke = supported_vector_body();
        let body = [&stroke[..], &stroke[..]].concat();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        let limits = Limits::default().with_max_vector_objects(1);

        for result in [
            writer
                .translate_vector_data(&source, 0, 0, limits)
                .map(|_| ()),
            writer
                .clone_vector_stroke(&source, 0, 0, 0, limits)
                .map(|_| ()),
            writer.remove_vector_stroke(&source, 0, limits).map(|_| ()),
        ] {
            assert!(matches!(
                result,
                Err(Error::LimitExceeded {
                    resource: "vector stroke records",
                    value: 2,
                    limit: 1,
                })
            ));
        }
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(writer.addition_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_clone_stroke_count_limit_without_a_replacement() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(
                &source,
                0,
                0,
                0,
                Limits::default().with_max_vector_objects(1)
            ),
            Err(Error::LimitExceeded {
                resource: "vector stroke records after clone",
                value: 2,
                limit: 1,
            })
        ));
        assert_eq!(writer.replacement_count(), 0);
        assert_eq!(writer.addition_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn enforces_clone_output_limit_without_a_replacement() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        let limit = (body.len() * 2 - 1) as u64;

        assert!(matches!(
            writer.clone_vector_stroke(
                &source,
                0,
                0,
                0,
                Limits::default().with_max_vector_data_bytes(limit)
            ),
            Err(Error::LimitExceeded {
                resource: "replacement vector data bytes",
                ..
            })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn rejects_truncated_clone_template_without_a_replacement() {
        let mut body = supported_vector_body();
        body.pop();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 0, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn clone_and_remove_reject_shared_vector_identifier_without_a_replacement() {
        let mut clip =
            ClipFile::open(Cursor::new(writable_sample(&supported_vector_body()))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .database()
            .connection()
            .execute(
                "INSERT INTO VectorObjectList VALUES (2, 2, 3, ?1)",
                params![IDENTIFIER],
            )
            .unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 0, 1, 1, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.remove_vector_stroke(&source, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 0);
    }

    #[cfg(feature = "write")]
    #[test]
    fn clone_then_remove_round_trips_through_the_writer() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();

        let (appended_index, summary) = writer
            .clone_vector_stroke(&source, 0, 3, -4, Limits::default())
            .unwrap();
        assert_eq!(appended_index, 1);
        assert_eq!(summary.strokes(), 1);
        assert_eq!(summary.points(), 2);
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(
            writer
                .remove_vector_stroke(&source, 1, Limits::default())
                .unwrap(),
            2
        );

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        rewritten.validate_external_index(&database).unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        assert_eq!(
            rewritten
                .read_vector_data(&database, &source, Limits::default())
                .unwrap(),
            body
        );
    }

    #[cfg(feature = "write")]
    #[test]
    fn failed_follow_up_edit_keeps_the_previous_vector_replacement() {
        let body = supported_vector_body();
        let mut clip = ClipFile::open(Cursor::new(writable_sample(&body))).unwrap();
        let database = clip.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let mut writer = clip.writer().unwrap();
        writer
            .clone_vector_stroke(&source, 0, 3, -4, Limits::default())
            .unwrap();

        assert!(matches!(
            writer.clone_vector_stroke(&source, 99, 0, 0, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert!(matches!(
            writer.remove_vector_stroke(&source, 99, Limits::default()),
            Err(Error::InvalidWrite { .. })
        ));
        assert_eq!(writer.replacement_count(), 1);
        assert_eq!(writer.addition_count(), 0);

        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        let source = database
            .vector_data_sources(3, Limits::default())
            .unwrap()
            .remove(0);
        let rewritten_body = rewritten
            .read_vector_data(&database, &source, Limits::default())
            .unwrap();
        assert_eq!(
            supported_vector_stroke_ranges(&rewritten_body, Limits::default())
                .unwrap()
                .len(),
            2
        );
        assert_eq!(&rewritten_body[..body.len()], body.as_slice());
    }
}
