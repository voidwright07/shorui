//! CSV to PDF: Lay out a CSV file as a table in a PDF.
//!
//! Pure Rust, no helper program. The file is read as UTF-8 (UTF-16 with a byte order mark
//! and Windows-1252 are also understood), split by RFC 4180 rules, and drawn with the
//! standard Helvetica fonts. Columns take the width their content needs; when the table
//! is wider than the page, long cells wrap and, as a last resort, the text gets smaller.
//! The header row repeats at the top of every page. Columns of numbers are right-aligned.

use crate::{Ctx, Error, Outcome, Result, doc, helpers};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Delimiter {
    /// Guess from the first lines of the file (`.tsv` files are always tab-separated).
    #[default]
    Auto,
    Comma,
    Semicolon,
    Tab,
    Pipe,
}

impl Delimiter {
    fn byte(self) -> Option<char> {
        match self {
            Delimiter::Auto => None,
            Delimiter::Comma => Some(','),
            Delimiter::Semicolon => Some(';'),
            Delimiter::Tab => Some('\t'),
            Delimiter::Pipe => Some('|'),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Paper {
    #[default]
    A4,
    Letter,
    A3,
    Legal,
}

impl Paper {
    /// Portrait width and height in points.
    fn points(self) -> (f32, f32) {
        match self {
            Paper::A4 => (595.276, 841.89),
            Paper::Letter => (612.0, 792.0),
            Paper::A3 => (841.89, 1190.551),
            Paper::Legal => (612.0, 1008.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    /// Landscape when the table does not fit across a portrait page.
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub delimiter: Delimiter,
    /// The first row holds column names: it is drawn in bold and repeated on every page.
    pub header: bool,
    pub paper: Paper,
    pub orientation: Orientation,
    /// Text size in points. Very wide tables may be drawn smaller, down to 5 pt.
    pub font_size: f32,
    /// White space around the table, in points.
    pub margin: f32,
    /// Lines around every cell.
    pub grid: bool,
    /// Shade every other row.
    pub stripes: bool,
    /// "Page 1 of 3" at the foot of each page, with the title on the left.
    pub page_numbers: bool,
    /// Document title. Empty means the file name.
    pub title: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            delimiter: Delimiter::Auto,
            header: true,
            paper: Paper::A4,
            orientation: Orientation::Auto,
            font_size: 9.0,
            margin: 36.0,
            grid: true,
            stripes: true,
            page_numbers: true,
            title: None,
        }
    }
}

/// Text is never drawn smaller than this to make a table fit.
const MIN_FONT: f32 = 5.0;
const FOOTER_SIZE: f32 = 8.0;
const FONT: doc::StdFont = doc::StdFont::Helvetica;
const BOLD: doc::StdFont = doc::StdFont::HelveticaBold;

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Decode a text file: UTF-8 (with or without a byte order mark), UTF-16 with a byte
/// order mark, or Windows-1252 when the bytes are not valid UTF-8.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let little = bytes[0] == 0xFF;
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        return String::from_utf16_lossy(&units);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
        '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Pick the separator that splits the first lines into the same number of fields,
/// preferring the one that gives the most fields. Quoted text is skipped.
pub fn sniff(text: &str) -> char {
    let candidates = [',', ';', '\t', '|'];
    let mut best = (',', 0usize, false);
    for &c in &candidates {
        let counts: Vec<usize> = logical_lines(text, 20).iter().map(|line| count_outside_quotes(line, c)).collect();
        let Some(&first) = counts.first() else { continue };
        let consistent = first > 0 && counts.iter().all(|&n| n == first);
        let total: usize = counts.iter().sum();
        let better = match (consistent, best.2) {
            (true, false) => true,
            (false, true) => false,
            (true, true) => first > best.1,
            (false, false) => total > best.1,
        };
        if better && (consistent || total > 0) {
            best = (c, if consistent { first } else { total }, consistent);
        }
    }
    best.0
}

/// Up to `limit` records of the text, joining lines that sit inside quotes.
fn logical_lines(text: &str, limit: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '\n' if !quoted => {
                let line = text[start..i].trim_end_matches('\r');
                if !line.is_empty() {
                    out.push(line);
                }
                start = i + 1;
                if out.len() == limit {
                    return out;
                }
            }
            _ => {}
        }
    }
    let rest = text[start..].trim_end_matches('\r');
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn count_outside_quotes(line: &str, sep: char) -> usize {
    let mut quoted = false;
    line.chars()
        .filter(|&c| {
            if c == '"' {
                quoted = !quoted;
            }
            c == sep && !quoted
        })
        .count()
}

/// Split CSV text into rows of fields (RFC 4180: quoted fields may hold the separator,
/// line breaks and doubled quotes). Blank lines are skipped.
pub fn parse(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    // Inside a field that began with a quote; the next lone quote ends the quoted part.
    let mut quoted = false;
    let mut at_field_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_field_start => {
                quoted = true;
                at_field_start = false;
            }
            c if c == sep => {
                row.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                if row.len() == 1 && row[0].is_empty() {
                    row.clear();
                } else {
                    rows.push(std::mem::take(&mut row));
                }
                at_field_start = true;
            }
            c => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// True for cells such as `1,234.50`, `-7`, `(12)`, `30%`, `$4.99`, `1.5e3` and `€ 12`.
fn is_number(cell: &str) -> bool {
    let s = cell.trim();
    let s = s.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(s);
    let s = s.trim_start_matches(['+', '-']);
    let s = s.trim_start_matches(['$', '€', '£', '¥', '₹']).trim();
    let s = s.strip_suffix('%').unwrap_or(s);
    let mut digits = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '0'..='9' => digits += 1,
            ',' | '.' | ' ' | '\u{A0}' | '\'' => {}
            'e' | 'E' if digits > 0 => {
                if matches!(chars.peek(), Some('+') | Some('-')) {
                    chars.next();
                }
            }
            _ => return false,
        }
    }
    digits > 0
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Sizes that follow from the font size.
#[derive(Clone, Copy)]
struct Metrics {
    size: f32,
    pad_x: f32,
    pad_y: f32,
    line: f32,
}

impl Metrics {
    fn new(size: f32) -> Self {
        Metrics { size, pad_x: size * 0.5, pad_y: size * 0.35, line: size * 1.22 }
    }
}

fn width(text: &str, bold: bool, size: f32) -> f32 {
    if bold { BOLD.width(text, size) } else { FONT.width(text, size) }
}

/// Widest line of a cell, and its widest single word.
fn measure(cell: &str, bold: bool, size: f32) -> (f32, f32) {
    let mut line_max = 0f32;
    let mut word_max = 0f32;
    for line in cell.lines() {
        line_max = line_max.max(width(line, bold, size));
        for word in line.split_whitespace() {
            word_max = word_max.max(width(word, bold, size));
        }
    }
    (line_max, word_max)
}

/// Column widths for a table that must fit in `avail`, CSS auto-layout style: every
/// column gets the room its longest word needs, and the rest is shared out in proportion
/// to how much more each column would like.
fn column_widths(rows: &[Vec<String>], header: bool, cols: usize, m: Metrics, avail: f32) -> (Vec<f32>, f32) {
    let mut natural = vec![0f32; cols];
    let mut minimum = vec![0f32; cols];
    for (r, row) in rows.iter().enumerate() {
        let bold = header && r == 0;
        for (c, cell) in row.iter().enumerate() {
            let (line, word) = measure(cell, bold, m.size);
            natural[c] = natural[c].max(line);
            minimum[c] = minimum[c].max(word);
        }
    }
    // A column never needs to be narrower than a few letters, nor wider than its text.
    let floor = width("0000", false, m.size);
    for c in 0..cols {
        natural[c] += 2.0 * m.pad_x;
        minimum[c] = (minimum[c].min(avail / 3.0).max(floor) + 2.0 * m.pad_x).min(natural[c].max(floor + 2.0 * m.pad_x));
        natural[c] = natural[c].max(minimum[c]);
    }
    let want: f32 = natural.iter().sum();
    if want <= avail {
        return (natural, want);
    }
    let least: f32 = minimum.iter().sum();
    if least >= avail {
        let k = avail / least;
        return (minimum.iter().map(|w| w * k).collect(), avail);
    }
    let spare = avail - least;
    let wish = want - least;
    (minimum.iter().zip(&natural).map(|(lo, hi)| lo + (hi - lo) / wish * spare).collect(), avail)
}

/// How small the text must be for the columns' longest words to fit across `avail`.
fn fitting_size(rows: &[Vec<String>], header: bool, cols: usize, size: f32, avail: f32) -> f32 {
    let m = Metrics::new(size);
    let mut minimum = vec![0f32; cols];
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            minimum[c] = minimum[c].max(measure(cell, header && r == 0, size).1.min(avail / 3.0));
        }
    }
    let least: f32 = minimum.iter().map(|w| w + 2.0 * m.pad_x).sum();
    if least <= avail { size } else { (size * avail / least).max(MIN_FONT) }
}

/// The natural width of the whole table at `size`, used to choose the orientation.
fn natural_width(rows: &[Vec<String>], header: bool, cols: usize, size: f32) -> f32 {
    let m = Metrics::new(size);
    let mut natural = vec![0f32; cols];
    for (r, row) in rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            natural[c] = natural[c].max(measure(cell, header && r == 0, size).0);
        }
    }
    natural.iter().map(|w| w + 2.0 * m.pad_x).sum()
}

/// Break a cell into lines no wider than `max`. Words longer than a line are split.
fn wrap(cell: &str, bold: bool, size: f32, max: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in cell.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if width(&candidate, bold, size) <= max {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // The word alone may still be too long: cut it into pieces that fit.
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if width(&piece, bold, size) > max && piece.chars().count() > 1 {
                    piece.pop();
                    out.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            line = piece;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

struct Laid {
    lines: Vec<Vec<String>>,
    height: f32,
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

struct Page {
    content: String,
}

struct Painter<'a> {
    m: Metrics,
    widths: &'a [f32],
    numeric: &'a [bool],
    left: f32,
    table_w: f32,
    opts: &'a Options,
}

impl Painter<'_> {
    /// Draw one row with its top edge at `top`. `shade` fills it first.
    fn row(&self, page: &mut Page, laid: &Laid, top: f32, bold: bool, shade: Option<f32>) {
        let c = &mut page.content;
        if let Some(gray) = shade {
            let _ = writeln!(c, "{} g {} {} {} {} re f", doc::fmt(gray), doc::fmt(self.left), doc::fmt(top - laid.height), doc::fmt(self.table_w), doc::fmt(laid.height));
        }
        let _ = writeln!(c, "0 g BT /{} {} Tf", if bold { "B" } else { "R" }, doc::fmt(self.m.size));
        let mut x = self.left;
        for (col, lines) in laid.lines.iter().enumerate() {
            let w = self.widths.get(col).copied().unwrap_or(0.0);
            for (n, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                // Baseline: one line down from the top padding, less the descender.
                let y = top - self.m.pad_y - self.m.line * n as f32 - self.m.size * 0.93;
                let tx = if self.numeric.get(col).copied().unwrap_or(false) { x + w - self.m.pad_x - width(line, bold, self.m.size) } else { x + self.m.pad_x };
                let _ = writeln!(c, "1 0 0 1 {} {} Tm {} Tj", doc::fmt(tx), doc::fmt(y), doc::pdf_string(line));
            }
            x += w;
        }
        c.push_str("ET\n");
    }

    /// Lines around and between the cells of the rows spanning `top..bottom`, with row
    /// boundaries at `edges`.
    fn grid(&self, page: &mut Page, top: f32, bottom: f32, edges: &[f32]) {
        let c = &mut page.content;
        c.push_str("0.6 G 0.5 w\n");
        let _ = writeln!(c, "{} {} {} {} re S", doc::fmt(self.left), doc::fmt(bottom), doc::fmt(self.table_w), doc::fmt(top - bottom));
        for &y in edges {
            let _ = writeln!(c, "{} {} m {} {} l S", doc::fmt(self.left), doc::fmt(y), doc::fmt(self.left + self.table_w), doc::fmt(y));
        }
        let mut x = self.left;
        for w in self.widths.iter().take(self.widths.len().saturating_sub(1)) {
            x += w;
            let _ = writeln!(c, "{} {} m {} {} l S", doc::fmt(x), doc::fmt(bottom), doc::fmt(x), doc::fmt(top));
        }
    }

    fn footer(&self, page: &mut Page, title: &str, n: usize, total: usize, page_w: f32) {
        let label = format!("Page {n} of {total}");
        let y = (self.opts.margin / 2.0 - FOOTER_SIZE / 2.0).max(4.0);
        let right = page_w - self.opts.margin.max(18.0);
        let left = self.opts.margin.max(18.0);
        let room = (right - left - FONT.width(&label, FOOTER_SIZE) - 18.0).max(0.0);
        let title = fit_line(title, FOOTER_SIZE, room);
        let c = &mut page.content;
        let _ = writeln!(c, "0.35 g BT /R {} Tf", doc::fmt(FOOTER_SIZE));
        if !title.is_empty() {
            let _ = writeln!(c, "1 0 0 1 {} {} Tm {} Tj", doc::fmt(left), doc::fmt(y), doc::pdf_string(&title));
        }
        let _ = writeln!(c, "1 0 0 1 {} {} Tm {} Tj", doc::fmt(right - FONT.width(&label, FOOTER_SIZE)), doc::fmt(y), doc::pdf_string(&label));
        c.push_str("ET\n");
    }
}

/// Shorten text with an ellipsis until it fits in `max`.
fn fit_line(text: &str, size: f32, max: f32) -> String {
    if FONT.width(text, size) <= max {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() && FONT.width(&format!("{s}\u{2026}"), size) > max {
        s.pop();
    }
    if s.is_empty() { String::new() } else { format!("{}\u{2026}", s.trim_end()) }
}

/// True when every character can be drawn with a standard font (Windows-1252).
fn representable(text: &str) -> bool {
    doc::winansi(text).iter().zip(text.chars()).all(|(&b, c)| b != b'?' || c == '?')
}

fn font_object(document: &mut Document, font: doc::StdFont) -> ObjectId {
    document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => font.base_name(),
        "Encoding" => "WinAnsiEncoding",
    })
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let [input] = inputs else {
        return Err(Error::invalid("CSV to PDF works on one CSV file at a time."));
    };
    if !(opts.font_size.is_finite() && (MIN_FONT..=72.0).contains(&opts.font_size)) {
        return Err(Error::invalid(format!("The text size must be between {} and 72 points.", doc::fmt(MIN_FONT))));
    }
    if !(opts.margin.is_finite() && opts.margin >= 0.0) {
        return Err(Error::invalid("The margin cannot be negative."));
    }
    let name = input.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| input.display().to_string());
    if helpers::ext(input) == "pdf" {
        return Err(Error::invalid(format!("{name} is a PDF. Choose a CSV file; Extract Tables turns a PDF into CSV.")));
    }

    ctx.report(0.0, "Reading the file");
    let bytes = doc::read_file(input)?;
    if bytes.iter().take(8000).any(|&b| b == 0) && !bytes.starts_with(&[0xFF, 0xFE]) && !bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(Error::invalid(format!("{name} is not a text file. Choose a CSV file saved as text.")));
    }
    let text = decode(&bytes);
    let sep = match opts.delimiter.byte() {
        Some(c) => c,
        None if helpers::ext(input) == "tsv" => '\t',
        None => sniff(&text),
    };
    let mut rows = parse(&text, sep);
    if rows.is_empty() {
        return Err(Error::invalid(format!("{name} has no rows to put in a PDF.")));
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let mut unsupported = 0usize;
    for row in &mut rows {
        row.resize(cols, String::new());
        for cell in row.iter_mut() {
            if cell.contains('\t') {
                *cell = cell.replace('\t', "    ");
            }
            if !representable(cell) {
                unsupported += 1;
            }
        }
    }
    let header = opts.header && rows.len() > 1;
    let body_start = usize::from(header);
    let numeric: Vec<bool> = (0..cols)
        .map(|c| {
            let mut cells = rows[body_start..].iter().map(|r| r[c].trim()).filter(|s| !s.is_empty()).peekable();
            cells.peek().is_some() && cells.all(is_number)
        })
        .collect();

    // Page size and orientation.
    let (short, long) = opts.paper.points();
    let footer_room = if opts.page_numbers { FOOTER_SIZE + 6.0 } else { 0.0 };
    let avail_for = |page_w: f32| page_w - 2.0 * opts.margin;
    let landscape = match opts.orientation {
        Orientation::Portrait => false,
        Orientation::Landscape => true,
        Orientation::Auto => natural_width(&rows, header, cols, opts.font_size) > avail_for(short),
    };
    let (page_w, page_h) = if landscape { (long, short) } else { (short, long) };
    let avail_w = avail_for(page_w);
    let top = page_h - opts.margin;
    let bottom = opts.margin + footer_room;
    if avail_w < 72.0 || top - bottom < 72.0 {
        return Err(Error::invalid("The margin leaves too little room for the table. Use a smaller margin."));
    }

    ctx.check()?;
    ctx.report(0.1, "Laying out the table");
    let size = fitting_size(&rows, header, cols, opts.font_size, avail_w);
    let m = Metrics::new(size);
    let (widths, table_w) = column_widths(&rows, header, cols, m, avail_w);

    // Wrap every cell. A row taller than a page is cut short.
    let max_lines = (((top - bottom) * 0.5 - 2.0 * m.pad_y) / m.line).floor().max(1.0) as usize;
    let mut cut = 0usize;
    let total_rows = rows.len();
    let mut laid = Vec::with_capacity(total_rows);
    for (r, row) in rows.iter().enumerate() {
        if r % 500 == 0 {
            ctx.check()?;
            ctx.progress_span(0.1, 0.5, r as f32 / total_rows as f32, "Laying out the table");
        }
        let bold = header && r == 0;
        let mut lines: Vec<Vec<String>> = row.iter().zip(&widths).map(|(cell, w)| wrap(cell, bold, size, (w - 2.0 * m.pad_x).max(1.0))).collect();
        let mut tallest = lines.iter().map(Vec::len).max().unwrap_or(1);
        if tallest > max_lines {
            cut += 1;
            for cell in &mut lines {
                if cell.len() > max_lines {
                    cell.truncate(max_lines);
                    if let Some(last) = cell.last_mut() {
                        last.push('\u{2026}');
                    }
                }
            }
            tallest = max_lines;
        }
        laid.push(Laid { lines, height: tallest as f32 * m.line + 2.0 * m.pad_y });
    }

    let left = opts.margin;
    let painter = Painter { m, widths: &widths, numeric: &numeric, left, table_w, opts };
    let title = opts.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| helpers::stem(input));

    // Fill pages.
    let mut pages: Vec<Page> = Vec::new();
    let mut r = body_start;
    let body_rows = total_rows - body_start;
    while r < total_rows || pages.is_empty() {
        ctx.check()?;
        ctx.progress_span(0.5, 0.9, (r - body_start) as f32 / body_rows.max(1) as f32, &format!("Drawing page {}", pages.len() + 1));
        let mut page = Page { content: String::new() };
        let mut y = top;
        let mut edges = Vec::new();
        if header {
            painter.row(&mut page, &laid[0], y, true, Some(0.88));
            y -= laid[0].height;
            edges.push(y);
        }
        let mut placed = 0;
        while r < total_rows {
            let row = &laid[r];
            if y - row.height < bottom && placed > 0 {
                break;
            }
            let shade = (opts.stripes && (r - body_start) % 2 == 1).then_some(0.95);
            painter.row(&mut page, row, y, false, shade);
            y -= row.height;
            edges.push(y);
            r += 1;
            placed += 1;
        }
        edges.pop();
        if opts.grid {
            painter.grid(&mut page, top, y, &edges);
        }
        pages.push(page);
    }

    ctx.check()?;
    ctx.report(0.9, "Writing the PDF");
    let (mut document, root) = doc::new_document();
    let regular = font_object(&mut document, FONT);
    let bold = font_object(&mut document, BOLD);
    let fonts = Object::Dictionary(dictionary! { "R" => regular, "B" => bold });
    let total = pages.len();
    let mut ids = Vec::with_capacity(total);
    for (i, mut page) in pages.into_iter().enumerate() {
        if opts.page_numbers {
            painter.footer(&mut page, &title, i + 1, total, page_w);
        }
        let id = doc::blank_page(&mut document, page_w, page_h);
        let content = document.add_object(Stream::new(Dictionary::new(), page.content.into_bytes()));
        let dict = document.get_dictionary_mut(id)?;
        dict.set("Contents", content);
        dict.set("Resources", dictionary! { "Font" => fonts.clone() });
        ids.push(id);
    }
    doc::append_pages(&mut document, root, &ids)?;
    doc::set_info(&mut document, "Title", &title)?;
    doc::save(&mut document, out)?;
    ctx.report(1.0, "Done");

    let mut outcome = Outcome::single(out.to_path_buf(), total, bytes.len() as u64);
    let body = total_rows - body_start;
    outcome.notes.push(format!("{body} {} and {cols} {}.", if body == 1 { "row" } else { "rows" }, if cols == 1 { "column" } else { "columns" }));
    if size < opts.font_size - 0.05 {
        outcome.notes.push(format!("The table is wide, so the text was made smaller ({} pt) to fit the page.", doc::fmt((size * 10.0).round() / 10.0)));
    }
    if cut > 0 {
        outcome.notes.push(format!("{cut} {} too long for a page and {} cut short.", if cut == 1 { "row was" } else { "rows were" }, if cut == 1 { "was" } else { "were" }));
    }
    if unsupported > 0 {
        outcome.notes.push(format!(
            "{unsupported} {} characters outside Western European text, shown as \"?\".",
            if unsupported == 1 { "cell has" } else { "cells have" }
        ));
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quotes_and_line_breaks() {
        let rows = parse("a,\"b, c\",\"say \"\"hi\"\"\"\r\n1,\"two\nlines\",3\n\n4,5\n", ',');
        assert_eq!(rows, vec![vec!["a", "b, c", "say \"hi\""], vec!["1", "two\nlines", "3"], vec!["4", "5"]]);
        assert_eq!(parse("x;y", ';'), vec![vec!["x", "y"]]);
        assert_eq!(parse("a,b,\n", ','), vec![vec!["a", "b", ""]]);
    }

    #[test]
    fn sniffs_the_separator() {
        assert_eq!(sniff("a,b,c\n1,2,3\n"), ',');
        assert_eq!(sniff("name;price\n\"x, y\";1,50\nz;2,00\n"), ';');
        assert_eq!(sniff("a\tb\n1\t2\n"), '\t');
        assert_eq!(sniff("a|b|c\n1|2|3\n"), '|');
        assert_eq!(sniff("just one column\nanother\n"), ',');
    }

    #[test]
    fn decodes_common_encodings() {
        assert_eq!(decode(b"\xEF\xBB\xBFcaf\xC3\xA9"), "café");
        assert_eq!(decode(b"caf\xE9 \x80"), "café €");
        assert_eq!(decode(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
    }

    #[test]
    fn recognises_numbers() {
        for s in ["12", "-7", "1,234.50", "(12)", "30%", "$4.99", "1.5e3", "€ 12", "1 000"] {
            assert!(is_number(s), "{s}");
        }
        for s in ["", "abc", "12a", "N/A", "-", "2024-01-05"] {
            assert!(!is_number(s), "{s}");
        }
    }

    #[test]
    fn wraps_and_breaks_long_words() {
        let lines = wrap("alpha beta gamma", false, 10.0, FONT.width("alpha beta", 10.0) + 0.1);
        assert_eq!(lines, ["alpha beta", "gamma"]);
        let lines = wrap("abcdefghij", false, 10.0, FONT.width("abcd", 10.0) + 0.1);
        assert_eq!(lines.concat(), "abcdefghij");
        assert!(lines.iter().all(|l| FONT.width(l, 10.0) <= FONT.width("abcd", 10.0) + 0.1));
    }

    #[test]
    fn columns_fill_but_never_exceed_the_page() {
        let rows = vec![vec!["id".to_string(), "x ".repeat(400)], vec!["1".into(), "short".into()]];
        let m = Metrics::new(9.0);
        let (widths, total) = column_widths(&rows, true, 2, m, 500.0);
        assert!((widths.iter().sum::<f32>() - 500.0).abs() < 0.01);
        assert_eq!(total, 500.0);
        assert!(widths[0] < widths[1]);
    }
}
