//! Sign: Place a signature image on a page.
//!
//! This is a visual signature: a picture of a signature, or a typed name, drawn on the
//! page. It is not a cryptographic digital signature and proves nothing about who signed.

use super::edit::{self, Fonts};
use super::flatten::single_input;
use crate::doc::{self, StdFont, fmt};
use crate::img;
use crate::{Ctx, Error, Outcome, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// A PNG or JPEG of the signature. Transparency is kept.
    pub image: Option<PathBuf>,
    /// A name to write out when there is no image.
    pub typed: Option<String>,
    /// 1-based page to sign.
    pub page: usize,
    /// Left edge of the signature, in points from the left edge of the page as shown.
    pub x: f32,
    /// Top edge of the signature, in points from the top edge of the page as shown.
    pub y: f32,
    /// Width in points. The height follows the image's shape (or the typed name's size).
    pub width: f32,
    /// Colour of a typed signature and of the date.
    pub color: [u8; 3],
    /// Print the date under the signature.
    pub add_date: bool,
    /// The date to print. Today (YYYY-MM-DD) when left out.
    pub date_text: Option<String>,
    pub date_size: f32,
    /// Sign every page at the same spot instead of only `page`.
    pub all_pages: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { image: None, typed: None, page: 1, x: 0.0, y: 0.0, width: 170.0, color: [27, 42, 107], add_date: false, date_text: None, date_size: 10.0, all_pages: false }
    }
}

/// Slant of a typed signature: x moves by this much for each unit of height.
const SHEAR: f32 = 0.22;
const FONT: StdFont = StdFont::Times;

fn typed_name(opts: &Options) -> Option<&str> {
    opts.typed.as_deref().map(str::trim).filter(|t| !t.is_empty())
}

/// Font size of a typed signature: as large as fits `width`, within reason for its length.
fn typed_size(name: &str, width: f32) -> f32 {
    let unit = FONT.width(name, 1.0).max(0.01);
    (width / (unit + SHEAR * 0.7)).min(width * 0.28).max(4.0)
}

/// The size in points of the box the signature itself will fill (without the date), so
/// the UI can draw a placeholder of the right shape before signing.
pub fn placed_size(opts: &Options) -> Result<(f32, f32)> {
    let width = check_width(opts)?;
    if let Some(path) = edit::image_path(&opts.image) {
        let (w, h) = edit::image_size(path)?;
        if w == 0 || h == 0 {
            return Err(Error::invalid(format!("{} is an empty image.", path.display())));
        }
        return Ok((width, width * h as f32 / w as f32));
    }
    match typed_name(opts) {
        Some(name) => Ok((width, typed_size(name, width) * 1.1)),
        None => Err(Error::invalid("Choose a signature image or type your name first.")),
    }
}

fn check_width(opts: &Options) -> Result<f32> {
    if ![opts.x, opts.y, opts.width, opts.date_size].iter().all(|v| v.is_finite()) {
        return Err(Error::invalid("The signature position or size is not a number."));
    }
    if opts.width <= 0.0 {
        return Err(Error::invalid("The signature width must be above zero."));
    }
    Ok(opts.width)
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = single_input(inputs)?;
    let width = check_width(opts)?;
    let image_path = edit::image_path(&opts.image);
    let name = typed_name(opts);
    if image_path.is_none() && name.is_none() {
        return Err(Error::invalid("Choose a signature image or type your name first."));
    }

    ctx.report(0.0, "Opening the file");
    let mut doc = doc::load(input, ctx.password())?;
    let ids = doc::page_ids(&doc);
    let pages: Vec<usize> = if opts.all_pages {
        (1..=ids.len()).collect()
    } else if opts.page == 0 || opts.page > ids.len() {
        return Err(Error::invalid(format!("Page {} does not exist. This file has {}.", opts.page, super::flatten::plural(ids.len(), "page"))));
    } else {
        vec![opts.page]
    };

    let image = match image_path {
        Some(path) => Some(edit::embed_image(&mut doc, path)?),
        None => None,
    };
    let date = if opts.add_date { Some(opts.date_text.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(edit::today)) } else { None };
    let date_size = if opts.date_size > 0.0 { opts.date_size.min(72.0) } else { 10.0 };
    let mut fonts = Fonts::default();
    let mut outside = false;

    for (done, &page) in pages.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.05 + 0.85 * done as f32 / pages.len() as f32, &format!("Signing page {page} of {}", ids.len()));
        let pid = ids[page - 1];
        let (pw, ph) = doc::visible_size(&doc, pid);
        let mut content = doc::cm(doc::visible_to_page(&doc, pid));

        // The signature, with its top-left corner at (x, y) in display space.
        let height = match (image, name) {
            (Some((id, iw, ih)), _) => {
                let height = width * ih as f32 / iw as f32;
                let xobject = doc::ensure_xobject(&mut doc, pid, id)?;
                content += &img::draw(&xobject, opts.x, ph - opts.y - height, width, height);
                height
            }
            (None, Some(name)) => {
                let size = typed_size(name, width);
                let font = fonts.name(&mut doc, pid, FONT)?;
                let baseline = ph - opts.y - 0.8 * size;
                content += &edit::fill_rgb(opts.color);
                // A shear matrix leans the upright Times letters like handwriting.
                content += &format!("BT /{font} {} Tf 1 0 {} 1 {} {} Tm {} Tj ET\n", fmt(size), fmt(SHEAR), fmt(opts.x), fmt(baseline), doc::pdf_string(name));
                size * 1.1
            }
            (None, None) => 0.0,
        };
        if let Some(date) = &date {
            let font = fonts.name(&mut doc, pid, StdFont::Helvetica)?;
            let baseline = ph - opts.y - height - 3.0 - edit::ascent(StdFont::Helvetica) * date_size;
            content += &edit::fill_rgb(opts.color);
            content += &format!("BT /{font} {} Tf 1 0 0 1 {} {} Tm {} Tj ET\n", fmt(date_size), fmt(opts.x), fmt(baseline), doc::pdf_string(date));
        }
        outside |= opts.x >= pw || opts.y >= ph || opts.x + width <= 0.0 || opts.y + height <= 0.0;
        doc::overlay(&mut doc, pid, content.into_bytes(), false)?;
    }

    ctx.check()?;
    ctx.report(0.95, "Saving");
    doc::save(&mut doc, out)?;
    let mut outcome = Outcome::single(out.to_path_buf(), ids.len(), crate::ctx::file_size(input));
    outcome.notes.push("This is a visual signature: a picture on the page. It is not a cryptographic digital signature and does not certify the document.".into());
    if outside {
        outcome.notes.push("The signature position is outside the page, so it will not be visible.".into());
    }
    if name.is_some_and(edit::loses_characters) && image.is_none() {
        outcome.notes.push(edit::LOSSY_NOTE.into());
    }
    super::flatten::unlocked_note(&doc, &mut outcome);
    ctx.report(1.0, "Done");
    Ok(outcome)
}
