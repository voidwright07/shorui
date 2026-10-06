//! Redact: Permanently remove marked content.
//!
//! Covering text with a black box leaves the text in the file. This tool does not do
//! that. Every page that carries a mark is rendered to an image, the marks are painted
//! onto the image, and the page is replaced by that image alone: no text, no fonts, no
//! annotations, none of its old resources. Pages without marks are left exactly as they
//! are. Places outside the pages that can repeat page text (document information, XMP,
//! bookmark titles, form values, accessibility tags) are cleaned as well.

use super::edit;
use super::flatten::{self, single_input};
use crate::doc::{self, StdFont, fmt};
use crate::img::{self, Encoding};
use crate::render::{self, Renderer};
use crate::text::{Line, Rect4, TextReader};
use crate::{Ctx, Error, Outcome, Result};
use image::{DynamicImage, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// A rectangle to remove, drawn by the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Area {
    /// 1-based page number.
    pub page: usize,
    /// In display space: points, origin at the top-left of the page as shown, y down.
    pub rect: Rect4,
}

impl Default for Area {
    fn default() -> Self {
        Area { page: 1, rect: Rect4 { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 } }
    }
}

/// Text to find and remove wherever it occurs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Search {
    pub text: String,
    pub match_case: bool,
    pub whole_words: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub areas: Vec<Area>,
    pub search: Vec<Search>,
    /// Kinds of data to find and remove: "email", "phone", "iban", "date", "card".
    pub patterns: Vec<String>,
    /// Resolution of the image that replaces a redacted page.
    pub dpi: u32,
    /// Colour of the marks.
    pub color: [u8; 3],
    /// Each mark grows by this many points on every side.
    pub padding: f32,
    /// Clear the document information and XMP metadata, which can repeat removed text.
    pub strip_metadata: bool,
    /// Text printed on each mark, such as "REDACTED".
    pub label: Option<String>,
    /// 1 to 100.
    pub jpeg_quality: u8,
}

impl Default for Options {
    fn default() -> Self {
        Options { areas: Vec::new(), search: Vec::new(), patterns: Vec::new(), dpi: 200, color: [0, 0, 0], padding: 1.0, strip_metadata: true, label: None, jpeg_quality: 85 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Area,
    Search,
    Pattern,
}

/// One rectangle that will be removed.
#[derive(Debug, Clone, Serialize)]
pub struct Mark {
    /// 1-based page number.
    pub page: usize,
    /// What will be painted over, padding included, in display space.
    pub rect: Rect4,
    pub source: Source,
    /// The text that matched. Empty for an area.
    pub label: String,
    /// The pattern that matched ("email", "phone", ...). Empty unless `source` is `pattern`.
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Email,
    Phone,
    Iban,
    Date,
    Card,
}

impl Pattern {
    fn parse(name: &str) -> Result<Pattern> {
        match name.trim().to_ascii_lowercase().as_str() {
            "email" => Ok(Pattern::Email),
            "phone" => Ok(Pattern::Phone),
            "iban" => Ok(Pattern::Iban),
            "date" => Ok(Pattern::Date),
            "card" => Ok(Pattern::Card),
            other => Err(Error::invalid(format!("\"{other}\" is not a pattern this tool knows. Use email, phone, iban, date or card."))),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Pattern::Email => "email",
            Pattern::Phone => "phone",
            Pattern::Iban => "iban",
            Pattern::Date => "date",
            Pattern::Card => "card",
        }
    }

    /// Character ranges of every match in `c`.
    fn spans(self, c: &[char]) -> Vec<(usize, usize)> {
        match self {
            Pattern::Email => emails(c),
            Pattern::Phone => phones(c),
            Pattern::Iban => ibans(c),
            Pattern::Date => dates(c),
            Pattern::Card => cards(c),
        }
    }
}

/// The marks `run` would apply: the given areas, every occurrence of the search texts,
/// and every match of the patterns, in page order. The UI uses this to preview marks and
/// to offer suggestions before anything is removed. Text inside comments and form fields
/// is searched too; such a match marks the whole comment or field.
pub fn find_marks(path: &Path, password: Option<&str>, opts: &Options) -> Result<Vec<Mark>> {
    find_marks_in(&doc::read_file(path)?, password, opts, &Ctx::none())
}

fn needles(opts: &Options) -> Vec<Search> {
    opts.search
        .iter()
        .map(|s| Search { text: s.text.split_whitespace().collect::<Vec<_>>().join(" "), ..s.clone() })
        .filter(|s| !s.text.is_empty())
        .collect()
}

fn find_marks_in(bytes: &[u8], password: Option<&str>, opts: &Options, ctx: &Ctx) -> Result<Vec<Mark>> {
    let patterns: Vec<Pattern> = opts.patterns.iter().filter(|p| !p.trim().is_empty()).map(|p| Pattern::parse(p)).collect::<Result<_>>()?;
    let needles = needles(opts);
    let padding = if opts.padding.is_finite() { opts.padding.clamp(0.0, 72.0) } else { 0.0 };
    let reader = TextReader::open(bytes.to_vec(), password)?;
    let count = reader.page_count();
    let mut sizes: Vec<Option<(f32, f32)>> = vec![None; count];
    let mut marks: Vec<Mark> = Vec::new();
    let add = |marks: &mut Vec<Mark>, size: (f32, f32), page: usize, rect: Rect4, source: Source, label: &str, pattern: &str| {
        let r = rect.grow(padding);
        let r = Rect4 { x0: r.x0.max(0.0), y0: r.y0.max(0.0), x1: r.x1.min(size.0), y1: r.y1.min(size.1) };
        if r.x1 - r.x0 > 0.01 && r.y1 - r.y0 > 0.01 {
            marks.push(Mark { page, rect: r, source, label: label.to_string(), pattern: pattern.to_string() });
        }
    };

    for (n, area) in opts.areas.iter().enumerate() {
        if area.page == 0 || area.page > count {
            return Err(Error::invalid(format!("Area {} is on page {}, but this file has {}.", n + 1, area.page, flatten::plural(count, "page"))));
        }
        let r = area.rect;
        if ![r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()) {
            return Err(Error::invalid(format!("Area {} has a position that is not a number.", n + 1)));
        }
        let r = Rect4::new(r.x0, r.y0, r.x1, r.y1);
        if r.width() <= 0.0 || r.height() <= 0.0 {
            return Err(Error::invalid(format!("Area {} is empty. Drag out a rectangle with some width and height.", n + 1)));
        }
        let size = match sizes[area.page - 1] {
            Some(size) => size,
            None => {
                let page = reader.page(area.page - 1)?;
                sizes[area.page - 1] = Some((page.width, page.height));
                (page.width, page.height)
            }
        };
        add(&mut marks, size, area.page, r, Source::Area, "", "");
    }

    if !needles.is_empty() || !patterns.is_empty() {
        let doc = doc::load_bytes(bytes, password)?;
        let ids = doc::page_ids(&doc);
        for index in 0..count {
            ctx.check()?;
            ctx.report(0.15 * index as f32 / count.max(1) as f32, &format!("Searching page {} of {count}", index + 1));
            let page = reader.page(index)?;
            let size = (page.width, page.height);
            for s in &needles {
                for rect in page.find(&s.text, s.match_case, s.whole_words) {
                    add(&mut marks, size, index + 1, rect, Source::Search, &s.text, "");
                }
            }
            if !patterns.is_empty() {
                for line in &page.lines {
                    let cells = cells(line);
                    let chars: Vec<char> = cells.iter().map(|c| c.0).collect();
                    let frame = frame_of(line);
                    for pattern in &patterns {
                        for (start, end) in pattern.spans(&chars) {
                            let rect = from_frame(line.dir, Rect4::new(cells[start].1, frame.y0, cells[end - 1].2, frame.y1));
                            let matched: String = chars[start..end].iter().collect();
                            add(&mut marks, size, index + 1, rect, Source::Pattern, &matched, pattern.name());
                        }
                    }
                }
            }
            // Comments and form fields hold text that is not part of the page content.
            let Some(&pid) = ids.get(index) else { continue };
            for (_, annot) in flatten::page_annots(&doc, pid) {
                let Some(rect) = annot.get(b"Rect").ok().and_then(|r| doc::rect_of(&doc, r)) else { continue };
                let rect = flatten::page_rect_to_display(&doc, pid, rect);
                for text in annot_texts(&doc, &annot) {
                    for s in &needles {
                        if contains(&text, s) {
                            add(&mut marks, size, index + 1, rect, Source::Search, &s.text, "");
                        }
                    }
                    let chars: Vec<char> = text.chars().collect();
                    for pattern in &patterns {
                        for (start, end) in pattern.spans(&chars) {
                            let matched: String = chars[start..end].iter().collect();
                            add(&mut marks, size, index + 1, rect, Source::Pattern, &matched, pattern.name());
                        }
                    }
                }
            }
        }
    }
    marks.sort_by(|a, b| {
        (a.page, a.rect.y0, a.rect.x0).partial_cmp(&(b.page, b.rect.y0, b.rect.x0)).unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(marks)
}

/// The text an annotation carries: its contents and, for a form field, its value.
fn annot_texts(doc: &Document, annot: &Dictionary) -> Vec<String> {
    let mut out = Vec::new();
    for key in [&b"Contents"[..], b"RC", b"TU"] {
        if let Ok(o) = annot.get(key) {
            let text = flatten::decode_text(doc::deref(doc, o));
            if !text.is_empty() {
                out.push(text);
            }
        }
    }
    let mut node = Some(annot.clone());
    for _ in 0..16 {
        let Some(dict) = node else { break };
        if let Ok(v) = dict.get(b"V") {
            match doc::deref(doc, v) {
                Object::Array(items) => out.extend(items.iter().map(|o| flatten::decode_text(doc::deref(doc, o))).filter(|t| !t.is_empty())),
                other => {
                    let text = flatten::decode_text(other);
                    if !text.is_empty() {
                        out.push(text);
                    }
                }
            }
            break;
        }
        node = dict.get(b"Parent").and_then(Object::as_reference).ok().and_then(|id| doc.get_dictionary(id).ok()).cloned();
    }
    out
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = single_input(inputs)?;
    if opts.areas.is_empty() && needles(opts).is_empty() && opts.patterns.iter().all(|p| p.trim().is_empty()) {
        return Err(Error::invalid("Nothing is marked for removal yet. Draw an area, type text to find, or pick a pattern."));
    }
    ctx.report(0.0, "Opening the file");
    let bytes = doc::read_file(input)?;
    let marks = find_marks_in(&bytes, ctx.password(), opts, ctx)?;
    if marks.is_empty() {
        return Err(Error::invalid(
            "Nothing to redact was found in this file, so no file was written. Check the text you searched for. Scanned pages have no text to search: draw areas on them instead.",
        ));
    }
    let mut by_page: BTreeMap<usize, Vec<Rect4>> = BTreeMap::new();
    for mark in &marks {
        by_page.entry(mark.page).or_default().push(mark.rect);
    }

    let mut doc = doc::load_bytes(&bytes, ctx.password())?;
    let ids = doc::page_ids(&doc);
    if by_page.keys().any(|page| *page > ids.len()) {
        return Err(Error::damaged("The pages of this file could not be read consistently, so it was not redacted. Repair it first."));
    }
    let dpi = opts.dpi.clamp(72, 600);
    let scale = render::dpi_to_scale(dpi as f32);

    // Check boxes, radio buttons and fields without a drawn value are not part of what
    // the rasteriser shows. Flattening a copy first makes the picture match the page.
    let has_annots = by_page.keys().any(|page| ids.get(page - 1).is_some_and(|pid| !flatten::page_annots(&doc, *pid).is_empty()));
    let renderer = if has_annots {
        let mut copy = doc.clone();
        flatten::flatten_doc(&mut copy, &flatten::Options { forms: true, annotations: true, keep_links: true }, &Ctx::none())?;
        Renderer::open(doc::to_bytes(&mut copy)?, None)?
    } else {
        Renderer::open(bytes, ctx.password())?
    };
    if renderer.page_count() != ids.len() {
        return Err(Error::damaged("The pages of this file could not be read consistently, so it was not redacted. Repair it first."));
    }

    // Labels are set as text on a separate sheet, rendered, and copied into the marks.
    let label = opts.label.as_deref().map(str::trim).filter(|l| !l.is_empty());
    let label_sheet = match label {
        Some(label) => {
            let pages: Vec<(f32, f32, &Vec<Rect4>)> = by_page.iter().map(|(page, rects)| (renderer.page_size(page - 1).unwrap_or((612.0, 792.0)), rects)).map(|((w, h), rects)| (w, h, rects)).collect();
            Some(Renderer::open(label_layer(&pages, label, opts.color)?, None)?)
        }
        None => None,
    };

    let mut removed_annots: HashSet<ObjectId> = HashSet::new();
    for (done, (&page, rects)) in by_page.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.15 + 0.7 * done as f32 / by_page.len() as f32, &format!("Removing content from page {page} of {}", ids.len()));
        let index = page - 1;
        let pid = ids[index];
        let (pw, ph) = renderer.page_size(index).ok_or_else(|| Error::other("A page could not be measured."))?;
        let mut picture: RgbImage = DynamicImage::ImageRgba8(renderer.render(index, scale)?).to_rgb8();
        let (iw, ih) = picture.dimensions();
        let (sx, sy) = (iw as f32 / pw.max(1.0), ih as f32 / ph.max(1.0));
        let labels = match &label_sheet {
            Some(sheet) => Some(DynamicImage::ImageRgba8(sheet.render(done, sx)?).to_rgb8()),
            None => None,
        };
        for r in rects {
            let x0 = (r.x0 * sx).floor().clamp(0.0, iw as f32) as u32;
            let x1 = (r.x1 * sx).ceil().clamp(0.0, iw as f32) as u32;
            let y0 = (r.y0 * sy).floor().clamp(0.0, ih as f32) as u32;
            let y1 = (r.y1 * sy).ceil().clamp(0.0, ih as f32) as u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let pixel = match &labels {
                        Some(l) if x < l.width() && y < l.height() => *l.get_pixel(x, y),
                        _ => image::Rgb(opts.color),
                    };
                    picture.put_pixel(x, y, pixel);
                }
            }
        }
        let grey = opts.color[0] == opts.color[1] && opts.color[1] == opts.color[2] && picture.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]);
        let picture = if grey { DynamicImage::ImageLuma8(DynamicImage::ImageRgb8(picture).to_luma8()) } else { DynamicImage::ImageRgb8(picture) };
        let image_id = img::add_image(&mut doc, &picture, Encoding::Jpeg(opts.jpeg_quality.clamp(1, 100)))?;

        // A new page under the old page's object number, so bookmarks and links that
        // point at the page still work. Nothing of the old page is carried over.
        let (vw, vh) = doc::visible_size(&doc, pid);
        removed_annots.extend(flatten::page_annots(&doc, pid).into_iter().filter_map(|(id, _)| id));
        let old = doc.get_dictionary(pid)?.clone();
        let page_box = || vec![0.0f32.into(), 0.0f32.into(), Object::Real(vw), Object::Real(vh)];
        let mut fresh = dictionary! { "Type" => "Page", "MediaBox" => page_box(), "CropBox" => page_box(), "Rotate" => 0, "Resources" => Dictionary::new() };
        for key in [&b"Parent"[..], b"UserUnit"] {
            if let Ok(v) = old.get(key) {
                fresh.set(key.to_vec(), v.clone());
            }
        }
        doc.objects.insert(pid, Object::Dictionary(fresh));
        let name = doc::ensure_xobject(&mut doc, pid, image_id)?;
        let content = doc.add_object(Stream::new(Dictionary::new(), img::draw(&name, 0.0, 0.0, vw, vh).into_bytes()));
        doc.get_dictionary_mut(pid)?.set("Contents", content);
    }

    ctx.check()?;
    ctx.report(0.88, "Cleaning up what the pages left behind");
    let mut notes: Vec<String> = Vec::new();
    notes.push(format!(
        "{} converted to {} at {dpi} dpi so the removed content cannot be recovered; text on {} is no longer selectable.",
        if by_page.len() == 1 { "1 page was".to_string() } else { format!("{} pages were", by_page.len()) },
        if by_page.len() == 1 { "an image" } else { "images" },
        if by_page.len() == 1 { "that page" } else { "those pages" },
    ));
    notes.push(format!("{} applied.", if marks.len() == 1 { "1 mark was".to_string() } else { format!("{} marks were", marks.len()) }));

    for id in &removed_annots {
        doc.objects.remove(id);
    }
    drop_fields(&mut doc, &removed_annots);
    let patterns: Vec<Pattern> = opts.patterns.iter().filter_map(|p| Pattern::parse(p).ok()).collect();
    let needles = needles(opts);
    let renamed = scrub_outlines(&mut doc, &needles, &patterns);
    if renamed > 0 {
        notes.push(format!("{} contained removed text and {} renamed.", if renamed == 1 { "1 bookmark".to_string() } else { format!("{renamed} bookmarks") }, if renamed == 1 { "was" } else { "were" }));
    }
    let mut attachments = false;
    if let Ok(catalog) = doc.catalog_mut() {
        if catalog.remove(b"StructTreeRoot").is_some() {
            catalog.remove(b"MarkInfo");
            notes.push("Accessibility tags were removed because they can repeat the removed text.".into());
        }
        if opts.strip_metadata {
            catalog.remove(b"Metadata");
            catalog.remove(b"PieceInfo");
        }
    }
    if let Ok(catalog) = doc.catalog() {
        attachments = catalog.get(b"Names").ok().and_then(|n| doc::deref(&doc, n).as_dict().ok()).is_some_and(|names| names.has(b"EmbeddedFiles"));
    }
    if opts.strip_metadata {
        match doc.trailer.get(b"Info").and_then(Object::as_reference) {
            Ok(id) => {
                doc.objects.insert(id, Object::Dictionary(Dictionary::new()));
            }
            Err(_) => {
                doc.trailer.remove(b"Info");
            }
        }
        notes.push("Document information and metadata were cleared.".into());
    }
    flatten::prune(&mut doc);

    let left = residue(&doc, &needles);
    for text in &left {
        notes.push(format!("\"{text}\" still appears in this file outside the page text, for example in a link, a comment or the document information. Check the result before sharing it."));
    }
    if attachments {
        notes.push("This file has attachments. They were not examined and may contain the removed text.".into());
    }

    ctx.check()?;
    ctx.report(0.92, "Saving");
    doc::save(&mut doc, out)?;

    // Read the result back: the redacted pages must hold no text, and no page may still
    // contain a search text.
    ctx.report(0.96, "Checking the result");
    let verified = (|| -> Result<bool> {
        let reader = TextReader::open_path(out, None)?;
        for page in by_page.keys() {
            if !reader.page(page - 1)?.is_empty() {
                return Ok(false);
            }
        }
        for index in 0..reader.page_count() {
            let page = reader.page(index)?;
            if needles.iter().any(|s| !page.find(&s.text, s.match_case, s.whole_words).is_empty()) {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    if !matches!(verified, Ok(true)) {
        let _ = std::fs::remove_file(out);
        return Err(Error::other("The result still contained text that should have been removed, so no file was kept. Please report this file; it is not safe to redact with this tool."));
    }

    let mut outcome = Outcome::single(out.to_path_buf(), ids.len(), bytes_len(input));
    outcome.notes = notes;
    flatten::unlocked_note(&doc, &mut outcome);
    ctx.report(1.0, "Done");
    Ok(outcome)
}

fn bytes_len(path: &Path) -> u64 {
    crate::ctx::file_size(path)
}

/// A PDF with one page per redacted page, filled with the mark colour and carrying the
/// label centred in each mark. Rendered and copied into the marks, so the label ends up
/// as pixels, not as text.
fn label_layer(pages: &[(f32, f32, &Vec<Rect4>)], label: &str, color: [u8; 3]) -> Result<Vec<u8>> {
    const FONT: StdFont = StdFont::HelveticaBold;
    let (mut d, root) = doc::new_document();
    let luminance = (0.299 * color[0] as f32 + 0.587 * color[1] as f32 + 0.114 * color[2] as f32) / 255.0;
    let ink = if luminance > 0.6 { [0, 0, 0] } else { [255, 255, 255] };
    let unit = FONT.width(label, 1.0).max(0.01);
    let mut ids = Vec::new();
    for (w, h, rects) in pages {
        let page = doc::blank_page(&mut d, *w, *h);
        let font = doc::ensure_font(&mut d, page, FONT)?;
        let mut c = format!("{}0 0 {} {} re f\n{}", edit::fill_rgb(color), fmt(*w), fmt(*h), edit::fill_rgb(ink));
        for r in rects.iter() {
            let v = r.to_visible(*h);
            let size = ((v[3] - v[1]) * 0.62).min((v[2] - v[0]) * 0.9 / unit).min(14.0);
            if size < 3.0 {
                continue;
            }
            let x = (v[0] + v[2]) / 2.0 - unit * size / 2.0;
            let y = (v[1] + v[3]) / 2.0 - edit::ascent(FONT) * size / 2.0;
            c += &format!("BT /{font} {} Tf 1 0 0 1 {} {} Tm {} Tj ET\n", fmt(size), fmt(x), fmt(y), doc::pdf_string(label));
        }
        doc::overlay(&mut d, page, c.into_bytes(), false)?;
        ids.push(page);
    }
    doc::append_pages(&mut d, root, &ids)?;
    doc::to_bytes(&mut d)
}

/// Take the form fields whose widgets sat on redacted pages out of the form, so their
/// values do not stay behind in the file. The XFA copy of the form data goes too.
fn drop_fields(doc: &mut Document, removed: &HashSet<ObjectId>) {
    fn keep(doc: &mut Document, id: ObjectId, removed: &HashSet<ObjectId>, depth: usize) -> bool {
        if removed.contains(&id) || depth > 32 {
            return false;
        }
        let kids: Option<Vec<ObjectId>> = match doc.get_dictionary(id) {
            Ok(dict) => dict.get(b"Kids").ok().and_then(|k| doc::deref(doc, k).as_array().ok()).map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect()),
            Err(_) => return false,
        };
        let Some(kids) = kids else { return true };
        if kids.is_empty() {
            return true;
        }
        let kept: Vec<Object> = kids.into_iter().filter(|kid| keep(doc, *kid, removed, depth + 1)).map(Object::Reference).collect();
        if kept.is_empty() {
            return false;
        }
        if let Ok(dict) = doc.get_dictionary_mut(id) {
            dict.set("Kids", kept);
        }
        true
    }
    let Some(acro) = flatten::acroform(doc) else { return };
    let roots: Vec<ObjectId> = acro.get(b"Fields").ok().and_then(|f| doc::deref(doc, f).as_array().ok()).map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect()).unwrap_or_default();
    let kept: Vec<Object> = roots.into_iter().filter(|id| keep(doc, *id, removed, 0)).map(Object::Reference).collect();
    if kept.is_empty() {
        if let Ok(catalog) = doc.catalog_mut() {
            catalog.remove(b"AcroForm");
        }
    } else if let Some(acro) = flatten::acroform_mut(doc) {
        acro.set("Fields", kept);
        acro.remove(b"XFA");
    }
}

/// Rename bookmarks whose titles contain a search text or match a pattern. Returns how many.
fn scrub_outlines(doc: &mut Document, needles: &[Search], patterns: &[Pattern]) -> usize {
    let Some(root) = doc.catalog().ok().and_then(|c| c.get(b"Outlines").ok()).and_then(|o| o.as_reference().ok()) else { return 0 };
    let mut stack = vec![root];
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut renamed = 0;
    while let Some(id) = stack.pop() {
        if !seen.insert(id) || seen.len() > 100_000 {
            continue;
        }
        let Ok(dict) = doc.get_dictionary(id) else { continue };
        for key in [&b"First"[..], b"Next"] {
            if let Ok(next) = dict.get(key).and_then(Object::as_reference) {
                stack.push(next);
            }
        }
        let Ok(title) = dict.get(b"Title") else { continue };
        let title = flatten::decode_text(doc::deref(doc, title));
        let chars: Vec<char> = title.chars().collect();
        if needles.iter().any(|s| contains(&title, s)) || patterns.iter().any(|p| !p.spans(&chars).is_empty()) {
            if let Ok(dict) = doc.get_dictionary_mut(id) {
                dict.set("Title", Object::string_literal("[redacted]"));
                renamed += 1;
            }
        }
    }
    renamed
}

/// Search texts that can still be found somewhere in the document's objects.
fn residue(doc: &Document, needles: &[Search]) -> Vec<String> {
    fn strings(obj: &Object, out: &mut Vec<String>) {
        match obj {
            Object::String(..) => out.push(flatten::decode_text(obj)),
            Object::Array(items) => items.iter().for_each(|o| strings(o, out)),
            Object::Dictionary(d) => d.iter().for_each(|(_, o)| strings(o, out)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, o)| strings(o, out)),
            _ => {}
        }
    }
    let mut found: Vec<String> = Vec::new();
    if needles.is_empty() {
        return found;
    }
    let note = |s: &Search, found: &mut Vec<String>| {
        if !found.contains(&s.text) {
            found.push(s.text.clone());
        }
    };
    for obj in doc.objects.values().chain(doc.trailer.iter().map(|(_, v)| v)) {
        let mut texts = Vec::new();
        strings(obj, &mut texts);
        for s in needles {
            if texts.iter().any(|t| contains(t, s)) {
                note(s, &mut found);
            }
        }
        if let Object::Stream(stream) = obj {
            if stream.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Image") {
                continue;
            }
            let content = stream.get_plain_content().unwrap_or_else(|_| stream.content.clone());
            for s in needles {
                if s.text.is_ascii() && bytes_contain(&content, s) {
                    note(s, &mut found);
                }
            }
        }
    }
    found
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Whether `hay` contains the search text, honouring its case and whole-word settings.
fn contains(hay: &str, s: &Search) -> bool {
    let fold = |c: char| if s.match_case { c } else { lower(c) };
    let hay: Vec<char> = hay.chars().map(fold).collect();
    let needle: Vec<char> = s.text.chars().map(fold).collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    (0..=hay.len() - needle.len()).any(|i| {
        hay[i..i + needle.len()] == needle[..]
            && (!s.whole_words || ((i == 0 || !hay[i - 1].is_alphanumeric()) && (i + needle.len() == hay.len() || !hay[i + needle.len()].is_alphanumeric())))
    })
}

fn bytes_contain(hay: &[u8], s: &Search) -> bool {
    let needle = s.text.as_bytes();
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    (0..=hay.len() - needle.len()).any(|i| {
        let window = &hay[i..i + needle.len()];
        (if s.match_case { window == needle } else { window.eq_ignore_ascii_case(needle) })
            && (!s.whole_words || ((i == 0 || !hay[i - 1].is_ascii_alphanumeric()) && (i + needle.len() == hay.len() || !hay[i + needle.len()].is_ascii_alphanumeric())))
    })
}

// ---------------------------------------------------------------------------
// Lines as characters with positions
// ---------------------------------------------------------------------------
//
// `text::Line` keeps its reading frame private, so the two small rotations below repeat
// what `text.rs` does to go between display space and the frame a line is read in.

fn to_frame(dir: u8, x: f32, y: f32) -> (f32, f32) {
    match dir {
        1 => (y, -x),
        2 => (-x, -y),
        3 => (-y, x),
        _ => (x, y),
    }
}

fn from_frame(dir: u8, r: Rect4) -> Rect4 {
    let p = |x: f32, y: f32| match dir {
        1 => (-y, x),
        2 => (-x, -y),
        3 => (y, -x),
        _ => (x, y),
    };
    let (ax, ay) = p(r.x0, r.y0);
    let (bx, by) = p(r.x1, r.y1);
    Rect4::new(ax, ay, bx, by)
}

fn frame_of(line: &Line) -> Rect4 {
    let (ax, ay) = to_frame(line.dir, line.rect.x0, line.rect.y0);
    let (bx, by) = to_frame(line.dir, line.rect.x1, line.rect.y1);
    Rect4::new(ax, ay, bx, by)
}

/// One entry per character of the line's text (words joined by single spaces), with the
/// horizontal extent it covers in the line's reading frame.
fn cells(line: &Line) -> Vec<(char, f32, f32)> {
    let mut cells = Vec::new();
    for (i, word) in line.words.iter().enumerate() {
        if i > 0 {
            let prev_end = line.words[i - 1].chars.last().map(|c| c.x1).unwrap_or(0.0);
            let next_start = word.chars.first().map(|c| c.x0).unwrap_or(prev_end);
            cells.push((' ', prev_end, next_start));
        }
        for ch in &word.chars {
            for c in ch.text.chars() {
                cells.push((c, ch.x0, ch.x1));
            }
        }
    }
    cells
}

// ---------------------------------------------------------------------------
// Patterns
// ---------------------------------------------------------------------------

fn boundary_before(c: &[char], i: usize) -> bool {
    i == 0 || !c[i - 1].is_alphanumeric()
}

fn boundary_after(c: &[char], end: usize) -> bool {
    end >= c.len() || !c[end].is_alphanumeric()
}

fn emails(c: &[char]) -> Vec<(usize, usize)> {
    let local = |ch: char| ch.is_ascii_alphanumeric() || "._%+-".contains(ch);
    let domain = |ch: char| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-';
    let mut out = Vec::new();
    for at in (0..c.len()).filter(|i| c[*i] == '@') {
        let mut start = at;
        while start > 0 && local(c[start - 1]) {
            start -= 1;
        }
        while start < at && c[start] == '.' {
            start += 1;
        }
        let mut end = at + 1;
        while end < c.len() && domain(c[end]) {
            end += 1;
        }
        while end > at + 1 && (c[end - 1] == '.' || c[end - 1] == '-') {
            end -= 1;
        }
        if start == at || end <= at + 1 {
            continue;
        }
        let host = &c[at + 1..end];
        let Some(dot) = host.iter().rposition(|ch| *ch == '.') else { continue };
        let tld = &host[dot + 1..];
        if dot == 0 || tld.len() < 2 || !tld.iter().all(|ch| ch.is_ascii_alphabetic()) {
            continue;
        }
        out.push((start, end));
    }
    out
}

/// Digits starting at `i`, at most `max` of them. Returns the value and the end position.
fn number(c: &[char], i: usize, max: usize) -> Option<(u32, usize)> {
    let mut end = i;
    let mut value = 0u32;
    while end < c.len() && end - i < max && c[end].is_ascii_digit() {
        value = value * 10 + c[end].to_digit(10).unwrap_or(0);
        end += 1;
    }
    (end > i).then_some((value, end))
}

const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

/// A month name or its three-letter form at `i`. Returns the end position.
fn month(c: &[char], i: usize) -> Option<usize> {
    let mut end = i;
    while end < c.len() && c[end].is_alphabetic() {
        end += 1;
    }
    let word: String = c[i..end].iter().map(|ch| lower(*ch)).collect();
    let known = MONTHS.iter().any(|m| *m == word || (word.len() == 3 && m.starts_with(&word)) || word == "sept");
    if !known {
        return None;
    }
    Some(if end < c.len() && c[end] == '.' && word.len() <= 4 { end + 1 } else { end })
}

fn year(c: &[char], i: usize) -> Option<usize> {
    let (value, end) = number(c, i, 4)?;
    (end - i == 4 && (1000..=2999).contains(&value) && boundary_after(c, end)).then_some(end)
}

fn skip(c: &[char], i: usize, ch: char) -> usize {
    if i < c.len() && c[i] == ch { i + 1 } else { i }
}

/// A day of the month at `i`, with an optional ordinal ending. Returns the end position.
fn day(c: &[char], i: usize) -> Option<usize> {
    let (value, mut end) = number(c, i, 2)?;
    if !(1..=31).contains(&value) {
        return None;
    }
    if end + 2 <= c.len() {
        let suffix: String = c[end..end + 2].iter().map(|ch| lower(*ch)).collect();
        if matches!(suffix.as_str(), "st" | "nd" | "rd" | "th") {
            end += 2;
        }
    }
    Some(end)
}

fn date_at(c: &[char], i: usize) -> Option<usize> {
    // 2026-10-02, 02/10/2026, 2.10.26
    if let Some((a, e1)) = number(c, i, 4) {
        if e1 < c.len() && matches!(c[e1], '-' | '/' | '.') {
            let sep = c[e1];
            if let Some((b, e2)) = number(c, e1 + 1, 2) {
                if e2 < c.len() && c[e2] == sep {
                    if let Some((d, e3)) = number(c, e2 + 1, 4) {
                        let (la, ld) = (e1 - i, e3 - e2 - 1);
                        let year_first = la == 4 && ld <= 2 && (1000..=2999).contains(&a) && (1..=12).contains(&b) && (1..=31).contains(&d);
                        let year_last = la <= 2 && (ld == 4 || ld == 2) && a >= 1 && b >= 1 && ((a <= 31 && b <= 12) || (a <= 12 && b <= 31)) && (ld == 2 || (1000..=2999).contains(&d));
                        let clean_end = boundary_after(c, e3) && !(e3 < c.len() && matches!(c[e3], '-' | '/') && e3 + 1 < c.len() && c[e3 + 1].is_ascii_digit());
                        if (year_first || year_last) && clean_end {
                            return Some(e3);
                        }
                    }
                }
            }
        }
    }
    // 2 October 2026, 2nd Oct. 2026
    if let Some(e) = day(c, i) {
        if e < c.len() && c[e] == ' ' {
            if let Some(e) = month(c, e + 1) {
                let e = skip(c, skip(c, e, ','), ' ');
                if let Some(e) = year(c, e) {
                    return Some(e);
                }
            }
        }
    }
    // October 2, 2026 and October 2026
    if let Some(e) = month(c, i) {
        if e < c.len() && c[e] == ' ' {
            if let Some(e) = year(c, e + 1) {
                return Some(e);
            }
            if let Some(e) = day(c, e + 1) {
                let e = skip(c, skip(c, e, ','), ' ');
                if let Some(e) = year(c, e) {
                    return Some(e);
                }
            }
        }
    }
    None
}

fn dates(c: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match date_at(c, i).filter(|_| boundary_before(c, i)) {
            Some(end) => {
                out.push((i, end));
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

/// A run of digits with single separators between groups, starting at `i`. Returns the
/// end position (after the last digit) and the length of each group of digits.
fn digit_groups(c: &[char], i: usize, separators: &[char]) -> (usize, Vec<usize>) {
    let mut groups = Vec::new();
    let mut end = i;
    let mut j = i;
    loop {
        let start = j;
        while j < c.len() && c[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            break;
        }
        groups.push(j - start);
        end = j;
        if j + 1 < c.len() && separators.contains(&c[j]) && c[j + 1].is_ascii_digit() {
            j += 1;
        } else {
            break;
        }
    }
    (end, groups)
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, d)| if i % 2 == 1 { if d * 2 > 9 { d * 2 - 9 } else { d * 2 } } else { *d })
        .sum();
    sum % 10 == 0
}

fn cards(c: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if !c[i].is_ascii_digit() || !boundary_before(c, i) {
            i += 1;
            continue;
        }
        let (end, groups) = digit_groups(c, i, &[' ', '-']);
        let total: usize = groups.iter().sum();
        let shaped = groups.len() == 1 || matches!(groups.as_slice(), [4, 4, 4, 4] | [4, 4, 4, 4, 3] | [4, 6, 5] | [4, 6, 4]);
        let digits: Vec<u32> = c[i..end].iter().filter_map(|ch| ch.to_digit(10)).collect();
        if (13..=19).contains(&total) && shaped && boundary_after(c, end) && luhn(&digits) {
            out.push((i, end));
        }
        i = end.max(i + 1);
    }
    out
}

fn iban_valid(compact: &[char]) -> bool {
    if !(15..=34).contains(&compact.len()) {
        return false;
    }
    let mut remainder = 0u32;
    for ch in compact[4..].iter().chain(&compact[..4]) {
        let value = match ch.to_digit(36) {
            Some(v) => v,
            None => return false,
        };
        remainder = if value >= 10 { (remainder * 100 + value) % 97 } else { (remainder * 10 + value) % 97 };
    }
    remainder == 1
}

fn ibans(c: &[char]) -> Vec<(usize, usize)> {
    let part = |ch: char| ch.is_ascii_uppercase() || ch.is_ascii_digit();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 4 <= c.len() {
        let head = c[i].is_ascii_uppercase() && c[i + 1].is_ascii_uppercase() && c[i + 2].is_ascii_digit() && c[i + 3].is_ascii_digit();
        if !head || !boundary_before(c, i) {
            i += 1;
            continue;
        }
        // Walk on over letters and digits, allowing single spaces, and remember every
        // place the number could end. The longest one that passes the checksum wins.
        let mut compact: Vec<char> = Vec::new();
        let mut ends: Vec<(usize, usize)> = Vec::new();
        let mut j = i;
        while j < c.len() && compact.len() < 34 {
            if part(c[j]) {
                compact.push(c[j]);
                j += 1;
                if boundary_after(c, j) {
                    ends.push((j, compact.len()));
                }
            } else if c[j] == ' ' && j + 1 < c.len() && part(c[j + 1]) {
                j += 1;
            } else {
                break;
            }
        }
        match ends.iter().rev().find(|(_, len)| iban_valid(&compact[..*len])) {
            Some((end, _)) => {
                out.push((i, *end));
                i = *end;
            }
            None => i += 1,
        }
    }
    out
}

fn phones(c: &[char]) -> Vec<(usize, usize)> {
    let separator = |ch: char| matches!(ch, ' ' | '-' | '.' | '(' | ')' | '/');
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let opens = c[i] == '+' || c[i] == '(' || c[i].is_ascii_digit();
        if !opens || !boundary_before(c, i) || (i > 0 && matches!(c[i - 1], '+' | '-' | '.' | '/' | '@' | '#')) {
            i += 1;
            continue;
        }
        // Take digits with at most two separator characters between them.
        let mut j = if c[i] == '+' { i + 1 } else { i };
        let mut end = i;
        let mut pending = 0;
        while j < c.len() {
            if c[j].is_ascii_digit() {
                pending = 0;
                j += 1;
                end = j;
            } else if separator(c[j]) && pending < 2 {
                pending += 1;
                j += 1;
            } else {
                break;
            }
        }
        if end <= i {
            i += 1;
            continue;
        }
        let run = &c[i..end];
        let digits = run.iter().filter(|ch| ch.is_ascii_digit()).count();
        let groups = run.split(|ch| !ch.is_ascii_digit()).filter(|g| !g.is_empty()).count();
        let dots = run.iter().filter(|ch| **ch == '.').count();
        let decimal = dots == 1 && groups == 2 && run.iter().all(|ch| ch.is_ascii_digit() || *ch == '.');
        let dated = date_at(c, i).is_some_and(|e| e >= end);
        let accepted = boundary_after(c, end)
            && !decimal
            && !dated
            && if c[i] == '+' {
                (8..=15).contains(&digits)
            } else if c[i] == '(' {
                (7..=15).contains(&digits)
            } else if c[i] == '0' {
                (9..=12).contains(&digits)
            } else {
                (10..=11).contains(&digits) && groups >= 3
            };
        if accepted {
            out.push((i, end));
        }
        // A rejected run is skipped whole: its tail is not a phone number either.
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: Pattern, text: &str) -> Vec<String> {
        let c: Vec<char> = text.chars().collect();
        pattern.spans(&c).into_iter().map(|(a, b)| c[a..b].iter().collect()).collect()
    }

    #[test]
    fn finds_emails() {
        assert_eq!(matches(Pattern::Email, "Notices are sent to mara.lindqvist@example.com or by phone."), ["mara.lindqvist@example.com"]);
        assert_eq!(matches(Pattern::Email, "A copy was sent to accounts@example.org on 2026-10-02."), ["accounts@example.org"]);
        assert_eq!(matches(Pattern::Email, "a@b, @example.com, x@y.z1, first.last+tag@sub.domain.co.uk."), ["first.last+tag@sub.domain.co.uk"]);
    }

    #[test]
    fn finds_phones() {
        assert_eq!(matches(Pattern::Phone, "or by phone on +44 7700 900142."), ["+44 7700 900142"]);
        assert_eq!(matches(Pattern::Phone, "Call (020) 7946 0958 or 07700 900142 or 555-123-4567."), ["(020) 7946 0958", "07700 900142", "555-123-4567"]);
        assert!(matches(Pattern::Phone, "Revenue 177560.00 on 2026-10-02, ref 1204, units 4439").is_empty());
        assert!(matches(Pattern::Phone, "account GB29 NWBK 6016 1331 9268 19.").is_empty());
        assert!(matches(Pattern::Phone, "The Client lives at 14 Alder Row, Leeds LS6 2QT.").is_empty());
    }

    #[test]
    fn finds_ibans() {
        assert_eq!(matches(Pattern::Iban, "Payments are collected from account GB29 NWBK 6016 1331 9268 19."), ["GB29 NWBK 6016 1331 9268 19"]);
        assert_eq!(matches(Pattern::Iban, "IBAN DE89370400440532013000 and more TEXT"), ["DE89370400440532013000"]);
        assert!(matches(Pattern::Iban, "GB29 NWBK 6016 1331 9268 18").is_empty());
    }

    #[test]
    fn finds_dates() {
        assert_eq!(matches(Pattern::Date, "sent on 2026-10-02."), ["2026-10-02"]);
        assert_eq!(matches(Pattern::Date, "On 2 October 2026, and October 2, 2026; also 02/10/2026 and Oct 2026."), ["2 October 2026", "October 2, 2026", "02/10/2026", "Oct 2026"]);
        assert!(matches(Pattern::Date, "The Supplier may rely on version 1.2.3 and 48160.00, part 12-34").is_empty());
    }

    #[test]
    fn finds_cards() {
        assert_eq!(matches(Pattern::Card, "Card 4111 1111 1111 1111 exp"), ["4111 1111 1111 1111"]);
        assert_eq!(matches(Pattern::Card, "Amex 3782-822463-10005."), ["3782-822463-10005"]);
        assert!(matches(Pattern::Card, "4111 1111 1111 1112 and GB29 NWBK 6016 1331 9268 19").is_empty());
    }

    #[test]
    fn plain_search_honours_options() {
        let s = |text: &str, match_case, whole_words| Search { text: text.into(), match_case, whole_words };
        assert!(contains("Signed by Mara Lindqvist", &s("mara lindqvist", false, false)));
        assert!(!contains("Signed by Mara Lindqvist", &s("mara lindqvist", true, false)));
        assert!(!contains("Maracas", &s("Mara", true, true)));
        assert!(bytes_contain(b"(Mara Lindqvist) Tj", &s("mara", false, true)));
        assert!(!bytes_contain(b"(Maracas) Tj", &s("mara", false, true)));
    }

    #[test]
    fn frames_round_trip() {
        for dir in 0..4u8 {
            let r = Rect4::new(10.0, 20.0, 110.0, 32.0);
            let (ax, ay) = to_frame(dir, r.x0, r.y0);
            let (bx, by) = to_frame(dir, r.x1, r.y1);
            assert_eq!(from_frame(dir, Rect4::new(ax, ay, bx, by)), r);
        }
    }
}
