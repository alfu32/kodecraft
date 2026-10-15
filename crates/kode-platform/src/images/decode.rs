use super::{DecodedImage, MAX_DIMENSION, MAX_ENCODED_BYTES, MAX_PIXELS, validate_size};
use crate::{files::open_regular_file, jobs::CancellationToken};
use image::{ImageDecoder, ImageFormat, ImageReader};
use kode_core::{DomainError, Result};
use std::{
    io::{Cursor, Read},
    path::Path,
};

fn invalid(message: impl ToString) -> DomainError {
    DomainError::Parse {
        format: "image",
        message: message.to_string(),
    }
}
fn io(path: &Path, operation: &'static str, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
pub fn load_image(path: &Path, cancel: &CancellationToken) -> Result<DecodedImage> {
    let mut file = open_regular_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|source| io(path, "stat image", source))?;
    if metadata.len() > MAX_ENCODED_BYTES as u64 {
        return Err(invalid("encoded image exceeds 64 MiB"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut chunk = [0; 65536];
    loop {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let read = file
            .read(&mut chunk)
            .map_err(|source| io(path, "read image", source))?;
        if read == 0 {
            break;
        }
        if bytes.len() + read > MAX_ENCODED_BYTES {
            return Err(invalid("encoded image grew beyond 64 MiB"));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let after = file
        .metadata()
        .map_err(|source| io(path, "stat loaded image", source))?;
    if metadata.len() != after.len() || metadata.modified().ok() != after.modified().ok() {
        return Err(DomainError::SaveConflict(path.into()));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let decoded = decode_bytes(&bytes, &extension, cancel)?;
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    Ok(decoded)
}
pub fn decode_bytes(
    bytes: &[u8],
    extension: &str,
    cancel: &CancellationToken,
) -> Result<DecodedImage> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(invalid("encoded image exceeds 64 MiB"));
    }
    // Third-party malformed-input panics become typed failures on the worker.
    std::panic::catch_unwind(|| {
        if bytes.starts_with(b"8BPS") {
            return decode_psd(bytes);
        }
        if extension == "kra" {
            return decode_kra(bytes, cancel);
        }
        if matches!(extension, "svg" | "svgz")
            || bytes
                .strip_prefix(b"\xef\xbb\xbf")
                .unwrap_or(bytes)
                .iter()
                .copied()
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(b'<')
        {
            return decode_svg(bytes);
        }
        // DXT4 is the premultiplied counterpart of DXT5. image supports the same
        // block encoding under the latter FourCC; restore straight RGB afterward.
        if bytes.starts_with(b"DDS ") && bytes.get(84..88) == Some(b"DXT4") {
            let mut adapted = bytes.to_vec();
            adapted[84..88].copy_from_slice(b"DXT5");
            let image = decode_raster(&adapted, Some(ImageFormat::Dds))?;
            let mut rgba = image.rgba().to_vec();
            unpremultiply(&mut rgba);
            return DecodedImage::new(image.width, image.height, rgba);
        }
        let format = image::guess_format(bytes)
            .ok()
            .or_else(|| ImageFormat::from_extension(extension));
        decode_raster(bytes, format)
    })
    .map_err(|_| invalid("decoder rejected malformed image data"))?
}
fn decode_raster(bytes: &[u8], format: Option<ImageFormat>) -> Result<DecodedImage> {
    let format = format.ok_or_else(|| invalid("unrecognized image format"))?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(invalid)?;
    let (width, height) = decoder.dimensions();
    validate_size(width, height)?;
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(invalid)?
        .into_rgba8()
        .into_raw();
    DecodedImage::new(width, height, rgba)
}
fn decode_kra(bytes: &[u8], cancel: &CancellationToken) -> Result<DecodedImage> {
    // Bound central-directory metadata before ZIP allocates its entry table.
    let tail = bytes.len().saturating_sub(65_557);
    let eocd = bytes[tail..]
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .map(|offset| tail + offset)
        .ok_or_else(|| invalid("KRA archive end record is missing"))?;
    let record = bytes
        .get(eocd..eocd + 22)
        .ok_or_else(|| invalid("truncated KRA end record"))?;
    let count = u16::from_le_bytes([record[10], record[11]]);
    if count > 4096 {
        return Err(invalid("KRA archive contains too many entries"));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    if archive.len() > 4096 {
        return Err(invalid("KRA archive contains too many entries"));
    }
    let mut entry = archive.by_name("mergedimage.png").map_err(invalid)?;
    if entry.size() > MAX_ENCODED_BYTES as u64 || entry.is_dir() {
        return Err(invalid("KRA merged preview exceeds the image limit"));
    }
    let mut png = Vec::new();
    let mut chunk = [0; 65536];
    loop {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let read = entry.read(&mut chunk).map_err(invalid)?;
        if read == 0 {
            break;
        }
        if png.len() + read > MAX_ENCODED_BYTES {
            return Err(invalid(
                "KRA merged preview expanded beyond the image limit",
            ));
        }
        png.extend_from_slice(&chunk[..read]);
    }
    decode_raster(&png, Some(ImageFormat::Png))
}
fn decode_svg(bytes: &[u8]) -> Result<DecodedImage> {
    use resvg::{tiny_skia, usvg};
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(DomainError::Unsupported(
            "compressed SVG is not an enabled decoder format".into(),
        ));
    }
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(invalid("SVG input exceeds 4 MiB"));
    }
    let mut options = usvg::Options::default();
    // Raster references need a shared allocation budget before resvg may decode
    // them. External file references are intentionally unavailable in this
    // self-contained viewer; they must never resolve against the working dir.
    let unavailable = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let external = unavailable.clone();
    options.image_href_resolver.resolve_string = Box::new(move |_, _| {
        external.store(true, std::sync::atomic::Ordering::Relaxed);
        None
    });
    let resolver = usvg::ImageHrefResolver::default_data_resolver();
    let pixels = std::sync::atomic::AtomicU64::new(0);
    let embedded = unavailable.clone();
    options.image_href_resolver.resolve_data = Box::new(move |mime, data, options| {
        let image = (|| {
            let format = image::guess_format(&data).ok()?;
            let dimensions = ImageReader::with_format(Cursor::new(data.as_slice()), format)
                .into_dimensions()
                .ok()?;
            validate_size(dimensions.0, dimensions.1).ok()?;
            let count = u64::from(dimensions.0) * u64::from(dimensions.1);
            let previous = pixels.fetch_add(count, std::sync::atomic::Ordering::Relaxed);
            if previous.saturating_add(count) > MAX_PIXELS {
                return None;
            }
            resolver(mime, data, options)
        })();
        if image.is_none() {
            embedded.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        image
    });
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(bytes, &options).map_err(invalid)?;
    if unavailable.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(DomainError::Unsupported(
            "SVG image reference is unavailable or exceeds the decode budget".into(),
        ));
    }
    let size = tree.size().to_int_size();
    validate_size(size.width(), size.height())?;
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| invalid("SVG raster allocation failed"))?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    unpremultiply(&mut rgba);
    DecodedImage::new(size.width(), size.height(), rgba)
}
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}
fn be16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| invalid("truncated PSD"))?
            .try_into()
            .expect("two bytes"),
    ))
}
fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("truncated PSD"))?
            .try_into()
            .expect("four bytes"),
    ))
}
fn decode_psd(bytes: &[u8]) -> Result<DecodedImage> {
    let channels = usize::from(be16(bytes, 12)?);
    let height = be32(bytes, 14)?;
    let width = be32(bytes, 18)?;
    validate_size(width, height)?;
    let depth = be16(bytes, 22)?;
    let color = be16(bytes, 24)?;
    if depth != 8 || !((color == 3 && matches!(channels, 3 | 4)) || (color == 1 && channels == 1)) {
        return Err(DomainError::Unsupported(
            "PSD requires 8-bit RGB/RGBA or grayscale composite data".into(),
        ));
    }
    let mut offset = 26usize;
    // Like the original viewer, display the composite, without parsing unrelated
    // layer/resource graphs. Still validate every encoded section boundary.
    for _ in 0..3 {
        let length = be32(bytes, offset)? as usize;
        offset = offset
            .checked_add(4)
            .and_then(|value| value.checked_add(length))
            .filter(|value| *value <= bytes.len())
            .ok_or_else(|| invalid("PSD section extends beyond file"))?;
    }
    let compression = be16(bytes, offset)?;
    let pixels = width as usize * height as usize;
    if compression == 0 {
        if bytes.len() - offset - 2 != pixels * channels {
            return Err(invalid("PSD composite channel length mismatch"));
        }
    } else if compression == 1 {
        let row_count = height as usize * channels;
        let mut data = offset
            .checked_add(2 + row_count * 2)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("truncated PSD RLE table"))?;
        for row in 0..row_count {
            let length = usize::from(be16(bytes, offset + 2 + row * 2)?);
            let end = data
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| invalid("truncated PSD RLE row"))?;
            let mut decoded = 0;
            while data < end {
                let header = bytes[data] as i8;
                data += 1;
                match header {
                    -128 => {}
                    0..=127 => {
                        let count = header as usize + 1;
                        data = data
                            .checked_add(count)
                            .filter(|data| *data <= end)
                            .ok_or_else(|| invalid("truncated PSD literal"))?;
                        decoded += count;
                    }
                    _ => {
                        if data >= end {
                            return Err(invalid("truncated PSD repetition"));
                        }
                        data += 1;
                        decoded += (1 - i16::from(header)) as usize;
                    }
                }
                if decoded > width as usize {
                    return Err(invalid("PSD RLE row exceeds image width"));
                }
            }
            if decoded != width as usize {
                return Err(invalid("PSD RLE row width mismatch"));
            }
        }
        if data != bytes.len() {
            return Err(invalid("PSD RLE trailing data"));
        }
    } else {
        return Err(DomainError::Unsupported(
            "PSD composite compression must be raw or PackBits".into(),
        ));
    }
    let mut composite = Vec::with_capacity(38 + bytes.len() - offset);
    composite.extend_from_slice(&bytes[..26]);
    composite.extend_from_slice(&[0; 12]);
    composite.extend_from_slice(&bytes[offset..]);
    let psd = psd::Psd::from_bytes(&composite).map_err(invalid)?;
    DecodedImage::new(width, height, psd.rgba())
}
