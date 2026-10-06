//! N-up / Booklet: Put several pages on one sheet, or impose a booklet.

use super::crop::paper_size;
use super::merge::{self, UNPROTECTED_NOTE, plural};
use crate::ctx::file_size;
use crate::{Ctx, Error, Outcome, Result, doc, range};
use lopdf::ObjectId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Order {
    /// Left to right, then down.
    #[default]
    Row,
    /// Top to bottom, then across.
    Column,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Pages per sheet: 2, 4, 6, 8, 9 or 16.
    pub per_sheet: usize,
    /// Sheet size: `a4`, `letter`, ... or `WxH` in points.
    pub paper: String,
    /// Sheet orientation. Leave unset to use whichever wastes less paper.
    pub landscape: Option<bool>,
    /// Space around the edge of the sheet, in points.
    pub margin: f32,
    /// Space between pages, in points.
    pub gutter: f32,
    /// Draw a thin line around each page.
    pub border: bool,
    pub order: Order,
    /// Impose a saddle-stitched booklet: two pages per side on landscape sheets, ordered
    /// so that printing double-sided and folding the stack gives the pages in order.
    /// `per_sheet`, `landscape` and `order` are ignored.
    pub booklet: bool,
    /// Which pages to use, and in what order. Empty means all.
    pub pages: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            per_sheet: 2,
            paper: "a4".into(),
            landscape: None,
            margin: 18.0,
            gutter: 9.0,
            border: false,
            order: Order::Row,
            booklet: false,
            pages: String::new(),
        }
    }
}

struct Layout {
    sheet: (f32, f32),
    cols: usize,
    rows: usize,
    cell: (f32, f32),
}

fn layout(paper: (f32, f32), landscape: bool, per_sheet: usize, margin: f32, gutter: f32) -> Result<Layout> {
    let (short, long) = match per_sheet {
        2 => (1, 2),
        4 => (2, 2),
        6 => (2, 3),
        8 => (2, 4),
        9 => (3, 3),
        16 => (4, 4),
        _ => return Err(Error::invalid(format!("{per_sheet} pages per sheet is not available. Choose 2, 4, 6, 8, 9 or 16."))),
    };
    let (small, large) = (paper.0.min(paper.1), paper.0.max(paper.1));
    let (sheet, cols, rows) = if landscape { ((large, small), long, short) } else { ((small, large), short, long) };
    let cell = (
        (sheet.0 - 2.0 * margin - (cols as f32 - 1.0) * gutter) / cols as f32,
        (sheet.1 - 2.0 * margin - (rows as f32 - 1.0) * gutter) / rows as f32,
    );
    if cell.0 < 10.0 || cell.1 < 10.0 {
        return Err(Error::invalid("The margin and the space between pages leave no room for the pages. Make them smaller."));
    }
    Ok(Layout { sheet, cols, rows, cell })
}

/// How much paper the pages cover with this layout.
fn coverage(layout: &Layout, sizes: &[(f32, f32)]) -> f32 {
    sizes
        .iter()
        .map(|(w, h)| {
            let s = (layout.cell.0 / w.max(1.0)).min(layout.cell.1 / h.max(1.0));
            s * s * w * h
        })
        .sum()
}

/// Page order for a saddle-stitched booklet of `count` pages: each inner list is one
/// side of a sheet, left then right, as 0-based page indexes; `None` is a blank.
/// Sides alternate front, back, front, back.
pub fn booklet_order(count: usize) -> Vec<[Option<usize>; 2]> {
    let padded = count.div_ceil(4) * 4;
    let page = |number: usize| if number >= 1 && number <= count { Some(number - 1) } else { None };
    let mut sides = Vec::with_capacity(padded / 2);
    for sheet in 0..padded / 4 {
        sides.push([page(padded - 2 * sheet), page(2 * sheet + 1)]);
        sides.push([page(2 * sheet + 2), page(padded - 2 * sheet - 1)]);
    }
    sides
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = merge::one_input(inputs, if opts.booklet { "Booklet" } else { "N-up" })?;
    merge::guard_output(inputs, out)?;
    if !(opts.margin.is_finite() && opts.gutter.is_finite()) || opts.margin < 0.0 || opts.gutter < 0.0 {
        return Err(Error::invalid("The margin and the space between pages cannot be negative."));
    }
    let (paper, _) = paper_size(&opts.paper)?;

    ctx.report(0.0, "Reading the file");
    let mut src = super::unlock::load(input, ctx.password())?;
    let ids = doc::page_ids(&src);
    if ids.is_empty() {
        return Err(Error::damaged("This file has no pages."));
    }
    let chosen: Vec<ObjectId> = range::parse(&opts.pages, ids.len())?.iter().filter_map(|n| ids.get(n.wrapping_sub(1)).copied()).collect();

    // Links, comments and form fields cannot follow a page onto a shared sheet.
    let mut annotated = 0usize;
    for id in &chosen {
        if let Ok(dict) = src.get_dictionary_mut(*id) {
            if dict.remove(b"Annots").is_some() {
                annotated += 1;
            }
        }
    }

    let (mut dst, root) = doc::new_document();
    let copies = doc::import_pages(&mut dst, &src, &chosen)?;
    let mut sources: Vec<(ObjectId, (f32, f32))> = Vec::with_capacity(copies.len());
    for (i, id) in copies.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.05 + 0.35 * i as f32 / copies.len() as f32, &format!("Preparing page {} of {}", i + 1, copies.len()));
        sources.push(doc::page_as_xobject(&mut dst, *id)?);
    }
    for id in &copies {
        dst.objects.remove(id);
    }
    let sizes: Vec<(f32, f32)> = sources.iter().map(|s| s.1).collect();

    // Which page goes in which slot of which sheet.
    let (plan, slots): (Layout, Vec<Vec<Option<usize>>>) = if opts.booklet {
        let plan = layout(paper, true, 2, opts.margin, opts.gutter)?;
        (plan, booklet_order(sources.len()).into_iter().map(|side| side.to_vec()).collect())
    } else {
        let plan = match opts.landscape {
            Some(landscape) => layout(paper, landscape, opts.per_sheet, opts.margin, opts.gutter)?,
            None => {
                let upright = layout(paper, false, opts.per_sheet, opts.margin, opts.gutter)?;
                let sideways = layout(paper, true, opts.per_sheet, opts.margin, opts.gutter)?;
                if coverage(&sideways, &sizes) > coverage(&upright, &sizes) * 1.001 { sideways } else { upright }
            }
        };
        let all: Vec<Option<usize>> = (0..sources.len()).map(Some).collect();
        let slots = all.chunks(opts.per_sheet.max(1)).map(<[Option<usize>]>::to_vec).collect();
        (plan, slots)
    };

    let mut sheets: Vec<ObjectId> = Vec::with_capacity(slots.len());
    let total = slots.len();
    for (n, sheet_slots) in slots.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.4 + 0.5 * n as f32 / total as f32, &format!("Laying out sheet {} of {total}", n + 1));
        let sheet = doc::blank_page(&mut dst, plan.sheet.0, plan.sheet.1);
        let mut content = String::new();
        for (slot, source) in sheet_slots.iter().enumerate() {
            let Some((xobject, (w, h))) = source.and_then(|i| sources.get(i)).copied() else { continue };
            if w <= 0.0 || h <= 0.0 {
                continue;
            }
            let (col, row) = match opts.order {
                Order::Column if !opts.booklet => (slot / plan.rows, slot % plan.rows),
                _ => (slot % plan.cols, slot / plan.cols),
            };
            let cell_x = opts.margin + col as f32 * (plan.cell.0 + opts.gutter);
            let cell_y = plan.sheet.1 - opts.margin - (row as f32 + 1.0) * plan.cell.1 - row as f32 * opts.gutter;
            let scale = (plan.cell.0 / w).min(plan.cell.1 / h);
            let (pw, ph) = (w * scale, h * scale);
            let (x, y) = (cell_x + (plan.cell.0 - pw) / 2.0, cell_y + (plan.cell.1 - ph) / 2.0);
            let name = doc::ensure_xobject(&mut dst, sheet, xobject)?;
            content += &format!("q\n{}/{name} Do\nQ\n", doc::cm([scale, 0.0, 0.0, scale, x, y]));
            if opts.border {
                content += &format!("q 0.5 w 0.45 G {} {} {} {} re S Q\n", doc::fmt(x), doc::fmt(y), doc::fmt(pw), doc::fmt(ph));
            }
        }
        doc::overlay(&mut dst, sheet, content.into_bytes(), false)?;
        sheets.push(sheet);
    }
    doc::append_pages(&mut dst, root, &sheets)?;
    merge::copy_info(&mut dst, &src, &["Title", "Author", "Subject", "Keywords"])?;
    merge::prune(&mut dst);

    ctx.check()?;
    ctx.report(0.92, "Saving");
    doc::save(&mut dst, out)?;
    ctx.report(1.0, "Done");

    let mut outcome = Outcome::single(out.to_path_buf(), sheets.len(), file_size(input));
    if opts.booklet {
        let blanks = sheets.len() * 2 - sources.len();
        outcome.notes.push(format!(
            "{} of paper. Print double-sided, flipping on the short edge, then fold the stack in half.",
            plural(sheets.len() / 2, "sheet", "sheets")
        ));
        if blanks > 0 {
            outcome.notes.push(format!("{} added at the end so the booklet folds evenly.", plural(blanks, "blank page was", "blank pages were")));
        }
    }
    if annotated > 0 {
        outcome.notes.push("Links, comments and form fields are not carried onto the sheets. Flatten the file first to keep what they show.".into());
    }
    if src.was_encrypted() {
        outcome.notes.push(UNPROTECTED_NOTE.into());
    }
    Ok(outcome)
}
