//! Template-free raster generation based on the independently tested clip-file
//! Python builder. Unknown object kinds are not synthesized.

use std::{
    fs::OpenOptions,
    io::{Cursor, Write},
    path::Path,
};

use flate2::{Compression, write::ZlibEncoder};
use image::{
    ImageFormat, RgbaImage,
    imageops::{self, FilterType},
};
use rusqlite::{Connection, MAIN_DB, params};

use crate::{ClipFile, Error, Limits, OffscreenAttributes, PixelFormat, RasterImage, Result};
use crate::{external::csp_block_checksum, raster::RasterEncoder};

const SCALES: [f64; 5] = [100.0, 50.0, 25.0, 12.5, 6.25];
const EXTERNAL_COLUMNS: [(&str, &str); 10] = [
    ("Offscreen", "BlockData"),
    ("VectorObjectList", "VectorData"),
    ("Canvas3DModelBank", "BankData"),
    ("Canvas3DModelLoader", "ModelData"),
    ("Track", "TrackActionMixer"),
    ("Track", "TrackActionMixer2"),
    ("CanvasItemBinary", "ItemBinaryData"),
    ("Manager3DOd", "SceneData"),
    ("ModelData3D", "Layer3DModelData"),
    ("TimeLapseBlob", "BlobData"),
];

/// Metadata for a new normal-blend RGBA raster layer.
#[derive(Clone, Debug)]
pub struct RasterLayerOptions {
    /// Layer display name; duplicate names are allowed and have independent IDs.
    pub name: String,
    /// Position of the input image relative to the canvas.
    pub offset: (i32, i32),
    /// CSP opacity in `0..=256`.
    pub opacity: u16,
    /// An existing folder ID returned by `add_folder`; `None` means root.
    pub parent: Option<i64>,
}

impl RasterLayerOptions {
    /// Creates root-level, full-opacity layer options at the origin.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            offset: (0, 0),
            opacity: 256,
            parent: None,
        }
    }
}

struct Node {
    name: String,
    parent: i64,
    opacity: u16,
    offset: (i32, i32),
    image: Option<RgbaImage>,
}

/// Creates a raster-only `.clip` without opening an existing document.
///
/// Children are stacked in insertion order, bottom to top. Supports positioned
/// RGBA layers, nested normal-blend folders, opacity, thumbnails, five mipmap
/// metadata levels and a PNG preview. The initial schema/defaults are derived
/// from clip-file's Python builder, rather than an official CSP specification.
///
/// ```no_run
/// use clipfile::{Limits, PixelFormat, RasterDocumentBuilder, RasterImage, RasterLayerOptions};
/// let mut builder = RasterDocumentBuilder::new(32, 24, Limits::default())?;
/// let image = RasterImage::from_pixels(32, 24, PixelFormat::Rgba8, vec![255; 32 * 24 * 4])?;
/// builder.add_layer(image, RasterLayerOptions::new("Paint"))?;
/// builder.write_to_path("new-drawing.clip")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct RasterDocumentBuilder {
    width: u32,
    height: u32,
    limits: Limits,
    nodes: Vec<Node>,
    preview: Option<RgbaImage>,
}

impl RasterDocumentBuilder {
    /// Creates a transparent canvas, initially without drawable layers.
    pub fn new(width: u32, height: u32, limits: Limits) -> Result<Self> {
        check_image_size(width, height, limits)?;
        check_limit(1, limits.max_layers(), "new document layers")?;
        Ok(Self {
            width,
            height,
            limits,
            nodes: Vec::new(),
            preview: None,
        })
    }

    /// Adds an RGBA8 image, preserving layer opacity separately from its pixels.
    /// Pixels outside the storage region are clipped as in clip-file's builder.
    pub fn add_layer(&mut self, image: RasterImage, options: RasterLayerOptions) -> Result<i64> {
        if image.format() != PixelFormat::Rgba8 {
            return Err(invalid("new raster documents require RGBA8 images"));
        }
        check_image_size(image.width(), image.height(), self.limits)?;
        self.validate_node(&options.name, options.parent, options.opacity)?;
        let storage_width = self
            .width
            .checked_add(options.offset.0.min(0).unsigned_abs())
            .ok_or(Error::OffsetOverflow)?;
        let storage_height = self
            .height
            .checked_add(options.offset.1.min(0).unsigned_abs())
            .ok_or(Error::OffsetOverflow)?;
        check_image_size(storage_width, storage_height, self.limits)?;
        let mut storage = RgbaImage::new(storage_width, storage_height);
        let source = RgbaImage::from_raw(image.width(), image.height(), image.into_pixels())
            .ok_or_else(|| invalid("invalid RGBA pixel buffer"))?;
        imageops::replace(
            &mut storage,
            &source,
            i64::from(options.offset.0.max(0)),
            i64::from(options.offset.1.max(0)),
        );
        self.nodes.push(Node {
            name: options.name,
            parent: options.parent.unwrap_or(2),
            opacity: options.opacity,
            offset: options.offset,
            image: Some(storage),
        });
        Ok(self.nodes.len() as i64 + 2)
    }

    /// Adds a normal-blend folder below an existing parent (or the root).
    pub fn add_folder(
        &mut self,
        name: impl Into<String>,
        parent: Option<i64>,
        opacity: u16,
    ) -> Result<i64> {
        let name = name.into();
        self.validate_node(&name, parent, opacity)?;
        self.nodes.push(Node {
            name,
            parent: parent.unwrap_or(2),
            opacity,
            offset: (0, 0),
            image: None,
        });
        Ok(self.nodes.len() as i64 + 2)
    }

    /// Overrides the generated PNG preview. Its dimensions may differ from the
    /// canvas, matching clip-file's explicit preview input behavior.
    pub fn set_preview(&mut self, image: RasterImage) -> Result<()> {
        if image.format() != PixelFormat::Rgba8 {
            return Err(invalid("preview must be RGBA8"));
        }
        check_image_size(image.width(), image.height(), self.limits)?;
        self.preview = Some(
            RgbaImage::from_raw(image.width(), image.height(), image.into_pixels())
                .ok_or_else(|| invalid("invalid preview pixels"))?,
        );
        Ok(())
    }

    fn validate_node(&self, name: &str, parent: Option<i64>, opacity: u16) -> Result<()> {
        check_limit(
            self.nodes.len() as u64 + 2,
            self.limits.max_layers(),
            "new document layers",
        )?;
        check_limit(
            name.len() as u64,
            self.limits.max_text_bytes(),
            "new layer name bytes",
        )?;
        if opacity > 256 {
            return Err(invalid("layer opacity must be in 0..=256"));
        }
        let mut current = parent.unwrap_or(2);
        let mut depth = 1;
        while current != 2 {
            let index = usize::try_from(current.checked_sub(3).ok_or(Error::OffsetOverflow)?)
                .map_err(|_| invalid("missing parent folder"))?;
            let node = self
                .nodes
                .get(index)
                .ok_or_else(|| invalid("missing parent folder"))?;
            if node.image.is_some() {
                return Err(invalid("parent must be a folder"));
            }
            depth += 1;
            current = node.parent;
        }
        check_limit(
            depth,
            self.limits.max_layer_tree_depth(),
            "new layer tree depth",
        )
    }

    /// Serializes and validates the complete container, SQLite index and layer
    /// tree before returning its bytes. No template file or Python is needed.
    pub fn build(&self) -> Result<Vec<u8>> {
        // Check fixed-size cache allocations before resizing or allocating them.
        check_image_size(512, 512, self.limits)?;
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(include_str!("builder_schema.sql"))?;
        let mut comp_info = Vec::new();
        push_u32(&mut comp_info, 8);
        push_u32(
            &mut comp_info,
            self.nodes
                .iter()
                .filter(|node| node.image.is_some())
                .count() as u32,
        );
        for (index, node) in self.nodes.iter().enumerate() {
            if node.image.is_some() {
                push_u32(&mut comp_info, 32);
                push_u32(&mut comp_info, 16);
                comp_info.extend_from_slice(&uuid(index as u64 + 3));
                push_u32(&mut comp_info, 0x661);
                push_u32(&mut comp_info, 0);
            }
        }
        connection.execute(
            "INSERT INTO Project VALUES (1, '1.1.0', ?1, 2, 1, 2, 3, 0)",
            params![
                (self
                    .nodes
                    .iter()
                    .filter(|node| node.image.is_some())
                    .count()
                    + 2)
                .to_string()
            ],
        )?;
        connection.execute_batch("INSERT INTO CanvasItemBank VALUES (1,1,0,0);
            INSERT INTO AnimationCutBank (_PW_ID,MainId,FirstTimeLine,FirstScenario,CurrentIndex,Enable) VALUES (1,2,0,0,0,0);
            INSERT INTO LayerCompManager VALUES (1,3,0,1,-1);")?;
        connection.execute(
            "INSERT INTO LayerComp VALUES (1,1,3,0,'',?1,?2)",
            params![uuid(1), comp_info],
        )?;
        let current = self
            .nodes
            .iter()
            .rposition(|node| node.image.is_some())
            .map_or(2, |index| index as i64 + 3);
        connection.execute("INSERT INTO Canvas (_PW_ID,MainId,CanvasUnit,CanvasWidth,CanvasHeight,CanvasResolution,CanvasChannelBytes,CanvasDefaultChannelOrder,CanvasRootFolder,CanvasCurrentLayer,CanvasDoSimulateColor,CanvasDefaultColorTypeIndex,CanvasDefaultColorBlackChecked,CanvasDefaultColorWhiteChecked,CanvasDefaultToneLine,CanvasDoublePage,CanvasRenderMipmapForceSaved,CanvasWorkTime,Canvas3DModelDataLoaderIndex) VALUES (1,2,0,?1,?2,72,1,33,2,?3,0,0,1,1,60,0,0,0,0)", params![self.width, self.height, current])?;
        for (table, column) in EXTERNAL_COLUMNS {
            connection.execute(
                "INSERT INTO ExternalTableAndColumnName VALUES (?1,?2)",
                params![table, column],
            )?;
        }
        let composite = self.compose_folder(2)?;
        let preview = self.preview.as_ref().unwrap_or(&composite);
        let mut png = Cursor::new(Vec::new());
        preview
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|error| invalid(error.to_string()))?;
        let png = png.into_inner();
        check_limit(
            png.len() as u64,
            self.limits.max_preview_bytes(),
            "new canvas preview bytes",
        )?;
        connection.execute(
            "INSERT INTO CanvasPreview VALUES (1,1,2,1,?1,?2,?3)",
            params![preview.width(), preview.height(), png],
        )?;
        let root = Node {
            name: String::new(),
            parent: 0,
            opacity: 256,
            offset: (0, 0),
            image: None,
        };
        let mut external = Vec::new();
        for (index, node) in std::iter::once(&root).chain(self.nodes.iter()).enumerate() {
            let id = index as i64 + 2;
            let folder = node.image.is_none();
            let first = self
                .nodes
                .iter()
                .position(|child| child.parent == id)
                .map_or(0, |index| index as i64 + 3);
            let next = if index == 0 {
                0
            } else {
                self.nodes
                    .iter()
                    .enumerate()
                    .skip(index)
                    .find(|(_, other)| other.parent == node.parent)
                    .map_or(0, |(index, _)| index as i64 + 3)
            };
            let raw_uuid = uuid(id as u64);
            let hex_uuid = raw_uuid
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            // Keep clip-file's textual UUID grouping; identity is independent
            // of display names so same-named layers never alias each other.
            let uuid_text = format!(
                "{}-{}-{}-{}-{}",
                &hex_uuid[..10],
                &hex_uuid[10..14],
                &hex_uuid[14..18],
                &hex_uuid[18..22],
                &hex_uuid[22..]
            );
            connection.execute("INSERT INTO Layer (_PW_ID,MainId,CanvasId,LayerName,LayerType,LayerLock,LayerClip,LayerMasking,LayerOffsetX,LayerOffsetY,LayerRenderOffscrOffsetX,LayerRenderOffscrOffsetY,LayerMaskOffsetX,LayerMaskOffsetY,LayerMaskOffscrOffsetX,LayerMaskOffscrOffsetY,LayerOpacity,LayerComposite,LayerUsePaletteColor,LayerNoticeablePaletteColor,LayerPaletteRed,LayerPaletteGreen,LayerPaletteBlue,LayerFolder,LayerVisibility,LayerSelect,LayerNextIndex,LayerFirstChildIndex,LayerUuid,LayerRenderMipmap,LayerLayerMaskMipmap,LayerRenderThumbnail,LayerLayerMaskThumbnail) VALUES (?1,?2,2,?3,?4,0,0,?5,?6,?7,?8,?9,?8,?9,0,0,?10,0,0,0,0,0,0,?11,1,?12,?13,?14,?15,?2,0,?2,0)", params![index as i64 + 1,id,node.name,if index==0 {256} else {i64::from(!folder)},if folder {32} else {0},node.offset.0,node.offset.1,-i64::from(node.offset.0),-i64::from(node.offset.1),node.opacity,i64::from(folder),i64::from(!folder && id==current),next,first,uuid_text])?;
            let blank;
            let image = if index == 0 {
                &composite
            } else if let Some(image) = &node.image {
                image
            } else {
                blank = RgbaImage::new(self.width, self.height);
                &blank
            };
            let base_info = 10 + index as i64 * 5;
            let base_offscreen = 100 + index as i64 * 6;
            connection.execute(
                "INSERT INTO Mipmap VALUES (?1,?2,2,?2,5,?3)",
                params![index as i64 + 1, id, base_info],
            )?;
            for (level, scale) in SCALES.into_iter().chain(std::iter::once(0.0)).enumerate() {
                let scaled = if level == 0 {
                    image.clone()
                } else if level == 5 {
                    if folder {
                        RgbaImage::new(512, 512)
                    } else {
                        imageops::resize(image, 512, 512, FilterType::Lanczos3)
                    }
                } else {
                    let width = (f64::from(image.width()) * scale / 100.0)
                        .round_ties_even()
                        .max(1.0) as u32;
                    let height = (f64::from(image.height()) * scale / 100.0)
                        .round_ties_even()
                        .max(1.0) as u32;
                    imageops::resize(image, width, height, FilterType::Lanczos3)
                };
                check_image_size(scaled.width(), scaled.height(), self.limits)?;
                let (body, attribute) =
                    encode_image(&scaled, node.offset != (0, 0) || level == 5, self.limits)?;
                let offscreen_id = base_offscreen + level as i64;
                let identifier = format!("extrnlid{:032X}", offscreen_id);
                connection.execute(
                    "INSERT INTO Offscreen VALUES (?1,?2,2,?3,?4,?5)",
                    params![
                        index as i64 * 6 + level as i64 + 1,
                        offscreen_id,
                        id,
                        attribute,
                        identifier.as_bytes()
                    ],
                )?;
                if level < 5 {
                    let info_id = base_info + level as i64;
                    connection.execute(
                        "INSERT INTO MipmapInfo VALUES (?1,?2,2,?3,?4,?5,?6)",
                        params![
                            index as i64 * 5 + level as i64 + 1,
                            info_id,
                            id,
                            scale,
                            offscreen_id,
                            if level == 4 { 0 } else { info_id + 1 }
                        ],
                    )?;
                }
                if (level == 0 && !folder) || level == 5 {
                    check_limit(
                        external.len() as u64 + 4,
                        self.limits.max_top_level_chunks(),
                        "new document chunks",
                    )?;
                    let mut payload = Vec::new();
                    payload.extend_from_slice(&(identifier.len() as u64).to_be_bytes());
                    payload.extend_from_slice(identifier.as_bytes());
                    payload.extend_from_slice(&(body.len() as u64).to_be_bytes());
                    payload.extend_from_slice(&body);
                    external.push((identifier, payload));
                }
            }
            connection.execute("INSERT INTO LayerThumbnail (_PW_ID,MainId,CanvasId,LayerId,ThumbnailDrewMode,ThumbnailFixMode,ThumbnailCanvasWidth,ThumbnailCanvasHeight,ThumbnailUseDrawColor,ThumbnailMainColorRed,ThumbnailMainColorGreen,ThumbnailMainColorBlue,ThumbnailSubColorRed,ThumbnailSubColorGreen,ThumbnailSubColorBlue,ThumbnailOffscreen) VALUES (?1,?2,2,?2,0,-1,?3,?4,0,0,0,0,0,0,0,?5)",params![index as i64+1,id,self.width,self.height,base_offscreen+5])?;
        }
        let mut database_offset = 80_u64;
        for (identifier, payload) in &external {
            connection.execute(
                "INSERT INTO ExternalChunk VALUES (?1,?2)",
                params![
                    identifier,
                    i64::try_from(database_offset).map_err(|_| Error::OffsetOverflow)?
                ],
            )?;
            database_offset = database_offset
                .checked_add(16 + payload.len() as u64)
                .ok_or(Error::OffsetOverflow)?;
        }
        let database = connection.serialize(MAIN_DB)?.to_vec();
        check_limit(
            database.len() as u64,
            self.limits.max_database_size(),
            "new document SQLite bytes",
        )?;
        check_limit(
            16,
            self.limits.max_identifier_size(),
            "new file identifier bytes",
        )?;
        check_limit(
            40,
            self.limits.max_identifier_size(),
            "new external identifier bytes",
        )?;
        let mut output = b"CSFCHUNK".to_vec();
        output.extend_from_slice(&0_u64.to_be_bytes());
        output.extend_from_slice(&24_u64.to_be_bytes());
        let mut header = 256_u64.to_be_bytes().to_vec();
        header.extend_from_slice(&database_offset.to_be_bytes());
        header.extend_from_slice(&16_u64.to_be_bytes());
        header.extend_from_slice(&uuid(0));
        chunk(&mut output, b"CHNKHead", &header);
        for (_, payload) in external {
            chunk(&mut output, b"CHNKExta", &payload);
        }
        chunk(&mut output, b"CHNKSQLi", &database);
        chunk(&mut output, b"CHNKFoot", &[]);
        let size = output.len() as u64;
        output[8..16].copy_from_slice(&size.to_be_bytes());
        let mut clip = ClipFile::open_with_limits(Cursor::new(&output), self.limits)?;
        clip.validate()?;
        let database = clip.open_database()?;
        database.quick_check()?;
        clip.validate_external_index(&database)?;
        crate::Document::load(&database, self.limits)?;
        Ok(output)
    }

    /// Writes a validated new file, refusing to overwrite an existing path.
    /// Removes an incomplete file if writing or syncing fails.
    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = self.build()?;
        let path = path.as_ref();
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let result = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result.map_err(Error::from)
    }

    fn compose_folder(&self, parent: i64) -> Result<RgbaImage> {
        let mut composite = RgbaImage::new(self.width, self.height);
        for (index, node) in self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.parent == parent)
        {
            let image = if let Some(storage) = &node.image {
                let mut projected = RgbaImage::new(self.width, self.height);
                imageops::replace(
                    &mut projected,
                    storage,
                    i64::from(node.offset.0.min(0)),
                    i64::from(node.offset.1.min(0)),
                );
                projected
            } else {
                self.compose_folder(index as i64 + 3)?
            };
            for (dst, src) in composite.pixels_mut().zip(image.pixels()) {
                let sa = u64::from(src[3]) * u64::from(node.opacity) / 256;
                if sa == 0 {
                    continue;
                }
                let da = u64::from(dst[3]);
                let alpha = sa * 255 + da * (255 - sa);
                for channel in 0..3 {
                    dst[channel] = ((u64::from(src[channel]) * sa * 255
                        + u64::from(dst[channel]) * da * (255 - sa)
                        + alpha / 2)
                        / alpha) as u8;
                }
                dst[3] = ((alpha + 127) / 255) as u8;
            }
        }
        Ok(composite)
    }
}

fn uuid(id: u64) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(b"clipbldr");
    bytes[8..].copy_from_slice(&id.to_be_bytes());
    bytes
}

fn check_image_size(width: u32, height: u32, limits: Limits) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(invalid("image dimensions must be positive"));
    }
    check_limit(
        u64::from(width.max(height)),
        u64::from(limits.max_canvas_dimension()),
        "new raster dimension",
    )?;
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|size| size.checked_mul(4))
        .ok_or(Error::OffsetOverflow)?;
    check_limit(bytes, limits.max_raster_bytes(), "new raster bytes")
}

fn check_limit(value: u64, limit: u64, resource: &'static str) -> Result<()> {
    if value > limit {
        return Err(Error::LimitExceeded {
            resource,
            value,
            limit,
        });
    }
    Ok(())
}

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidWrite {
        reason: reason.into(),
    }
}
fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}
fn named(name: &str, values: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_u32(&mut bytes, name.encode_utf16().count() as u32);
    for unit in name.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    for value in values {
        push_u32(&mut bytes, *value);
    }
    bytes
}
fn attributes(width: u32, height: u32, sizes: &[u32]) -> Vec<u8> {
    let parameter = named(
        "Parameter",
        &[
            width,
            height,
            width.div_ceil(256),
            height.div_ceil(256),
            33,
            1,
            4,
            5,
            65536,
            4,
            1024,
            1,
            256,
            65536,
            256,
            256,
            8,
            8,
            0,
            0,
        ],
    );
    let initial = named("InitColor", &[20, 0, 0, 0, 4]);
    let mut values = vec![12, sizes.len() as u32, 4];
    values.extend_from_slice(sizes);
    let blocks = named("BlockSize", &values);
    let mut bytes = Vec::new();
    for value in [
        16,
        parameter.len() as u32,
        initial.len() as u32,
        blocks.len() as u32,
    ] {
        push_u32(&mut bytes, value);
    }
    bytes.extend(parameter);
    bytes.extend(initial);
    bytes.extend(blocks);
    bytes
}

fn encode_image(
    image: &RgbaImage,
    allow_empty: bool,
    limits: Limits,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let count = image
        .width()
        .div_ceil(256)
        .checked_mul(image.height().div_ceil(256))
        .ok_or(Error::OffsetOverflow)?;
    check_limit(
        u64::from(count),
        limits.max_blocks_per_external(),
        "new raster blocks",
    )?;
    let initial = attributes(image.width(), image.height(), &vec![0; count as usize]);
    let parsed = OffscreenAttributes::parse(&initial)?;
    let encoder = RasterEncoder::new(
        &parsed,
        PixelFormat::Rgba8,
        image.as_raw(),
        limits.max_canvas_dimension(),
        limits.max_raster_bytes(),
        limits.max_decompressed_block_size(),
    )?;
    let mut body = Vec::new();
    let mut sizes = Vec::new();
    let mut checksums = vec![12, count, 4];
    for index in 0..encoder.tile_count() {
        let mut raw = encoder.encode_tile(index, None)?;
        // Match clip-file's transparent-pixel convention, including padded edges.
        for pixel in 0..65536 {
            if raw[pixel] == 0 {
                raw[65536 + pixel * 4..65536 + pixel * 4 + 4].fill(255);
            }
        }
        let empty = allow_empty && raw[..65536].iter().all(|alpha| *alpha == 0);
        let compressed = if empty {
            Vec::new()
        } else {
            let mut zlib = ZlibEncoder::new(Vec::new(), Compression::fast());
            zlib.write_all(&raw)?;
            zlib.finish()?
        };
        let mut record = named(
            "BlockDataBeginChunk",
            &[index, 327680, 256, 256, u32::from(!empty)],
        );
        if !empty {
            let size = u32::try_from(compressed.len()).map_err(|_| Error::OffsetOverflow)?;
            push_u32(
                &mut record,
                size.checked_add(4).ok_or(Error::OffsetOverflow)?,
            );
            record.extend_from_slice(&size.to_le_bytes());
            record.extend_from_slice(&compressed);
        }
        record.extend(named("BlockDataEndChunk", &[]));
        let size = u32::try_from(record.len() + 4).map_err(|_| Error::OffsetOverflow)?;
        check_limit(
            body.len() as u64 + u64::from(size),
            limits.max_write_external_body_size(),
            "new raster external bytes",
        )?;
        push_u32(&mut body, size);
        body.extend(record);
        sizes.push(size);
        checksums.push(if empty {
            0
        } else {
            csp_block_checksum(&compressed)?
        });
    }
    let mut statuses = vec![12, count, 4];
    statuses.resize(count as usize + 3, 1);
    body.extend(named("BlockStatus", &statuses));
    body.extend(named("BlockCheckSum", &checksums));
    check_limit(
        body.len() as u64,
        limits.max_write_external_body_size(),
        "new raster external bytes",
    )?;
    Ok((body, attributes(image.width(), image.height(), &sizes)))
}

fn chunk(output: &mut Vec<u8>, name: &[u8; 8], payload: &[u8]) {
    output.extend_from_slice(name);
    output.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    output.extend_from_slice(payload);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExternalBody;

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RasterImage {
        RasterImage::from_pixels(
            width,
            height,
            PixelFormat::Rgba8,
            color.repeat(width as usize * height as usize),
        )
        .unwrap()
    }

    #[test]
    fn generates_positioned_multitile_rgba_and_compatible_checksums() {
        let mut builder = RasterDocumentBuilder::new(260, 256, Limits::default()).unwrap();
        let pixels: Vec<u8> = (0..257)
            .flat_map(|y| {
                (0..259).flat_map(move |x| [x as u8, y as u8, 73, if x == 0 { 0 } else { 128 }])
            })
            .collect();
        let image = RasterImage::from_pixels(259, 257, PixelFormat::Rgba8, pixels).unwrap();
        let mut options = RasterLayerOptions::new("positioned");
        options.offset = (3, -2);
        options.opacity = 77;
        let id = builder.add_layer(image, options).unwrap();
        let mut clip = ClipFile::open(Cursor::new(builder.build().unwrap())).unwrap();
        let database = clip.open_database().unwrap();
        let source = database.layer_raster_source(id).unwrap().unwrap();
        let raster = clip.decode_raster(&database, &source).unwrap();
        assert_eq!((raster.width(), raster.height()), (260, 258));
        assert_eq!(&raster.pixels()[4 * 264..4 * 265], &[1, 1, 73, 128]);
        assert_eq!(&raster.pixels()[..4], &[255, 255, 255, 0]);
        let row=database.connection().query_row("SELECT LayerOffsetX,LayerOffsetY,LayerOpacity,LayerRenderOffscrOffsetX,LayerRenderOffscrOffsetY FROM Layer WHERE MainId=?1",params![id],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?,row.get::<_,i64>(4)?))).unwrap();
        assert_eq!(row, (3, -2, 77, -3, 2));
        let chunks = clip.chunks().collect::<Result<Vec<_>>>().unwrap();
        for chunk in chunks
            .iter()
            .filter(|chunk| chunk.kind() == crate::ChunkKind::External)
        {
            let object = clip.inspect_external_chunk(chunk).unwrap();
            assert!(matches!(object.body(), ExternalBody::BlockData));
            let blocks = clip.read_block_data(&object).unwrap();
            for block in blocks.blocks() {
                if let Some(payload) = block.payload() {
                    let compressed = clip
                        .read_block_payload(
                            payload,
                            Limits::default().max_write_external_body_size(),
                        )
                        .unwrap();
                    assert_eq!(block.checksum(), csp_block_checksum(&compressed).unwrap());
                } else {
                    assert_eq!(block.checksum(), 0);
                }
            }
        }
    }

    #[test]
    fn preserves_nested_folders_sibling_order_and_duplicate_layer_names() {
        let mut builder = RasterDocumentBuilder::new(8, 6, Limits::default()).unwrap();
        let outer = builder.add_folder("outer", None, 128).unwrap();
        let inner = builder.add_folder("inner", Some(outer), 154).unwrap();
        let mut options = RasterLayerOptions::new("same name");
        options.parent = Some(inner);
        let first = builder
            .add_layer(solid(8, 6, [12, 34, 56, 255]), options)
            .unwrap();
        let second = builder
            .add_layer(
                solid(4, 3, [80, 90, 100, 255]),
                RasterLayerOptions::new("same name"),
            )
            .unwrap();
        let mut clip = ClipFile::open(Cursor::new(builder.build().unwrap())).unwrap();
        let document = clip.read_document().unwrap();
        assert_eq!(document.layer_tree(2).unwrap().reachable_layer_count(), 5);
        assert_eq!(document.layer(2).unwrap().first_child_id(), Some(outer));
        assert_eq!(document.layer(outer).unwrap().first_child_id(), Some(inner));
        assert_eq!(document.layer(inner).unwrap().first_child_id(), Some(first));
        assert_eq!(
            document.layer(outer).unwrap().next_sibling_id(),
            Some(second)
        );
        let database = clip.open_database().unwrap();
        for (id, color) in [(first, [12, 34, 56, 255]), (second, [80, 90, 100, 255])] {
            let source = database.layer_raster_source(id).unwrap().unwrap();
            assert_eq!(
                &clip.decode_raster(&database, &source).unwrap().pixels()[..4],
                &color
            );
        }
        let unique: i64 = database
            .connection()
            .query_row("SELECT count(DISTINCT LayerUuid) FROM Layer", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(unique, 5);
    }

    #[test]
    fn supports_empty_documents_and_explicit_preview() {
        let mut builder = RasterDocumentBuilder::new(7, 5, Limits::default()).unwrap();
        builder.set_preview(solid(3, 2, [10, 20, 30, 255])).unwrap();
        let mut clip = ClipFile::open(Cursor::new(builder.build().unwrap())).unwrap();
        assert_eq!(clip.read_document().unwrap().layers().len(), 1);
        let database = clip.open_database().unwrap();
        let preview = database
            .canvas_preview(2, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!((preview.width(), preview.height()), (3, 2));
        assert_eq!(
            image::load_from_memory(preview.data())
                .unwrap()
                .into_rgba8()
                .get_pixel(2, 1)
                .0,
            [10, 20, 30, 255]
        );
    }

    #[test]
    fn generated_documents_can_be_edited_and_rewritten() {
        let mut builder = RasterDocumentBuilder::new(8, 6, Limits::default()).unwrap();
        let id = builder
            .add_layer(
                solid(8, 6, [10, 20, 30, 255]),
                RasterLayerOptions::new("editable"),
            )
            .unwrap();
        let mut clip = ClipFile::open(Cursor::new(builder.build().unwrap())).unwrap();
        let mut writer = clip.writer().unwrap();
        writer
            .replace_layer_raster(id, solid(8, 6, [100, 110, 120, 255]))
            .unwrap();
        let mut output = Vec::new();
        writer.write_to(&mut output).unwrap();
        let mut rewritten = ClipFile::open(Cursor::new(output)).unwrap();
        let database = rewritten.open_database().unwrap();
        let source = database.layer_raster_source(id).unwrap().unwrap();
        assert_eq!(
            rewritten
                .decode_raster(&database, &source)
                .unwrap()
                .pixels(),
            [100, 110, 120, 255].repeat(48)
        );
    }

    #[test]
    fn preview_applies_folder_opacity_to_the_composited_group() {
        let mut builder = RasterDocumentBuilder::new(1, 1, Limits::default()).unwrap();
        let folder = builder.add_folder("group", None, 128).unwrap();
        for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            let mut options = RasterLayerOptions::new("paint");
            options.parent = Some(folder);
            builder.add_layer(solid(1, 1, color), options).unwrap();
        }
        let mut clip = ClipFile::open(Cursor::new(builder.build().unwrap())).unwrap();
        let database = clip.open_database().unwrap();
        let preview = database
            .canvas_preview(2, Limits::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            image::load_from_memory(preview.data())
                .unwrap()
                .into_rgba8()
                .get_pixel(0, 0)
                .0,
            [0, 0, 255, 127]
        );
    }

    #[test]
    fn path_output_refuses_to_overwrite_existing_files() {
        let name = format!(
            "clipfile-builder-{}-{}.clip",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(name);
        let builder = RasterDocumentBuilder::new(3, 3, Limits::default()).unwrap();
        builder.write_to_path(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(builder.write_to_path(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_invalid_inputs_without_adding_nodes() {
        assert!(RasterDocumentBuilder::new(0, 5, Limits::default()).is_err());
        assert!(matches!(
            RasterDocumentBuilder::new(100, 100, Limits::default().with_max_raster_bytes(4)),
            Err(Error::LimitExceeded { .. })
        ));
        let mut builder = RasterDocumentBuilder::new(3, 3, Limits::default()).unwrap();
        for options in [
            RasterLayerOptions {
                name: "invalid".into(),
                offset: (0, 0),
                opacity: 257,
                parent: None,
            },
            RasterLayerOptions {
                name: "invalid".into(),
                offset: (0, 0),
                opacity: 256,
                parent: Some(999),
            },
            RasterLayerOptions {
                name: "invalid".into(),
                offset: (i32::MIN, 0),
                opacity: 256,
                parent: None,
            },
        ] {
            assert!(
                builder
                    .add_layer(solid(1, 1, [0, 0, 0, 255]), options)
                    .is_err()
            );
        }
        assert!(builder.nodes.is_empty());
        let leaf = builder
            .add_layer(solid(1, 1, [0, 0, 0, 255]), RasterLayerOptions::new("leaf"))
            .unwrap();
        assert!(builder.add_folder("invalid", Some(leaf), 256).is_err());
        let mut limited =
            RasterDocumentBuilder::new(3, 3, Limits::default().with_max_layers(1)).unwrap();
        assert!(matches!(
            limited.add_folder("too many", None, 256),
            Err(Error::LimitExceeded { .. })
        ));
        let mut shallow =
            RasterDocumentBuilder::new(3, 3, Limits::default().with_max_layer_tree_depth(1))
                .unwrap();
        let folder = shallow.add_folder("folder", None, 256).unwrap();
        assert!(matches!(
            shallow.add_folder("too deep", Some(folder), 256),
            Err(Error::LimitExceeded { .. })
        ));
        for limits in [
            Limits::default().with_max_preview_bytes(1),
            Limits::default().with_max_database_size(1),
            Limits::default().with_max_blocks_per_external(0),
            Limits::default().with_max_write_external_body_size(1),
        ] {
            assert!(matches!(
                RasterDocumentBuilder::new(3, 3, limits).unwrap().build(),
                Err(Error::LimitExceeded { .. })
            ));
        }
    }
}
