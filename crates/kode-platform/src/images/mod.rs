//! Worker-side decoding and portable colored-cell sampling.
pub mod cells;
pub mod decode;
use kode_core::{DomainError, Result};
use std::sync::Arc;

pub const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 16_384;

#[derive(Debug, Clone)]
pub struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}
impl DecodedImage {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        validate_size(width, height)?;
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(DomainError::Parse {
                format: "image pixels",
                message: "RGBA length does not match dimensions".into(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba: rgba.into(),
        })
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4) as usize;
        self.rgba[offset..offset + 4]
            .try_into()
            .expect("validated RGBA pixel")
    }
}
pub fn validate_size(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(DomainError::Unsupported(
            "image exceeds the dimension/16-megapixel decode limit".into(),
        ));
    }
    Ok(())
}

pub const DECODER_MIMES: &[&str] = &[
    "image/png",
    "image/bmp",
    "image/x-ms-bmp",
    "image/gif",
    "image/jpeg",
    "image/jpg",
    "image/webp",
    "image/x-tga",
    "image/x-targa",
    "image/tga",
    "image/vnd.ms-dds",
    "image/vnd-ms.dds",
    "image/x-dds",
    "image/vnd.adobe.photoshop",
    "image/x-photoshop",
    "image/psd",
    "application/x-krita",
    "image/svg+xml",
];
pub const IMAGE_EXTENSION_DEFAULTS: &[(&str, &str)] = &[
    ("tga", "image/x-tga"),
    ("dds", "image/vnd-ms.dds"),
    ("psd", "image/vnd.adobe.photoshop"),
    ("kra", "application/x-krita"),
];
