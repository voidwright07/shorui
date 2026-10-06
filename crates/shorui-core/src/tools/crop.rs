//! Crop & Resize: Trim page margins or change the page size.

use super::merge::{self, UNPROTECTED_NOTE, plural};
use crate::ctx::file_size;
use crate::render::Renderer;
use crate::{Ctx, Error, Outcome, Result, doc, range};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Trim (or, with negative numbers, add) a fixed amount on each side.
    #[default]
    Margins,
    /// Trim to what is actually printed on each page.
    Auto,
    /// Change the paper size and scale the page onto it.
    Resize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    /// Keep the proportions and show the whole page, centred.
    #[default]
    Fit,
    /// Keep the proportions and cover the new page; what does not fit is cut off.
    Fill,
    /// Stretch to the new page exactly.
    Stretch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub mode: Mode,
    /// Points to trim from each side as the reader sees the page. Negative adds space.
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    /// Auto mode: space to leave around the content, in points.
    pub auto_padding: f32,
    /// Auto mode: give every chosen page the same crop (the one that fits all of them),
    /// so pages of one size stay one size.
    pub uniform: bool,
    /// Resize mode: `a3`, `a4`, `a5`, `letter`, `legal`, ... or `WxH` in points
    /// (`420x595`; `210x297mm` and `8.5x11in` also work).
    pub paper: String,
    /// Resize mode: how the old page maps onto the new size.
    pub fit: Fit,
    /// Resize mode: turn a named paper size sideways for pages that are wider than tall.
    pub keep_orientation: bool,
    /// Which pages to change. Empty means all.
    pub pages: String,
    /// Crop modes: also shrink the page's media box, so other programs cannot simply
    /// widen the view again. The trimmed content is still stored in the file.
    pub remove_hidden: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: Mode::Margins,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
            auto_padding: 10.0,
            uniform: false,
            paper: "a4".into(),
            fit: Fit::Fit,
            keep_orientation: true,
            pages: String::new(),
            remove_hidden: false,
        }
    }
}

const MM: f32 = 72.0 / 25.4;

/// Width and height in points (portrait for named sizes) and whether the size was
/// given by name. Accepts `a0`-`a6`, `b4`, `b5`, `letter`, `legal`, `tabloid`, `ledger`,
/// `executive`, `statement`, or `WxH` in points, optionally ending in `mm`, `cm`, `in` or `pt`.
pub fn paper_size(spec: &str) -> Result<((f32, f32), bool)> {
    let s: String = spec.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let mm = |w: f32, h: f32| ((w * MM, h * MM), true);
    let named = match s.as_str() {
        "a0" => Some(mm(841.0, 1189.0)),
        "a1" => Some(mm(594.0, 841.0)),
        "a2" => Some(mm(420.0, 594.0)),
        "a3" => Some(mm(297.0, 420.0)),
        "a4" | "" => Some(mm(210.0, 297.0)),
        "a5" => Some(mm(148.0, 210.0)),
        "a6" => Some(mm(105.0, 148.0)),
        "b4" => Some(mm(250.0, 353.0)),
        "b5" => Some(mm(176.0, 250.0)),
        "letter" => Some(((612.0, 792.0), true)),
        "legal" => Some(((612.0, 1008.0), true)),
        "tabloid" | "ledger" => Some(((792.0, 1224.0), true)),
        "executive" => Some(((522.0, 756.0), true)),
        "statement" | "half-letter" => Some(((396.0, 612.0), true)),
        _ => None,
    };
    if let Some(found) = named {
        return Ok(found);
    }
    let bad = || Error::invalid(format!("\"{}\" is not a paper size. Use a name such as a4 or letter, or a size in points such as 420x595.", spec.trim()));
    let (body, unit) = if let Some(b) = s.strip_suffix("mm") {
        (b, MM)
    } else if let Some(b) = s.strip_suffix("cm") {
        (b, MM * 10.0)
    } else if let Some(b) = s.strip_suffix("in") {
        (b, 72.0)
    } else if let Some(b) = s.strip_suffix("pt") {
        (b, 1.0)
    } else {
        (s.as_str(), 1.0)
    };
    let (w, h) = body.split_once(['x', '×', '*']).ok_or_else(bad)?;
    let w: f32 = w.parse().map_err(|_| bad())?;
    let h: f32 = h.parse().map_err(|_| bad())?;
    let (w, h) = (w * unit, h * unit);
    if !(w.is_finite() && h.is_finite()) || w < 10.0 || h < 10.0 || w > 14_400.0 || h > 14_400.0 {
        return Err(Error::invalid("The paper size must be between 10 and 14400 points on each side."));
    }
    Ok(((w, h), false))
}

// ----- small matrix helpers ([a b c d e f], points are row vectors) -----

/// The matrix that applies `a` first and then `b`.
pub(crate) fn then(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub(crate) fn apply(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// The bounding box of a rectangle after a transform.
fn map_rect(m: [f32; 6], r: [f32; 4]) -> [f32; 4] {
    let corners = [apply(m, r[0], r[1]), apply(m, r[2], r[1]), apply(m, r[0], r[3]), apply(m, r[2], r[3])];
    let xs = corners.iter().map(|c| c.0);
    let ys = corners.iter().map(|c| c.1);
    [
        xs.clone().fold(f32::INFINITY, f32::min),
        ys.clone().fold(f32::INFINITY, f32::min),
        xs.fold(f32::NEG_INFINITY, f32::max),
        ys.fold(f32::NEG_INFINITY, f32::max),
    ]
}

fn rect_object(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[2] - r[0] > 0.5 && r[3] - r[1] > 0.5).then_some(r)
}

/// Show only `visible` (a rectangle in the page's current visible space) of a page.
fn set_visible_area(doc: &mut Document, page: ObjectId, visible: [f32; 4], remove_hidden: bool) -> Result<()> {
    let to_page = doc::visible_to_page(doc, page);
    let rect = map_rect(to_page, visible);
    doc::flatten_inherited(doc, page)?;
    let media = doc::media_box(doc, page);
    let grown = [media[0].min(rect[0]), media[1].min(rect[1]), media[2].max(rect[2]), media[3].max(rect[3])];
    let other: Vec<(&[u8], Option<[f32; 4]>)> = [&b"TrimBox"[..], b"BleedBox", b"ArtBox"]
        .into_iter()
        .map(|key| (key, doc.get_dictionary(page).ok().and_then(|d| d.get(key).ok()).and_then(|o| doc::rect_of(doc, o))))
        .collect();
    let dict = doc.get_dictionary_mut(page)?;
    dict.set("MediaBox", rect_object(if remove_hidden { rect } else { grown }));
    dict.set("CropBox", rect_object(rect));
    for (key, value) in other {
        match value.and_then(|v| intersect(v, rect)) {
            Some(clipped) => dict.set(key.to_vec(), rect_object(clipped)),
            None => {
                dict.remove(key);
            }
        }
    }
    Ok(())
}

/// The box around everything that is not white, in visible space, or `None` for a blank page.
fn content_box(image: &image::RgbaImage, visible: (f32, f32)) -> Option<[f32; 4]> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut found = false;
    for (y, row) in image.as_raw().chunks_exact(w * 4).enumerate() {
        let inked = |px: &[u8]| px.iter().take(3).any(|c| *c < 245);
        let Some(first) = row.chunks_exact(4).position(inked) else { continue };
        let last = row.chunks_exact(4).rposition(inked).unwrap_or(first);
        found = true;
        min_x = min_x.min(first);
        max_x = max_x.max(last);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    if !found {
        return None;
    }
    let (sx, sy) = (visible.0 / w as f32, visible.1 / h as f32);
    Some([min_x as f32 * sx, visible.1 - (max_y + 1) as f32 * sy, (max_x + 1) as f32 * sx, visible.1 - min_y as f32 * sy])
}

/// For the app's preview: the box auto crop would keep on one page (0-based index), in
/// visible space (points, origin bottom-left), before padding. `None` for a blank page.
pub fn detect_content_box(renderer: &Renderer, index: usize) -> Result<Option<[f32; 4]>> {
    let size = renderer.page_size(index).ok_or_else(|| Error::invalid(format!("Page {} does not exist.", index + 1)))?;
    let scale = (2000.0 / size.0.max(size.1)).min(2.0);
    let image = renderer.render(index, scale)?;
    Ok(content_box(&image, size))
}

fn numbers(doc: &Document, obj: &Object) -> Option<Vec<f32>> {
    let items = doc::deref(doc, obj).as_array().ok()?;
    items.iter().map(|o| doc::as_f32(doc::deref(doc, o))).collect()
}

fn map_points(m: [f32; 6], values: &[f32]) -> Object {
    let mut out = Vec::with_capacity(values.len());
    for pair in values.chunks(2) {
        match pair {
            [x, y] => {
                let (nx, ny) = apply(m, *x, *y);
                out.push(Object::Real(nx));
                out.push(Object::Real(ny));
            }
            rest => out.extend(rest.iter().map(|v| Object::Real(*v))),
        }
    }
    Object::Array(out)
}

/// Move an annotation with its page: its rectangle and any point lists it carries.
fn transform_annotation(doc: &mut Document, id: ObjectId, m: [f32; 6]) {
    let Ok(dict) = doc.get_dictionary(id) else { return };
    let mut changes: Vec<(&[u8], Object)> = Vec::new();
    if let Some(r) = dict.get(b"Rect").ok().and_then(|o| doc::rect_of(doc, o)) {
        changes.push((b"Rect", rect_object(map_rect(m, r))));
    }
    for key in [&b"QuadPoints"[..], b"Vertices", b"L", b"CL"] {
        if let Some(values) = dict.get(key).ok().and_then(|o| numbers(doc, o)) {
            changes.push((key, map_points(m, &values)));
        }
    }
    if let Ok(Object::Array(strokes)) = dict.get(b"InkList").map(|o| doc::deref(doc, o)) {
        let mapped: Vec<Object> = strokes.iter().filter_map(|s| numbers(doc, s)).map(|v| map_points(m, &v)).collect();
        if mapped.len() == strokes.len() {
            changes.push((b"InkList", Object::Array(mapped)));
        }
    }
    if let Ok(dict) = doc.get_dictionary_mut(id) {
        for (key, value) in changes {
            dict.set(key.to_vec(), value);
        }
    }
}

/// Put one page onto a new paper size. Returns how many annotations moved with it.
fn resize_page(doc: &mut Document, page: ObjectId, paper: (f32, f32), named: bool, opts: &Options, moved: &mut HashSet<ObjectId>) -> Result<usize> {
    let old_to_visible = doc::page_to_visible(doc, page);
    let rotation = doc::rotation(doc, page);
    let (xobject, (w, h)) = doc::page_as_xobject(doc, page)?;
    if w <= 0.0 || h <= 0.0 {
        return Ok(0);
    }
    let (mut tw, mut th) = paper;
    if named && opts.keep_orientation && (w > h) != (tw > th) {
        std::mem::swap(&mut tw, &mut th);
    }
    let (sx, sy) = match opts.fit {
        Fit::Fit => {
            let s = (tw / w).min(th / h);
            (s, s)
        }
        Fit::Fill => {
            let s = (tw / w).max(th / h);
            (s, s)
        }
        Fit::Stretch => (tw / w, th / h),
    };
    let place = [sx, 0.0, 0.0, sy, (tw - w * sx) / 2.0, (th - h * sy) / 2.0];

    // The page keeps its /Rotate, so the media box is the new size as stored, unrotated.
    let stored = if rotation % 180 == 0 { [0.0, 0.0, tw, th] } else { [0.0, 0.0, th, tw] };
    {
        let dict = doc.get_dictionary_mut(page)?;
        dict.set("MediaBox", rect_object(stored));
        dict.set("Rotate", rotation as i64);
        dict.set("Resources", Dictionary::new());
        for key in [&b"CropBox"[..], b"TrimBox", b"BleedBox", b"ArtBox"] {
            dict.remove(key);
        }
    }
    let new_to_page = doc::visible_to_page(doc, page);
    let name = doc::ensure_xobject(doc, page, xobject)?;
    let content = format!("q\n{}{}/{name} Do\nQ\n", doc::cm(new_to_page), doc::cm(place));
    let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
    doc.get_dictionary_mut(page)?.set("Contents", content_id);

    // Old page space -> old visible space -> placed on the new page -> new page space.
    let annotation_move = then(then(old_to_visible, place), new_to_page);
    let mut count = 0;
    for id in merge::annot_ids(doc, page) {
        if moved.insert(id) {
            transform_annotation(doc, id, annotation_move);
            count += 1;
        }
    }
    Ok(count)
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let tool = if opts.mode == Mode::Resize { "Resize" } else { "Crop" };
    let input = merge::one_input(inputs, tool)?;
    merge::guard_output(inputs, out)?;
    ctx.report(0.0, "Reading the file");
    let bytes = doc::read_file(input)?;
    let (mut document, password) = super::unlock::load_bytes(&bytes, ctx.password())?;
    let ids = doc::page_ids(&document);
    if ids.is_empty() {
        return Err(Error::damaged("This file has no pages."));
    }
    let mut chosen: Vec<usize> = range::parse(&opts.pages, ids.len())?;
    chosen.sort_unstable();
    chosen.dedup();
    let total = chosen.len();
    let mut notes: Vec<String> = Vec::new();

    match opts.mode {
        Mode::Margins => {
            for value in [opts.top, opts.right, opts.bottom, opts.left] {
                if !value.is_finite() {
                    return Err(Error::invalid("The margins must be numbers."));
                }
            }
            for (i, &number) in chosen.iter().enumerate() {
                ctx.check()?;
                ctx.report(0.1 + 0.7 * i as f32 / total as f32, &format!("Cropping page {} of {total}", i + 1));
                let Some(&page) = ids.get(number - 1) else { continue };
                let (w, h) = doc::visible_size(&document, page);
                let area = [opts.left, opts.bottom, w - opts.right, h - opts.top];
                if area[2] - area[0] < 3.0 || area[3] - area[1] < 3.0 {
                    return Err(Error::invalid(format!(
                        "Nothing would be left of page {number}: it is {:.0} x {:.0} points and the margins take away more than that.",
                        w, h
                    )));
                }
                set_visible_area(&mut document, page, area, opts.remove_hidden)?;
            }
        }
        Mode::Auto => {
            if !opts.auto_padding.is_finite() {
                return Err(Error::invalid("The padding must be a number."));
            }
            let renderer = Renderer::open(bytes, password.as_deref())?;
            if renderer.page_count() != ids.len() {
                return Err(Error::damaged("The pages of this file could not be read consistently, so their content could not be measured. Try Repair first."));
            }
            let mut boxes: Vec<(usize, ObjectId, [f32; 4])> = Vec::new();
            let mut blank = 0usize;
            for (i, &number) in chosen.iter().enumerate() {
                ctx.check()?;
                ctx.report(0.05 + 0.8 * i as f32 / total as f32, &format!("Measuring page {} of {total}", i + 1));
                let Some(&page) = ids.get(number - 1) else { continue };
                let size = doc::visible_size(&document, page);
                let scale = (2000.0 / size.0.max(size.1)).min(2.0);
                let image = renderer.render(number - 1, scale)?;
                match content_box(&image, size) {
                    Some(found) => boxes.push((number, page, found)),
                    None => blank += 1,
                }
            }
            if opts.uniform {
                if let Some(union) = boxes.iter().map(|b| b.2).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]) {
                    for entry in &mut boxes {
                        entry.2 = union;
                    }
                }
            }
            let pad = opts.auto_padding;
            for (number, page, found) in boxes {
                let (w, h) = doc::visible_size(&document, page);
                let area = [(found[0] - pad).max(0.0), (found[1] - pad).max(0.0), (found[2] + pad).min(w), (found[3] + pad).min(h)];
                if area[2] - area[0] < 3.0 || area[3] - area[1] < 3.0 {
                    return Err(Error::invalid(format!("Nothing would be left of page {number} with that padding.")));
                }
                set_visible_area(&mut document, page, area, opts.remove_hidden)?;
            }
            if blank > 0 {
                notes.push(format!("{} blank and left as {}.", plural(blank, "page was", "pages were"), if blank == 1 { "it was" } else { "they were" }));
            }
        }
        Mode::Resize => {
            let (paper, named) = paper_size(&opts.paper)?;
            let mut moved: HashSet<ObjectId> = HashSet::new();
            let mut annotations = 0usize;
            for (i, &number) in chosen.iter().enumerate() {
                ctx.check()?;
                ctx.report(0.1 + 0.7 * i as f32 / total as f32, &format!("Resizing page {} of {total}", i + 1));
                let Some(&page) = ids.get(number - 1) else { continue };
                annotations += resize_page(&mut document, page, paper, named, opts, &mut moved)?;
            }
            if annotations > 0 {
                notes.push("Links, comments and form fields were moved and scaled with their pages. Text typed into a form field keeps its original size.".into());
            }
            merge::prune(&mut document);
        }
    }

    if opts.mode != Mode::Resize {
        let grows = opts.mode == Mode::Margins && [opts.top, opts.right, opts.bottom, opts.left].iter().all(|v| *v <= 0.0);
        if !grows {
            notes.push(if opts.remove_hidden {
                "The trimmed area is cut off for every program, but what was printed there is still stored in the file. Use Redact to delete it for good.".to_string()
            } else {
                "Cropped content is hidden, not deleted: it is still in the file and another program can show it again.".to_string()
            });
        }
    }
    if document.was_encrypted() {
        notes.push(UNPROTECTED_NOTE.into());
    }

    ctx.check()?;
    ctx.report(0.9, "Saving");
    doc::save(&mut document, out)?;
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), ids.len(), file_size(input));
    outcome.notes = notes;
    Ok(outcome)
}
