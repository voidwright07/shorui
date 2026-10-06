//! Compare: Find the differences between two PDFs.
//!
//! The first input is the old version, the second the new one. Pages are paired by
//! position (page 3 with page 3); pages only one file has are reported as added or
//! removed. The output folder receives:
//!
//! - `report.json`: machine-readable result (see [`Report`]).
//! - `report.txt`: a readable summary with a unified diff of the text of each changed page.
//! - `diff-p<n>.png` for every page that looks different (modes `visual` and `both`): the
//!   new page in pale grey, with what was removed tinted red and what was added tinted green.
//! - `diff.pdf`: those pictures gathered one per page.

use crate::helpers::{ensure_dir, stem};
use crate::render::{Renderer, dpi_to_scale};
use crate::text::TextReader;
use crate::{Ctx, Error, Outcome, Result, doc, img, range};
use image::{DynamicImage, Rgb, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Compare the extracted text only. Fast; blind to pictures and layout.
    Text,
    /// Compare how the pages look only.
    Visual,
    #[default]
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// `text`, `visual` or `both`.
    pub mode: Mode,
    /// Resolution the pages are rendered at for the visual comparison.
    pub dpi: u32,
    /// How far a colour channel (0 to 255) must move before a pixel counts as changed.
    pub threshold: u8,
    /// Pages to compare, such as `1-3, 7`. Empty means all. Numbers refer to the longer file.
    pub pages: String,
}

impl Default for Options {
    fn default() -> Self {
        Options { mode: Mode::Both, dpi: 100, threshold: 24, pages: String::new() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Presence {
    /// The page exists in both files.
    Both,
    /// The page exists only in the new file.
    Added,
    /// The page exists only in the old file.
    Removed,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageReport {
    /// 1-based page number.
    pub page: usize,
    pub presence: Presence,
    pub text_changed: bool,
    pub visual_changed: bool,
    /// Share of the page's pixels that changed, 0.0 to 1.0. Always 0 in `text` mode.
    pub changed_ratio: f32,
    /// Lines of text the new page has and the old one does not.
    pub added: Vec<String>,
    /// Lines of text the old page has and the new one does not.
    pub removed: Vec<String>,
}

/// What `report.json` holds.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub pages_a: usize,
    pub pages_b: usize,
    pub identical: bool,
    pub pages: Vec<PageReport>,
}

/// The result of the quick comparison made by [`summary`].
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub pages_a: usize,
    pub pages_b: usize,
    /// Same page count and the same text on every page. Pictures are not looked at.
    pub identical: bool,
    /// 1-based numbers of the pages whose text differs, including pages only one file has.
    pub changed_pages: Vec<usize>,
    pub added_lines: usize,
    pub removed_lines: usize,
}

/// A cheap, text-only comparison of two files for the UI to show before the real run:
/// nothing is rendered and nothing is written. `a` is the old file, `b` the new one.
pub fn summary(a: &Path, b: &Path, password: Option<&str>) -> Result<Summary> {
    let old = TextReader::open_path(a, password)?;
    let new = TextReader::open_path(b, password)?;
    let (pages_a, pages_b) = (old.page_count(), new.page_count());
    let mut out = Summary { pages_a, pages_b, identical: pages_a == pages_b, changed_pages: Vec::new(), added_lines: 0, removed_lines: 0 };
    for index in 0..pages_a.max(pages_b) {
        let (added, removed) = text_changes(&lines_of(&old, index, pages_a)?, &lines_of(&new, index, pages_b)?);
        if !added.is_empty() || !removed.is_empty() || index >= pages_a || index >= pages_b {
            out.changed_pages.push(index + 1);
        }
        out.added_lines += added.len();
        out.removed_lines += removed.len();
    }
    out.identical = out.identical && out.changed_pages.is_empty();
    Ok(out)
}

/// The text lines of a page, or nothing when the file has no such page.
fn lines_of(reader: &TextReader, index: usize, count: usize) -> Result<Vec<String>> {
    if index >= count {
        return Ok(Vec::new());
    }
    Ok(reader.page(index)?.lines.iter().map(|l| l.text()).filter(|t| !t.trim().is_empty()).collect())
}

fn joined(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Lines only the new text has, and lines only the old text has.
fn text_changes(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let (a, b) = (joined(old), joined(new));
    let diff = TextDiff::from_lines(&a, &b);
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    for change in diff.iter_all_changes() {
        let line = change.value().trim_end_matches('\n').to_string();
        match change.tag() {
            ChangeTag::Insert => added.push(line),
            ChangeTag::Delete => removed.push(line),
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

fn unified(old: &[String], new: &[String]) -> String {
    let (a, b) = (joined(old), joined(new));
    TextDiff::from_lines(&a, &b).unified_diff().context_radius(2).to_string()
}

const RED: [f32; 3] = [214.0, 39.0, 40.0];
const GREEN: [f32; 3] = [28.0, 150.0, 66.0];

/// Compare two renders pixel by pixel. Returns the picture described in the module
/// documentation and the share of pixels that changed. A missing side counts as a blank
/// white page, and so does the area one page has beyond the edge of the other.
fn visual_diff(old: Option<&RgbaImage>, new: Option<&RgbaImage>, threshold: u8) -> (RgbImage, f32) {
    let dims = |i: Option<&RgbaImage>| i.map(|i| i.dimensions()).unwrap_or((0, 0));
    let (w, h) = (dims(old).0.max(dims(new).0).max(1), dims(old).1.max(dims(new).1).max(1));
    let pixel = |image: Option<&RgbaImage>, x: u32, y: u32| -> [u8; 3] {
        match image {
            Some(i) if x < i.width() && y < i.height() => {
                let p = i.get_pixel(x, y).0;
                [p[0], p[1], p[2]]
            }
            _ => [255, 255, 255],
        }
    };
    let luma = |p: [u8; 3]| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
    let mut changed = 0u64;
    let out = RgbImage::from_fn(w, h, |x, y| {
        let (a, b) = (pixel(old, x, y), pixel(new, x, y));
        let distance = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        // The new page, washed out so the tints stand out.
        let grey = 255.0 - (255.0 - luma(b)) * 0.35;
        if distance <= threshold {
            let g = grey as u8;
            return Rgb([g, g, g]);
        }
        changed += 1;
        // Darker than before means ink was added; lighter means it was taken away.
        let tint = if luma(b) < luma(a) { GREEN } else { RED };
        let strength = 0.55 + 0.45 * distance as f32 / 255.0;
        let mix = |c: usize| (grey * (1.0 - strength) + tint[c] * strength) as u8;
        Rgb([mix(0), mix(1), mix(2)])
    });
    (out, changed as f32 / (w as f32 * h as f32))
}

struct Picture {
    page: usize,
    image: RgbImage,
    /// Size of the page in points.
    size: (f32, f32),
}

fn list(pages: &[usize]) -> String {
    range::format(pages)
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let [path_a, path_b] = inputs else {
        return Err(Error::invalid("Compare needs exactly two PDFs: the old version first, then the new one."));
    };
    if opts.dpi == 0 || opts.dpi > 600 {
        return Err(Error::invalid("The comparison resolution must be between 1 and 600 dpi."));
    }
    ctx.report(0.0, "Opening both files");
    let bytes_a = doc::read_file(path_a)?;
    let bytes_b = doc::read_file(path_b)?;
    let bytes_in = (bytes_a.len() + bytes_b.len()) as u64;
    let (want_text, want_visual) = (opts.mode != Mode::Visual, opts.mode != Mode::Text);

    let text_a = TextReader::open(bytes_a.clone(), ctx.password())?;
    let text_b = TextReader::open(bytes_b.clone(), ctx.password())?;
    let (pages_a, pages_b) = (text_a.page_count(), text_b.page_count());
    let total = pages_a.max(pages_b);
    if total == 0 {
        return Err(Error::damaged("Neither file has any pages to compare."));
    }
    let renderers = if want_visual { Some((Renderer::open(bytes_a, ctx.password())?, Renderer::open(bytes_b, ctx.password())?)) } else { None };
    let selected = range::parse(&opts.pages, total)?;
    let whole_document = selected.len() == total;

    ensure_dir(out)?;
    let mut reports: Vec<PageReport> = Vec::new();
    let mut pictures: Vec<Picture> = Vec::new();
    let mut text_report = String::new();
    let scale = dpi_to_scale(opts.dpi as f32);

    for (n, &page) in selected.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.02 + 0.9 * n as f32 / selected.len() as f32, &format!("Comparing page {page} of {total}"));
        let index = page - 1;
        let presence = match (index < pages_a, index < pages_b) {
            (true, true) => Presence::Both,
            (false, _) => Presence::Added,
            (true, false) => Presence::Removed,
        };
        let mut report = PageReport { page, presence, text_changed: false, visual_changed: false, changed_ratio: 0.0, added: Vec::new(), removed: Vec::new() };

        // Text is read in every mode: it is cheap and the report is more useful with it.
        let old_lines = lines_of(&text_a, index, pages_a)?;
        let new_lines = lines_of(&text_b, index, pages_b)?;
        let (added, removed) = text_changes(&old_lines, &new_lines);
        if want_text {
            report.text_changed = !added.is_empty() || !removed.is_empty();
            report.added = added;
            report.removed = removed;
        }

        if let Some((render_a, render_b)) = &renderers {
            let old = if index < pages_a { Some(render_a.render(index, scale)?) } else { None };
            let new = if index < pages_b { Some(render_b.render(index, scale)?) } else { None };
            let (image, ratio) = visual_diff(old.as_ref(), new.as_ref(), opts.threshold);
            report.visual_changed = ratio > 0.0 || presence != Presence::Both;
            report.changed_ratio = if presence == Presence::Both { ratio } else { 1.0 };
            if report.visual_changed {
                let size = if index < pages_b { render_b.page_size(index) } else { render_a.page_size(index) }.unwrap_or((595.0, 842.0));
                pictures.push(Picture { page, image, size });
            }
        }

        let differs = report.text_changed || report.visual_changed || presence != Presence::Both;
        if differs {
            let what = match presence {
                Presence::Added => "only in the new file".to_string(),
                Presence::Removed => "only in the old file".to_string(),
                Presence::Both => {
                    let mut parts = Vec::new();
                    if report.text_changed {
                        parts.push("text changed".to_string());
                    }
                    if report.visual_changed {
                        parts.push(format!("{:.2}% of the page looks different", report.changed_ratio * 100.0));
                    }
                    parts.join(", ")
                }
            };
            text_report += &format!("=== Page {page}: {what}\n");
            if want_text && report.text_changed {
                text_report += &unified(&old_lines, &new_lines);
            } else if want_text && presence == Presence::Both {
                text_report += "(the text is the same)\n";
            }
            text_report.push('\n');
        }
        reports.push(report);
    }
    ctx.check()?;
    ctx.report(0.93, "Writing the report");

    let changed: Vec<usize> = reports.iter().filter(|r| r.text_changed || r.visual_changed || r.presence != Presence::Both).map(|r| r.page).collect();
    let identical = changed.is_empty() && (pages_a == pages_b || !whole_document);
    let report = Report { pages_a, pages_b, identical, pages: reports };

    let verdict = if identical {
        match opts.mode {
            Mode::Text => "No differences found in the text.".to_string(),
            _ if whole_document => "The two files are identical.".to_string(),
            _ => "No differences found on the pages compared.".to_string(),
        }
    } else if changed.is_empty() {
        format!("The files have a different number of pages ({pages_a} and {pages_b}).")
    } else if changed.len() == 1 {
        format!("Page {} differs.", changed[0])
    } else {
        format!("{} pages differ: {}.", changed.len(), list(&changed))
    };

    let mut outcome = Outcome { pages: selected.len(), bytes_in, ..Default::default() };
    let json_path = out.join("report.json");
    let json = serde_json::to_vec_pretty(&report).map_err(|e| Error::other(format!("Could not write the report: {e}")))?;
    doc::write_file(&json_path, &json)?;
    outcome.push(json_path);

    let header = format!(
        "Comparison\nOld: {}.pdf ({} pages)\nNew: {}.pdf ({} pages)\nPages compared: {}\nMode: {}\n\n{}\n\n",
        stem(path_a),
        pages_a,
        stem(path_b),
        pages_b,
        if whole_document { "all".to_string() } else { list(&selected) },
        match opts.mode {
            Mode::Text => "text",
            Mode::Visual => "visual",
            Mode::Both => "text and visual",
        },
        verdict
    );
    let txt_path = out.join("report.txt");
    doc::write_file(&txt_path, (header + &text_report).as_bytes())?;
    outcome.push(txt_path);

    if !pictures.is_empty() {
        let (mut pdf, root) = doc::new_document();
        let mut page_ids = Vec::with_capacity(pictures.len());
        for (n, picture) in pictures.into_iter().enumerate() {
            ctx.check()?;
            ctx.report(0.94 + 0.05 * n as f32 / changed.len().max(1) as f32, &format!("Saving the picture for page {}", picture.page));
            let png = out.join(format!("diff-p{}.png", picture.page));
            picture.image.save(&png).map_err(|e| Error::other(format!("Could not write {}: {e}", png.display())))?;
            outcome.push(png);
            let image_id = img::add_image(&mut pdf, &DynamicImage::ImageRgb8(picture.image), img::Encoding::Flate)?;
            let page_id = doc::blank_page(&mut pdf, picture.size.0, picture.size.1);
            let name = doc::ensure_xobject(&mut pdf, page_id, image_id)?;
            doc::overlay(&mut pdf, page_id, img::draw(&name, 0.0, 0.0, picture.size.0, picture.size.1).into_bytes(), false)?;
            page_ids.push(page_id);
        }
        doc::append_pages(&mut pdf, root, &page_ids)?;
        doc::set_info(&mut pdf, "Title", "Differences")?;
        let pdf_path = out.join("diff.pdf");
        doc::save(&mut pdf, &pdf_path)?;
        outcome.push(pdf_path);
    }

    outcome.notes.push(verdict);
    if pages_a != pages_b && !changed.is_empty() {
        outcome.notes.push(format!("The old file has {pages_a} pages and the new one has {pages_b}."));
    }
    ctx.report(1.0, "Done");
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn text_changes_split_into_added_and_removed() {
        let old = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        let new = vec!["one".to_string(), "2".to_string(), "three".to_string(), "four".to_string()];
        let (added, removed) = text_changes(&old, &new);
        assert_eq!(added, vec!["2", "four"]);
        assert_eq!(removed, vec!["two"]);
        let text = unified(&old, &new);
        assert!(text.contains("-two") && text.contains("+2") && text.contains("+four"), "{text}");
    }

    #[test]
    fn added_ink_is_green_and_removed_ink_is_red() {
        let white = Rgba([255, 255, 255, 255]);
        let black = Rgba([0, 0, 0, 255]);
        let mut old = RgbaImage::from_pixel(4, 1, white);
        let mut new = RgbaImage::from_pixel(4, 1, white);
        old.put_pixel(0, 0, black); // removed
        new.put_pixel(1, 0, black); // added
        old.put_pixel(2, 0, black); // unchanged ink
        new.put_pixel(2, 0, black);
        let (image, ratio) = visual_diff(Some(&old), Some(&new), 24);
        assert!((ratio - 0.5).abs() < 1e-6);
        let removed = image.get_pixel(0, 0).0;
        let added = image.get_pixel(1, 0).0;
        assert!(removed[0] > removed[1] + 60 && removed[0] > removed[2] + 60, "{removed:?}");
        assert!(added[1] > added[0] + 60 && added[1] > added[2] + 40, "{added:?}");
        let same = image.get_pixel(2, 0).0;
        assert!(same[0] == same[1] && same[1] == same[2]);
        assert_eq!(image.get_pixel(3, 0).0, [255, 255, 255]);
        // Identical inputs: nothing changes.
        assert_eq!(visual_diff(Some(&new), Some(&new), 0).1, 0.0);
    }
}
