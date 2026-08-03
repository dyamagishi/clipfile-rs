use crate::{Error, Result};

const NORMAL_FLAG: u32 = 0x0001;
const RULER_FLAG: u32 = 0x0004;
const FRAME_FLAG: u32 = 0x0008;
const KIND_FLAGS: u32 = NORMAL_FLAG | RULER_FLAG | FRAME_FLAG;

const STRAIGHT_FLAG: u32 = 0x0010;
const QUADRATIC_FLAG: u32 = 0x0020;
const CUBIC_FLAG: u32 = 0x0040;
const SPLINE_FLAG: u32 = 0x0080;
const CURVE_FLAGS: u32 = STRAIGHT_FLAG | QUADRATIC_FLAG | CUBIC_FLAG | SPLINE_FLAG;

const MAIN_COLOR_OFFSET: usize = 40;
const SUB_COLOR_OFFSET: usize = 52;
const STROKE_OPACITY_OFFSET: usize = 64;
const BASE_BRUSH_ID_OFFSET: usize = 72;
const LEGACY_NORMAL_RADIUS_OFFSET: usize = 76;
const MODERN_NORMAL_EXTRA_OFFSET: usize = 76;
const MODERN_NORMAL_RADIUS_OFFSET: usize = 80;
const FRAME_BRUSH_ID_OFFSET: usize = 76;
const FRAME_FILL_STYLE_ID_OFFSET: usize = 80;
const FRAME_RADIUS_OFFSET: usize = 84;

const POINT_WIDTH_FACTOR_OFFSET: usize = 56;
const POINT_OPACITY_FACTOR_OFFSET: usize = 60;
const CONTROL_POINTS_OFFSET: usize = 88;
const POSITION_SIZE: usize = 16;

/// The four size words declared at the start of a vector stroke.
///
/// This private representation keeps the semantic decoder independent of the
/// public view types in `vector.rs`. Unknown combinations intentionally do not
/// produce guessed values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SemanticLayout {
    pub(crate) header_size: u32,
    pub(crate) point_section_size: u32,
    pub(crate) point_size: u32,
    pub(crate) default_point_size: u32,
}

/// A lossless CLIP vector color.
///
/// Observed channel words include both `0xffff_ffff` and repeated 16-bit
/// patterns, so the raw values remain authoritative. The high byte is the
/// independently implemented conversion also used by the available
/// third-party parser (`clip-tools`, `Vector.py`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DecodedColor {
    pub(crate) red_raw: u32,
    pub(crate) green_raw: u32,
    pub(crate) blue_raw: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodedStrokeKind {
    Normal,
    Ruler,
    Frame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodedCurveKind {
    Straight,
    Quadratic,
    Cubic,
    Spline,
}

/// Style fields whose location is confirmed for one observed stroke layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DecodedStrokeStyle {
    pub(crate) kind: DecodedStrokeKind,
    pub(crate) main_color: DecodedColor,
    pub(crate) sub_color: DecodedColor,
    pub(crate) stroke_opacity: f64,
    /// The common word at offset 72. Frame records retain it separately from
    /// the effective frame brush identifier at offset 76.
    pub(crate) base_brush_id: u32,
    pub(crate) brush_id: u32,
    pub(crate) brush_radius: Option<f64>,
    pub(crate) fill_style_id: Option<u32>,
    /// The additional modern-normal word at offset 76 has no confirmed
    /// semantic name, but retaining it here avoids silently conflating the
    /// 88- and 92-byte normal layouts.
    pub(crate) modern_normal_extra: Option<u32>,
}

/// Confirmed per-point width and opacity factors.
///
/// These names deliberately do not claim that either value is raw tablet
/// pressure. Width factors above one occur in the observed corpus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DecodedPointFactors {
    pub(crate) width_factor: f32,
    pub(crate) opacity_factor: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DecodedPosition {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// Curve-control coordinates appended to the common 88-byte point prefix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DecodedControlPoints {
    NoControls,
    Quadratic(DecodedPosition),
    Cubic {
        first: DecodedPosition,
        second: DecodedPosition,
    },
}

/// Classifies exactly one known stroke-kind bit while preserving all other
/// flag bits for the caller.
pub(crate) const fn decode_stroke_kind(flags: u32) -> Option<DecodedStrokeKind> {
    match flags & KIND_FLAGS {
        NORMAL_FLAG => Some(DecodedStrokeKind::Normal),
        RULER_FLAG => Some(DecodedStrokeKind::Ruler),
        FRAME_FLAG => Some(DecodedStrokeKind::Frame),
        _ => None,
    }
}

/// Classifies exactly one known curve-kind bit while preserving all other
/// flag bits for the caller.
pub(crate) const fn decode_curve_kind(flags: u32) -> Option<DecodedCurveKind> {
    match flags & CURVE_FLAGS {
        STRAIGHT_FLAG => Some(DecodedCurveKind::Straight),
        QUADRATIC_FLAG => Some(DecodedCurveKind::Quadratic),
        CUBIC_FLAG => Some(DecodedCurveKind::Cubic),
        SPLINE_FLAG => Some(DecodedCurveKind::Spline),
        _ => None,
    }
}

/// Returns whether the header/kind and point-size/curve correlations each
/// match a supported field layout.
///
/// The header/kind and point/curve correlations were observed independently.
/// Header style fields and point suffixes are separately length-delimited, so
/// this accepts their valid cross-product; it does not claim that every
/// combined tuple occurred directly in the local corpus.
pub(crate) const fn is_supported_semantic_layout(layout: SemanticLayout, flags: u32) -> bool {
    if layout.default_point_size != 88 {
        return false;
    }

    let header_matches = match decode_stroke_kind(flags) {
        Some(DecodedStrokeKind::Normal) => {
            (layout.header_size == 88 && layout.point_section_size == 72)
                || (layout.header_size == 92 && layout.point_section_size == 76)
        }
        Some(DecodedStrokeKind::Ruler) => {
            layout.header_size == 76 && layout.point_section_size == 72
        }
        Some(DecodedStrokeKind::Frame) => {
            layout.header_size == 96 && layout.point_section_size == 72
        }
        None => false,
    };
    if !header_matches {
        return false;
    }

    match decode_curve_kind(flags) {
        Some(DecodedCurveKind::Straight | DecodedCurveKind::Spline) => layout.point_size == 88,
        Some(DecodedCurveKind::Quadratic) => layout.point_size == 104,
        Some(DecodedCurveKind::Cubic) => layout.point_size == 120,
        None => false,
    }
}

/// Decodes style data only for a fully correlated known layout.
///
/// An unknown layout or ambiguous flag combination returns `Ok(None)` so its
/// raw bytes remain usable by forward-compatible callers. Once the layout is
/// known, truncation and invalid numeric values are malformed data.
pub(crate) fn decode_stroke_style(
    header: &[u8],
    layout: SemanticLayout,
    flags: u32,
) -> Result<Option<DecodedStrokeStyle>> {
    if !is_supported_semantic_layout(layout, flags) {
        return Ok(None);
    }

    let expected_header_size = layout.header_size as usize;
    if header.len() < expected_header_size {
        return Err(invalid_vector_data(format!(
            "vector stroke header has {} bytes, expected {expected_header_size}",
            header.len()
        )));
    }

    let kind = decode_stroke_kind(flags).expect("known layout has one stroke kind");
    let main_color = decode_color(header, MAIN_COLOR_OFFSET, "main color")?;
    let sub_color = decode_color(header, SUB_COLOR_OFFSET, "sub color")?;
    let stroke_opacity = read_f64(header, STROKE_OPACITY_OFFSET, "stroke opacity")?;
    validate_unit_f64(stroke_opacity, "stroke opacity")?;
    let base_brush_id = read_u32(header, BASE_BRUSH_ID_OFFSET, "base brush identifier")?;

    let (brush_id, brush_radius, fill_style_id, modern_normal_extra) =
        match (kind, layout.header_size) {
            (DecodedStrokeKind::Normal, 88) => {
                let radius = read_f64(header, LEGACY_NORMAL_RADIUS_OFFSET, "brush radius")?;
                validate_nonnegative_f64(radius, "brush radius")?;
                (base_brush_id, Some(radius), None, None)
            }
            (DecodedStrokeKind::Normal, 92) => {
                let extra = read_u32(
                    header,
                    MODERN_NORMAL_EXTRA_OFFSET,
                    "modern normal extra word",
                )?;
                let radius = read_f64(header, MODERN_NORMAL_RADIUS_OFFSET, "brush radius")?;
                validate_nonnegative_f64(radius, "brush radius")?;
                (base_brush_id, Some(radius), None, Some(extra))
            }
            (DecodedStrokeKind::Ruler, 76) => (base_brush_id, None, None, None),
            (DecodedStrokeKind::Frame, 96) => {
                let frame_brush_id =
                    read_u32(header, FRAME_BRUSH_ID_OFFSET, "frame brush identifier")?;
                let fill_style_id = read_u32(
                    header,
                    FRAME_FILL_STYLE_ID_OFFSET,
                    "frame fill-style identifier",
                )?;
                let radius = read_f64(header, FRAME_RADIUS_OFFSET, "frame brush radius")?;
                validate_nonnegative_f64(radius, "frame brush radius")?;
                (frame_brush_id, Some(radius), Some(fill_style_id), None)
            }
            _ => unreachable!("supported semantic layout has a correlated header"),
        };

    Ok(Some(DecodedStrokeStyle {
        kind,
        main_color,
        sub_color,
        stroke_opacity,
        base_brush_id,
        brush_id,
        brush_radius,
        fill_style_id,
        modern_normal_extra,
    }))
}

/// Decodes width and opacity factors from the common point prefix.
pub(crate) fn decode_point_factors(
    point: &[u8],
    layout: SemanticLayout,
    stroke_flags: u32,
) -> Result<Option<DecodedPointFactors>> {
    if !is_supported_semantic_layout(layout, stroke_flags) {
        return Ok(None);
    }
    ensure_point_size(point, layout)?;

    let width_factor = read_f32(point, POINT_WIDTH_FACTOR_OFFSET, "point width factor")?;
    let opacity_factor = read_f32(point, POINT_OPACITY_FACTOR_OFFSET, "point opacity factor")?;
    validate_nonnegative_f32(width_factor, "point width factor")?;
    validate_unit_f32(opacity_factor, "point opacity factor")?;

    Ok(Some(DecodedPointFactors {
        width_factor,
        opacity_factor,
    }))
}

/// Decodes the curve-control suffix correlated with the stroke's curve flags.
pub(crate) fn decode_control_points(
    point: &[u8],
    layout: SemanticLayout,
    stroke_flags: u32,
) -> Result<Option<DecodedControlPoints>> {
    if !is_supported_semantic_layout(layout, stroke_flags) {
        return Ok(None);
    }
    ensure_point_size(point, layout)?;

    let controls = match decode_curve_kind(stroke_flags) {
        Some(DecodedCurveKind::Straight | DecodedCurveKind::Spline) => {
            DecodedControlPoints::NoControls
        }
        Some(DecodedCurveKind::Quadratic) => DecodedControlPoints::Quadratic(read_position(
            point,
            CONTROL_POINTS_OFFSET,
            "quadratic control point",
        )?),
        Some(DecodedCurveKind::Cubic) => DecodedControlPoints::Cubic {
            first: read_position(point, CONTROL_POINTS_OFFSET, "first cubic control point")?,
            second: read_position(
                point,
                CONTROL_POINTS_OFFSET + POSITION_SIZE,
                "second cubic control point",
            )?,
        },
        None => unreachable!("supported semantic layout has one curve kind"),
    };

    Ok(Some(controls))
}

fn ensure_point_size(point: &[u8], layout: SemanticLayout) -> Result<()> {
    let expected_point_size = layout.point_size as usize;
    if point.len() < expected_point_size {
        return Err(invalid_vector_data(format!(
            "vector point record has {} bytes, expected {expected_point_size}",
            point.len()
        )));
    }
    Ok(())
}

fn decode_color(bytes: &[u8], offset: usize, name: &str) -> Result<DecodedColor> {
    Ok(DecodedColor {
        red_raw: read_u32(bytes, offset, name)?,
        green_raw: read_u32(bytes, offset + 4, name)?,
        blue_raw: read_u32(bytes, offset + 8, name)?,
    })
}

fn read_u32(bytes: &[u8], offset: usize, name: &str) -> Result<u32> {
    let raw = bytes.get(offset..offset + 4).ok_or_else(|| {
        invalid_vector_data(format!("{name} is truncated at vector byte {offset}"))
    })?;
    Ok(u32::from_be_bytes(raw.try_into().expect("four-byte slice")))
}

fn read_f32(bytes: &[u8], offset: usize, name: &str) -> Result<f32> {
    Ok(f32::from_bits(read_u32(bytes, offset, name)?))
}

fn read_f64(bytes: &[u8], offset: usize, name: &str) -> Result<f64> {
    let raw = bytes.get(offset..offset + 8).ok_or_else(|| {
        invalid_vector_data(format!("{name} is truncated at vector byte {offset}"))
    })?;
    Ok(f64::from_be_bytes(
        raw.try_into().expect("eight-byte slice"),
    ))
}

fn read_position(bytes: &[u8], offset: usize, name: &str) -> Result<DecodedPosition> {
    let x = read_f64(bytes, offset, name)?;
    let y = read_f64(bytes, offset + 8, name)?;
    if !x.is_finite() || !y.is_finite() {
        return Err(invalid_vector_data(format!(
            "{name} at vector byte {offset} is not finite"
        )));
    }
    Ok(DecodedPosition { x, y })
}

fn validate_unit_f64(value: f64, name: &str) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid_vector_data(format!(
            "{name} {value} is outside 0..=1"
        )));
    }
    Ok(())
}

fn validate_nonnegative_f64(value: f64, name: &str) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(invalid_vector_data(format!(
            "{name} {value} is not a finite non-negative value"
        )));
    }
    Ok(())
}

fn validate_unit_f32(value: f32, name: &str) -> Result<()> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(invalid_vector_data(format!(
            "{name} {value} is outside 0..=1"
        )));
    }
    Ok(())
}

fn validate_nonnegative_f32(value: f32, name: &str) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(invalid_vector_data(format!(
            "{name} {value} is not a finite non-negative value"
        )));
    }
    Ok(())
}

fn invalid_vector_data(reason: String) -> Error {
    Error::InvalidVectorData { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL_STRAIGHT: u32 = NORMAL_FLAG | STRAIGHT_FLAG;
    const NORMAL_QUADRATIC: u32 = NORMAL_FLAG | QUADRATIC_FLAG;
    const NORMAL_CUBIC: u32 = NORMAL_FLAG | CUBIC_FLAG;
    const NORMAL_SPLINE: u32 = NORMAL_FLAG | SPLINE_FLAG | 0x2000;
    const RULER_STRAIGHT: u32 = RULER_FLAG | STRAIGHT_FLAG;
    const FRAME_STRAIGHT: u32 = FRAME_FLAG | STRAIGHT_FLAG | 0x0100 | 0x1000;

    const fn layout(header: u32, section: u32, point: u32) -> SemanticLayout {
        SemanticLayout {
            header_size: header,
            point_section_size: section,
            point_size: point,
            default_point_size: 88,
        }
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn write_f32(bytes: &mut [u8], offset: usize, value: f32) {
        write_u32(bytes, offset, value.to_bits());
    }

    fn write_f64(bytes: &mut [u8], offset: usize, value: f64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    fn base_header(size: usize) -> Vec<u8> {
        let mut bytes = vec![0; size];
        for (offset, value) in [
            (40, 0x4faf_4faf),
            (44, 0x56d1_56d1),
            (48, 0xcb1f_cb1f),
            (52, 0xffff_ffff),
            (56, 0x8000_0000),
            (60, 0),
        ] {
            write_u32(&mut bytes, offset, value);
        }
        write_f64(&mut bytes, STROKE_OPACITY_OFFSET, 0.75);
        write_u32(&mut bytes, BASE_BRUSH_ID_OFFSET, 41);
        bytes
    }

    fn base_point(size: usize) -> Vec<u8> {
        let mut bytes = vec![0; size];
        write_f32(&mut bytes, POINT_WIDTH_FACTOR_OFFSET, 1.375);
        write_f32(&mut bytes, POINT_OPACITY_FACTOR_OFFSET, 0.625);
        bytes
    }

    #[test]
    fn supported_composition_requires_exact_component_correlations() {
        for (layout, flags) in [
            (layout(76, 72, 88), 0x0014),
            (layout(76, 72, 88), 0x0084),
            (layout(76, 72, 88), 0x0114),
            (layout(76, 72, 104), 0x0124),
            (layout(76, 72, 120), 0x0144),
            (layout(76, 72, 88), 0x0184),
            (layout(88, 72, 88), 0x0011),
            (layout(88, 72, 104), 0x0021),
            (layout(88, 72, 88), 0x2081),
            (layout(92, 76, 88), 0x2081),
            (layout(96, 72, 104), 0x0128),
            (layout(96, 72, 88), 0x1118),
            (layout(96, 72, 120), 0x1148),
            (layout(96, 72, 88), 0x1188),
            (layout(96, 72, 88), 0x3188),
        ] {
            assert!(is_supported_semantic_layout(layout, flags));
        }

        assert!(!is_supported_semantic_layout(
            layout(92, 72, 88),
            NORMAL_STRAIGHT
        ));
        assert!(!is_supported_semantic_layout(
            layout(88, 72, 120),
            NORMAL_QUADRATIC
        ));
        assert!(!is_supported_semantic_layout(
            SemanticLayout {
                default_point_size: 104,
                ..layout(88, 72, 88)
            },
            NORMAL_STRAIGHT
        ));
        assert!(!is_supported_semantic_layout(
            layout(88, 72, 88),
            NORMAL_FLAG | RULER_FLAG | STRAIGHT_FLAG
        ));
        assert!(!is_supported_semantic_layout(
            layout(88, 72, 88),
            NORMAL_FLAG | STRAIGHT_FLAG | SPLINE_FLAG
        ));
    }

    #[test]
    fn decodes_legacy_and_modern_normal_styles() {
        let mut legacy = base_header(88);
        write_f64(&mut legacy, LEGACY_NORMAL_RADIUS_OFFSET, 19.5);
        let style = decode_stroke_style(&legacy, layout(88, 72, 88), NORMAL_STRAIGHT)
            .unwrap()
            .unwrap();
        assert_eq!(style.kind, DecodedStrokeKind::Normal);
        assert_eq!(
            style.main_color,
            DecodedColor {
                red_raw: 0x4faf_4faf,
                green_raw: 0x56d1_56d1,
                blue_raw: 0xcb1f_cb1f,
            }
        );
        assert_eq!(
            style.sub_color,
            DecodedColor {
                red_raw: 0xffff_ffff,
                green_raw: 0x8000_0000,
                blue_raw: 0,
            }
        );
        assert_eq!(style.stroke_opacity, 0.75);
        assert_eq!(style.base_brush_id, 41);
        assert_eq!(style.brush_id, 41);
        assert_eq!(style.brush_radius, Some(19.5));
        assert_eq!(style.fill_style_id, None);
        assert_eq!(style.modern_normal_extra, None);

        let mut modern = base_header(92);
        write_u32(&mut modern, MODERN_NORMAL_EXTRA_OFFSET, 1);
        write_f64(&mut modern, MODERN_NORMAL_RADIUS_OFFSET, 5.0);
        let style = decode_stroke_style(&modern, layout(92, 76, 88), NORMAL_SPLINE)
            .unwrap()
            .unwrap();
        assert_eq!(style.brush_id, 41);
        assert_eq!(style.brush_radius, Some(5.0));
        assert_eq!(style.modern_normal_extra, Some(1));
    }

    #[test]
    fn decodes_ruler_and_frame_style_placements() {
        let ruler = base_header(76);
        let style = decode_stroke_style(&ruler, layout(76, 72, 88), RULER_STRAIGHT)
            .unwrap()
            .unwrap();
        assert_eq!(style.kind, DecodedStrokeKind::Ruler);
        assert_eq!(style.brush_id, 41);
        assert_eq!(style.brush_radius, None);
        assert_eq!(style.fill_style_id, None);

        let mut frame = base_header(96);
        write_u32(&mut frame, FRAME_BRUSH_ID_OFFSET, 7);
        write_u32(&mut frame, FRAME_FILL_STYLE_ID_OFFSET, 11);
        write_f64(&mut frame, FRAME_RADIUS_OFFSET, 1.5);
        let style = decode_stroke_style(&frame, layout(96, 72, 88), FRAME_STRAIGHT)
            .unwrap()
            .unwrap();
        assert_eq!(style.kind, DecodedStrokeKind::Frame);
        assert_eq!(style.base_brush_id, 41);
        assert_eq!(style.brush_id, 7);
        assert_eq!(style.brush_radius, Some(1.5));
        assert_eq!(style.fill_style_id, Some(11));
    }

    #[test]
    fn decodes_factors_without_calling_them_pressure() {
        let point = base_point(88);
        let factors = decode_point_factors(&point, layout(88, 72, 88), NORMAL_STRAIGHT)
            .unwrap()
            .unwrap();
        assert_eq!(factors.width_factor, 1.375);
        assert_eq!(factors.opacity_factor, 0.625);
    }

    #[test]
    fn decodes_quadratic_and_cubic_control_suffixes() {
        let mut quadratic = base_point(104);
        write_f64(&mut quadratic, 88, 12.25);
        write_f64(&mut quadratic, 96, -3.5);
        assert_eq!(
            decode_control_points(&quadratic, layout(88, 72, 104), NORMAL_QUADRATIC).unwrap(),
            Some(DecodedControlPoints::Quadratic(DecodedPosition {
                x: 12.25,
                y: -3.5,
            }))
        );

        let mut cubic = base_point(120);
        for (offset, value) in [(88, 1.0), (96, 2.0), (104, 3.0), (112, 4.0)] {
            write_f64(&mut cubic, offset, value);
        }
        assert_eq!(
            decode_control_points(&cubic, layout(88, 72, 120), NORMAL_CUBIC).unwrap(),
            Some(DecodedControlPoints::Cubic {
                first: DecodedPosition { x: 1.0, y: 2.0 },
                second: DecodedPosition { x: 3.0, y: 4.0 },
            })
        );
        assert_eq!(
            decode_control_points(&base_point(88), layout(88, 72, 88), NORMAL_STRAIGHT).unwrap(),
            Some(DecodedControlPoints::NoControls)
        );
    }

    #[test]
    fn unknown_layouts_are_opaque_even_when_short() {
        assert_eq!(
            decode_stroke_style(&[], layout(80, 72, 88), NORMAL_STRAIGHT).unwrap(),
            None
        );
        assert_eq!(
            decode_point_factors(&[], layout(88, 72, 96), NORMAL_STRAIGHT).unwrap(),
            None
        );
        assert_eq!(
            decode_control_points(&[], layout(88, 72, 88), NORMAL_FLAG).unwrap(),
            None
        );
    }

    #[test]
    fn supported_truncated_records_are_errors() {
        assert!(matches!(
            decode_stroke_style(&[0; 75], layout(76, 72, 88), RULER_STRAIGHT),
            Err(Error::InvalidVectorData { .. })
        ));
        assert!(matches!(
            decode_point_factors(&[0; 87], layout(88, 72, 88), NORMAL_STRAIGHT),
            Err(Error::InvalidVectorData { .. })
        ));
        assert!(matches!(
            decode_control_points(&[0; 103], layout(88, 72, 104), NORMAL_QUADRATIC),
            Err(Error::InvalidVectorData { .. })
        ));
    }

    #[test]
    fn invalid_style_numbers_are_rejected() {
        for opacity in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
            let mut header = base_header(88);
            write_f64(&mut header, STROKE_OPACITY_OFFSET, opacity);
            write_f64(&mut header, LEGACY_NORMAL_RADIUS_OFFSET, 1.0);
            assert!(matches!(
                decode_stroke_style(&header, layout(88, 72, 88), NORMAL_STRAIGHT),
                Err(Error::InvalidVectorData { .. })
            ));
        }

        for radius in [f64::NAN, f64::INFINITY, -0.01] {
            let mut header = base_header(88);
            write_f64(&mut header, LEGACY_NORMAL_RADIUS_OFFSET, radius);
            assert!(matches!(
                decode_stroke_style(&header, layout(88, 72, 88), NORMAL_STRAIGHT),
                Err(Error::InvalidVectorData { .. })
            ));
        }
    }

    #[test]
    fn invalid_point_numbers_are_rejected() {
        for width in [f32::NAN, f32::INFINITY, -0.01] {
            let mut point = base_point(88);
            write_f32(&mut point, POINT_WIDTH_FACTOR_OFFSET, width);
            assert!(matches!(
                decode_point_factors(&point, layout(88, 72, 88), NORMAL_STRAIGHT),
                Err(Error::InvalidVectorData { .. })
            ));
        }

        for opacity in [f32::NAN, f32::INFINITY, -0.01, 1.01] {
            let mut point = base_point(88);
            write_f32(&mut point, POINT_OPACITY_FACTOR_OFFSET, opacity);
            assert!(matches!(
                decode_point_factors(&point, layout(88, 72, 88), NORMAL_STRAIGHT),
                Err(Error::InvalidVectorData { .. })
            ));
        }

        let mut point = base_point(104);
        write_f64(&mut point, CONTROL_POINTS_OFFSET, f64::NAN);
        write_f64(&mut point, CONTROL_POINTS_OFFSET + 8, 0.0);
        assert!(matches!(
            decode_control_points(&point, layout(88, 72, 104), NORMAL_QUADRATIC),
            Err(Error::InvalidVectorData { .. })
        ));
    }
}
