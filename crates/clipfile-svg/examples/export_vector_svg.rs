//! Exports one CLIP vector layer through the `clipfile-svg` adapter.

use std::{
    env,
    fs::{File, OpenOptions},
    io::{BufWriter, Write as _},
};

use clipfile::{ClipFile, Document, Limits};
use clipfile_svg::{SvgCanvas, SvgOptions, SvgWriter};

const USAGE: &str = "usage: cargo run -p clipfile-svg --example export_vector_svg -- \
     <input.clip> <new-output.svg> <layer-id>";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(USAGE)?;
    let output_path = arguments.next().ok_or(USAGE)?;
    let layer_id = arguments
        .next()
        .ok_or(USAGE)?
        .to_string_lossy()
        .parse::<i64>()?;
    if arguments.next().is_some() {
        return Err(USAGE.into());
    }

    let limits = Limits::default();
    let mut clip = ClipFile::open(File::open(input)?)?;
    let database = clip.open_database()?;
    let document = Document::load(&database, limits)?;
    let layer = document
        .layer(layer_id)
        .ok_or_else(|| format!("layer {layer_id} does not exist"))?;
    let canvas = document
        .canvas(layer.canvas_id())
        .ok_or_else(|| format!("layer {layer_id} does not resolve to a canvas"))?;
    let (canvas_id, width, height) = (canvas.id(), canvas.width(), canvas.height());
    if canvas.unit() != 0 {
        eprintln!(
            "warning: canvas {canvas_id}: opaque canvas unit {} is represented as SVG user units",
            canvas.unit()
        );
    }
    let canvas = SvgCanvas::new(width, height)?;

    let raw_layer_opacity = layer.opacity_fraction();
    let layer_opacity = raw_layer_opacity.clamp(0.0, 1.0);
    if layer_opacity != raw_layer_opacity {
        eprintln!(
            "warning: layer {layer_id}: opacity {raw_layer_opacity} was clamped to {layer_opacity}"
        );
    }

    let sources = database.vector_data_sources(layer_id, limits)?;
    if sources.is_empty() {
        return Err(format!("layer {layer_id} has no vector data").into());
    }
    if let Some(source) = sources
        .iter()
        .find(|source| source.canvas_id() != canvas_id)
    {
        return Err(format!(
            "vector object {} belongs to canvas {}, expected {canvas_id}",
            source.id(),
            source.canvas_id()
        )
        .into());
    }

    // Never overwrite an existing export. A conversion or I/O error can leave
    // this newly created path as a partial document.
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?;
    let options = SvgOptions::default()
        .with_title(format!("CLIP vector layer {layer_id}"))
        .with_group_opacity(layer_opacity);
    let mut writer = SvgWriter::new(BufWriter::new(output), canvas, options)?;
    for source in &sources {
        let vector = clip.read_vector(&database, source, limits)?;
        writer.write_vector(&vector)?;
    }
    let (mut output, report) = writer.finish()?;
    output.flush()?;

    println!(
        "wrote {} SVG path(s) from {} vector object(s); skipped {} stroke(s)",
        report.paths_written(),
        report.vector_objects(),
        report.strokes_skipped()
    );
    for (kind, count) in report.losses() {
        eprintln!("warning: {count} stroke(s): {kind}");
    }
    Ok(())
}
