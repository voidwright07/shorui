//! Watermark: Stamp text or an image across pages.

use super::edit::{self, Fonts};
use super::flatten::single_input;
use crate::doc::{self, StdFont, fmt};
use crate::{Ctx, Error, Outcome, Result, range};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    #[default]
    Center,
    /// Repeated across the whole page.
    Tile,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// The words to stamp. A line break starts a new line.
    pub text: String,
    /// Stamp this picture instead of the text.
    pub image: Option<PathBuf>,
    /// 0 (invisible) to 1 (solid).
    pub opacity: f32,
    /// Degrees, counter-clockwise. Applies to text and images.
    pub rotation: f32,
    /// Font size in points. 0 fits the text to the page: about 70% of the diagonal when
    /// centred, smaller for a corner or a tile.
    pub size: f32,
    pub color: [u8; 3],
    pub position: Position,
    /// Width of an image watermark as a fraction of the page width.
    pub scale: f32,
    /// Put the watermark behind the page content instead of on top of it.
    pub under: bool,
    /// Page range, such as "1-3, 7". Empty means every page.
    pub pages: String,
    pub font: StdFont,
    /// Distance from the page edge for the corner positions, in points.
    pub margin: f32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            text: "CONFIDENTIAL".into(),
            image: None,
            opacity: 0.25,
            rotation: 45.0,
            size: 0.0,
            color: [128, 128, 128],
            position: Position::Center,
            scale: 0.4,
            under: false,
            pages: String::new(),
            font: StdFont::HelveticaBold,
            margin: 36.0,
        }
    }
}

const LEADING: f32 = 1.2;

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = single_input(inputs)?;
    let image_path = edit::image_path(&opts.image);
    let lines: Vec<&str> = opts.text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if image_path.is_none() && lines.is_empty() {
        return Err(Error::invalid("Type the watermark text or choose an image first."));
    }
    if ![opts.opacity, opts.rotation, opts.size, opts.scale, opts.margin].iter().all(|v| v.is_finite()) {
        return Err(Error::invalid("One of the watermark settings is not a number."));
    }
    let opacity = opts.opacity.clamp(0.01, 1.0);
    let margin = opts.margin.max(0.0);

    ctx.report(0.0, "Opening the file");
    let mut doc = doc::load(input, ctx.password())?;
    let ids = doc::page_ids(&doc);
    let pages: BTreeSet<usize> = range::parse(&opts.pages, ids.len())?.into_iter().collect();

    let image = match image_path {
        Some(path) => Some(edit::embed_image(&mut doc, path)?),
        None => None,
    };
    let mut fonts = Fonts::default();
    let theta = opts.rotation.to_radians();
    let (sin, cos) = theta.sin_cos();

    for (done, &page) in pages.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.05 + 0.85 * done as f32 / pages.len() as f32, &format!("Stamping page {page} of {}", ids.len()));
        let pid = ids[page - 1];
        let (pw, ph) = doc::visible_size(&doc, pid);
        let diagonal = pw.hypot(ph);

        // The size of one stamp before rotation, and the operators that draw it with its
        // centre at the origin.
        let (bw, bh, stamp) = match image {
            Some((id, iw, ih)) => {
                let w = (opts.scale.clamp(0.01, 4.0)) * pw;
                let h = w * ih as f32 / iw as f32;
                let name = doc::ensure_xobject(&mut doc, pid, id)?;
                (w, h, format!("{} 0 0 {} {} {} cm /{name} Do\n", fmt(w), fmt(h), fmt(-w / 2.0), fmt(-h / 2.0)))
            }
            None => {
                let unit = lines.iter().map(|l| opts.font.width(l, 1.0)).fold(0.0, f32::max).max(0.01);
                let size = if opts.size > 0.0 {
                    opts.size.min(2000.0)
                } else {
                    match opts.position {
                        Position::Center => {
                            // Along the direction the text runs, this much of the page is available.
                            let run = (pw / cos.abs().max(1e-3)).min(ph / sin.abs().max(1e-3));
                            ((0.7 * diagonal).min(0.9 * run) / unit).min(0.4 * pw.min(ph))
                        }
                        Position::Tile => (0.25 * pw / unit).clamp(8.0, 40.0),
                        _ => (0.4 * pw / unit).clamp(6.0, 48.0),
                    }
                };
                let cap = edit::ascent(opts.font) * size;
                let w = unit * size;
                let h = cap + (lines.len() - 1) as f32 * size * LEADING;
                let name = fonts.name(&mut doc, pid, opts.font)?;
                let mut s = format!("BT /{name} {} Tf\n", fmt(size));
                for (i, line) in lines.iter().enumerate() {
                    // Each line is centred on the block; the block is centred on the origin.
                    let x = -opts.font.width(line, size) / 2.0;
                    let y = h / 2.0 - cap - i as f32 * size * LEADING;
                    s += &format!("1 0 0 1 {} {} Tm {} Tj\n", fmt(x), fmt(y), doc::pdf_string(line));
                }
                s += "ET\n";
                (w, h, s)
            }
        };

        // Half the size of the upright box around the rotated stamp.
        let hw = (bw * cos.abs() + bh * sin.abs()) / 2.0;
        let hh = (bw * sin.abs() + bh * cos.abs()) / 2.0;
        let centres: Vec<(f32, f32)> = match opts.position {
            Position::Center => vec![(pw / 2.0, ph / 2.0)],
            Position::TopLeft => vec![(margin + hw, ph - margin - hh)],
            Position::TopRight => vec![(pw - margin - hw, ph - margin - hh)],
            Position::BottomLeft => vec![(margin + hw, margin + hh)],
            Position::BottomRight => vec![(pw - margin - hw, margin + hh)],
            Position::Tile => {
                // A grid laid out along the rotated axes, every other row shifted by half a step.
                let step_x = (bw * 1.3 + 0.04 * pw).max(12.0);
                let step_y = (bh * 2.2 + 0.07 * ph).max(12.0);
                let reach = diagonal / 2.0;
                let cols = (reach / step_x).ceil() as i32 + 1;
                let rows = (reach / step_y).ceil() as i32 + 1;
                let mut v = Vec::new();
                for j in -rows..=rows {
                    for i in -cols..=cols {
                        let u = i as f32 * step_x + if j.rem_euclid(2) == 1 { step_x / 2.0 } else { 0.0 };
                        let w = j as f32 * step_y;
                        let x = pw / 2.0 + u * cos - w * sin;
                        let y = ph / 2.0 + u * sin + w * cos;
                        if x + hw > 0.0 && x - hw < pw && y + hh > 0.0 && y - hh < ph {
                            v.push((x, y));
                        }
                    }
                }
                v
            }
        };

        let gs = doc::ensure_alpha(&mut doc, pid, opacity)?;
        let mut content = doc::cm(doc::visible_to_page(&doc, pid));
        content += &format!("/{gs} gs\n");
        content += &edit::fill_rgb(opts.color);
        for (x, y) in centres {
            content += &format!("q {} {} {} {} {} {} cm\n{stamp}Q\n", fmt(cos), fmt(sin), fmt(-sin), fmt(cos), fmt(x), fmt(y));
        }
        doc::overlay(&mut doc, pid, content.into_bytes(), opts.under)?;
    }

    ctx.check()?;
    ctx.report(0.95, "Saving");
    doc::save(&mut doc, out)?;
    let mut outcome = Outcome::single(out.to_path_buf(), ids.len(), crate::ctx::file_size(input));
    if image.is_none() && lines.iter().any(|l| edit::loses_characters(l)) {
        outcome.notes.push(edit::LOSSY_NOTE.into());
    }
    if opts.under {
        outcome.notes.push("The watermark is behind the page content, so pages with a solid background or full-page images hide it.".into());
    }
    super::flatten::unlocked_note(&doc, &mut outcome);
    ctx.report(1.0, "Done");
    Ok(outcome)
}
