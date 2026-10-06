//! Images to PDF: Turn images into a PDF, one image per page.
//!
//! JPEG files go into the PDF byte for byte unless a quality is asked for, so nothing is
//! lost and nothing is recompressed. Everything else is stored losslessly, or as JPEG when
//! a quality is given (images with transparency always stay lossless).

use crate::img::{self, Encoding};
use crate::{Ctx, Error, Outcome, Result, doc};
use image::metadata::Orientation as Exif;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader};
use lopdf::{Dictionary, Object, ObjectId, Stream, StringFormat};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PageSize {
    /// The page is exactly the image, at `dpi`.
    #[default]
    Fit,
    A4,
    Letter,
    A5,
    Legal,
}

impl PageSize {
    /// Portrait size in points, or `None` for `Fit`.
    pub fn points(self) -> Option<(f32, f32)> {
        match self {
            PageSize::Fit => None,
            PageSize::A4 => Some((595.276, 841.89)),
            PageSize::Letter => Some((612.0, 792.0)),
            PageSize::A5 => Some((419.528, 595.276)),
            PageSize::Legal => Some((612.0, 1008.0)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    /// Landscape pages for images that are wider than tall, portrait otherwise.
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// `fit` makes each page the size of its image at `dpi`; the others are fixed paper sizes.
    pub page_size: PageSize,
    /// Resolution the image is assumed to have when `page_size` is `fit`.
    pub dpi: f32,
    /// Paper orientation for the fixed sizes. Ignored for `fit`, where the page follows the image.
    pub orientation: Orientation,
    /// White space around the image, in points. `None` means 0 for `fit` and 18 otherwise.
    pub margin: Option<f32>,
    /// `None` keeps JPEG files as they are and stores other images losslessly.
    /// `Some(1..=100)` re-encodes as JPEG; images with transparency stay lossless.
    pub quality: Option<u8>,
    /// Document title to record in the PDF.
    pub title: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { page_size: PageSize::Fit, dpi: 150.0, orientation: Orientation::Auto, margin: None, quality: None, title: None }
    }
}

/// The largest page a PDF viewer is required to handle, in points.
const MAX_PAGE: f32 = 14_400.0;

/// What the JPEG frame header says about the picture.
struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
}

/// Read the frame header of a JPEG. `None` when the file is not a baseline or progressive
/// 8-bit JPEG, which is all a PDF viewer is required to decode.
fn jpeg_info(bytes: &[u8]) -> Option<JpegInfo> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        match marker {
            0xFF => {
                i += 1;
                continue;
            }
            0x00 | 0x01 | 0xD0..=0xD8 => {
                i += 2;
                continue;
            }
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 {
            return None;
        }
        match marker {
            // Baseline, extended sequential and progressive Huffman frames.
            0xC0..=0xC2 => {
                let seg = bytes.get(i + 4..i + 2 + len)?;
                if seg.len() < 6 || seg[0] != 8 {
                    return None;
                }
                let height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                let width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                let components = seg[5];
                if width == 0 || height == 0 || !matches!(components, 1 | 3 | 4) {
                    return None;
                }
                return Some(JpegInfo { width, height, components });
            }
            // Lossless, differential and arithmetic-coded frames.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            _ => {}
        }
        i += 2 + len;
    }
    None
}

/// The matrix that draws the unit image square into `x, y, w, h` (the size after
/// orientation) turned the way the EXIF orientation asks.
fn placement(o: Exif, x: f32, y: f32, w: f32, h: f32) -> [f32; 6] {
    match o {
        Exif::NoTransforms => [w, 0.0, 0.0, h, x, y],
        Exif::Rotate90 => [0.0, -h, w, 0.0, x, y + h],
        Exif::Rotate180 => [-w, 0.0, 0.0, -h, x + w, y + h],
        Exif::Rotate270 => [0.0, h, -w, 0.0, x + w, y],
        Exif::FlipHorizontal => [-w, 0.0, 0.0, h, x + w, y],
        Exif::FlipVertical => [w, 0.0, 0.0, -h, x, y + h],
        Exif::Rotate90FlipH => [0.0, -h, -w, 0.0, x + w, y + h],
        Exif::Rotate270FlipH => [0.0, h, w, 0.0, x, y],
    }
}

fn swaps_axes(o: Exif) -> bool {
    matches!(o, Exif::Rotate90 | Exif::Rotate270 | Exif::Rotate90FlipH | Exif::Rotate270FlipH)
}

/// An image added to the document and how to draw it.
struct Placed {
    id: ObjectId,
    /// Pixel size as displayed, after orientation.
    width: u32,
    height: u32,
    /// Orientation still to be applied when drawing (only for JPEGs kept as they are).
    turn: Exif,
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string())
}

fn add(document: &mut lopdf::Document, path: &Path, quality: Option<u8>) -> Result<Placed> {
    let bytes = doc::read_file(path)?;
    let name = file_name(path);
    let unreadable = |detail: String| Error::invalid(format!("{name} could not be read as an image ({detail}). Supported formats are PNG, JPEG, BMP, GIF, TIFF and WebP."));

    let reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| unreadable(e.to_string()))?;
    let format = reader.format();
    if format.is_none() {
        return Err(unreadable("the format was not recognised".into()));
    }
    let mut decoder = reader.into_decoder().map_err(|e| unreadable(e.to_string()))?;
    let turn = decoder.orientation().unwrap_or(Exif::NoTransforms);

    if quality.is_none() && format == Some(ImageFormat::Jpeg) {
        if let Some(info) = jpeg_info(&bytes) {
            drop(decoder);
            let id = img::add_jpeg_bytes(document, bytes, info.width, info.height, info.components);
            let (width, height) = if swaps_axes(turn) { (info.height, info.width) } else { (info.width, info.height) };
            return Ok(Placed { id, width, height, turn });
        }
    }

    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| unreadable(e.to_string()))?;
    image.apply_orientation(turn);
    let transparent = image.color().has_alpha() && image.to_rgba8().pixels().any(|p| p.0[3] != 255);
    let encoding = match quality {
        Some(q) if !transparent => Encoding::Jpeg(q.clamp(1, 100)),
        _ => Encoding::Flate,
    };
    let (width, height) = image.dimensions();
    let id = img::add_image(document, &image, encoding)?;
    Ok(Placed { id, width, height, turn: Exif::NoTransforms })
}

/// A PDF text string: plain when it fits Latin-1, UTF-16 otherwise.
fn text_string(text: &str) -> Object {
    if text.chars().all(|c| (c as u32) < 0x80) {
        Object::string_literal(text)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    if inputs.is_empty() {
        return Err(Error::invalid("Add at least one image to turn into a PDF."));
    }
    if !(opts.dpi.is_finite() && opts.dpi >= 1.0) {
        return Err(Error::invalid("The resolution must be at least 1 dpi."));
    }
    if let Some(q) = opts.quality {
        if !(1..=100).contains(&q) {
            return Err(Error::invalid("JPEG quality must be between 1 and 100."));
        }
    }
    let margin = match opts.margin {
        Some(m) if m.is_finite() && m >= 0.0 => m,
        Some(_) => return Err(Error::invalid("The margin cannot be negative.")),
        None => {
            if opts.page_size == PageSize::Fit {
                0.0
            } else {
                18.0
            }
        }
    };

    let (mut document, root) = doc::new_document();
    let mut pages = Vec::with_capacity(inputs.len());
    let mut bytes_in = 0u64;
    let mut notes = Vec::new();
    let mut shrunk = 0usize;
    let total = inputs.len();

    for (index, path) in inputs.iter().enumerate() {
        ctx.check()?;
        ctx.report(index as f32 / total as f32, &format!("Adding image {} of {}", index + 1, total));
        bytes_in += crate::ctx::file_size(path);
        let placed = add(&mut document, path, opts.quality)?;
        let (px_w, px_h) = (placed.width as f32, placed.height as f32);

        // Page size and the box the image is drawn in.
        let (page_w, page_h, x, y, w, h) = match opts.page_size.points() {
            None => {
                let mut w = px_w * 72.0 / opts.dpi;
                let mut h = px_h * 72.0 / opts.dpi;
                let limit = (MAX_PAGE - 2.0 * margin).max(72.0);
                if w > limit || h > limit {
                    let k = (limit / w).min(limit / h);
                    w *= k;
                    h *= k;
                    shrunk += 1;
                }
                (w + 2.0 * margin, h + 2.0 * margin, margin, margin, w, h)
            }
            Some((short, long)) => {
                let landscape = match opts.orientation {
                    Orientation::Auto => px_w > px_h,
                    Orientation::Portrait => false,
                    Orientation::Landscape => true,
                };
                let (page_w, page_h) = if landscape { (long, short) } else { (short, long) };
                let (avail_w, avail_h) = (page_w - 2.0 * margin, page_h - 2.0 * margin);
                if avail_w < 1.0 || avail_h < 1.0 {
                    return Err(Error::invalid("The margin leaves no room for the image. Use a smaller margin."));
                }
                let k = (avail_w / px_w).min(avail_h / px_h);
                let (w, h) = (px_w * k, px_h * k);
                (page_w, page_h, (page_w - w) / 2.0, (page_h - h) / 2.0, w, h)
            }
        };

        let page = doc::blank_page(&mut document, page_w, page_h);
        let name = doc::ensure_xobject(&mut document, page, placed.id)?;
        let content = if placed.turn == Exif::NoTransforms {
            img::draw(&name, x, y, w, h)
        } else {
            format!("q {}/{} Do Q\n", doc::cm(placement(placed.turn, x, y, w, h)), name)
        };
        let content_id = document.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        document.get_dictionary_mut(page)?.set("Contents", content_id);
        pages.push(page);
    }
    doc::append_pages(&mut document, root, &pages)?;

    if let Some(title) = opts.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let info = document.trailer.get(b"Info").and_then(Object::as_reference)?;
        document.get_dictionary_mut(info)?.set("Title", text_string(title));
    }
    if shrunk > 0 {
        notes.push(format!(
            "{shrunk} {} too large for a PDF page at {} dpi and {} scaled down to fit.",
            if shrunk == 1 { "image was" } else { "images were" },
            doc::fmt(opts.dpi),
            if shrunk == 1 { "was" } else { "were" }
        ));
    }

    ctx.check()?;
    ctx.report(0.98, "Writing the PDF");
    doc::save(&mut document, out)?;
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), pages.len(), bytes_in);
    outcome.notes = notes;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jpeg_frame_header() {
        let mut buf = Vec::new();
        let rgb = image::RgbImage::from_pixel(37, 21, image::Rgb([200, 10, 10]));
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80).encode_image(&rgb).unwrap();
        let info = jpeg_info(&buf).unwrap();
        assert_eq!((info.width, info.height, info.components), (37, 21, 3));
        assert!(jpeg_info(b"\x89PNG\r\n").is_none());
    }

    #[test]
    fn placement_keeps_the_box() {
        // Every orientation must map the unit square onto the same target box.
        for o in [Exif::NoTransforms, Exif::Rotate90, Exif::Rotate180, Exif::Rotate270, Exif::FlipHorizontal, Exif::FlipVertical, Exif::Rotate90FlipH, Exif::Rotate270FlipH] {
            let m = placement(o, 10.0, 20.0, 30.0, 40.0);
            let map = |u: f32, v: f32| (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
            let corners = [map(0.0, 0.0), map(1.0, 0.0), map(0.0, 1.0), map(1.0, 1.0)];
            let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
            let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
            let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
            let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
            assert_eq!((min_x, max_x, min_y, max_y), (10.0, 40.0, 20.0, 60.0), "{o:?}");
        }
    }
}
