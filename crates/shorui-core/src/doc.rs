//! Loading, saving and the page-level plumbing every tool shares.

use crate::{Error, Result};
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId, Stream, dictionary};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

pub const PRODUCER: &str = "Shorui";

// ---------------------------------------------------------------------------
// Loading and saving
// ---------------------------------------------------------------------------

pub fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| Error::read(path, e))
}

/// Load a PDF from disk. Encrypted files open with `password`, or with an empty
/// password when none is given; otherwise `PasswordRequired` or `WrongPassword`.
pub fn load(path: &Path, password: Option<&str>) -> Result<Document> {
    load_bytes(&read_file(path)?, password)
}

pub fn load_bytes(bytes: &[u8], password: Option<&str>) -> Result<Document> {
    let head = &bytes[..bytes.len().min(1024)];
    if !head.windows(4).any(|w| w == b"%PDF") {
        return Err(Error::damaged("This is not a PDF file."));
    }
    // On 40-bit and 128-bit files an owner password has to be turned into the user
    // password first; given as is, the file would "open" with every string and stream
    // decrypted to noise.
    #[cfg(feature = "set-organise")]
    let recovered = match password {
        Some(given) => crate::tools::unlock::user_password_behind_owner(bytes, given).ok().flatten(),
        None => None,
    };
    #[cfg(feature = "set-organise")]
    let password = recovered.as_deref().or(password);
    let options = LoadOptions { password: password.map(str::to_string), ..Default::default() };
    match Document::load_mem_with_options(bytes, options) {
        Ok(mut doc) => {
            if doc.is_encrypted() {
                match password {
                    Some(p) => doc.decrypt(p).map_err(|_| Error::WrongPassword)?,
                    None => doc.decrypt("").map_err(|_| Error::PasswordRequired)?,
                }
            }
            Ok(doc)
        }
        Err(lopdf::Error::InvalidPassword) | Err(lopdf::Error::Decryption(_)) => {
            Err(if password.is_some() { Error::WrongPassword } else { Error::PasswordRequired })
        }
        Err(e) => Err(e.into()),
    }
}

/// True when the file needs a password that an empty string does not satisfy.
pub fn needs_password(bytes: &[u8]) -> bool {
    matches!(load_bytes(bytes, None), Err(Error::PasswordRequired))
}

fn prepare_for_save(doc: &mut Document) {
    if let Ok(info_id) = doc.trailer.get(b"Info").and_then(Object::as_reference) {
        if let Ok(info) = doc.get_dictionary_mut(info_id) {
            info.set("Producer", Object::string_literal(PRODUCER));
        }
    }
    doc.compress();
}

pub fn to_bytes(doc: &mut Document) -> Result<Vec<u8>> {
    prepare_for_save(doc);
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| Error::other(format!("Could not build the PDF: {e}")))?;
    Ok(out)
}

/// Save with compressed streams. Creates the parent folder. Returns the size written.
pub fn save(doc: &mut Document, path: &Path) -> Result<u64> {
    let bytes = to_bytes(doc)?;
    write_file(path, &bytes)?;
    Ok(bytes.len() as u64)
}

/// Save using object streams and a cross-reference stream, which is smaller.
pub fn save_compact(doc: &mut Document, path: &Path) -> Result<u64> {
    prepare_for_save(doc);
    let mut out = Vec::new();
    let options = lopdf::SaveOptions::builder().use_object_streams(true).use_xref_streams(true).compression_level(9).build();
    doc.save_with_options(&mut out, options).map_err(|e| Error::other(format!("Could not build the PDF: {e}")))?;
    write_file(path, &out)?;
    Ok(out.len() as u64)
}

pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::write(parent, e))?;
        }
    }
    std::fs::write(path, bytes).map_err(|e| Error::write(path, e))
}

/// What can be learned about a file without loading all of it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct QuickInfo {
    pub pages: usize,
    pub bytes: u64,
    /// The file is encrypted and an empty password does not open it.
    pub locked: bool,
    pub title: Option<String>,
}

/// Page count and size, read quickly. Never fails on a locked file: it reports `locked`.
pub fn quick_info(path: &Path) -> Result<QuickInfo> {
    let bytes = std::fs::metadata(path).map_err(|e| Error::read(path, e))?.len();
    match Document::load_metadata(path) {
        Ok(meta) => {
            let locked = meta.page_count == 0 && needs_password(&read_file(path)?);
            Ok(QuickInfo { pages: meta.page_count as usize, bytes, locked, title: meta.title })
        }
        Err(lopdf::Error::InvalidPassword) | Err(lopdf::Error::Decryption(_)) => Ok(QuickInfo { pages: 0, bytes, locked: true, title: None }),
        Err(e) => {
            // The quick path is stricter than a full load; fall back before giving up.
            match load(path, None) {
                Ok(doc) => Ok(QuickInfo { pages: page_count(&doc), bytes, locked: false, title: None }),
                Err(Error::PasswordRequired) => Ok(QuickInfo { pages: 0, bytes, locked: true, title: None }),
                Err(_) => Err(e.into()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

/// Page object ids in reading order.
pub fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

pub fn page_count(doc: &Document) -> usize {
    doc.get_pages().len()
}

pub fn deref<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    match doc.dereference(obj) {
        Ok((_, o)) => o,
        Err(_) => obj,
    }
}

/// A page attribute, following `/Parent` when the page does not carry it. Dereferenced.
pub fn inherited(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut id = page_id;
    for _ in 0..64 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(deref(doc, v).clone());
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

pub fn as_f32(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// `[x0, y0, x1, y1]` with x0 < x1 and y0 < y1.
pub fn rect_of(doc: &Document, obj: &Object) -> Option<[f32; 4]> {
    let arr = deref(doc, obj).as_array().ok()?;
    if arr.len() < 4 {
        return None;
    }
    let v: Vec<f32> = arr.iter().take(4).filter_map(|o| as_f32(deref(doc, o))).collect();
    if v.len() < 4 {
        return None;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

pub fn media_box(doc: &Document, page_id: ObjectId) -> [f32; 4] {
    inherited(doc, page_id, b"MediaBox").and_then(|o| rect_of(doc, &o)).unwrap_or([0.0, 0.0, 612.0, 792.0])
}

/// The crop box clipped to the media box. This is the area a viewer shows.
pub fn crop_box(doc: &Document, page_id: ObjectId) -> [f32; 4] {
    let m = media_box(doc, page_id);
    match inherited(doc, page_id, b"CropBox").and_then(|o| rect_of(doc, &o)) {
        Some(c) => {
            let r = [c[0].max(m[0]), c[1].max(m[1]), c[2].min(m[2]), c[3].min(m[3])];
            if r[2] > r[0] && r[3] > r[1] { r } else { m }
        }
        None => m,
    }
}

/// 0, 90, 180 or 270, clockwise.
pub fn rotation(doc: &Document, page_id: ObjectId) -> i32 {
    let r = inherited(doc, page_id, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0);
    (((r % 360) + 360) % 360) as i32 / 90 * 90
}

/// Width and height of the page as displayed, in points.
pub fn visible_size(doc: &Document, page_id: ObjectId) -> (f32, f32) {
    let b = crop_box(doc, page_id);
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    if rotation(doc, page_id) % 180 == 0 { (w, h) } else { (h, w) }
}

/// Matrix `[a b c d e f]` taking "visible space" to page user space.
///
/// Visible space is the page as the reader sees it: origin at the bottom-left corner,
/// x to the right, y up, size `visible_size`. Emit this with `cm` before drawing an
/// overlay and the overlay lands upright whatever the page's `/Rotate` and crop box are.
pub fn visible_to_page(doc: &Document, page_id: ObjectId) -> [f32; 6] {
    let b = crop_box(doc, page_id);
    match rotation(doc, page_id) {
        90 => [0.0, 1.0, -1.0, 0.0, b[2], b[1]],
        180 => [-1.0, 0.0, 0.0, -1.0, b[2], b[3]],
        270 => [0.0, -1.0, 1.0, 0.0, b[0], b[3]],
        _ => [1.0, 0.0, 0.0, 1.0, b[0], b[1]],
    }
}

/// The inverse of `visible_to_page`.
pub fn page_to_visible(doc: &Document, page_id: ObjectId) -> [f32; 6] {
    let b = crop_box(doc, page_id);
    match rotation(doc, page_id) {
        90 => [0.0, -1.0, 1.0, 0.0, -b[1], b[2]],
        180 => [-1.0, 0.0, 0.0, -1.0, b[2], b[3]],
        270 => [0.0, 1.0, -1.0, 0.0, b[3], -b[0]],
        _ => [1.0, 0.0, 0.0, 1.0, -b[0], -b[1]],
    }
}

/// Copy inherited `Resources`, `MediaBox`, `CropBox` and `Rotate` onto the page itself.
pub fn flatten_inherited(doc: &mut Document, page_id: ObjectId) -> Result<()> {
    for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
        let has = doc.get_dictionary(page_id)?.has(key);
        if !has {
            if let Some(v) = inherited(doc, page_id, key) {
                doc.get_dictionary_mut(page_id)?.set(key.to_vec(), v);
            }
        }
    }
    Ok(())
}

pub fn pages_root(doc: &Document) -> Result<ObjectId> {
    Ok(doc.catalog()?.get(b"Pages").and_then(Object::as_reference)?)
}

/// Replace the page tree with a flat one holding exactly `ordered`. Pages left out are
/// removed. A page id may not repeat; use `duplicate_page` first.
pub fn set_page_order(doc: &mut Document, ordered: &[ObjectId]) -> Result<()> {
    let all = page_ids(doc);
    for &id in &all {
        flatten_inherited(doc, id)?;
    }
    let root = pages_root(doc)?;
    let keep: HashSet<ObjectId> = ordered.iter().copied().collect();
    for &id in ordered {
        doc.get_dictionary_mut(id)?.set("Parent", Object::Reference(root));
    }
    {
        let root_dict = doc.get_dictionary_mut(root)?;
        root_dict.set("Kids", ordered.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>());
        root_dict.set("Count", ordered.len() as i64);
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            root_dict.remove(key);
        }
    }
    for id in all {
        if !keep.contains(&id) {
            doc.objects.remove(&id);
        }
    }
    doc.prune_objects();
    Ok(())
}

/// A shallow copy of a page that shares its content and resources.
pub fn duplicate_page(doc: &mut Document, page_id: ObjectId) -> Result<ObjectId> {
    flatten_inherited(doc, page_id)?;
    let mut dict = doc.get_dictionary(page_id)?.clone();
    dict.remove(b"Annots");
    Ok(doc.add_object(dict))
}

/// An empty document with a catalog and an empty page tree. Returns the page tree id.
pub fn new_document() -> (Document, ObjectId) {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.add_object(dictionary! { "Type" => "Pages", "Kids" => Vec::<Object>::new(), "Count" => 0 });
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    let info_id = doc.add_object(dictionary! { "Producer" => Object::string_literal(PRODUCER) });
    doc.trailer.set("Root", catalog_id);
    doc.trailer.set("Info", info_id);
    (doc, pages_id)
}

/// Attach pages to the end of the page tree rooted at `root`.
pub fn append_pages(doc: &mut Document, root: ObjectId, pages: &[ObjectId]) -> Result<()> {
    for &id in pages {
        doc.get_dictionary_mut(id)?.set("Parent", Object::Reference(root));
    }
    let root_dict = doc.get_dictionary_mut(root)?;
    let mut kids = root_dict.get(b"Kids").and_then(Object::as_array).cloned().unwrap_or_default();
    kids.extend(pages.iter().map(|id| Object::Reference(*id)));
    root_dict.set("Count", kids.len() as i64);
    root_dict.set("Kids", kids);
    Ok(())
}

/// Add a blank page object (not yet attached to the tree).
pub fn blank_page(doc: &mut Document, width: f32, height: f32) -> ObjectId {
    doc.add_object(dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![0.0f32.into(), 0.0f32.into(), Object::Real(width), Object::Real(height)],
        "Resources" => Dictionary::new(),
    })
}

/// Deep-copy pages from `src` into `dst`. Returns the new page ids in the order asked
/// for; they are not attached to a page tree yet (see `append_pages`). A page asked for
/// twice is copied twice and shares its resources. Links to pages that are not copied
/// are dropped.
pub fn import_pages(dst: &mut Document, src: &Document, pages: &[ObjectId]) -> Result<Vec<ObjectId>> {
    let src_pages: HashSet<ObjectId> = src.get_pages().into_values().collect();
    let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
    let mut queue: VecDeque<ObjectId> = VecDeque::new();
    let mut out = Vec::with_capacity(pages.len());
    let mut extra_copies: Vec<(ObjectId, ObjectId)> = Vec::new();

    for &page in pages {
        if !src_pages.contains(&page) {
            return Err(Error::other("Internal error: asked to copy an object that is not a page."));
        }
        let new_id = dst.new_object_id();
        if map.contains_key(&page) {
            extra_copies.push((page, new_id));
        } else {
            map.insert(page, new_id);
            queue.push_back(page);
        }
        out.push(new_id);
    }

    fn rewrite(
        obj: &mut Object, src: &Document, dst: &mut Document, src_pages: &HashSet<ObjectId>,
        map: &mut HashMap<ObjectId, ObjectId>, queue: &mut VecDeque<ObjectId>,
    ) {
        match obj {
            Object::Reference(id) => {
                if let Some(new_id) = map.get(id) {
                    *obj = Object::Reference(*new_id);
                    return;
                }
                let foreign_page = src_pages.contains(id);
                let page_tree = src.get_dictionary(*id).map(|d| d.has_type(b"Pages")).unwrap_or(false);
                if foreign_page || page_tree || !src.has_object(*id) {
                    *obj = Object::Null;
                    return;
                }
                let new_id = dst.new_object_id();
                map.insert(*id, new_id);
                queue.push_back(*id);
                *obj = Object::Reference(new_id);
            }
            Object::Array(items) => {
                for item in items.iter_mut() {
                    rewrite(item, src, dst, src_pages, map, queue);
                }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter_mut() {
                    rewrite(v, src, dst, src_pages, map, queue);
                }
            }
            Object::Stream(stream) => {
                for (_, v) in stream.dict.iter_mut() {
                    rewrite(v, src, dst, src_pages, map, queue);
                }
            }
            _ => {}
        }
    }

    let prepare_page = |src: &Document, id: ObjectId| -> Result<Object> {
        let mut dict = src.get_dictionary(id)?.clone();
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            if !dict.has(key) {
                if let Some(v) = inherited(src, id, key) {
                    dict.set(key.to_vec(), v);
                }
            }
        }
        for key in [&b"Parent"[..], b"StructParents", b"B"] {
            dict.remove(key);
        }
        Ok(Object::Dictionary(dict))
    };

    while let Some(old_id) = queue.pop_front() {
        let new_id = map[&old_id];
        let mut obj = if src_pages.contains(&old_id) { prepare_page(src, old_id)? } else { src.get_object(old_id)?.clone() };
        rewrite(&mut obj, src, dst, &src_pages, &mut map, &mut queue);
        dst.objects.insert(new_id, obj);
    }
    for (old_id, new_id) in extra_copies {
        let mut obj = prepare_page(src, old_id)?;
        if let Object::Dictionary(d) = &mut obj {
            d.remove(b"Annots");
        }
        rewrite(&mut obj, src, dst, &src_pages, &mut map, &mut queue);
        dst.objects.insert(new_id, obj);
        while let Some(old) = queue.pop_front() {
            let nid = map[&old];
            let mut o = src.get_object(old)?.clone();
            rewrite(&mut o, src, dst, &src_pages, &mut map, &mut queue);
            dst.objects.insert(nid, o);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Resources and overlays
// ---------------------------------------------------------------------------

/// The page's own, inline `Resources` dictionary, created or un-shared as needed so that
/// changing it cannot affect another page.
pub fn resources_mut(doc: &mut Document, page_id: ObjectId) -> Result<&mut Dictionary> {
    let mut res = match inherited(doc, page_id, b"Resources") {
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    // Sub-dictionaries may be shared by reference between pages: pull them inline.
    for key in [&b"Font"[..], b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"] {
        let inline = match res.get(key) {
            Ok(Object::Reference(id)) => doc.get_object(*id).ok().and_then(|o| o.as_dict().ok()).cloned(),
            _ => None,
        };
        if let Some(d) = inline {
            res.set(key.to_vec(), Object::Dictionary(d));
        }
    }
    let page = doc.get_dictionary_mut(page_id)?;
    page.set("Resources", Object::Dictionary(res));
    Ok(page.get_mut(b"Resources")?.as_dict_mut()?)
}

/// Register `value` under a fresh name in a resource category (`Font`, `XObject`,
/// `ExtGState`). Returns the name to use in the content stream, without the slash.
pub fn add_resource(doc: &mut Document, page_id: ObjectId, category: &str, prefix: &str, value: Object) -> Result<String> {
    let res = resources_mut(doc, page_id)?;
    if !matches!(res.get(category.as_bytes()), Ok(Object::Dictionary(_))) {
        res.set(category, Dictionary::new());
    }
    let cat = res.get_mut(category.as_bytes())?.as_dict_mut()?;
    let mut n = 0;
    let name = loop {
        let candidate = format!("{prefix}{n}");
        if !cat.has(candidate.as_bytes()) {
            break candidate;
        }
        n += 1;
    };
    cat.set(name.clone(), value);
    Ok(name)
}

/// Draw `content` on a page without disturbing what is there. The existing content is
/// wrapped in `q`/`Q` so it cannot leak graphics state into the overlay.
/// `under` puts the new content behind the page instead of on top.
pub fn overlay(doc: &mut Document, page_id: ObjectId, content: Vec<u8>, under: bool) -> Result<()> {
    let existing: Vec<Object> = match doc.get_dictionary(page_id)?.get(b"Contents") {
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(arr)) => arr.clone(),
            _ => vec![Object::Reference(*id)],
        },
        Ok(Object::Array(arr)) => arr.clone(),
        _ => vec![],
    };
    let mut wrapped = Vec::with_capacity(content.len() + 8);
    wrapped.extend_from_slice(b"q\n");
    wrapped.extend_from_slice(&content);
    wrapped.extend_from_slice(b"\nQ\n");
    let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
    let new = doc.add_object(Stream::new(Dictionary::new(), wrapped));
    let mut list = Vec::with_capacity(existing.len() + 3);
    if under {
        list.push(Object::Reference(new));
    }
    list.push(Object::Reference(open));
    list.extend(existing);
    list.push(Object::Reference(close));
    if !under {
        list.push(Object::Reference(new));
    }
    doc.get_dictionary_mut(page_id)?.set("Contents", list);
    Ok(())
}

/// Turn a page into a Form XObject drawn in visible space: its origin is the bottom-left
/// corner as displayed and its size is `visible_size`, with rotation already applied.
/// Returns the XObject id and that size.
pub fn page_as_xobject(doc: &mut Document, page_id: ObjectId) -> Result<(ObjectId, (f32, f32))> {
    let content = doc.get_page_content(page_id);
    let resources = inherited(doc, page_id, b"Resources").unwrap_or(Object::Dictionary(Dictionary::new()));
    let b = crop_box(doc, page_id);
    let m = page_to_visible(doc, page_id);
    let size = visible_size(doc, page_id);
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "FormType" => 1,
        "BBox" => b.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        "Matrix" => m.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        "Resources" => resources,
    };
    Ok((doc.add_object(Stream::new(dict, content)), size))
}

// ---------------------------------------------------------------------------
// Text drawing with the standard fonts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StdFont {
    #[default]
    Helvetica,
    HelveticaBold,
    Times,
    Courier,
}

impl StdFont {
    pub fn base_name(self) -> &'static str {
        match self {
            StdFont::Helvetica => "Helvetica",
            StdFont::HelveticaBold => "Helvetica-Bold",
            StdFont::Times => "Times-Roman",
            StdFont::Courier => "Courier",
        }
    }

    fn widths(self) -> Option<&'static [u16; 95]> {
        match self {
            StdFont::Helvetica => Some(&HELVETICA),
            StdFont::HelveticaBold => Some(&HELVETICA_BOLD),
            StdFont::Times => Some(&TIMES),
            StdFont::Courier => None,
        }
    }

    /// Width of `text` at `size` points.
    pub fn width(self, text: &str, size: f32) -> f32 {
        let units: u32 = text
            .chars()
            .map(|c| match self.widths() {
                None => 600,
                Some(t) => {
                    let code = c as u32;
                    if (32..127).contains(&code) { t[(code - 32) as usize] as u32 } else { 556 }
                }
            })
            .sum();
        units as f32 * size / 1000.0
    }
}

#[rustfmt::skip]
const HELVETICA: [u16; 95] = [
    278,278,355,556,556,889,667,191,333,333,389,584,278,333,278,278,
    556,556,556,556,556,556,556,556,556,556,278,278,584,584,584,556,
    1015,667,667,722,722,667,611,778,722,278,500,667,556,833,722,778,
    667,778,722,667,611,722,667,944,667,667,611,278,278,278,469,556,
    333,556,556,500,556,556,278,556,556,222,222,500,222,833,556,556,
    556,556,333,500,278,556,500,722,500,500,500,334,260,334,584,
];
#[rustfmt::skip]
const HELVETICA_BOLD: [u16; 95] = [
    278,333,474,556,556,889,722,238,333,333,389,584,278,333,278,278,
    556,556,556,556,556,556,556,556,556,556,333,333,584,584,584,611,
    975,722,722,722,722,667,611,778,722,278,556,722,611,833,722,778,
    667,778,722,667,611,722,667,944,667,667,611,333,278,333,584,556,
    333,556,611,556,611,556,333,611,611,278,278,556,278,889,611,611,
    611,611,389,556,333,611,556,778,556,556,500,389,280,389,584,
];
#[rustfmt::skip]
const TIMES: [u16; 95] = [
    250,333,408,500,500,833,778,180,333,333,500,564,250,333,250,278,
    500,500,500,500,500,500,500,500,500,500,278,278,564,564,564,444,
    921,722,667,667,722,611,556,722,722,333,389,722,611,889,722,722,
    556,722,667,556,611,722,722,944,722,722,611,333,278,333,469,500,
    333,444,500,444,500,444,333,500,500,278,278,500,278,778,500,500,
    500,500,333,389,278,500,500,722,500,500,444,480,200,480,541,
];

/// Make a standard font available on the page. Returns its resource name.
pub fn ensure_font(doc: &mut Document, page_id: ObjectId, font: StdFont) -> Result<String> {
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => font.base_name(),
        "Encoding" => "WinAnsiEncoding",
    });
    add_resource(doc, page_id, "Font", "ShF", Object::Reference(font_id))
}

/// Make a constant-alpha graphics state available on the page. Returns its resource name.
pub fn ensure_alpha(doc: &mut Document, page_id: ObjectId, alpha: f32) -> Result<String> {
    let gs = dictionary! { "Type" => "ExtGState", "ca" => Object::Real(alpha), "CA" => Object::Real(alpha) };
    add_resource(doc, page_id, "ExtGState", "ShG", Object::Dictionary(gs))
}

/// Register an XObject on the page. Returns its resource name.
pub fn ensure_xobject(doc: &mut Document, page_id: ObjectId, xobject: ObjectId) -> Result<String> {
    add_resource(doc, page_id, "XObject", "ShX", Object::Reference(xobject))
}

/// Encode text for a WinAnsi font. Characters outside Windows-1252 become `?`.
pub fn winansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            '\u{20AC}' => 0x80,
            '\u{2026}' => 0x85,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{201C}' => 0x93,
            '\u{201D}' => 0x94,
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            '\u{2122}' => 0x99,
            c if (c as u32) < 0x80 => c as u8,
            c if (0xA0..=0xFF).contains(&(c as u32)) => c as u32 as u8,
            _ => b'?',
        })
        .collect()
}

/// A PDF literal string, with parentheses, ready for a content stream.
pub fn pdf_string(text: &str) -> String {
    let mut out = String::from("(");
    for b in winansi(text) {
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push(')');
    out
}

/// A number for a content stream: no exponent, no trailing zeros.
pub fn fmt(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// `a b c d e f cm` for a matrix.
pub fn cm(m: [f32; 6]) -> String {
    format!("{} {} {} {} {} {} cm\n", fmt(m[0]), fmt(m[1]), fmt(m[2]), fmt(m[3]), fmt(m[4]), fmt(m[5]))
}

/// Set a key in the document information dictionary, creating it when missing.
pub fn set_info(doc: &mut Document, key: &str, value: &str) -> Result<()> {
    let info_id = match doc.trailer.get(b"Info").and_then(Object::as_reference) {
        Ok(id) if doc.has_object(id) => id,
        _ => {
            let id = doc.add_object(Dictionary::new());
            doc.trailer.set("Info", id);
            id
        }
    };
    doc.get_dictionary_mut(info_id)?.set(key, Object::string_literal(winansi(value)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(fmt(1.0), "1");
        assert_eq!(fmt(0.5), "0.5");
        assert_eq!(fmt(-0.00001), "0");
        assert_eq!(fmt(12.3456789), "12.3457");
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(pdf_string("a(b)c\\"), "(a\\(b\\)c\\\\)");
        assert_eq!(pdf_string("é"), "(\\351)");
    }

    #[test]
    fn widths_match_afm() {
        assert!((StdFont::Helvetica.width("Hello", 10.0) - 22.78).abs() < 0.01);
        assert!((StdFont::Courier.width("abc", 10.0) - 18.0).abs() < 0.01);
    }
}
