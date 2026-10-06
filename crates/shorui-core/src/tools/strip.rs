//! Strip Metadata: Remove author, title, dates and other hidden information.

use super::merge::{self, UNPROTECTED_NOTE, plural};
use super::protect::new_file_id;
use crate::ctx::file_size;
use crate::{Ctx, Outcome, Result, doc};
use lopdf::{Dictionary, Document, Object, ObjectId, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Title, author, subject, keywords, dates and the name of the program that made the file.
    pub info: bool,
    /// The XMP packet and any metadata attached to pages, images and fonts.
    pub xmp: bool,
    /// Embedded page thumbnails.
    pub thumbnails: bool,
    /// Scripts: document-level JavaScript and script actions on open, pages, links and fields.
    pub javascript: bool,
    /// Embedded files, including file-attachment comments.
    pub attachments: bool,
    /// Author names, dates and ids on comments and other annotations.
    pub annotations_meta: bool,
    /// Private data left by editing programs, last-modified stamps, and the file identifier.
    pub private_data: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { info: true, xmp: true, thumbnails: true, javascript: true, attachments: true, annotations_meta: true, private_data: true }
    }
}

/// What was found and removed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    /// Document information fields, in plain words: "title", "author", ...
    pub info_fields: Vec<String>,
    /// Information fields without a common name (dates, custom entries).
    pub other_info_fields: usize,
    pub xmp_packet: bool,
    /// Metadata attached to individual pages, images or fonts.
    pub object_metadata: usize,
    pub thumbnails: usize,
    pub scripts: usize,
    pub attachments: usize,
    pub annotations: usize,
    pub private_data: usize,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.info_fields.is_empty()
            && self.other_info_fields == 0
            && !self.xmp_packet
            && self.object_metadata == 0
            && self.thumbnails == 0
            && self.scripts == 0
            && self.attachments == 0
            && self.annotations == 0
            && self.private_data == 0
    }

    /// Sentences for the user, one per kind of thing removed.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        let mut meta: Vec<String> = self.info_fields.clone();
        if self.other_info_fields > 0 {
            meta.push(plural(self.other_info_fields, if meta.is_empty() { "information field" } else { "other field" }, if meta.is_empty() { "information fields" } else { "other fields" }));
        }
        if self.xmp_packet {
            meta.push("the XMP packet".into());
        }
        if self.object_metadata > 0 {
            meta.push(format!("metadata on {}", plural(self.object_metadata, "page or image", "pages or images")));
        }
        if let Some((last, rest)) = meta.split_last() {
            notes.push(if rest.is_empty() { format!("Removed {last}.") } else { format!("Removed {} and {last}.", rest.join(", ")) });
        }
        if self.thumbnails > 0 {
            notes.push(format!("Removed {}.", plural(self.thumbnails, "page thumbnail", "page thumbnails")));
        }
        if self.scripts > 0 {
            notes.push(format!("Removed {}.", plural(self.scripts, "script", "scripts")));
        }
        if self.attachments > 0 {
            notes.push(format!("Removed {}.", plural(self.attachments, "attached file", "attached files")));
        }
        if self.annotations > 0 {
            notes.push(format!("Cleared the author and date on {}.", plural(self.annotations, "comment", "comments")));
        }
        if self.private_data > 0 {
            notes.push("Removed data left behind by editing programs.".into());
        }
        notes
    }
}

fn name_is(dict: &Dictionary, key: &[u8], value: &[u8]) -> bool {
    dict.get(key).and_then(Object::as_name).map(|n| n == value).unwrap_or(false)
}

fn dict_of(obj: &mut Object) -> Option<&mut Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&mut s.dict),
        _ => None,
    }
}

fn count_tree(doc: &Document, node: &Object, depth: usize) -> usize {
    let Ok(dict) = doc::deref(doc, node).as_dict() else { return 0 };
    let mut n = match dict.get(b"Names").map(|o| doc::deref(doc, o)) {
        Ok(Object::Array(items)) => items.len() / 2,
        _ => 0,
    };
    if depth < 32 {
        if let Ok(Object::Array(kids)) = dict.get(b"Kids").map(|o| doc::deref(doc, o)) {
            n += kids.iter().map(|k| count_tree(doc, k, depth + 1)).sum::<usize>();
        }
    }
    n
}

/// Take an entry out of the catalog's `/Names` dictionary and count what it listed.
fn take_name_tree(doc: &mut Document, key: &[u8]) -> usize {
    let names = doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).cloned();
    let removed = match names {
        Some(Object::Reference(id)) => doc.get_dictionary_mut(id).ok().and_then(|d| d.remove(key)),
        Some(Object::Dictionary(_)) => {
            doc.catalog_mut().ok().and_then(|c| c.get_mut(b"Names").ok()).and_then(|n| n.as_dict_mut().ok()).and_then(|d| d.remove(key))
        }
        _ => None,
    };
    match removed {
        Some(tree) => count_tree(doc, &tree, 0).max(1),
        None => 0,
    }
}

fn is_script(obj: &Object, scripts: &HashSet<ObjectId>) -> bool {
    match obj {
        Object::Reference(id) => scripts.contains(id),
        Object::Dictionary(d) => name_is(d, b"S", b"JavaScript"),
        _ => false,
    }
}

/// Remove every script action hanging off this object, however deep.
fn strip_scripts(obj: &mut Object, scripts: &HashSet<ObjectId>, removed: &mut usize) {
    match obj {
        Object::Array(items) => {
            let before = items.len();
            items.retain(|o| !is_script(o, scripts));
            *removed += before - items.len();
            items.iter_mut().for_each(|o| strip_scripts(o, scripts, removed));
        }
        Object::Dictionary(_) | Object::Stream(_) => {
            let Some(dict) = dict_of(obj) else { return };
            let doomed: Vec<Vec<u8>> = dict.iter().filter(|(_, v)| is_script(v, scripts)).map(|(k, _)| k.clone()).collect();
            for key in doomed {
                dict.remove(&key);
                *removed += 1;
            }
            dict.iter_mut().for_each(|(_, v)| strip_scripts(v, scripts, removed));
        }
        _ => {}
    }
}

fn is_empty_dict(obj: &Object, empties: &HashSet<ObjectId>) -> bool {
    match obj {
        Object::Reference(id) => empties.contains(id),
        Object::Dictionary(d) => d.is_empty(),
        _ => false,
    }
}

const INFO_NAMES: [(&str, &str); 5] =
    [("Title", "title"), ("Author", "author"), ("Subject", "subject"), ("Keywords", "keywords"), ("Creator", "creating program")];

/// Remove what `opts` asks for from a loaded document and say what was there.
pub fn clean(document: &mut Document, opts: &Options) -> Result<Report> {
    let mut report = Report::default();

    if opts.info {
        if let Some(info) = document.trailer.get(b"Info").ok().map(|o| doc::deref(document, o)).and_then(|o| o.as_dict().ok()) {
            let filled = |v: &Object| !matches!(doc::deref(document, v), Object::String(s, _) if s.is_empty());
            for (key, value) in info.iter() {
                if !filled(value) {
                    continue;
                }
                match INFO_NAMES.iter().find(|(k, _)| k.as_bytes() == key.as_slice()) {
                    Some((_, plain)) => report.info_fields.push((*plain).to_string()),
                    None => {
                        let ours = key.as_slice() == b"Producer" && doc::deref(document, value).as_str().map(|s| s == doc::PRODUCER.as_bytes()).unwrap_or(false);
                        if !ours {
                            report.other_info_fields += 1;
                        }
                    }
                }
            }
            report.info_fields.sort_by_key(|f| INFO_NAMES.iter().position(|(_, plain)| plain == f));
        }
        let fresh = document.add_object(dictionary! { "Producer" => Object::string_literal(doc::PRODUCER) });
        document.trailer.set("Info", fresh);
    }

    if opts.xmp {
        if let Ok(catalog) = document.catalog_mut() {
            report.xmp_packet = catalog.remove(b"Metadata").is_some();
        }
        for obj in document.objects.values_mut() {
            if let Some(dict) = dict_of(obj) {
                if dict.remove(b"Metadata").is_some() {
                    report.object_metadata += 1;
                }
            }
        }
    }

    if opts.thumbnails {
        for page in doc::page_ids(document) {
            if let Ok(dict) = document.get_dictionary_mut(page) {
                if dict.remove(b"Thumb").is_some() {
                    report.thumbnails += 1;
                }
            }
        }
    }

    if opts.javascript {
        report.scripts += take_name_tree(document, b"JavaScript");
        // The name tree is gone from the catalog; drop its objects before looking for
        // script actions so they are not counted twice.
        merge::prune(document);
        let scripts: HashSet<ObjectId> = document
            .objects
            .iter()
            .filter(|(_, o)| o.as_dict().map(|d| name_is(d, b"S", b"JavaScript")).unwrap_or(false))
            .map(|(id, _)| *id)
            .collect();
        let mut removed = 0usize;
        for obj in document.objects.values_mut() {
            strip_scripts(obj, &scripts, &mut removed);
        }
        report.scripts += removed;
        // Additional-action dictionaries that held nothing but scripts.
        let empties: HashSet<ObjectId> = document.objects.iter().filter(|(_, o)| matches!(o, Object::Dictionary(d) if d.is_empty())).map(|(id, _)| *id).collect();
        for obj in document.objects.values_mut() {
            if let Some(dict) = dict_of(obj) {
                if dict.get(b"AA").map(|v| is_empty_dict(v, &empties)).unwrap_or(false) {
                    dict.remove(b"AA");
                }
            }
        }
    }

    if opts.attachments {
        report.attachments += take_name_tree(document, b"EmbeddedFiles");
        if let Ok(catalog) = document.catalog_mut() {
            catalog.remove(b"Collection");
        }
        for page in doc::page_ids(document) {
            let all = merge::annots(document, page);
            if all.is_empty() {
                continue;
            }
            let is_attachment = |o: &Object| doc::deref(document, o).as_dict().map(|d| name_is(d, b"Subtype", b"FileAttachment")).unwrap_or(false);
            let gone: HashSet<ObjectId> = all.iter().filter(|o| is_attachment(o)).filter_map(|o| o.as_reference().ok()).collect();
            let kept: Vec<Object> = all
                .iter()
                .filter(|o| {
                    if is_attachment(o) {
                        return false;
                    }
                    // A pop-up window belonging to a removed attachment goes too.
                    let parent = doc::deref(document, o).as_dict().ok().and_then(|d| d.get(b"Parent").ok()).and_then(|p| p.as_reference().ok());
                    !parent.map(|p| gone.contains(&p)).unwrap_or(false)
                })
                .cloned()
                .collect();
            if kept.len() != all.len() {
                report.attachments += all.iter().filter(|o| is_attachment(o)).count();
                let dict = document.get_dictionary_mut(page)?;
                if kept.is_empty() {
                    dict.remove(b"Annots");
                } else {
                    dict.set("Annots", kept);
                }
            }
        }
        merge::prune(document);
        // Anything else that still carries an embedded file (associated files, media assets).
        for obj in document.objects.values_mut() {
            if let Some(dict) = dict_of(obj) {
                dict.remove(b"AF");
                if dict.remove(b"EF").is_some() {
                    report.attachments += 1;
                }
            }
        }
    }

    if opts.annotations_meta {
        let mut seen: HashSet<ObjectId> = HashSet::new();
        for page in doc::page_ids(document) {
            for id in merge::annot_ids(document, page) {
                if !seen.insert(id) {
                    continue;
                }
                let Ok(dict) = document.get_dictionary_mut(id) else { continue };
                // On a form field /T is the field's name, not an author.
                let field = name_is(dict, b"Subtype", b"Widget") || dict.has(b"FT");
                let keys: &[&[u8]] = if field { &[b"M"] } else { &[b"T", b"M", b"CreationDate", b"NM"] };
                let mut touched = false;
                for key in keys {
                    touched |= dict.remove(key).is_some();
                }
                if touched && !field {
                    report.annotations += 1;
                }
            }
        }
    }

    if opts.private_data {
        for obj in document.objects.values_mut() {
            if let Some(dict) = dict_of(obj) {
                if dict.remove(b"PieceInfo").is_some() {
                    report.private_data += 1;
                }
                dict.remove(b"LastModified");
            }
        }
        document.trailer.set("ID", new_file_id()?);
    }

    merge::prune(document);
    Ok(report)
}

/// What Strip would remove from a file with every option on, without writing anything.
/// The app shows this before the user runs the tool.
pub fn inspect(path: &Path, password: Option<&str>) -> Result<Report> {
    let mut document = super::unlock::load(path, password)?;
    clean(&mut document, &Options::default())
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = merge::one_input(inputs, "Strip Metadata")?;
    merge::guard_output(inputs, out)?;
    ctx.report(0.0, "Reading the file");
    let mut document = super::unlock::load(input, ctx.password())?;
    let pages = doc::page_count(&document);
    ctx.check()?;
    ctx.report(0.3, "Removing hidden information");
    let report = clean(&mut document, opts)?;
    ctx.check()?;
    ctx.report(0.7, "Saving");
    doc::save(&mut document, out)?;
    ctx.report(1.0, "Done");

    let mut outcome = Outcome::single(out.to_path_buf(), pages, file_size(input));
    outcome.notes = report.notes();
    if report.is_empty() {
        outcome.notes.push("None of the chosen kinds of hidden information were found. The file was saved again as it is.".into());
    }
    outcome.notes.push("Earlier saved versions inside the file were dropped: it was written afresh.".into());
    if document.was_encrypted() {
        outcome.notes.push(UNPROTECTED_NOTE.into());
    }
    Ok(outcome)
}
