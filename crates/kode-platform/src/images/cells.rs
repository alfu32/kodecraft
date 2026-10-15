//! Kode's RGB sampling, inclusive area averages, thresholds and quadrant masks.
//! Alpha is intentionally ignored, matching AsciiCore's forced-opaque input.
use super::DecodedImage;
use kode_core::{DomainError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellMode {
    #[default]
    Quart,
    Demi,
    Braille,
    Bixel,
    Ascii,
}
impl CellMode {
    pub fn next(self) -> Self {
        match self {
            Self::Quart => Self::Demi,
            Self::Demi => Self::Braille,
            Self::Braille => Self::Bixel,
            _ => Self::Quart,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Quart => "quart-glyph",
            Self::Demi => "demi-glyph",
            Self::Braille => "braille",
            Self::Bixel => "bixel",
            Self::Ascii => "ascii",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageCell {
    pub glyph: char,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
}
#[derive(Debug, Clone, Copy)]
pub struct CellOptions {
    pub width: u16,
    pub height: u16,
    pub mode: CellMode,
    pub gray: f64,
    pub scatter: f64,
}
#[derive(Debug, Clone)]
pub struct CellImage {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<ImageCell>,
}
#[derive(Default, Clone, Copy)]
struct Average {
    sum: [u64; 3],
    lum: f64,
    square: f64,
    count: u64,
}
impl Average {
    fn add(&mut self, p: [u8; 4]) {
        for (sum, c) in self.sum.iter_mut().zip(p) {
            *sum += u64::from(c);
        }
        let lum = luminance(p);
        self.lum += lum;
        self.square += lum * lum;
        self.count += 1;
    }
    fn rgb(self) -> [u8; 3] {
        self.sum.map(|sum| (sum / self.count.max(1)) as u8)
    }
    fn contrast(self, threshold: f64) -> [u8; 3] {
        let mean = self.lum / self.count.max(1) as f64;
        let variance = self.square / self.count.max(1) as f64 - mean * mean;
        let factor = if variance > threshold {
            if mean > 128.0 { 0.6 } else { 1.4 }
        } else {
            1.0
        };
        self.rgb()
            .map(|c| (f64::from(c) * factor).round().clamp(0.0, 255.0) as u8)
    }
}
fn luminance(p: [u8; 4]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}
fn adjusted(lum: f64, t: f64) -> f64 {
    if t >= 1.0 {
        0.0
    } else if t > 0.0 {
        ((lum / 255.0 - t) / (1.0 - t)).clamp(0.0, 1.0)
    } else {
        lum / 255.0
    }
}
fn area(image: &DecodedImage, x0: f64, x1: f64, y0: f64, y1: f64) -> Average {
    let x0 = (x0 as u32).min(image.width - 1);
    let x1 = (x1 as u32).clamp(x0, image.width - 1);
    let y0 = (y0 as u32).min(image.height - 1);
    let y1 = (y1 as u32).clamp(y0, image.height - 1);
    let mut average = Average::default();
    for y in y0..=y1 {
        for x in x0..=x1 {
            average.add(image.pixel(x, y));
        }
    }
    average
}
/// Only visible rows need sampling. `full_height` retains aspect scaling while
/// clipping avoids allocating a huge cell surface for a very tall source image.
pub fn render_cells(
    image: &DecodedImage,
    options: CellOptions,
    full_height: u32,
    mut cancelled: impl FnMut() -> bool,
) -> Result<CellImage> {
    if options.width == 0
        || options.height == 0
        || full_height == 0
        || !options.gray.is_finite()
        || !options.scatter.is_finite()
        || u64::from(options.width) * u64::from(options.height) > 1_000_000
    {
        return Err(DomainError::Unsupported(
            "invalid or oversized image cell surface".into(),
        ));
    }
    let mut cells = Vec::with_capacity(options.width as usize * options.height as usize);
    let width = f64::from(options.width);
    let height = f64::from(full_height);
    let gray = options.gray.clamp(0.0, 1.0);
    for y in 0..options.height {
        if cancelled() {
            return Err(DomainError::Cancelled);
        }
        for x in 0..options.width {
            let x = f64::from(x);
            let y = f64::from(y);
            let cell = match options.mode {
                CellMode::Demi | CellMode::Bixel => {
                    let sx = f64::from(image.width) / width;
                    let sy = f64::from(image.height) / (height * 2.0);
                    let top = area(
                        image,
                        x * sx,
                        (x + 1.0) * sx,
                        y * 2.0 * sy,
                        (y * 2.0 + 1.0) * sy,
                    );
                    let bottom = area(
                        image,
                        x * sx,
                        (x + 1.0) * sx,
                        (y * 2.0 + 1.0) * sy,
                        (y * 2.0 + 2.0) * sy,
                    );
                    ImageCell {
                        glyph: '▄',
                        foreground: bottom.rgb(),
                        background: top.rgb(),
                    }
                }
                CellMode::Quart => {
                    let sx = f64::from(image.width) / (width * 2.0);
                    let sy = f64::from(image.height) / (height * 2.0);
                    let points: [_; 4] = std::array::from_fn(|i| {
                        image.pixel(
                            ((x * 2.0 + (i % 2) as f64) * sx) as u32,
                            ((y * 2.0 + (i / 2) as f64) * sy) as u32,
                        )
                    });
                    let mean = points.iter().map(|p| luminance(*p)).sum::<f64>() / 4.0;
                    let mut fg = Average::default();
                    let mut bg = Average::default();
                    let mut mask = 0;
                    for (i, p) in points.into_iter().enumerate() {
                        if luminance(p) >= mean {
                            mask |= 1 << i;
                            fg.add(p);
                        } else {
                            bg.add(p);
                        }
                    }
                    let glyph = [
                        ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟',
                        '█',
                    ][mask];
                    ImageCell {
                        glyph,
                        foreground: if fg.count > 0 { fg.rgb() } else { bg.rgb() },
                        background: if bg.count > 0 { bg.rgb() } else { fg.rgb() },
                    }
                }
                CellMode::Braille => {
                    let sx = f64::from(image.width) / (width * 2.0);
                    let sy = f64::from(image.height) / (height * 4.0);
                    let mut mask = 0;
                    let mut average = Average::default();
                    for dy in 0..4 {
                        for dx in 0..2 {
                            let p = image.pixel(
                                ((x * 2.0 + f64::from(dx)) * sx) as u32,
                                ((y * 4.0 + f64::from(dy)) * sy) as u32,
                            );
                            average.add(p);
                            if adjusted(luminance(p), gray) > 0.5 {
                                mask |=
                                    [[1, 8], [2, 16], [4, 32], [64, 128]][dy as usize][dx as usize];
                            }
                        }
                    }
                    ImageCell {
                        glyph: char::from_u32(0x2800 + mask).expect("Braille mask"),
                        foreground: average.contrast(options.scatter),
                        background: average.rgb(),
                    }
                }
                CellMode::Ascii => {
                    let sx = f64::from(image.width) / width;
                    let sy = f64::from(image.height) / height;
                    let average = area(image, x * sx, (x + 1.0) * sx, y * sy, (y + 1.0) * sy);
                    let index = (9.0 * adjusted(average.lum / average.count.max(1) as f64, gray))
                        .round() as usize;
                    ImageCell {
                        glyph: [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'][index.min(9)],
                        foreground: average.contrast(options.scatter),
                        background: average.rgb(),
                    }
                }
            };
            cells.push(cell);
        }
    }
    Ok(CellImage {
        width: options.width,
        height: options.height,
        cells,
    })
}
