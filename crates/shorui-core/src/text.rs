//! Text with positions. Used by search-to-redact, table extraction, compare and OCR checks.
//!
//! All coordinates here are in "display space": points, origin at the top-left corner of
//! the page as shown, y running down. That matches a rendered page at scale 1.0.

use crate::{Error, Result};
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::hayro_syntax::{LoadPdfError, Pdf};
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page};
use hayro::vello_cpu::kurbo::{Affine, BezPath, Point, Rect};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, serde::Deserialize)]
pub struct Rect4 {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect4 {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect4 { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn union(&self, o: &Rect4) -> Rect4 {
        Rect4 { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn intersects(&self, o: &Rect4) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
    pub fn grow(&self, by: f32) -> Rect4 {
        Rect4 { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }
    /// The same rectangle in visible space (origin bottom-left, y up) for a page `page_height` points tall.
    pub fn to_visible(&self, page_height: f32) -> [f32; 4] {
        [self.x0, page_height - self.y1, self.x1, page_height - self.y0]
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Ch {
    pub text: String,
    pub x0: f32,
    pub x1: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Word {
    pub text: String,
    pub rect: Rect4,
    #[serde(skip)]
    pub chars: Vec<Ch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub words: Vec<Word>,
    pub rect: Rect4,
    /// Font size of the line in points.
    pub size: f32,
    /// Direction the text runs in on the displayed page: 0 right, 1 down, 2 left, 3 up.
    pub dir: u8,
    /// The exact angle of the text in degrees, clockwise on the displayed page. 0 for
    /// ordinary text; a diagonal watermark reads about -45.
    pub angle: f32,
    /// The line box in its own reading frame (text running right). `Ch` extents use this frame.
    #[serde(skip)]
    frame: Rect4,
}

/// Rotate a display-space point into the reading frame of text running at `angle` degrees.
fn to_frame(angle: f32, x: f32, y: f32) -> (f32, f32) {
    let (sin, cos) = exact_sin_cos(angle);
    (x * cos + y * sin, -x * sin + y * cos)
}

/// Map a rectangle from a reading frame back to display space. For text at an angle the
/// result is the box that encloses the rotated rectangle.
fn from_frame(angle: f32, r: Rect4) -> Rect4 {
    let (sin, cos) = exact_sin_cos(angle);
    let p = |x: f32, y: f32| (x * cos - y * sin, x * sin + y * cos);
    let corners = [p(r.x0, r.y0), p(r.x1, r.y0), p(r.x0, r.y1), p(r.x1, r.y1)];
    let xs = corners.iter().map(|c| c.0);
    let ys = corners.iter().map(|c| c.1);
    Rect4 { x0: xs.clone().fold(f32::INFINITY, f32::min), y0: ys.clone().fold(f32::INFINITY, f32::min), x1: xs.fold(f32::NEG_INFINITY, f32::max), y1: ys.fold(f32::NEG_INFINITY, f32::max) }
}

/// Sine and cosine with the quarter turns exact, so upright text keeps exact coordinates.
fn exact_sin_cos(angle: f32) -> (f32, f32) {
    match angle.rem_euclid(360.0) {
        a if a == 0.0 => (0.0, 1.0),
        a if a == 90.0 => (1.0, 0.0),
        a if a == 180.0 => (0.0, -1.0),
        a if a == 270.0 => (-1.0, 0.0),
        a => a.to_radians().sin_cos(),
    }
}

/// Snap an angle to a whole degree, and to a quarter turn when it is within one degree of it.
fn snap_angle(degrees: f32) -> f32 {
    let quarter = (degrees / 90.0).round() * 90.0;
    let snapped = if (degrees - quarter).abs() <= 1.0 { quarter } else { degrees.round() };
    snapped.rem_euclid(360.0)
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PageText {
    pub width: f32,
    pub height: f32,
    pub lines: Vec<Line>,
}

impl PageText {
    pub fn plain(&self) -> String {
        self.lines.iter().map(|l| l.text()).collect::<Vec<_>>().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.words.is_empty())
    }

    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.lines.iter().flat_map(|l| l.words.iter())
    }

    /// Rectangles covering every occurrence of `needle`. A match never spans lines.
    pub fn find(&self, needle: &str, match_case: bool, whole_words: bool) -> Vec<Rect4> {
        let needle: Vec<char> = if match_case { needle.chars().collect() } else { needle.to_lowercase().chars().collect() };
        let mut hits = Vec::new();
        if needle.is_empty() {
            return hits;
        }
        for line in &self.lines {
            // One entry per character of the line text, with the x extent it covers.
            let mut cells: Vec<(char, f32, f32)> = Vec::new();
            for (i, word) in line.words.iter().enumerate() {
                if i > 0 {
                    let prev_end = line.words[i - 1].chars.last().map(|c| c.x1).unwrap_or(0.0);
                    let next_start = word.chars.first().map(|c| c.x0).unwrap_or(prev_end);
                    cells.push((' ', prev_end, next_start));
                }
                for ch in &word.chars {
                    for c in ch.text.chars() {
                        let c = if match_case { c } else { c.to_lowercase().next().unwrap_or(c) };
                        cells.push((c, ch.x0, ch.x1));
                    }
                }
            }
            let mut i = 0;
            while i + needle.len() <= cells.len() {
                let matched = (0..needle.len()).all(|k| cells[i + k].0 == needle[k]);
                if matched {
                    let before_ok = i == 0 || !cells[i - 1].0.is_alphanumeric();
                    let after = i + needle.len();
                    let after_ok = after >= cells.len() || !cells[after].0.is_alphanumeric();
                    if !whole_words || (before_ok && after_ok) {
                        let x0 = cells[i].1;
                        let x1 = cells[after - 1].2;
                        hits.push(from_frame(line.angle, Rect4::new(x0, line.frame.y0, x1, line.frame.y1)));
                        i = after;
                        continue;
                    }
                }
                i += 1;
            }
        }
        hits
    }
}

/// A glyph in the reading frame of its direction: `x` is where it starts, `y` its baseline.
struct RawGlyph {
    text: String,
    x: f32,
    y: f32,
    w: f32,
    size: f32,
    /// Degrees, clockwise on the displayed page.
    angle: f32,
}

#[derive(Default)]
struct Collector {
    glyphs: Vec<RawGlyph>,
}

impl<'a> Device<'a> for Collector {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let text = match glyph.as_unicode() {
            Some(BfString::Char(c)) => c.to_string(),
            Some(BfString::String(s)) => s,
            None => return,
        };
        let t = transform * glyph_transform;
        let origin = t * Point::new(0.0, 0.0);
        let advance = match glyph {
            Glyph::Outline(g) => g.advance_width().unwrap_or(500.0),
            Glyph::Type3(_) => 500.0,
        } as f64;
        let end = t * Point::new(advance, 0.0);
        let up = t * Point::new(0.0, 1000.0);
        let size = origin.distance(up) as f32;
        let (dx, dy) = ((end.x - origin.x) as f32, (end.y - origin.y) as f32);
        let w = dx.hypot(dy);
        if !(size.is_finite() && w.is_finite()) || size <= 0.0 {
            return;
        }
        let angle = if w > 0.0 { snap_angle(dy.atan2(dx).to_degrees()) } else { 0.0 };
        let (x, y) = to_frame(angle, origin.x as f32, origin.y as f32);
        self.glyphs.push(RawGlyph { text, x, y, w, size, angle });
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

fn group(glyphs: Vec<RawGlyph>, width: f32, height: f32) -> PageText {
    struct Building {
        words: Vec<Word>,
        cur: Option<Word>,
        baseline: f32,
        size: f32,
        last_x1: f32,
        angle: f32,
    }
    fn close_word(b: &mut Building) {
        if let Some(w) = b.cur.take() {
            if !w.text.trim().is_empty() {
                b.words.push(w);
            }
        }
    }
    fn close_line(b: Building, lines: &mut Vec<Line>) {
        let mut b = b;
        close_word(&mut b);
        if b.words.is_empty() {
            return;
        }
        let frame = b.words.iter().skip(1).fold(b.words[0].rect, |r, w| r.union(&w.rect));
        let angle = b.angle;
        for w in &mut b.words {
            w.rect = from_frame(angle, w.rect);
        }
        let dir = (((angle / 90.0).round() as i32).rem_euclid(4)) as u8;
        lines.push(Line { words: b.words, rect: from_frame(angle, frame), size: b.size, dir, angle, frame });
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut cur: Option<Building> = None;
    for g in glyphs {
        let same_line = cur.as_ref().is_some_and(|b| b.angle == g.angle && (g.y - b.baseline).abs() < 0.5 * b.size.max(g.size) && g.x > b.last_x1 - 2.0 * b.size);
        if !same_line {
            if let Some(b) = cur.take() {
                close_line(b, &mut lines);
            }
            cur = Some(Building { words: Vec::new(), cur: None, baseline: g.y, size: g.size, last_x1: g.x, angle: g.angle });
        }
        let b = cur.as_mut().unwrap();
        let is_space = g.text.chars().all(char::is_whitespace);
        let gap = g.x - b.last_x1;
        if is_space || (b.cur.is_some() && gap > 0.25 * g.size) {
            close_word(b);
        }
        if !is_space {
            let rect = Rect4::new(g.x, g.y - 0.85 * g.size, g.x + g.w, g.y + 0.22 * g.size);
            let ch = Ch { text: g.text.clone(), x0: g.x, x1: g.x + g.w };
            match b.cur.as_mut() {
                Some(w) => {
                    w.text.push_str(&g.text);
                    w.rect = w.rect.union(&rect);
                    w.chars.push(ch);
                }
                None => b.cur = Some(Word { text: g.text.clone(), rect, chars: vec![ch] }),
            }
        }
        b.last_x1 = g.x + g.w;
        b.size = b.size.max(g.size);
    }
    if let Some(b) = cur.take() {
        close_line(b, &mut lines);
    }

    // Reading order: top to bottom, then left to right. Pieces of one visual row that
    // were emitted separately (table cells, columns) end up next to each other.
    lines.sort_by(|a, b| {
        let ay = (a.rect.y0 + a.rect.y1) / 2.0;
        let by = (b.rect.y0 + b.rect.y1) / 2.0;
        if (ay - by).abs() < 0.4 * a.size.max(b.size) {
            a.rect.x0.partial_cmp(&b.rect.x0).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    PageText { width, height, lines }
}

/// An opened document ready to read text from.
pub struct TextReader {
    pdf: Pdf,
}

impl TextReader {
    pub fn open(bytes: Vec<u8>, password: Option<&str>) -> Result<Self> {
        let pdf = Pdf::new_with_password(bytes, password.unwrap_or("")).map_err(|e| match e {
            LoadPdfError::Decryption(_) => {
                if password.is_some() { Error::WrongPassword } else { Error::PasswordRequired }
            }
            LoadPdfError::Invalid => Error::damaged("This file is damaged and its text could not be read. Repair can usually rebuild it."),
        })?;
        Ok(Self { pdf })
    }

    pub fn open_path(path: &std::path::Path, password: Option<&str>) -> Result<Self> {
        Self::open(crate::doc::read_file(path)?, password)
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Text of one page. `index` is 0-based. Includes invisible text such as an OCR layer.
    pub fn page(&self, index: usize) -> Result<PageText> {
        let page = self.pdf.pages().get(index).ok_or_else(|| Error::invalid(format!("Page {} does not exist.", index + 1)))?;
        let (w, h) = page.render_dimensions();
        let cache = InterpreterCache::new();
        let settings = InterpreterSettings { render_annotations: false, ..Default::default() };
        let mut context = Context::new(page.initial_transform(true).to_kurbo(), Rect::new(0.0, 0.0, w as f64, h as f64), &cache, page.xref(), settings);
        let mut collector = Collector::default();
        interpret_page(page, &mut context, &mut collector);
        Ok(group(collector.glyphs, w, h))
    }

    pub fn all(&self) -> Result<Vec<PageText>> {
        (0..self.page_count()).map(|i| self.page(i)).collect()
    }
}
