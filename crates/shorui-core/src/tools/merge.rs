//! Merge: Combine several files into one PDF.
//!
//! This file also holds the plumbing the other organise tools share: carrying form
//! fields and bookmarks across `doc::import_pages`, rebuilding an outline, pruning
//! unreachable objects, and a few small guards.

use crate::ctx::file_size;
use crate::doc::{self, StdFont};
use crate::{Ctx, Error, Outcome, Result, helpers, range};
use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Bookmarks {
    /// One entry per source file, titled with the file name, pointing at its first page.
    #[default]
    PerFile,
    /// Carry over the bookmarks each file already has.
    Keep,
    /// No bookmarks.
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// One page range per input, in input order. Missing or empty means all pages.
    pub ranges: Vec<String>,
    pub bookmarks: Bookmarks,
    /// Put a page at the front listing each file and the page it starts on.
    pub contents_page: bool,
    /// When false, form fields are dropped (what they show is lost too unless flattened first).
    pub keep_form_fields: bool,
    /// Insert a blank page where needed so every file starts on an odd page.
    pub start_on_odd: bool,
    /// Title of the merged document. Overrides the title taken from the first file.
    pub title: Option<String>,
    /// Copy Title and Author from the first file.
    pub metadata_from_first: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            ranges: Vec::new(),
            bookmarks: Bookmarks::PerFile,
            contents_page: false,
            keep_form_fields: true,
            start_on_odd: false,
            title: None,
            metadata_from_first: true,
        }
    }
}

/// A rough size for the merged file before it is made: the sum of the input sizes.
/// The real result is usually a little smaller.
pub fn estimate_size(inputs: &[PathBuf]) -> u64 {
    inputs.iter().map(|p| file_size(p)).sum()
}

struct Part {
    name: String,
    pages: Vec<ObjectId>,
    outline: Vec<Node>,
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    if inputs.is_empty() {
        return Err(Error::invalid("Add at least one PDF to merge."));
    }
    guard_output(inputs, out)?;

    let (mut dst, root) = doc::new_document();
    let total = inputs.len();
    let mut parts: Vec<Part> = Vec::with_capacity(total);
    let mut forms: Vec<Dictionary> = Vec::new();
    let mut first_info: Vec<(&str, Object)> = Vec::new();
    let mut any_encrypted = false;
    let mut widgets_dropped = 0usize;

    for (i, path) in inputs.iter().enumerate() {
        ctx.check()?;
        let name = helpers::stem(path);
        ctx.report(0.9 * i as f32 / total as f32, &format!("Adding {name}, file {} of {total}", i + 1));
        let mut src = super::unlock::load(path, ctx.password()).map_err(|e| name_error(e, path))?;
        any_encrypted |= src.was_encrypted();
        let ids = doc::page_ids(&src);
        if ids.is_empty() {
            return Err(Error::damaged(format!("{} has no pages, so there is nothing to merge from it.", file_name(path))));
        }
        let spec = opts.ranges.get(i).map(String::as_str).unwrap_or("");
        let numbers = range::parse(spec, ids.len()).map_err(|e| name_error(e, path))?;
        let chosen: Vec<ObjectId> = numbers.iter().filter_map(|n| ids.get(n.wrapping_sub(1)).copied()).collect();
        if i == 0 && opts.metadata_from_first {
            for key in ["Title", "Author"] {
                if let Some(v) = info_entry(&src, key) {
                    first_info.push((key, v));
                }
            }
        }
        if !opts.keep_form_fields {
            widgets_dropped += chosen.iter().collect::<HashSet<_>>().into_iter().map(|p| count_widgets(&src, *p)).sum::<usize>();
        }
        let imported = import(&mut dst, &mut src, &chosen, opts.keep_form_fields, opts.bookmarks == Bookmarks::Keep)?;
        let valid: HashSet<ObjectId> = imported.pages.iter().copied().collect();
        let outline = match imported.outline_root {
            Some(root_id) => read_outline(&dst, root_id, &valid, false),
            None => Vec::new(),
        };
        if let Some(form) = imported.form {
            forms.push(form);
        }
        parts.push(Part { name, pages: imported.pages, outline });
    }

    ctx.check()?;
    ctx.report(0.9, "Putting the pages together");

    // Work out where everything lands before any page is attached.
    let first_size = parts.first().and_then(|p| p.pages.first()).map(|id| doc::visible_size(&dst, *id)).unwrap_or(crate::fixtures::A4);
    let contents_size = if first_size.0 >= 360.0 && first_size.1 >= 360.0 { first_size } else { crate::fixtures::A4 };
    let per_page = contents_lines_per_page(contents_size);
    let contents_count = if opts.contents_page { parts.len().div_ceil(per_page).max(1) } else { 0 };
    let contents_pages: Vec<ObjectId> = (0..contents_count).map(|_| doc::blank_page(&mut dst, contents_size.0, contents_size.1)).collect();

    let mut all: Vec<ObjectId> = contents_pages.clone();
    let mut starts: Vec<usize> = Vec::with_capacity(parts.len());
    let mut blanks = 0usize;
    for part in &parts {
        if opts.start_on_odd && all.len() % 2 == 1 {
            let size = all.last().map(|id| doc::visible_size(&dst, *id)).unwrap_or(contents_size);
            all.push(doc::blank_page(&mut dst, size.0, size.1));
            blanks += 1;
        }
        starts.push(all.len() + 1);
        all.extend(part.pages.iter().copied());
    }

    if opts.contents_page {
        let entries: Vec<(String, usize, Option<ObjectId>)> =
            parts.iter().zip(&starts).map(|(p, s)| (p.name.clone(), *s, p.pages.first().copied())).collect();
        draw_contents(&mut dst, &contents_pages, contents_size, &entries, per_page)?;
    }
    doc::append_pages(&mut dst, root, &all)?;

    // Bookmarks.
    let mut nodes: Vec<Node> = Vec::new();
    match opts.bookmarks {
        Bookmarks::PerFile => {
            if let Some(first) = contents_pages.first() {
                nodes.push(new_bookmark(&mut dst, "Contents", *first));
            }
            for part in &parts {
                if let Some(first) = part.pages.first() {
                    nodes.push(new_bookmark(&mut dst, &part.name, *first));
                }
            }
        }
        Bookmarks::Keep => {
            for part in &mut parts {
                nodes.append(&mut part.outline);
            }
        }
        Bookmarks::None => {}
    }
    write_outline(&mut dst, &nodes)?;

    // Forms.
    let mut notes: Vec<String> = Vec::new();
    let form_report = install_forms(&mut dst, forms)?;
    if form_report.renamed > 0 {
        notes.push(format!(
            "{} had the same name in more than one file and {} renamed so each keeps its own value.",
            plural(form_report.renamed, "form field", "form fields"),
            if form_report.renamed == 1 { "was" } else { "were" }
        ));
    }
    if form_report.signed > 0 {
        notes.push("Digital signatures do not survive merging: the signed files are changed by being combined.".into());
    }
    if widgets_dropped > 0 {
        notes.push(format!("{} left out. Flatten a file first to keep what its fields show.", plural(widgets_dropped, "form field was", "form fields were")));
    }
    if blanks > 0 {
        notes.push(format!("{} added so each file starts on an odd page.", plural(blanks, "blank page was", "blank pages were")));
    }
    if any_encrypted {
        notes.push(UNPROTECTED_NOTE.into());
    }

    // Document information.
    for (key, value) in first_info {
        info_mut(&mut dst)?.set(key, value);
    }
    if let Some(title) = opts.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        info_mut(&mut dst)?.set("Title", text_string(title));
    }

    prune(&mut dst);
    ctx.check()?;
    ctx.report(0.95, "Saving");
    doc::save(&mut dst, out)?;
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), all.len(), estimate_size(inputs));
    outcome.notes = notes;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// The contents page
// ---------------------------------------------------------------------------

const CONTENTS_TOP: f32 = 112.0;
const CONTENTS_STEP: f32 = 20.0;
const CONTENTS_BOTTOM: f32 = 56.0;

fn contents_lines_per_page(size: (f32, f32)) -> usize {
    (((size.1 - CONTENTS_TOP - CONTENTS_BOTTOM) / CONTENTS_STEP).floor() as usize + 1).max(1)
}

/// Shorten `text` with an ellipsis until it fits in `max` points.
fn fit_text(text: &str, font: StdFont, size: f32, max: f32) -> String {
    if font.width(text, size) <= max {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.pop().is_some() {
        let candidate = format!("{}...", chars.iter().collect::<String>().trim_end());
        if font.width(&candidate, size) <= max {
            return candidate;
        }
    }
    "...".into()
}

fn draw_contents(
    dst: &mut Document, pages: &[ObjectId], size: (f32, f32), entries: &[(String, usize, Option<ObjectId>)], per_page: usize,
) -> Result<()> {
    let (w, h) = size;
    let margin = 56.0f32.min(w * 0.1);
    let body = 11.0;
    for (page, chunk) in pages.iter().zip(entries.chunks(per_page.max(1))) {
        let regular = doc::ensure_font(dst, *page, StdFont::Helvetica)?;
        let bold = doc::ensure_font(dst, *page, StdFont::HelveticaBold)?;
        let mut c = String::new();
        c += &format!("0 g BT /{bold} 20 Tf {} {} Td {} Tj ET\n", doc::fmt(margin), doc::fmt(h - 72.0), doc::pdf_string("Contents"));
        let mut links: Vec<Object> = Vec::new();
        // One column for the numbers, so the dot leaders of every row line up.
        let widest = chunk.iter().map(|(_, start, _)| StdFont::Helvetica.width(&start.to_string(), body)).fold(0.0f32, f32::max);
        for (row, (name, start, target)) in chunk.iter().enumerate() {
            let y = h - CONTENTS_TOP - row as f32 * CONTENTS_STEP;
            let number = start.to_string();
            let number_w = StdFont::Helvetica.width(&number, body);
            let room = (w - 2.0 * margin - widest - 24.0).max(40.0);
            let label = fit_text(name, StdFont::Helvetica, body, room);
            let label_w = StdFont::Helvetica.width(&label, body);
            c += &format!("BT /{regular} {} Tf {} {} Td {} Tj ET\n", doc::fmt(body), doc::fmt(margin), doc::fmt(y), doc::pdf_string(&label));
            let number_x = w - margin - number_w;
            c += &format!("BT /{regular} {} Tf {} {} Td {} Tj ET\n", doc::fmt(body), doc::fmt(number_x), doc::fmt(y), doc::pdf_string(&number));
            // Dot leaders between the name and the number.
            let unit = StdFont::Helvetica.width(" .", body);
            let (from, to) = (margin + label_w + 6.0, w - margin - widest - 6.0);
            let dots = ((to - from) / unit).floor();
            if dots >= 3.0 {
                let dots = dots as usize;
                c += &format!(
                    "0.55 g BT /{regular} {} Tf {} {} Td {} Tj ET 0 g\n",
                    doc::fmt(body),
                    doc::fmt(to - dots as f32 * unit),
                    doc::fmt(y),
                    doc::pdf_string(&" .".repeat(dots))
                );
            }
            if let Some(target) = target {
                links.push(Object::Reference(dst.add_object(dictionary! {
                    "Type" => "Annot",
                    "Subtype" => "Link",
                    "Rect" => vec![Object::Real(margin), Object::Real(y - 5.0), Object::Real(w - margin), Object::Real(y + 12.0)],
                    "Border" => vec![0.into(), 0.into(), 0.into()],
                    "Dest" => page_dest(*target),
                    "P" => *page,
                })));
            }
        }
        doc::overlay(dst, *page, c.into_bytes(), false)?;
        if !links.is_empty() {
            dst.get_dictionary_mut(*page)?.set("Annots", links);
        }
    }
    Ok(())
}

/// A destination that goes to a page without changing the reader's zoom.
fn page_dest(page: ObjectId) -> Object {
    Object::Array(vec![Object::Reference(page), "XYZ".into(), Object::Null, Object::Null, Object::Null])
}

fn new_bookmark(doc: &mut Document, title: &str, page: ObjectId) -> Node {
    let id = doc.add_object(dictionary! { "Title" => text_string(title), "Dest" => page_dest(page) });
    Node { id, children: Vec::new(), open: false, dead: false }
}

// ---------------------------------------------------------------------------
// Shared by the organise tools
// ---------------------------------------------------------------------------

pub(crate) const UNPROTECTED_NOTE: &str = "The original was password protected. The result is not: use Protect to add a password again.";

/// `1 page` or `3 pages`.
pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string())
}

/// Say which file a problem is in. Password errors keep their kind so the app can ask for one.
pub(crate) fn name_error(e: Error, path: &Path) -> Error {
    let name = file_name(path);
    match e {
        Error::Damaged(m) => Error::Damaged(format!("{name}: {m}")),
        Error::Invalid(m) => Error::Invalid(format!("{name}: {m}")),
        Error::Other(m) => Error::Other(format!("{name}: {m}")),
        other => other,
    }
}

/// The tools that take exactly one PDF.
pub(crate) fn one_input<'a>(inputs: &'a [PathBuf], tool: &str) -> Result<&'a Path> {
    match inputs {
        [one] => Ok(one.as_path()),
        [] => Err(Error::invalid(format!("Choose a PDF for {tool} to work on."))),
        _ => Err(Error::invalid(format!("{tool} works on one PDF at a time. Run it once for each file."))),
    }
}

/// Refuse to write on top of an input.
pub(crate) fn guard_output(inputs: &[PathBuf], out: &Path) -> Result<()> {
    let target = std::fs::canonicalize(out).ok();
    for input in inputs {
        if same_file(target.as_deref(), out, input) {
            return Err(Error::invalid("The result would overwrite a file you are working from. Choose a different name or folder for it."));
        }
    }
    Ok(())
}

pub(crate) fn same_file(canonical_a: Option<&Path>, a: &Path, b: &Path) -> bool {
    match (canonical_a, std::fs::canonicalize(b).ok()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// A PDF text string: plain when it is printable ASCII, UTF-16 otherwise.
pub(crate) fn text_string(text: &str) -> Object {
    if text.chars().all(|c| (' '..='~').contains(&c)) {
        return Object::string_literal(text);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, StringFormat::Hexadecimal)
}

pub(crate) fn decode_text(obj: &Object) -> String {
    match lopdf::decode_text_string(obj) {
        Ok(s) => s,
        Err(_) => obj.as_str().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default(),
    }
}

/// A string entry of the document information dictionary.
pub(crate) fn info_entry(doc: &Document, key: &str) -> Option<Object> {
    let info = doc::deref(doc, doc.trailer.get(b"Info").ok()?).as_dict().ok()?;
    match doc::deref(doc, info.get(key.as_bytes()).ok()?) {
        Object::String(bytes, format) if !bytes.is_empty() => Some(Object::String(bytes.clone(), *format)),
        _ => None,
    }
}

pub(crate) fn info_mut(doc: &mut Document) -> Result<&mut Dictionary> {
    let id = match doc.trailer.get(b"Info").and_then(Object::as_reference) {
        Ok(id) if doc.get_dictionary(id).is_ok() => id,
        _ => {
            let id = doc.add_object(Dictionary::new());
            doc.trailer.set("Info", id);
            id
        }
    };
    Ok(doc.get_dictionary_mut(id)?)
}

pub(crate) fn copy_info(dst: &mut Document, src: &Document, keys: &[&str]) -> Result<()> {
    for key in keys {
        if let Some(v) = info_entry(src, key) {
            info_mut(dst)?.set(*key, v);
        }
    }
    Ok(())
}

/// Drop every object that cannot be reached from the trailer.
pub(crate) fn prune(doc: &mut Document) {
    fn scan(obj: &Object, seen: &mut HashSet<ObjectId>, stack: &mut Vec<ObjectId>) {
        match obj {
            Object::Reference(id) => {
                if seen.insert(*id) {
                    stack.push(*id);
                }
            }
            Object::Array(items) => items.iter().for_each(|o| scan(o, seen, stack)),
            Object::Dictionary(dict) => dict.iter().for_each(|(_, o)| scan(o, seen, stack)),
            Object::Stream(stream) => stream.dict.iter().for_each(|(_, o)| scan(o, seen, stack)),
            _ => {}
        }
    }
    let mut seen = HashSet::new();
    let mut stack = Vec::new();
    for (_, v) in doc.trailer.iter() {
        scan(v, &mut seen, &mut stack);
    }
    while let Some(id) = stack.pop() {
        if let Some(obj) = doc.objects.get(&id) {
            scan(obj, &mut seen, &mut stack);
        }
    }
    doc.objects.retain(|id, _| seen.contains(id));
}

fn name_is(dict: &Dictionary, key: &[u8], value: &[u8]) -> bool {
    dict.get(key).and_then(Object::as_name).map(|n| n == value).unwrap_or(false)
}

/// The elements of a page's `/Annots`, whether the array is inline or referenced.
pub(crate) fn annots(doc: &Document, page: ObjectId) -> Vec<Object> {
    let Ok(dict) = doc.get_dictionary(page) else { return Vec::new() };
    match dict.get(b"Annots").map(|a| doc::deref(doc, a)) {
        Ok(Object::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

pub(crate) fn annot_ids(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    annots(doc, page).iter().filter_map(|o| o.as_reference().ok()).collect()
}

fn is_widget(doc: &Document, annot: &Object) -> bool {
    doc::deref(doc, annot).as_dict().map(|d| name_is(d, b"Subtype", b"Widget")).unwrap_or(false)
}

fn count_widgets(doc: &Document, page: ObjectId) -> usize {
    annots(doc, page).iter().filter(|a| is_widget(doc, a)).count()
}

const CARRY: &[u8] = b"ShoruiCarry";

/// What `import` brought across.
pub(crate) struct Imported {
    /// The copied pages, in the order asked for, not yet attached to a page tree.
    pub pages: Vec<ObjectId>,
    /// The source's form dictionary, with `/Fields` already narrowed to the fields that
    /// have a widget on a copied page. Hand it to `install_forms`.
    pub form: Option<Dictionary>,
    /// The copied outline root (raw: read it with `read_outline`).
    pub outline_root: Option<ObjectId>,
}

/// `doc::import_pages`, plus the document-level things that pages depend on but do not
/// point at: the form dictionary and, when asked, the outline. They ride along on the
/// first page so the copy shares one object map with the pages.
pub(crate) fn import(dst: &mut Document, src: &mut Document, pages: &[ObjectId], forms: bool, outlines: bool) -> Result<Imported> {
    let Some(&first) = pages.first() else {
        return Ok(Imported { pages: Vec::new(), form: None, outline_root: None });
    };

    if !forms {
        let unique: HashSet<ObjectId> = pages.iter().copied().collect();
        for page in unique {
            let all = annots(src, page);
            if all.is_empty() {
                continue;
            }
            let kept: Vec<Object> = all.iter().filter(|a| !is_widget(src, a)).cloned().collect();
            if kept.len() != all.len() {
                let dict = src.get_dictionary_mut(page)?;
                if kept.is_empty() {
                    dict.remove(b"Annots");
                } else {
                    dict.set("Annots", kept);
                }
            }
        }
    }

    let mut carry = Dictionary::new();
    if forms {
        let form = src.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).map(|o| doc::deref(src, o)).and_then(|o| o.as_dict().ok()).cloned();
        if let Some(mut form) = form {
            // XFA describes the whole original form and calculation order names fields
            // that may not come along; neither is valid for a part of the document.
            for key in [&b"XFA"[..], b"CO", b"SigFlags"] {
                form.remove(key);
            }
            carry.set("AcroForm", Object::Dictionary(form));
        }
    }
    if outlines {
        if let Some(root) = src.catalog().ok().and_then(|c| c.get(b"Outlines").ok()).and_then(|o| o.as_reference().ok()) {
            prepare_outline(src, root);
            carry.set("Outlines", Object::Reference(root));
        }
    }
    if !carry.is_empty() {
        src.get_dictionary_mut(first)?.set(CARRY, Object::Dictionary(carry));
    }

    let imported = doc::import_pages(dst, src, pages);
    if let Ok(d) = src.get_dictionary_mut(first) {
        d.remove(CARRY);
    }
    let new_pages = imported?;

    let mut carried: Option<Dictionary> = None;
    for id in &new_pages {
        if let Ok(d) = dst.get_dictionary_mut(*id) {
            if let Some(Object::Dictionary(c)) = d.remove(CARRY) {
                carried.get_or_insert(c);
            }
        }
    }
    let mut form = None;
    let mut outline_root = None;
    if let Some(mut carried) = carried {
        if let Some(Object::Dictionary(f)) = carried.remove(b"AcroForm") {
            form = Some(f);
        }
        outline_root = carried.get(b"Outlines").ok().and_then(|o| o.as_reference().ok());
    }
    if forms {
        form = settle_form(dst, form, &new_pages);
    }
    Ok(Imported { pages: new_pages, form, outline_root })
}

// ----- forms -----

fn field_kids(doc: &Document, dict: &Dictionary) -> Option<Vec<ObjectId>> {
    match dict.get(b"Kids").map(|k| doc::deref(doc, k)) {
        Ok(Object::Array(items)) => Some(items.iter().filter_map(|o| o.as_reference().ok()).collect()),
        _ => None,
    }
}

/// Keep a field only when it, or something under it, has a widget on a page we hold.
fn settle_field(doc: &mut Document, id: ObjectId, on_page: &HashSet<ObjectId>, seen: &mut HashSet<ObjectId>, depth: usize) -> bool {
    if depth > 48 || !seen.insert(id) {
        return false;
    }
    let Ok(dict) = doc.get_dictionary(id) else { return false };
    match field_kids(doc, dict) {
        Some(kids) => {
            let kept: Vec<Object> = kids.into_iter().filter(|kid| settle_field(doc, *kid, on_page, seen, depth + 1)).map(Object::Reference).collect();
            let alive = !kept.is_empty();
            if let Ok(d) = doc.get_dictionary_mut(id) {
                d.set("Kids", kept);
            }
            alive
        }
        None => !name_is(dict, b"Subtype", b"Widget") || on_page.contains(&id),
    }
}

fn field_root(doc: &Document, id: ObjectId) -> ObjectId {
    let mut current = id;
    for _ in 0..48 {
        match doc.get_dictionary(current).ok().and_then(|d| d.get(b"Parent").ok()).and_then(|p| p.as_reference().ok()) {
            Some(parent) if doc.get_dictionary(parent).is_ok() => current = parent,
            _ => break,
        }
    }
    current
}

/// Narrow a copied form dictionary to the fields present on `pages`. Widgets that the
/// source did not list under `/Fields` are added so they keep working.
fn settle_form(doc: &mut Document, form: Option<Dictionary>, pages: &[ObjectId]) -> Option<Dictionary> {
    let on_page: HashSet<ObjectId> = pages.iter().flat_map(|p| annot_ids(doc, *p)).collect();
    let mut form = form.unwrap_or_default();
    let mut roots: Vec<ObjectId> = match form.get(b"Fields").map(|f| doc::deref(doc, f)) {
        Ok(Object::Array(items)) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => Vec::new(),
    };
    let mut listed: HashSet<ObjectId> = roots.iter().copied().collect();
    for page in pages {
        for id in annot_ids(doc, *page) {
            let Ok(d) = doc.get_dictionary(id) else { continue };
            if !name_is(d, b"Subtype", b"Widget") {
                continue;
            }
            let root = field_root(doc, id);
            let is_field = doc.get_dictionary(root).map(|r| r.has(b"T") || r.has(b"FT")).unwrap_or(false);
            if is_field && listed.insert(root) {
                roots.push(root);
            }
        }
    }
    let mut seen = HashSet::new();
    let kept: Vec<ObjectId> = roots.into_iter().filter(|id| settle_field(doc, *id, &on_page, &mut seen, 0)).collect();
    if kept.is_empty() {
        return None;
    }
    form.set("Fields", kept.into_iter().map(Object::Reference).collect::<Vec<_>>());
    Some(form)
}

#[derive(Default)]
pub(crate) struct FormReport {
    pub fields: usize,
    pub renamed: usize,
    pub signed: usize,
}

fn count_signed(doc: &Document, id: ObjectId, depth: usize) -> usize {
    let Ok(dict) = doc.get_dictionary(id) else { return 0 };
    let own = usize::from(name_is(dict, b"FT", b"Sig") && dict.has(b"V"));
    if depth > 48 {
        return own;
    }
    own + field_kids(doc, dict).unwrap_or_default().into_iter().map(|k| count_signed(doc, k, depth + 1)).sum::<usize>()
}

/// Combine the form dictionaries of the imported files into one `/AcroForm` on the
/// catalog. A top-level field whose name is already taken is renamed (`name_2`), because
/// two fields with one name are one field in PDF and would share a value.
pub(crate) fn install_forms(doc: &mut Document, forms: Vec<Dictionary>) -> Result<FormReport> {
    let mut report = FormReport::default();
    if forms.is_empty() {
        return Ok(report);
    }
    let mut fields: Vec<Object> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    let mut resources = Dictionary::new();
    let mut default_appearance: Option<Object> = None;
    let mut need_appearances = false;

    for form in &forms {
        let da = form.get(b"DA").ok().map(|o| doc::deref(doc, o).clone());
        let q = form.get(b"Q").ok().map(|o| doc::deref(doc, o).clone());
        if default_appearance.is_none() {
            default_appearance = da.clone();
        }
        need_appearances |= form.get(b"NeedAppearances").map(|o| doc::deref(doc, o).as_bool().unwrap_or(false)).unwrap_or(false);

        // Default resources: first file to define a name wins.
        if let Ok(Object::Dictionary(dr)) = form.get(b"DR").map(|o| doc::deref(doc, o).clone()) {
            for (category, value) in dr.iter() {
                match doc::deref(doc, value).clone() {
                    Object::Dictionary(entries) => {
                        if !matches!(resources.get(category), Ok(Object::Dictionary(_))) {
                            resources.set(category.clone(), Dictionary::new());
                        }
                        if let Ok(Object::Dictionary(target)) = resources.get_mut(category) {
                            for (name, v) in entries.iter() {
                                if !target.has(name) {
                                    target.set(name.clone(), v.clone());
                                }
                            }
                        }
                    }
                    other => {
                        if !resources.has(category) {
                            resources.set(category.clone(), other);
                        }
                    }
                }
            }
        }

        let roots: Vec<ObjectId> = match form.get(b"Fields") {
            Ok(Object::Array(items)) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => Vec::new(),
        };
        for id in roots {
            report.signed += count_signed(doc, id, 0);
            let name = doc.get_dictionary(id).ok().and_then(|d| d.get(b"T").ok()).map(decode_text);
            let dict = doc.get_dictionary_mut(id)?;
            // What the field used to inherit from its own document's form dictionary.
            if let (Some(da), false) = (&da, dict.has(b"DA")) {
                dict.set("DA", da.clone());
            }
            if let (Some(q), false) = (&q, dict.has(b"Q")) {
                dict.set("Q", q.clone());
            }
            if let Some(name) = name {
                if !used.insert(name.clone()) {
                    let mut n = 2;
                    let fresh = loop {
                        let candidate = format!("{name}_{n}");
                        if used.insert(candidate.clone()) {
                            break candidate;
                        }
                        n += 1;
                    };
                    dict.set("T", text_string(&fresh));
                    report.renamed += 1;
                }
            }
            fields.push(Object::Reference(id));
            report.fields += 1;
        }
    }
    if fields.is_empty() {
        return Ok(report);
    }
    let mut acro = dictionary! { "Fields" => fields };
    if let Some(da) = default_appearance {
        acro.set("DA", da);
    }
    if !resources.is_empty() {
        acro.set("DR", resources);
    }
    if need_appearances {
        acro.set("NeedAppearances", true);
    }
    let id = doc.add_object(acro);
    doc.catalog_mut()?.set("AcroForm", id);
    Ok(report)
}

/// After pages were removed from a document in place: drop the form fields that no
/// longer have a widget on any page.
pub(crate) fn tidy_form(doc: &mut Document) {
    let Some(form_obj) = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).cloned() else { return };
    let (form_id, form) = match &form_obj {
        Object::Reference(id) => (Some(*id), doc.get_dictionary(*id).ok().cloned()),
        Object::Dictionary(d) => (None, Some(d.clone())),
        _ => (None, None),
    };
    let Some(form) = form else { return };
    let pages = doc::page_ids(doc);
    let settled = settle_form(doc, Some(form), &pages);
    let Ok(catalog) = doc.catalog_mut() else { return };
    match (settled, form_id) {
        (Some(form), Some(id)) => {
            doc.objects.insert(id, Object::Dictionary(form));
        }
        (Some(form), None) => catalog.set("AcroForm", Object::Dictionary(form)),
        (None, _) => {
            catalog.remove(b"AcroForm");
        }
    }
}

// ----- outlines -----

/// One bookmark and what hangs under it. `id` is the outline item object.
pub(crate) struct Node {
    pub id: ObjectId,
    pub children: Vec<Node>,
    pub open: bool,
    /// The item pointed at a page that is gone; it stays only as a heading for its children.
    pub dead: bool,
}

fn collect_name_tree(doc: &Document, node: &Dictionary, out: &mut HashMap<Vec<u8>, Object>, depth: usize) {
    if depth > 32 || out.len() > 200_000 {
        return;
    }
    if let Ok(Object::Array(pairs)) = node.get(b"Names").map(|o| doc::deref(doc, o)) {
        for pair in pairs.chunks(2) {
            if let [key, value] = pair {
                if let Ok(k) = doc::deref(doc, key).as_str() {
                    out.insert(k.to_vec(), doc::deref(doc, value).clone());
                }
            }
        }
    }
    if let Ok(Object::Array(kids)) = node.get(b"Kids").map(|o| doc::deref(doc, o)) {
        for kid in kids {
            if let Ok(d) = doc::deref(doc, kid).as_dict() {
                collect_name_tree(doc, d, out, depth + 1);
            }
        }
    }
}

/// Every named destination in the document, as an explicit destination array.
fn named_destinations(doc: &Document) -> HashMap<Vec<u8>, Object> {
    let mut raw: HashMap<Vec<u8>, Object> = HashMap::new();
    if let Ok(catalog) = doc.catalog() {
        if let Ok(Object::Dictionary(dests)) = catalog.get(b"Dests").map(|o| doc::deref(doc, o)) {
            for (name, value) in dests.iter() {
                raw.insert(name.clone(), doc::deref(doc, value).clone());
            }
        }
        if let Ok(Object::Dictionary(names)) = catalog.get(b"Names").map(|o| doc::deref(doc, o)) {
            if let Ok(Object::Dictionary(tree)) = names.get(b"Dests").map(|o| doc::deref(doc, o)) {
                collect_name_tree(doc, tree, &mut raw, 0);
            }
        }
    }
    raw.into_iter()
        .filter_map(|(name, value)| {
            let array = match value {
                Object::Array(a) => Some(a),
                Object::Dictionary(d) => d.get(b"D").ok().map(|o| doc::deref(doc, o)).and_then(|o| o.as_array().ok()).cloned(),
                _ => None,
            }?;
            Some((name, Object::Array(array)))
        })
        .collect()
}

fn outline_items(doc: &Document, root: ObjectId) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    let mut stack = vec![root];
    while let Some(parent) = stack.pop() {
        let mut next = doc.get_dictionary(parent).ok().and_then(|d| d.get(b"First").ok()).and_then(|o| o.as_reference().ok());
        while let Some(id) = next {
            if !seen.insert(id) || seen.len() > 200_000 {
                break;
            }
            let Ok(d) = doc.get_dictionary(id) else { break };
            next = d.get(b"Next").ok().and_then(|o| o.as_reference().ok());
            out.push(id);
            stack.push(id);
        }
    }
    out
}

fn dest_name(obj: &Object) -> Option<Vec<u8>> {
    match obj {
        Object::Name(n) => Some(n.clone()),
        Object::String(s, _) => Some(s.clone()),
        _ => None,
    }
}

/// Make an outline safe to copy on its own: named destinations become explicit ones
/// (the name tables are not copied) and links into the structure tree are cut.
fn prepare_outline(src: &mut Document, root: ObjectId) {
    // Split imports from one source many times; the work only needs doing once. The
    // marker sits on the old outline root, which no output keeps.
    const READY: &[u8] = b"ShoruiReady";
    match src.get_dictionary_mut(root) {
        Ok(d) if !d.has(READY) => d.set(READY, true),
        _ => return,
    }
    let items = outline_items(src, root);
    if items.is_empty() {
        return;
    }
    let names = named_destinations(src);
    for id in items {
        let Ok(dict) = src.get_dictionary(id) else { continue };
        let dest_fix = dict.get(b"Dest").ok().map(|o| doc::deref(src, o)).and_then(dest_name).and_then(|n| names.get(&n).cloned());
        let action = dict.get(b"A").ok().cloned();
        let action_fix = action.as_ref().map(|a| doc::deref(src, a)).and_then(|a| a.as_dict().ok()).and_then(|a| {
            if !name_is(a, b"S", b"GoTo") {
                return None;
            }
            a.get(b"D").ok().map(|o| doc::deref(src, o)).and_then(dest_name).and_then(|n| names.get(&n).cloned())
        });
        if let Some(fix) = action_fix {
            match action {
                Some(Object::Reference(action_id)) => {
                    if let Ok(a) = src.get_dictionary_mut(action_id) {
                        a.set("D", fix);
                    }
                }
                _ => {
                    if let Ok(Object::Dictionary(a)) = src.get_dictionary_mut(id).and_then(|d| d.get_mut(b"A")) {
                        a.set("D", fix);
                    }
                }
            }
        }
        if let Ok(dict) = src.get_dictionary_mut(id) {
            if let Some(fix) = dest_fix {
                dict.set("Dest", fix);
            }
            dict.remove(b"SE");
        }
    }
}

#[derive(PartialEq)]
enum Target {
    Live,
    Dead,
    Unknown,
}

fn dest_state(doc: &Document, dest: &Object, valid: &HashSet<ObjectId>, names_resolve: bool) -> Target {
    match doc::deref(doc, dest) {
        Object::Array(items) => match items.first() {
            Some(Object::Reference(id)) => {
                if valid.contains(id) {
                    Target::Live
                } else {
                    Target::Dead
                }
            }
            Some(Object::Integer(_)) => Target::Unknown,
            _ => Target::Dead,
        },
        Object::Name(_) | Object::String(..) => {
            if names_resolve {
                Target::Unknown
            } else {
                Target::Dead
            }
        }
        _ => Target::Dead,
    }
}

fn item_state(doc: &Document, item: &Dictionary, valid: &HashSet<ObjectId>, names_resolve: bool) -> Target {
    if let Ok(dest) = item.get(b"Dest") {
        return dest_state(doc, dest, valid, names_resolve);
    }
    if let Ok(Object::Dictionary(action)) = item.get(b"A").map(|a| doc::deref(doc, a)) {
        if name_is(action, b"S", b"GoTo") {
            return match action.get(b"D") {
                Ok(dest) => dest_state(doc, dest, valid, names_resolve),
                Err(_) => Target::Dead,
            };
        }
    }
    Target::Unknown
}

fn read_level(
    doc: &Document, parent: ObjectId, valid: &HashSet<ObjectId>, names_resolve: bool, seen: &mut HashSet<ObjectId>, depth: usize,
) -> Vec<Node> {
    let mut out = Vec::new();
    let mut next = doc.get_dictionary(parent).ok().and_then(|d| d.get(b"First").ok()).and_then(|o| o.as_reference().ok());
    while let Some(id) = next {
        if !seen.insert(id) || seen.len() > 200_000 {
            break;
        }
        let Ok(dict) = doc.get_dictionary(id) else { break };
        next = dict.get(b"Next").ok().and_then(|o| o.as_reference().ok());
        let open = dict.get(b"Count").and_then(Object::as_i64).map(|c| c > 0).unwrap_or(false);
        let children = if depth < 64 { read_level(doc, id, valid, names_resolve, seen, depth + 1) } else { Vec::new() };
        let dead = item_state(doc, dict, valid, names_resolve) == Target::Dead;
        if dead && children.is_empty() {
            continue;
        }
        out.push(Node { id, children, open, dead });
    }
    out
}

/// Read the outline under `root`, leaving out bookmarks whose page is not in `valid`.
/// `names_resolve` says whether named destinations still work in this document.
pub(crate) fn read_outline(doc: &Document, root: ObjectId, valid: &HashSet<ObjectId>, names_resolve: bool) -> Vec<Node> {
    read_level(doc, root, valid, names_resolve, &mut HashSet::new(), 0)
}

fn link_level(doc: &mut Document, parent: ObjectId, nodes: &[Node]) -> i64 {
    let mut visible = 0i64;
    let mut prev: Option<ObjectId> = None;
    let mut iter = nodes.iter().peekable();
    while let Some(node) = iter.next() {
        let inner = link_level(doc, node.id, &node.children);
        let next = iter.peek().map(|n| n.id);
        if let Ok(d) = doc.get_dictionary_mut(node.id) {
            d.set("Parent", parent);
            match prev {
                Some(p) => d.set("Prev", p),
                None => {
                    d.remove(b"Prev");
                }
            }
            match next {
                Some(n) => d.set("Next", n),
                None => {
                    d.remove(b"Next");
                }
            }
            match (node.children.first(), node.children.last()) {
                (Some(first), Some(last)) => {
                    d.set("First", first.id);
                    d.set("Last", last.id);
                    d.set("Count", if node.open { inner } else { -inner });
                }
                _ => {
                    for key in [&b"First"[..], b"Last", b"Count"] {
                        d.remove(key);
                    }
                }
            }
            if node.dead {
                d.remove(b"Dest");
                d.remove(b"A");
            }
        }
        visible += 1 + if node.open { inner } else { 0 };
        prev = Some(node.id);
    }
    visible
}

/// Make `nodes` the document outline, rewriting every link between the items. With no
/// nodes the document ends up with no outline.
pub(crate) fn write_outline(doc: &mut Document, nodes: &[Node]) -> Result<()> {
    let (Some(first), Some(last)) = (nodes.first(), nodes.last()) else {
        doc.catalog_mut()?.remove(b"Outlines");
        return Ok(());
    };
    let root = doc.add_object(dictionary! { "Type" => "Outlines", "First" => first.id, "Last" => last.id });
    let visible = link_level(doc, root, nodes);
    doc.get_dictionary_mut(root)?.set("Count", visible);
    doc.catalog_mut()?.set("Outlines", root);
    Ok(())
}
