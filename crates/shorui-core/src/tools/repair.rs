//! Repair: Rebuild a damaged PDF.
//!
//! A PDF ends with an index (the cross-reference table) that says where every object
//! lives. When that index is wrong or missing (a download cut short, junk glued to the
//! front of the file, a careless edit) most readers give up even though the pages are
//! still in the file. Repair works in three steps:
//!
//! 1. Try to open the file strictly. If that works the file was fine; it is saved again
//!    so its index is fresh, and the notes say so.
//! 2. Otherwise scan the raw bytes for `N G obj ... endobj`, keep the last copy of each
//!    object number, find the catalog (from a `trailer` dictionary, a cross-reference
//!    stream, or failing that the object with `/Type /Catalog`), and write a fresh file
//!    with a correct index. Objects packed inside object streams are unpacked when that
//!    file is loaded. The lenient loader of `lopdf` is tried as well and whichever
//!    recovers more pages wins.
//! 3. Walk the page tree, drop pages whose content is gone, fix the page counts and save.
//!
//! A file that opens normally is only checked for a consistent page list; its pages are
//! not judged by their content. In a damaged file a page is dropped when its content
//! stream is missing or is not something a viewer could draw.
//!
//! Encrypted files: the password comes from `ctx` and the repaired copy is saved without
//! encryption. When the trailer is lost, the encryption dictionary is looked for among the
//! objects; AES-256 files can then still be opened with their password, while older
//! encryption (RC4, AES-128) also needs the file identifier that was in the trailer, so
//! those cannot be recovered and the error says so. An encrypted file that is damaged
//! *and* keeps its objects in object streams cannot be recovered either (the streams
//! cannot be decrypted before the index exists).
//!
//! Other limits: when a file was updated several times and two object streams hold the
//! same object number, which copy survives is not defined.

use crate::{Ctx, Error, Outcome, Result, doc};
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Repair has nothing to configure. The password for an encrypted input comes from `ctx`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Options {}

/// Object numbers above this are treated as noise (it is the limit the PDF standard sets).
const MAX_OBJECT_NUMBER: u32 = 8_388_607;

pub fn run(inputs: &[PathBuf], out: &Path, _opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let [input] = inputs else {
        return Err(Error::invalid("Repair works on one PDF at a time."));
    };
    ctx.report(0.0, "Reading the file");
    let bytes = doc::read_file(input)?;
    let bytes_in = bytes.len() as u64;
    if bytes.is_empty() {
        return Err(Error::damaged("This file is empty, so there is nothing to repair."));
    }
    ctx.check()?;

    let mut repaired = repair_bytes(&bytes, ctx)?;
    ctx.check()?;
    ctx.report(0.85, "Saving the repaired file");
    repaired.doc.prune_objects();
    doc::save(&mut repaired.doc, out)?;

    // The result has to open cleanly, or it is not a repair.
    let check = doc::load(out, None).map_err(|_| Error::damaged("The file was rebuilt but the result still does not open, so it cannot be repaired."))?;
    let pages = doc::page_count(&check);
    if pages == 0 {
        return Err(Error::damaged("No pages could be recovered from this file."));
    }
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), pages, bytes_in);
    outcome.notes = repaired.notes;
    Ok(outcome)
}

struct Repaired {
    doc: Document,
    notes: Vec<String>,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn repair_bytes(bytes: &[u8], ctx: &Ctx) -> Result<Repaired> {
    let password = ctx.password();
    let header_at = find(bytes, b"%PDF-", 0);

    // 1. A healthy file: strict parsing succeeds and the header is where it should be.
    if header_at == Some(0) {
        ctx.report(0.1, "Checking the file");
        let strict = LoadOptions { password: password.map(str::to_string), strict: true, ..Default::default() };
        if Document::load_mem_with_options(bytes, strict).is_ok() {
            let mut d = doc::load_bytes(bytes, password)?;
            let pages = fix_pages(&mut d, false);
            if pages.kept > 0 {
                let mut notes = Vec::new();
                if pages.changed {
                    notes.push(format!("The file opened normally, but its page list was wrong and has been rebuilt; {}.", pages.summary()));
                } else {
                    notes.push("This file opened normally. It was saved again with a fresh page index.".to_string());
                }
                if d.was_encrypted() {
                    notes.push("The repaired copy is not password protected. Use Protect to add a password again.".to_string());
                }
                return Ok(Repaired { doc: d, notes });
            }
        }
    }

    // 2. Damaged. Rebuild from the raw bytes, and let the lenient loader have a go too.
    ctx.report(0.2, "Scanning for objects");
    let scanned = scan(bytes);
    ctx.check()?;
    ctx.report(0.5, "Rebuilding the page index");

    let mut best: Option<(Document, PageFix, usize, bool)> = None;
    let mut password_error: Option<Error> = None;
    let mut unreadable_pages = 0;

    if !scanned.objects.is_empty() {
        let rebuilt = rebuild(bytes, &scanned);
        match doc::load_bytes(&rebuilt, password) {
            Ok(mut d) => {
                ensure_catalog(&mut d, scanned.info);
                let pages = fix_pages(&mut d, true);
                if pages.kept > 0 {
                    best = Some((d, pages, scanned.objects.len(), true));
                } else {
                    unreadable_pages = pages.lost;
                }
            }
            // With the identifier gone no password can open the file, and it is kinder to
            // say so than to ask for the password again.
            Err(_) if scanned.identifier_lost => {
                return Err(Error::damaged(
                    "This file is encrypted and its end is missing, including the identifier needed to decrypt it. Its contents cannot be recovered. If you have another copy of the file, use that one.",
                ));
            }
            Err(e @ (Error::PasswordRequired | Error::WrongPassword)) => password_error = Some(e),
            Err(_) => {}
        }
    }
    ctx.check()?;

    if let Some(start) = header_at {
        match doc::load_bytes(&bytes[start..], password) {
            Ok(mut d) => {
                let objects = d.objects.len();
                ensure_catalog(&mut d, None);
                let pages = fix_pages(&mut d, true);
                let better = match &best {
                    Some((_, mine, _, _)) => pages.kept > mine.kept,
                    None => pages.kept > 0,
                };
                if better {
                    best = Some((d, pages, objects, false));
                }
            }
            Err(e @ (Error::PasswordRequired | Error::WrongPassword)) => password_error = Some(e),
            Err(_) => {}
        }
    }

    let Some((d, pages, objects, from_scan)) = best else {
        if let Some(e) = password_error {
            return Err(e);
        }
        if unreadable_pages > 0 {
            return Err(Error::damaged(format!(
                "The page list was found, but the content of {} cannot be read, so nothing could be recovered. If the file was password protected, the part needed to decrypt it is missing.",
                if unreadable_pages == 1 { "its page".to_string() } else { format!("its {unreadable_pages} pages") }
            )));
        }
        return Err(Error::damaged(if header_at.is_none() && scanned.objects.is_empty() {
            "This does not look like a PDF: no PDF header and no PDF objects were found in it."
        } else if scanned.objects.is_empty() {
            "Nothing could be recovered: the file has a PDF header but no readable objects. It may be only the beginning of a larger file."
        } else {
            "No pages could be recovered. The parts of this file that describe its pages are missing or unreadable."
        }));
    };

    let mut notes = vec![format!("Rebuilt the page index from {}; {}.", plural(objects, "object", "objects"), pages.summary())];
    if let Some(start) = header_at.filter(|s| *s > 0) {
        notes.push(format!("Removed {} of unrelated data from the start of the file.", plural(start, "byte", "bytes")));
    }
    if header_at.is_none() {
        notes.push("The file had no PDF header; one was added.".to_string());
    }
    if from_scan && scanned.damaged > 0 {
        notes.push(format!("{} could not be read and {} left out.", plural(scanned.damaged, "damaged object", "damaged objects"), if scanned.damaged == 1 { "was" } else { "were" }));
    }
    if pages.orphans > 0 {
        notes.push(format!("{} found outside the page list and added at the end.", plural(pages.orphans, "page was", "pages were")));
    }
    if d.was_encrypted() {
        notes.push("The repaired copy is not password protected. Use Protect to add a password again.".to_string());
    }
    Ok(Repaired { doc: d, notes })
}

// ---------------------------------------------------------------------------
// A small tolerant reader for raw PDF syntax
// ---------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    let first = needle[0];
    let last_start = haystack.len() - needle.len();
    let mut i = from;
    while i <= last_start {
        match haystack[i..=last_start].iter().position(|b| *b == first) {
            None => return None,
            Some(off) => {
                i += off;
                if &haystack[i..i + needle.len()] == needle {
                    return Some(i);
                }
                i += 1;
            }
        }
    }
    None
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_regular(b: u8) -> bool {
    !is_space(b) && !is_delimiter(b)
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Null,
    Bool,
    Int(i64),
    Real,
    Name(Vec<u8>),
    Str,
    Ref(u32, u16),
    Array(Vec<Val>),
    Dict(Vec<Entry>),
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    key: Vec<u8>,
    val: Val,
    /// Where the `/Key` starts and where its value starts and ends, in the file.
    key_start: usize,
    val_start: usize,
    end: usize,
}

impl Val {
    fn get(&self, key: &[u8]) -> Option<&Entry> {
        match self {
            // The last one wins, as it does in readers.
            Val::Dict(entries) => entries.iter().rev().find(|e| e.key == key),
            _ => None,
        }
    }
    fn name_is(&self, key: &[u8], name: &[u8]) -> bool {
        matches!(self.get(key), Some(Entry { val: Val::Name(n), .. }) if n == name)
    }
    fn reference(&self, key: &[u8]) -> Option<(u32, u16)> {
        match self.get(key) {
            Some(Entry { val: Val::Ref(n, g), .. }) => Some((*n, *g)),
            _ => None,
        }
    }
}

struct Lexer<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.peek() {
            if is_space(c) {
                self.pos += 1;
            } else if c == b'%' {
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn starts_with(&self, word: &[u8]) -> bool {
        self.b.get(self.pos..).is_some_and(|rest| rest.starts_with(word))
    }

    /// A keyword followed by something that ends it.
    fn keyword(&self, word: &[u8]) -> bool {
        self.starts_with(word) && self.b.get(self.pos + word.len()).is_none_or(|c| !is_regular(*c))
    }

    fn unsigned(&mut self) -> Option<u64> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.pos == start || self.pos - start > 15 {
            self.pos = start;
            return None;
        }
        std::str::from_utf8(&self.b[start..self.pos]).ok()?.parse().ok()
    }

    fn value(&mut self, depth: usize) -> Option<Val> {
        if depth > 96 {
            return None;
        }
        self.skip_space();
        let c = self.peek()?;
        match c {
            b'<' if self.starts_with(b"<<") => {
                self.pos += 2;
                let mut entries = Vec::new();
                loop {
                    self.skip_space();
                    if self.starts_with(b">>") {
                        self.pos += 2;
                        return Some(Val::Dict(entries));
                    }
                    if self.peek()? != b'/' {
                        return None;
                    }
                    let key_start = self.pos;
                    let key = self.name()?;
                    self.skip_space();
                    let val_start = self.pos;
                    let val = self.value(depth + 1)?;
                    entries.push(Entry { key, val, key_start, val_start, end: self.pos });
                }
            }
            b'<' => {
                self.pos += 1;
                loop {
                    let c = self.peek()?;
                    self.pos += 1;
                    if c == b'>' {
                        return Some(Val::Str);
                    }
                    if !(c.is_ascii_hexdigit() || is_space(c)) {
                        return None;
                    }
                }
            }
            b'(' => {
                self.pos += 1;
                let mut nesting = 1usize;
                loop {
                    let c = self.peek()?;
                    self.pos += 1;
                    match c {
                        b'\\' => self.pos += 1,
                        b'(' => nesting += 1,
                        b')' => {
                            nesting -= 1;
                            if nesting == 0 {
                                return Some(Val::Str);
                            }
                        }
                        _ => {}
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    if self.peek()? == b']' {
                        self.pos += 1;
                        return Some(Val::Array(items));
                    }
                    items.push(self.value(depth + 1)?);
                }
            }
            b'/' => Some(Val::Name(self.name()?)),
            b'+' | b'-' | b'.' | b'0'..=b'9' => {
                let start = self.pos;
                self.pos += 1;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+') {
                    self.pos += 1;
                }
                let text = std::str::from_utf8(&self.b[start..self.pos]).ok()?;
                let Ok(int) = text.parse::<i64>() else {
                    return text.parse::<f64>().ok().map(|_| Val::Real);
                };
                // `12 0 R` is a reference; look ahead for it.
                if int >= 0 && c.is_ascii_digit() {
                    let after_number = self.pos;
                    self.skip_space();
                    if let Some(generation) = self.unsigned() {
                        self.skip_space();
                        if self.keyword(b"R") {
                            self.pos += 1;
                            if int <= MAX_OBJECT_NUMBER as i64 && generation <= u16::MAX as u64 {
                                return Some(Val::Ref(int as u32, generation as u16));
                            }
                            return Some(Val::Null);
                        }
                    }
                    self.pos = after_number;
                }
                Some(Val::Int(int))
            }
            _ => {
                for (word, val) in [(&b"true"[..], Val::Bool), (b"false", Val::Bool), (b"null", Val::Null)] {
                    if self.keyword(word) {
                        self.pos += word.len();
                        return Some(val);
                    }
                }
                None
            }
        }
    }

    fn name(&mut self) -> Option<Vec<u8>> {
        if self.peek()? != b'/' {
            return None;
        }
        self.pos += 1;
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        Some(self.b[start..self.pos].to_vec())
    }
}

// ---------------------------------------------------------------------------
// Scanning for objects
// ---------------------------------------------------------------------------

struct RawObject {
    generation: u16,
    val: Val,
    /// The object's value: everything for a plain object, the dictionary for a stream.
    span: (usize, usize),
    /// Stream data, when the object is a stream.
    data: Option<(usize, usize)>,
    /// Where the stream data starts, kept so an indirect `/Length` can be applied later.
    data_start: usize,
}

#[derive(Default)]
struct Scan {
    objects: BTreeMap<u32, RawObject>,
    /// Objects whose header was found but whose body could not be read.
    damaged: usize,
    root: Option<(u32, u16)>,
    info: Option<(u32, u16)>,
    /// Raw text of the trailer's `/Encrypt` and `/ID` values, needed to decrypt.
    encrypt: Option<Vec<u8>>,
    id: Option<Vec<u8>>,
    /// The file is encrypted, its trailer is lost, and its kind of encryption cannot be
    /// undone without the file identifier that was in that trailer.
    identifier_lost: bool,
}

/// If `obj` at `at` is preceded by `N G `, return the object number, generation and
/// where the header starts.
fn header_before(b: &[u8], at: usize) -> Option<(u32, u16)> {
    if b.get(at + 3).is_some_and(|c| is_regular(*c)) {
        return None;
    }
    let mut i = at;
    let digits_back = |i: &mut usize| -> Option<u64> {
        let end = *i;
        while *i > 0 && b[*i - 1].is_ascii_digit() {
            *i -= 1;
        }
        if *i == end || end - *i > 10 {
            return None;
        }
        std::str::from_utf8(&b[*i..end]).ok()?.parse().ok()
    };
    let space_back = |i: &mut usize| -> bool {
        let end = *i;
        while *i > 0 && is_space(b[*i - 1]) {
            *i -= 1;
        }
        *i < end
    };
    if !space_back(&mut i) {
        return None;
    }
    let generation = digits_back(&mut i)?;
    if !space_back(&mut i) {
        return None;
    }
    let number = digits_back(&mut i)?;
    if i > 0 && is_regular(b[i - 1]) {
        return None;
    }
    if number == 0 || number > MAX_OBJECT_NUMBER as u64 || generation > u16::MAX as u64 {
        return None;
    }
    Some((number as u32, generation as u16))
}

/// Does `endstream` follow at `pos`, allowing for the line break before it?
fn endstream_at(b: &[u8], pos: usize) -> Option<usize> {
    let mut p = pos;
    let mut skipped = 0;
    while skipped < 4 && b.get(p).is_some_and(|c| is_space(*c)) {
        p += 1;
        skipped += 1;
    }
    b.get(p..)?.starts_with(b"endstream").then_some(p + 9)
}

fn scan(b: &[u8]) -> Scan {
    let mut out = Scan::default();
    // (offset, dictionary) of everything that can serve as a trailer.
    let mut trailers: Vec<(usize, Val)> = Vec::new();

    let mut pos = 0;
    while let Some(at) = find(b, b"obj", pos) {
        pos = at + 3;
        let Some((number, generation)) = header_before(b, at) else {
            continue;
        };
        let mut lx = Lexer { b, pos: at + 3 };
        lx.skip_space();
        let start = lx.pos;
        let Some(val) = lx.value(0) else {
            out.damaged += 1;
            continue;
        };
        let end = lx.pos;
        lx.skip_space();
        let is_stream = matches!(val, Val::Dict(_)) && lx.starts_with(b"stream");
        if !is_stream {
            pos = end;
            out.objects.insert(number, RawObject { generation, val, span: (start, end), data: None, data_start: 0 });
            continue;
        }

        let mut data_start = lx.pos + 6;
        if b.get(data_start..).is_some_and(|r| r.starts_with(b"\r\n")) {
            data_start += 2;
        } else if matches!(b.get(data_start), Some(b'\n' | b'\r')) {
            data_start += 1;
        }
        // Trust /Length when `endstream` really follows the data it describes.
        let by_length = match val.get(b"Length") {
            Some(Entry { val: Val::Int(n), .. }) if *n >= 0 => {
                let data_end = data_start.saturating_add(*n as usize);
                endstream_at(b, data_end).map(|after| (data_end, after))
            }
            _ => None,
        };
        let found = by_length.or_else(|| {
            let marker = find(b, b"endstream", data_start)?;
            let mut data_end = marker;
            if data_end >= data_start + 2 && &b[data_end - 2..data_end] == b"\r\n" {
                data_end -= 2;
            } else if data_end > data_start && matches!(b[data_end - 1], b'\n' | b'\r') {
                data_end -= 1;
            }
            Some((data_end, marker + 9))
        });
        let Some((data_end, after)) = found else {
            // The stream runs off the end of the file: it cannot be trusted.
            out.damaged += 1;
            pos = data_start;
            continue;
        };
        pos = after;
        if val.name_is(b"Type", b"XRef") {
            // An old index. It is not copied, but it names the catalog.
            trailers.push((at, val));
            continue;
        }
        out.objects.insert(number, RawObject { generation, val, span: (start, end), data: Some((data_start, data_end)), data_start });
    }

    // A /Length that points at another object could not be used on the way through.
    let lengths: Vec<(u32, usize)> = out
        .objects
        .iter()
        .filter(|(_, o)| o.data.is_some())
        .filter_map(|(n, o)| {
            let (target, _) = o.val.reference(b"Length")?;
            match out.objects.get(&target)?.val {
                Val::Int(len) if len >= 0 => Some((*n, len as usize)),
                _ => None,
            }
        })
        .collect();
    for (number, len) in lengths {
        if let Some(o) = out.objects.get_mut(&number) {
            let data_end = o.data_start.saturating_add(len);
            if endstream_at(b, data_end).is_some() {
                o.data = Some((o.data_start, data_end));
            }
        }
    }

    let mut from = 0;
    while let Some(at) = find(b, b"trailer", from) {
        from = at + 7;
        let mut lx = Lexer { b, pos: at + 7 };
        if let Some(val @ Val::Dict(_)) = lx.value(0) {
            trailers.push((at, val));
        }
    }
    trailers.sort_by_key(|(at, _)| *at);

    // The newest trailer whose catalog is among the recovered objects; failing that the
    // newest trailer at all (its catalog may sit inside an object stream).
    let with_root: Vec<&(usize, Val)> = trailers.iter().filter(|(_, t)| t.reference(b"Root").is_some()).collect();
    let chosen = with_root
        .iter()
        .rev()
        .find(|(_, t)| t.reference(b"Root").is_some_and(|(n, _)| out.objects.contains_key(&n)))
        .or(with_root.last())
        .copied();
    if let Some((_, trailer)) = chosen {
        out.root = trailer.reference(b"Root");
        out.info = trailer.reference(b"Info");
        out.encrypt = trailer.get(b"Encrypt").map(|e| b[e.val_start..e.end].to_vec());
        out.id = trailer.get(b"ID").map(|e| b[e.val_start..e.end].to_vec());
    }
    if out.encrypt.is_none() {
        // The trailer of an encrypted file is gone, but its encryption dictionary may not
        // be. Without it every string and stream would be read as nonsense.
        let found = out
            .objects
            .iter()
            .filter(|(_, o)| o.data.is_none() && o.val.name_is(b"Filter", b"Standard") && o.val.get(b"O").is_some() && o.val.get(b"U").is_some())
            .max_by_key(|(_, o)| o.span.0);
        if let Some((n, o)) = found {
            out.encrypt = Some(format!("{n} {} R", o.generation).into_bytes());
            // Revisions before 5 mix the file identifier from the trailer into the key.
            out.identifier_lost = !matches!(o.val.get(b"R"), Some(Entry { val: Val::Int(r), .. }) if *r >= 5);
        }
    }
    if out.root.is_none() {
        // No trailer survived: look for the catalog itself. The last one in the file wins.
        out.root = out
            .objects
            .iter()
            .filter(|(_, o)| o.val.name_is(b"Type", b"Catalog"))
            .max_by_key(|(_, o)| o.span.0)
            .map(|(n, o)| (*n, o.generation));
    }
    out
}

/// Write the recovered objects out again as a plain PDF with a correct index.
fn rebuild(b: &[u8], scan: &Scan) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(b.len() + scan.objects.len() * 32 + 256);
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets: Vec<(u32, u16, usize)> = Vec::with_capacity(scan.objects.len());
    for (number, o) in &scan.objects {
        offsets.push((*number, o.generation, out.len()));
        out.extend_from_slice(format!("{number} {} obj\n", o.generation).as_bytes());
        match o.data {
            None => out.extend_from_slice(&b[o.span.0..o.span.1]),
            Some((data_start, data_end)) => {
                // The dictionary again, with /Length replaced by what was actually found.
                let (dict_start, dict_end) = o.span;
                let mut cursor = dict_start;
                if let Val::Dict(entries) = &o.val {
                    for e in entries.iter().filter(|e| e.key == b"Length") {
                        out.extend_from_slice(&b[cursor..e.key_start]);
                        cursor = e.end;
                    }
                }
                let close = dict_end.saturating_sub(2).max(cursor);
                out.extend_from_slice(&b[cursor..close]);
                out.extend_from_slice(format!(" /Length {}>>\nstream\n", data_end - data_start).as_bytes());
                out.extend_from_slice(&b[data_start..data_end]);
                out.extend_from_slice(b"\nendstream");
            }
        }
        out.extend_from_slice(b"\nendobj\n");
    }

    let xref_at = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
    let mut i = 0;
    while i < offsets.len() {
        let mut j = i;
        while j + 1 < offsets.len() && offsets[j + 1].0 == offsets[j].0 + 1 {
            j += 1;
        }
        out.extend_from_slice(format!("{} {}\n", offsets[i].0, j - i + 1).as_bytes());
        for (_, generation, offset) in &offsets[i..=j] {
            out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
        }
        i = j + 1;
    }
    let size = offsets.last().map(|o| o.0 + 1).unwrap_or(1);
    out.extend_from_slice(format!("trailer\n<< /Size {size}").as_bytes());
    if let Some((n, g)) = scan.root {
        out.extend_from_slice(format!(" /Root {n} {g} R").as_bytes());
    }
    if let Some((n, g)) = scan.info {
        out.extend_from_slice(format!(" /Info {n} {g} R").as_bytes());
    }
    if let Some(encrypt) = &scan.encrypt {
        out.extend_from_slice(b" /Encrypt ");
        out.extend_from_slice(encrypt);
    }
    if let Some(id) = &scan.id {
        out.extend_from_slice(b" /ID ");
        out.extend_from_slice(id);
    }
    out.extend_from_slice(format!(" >>\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

// ---------------------------------------------------------------------------
// The catalog and the page tree
// ---------------------------------------------------------------------------

/// Make sure the trailer points at a usable catalog, looking for one or making one.
fn ensure_catalog(d: &mut Document, info: Option<(u32, u16)>) {
    let usable = |d: &Document, id: ObjectId| d.get_dictionary(id).is_ok_and(|c| c.has(b"Pages") || c.has_type(b"Catalog"));
    let current = d.trailer.get(b"Root").and_then(Object::as_reference).ok();
    if !current.is_some_and(|id| usable(d, id)) {
        // Object streams are unpacked by now, so a catalog hidden in one is visible.
        let found = d.objects.iter().rev().find(|(_, o)| o.as_dict().is_ok_and(|c| c.has_type(b"Catalog") && c.has(b"Pages"))).map(|(id, _)| *id);
        let id = match found {
            Some(id) => id,
            None => {
                // No catalog at all. If the top of the page tree survived, hang it on a new one.
                let top = d
                    .objects
                    .iter()
                    .filter(|(_, o)| o.as_dict().is_ok_and(|n| n.has_type(b"Pages") && n.has(b"Kids") && !n.has(b"Parent")))
                    .max_by_key(|(_, o)| o.as_dict().ok().and_then(|n| n.get(b"Count").and_then(Object::as_i64).ok()).unwrap_or(0))
                    .map(|(id, _)| *id);
                match top {
                    Some(pages) => d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages }),
                    None => d.add_object(dictionary! { "Type" => "Catalog" }),
                }
            }
        };
        d.trailer.set("Root", id);
    }
    if !d.trailer.has(b"Info") {
        let known = info.filter(|id| d.get_dictionary(*id).is_ok());
        let guessed = known.or_else(|| {
            d.objects
                .iter()
                .rev()
                .find(|(_, o)| o.as_dict().is_ok_and(|i| !i.has(b"Type") && !i.has(b"Subtype") && !i.has(b"Parent") && (i.has(b"Producer") || i.has(b"Creator"))))
                .map(|(id, _)| *id)
        });
        if let Some(id) = guessed {
            d.trailer.set("Info", id);
        }
    }
}

#[derive(Debug, Default)]
struct PageFix {
    kept: usize,
    /// Pages the file says it has but that could not be read.
    lost: usize,
    orphans: usize,
    /// The page tree had to be rewritten.
    changed: bool,
}

impl PageFix {
    fn summary(&self) -> String {
        if self.lost == 0 {
            match self.kept {
                1 => "the 1 page was recovered".to_string(),
                n => format!("all {n} pages recovered"),
            }
        } else {
            format!("{} of {} pages recovered, {} could not be read", self.kept, self.kept + self.lost, self.lost)
        }
    }
}

const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Does a content stream hold something a viewer could draw? Garbage here usually means
/// the stream is still encrypted, or that its bytes belong to something else.
fn stream_readable(s: &lopdf::Stream) -> bool {
    use std::io::Read;
    if s.content.is_empty() {
        return true;
    }
    let filters: Vec<&[u8]> = s.filters().unwrap_or_default();
    match filters.as_slice() {
        [] => {
            // Stored raw: content streams are text.
            let sample = &s.content[..s.content.len().min(2048)];
            let texty = sample.iter().filter(|b| matches!(**b, 9 | 10 | 13 | 32..=126)).count();
            texty * 10 >= sample.len() * 7
        }
        [b"FlateDecode"] => {
            // A stream that breaks off part-way still shows what came before the break.
            let mut out = Vec::new();
            let result = flate2::read::ZlibDecoder::new(s.content.as_slice()).take(256 << 20).read_to_end(&mut out);
            result.is_ok() || !out.is_empty()
        }
        _ => match s.decompressed_content() {
            Ok(data) => !data.is_empty(),
            Err(lopdf::Error::Unimplemented(_)) => true,
            Err(_) => false,
        },
    }
}

/// Can every content stream of this page be found and decoded?
fn content_readable(d: &Document, page: &Dictionary) -> bool {
    let Ok(contents) = page.get(b"Contents") else {
        return true; // a blank page
    };
    let stream_ok = |obj: &Object| -> bool {
        let Ok(id) = obj.as_reference() else {
            return false;
        };
        matches!(d.get_object(id), Ok(Object::Stream(s)) if stream_readable(s))
    };
    match contents {
        Object::Array(items) => items.iter().all(stream_ok),
        Object::Reference(id) => match d.get_object(*id) {
            Ok(Object::Array(items)) => items.iter().all(stream_ok),
            _ => stream_ok(contents),
        },
        Object::Null => true,
        _ => false,
    }
}

struct Walk {
    good: Vec<(ObjectId, Vec<(&'static [u8], Object)>)>,
    lost: usize,
    problems: bool,
    seen: HashSet<ObjectId>,
    /// Also drop pages whose content cannot be read. Off for files that opened normally:
    /// there the page list is checked, but no page is judged by its content.
    check_content: bool,
}

fn walk(d: &Document, node: &Object, inherited: &[(&'static [u8], Object)], depth: usize, w: &mut Walk) -> usize {
    let Ok(id) = node.as_reference() else {
        w.problems = true;
        return 0;
    };
    if depth > 64 || !w.seen.insert(id) {
        w.problems = true;
        return 0;
    }
    let Ok(dict) = d.get_dictionary(id) else {
        w.problems = true;
        w.lost += 1;
        return 0;
    };
    let kids = match dict.get(b"Kids") {
        Ok(k) => doc::deref(d, k).as_array().ok(),
        Err(_) => None,
    };
    if let Some(kids) = kids {
        let mut passed: Vec<(&'static [u8], Object)> = inherited.to_vec();
        for key in INHERITED {
            if let Ok(v) = dict.get(key) {
                passed.retain(|(k, _)| *k != key);
                passed.push((key, doc::deref(d, v).clone()));
            }
        }
        let mut count = 0;
        for kid in kids {
            count += walk(d, kid, &passed, depth + 1, w);
        }
        if dict.get(b"Count").and_then(Object::as_i64).ok() != Some(count as i64) {
            w.problems = true;
        }
        return count;
    }
    if dict.has_type(b"Pages") {
        // A node with no children: nothing to show, nothing lost.
        w.problems = true;
        return 0;
    }
    let is_page = dict.has_type(b"Page") || (!dict.has(b"Type") && (dict.has(b"Contents") || dict.has(b"MediaBox")));
    if !is_page || (w.check_content && !content_readable(d, dict)) {
        w.problems = true;
        w.lost += 1;
        return 0;
    }
    w.good.push((id, inherited.to_vec()));
    1
}

/// Check the page tree and rewrite it as a flat list when anything about it is wrong.
fn fix_pages(d: &mut Document, check_content: bool) -> PageFix {
    let Ok(catalog_id) = d.trailer.get(b"Root").and_then(Object::as_reference) else {
        return PageFix::default();
    };
    let pages_ref = d.get_dictionary(catalog_id).ok().and_then(|c| c.get(b"Pages").ok().cloned());
    let mut w = Walk { good: Vec::new(), lost: 0, problems: false, seen: HashSet::new(), check_content };
    match &pages_ref {
        Some(node) => {
            walk(d, node, &[], 0, &mut w);
            // The root says how many pages there should be; anything short of that is lost.
            let declared = node.as_reference().ok().and_then(|id| d.get_dictionary(id).ok()).and_then(|n| n.get(b"Count").and_then(Object::as_i64).ok()).unwrap_or(0);
            let declared = declared.clamp(0, 1_000_000) as usize;
            if declared > w.good.len() + w.lost {
                w.lost = declared - w.good.len();
            }
        }
        None => w.problems = true,
    }

    // Pages cut loose because the node above them was lost.
    let reached: HashSet<ObjectId> = w.good.iter().map(|(id, _)| *id).collect();
    let is_loose_page = |id: &ObjectId, o: &Object, need_lost_parent: bool| -> bool {
        let Ok(dict) = o.as_dict() else {
            return false;
        };
        if !dict.has_type(b"Page") || reached.contains(id) || w.seen.contains(id) {
            return false;
        }
        let parent_alive = dict.get(b"Parent").and_then(Object::as_reference).ok().and_then(|p| d.get_dictionary(p).ok()).is_some_and(|p| p.has(b"Kids"));
        (!need_lost_parent || !parent_alive) && (!check_content || content_readable(d, dict))
    };
    let mut orphans: Vec<ObjectId> = d.objects.iter().filter(|(id, o)| is_loose_page(id, o, true)).map(|(id, _)| *id).collect();
    if w.good.is_empty() && orphans.is_empty() {
        orphans = d.objects.iter().filter(|(id, o)| is_loose_page(id, o, false)).map(|(id, _)| *id).collect();
    }
    let orphan_count = orphans.len();
    let lost = w.lost.saturating_sub(orphan_count);

    if !w.problems && orphans.is_empty() {
        return PageFix { kept: w.good.len(), lost: 0, orphans: 0, changed: false };
    }
    if w.good.is_empty() && orphans.is_empty() {
        return PageFix { kept: 0, lost, orphans: 0, changed: false };
    }

    // Rewrite as one flat list, pushing inherited attributes down onto each page.
    let root = match pages_ref.as_ref().and_then(|r| r.as_reference().ok()).filter(|id| d.get_dictionary(*id).is_ok()) {
        Some(id) => id,
        None => d.add_object(Dictionary::new()),
    };
    let mut kids: Vec<Object> = Vec::new();
    for (id, inherited) in &w.good {
        if let Ok(page) = d.get_dictionary_mut(*id) {
            for (key, value) in inherited {
                if !page.has(key) {
                    page.set(key.to_vec(), value.clone());
                }
            }
            page.set("Type", "Page");
            page.set("Parent", Object::Reference(root));
            kids.push(Object::Reference(*id));
        }
    }
    for id in &orphans {
        if let Ok(page) = d.get_dictionary_mut(*id) {
            if !page.has(b"MediaBox") {
                page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
            }
            page.set("Parent", Object::Reference(root));
            kids.push(Object::Reference(*id));
        }
    }
    let kept = kids.len();
    if let Ok(node) = d.get_dictionary_mut(root) {
        node.set("Type", "Pages");
        node.set("Count", kept as i64);
        node.set("Kids", kids);
        node.remove(b"Parent");
        for key in INHERITED {
            node.remove(key);
        }
    }
    if let Ok(catalog) = d.get_dictionary_mut(catalog_id) {
        catalog.set("Type", "Catalog");
        catalog.set("Pages", Object::Reference(root));
    }
    PageFix { kept, lost, orphans: orphan_count, changed: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &[u8]) -> Option<Val> {
        Lexer { b: text, pos: 0 }.value(0)
    }

    #[test]
    fn lexer_reads_nested_values() {
        let v = parse(b"<< /Type /Page /Kids [ 3 0 R 4 0 R ] /N 12 /S (a \\) b (c)) /H <48 49> /R 1.5 /B true >>").unwrap();
        assert!(v.name_is(b"Type", b"Page"));
        assert_eq!(v.get(b"Kids").unwrap().val, Val::Array(vec![Val::Ref(3, 0), Val::Ref(4, 0)]));
        assert_eq!(v.get(b"N").unwrap().val, Val::Int(12));
        assert_eq!(v.get(b"S").unwrap().val, Val::Str);
        assert_eq!(v.get(b"R").unwrap().val, Val::Real);
        // Two integers in a row are not a reference.
        assert_eq!(parse(b"[1 2 3]").unwrap(), Val::Array(vec![Val::Int(1), Val::Int(2), Val::Int(3)]));
        assert!(parse(b"<< /Type /Page").is_none());
    }

    #[test]
    fn scan_keeps_the_last_copy_and_fixes_lengths() {
        let file = b"junk 1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
            3 0 obj\n<< /Length 999 >>\nstream\nBT ET\nendstream\nendobj\n\
            3 0 obj\n<< /Length 4 0 R >>\nstream\nq Q q Q\nendstream\nendobj\n\
            4 0 obj\n7\nendobj\n\
            5 0 obj\n<< /Length 50 >>\nstream\ncut off";
        let s = scan(file);
        assert_eq!(s.objects.len(), 3);
        assert_eq!(s.damaged, 1);
        assert_eq!(s.root, Some((1, 0)));
        let stream = &s.objects[&3];
        let (a, b) = stream.data.unwrap();
        assert_eq!(&file[a..b], b"q Q q Q");
        let rebuilt = rebuild(file, &s);
        let text = String::from_utf8_lossy(&rebuilt);
        assert!(text.contains("/Length 7>>\nstream\nq Q q Q\nendstream"), "{text}");
        assert!(text.contains("/Root 1 0 R"));
    }

    #[test]
    fn headers_need_two_numbers() {
        assert_eq!(header_before(b"12 0 obj", 5), Some((12, 0)));
        assert_eq!(header_before(b"endobj", 3), None);
        assert_eq!(header_before(b"x12 0 obj", 6), None);
        assert_eq!(header_before(b"12 0 object", 5), None);
    }
}
