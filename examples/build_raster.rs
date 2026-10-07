//! Creates raster documents from PNG files, using clip-file-compatible metadata.
use clipfile::{Limits, RasterDocumentBuilder, RasterImage, RasterLayerOptions};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
};

const USAGE: &str = "usage: cargo run --features build --example build_raster -- <input.png|directory> <new-output.clip> [--canvas-size auto|WIDTHxHEIGHT] [--metadata metadata.json] [--preview preview.png] [--order name|natural] [--pattern '*.png']";
type AnyError = Box<dyn std::error::Error>;

struct Input {
    path: PathBuf,
    offset: (i32, i32),
    opacity: u16,
    folder: Option<i64>,
}

fn pair(value: &Value) -> Result<(i32, i32), AnyError> {
    let values = value.as_array().ok_or("expected a two-integer array")?;
    if values.len() != 2 {
        return Err("expected a two-integer array".into());
    }
    Ok((
        i32::try_from(values[0].as_i64().ok_or("expected an integer")?)?,
        i32::try_from(values[1].as_i64().ok_or("expected an integer")?)?,
    ))
}
fn opacity(value: &Value) -> Result<u16, AnyError> {
    let value = value.get("source_opacity").or_else(|| value.get("opacity"));
    let value = value
        .map(|value| value.as_u64().ok_or("opacity must be an integer"))
        .transpose()?
        .unwrap_or(256);
    if value > 256 {
        return Err("opacity must be in 0..=256".into());
    }
    Ok(value as u16)
}
fn folder_chain(id: i64, folders: &BTreeMap<i64, Value>) -> Result<Vec<i64>, AnyError> {
    let mut chain = Vec::new();
    let mut current = Some(id);
    while let Some(id) = current {
        if chain.contains(&id) {
            return Err("folder parent cycle".into());
        }
        chain.push(id);
        let folder = folders.get(&id).ok_or("missing folder")?;
        current = folder
            .get("parent_id")
            .filter(|value| !value.is_null())
            .map(|value| value.as_i64().ok_or("invalid parent id"))
            .transpose()?;
    }
    chain.reverse();
    Ok(chain)
}
fn slug(folder: &Value) -> String {
    if let Some(slug) = folder.get("slug").and_then(Value::as_str) {
        return slug.to_owned();
    }
    let name = folder
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("folder");
    let mut slug = String::new();
    for character in name.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let slug = slug.trim_matches('_');
    if slug.is_empty() {
        "folder".to_owned()
    } else {
        slug.to_owned()
    }
}
fn load(path: &Path) -> Result<RasterImage, AnyError> {
    let mut reader = image::ImageReader::open(path)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(Limits::default().max_raster_bytes());
    limits.max_image_width = Some(Limits::default().max_canvas_dimension());
    limits.max_image_height = Some(Limits::default().max_canvas_dimension());
    reader.limits(limits);
    Ok(RasterImage::try_from_dynamic_image(
        image::DynamicImage::ImageRgba8(reader.decode()?.into_rgba8()),
    )?)
}
fn natural_key(value: &str) -> Vec<(u8, String)> {
    let mut parts = Vec::new();
    let mut digits = false;
    let mut part = String::new();
    for character in value.to_lowercase().chars() {
        if !part.is_empty() && character.is_ascii_digit() != digits {
            parts.push(sort_part(&part, digits));
            part.clear();
        }
        digits = character.is_ascii_digit();
        part.push(character);
    }
    if !part.is_empty() {
        parts.push(sort_part(&part, digits));
    }
    parts
}
fn sort_part(part: &str, digits: bool) -> (u8, String) {
    if digits {
        let trimmed = part.trim_start_matches('0');
        (0, format!("{:020}:{trimmed}", trimmed.len()))
    } else {
        (1, part.to_owned())
    }
}

fn main() -> Result<(), AnyError> {
    let mut args = env::args_os().skip(1);
    let source = PathBuf::from(args.next().ok_or(USAGE)?);
    let output = PathBuf::from(args.next().ok_or(USAGE)?);
    let mut canvas = None;
    let mut metadata = None;
    let mut preview = None;
    let mut order = "name".to_owned();
    let mut pattern = "*.png".to_owned();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match flag.to_str().ok_or(USAGE)? {
            "--canvas-size" => {
                let text = value.to_str().ok_or(USAGE)?;
                if text != "auto" {
                    let (w, h) = text.split_once('x').ok_or(USAGE)?;
                    canvas = Some((w.parse::<u32>()?, h.parse::<u32>()?));
                }
            }
            "--metadata" => metadata = Some(PathBuf::from(value)),
            "--preview" => preview = Some(PathBuf::from(value)),
            "--order" => order = value.to_str().ok_or(USAGE)?.to_owned(),
            "--pattern" => pattern = value.to_str().ok_or(USAGE)?.to_owned(),
            _ => return Err(USAGE.into()),
        }
    }
    if order != "name" && order != "natural" {
        return Err("order must be name or natural".into());
    }
    let mut folders = BTreeMap::new();
    let mut inputs = Vec::new();
    let metadata = metadata.or_else(|| {
        source
            .is_dir()
            .then(|| source.join("metadata.json"))
            .filter(|path| path.is_file())
    });
    if source.is_file() {
        inputs.push(Input {
            path: source.clone(),
            offset: (0, 0),
            opacity: 256,
            folder: None,
        });
    } else if let Some(path) = metadata {
        let metadata: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        let upper = metadata.get("CANVAS_SIZE").map(pair).transpose()?;
        let lower = metadata.get("canvas_size").map(pair).transpose()?;
        if upper.is_some() && lower.is_some() && upper != lower {
            return Err("canvas size metadata keys disagree".into());
        }
        if canvas.is_none() {
            let (w, h) = upper.or(lower).unwrap_or((1024, 768));
            canvas = Some((u32::try_from(w)?, u32::try_from(h)?));
        }
        let entries: Vec<(String, Value)> = if metadata.get("version") == Some(&Value::from(2)) {
            if let Some(raw) = metadata.get("folders") {
                for folder in raw.as_array().ok_or("folders must be an array")? {
                    let id = folder
                        .get("id")
                        .and_then(Value::as_i64)
                        .filter(|id| *id > 0)
                        .ok_or("invalid folder id")?;
                    if folders.insert(id, folder.clone()).is_some() {
                        return Err("duplicate folder id".into());
                    }
                }
            }
            metadata
                .get("layers")
                .and_then(Value::as_array)
                .ok_or("missing layers array")?
                .iter()
                .map(|value| {
                    Ok((
                        value
                            .get("file")
                            .and_then(Value::as_str)
                            .ok_or("missing layer file")?
                            .to_owned(),
                        value.clone(),
                    ))
                })
                .collect::<Result<_, AnyError>>()?
        } else {
            if metadata.get("version").is_some() {
                return Err("unsupported metadata version".into());
            }
            metadata
                .as_object()
                .ok_or("metadata must be an object")?
                .iter()
                .filter(|(key, _)| key.as_str() != "CANVAS_SIZE" && key.as_str() != "canvas_size")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        };
        for id in folders.keys() {
            folder_chain(*id, &folders)?;
            opacity(&folders[id])?;
        }
        for (file, info) in entries {
            let folder = info
                .get("folder_id")
                .filter(|value| !value.is_null())
                .map(|value| value.as_i64().ok_or("invalid folder id"))
                .transpose()?;
            let mut path = source.clone();
            if let Some(id) = folder {
                for id in folder_chain(id, &folders)? {
                    path.push(slug(&folders[&id]));
                }
            }
            path.push(file);
            if let Some(size) = info.get("size") {
                let expected = pair(size)?;
                let actual = image::image_dimensions(&path)?;
                if (i64::from(expected.0), i64::from(expected.1))
                    != (i64::from(actual.0), i64::from(actual.1))
                {
                    return Err("metadata image size mismatch".into());
                }
            }
            inputs.push(Input {
                path,
                offset: pair(info.get("offset").ok_or("missing offset")?)?,
                opacity: opacity(&info)?,
                folder,
            });
        }
    } else {
        let mut paths = glob::glob(source.join(&pattern).to_str().ok_or("non-UTF8 glob path")?)?
            .collect::<Result<Vec<_>, _>>()?;
        paths.retain(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
                && (pattern != "*.png"
                    || !path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase()
                        .starts_with("combine"))
        });
        paths.sort_by_key(|path| {
            if order == "natural" {
                natural_key(&path.file_name().unwrap_or_default().to_string_lossy())
            } else {
                vec![(
                    1,
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase(),
                )]
            }
        });
        let mut preferred = Vec::new();
        for name in [
            "layer_tiles.png",
            "layer_grid.png",
            "layer_labels.png",
            "layer_markers.png",
        ] {
            if let Some(index) = paths
                .iter()
                .position(|path| path.file_name().is_some_and(|file| file == name))
            {
                preferred.push(paths.remove(index));
            }
        }
        preferred.extend(paths);
        inputs = preferred
            .into_iter()
            .map(|path| Input {
                path,
                offset: (0, 0),
                opacity: 256,
                folder: None,
            })
            .collect();
    }
    if inputs.is_empty() {
        return Err("no PNG inputs".into());
    }
    let images = inputs
        .iter()
        .map(|input| load(&input.path))
        .collect::<Result<Vec<_>, _>>()?;
    let (width, height) = canvas.unwrap_or((
        images.iter().map(RasterImage::width).max().unwrap(),
        images.iter().map(RasterImage::height).max().unwrap(),
    ));
    let mut builder = RasterDocumentBuilder::new(width, height, Limits::default())?;
    let mut folder_ids = BTreeMap::new();
    for (input, image) in inputs.iter().zip(images) {
        if let Some(id) = input.folder {
            let mut parent = None;
            for id in folder_chain(id, &folders)? {
                if let std::collections::btree_map::Entry::Vacant(entry) = folder_ids.entry(id) {
                    entry.insert(
                        builder.add_folder(
                            folders[&id]
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("folder"),
                            parent,
                            opacity(&folders[&id])?,
                        )?,
                    );
                }
                parent = Some(folder_ids[&id]);
            }
        }
        let mut options = RasterLayerOptions::new(if source.is_file() {
            "layer_01".to_owned()
        } else {
            input
                .path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        options.offset = input.offset;
        options.opacity = input.opacity;
        options.parent = input.folder.map(|id| folder_ids[&id]);
        builder.add_layer(image, options)?;
    }
    if let Some(preview) = preview {
        builder.set_preview(load(&preview)?)?;
    }
    builder.write_to_path(&output)?;
    println!(
        "wrote {}x{} document with {} raster layers: {}",
        width,
        height,
        inputs.len(),
        output.display()
    );
    Ok(())
}
