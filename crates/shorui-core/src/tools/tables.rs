//! Extract Tables: Pull tables out of a PDF as CSV.
//!
//! There is no table structure in a PDF, only words at positions. A table is found where
//! consecutive rows of words leave the same vertical strips of white space between them:
//! those strips are the column boundaries. Running text does not qualify, because the
//! gaps between its words are one space wide and do not line up from row to row.

use crate::text::{PageText, Rect4, TextReader, Word};
use crate::{Ctx, Error, Outcome, Result, doc, helpers, range};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Delimiter {
    #[default]
    Comma,
    Tab,
    Semicolon,
}

impl Delimiter {
    pub fn char(self) -> char {
        match self {
            Delimiter::Comma => ',',
            Delimiter::Tab => '\t',
            Delimiter::Semicolon => ';',
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Page range such as `1-3, 7`. Empty means every page.
    pub pages: String,
    pub delimiter: Delimiter,
    /// Smallest number of rows a table must have.
    pub min_rows: usize,
    /// Smallest number of columns a table must have.
    pub min_cols: usize,
    /// Write every table into one CSV, with a blank line between tables.
    pub single_file: bool,
    /// Start each file with a UTF-8 byte order mark so that Excel reads accented
    /// characters correctly.
    pub bom: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { pages: String::new(), delimiter: Delimiter::Comma, min_rows: 2, min_cols: 2, single_file: false, bom: true }
    }
}

/// A table found on a page.
#[derive(Debug, Clone, Serialize)]
pub struct Table {
    /// 1-based page number.
    pub page: usize,
    /// Where the table is, in display space (points, origin top-left, y down).
    pub rect: Rect4,
    /// Cell text, row by row. Every row has the same number of cells; empty cells are "".
    pub rows: Vec<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// A column gap must be at least this many ems wide across every row of the table...
const MIN_GAP_EM: f32 = 0.45;
/// ...and this wide in a fair share of the rows, so that an ordinary space never counts.
const WIDE_GAP_EM: f32 = 0.9;
/// Rows further apart than this many ems of white space belong to different tables.
const MAX_ROW_GAP_EM: f32 = 3.0;

struct Row<'a> {
    /// Sorted left to right.
    words: Vec<&'a Word>,
    y0: f32,
    y1: f32,
    /// Font size, estimated from the word boxes.
    size: f32,
}

impl Row<'_> {
    fn centre(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }
}

/// Group the words of a page into visual rows, top to bottom.
fn rows_of(page: &PageText) -> Vec<Row<'_>> {
    let mut words: Vec<&Word> = page.lines.iter().filter(|l| l.dir == 0 && l.angle == 0.0).flat_map(|l| l.words.iter()).filter(|w| w.rect.width() > 0.0 && w.rect.height() > 0.0).collect();
    words.sort_by(|a, b| {
        let (ay, by) = ((a.rect.y0 + a.rect.y1) / 2.0, (b.rect.y0 + b.rect.y1) / 2.0);
        ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut rows: Vec<Row> = Vec::new();
    for word in words {
        let centre = (word.rect.y0 + word.rect.y1) / 2.0;
        let height = word.rect.height();
        match rows.last_mut() {
            Some(row) if (centre - row.centre()).abs() <= 0.45 * height.max(row.y1 - row.y0) => {
                row.y0 = row.y0.min(word.rect.y0);
                row.y1 = row.y1.max(word.rect.y1);
                row.size = row.size.max(height / 1.07);
                row.words.push(word);
            }
            _ => rows.push(Row { words: vec![word], y0: word.rect.y0, y1: word.rect.y1, size: height / 1.07 }),
        }
    }
    for row in &mut rows {
        row.words.sort_by(|a, b| a.rect.x0.partial_cmp(&b.rect.x0).unwrap_or(std::cmp::Ordering::Equal));
    }
    rows
}

/// The width of an ordinary space on this page, in ems. Measured from the gaps between
/// neighbouring words; falls back to a typical value when the page has too few.
fn space_em(rows: &[Row]) -> f32 {
    let mut gaps: Vec<f32> = Vec::new();
    for row in rows {
        for pair in row.words.windows(2) {
            let gap = (pair[1].rect.x0 - pair[0].rect.x1) / row.size.max(1.0);
            if gap > 0.05 && gap < 0.8 {
                gaps.push(gap);
            }
        }
    }
    if gaps.len() < 8 {
        return 0.3;
    }
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    gaps[gaps.len() / 2]
}

/// The column separators shared by a run of rows: strips of white space, as `(x0, x1)`,
/// that no word in any of the rows touches.
fn separators(rows: &[Row], min_gap_em: f32, wide_gap_em: f32) -> Vec<(f32, f32)> {
    let size = rows.iter().map(|r| r.size).fold(0.0f32, f32::max).max(1.0);
    let lo = rows.iter().filter_map(|r| r.words.first()).map(|w| w.rect.x0).fold(f32::MAX, f32::min);
    let hi = rows.iter().filter_map(|r| r.words.last()).map(|w| w.rect.x1).fold(f32::MIN, f32::max);
    if !(lo < hi) {
        return Vec::new();
    }
    // Subtract every word from the free strip.
    let mut covered: Vec<(f32, f32)> = rows.iter().flat_map(|r| r.words.iter()).map(|w| (w.rect.x0, w.rect.x1)).collect();
    covered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut free = Vec::new();
    let mut edge = lo;
    for (x0, x1) in covered {
        if x0 - edge >= min_gap_em * size {
            free.push((edge, x0));
        }
        edge = edge.max(x1);
    }
    // Keep the strips that are clearly wider than a space in enough of the rows.
    free.retain(|&(x0, x1)| {
        let (mut both, mut wide) = (0usize, 0usize);
        for row in rows {
            let left = row.words.iter().filter(|w| w.rect.x1 <= x0 + 0.01).map(|w| w.rect.x1).fold(f32::MIN, f32::max);
            let right = row.words.iter().filter(|w| w.rect.x0 >= x1 - 0.01).map(|w| w.rect.x0).fold(f32::MAX, f32::min);
            if left > f32::MIN && right < f32::MAX {
                both += 1;
                if right - left >= wide_gap_em * row.size.max(1.0) {
                    wide += 1;
                }
            }
        }
        both >= 1 && wide * 10 >= both * 4
    });
    free
}

/// Which column a word falls in.
fn column_of(word: &Word, seps: &[(f32, f32)]) -> usize {
    let centre = (word.rect.x0 + word.rect.x1) / 2.0;
    seps.iter().filter(|s| centre > (s.0 + s.1) / 2.0).count()
}

fn cells_of(row: &Row, seps: &[(f32, f32)]) -> Vec<String> {
    let mut cells = vec![String::new(); seps.len() + 1];
    for word in &row.words {
        if let Some(cell) = cells.get_mut(column_of(word, seps)) {
            if !cell.is_empty() {
                cell.push(' ');
            }
            cell.push_str(word.text.trim());
        }
    }
    cells
}

fn filled(row: &Row, seps: &[(f32, f32)]) -> usize {
    cells_of(row, seps).iter().filter(|c| !c.is_empty()).count()
}

/// Find the tables on one page.
pub fn tables_on_page(page: &PageText, page_number: usize, opts: &Options) -> Vec<Table> {
    let min_rows = opts.min_rows.max(2);
    let min_cols = opts.min_cols.max(2);
    let rows = rows_of(page);
    let space = space_em(&rows);
    let min_gap = MIN_GAP_EM.max(1.6 * space);
    let wide_gap = WIDE_GAP_EM.max(1.9 * space);
    let seps = |a: usize, b: usize| separators(&rows[a..=b], min_gap, wide_gap);
    let close = |a: usize, b: usize| rows[b].y0 - rows[a].y1 <= MAX_ROW_GAP_EM * rows[a].size.max(rows[b].size);

    let mut tables = Vec::new();
    let mut floor = 0usize; // first row not yet used by a table
    let mut i = 0usize;
    while i < rows.len() {
        let mut current = seps(i, i);
        if current.len() + 1 < min_cols {
            i += 1;
            continue;
        }
        // Grow downwards while the column structure holds.
        let mut end = i; // last row accepted
        let mut last_full = i; // last row with text in at least two columns
        while end + 1 < rows.len() && close(end, end + 1) {
            let next = seps(i, end + 1);
            if next.len() + 1 < min_cols || next.len() < current.len() {
                break;
            }
            end += 1;
            current = next;
            if filled(&rows[end], &current) >= 2 {
                last_full = end;
            }
        }
        let end = last_full;
        let mut start = i;
        let mut current = seps(start, end);
        // A header whose labels sit close together is not a seed; pick it up from below.
        while start > floor && close(start - 1, start) {
            let with = seps(start - 1, end);
            if with.len() < current.len() || filled(&rows[start - 1], &with) < 2 {
                break;
            }
            start -= 1;
            current = with;
        }

        let block = &rows[start..=end];
        let cells: Vec<Vec<String>> = block.iter().map(|r| cells_of(r, &current)).collect();
        let full_rows = cells.iter().filter(|r| r.iter().filter(|c| !c.is_empty()).count() >= 2).count();
        // Two newspaper-style text columns look like a two-column table. Tell them apart
        // by how much text the cells hold: prose has many words in every column.
        let prose = (0..=current.len()).all(|col| {
            let mut counts: Vec<usize> = cells.iter().filter_map(|r| r.get(col)).filter(|c| !c.is_empty()).map(|c| c.split(' ').count()).collect();
            counts.sort_unstable();
            counts.get(counts.len() / 2).copied().unwrap_or(0) >= 5
        });
        if block.len() >= min_rows && current.len() + 1 >= min_cols && full_rows >= min_rows && !prose {
            let mut rect: Option<Rect4> = None;
            for word in block.iter().flat_map(|r| r.words.iter()) {
                rect = Some(match rect {
                    Some(r) => r.union(&word.rect),
                    None => word.rect,
                });
            }
            if let Some(rect) = rect {
                tables.push(Table { page: page_number, rect, rows: cells });
            }
            floor = end + 1;
            i = end + 1;
        } else {
            i += 1;
        }
    }
    tables
}

struct Found {
    tables: Vec<Table>,
    /// Pages looked at, and how many of them had no text at all.
    pages: usize,
    pages_without_text: usize,
}

fn detect_with(path: &Path, password: Option<&str>, opts: &Options, ctx: &Ctx) -> Result<Found> {
    let reader = TextReader::open_path(path, password)?;
    let mut pages = range::parse(&opts.pages, reader.page_count())?;
    let mut seen = std::collections::HashSet::new();
    pages.retain(|p| seen.insert(*p));
    let mut found = Found { tables: Vec::new(), pages: pages.len(), pages_without_text: 0 };
    for (i, &page) in pages.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.9 * i as f32 / pages.len().max(1) as f32, &format!("Looking for tables on page {page}"));
        let text = reader.page(page - 1)?;
        if text.is_empty() {
            found.pages_without_text += 1;
            continue;
        }
        found.tables.extend(tables_on_page(&text, page, opts));
    }
    Ok(found)
}

/// The tables in a PDF, for a preview. Returns an empty list when there are none.
/// `opts.pages`, `min_rows` and `min_cols` apply; the other options only affect `run`.
pub fn detect(path: &Path, password: Option<&str>, opts: &Options) -> Result<Vec<Table>> {
    Ok(detect_with(path, password, opts, &Ctx::none())?.tables)
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

fn csv_field(field: &str, delimiter: char) -> String {
    let needs_quotes = field.contains(delimiter) || field.contains('"') || field.contains('\n') || field.contains('\r') || field.starts_with(' ') || field.ends_with(' ');
    if needs_quotes { format!("\"{}\"", field.replace('"', "\"\"")) } else { field.to_string() }
}

/// One table as CSV text. Lines end with CRLF, as RFC 4180 asks.
pub fn to_csv(rows: &[Vec<String>], delimiter: Delimiter) -> String {
    let d = delimiter.char();
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row.iter().map(|f| csv_field(f, d)).collect();
        out.push_str(&line.join(&d.to_string()));
        out.push_str("\r\n");
    }
    out
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = match inputs {
        [one] => one,
        [] => return Err(Error::invalid("Choose a PDF to extract tables from.")),
        _ => return Err(Error::invalid("Extract Tables works on one PDF at a time.")),
    };
    let bytes_in = crate::ctx::file_size(input);
    let found = detect_with(input, ctx.password(), opts, ctx)?;
    if found.tables.is_empty() {
        return Err(Error::invalid(if found.pages_without_text == found.pages {
            "No tables were found: these pages have no text layer, so they are probably scans. Run OCR on the file first, then try again."
        } else if found.pages_without_text > 0 {
            "No tables were found on the selected pages. Some of them have no text layer; run OCR first if the tables are on scanned pages."
        } else {
            "No tables were found on the selected pages."
        }));
    }

    helpers::ensure_dir(out)?;
    let name = helpers::stem(input);
    let prefix = if opts.bom { "\u{FEFF}" } else { "" };
    let mut outcome = Outcome { bytes_in, ..Default::default() };
    let mut pages_with_tables: Vec<usize> = found.tables.iter().map(|t| t.page).collect();
    pages_with_tables.dedup();
    outcome.pages = pages_with_tables.len();
    ctx.report(0.92, "Writing CSV files");

    if opts.single_file {
        let body: Vec<String> = found.tables.iter().map(|t| to_csv(&t.rows, opts.delimiter)).collect();
        let path = out.join(format!("{name}-tables.csv"));
        doc::write_file(&path, format!("{prefix}{}", body.join("\r\n")).as_bytes())?;
        outcome.push(path);
    } else {
        let mut page = 0usize;
        let mut k = 0usize;
        for table in &found.tables {
            ctx.check()?;
            if table.page != page {
                page = table.page;
                k = 0;
            }
            k += 1;
            let path = out.join(format!("{name}-p{page}-t{k}.csv"));
            doc::write_file(&path, format!("{prefix}{}", to_csv(&table.rows, opts.delimiter)).as_bytes())?;
            outcome.push(path);
        }
    }
    let n = found.tables.len();
    outcome.notes.push(format!(
        "Found {n} {} on {} {}.",
        if n == 1 { "table" } else { "tables" },
        outcome.pages,
        if outcome.pages == 1 { "page" } else { "pages" }
    ));
    if found.pages_without_text > 0 {
        outcome.notes.push(format!(
            "{} {} no text layer and could not be searched for tables. Run OCR first to include them.",
            found.pages_without_text,
            if found.pages_without_text == 1 { "page has" } else { "pages have" }
        ));
    }
    ctx.report(1.0, "Done");
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_fields() {
        let rows = vec![vec!["a,b".to_string(), "say \"hi\"".to_string(), "plain".to_string()], vec!["".to_string(), "x".to_string(), " pad".to_string()]];
        assert_eq!(to_csv(&rows, Delimiter::Comma), "\"a,b\",\"say \"\"hi\"\"\",plain\r\n,x,\" pad\"\r\n");
        assert_eq!(to_csv(&rows[..1], Delimiter::Semicolon), "a,b;\"say \"\"hi\"\"\";plain\r\n");
    }
}
