//! Putting raster images into a PDF.

use crate::{Error, Result};
use flate2::{Compression, write::ZlibEncoder};
use image::{DynamicImage, GenericImageView};
use lopdf::{Document, Object, ObjectId, Stream, dictionary};
use std::io::Write;

#[derive(Debug, Clone, Copy)]
pub enum Encoding {
    /// Lossy, 1 to 100. Right for photographs and scans.
    Jpeg(u8),
    /// Lossless. Right for line art, screenshots and anything with sharp edges.
    Flate,
}

fn deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::new(6));
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

/// Add an image XObject. Transparency is kept as a soft mask. Returns the object id.
pub fn add_image(doc: &mut Document, img: &DynamicImage, encoding: Encoding) -> Result<ObjectId> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::invalid("The image is empty."));
    }
    let gray = matches!(img.color(), image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8 | image::ColorType::La16);
    let has_alpha = img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] != 255);

    let smask = if has_alpha {
        let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p.0[3]).collect();
        let dict = dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
            "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8, "Filter" => "FlateDecode",
        };
        Some(doc.add_object(Stream::new(dict, deflate(&alpha)?).with_compression(false)))
    } else {
        None
    };

    let (data, filter, space) = match encoding {
        Encoding::Jpeg(quality) => {
            let mut buf = Vec::new();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
            if gray {
                enc.encode_image(&img.to_luma8())?;
            } else {
                enc.encode_image(&img.to_rgb8())?;
            }
            (buf, "DCTDecode", if gray { "DeviceGray" } else { "DeviceRGB" })
        }
        Encoding::Flate => {
            if gray {
                (deflate(img.to_luma8().as_raw())?, "FlateDecode", "DeviceGray")
            } else {
                (deflate(img.to_rgb8().as_raw())?, "FlateDecode", "DeviceRGB")
            }
        }
    };
    let mut dict = dictionary! {
        "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
        "ColorSpace" => space, "BitsPerComponent" => 8, "Filter" => filter,
    };
    if let Some(id) = smask {
        dict.set("SMask", Object::Reference(id));
    }
    Ok(doc.add_object(Stream::new(dict, data).with_compression(false)))
}

/// Add an existing JPEG file as is, without decoding and re-encoding it.
/// `components` is 1 for grey, 3 for RGB, 4 for CMYK.
pub fn add_jpeg_bytes(doc: &mut Document, jpeg: Vec<u8>, width: u32, height: u32, components: u8) -> ObjectId {
    let space = match components {
        1 => "DeviceGray",
        4 => "DeviceCMYK",
        _ => "DeviceRGB",
    };
    let mut dict = dictionary! {
        "Type" => "XObject", "Subtype" => "Image", "Width" => width as i64, "Height" => height as i64,
        "ColorSpace" => space, "BitsPerComponent" => 8, "Filter" => "DCTDecode",
    };
    if components == 4 {
        // Adobe CMYK JPEGs store inverted values.
        dict.set("Decode", vec![1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into()]);
    }
    doc.add_object(Stream::new(dict, jpeg).with_compression(false))
}

/// Content-stream operators that draw a registered image into the box `x, y, w, h`.
/// A small picture of an image file for the UI: camera orientation applied, the longer
/// side at most `max_px`.
pub fn thumbnail(path: &std::path::Path, max_px: u32) -> Result<image::RgbaImage> {
    use image::{ImageDecoder, metadata::Orientation};
    let unreadable = || Error::invalid(format!("{} is not an image this app can read.", path.display()));
    let reader = image::ImageReader::open(path).map_err(|e| Error::read(path, e))?.with_guessed_format().map_err(|e| Error::read(path, e))?;
    let mut decoder = reader.into_decoder().map_err(|_| unreadable())?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| unreadable())?;
    image.apply_orientation(orientation);
    Ok(image.thumbnail(max_px, max_px).to_rgba8())
}

pub fn draw(name: &str, x: f32, y: f32, w: f32, h: f32) -> String {
    use crate::doc::fmt;
    format!("q {} 0 0 {} {} {} cm /{} Do Q\n", fmt(w), fmt(h), fmt(x), fmt(y), name)
}
