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

use clipfile::{VectorCurveKind, VectorData, VectorPoint, VectorStroke};
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
    /// CLIP spline interpolation was replaced with straight segments.
    SplineToPolyline,
    /// Per-point width factors were replaced with one average width.
    VariableWidthAveraged,
    /// Per-point opacity factors were replaced with one average opacity.
    VariableOpacityAveraged,
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
    /// A stroke with an unsupported semantic layout was skipped.
    UnsupportedStrokeSkipped,
}

impl SvgLossKind {
    const ALL: [Self; 11] = [
        Self::SplineToPolyline,
        Self::VariableWidthAveraged,
        Self::VariableOpacityAveraged,
        Self::BrushToRoundStroke,
        Self::SecondaryColorIgnored,
        Self::FrameFillIgnored,
        Self::MissingWidthFallback,
        Self::MissingWidthFactorFallback,
        Self::MissingOpacityFactorFallback,
        Self::BezierControlsToLine,
        Self::UnsupportedStrokeSkipped,
    ];

    const fn index(self) -> usize {
        match self {
            Self::SplineToPolyline => 0,
            Self::VariableWidthAveraged => 1,
            Self::VariableOpacityAveraged => 2,
            Self::BrushToRoundStroke => 3,
            Self::SecondaryColorIgnored => 4,
            Self::FrameFillIgnored => 5,
            Self::MissingWidthFallback => 6,
            Self::MissingWidthFactorFallback => 7,
            Self::MissingOpacityFactorFallback => 8,
            Self::BezierControlsToLine => 9,
            Self::UnsupportedStrokeSkipped => 10,
        }
    }

    /// User-facing impact category for this conversion loss.
    #[must_use]
    pub const fn severity(self) -> SvgLossSeverity {
        match self {
            Self::SplineToPolyline
            | Self::VariableWidthAveraged
            | Self::VariableOpacityAveraged
            | Self::BrushToRoundStroke => SvgLossSeverity::Approximation,
            Self::SecondaryColorIgnored
            | Self::FrameFillIgnored
            | Self::UnsupportedStrokeSkipped => SvgLossSeverity::Omission,
            Self::MissingWidthFallback
            | Self::MissingWidthFactorFallback
            | Self::MissingOpacityFactorFallback
            | Self::BezierControlsToLine => SvgLossSeverity::Fallback,
        }
    }
}

impl fmt::Display for SvgLossKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::SplineToPolyline => "CLIP spline approximated by a polyline",
            Self::VariableWidthAveraged => "variable width averaged",
            Self::VariableOpacityAveraged => "variable opacity averaged",
            Self::BrushToRoundStroke => "CLIP brush approximated by a round stroke",
            Self::SecondaryColorIgnored => "secondary color metadata not represented",
            Self::FrameFillIgnored => "frame fill style ignored",
            Self::MissingWidthFallback => "missing brush width replaced with one",
            Self::MissingWidthFactorFallback => "missing width factors replaced",
            Self::MissingOpacityFactorFallback => "missing opacity factors replaced",
            Self::BezierControlsToLine => "Bezier segment without controls replaced by a line",
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

/// Configuration for an SVG document.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgOptions {
    title: Option<String>,
    group_opacity: f64,
    loss_handling: [SvgLossHandling; SvgLossKind::ALL.len()],
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            title: None,
            group_opacity: 1.0,
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

    /// Current handling for one loss category.
    #[must_use]
    pub const fn loss_handling(&self, kind: SvgLossKind) -> SvgLossHandling {
        self.loss_handling[kind.index()]
    }
}

/// Aggregate counts from one SVG conversion.
///
/// Counts are bounded by the number of input strokes and loss categories, so
/// the report does not grow with document size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SvgExportReport {
    vector_objects: u64,
    strokes_seen: u64,
    paths_written: u64,
    dots_written: u64,
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
            dots_written: 0,
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

    /// Number of one-point strokes written as SVG circles.
    #[must_use]
    pub const fn dots_written(&self) -> u64 {
        self.dots_written
    }

    /// Total number of paths and one-point circles written.
    #[must_use]
    pub const fn marks_written(&self) -> u64 {
        self.paths_written.saturating_add(self.dots_written)
    }

    /// Number of input strokes represented by an SVG mark.
    #[must_use]
    pub const fn strokes_written(&self) -> u64 {
        self.marks_written()
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
        self.strokes_seen == self.marks_written().saturating_add(self.strokes_skipped)
            && self.strokes_skipped == classified_skips
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
        self.dots_written = self.dots_written.saturating_add(other.dots_written);
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
}

impl Default for SvgGroupOptions {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            id: None,
            title: None,
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
}

/// Streaming SVG document writer.
///
/// Call [`Self::finish`] to write the closing tags and recover the destination.
/// An error can leave a syntactically incomplete document in the destination.
#[must_use = "call SvgWriter::finish to close the SVG document"]
pub struct SvgWriter<W: Write> {
    writer: Writer<W>,
    options: SvgOptions,
    report: SvgExportReport,
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
            options,
            report: SvgExportReport::default(),
        })
    }

    /// Writes every stroke from one validated vector body.
    pub fn write_vector(&mut self, vector: &VectorData) -> Result<()> {
        self.report.vector_objects += 1;
        for stroke in vector.strokes() {
            self.write_stroke(stroke)?;
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

        let built = build_stroke(stroke, curve);
        if built.incomplete_geometry {
            return self.skip_unsupported_stroke(stroke_index, SvgSkipKind::IncompleteGeometry);
        }
        if matches!(built.mark, BuiltMark::Empty) {
            self.record_skip(SvgSkipKind::EmptyStroke);
            return Ok(());
        }

        let mut losses = StrokeLosses::default();
        if approximated_spline && matches!(built.mark, BuiltMark::Path(_)) {
            losses.add(SvgLossKind::SplineToPolyline);
        }
        if built.bezier_fallback {
            losses.add(SvgLossKind::BezierControlsToLine);
        }
        if built.width.variable {
            losses.add(SvgLossKind::VariableWidthAveraged);
        }
        if built.opacity.variable {
            losses.add(SvgLossKind::VariableOpacityAveraged);
        }
        if built.width.incomplete {
            losses.add(SvgLossKind::MissingWidthFactorFallback);
        }
        if built.opacity.incomplete {
            losses.add(SvgLossKind::MissingOpacityFactorFallback);
        }

        let style = stroke
            .style()
            .expect("a semantically supported vector stroke has a style");
        losses.add(SvgLossKind::BrushToRoundStroke);
        let color = style.main_color().rgb_8bit();
        if style.sub_color().rgb_8bit() != color {
            losses.add(SvgLossKind::SecondaryColorIgnored);
        }
        if style.fill_style_id().is_some() {
            losses.add(SvgLossKind::FrameFillIgnored);
        }
        let nominal_width = match style.brush_diameter() {
            Some(width) => width,
            None => {
                losses.add(SvgLossKind::MissingWidthFallback);
                1.0
            }
        };
        let stroke_width = finite_svg_product(nominal_width, built.width.value, "stroke width")?;
        let stroke_opacity = (style.opacity() * built.opacity.value).clamp(0.0, 1.0);
        self.apply_losses(&losses, stroke_index)?;

        let rendered_style = RenderStyle {
            color,
            width: stroke_width.max(0.0),
            opacity: stroke_opacity,
        };
        match write_mark(&mut self.writer, &built.mark, rendered_style)? {
            WrittenMark::Path => self.report.paths_written += 1,
            WrittenMark::Dot => self.report.dots_written += 1,
        }
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
        for kind in losses.iter() {
            if self.options.loss_handling(kind) == SvgLossHandling::Reject {
                self.record_skip(SvgSkipKind::PolicyRejected);
                return Err(Error::LossyConversion { stroke_index, kind });
            }
        }
        for kind in losses.iter() {
            self.report.losses[kind.index()] += 1;
        }
        Ok(())
    }

    fn record_skip(&mut self, kind: SvgSkipKind) {
        self.report.strokes_skipped += 1;
        self.report.skips[kind.index()] += 1;
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
}

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
    incomplete_geometry: bool,
    bezier_fallback: bool,
}

fn build_stroke(stroke: VectorStroke<'_>, curve: CurveMode) -> BuiltStroke {
    let mut path = PathBuilder::default();
    let mut width = FactorAccumulator::default();
    let mut opacity = FactorAccumulator::default();
    for point in stroke.points() {
        width.push(point.width_factor());
        opacity.push(point.opacity_factor());
        path.push(PointData::from_vector(point), curve);
    }
    let mark = path.finish(stroke.is_closed(), curve);
    BuiltStroke {
        mark,
        width: width.finish(),
        opacity: opacity.finish(),
        incomplete_geometry: path.incomplete_geometry,
        bezier_fallback: path.bezier_fallback,
    }
}

#[derive(Debug, PartialEq)]
enum BuiltMark {
    Empty,
    Dot(Coordinate),
    Path(String),
}

#[derive(Debug, Default)]
struct PathBuilder {
    data: String,
    first: Option<PointData>,
    previous: Option<PointData>,
    segment_count: u64,
    incomplete_geometry: bool,
    bezier_fallback: bool,
}

impl PathBuilder {
    fn push(&mut self, point: Option<PointData>, curve: CurveMode) {
        let Some(point) = point else {
            self.incomplete_geometry = true;
            self.previous = None;
            return;
        };
        if self.first.is_none() {
            self.first = Some(point);
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
struct RenderStyle {
    color: [u8; 3],
    width: f64,
    opacity: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WrittenMark {
    Path,
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
            path.push_attribute(("stroke", color.as_str()));
            path.push_attribute(("stroke-width", width.as_str()));
            path.push_attribute(("stroke-opacity", opacity.as_str()));
            writer.write_event(Event::Empty(path))?;
            Ok(WrittenMark::Path)
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
                    .with_title("line < rough"),
                |_| Ok::<(), Error>(()),
            )
            .unwrap();
        assert!(matches!(
            writer.write_group(SvgGroupOptions::default(), |_| Err(Error::InvalidXmlText)),
            Err(Error::InvalidXmlText)
        ));
        let (output, _) = writer.finish().unwrap();
        let xml = String::from_utf8(output.into_inner()).unwrap();
        assert!(xml.contains("<g opacity=\"0.5\" id=\"line-layer\">"));
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
    fn approximates_splines_and_missing_controls_with_lines() {
        let mut spline = PathBuilder::default();
        spline.push(Some(point(1.0, 2.0, &[])), CurveMode::Spline);
        spline.push(Some(point(5.0, 6.0, &[])), CurveMode::Spline);
        assert_eq!(
            spline.finish(false, CurveMode::Spline),
            BuiltMark::Path("M1 2 L5 6".to_owned())
        );

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
    fn averages_variable_factors_without_point_sized_storage() {
        let mut factor = FactorAccumulator::default();
        factor.push(Some(0.5));
        factor.push(Some(1.5));
        let factor = factor.finish();
        assert_eq!(factor.value, 1.0);
        assert!(factor.variable);
        assert!(!factor.incomplete);
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
            dots_written: 1,
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
        assert_eq!(first.strokes_skipped(), 1);
        assert_eq!(first.skip_count(SvgSkipKind::EmptyStroke), 1);
        assert!(first.is_consistent());
    }
}
