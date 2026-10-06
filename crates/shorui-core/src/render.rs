//! Rasterising pages. Pure Rust, no system libraries.

use crate::{Error, Result};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};
use image::RgbaImage;

/// An opened document ready to render pages from.
pub struct Renderer {
    pdf: Pdf,
}

impl Renderer {
    pub fn open(bytes: Vec<u8>, password: Option<&str>) -> Result<Self> {
        let pdf = Pdf::new_with_password(bytes, password.unwrap_or("")).map_err(|e| match e {
            LoadPdfError::Decryption(_) => {
                if password.is_some() { Error::WrongPassword } else { Error::PasswordRequired }
            }
            LoadPdfError::Invalid => Error::damaged("This file is damaged and its pages could not be read. Repair can usually rebuild it."),
        })?;
        Ok(Self { pdf })
    }

    pub fn open_path(path: &std::path::Path, password: Option<&str>) -> Result<Self> {
        Self::open(crate::doc::read_file(path)?, password)
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Size of a page as displayed, in points. `index` is 0-based.
    pub fn page_size(&self, index: usize) -> Option<(f32, f32)> {
        self.pdf.pages().get(index).map(|p| p.render_dimensions())
    }

    /// Render one page on white. `scale` 1.0 is 72 dpi; 2.0833 is 150 dpi.
    pub fn render(&self, index: usize, scale: f32) -> Result<RgbaImage> {
        let page = self.pdf.pages().get(index).ok_or_else(|| Error::invalid(format!("Page {} does not exist.", index + 1)))?;
        let (w, h) = page.render_dimensions();
        // The rasteriser addresses pixels with 16 bits.
        let max_scale = (16_000.0 / w.max(h)).max(0.05);
        let scale = scale.clamp(0.02, max_scale);
        let cache = RenderCache::new();
        let settings = RenderSettings { x_scale: scale, y_scale: scale, bg_color: WHITE, ..Default::default() };
        let pixmap = hayro::render(page, &cache, &InterpreterSettings::default(), &settings);
        let (pw, ph) = (pixmap.width() as u32, pixmap.height() as u32);
        let data: Vec<u8> = pixmap.take_unpremultiplied().into_iter().flat_map(|p| [p.r, p.g, p.b, p.a]).collect();
        RgbaImage::from_raw(pw, ph, data).ok_or_else(|| Error::other("The page could not be rendered."))
    }

    /// Render a page to fit inside `max_px` on its longer side.
    pub fn thumbnail(&self, index: usize, max_px: u32) -> Result<RgbaImage> {
        let (w, h) = self.page_size(index).ok_or_else(|| Error::invalid(format!("Page {} does not exist.", index + 1)))?;
        self.render(index, max_px as f32 / w.max(h))
    }
}

/// Dots per inch to a render scale.
pub fn dpi_to_scale(dpi: f32) -> f32 {
    dpi / 72.0
}
