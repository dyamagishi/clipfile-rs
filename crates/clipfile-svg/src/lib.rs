//! Loss-aware, streaming SVG output for [`clipfile`] vector data.
//!
//! SVG paths can reproduce straight, quadratic Bezier, and cubic Bezier
//! centerlines exactly. Other CLIP features require an explicit approximation.
//! [`SvgExportReport`] records those approximations without retaining a warning
//! for every stroke, and [`SvgOptions`] can reject selected loss kinds.

use std::{
    error,
    fmt::{self, Write as _},
    io::{self, Write},
    ops::AddAssign,
};

use clipfile::{
    BrushStyle, VectorCurveKind, VectorData, VectorPoint, VectorPointFlags, VectorStroke,
    VectorStrokeStyle,
};
use quick_xml::{
    Writer,
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
};

/// Result type used by this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// An error while configuring or streaming an SVG document.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The destination writer returned an I/O error.
    Io(io::Error),
    /// Canvas dimensions must be finite and greater than zero.
    InvalidCanvas { width: f64, height: f64 },
    /// Viewport origin must be finite and its dimensions greater than zero.
    InvalidViewport {
        min_x: f64,
        min_y: f64,
        width: f64,
        height: f64,
    },
    /// Group opacity must be finite and in the inclusive range `0.0..=1.0`.
    InvalidGroupOpacity { opacity: f64 },
    /// Spline fitting tolerance must be finite and greater than zero.
    InvalidSplineTolerance { tolerance: f64 },
    /// Nominal stroke-width adjustment must be finite.
    InvalidNominalWidthAdjustment { adjustment: f64 },
    /// A title contains a character disallowed by XML 1.0.
    InvalidXmlText,
    /// A group identifier is empty or is not valid XML 1.0 text.
    InvalidGroupId,
    /// A configured policy rejected a lossy conversion.
    LossyConversion {
        /// Zero-based stroke number across all vector objects written so far.
        stroke_index: u64,
        /// The approximation that was rejected.
        kind: SvgLossKind,
    },
    /// An arithmetic result cannot be represented as a finite SVG number.
    InvalidSvgNumber {
        /// Name of the value being written.
        field: &'static str,
        /// Non-finite value that was produced.
        value: f64,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not write SVG: {error}"),
            Self::InvalidCanvas { width, height } => {
                write!(formatter, "invalid SVG canvas dimensions {width}x{height}")
            }
            Self::InvalidViewport {
                min_x,
                min_y,
                width,
                height,
            } => write!(
                formatter,
                "invalid SVG viewport {min_x} {min_y} {width} {height}"
            ),
            Self::InvalidGroupOpacity { opacity } => {
                write!(formatter, "invalid SVG group opacity {opacity}")
            }
            Self::InvalidSplineTolerance { tolerance } => {
                write!(formatter, "invalid spline fitting tolerance {tolerance}")
            }
            Self::InvalidNominalWidthAdjustment { adjustment } => {
                write!(
                    formatter,
                    "invalid nominal stroke-width adjustment {adjustment}"
                )
            }
            Self::InvalidXmlText => write!(formatter, "SVG text contains invalid XML 1.0 text"),
            Self::InvalidGroupId => write!(formatter, "invalid SVG group identifier"),
            Self::LossyConversion { stroke_index, kind } => write!(
                formatter,
                "stroke {stroke_index} requires rejected SVG conversion: {kind}"
            ),
            Self::InvalidSvgNumber { field, value } => {
                write!(
                    formatter,
                    "{field} cannot be written as a finite SVG number: {value}"
                )
            }
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

/// Finite, positive SVG viewport dimensions in user units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgCanvas {
    width: f64,
    height: f64,
    viewport: SvgViewport,
}

impl SvgCanvas {
    /// Creates a validated SVG canvas.
    pub fn new(width: f64, height: f64) -> Result<Self> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(Error::InvalidCanvas { width, height });
        }
        Ok(Self {
            width,
            height,
            viewport: SvgViewport {
                min_x: 0.0,
                min_y: 0.0,
                width,
                height,
            },
        })
    }

    /// Canvas width in SVG user units.
    #[must_use]
    pub const fn width(self) -> f64 {
        self.width
    }

    /// Canvas height in SVG user units.
    #[must_use]
    pub const fn height(self) -> f64 {
        self.height
    }

    /// Uses an independently validated SVG `viewBox`.
    #[must_use]
    pub const fn with_viewport(mut self, viewport: SvgViewport) -> Self {
        self.viewport = viewport;
        self
    }

    /// Source rectangle represented by the SVG `viewBox`.
    #[must_use]
    pub const fn viewport(self) -> SvgViewport {
        self.viewport
    }
}

/// A finite SVG `viewBox` with a positive extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgViewport {
    min_x: f64,
    min_y: f64,
    width: f64,
    height: f64,
}

impl SvgViewport {
    /// Creates a viewport. Its origin may be negative, but all values must be
    /// finite and both dimensions must be greater than zero.
    pub fn new(min_x: f64, min_y: f64, width: f64, height: f64) -> Result<Self> {
        if !min_x.is_finite()
            || !min_y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return Err(Error::InvalidViewport {
                min_x,
                min_y,
                width,
                height,
            });
        }
        Ok(Self {
            min_x,
            min_y,
            width,
            height,
        })
    }

    #[must_use]
    pub const fn min_x(self) -> f64 {
        self.min_x
    }

    #[must_use]
    pub const fn min_y(self) -> f64 {
        self.min_y
    }

    #[must_use]
    pub const fn width(self) -> f64 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> f64 {
        self.height
    }
}

/// One category of information that plain SVG paths cannot preserve exactly.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SvgLossKind {
    /// CLIP spline samples were joined with straight SVG path segments.
    SplineToPolyline,
    /// CLIP spline samples were fitted with cubic Bezier segments.
    SplineToCubicApproximation,
    /// Per-point widths were replaced with one arithmetic mean width.
    VariableWidthAveraged,
    /// Per-point width factors were preserved as a filled SVG outline.
    VariableWidthOutlined,
    /// Per-point opacities were replaced with one arithmetic mean opacity.
    VariableOpacityAveraged,
    /// Per-point opacity factors were preserved as gradient-filled segments.
    VariableOpacitySegmented,
    /// A CLIP brush shape or texture was replaced with a round SVG stroke.
    BrushToRoundStroke,
    /// The secondary brush color was not represented.
    SecondaryColorIgnored,
    /// A frame fill style was not represented.
    FrameFillIgnored,
    /// A missing nominal brush width was replaced with one SVG user unit.
    MissingWidthFallback,
    /// Width factors were unavailable for at least one drawable point.
    MissingWidthFactorFallback,
    /// Opacity factors were unavailable for at least one drawable point.
    MissingOpacityFactorFallback,
    /// Missing Bezier controls caused at least one segment to become a line.
    BezierControlsToLine,
    /// An unsafe spline-fit segment was locally replaced with a line.
    SplineFitSegmentToLine,
    /// Consecutive nearby one-point strokes were joined into a continuous path.
    AdjacentOnePointStrokesConnected,
    /// A stroke with an unsupported semantic layout was skipped.
    UnsupportedStrokeSkipped,
}

impl SvgLossKind {
    const ALL: [Self; 16] = [
        Self::SplineToPolyline,
        Self::SplineToCubicApproximation,
        Self::VariableWidthAveraged,
        Self::VariableWidthOutlined,
        Self::VariableOpacityAveraged,
        Self::VariableOpacitySegmented,
        Self::BrushToRoundStroke,
        Self::SecondaryColorIgnored,
        Self::FrameFillIgnored,
        Self::MissingWidthFallback,
        Self::MissingWidthFactorFallback,
        Self::MissingOpacityFactorFallback,
        Self::BezierControlsToLine,
        Self::SplineFitSegmentToLine,
        Self::AdjacentOnePointStrokesConnected,
        Self::UnsupportedStrokeSkipped,
    ];

    const fn index(self) -> usize {
        match self {
            Self::SplineToPolyline => 0,
            Self::SplineToCubicApproximation => 1,
            Self::VariableWidthAveraged => 2,
            Self::VariableWidthOutlined => 3,
            Self::VariableOpacityAveraged => 4,
            Self::VariableOpacitySegmented => 5,
            Self::BrushToRoundStroke => 6,
            Self::SecondaryColorIgnored => 7,
            Self::FrameFillIgnored => 8,
            Self::MissingWidthFallback => 9,
            Self::MissingWidthFactorFallback => 10,
            Self::MissingOpacityFactorFallback => 11,
            Self::BezierControlsToLine => 12,
            Self::SplineFitSegmentToLine => 13,
            Self::AdjacentOnePointStrokesConnected => 14,
            Self::UnsupportedStrokeSkipped => 15,
        }
    }

    /// User-facing impact category for this conversion loss.
    #[must_use]
    pub const fn severity(self) -> SvgLossSeverity {
        match self {
            Self::SplineToPolyline
            | Self::SplineToCubicApproximation
            | Self::VariableWidthAveraged
            | Self::VariableWidthOutlined
            | Self::VariableOpacityAveraged
            | Self::VariableOpacitySegmented
            | Self::BrushToRoundStroke
            | Self::AdjacentOnePointStrokesConnected => SvgLossSeverity::Approximation,
            Self::SecondaryColorIgnored
            | Self::FrameFillIgnored
            | Self::UnsupportedStrokeSkipped => SvgLossSeverity::Omission,
            Self::MissingWidthFallback
            | Self::MissingWidthFactorFallback
            | Self::MissingOpacityFactorFallback
            | Self::BezierControlsToLine
            | Self::SplineFitSegmentToLine => SvgLossSeverity::Fallback,
        }
    }
}

impl fmt::Display for SvgLossKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::SplineToPolyline => "CLIP spline approximated by a polyline",
            Self::SplineToCubicApproximation => {
                "CLIP spline samples fitted with cubic Bezier segments"
            }
            Self::VariableWidthAveraged => "variable width averaged",
            Self::VariableWidthOutlined => "variable width preserved as a fitted outline",
            Self::VariableOpacityAveraged => "variable opacity averaged",
            Self::VariableOpacitySegmented => {
                "variable opacity preserved as gradient-filled segments"
            }
            Self::BrushToRoundStroke => "CLIP brush approximated by a round stroke",
            Self::SecondaryColorIgnored => "secondary color metadata not represented",
            Self::FrameFillIgnored => "frame fill style ignored",
            Self::MissingWidthFallback => "missing brush width replaced with one",
            Self::MissingWidthFactorFallback => "missing width factors replaced",
            Self::MissingOpacityFactorFallback => "missing opacity factors replaced",
            Self::BezierControlsToLine => "Bezier segment without controls replaced by a line",
            Self::SplineFitSegmentToLine => "unsafe spline-fit segment replaced by a line",
            Self::AdjacentOnePointStrokesConnected => {
                "adjacent one-point strokes connected into a continuous path"
            }
            Self::UnsupportedStrokeSkipped => "unsupported stroke skipped",
        };
        formatter.write_str(description)
    }
}

/// Severity of an SVG conversion loss.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SvgLossSeverity {
    /// A visible SVG approximation was emitted.
    Approximation,
    /// Stored information or a complete stroke was omitted.
    Omission,
    /// Missing or incomplete input was replaced with a fallback.
    Fallback,
}

/// Reason that an input stroke produced no SVG mark.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SvgSkipKind {
    /// The input stroke has no point records.
    EmptyStroke,
    /// The record layout has no supported semantic interpretation.
    UnsupportedSemanticLayout,
    /// The semantic layout is known but its curve kind is not supported.
    UnsupportedCurveKind,
    /// At least one required position was unavailable.
    IncompleteGeometry,
    /// A configured loss policy rejected the stroke before output.
    PolicyRejected,
}

impl SvgSkipKind {
    const ALL: [Self; 5] = [
        Self::EmptyStroke,
        Self::UnsupportedSemanticLayout,
        Self::UnsupportedCurveKind,
        Self::IncompleteGeometry,
        Self::PolicyRejected,
    ];

    const fn index(self) -> usize {
        match self {
            Self::EmptyStroke => 0,
            Self::UnsupportedSemanticLayout => 1,
            Self::UnsupportedCurveKind => 2,
            Self::IncompleteGeometry => 3,
            Self::PolicyRejected => 4,
        }
    }
}

impl fmt::Display for SvgSkipKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyStroke => "empty stroke",
            Self::UnsupportedSemanticLayout => "unsupported semantic layout",
            Self::UnsupportedCurveKind => "unsupported curve kind",
            Self::IncompleteGeometry => "incomplete geometry",
            Self::PolicyRejected => "conversion rejected by policy",
        })
    }
}

/// Whether a lossy SVG conversion is permitted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SvgLossHandling {
    /// Perform the conversion and record it in [`SvgExportReport`].
    Allow,
    /// Return [`Error::LossyConversion`] before writing the affected path.
    Reject,
}

/// Default maximum fitting error for CLIP spline samples, in SVG user units.
///
/// This half-unit tolerance is calibrated against CLIP STUDIO's SVG export
/// using an observed production vector layer. It is still an approximation of
/// CLIP's undocumented smoothing and subdivision algorithm.
pub const DEFAULT_SPLINE_TOLERANCE: f64 = 0.5;

/// How per-point width and opacity are represented in SVG.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SvgStrokeRepresentation {
    /// Preserve appearance using filled outlines and opacity-gradient segments.
    PreserveAppearance,
    /// Emit one editable centerline path and use arithmetic-mean factors.
    EditableCenterline,
}

/// How consecutive nearby CLIP strokes containing one drawable point are represented.
///
/// A single one-point stroke is always preserved as a dot. Connection is only
/// attempted while writing a complete [`VectorData`], where neighboring stroke
/// order and style are available.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SvgOnePointStrokeHandling {
    /// Preserve every one-point stroke as a visible round dot.
    PreserveDots,
    /// Join runs of at least two nearby one-point strokes into a continuous path.
    ConnectNearby,
}

/// How stored CLIP spline samples are converted to SVG path commands.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum SvgSplineConversion {
    /// Join drawable samples in stored order with `M` and `L` commands.
    Polyline,
    /// Fit guarded cubic Bezier segments to the drawable samples.
    CubicFit,
}

/// Configuration for an SVG document.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgOptions {
    title: Option<String>,
    group_opacity: f64,
    spline_tolerance: f64,
    nominal_width_adjustment: f64,
    stroke_representation: SvgStrokeRepresentation,
    one_point_stroke_handling: SvgOnePointStrokeHandling,
    spline_conversion: SvgSplineConversion,
    loss_handling: [SvgLossHandling; SvgLossKind::ALL.len()],
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            title: None,
            group_opacity: 1.0,
            spline_tolerance: DEFAULT_SPLINE_TOLERANCE,
            nominal_width_adjustment: 0.0,
            stroke_representation: SvgStrokeRepresentation::PreserveAppearance,
            one_point_stroke_handling: SvgOnePointStrokeHandling::PreserveDots,
            spline_conversion: SvgSplineConversion::CubicFit,
            loss_handling: [SvgLossHandling::Allow; SvgLossKind::ALL.len()],
        }
    }
}

impl SvgOptions {
    /// Adds an escaped XML title to the document.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the opacity applied to the document's path group.
    ///
    /// The value is validated by [`SvgWriter::new`].
    #[must_use]
    pub const fn with_group_opacity(mut self, opacity: f64) -> Self {
        self.group_opacity = opacity;
        self
    }

    /// Sets the maximum geometric fitting error for CLIP spline samples.
    ///
    /// The value is measured in SVG user units and must be finite and greater
    /// than zero. Smaller values retain more cubic Bezier segments.
    #[must_use]
    pub const fn with_spline_tolerance(mut self, tolerance: f64) -> Self {
        self.spline_tolerance = tolerance;
        self
    }

    /// Adds a finite SVG-user-unit adjustment to stored nominal stroke widths.
    ///
    /// The default is `0.0`. Adjusted stored widths are clamped to zero. The
    /// one-unit fallback for a missing stored width is not adjusted.
    #[must_use]
    pub const fn with_nominal_width_adjustment(mut self, adjustment: f64) -> Self {
        self.nominal_width_adjustment = adjustment;
        self
    }

    /// Selects appearance-preserving outlines or editable centerline strokes.
    #[must_use]
    pub const fn with_stroke_representation(
        mut self,
        representation: SvgStrokeRepresentation,
    ) -> Self {
        self.stroke_representation = representation;
        self
    }

    /// Selects whether nearby runs of one-point strokes stay as dots or become a path.
    #[must_use]
    pub const fn with_one_point_stroke_handling(
        mut self,
        handling: SvgOnePointStrokeHandling,
    ) -> Self {
        self.one_point_stroke_handling = handling;
        self
    }

    /// Selects straight polyline or guarded cubic fitting for CLIP splines.
    #[must_use]
    pub const fn with_spline_conversion(mut self, conversion: SvgSplineConversion) -> Self {
        self.spline_conversion = conversion;
        self
    }

    /// Changes the handling of one loss category.
    #[must_use]
    pub fn with_loss_handling(mut self, kind: SvgLossKind, handling: SvgLossHandling) -> Self {
        self.loss_handling[kind.index()] = handling;
        self
    }

    /// Rejects every currently known lossy conversion.
    #[must_use]
    pub fn reject_all_losses(mut self) -> Self {
        self.loss_handling.fill(SvgLossHandling::Reject);
        self
    }

    /// Rejects omissions while continuing to permit approximations and
    /// fallbacks.
    #[must_use]
    pub fn reject_omissions(mut self) -> Self {
        for kind in SvgLossKind::ALL {
            if kind.severity() == SvgLossSeverity::Omission {
                self.loss_handling[kind.index()] = SvgLossHandling::Reject;
            }
        }
        self
    }

    /// Optional document title.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Opacity applied to the document's path group.
    #[must_use]
    pub const fn group_opacity(&self) -> f64 {
        self.group_opacity
    }

    /// Maximum geometric fitting error for CLIP spline samples.
    #[must_use]
    pub const fn spline_tolerance(&self) -> f64 {
        self.spline_tolerance
    }

    /// Adjustment added to stored nominal stroke widths in SVG user units.
    #[must_use]
    pub const fn nominal_width_adjustment(&self) -> f64 {
        self.nominal_width_adjustment
    }

    /// Current variable-factor representation.
    #[must_use]
    pub const fn stroke_representation(&self) -> SvgStrokeRepresentation {
        self.stroke_representation
    }

    /// Current handling of consecutive nearby one-point strokes.
    #[must_use]
    pub const fn one_point_stroke_handling(&self) -> SvgOnePointStrokeHandling {
        self.one_point_stroke_handling
    }

    /// Current CLIP spline conversion.
    #[must_use]
    pub const fn spline_conversion(&self) -> SvgSplineConversion {
        self.spline_conversion
    }

    /// Current handling for one loss category.
    #[must_use]
    pub const fn loss_handling(&self, kind: SvgLossKind) -> SvgLossHandling {
        self.loss_handling[kind.index()]
    }
}

/// Aggregate counts from one SVG conversion.
///
/// The report stores aggregate counters rather than per-stroke entries, so its
/// memory use does not grow with document size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SvgExportReport {
    vector_objects: u64,
    strokes_seen: u64,
    paths_written: u64,
    centerline_paths_written: u64,
    filled_outline_paths_written: u64,
    dots_written: u64,
    strokes_written: u64,
    strokes_skipped: u64,
    losses: [u64; SvgLossKind::ALL.len()],
    skips: [u64; SvgSkipKind::ALL.len()],
}

impl Default for SvgExportReport {
    fn default() -> Self {
        Self {
            vector_objects: 0,
            strokes_seen: 0,
            paths_written: 0,
            centerline_paths_written: 0,
            filled_outline_paths_written: 0,
            dots_written: 0,
            strokes_written: 0,
            strokes_skipped: 0,
            losses: [0; SvgLossKind::ALL.len()],
            skips: [0; SvgSkipKind::ALL.len()],
        }
    }
}

impl SvgExportReport {
    /// Number of [`VectorData`] objects passed to the writer.
    #[must_use]
    pub const fn vector_objects(&self) -> u64 {
        self.vector_objects
    }

    /// Number of input strokes examined.
    #[must_use]
    pub const fn strokes_seen(&self) -> u64 {
        self.strokes_seen
    }

    /// Number of SVG paths written.
    #[must_use]
    pub const fn paths_written(&self) -> u64 {
        self.paths_written
    }

    /// Number of editable or fixed-factor centerline SVG paths written.
    #[must_use]
    pub const fn centerline_paths_written(&self) -> u64 {
        self.centerline_paths_written
    }

    /// Number of filled outline SVG paths written.
    ///
    /// Variable-opacity strokes can contribute more than one path.
    #[must_use]
    pub const fn filled_outline_paths_written(&self) -> u64 {
        self.filled_outline_paths_written
    }

    /// Number of one-point strokes written as SVG circles.
    #[must_use]
    pub const fn dots_written(&self) -> u64 {
        self.dots_written
    }

    /// Total number of SVG paths and one-point circles written.
    #[must_use]
    pub const fn marks_written(&self) -> u64 {
        self.paths_written.saturating_add(self.dots_written)
    }

    /// Number of input strokes represented by an SVG mark.
    #[must_use]
    pub const fn strokes_written(&self) -> u64 {
        self.strokes_written
    }

    /// Number of empty or unsupported strokes skipped.
    #[must_use]
    pub const fn strokes_skipped(&self) -> u64 {
        self.strokes_skipped
    }

    /// Number of strokes affected by one loss category.
    #[must_use]
    pub const fn loss_count(&self, kind: SvgLossKind) -> u64 {
        self.losses[kind.index()]
    }

    /// Number of strokes skipped for one reason.
    #[must_use]
    pub const fn skip_count(&self, kind: SvgSkipKind) -> u64 {
        self.skips[kind.index()]
    }

    /// Whether any lossy conversion was performed or skipped.
    #[must_use]
    pub fn has_loss(&self) -> bool {
        self.losses.iter().any(|&count| count != 0)
    }

    /// Whether any input stroke was skipped.
    #[must_use]
    pub const fn has_skips(&self) -> bool {
        self.strokes_skipped != 0
    }

    /// Whether written and skipped counts account for every seen stroke.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        let classified_skips = self.skips.iter().copied().fold(0_u64, u64::saturating_add);
        self.strokes_seen == self.strokes_written.saturating_add(self.strokes_skipped)
            && self.strokes_skipped == classified_skips
            && self.paths_written
                == self
                    .centerline_paths_written
                    .saturating_add(self.filled_outline_paths_written)
    }

    /// Non-zero loss counts in stable category order.
    pub fn losses(&self) -> impl Iterator<Item = (SvgLossKind, u64)> + '_ {
        SvgLossKind::ALL.into_iter().filter_map(|kind| {
            let count = self.loss_count(kind);
            (count != 0).then_some((kind, count))
        })
    }

    /// Non-zero skip counts in stable category order.
    pub fn skips(&self) -> impl Iterator<Item = (SvgSkipKind, u64)> + '_ {
        SvgSkipKind::ALL.into_iter().filter_map(|kind| {
            let count = self.skip_count(kind);
            (count != 0).then_some((kind, count))
        })
    }

    /// Adds another report's counts using saturating arithmetic.
    pub fn merge(&mut self, other: &Self) {
        self.vector_objects = self.vector_objects.saturating_add(other.vector_objects);
        self.strokes_seen = self.strokes_seen.saturating_add(other.strokes_seen);
        self.paths_written = self.paths_written.saturating_add(other.paths_written);
        self.centerline_paths_written = self
            .centerline_paths_written
            .saturating_add(other.centerline_paths_written);
        self.filled_outline_paths_written = self
            .filled_outline_paths_written
            .saturating_add(other.filled_outline_paths_written);
        self.dots_written = self.dots_written.saturating_add(other.dots_written);
        self.strokes_written = self.strokes_written.saturating_add(other.strokes_written);
        self.strokes_skipped = self.strokes_skipped.saturating_add(other.strokes_skipped);
        for (target, source) in self.losses.iter_mut().zip(other.losses) {
            *target = target.saturating_add(source);
        }
        for (target, source) in self.skips.iter_mut().zip(other.skips) {
            *target = target.saturating_add(source);
        }
    }
}

impl AddAssign<&Self> for SvgExportReport {
    fn add_assign(&mut self, rhs: &Self) {
        self.merge(rhs);
    }
}

impl AddAssign for SvgExportReport {
    fn add_assign(&mut self, rhs: Self) {
        self.merge(&rhs);
    }
}

/// Options for one safely nested SVG group.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgGroupOptions {
    opacity: f64,
    id: Option<String>,
    title: Option<String>,
    transform: Option<[f64; 6]>,
}

impl Default for SvgGroupOptions {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            id: None,
            title: None,
            transform: None,
        }
    }
}

impl SvgGroupOptions {
    /// Sets the group's opacity. It is validated when the group is written.
    #[must_use]
    pub const fn with_opacity(mut self, opacity: f64) -> Self {
        self.opacity = opacity;
        self
    }

    /// Adds an SVG `id` attribute.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Adds an escaped child `title` element.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Applies an SVG affine matrix to the group.
    ///
    /// Values use SVG's `matrix(a b c d e f)` order.
    #[must_use]
    pub const fn with_transform_matrix(mut self, matrix: [f64; 6]) -> Self {
        self.transform = Some(matrix);
        self
    }

    /// Group opacity in the inclusive range `0.0..=1.0`.
    #[must_use]
    pub const fn opacity(&self) -> f64 {
        self.opacity
    }

    /// Optional SVG identifier.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Optional accessible group title.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Optional SVG affine matrix in `matrix(a b c d e f)` order.
    #[must_use]
    pub const fn transform_matrix(&self) -> Option<[f64; 6]> {
        self.transform
    }
}

/// Streaming SVG document writer.
///
/// Call [`Self::finish`] to write the closing tags and recover the destination.
/// An error can leave a syntactically incomplete document in the destination.
#[must_use = "call SvgWriter::finish to close the SVG document"]
pub struct SvgWriter<W: Write> {
    writer: Writer<W>,
    canvas: SvgCanvas,
    options: SvgOptions,
    report: SvgExportReport,
    next_eraser_group_id: u64,
}

impl<W: Write> SvgWriter<W> {
    /// Starts a new SVG document on `destination`.
    pub fn new(destination: W, canvas: SvgCanvas, options: SvgOptions) -> Result<Self> {
        validate_options(&options)?;
        let mut writer = Writer::new_with_indent(destination, b' ', 2);
        writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

        let width = svg_number(canvas.width);
        let height = svg_number(canvas.height);
        let viewport = canvas.viewport;
        let view_box = format!(
            "{} {} {} {}",
            svg_number(viewport.min_x),
            svg_number(viewport.min_y),
            svg_number(viewport.width),
            svg_number(viewport.height)
        );
        let mut svg = BytesStart::new("svg");
        svg.push_attribute(("xmlns", "http://www.w3.org/2000/svg"));
        svg.push_attribute(("xmlns:xlink", "http://www.w3.org/1999/xlink"));
        svg.push_attribute(("width", width.as_str()));
        svg.push_attribute(("height", height.as_str()));
        svg.push_attribute(("viewBox", view_box.as_str()));
        writer.write_event(Event::Start(svg))?;

        if let Some(title) = options.title() {
            writer.write_event(Event::Start(BytesStart::new("title")))?;
            writer.write_event(Event::Text(BytesText::new(title)))?;
            writer.write_event(Event::End(BytesEnd::new("title")))?;
        }

        let group_opacity = svg_number(options.group_opacity);
        let mut group = BytesStart::new("g");
        group.push_attribute(("fill", "none"));
        group.push_attribute(("stroke-linecap", "round"));
        group.push_attribute(("stroke-linejoin", "round"));
        group.push_attribute(("opacity", group_opacity.as_str()));
        writer.write_event(Event::Start(group))?;

        Ok(Self {
            writer,
            canvas,
            options,
            report: SvgExportReport::default(),
            next_eraser_group_id: 0,
        })
    }

    /// Writes every stroke from one validated vector body.
    pub fn write_vector(&mut self, vector: &VectorData) -> Result<()> {
        self.write_vector_with_brush_styles(vector, |_| None)
    }

    /// Writes a vector body while resolving its referenced core brush styles.
    pub fn write_vector_with_brush_styles<'a>(
        &mut self,
        vector: &VectorData,
        mut resolve: impl FnMut(u32) -> Option<&'a BrushStyle>,
    ) -> Result<()> {
        self.report.vector_objects += 1;
        let strokes = vector.strokes().collect::<Vec<_>>();
        let brushes = strokes
            .iter()
            .map(|stroke| {
                stroke
                    .style()
                    .and_then(|style| style.brush_style_id())
                    .and_then(&mut resolve)
            })
            .collect::<Vec<_>>();
        let has_eraser = strokes
            .iter()
            .zip(&brushes)
            .any(|(stroke, brush)| resolved_brush_is_eraser(stroke.style(), *brush));

        if has_eraser {
            let mut operations = Vec::new();
            let mut index = 0;
            while index < strokes.len() {
                if self.options.one_point_stroke_handling()
                    == SvgOnePointStrokeHandling::ConnectNearby
                    && let Some(run) = connected_one_point_run(
                        &strokes,
                        index,
                        self.options.nominal_width_adjustment(),
                    )
                {
                    let brush = brushes[index];
                    let is_eraser = resolved_brush_is_eraser(Some(run.style), brush);
                    let end_index = run.end_index;
                    operations.push(VectorWriteOperation::Connected {
                        run,
                        brush,
                        is_eraser,
                    });
                    index = end_index;
                    continue;
                }
                let stroke = strokes[index];
                let brush = brushes[index];
                operations.push(VectorWriteOperation::Stroke {
                    stroke,
                    brush,
                    is_eraser: resolved_brush_is_eraser(stroke.style(), brush),
                });
                index += 1;
            }
            return self.write_eraser_composited_vector(&operations);
        }

        let mut index = 0;
        while index < strokes.len() {
            if self.options.one_point_stroke_handling() == SvgOnePointStrokeHandling::ConnectNearby
                && let Some(run) = connected_one_point_run(
                    &strokes,
                    index,
                    self.options.nominal_width_adjustment(),
                )
            {
                let brush = brushes[index];
                self.write_connected_one_point_run(&run, brush)?;
                index = run.end_index;
                continue;
            }
            let stroke = strokes[index];
            let brush = brushes[index];
            self.write_stroke_with_brush_style(stroke, brush)?;
            index += 1;
        }
        Ok(())
    }

    fn write_eraser_composited_vector(
        &mut self,
        operations: &[VectorWriteOperation<'_, '_>],
    ) -> Result<()> {
        let group_number = self.next_eraser_group_id;
        self.next_eraser_group_id = self.next_eraser_group_id.saturating_add(1);
        let prefix = format!("clipfile-svg-eraser-{group_number}");
        let eraser_indices = operations
            .iter()
            .enumerate()
            .filter_map(|(index, operation)| operation.is_eraser().then_some(index))
            .collect::<Vec<_>>();

        self.writer
            .write_event(Event::Start(BytesStart::new("defs")))?;
        self.write_eraser_color_filter(&prefix)?;
        for (index, operation) in operations.iter().enumerate() {
            let id = format!("{prefix}-operation-{index}");
            let mut group = BytesStart::new("g");
            group.push_attribute(("id", id.as_str()));
            self.writer.write_event(Event::Start(group))?;
            match operation {
                VectorWriteOperation::Stroke { stroke, brush, .. } => {
                    self.write_stroke_with_brush_style(*stroke, *brush)?;
                }
                VectorWriteOperation::Connected { run, brush, .. } => {
                    self.write_connected_one_point_run(run, *brush)?;
                }
            }
            self.writer.write_event(Event::End(BytesEnd::new("g")))?;
        }
        for &eraser_index in &eraser_indices {
            self.write_eraser_mask(&prefix, eraser_index, &eraser_indices)?;
        }
        self.writer.write_event(Event::End(BytesEnd::new("defs")))?;

        let mut segment_start = 0;
        for &eraser_index in &eraser_indices {
            self.write_paint_operation_segment(
                &prefix,
                operations,
                segment_start,
                eraser_index,
                Some(eraser_index),
            )?;
            segment_start = eraser_index + 1;
        }
        self.write_paint_operation_segment(
            &prefix,
            operations,
            segment_start,
            operations.len(),
            None,
        )
    }

    fn write_eraser_color_filter(&mut self, prefix: &str) -> Result<()> {
        let viewport = self.canvas.viewport();
        let id = format!("{prefix}-black");
        let x = svg_number(viewport.min_x());
        let y = svg_number(viewport.min_y());
        let width = svg_number(viewport.width());
        let height = svg_number(viewport.height());
        let mut filter = BytesStart::new("filter");
        filter.push_attribute(("id", id.as_str()));
        filter.push_attribute(("filterUnits", "userSpaceOnUse"));
        filter.push_attribute(("x", x.as_str()));
        filter.push_attribute(("y", y.as_str()));
        filter.push_attribute(("width", width.as_str()));
        filter.push_attribute(("height", height.as_str()));
        filter.push_attribute(("color-interpolation-filters", "sRGB"));
        self.writer.write_event(Event::Start(filter))?;
        let mut matrix = BytesStart::new("feColorMatrix");
        matrix.push_attribute(("type", "matrix"));
        matrix.push_attribute(("values", "0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0"));
        self.writer.write_event(Event::Empty(matrix))?;
        self.writer
            .write_event(Event::End(BytesEnd::new("filter")))?;
        Ok(())
    }

    fn write_eraser_mask(
        &mut self,
        prefix: &str,
        eraser_index: usize,
        eraser_indices: &[usize],
    ) -> Result<()> {
        let viewport = self.canvas.viewport();
        let id = format!("{prefix}-mask-{eraser_index}");
        let x = svg_number(viewport.min_x());
        let y = svg_number(viewport.min_y());
        let width = svg_number(viewport.width());
        let height = svg_number(viewport.height());
        let mut mask = BytesStart::new("mask");
        mask.push_attribute(("id", id.as_str()));
        mask.push_attribute(("maskUnits", "userSpaceOnUse"));
        mask.push_attribute(("maskContentUnits", "userSpaceOnUse"));
        mask.push_attribute(("mask-type", "luminance"));
        mask.push_attribute(("x", x.as_str()));
        mask.push_attribute(("y", y.as_str()));
        mask.push_attribute(("width", width.as_str()));
        mask.push_attribute(("height", height.as_str()));
        self.writer.write_event(Event::Start(mask))?;

        let mut background = BytesStart::new("rect");
        background.push_attribute(("x", x.as_str()));
        background.push_attribute(("y", y.as_str()));
        background.push_attribute(("width", width.as_str()));
        background.push_attribute(("height", height.as_str()));
        background.push_attribute(("fill", "white"));
        self.writer.write_event(Event::Empty(background))?;

        let filter = format!("url(#{prefix}-black)");
        for &future_index in eraser_indices
            .iter()
            .filter(|&&future_index| future_index >= eraser_index)
        {
            let href = format!("#{prefix}-operation-{future_index}");
            let mut use_element = BytesStart::new("use");
            use_element.push_attribute(("href", href.as_str()));
            use_element.push_attribute(("xlink:href", href.as_str()));
            use_element.push_attribute(("filter", filter.as_str()));
            self.writer.write_event(Event::Empty(use_element))?;
        }
        self.writer.write_event(Event::End(BytesEnd::new("mask")))?;
        Ok(())
    }

    fn write_paint_operation_segment(
        &mut self,
        prefix: &str,
        operations: &[VectorWriteOperation<'_, '_>],
        start: usize,
        end: usize,
        eraser_mask: Option<usize>,
    ) -> Result<()> {
        if start >= end {
            return Ok(());
        }
        let has_paint = operations[start..end]
            .iter()
            .any(|operation| !operation.is_eraser());
        if !has_paint {
            return Ok(());
        }

        if let Some(eraser_index) = eraser_mask {
            let mask = format!("url(#{prefix}-mask-{eraser_index})");
            let mut group = BytesStart::new("g");
            group.push_attribute(("mask", mask.as_str()));
            self.writer.write_event(Event::Start(group))?;
        }
        for (index, operation) in operations.iter().enumerate().take(end).skip(start) {
            if operation.is_eraser() {
                continue;
            }
            let href = format!("#{prefix}-operation-{index}");
            let mut use_element = BytesStart::new("use");
            use_element.push_attribute(("href", href.as_str()));
            use_element.push_attribute(("xlink:href", href.as_str()));
            self.writer.write_event(Event::Empty(use_element))?;
        }
        if eraser_mask.is_some() {
            self.writer.write_event(Event::End(BytesEnd::new("g")))?;
        }
        Ok(())
    }

    /// Writes a balanced nested SVG group through a callback.
    ///
    /// The callback may use an application error type that implements
    /// `From<Error>`. The closing tag is attempted even when the callback
    /// returns an error. An underlying I/O failure can still leave the stream
    /// incomplete, after which the writer should be discarded.
    pub fn write_group<E>(
        &mut self,
        options: SvgGroupOptions,
        write_contents: impl FnOnce(&mut Self) -> std::result::Result<(), E>,
    ) -> std::result::Result<(), E>
    where
        E: From<Error>,
    {
        validate_group_options(&options).map_err(E::from)?;
        let opacity = svg_number(options.opacity);
        let mut group = BytesStart::new("g");
        group.push_attribute(("opacity", opacity.as_str()));
        if let Some(id) = options.id() {
            group.push_attribute(("id", id));
        }
        let transform = options.transform_matrix().map(|matrix| {
            format!(
                "matrix({} {} {} {} {} {})",
                svg_number(matrix[0]),
                svg_number(matrix[1]),
                svg_number(matrix[2]),
                svg_number(matrix[3]),
                svg_number(matrix[4]),
                svg_number(matrix[5]),
            )
        });
        if let Some(transform) = &transform {
            group.push_attribute(("transform", transform.as_str()));
        }
        self.writer
            .write_event(Event::Start(group))
            .map_err(Error::from)
            .map_err(E::from)?;
        if let Some(title) = options.title() {
            self.writer
                .write_event(Event::Start(BytesStart::new("title")))
                .map_err(Error::from)
                .map_err(E::from)?;
            self.writer
                .write_event(Event::Text(BytesText::new(title)))
                .map_err(Error::from)
                .map_err(E::from)?;
            self.writer
                .write_event(Event::End(BytesEnd::new("title")))
                .map_err(Error::from)
                .map_err(E::from)?;
        }

        let body_result = write_contents(self);
        let close_result = self
            .writer
            .write_event(Event::End(BytesEnd::new("g")))
            .map_err(Error::from)
            .map_err(E::from);
        match (body_result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(()), result) => result,
        }
    }

    /// Writes vector objects inside one safely balanced nested SVG group.
    pub fn write_vectors_group<'a, I>(&mut self, vectors: I, options: SvgGroupOptions) -> Result<()>
    where
        I: IntoIterator<Item = &'a VectorData>,
    {
        self.write_group(options, |writer| -> Result<()> {
            for vector in vectors {
                writer.write_vector(vector)?;
            }
            Ok(())
        })
    }

    /// Writes one vector stroke, or records that it was skipped.
    pub fn write_stroke(&mut self, stroke: VectorStroke<'_>) -> Result<()> {
        self.write_stroke_with_brush_style(stroke, None)
    }

    /// Writes one vector stroke with its resolved core brush style.
    ///
    /// Supplying the matching [`BrushStyle`] lets the adapter distinguish a
    /// plain hard round tip from brush shapes that still require a rendered
    /// texture or dab sequence. A missing or mismatched style remains a
    /// reported [`SvgLossKind::BrushToRoundStroke`] conversion.
    pub fn write_stroke_with_brush_style(
        &mut self,
        stroke: VectorStroke<'_>,
        brush_style: Option<&BrushStyle>,
    ) -> Result<()> {
        let stroke_index = self.report.strokes_seen;
        self.report.strokes_seen += 1;

        if !stroke.is_semantically_supported() {
            return self
                .skip_unsupported_stroke(stroke_index, SvgSkipKind::UnsupportedSemanticLayout);
        }

        let (curve, approximated_spline) = match stroke.curve_kind() {
            Some(VectorCurveKind::Straight) => (CurveMode::Straight, false),
            Some(VectorCurveKind::Quadratic) => (CurveMode::Quadratic, false),
            Some(VectorCurveKind::Cubic) => (CurveMode::Cubic, false),
            Some(VectorCurveKind::Spline) => (CurveMode::Spline, true),
            _ => {
                return self
                    .skip_unsupported_stroke(stroke_index, SvgSkipKind::UnsupportedCurveKind);
            }
        };

        let built = build_stroke(
            stroke,
            curve,
            self.options.spline_tolerance(),
            self.options.spline_conversion(),
        );
        if built.incomplete_geometry {
            return self.skip_unsupported_stroke(stroke_index, SvgSkipKind::IncompleteGeometry);
        }
        if matches!(built.mark, BuiltMark::Empty) {
            self.record_skip(SvgSkipKind::EmptyStroke);
            return Ok(());
        }

        let mut losses = StrokeLosses::default();
        let is_path = matches!(built.mark, BuiltMark::Path(_));
        if approximated_spline && is_path {
            losses.add(match self.options.spline_conversion() {
                SvgSplineConversion::Polyline => SvgLossKind::SplineToPolyline,
                SvgSplineConversion::CubicFit => SvgLossKind::SplineToCubicApproximation,
            });
        }
        if built.bezier_fallback {
            losses.add(SvgLossKind::BezierControlsToLine);
        }
        if built.spline_fit_fallback && is_path {
            losses.add(SvgLossKind::SplineFitSegmentToLine);
        }
        add_factor_representation_losses(
            &mut losses,
            built.width.variable,
            built.opacity.variable,
            is_path,
            self.options.stroke_representation(),
        );
        if built.width.incomplete {
            losses.add(SvgLossKind::MissingWidthFactorFallback);
        }
        if built.opacity.incomplete {
            losses.add(SvgLossKind::MissingOpacityFactorFallback);
        }

        let style = stroke
            .style()
            .expect("a semantically supported vector stroke has a style");
        let resolved_brush_is_round = style.brush_style_id().is_some_and(|id| {
            brush_style.is_some_and(|brush| brush.id() == id && brush.is_plain_round_tip())
        });
        if !resolved_brush_is_round {
            losses.add(SvgLossKind::BrushToRoundStroke);
        }
        let color = style.main_color().rgb_8bit();
        if style.sub_color().rgb_8bit() != color {
            losses.add(SvgLossKind::SecondaryColorIgnored);
        }
        if style.fill_style_id().is_some() {
            losses.add(SvgLossKind::FrameFillIgnored);
        }
        let nominal_width =
            match adjusted_nominal_width(style, self.options.nominal_width_adjustment())? {
                Some(width) => width,
                None => {
                    losses.add(SvgLossKind::MissingWidthFallback);
                    1.0
                }
            };
        let preserve_appearance =
            self.options.stroke_representation() == SvgStrokeRepresentation::PreserveAppearance;
        let stroke_width = if preserve_appearance && built.width.variable {
            nominal_width
        } else {
            finite_svg_product(nominal_width, built.width.value, "stroke width")?
        };
        let stroke_opacity = if preserve_appearance && built.opacity.variable {
            style.opacity()
        } else {
            style.opacity() * built.opacity.value
        }
        .clamp(0.0, 1.0);
        self.apply_losses(&losses, stroke_index)?;

        let rendered_style = RenderStyle {
            color,
            width: stroke_width.max(0.0),
            opacity: stroke_opacity,
        };
        let smooth_spline_outline = curve != CurveMode::Spline
            || self.options.spline_conversion() == SvgSplineConversion::CubicFit;
        let written = if preserve_appearance && built.opacity.variable && is_path {
            write_variable_opacity_segments(
                &mut self.writer,
                &built.outline_samples,
                rendered_style,
                VariableOpacityOptions {
                    nominal_width,
                    stroke_index,
                    tolerance: self.options.spline_tolerance(),
                    closed: built.closed,
                    smooth: smooth_spline_outline,
                },
            )?
        } else if preserve_appearance && built.width.variable && is_path {
            let outline = build_variable_width_outline(
                &built.outline_samples,
                nominal_width,
                self.options.spline_tolerance(),
                built.closed,
                smooth_spline_outline,
            )?;
            write_filled_path(&mut self.writer, &outline, rendered_style)?
        } else {
            write_mark(&mut self.writer, &built.mark, rendered_style)?
        };
        match written {
            WrittenMark::CenterlinePath => {
                self.report.paths_written += 1;
                self.report.centerline_paths_written += 1;
            }
            WrittenMark::FilledOutlinePath(count) => {
                self.report.paths_written += count;
                self.report.filled_outline_paths_written += count;
            }
            WrittenMark::Dot => self.report.dots_written += 1,
        }
        self.report.strokes_written += 1;
        Ok(())
    }

    fn write_connected_one_point_run(
        &mut self,
        run: &ConnectedOnePointRun,
        brush_style: Option<&BrushStyle>,
    ) -> Result<()> {
        let stroke_index = self.report.strokes_seen;
        let source_count = u64::try_from(run.samples.len()).unwrap_or(u64::MAX);
        self.report.strokes_seen = self.report.strokes_seen.saturating_add(source_count);

        let (mark, spline_fit_fallback) = build_spline_mark(
            &run.bridge_points,
            false,
            self.options.spline_tolerance(),
            self.options.spline_conversion(),
        );
        if !matches!(mark, BuiltMark::Path(_)) {
            return Err(Error::InvalidSvgNumber {
                field: "connected one-point stroke path",
                value: run.bridge_points.len() as f64,
            });
        }

        let mut width = FactorAccumulator::default();
        let mut opacity = FactorAccumulator::default();
        for sample in &run.samples {
            width.push(sample.width_factor);
            opacity.push(sample.opacity_factor);
        }
        let width = width.finish();
        let opacity = opacity.finish();

        let mut losses = StrokeLosses::default();
        losses.add(SvgLossKind::AdjacentOnePointStrokesConnected);
        losses.add(match self.options.spline_conversion() {
            SvgSplineConversion::Polyline => SvgLossKind::SplineToPolyline,
            SvgSplineConversion::CubicFit => SvgLossKind::SplineToCubicApproximation,
        });
        if spline_fit_fallback {
            losses.add(SvgLossKind::SplineFitSegmentToLine);
        }
        let preserve_appearance =
            self.options.stroke_representation() == SvgStrokeRepresentation::PreserveAppearance;
        add_factor_representation_losses(
            &mut losses,
            width.variable,
            opacity.variable,
            true,
            self.options.stroke_representation(),
        );
        if width.incomplete {
            losses.add(SvgLossKind::MissingWidthFactorFallback);
        }
        if opacity.incomplete {
            losses.add(SvgLossKind::MissingOpacityFactorFallback);
        }
        let resolved_brush_is_round = run.style.brush_style_id().is_some_and(|id| {
            brush_style.is_some_and(|brush| brush.id() == id && brush.is_plain_round_tip())
        });
        if !resolved_brush_is_round {
            losses.add(SvgLossKind::BrushToRoundStroke);
        }
        let color = run.style.main_color().rgb_8bit();
        if run.style.sub_color().rgb_8bit() != color {
            losses.add(SvgLossKind::SecondaryColorIgnored);
        }
        if run.style.fill_style_id().is_some() {
            losses.add(SvgLossKind::FrameFillIgnored);
        }
        let nominal_width =
            match adjusted_nominal_width(run.style, self.options.nominal_width_adjustment())? {
                Some(value) => value,
                None => {
                    losses.add(SvgLossKind::MissingWidthFallback);
                    1.0
                }
            };
        self.apply_losses_count(&losses, stroke_index, source_count)?;

        let rendered_style = RenderStyle {
            color,
            width: if preserve_appearance && width.variable {
                nominal_width
            } else {
                finite_svg_product(nominal_width, width.value, "stroke width")?
            }
            .max(0.0),
            opacity: if preserve_appearance && opacity.variable {
                run.style.opacity()
            } else {
                run.style.opacity() * opacity.value
            }
            .clamp(0.0, 1.0),
        };
        let outline_samples = connected_outline_samples(&run.bridge_points, &run.samples);
        let smooth_outline = self.options.spline_conversion() == SvgSplineConversion::CubicFit;
        let written = if preserve_appearance && opacity.variable {
            write_variable_opacity_segments(
                &mut self.writer,
                &outline_samples,
                rendered_style,
                VariableOpacityOptions {
                    nominal_width,
                    stroke_index,
                    tolerance: self.options.spline_tolerance(),
                    closed: false,
                    smooth: smooth_outline,
                },
            )?
        } else if preserve_appearance && width.variable {
            let outline = build_variable_width_outline(
                &outline_samples,
                nominal_width,
                self.options.spline_tolerance(),
                false,
                smooth_outline,
            )?;
            write_filled_path(&mut self.writer, &outline, rendered_style)?
        } else {
            write_mark(&mut self.writer, &mark, rendered_style)?
        };
        match written {
            WrittenMark::CenterlinePath => {
                self.report.paths_written += 1;
                self.report.centerline_paths_written += 1;
            }
            WrittenMark::FilledOutlinePath(count) => {
                self.report.paths_written += count;
                self.report.filled_outline_paths_written += count;
            }
            WrittenMark::Dot => unreachable!("a connected one-point run is a path"),
        }
        self.report.strokes_written = self.report.strokes_written.saturating_add(source_count);
        Ok(())
    }

    /// Returns the report accumulated so far.
    #[must_use]
    pub const fn report(&self) -> &SvgExportReport {
        &self.report
    }

    /// Closes the SVG document and returns the destination and final report.
    pub fn finish(mut self) -> Result<(W, SvgExportReport)> {
        self.writer.write_event(Event::End(BytesEnd::new("g")))?;
        self.writer.write_event(Event::End(BytesEnd::new("svg")))?;
        Ok((self.writer.into_inner(), self.report))
    }

    fn skip_unsupported_stroke(&mut self, stroke_index: u64, skip_kind: SvgSkipKind) -> Result<()> {
        let mut losses = StrokeLosses::default();
        losses.add(SvgLossKind::UnsupportedStrokeSkipped);
        self.apply_losses(&losses, stroke_index)?;
        self.record_skip(skip_kind);
        Ok(())
    }

    fn apply_losses(&mut self, losses: &StrokeLosses, stroke_index: u64) -> Result<()> {
        self.apply_losses_count(losses, stroke_index, 1)
    }

    fn apply_losses_count(
        &mut self,
        losses: &StrokeLosses,
        stroke_index: u64,
        count: u64,
    ) -> Result<()> {
        for kind in losses.iter() {
            if self.options.loss_handling(kind) == SvgLossHandling::Reject {
                self.record_skip_count(SvgSkipKind::PolicyRejected, count);
                return Err(Error::LossyConversion { stroke_index, kind });
            }
        }
        for kind in losses.iter() {
            self.report.losses[kind.index()] =
                self.report.losses[kind.index()].saturating_add(count);
        }
        Ok(())
    }

    fn record_skip(&mut self, kind: SvgSkipKind) {
        self.record_skip_count(kind, 1);
    }

    fn record_skip_count(&mut self, kind: SvgSkipKind, count: u64) {
        self.report.strokes_skipped = self.report.strokes_skipped.saturating_add(count);
        self.report.skips[kind.index()] = self.report.skips[kind.index()].saturating_add(count);
    }
}

/// Writes one complete SVG document from borrowed vector bodies.
pub fn write_svg<'a, W, I>(
    destination: W,
    canvas: SvgCanvas,
    vectors: I,
    options: SvgOptions,
) -> Result<(W, SvgExportReport)>
where
    W: Write,
    I: IntoIterator<Item = &'a VectorData>,
{
    let mut writer = SvgWriter::new(destination, canvas, options)?;
    for vector in vectors {
        writer.write_vector(vector)?;
    }
    writer.finish()
}

fn validate_options(options: &SvgOptions) -> Result<()> {
    if !options.group_opacity.is_finite() || !(0.0..=1.0).contains(&options.group_opacity) {
        return Err(Error::InvalidGroupOpacity {
            opacity: options.group_opacity,
        });
    }
    if !options.spline_tolerance.is_finite() || options.spline_tolerance <= 0.0 {
        return Err(Error::InvalidSplineTolerance {
            tolerance: options.spline_tolerance,
        });
    }
    if !options.nominal_width_adjustment.is_finite() {
        return Err(Error::InvalidNominalWidthAdjustment {
            adjustment: options.nominal_width_adjustment,
        });
    }
    if options
        .title()
        .is_some_and(|title| !title.chars().all(is_xml_10_character))
    {
        return Err(Error::InvalidXmlText);
    }
    Ok(())
}

fn validate_group_options(options: &SvgGroupOptions) -> Result<()> {
    if !options.opacity.is_finite() || !(0.0..=1.0).contains(&options.opacity) {
        return Err(Error::InvalidGroupOpacity {
            opacity: options.opacity,
        });
    }
    if options.title().is_some_and(|text| !is_valid_xml_text(text)) {
        return Err(Error::InvalidXmlText);
    }
    if options.id().is_some_and(|id| {
        id.is_empty() || id.chars().any(char::is_whitespace) || !is_valid_xml_text(id)
    }) {
        return Err(Error::InvalidGroupId);
    }
    if let Some(matrix) = options.transform_matrix()
        && let Some(value) = matrix.into_iter().find(|value| !value.is_finite())
    {
        return Err(Error::InvalidSvgNumber {
            field: "group transform",
            value,
        });
    }
    Ok(())
}

fn is_valid_xml_text(text: &str) -> bool {
    text.chars().all(is_xml_10_character)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StrokeLosses([bool; SvgLossKind::ALL.len()]);

impl StrokeLosses {
    fn add(&mut self, kind: SvgLossKind) {
        self.0[kind.index()] = true;
    }

    fn iter(&self) -> impl Iterator<Item = SvgLossKind> + '_ {
        SvgLossKind::ALL
            .into_iter()
            .filter(|kind| self.0[kind.index()])
    }
}

fn add_factor_representation_losses(
    losses: &mut StrokeLosses,
    width_variable: bool,
    opacity_variable: bool,
    is_path: bool,
    representation: SvgStrokeRepresentation,
) {
    if !is_path {
        return;
    }
    match representation {
        SvgStrokeRepresentation::PreserveAppearance => {
            if width_variable {
                losses.add(SvgLossKind::VariableWidthOutlined);
            }
            if opacity_variable {
                losses.add(SvgLossKind::VariableOpacitySegmented);
            }
        }
        SvgStrokeRepresentation::EditableCenterline => {
            if width_variable {
                losses.add(SvgLossKind::VariableWidthAveraged);
            }
            if opacity_variable {
                losses.add(SvgLossKind::VariableOpacityAveraged);
            }
        }
    }
}

fn is_xml_10_character(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{a}' | '\u{d}')
        || ('\u{20}'..='\u{d7ff}').contains(&character)
        || ('\u{e000}'..='\u{fffd}').contains(&character)
        || ('\u{10000}'..='\u{10ffff}').contains(&character)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurveMode {
    Straight,
    Quadratic,
    Cubic,
    Spline,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Coordinate {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PointData {
    position: Coordinate,
    controls: [Option<Coordinate>; 2],
    control_count: u8,
    flags: u32,
}

#[derive(Clone, Copy, Debug)]
struct OnePointStrokeSample {
    point: PointData,
    width_factor: Option<f32>,
    opacity_factor: Option<f32>,
}

#[derive(Debug)]
struct ConnectedOnePointRun {
    end_index: usize,
    style: VectorStrokeStyle,
    samples: Vec<OnePointStrokeSample>,
    bridge_points: Vec<PointData>,
}

enum VectorWriteOperation<'vector, 'brush> {
    Stroke {
        stroke: VectorStroke<'vector>,
        brush: Option<&'brush BrushStyle>,
        is_eraser: bool,
    },
    Connected {
        run: ConnectedOnePointRun,
        brush: Option<&'brush BrushStyle>,
        is_eraser: bool,
    },
}

impl VectorWriteOperation<'_, '_> {
    const fn is_eraser(&self) -> bool {
        match self {
            Self::Stroke { is_eraser, .. } | Self::Connected { is_eraser, .. } => *is_eraser,
        }
    }
}

fn resolved_brush_is_eraser(style: Option<VectorStrokeStyle>, brush: Option<&BrushStyle>) -> bool {
    style.is_some_and(|style| {
        style
            .brush_style_id()
            .is_some_and(|id| brush.is_some_and(|brush| brush.id() == id && brush.is_eraser()))
    })
}

type NormalizedSplineSample = (PointData, Option<f32>, Option<f32>);

impl PointData {
    fn from_vector(point: VectorPoint<'_>) -> Option<Self> {
        let position = point.position()?;
        let mut controls = [None, None];
        let mut control_count = 0_u8;
        for (index, control) in point.control_points().enumerate() {
            control_count = control_count.saturating_add(1);
            if let Some(slot) = controls.get_mut(index) {
                *slot = Some(Coordinate {
                    x: control.x(),
                    y: control.y(),
                });
            }
        }
        Some(Self {
            position: Coordinate {
                x: position.x(),
                y: position.y(),
            },
            controls,
            control_count,
            flags: point.flags().map_or(0, |flags| flags.raw()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FactorApproximation {
    value: f64,
    variable: bool,
    incomplete: bool,
}

#[derive(Clone, Copy, Debug)]
struct FactorAccumulator {
    sum: f64,
    minimum: f64,
    maximum: f64,
    count: u64,
    missing: u64,
}

impl Default for FactorAccumulator {
    fn default() -> Self {
        Self {
            sum: 0.0,
            minimum: f64::INFINITY,
            maximum: f64::NEG_INFINITY,
            count: 0,
            missing: 0,
        }
    }
}

impl FactorAccumulator {
    fn push(&mut self, value: Option<f32>) {
        if let Some(value) = value {
            let value = f64::from(value);
            self.sum += value;
            self.minimum = self.minimum.min(value);
            self.maximum = self.maximum.max(value);
            self.count += 1;
        } else {
            self.missing += 1;
        }
    }

    fn finish(self) -> FactorApproximation {
        let value = if self.count == 0 {
            1.0
        } else {
            self.sum / self.count as f64
        };
        let tolerance = self.maximum.abs().max(self.minimum.abs()).max(1.0) * 1.0e-6;
        FactorApproximation {
            value,
            variable: self.count > 1 && self.maximum - self.minimum > tolerance,
            incomplete: self.missing != 0,
        }
    }
}

#[derive(Debug)]
struct BuiltStroke {
    mark: BuiltMark,
    width: FactorApproximation,
    opacity: FactorApproximation,
    outline_samples: Vec<OutlineSample>,
    closed: bool,
    incomplete_geometry: bool,
    bezier_fallback: bool,
    spline_fit_fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OutlineSample {
    position: Coordinate,
    width_factor: Option<f32>,
    opacity_factor: Option<f32>,
    flags: u32,
}

fn build_stroke(
    stroke: VectorStroke<'_>,
    curve: CurveMode,
    spline_tolerance: f64,
    spline_conversion: SvgSplineConversion,
) -> BuiltStroke {
    let mut samples = Vec::new();
    let mut incomplete_geometry = false;
    for point in stroke.points() {
        let width_factor = point.width_factor();
        let opacity_factor = point.opacity_factor();
        if let Some(point) = PointData::from_vector(point) {
            samples.push((point, width_factor, opacity_factor));
        } else {
            incomplete_geometry = true;
        }
    }
    if curve == CurveMode::Spline {
        normalize_spline_samples(&mut samples);
    }

    let mut path = PathBuilder::new(spline_tolerance, spline_conversion);
    let mut width = FactorAccumulator::default();
    let mut opacity = FactorAccumulator::default();
    let mut outline_samples = Vec::with_capacity(samples.len());
    for (point, width_factor, opacity_factor) in samples {
        width.push(width_factor);
        opacity.push(opacity_factor);
        outline_samples.push(OutlineSample {
            position: point.position,
            width_factor,
            opacity_factor,
            flags: point.flags,
        });
        path.push(Some(point), curve);
    }
    let mark = path.finish(stroke.is_closed(), curve);
    BuiltStroke {
        mark,
        width: width.finish(),
        opacity: opacity.finish(),
        outline_samples,
        closed: stroke.is_closed(),
        incomplete_geometry: incomplete_geometry || path.incomplete_geometry,
        bezier_fallback: path.bezier_fallback,
        spline_fit_fallback: path.spline_fit_fallback,
    }
}

fn normalize_spline_samples(samples: &mut Vec<(PointData, Option<f32>, Option<f32>)>) {
    if samples
        .first()
        .is_some_and(|(point, _, _)| point.flags == 0x10)
    {
        samples.remove(0);
    }
    if samples
        .last()
        .is_some_and(|(point, _, _)| point.flags == 0x20)
    {
        samples.pop();
    }
    samples.dedup_by(|right, left| {
        if coordinates_nearly_equal(left.0.position, right.0.position) {
            left.0.flags |= right.0.flags;
            left.1 = right.1;
            left.2 = right.2;
            true
        } else {
            false
        }
    });
}

fn normalized_open_spline_samples(stroke: VectorStroke<'_>) -> Option<Vec<NormalizedSplineSample>> {
    if !stroke.is_semantically_supported()
        || stroke.curve_kind() != Some(VectorCurveKind::Spline)
        || stroke.is_closed()
    {
        return None;
    }
    let mut samples = stroke
        .points()
        .map(|point| {
            Some((
                PointData::from_vector(point)?,
                point.width_factor(),
                point.opacity_factor(),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    normalize_spline_samples(&mut samples);
    Some(samples)
}

fn one_point_stroke_sample(
    stroke: VectorStroke<'_>,
) -> Option<(VectorStrokeStyle, OnePointStrokeSample)> {
    let style = stroke.style()?;
    let samples = normalized_open_spline_samples(stroke)?;
    let [(point, width_factor, opacity_factor)] = samples.as_slice() else {
        return None;
    };
    Some((
        style,
        OnePointStrokeSample {
            point: *point,
            width_factor: *width_factor,
            opacity_factor: *opacity_factor,
        },
    ))
}

fn one_point_connection_limit(
    diameter: f64,
    left: OnePointStrokeSample,
    right: OnePointStrokeSample,
) -> f64 {
    let left_factor = f64::from(left.width_factor.unwrap_or(1.0)).max(0.0);
    let right_factor = f64::from(right.width_factor.unwrap_or(1.0)).max(0.0);
    diameter.max(0.0) * left_factor.min(right_factor) * 0.5
}

fn adjusted_nominal_width(style: VectorStrokeStyle, adjustment: f64) -> Result<Option<f64>> {
    let Some(width) = style.brush_diameter() else {
        return Ok(None);
    };
    apply_nominal_width_adjustment(width, adjustment).map(Some)
}

fn apply_nominal_width_adjustment(width: f64, adjustment: f64) -> Result<f64> {
    let adjusted = width + adjustment;
    if !adjusted.is_finite() {
        return Err(Error::InvalidSvgNumber {
            field: "adjusted nominal stroke width",
            value: adjusted,
        });
    }
    Ok(adjusted.max(0.0))
}

fn one_point_samples_are_nearby(
    style: VectorStrokeStyle,
    nominal_width_adjustment: f64,
    left: OnePointStrokeSample,
    right: OnePointStrokeSample,
) -> bool {
    let diameter = adjusted_nominal_width(style, nominal_width_adjustment)
        .ok()
        .flatten()
        .unwrap_or(1.0);
    one_point_samples_are_nearby_for_diameter(diameter, left, right)
}

fn one_point_samples_are_nearby_for_diameter(
    diameter: f64,
    left: OnePointStrokeSample,
    right: OnePointStrokeSample,
) -> bool {
    let limit = one_point_connection_limit(diameter, left, right);
    limit > 0.0
        && length_squared(subtract(left.point.position, right.point.position)) <= limit * limit
}

fn endpoint_is_nearby(
    style: VectorStrokeStyle,
    nominal_width_adjustment: f64,
    endpoint: PointData,
    sample: OnePointStrokeSample,
) -> bool {
    let endpoint_sample = OnePointStrokeSample {
        point: endpoint,
        width_factor: sample.width_factor,
        opacity_factor: sample.opacity_factor,
    };
    one_point_samples_are_nearby(style, nominal_width_adjustment, endpoint_sample, sample)
}

fn push_distinct_point(points: &mut Vec<PointData>, point: PointData) {
    if !points
        .last()
        .is_some_and(|previous| coordinates_nearly_equal(previous.position, point.position))
    {
        points.push(point);
    }
}

fn connected_one_point_run(
    strokes: &[VectorStroke<'_>],
    start_index: usize,
    nominal_width_adjustment: f64,
) -> Option<ConnectedOnePointRun> {
    let (style, first) = one_point_stroke_sample(*strokes.get(start_index)?)?;
    let mut samples = vec![first];
    let mut end_index = start_index + 1;
    while let Some(stroke) = strokes.get(end_index) {
        let Some((next_style, next)) = one_point_stroke_sample(*stroke) else {
            break;
        };
        if next_style != style
            || !one_point_samples_are_nearby(
                style,
                nominal_width_adjustment,
                *samples.last().expect("nonempty"),
                next,
            )
        {
            break;
        }
        samples.push(next);
        end_index += 1;
    }
    if samples.len() < 2 {
        return None;
    }

    let mut bridge_points = Vec::with_capacity(samples.len() + 2);
    if let Some(previous) = start_index
        .checked_sub(1)
        .and_then(|index| strokes.get(index))
        .filter(|stroke| stroke.style() == Some(style))
        .and_then(|stroke| normalized_open_spline_samples(*stroke))
        .and_then(|points| points.last().map(|(point, _, _)| *point))
        .filter(|point| endpoint_is_nearby(style, nominal_width_adjustment, *point, first))
    {
        push_distinct_point(&mut bridge_points, previous);
    }
    for sample in &samples {
        push_distinct_point(&mut bridge_points, sample.point);
    }
    if let Some(next) = strokes
        .get(end_index)
        .filter(|stroke| stroke.style() == Some(style))
        .and_then(|stroke| normalized_open_spline_samples(*stroke))
        .and_then(|points| points.first().map(|(point, _, _)| *point))
        .filter(|point| {
            endpoint_is_nearby(
                style,
                nominal_width_adjustment,
                *point,
                *samples.last().expect("nonempty"),
            )
        })
    {
        push_distinct_point(&mut bridge_points, next);
    }
    (bridge_points.len() >= 2).then_some(ConnectedOnePointRun {
        end_index,
        style,
        samples,
        bridge_points,
    })
}

fn connected_outline_samples(
    bridge_points: &[PointData],
    samples: &[OnePointStrokeSample],
) -> Vec<OutlineSample> {
    let first = samples.first().expect("a connected run is nonempty");
    let last = samples.last().expect("a connected run is nonempty");
    bridge_points
        .iter()
        .map(|point| {
            let factors = samples
                .iter()
                .find(|sample| coordinates_nearly_equal(sample.point.position, point.position))
                .unwrap_or_else(|| {
                    if length_squared(subtract(point.position, first.point.position))
                        <= length_squared(subtract(point.position, last.point.position))
                    {
                        first
                    } else {
                        last
                    }
                });
            OutlineSample {
                position: point.position,
                width_factor: factors.width_factor,
                opacity_factor: factors.opacity_factor,
                flags: point.flags,
            }
        })
        .collect()
}

fn coordinates_nearly_equal(left: Coordinate, right: Coordinate) -> bool {
    let scale = left
        .x
        .abs()
        .max(left.y.abs())
        .max(right.x.abs())
        .max(right.y.abs())
        .max(1.0);
    length_squared(subtract(left, right)) <= (scale * 1.0e-12).max(1.0e-9).powi(2)
}

#[derive(Debug, PartialEq)]
enum BuiltMark {
    Empty,
    Dot(Coordinate),
    Path(String),
}

#[derive(Debug)]
struct PathBuilder {
    data: String,
    first: Option<PointData>,
    previous: Option<PointData>,
    spline_points: Vec<PointData>,
    spline_tolerance: f64,
    segment_count: u64,
    incomplete_geometry: bool,
    bezier_fallback: bool,
    spline_fit_fallback: bool,
    spline_conversion: SvgSplineConversion,
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self::new(1.0, SvgSplineConversion::CubicFit)
    }
}

impl PathBuilder {
    fn new(spline_tolerance: f64, spline_conversion: SvgSplineConversion) -> Self {
        Self {
            data: String::new(),
            first: None,
            previous: None,
            spline_points: Vec::new(),
            spline_tolerance,
            segment_count: 0,
            incomplete_geometry: false,
            bezier_fallback: false,
            spline_fit_fallback: false,
            spline_conversion,
        }
    }

    fn push(&mut self, point: Option<PointData>, curve: CurveMode) {
        let Some(point) = point else {
            self.incomplete_geometry = true;
            self.previous = None;
            return;
        };
        if self.first.is_none() {
            self.first = Some(point);
        }
        if curve == CurveMode::Spline {
            self.spline_points.push(point);
            self.previous = Some(point);
            return;
        }
        if let Some(previous) = self.previous {
            self.bezier_fallback |= append_segment(&mut self.data, previous, point.position, curve);
            self.segment_count += 1;
        } else {
            push_pair_command(&mut self.data, 'M', point.position);
        }
        self.previous = Some(point);
    }

    fn finish(&mut self, closed: bool, curve: CurveMode) -> BuiltMark {
        if curve == CurveMode::Spline {
            let (mark, fallback) = build_spline_mark(
                &self.spline_points,
                closed,
                self.spline_tolerance,
                self.spline_conversion,
            );
            self.spline_fit_fallback = fallback;
            return mark;
        }
        if self.segment_count == 0 {
            return self
                .first
                .map_or(BuiltMark::Empty, |point| BuiltMark::Dot(point.position));
        }
        if closed && !self.incomplete_geometry {
            if matches!(curve, CurveMode::Quadratic | CurveMode::Cubic) {
                let previous = self.previous.expect("a path has a last point");
                let first = self.first.expect("a path has a first point");
                self.bezier_fallback |=
                    append_segment(&mut self.data, previous, first.position, curve);
            }
            self.data.push_str(" Z");
        }
        BuiltMark::Path(std::mem::take(&mut self.data))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CubicSegment {
    first_control: Coordinate,
    second_control: Coordinate,
    end: Coordinate,
}

fn build_spline_mark(
    points: &[PointData],
    closed: bool,
    tolerance: f64,
    conversion: SvgSplineConversion,
) -> (BuiltMark, bool) {
    match points {
        [] => return (BuiltMark::Empty, false),
        [point] => return (BuiltMark::Dot(point.position), false),
        _ => {}
    }
    let mut data = String::new();
    push_pair_command(&mut data, 'M', points[0].position);
    let mut fallback = false;
    match conversion {
        SvgSplineConversion::Polyline => {
            for point in &points[1..] {
                push_pair_command(&mut data, 'L', point.position);
            }
        }
        SvgSplineConversion::CubicFit => {
            let fitted = fit_spline_points(points, closed, tolerance);
            fallback = fitted.fallback;
            for segment in fitted.segments {
                match segment {
                    FittedSegment::Cubic(segment) => append_cubic(&mut data, segment),
                    FittedSegment::Line(end) => push_pair_command(&mut data, 'L', end),
                }
            }
        }
    }
    if closed {
        data.push_str(" Z");
    }
    (BuiltMark::Path(data), fallback)
}

#[derive(Debug)]
struct FittedSegments {
    segments: Vec<FittedSegment>,
    fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FittedSegment {
    Cubic(CubicSegment),
    Line(Coordinate),
}

fn fit_spline_points(points: &[PointData], closed: bool, tolerance: f64) -> FittedSegments {
    if closed {
        return fit_closed_spline(points, tolerance);
    }

    let mut boundaries = vec![0];
    for index in 1..points.len() - 1 {
        if is_fit_boundary(points, index, false) {
            boundaries.push(index);
        }
    }
    boundaries.push(points.len() - 1);

    let mut result = FittedSegments {
        segments: Vec::new(),
        fallback: false,
    };
    for pair in boundaries.windows(2) {
        let positions = points[pair[0]..=pair[1]]
            .iter()
            .map(|point| point.position)
            .collect::<Vec<_>>();
        let fitted = fit_cubic_beziers_guarded(&positions, tolerance);
        result.segments.extend(fitted.segments);
        result.fallback |= fitted.fallback;
    }
    result
}

fn fit_closed_spline(points: &[PointData], tolerance: f64) -> FittedSegments {
    let mut result = FittedSegments {
        segments: Vec::with_capacity(points.len()),
        fallback: false,
    };
    for index in 0..points.len() {
        let previous = points[(index + points.len() - 1) % points.len()].position;
        let start = points[index].position;
        let end_index = (index + 1) % points.len();
        let end = points[end_index].position;
        let next = points[(index + 2) % points.len()].position;
        let chord = length(subtract(end, start));
        let start_boundary = is_fit_boundary(points, index, true);
        let end_boundary = is_fit_boundary(points, end_index, true);
        let start_tangent = if start_boundary {
            normalized(subtract(end, start))
        } else {
            normalized(subtract(end, previous))
        }
        .unwrap_or(Coordinate { x: 1.0, y: 0.0 });
        let end_tangent = if end_boundary {
            normalized(subtract(start, end))
        } else {
            normalized(subtract(start, next))
        }
        .unwrap_or(Coordinate { x: -1.0, y: 0.0 });
        let segment = CubicSegment {
            first_control: add(start, scale(start_tangent, chord / 3.0)),
            second_control: add(end, scale(end_tangent, chord / 3.0)),
            end,
        };
        let reference = [start, end];
        if cubic_geometry_is_safe(start, segment, &reference, tolerance) {
            result.segments.push(FittedSegment::Cubic(segment));
        } else {
            result.segments.push(FittedSegment::Line(end));
            result.fallback = true;
        }
    }
    result
}

fn is_fit_boundary(points: &[PointData], index: usize, closed: bool) -> bool {
    if VectorPointFlags::from_raw(points[index].flags).is_corner() {
        return true;
    }
    if !closed && (index == 0 || index + 1 == points.len()) {
        return false;
    }
    let previous = if index == 0 {
        points[points.len() - 1].position
    } else {
        points[index - 1].position
    };
    let next = points[(index + 1) % points.len()].position;
    let incoming = normalized(subtract(points[index].position, previous));
    let outgoing = normalized(subtract(next, points[index].position));
    matches!((incoming, outgoing), (Some(left), Some(right)) if dot(left, right) < -0.95)
}

fn fit_cubic_beziers_guarded(points: &[Coordinate], tolerance: f64) -> FittedSegments {
    debug_assert!(points.len() >= 2);
    let left_tangent =
        normalized(subtract(points[1], points[0])).unwrap_or(Coordinate { x: 1.0, y: 0.0 });
    let right_tangent = normalized(subtract(points[points.len() - 2], points[points.len() - 1]))
        .unwrap_or(Coordinate { x: -1.0, y: 0.0 });
    let mut segments = Vec::new();
    let mut fallback = false;
    fit_cubic_range(
        points,
        left_tangent,
        right_tangent,
        tolerance * tolerance,
        0,
        &mut segments,
        &mut fallback,
    );
    FittedSegments { segments, fallback }
}

fn fit_cubic_range(
    points: &[Coordinate],
    left_tangent: Coordinate,
    right_tangent: Coordinate,
    error_squared: f64,
    depth: u32,
    segments: &mut Vec<FittedSegment>,
    fallback: &mut bool,
) {
    if points.len() == 2 {
        let candidate = line_cubic(points[0], points[1], left_tangent, right_tangent);
        if cubic_geometry_is_safe(points[0], candidate, points, error_squared.sqrt()) {
            segments.push(FittedSegment::Cubic(candidate));
        } else {
            segments.push(FittedSegment::Line(points[1]));
            *fallback = true;
        }
        return;
    }
    if depth >= 32 {
        for pair in points.windows(2) {
            let left = normalized(subtract(pair[1], pair[0])).unwrap_or(left_tangent);
            let _ = left;
            segments.push(FittedSegment::Line(pair[1]));
        }
        *fallback = true;
        return;
    }

    let mut parameters = chord_length_parameters(points);
    let mut cubic = generate_cubic(points, &parameters, left_tangent, right_tangent);
    let (mut maximum_error, mut split) = maximum_fitting_error(points, &parameters, cubic);
    let mut guard_failed = !cubic_geometry_is_safe(points[0], cubic, points, error_squared.sqrt());
    if maximum_error <= error_squared && !guard_failed {
        segments.push(FittedSegment::Cubic(cubic));
        return;
    }

    if maximum_error <= error_squared * 4.0 {
        for _ in 0..4 {
            let Some(reparameterized) = reparameterize(points, &parameters, cubic) else {
                break;
            };
            parameters = reparameterized;
            cubic = generate_cubic(points, &parameters, left_tangent, right_tangent);
            (maximum_error, split) = maximum_fitting_error(points, &parameters, cubic);
            guard_failed = !cubic_geometry_is_safe(points[0], cubic, points, error_squared.sqrt());
            if maximum_error <= error_squared && !guard_failed {
                segments.push(FittedSegment::Cubic(cubic));
                return;
            }
        }
    }

    if guard_failed && points.len() == 3 {
        for pair in points.windows(2) {
            segments.push(FittedSegment::Line(pair[1]));
        }
        *fallback = true;
        return;
    }

    split = split.clamp(1, points.len() - 2);
    let center_tangent = normalized(subtract(points[split - 1], points[split + 1]))
        .or_else(|| normalized(subtract(points[split - 1], points[split])))
        .unwrap_or(right_tangent);
    fit_cubic_range(
        &points[..=split],
        left_tangent,
        center_tangent,
        error_squared,
        depth + 1,
        segments,
        fallback,
    );
    fit_cubic_range(
        &points[split..],
        negate(center_tangent),
        right_tangent,
        error_squared,
        depth + 1,
        segments,
        fallback,
    );
}

fn line_cubic(
    start: Coordinate,
    end: Coordinate,
    left_tangent: Coordinate,
    right_tangent: Coordinate,
) -> CubicSegment {
    let distance = length(subtract(end, start)) / 3.0;
    CubicSegment {
        first_control: add(start, scale(left_tangent, distance)),
        second_control: add(end, scale(right_tangent, distance)),
        end,
    }
}

fn chord_length_parameters(points: &[Coordinate]) -> Vec<f64> {
    let mut parameters = Vec::with_capacity(points.len());
    parameters.push(0.0);
    for pair in points.windows(2) {
        parameters.push(parameters.last().copied().unwrap() + length(subtract(pair[1], pair[0])));
    }
    let total = parameters.last().copied().unwrap_or(0.0);
    if total > 0.0 {
        for parameter in &mut parameters[1..] {
            *parameter /= total;
        }
    } else {
        let denominator = (points.len() - 1) as f64;
        for (index, parameter) in parameters.iter_mut().enumerate() {
            *parameter = index as f64 / denominator;
        }
    }
    parameters
}

fn generate_cubic(
    points: &[Coordinate],
    parameters: &[f64],
    left_tangent: Coordinate,
    right_tangent: Coordinate,
) -> CubicSegment {
    let start = points[0];
    let end = points[points.len() - 1];
    let mut c00 = 0.0;
    let mut c01 = 0.0;
    let mut c11 = 0.0;
    let mut x0 = 0.0;
    let mut x1 = 0.0;

    for (&point, &u) in points.iter().zip(parameters) {
        let one_minus = 1.0 - u;
        let b0 = one_minus * one_minus * one_minus;
        let b1 = 3.0 * u * one_minus * one_minus;
        let b2 = 3.0 * u * u * one_minus;
        let b3 = u * u * u;
        let a1 = scale(left_tangent, b1);
        let a2 = scale(right_tangent, b2);
        let base = add(scale(start, b0 + b1), scale(end, b2 + b3));
        let residual = subtract(point, base);
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        x0 += dot(a1, residual);
        x1 += dot(a2, residual);
    }

    let determinant = c00 * c11 - c01 * c01;
    let (mut alpha_left, mut alpha_right) = if determinant.abs() > 1.0e-12 {
        (
            (x0 * c11 - x1 * c01) / determinant,
            (c00 * x1 - c01 * x0) / determinant,
        )
    } else {
        (0.0, 0.0)
    };
    let segment_length = length(subtract(end, start));
    let polyline_length = points
        .windows(2)
        .map(|pair| length(subtract(pair[1], pair[0])))
        .sum::<f64>();
    let minimum_alpha = segment_length * 1.0e-6;
    if !alpha_left.is_finite()
        || !alpha_right.is_finite()
        || alpha_left < minimum_alpha
        || alpha_right < minimum_alpha
    {
        alpha_left = segment_length / 3.0;
        alpha_right = segment_length / 3.0;
    }
    let maximum_alpha = polyline_length.max(segment_length) * 2.0;
    alpha_left = alpha_left.min(maximum_alpha);
    alpha_right = alpha_right.min(maximum_alpha);

    CubicSegment {
        first_control: add(start, scale(left_tangent, alpha_left)),
        second_control: add(end, scale(right_tangent, alpha_right)),
        end,
    }
}

fn maximum_fitting_error(
    points: &[Coordinate],
    parameters: &[f64],
    cubic: CubicSegment,
) -> (f64, usize) {
    let start = points[0];
    let mut maximum = 0.0;
    let mut split = points.len() / 2;
    for index in 1..points.len() - 1 {
        let fitted = evaluate_cubic(start, cubic, parameters[index]);
        let error = length_squared(subtract(fitted, points[index]));
        if error >= maximum {
            maximum = error;
            split = index;
        }
    }
    (maximum, split)
}

fn cubic_geometry_is_safe(
    start: Coordinate,
    cubic: CubicSegment,
    reference: &[Coordinate],
    tolerance: f64,
) -> bool {
    if !coordinate_is_finite(start)
        || !coordinate_is_finite(cubic.first_control)
        || !coordinate_is_finite(cubic.second_control)
        || !coordinate_is_finite(cubic.end)
        || reference.len() < 2
    {
        return false;
    }
    let reference_length = reference
        .windows(2)
        .map(|pair| length(subtract(pair[1], pair[0])))
        .sum::<f64>();
    let handle_limit = reference_length.max(length(subtract(cubic.end, start))) * 2.0;
    if length(subtract(cubic.first_control, start)) > handle_limit
        || length(subtract(cubic.second_control, cubic.end)) > handle_limit
    {
        return false;
    }
    if !controls_preserve_monotonic_axis(reference, start, cubic, |point| point.x, tolerance)
        || !controls_preserve_monotonic_axis(reference, start, cubic, |point| point.y, tolerance)
    {
        return false;
    }

    let mut flattened = vec![start];
    let flatness = tolerance.min(reference_length.max(1.0) * 0.02).max(1.0e-6);
    if !flatten_cubic(start, cubic, flatness, 0, &mut flattened) {
        return false;
    }
    let (minimum, maximum) = coordinate_bounds(reference);
    if flattened.iter().any(|point| {
        !coordinate_is_finite(*point)
            || point.x < minimum.x - tolerance
            || point.x > maximum.x + tolerance
            || point.y < minimum.y - tolerance
            || point.y > maximum.y + tolerance
            || distance_to_polyline(*point, reference) > tolerance
    }) {
        return false;
    }
    if !preserves_monotonic_axis(reference, &flattened, |point| point.x, tolerance)
        || !preserves_monotonic_axis(reference, &flattened, |point| point.y, tolerance)
    {
        return false;
    }
    !polyline_self_intersects(&flattened)
}

fn controls_preserve_monotonic_axis(
    reference: &[Coordinate],
    start: Coordinate,
    cubic: CubicSegment,
    axis: impl Fn(&Coordinate) -> f64,
    tolerance: f64,
) -> bool {
    let increasing = reference
        .windows(2)
        .all(|pair| axis(&pair[1]) >= axis(&pair[0]));
    let decreasing = reference
        .windows(2)
        .all(|pair| axis(&pair[1]) <= axis(&pair[0]));
    if !increasing && !decreasing {
        return true;
    }
    let controls = [start, cubic.first_control, cubic.second_control, cubic.end];
    controls.windows(2).all(|pair| {
        (!increasing || axis(&pair[1]) + tolerance >= axis(&pair[0]))
            && (!decreasing || axis(&pair[1]) <= axis(&pair[0]) + tolerance)
    })
}

fn coordinate_is_finite(point: Coordinate) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn coordinate_bounds(points: &[Coordinate]) -> (Coordinate, Coordinate) {
    let mut minimum = Coordinate {
        x: f64::INFINITY,
        y: f64::INFINITY,
    };
    let mut maximum = Coordinate {
        x: f64::NEG_INFINITY,
        y: f64::NEG_INFINITY,
    };
    for point in points {
        minimum.x = minimum.x.min(point.x);
        minimum.y = minimum.y.min(point.y);
        maximum.x = maximum.x.max(point.x);
        maximum.y = maximum.y.max(point.y);
    }
    (minimum, maximum)
}

fn flatten_cubic(
    start: Coordinate,
    cubic: CubicSegment,
    tolerance: f64,
    depth: u8,
    output: &mut Vec<Coordinate>,
) -> bool {
    let control_distance = distance_to_line(cubic.first_control, start, cubic.end)
        .max(distance_to_line(cubic.second_control, start, cubic.end));
    if control_distance <= tolerance {
        output.push(evaluate_cubic(start, cubic, 0.5));
        output.push(cubic.end);
        return true;
    }
    if depth >= 8 {
        return false;
    }
    let first = midpoint(start, cubic.first_control);
    let second = midpoint(cubic.first_control, cubic.second_control);
    let third = midpoint(cubic.second_control, cubic.end);
    let fourth = midpoint(first, second);
    let fifth = midpoint(second, third);
    let middle = midpoint(fourth, fifth);
    let first_complete = flatten_cubic(
        start,
        CubicSegment {
            first_control: first,
            second_control: fourth,
            end: middle,
        },
        tolerance,
        depth + 1,
        output,
    );
    let second_complete = flatten_cubic(
        middle,
        CubicSegment {
            first_control: fifth,
            second_control: third,
            end: cubic.end,
        },
        tolerance,
        depth + 1,
        output,
    );
    first_complete && second_complete
}

fn midpoint(left: Coordinate, right: Coordinate) -> Coordinate {
    scale(add(left, right), 0.5)
}

fn distance_to_polyline(point: Coordinate, polyline: &[Coordinate]) -> f64 {
    polyline
        .windows(2)
        .map(|pair| distance_to_segment(point, pair[0], pair[1]))
        .fold(f64::INFINITY, f64::min)
}

fn distance_to_line(point: Coordinate, start: Coordinate, end: Coordinate) -> f64 {
    let direction = subtract(end, start);
    let denominator = length(direction);
    if denominator <= 1.0e-12 {
        length(subtract(point, start))
    } else {
        ((point.x - start.x) * direction.y - (point.y - start.y) * direction.x).abs() / denominator
    }
}

fn distance_to_segment(point: Coordinate, start: Coordinate, end: Coordinate) -> f64 {
    let direction = subtract(end, start);
    let denominator = length_squared(direction);
    if denominator <= 1.0e-24 {
        return length(subtract(point, start));
    }
    let parameter = (dot(subtract(point, start), direction) / denominator).clamp(0.0, 1.0);
    length(subtract(point, add(start, scale(direction, parameter))))
}

fn preserves_monotonic_axis(
    reference: &[Coordinate],
    flattened: &[Coordinate],
    axis: impl Fn(&Coordinate) -> f64,
    tolerance: f64,
) -> bool {
    let increasing = reference
        .windows(2)
        .all(|pair| axis(&pair[1]) >= axis(&pair[0]));
    let decreasing = reference
        .windows(2)
        .all(|pair| axis(&pair[1]) <= axis(&pair[0]));
    if !increasing && !decreasing {
        return true;
    }
    flattened.windows(2).all(|pair| {
        (!increasing || axis(&pair[1]) + tolerance >= axis(&pair[0]))
            && (!decreasing || axis(&pair[1]) <= axis(&pair[0]) + tolerance)
    })
}

fn polyline_self_intersects(points: &[Coordinate]) -> bool {
    for first in 0..points.len().saturating_sub(1) {
        for second in first + 2..points.len().saturating_sub(1) {
            if second == first + 1 {
                continue;
            }
            if segments_intersect(
                points[first],
                points[first + 1],
                points[second],
                points[second + 1],
            ) {
                return true;
            }
        }
    }
    false
}

fn segments_intersect(
    first_start: Coordinate,
    first_end: Coordinate,
    second_start: Coordinate,
    second_end: Coordinate,
) -> bool {
    fn orientation(left: Coordinate, middle: Coordinate, right: Coordinate) -> f64 {
        (middle.x - left.x) * (right.y - left.y) - (middle.y - left.y) * (right.x - left.x)
    }
    let first_a = orientation(first_start, first_end, second_start);
    let first_b = orientation(first_start, first_end, second_end);
    let second_a = orientation(second_start, second_end, first_start);
    let second_b = orientation(second_start, second_end, first_end);
    first_a * first_b < -1.0e-12 && second_a * second_b < -1.0e-12
}

fn reparameterize(
    points: &[Coordinate],
    parameters: &[f64],
    cubic: CubicSegment,
) -> Option<Vec<f64>> {
    let start = points[0];
    let mut result = Vec::with_capacity(points.len());
    let mut previous = -1.0;
    for (&point, &parameter) in points.iter().zip(parameters) {
        let value = newton_parameter(start, cubic, point, parameter).clamp(0.0, 1.0);
        if value <= previous {
            return None;
        }
        result.push(value);
        previous = value;
    }
    Some(result)
}

fn newton_parameter(
    start: Coordinate,
    cubic: CubicSegment,
    point: Coordinate,
    parameter: f64,
) -> f64 {
    let controls = [start, cubic.first_control, cubic.second_control, cubic.end];
    let first = [
        scale(subtract(controls[1], controls[0]), 3.0),
        scale(subtract(controls[2], controls[1]), 3.0),
        scale(subtract(controls[3], controls[2]), 3.0),
    ];
    let second = [
        scale(subtract(first[1], first[0]), 2.0),
        scale(subtract(first[2], first[1]), 2.0),
    ];
    let value = evaluate_cubic(start, cubic, parameter);
    let first_derivative = evaluate_quadratic(first, parameter);
    let second_derivative = evaluate_linear(second, parameter);
    let difference = subtract(value, point);
    let denominator = dot(first_derivative, first_derivative) + dot(difference, second_derivative);
    if denominator.abs() <= 1.0e-12 {
        parameter
    } else {
        parameter - dot(difference, first_derivative) / denominator
    }
}

fn evaluate_cubic(start: Coordinate, cubic: CubicSegment, parameter: f64) -> Coordinate {
    let one_minus = 1.0 - parameter;
    add(
        add(
            scale(start, one_minus * one_minus * one_minus),
            scale(cubic.first_control, 3.0 * one_minus * one_minus * parameter),
        ),
        add(
            scale(
                cubic.second_control,
                3.0 * one_minus * parameter * parameter,
            ),
            scale(cubic.end, parameter * parameter * parameter),
        ),
    )
}

fn evaluate_quadratic(controls: [Coordinate; 3], parameter: f64) -> Coordinate {
    let one_minus = 1.0 - parameter;
    add(
        add(
            scale(controls[0], one_minus * one_minus),
            scale(controls[1], 2.0 * one_minus * parameter),
        ),
        scale(controls[2], parameter * parameter),
    )
}

fn evaluate_linear(controls: [Coordinate; 2], parameter: f64) -> Coordinate {
    add(
        scale(controls[0], 1.0 - parameter),
        scale(controls[1], parameter),
    )
}

fn add(left: Coordinate, right: Coordinate) -> Coordinate {
    Coordinate {
        x: left.x + right.x,
        y: left.y + right.y,
    }
}

fn subtract(left: Coordinate, right: Coordinate) -> Coordinate {
    Coordinate {
        x: left.x - right.x,
        y: left.y - right.y,
    }
}

fn scale(value: Coordinate, factor: f64) -> Coordinate {
    Coordinate {
        x: value.x * factor,
        y: value.y * factor,
    }
}

fn negate(value: Coordinate) -> Coordinate {
    scale(value, -1.0)
}

fn dot(left: Coordinate, right: Coordinate) -> f64 {
    left.x * right.x + left.y * right.y
}

fn length_squared(value: Coordinate) -> f64 {
    dot(value, value)
}

fn length(value: Coordinate) -> f64 {
    length_squared(value).sqrt()
}

fn normalized(value: Coordinate) -> Option<Coordinate> {
    let magnitude = length(value);
    (magnitude > 1.0e-12).then(|| scale(value, 1.0 / magnitude))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderStyle {
    color: [u8; 3],
    width: f64,
    opacity: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WrittenMark {
    CenterlinePath,
    FilledOutlinePath(u64),
    Dot,
}

fn write_mark<W: Write>(
    writer: &mut Writer<W>,
    mark: &BuiltMark,
    style: RenderStyle,
) -> io::Result<WrittenMark> {
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        style.color[0], style.color[1], style.color[2]
    );
    let opacity = svg_number(style.opacity);
    match mark {
        BuiltMark::Empty => unreachable!("an empty mark is skipped before SVG output"),
        BuiltMark::Path(path_data) => {
            let width = svg_number(style.width);
            let mut path = BytesStart::new("path");
            path.push_attribute(("d", path_data.as_str()));
            path.push_attribute(("fill", "none"));
            path.push_attribute(("stroke", color.as_str()));
            path.push_attribute(("stroke-width", width.as_str()));
            path.push_attribute(("stroke-opacity", opacity.as_str()));
            path.push_attribute(("stroke-linecap", "round"));
            path.push_attribute(("stroke-linejoin", "round"));
            writer.write_event(Event::Empty(path))?;
            Ok(WrittenMark::CenterlinePath)
        }
        BuiltMark::Dot(position) => {
            let cx = svg_number(position.x);
            let cy = svg_number(position.y);
            let radius = svg_number(style.width / 2.0);
            let mut circle = BytesStart::new("circle");
            circle.push_attribute(("cx", cx.as_str()));
            circle.push_attribute(("cy", cy.as_str()));
            circle.push_attribute(("r", radius.as_str()));
            circle.push_attribute(("fill", color.as_str()));
            circle.push_attribute(("fill-opacity", opacity.as_str()));
            writer.write_event(Event::Empty(circle))?;
            Ok(WrittenMark::Dot)
        }
    }
}

fn build_variable_width_outline(
    samples: &[OutlineSample],
    nominal_width: f64,
    tolerance: f64,
    closed: bool,
    smooth: bool,
) -> Result<String> {
    let outline = build_sampled_outline(samples, nominal_width, closed)?;
    if outline_has_conversion_intersection(&outline, closed) {
        return Ok(build_segment_ribbons(&outline, closed));
    }
    let mut right = outline.right;
    let left = outline.left;

    let mut data = String::new();
    push_pair_command(&mut data, 'M', left[0]);
    if closed {
        append_outline_side(&mut data, &left, &outline.samples, true, tolerance, smooth);
        data.push_str(" Z");
        right.reverse();
        let mut right_samples = outline.samples.clone();
        right_samples.reverse();
        push_pair_command(&mut data, 'M', right[0]);
        append_outline_side(&mut data, &right, &right_samples, true, tolerance, smooth);
        data.push_str(" Z");
    } else {
        append_outline_side(&mut data, &left, &outline.samples, false, tolerance, smooth);
        append_round_cap(
            &mut data,
            left[left.len() - 1],
            right[right.len() - 1],
            outline.radii[outline.radii.len() - 1],
        );
        right.reverse();
        let mut right_samples = outline.samples.clone();
        right_samples.reverse();
        append_outline_side(&mut data, &right, &right_samples, false, tolerance, smooth);
        append_round_cap(&mut data, right[right.len() - 1], left[0], outline.radii[0]);
        data.push_str(" Z");
    }
    Ok(data)
}

fn outline_has_conversion_intersection(outline: &SampledOutline, closed: bool) -> bool {
    let mut centerline = outline
        .samples
        .iter()
        .map(|sample| sample.position)
        .collect::<Vec<_>>();
    let mut left = outline.left.clone();
    let mut right = outline.right.clone();
    if closed {
        centerline.push(centerline[0]);
        left.push(left[0]);
        right.push(right[0]);
    }
    if polyline_self_intersects(&centerline) {
        return false;
    }
    polyline_self_intersects(&left)
        || polyline_self_intersects(&right)
        || polylines_cross(&left, &right)
}

fn polylines_cross(left: &[Coordinate], right: &[Coordinate]) -> bool {
    left.windows(2).any(|left_pair| {
        right.windows(2).any(|right_pair| {
            segments_intersect(left_pair[0], left_pair[1], right_pair[0], right_pair[1])
        })
    })
}

fn build_segment_ribbons(outline: &SampledOutline, closed: bool) -> String {
    let mut data = String::new();
    let segment_count = if closed {
        outline.samples.len()
    } else {
        outline.samples.len() - 1
    };
    for index in 0..segment_count {
        let next = (index + 1) % outline.samples.len();
        let start = outline.samples[index].position;
        let end = outline.samples[next].position;
        let tangent = normalized(subtract(end, start)).unwrap_or(Coordinate { x: 1.0, y: 0.0 });
        let normal = Coordinate {
            x: -tangent.y,
            y: tangent.x,
        };
        let start_left = add(start, scale(normal, outline.radii[index]));
        let start_right = subtract(start, scale(normal, outline.radii[index]));
        let end_left = add(end, scale(normal, outline.radii[next]));
        let end_right = subtract(end, scale(normal, outline.radii[next]));
        push_pair_command(&mut data, 'M', start_left);
        push_pair_command(&mut data, 'L', end_left);
        if closed {
            push_pair_command(&mut data, 'L', end_right);
        } else {
            append_round_cap(&mut data, end_left, end_right, outline.radii[next]);
        }
        push_pair_command(&mut data, 'L', start_right);
        if !closed {
            append_round_cap(&mut data, start_right, start_left, outline.radii[index]);
        }
        data.push_str(" Z");
    }
    data
}

struct SampledOutline {
    samples: Vec<OutlineSample>,
    left: Vec<Coordinate>,
    right: Vec<Coordinate>,
    radii: Vec<f64>,
}

fn build_sampled_outline(
    samples: &[OutlineSample],
    nominal_width: f64,
    closed: bool,
) -> Result<SampledOutline> {
    let mut start = 0;
    let mut end = samples.len();
    if samples.first().is_some_and(|sample| sample.flags == 0x10) {
        start += 1;
    }
    if start < end && samples.last().is_some_and(|sample| sample.flags == 0x20) {
        end -= 1;
    }
    let mut samples = samples[start..end].to_vec();
    samples.dedup_by(|right, left| {
        if left.position == right.position {
            left.width_factor = right.width_factor;
            true
        } else {
            false
        }
    });
    if samples.len() < 2 {
        return Err(Error::InvalidSvgNumber {
            field: "variable-width outline points",
            value: samples.len() as f64,
        });
    }
    samples = refine_width_transitions(&samples, closed);

    let mut radii = Vec::with_capacity(samples.len());
    for sample in &samples {
        let factor = f64::from(sample.width_factor.unwrap_or(1.0));
        let radius = finite_svg_product(nominal_width, factor * 0.5, "outline radius")?;
        radii.push(radius.max(0.0));
    }

    let mut left = Vec::with_capacity(samples.len());
    let mut right = Vec::with_capacity(samples.len());
    for index in 0..samples.len() {
        let offset = outline_join_offset(&samples, &radii, index, closed);
        left.push(add(samples[index].position, offset));
        right.push(subtract(samples[index].position, offset));
    }

    Ok(SampledOutline {
        samples,
        left,
        right,
        radii,
    })
}

fn refine_width_transitions(samples: &[OutlineSample], closed: bool) -> Vec<OutlineSample> {
    let mut refined = Vec::with_capacity(samples.len());
    let segment_count = if closed {
        samples.len()
    } else {
        samples.len() - 1
    };
    for index in 0..segment_count {
        let next = (index + 1) % samples.len();
        let start = samples[index];
        let end = samples[next];
        if refined.is_empty() {
            refined.push(start);
        }
        let start_width = f64::from(start.width_factor.unwrap_or(1.0));
        let end_width = f64::from(end.width_factor.unwrap_or(1.0));
        let threshold = start_width.abs().max(end_width.abs()).max(1.0e-6) * 0.25;
        let divisions = ((end_width - start_width).abs() / threshold)
            .ceil()
            .clamp(1.0, 16.0) as usize;
        for division in 1..=divisions {
            if closed && next == 0 && division == divisions {
                continue;
            }
            let parameter = division as f64 / divisions as f64;
            refined.push(interpolate_outline_sample(start, end, parameter));
        }
    }
    refined
}

fn interpolate_outline_sample(
    start: OutlineSample,
    end: OutlineSample,
    parameter: f64,
) -> OutlineSample {
    fn interpolate_factor(start: Option<f32>, end: Option<f32>, parameter: f64) -> Option<f32> {
        Some((f64::from(start?) * (1.0 - parameter) + f64::from(end?) * parameter) as f32)
    }
    OutlineSample {
        position: add(
            scale(start.position, 1.0 - parameter),
            scale(end.position, parameter),
        ),
        width_factor: interpolate_factor(start.width_factor, end.width_factor, parameter),
        opacity_factor: interpolate_factor(start.opacity_factor, end.opacity_factor, parameter),
        flags: if parameter == 1.0 { end.flags } else { 0 },
    }
}

fn outline_join_offset(
    samples: &[OutlineSample],
    radii: &[f64],
    index: usize,
    closed: bool,
) -> Coordinate {
    let radius = radii[index];
    let count = samples.len();
    let incoming = if index == 0 && !closed {
        None
    } else {
        let previous = if index == 0 { count - 1 } else { index - 1 };
        normalized(subtract(
            samples[index].position,
            samples[previous].position,
        ))
    };
    let outgoing = if index + 1 == count && !closed {
        None
    } else {
        let next = (index + 1) % count;
        normalized(subtract(samples[next].position, samples[index].position))
    };
    let tangent = outgoing
        .or(incoming)
        .unwrap_or(Coordinate { x: 1.0, y: 0.0 });
    let normal = Coordinate {
        x: -tangent.y,
        y: tangent.x,
    };
    let (Some(incoming), Some(outgoing)) = (incoming, outgoing) else {
        return scale(normal, radius);
    };
    if dot(incoming, outgoing) < -0.95 {
        return scale(normal, radius);
    }
    let incoming_normal = Coordinate {
        x: -incoming.y,
        y: incoming.x,
    };
    let outgoing_normal = Coordinate {
        x: -outgoing.y,
        y: outgoing.x,
    };
    let Some(miter) = normalized(add(incoming_normal, outgoing_normal)) else {
        return scale(normal, radius);
    };
    let denominator = dot(miter, outgoing_normal).abs();
    if denominator <= 1.0e-6 {
        return scale(normal, radius);
    }
    let requested = radius / denominator;
    let limit = if VectorPointFlags::from_raw(samples[index].flags).is_corner() {
        radius * 2.0
    } else {
        radius * 4.0
    };
    scale(miter, requested.min(limit))
}

fn append_outline_side(
    data: &mut String,
    positions: &[Coordinate],
    samples: &[OutlineSample],
    closed: bool,
    tolerance: f64,
    smooth: bool,
) {
    if !smooth {
        for &position in &positions[1..] {
            push_pair_command(data, 'L', position);
        }
        if closed {
            push_pair_command(data, 'L', positions[0]);
        }
        return;
    }
    let points = positions
        .iter()
        .zip(samples)
        .map(|(&position, sample)| PointData {
            position,
            controls: [None, None],
            control_count: 0,
            flags: sample.flags,
        })
        .collect::<Vec<_>>();
    for segment in fit_spline_points(&points, closed, tolerance).segments {
        match segment {
            FittedSegment::Cubic(segment) => append_cubic(data, segment),
            FittedSegment::Line(end) => push_pair_command(data, 'L', end),
        }
    }
}

fn append_cubic(data: &mut String, segment: CubicSegment) {
    write!(
        data,
        " C{} {} {} {} {} {}",
        display_number(segment.first_control.x),
        display_number(segment.first_control.y),
        display_number(segment.second_control.x),
        display_number(segment.second_control.y),
        display_number(segment.end.x),
        display_number(segment.end.y),
    )
    .expect("writing to a String cannot fail");
}

fn chord_aware_catmull_rom_segment(
    points: &[Coordinate],
    index: usize,
    closed: bool,
) -> CubicSegment {
    let start = points[index];
    let end_index = (index + 1) % points.len();
    let end = points[end_index];
    let previous = if index == 0 {
        if closed {
            points[points.len() - 1]
        } else {
            start
        }
    } else {
        points[index - 1]
    };
    let next = if end_index + 1 < points.len() {
        points[end_index + 1]
    } else if closed {
        points[0]
    } else {
        end
    };
    let chord = length(subtract(end, start));
    let start_tangent = spacing_aware_tangent(previous, start, end);
    let end_tangent = spacing_aware_tangent(start, end, next);
    CubicSegment {
        first_control: add(start, scale(start_tangent, chord / 3.0)),
        second_control: subtract(end, scale(end_tangent, chord / 3.0)),
        end,
    }
}

fn spacing_aware_tangent(
    previous: Coordinate,
    current: Coordinate,
    next: Coordinate,
) -> Coordinate {
    let incoming = subtract(current, previous);
    let outgoing = subtract(next, current);
    let incoming_length = length(incoming);
    let outgoing_length = length(outgoing);
    match (normalized(incoming), normalized(outgoing)) {
        (Some(incoming), Some(outgoing)) => normalized(add(
            scale(incoming, outgoing_length),
            scale(outgoing, incoming_length),
        ))
        .unwrap_or(outgoing),
        (Some(incoming), None) => incoming,
        (None, Some(outgoing)) => outgoing,
        (None, None) => Coordinate { x: 1.0, y: 0.0 },
    }
}

fn append_round_cap(data: &mut String, source: Coordinate, destination: Coordinate, radius: f64) {
    if radius <= 0.0 || source == destination {
        push_pair_command(data, 'L', destination);
        return;
    }
    write!(
        data,
        " A{} {} 0 0 1 {} {}",
        display_number(radius),
        display_number(radius),
        display_number(destination.x),
        display_number(destination.y),
    )
    .expect("writing to a String cannot fail");
}

fn write_filled_path<W: Write>(
    writer: &mut Writer<W>,
    path_data: &str,
    style: RenderStyle,
) -> io::Result<WrittenMark> {
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        style.color[0], style.color[1], style.color[2]
    );
    let opacity = svg_number(style.opacity);
    let mut path = BytesStart::new("path");
    path.push_attribute(("d", path_data));
    path.push_attribute(("fill", color.as_str()));
    path.push_attribute(("fill-opacity", opacity.as_str()));
    path.push_attribute(("stroke", "none"));
    writer.write_event(Event::Empty(path))?;
    Ok(WrittenMark::FilledOutlinePath(1))
}

#[derive(Clone, Copy, Debug)]
struct VariableOpacityOptions {
    nominal_width: f64,
    stroke_index: u64,
    tolerance: f64,
    closed: bool,
    smooth: bool,
}

fn write_variable_opacity_segments<W: Write>(
    writer: &mut Writer<W>,
    samples: &[OutlineSample],
    style: RenderStyle,
    options: VariableOpacityOptions,
) -> Result<WrittenMark> {
    let VariableOpacityOptions {
        nominal_width,
        stroke_index,
        tolerance,
        closed,
        smooth,
    } = options;
    let outline = build_sampled_outline(samples, nominal_width, closed)?;
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        style.color[0], style.color[1], style.color[2]
    );
    let mut written = 0_u64;
    let segment_count = if closed {
        outline.samples.len()
    } else {
        outline.samples.len() - 1
    };
    for segment_index in 0..segment_count {
        let end_index = (segment_index + 1) % outline.samples.len();
        let first_sample = outline.samples[segment_index];
        let second_sample = outline.samples[end_index];
        let first_opacity =
            (style.opacity * f64::from(first_sample.opacity_factor.unwrap_or(1.0))).clamp(0.0, 1.0);
        let second_opacity = (style.opacity
            * f64::from(second_sample.opacity_factor.unwrap_or(1.0)))
        .clamp(0.0, 1.0);
        let gradient_id = format!("clip-opacity-{stroke_index}-{segment_index}");
        let x1 = svg_number(first_sample.position.x);
        let y1 = svg_number(first_sample.position.y);
        let x2 = svg_number(second_sample.position.x);
        let y2 = svg_number(second_sample.position.y);
        let mut gradient = BytesStart::new("linearGradient");
        gradient.push_attribute(("id", gradient_id.as_str()));
        gradient.push_attribute(("gradientUnits", "userSpaceOnUse"));
        gradient.push_attribute(("x1", x1.as_str()));
        gradient.push_attribute(("y1", y1.as_str()));
        gradient.push_attribute(("x2", x2.as_str()));
        gradient.push_attribute(("y2", y2.as_str()));
        writer.write_event(Event::Start(gradient))?;
        let first_opacity = svg_number(first_opacity);
        let mut first_stop = BytesStart::new("stop");
        first_stop.push_attribute(("offset", "0"));
        first_stop.push_attribute(("stop-color", color.as_str()));
        first_stop.push_attribute(("stop-opacity", first_opacity.as_str()));
        writer.write_event(Event::Empty(first_stop))?;
        let second_opacity = svg_number(second_opacity);
        let mut second_stop = BytesStart::new("stop");
        second_stop.push_attribute(("offset", "1"));
        second_stop.push_attribute(("stop-color", color.as_str()));
        second_stop.push_attribute(("stop-opacity", second_opacity.as_str()));
        writer.write_event(Event::Empty(second_stop))?;
        writer.write_event(Event::End(BytesEnd::new("linearGradient")))?;

        let left = chord_aware_catmull_rom_segment(&outline.left, segment_index, closed);
        let right = chord_aware_catmull_rom_segment(&outline.right, segment_index, closed);
        let left_safe =
            outline_cubic_is_safe(&outline.left, segment_index, closed, left, tolerance);
        let right_safe =
            outline_cubic_is_safe(&outline.right, segment_index, closed, right, tolerance);
        let mut data = String::new();
        push_pair_command(&mut data, 'M', outline.left[segment_index]);
        if smooth && left_safe {
            append_cubic(&mut data, left);
        } else {
            push_pair_command(&mut data, 'L', left.end);
        }
        if !closed && end_index + 1 == outline.samples.len() {
            append_round_cap(
                &mut data,
                outline.left[end_index],
                outline.right[end_index],
                outline.radii[end_index],
            );
        } else {
            push_pair_command(&mut data, 'L', outline.right[end_index]);
        }
        let reversed_right = CubicSegment {
            first_control: right.second_control,
            second_control: right.first_control,
            end: outline.right[segment_index],
        };
        if smooth && right_safe {
            append_cubic(&mut data, reversed_right);
        } else {
            push_pair_command(&mut data, 'L', reversed_right.end);
        }
        if !closed && segment_index == 0 {
            append_round_cap(
                &mut data,
                outline.right[0],
                outline.left[0],
                outline.radii[0],
            );
        }
        data.push_str(" Z");
        let fill = format!("url(#{gradient_id})");
        let mut path = BytesStart::new("path");
        path.push_attribute(("d", data.as_str()));
        path.push_attribute(("fill", fill.as_str()));
        path.push_attribute(("stroke", "none"));
        writer.write_event(Event::Empty(path))?;
        written += 1;
    }
    if written == 0 {
        return Err(Error::InvalidSvgNumber {
            field: "variable-opacity segments",
            value: 0.0,
        });
    }
    Ok(WrittenMark::FilledOutlinePath(written))
}

fn outline_cubic_is_safe(
    points: &[Coordinate],
    index: usize,
    _closed: bool,
    cubic: CubicSegment,
    tolerance: f64,
) -> bool {
    let start = points[index];
    let end_index = (index + 1) % points.len();
    cubic_geometry_is_safe(start, cubic, &[start, points[end_index]], tolerance)
}

fn append_segment(
    data: &mut String,
    source: PointData,
    destination: Coordinate,
    curve: CurveMode,
) -> bool {
    match curve {
        CurveMode::Quadratic if source.control_count == 1 => {
            if let Some(control) = source.controls[0] {
                write!(
                    data,
                    " Q{} {} {} {}",
                    display_number(control.x),
                    display_number(control.y),
                    display_number(destination.x),
                    display_number(destination.y)
                )
                .expect("writing to a String cannot fail");
                false
            } else {
                push_pair_command(data, 'L', destination);
                true
            }
        }
        CurveMode::Cubic if source.control_count == 2 => {
            if let (Some(first), Some(second)) = (source.controls[0], source.controls[1]) {
                write!(
                    data,
                    " C{} {} {} {} {} {}",
                    display_number(first.x),
                    display_number(first.y),
                    display_number(second.x),
                    display_number(second.y),
                    display_number(destination.x),
                    display_number(destination.y)
                )
                .expect("writing to a String cannot fail");
                false
            } else {
                push_pair_command(data, 'L', destination);
                true
            }
        }
        CurveMode::Quadratic | CurveMode::Cubic => {
            push_pair_command(data, 'L', destination);
            true
        }
        CurveMode::Straight | CurveMode::Spline => {
            push_pair_command(data, 'L', destination);
            false
        }
    }
}

fn push_pair_command(data: &mut String, command: char, point: Coordinate) {
    if !data.is_empty() {
        data.push(' ');
    }
    write!(
        data,
        "{command}{} {}",
        display_number(point.x),
        display_number(point.y)
    )
    .expect("writing to a String cannot fail");
}

fn display_number(value: f64) -> f64 {
    if value.to_bits() == (-0.0_f64).to_bits() {
        0.0
    } else {
        value
    }
}

fn svg_number(value: f64) -> String {
    display_number(value).to_string()
}

fn finite_svg_product(left: f64, right: f64, field: &'static str) -> Result<f64> {
    let value = left * right;
    if !value.is_finite() {
        return Err(Error::InvalidSvgNumber { field, value });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use quick_xml::{Reader, events::Event as XmlEvent};

    use super::*;

    fn point(x: f64, y: f64, controls: &[(f64, f64)]) -> PointData {
        let mut stored = [None, None];
        for (index, &(x, y)) in controls.iter().take(2).enumerate() {
            stored[index] = Some(Coordinate { x, y });
        }
        PointData {
            position: Coordinate { x, y },
            controls: stored,
            control_count: controls.len() as u8,
            flags: 0,
        }
    }

    fn assert_well_formed(xml: &str) {
        let mut reader = Reader::from_str(xml);
        loop {
            match reader.read_event() {
                Ok(XmlEvent::Eof) => break,
                Ok(_) => {}
                Err(error) => panic!("malformed XML at {}: {error}", reader.error_position()),
            }
        }
    }

    #[test]
    fn validates_canvas_and_document_options() {
        assert_eq!(
            SvgOptions::default().spline_tolerance(),
            DEFAULT_SPLINE_TOLERANCE
        );
        assert_eq!(SvgOptions::default().nominal_width_adjustment(), 0.0);
        assert_eq!(
            SvgOptions::default().stroke_representation(),
            SvgStrokeRepresentation::PreserveAppearance
        );
        assert_eq!(
            SvgOptions::default().one_point_stroke_handling(),
            SvgOnePointStrokeHandling::PreserveDots
        );
        assert_eq!(
            SvgOptions::default().spline_conversion(),
            SvgSplineConversion::CubicFit
        );
        let editable_polyline = SvgOptions::default()
            .with_stroke_representation(SvgStrokeRepresentation::EditableCenterline)
            .with_one_point_stroke_handling(SvgOnePointStrokeHandling::ConnectNearby)
            .with_spline_conversion(SvgSplineConversion::Polyline)
            .with_nominal_width_adjustment(-0.5);
        assert_eq!(
            editable_polyline.stroke_representation(),
            SvgStrokeRepresentation::EditableCenterline
        );
        assert_eq!(
            editable_polyline.spline_conversion(),
            SvgSplineConversion::Polyline
        );
        assert_eq!(
            editable_polyline.one_point_stroke_handling(),
            SvgOnePointStrokeHandling::ConnectNearby
        );
        assert_eq!(editable_polyline.nominal_width_adjustment(), -0.5);
        assert!(SvgCanvas::new(0.0, 10.0).is_err());
        assert!(SvgCanvas::new(f64::NAN, 10.0).is_err());
        let canvas = SvgCanvas::new(10.0, 20.0).unwrap();
        assert_eq!((canvas.width(), canvas.height()), (10.0, 20.0));

        let output = Cursor::new(Vec::new());
        let error = match SvgWriter::new(
            output,
            canvas,
            SvgOptions::default().with_group_opacity(2.0),
        ) {
            Ok(_) => panic!("invalid opacity was accepted"),
            Err(error) => error,
        };
        assert!(matches!(error, Error::InvalidGroupOpacity { .. }));
        assert!(matches!(
            SvgWriter::new(
                Cursor::new(Vec::new()),
                canvas,
                SvgOptions::default().with_spline_tolerance(0.0),
            ),
            Err(Error::InvalidSplineTolerance { .. })
        ));
        assert!(matches!(
            SvgWriter::new(
                Cursor::new(Vec::new()),
                canvas,
                SvgOptions::default().with_nominal_width_adjustment(f64::NAN),
            ),
            Err(Error::InvalidNominalWidthAdjustment { .. })
        ));
    }

    #[test]
    fn nearby_one_point_threshold_uses_half_the_smaller_effective_diameter() {
        let sample = |x, width_factor| OnePointStrokeSample {
            point: point(x, 0.0, &[]),
            width_factor,
            opacity_factor: Some(1.0),
        };
        assert!(one_point_samples_are_nearby_for_diameter(
            4.0,
            sample(0.0, Some(1.0)),
            sample(2.0, Some(1.0)),
        ));
        assert!(!one_point_samples_are_nearby_for_diameter(
            4.0,
            sample(0.0, Some(1.0)),
            sample(2.01, Some(1.0)),
        ));
        assert!(!one_point_samples_are_nearby_for_diameter(
            4.0,
            sample(0.0, Some(0.5)),
            sample(1.01, Some(1.0)),
        ));
    }

    #[test]
    fn nominal_width_adjustment_is_absolute_and_clamped() {
        assert_eq!(apply_nominal_width_adjustment(4.0, -0.5).unwrap(), 3.5);
        assert_eq!(apply_nominal_width_adjustment(0.25, -0.5).unwrap(), 0.0);
        assert!(matches!(
            apply_nominal_width_adjustment(f64::MAX, f64::MAX),
            Err(Error::InvalidSvgNumber { .. })
        ));
    }

    #[test]
    fn connected_outline_keeps_per_point_width_and_opacity_factors() {
        let sample = |x, width_factor, opacity_factor| OnePointStrokeSample {
            point: point(x, 0.0, &[]),
            width_factor: Some(width_factor),
            opacity_factor: Some(opacity_factor),
        };
        let samples = [sample(0.0, 0.25, 0.4), sample(2.0, 1.0, 0.9)];
        let bridge_points = [
            point(-1.0, 0.0, &[]),
            samples[0].point,
            samples[1].point,
            point(3.0, 0.0, &[]),
        ];

        let outline = connected_outline_samples(&bridge_points, &samples);
        assert_eq!(outline.len(), 4);
        assert_eq!(outline[0].width_factor, Some(0.25));
        assert_eq!(outline[0].opacity_factor, Some(0.4));
        assert_eq!(outline[1].width_factor, Some(0.25));
        assert_eq!(outline[1].opacity_factor, Some(0.4));
        assert_eq!(outline[2].width_factor, Some(1.0));
        assert_eq!(outline[2].opacity_factor, Some(0.9));
        assert_eq!(outline[3].width_factor, Some(1.0));
        assert_eq!(outline[3].opacity_factor, Some(0.9));

        let path = build_variable_width_outline(&outline, 8.0, 0.5, false, true).unwrap();
        assert!(path.contains("M-1 1"));
        assert!(path.contains("3 -4"));
    }

    #[test]
    fn separates_display_size_from_negative_or_nonzero_viewport() {
        let viewport = SvgViewport::new(-12.5, 24.0, 200.0, 100.0).unwrap();
        let canvas = SvgCanvas::new(100.0, 50.0).unwrap().with_viewport(viewport);
        assert_eq!(canvas.viewport(), viewport);
        assert_eq!(viewport.min_x(), -12.5);
        assert_eq!(viewport.min_y(), 24.0);
        assert_eq!((viewport.width(), viewport.height()), (200.0, 100.0));

        let writer =
            SvgWriter::new(Cursor::new(Vec::new()), canvas, SvgOptions::default()).unwrap();
        let (output, _) = writer.finish().unwrap();
        let xml = String::from_utf8(output.into_inner()).unwrap();
        assert!(xml.contains("width=\"100\" height=\"50\""));
        assert!(xml.contains("viewBox=\"-12.5 24 200 100\""));

        assert!(SvgViewport::new(f64::NAN, 0.0, 1.0, 1.0).is_err());
        assert!(SvgViewport::new(0.0, f64::INFINITY, 1.0, 1.0).is_err());
        assert!(SvgViewport::new(0.0, 0.0, 0.0, 1.0).is_err());
        assert!(SvgViewport::new(0.0, 0.0, 1.0, -1.0).is_err());
    }

    #[test]
    fn streams_a_well_formed_empty_document_and_escapes_title() {
        let output = Cursor::new(Vec::new());
        let writer = SvgWriter::new(
            output,
            SvgCanvas::new(10.0, 20.0).unwrap(),
            SvgOptions::default().with_title("a < b & c"),
        )
        .unwrap();
        let (output, report) = writer.finish().unwrap();
        let xml = String::from_utf8(output.into_inner()).unwrap();
        assert!(xml.contains("<title>a &lt; b &amp; c</title>"));
        assert!(xml.contains("viewBox=\"0 0 10 20\""));
        assert!(xml.ends_with("</svg>"));
        assert_well_formed(&xml);
        assert_eq!(report, SvgExportReport::default());
    }

    #[test]
    fn writes_suffix_eraser_masks_with_legacy_and_svg2_use_links() {
        let mut writer = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(100.0, 50.0).unwrap(),
            SvgOptions::default(),
        )
        .unwrap();
        writer.write_eraser_color_filter("eraser-test").unwrap();
        writer
            .write_eraser_mask("eraser-test", 2, &[0, 2, 4])
            .unwrap();
        let (output, _) = writer.finish().unwrap();
        let xml = String::from_utf8(output.into_inner()).unwrap();

        assert!(xml.contains("xmlns:xlink=\"http://www.w3.org/1999/xlink\""));
        assert!(xml.contains("id=\"eraser-test-black\""));
        assert!(xml.contains("mask-type=\"luminance\""));
        assert!(!xml.contains("#eraser-test-operation-0"));
        assert!(xml.contains("href=\"#eraser-test-operation-2\""));
        assert!(xml.contains("xlink:href=\"#eraser-test-operation-4\""));
        assert_well_formed(&xml);
    }

    #[test]
    fn writes_balanced_escaped_groups_and_closes_after_callback_error() {
        let mut writer = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(10.0, 20.0).unwrap(),
            SvgOptions::default(),
        )
        .unwrap();
        writer
            .write_group(
                SvgGroupOptions::default()
                    .with_opacity(0.5)
                    .with_id("line-layer")
                    .with_title("line < rough")
                    .with_transform_matrix([1.0, 0.25, -0.5, 2.0, 12.0, -3.5]),
                |_| Ok::<(), Error>(()),
            )
            .unwrap();
        assert!(matches!(
            writer.write_group(SvgGroupOptions::default(), |_| Err(Error::InvalidXmlText)),
            Err(Error::InvalidXmlText)
        ));
        let (output, _) = writer.finish().unwrap();
        let xml = String::from_utf8(output.into_inner()).unwrap();
        assert!(xml.contains(
            "<g opacity=\"0.5\" id=\"line-layer\" transform=\"matrix(1 0.25 -0.5 2 12 -3.5)\">"
        ));
        assert!(xml.contains("<title>line &lt; rough</title>"));
        assert_well_formed(&xml);

        let mut invalid_writer = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(1.0, 1.0).unwrap(),
            SvgOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            invalid_writer.write_group(SvgGroupOptions::default().with_id("invalid id"), |_| {
                Ok::<(), Error>(())
            }),
            Err(Error::InvalidGroupId)
        ));
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                invalid_writer.write_group(
                    SvgGroupOptions::default()
                        .with_transform_matrix([1.0, 0.0, 0.0, value, 0.0, 0.0]),
                    |_| Ok::<(), Error>(()),
                ),
                Err(Error::InvalidSvgNumber {
                    field: "group transform",
                    value: rejected,
                }) if rejected.to_bits() == value.to_bits()
            ));
        }
    }

    #[test]
    fn rejects_xml_control_characters_before_writing() {
        let result = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(1.0, 1.0).unwrap(),
            SvgOptions::default().with_title("invalid\u{1}"),
        );
        assert!(matches!(result, Err(Error::InvalidXmlText)));
    }

    #[test]
    fn configures_individual_or_all_loss_policies() {
        let options = SvgOptions::default()
            .with_loss_handling(SvgLossKind::VariableWidthAveraged, SvgLossHandling::Reject);
        assert_eq!(
            options.loss_handling(SvgLossKind::VariableWidthAveraged),
            SvgLossHandling::Reject
        );
        assert_eq!(
            options.loss_handling(SvgLossKind::SplineToPolyline),
            SvgLossHandling::Allow
        );
        let strict = SvgOptions::default().reject_all_losses();
        assert!(
            SvgLossKind::ALL
                .into_iter()
                .all(|kind| strict.loss_handling(kind) == SvgLossHandling::Reject)
        );
        let omissions = SvgOptions::default().reject_omissions();
        assert_eq!(
            omissions.loss_handling(SvgLossKind::UnsupportedStrokeSkipped),
            SvgLossHandling::Reject
        );
        assert_eq!(
            omissions.loss_handling(SvgLossKind::SplineToPolyline),
            SvgLossHandling::Allow
        );
        assert_eq!(
            SvgLossKind::BrushToRoundStroke.severity(),
            SvgLossSeverity::Approximation
        );
        assert_eq!(
            SvgLossKind::FrameFillIgnored.severity(),
            SvgLossSeverity::Omission
        );
    }

    #[test]
    fn rejected_loss_does_not_commit_other_stroke_losses() {
        let options = SvgOptions::default()
            .with_loss_handling(SvgLossKind::BrushToRoundStroke, SvgLossHandling::Reject);
        let mut writer = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(1.0, 1.0).unwrap(),
            options,
        )
        .unwrap();
        let mut losses = StrokeLosses::default();
        losses.add(SvgLossKind::SplineToPolyline);
        losses.add(SvgLossKind::BrushToRoundStroke);

        writer.report.strokes_seen = 1;
        assert!(matches!(
            writer.apply_losses(&losses, 4),
            Err(Error::LossyConversion {
                stroke_index: 4,
                kind: SvgLossKind::BrushToRoundStroke
            })
        ));
        assert!(!writer.report().has_loss());
        assert_eq!(writer.report().skip_count(SvgSkipKind::PolicyRejected), 1);
        assert!(writer.report().is_consistent());
    }

    #[test]
    fn classifies_empty_and_unsupported_stroke_skips() {
        let mut empty_path = PathBuilder::default();
        assert_eq!(
            empty_path.finish(false, CurveMode::Straight),
            BuiltMark::Empty
        );

        let mut writer = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(1.0, 1.0).unwrap(),
            SvgOptions::default(),
        )
        .unwrap();
        writer.report.strokes_seen = 2;
        writer.record_skip(SvgSkipKind::EmptyStroke);
        writer
            .skip_unsupported_stroke(1, SvgSkipKind::UnsupportedSemanticLayout)
            .unwrap();
        assert_eq!(writer.report().strokes_skipped(), 2);
        assert_eq!(writer.report().skip_count(SvgSkipKind::EmptyStroke), 1);
        assert_eq!(
            writer
                .report()
                .skip_count(SvgSkipKind::UnsupportedSemanticLayout),
            1
        );
        assert_eq!(
            writer
                .report()
                .loss_count(SvgLossKind::UnsupportedStrokeSkipped),
            1
        );
        assert!(writer.report().is_consistent());
    }

    #[test]
    fn strict_policy_rejects_unsupported_stroke_before_output() {
        let options = SvgOptions::default().with_loss_handling(
            SvgLossKind::UnsupportedStrokeSkipped,
            SvgLossHandling::Reject,
        );
        let mut writer = SvgWriter::new(
            Cursor::new(Vec::new()),
            SvgCanvas::new(1.0, 1.0).unwrap(),
            options,
        )
        .unwrap();
        writer.report.strokes_seen = 1;
        assert!(matches!(
            writer.skip_unsupported_stroke(0, SvgSkipKind::UnsupportedSemanticLayout),
            Err(Error::LossyConversion {
                stroke_index: 0,
                kind: SvgLossKind::UnsupportedStrokeSkipped
            })
        ));
        assert_eq!(writer.report().skip_count(SvgSkipKind::PolicyRejected), 1);
        assert_eq!(writer.report().marks_written(), 0);
        assert!(writer.report().is_consistent());

        let (output, _) = writer.finish().unwrap();
        let xml = String::from_utf8(output.into_inner()).unwrap();
        assert!(!xml.contains("<path"));
        assert!(!xml.contains("<circle"));
        assert_well_formed(&xml);
    }

    #[test]
    fn writes_one_point_stroke_as_visible_styled_circle() {
        let mut path = PathBuilder::default();
        path.push(Some(point(10.0, 20.0, &[])), CurveMode::Spline);
        let mark = path.finish(false, CurveMode::Spline);
        assert_eq!(mark, BuiltMark::Dot(Coordinate { x: 10.0, y: 20.0 }));

        let output = Cursor::new(Vec::new());
        let mut xml_writer = Writer::new(output);
        let written = write_mark(
            &mut xml_writer,
            &mark,
            RenderStyle {
                color: [0x0c, 0x22, 0x38],
                width: 10.0 * 0.5,
                opacity: 0.8 * 0.25,
            },
        )
        .unwrap();
        assert_eq!(written, WrittenMark::Dot);
        let xml = String::from_utf8(xml_writer.into_inner().into_inner()).unwrap();
        assert_eq!(
            xml,
            "<circle cx=\"10\" cy=\"20\" r=\"2.5\" fill=\"#0c2238\" fill-opacity=\"0.2\"/>"
        );
        assert_well_formed(&xml);

        let mut report = SvgExportReport {
            strokes_seen: 1,
            dots_written: 1,
            strokes_written: 1,
            ..SvgExportReport::default()
        };
        report.losses[SvgLossKind::BrushToRoundStroke.index()] = 1;
        assert_eq!(report.paths_written(), 0);
        assert_eq!(report.dots_written(), 1);
        assert_eq!(report.marks_written(), 1);
        assert_eq!(
            report.loss_count(SvgLossKind::SplineToPolyline),
            0,
            "a dot has no spline segment to approximate"
        );
        assert!(report.is_consistent());
    }

    #[test]
    fn builds_quadratic_and_closed_cubic_paths() {
        let mut quadratic = PathBuilder::default();
        quadratic.push(Some(point(1.0, 2.0, &[(3.0, 4.0)])), CurveMode::Quadratic);
        quadratic.push(Some(point(5.0, 6.0, &[])), CurveMode::Quadratic);
        assert_eq!(
            quadratic.finish(false, CurveMode::Quadratic),
            BuiltMark::Path("M1 2 Q3 4 5 6".to_owned())
        );

        let mut cubic = PathBuilder::default();
        cubic.push(
            Some(point(1.0, 2.0, &[(3.0, 4.0), (5.0, 6.0)])),
            CurveMode::Cubic,
        );
        cubic.push(
            Some(point(7.0, 8.0, &[(9.0, 10.0), (11.0, 12.0)])),
            CurveMode::Cubic,
        );
        assert_eq!(
            cubic.finish(true, CurveMode::Cubic),
            BuiltMark::Path("M1 2 C3 4 5 6 7 8 C9 10 11 12 1 2 Z".to_owned())
        );
    }

    #[test]
    fn centerline_paths_explicitly_set_editable_stroke_attributes() {
        let mark = BuiltMark::Path("M0 0 L10 0".to_owned());
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        assert_eq!(
            write_mark(
                &mut writer,
                &mark,
                RenderStyle {
                    color: [1, 2, 3],
                    width: 2.0,
                    opacity: 0.5,
                },
            )
            .unwrap(),
            WrittenMark::CenterlinePath
        );
        let xml = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(xml.contains("fill=\"none\""));
        assert!(xml.contains("stroke-linecap=\"round\""));
        assert!(xml.contains("stroke-linejoin=\"round\""));
    }

    #[test]
    fn fits_splines_with_cubics_and_falls_back_for_missing_controls() {
        let mut spline = PathBuilder::default();
        spline.push(Some(point(1.0, 2.0, &[])), CurveMode::Spline);
        spline.push(Some(point(5.0, 6.0, &[])), CurveMode::Spline);
        assert_eq!(
            spline.finish(false, CurveMode::Spline),
            BuiltMark::Path(
                "M1 2 C2.333333333333333 3.333333333333333 3.666666666666667 4.666666666666667 5 6"
                    .to_owned()
            )
        );

        let mut combined_start = point(1.0, 2.0, &[]);
        combined_start.flags = 0x11;
        let mut combined_end = point(5.0, 6.0, &[]);
        combined_end.flags = 0x21;
        assert!(matches!(
            build_spline_mark(
                &[combined_start, combined_end],
                false,
                DEFAULT_SPLINE_TOLERANCE,
                SvgSplineConversion::CubicFit,
            )
            .0,
            BuiltMark::Path(_),
        ));

        let mut quadratic = PathBuilder::default();
        quadratic.push(Some(point(1.0, 2.0, &[])), CurveMode::Quadratic);
        quadratic.push(Some(point(5.0, 6.0, &[])), CurveMode::Quadratic);
        quadratic.push(Some(point(7.0, 8.0, &[])), CurveMode::Quadratic);
        assert!(quadratic.bezier_fallback);
        assert_eq!(
            quadratic.finish(false, CurveMode::Quadratic),
            BuiltMark::Path("M1 2 L5 6 L7 8".to_owned())
        );
    }

    #[test]
    fn spline_polyline_passes_drawable_points_without_curve_commands() {
        let points = [
            point(0.0, 0.0, &[]),
            point(2.0, 3.0, &[]),
            point(8.0, -1.0, &[]),
        ];
        let (open, fallback) = build_spline_mark(
            &points,
            false,
            DEFAULT_SPLINE_TOLERANCE,
            SvgSplineConversion::Polyline,
        );
        assert_eq!(open, BuiltMark::Path("M0 0 L2 3 L8 -1".to_owned()));
        assert!(!fallback);
        let (closed, fallback) = build_spline_mark(
            &points,
            true,
            DEFAULT_SPLINE_TOLERANCE,
            SvgSplineConversion::Polyline,
        );
        assert_eq!(closed, BuiltMark::Path("M0 0 L2 3 L8 -1 Z".to_owned()));
        assert!(!fallback);
    }

    #[test]
    fn spline_normalization_keeps_combined_flags_and_merges_tiny_segments() {
        let mut start_sentinel = point(-1.0, -1.0, &[]);
        start_sentinel.flags = 0x10;
        let mut combined_start = point(0.0, 0.0, &[]);
        combined_start.flags = 0x11;
        let mut near_duplicate = point(0.0 + 1.0e-10, 0.0, &[]);
        near_duplicate.flags = 0x20;
        let mut end_sentinel = point(9.0, 9.0, &[]);
        end_sentinel.flags = 0x20;
        let mut samples = vec![
            (start_sentinel, Some(1.0), Some(1.0)),
            (combined_start, Some(0.5), Some(0.25)),
            (near_duplicate, Some(0.75), Some(0.5)),
            (point(5.0, 5.0, &[]), Some(1.0), Some(1.0)),
            (end_sentinel, Some(1.0), Some(1.0)),
        ];
        normalize_spline_samples(&mut samples);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].0.position, combined_start.position);
        assert!(VectorPointFlags::from_raw(samples[0].0.flags).is_corner());
        assert_eq!((samples[0].1, samples[0].2), (Some(0.75), Some(0.5)));
    }

    #[test]
    fn cubic_fit_anchors_corners_and_splits_uturns() {
        let mut corner = point(10.0, 0.0, &[]);
        corner.flags = VectorPointFlags::CORNER.raw();
        let corner_points = [point(0.0, 0.0, &[]), corner, point(10.0, 10.0, &[])];
        let fitted = fit_spline_points(&corner_points, false, 0.5);
        assert_eq!(fitted.segments.len(), 2);
        let first = match fitted.segments[0] {
            FittedSegment::Cubic(segment) => segment,
            FittedSegment::Line(_) => panic!("corner input unexpectedly fell back"),
        };
        let second = match fitted.segments[1] {
            FittedSegment::Cubic(segment) => segment,
            FittedSegment::Line(_) => panic!("corner input unexpectedly fell back"),
        };
        assert_eq!(first.end, corner.position);
        assert_eq!(second.end, Coordinate { x: 10.0, y: 10.0 });
        let incoming = subtract(first.end, first.second_control);
        let outgoing = subtract(second.first_control, corner.position);
        assert!(dot(incoming, outgoing).abs() <= 1.0e-12);

        let uturn = [
            point(0.0, 0.0, &[]),
            point(10.0, 0.0, &[]),
            point(0.0, 0.0, &[]),
        ];
        let fitted = fit_spline_points(&uturn, false, 0.5);
        assert_eq!(fitted.segments.len(), 2);
    }

    #[test]
    fn cubic_guard_rejects_spikes_loops_and_collinear_backtracking() {
        let reference = [
            Coordinate { x: 0.0, y: 0.0 },
            Coordinate { x: 10.0, y: 0.0 },
        ];
        let spike = CubicSegment {
            first_control: Coordinate { x: 2.0, y: 100.0 },
            second_control: Coordinate { x: 8.0, y: -100.0 },
            end: reference[1],
        };
        assert!(!cubic_geometry_is_safe(
            reference[0],
            spike,
            &reference,
            0.5
        ));
        let backtracking = CubicSegment {
            first_control: Coordinate { x: 8.0, y: 0.0 },
            second_control: Coordinate { x: -4.0, y: 0.0 },
            end: reference[1],
        };
        assert!(!cubic_geometry_is_safe(
            reference[0],
            backtracking,
            &reference,
            0.5
        ));
    }

    #[test]
    fn unsafe_spline_fit_emits_local_lines_and_reports_fallback_state() {
        let points = [
            point(0.0, 0.0, &[]),
            point(10.0, 10.0, &[]),
            point(20.0, 0.0, &[]),
        ];
        let (mark, fallback) =
            build_spline_mark(&points, false, 0.01, SvgSplineConversion::CubicFit);
        assert!(fallback);
        let BuiltMark::Path(data) = mark else {
            panic!("three drawable samples must produce a path");
        };
        assert!(data.contains(" L"));
    }

    #[test]
    fn closed_spline_fit_is_stable_under_cyclic_start_changes() {
        let points = vec![
            point(0.0, 0.0, &[]),
            point(10.0, 0.0, &[]),
            point(10.0, 10.0, &[]),
            point(0.0, 10.0, &[]),
        ];
        let first = fit_spline_points(&points, true, 0.5).segments;
        let mut rotated_points = points.clone();
        rotated_points.rotate_left(1);
        let second = fit_spline_points(&rotated_points, true, 0.5).segments;
        let mut rotated_segments = first.clone();
        rotated_segments.rotate_left(1);
        assert_eq!(rotated_segments, second);
    }

    #[test]
    fn factor_losses_match_selected_representation_and_ignore_dots() {
        let mut appearance = StrokeLosses::default();
        add_factor_representation_losses(
            &mut appearance,
            true,
            true,
            true,
            SvgStrokeRepresentation::PreserveAppearance,
        );
        assert_eq!(
            appearance.iter().collect::<Vec<_>>(),
            vec![
                SvgLossKind::VariableWidthOutlined,
                SvgLossKind::VariableOpacitySegmented
            ]
        );
        let mut editable = StrokeLosses::default();
        add_factor_representation_losses(
            &mut editable,
            true,
            true,
            true,
            SvgStrokeRepresentation::EditableCenterline,
        );
        assert_eq!(
            editable.iter().collect::<Vec<_>>(),
            vec![
                SvgLossKind::VariableWidthAveraged,
                SvgLossKind::VariableOpacityAveraged
            ]
        );
        let mut dot = StrokeLosses::default();
        add_factor_representation_losses(
            &mut dot,
            true,
            true,
            false,
            SvgStrokeRepresentation::PreserveAppearance,
        );
        assert_eq!(dot.iter().count(), 0);
    }

    #[test]
    fn outline_limits_sharp_joins_and_closed_paths_have_no_caps() {
        let samples = [
            OutlineSample {
                position: Coordinate { x: 0.0, y: 0.0 },
                width_factor: Some(1.0),
                opacity_factor: Some(1.0),
                flags: 0,
            },
            OutlineSample {
                position: Coordinate { x: 10.0, y: 0.0 },
                width_factor: Some(1.0),
                opacity_factor: Some(1.0),
                flags: VectorPointFlags::CORNER.raw(),
            },
            OutlineSample {
                position: Coordinate { x: 0.01, y: 0.01 },
                width_factor: Some(1.0),
                opacity_factor: Some(1.0),
                flags: 0,
            },
        ];
        let outline = build_sampled_outline(&samples, 4.0, false).unwrap();
        assert!(
            length(subtract(outline.left[1], samples[1].position)) <= 4.0 + 1.0e-12,
            "corner miter must remain within the 2x-radius corner limit"
        );
        let closed = build_variable_width_outline(&samples, 4.0, 0.5, true, true).unwrap();
        assert!(!closed.contains(" A"), "closed outlines must not emit caps");
        let polyline = build_variable_width_outline(&samples, 4.0, 0.5, false, false).unwrap();
        assert!(!polyline.contains(" C"));

        let mut abrupt = samples;
        abrupt[0].width_factor = Some(0.0);
        let refined = refine_width_transitions(&abrupt, false);
        assert!(refined.len() > abrupt.len());
        assert!(refined.len() <= 1 + (abrupt.len() - 1) * 16);

        let crossing = [
            OutlineSample {
                position: Coordinate { x: 0.0, y: 0.0 },
                ..samples[0]
            },
            OutlineSample {
                position: Coordinate { x: 10.0, y: 10.0 },
                ..samples[0]
            },
            OutlineSample {
                position: Coordinate { x: 0.0, y: 10.0 },
                ..samples[0]
            },
            OutlineSample {
                position: Coordinate { x: 10.0, y: 0.0 },
                ..samples[0]
            },
        ];
        let crossing_outline = build_sampled_outline(&crossing, 1.0, false).unwrap();
        assert!(
            !outline_has_conversion_intersection(&crossing_outline, false),
            "an intersection already present in the source centerline is not conversion-created"
        );
    }

    #[test]
    fn tracks_factor_variation_without_point_sized_storage() {
        let mut factor = FactorAccumulator::default();
        factor.push(Some(0.5));
        factor.push(Some(1.5));
        let factor = factor.finish();
        assert_eq!(factor.value, 1.0);
        assert!(factor.variable);
        assert!(!factor.incomplete);
    }

    #[test]
    fn writes_variable_width_as_outline_and_opacity_as_gradients() {
        let samples = [
            OutlineSample {
                position: Coordinate { x: 0.0, y: 0.0 },
                width_factor: Some(0.5),
                opacity_factor: Some(0.25),
                flags: 0,
            },
            OutlineSample {
                position: Coordinate { x: 10.0, y: 0.0 },
                width_factor: Some(1.0),
                opacity_factor: Some(0.75),
                flags: 0,
            },
        ];
        let outline = build_variable_width_outline(&samples, 4.0, 0.45, false, true).unwrap();
        assert!(outline.starts_with("M0 1"));
        assert!(outline.contains("A2 2 0 0 1 10 -2"));
        assert!(outline.ends_with(" Z"));

        let opacity_samples = [
            samples[0],
            samples[1],
            OutlineSample {
                position: Coordinate { x: 20.0, y: 0.0 },
                width_factor: Some(0.75),
                opacity_factor: Some(0.5),
                flags: 0,
            },
        ];
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        let expected_open_segments = build_sampled_outline(&opacity_samples, 4.0, false)
            .unwrap()
            .samples
            .len()
            - 1;
        let written = write_variable_opacity_segments(
            &mut writer,
            &opacity_samples,
            RenderStyle {
                color: [1, 2, 3],
                width: 4.0,
                opacity: 0.8,
            },
            VariableOpacityOptions {
                nominal_width: 4.0,
                stroke_index: 7,
                tolerance: DEFAULT_SPLINE_TOLERANCE,
                closed: false,
                smooth: true,
            },
        )
        .unwrap();
        assert_eq!(
            written,
            WrittenMark::FilledOutlinePath(expected_open_segments as u64)
        );
        let xml = String::from_utf8(writer.into_inner().into_inner()).unwrap();
        assert!(xml.contains("id=\"clip-opacity-7-0\""));
        assert!(xml.contains("stop-opacity=\"0.2\""));
        assert!(xml.contains("stop-opacity=\"0.6000000000000001\""));
        assert!(xml.contains("fill=\"url(#clip-opacity-7-0)\""));
        assert!(xml.contains("fill=\"url(#clip-opacity-7-1)\""));
        assert!(!xml.contains("NaN"));
        assert_well_formed(&format!("<svg>{xml}</svg>"));

        let mut closed_writer = Writer::new(Cursor::new(Vec::new()));
        let closed_written = write_variable_opacity_segments(
            &mut closed_writer,
            &opacity_samples,
            RenderStyle {
                color: [1, 2, 3],
                width: 4.0,
                opacity: 0.8,
            },
            VariableOpacityOptions {
                nominal_width: 4.0,
                stroke_index: 8,
                tolerance: DEFAULT_SPLINE_TOLERANCE,
                closed: true,
                smooth: true,
            },
        )
        .unwrap();
        let expected_closed_segments = build_sampled_outline(&opacity_samples, 4.0, true)
            .unwrap()
            .samples
            .len();
        assert_eq!(
            closed_written,
            WrittenMark::FilledOutlinePath(expected_closed_segments as u64)
        );
        let closed_xml = String::from_utf8(closed_writer.into_inner().into_inner()).unwrap();
        assert!(!closed_xml.contains(" A"));
        assert_well_formed(&format!("<svg>{closed_xml}</svg>"));
    }

    #[test]
    fn report_iterates_only_nonzero_losses() {
        let mut report = SvgExportReport::default();
        report.losses[SvgLossKind::SplineToPolyline.index()] = 2;
        assert!(report.has_loss());
        assert_eq!(
            report.losses().collect::<Vec<_>>(),
            vec![(SvgLossKind::SplineToPolyline, 2)]
        );
    }

    #[test]
    fn merges_reports_for_multi_file_exports() {
        let mut first = SvgExportReport {
            vector_objects: 1,
            strokes_seen: 2,
            paths_written: 1,
            centerline_paths_written: 1,
            dots_written: 1,
            strokes_written: 2,
            ..SvgExportReport::default()
        };
        first.losses[SvgLossKind::BrushToRoundStroke.index()] = 2;

        let mut second = SvgExportReport {
            vector_objects: 2,
            strokes_seen: 1,
            strokes_skipped: 1,
            ..SvgExportReport::default()
        };
        second.skips[SvgSkipKind::EmptyStroke.index()] = 1;

        first += &second;
        assert_eq!(first.vector_objects(), 3);
        assert_eq!(first.strokes_seen(), 3);
        assert_eq!(first.marks_written(), 2);
        assert_eq!(first.strokes_written(), 2);
        assert_eq!(first.strokes_skipped(), 1);
        assert_eq!(first.skip_count(SvgSkipKind::EmptyStroke), 1);
        assert!(first.is_consistent());
    }
}
