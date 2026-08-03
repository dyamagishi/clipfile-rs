//! Reads one layer's validated vector strokes, styles, and points.

use std::{env, fs::File};

use clipfile::{ClipFile, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Vector references are queried by their owning layer ID.
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(
        "usage: cargo run --features sqlite --example inspect_vector -- <input.clip> <layer-id>",
    )?;
    let layer_id = arguments
        .next()
        .ok_or(
            "usage: cargo run --features sqlite --example inspect_vector -- <input.clip> <layer-id>",
        )?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err(
            "usage: cargo run --features sqlite --example inspect_vector -- <input.clip> <layer-id>"
                .into(),
        );
    }

    // The database resolves ownership; each external body is then parsed separately.
    let limits = Limits::default();
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let sources = database.vector_data_sources(layer_id, limits)?;
    println!("layer {layer_id}: {} vector object(s)", sources.len());

    // Unknown layouts remain available through the raw accessors.
    for source in &sources {
        let vector = clip.read_vector(&database, source, limits)?;
        println!(
            "vector object {}: canvas {}, layer {}, {} stroke(s), {} raw bytes",
            source.id(),
            source.canvas_id(),
            source.layer_id(),
            vector.stroke_count(),
            vector.raw().len()
        );
        for (stroke_index, stroke) in vector.strokes().enumerate() {
            let layout = stroke.layout();
            println!(
                "  stroke {stroke_index}: {:?} {:?}, {} point(s), layout {}/{}/{}/{}, supported={}",
                stroke.kind(),
                stroke.curve_kind(),
                stroke.point_count(),
                layout.header_size(),
                layout.point_section_size(),
                layout.point_size(),
                layout.default_point_size(),
                stroke.is_semantically_supported()
            );
            if let Some(style) = stroke.style() {
                println!(
                    "    style: color #{:02x}{:02x}{:02x}, opacity {}, brush {}, radius {:?}",
                    style.main_color().red_8bit(),
                    style.main_color().green_8bit(),
                    style.main_color().blue_8bit(),
                    style.opacity(),
                    style.brush_id(),
                    style.brush_radius()
                );
            }
            for (point_index, point) in stroke.points().enumerate() {
                let controls = point
                    .control_points()
                    .map(|position| (position.x(), position.y()))
                    .collect::<Vec<_>>();
                println!(
                    "    point {point_index}: position={:?}, width_factor={:?}, opacity_factor={:?}, controls={controls:?}",
                    point
                        .position()
                        .map(|position| (position.x(), position.y())),
                    point.width_factor(),
                    point.opacity_factor()
                );
            }
        }
    }
    Ok(())
}
