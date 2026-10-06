//! PDF/A: Convert to the PDF/A archival format.
//!
//! The target is PDF/A-2b. Two routes lead there:
//!
//! - **preserve** keeps the pages exactly as they are and makes the document-level changes
//!   the standard asks for: an sRGB output intent, XMP metadata that declares PDF/A-2b and
//!   mirrors the document information, a file identifier, no encryption, no JavaScript or
//!   other forbidden actions, no embedded files, and a few keys cleaned up. That is enough
//!   only when the pages themselves already qualify, which above all means every font
//!   that draws text is embedded and no device CMYK colour is used. Shorui cannot embed a
//!   missing font.
//! - **image** draws every page to a picture and builds a new document from the pictures.
//!   Nothing on such a page can break a rule, but the text can no longer be selected.
//!
//! **auto** takes the first route when `check` finds nothing in its way, otherwise the
//! second. Shorui checks the rules listed here and no others: it is not a PDF/A validator.

use crate::img::{self, Encoding};
use crate::render::{Renderer, dpi_to_scale};
use crate::{Ctx, Error, Outcome, Result, doc};
use image::DynamicImage;
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Level {
    /// PDF/A-2b: based on PDF 1.7, visual appearance preserved.
    #[default]
    #[serde(rename = "2b")]
    A2b,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Keep the pages when they qualify, otherwise turn them into images.
    #[default]
    Auto,
    /// Keep the pages as they are whatever `check` finds.
    Preserve,
    /// Turn every page into an image.
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub level: Level,
    pub mode: Mode,
    /// Resolution of the page images when pages are turned into images.
    pub dpi: u32,
    /// JPEG quality of those images, 1 to 100.
    pub quality: u8,
}

impl Default for Options {
    fn default() -> Self {
        Options { level: Level::A2b, mode: Mode::Auto, dpi: 200, quality: 85 }
    }
}

// ---------------------------------------------------------------------------
// The sRGB profile
// ---------------------------------------------------------------------------

/// A complete ICC version 2 display profile for sRGB (IEC 61966-2.1): D50-adapted
/// primaries and the sRGB tone curve as a 1024-point table shared by the three channels.
pub fn srgb_icc() -> Vec<u8> {
    fn pad(v: &mut Vec<u8>) {
        while v.len() % 4 != 0 {
            v.push(0);
        }
    }
    fn xyz(x: u32, y: u32, z: u32) -> Vec<u8> {
        let mut v = b"XYZ \0\0\0\0".to_vec();
        for n in [x, y, z] {
            v.extend_from_slice(&n.to_be_bytes());
        }
        v
    }
    let name = b"sRGB IEC61966-2.1";
    let mut desc = b"desc\0\0\0\0".to_vec();
    desc.extend_from_slice(&(name.len() as u32 + 1).to_be_bytes());
    desc.extend_from_slice(name);
    desc.push(0);
    desc.extend_from_slice(&[0u8; 4 + 4 + 2 + 1 + 67]); // no Unicode or ScriptCode name
    let mut cprt = b"text\0\0\0\0".to_vec();
    cprt.extend_from_slice(b"No copyright, use freely\0");
    let mut curve = b"curv\0\0\0\0".to_vec();
    curve.extend_from_slice(&1024u32.to_be_bytes());
    for i in 0..1024u32 {
        let v = i as f64 / 1023.0;
        let linear = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        curve.extend_from_slice(&((linear * 65535.0).round() as u16).to_be_bytes());
    }
    // s15Fixed16 values of the published sRGB profile.
    let tags: [(&[u8; 4], Vec<u8>); 7] = [
        (b"desc", desc),
        (b"cprt", cprt),
        (b"wtpt", xyz(0x0000_F351, 0x0001_0000, 0x0001_16CC)),
        (b"rXYZ", xyz(0x0000_6FA2, 0x0000_38F5, 0x0000_0390)),
        (b"gXYZ", xyz(0x0000_6299, 0x0000_B785, 0x0000_18DA)),
        (b"bXYZ", xyz(0x0000_24A0, 0x0000_0F84, 0x0000_B6CF)),
        (b"rTRC", curve),
    ];
    let tag_count = tags.len() as u32 + 2; // gTRC and bTRC point at the rTRC data
    let mut table: Vec<u8> = tag_count.to_be_bytes().to_vec();
    let mut data: Vec<u8> = Vec::new();
    let data_start = 128 + 4 + 12 * tag_count;
    let mut entry = |sig: &[u8; 4], offset: u32, len: u32| {
        table.extend_from_slice(sig);
        table.extend_from_slice(&offset.to_be_bytes());
        table.extend_from_slice(&len.to_be_bytes());
    };
    for (sig, bytes) in &tags {
        let offset = data_start + data.len() as u32;
        entry(sig, offset, bytes.len() as u32);
        if *sig == b"rTRC" {
            entry(b"gTRC", offset, bytes.len() as u32);
            entry(b"bTRC", offset, bytes.len() as u32);
        }
        data.extend_from_slice(bytes);
        pad(&mut data);
    }

    let mut header = vec![0u8; 128];
    let size = data_start + data.len() as u32;
    header[0..4].copy_from_slice(&size.to_be_bytes());
    header[8..12].copy_from_slice(&[2, 0x10, 0, 0]); // version 2.1
    header[12..16].copy_from_slice(b"mntr");
    header[16..20].copy_from_slice(b"RGB ");
    header[20..24].copy_from_slice(b"XYZ ");
    for (i, part) in [2024u16, 1, 1, 0, 0, 0].iter().enumerate() {
        header[24 + 2 * i..26 + 2 * i].copy_from_slice(&part.to_be_bytes());
    }
    header[36..40].copy_from_slice(b"acsp");
    // Rendering intent 0 (perceptual); PCS illuminant D50.
    header[68..72].copy_from_slice(&0x0000_F6D6u32.to_be_bytes());
    header[72..76].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    header[76..80].copy_from_slice(&0x0000_D32Du32.to_be_bytes());

    let mut out = header;
    out.extend_from_slice(&table);
    out.extend_from_slice(&data);
    out
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn name_of(dict: &Dictionary, key: &[u8]) -> Option<Vec<u8>> {
    dict.get(key).and_then(Object::as_name).ok().map(<[u8]>::to_vec)
}

fn deref_dict<'a>(document: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match doc::deref(document, obj) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// Visit every dictionary in an object, nested ones included.
fn walk<'a>(obj: &'a Object, f: &mut impl FnMut(&'a Dictionary)) {
    match obj {
        Object::Dictionary(d) => {
            f(d);
            for (_, v) in d.iter() {
                walk(v, f);
            }
        }
        Object::Stream(s) => {
            f(&s.dict);
            for (_, v) in s.dict.iter() {
                walk(v, f);
            }
        }
        Object::Array(items) => {
            for v in items {
                walk(v, f);
            }
        }
        _ => {}
    }
}

fn walk_mut(obj: &mut Object, f: &mut impl FnMut(&mut Dictionary)) {
    match obj {
        Object::Dictionary(d) => {
            f(d);
            for (_, v) in d.iter_mut() {
                walk_mut(v, f);
            }
        }
        Object::Stream(s) => {
            f(&mut s.dict);
            for (_, v) in s.dict.iter_mut() {
                walk_mut(v, f);
            }
        }
        Object::Array(items) => {
            for v in items.iter_mut() {
                walk_mut(v, f);
            }
        }
        _ => {}
    }
}

/// Text of a string from the document information dictionary.
fn info_text(obj: &Object) -> Option<String> {
    let bytes = obj.as_str().ok()?;
    let text = if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&bytes[3..]).to_string()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    };
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `D:20261002153000+05'30'` as `2026-10-02T15:30:00+05:30`.
fn iso_date(pdf_date: &str) -> Option<String> {
    let s = pdf_date.trim().trim_start_matches("D:");
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |from: usize, len: usize, default: &str| digits.get(from..from + len).unwrap_or(default).to_string();
    let (year, month, day) = (part(0, 4, "0000"), part(4, 2, "01"), part(6, 2, "01"));
    let (hour, minute, second) = (part(8, 2, "00"), part(10, 2, "00"), part(12, 2, "00"));
    let ok = |v: &str, lo: u32, hi: u32| v.parse::<u32>().is_ok_and(|n| (lo..=hi).contains(&n));
    if !(ok(&month, 1, 12) && ok(&day, 1, 31) && ok(&hour, 0, 23) && ok(&minute, 0, 59) && ok(&second, 0, 59)) {
        return None;
    }
    let rest = &s[digits.len()..];
    let zone = match rest.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let nums: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            let (zh, zm) = (nums.get(0..2).unwrap_or("00").to_string(), nums.get(2..4).unwrap_or("00").to_string());
            if !(ok(&zh, 0, 23) && ok(&zm, 0, 59)) {
                return None;
            }
            format!("{sign}{zh}:{zm}")
        }
        _ => "Z".to_string(),
    };
    Some(format!("{year}-{month}-{day}T{hour}:{minute}:{second}{zone}"))
}

/// The current time in UTC as `(PDF date, ISO date)`.
fn now() -> (String, String) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    (format!("D:{year:04}{month:02}{day:02}{h:02}{m:02}{s:02}Z"), format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z"))
}

// ---------------------------------------------------------------------------
// Checking what stands in the way of keeping the pages
// ---------------------------------------------------------------------------

/// What `check` found. Empty means the pages can be kept as they are.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    /// Fonts that draw visible text but are not embedded in the file.
    pub fonts_not_embedded: Vec<String>,
    /// Fonts that are not embedded but only place invisible text (an OCR layer). Allowed.
    pub invisible_text_fonts: Vec<String>,
    /// Device CMYK colour is used, which an sRGB output intent does not cover.
    pub device_cmyk: bool,
    /// Annotations with no appearance to draw them from.
    pub annotations_without_appearance: usize,
    /// Anything else Shorui cannot repair, in plain words.
    pub other: Vec<String>,
}

impl Report {
    /// True when nothing stands in the way of keeping the pages as they are.
    pub fn can_preserve(&self) -> bool {
        self.fonts_not_embedded.is_empty() && !self.device_cmyk && self.annotations_without_appearance == 0 && self.other.is_empty()
    }

    /// The problems as one sentence fragment, such as "2 fonts are not embedded (Helvetica, Courier)".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self.fonts_not_embedded.len() {
            0 => {}
            1 => parts.push(format!("the font {} is not embedded", self.fonts_not_embedded[0])),
            n => parts.push(format!("{n} fonts are not embedded ({})", self.fonts_not_embedded.join(", "))),
        }
        if self.device_cmyk {
            parts.push("it uses CMYK colour without a colour profile".to_string());
        }
        match self.annotations_without_appearance {
            0 => {}
            1 => parts.push("1 annotation has no fixed appearance".to_string()),
            n => parts.push(format!("{n} annotations have no fixed appearance")),
        }
        parts.extend(self.other.iter().cloned());
        parts.join("; ")
    }
}

/// How fonts are used by the text on the pages.
#[derive(Default)]
struct FontUse {
    visible: HashSet<ObjectId>,
    invisible: HashSet<ObjectId>,
    device_cmyk: bool,
}

#[derive(Clone, Copy)]
struct TextState {
    font: Option<ObjectId>,
    mode: i64,
}

/// Follow a content stream: which font draws text, in which rendering mode, and whether
/// device CMYK is set. Form XObjects are followed too.
fn scan_content(document: &Document, content: &[u8], resources: Option<&Dictionary>, state: TextState, usage: &mut FontUse, seen: &mut HashSet<ObjectId>, depth: usize) {
    let Ok(content) = Content::decode(content) else { return };
    let fonts = resources.and_then(|r| r.get(b"Font").ok()).and_then(|o| deref_dict(document, o));
    let xobjects = resources.and_then(|r| r.get(b"XObject").ok()).and_then(|o| deref_dict(document, o));
    let mut state = state;
    let mut stack: Vec<TextState> = Vec::new();
    for op in &content.operations {
        match op.operator.as_str() {
            "q" => stack.push(state),
            "Q" => {
                if let Some(s) = stack.pop() {
                    state = s;
                }
            }
            "Tr" => state.mode = op.operands.first().and_then(|o| o.as_i64().ok()).unwrap_or(0),
            "Tf" => {
                state.font = op.operands.first().and_then(|o| o.as_name().ok()).and_then(|name| fonts.and_then(|f| f.get(name).ok())).and_then(|o| o.as_reference().ok());
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if let Some(id) = state.font {
                    if state.mode == 3 {
                        usage.invisible.insert(id);
                    } else {
                        usage.visible.insert(id);
                    }
                }
            }
            "k" | "K" => usage.device_cmyk = true,
            "cs" | "CS" => {
                if op.operands.first().and_then(|o| o.as_name().ok()) == Some(b"DeviceCMYK") {
                    usage.device_cmyk = true;
                }
            }
            "Do" if depth < 12 => {
                let target = op.operands.first().and_then(|o| o.as_name().ok()).and_then(|name| xobjects.and_then(|x| x.get(name).ok()));
                if let Some(Object::Reference(id)) = target {
                    if let Ok(Object::Stream(form)) = document.get_object(*id) {
                        if name_of(&form.dict, b"Subtype").as_deref() == Some(b"Form") && seen.insert(*id) {
                            let data = form.decompressed_content().unwrap_or_else(|_| form.content.clone());
                            let own = form.dict.get(b"Resources").ok().and_then(|o| deref_dict(document, o)).or(resources);
                            scan_content(document, &data, own, state, usage, seen, depth + 1);
                            // The same form may be drawn again in another text mode.
                            seen.remove(id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn font_embedded(document: &Document, font: &Dictionary) -> bool {
    match name_of(font, b"Subtype").as_deref() {
        Some(b"Type3") => true,
        // A composite font is judged by its descendant, which is a font object of its own.
        Some(b"Type0") => true,
        _ => font.get(b"FontDescriptor").ok().and_then(|o| deref_dict(document, o)).is_some_and(|d| d.has(b"FontFile") || d.has(b"FontFile2") || d.has(b"FontFile3")),
    }
}

fn font_name(font: &Dictionary) -> String {
    let raw = name_of(font, b"BaseFont").or_else(|| name_of(font, b"Name")).unwrap_or_else(|| b"unnamed font".to_vec());
    let name = String::from_utf8_lossy(&raw).to_string();
    // Drop a subset tag such as "ABCDEF+".
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.chars().all(|c| c.is_ascii_uppercase()) => rest.to_string(),
        _ => name,
    }
}

fn array_mentions_cmyk(obj: &Object, depth: usize) -> bool {
    match obj {
        Object::Name(n) => n == b"DeviceCMYK",
        Object::Array(items) if depth < 3 => items.iter().any(|o| array_mentions_cmyk(o, depth + 1)),
        _ => false,
    }
}

fn inspect(document: &Document) -> Report {
    let mut report = Report::default();
    let mut usage = FontUse::default();
    let start = TextState { font: None, mode: 0 };

    // Page content, and the appearance streams of annotations.
    for page_id in doc::page_ids(document) {
        let content = document.get_page_content(page_id);
        let resources = doc::inherited(document, page_id, b"Resources");
        let resources = resources.as_ref().and_then(|o| o.as_dict().ok());
        scan_content(document, &content, resources, start, &mut usage, &mut HashSet::new(), 0);
        let annots = document.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok()).map(|o| doc::deref(document, o));
        if let Some(Object::Array(items)) = annots {
            for item in items {
                let Some(annot) = deref_dict(document, item) else { continue };
                let subtype = name_of(annot, b"Subtype").unwrap_or_default();
                // Annotations that `clean` takes out do not count against the file.
                if will_be_removed(annot) {
                    continue;
                }
                let normal = annot.get(b"AP").ok().and_then(|o| deref_dict(document, o)).and_then(|ap| ap.get(b"N").ok());
                let empty_rect = doc::rect_of(document, annot.get(b"Rect").unwrap_or(&Object::Null)).is_some_and(|r| r[2] - r[0] <= 0.0 && r[3] - r[1] <= 0.0);
                if normal.is_none() && subtype != b"Popup" && subtype != b"Link" && !empty_rect {
                    report.annotations_without_appearance += 1;
                }
                // The appearance is either one stream or one stream per state.
                let mut streams: Vec<&Stream> = Vec::new();
                match normal.map(|o| doc::deref(document, o)) {
                    Some(Object::Stream(s)) => streams.push(s),
                    Some(Object::Dictionary(states)) => {
                        for (_, v) in states.iter() {
                            if let Object::Stream(s) = doc::deref(document, v) {
                                streams.push(s);
                            }
                        }
                    }
                    _ => {}
                }
                for stream in streams {
                    let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
                    let own = stream.dict.get(b"Resources").ok().and_then(|o| deref_dict(document, o));
                    scan_content(document, &data, own, start, &mut usage, &mut HashSet::new(), 0);
                }
            }
        }
    }
    report.device_cmyk = usage.device_cmyk;

    // Every font object in the file.
    let mut missing: Vec<String> = Vec::new();
    let mut invisible: Vec<String> = Vec::new();
    let mut postscript = false;
    let mut external = false;
    let mut lzw = false;
    for (id, object) in &document.objects {
        if let Object::Stream(stream) = object {
            if stream.dict.has(b"F") && !stream.dict.has_type(b"Filespec") && !stream.dict.has_type(b"EmbeddedFile") && name_of(&stream.dict, b"Subtype").as_deref() != Some(b"Form") {
                external = true;
            }
            if stream.filters().is_ok_and(|f| f.iter().any(|n| *n == b"LZWDecode")) {
                lzw = true;
            }
        }
        let top_level_font = match object {
            Object::Dictionary(d) if d.has_type(b"Font") => Some(d),
            _ => None,
        };
        walk(object, &mut |dict| {
            if name_of(dict, b"Subtype").as_deref() == Some(b"PS") || dict.has(b"PS") {
                postscript = true;
            }
            if let Ok(space) = dict.get(b"ColorSpace") {
                if array_mentions_cmyk(space, 0) || matches!(space, Object::Dictionary(d) if d.iter().any(|(_, v)| array_mentions_cmyk(v, 0))) {
                    report.device_cmyk = true;
                }
            }
            if let Ok(space) = dict.get(b"CS") {
                if array_mentions_cmyk(space, 0) {
                    report.device_cmyk = true;
                }
            }
            if dict.has_type(b"Font") && !font_embedded(document, dict) {
                let name = font_name(dict);
                let is_top = top_level_font.is_some_and(|d| std::ptr::eq(d, dict));
                let only_invisible = is_top && usage.invisible.contains(id) && !usage.visible.contains(id);
                let list = if only_invisible { &mut invisible } else { &mut missing };
                if !list.contains(&name) {
                    list.push(name);
                }
            }
        });
    }
    missing.sort();
    invisible.sort();
    report.fonts_not_embedded = missing;
    report.invisible_text_fonts = invisible;
    if postscript {
        report.other.push("it contains PostScript code".into());
    }
    if external {
        report.other.push("some of its data is stored in a separate file".into());
    }
    if lzw {
        report.other.push("it uses LZW compression".into());
    }
    report
}

/// Look at a PDF and say what stands in the way of keeping its pages as they are in a
/// PDF/A file. An empty report means `preserve` gives a file that meets every rule Shorui
/// checks; anything else makes `auto` turn the pages into images.
pub fn check(path: &Path, password: Option<&str>) -> Result<Report> {
    Ok(inspect(&doc::load(path, password)?))
}

// ---------------------------------------------------------------------------
// Document-level changes
// ---------------------------------------------------------------------------

/// What was taken out to meet the standard.
#[derive(Default)]
struct Removed {
    encryption: bool,
    actions: usize,
    embedded_files: usize,
    annotations: usize,
    form_extras: bool,
    keys: usize,
}

const FORBIDDEN_ACTIONS: [&[u8]; 11] = [b"JavaScript", b"Launch", b"Sound", b"Movie", b"ResetForm", b"ImportData", b"Hide", b"SetOCGState", b"Rendition", b"Trans", b"GoTo3DView"];
const ALLOWED_NAMED: [&[u8]; 4] = [b"NextPage", b"PrevPage", b"FirstPage", b"LastPage"];

fn forbidden_action(dict: &Dictionary) -> bool {
    match name_of(dict, b"S") {
        Some(s) if FORBIDDEN_ACTIONS.contains(&s.as_slice()) => true,
        Some(s) if s == b"Named" => !name_of(dict, b"N").is_some_and(|n| ALLOWED_NAMED.contains(&n.as_slice())),
        _ => false,
    }
}

/// Whether an annotation is of a kind PDF/A-2 does not allow (multimedia, attached files)
/// or is never shown on screen: Invisible (1), Hidden (2) or NoView (32).
fn will_be_removed(annot: &Dictionary) -> bool {
    let subtype = name_of(annot, b"Subtype").unwrap_or_default();
    let flags = annot.get(b"F").and_then(Object::as_i64).unwrap_or(0);
    let banned = matches!(subtype.as_slice(), b"3D" | b"Sound" | b"Screen" | b"Movie" | b"FileAttachment");
    banned || (subtype != b"Popup" && flags & (1 | 2 | 32) != 0)
}

/// Remove what PDF/A-2 forbids and Shorui can remove without changing how pages look.
fn clean(document: &mut Document) -> Result<Removed> {
    let mut removed = Removed::default();

    // Encryption. Loading already decrypted the objects; make sure none is written back.
    if document.encryption_state.is_some() || document.trailer.has(b"Encrypt") {
        removed.encryption = true;
    }
    document.encryption_state = None;
    if let Some(Object::Reference(id)) = document.trailer.remove(b"Encrypt") {
        document.objects.remove(&id);
    }

    // Catalog.
    let catalog_id = document.trailer.get(b"Root").and_then(Object::as_reference)?;
    let names = document.get_dictionary(catalog_id)?.get(b"Names").ok().cloned();
    if let Some(names) = names {
        let count_files = |d: &Dictionary| -> usize {
            let mut n = 0;
            if let Ok(tree) = d.get(b"EmbeddedFiles") {
                walk_refs(document, tree, &mut |dict| {
                    if let Ok(Object::Array(pairs)) = dict.get(b"Names") {
                        n += pairs.len() / 2;
                    }
                });
            }
            n
        };
        match &names {
            Object::Reference(id) => {
                let files = document.get_dictionary(*id).map(count_files).unwrap_or(0);
                if let Ok(d) = document.get_dictionary_mut(*id) {
                    if d.remove(b"EmbeddedFiles").is_some() {
                        removed.embedded_files += files.max(1);
                    }
                    if d.remove(b"JavaScript").is_some() {
                        removed.actions += 1;
                    }
                }
            }
            Object::Dictionary(d) => {
                let files = count_files(d);
                if let Ok(Object::Dictionary(d)) = document.get_dictionary_mut(catalog_id)?.get_mut(b"Names") {
                    if d.remove(b"EmbeddedFiles").is_some() {
                        removed.embedded_files += files.max(1);
                    }
                    if d.remove(b"JavaScript").is_some() {
                        removed.actions += 1;
                    }
                }
            }
            _ => {}
        }
    }
    let acro = document.get_dictionary(catalog_id)?.get(b"AcroForm").ok().cloned();
    let fix_form = |d: &mut Dictionary, removed: &mut Removed| {
        if d.remove(b"XFA").is_some() {
            removed.form_extras = true;
        }
        d.remove(b"NeedAppearances");
    };
    match acro {
        Some(Object::Reference(id)) => {
            if let Ok(d) = document.get_dictionary_mut(id) {
                fix_form(d, &mut removed);
            }
        }
        Some(Object::Dictionary(_)) => {
            if let Ok(Object::Dictionary(d)) = document.get_dictionary_mut(catalog_id)?.get_mut(b"AcroForm") {
                fix_form(d, &mut removed);
            }
        }
        _ => {}
    }
    {
        let catalog = document.get_dictionary_mut(catalog_id)?;
        if catalog.remove(b"Requirements").is_some() {
            removed.keys += 1;
        }
    }

    // Annotations that may not appear in PDF/A-2, or that are hidden.
    for page_id in doc::page_ids(document) {
        let annots = match document.get_dictionary(page_id)?.get(b"Annots") {
            Ok(o) => doc::deref(document, o).clone(),
            Err(_) => continue,
        };
        let Object::Array(items) = annots else { continue };
        let mut kept = Vec::with_capacity(items.len());
        for item in items {
            let Some(annot) = deref_dict(document, &item) else { continue };
            let subtype = name_of(annot, b"Subtype").unwrap_or_default();
            let flags = annot.get(b"F").and_then(Object::as_i64).unwrap_or(0);
            if will_be_removed(annot) {
                removed.annotations += 1;
                if subtype == b"FileAttachment" {
                    removed.embedded_files += 1;
                }
                continue;
            }
            // Must print, and may not switch visibility.
            if subtype != b"Popup" {
                let wanted = (flags | 4) & !256;
                if wanted != flags || !annot.has(b"F") {
                    match &item {
                        Object::Reference(id) => {
                            if let Ok(d) = document.get_dictionary_mut(*id) {
                                d.set("F", wanted);
                            }
                            kept.push(item);
                        }
                        Object::Dictionary(d) => {
                            let mut d = d.clone();
                            d.set("F", wanted);
                            kept.push(Object::Dictionary(d));
                        }
                        _ => kept.push(item),
                    }
                    continue;
                }
            }
            kept.push(item);
        }
        document.get_dictionary_mut(page_id)?.set("Annots", kept);
    }

    // Actions and keys anywhere in the file.
    let bad_actions: HashSet<ObjectId> = document.objects.iter().filter(|(_, o)| matches!(o, Object::Dictionary(d) if forbidden_action(d))).map(|(id, _)| *id).collect();
    let mut lzw: Vec<ObjectId> = Vec::new();
    for (id, object) in document.objects.iter_mut() {
        if let Object::Stream(stream) = object {
            if stream.filters().is_ok_and(|f| f.iter().any(|n| *n == b"LZWDecode")) {
                lzw.push(*id);
            }
        }
        walk_mut(object, &mut |dict| {
            let is_bad = |o: &Object| match o {
                Object::Reference(id) => bad_actions.contains(id),
                Object::Dictionary(d) => forbidden_action(d),
                _ => false,
            };
            for key in [&b"A"[..], b"OpenAction", b"Next"] {
                if dict.get(key).is_ok_and(is_bad) {
                    dict.remove(key);
                    removed.actions += 1;
                }
            }
            // A form field may carry no action at all.
            if name_of(dict, b"Subtype").as_deref() == Some(b"Widget") && dict.remove(b"A").is_some() {
                removed.actions += 1;
            }
            if !dict.has_type(b"ExtGState") && dict.remove(b"AA").is_some() {
                removed.actions += 1;
            }
            // Graphics state: transfer functions and halftone origins are not allowed.
            let graphics_state = dict.has_type(b"ExtGState") || (!dict.has(b"G") && (dict.has(b"TR") || dict.has(b"TR2") || dict.has(b"HTP")) && !dict.has(b"Subtype"));
            if graphics_state {
                if dict.remove(b"TR").is_some() {
                    removed.keys += 1;
                }
                if dict.get(b"TR2").is_ok_and(|o| o.as_name().ok() != Some(b"Default")) {
                    dict.remove(b"TR2");
                    removed.keys += 1;
                }
                if dict.remove(b"HTP").is_some() {
                    removed.keys += 1;
                }
            }
            match name_of(dict, b"Subtype").as_deref() {
                Some(b"Image") => {
                    if dict.get(b"Interpolate").and_then(Object::as_bool).unwrap_or(false) {
                        dict.set("Interpolate", false);
                        removed.keys += 1;
                    }
                    for key in [&b"Alternates"[..], b"OPI"] {
                        if dict.remove(key).is_some() {
                            removed.keys += 1;
                        }
                    }
                }
                Some(b"Form") => {
                    for key in [&b"OPI"[..], b"Ref"] {
                        if dict.remove(key).is_some() {
                            removed.keys += 1;
                        }
                    }
                }
                _ => {}
            }
        });
    }
    // LZW is not allowed: store those streams with Flate instead (done when saving).
    for id in lzw {
        if let Ok(Object::Stream(stream)) = document.get_object_mut(id) {
            if stream.decompress().is_ok() {
                stream.allows_compression = true;
            }
        }
    }
    document.prune_objects();
    Ok(removed)
}

/// Visit the dictionaries of a name tree, following references.
fn walk_refs(document: &Document, obj: &Object, f: &mut impl FnMut(&Dictionary)) {
    fn inner(document: &Document, obj: &Object, f: &mut dyn FnMut(&Dictionary), depth: usize) {
        if depth > 24 {
            return;
        }
        if let Some(dict) = deref_dict(document, obj) {
            f(dict);
            if let Ok(Object::Array(kids)) = dict.get(b"Kids").map(|o| doc::deref(document, o)) {
                for kid in kids {
                    inner(document, kid, f, depth + 1);
                }
            }
        }
    }
    inner(document, obj, f, 0);
}

fn xmp_packet(info: &HashMap<&'static str, String>, created: Option<&str>, modified: &str) -> Vec<u8> {
    let mut dc = String::from("<dc:format>application/pdf</dc:format>");
    if let Some(title) = info.get("Title") {
        dc += &format!("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>", xml_escape(title));
    }
    if let Some(author) = info.get("Author") {
        dc += &format!("<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>", xml_escape(author));
    }
    if let Some(subject) = info.get("Subject") {
        dc += &format!("<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>", xml_escape(subject));
    }
    let mut xmp = String::new();
    if let Some(tool) = info.get("Creator") {
        xmp += &format!("<xmp:CreatorTool>{}</xmp:CreatorTool>", xml_escape(tool));
    }
    if let Some(created) = created {
        xmp += &format!("<xmp:CreateDate>{created}</xmp:CreateDate>");
    }
    xmp += &format!("<xmp:ModifyDate>{modified}</xmp:ModifyDate><xmp:MetadataDate>{modified}</xmp:MetadataDate>");
    let mut pdf = format!("<pdf:Producer>{}</pdf:Producer>", xml_escape(doc::PRODUCER));
    if let Some(keywords) = info.get("Keywords") {
        pdf += &format!("<pdf:Keywords>{}</pdf:Keywords>", xml_escape(keywords));
    }
    let packet = format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"><pdfaid:part>2</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance></rdf:Description>\n\
         <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">{dc}</rdf:Description>\n\
         <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">{xmp}</rdf:Description>\n\
         <rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">{pdf}</rdf:Description>\n\
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         {padding}\n\
         <?xpacket end=\"w\"?>",
        padding = " ".repeat(200)
    );
    packet.into_bytes()
}

fn text_object(text: &str) -> Object {
    if text.is_ascii() {
        Object::string_literal(text)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

/// Add what every PDF/A-2b file carries: the output intent, the XMP packet, matching
/// document information and a file identifier.
fn finalize(document: &mut Document) -> Result<()> {
    document.version = "1.7".to_string();
    document.binary_mark = vec![0xE2, 0xE3, 0xCF, 0xD3];
    let catalog_id = document.trailer.get(b"Root").and_then(Object::as_reference)?;

    // Document information, read once and written back in a form that matches the XMP.
    let old_info = document.trailer.get(b"Info").ok().and_then(|o| deref_dict(document, o)).cloned().unwrap_or_default();
    let mut info: HashMap<&'static str, String> = HashMap::new();
    for key in ["Title", "Author", "Subject", "Keywords", "Creator"] {
        if let Some(text) = old_info.get(key.as_bytes()).ok().and_then(info_text) {
            info.insert(key, text);
        }
    }
    let created_pdf = old_info.get(b"CreationDate").ok().and_then(info_text);
    let created_iso = created_pdf.as_deref().and_then(iso_date);
    let (now_pdf, now_iso) = now();
    let mut new_info = Dictionary::new();
    for (key, text) in &info {
        new_info.set(*key, text_object(text));
    }
    if let (Some(pdf), Some(_)) = (&created_pdf, &created_iso) {
        new_info.set("CreationDate", Object::string_literal(pdf.as_str()));
    }
    new_info.set("ModDate", Object::string_literal(now_pdf));
    new_info.set("Producer", Object::string_literal(doc::PRODUCER));
    // Not part of the standard, which looks at the XMP alone, but poppler-based tools
    // (pdfinfo, Okular, Evince) read the PDF/A level from this key.
    new_info.set("GTS_PDFA1Version", Object::string_literal("PDF/A-2b:2011"));
    let info_id = document.add_object(new_info);
    document.trailer.set("Info", info_id);

    // XMP. Left uncompressed so that any tool can find it.
    let packet = xmp_packet(&info, created_iso.as_deref(), &now_iso);
    let metadata = document.add_object(Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, packet).with_compression(false));

    // Output intent.
    let profile = document.add_object(Stream::new(dictionary! { "N" => 3 }, srgb_icc()));
    let intent = document.add_object(dictionary! {
        "Type" => "OutputIntent",
        "S" => "GTS_PDFA1",
        "OutputConditionIdentifier" => Object::string_literal("sRGB IEC61966-2.1"),
        "Info" => Object::string_literal("sRGB IEC61966-2.1"),
        "DestOutputProfile" => profile,
    });
    {
        let catalog = document.get_dictionary_mut(catalog_id)?;
        catalog.set("Metadata", metadata);
        catalog.set("OutputIntents", vec![Object::Reference(intent)]);
    }

    // File identifier: keep the permanent half when there is one.
    let fresh = |seed: u64| -> Vec<u8> {
        use std::hash::{Hash, Hasher};
        let mut out = Vec::with_capacity(16);
        for round in 0..2u64 {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (seed, round, &now_iso, document.objects.len(), std::process::id()).hash(&mut hasher);
            out.extend_from_slice(&hasher.finish().to_be_bytes());
        }
        out
    };
    let permanent = match document.trailer.get(b"ID") {
        Ok(Object::Array(ids)) => ids.first().and_then(|o| o.as_str().ok()).filter(|b| !b.is_empty()).map(<[u8]>::to_vec),
        _ => None,
    }
    .unwrap_or_else(|| fresh(1));
    let changing = fresh(2);
    document.trailer.set("ID", vec![Object::String(permanent, StringFormat::Hexadecimal), Object::String(changing, StringFormat::Hexadecimal)]);
    document.prune_objects();
    fill_gaps(document);
    Ok(())
}

/// Give every unused object number a null object, so that the cross-reference table is
/// written as one run. A table in several runs is valid, but poppler (pdfinfo, Evince,
/// Okular) then fails to read any dictionary holding a string longer than 128 bytes, and
/// the document information of the file, title included, silently disappears.
fn fill_gaps(document: &mut Document) {
    let used: HashSet<u32> = document.objects.keys().map(|id| id.0).collect();
    let highest = used.iter().copied().max().unwrap_or(0);
    for number in 1..highest {
        if !used.contains(&number) {
            document.objects.insert((number, 0), Object::Null);
        }
    }
    document.max_id = document.max_id.max(highest);
}

// ---------------------------------------------------------------------------
// The two routes
// ---------------------------------------------------------------------------

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn removal_note(removed: &Removed) -> Option<String> {
    let mut parts = Vec::new();
    if removed.encryption {
        parts.push("the password protection".to_string());
    }
    if removed.actions > 0 {
        parts.push(plural(removed.actions, "script or action", "scripts and actions"));
    }
    if removed.embedded_files > 0 {
        parts.push(plural(removed.embedded_files, "attached file", "attached files"));
    }
    if removed.annotations > 0 {
        parts.push(plural(removed.annotations, "hidden or multimedia annotation", "hidden or multimedia annotations"));
    }
    if removed.form_extras {
        parts.push("the XFA form data".to_string());
    }
    if removed.keys > 0 {
        parts.push(plural(removed.keys, "setting PDF/A does not allow", "settings PDF/A does not allow"));
    }
    (!parts.is_empty()).then(|| format!("Removed because PDF/A does not allow them: {}.", parts.join(", ")))
}

const VALIDATOR_NOTE: &str = "Shorui checks the main PDF/A rules but is not a full PDF/A validator. If conformance has to be proven, test the file with a validator such as veraPDF.";

fn preserve(mut document: Document, report: &Report, out: &Path) -> Result<(usize, Vec<String>)> {
    let removed = clean(&mut document)?;
    finalize(&mut document)?;
    let pages = doc::page_count(&document);
    doc::save(&mut document, out)?;
    let mut notes = vec!["Kept every page as it is and added what PDF/A-2b requires of the document: an sRGB output intent, XMP metadata and a file identifier.".to_string()];
    notes.extend(removal_note(&removed));
    if !report.invisible_text_fonts.is_empty() {
        notes.push("The invisible text layer uses a font that is not embedded. PDF/A allows that for text that is never drawn.".to_string());
    }
    if !report.can_preserve() {
        notes.push(format!("This file probably does not meet PDF/A-2b, because {}. Choose the automatic or the image mode to get a file that does.", report.summary()));
    }
    Ok((pages, notes))
}

fn rasterise(bytes: Vec<u8>, source: Option<&Document>, opts: &Options, out: &Path, ctx: &Ctx) -> Result<usize> {
    let renderer = Renderer::open(bytes, ctx.password())?;
    let count = renderer.page_count();
    if count == 0 {
        return Err(Error::damaged("This PDF has no pages."));
    }
    let (mut document, root) = doc::new_document();
    let mut pages = Vec::with_capacity(count);
    for index in 0..count {
        ctx.check()?;
        ctx.report(0.05 + 0.85 * index as f32 / count as f32, &format!("Turning page {} of {} into an image", index + 1, count));
        let (width, height) = renderer.page_size(index).unwrap_or((595.0, 842.0));
        let image = DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(renderer.render(index, dpi_to_scale(opts.dpi as f32))?).into_rgb8());
        let id = img::add_image(&mut document, &image, Encoding::Jpeg(opts.quality))?;
        let page = doc::blank_page(&mut document, width, height);
        let name = doc::ensure_xobject(&mut document, page, id)?;
        let content = document.add_object(Stream::new(Dictionary::new(), img::draw(&name, 0.0, 0.0, width, height).into_bytes()));
        document.get_dictionary_mut(page)?.set("Contents", content);
        pages.push(page);
    }
    doc::append_pages(&mut document, root, &pages)?;
    // Carry the document information over.
    if let Some(info) = source.and_then(|s| s.trailer.get(b"Info").ok().and_then(|o| deref_dict(s, o))) {
        let target = document.trailer.get(b"Info").and_then(Object::as_reference)?;
        for key in ["Title", "Author", "Subject", "Keywords", "Creator", "CreationDate"] {
            if let Ok(value @ Object::String(..)) = info.get(key.as_bytes()) {
                document.get_dictionary_mut(target)?.set(key, value.clone());
            }
        }
    }
    finalize(&mut document)?;
    ctx.report(0.95, "Writing the PDF");
    doc::save(&mut document, out)?;
    Ok(count)
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = match inputs {
        [one] => one,
        [] => return Err(Error::invalid("Choose a PDF to convert to PDF/A.")),
        _ => return Err(Error::invalid("PDF/A conversion works on one PDF at a time.")),
    };
    if !(36..=600).contains(&opts.dpi) {
        return Err(Error::invalid("The resolution must be between 36 and 600 dpi."));
    }
    if !(1..=100).contains(&opts.quality) {
        return Err(Error::invalid("JPEG quality must be between 1 and 100."));
    }
    let bytes = doc::read_file(input)?;
    let bytes_in = bytes.len() as u64;
    ctx.report(0.02, "Checking fonts, colours and annotations");
    // A file lopdf cannot read may still be one the renderer can draw.
    let loaded = match doc::load_bytes(&bytes, ctx.password()) {
        Ok(d) => Some(d),
        Err(e @ (Error::PasswordRequired | Error::WrongPassword)) => return Err(e),
        Err(e) if opts.mode == Mode::Preserve => return Err(e),
        Err(_) => None,
    };
    let report = loaded.as_ref().map(inspect);
    ctx.check()?;

    let keep = match (opts.mode, &report) {
        (Mode::Preserve, _) => true,
        (Mode::Image, _) => false,
        (Mode::Auto, Some(r)) => r.can_preserve(),
        (Mode::Auto, None) => false,
    };
    let (pages, mut notes) = match (keep, loaded) {
        (true, Some(document)) => {
            ctx.report(0.3, "Adding the PDF/A information");
            preserve(document, report.as_ref().unwrap_or(&Report::default()), out)?
        }
        (_, loaded) => {
            let pages = rasterise(bytes, loaded.as_ref(), opts, out, ctx)?;
            let why = match (opts.mode, &report) {
                (Mode::Auto, Some(r)) => format!(" because {}", r.summary()),
                (Mode::Auto, None) => " because the file's structure is damaged".to_string(),
                _ => String::new(),
            };
            let notes = vec![
                format!("Every page was turned into an image at {} dpi{why}. The pages look the same, but their text can no longer be selected or searched.", opts.dpi),
                "Added what PDF/A-2b requires of the document: an sRGB output intent, XMP metadata and a file identifier.".to_string(),
            ];
            (pages, notes)
        }
    };
    notes.push(VALIDATOR_NOTE.to_string());
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), pages, bytes_in);
    outcome.notes = notes;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_well_formed() {
        let icc = srgb_icc();
        assert_eq!(u32::from_be_bytes([icc[0], icc[1], icc[2], icc[3]]) as usize, icc.len());
        assert_eq!(icc.len() % 4, 0);
        assert_eq!(&icc[36..40], b"acsp");
        assert_eq!(&icc[12..24], b"mntrRGB XYZ ");
        let count = u32::from_be_bytes([icc[128], icc[129], icc[130], icc[131]]) as usize;
        assert_eq!(count, 9);
        let mut seen = Vec::new();
        for i in 0..count {
            let e = &icc[132 + 12 * i..144 + 12 * i];
            let offset = u32::from_be_bytes([e[4], e[5], e[6], e[7]]) as usize;
            let len = u32::from_be_bytes([e[8], e[9], e[10], e[11]]) as usize;
            assert!(offset % 4 == 0 && offset >= 132 + 12 * count && offset + len <= icc.len(), "tag {i}");
            seen.push(e[0..4].to_vec());
        }
        for tag in [b"desc", b"cprt", b"wtpt", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"] {
            assert!(seen.contains(&tag.to_vec()));
        }
        // krilla reads the header the same way a PDF library would.
        assert!(krilla::icc::ICCProfile::<3>::new(&icc).is_some());
    }

    #[test]
    fn converts_dates() {
        assert_eq!(iso_date("D:20261002153000+05'30'").as_deref(), Some("2026-10-02T15:30:00+05:30"));
        assert_eq!(iso_date("D:20240229").as_deref(), Some("2024-02-29T00:00:00Z"));
        assert_eq!(iso_date("D:20261002153000Z").as_deref(), Some("2026-10-02T15:30:00Z"));
        assert_eq!(iso_date("yesterday"), None);
        let (pdf, iso) = now();
        assert!(pdf.starts_with("D:20") && pdf.ends_with('Z') && pdf.len() == 17);
        assert_eq!(iso_date(&pdf).as_deref(), Some(iso.as_str()));
    }
}
