//! Page Numbers & Bates: Add page numbers, headers, footers or Bates numbers.
//!
//! How pages are counted: the pages chosen by `pages` are numbered in document order.
//! The first of them gets `start` (or `bates_start`), the next one more, and so on.
//! `skip_first` leaves the first chosen page without a label, but that page still uses up
//! its number, so the second chosen page shows `start + 1`. To have the page after a
//! cover show 1, either set `start` to 0 with `skip_first`, or set `pages` to "2-".
//! `{total}` is the number the last chosen page gets (`start - 1 +` pages chosen) unless
//! `total` overrides it.

use super::edit::{self, Fonts};
use super::flatten::single_input;
use crate::doc::{self, StdFont, fmt};
use crate::{Ctx, Error, Outcome, Result, helpers, range};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// One label per page, from `format`, at `position`.
    #[default]
    PageNumbers,
    /// One Bates number per page, at `position`.
    Bates,
    /// Up to six labels per page from the `header_*` and `footer_*` templates.
    HeaderFooter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    #[default]
    BottomCenter,
    BottomLeft,
    BottomRight,
    TopCenter,
    TopLeft,
    TopRight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub mode: Mode,
    /// Template for page-numbers mode. Tokens: `{n}` page number, `{total}`, `{date}`
    /// (YYYY-MM-DD), `{file}` (name of the input without its extension), `{bates}`.
    pub format: String,
    pub position: Position,
    /// Number given to the first numbered page.
    pub start: usize,
    /// Do not print on the first numbered page. It still counts.
    pub skip_first: bool,
    pub bates_prefix: String,
    pub bates_suffix: String,
    /// Bates numbers are padded with zeros to this many digits.
    pub bates_digits: usize,
    pub bates_start: u64,
    pub header_left: String,
    pub header_center: String,
    pub header_right: String,
    pub footer_left: String,
    pub footer_center: String,
    pub footer_right: String,
    pub font: StdFont,
    pub size: f32,
    /// Distance from the page edge, in points.
    pub margin: f32,
    pub color: [u8; 3],
    /// Page range to number, such as "2-". Empty means every page.
    pub pages: String,
    /// What `{total}` shows. 0 works it out from `start` and the pages numbered.
    pub total: usize,
    /// What `{date}` shows. Today when left out.
    pub date_text: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: Mode::PageNumbers,
            format: "{n}".into(),
            position: Position::BottomCenter,
            start: 1,
            skip_first: false,
            bates_prefix: String::new(),
            bates_suffix: String::new(),
            bates_digits: 6,
            bates_start: 1,
            header_left: String::new(),
            header_center: String::new(),
            header_right: String::new(),
            footer_left: String::new(),
            footer_center: String::new(),
            footer_right: String::new(),
            font: StdFont::Helvetica,
            size: 10.0,
            margin: 28.0,
            color: [0, 0, 0],
            pages: String::new(),
            total: 0,
            date_text: None,
        }
    }
}

/// What one page is given: its place in the numbering.
#[derive(Debug, Clone, Copy)]
struct Slot {
    /// 0 for the first numbered page, 1 for the next, and so on.
    sequence: usize,
    /// False for the first numbered page when `skip_first` is on.
    printed: bool,
}

/// Which pages (1-based) are numbered, and how.
fn plan(opts: &Options, page_count: usize) -> Result<BTreeMap<usize, Slot>> {
    let mut pages = range::parse(&opts.pages, page_count)?;
    pages.sort_unstable();
    pages.dedup();
    Ok(pages.into_iter().enumerate().map(|(sequence, page)| (page, Slot { sequence, printed: !(opts.skip_first && sequence == 0) })).collect())
}

fn bates(opts: &Options, sequence: usize) -> String {
    let number = opts.bates_start.saturating_add(sequence as u64);
    format!("{}{:0width$}{}", opts.bates_prefix, number, opts.bates_suffix, width = opts.bates_digits.min(32))
}

fn fill(template: &str, opts: &Options, sequence: usize, numbered: usize, file: &str, date: &str) -> String {
    let n = opts.start + sequence;
    let total = if opts.total > 0 { opts.total } else { (opts.start + numbered).saturating_sub(1) };
    template
        .replace("{n}", &n.to_string())
        .replace("{total}", &total.to_string())
        .replace("{date}", date)
        .replace("{file}", file)
        .replace("{bates}", &bates(opts, sequence))
}

/// Every label a page gets, with where it goes. Empty for a page that is not numbered.
fn labels(opts: &Options, slot: Slot, numbered: usize, file: &str, date: &str) -> Vec<(Position, String)> {
    if !slot.printed {
        return Vec::new();
    }
    let mut out: Vec<(Position, String)> = match opts.mode {
        Mode::PageNumbers => vec![(opts.position, fill(&opts.format, opts, slot.sequence, numbered, file, date))],
        Mode::Bates => vec![(opts.position, bates(opts, slot.sequence))],
        Mode::HeaderFooter => [
            (Position::TopLeft, &opts.header_left),
            (Position::TopCenter, &opts.header_center),
            (Position::TopRight, &opts.header_right),
            (Position::BottomLeft, &opts.footer_left),
            (Position::BottomCenter, &opts.footer_center),
            (Position::BottomRight, &opts.footer_right),
        ]
        .into_iter()
        .map(|(position, template)| (position, fill(template, opts, slot.sequence, numbered, file, date)))
        .collect(),
    };
    out.retain(|(_, text)| !text.trim().is_empty());
    out
}

fn date_of(opts: &Options) -> String {
    opts.date_text.clone().filter(|d| !d.trim().is_empty()).unwrap_or_else(edit::today)
}

/// The labels `run` would print on page `page` (1-based) of a `total`-page file named
/// `file` (without extension), each with its position. Empty when the page gets none,
/// including when `pages` is not a valid range.
pub fn preview_labels(opts: &Options, page: usize, total: usize, file: &str) -> Vec<(Position, String)> {
    let Ok(plan) = plan(opts, total) else { return Vec::new() };
    match plan.get(&page) {
        Some(slot) => labels(opts, *slot, plan.len(), file, &date_of(opts)),
        None => Vec::new(),
    }
}

/// The text `run` would print on page `page` (1-based) of a `total`-page file named
/// `file`, for the UI's live preview. Header-footer mode can print several labels; they
/// are joined with " | ", left to right, headers first. Empty when the page gets no label.
pub fn preview_label(opts: &Options, page: usize, total: usize, file: &str) -> String {
    preview_labels(opts, page, total, file).into_iter().map(|(_, text)| text).collect::<Vec<_>>().join(" | ")
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = single_input(inputs)?;
    if !(opts.size.is_finite() && opts.size > 0.0) {
        return Err(Error::invalid("The text size must be above zero."));
    }
    if !opts.margin.is_finite() {
        return Err(Error::invalid("The margin is not a number."));
    }
    if opts.mode == Mode::HeaderFooter
        && [&opts.header_left, &opts.header_center, &opts.header_right, &opts.footer_left, &opts.footer_center, &opts.footer_right].iter().all(|t| t.trim().is_empty())
    {
        return Err(Error::invalid("Type a header or a footer first."));
    }
    if opts.mode == Mode::PageNumbers && opts.format.trim().is_empty() {
        return Err(Error::invalid("The number format is empty. Use something like {n} or Page {n} of {total}."));
    }

    ctx.report(0.0, "Opening the file");
    let mut doc = doc::load(input, ctx.password())?;
    let ids = doc::page_ids(&doc);
    let plan = plan(opts, ids.len())?;
    let file = helpers::stem(input);
    let date = date_of(opts);
    let size = opts.size.min(200.0);
    let margin = opts.margin.max(0.0);
    let mut fonts = Fonts::default();
    let mut lossy = false;
    let mut printed = 0usize;

    for (done, (&page, &slot)) in plan.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.05 + 0.85 * done as f32 / plan.len() as f32, &format!("Numbering page {page} of {}", ids.len()));
        let labels = labels(opts, slot, plan.len(), &file, &date);
        if labels.is_empty() {
            continue;
        }
        let pid = ids[page - 1];
        let (pw, ph) = doc::visible_size(&doc, pid);
        let font = fonts.name(&mut doc, pid, opts.font)?;
        let mut content = doc::cm(doc::visible_to_page(&doc, pid));
        content += &edit::fill_rgb(opts.color);
        content += &format!("BT /{font} {} Tf\n", fmt(size));
        for (position, text) in labels {
            lossy |= edit::loses_characters(&text);
            let width = opts.font.width(&text, size);
            let x = match position {
                Position::BottomLeft | Position::TopLeft => margin,
                Position::BottomCenter | Position::TopCenter => (pw - width) / 2.0,
                Position::BottomRight | Position::TopRight => pw - margin - width,
            };
            // The margin is measured to the edge of the letters: their feet at the
            // bottom of the page, their tops at the top.
            let y = match position {
                Position::BottomLeft | Position::BottomCenter | Position::BottomRight => margin,
                _ => ph - margin - edit::ascent(opts.font) * size,
            };
            content += &format!("1 0 0 1 {} {} Tm {} Tj\n", fmt(x), fmt(y), doc::pdf_string(&text));
        }
        content += "ET\n";
        doc::overlay(&mut doc, pid, content.into_bytes(), false)?;
        printed += 1;
    }

    ctx.check()?;
    ctx.report(0.95, "Saving");
    doc::save(&mut doc, out)?;
    let mut outcome = Outcome::single(out.to_path_buf(), ids.len(), crate::ctx::file_size(input));
    if opts.mode == Mode::Bates {
        if let (Some(first), Some(last)) = (plan.values().find(|s| s.printed), plan.values().next_back()) {
            outcome.notes.push(format!("Bates numbers {} to {} on {}.", bates(opts, first.sequence), bates(opts, last.sequence), super::flatten::plural(printed, "page")));
        }
    }
    if lossy {
        outcome.notes.push(edit::LOSSY_NOTE.into());
    }
    super::flatten::unlocked_note(&doc, &mut outcome);
    ctx.report(1.0, "Done");
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_plan() {
        let opts = Options { format: "Page {n} of {total}".into(), ..Default::default() };
        assert_eq!(preview_label(&opts, 3, 6, "report"), "Page 3 of 6");

        let opts = Options { format: "{n}/{total} {file}".into(), pages: "3-".into(), skip_first: true, ..Default::default() };
        assert_eq!(preview_label(&opts, 2, 6, "report"), "");
        assert_eq!(preview_label(&opts, 3, 6, "report"), "");
        assert_eq!(preview_label(&opts, 4, 6, "report"), "2/4 report");

        let opts = Options { start: 0, skip_first: true, ..Default::default() };
        assert_eq!(preview_label(&opts, 1, 3, "x"), "");
        assert_eq!(preview_label(&opts, 2, 3, "x"), "1");

        let opts = Options { mode: Mode::Bates, bates_prefix: "ABC".into(), ..Default::default() };
        assert_eq!(preview_label(&opts, 3, 6, "x"), "ABC000003");

        let opts = Options { mode: Mode::HeaderFooter, header_left: "{file}".into(), footer_right: "{n}".into(), date_text: Some("2026-01-05".into()), footer_center: "{date}".into(), ..Default::default() };
        assert_eq!(preview_label(&opts, 2, 6, "report"), "report | 2026-01-05 | 2");
        assert_eq!(preview_label(&opts, 9, 6, "report"), "");
    }
}
