//! PDF to Images: Save pages as PNG or JPEG images.

use crate::render::{Renderer, dpi_to_scale};
use crate::{Ctx, Error, Outcome, Result, doc, helpers, range};
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    #[default]
    Png,
    #[serde(alias = "jpeg")]
    Jpg,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub format: Format,
    /// Resolution of the images. 72 gives one pixel per point.
    pub dpi: u32,
    /// Page range such as `1-3, 7`. Empty means every page.
    pub pages: String,
    /// JPEG quality, 1 to 100. Not used for PNG.
    pub quality: u8,
    /// File name without extension. `{name}` is the PDF's name, `{n}` the page number
    /// padded with zeros to the width of the last page number, `{total}` the page count.
    pub name_pattern: String,
}

impl Default for Options {
    fn default() -> Self {
        Options { format: Format::Png, dpi: 150, pages: String::new(), quality: 90, name_pattern: "{name}-{n}".into() }
    }
}

/// The file name (without extension) for a page. Characters that cannot be part of a
/// file name are replaced, so a pattern can never point outside the output folder.
pub fn file_stem(pattern: &str, name: &str, page: usize, page_count: usize) -> String {
    let width = page_count.max(1).to_string().len();
    let pattern = if pattern.trim().is_empty() { "{name}-{n}" } else { pattern.trim() };
    let pattern = if pattern.contains("{n}") || pattern.contains("{page}") { pattern.to_string() } else { format!("{pattern}-{{n}}") };
    let number = format!("{page:0width$}");
    let raw = pattern.replace("{name}", name).replace("{n}", &number).replace("{page}", &number).replace("{total}", &page_count.to_string());
    let clean: String = raw.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    let clean = clean.trim_matches(|c: char| c == ' ' || c == '.').to_string();
    if clean.is_empty() { number } else { clean }
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = match inputs {
        [one] => one,
        [] => return Err(Error::invalid("Choose a PDF to turn into images.")),
        _ => return Err(Error::invalid("PDF to Images works on one PDF at a time.")),
    };
    if !(18..=1200).contains(&opts.dpi) {
        return Err(Error::invalid("The resolution must be between 18 and 1200 dpi."));
    }
    if opts.format == Format::Jpg && !(1..=100).contains(&opts.quality) {
        return Err(Error::invalid("JPEG quality must be between 1 and 100."));
    }
    let bytes = doc::read_file(input)?;
    let bytes_in = bytes.len() as u64;
    let renderer = Renderer::open(bytes, ctx.password())?;
    let page_count = renderer.page_count();
    if page_count == 0 {
        return Err(Error::damaged("This PDF has no pages."));
    }
    let mut pages = range::parse(&opts.pages, page_count)?;
    let mut seen = std::collections::HashSet::new();
    pages.retain(|p| seen.insert(*p));

    helpers::ensure_dir(out)?;
    let name = helpers::stem(input);
    let scale = dpi_to_scale(opts.dpi as f32);
    let mut outcome = Outcome { bytes_in, ..Default::default() };
    let mut capped = 0usize;
    let total = pages.len();

    for (i, &page) in pages.iter().enumerate() {
        ctx.check()?;
        ctx.report(i as f32 / total as f32, &format!("Rendering page {page} ({} of {total})", i + 1));
        let image = renderer.render(page - 1, scale)?;
        if let Some((w, _)) = renderer.page_size(page - 1) {
            if (image.width() as f32) < w * scale * 0.98 {
                capped += 1;
            }
        }
        let rgb = DynamicImage::ImageRgba8(image).into_rgb8();
        let path = out.join(format!("{}.{}", file_stem(&opts.name_pattern, &name, page, page_count), opts.format.ext()));
        let mut encoded = Vec::new();
        match opts.format {
            Format::Png => {
                let encoder = image::codecs::png::PngEncoder::new(&mut encoded);
                rgb.write_with_encoder(encoder)?;
            }
            Format::Jpg => {
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, opts.quality);
                rgb.write_with_encoder(encoder)?;
            }
        }
        doc::write_file(&path, &encoded)?;
        outcome.push(path);
        outcome.pages += 1;
    }
    if capped > 0 {
        outcome.notes.push(format!(
            "{capped} {} too large to render at {} dpi and {} saved at a lower resolution.",
            if capped == 1 { "page was" } else { "pages were" },
            opts.dpi,
            if capped == 1 { "was" } else { "were" }
        ));
    }
    ctx.report(1.0, "Done");
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_pages() {
        assert_eq!(file_stem("{name}-{n}", "report", 2, 6), "report-2");
        assert_eq!(file_stem("{name}-{n}", "report", 2, 120), "report-002");
        assert_eq!(file_stem("page", "report", 7, 12), "page-07");
        assert_eq!(file_stem("../{name}/{n}", "r", 1, 1), "_r_1");
        assert_eq!(file_stem("", "r", 1, 1), "r-1");
    }
}
