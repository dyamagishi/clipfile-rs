use rusqlite::{OptionalExtension, Row, params, types::ValueRef};

use crate::{Database, Error, Limits, Result};

const STYLE_TABLE: &str = "BrushStyle";
const GRAPH_TABLE: &str = "BrushEffectorGraphData";
const MANAGER_TABLE: &str = "BrushStyleManager";

/// Stored configuration for one brush dynamic such as size or opacity.
///
/// CLIP stores a four-byte flag word and, for graph-backed dynamics, an
/// observed twelve-byte form whose final word references
/// [`BrushEffectorGraph::id`]. The complete bytes remain available for future
/// input-source semantics that have not yet been independently identified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrushEffector {
    raw: Box<[u8]>,
    flags: Option<u32>,
    auxiliary_word: Option<u32>,
    graph_id: Option<u32>,
}

impl BrushEffector {
    fn parse(raw: Vec<u8>) -> Self {
        let flags = read_be_u32(&raw, 0);
        let (auxiliary_word, graph_id) = if raw.len() == 12 {
            (
                read_be_u32(&raw, 4),
                read_be_u32(&raw, 8).filter(|&value| value != 0),
            )
        } else {
            (None, None)
        };
        Self {
            raw: raw.into_boxed_slice(),
            flags,
            auxiliary_word,
            graph_id,
        }
    }

    /// Complete stored effector bytes.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Leading big-endian flag word, when present.
    #[must_use]
    pub const fn flags(&self) -> Option<u32> {
        self.flags
    }

    /// Middle word of the observed graph-backed representation.
    #[must_use]
    pub const fn auxiliary_word(&self) -> Option<u32> {
        self.auxiliary_word
    }

    /// Referenced brush-effector graph identifier, when stored.
    #[must_use]
    pub const fn graph_id(&self) -> Option<u32> {
        self.graph_id
    }
}

/// One normalized point from a brush dynamics response graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushEffectorPoint {
    input: f64,
    output: f64,
}

impl BrushEffectorPoint {
    /// Stored graph input coordinate.
    #[must_use]
    pub const fn input(self) -> f64 {
        self.input
    }

    /// Stored graph output coordinate.
    #[must_use]
    pub const fn output(self) -> f64 {
        self.output
    }
}

/// A graph used by one or more brush dynamics effectors.
#[derive(Clone, Debug, PartialEq)]
pub struct BrushEffectorGraph {
    id: u32,
    canvas_id: i64,
    next_id: Option<u32>,
    point_stride: u32,
    points: Vec<BrushEffectorPoint>,
    raw_points: Box<[u8]>,
}

impl BrushEffectorGraph {
    /// `BrushEffectorGraphData.MainId`.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Owning canvas identifier.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }

    /// Next row in CLIP's linked list.
    #[must_use]
    pub const fn next_id(&self) -> Option<u32> {
        self.next_id
    }

    /// Bytes stored for each graph point record.
    #[must_use]
    pub const fn point_stride(&self) -> u32 {
        self.point_stride
    }

    /// Decoded big-endian `f64` input/output pairs.
    #[must_use]
    pub fn points(&self) -> &[BrushEffectorPoint] {
        &self.points
    }

    /// Complete point payload, including any future per-point suffix.
    #[must_use]
    pub fn raw_points(&self) -> &[u8] {
        &self.raw_points
    }
}

/// Stored brush renderer settings referenced by a normal vector stroke.
#[derive(Clone, Debug, PartialEq)]
pub struct BrushStyle {
    id: u32,
    canvas_id: i64,
    next_id: Option<u32>,
    style_flags: u32,
    pen_radius: f64,
    size_effector: BrushEffector,
    opacity_effector: BrushEffector,
    flow_base: f64,
    flow_effector: BrushEffector,
    anti_alias: i64,
    mipmap_index_to_plot: Option<u32>,
    hardness: f64,
    interval_base: f64,
    interval_effector: BrushEffector,
    thickness_base: f64,
    thickness_effector: BrushEffector,
    rotation_base: f64,
    rotation_effector: u32,
    rotation_random: f64,
    pattern_style_id: Option<u32>,
    composite_mode: i64,
    texture_pattern_id: Option<u32>,
    texture_flags: u32,
    texture_scale: f64,
    texture_rotation: f64,
    texture_density_base: f64,
    texture_density_effector: BrushEffector,
    sub_color_base: f64,
    sub_color_effector: BrushEffector,
    spray_flags: u32,
    spray_size_base: f64,
    spray_size_effector: BrushEffector,
    spray_density_base: f64,
    spray_density_effector: BrushEffector,
    fixed_spray_id: Option<u32>,
}

impl BrushStyle {
    /// `BrushStyle.MainId`, referenced by [`crate::VectorStrokeStyle::brush_style_id`].
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }
    /// Owning canvas identifier.
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }
    /// Next row in CLIP's linked list.
    #[must_use]
    pub const fn next_id(&self) -> Option<u32> {
        self.next_id
    }
    /// Complete stored style flag word.
    #[must_use]
    pub const fn style_flags(&self) -> u32 {
        self.style_flags
    }
    /// Brush radius stored with the style.
    #[must_use]
    pub const fn pen_radius(&self) -> f64 {
        self.pen_radius
    }
    /// Size dynamics configuration.
    #[must_use]
    pub const fn size_effector(&self) -> &BrushEffector {
        &self.size_effector
    }
    /// Opacity dynamics configuration.
    #[must_use]
    pub const fn opacity_effector(&self) -> &BrushEffector {
        &self.opacity_effector
    }
    /// Base flow or brush-density value.
    #[must_use]
    pub const fn flow_base(&self) -> f64 {
        self.flow_base
    }
    /// Flow dynamics configuration.
    #[must_use]
    pub const fn flow_effector(&self) -> &BrushEffector {
        &self.flow_effector
    }
    /// Stored anti-alias mode.
    #[must_use]
    pub const fn anti_alias(&self) -> i64 {
        self.anti_alias
    }
    /// Referenced mipmap used while plotting the brush tip.
    #[must_use]
    pub const fn mipmap_index_to_plot(&self) -> Option<u32> {
        self.mipmap_index_to_plot
    }
    /// Stored brush-tip hardness.
    #[must_use]
    pub const fn hardness(&self) -> f64 {
        self.hardness
    }
    /// Base dab interval.
    #[must_use]
    pub const fn interval_base(&self) -> f64 {
        self.interval_base
    }
    /// Dab-interval dynamics configuration.
    #[must_use]
    pub const fn interval_effector(&self) -> &BrushEffector {
        &self.interval_effector
    }
    /// Base brush-tip thickness/aspect value.
    #[must_use]
    pub const fn thickness_base(&self) -> f64 {
        self.thickness_base
    }
    /// Brush-tip thickness dynamics configuration.
    #[must_use]
    pub const fn thickness_effector(&self) -> &BrushEffector {
        &self.thickness_effector
    }
    /// Base brush-tip rotation.
    #[must_use]
    pub const fn rotation_base(&self) -> f64 {
        self.rotation_base
    }
    /// Stored rotation effector selector.
    #[must_use]
    pub const fn rotation_effector(&self) -> u32 {
        self.rotation_effector
    }
    /// Stored random-rotation amount.
    #[must_use]
    pub const fn rotation_random(&self) -> f64 {
        self.rotation_random
    }
    /// Referenced multi-image brush-tip style.
    #[must_use]
    pub const fn pattern_style_id(&self) -> Option<u32> {
        self.pattern_style_id
    }
    /// Stored brush composite mode.
    #[must_use]
    pub const fn composite_mode(&self) -> i64 {
        self.composite_mode
    }
    /// Referenced texture pattern.
    #[must_use]
    pub const fn texture_pattern_id(&self) -> Option<u32> {
        self.texture_pattern_id
    }
    /// Complete stored texture flag word.
    #[must_use]
    pub const fn texture_flags(&self) -> u32 {
        self.texture_flags
    }
    /// Stored texture scale.
    #[must_use]
    pub const fn texture_scale(&self) -> f64 {
        self.texture_scale
    }
    /// Stored texture rotation.
    #[must_use]
    pub const fn texture_rotation(&self) -> f64 {
        self.texture_rotation
    }
    /// Base texture-density value.
    #[must_use]
    pub const fn texture_density_base(&self) -> f64 {
        self.texture_density_base
    }
    /// Texture-density dynamics configuration.
    #[must_use]
    pub const fn texture_density_effector(&self) -> &BrushEffector {
        &self.texture_density_effector
    }
    /// Base amount of secondary-color mixing.
    #[must_use]
    pub const fn sub_color_base(&self) -> f64 {
        self.sub_color_base
    }
    /// Secondary-color dynamics configuration.
    #[must_use]
    pub const fn sub_color_effector(&self) -> &BrushEffector {
        &self.sub_color_effector
    }
    /// Complete stored spray flag word.
    #[must_use]
    pub const fn spray_flags(&self) -> u32 {
        self.spray_flags
    }
    /// Base spray-area size.
    #[must_use]
    pub const fn spray_size_base(&self) -> f64 {
        self.spray_size_base
    }
    /// Spray-area size dynamics configuration.
    #[must_use]
    pub const fn spray_size_effector(&self) -> &BrushEffector {
        &self.spray_size_effector
    }
    /// Base spray density.
    #[must_use]
    pub const fn spray_density_base(&self) -> f64 {
        self.spray_density_base
    }
    /// Spray-density dynamics configuration.
    #[must_use]
    pub const fn spray_density_effector(&self) -> &BrushEffector {
        &self.spray_density_effector
    }
    /// Referenced fixed-spray record.
    #[must_use]
    pub const fn fixed_spray_id(&self) -> Option<u32> {
        self.fixed_spray_id
    }

    /// Whether the stored tip is a hard, circular, untextured non-spray tip.
    ///
    /// Point width and opacity factors remain separate and still need to be
    /// applied by the renderer. This predicate only classifies the brush-tip
    /// shape represented by the fields exposed here.
    #[must_use]
    pub fn is_plain_round_tip(&self) -> bool {
        self.mipmap_index_to_plot.is_none()
            && self.pattern_style_id.is_none()
            && self.texture_pattern_id.is_none()
            && self.texture_flags == 0
            && self.spray_flags == 0
            && self.fixed_spray_id.is_none()
            && (self.hardness - 1.0).abs() <= f64::EPSILON
            && (self.thickness_base - 1.0).abs() <= f64::EPSILON
            && (self.sub_color_base).abs() <= f64::EPSILON
    }
}

/// Top-level roots of the brush metadata linked lists for one canvas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrushStyleManager {
    id: u32,
    canvas_id: i64,
    first_graph_id: Option<u32>,
    first_pattern_image_id: Option<u32>,
    first_pattern_id: Option<u32>,
    first_fixed_spray_id: Option<u32>,
    first_brush_style_id: Option<u32>,
    first_fill_style_id: Option<u32>,
}

impl BrushStyleManager {
    #[must_use]
    pub const fn id(self) -> u32 {
        self.id
    }
    #[must_use]
    pub const fn canvas_id(self) -> i64 {
        self.canvas_id
    }
    #[must_use]
    pub const fn first_graph_id(self) -> Option<u32> {
        self.first_graph_id
    }
    #[must_use]
    pub const fn first_pattern_image_id(self) -> Option<u32> {
        self.first_pattern_image_id
    }
    #[must_use]
    pub const fn first_pattern_id(self) -> Option<u32> {
        self.first_pattern_id
    }
    #[must_use]
    pub const fn first_fixed_spray_id(self) -> Option<u32> {
        self.first_fixed_spray_id
    }
    #[must_use]
    pub const fn first_brush_style_id(self) -> Option<u32> {
        self.first_brush_style_id
    }
    #[must_use]
    pub const fn first_fill_style_id(self) -> Option<u32> {
        self.first_fill_style_id
    }
}

/// One stored image that can participate in a multi-image brush tip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrushPatternImage {
    id: u32,
    canvas_id: i64,
    next_id: Option<u32>,
    uuid: Box<[u8]>,
    name: Option<String>,
    mipmap_id: Option<u32>,
}

impl BrushPatternImage {
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }
    #[must_use]
    pub const fn next_id(&self) -> Option<u32> {
        self.next_id
    }
    #[must_use]
    pub fn uuid(&self) -> &[u8] {
        &self.uuid
    }
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    #[must_use]
    pub const fn mipmap_id(&self) -> Option<u32> {
        self.mipmap_id
    }
}

/// Stored ordering metadata for a multi-image brush tip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrushPatternStyle {
    id: u32,
    canvas_id: i64,
    next_id: Option<u32>,
    image_count: u64,
    raw_image_indices: Box<[u8]>,
    order_type: i64,
    reverse: bool,
    reverse_secondary: bool,
}

impl BrushPatternStyle {
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }
    #[must_use]
    pub const fn next_id(&self) -> Option<u32> {
        self.next_id
    }
    #[must_use]
    pub const fn image_count(&self) -> u64 {
        self.image_count
    }
    /// Opaque stored image-index array; its element encoding remains unconfirmed.
    #[must_use]
    pub fn raw_image_indices(&self) -> &[u8] {
        &self.raw_image_indices
    }
    #[must_use]
    pub const fn order_type(&self) -> i64 {
        self.order_type
    }
    #[must_use]
    pub const fn is_reversed(&self) -> bool {
        self.reverse
    }
    #[must_use]
    pub const fn is_secondary_reversed(&self) -> bool {
        self.reverse_secondary
    }
}

/// Stored fixed-spray dab coordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrushFixedSpray {
    id: u32,
    canvas_id: i64,
    next_id: Option<u32>,
    plot_count: u64,
    plot_stride: u32,
    raw_plot_points: Box<[u8]>,
}

impl BrushFixedSpray {
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }
    #[must_use]
    pub const fn canvas_id(&self) -> i64 {
        self.canvas_id
    }
    #[must_use]
    pub const fn next_id(&self) -> Option<u32> {
        self.next_id
    }
    #[must_use]
    pub const fn plot_count(&self) -> u64 {
        self.plot_count
    }
    #[must_use]
    pub const fn plot_stride(&self) -> u32 {
        self.plot_stride
    }
    #[must_use]
    pub fn raw_plot_points(&self) -> &[u8] {
        &self.raw_plot_points
    }
}

/// Fill settings referenced by frame vector strokes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FillStyle {
    id: u32,
    canvas_id: i64,
    next_id: Option<u32>,
    style_flags: u32,
    anti_alias: i64,
    composite_mode: i64,
    texture_density: i64,
}

impl FillStyle {
    #[must_use]
    pub const fn id(self) -> u32 {
        self.id
    }
    #[must_use]
    pub const fn canvas_id(self) -> i64 {
        self.canvas_id
    }
    #[must_use]
    pub const fn next_id(self) -> Option<u32> {
        self.next_id
    }
    #[must_use]
    pub const fn style_flags(self) -> u32 {
        self.style_flags
    }
    #[must_use]
    pub const fn anti_alias(self) -> i64 {
        self.anti_alias
    }
    #[must_use]
    pub const fn composite_mode(self) -> i64 {
        self.composite_mode
    }
    #[must_use]
    pub const fn texture_density(self) -> i64 {
        self.texture_density
    }
}

impl Database {
    /// Reads one brush style by the identifier stored in a vector stroke.
    /// Files without modern brush tables return `None`.
    pub fn brush_style(&self, id: u32, limits: Limits) -> Result<Option<BrushStyle>> {
        if self.schema().table(STYLE_TABLE).is_none() {
            return Ok(None);
        }
        require_style_columns(self)?;
        let mut statement = self.connection().prepare(STYLE_QUERY_BY_ID)?;
        let raw = statement
            .query_row(params![i64::from(id)], read_style_row)
            .optional()?;
        raw.map(|raw| finish_style(raw, limits)).transpose()
    }

    /// Reads every brush style owned by one canvas, ordered by identifier.
    pub fn brush_styles(&self, canvas_id: i64, limits: Limits) -> Result<Vec<BrushStyle>> {
        if self.schema().table(STYLE_TABLE).is_none() {
            return Ok(Vec::new());
        }
        require_style_columns(self)?;
        let mut statement = self.connection().prepare(STYLE_QUERY_BY_CANVAS)?;
        let rows = statement
            .query_map(params![canvas_id], read_style_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        enforce_count(rows.len(), limits.max_brush_items(), "brush styles")?;
        rows.into_iter()
            .map(|raw| finish_style(raw, limits))
            .collect()
    }

    /// Reads one brush-dynamics graph by identifier.
    pub fn brush_effector_graph(
        &self,
        id: u32,
        limits: Limits,
    ) -> Result<Option<BrushEffectorGraph>> {
        if self.schema().table(GRAPH_TABLE).is_none() {
            return Ok(None);
        }
        require_graph_columns(self)?;
        let raw = self
            .connection()
            .query_row(
                "SELECT MainId, CanvasId, NextIndex, ControlNumber, ControlDataSize, ControlPoints \
                 FROM BrushEffectorGraphData WHERE MainId = ?1 LIMIT 1",
                params![i64::from(id)],
                read_graph_row,
            )
            .optional()?;
        raw.map(|raw| finish_graph(raw, limits)).transpose()
    }

    /// Reads every brush-dynamics graph owned by one canvas.
    pub fn brush_effector_graphs(
        &self,
        canvas_id: i64,
        limits: Limits,
    ) -> Result<Vec<BrushEffectorGraph>> {
        if self.schema().table(GRAPH_TABLE).is_none() {
            return Ok(Vec::new());
        }
        require_graph_columns(self)?;
        let mut statement = self.connection().prepare(
            "SELECT MainId, CanvasId, NextIndex, ControlNumber, ControlDataSize, ControlPoints \
             FROM BrushEffectorGraphData WHERE CanvasId = ?1 ORDER BY MainId",
        )?;
        let rows = statement
            .query_map(params![canvas_id], read_graph_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        enforce_count(
            rows.len(),
            limits.max_brush_items(),
            "brush effector graphs",
        )?;
        rows.into_iter()
            .map(|raw| finish_graph(raw, limits))
            .collect()
    }

    /// Reads the brush metadata manager for one canvas.
    pub fn brush_style_manager(&self, canvas_id: i64) -> Result<Option<BrushStyleManager>> {
        if self.schema().table(MANAGER_TABLE).is_none() {
            return Ok(None);
        }
        for column in [
            "MainId",
            "CanvasId",
            "FirstGraphData",
            "FirstPatternImage",
            "FirstPattern",
            "FirstFixedSpray",
            "FirstBrushStyle",
            "FirstFillStyle",
        ] {
            self.require_column(MANAGER_TABLE, column)?;
        }
        let mut statement = self.connection().prepare(
            "SELECT MainId, CanvasId, FirstGraphData, FirstPatternImage, FirstPattern, \
             FirstFixedSpray, FirstBrushStyle, FirstFillStyle \
             FROM BrushStyleManager WHERE CanvasId = ?1 ORDER BY MainId",
        )?;
        let mut rows = statement.query(params![canvas_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let manager = BrushStyleManager {
            id: required_id(row.get(0)?, "BrushStyleManager.MainId")?,
            canvas_id: row.get(1)?,
            first_graph_id: optional_id(row.get(2)?, "FirstGraphData")?,
            first_pattern_image_id: optional_id(row.get(3)?, "FirstPatternImage")?,
            first_pattern_id: optional_id(row.get(4)?, "FirstPattern")?,
            first_fixed_spray_id: optional_id(row.get(5)?, "FirstFixedSpray")?,
            first_brush_style_id: optional_id(row.get(6)?, "FirstBrushStyle")?,
            first_fill_style_id: optional_id(row.get(7)?, "FirstFillStyle")?,
        };
        if rows.next()?.is_some() {
            return Err(brush_error(format!(
                "canvas {canvas_id} has more than one BrushStyleManager row"
            )));
        }
        Ok(Some(manager))
    }

    /// Reads one stored brush-tip image. Older files without the table return `None`.
    pub fn brush_pattern_image(
        &self,
        id: u32,
        limits: Limits,
    ) -> Result<Option<BrushPatternImage>> {
        const TABLE: &str = "BrushPatternImage";
        if self.schema().table(TABLE).is_none() {
            return Ok(None);
        }
        for column in ["MainId", "CanvasId", "NextIndex", "Uuid", "Name", "Mipmap"] {
            self.require_column(TABLE, column)?;
        }
        let raw = self
            .connection()
            .query_row(
                "SELECT MainId, CanvasId, NextIndex, Uuid, Name, Mipmap \
             FROM BrushPatternImage WHERE MainId = ?1 LIMIT 1",
                params![i64::from(id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        owned_bytes(row.get_ref(3)?, 3, "Uuid")?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, canvas_id, next_id, uuid, name, mipmap_id)) = raw else {
            return Ok(None);
        };
        enforce_bytes(
            uuid.len(),
            limits.max_brush_bytes(),
            "brush pattern UUID bytes",
        )?;
        Ok(Some(BrushPatternImage {
            id: required_id(id, "BrushPatternImage.MainId")?,
            canvas_id,
            next_id: optional_id(next_id, "BrushPatternImage.NextIndex")?,
            uuid: uuid.into_boxed_slice(),
            name,
            mipmap_id: optional_id(mipmap_id, "BrushPatternImage.Mipmap")?,
        }))
    }

    /// Reads one multi-image brush-tip style while preserving its raw index array.
    pub fn brush_pattern_style(
        &self,
        id: u32,
        limits: Limits,
    ) -> Result<Option<BrushPatternStyle>> {
        const TABLE: &str = "BrushPatternStyle";
        if self.schema().table(TABLE).is_none() {
            return Ok(None);
        }
        for column in [
            "MainId",
            "CanvasId",
            "NextIndex",
            "ImageNumber",
            "ImageIndex",
            "OrderType",
            "Reverse",
            "Reverse2",
        ] {
            self.require_column(TABLE, column)?;
        }
        let raw = self.connection().query_row(
            "SELECT MainId, CanvasId, NextIndex, ImageNumber, ImageIndex, OrderType, Reverse, Reverse2 \
             FROM BrushPatternStyle WHERE MainId = ?1 LIMIT 1",
            params![i64::from(id)],
            |row| Ok((
                row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<i64>>(2)?,
                row.get::<_, i64>(3)?, owned_bytes(row.get_ref(4)?, 4, "ImageIndex")?,
                row.get::<_, i64>(5)?, row.get::<_, i64>(6)?, row.get::<_, i64>(7)?,
            )),
        ).optional()?;
        let Some((
            id,
            canvas_id,
            next_id,
            image_count,
            indices,
            order_type,
            reverse,
            reverse_secondary,
        )) = raw
        else {
            return Ok(None);
        };
        let image_count = nonnegative(image_count, "BrushPatternStyle.ImageNumber")?;
        if image_count > limits.max_brush_items() {
            return Err(Error::LimitExceeded {
                resource: "brush pattern images",
                value: image_count,
                limit: limits.max_brush_items(),
            });
        }
        enforce_bytes(
            indices.len(),
            limits.max_brush_bytes(),
            "brush pattern index bytes",
        )?;
        Ok(Some(BrushPatternStyle {
            id: required_id(id, "BrushPatternStyle.MainId")?,
            canvas_id,
            next_id: optional_id(next_id, "BrushPatternStyle.NextIndex")?,
            image_count,
            raw_image_indices: indices.into_boxed_slice(),
            order_type,
            reverse: sqlite_bool(reverse, "BrushPatternStyle.Reverse")?,
            reverse_secondary: sqlite_bool(reverse_secondary, "BrushPatternStyle.Reverse2")?,
        }))
    }

    /// Reads one fixed-spray record while preserving its plot payload.
    pub fn brush_fixed_spray(&self, id: u32, limits: Limits) -> Result<Option<BrushFixedSpray>> {
        const TABLE: &str = "BrushFixedSpray";
        if self.schema().table(TABLE).is_none() {
            return Ok(None);
        }
        for column in [
            "MainId",
            "CanvasId",
            "NextIndex",
            "PlotNumber",
            "PlotDataSize",
            "PlotPoints",
        ] {
            self.require_column(TABLE, column)?;
        }
        let raw = self
            .connection()
            .query_row(
                "SELECT MainId, CanvasId, NextIndex, PlotNumber, PlotDataSize, PlotPoints \
             FROM BrushFixedSpray WHERE MainId = ?1 LIMIT 1",
                params![i64::from(id)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        owned_bytes(row.get_ref(5)?, 5, "PlotPoints")?,
                    ))
                },
            )
            .optional()?;
        let Some((id, canvas_id, next_id, plot_count, plot_stride, points)) = raw else {
            return Ok(None);
        };
        let plot_count = nonnegative(plot_count, "BrushFixedSpray.PlotNumber")?;
        let plot_stride = nonnegative(plot_stride, "BrushFixedSpray.PlotDataSize")?;
        if plot_count > limits.max_brush_items() {
            return Err(Error::LimitExceeded {
                resource: "fixed spray plots",
                value: plot_count,
                limit: limits.max_brush_items(),
            });
        }
        enforce_bytes(
            points.len(),
            limits.max_brush_bytes(),
            "fixed spray plot bytes",
        )?;
        let expected = plot_count
            .checked_mul(plot_stride)
            .ok_or_else(|| brush_error("fixed spray size overflow"))?;
        if expected != points.len() as u64 {
            return Err(brush_error(format!(
                "fixed spray {id} declares {plot_count}x{plot_stride} bytes but stores {}",
                points.len()
            )));
        }
        Ok(Some(BrushFixedSpray {
            id: required_id(id, "BrushFixedSpray.MainId")?,
            canvas_id,
            next_id: optional_id(next_id, "BrushFixedSpray.NextIndex")?,
            plot_count,
            plot_stride: u32::try_from(plot_stride)
                .map_err(|_| brush_error("fixed spray stride exceeds u32"))?,
            raw_plot_points: points.into_boxed_slice(),
        }))
    }

    /// Reads one frame fill style. Files without frame-style tables return `None`.
    pub fn fill_style(&self, id: u32) -> Result<Option<FillStyle>> {
        const TABLE: &str = "FillStyle";
        if self.schema().table(TABLE).is_none() {
            return Ok(None);
        }
        for column in [
            "MainId",
            "CanvasId",
            "NextIndex",
            "StyleFlag",
            "AntiAlias",
            "CompositeMode",
            "TextureDensity",
        ] {
            self.require_column(TABLE, column)?;
        }
        let raw = self.connection().query_row(
            "SELECT MainId, CanvasId, NextIndex, StyleFlag, AntiAlias, CompositeMode, TextureDensity \
             FROM FillStyle WHERE MainId = ?1 LIMIT 1",
            params![i64::from(id)],
            |row| Ok((
                row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Option<i64>>(2)?,
                row.get::<_, i64>(3)?, row.get::<_, i64>(4)?, row.get::<_, i64>(5)?, row.get::<_, i64>(6)?,
            )),
        ).optional()?;
        let Some((
            id,
            canvas_id,
            next_id,
            style_flags,
            anti_alias,
            composite_mode,
            texture_density,
        )) = raw
        else {
            return Ok(None);
        };
        Ok(Some(FillStyle {
            id: required_id(id, "FillStyle.MainId")?,
            canvas_id,
            next_id: optional_id(next_id, "FillStyle.NextIndex")?,
            style_flags: word(style_flags, "FillStyle.StyleFlag")?,
            anti_alias,
            composite_mode,
            texture_density,
        }))
    }
}

const STYLE_COLUMNS: &[&str] = &[
    "MainId",
    "CanvasId",
    "NextIndex",
    "StyleFlag",
    "PenRadius",
    "SizeEffector",
    "OpacityEffector",
    "FlowBase",
    "FlowEffector",
    "AntiAlias",
    "MipmapIndexToPlot",
    "Hardness",
    "IntervalBase",
    "IntervalEffector",
    "ThicknessBase",
    "ThicknessEffector",
    "RotationBase",
    "RotationEffector",
    "RotationRandom",
    "PatternStyle",
    "CompositeMode",
    "TexturePattern",
    "TextureFlag",
    "TextureScale",
    "TextureRotate",
    "TextureDensityBase",
    "TextureDensityEffector",
    "SubColorBase",
    "SubColorEffector",
    "SprayFlag",
    "SpraySizeBase",
    "SpraySizeEffector",
    "SprayDensityBase",
    "SprayDensityEffector",
    "FixedSpray",
];

const STYLE_QUERY_BY_ID: &str = "SELECT MainId, CanvasId, NextIndex, StyleFlag, PenRadius, SizeEffector, \
    OpacityEffector, FlowBase, FlowEffector, AntiAlias, MipmapIndexToPlot, Hardness, IntervalBase, \
    IntervalEffector, ThicknessBase, ThicknessEffector, RotationBase, RotationEffector, \
    RotationRandom, PatternStyle, CompositeMode, TexturePattern, TextureFlag, TextureScale, \
    TextureRotate, TextureDensityBase, TextureDensityEffector, SubColorBase, SubColorEffector, \
    SprayFlag, SpraySizeBase, SpraySizeEffector, SprayDensityBase, SprayDensityEffector, FixedSpray \
    FROM BrushStyle WHERE MainId = ?1 LIMIT 1";

const STYLE_QUERY_BY_CANVAS: &str = "SELECT MainId, CanvasId, NextIndex, StyleFlag, PenRadius, SizeEffector, \
    OpacityEffector, FlowBase, FlowEffector, AntiAlias, MipmapIndexToPlot, Hardness, IntervalBase, \
    IntervalEffector, ThicknessBase, ThicknessEffector, RotationBase, RotationEffector, \
    RotationRandom, PatternStyle, CompositeMode, TexturePattern, TextureFlag, TextureScale, \
    TextureRotate, TextureDensityBase, TextureDensityEffector, SubColorBase, SubColorEffector, \
    SprayFlag, SpraySizeBase, SpraySizeEffector, SprayDensityBase, SprayDensityEffector, FixedSpray \
    FROM BrushStyle WHERE CanvasId = ?1 ORDER BY MainId";

#[derive(Debug)]
struct RawStyle {
    integers: [i64; 13],
    reals: [f64; 13],
    effectors: [Vec<u8>; 9],
}

fn read_style_row(row: &Row<'_>) -> rusqlite::Result<RawStyle> {
    Ok(RawStyle {
        integers: [
            row.get(0)?,
            row.get(1)?,
            row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            row.get(3)?,
            row.get(9)?,
            row.get::<_, Option<i64>>(10)?.unwrap_or(0),
            row.get(17)?,
            row.get(19)?,
            row.get(20)?,
            row.get(21)?,
            row.get(22)?,
            row.get(29)?,
            row.get::<_, Option<i64>>(34)?.unwrap_or(0),
        ],
        reals: [
            row.get(4)?,
            row.get(7)?,
            row.get(11)?,
            row.get(12)?,
            row.get(14)?,
            row.get(16)?,
            row.get(18)?,
            row.get(23)?,
            row.get(24)?,
            row.get(25)?,
            row.get(27)?,
            row.get(30)?,
            row.get(32)?,
        ],
        effectors: [
            owned_bytes(row.get_ref(5)?, 5, "SizeEffector")?,
            owned_bytes(row.get_ref(6)?, 6, "OpacityEffector")?,
            owned_bytes(row.get_ref(8)?, 8, "FlowEffector")?,
            owned_bytes(row.get_ref(13)?, 13, "IntervalEffector")?,
            owned_bytes(row.get_ref(15)?, 15, "ThicknessEffector")?,
            owned_bytes(row.get_ref(26)?, 26, "TextureDensityEffector")?,
            owned_bytes(row.get_ref(28)?, 28, "SubColorEffector")?,
            owned_bytes(row.get_ref(31)?, 31, "SpraySizeEffector")?,
            owned_bytes(row.get_ref(33)?, 33, "SprayDensityEffector")?,
        ],
    })
}

fn finish_style(raw: RawStyle, limits: Limits) -> Result<BrushStyle> {
    let bytes = raw.effectors.iter().map(Vec::len).sum::<usize>();
    enforce_bytes(bytes, limits.max_brush_bytes(), "brush effector bytes")?;
    for (name, value) in [
        ("PenRadius", raw.reals[0]),
        ("FlowBase", raw.reals[1]),
        ("Hardness", raw.reals[2]),
        ("IntervalBase", raw.reals[3]),
        ("ThicknessBase", raw.reals[4]),
        ("RotationBase", raw.reals[5]),
        ("RotationRandom", raw.reals[6]),
        ("TextureScale", raw.reals[7]),
        ("TextureRotate", raw.reals[8]),
        ("TextureDensityBase", raw.reals[9]),
        ("SubColorBase", raw.reals[10]),
        ("SpraySizeBase", raw.reals[11]),
        ("SprayDensityBase", raw.reals[12]),
    ] {
        finite(value, name)?;
    }
    let mut effectors = raw.effectors.into_iter().map(BrushEffector::parse);
    Ok(BrushStyle {
        id: required_id(raw.integers[0], "BrushStyle.MainId")?,
        canvas_id: raw.integers[1],
        next_id: optional_id(Some(raw.integers[2]), "BrushStyle.NextIndex")?,
        style_flags: word(raw.integers[3], "StyleFlag")?,
        pen_radius: raw.reals[0],
        size_effector: effectors.next().unwrap(),
        opacity_effector: effectors.next().unwrap(),
        flow_base: raw.reals[1],
        flow_effector: effectors.next().unwrap(),
        anti_alias: raw.integers[4],
        mipmap_index_to_plot: optional_id(Some(raw.integers[5]), "MipmapIndexToPlot")?,
        hardness: raw.reals[2],
        interval_base: raw.reals[3],
        interval_effector: effectors.next().unwrap(),
        thickness_base: raw.reals[4],
        thickness_effector: effectors.next().unwrap(),
        rotation_base: raw.reals[5],
        rotation_effector: word(raw.integers[6], "RotationEffector")?,
        rotation_random: raw.reals[6],
        pattern_style_id: optional_id(Some(raw.integers[7]), "PatternStyle")?,
        composite_mode: raw.integers[8],
        texture_pattern_id: optional_id(Some(raw.integers[9]), "TexturePattern")?,
        texture_flags: word(raw.integers[10], "TextureFlag")?,
        texture_scale: raw.reals[7],
        texture_rotation: raw.reals[8],
        texture_density_base: raw.reals[9],
        texture_density_effector: effectors.next().unwrap(),
        sub_color_base: raw.reals[10],
        sub_color_effector: effectors.next().unwrap(),
        spray_flags: word(raw.integers[11], "SprayFlag")?,
        spray_size_base: raw.reals[11],
        spray_size_effector: effectors.next().unwrap(),
        spray_density_base: raw.reals[12],
        spray_density_effector: effectors.next().unwrap(),
        fixed_spray_id: optional_id(Some(raw.integers[12]), "FixedSpray")?,
    })
}

#[derive(Debug)]
struct RawGraph {
    id: i64,
    canvas_id: i64,
    next_id: Option<i64>,
    point_count: i64,
    stride: i64,
    points: Vec<u8>,
}

fn read_graph_row(row: &Row<'_>) -> rusqlite::Result<RawGraph> {
    Ok(RawGraph {
        id: row.get(0)?,
        canvas_id: row.get(1)?,
        next_id: row.get(2)?,
        point_count: row.get(3)?,
        stride: row.get(4)?,
        points: owned_bytes(row.get_ref(5)?, 5, "ControlPoints")?,
    })
}

fn finish_graph(raw: RawGraph, limits: Limits) -> Result<BrushEffectorGraph> {
    let count = nonnegative(raw.point_count, "ControlNumber")?;
    let stride = nonnegative(raw.stride, "ControlDataSize")?;
    if count > limits.max_brush_items() {
        return Err(Error::LimitExceeded {
            resource: "brush graph points",
            value: count,
            limit: limits.max_brush_items(),
        });
    }
    enforce_bytes(
        raw.points.len(),
        limits.max_brush_bytes(),
        "brush graph bytes",
    )?;
    let expected = count
        .checked_mul(stride)
        .ok_or_else(|| brush_error("brush graph size overflow"))?;
    if expected != raw.points.len() as u64 {
        return Err(brush_error(format!(
            "brush graph {} declares {count}x{stride} bytes but stores {}",
            raw.id,
            raw.points.len()
        )));
    }
    if count != 0 && stride < 16 {
        return Err(brush_error(format!(
            "brush graph {} point stride {stride} is smaller than 16",
            raw.id
        )));
    }
    let stride_usize = usize::try_from(stride)
        .map_err(|_| brush_error("brush graph stride does not fit memory"))?;
    let mut points = Vec::with_capacity(
        usize::try_from(count).map_err(|_| brush_error("brush graph count does not fit memory"))?,
    );
    for record in raw.points.chunks_exact(stride_usize) {
        let input = f64::from_bits(u64::from_be_bytes(record[0..8].try_into().unwrap()));
        let output = f64::from_bits(u64::from_be_bytes(record[8..16].try_into().unwrap()));
        finite(input, "brush graph input")?;
        finite(output, "brush graph output")?;
        points.push(BrushEffectorPoint { input, output });
    }
    Ok(BrushEffectorGraph {
        id: required_id(raw.id, "BrushEffectorGraphData.MainId")?,
        canvas_id: raw.canvas_id,
        next_id: optional_id(raw.next_id, "BrushEffectorGraphData.NextIndex")?,
        point_stride: u32::try_from(stride)
            .map_err(|_| brush_error("brush graph stride exceeds u32"))?,
        points,
        raw_points: raw.points.into_boxed_slice(),
    })
}

fn require_style_columns(database: &Database) -> Result<()> {
    for &column in STYLE_COLUMNS {
        database.require_column(STYLE_TABLE, column)?;
    }
    Ok(())
}

fn require_graph_columns(database: &Database) -> Result<()> {
    for column in [
        "MainId",
        "CanvasId",
        "NextIndex",
        "ControlNumber",
        "ControlDataSize",
        "ControlPoints",
    ] {
        database.require_column(GRAPH_TABLE, column)?;
    }
    Ok(())
}

fn owned_bytes(value: ValueRef<'_>, column: usize, name: &str) -> rusqlite::Result<Vec<u8>> {
    match value {
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Ok(bytes.to_vec()),
        value => Err(rusqlite::Error::InvalidColumnType(
            column,
            name.to_owned(),
            value.data_type(),
        )),
    }
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|raw| u32::from_be_bytes(raw.try_into().unwrap()))
}

fn required_id(value: i64, name: &str) -> Result<u32> {
    if value <= 0 {
        return Err(brush_error(format!(
            "{name} must be positive, found {value}"
        )));
    }
    u32::try_from(value).map_err(|_| brush_error(format!("{name} exceeds u32: {value}")))
}

fn optional_id(value: Option<i64>, name: &str) -> Result<Option<u32>> {
    match value {
        None | Some(0) => Ok(None),
        Some(value) => required_id(value, name).map(Some),
    }
}

fn word(value: i64, name: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| brush_error(format!("{name} is outside u32: {value}")))
}

fn nonnegative(value: i64, name: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| brush_error(format!("{name} is negative: {value}")))
}

fn sqlite_bool(value: i64, name: &str) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(brush_error(format!(
            "{name} must be zero or one, found {value}"
        ))),
    }
}

fn finite(value: f64, name: &str) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(brush_error(format!("{name} is not finite")))
    }
}

fn enforce_count(value: usize, limit: u64, resource: &'static str) -> Result<()> {
    let value = value as u64;
    if value > limit {
        Err(Error::LimitExceeded {
            resource,
            value,
            limit,
        })
    } else {
        Ok(())
    }
}

fn enforce_bytes(value: usize, limit: u64, resource: &'static str) -> Result<()> {
    let value = value as u64;
    if value > limit {
        Err(Error::LimitExceeded {
            resource,
            value,
            limit,
        })
    } else {
        Ok(())
    }
}

fn brush_error(reason: impl Into<String>) -> Error {
    Error::InvalidBrush {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    #[test]
    fn parses_effectors_and_big_endian_graph_points() {
        let effector = BrushEffector::parse(vec![0, 0, 0, 0x11, 0, 0, 0, 0, 0, 0, 0, 8]);
        assert_eq!(effector.flags(), Some(0x11));
        assert_eq!(effector.graph_id(), Some(8));

        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE BrushEffectorGraphData \
             (MainId INTEGER, CanvasId INTEGER, NextIndex INTEGER, ControlNumber INTEGER, \
              ControlDataSize INTEGER, ControlPoints BLOB); \
             INSERT INTO BrushEffectorGraphData VALUES \
             (8, 1, 0, 2, 16, \
              x'000000000000000000000000000000003ff00000000000003ff0000000000000');",
            )
            .unwrap();
        let database = Database::from_connection(connection).unwrap();
        let graph = database
            .brush_effector_graph(8, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            graph.points(),
            &[
                BrushEffectorPoint {
                    input: 0.0,
                    output: 0.0
                },
                BrushEffectorPoint {
                    input: 1.0,
                    output: 1.0
                },
            ]
        );
    }

    #[test]
    fn absent_optional_brush_tables_are_empty() {
        let database = Database::from_connection(Connection::open_in_memory().unwrap()).unwrap();
        assert!(
            database
                .brush_style(1, Limits::default())
                .unwrap()
                .is_none()
        );
        assert!(
            database
                .brush_styles(1, Limits::default())
                .unwrap()
                .is_empty()
        );
        assert!(database.brush_style_manager(1).unwrap().is_none());
    }
}
