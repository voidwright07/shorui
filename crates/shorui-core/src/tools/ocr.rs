//! OCR: Add a searchable text layer to scanned pages.
//!
//! Each selected page is rendered to a picture, the picture is read by an OCR engine, and
//! the words it finds are written back over the page as invisible text (text rendering
//! mode 3), so the page looks the same but can be searched, selected and copied.
//!
//! Engines, behind the [`OcrEngine`] trait:
//!
//! - **Windows**: the engine built into the system (`Windows.Media.Ocr`). It works offline
//!   and reads the languages installed under Settings > Time & language.
//! - **Any platform**: the `tesseract` command line, when it is installed.
//! - **macOS** has an engine of its own (Vision) that is not used here; macOS and Linux
//!   rely on Tesseract.
//!
//! `engine: "auto"` picks the system engine on Windows (falling back to Tesseract when it
//! has no usable language) and Tesseract elsewhere.
//!
//! The text layer is set in Helvetica with the standard Windows-1252 encoding, so it can
//! hold Western European text. Characters outside that set are stored as `?` and a note
//! says how many words were affected.
//!
//! The system engine waits for Windows to finish each page, so call `run` from a worker
//! thread, not the UI thread.

use crate::doc::{StdFont, fmt, pdf_string};
use crate::helpers::{self, TempDir};
use crate::render::{Renderer, dpi_to_scale};
use crate::text::TextReader;
use crate::{Ctx, Error, Outcome, Result, doc, range};
use image::RgbaImage;
use lopdf::{Object, dictionary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// The system engine on Windows, Tesseract elsewhere.
    #[default]
    Auto,
    /// The engine built into the operating system. Windows only.
    System,
    /// The `tesseract` program, which must be installed.
    Tesseract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Language of the text as a BCP-47 tag such as `en-US` (a Tesseract code such as
    /// `deu` is accepted too). Empty means the system's first available OCR language.
    pub language: String,
    /// Resolution the pages are rendered at before they are read.
    pub dpi: u32,
    /// Pages to read, such as `1-3, 7`. Empty means all.
    pub pages: String,
    /// Leave pages that already have a text layer exactly as they are.
    pub skip_text_pages: bool,
    /// `auto`, `system` or `tesseract`.
    pub engine: Engine,
}

impl Default for Options {
    fn default() -> Self {
        Options { language: String::new(), dpi: 200, pages: String::new(), skip_text_pages: true, engine: Engine::Auto }
    }
}

/// One recognised word. The box is in pixels of the picture given to the engine, origin
/// at the top-left corner, y running down.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Words with the same number were read as one line of text.
    pub line: u32,
}

/// Something that can read the words in a picture of a page.
pub trait OcrEngine {
    fn recognise(&self, image: &RgbaImage) -> Result<Vec<OcrWord>>;
    /// A name to show, such as "Windows OCR (en-US)".
    fn describe(&self) -> String;
}

const TESSERACT_HINT: &str = "Install Tesseract OCR and try again: on Windows use the installer from the Tesseract project (UB Mannheim build), on macOS run \"brew install tesseract\", on Linux install the \"tesseract-ocr\" package.";

fn missing_engine() -> Error {
    Error::MissingHelper { tool: "An OCR engine (Tesseract)".into(), hint: TESSERACT_HINT.into() }
}

// ---------------------------------------------------------------------------
// The Windows engine
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod system {
    use super::{OcrEngine, OcrWord};
    use crate::{Error, Result};
    use image::RgbaImage;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine as WinOcr;
    use windows::Storage::Streams::DataWriter;
    use windows::core::HSTRING;

    fn failed(e: windows::core::Error) -> Error {
        Error::External(format!("Windows OCR failed: {}", e.message()))
    }

    /// Language tags the installed recognisers understand, such as `en-US`.
    pub fn languages() -> Vec<String> {
        let Ok(list) = WinOcr::AvailableRecognizerLanguages() else {
            return Vec::new();
        };
        list.into_iter().filter_map(|l| l.LanguageTag().ok()).map(|t| t.to_string()).collect()
    }

    pub struct SystemEngine {
        engine: WinOcr,
        tag: String,
        max_dimension: u32,
    }

    /// Open the recogniser for `language`, or the user's own language when it is empty.
    /// A tag that is not installed exactly falls back to another variant of the same
    /// language (`en-AU` to `en-US`).
    pub fn open(language: &str) -> Result<SystemEngine> {
        let installed = languages();
        if installed.is_empty() {
            return Err(Error::Unsupported("Windows has no OCR language installed. Add one under Settings > Time & language > Language & region, or install Tesseract.".into()));
        }
        let wanted = language.trim();
        let engine = if wanted.is_empty() {
            match WinOcr::TryCreateFromUserProfileLanguages() {
                Ok(engine) => Some(engine),
                Err(_) => Language::CreateLanguage(&HSTRING::from(installed[0].as_str())).ok().and_then(|l| WinOcr::TryCreateFromLanguage(&l).ok()),
            }
        } else {
            let primary = wanted.split(['-', '_']).next().unwrap_or(wanted).to_ascii_lowercase();
            let exact = installed.iter().find(|t| t.eq_ignore_ascii_case(wanted));
            let related = installed.iter().find(|t| t.split('-').next().is_some_and(|p| p.eq_ignore_ascii_case(&primary)));
            match exact.or(related) {
                Some(tag) => Language::CreateLanguage(&HSTRING::from(tag.as_str())).ok().and_then(|l| WinOcr::TryCreateFromLanguage(&l).ok()),
                None => None,
            }
        };
        let Some(engine) = engine else {
            return Err(Error::invalid(format!(
                "Windows has no OCR language for \"{wanted}\". Installed: {}. Add the language under Settings > Time & language, or choose one of these.",
                installed.join(", ")
            )));
        };
        let tag = engine.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_default();
        let max_dimension = WinOcr::MaxImageDimension().unwrap_or(10_000).max(64);
        Ok(SystemEngine { engine, tag, max_dimension })
    }

    impl OcrEngine for SystemEngine {
        fn recognise(&self, image: &RgbaImage) -> Result<Vec<OcrWord>> {
            // The engine refuses pictures larger than its limit: shrink, then scale the boxes back.
            let (w, h) = image.dimensions();
            let longest = w.max(h);
            let shrunk;
            let (picture, factor) = if longest > self.max_dimension {
                let f = self.max_dimension as f32 / longest as f32;
                let (nw, nh) = (((w as f32 * f) as u32).max(1), ((h as f32 * f) as u32).max(1));
                shrunk = image::imageops::resize(image, nw, nh, image::imageops::FilterType::Triangle);
                (&shrunk, w as f32 / nw as f32)
            } else {
                (image, 1.0)
            };
            let (pw, ph) = picture.dimensions();
            let mut bgra = Vec::with_capacity(picture.as_raw().len());
            for p in picture.pixels() {
                bgra.extend_from_slice(&[p.0[2], p.0[1], p.0[0], 255]);
            }
            let writer = DataWriter::new().map_err(failed)?;
            writer.WriteBytes(&bgra).map_err(failed)?;
            let buffer = writer.DetachBuffer().map_err(failed)?;
            let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, pw as i32, ph as i32).map_err(failed)?;
            let result = self.engine.RecognizeAsync(&bitmap).map_err(failed)?.join().map_err(failed)?;

            let mut words = Vec::new();
            for (line_no, line) in result.Lines().map_err(failed)?.into_iter().enumerate() {
                for word in line.Words().map_err(failed)? {
                    let text = word.Text().map_err(failed)?.to_string();
                    let r = word.BoundingRect().map_err(failed)?;
                    if text.trim().is_empty() || r.Width <= 0.0 || r.Height <= 0.0 {
                        continue;
                    }
                    words.push(OcrWord { text, x: r.X * factor, y: r.Y * factor, w: r.Width * factor, h: r.Height * factor, line: line_no as u32 });
                }
            }
            Ok(words)
        }

        fn describe(&self) -> String {
            if self.tag.is_empty() { "Windows OCR".to_string() } else { format!("Windows OCR ({})", self.tag) }
        }
    }
}

// ---------------------------------------------------------------------------
// Tesseract
// ---------------------------------------------------------------------------

/// BCP-47 primary language to Tesseract's three-letter code.
const TESSERACT_CODES: [(&str, &str); 40] = [
    ("en", "eng"), ("de", "deu"), ("fr", "fra"), ("es", "spa"), ("it", "ita"), ("pt", "por"), ("nl", "nld"), ("sv", "swe"),
    ("da", "dan"), ("nb", "nor"), ("no", "nor"), ("fi", "fin"), ("pl", "pol"), ("cs", "ces"), ("sk", "slk"), ("sl", "slv"),
    ("hu", "hun"), ("ro", "ron"), ("bg", "bul"), ("hr", "hrv"), ("sr", "srp"), ("ru", "rus"), ("uk", "ukr"), ("tr", "tur"),
    ("el", "ell"), ("ca", "cat"), ("id", "ind"), ("vi", "vie"), ("th", "tha"), ("ja", "jpn"), ("ko", "kor"), ("hi", "hin"),
    ("bn", "ben"), ("ta", "tam"), ("ar", "ara"), ("he", "heb"), ("fa", "fas"), ("lt", "lit"), ("lv", "lav"), ("et", "est"),
];

/// Turn `en-US` into `eng`. Something that already looks like a Tesseract code (`deu`,
/// `chi_sim`, `eng+fra`) is passed through.
pub fn tesseract_language(tag: &str) -> String {
    let tag = tag.trim();
    let lower = tag.to_ascii_lowercase();
    if lower.starts_with("zh") {
        let traditional = lower.contains("hant") || lower.ends_with("-tw") || lower.ends_with("-hk") || lower.ends_with("-mo");
        return if traditional { "chi_tra".into() } else { "chi_sim".into() };
    }
    if tag.contains('+') || tag.contains('_') || (tag.len() == 3 && !tag.contains('-')) {
        return tag.to_string();
    }
    let primary = lower.split('-').next().unwrap_or("");
    TESSERACT_CODES.iter().find(|(bcp, _)| *bcp == primary).map(|(_, code)| code.to_string()).unwrap_or_else(|| tag.to_string())
}

/// Read Tesseract's TSV output (`tesseract in.png stdout tsv`). Only word rows (level 5)
/// with a confidence are kept.
pub fn parse_tesseract_tsv(tsv: &str) -> Vec<OcrWord> {
    let mut words = Vec::new();
    let mut lines: Vec<(u32, u32, u32, u32)> = Vec::new();
    for row in tsv.lines() {
        let cols: Vec<&str> = row.split('\t').collect();
        if cols.len() < 12 || cols[0] != "5" {
            continue;
        }
        let num = |i: usize| cols[i].trim().parse::<f32>().ok();
        let key = |i: usize| cols[i].trim().parse::<u32>().unwrap_or(0);
        let (Some(x), Some(y), Some(w), Some(h), Some(conf)) = (num(6), num(7), num(8), num(9), num(10)) else {
            continue;
        };
        // The text is the last column and may itself contain tabs in theory.
        let text = cols[11..].join(" ").trim().to_string();
        if conf < 0.0 || text.is_empty() || w <= 0.0 || h <= 0.0 {
            continue;
        }
        let line_key = (key(1), key(2), key(3), key(4));
        let line = match lines.iter().position(|k| *k == line_key) {
            Some(i) => i,
            None => {
                lines.push(line_key);
                lines.len() - 1
            }
        };
        words.push(OcrWord { text, x, y, w, h, line: line as u32 });
    }
    words
}

fn tesseract_list(exe: &Path) -> Vec<String> {
    let Ok(output) = helpers::run(Command::new(exe).arg("--list-langs"), "Tesseract") else {
        return Vec::new();
    };
    let text = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.contains(' ') && !l.contains(':') && *l != "osd" && *l != "equ").map(str::to_string).collect()
}

struct Tesseract {
    exe: PathBuf,
    language: String,
    dpi: u32,
}

impl Tesseract {
    fn open(language: &str, dpi: u32) -> Result<Tesseract> {
        let exe = helpers::find_tesseract().ok_or_else(|| Error::MissingHelper { tool: "Tesseract".into(), hint: TESSERACT_HINT.into() })?;
        let installed = tesseract_list(&exe);
        let language = if language.trim().is_empty() {
            if installed.is_empty() || installed.iter().any(|l| l == "eng") { "eng".to_string() } else { installed[0].clone() }
        } else {
            tesseract_language(language)
        };
        let missing: Vec<&str> = language.split('+').filter(|part| !installed.is_empty() && !installed.iter().any(|l| l == part)).collect();
        if !missing.is_empty() {
            return Err(Error::invalid(format!(
                "Tesseract has no language data for \"{}\". Installed: {}. Install that language pack or choose another language.",
                missing.join("+"),
                installed.join(", ")
            )));
        }
        Ok(Tesseract { exe, language, dpi })
    }
}

impl OcrEngine for Tesseract {
    fn recognise(&self, image: &RgbaImage) -> Result<Vec<OcrWord>> {
        let tmp = TempDir::new("ocr")?;
        let png = tmp.path().join("page.png");
        image::DynamicImage::ImageRgba8(image.clone()).to_rgb8().save(&png)?;
        let mut command = Command::new(&self.exe);
        command.arg(&png).arg("stdout").args(["-l", &self.language, "--dpi", &self.dpi.to_string(), "tsv"]);
        let output = helpers::run(&mut command, "Tesseract")?;
        Ok(parse_tesseract_tsv(&String::from_utf8_lossy(&output.stdout)))
    }

    fn describe(&self) -> String {
        format!("Tesseract ({})", self.language)
    }
}

// ---------------------------------------------------------------------------
// Choosing an engine
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn open_system(language: &str) -> Result<Box<dyn OcrEngine>> {
    Ok(Box::new(system::open(language)?))
}

#[cfg(not(windows))]
fn open_system(_language: &str) -> Result<Box<dyn OcrEngine>> {
    Err(Error::Unsupported("The built-in OCR engine is only available on Windows. Choose Tesseract instead.".into()))
}

fn open_engine(opts: &Options) -> Result<Box<dyn OcrEngine>> {
    match opts.engine {
        Engine::System => open_system(&opts.language),
        Engine::Tesseract => Ok(Box::new(Tesseract::open(&opts.language, opts.dpi)?)),
        Engine::Auto => {
            let system = if cfg!(windows) { Some(open_system(&opts.language)) } else { None };
            match system {
                Some(Ok(engine)) => Ok(engine),
                other => match Tesseract::open(&opts.language, opts.dpi) {
                    Ok(engine) => Ok(Box::new(engine)),
                    Err(Error::MissingHelper { .. }) => match other {
                        // Windows has an engine but not this language: say that, it is the more useful message.
                        Some(Err(e @ Error::Invalid(_))) => Err(e),
                        _ => Err(missing_engine()),
                    },
                    Err(e) => Err(e),
                },
            }
        }
    }
}

/// Languages the engine `auto` would use can read, as BCP-47 tags on Windows (`en-US`)
/// and Tesseract codes elsewhere (`eng`). Empty when there is no engine.
pub fn available_languages() -> Vec<String> {
    #[cfg(windows)]
    {
        let system = system::languages();
        if !system.is_empty() {
            return system;
        }
    }
    helpers::find_tesseract().map(|exe| tesseract_list(&exe)).unwrap_or_default()
}

/// The engine `auto` would use: "Windows OCR", "Tesseract", or `None` when OCR cannot run
/// on this machine.
pub fn engine_name() -> Option<String> {
    #[cfg(windows)]
    {
        if !system::languages().is_empty() {
            return Some("Windows OCR".to_string());
        }
    }
    helpers::find_tesseract().map(|_| "Tesseract".to_string())
}

// ---------------------------------------------------------------------------
// The text layer
// ---------------------------------------------------------------------------

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

/// Font size and baseline (both in pixels) a word box implies. A box is as tall as the
/// letters in it: a word with a capital and a descender spans more of the em square than
/// one made of small letters, and only some words reach below the baseline.
fn word_metrics(word: &OcrWord) -> (f32, f32) {
    let tall = word.text.chars().any(|c| c.is_uppercase() || c.is_ascii_digit() || "bdfhklt()[]{}/|!?$%&#@".contains(c) || !c.is_ascii());
    let below = word.text.chars().any(|c| "gjpqy,;()[]{}|".contains(c));
    let above_part = if tall { 0.72 } else { 0.52 };
    let below_part = if below { 0.21 } else { 0.0 };
    let size = word.h / (above_part + below_part);
    (size, word.y + word.h - below_part * size)
}

/// Content-stream text for the recognised words of one page, to be drawn in visible
/// space. `sx`, `sy` are pixels per point; `height` is the page height in points.
/// Returns the operators and how many words held characters the font cannot store.
fn text_layer(words: &[OcrWord], font: &str, sx: f32, sy: f32, height: f32) -> (String, usize) {
    let mut out = String::from("BT\n3 Tr\n");
    let mut lossy = 0;
    let mut start = 0;
    while start < words.len() {
        let mut end = start + 1;
        while end < words.len() && words[end].line == words[start].line {
            end += 1;
        }
        let line = &words[start..end];
        // One size and one baseline for the whole line, so it reads back as one line.
        let metrics: Vec<(f32, f32)> = line.iter().map(word_metrics).collect();
        let size_px = median(&mut metrics.iter().map(|m| m.0).collect::<Vec<_>>());
        let baseline_px = median(&mut metrics.iter().map(|m| m.1).collect::<Vec<_>>());
        let size = (size_px / sy).clamp(1.0, 800.0);
        let y = height - baseline_px / sy;
        for (i, word) in line.iter().enumerate() {
            let text = word.text.trim();
            if text.is_empty() {
                continue;
            }
            if text.chars().any(|c| c != '?' && doc::winansi(&c.to_string()) == b"?") {
                lossy += 1;
            }
            let natural = StdFont::Helvetica.width(text, size);
            let stretch = if natural > 0.0 { (100.0 * (word.w / sx) / natural).clamp(5.0, 2000.0) } else { 100.0 };
            // A trailing space keeps neighbouring words apart for programs that copy the text.
            let shown = if i + 1 < line.len() { format!("{text} ") } else { text.to_string() };
            out += &format!("/{font} {} Tf {} Tz 1 0 0 1 {} {} Tm {} Tj\n", fmt(size), fmt(stretch), fmt(word.x / sx), fmt(y), pdf_string(&shown));
        }
        start = end;
    }
    out += "ET\n";
    (out, lossy)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let [input] = inputs else {
        return Err(Error::invalid("OCR works on one PDF at a time."));
    };
    if opts.dpi < 50 || opts.dpi > 1200 {
        return Err(Error::invalid("The OCR resolution must be between 50 and 1200 dpi."));
    }
    ctx.report(0.0, "Opening the file");
    let bytes = doc::read_file(input)?;
    let mut d = doc::load_bytes(&bytes, ctx.password())?;
    let page_ids = doc::page_ids(&d);
    let selected = range::parse(&opts.pages, page_ids.len())?;
    let reader = TextReader::open(bytes.clone(), ctx.password())?;
    let renderer = Renderer::open(bytes.clone(), ctx.password())?;
    let engine = open_engine(opts)?;
    let mut notes = run_with_engine(engine.as_ref(), &mut d, &reader, &renderer, &selected, opts, ctx)?;
    ctx.report(0.97, "Saving");
    doc::save(&mut d, out)?;
    let mut outcome = Outcome::single(out.to_path_buf(), page_ids.len(), bytes.len() as u64);
    outcome.notes.append(&mut notes);
    if d.was_encrypted() {
        outcome.notes.push("The copy with the text layer is not password protected. Use Protect to add a password again.".to_string());
    }
    ctx.report(1.0, "Done");
    Ok(outcome)
}

/// Add the text layer to `selected` pages (1-based) of `d` using `engine`. Returns the
/// notes for the user. Split from `run` so a different engine can be supplied.
pub fn run_with_engine(engine: &dyn OcrEngine, d: &mut lopdf::Document, reader: &TextReader, renderer: &Renderer, selected: &[usize], opts: &Options, ctx: &Ctx) -> Result<Vec<String>> {
    let page_ids = doc::page_ids(d);
    let font_id = d.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let (mut read, mut skipped, mut empty, mut words_total, mut lossy_total) = (0usize, 0usize, 0usize, 0usize, 0usize);

    for (n, &page) in selected.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.03 + 0.92 * n as f32 / selected.len().max(1) as f32, &format!("Reading text, page {page} of {}", page_ids.len()));
        let index = page - 1;
        let Some(&page_id) = page_ids.get(index) else { continue };
        if opts.skip_text_pages && !reader.page(index)?.is_empty() {
            skipped += 1;
            continue;
        }
        let picture = renderer.render(index, dpi_to_scale(opts.dpi as f32))?;
        let words = engine.recognise(&picture)?;
        if words.is_empty() {
            empty += 1;
            continue;
        }
        // The picture shows the page as displayed, which is exactly "visible space".
        let (vis_w, vis_h) = doc::visible_size(d, page_id);
        let (sx, sy) = (picture.width() as f32 / vis_w.max(1.0), picture.height() as f32 / vis_h.max(1.0));
        let font = doc::add_resource(d, page_id, "Font", "ShOcr", Object::Reference(font_id))?;
        let (layer, lossy) = text_layer(&words, &font, sx, sy, vis_h);
        let content = doc::cm(doc::visible_to_page(d, page_id)) + &layer;
        doc::overlay(d, page_id, content.into_bytes(), false)?;
        read += 1;
        words_total += words.len();
        lossy_total += lossy;
    }

    let mut notes = Vec::new();
    if read > 0 {
        notes.push(format!("Added a text layer to {} ({}) using {}.", plural(read, "page", "pages"), plural(words_total, "word", "words"), engine.describe()));
    } else {
        notes.push("No text layer was added.".to_string());
    }
    if skipped > 0 {
        notes.push(format!("{} already had text and {} left alone.", plural(skipped, "page", "pages"), if skipped == 1 { "was" } else { "were" }));
    }
    if empty > 0 {
        notes.push(format!("No text was found on {}.", plural(empty, "page", "pages")));
    }
    if lossy_total > 0 {
        notes.push(format!("{} contained characters outside the Western European set; those characters are stored as \"?\" in the text layer.", plural(lossy_total, "word", "words")));
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_map_to_tesseract_codes() {
        assert_eq!(tesseract_language("en-US"), "eng");
        assert_eq!(tesseract_language("de"), "deu");
        assert_eq!(tesseract_language("zh-Hans-CN"), "chi_sim");
        assert_eq!(tesseract_language("zh-TW"), "chi_tra");
        assert_eq!(tesseract_language("eng+fra"), "eng+fra");
        assert_eq!(tesseract_language("deu"), "deu");
    }

    #[test]
    fn tsv_rows_become_words_grouped_by_line() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
            1\t1\t0\t0\t0\t0\t0\t0\t1654\t2339\t-1\t\n\
            4\t1\t1\t1\t1\t0\t156\t150\t400\t60\t-1\t\n\
            5\t1\t1\t1\t1\t1\t156\t150\t220\t60\t96.4\tDelivery\n\
            5\t1\t1\t1\t1\t2\t390\t152\t120\t46\t95.1\tNote\n\
            5\t1\t1\t1\t2\t1\t156\t240\t90\t30\t91.0\tPage\n\
            5\t1\t1\t1\t2\t2\t260\t240\t20\t30\t-1\t \n";
        let words = parse_tesseract_tsv(tsv);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0], OcrWord { text: "Delivery".into(), x: 156.0, y: 150.0, w: 220.0, h: 60.0, line: 0 });
        assert_eq!((words[1].text.as_str(), words[1].line), ("Note", 0));
        assert_eq!((words[2].text.as_str(), words[2].line), ("Page", 1));
    }

    #[test]
    fn text_layer_is_invisible_and_fits_the_boxes() {
        let words = vec![
            OcrWord { text: "Delivery".into(), x: 100.0, y: 100.0, w: 200.0, h: 46.5, line: 0 },
            OcrWord { text: "Note".into(), x: 320.0, y: 100.0, w: 100.0, h: 36.0, line: 0 },
        ];
        // 2 pixels per point on a page 400 points tall.
        let (layer, lossy) = text_layer(&words, "F9", 2.0, 2.0, 400.0);
        assert_eq!(lossy, 0);
        assert!(layer.starts_with("BT\n3 Tr\n") && layer.ends_with("ET\n"));
        assert!(layer.contains("(Delivery ) Tj") && layer.contains("(Note) Tj"), "{layer}");
        // Both words sit on the baseline of the line: 100 + 36 = 136 px down, 68 pt, so y = 332.
        assert_eq!(layer.matches(" 332 Tm").count(), 2, "{layer}");
        // 50 px cap height is 25 pt.
        assert!(layer.contains("/F9 25 Tf"), "{layer}");
    }
}
