//! Exports one CLIP vector layer through the `clipfile-svg` adapter.

use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write as _},
    path::{Path, PathBuf},
};

use clipfile::{ClipFile, Document, Limits};
use clipfile_svg::{SvgCanvas, SvgGroupOptions, SvgOptions, SvgWriter};

const USAGE: &str = "usage: cargo run -p clipfile-svg --example export_vector_svg -- \
     <input.clip> <new-output.svg> <layer-id>";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let input = arguments.next().ok_or(USAGE)?;
    let output_path = PathBuf::from(arguments.next().ok_or(USAGE)?);
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

    // Write beside the destination, then publish the complete file without
    // overwriting a path created by this or another process.
    let (temporary, output) = TemporaryOutput::create(&output_path)?;
    let options = SvgOptions::default().with_title(format!("CLIP vector layer {layer_id}"));
    let mut writer = SvgWriter::new(BufWriter::new(output), canvas, options)?;
    let group = SvgGroupOptions::default()
        .with_opacity(layer_opacity)
        .with_id(format!("clip-layer-{layer_id}"))
        .with_title(format!("CLIP layer {layer_id}"));
    writer.write_group(group, |writer| -> Result<(), Box<dyn std::error::Error>> {
        for source in &sources {
            let vector = clip.read_vector(&database, source, limits)?;
            writer.write_vector(&vector)?;
        }
        Ok(())
    })?;
    let (mut output, report) = writer.finish()?;
    output.flush()?;
    output.get_ref().sync_all()?;
    drop(output);
    temporary.publish()?;

    println!(
        "wrote {} SVG mark(s) ({} path(s), {} dot(s)) from {} vector object(s); skipped {} stroke(s)",
        report.marks_written(),
        report.paths_written(),
        report.dots_written(),
        report.vector_objects(),
        report.strokes_skipped()
    );
    for (kind, count) in report.losses() {
        eprintln!("warning: {count} stroke(s): {kind}");
    }
    for (kind, count) in report.skips() {
        eprintln!("warning: {count} stroke(s) skipped: {kind}");
    }
    Ok(())
}

/// Same-directory temporary file that becomes visible only after the complete
/// SVG has been flushed. `hard_link` provides create-new semantics on Windows,
/// Linux, and macOS; unlike Unix `rename`, it cannot replace a racing target.
struct TemporaryOutput {
    temporary_path: PathBuf,
    final_path: PathBuf,
    published: bool,
}

impl TemporaryOutput {
    fn create(final_path: &Path) -> io::Result<(Self, File)> {
        if final_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("output already exists: {}", final_path.display()),
            ));
        }
        let file_name = final_path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "output path has no file name")
        })?;
        let parent = final_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));

        for attempt in 0_u16..1024 {
            let mut temporary_name = OsString::from(".");
            temporary_name.push(file_name);
            temporary_name.push(format!(
                ".clipfile-svg-{}-{attempt}.tmp",
                std::process::id()
            ));
            let temporary_path = parent.join(temporary_name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary_path)
            {
                Ok(file) => {
                    return Ok((
                        Self {
                            temporary_path,
                            final_path: final_path.to_owned(),
                            published: false,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary output path",
        ))
    }

    fn publish(mut self) -> io::Result<()> {
        fs::hard_link(&self.temporary_path, &self.final_path)?;
        self.published = true;
        if let Err(error) = fs::remove_file(&self.temporary_path) {
            eprintln!(
                "warning: SVG was published, but temporary file {} could not be removed: {error}",
                self.temporary_path.display()
            );
        }
        Ok(())
    }
}

impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> PathBuf {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "clipfile-svg-example-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn publishes_only_complete_new_files() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("layer.svg");
        let (temporary, mut file) = TemporaryOutput::create(&destination).unwrap();
        file.write_all(b"<svg/>").unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
        drop(file);
        temporary.publish().unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"<svg/>");
        assert!(matches!(
            TemporaryOutput::create(&destination),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        fs::remove_file(destination).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn removes_partial_temporary_file_on_error_path() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("layer.svg");
        let (temporary, file) = TemporaryOutput::create(&destination).unwrap();
        let temporary_path = temporary.temporary_path.clone();
        drop(file);
        drop(temporary);
        assert!(!temporary_path.exists());
        assert!(!destination.exists());
        fs::remove_dir(directory).unwrap();
    }
}
