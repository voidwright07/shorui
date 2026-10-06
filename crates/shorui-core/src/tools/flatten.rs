//! Flatten: Turn form fields and annotations into fixed page content.
//!
//! This file also holds the AcroForm plumbing that `edit.rs` and `redact.rs` share:
//! walking the field tree, reading appearances and writing a text appearance stream.

use crate::doc::{self, fmt};
use crate::text::Rect4;
use crate::{Ctx, Error, Outcome, Result};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Turn form fields into fixed content and remove the form.
    pub forms: bool,
    /// Turn comments, stamps, highlights and other annotations into fixed content.
    pub annotations: bool,
    /// Leave link annotations live. When off (and `annotations` is on) links are removed.
    pub keep_links: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { forms: true, annotations: true, keep_links: true }
    }
}

/// What `flatten_doc` did.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Stats {
    /// Form fields turned into page content (a radio group counts once).
    pub fields: usize,
    /// Other annotations turned into page content.
    pub annotations: usize,
    /// Annotations that have no drawn appearance and were left as they are.
    pub left: usize,
    /// Links, pop-ups and hidden annotations that were removed without drawing anything.
    pub removed: usize,
    /// Signature fields that carried a signature.
    pub signatures: usize,
}

/// Counts shown by the UI before flattening.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Summary {
    pub fields: usize,
    pub annotations: usize,
    pub links: usize,
}

/// How many form fields, annotations and links a file has, so the UI can say what
/// flattening will do before it runs.
pub fn summary(path: &Path, password: Option<&str>) -> Result<Summary> {
    let doc = doc::load(path, password)?;
    let mut s = Summary { fields: form_fields(&doc).len(), ..Default::default() };
    for pid in doc::page_ids(&doc) {
        for (_, annot) in page_annots(&doc, pid) {
            match annot.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"") {
                b"Widget" | b"Popup" => {}
                b"Link" => s.links += 1,
                _ => s.annotations += 1,
            }
        }
    }
    Ok(s)
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = single_input(inputs)?;
    if !opts.forms && !opts.annotations {
        return Err(Error::invalid("Choose what to flatten: form fields, annotations, or both."));
    }
    ctx.report(0.0, "Opening the file");
    let mut doc = doc::load(input, ctx.password())?;
    let pages = doc::page_count(&doc);
    let stats = flatten_doc(&mut doc, opts, ctx)?;
    ctx.check()?;
    ctx.report(0.95, "Saving");
    doc::save(&mut doc, out)?;
    let mut outcome = Outcome::single(out.to_path_buf(), pages, crate::ctx::file_size(input));
    if stats.fields == 0 && stats.annotations == 0 && stats.removed == 0 {
        outcome.notes.push("There was nothing to flatten in this file.".into());
    } else {
        outcome.notes.push(format!("Flattened {} and {}.", plural(stats.fields, "form field"), plural(stats.annotations, "annotation")));
    }
    if stats.left > 0 {
        outcome.notes.push(format!("{} no drawn appearance and {} left as {}.", plural_have(stats.left, "annotation"), if stats.left == 1 { "was" } else { "were" }, if stats.left == 1 { "it is" } else { "they are" }));
    }
    if stats.signatures > 0 {
        outcome.notes.push("A digital signature was flattened into a picture. It no longer verifies.".into());
    }
    unlocked_note(&doc, &mut outcome);
    ctx.report(1.0, "Done");
    Ok(outcome)
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

fn plural_have(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word} has") } else { format!("{n} {word}s have") }
}

/// Tools write their result without a password. Say so when the input had one.
pub(crate) fn unlocked_note(doc: &Document, outcome: &mut Outcome) {
    if doc.was_encrypted() {
        outcome.notes.push("The original was password protected; this copy is not. Use Protect to add a password again.".into());
    }
}

/// The one input a single-file tool works on.
pub(crate) fn single_input(inputs: &[PathBuf]) -> Result<&Path> {
    match inputs {
        [one] => Ok(one.as_path()),
        [] => Err(Error::invalid("Choose a PDF file first.")),
        _ => Err(Error::invalid("This tool works on one PDF at a time.")),
    }
}

// ---------------------------------------------------------------------------
// Flattening
// ---------------------------------------------------------------------------

enum Fate {
    Keep,
    /// Draw this Form XObject at the annotation rectangle, then remove the annotation.
    Draw(ObjectId, [f32; 4]),
    Drop,
    /// A pop-up: goes when its parent goes.
    Popup(Option<ObjectId>),
}

/// Flatten in place. Used by `run`, by Edit & Fill when asked to flatten after filling,
/// and by Redact so that every appearance is part of the page before it is rasterised.
pub fn flatten_doc(doc: &mut Document, opts: &Options, ctx: &Ctx) -> Result<Stats> {
    flatten_some(doc, opts, None, ctx)
}

/// Turn just these form widgets into fixed page content and take them out of the form.
/// The rest of the form stays fillable.
pub(crate) fn flatten_widgets(doc: &mut Document, only: &HashSet<ObjectId>, ctx: &Ctx) -> Result<Stats> {
    flatten_some(doc, &Options { forms: true, annotations: false, keep_links: true }, Some(only), ctx)
}

fn flatten_some(doc: &mut Document, opts: &Options, only: Option<&HashSet<ObjectId>>, ctx: &Ctx) -> Result<Stats> {
    let mut stats = Stats::default();
    let fields = form_fields(doc);
    let mut widget_field: HashMap<ObjectId, usize> = HashMap::new();
    for (i, f) in fields.iter().enumerate() {
        for w in &f.widgets {
            widget_field.insert(*w, i);
        }
    }

    if opts.forms {
        // Fields that hold a value but have nothing drawn for it get an appearance first.
        let regenerate = need_appearances(doc);
        let mut helv = None;
        for f in &fields {
            if f.kind() == FieldKind::Signature && f.value.is_some() {
                stats.signatures += 1;
            }
            if !matches!(f.kind(), FieldKind::Text | FieldKind::Choice) {
                continue;
            }
            if only.is_some_and(|set| !f.widgets.iter().any(|w| set.contains(w))) {
                continue;
            }
            let text = f.display_value();
            if text.is_empty() {
                continue;
            }
            for &w in &f.widgets {
                if lacks_text_appearance(doc, w, regenerate) {
                    write_text_appearance(doc, f, w, &text, &mut helv)?;
                }
            }
        }
    }

    let ids = doc::page_ids(doc);
    let mut flattened_fields: HashSet<usize> = HashSet::new();
    for (index, &pid) in ids.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.05 + 0.85 * index as f32 / ids.len().max(1) as f32, &format!("Flattening page {} of {}", index + 1, ids.len()));
        let annots = page_annots(doc, pid);
        if annots.is_empty() {
            continue;
        }

        let mut fates: Vec<Fate> = Vec::with_capacity(annots.len());
        for (id, annot) in &annots {
            let subtype = annot.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
            let chosen = only.is_none_or(|set| id.is_some_and(|id| set.contains(&id)));
            let flags = annot.get(b"F").and_then(Object::as_i64).unwrap_or(0);
            // Hidden (bit 2) and NoView (bit 6) annotations are not shown on screen.
            let invisible = flags & 2 != 0 || flags & 32 != 0;
            let drawable = || match (invisible, appearance_stream(doc, annot), annot.get(b"Rect").ok().and_then(|r| doc::rect_of(doc, r))) {
                (false, Some(ap), Some(rect)) if rect[2] > rect[0] && rect[3] > rect[1] => Some((ap, rect)),
                _ => None,
            };
            let fate = match subtype {
                b"Widget" if !opts.forms || !chosen => Fate::Keep,
                b"Widget" => match drawable() {
                    Some((ap, rect)) => Fate::Draw(ap, rect),
                    None => Fate::Drop,
                },
                _ if !opts.annotations => Fate::Keep,
                b"Link" if opts.keep_links => Fate::Keep,
                b"Link" => Fate::Drop,
                b"Popup" => Fate::Popup(annot.get(b"Parent").and_then(Object::as_reference).ok()),
                _ if invisible => Fate::Drop,
                _ => match drawable() {
                    Some((ap, rect)) => Fate::Draw(ap, rect),
                    None => Fate::Keep,
                },
            };
            fates.push(fate);
        }
        // A pop-up stays only while the annotation it belongs to stays.
        let kept: HashSet<ObjectId> = annots.iter().zip(&fates).filter(|(_, f)| matches!(f, Fate::Keep)).filter_map(|((id, _), _)| *id).collect();
        for fate in &mut fates {
            if let Fate::Popup(parent) = fate {
                *fate = if parent.is_some_and(|p| kept.contains(&p)) { Fate::Keep } else { Fate::Drop };
            }
        }

        let mut content = String::new();
        let mut remaining: Vec<Object> = Vec::new();
        for ((id, annot), fate) in annots.iter().zip(&fates) {
            let is_widget = annot.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Widget");
            match fate {
                Fate::Keep | Fate::Popup(_) => {
                    if !is_widget && !matches!(annot.get(b"Subtype").and_then(Object::as_name), Ok(b"Link") | Ok(b"Popup")) && opts.annotations {
                        stats.left += 1;
                    }
                    remaining.push(match id {
                        Some(id) => Object::Reference(*id),
                        None => Object::Dictionary(annot.clone()),
                    });
                    continue;
                }
                Fate::Draw(ap, rect) => {
                    if let Some(matrix) = prepare_form(doc, *ap, *rect) {
                        let name = doc::ensure_xobject(doc, pid, *ap)?;
                        content += &format!("q {}/{} Do Q\n", doc::cm(matrix), name);
                    }
                    if is_widget {
                        match id.and_then(|id| widget_field.get(&id)) {
                            Some(field) => {
                                if flattened_fields.insert(*field) {
                                    stats.fields += 1;
                                }
                            }
                            None => stats.fields += 1,
                        }
                    } else {
                        stats.annotations += 1;
                    }
                }
                Fate::Drop => {
                    if is_widget {
                        // A field with nothing to show still counts as flattened: it is gone.
                        if let Some(field) = id.and_then(|id| widget_field.get(&id)) {
                            if flattened_fields.insert(*field) {
                                stats.fields += 1;
                            }
                        }
                    } else {
                        stats.removed += 1;
                    }
                }
            }
            if let Some(id) = id {
                doc.objects.remove(id);
            }
        }
        if !content.is_empty() {
            doc::overlay(doc, pid, content.into_bytes(), false)?;
        }
        let page = doc.get_dictionary_mut(pid)?;
        if remaining.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", remaining);
        }
    }

    if opts.forms {
        match only {
            None => {
                if let Ok(catalog) = doc.catalog_mut() {
                    catalog.remove(b"AcroForm");
                }
            }
            Some(_) => drop_missing_fields(doc),
        }
    }
    prune(doc);
    Ok(stats)
}

/// Take references to widgets that no longer exist out of the form's field tree.
fn drop_missing_fields(doc: &mut Document) {
    let exists: HashSet<ObjectId> = doc.objects.keys().copied().collect();
    let keep = |o: &Object| !matches!(o, Object::Reference(id) if !exists.contains(id));
    let nodes: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for id in nodes {
        let Ok(dict) = doc.get_dictionary_mut(id) else { continue };
        // Field nodes only: the page tree also uses /Kids.
        if !(dict.has(b"T") || dict.has(b"FT")) {
            continue;
        }
        if let Ok(kids) = dict.get_mut(b"Kids").and_then(Object::as_array_mut) {
            kids.retain(keep);
        }
    }
    if let Some(acro) = acroform_mut(doc) {
        if let Ok(fields) = acro.get_mut(b"Fields").and_then(Object::as_array_mut) {
            fields.retain(keep);
        }
    }
}

/// Make sure `ap` is usable as a Form XObject and work out the matrix that places it on
/// `rect`, following the annotation appearance algorithm of the PDF specification: the
/// form's BBox is transformed by its Matrix, and the upright box around the result is
/// scaled and moved onto the annotation rectangle. `Do` applies the form's own Matrix.
fn prepare_form(doc: &mut Document, ap: ObjectId, rect: [f32; 4]) -> Option<[f32; 6]> {
    let (bbox, matrix) = {
        let stream = doc.get_object(ap).ok()?.as_stream().ok()?;
        let bbox = stream.dict.get(b"BBox").ok().and_then(|b| doc::rect_of(doc, b));
        let matrix = stream.dict.get(b"Matrix").ok().and_then(|m| matrix_of(doc, m)).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        (bbox, matrix)
    };
    let default_box = [0.0, 0.0, rect[2] - rect[0], rect[3] - rect[1]];
    {
        let stream = doc.get_object_mut(ap).ok()?.as_stream_mut().ok()?;
        stream.dict.set("Type", "XObject");
        stream.dict.set("Subtype", "Form");
        if bbox.is_none() {
            stream.dict.set("BBox", default_box.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>());
        }
    }
    let b = bbox.unwrap_or(default_box);
    let corners = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| apply(matrix, x, y));
    let x0 = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
    let x1 = corners.iter().map(|c| c.0).fold(f32::NEG_INFINITY, f32::max);
    let y0 = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
    let y1 = corners.iter().map(|c| c.1).fold(f32::NEG_INFINITY, f32::max);
    if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
        return None;
    }
    let sx = if x1 - x0 > 1e-4 { (rect[2] - rect[0]) / (x1 - x0) } else { 1.0 };
    let sy = if y1 - y0 > 1e-4 { (rect[3] - rect[1]) / (y1 - y0) } else { 1.0 };
    Some([sx, 0.0, 0.0, sy, rect[0] - x0 * sx, rect[1] - y0 * sy])
}

pub(crate) fn apply(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn matrix_of(doc: &Document, obj: &Object) -> Option<[f32; 6]> {
    let arr = doc::deref(doc, obj).as_array().ok()?;
    let v: Vec<f32> = arr.iter().filter_map(|o| doc::as_f32(doc::deref(doc, o))).collect();
    if v.len() == 6 { Some([v[0], v[1], v[2], v[3], v[4], v[5]]) } else { None }
}

/// Remove every object that nothing refers to any more.
pub(crate) fn prune(doc: &mut Document) {
    fn visit(obj: &Object, stack: &mut Vec<ObjectId>) {
        match obj {
            Object::Reference(id) => stack.push(*id),
            Object::Array(items) => items.iter().for_each(|o| visit(o, stack)),
            Object::Dictionary(d) => d.iter().for_each(|(_, o)| visit(o, stack)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, o)| visit(o, stack)),
            _ => {}
        }
    }
    let mut stack = Vec::new();
    for (_, v) in doc.trailer.iter() {
        visit(v, &mut stack);
    }
    let mut live: HashSet<ObjectId> = HashSet::new();
    while let Some(id) = stack.pop() {
        if live.insert(id) {
            if let Some(obj) = doc.objects.get(&id) {
                visit(obj, &mut stack);
            }
        }
    }
    doc.objects.retain(|id, _| live.contains(id));
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

/// The annotations of a page: the object id when the annotation is an indirect object,
/// and a copy of its dictionary.
pub(crate) fn page_annots(doc: &Document, page_id: ObjectId) -> Vec<(Option<ObjectId>, Dictionary)> {
    let Ok(page) = doc.get_dictionary(page_id) else { return Vec::new() };
    let Ok(annots) = page.get(b"Annots") else { return Vec::new() };
    let Ok(array) = doc::deref(doc, annots).as_array() else { return Vec::new() };
    array
        .iter()
        .filter_map(|item| match item {
            Object::Reference(id) => doc.get_dictionary(*id).ok().map(|d| (Some(*id), d.clone())),
            Object::Dictionary(d) => Some((None, d.clone())),
            _ => None,
        })
        .collect()
}

/// The normal appearance of an annotation.
pub(crate) enum Normal {
    None,
    Stream(ObjectId),
    /// Appearance states by name, as check boxes and radio buttons have.
    States(Vec<(Vec<u8>, ObjectId)>),
}

pub(crate) fn normal_appearance(doc: &Document, annot: &Dictionary) -> Normal {
    let Some(ap) = annot.get(b"AP").ok().and_then(|o| doc::deref(doc, o).as_dict().ok()) else { return Normal::None };
    let Ok(n) = ap.get(b"N") else { return Normal::None };
    if let Object::Reference(id) = n {
        if matches!(doc.get_object(*id), Ok(Object::Stream(_))) {
            return Normal::Stream(*id);
        }
    }
    match doc::deref(doc, n) {
        Object::Dictionary(states) => Normal::States(
            states
                .iter()
                .filter_map(|(k, v)| match v {
                    Object::Reference(id) if matches!(doc.get_object(*id), Ok(Object::Stream(_))) => Some((k.clone(), *id)),
                    _ => None,
                })
                .collect(),
        ),
        _ => Normal::None,
    }
}

/// The stream a viewer would draw for this annotation right now.
fn appearance_stream(doc: &Document, annot: &Dictionary) -> Option<ObjectId> {
    match normal_appearance(doc, annot) {
        Normal::None => None,
        Normal::Stream(id) => Some(id),
        Normal::States(states) => {
            let state = annot.get(b"AS").and_then(Object::as_name).ok()?;
            states.iter().find(|(name, _)| name == state).map(|(_, id)| *id)
        }
    }
}

/// A rectangle in page space as a rectangle in display space (origin top-left, y down).
pub(crate) fn page_rect_to_display(doc: &Document, page_id: ObjectId, r: [f32; 4]) -> Rect4 {
    let m = doc::page_to_visible(doc, page_id);
    let (_, h) = doc::visible_size(doc, page_id);
    let (ax, ay) = apply(m, r[0], r[1]);
    let (bx, by) = apply(m, r[2], r[3]);
    Rect4::new(ax, h - ay, bx, h - by)
}

// ---------------------------------------------------------------------------
// Form fields
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    Text,
    Checkbox,
    Radio,
    Choice,
    Signature,
    Button,
    Unknown,
}

/// A terminal form field with everything it inherits already resolved.
#[derive(Debug, Clone)]
pub(crate) struct FormField {
    pub id: ObjectId,
    /// Fully qualified name: the partial names from the root down, joined with dots.
    pub name: String,
    pub ft: Vec<u8>,
    pub flags: i64,
    pub value: Option<Object>,
    pub da: Option<String>,
    pub quadding: i64,
    pub max_len: i64,
    /// `(export value, display text)` for choice fields.
    pub options: Vec<(String, String)>,
    pub widgets: Vec<ObjectId>,
}

impl FormField {
    pub fn kind(&self) -> FieldKind {
        match self.ft.as_slice() {
            b"Tx" => FieldKind::Text,
            b"Btn" if self.flags & (1 << 16) != 0 => FieldKind::Button,
            b"Btn" if self.flags & (1 << 15) != 0 => FieldKind::Radio,
            b"Btn" => FieldKind::Checkbox,
            b"Ch" => FieldKind::Choice,
            b"Sig" => FieldKind::Signature,
            _ => FieldKind::Unknown,
        }
    }

    pub fn multiline(&self) -> bool {
        self.kind() == FieldKind::Text && self.flags & (1 << 12) != 0
    }

    /// The value as text: a string value decoded, a name value as written.
    pub fn value_text(&self) -> String {
        match &self.value {
            Some(Object::String(..)) => self.value.as_ref().map(decode_text).unwrap_or_default(),
            Some(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
            Some(Object::Array(items)) => items.iter().map(decode_text).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        }
    }

    /// What the field shows: for a choice field the display text of the chosen option.
    pub fn display_value(&self) -> String {
        let value = self.value_text();
        if self.kind() == FieldKind::Choice {
            if let Some((_, shown)) = self.options.iter().find(|(export, _)| *export == value) {
                return shown.clone();
            }
        }
        value
    }
}

/// Decode a PDF text string (PDFDocEncoding, UTF-16 or UTF-8 with a byte order mark).
pub(crate) fn decode_text(obj: &Object) -> String {
    match obj {
        Object::String(bytes, _) => lopdf::decode_text_string(obj).unwrap_or_else(|_| bytes.iter().map(|b| *b as char).collect()),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => String::new(),
    }
}

/// Encode a PDF text string: plain bytes when Latin-1 can hold it, UTF-16 otherwise.
pub(crate) fn encode_text(text: &str) -> Object {
    if text.chars().all(|c| matches!(c as u32, 0x20..=0x7E | 0xA1..=0xFF | 0x0A | 0x0D | 0x09)) {
        Object::string_literal(text.chars().map(|c| c as u32 as u8).collect::<Vec<u8>>())
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, lopdf::StringFormat::Hexadecimal)
    }
}

pub(crate) fn acroform(doc: &Document) -> Option<&Dictionary> {
    doc::deref(doc, doc.catalog().ok()?.get(b"AcroForm").ok()?).as_dict().ok()
}

pub(crate) fn acroform_mut(doc: &mut Document) -> Option<&mut Dictionary> {
    match doc.catalog().ok()?.get(b"AcroForm").ok()? {
        Object::Reference(id) => {
            let id = *id;
            doc.get_dictionary_mut(id).ok()
        }
        Object::Dictionary(_) => doc.catalog_mut().ok()?.get_mut(b"AcroForm").ok()?.as_dict_mut().ok(),
        _ => None,
    }
}

fn need_appearances(doc: &Document) -> bool {
    acroform(doc).and_then(|a| a.get(b"NeedAppearances").ok()).and_then(|o| doc::deref(doc, o).as_bool().ok()).unwrap_or(false)
}

#[derive(Clone, Default)]
struct Inherited {
    ft: Vec<u8>,
    flags: i64,
    value: Option<Object>,
    da: Option<String>,
    quadding: i64,
    max_len: i64,
    options: Vec<(String, String)>,
}

/// Every terminal field of the form, in the order of the field tree.
pub(crate) fn form_fields(doc: &Document) -> Vec<FormField> {
    let Some(acro) = acroform(doc) else { return Vec::new() };
    let base = Inherited {
        da: acro.get(b"DA").ok().map(|o| decode_text(doc::deref(doc, o))),
        quadding: acro.get(b"Q").ok().and_then(|o| doc::deref(doc, o).as_i64().ok()).unwrap_or(0),
        ..Default::default()
    };
    let roots: Vec<ObjectId> = acro
        .get(b"Fields")
        .ok()
        .and_then(|o| doc::deref(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for id in roots {
        walk_field(doc, id, "", &base, &mut out, &mut seen, 0);
    }
    out
}

fn walk_field(doc: &Document, id: ObjectId, prefix: &str, inherited: &Inherited, out: &mut Vec<FormField>, seen: &mut HashSet<ObjectId>, depth: usize) {
    if depth > 32 || !seen.insert(id) {
        return;
    }
    let Ok(dict) = doc.get_dictionary(id) else { return };
    let mut inh = inherited.clone();
    if let Ok(ft) = dict.get(b"FT").map(|o| doc::deref(doc, o)).and_then(Object::as_name) {
        inh.ft = ft.to_vec();
    }
    if let Ok(ff) = dict.get(b"Ff").map(|o| doc::deref(doc, o)).and_then(Object::as_i64) {
        inh.flags = ff;
    }
    if let Ok(v) = dict.get(b"V") {
        inh.value = Some(doc::deref(doc, v).clone());
    }
    if let Ok(da) = dict.get(b"DA") {
        inh.da = Some(decode_text(doc::deref(doc, da)));
    }
    if let Ok(q) = dict.get(b"Q").map(|o| doc::deref(doc, o)).and_then(Object::as_i64) {
        inh.quadding = q;
    }
    if let Ok(n) = dict.get(b"MaxLen").map(|o| doc::deref(doc, o)).and_then(Object::as_i64) {
        inh.max_len = n;
    }
    if let Ok(opt) = dict.get(b"Opt").map(|o| doc::deref(doc, o)).and_then(Object::as_array) {
        inh.options = opt
            .iter()
            .map(|item| match doc::deref(doc, item) {
                Object::Array(pair) if pair.len() >= 2 => (decode_text(doc::deref(doc, &pair[0])), decode_text(doc::deref(doc, &pair[1]))),
                other => {
                    let text = decode_text(other);
                    (text.clone(), text)
                }
            })
            .collect();
    }
    let name = match dict.get(b"T") {
        Ok(t) => {
            let part = decode_text(doc::deref(doc, t));
            if prefix.is_empty() { part } else { format!("{prefix}.{part}") }
        }
        Err(_) => prefix.to_string(),
    };

    let kids: Vec<ObjectId> = dict
        .get(b"Kids")
        .ok()
        .and_then(|o| doc::deref(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let child_fields: Vec<ObjectId> = kids.iter().copied().filter(|k| doc.get_dictionary(*k).is_ok_and(|d| d.has(b"T"))).collect();
    if !child_fields.is_empty() {
        for kid in child_fields {
            walk_field(doc, kid, &name, &inh, out, seen, depth + 1);
        }
        return;
    }
    let widgets: Vec<ObjectId> = if kids.is_empty() {
        if dict.has(b"Rect") || dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Widget") { vec![id] } else { Vec::new() }
    } else {
        kids.into_iter().filter(|k| doc.get_dictionary(*k).is_ok()).collect()
    };
    out.push(FormField { id, name, ft: inh.ft, flags: inh.flags, value: inh.value, da: inh.da, quadding: inh.quadding, max_len: inh.max_len, options: inh.options, widgets });
}

/// The appearance state names of a check box or radio widget other than `Off`.
pub(crate) fn on_states(doc: &Document, widget: ObjectId) -> Vec<Vec<u8>> {
    let Ok(dict) = doc.get_dictionary(widget) else { return Vec::new() };
    match normal_appearance(doc, dict) {
        Normal::States(states) => states.into_iter().map(|(name, _)| name).filter(|n| n != b"Off").collect(),
        _ => Vec::new(),
    }
}

fn stream_text(doc: &Document, id: ObjectId) -> Option<(Dictionary, Vec<u8>)> {
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    Some((stream.dict.clone(), stream.get_plain_content().ok()?))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// True when a text widget has nothing drawn for its value. With `regenerate` (the form
/// asks viewers to rebuild appearances) an appearance that shows no text also counts.
fn lacks_text_appearance(doc: &Document, widget: ObjectId, regenerate: bool) -> bool {
    let Ok(dict) = doc.get_dictionary(widget) else { return false };
    match normal_appearance(doc, dict) {
        Normal::None => true,
        Normal::States(_) => false,
        Normal::Stream(id) => regenerate && stream_text(doc, id).is_some_and(|(_, content)| find(&content, b"Tj").is_none() && find(&content, b"TJ").is_none()),
    }
}

/// Font size and colour operators from a default appearance string such as `/Helv 11 Tf 0 g`.
fn parse_da(da: &str) -> (f32, String) {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut size = 0.0;
    let mut color = String::from("0 g");
    for (i, token) in tokens.iter().enumerate() {
        let operands = |n: usize| -> Option<String> {
            if i < n {
                return None;
            }
            let parts = &tokens[i - n..i];
            parts.iter().all(|p| p.parse::<f32>().is_ok()).then(|| parts.join(" "))
        };
        match *token {
            "Tf" if i >= 1 => size = tokens[i - 1].parse::<f32>().unwrap_or(0.0),
            "g" => {
                if let Some(o) = operands(1) {
                    color = format!("{o} g");
                }
            }
            "rg" => {
                if let Some(o) = operands(3) {
                    color = format!("{o} rg");
                }
            }
            "k" => {
                if let Some(o) = operands(4) {
                    color = format!("{o} k");
                }
            }
            _ => {}
        }
    }
    (if size.is_finite() && size > 0.0 { size.min(144.0) } else { 0.0 }, color)
}

fn color_op(doc: &Document, obj: &Object, stroke: bool) -> Option<String> {
    let arr = doc::deref(doc, obj).as_array().ok()?;
    let v: Vec<String> = arr.iter().filter_map(|o| doc::as_f32(doc::deref(doc, o))).map(fmt).collect();
    let op = match (v.len(), stroke) {
        (1, false) => "g",
        (1, true) => "G",
        (3, false) => "rg",
        (3, true) => "RG",
        (4, false) => "k",
        (4, true) => "K",
        _ => return None,
    };
    Some(format!("{} {op}", v.join(" ")))
}

/// Break text into lines no wider than `width` at `size`, keeping explicit line breaks.
pub(crate) fn wrap(text: &str, font: doc::StdFont, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && width > 0.0 && font.width(&candidate, size) > width {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

/// Give a text or choice widget an appearance stream showing `text`, so that the value
/// is visible in every viewer and survives flattening.
///
/// The text is set in Helvetica at the size and colour of the field's default appearance
/// (size 0 means "fit"), clipped to the field. What an existing appearance draws around
/// the text (border, background) is kept; without one, the border and background come
/// from the widget's `/MK` entry.
pub(crate) fn write_text_appearance(doc: &mut Document, field: &FormField, widget: ObjectId, text: &str, helv: &mut Option<ObjectId>) -> Result<()> {
    use doc::StdFont::Helvetica;
    let wdict = doc.get_dictionary(widget)?.clone();
    let Some(rect) = wdict.get(b"Rect").ok().and_then(|r| doc::rect_of(doc, r)) else { return Ok(()) };
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    if rw <= 0.0 || rh <= 0.0 {
        return Ok(());
    }
    let mk = wdict.get(b"MK").ok().and_then(|o| doc::deref(doc, o).as_dict().ok()).cloned();
    let border_width = wdict
        .get(b"BS")
        .ok()
        .and_then(|o| doc::deref(doc, o).as_dict().ok())
        .and_then(|bs| bs.get(b"W").ok())
        .and_then(|w| doc::as_f32(doc::deref(doc, w)))
        .unwrap_or(1.0)
        .clamp(0.0, 12.0);

    let existing = match normal_appearance(doc, &wdict) {
        Normal::Stream(id) => stream_text(doc, id),
        _ => None,
    };

    // Geometry and what is drawn around the text.
    let mut matrix: Option<[f32; 6]> = None;
    let mut resources = Dictionary::new();
    let mut before = String::new();
    let mut after = Vec::new();
    let mut prefix = Vec::new();
    let bbox;
    let reuse = existing.as_ref().and_then(|(dict, content)| {
        let b = dict.get(b"BBox").ok().and_then(|b| doc::rect_of(doc, b))?;
        if b[2] - b[0] <= 0.0 || b[3] - b[1] <= 0.0 {
            return None;
        }
        let marked = find(content, b"/Tx BMC").and_then(|start| find(&content[start..], b"EMC").map(|len| (start, start + len + 3)));
        // Text outside a marked section would show the old value under the new one.
        if marked.is_none() && find(content, b"BT").is_some() {
            return None;
        }
        Some((b, dict, content, marked))
    });
    match reuse {
        Some((b, dict, content, marked)) => {
            bbox = b;
            matrix = dict.get(b"Matrix").ok().and_then(|m| matrix_of(doc, m));
            if let Some(res) = dict.get(b"Resources").ok().and_then(|o| doc::deref(doc, o).as_dict().ok()) {
                resources = res.clone();
            }
            match marked {
                Some((start, end)) => {
                    prefix = content[..start].to_vec();
                    after = content[end..].to_vec();
                }
                None => {
                    prefix.extend_from_slice(b"q\n");
                    prefix.extend_from_slice(content);
                    prefix.extend_from_slice(b"\nQ\n");
                }
            }
        }
        None => {
            let rotate = mk.as_ref().and_then(|m| m.get(b"R").ok()).and_then(|r| doc::deref(doc, r).as_i64().ok()).unwrap_or(0).rem_euclid(360);
            match rotate {
                90 => {
                    bbox = [0.0, 0.0, rh, rw];
                    matrix = Some([0.0, 1.0, -1.0, 0.0, rw, 0.0]);
                }
                180 => {
                    bbox = [0.0, 0.0, rw, rh];
                    matrix = Some([-1.0, 0.0, 0.0, -1.0, rw, rh]);
                }
                270 => {
                    bbox = [0.0, 0.0, rh, rw];
                    matrix = Some([0.0, -1.0, 1.0, 0.0, 0.0, rh]);
                }
                _ => bbox = [0.0, 0.0, rw, rh],
            }
            if let Some(mk) = &mk {
                if let Some(fill) = mk.get(b"BG").ok().and_then(|c| color_op(doc, c, false)) {
                    before += &format!("q {fill} {} {} {} {} re f Q\n", fmt(bbox[0]), fmt(bbox[1]), fmt(bbox[2] - bbox[0]), fmt(bbox[3] - bbox[1]));
                }
                if border_width > 0.0 {
                    if let Some(stroke) = mk.get(b"BC").ok().and_then(|c| color_op(doc, c, true)) {
                        let half = border_width / 2.0;
                        before += &format!(
                            "q {stroke} {} w {} {} {} {} re S Q\n",
                            fmt(border_width),
                            fmt(bbox[0] + half),
                            fmt(bbox[1] + half),
                            fmt(bbox[2] - bbox[0] - border_width),
                            fmt(bbox[3] - bbox[1] - border_width)
                        );
                    }
                }
            }
        }
    }
    let (bx, by, bw, bh) = (bbox[0], bbox[1], bbox[2] - bbox[0], bbox[3] - bbox[1]);

    // The font, under a name the existing resources do not use.
    let font_id = match helv {
        Some(id) => *id,
        None => {
            let id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
            *helv = Some(id);
            id
        }
    };
    let mut fonts = match resources.get(b"Font").map(|o| doc::deref(doc, o)) {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    let mut n = 0;
    let font_name = loop {
        let candidate = if n == 0 { "ShHelv".to_string() } else { format!("ShHelv{n}") };
        if !fonts.has(candidate.as_bytes()) {
            break candidate;
        }
        n += 1;
    };
    fonts.set(font_name.clone(), Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));

    // The text itself.
    let (da_size, color) = parse_da(field.da.as_deref().unwrap_or(""));
    let password = field.kind() == FieldKind::Text && field.flags & (1 << 13) != 0;
    let shown: String = if password { "*".repeat(text.chars().count()) } else { text.to_string() };
    let pad = (border_width + 1.0).max(2.0).min(bw / 4.0).min(bh / 4.0);
    let inner_w = (bw - 2.0 * pad).max(1.0);
    let inner_h = (bh - 2.0 * pad).max(1.0);
    let multiline = field.multiline();
    let comb = field.kind() == FieldKind::Text && field.flags & (1 << 24) != 0 && field.max_len > 0 && !multiline;

    let mut body = String::new();
    if multiline {
        let mut size = if da_size > 0.0 { da_size } else { 12.0 };
        let mut lines = wrap(&shown, Helvetica, size, inner_w);
        if da_size <= 0.0 {
            while size > 4.0 && lines.len() as f32 * size * 1.15 > inner_h {
                size -= 0.5;
                lines = wrap(&shown, Helvetica, size, inner_w);
            }
        }
        body += &format!("/{font_name} {} Tf\n", fmt(size));
        let mut y = by + bh - pad - 0.8 * size;
        for line in lines {
            let width = Helvetica.width(&line, size);
            let x = match field.quadding {
                1 => bx + (bw - width) / 2.0,
                2 => bx + bw - pad - width,
                _ => bx + pad,
            };
            body += &format!("1 0 0 1 {} {} Tm {} Tj\n", fmt(x), fmt(y), doc::pdf_string(&line));
            y -= size * 1.15;
        }
    } else {
        let line: String = shown.replace(['\r', '\n'], " ");
        let unit = Helvetica.width(&line, 1.0);
        let size = if da_size > 0.0 {
            da_size
        } else {
            let by_height = (bh - 4.0).clamp(4.0, 12.0);
            if unit > 0.0 && !comb { by_height.min(inner_w / unit).max(4.0) } else { by_height }
        };
        let y = by + (bh - 0.718 * size) / 2.0;
        body += &format!("/{font_name} {} Tf\n", fmt(size));
        if comb {
            let cell = bw / field.max_len as f32;
            for (i, ch) in line.chars().take(field.max_len as usize).enumerate() {
                let s = ch.to_string();
                let x = bx + cell * i as f32 + (cell - Helvetica.width(&s, size)) / 2.0;
                body += &format!("1 0 0 1 {} {} Tm {} Tj\n", fmt(x), fmt(y), doc::pdf_string(&s));
            }
        } else {
            let width = unit * size;
            let x = match field.quadding {
                1 => bx + (bw - width) / 2.0,
                2 => bx + bw - pad - width,
                _ => bx + pad,
            };
            body += &format!("1 0 0 1 {} {} Tm {} Tj\n", fmt(x), fmt(y), doc::pdf_string(&line));
        }
    }

    let mut content = prefix;
    content.extend_from_slice(before.as_bytes());
    content.extend_from_slice(
        format!("/Tx BMC\nq\n{} {} {} {} re W n\nBT\n{color}\n{body}ET\nQ\nEMC", fmt(bx + pad / 2.0), fmt(by + pad / 2.0), fmt(bw - pad), fmt(bh - pad)).as_bytes(),
    );
    content.extend_from_slice(&after);

    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "FormType" => 1,
        "BBox" => bbox.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        "Resources" => resources,
    };
    if let Some(m) = matrix {
        dict.set("Matrix", m.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>());
    }
    let ap = doc.add_object(Stream::new(dict, content));
    doc.get_dictionary_mut(widget)?.set("AP", dictionary! { "N" => ap });
    Ok(())
}

/// Give a check box that has no appearance states a plain tick for on and nothing for
/// off. Returns the name of the on state.
pub(crate) fn write_check_appearance(doc: &mut Document, widget: ObjectId) -> Result<Vec<u8>> {
    let rect = doc.get_dictionary(widget)?.get(b"Rect").ok().and_then(|r| doc::rect_of(doc, r)).unwrap_or([0.0, 0.0, 12.0, 12.0]);
    let (w, h) = ((rect[2] - rect[0]).max(1.0), (rect[3] - rect[1]).max(1.0));
    let bbox = || vec![0.0f32.into(), 0.0f32.into(), Object::Real(w), Object::Real(h)];
    let tick = format!(
        "q 0 G 1 J 1 j {} w {} {} m {} {} l {} {} l S Q",
        fmt((w.min(h) * 0.12).max(0.8)),
        fmt(w * 0.2),
        fmt(h * 0.5),
        fmt(w * 0.42),
        fmt(h * 0.24),
        fmt(w * 0.8),
        fmt(h * 0.8)
    );
    let on = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => bbox() }, tick.into_bytes()));
    let off = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => bbox() }, Vec::new()));
    doc.get_dictionary_mut(widget)?.set("AP", dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } });
    Ok(b"Yes".to_vec())
}
