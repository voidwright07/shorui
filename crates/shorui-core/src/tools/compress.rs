//! Compress: Make a PDF smaller.
//!
//! Most of the weight of a large PDF is in its pictures, so that is where the work goes:
//!
//! 1. Find every image the pages draw (directly or inside Form XObjects) and work out how
//!    large it is drawn, by following the transformation matrix through the page content.
//!    An image whose placement cannot be worked out is assumed to fill a page.
//! 2. Decode it, scale it down when it has more pixels than `max_dpi` needs, and store it
//!    as JPEG at `image_quality`. The new stream replaces the old one only when it is
//!    smaller. A soft mask (transparency) is kept and scaled to match.
//! 3. Flate-compress streams that were stored raw, squeeze existing Flate streams harder,
//!    drop objects nothing refers to, merge byte-identical streams, and write the file
//!    with object streams.
//!
//! Images that cannot be decoded safely are left exactly as they are: JBIG2, JPEG 2000 and
//! CCITT fax data, CMYK and other non-RGB/grey colour spaces, stencil masks, images with a
//! colour-key mask or a pre-multiplied soft mask, and inline images.
//!
//! The output is never larger than the input: when nothing is gained the input is copied.

use crate::helpers::TempDir;
use crate::{Ctx, Error, Outcome, Result, doc, render};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use image::{DynamicImage, GrayImage, RgbImage, imageops::FilterType};
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// JPEG quality 85, images capped at 220 dpi.
    Light,
    /// JPEG quality 60, images capped at 150 dpi.
    #[default]
    Balanced,
    /// JPEG quality 35, images capped at 96 dpi.
    Strong,
    /// Use `image_quality` and `max_dpi` as given.
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// `light`, `balanced`, `strong` or `custom`. Anything but `custom` overrides
    /// `image_quality` and `max_dpi`.
    pub preset: Preset,
    /// JPEG quality, 1 to 100. Used when `preset` is `custom`.
    pub image_quality: u8,
    /// Images drawn at a higher resolution than this are scaled down. Used when `preset`
    /// is `custom`.
    pub max_dpi: u32,
    /// Store recompressed colour images in grey. Text and line art keep their colours.
    pub grayscale: bool,
    /// Fonts are not subset (that cannot be done safely on arbitrary files here). Unused
    /// objects and duplicate streams are always removed; with this on, duplicate font
    /// objects (font dictionaries, descriptors and encodings that are identical once
    /// their embedded font programs have been merged) are merged as well, which is what
    /// shrinks files assembled from many documents that each embedded the same font.
    pub subset_fonts: bool,
    /// Remove the document information, XMP metadata and editing-application private data.
    pub strip_metadata: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { preset: Preset::Balanced, image_quality: 60, max_dpi: 150, grayscale: false, subset_fonts: false, strip_metadata: false }
    }
}

impl Options {
    /// The JPEG quality and dpi cap in force once the preset is applied.
    pub fn effective(&self) -> (u8, u32) {
        match self.preset {
            Preset::Light => (85, 220),
            Preset::Balanced => (60, 150),
            Preset::Strong => (35, 96),
            Preset::Custom => (self.image_quality.clamp(1, 100), self.max_dpi.clamp(18, 2400)),
        }
    }
}

/// Files up to this size are estimated by really compressing them in memory.
const FULL_ESTIMATE_LIMIT: u64 = 30 * 1024 * 1024;
/// Largest image, in pixels, that will be decoded.
const MAX_PIXELS: u64 = 160_000_000;

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let [input] = inputs else {
        return Err(Error::invalid("Compress works on one PDF at a time."));
    };
    ctx.report(0.0, "Reading the file");
    let bytes = doc::read_file(input)?;
    let built = build(&bytes, ctx.password(), opts, ctx, out)?;
    ctx.report(1.0, "Done");

    let mut outcome = Outcome::single(out.to_path_buf(), built.pages, bytes.len() as u64);
    if built.copied {
        outcome.notes.push("Already small, copied as is".to_string());
        return Ok(outcome);
    }
    if built.images_total > 0 {
        let left = built.images_total - built.images_done;
        let mut note = format!("Recompressed {} of {}", built.images_done, plural(built.images_total, "image", "images"));
        if left > 0 {
            note += &format!("; {left} left as {} (already small, or stored in a format that is not safe to change)", if left == 1 { "it was" } else { "they were" });
        }
        outcome.notes.push(note + ".");
    }
    if built.was_encrypted {
        outcome.notes.push("The compressed copy is not password protected. Use Protect to add a password again.".to_string());
    }
    Ok(outcome)
}

/// A fast estimate of the size `run` would produce, in bytes, for the "est." figure.
///
/// Files up to about 30 MB are really compressed (into a temporary folder that is removed
/// straight away), so the figure is exact. Larger files are not: up to twelve of their
/// images, spread evenly through the document, are recompressed, and the saving found on
/// those is applied to all image data while everything else is counted at its present
/// size. The result is never more than the size of the input.
pub fn estimate(path: &Path, password: Option<&str>, opts: &Options) -> Result<u64> {
    let bytes = doc::read_file(path)?;
    let size = bytes.len() as u64;
    if size <= FULL_ESTIMATE_LIMIT {
        let tmp = TempDir::new("estimate")?;
        let built = build(&bytes, password, opts, &Ctx::none(), &tmp.path().join("estimate.pdf"))?;
        return Ok(built.size.min(size));
    }

    estimate_sampled(&bytes, password, opts)
}

/// The estimate for large files: recompress a sample of the images and extrapolate.
fn estimate_sampled(bytes: &[u8], password: Option<&str>, opts: &Options) -> Result<u64> {
    let size = bytes.len() as u64;
    let d = doc::load_bytes(bytes, password)?;
    let settings = Settings::new(opts);
    let usages = collect_usages(&d);
    let stored: Vec<(ObjectId, u64)> = usages.order.iter().filter_map(|id| Some((*id, d.get_object(*id).ok()?.as_stream().ok()?.content.len() as u64))).collect();
    let image_bytes: u64 = stored.iter().map(|(_, n)| n).sum();
    if stored.is_empty() || image_bytes == 0 {
        return Ok(size);
    }
    let step = stored.len().div_ceil(12).max(1);
    let (mut before, mut after) = (0u64, 0u64);
    let mut done_masks = HashSet::new();
    for (id, len) in stored.iter().step_by(step) {
        before += len;
        after += match usages.map.get(id).and_then(|u| plan_image(&d, *id, u, &settings, &mut done_masks)) {
            Some(plan) => plan.data.len() as u64,
            None => *len,
        };
    }
    let ratio = if before == 0 { 1.0 } else { after as f64 / before as f64 };
    let rest = size.saturating_sub(image_bytes);
    Ok((rest + (image_bytes as f64 * ratio) as u64).min(size))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

struct Built {
    size: u64,
    pages: usize,
    images_total: usize,
    images_done: usize,
    /// Nothing was gained, so the output is the input.
    copied: bool,
    was_encrypted: bool,
}

struct Settings {
    quality: u8,
    max_dpi: f64,
    grayscale: bool,
}

impl Settings {
    fn new(opts: &Options) -> Self {
        let (quality, dpi) = opts.effective();
        Settings { quality, max_dpi: dpi as f64, grayscale: opts.grayscale }
    }
}

/// The whole pipeline. Writes the result (or a copy of the input) to `dest`.
fn build(input: &[u8], password: Option<&str>, opts: &Options, ctx: &Ctx, dest: &Path) -> Result<Built> {
    let mut d = doc::load_bytes(input, password)?;
    let was_encrypted = d.was_encrypted();
    let page_ids = doc::page_ids(&d);
    let pages = page_ids.len();
    if pages == 0 {
        return Err(Error::damaged("This file has no pages. If it should have, try Repair first."));
    }
    ctx.check()?;

    // 1. Images.
    ctx.report(0.02, "Looking for images");
    let settings = Settings::new(opts);
    let usages = collect_usages(&d);
    let images_total = usages.order.len();
    let mut images_done = 0;
    let mut done_masks: HashSet<ObjectId> = HashSet::new();
    for (n, id) in usages.order.iter().enumerate() {
        ctx.check()?;
        let Some(usage) = usages.map.get(id) else { continue };
        let what = if usage.page < pages { format!("Recompressing images, page {} of {}", usage.page + 1, pages) } else { "Recompressing images".to_string() };
        ctx.report(0.05 + 0.75 * n as f32 / images_total.max(1) as f32, &what);
        if let Some(plan) = plan_image(&d, *id, usage, &settings, &mut done_masks) {
            apply_plan(&mut d, *id, plan);
            images_done += 1;
        }
    }
    ctx.check()?;

    // 2. Everything else.
    ctx.report(0.82, "Removing what is not needed");
    for id in &page_ids {
        if let Ok(page) = d.get_dictionary_mut(*id) {
            // Embedded page thumbnails are redundant: every viewer draws its own.
            page.remove(b"Thumb");
        }
    }
    if opts.strip_metadata {
        strip_metadata(&mut d);
    }
    d.prune_objects();
    ctx.check()?;
    ctx.report(0.86, "Compressing page content");
    for (n, object) in d.objects.values_mut().enumerate() {
        if n % 64 == 0 {
            ctx.check()?;
        }
        if let Object::Stream(stream) = object {
            squeeze(stream);
        }
    }
    ctx.report(0.92, "Merging duplicates");
    merge_duplicates(&mut d, opts.subset_fonts);
    d.prune_objects();
    ctx.check()?;

    // 3. Save. The compact form is tried first and has to read back correctly.
    ctx.report(0.95, "Saving");
    let reads_back = |path: &Path| -> bool {
        let Ok(bytes) = std::fs::read(path) else { return false };
        let lopdf_ok = doc::load_bytes(&bytes, None).is_ok_and(|check| doc::page_count(&check) == pages);
        lopdf_ok && render::Renderer::open(bytes, None).is_ok_and(|r| r.page_count() == pages)
    };
    let mut size = doc::save_compact(&mut d, dest)?;
    if !reads_back(dest) {
        size = doc::save(&mut d, dest)?;
        if !reads_back(dest) {
            doc::write_file(dest, input)?;
            return Ok(Built { size: input.len() as u64, pages, images_total, images_done: 0, copied: true, was_encrypted });
        }
    }
    if size >= input.len() as u64 {
        doc::write_file(dest, input)?;
        return Ok(Built { size: input.len() as u64, pages, images_total, images_done: 0, copied: true, was_encrypted });
    }
    Ok(Built { size, pages, images_total, images_done, copied: false, was_encrypted })
}

// ---------------------------------------------------------------------------
// Where images are drawn, and how large
// ---------------------------------------------------------------------------

type Mat = [f64; 6];
const IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b`, in the row-vector convention PDF uses.
fn mul(a: Mat, b: Mat) -> Mat {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn matrix(items: &[Object]) -> Option<Mat> {
    if items.len() < 6 {
        return None;
    }
    let mut m = IDENTITY;
    for (slot, item) in m.iter_mut().zip(items) {
        *slot = number(item)?;
    }
    m.iter().all(|v| v.is_finite()).then_some(m)
}

fn dict_of<'a>(d: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match doc::deref(d, obj) {
        Object::Dictionary(dict) => Some(dict),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn page_resources(d: &Document, page: ObjectId) -> Option<&Dictionary> {
    let mut id = page;
    for _ in 0..64 {
        let dict = d.get_dictionary(id).ok()?;
        if let Ok(r) = dict.get(b"Resources") {
            return dict_of(d, r);
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// One use of an image. `mat` maps the image's unit square to the space of whatever
/// drew it; `None` when the placement could not be worked out.
#[derive(Clone, Copy)]
struct Draw {
    image: ObjectId,
    mat: Option<Mat>,
}

struct Walker<'a> {
    d: &'a Document,
    forms: HashMap<ObjectId, Rc<Vec<Draw>>>,
    stack: Vec<ObjectId>,
}

impl<'a> Walker<'a> {
    fn xobjects(&self, resources: Option<&'a Dictionary>) -> Option<&'a Dictionary> {
        dict_of(self.d, resources?.get(b"XObject").ok()?)
    }

    /// Every image drawn by a content stream.
    fn draws(&mut self, content: &[u8], resources: Option<&'a Dictionary>) -> Vec<Draw> {
        let Some(xobjects) = self.xobjects(resources) else {
            return Vec::new();
        };
        let Ok(parsed) = Content::decode(content) else {
            return self.unplaced(xobjects, resources);
        };
        let mut out = Vec::new();
        let mut ctm = IDENTITY;
        let mut saved: Vec<Mat> = Vec::new();
        for op in &parsed.operations {
            match op.operator.as_str() {
                "q" => saved.push(ctm),
                "Q" => {
                    if let Some(m) = saved.pop() {
                        ctm = m;
                    }
                }
                "cm" => {
                    if let Some(m) = matrix(&op.operands) {
                        ctm = mul(m, ctm);
                    }
                }
                "Do" => {
                    let target = op.operands.first().and_then(|o| o.as_name().ok()).and_then(|name| xobjects.get(name).ok()).and_then(|o| o.as_reference().ok());
                    if let Some(id) = target {
                        self.draw(id, Some(ctm), resources, &mut out);
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn draw(&mut self, id: ObjectId, ctm: Option<Mat>, resources: Option<&'a Dictionary>, out: &mut Vec<Draw>) {
        let Ok(Object::Stream(stream)) = self.d.get_object(id) else {
            return;
        };
        match stream.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Image") => out.push(Draw { image: id, mat: ctm }),
            Ok(b"Form") => {
                for inner in self.form(id, resources).iter() {
                    let mat = match (inner.mat, ctm) {
                        (Some(m), Some(ctm)) => Some(mul(m, ctm)),
                        _ => None,
                    };
                    out.push(Draw { image: inner.image, mat });
                }
            }
            _ => {}
        }
    }

    /// The images a Form XObject draws, in the space of whatever draws the form.
    fn form(&mut self, id: ObjectId, parent_resources: Option<&'a Dictionary>) -> Rc<Vec<Draw>> {
        if let Some(known) = self.forms.get(&id) {
            return known.clone();
        }
        if self.stack.contains(&id) || self.stack.len() > 24 {
            return Rc::new(Vec::new());
        }
        let Ok(Object::Stream(stream)) = self.d.get_object(id) else {
            return Rc::new(Vec::new());
        };
        self.stack.push(id);
        let resources = stream.dict.get(b"Resources").ok().and_then(|o| dict_of(self.d, o)).or(parent_resources);
        let form_matrix = stream.dict.get(b"Matrix").ok().and_then(|o| doc::deref(self.d, o).as_array().ok()).and_then(|a| matrix(a)).unwrap_or(IDENTITY);
        let inner = match stream.get_plain_content() {
            Ok(content) => self.draws(&content, resources),
            Err(_) => match self.xobjects(resources) {
                Some(xobjects) => self.unplaced(xobjects, resources),
                None => Vec::new(),
            },
        };
        self.stack.pop();
        let result = Rc::new(inner.into_iter().map(|draw| Draw { image: draw.image, mat: draw.mat.map(|m| mul(m, form_matrix)) }).collect::<Vec<_>>());
        self.forms.insert(id, result.clone());
        result
    }

    /// Everything in a resource dictionary, for content that could not be parsed.
    fn unplaced(&mut self, xobjects: &'a Dictionary, resources: Option<&'a Dictionary>) -> Vec<Draw> {
        let mut out = Vec::new();
        for (_, value) in xobjects.iter() {
            if let Ok(id) = value.as_reference() {
                self.draw(id, None, resources, &mut out);
            }
        }
        out
    }
}

struct Usage {
    /// The largest size the image is drawn at, in points.
    w_pt: f64,
    h_pt: f64,
    /// 0-based index of the first page that uses it; `usize::MAX` when no page content does.
    page: usize,
}

struct Usages {
    map: HashMap<ObjectId, Usage>,
    /// Images in the order they are first met, page by page.
    order: Vec<ObjectId>,
}

fn is_image(o: &Object) -> bool {
    matches!(o, Object::Stream(s) if matches!(s.dict.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")))
}

fn collect_usages(d: &Document) -> Usages {
    // Masks are handled together with the image they belong to, never on their own.
    let mut masks: HashSet<ObjectId> = HashSet::new();
    for object in d.objects.values() {
        if let Object::Stream(s) = object {
            for key in [&b"SMask"[..], b"Mask"] {
                if let Ok(id) = s.dict.get(key).and_then(Object::as_reference) {
                    masks.insert(id);
                }
            }
        }
    }

    let mut walker = Walker { d, forms: HashMap::new(), stack: Vec::new() };
    let mut usages = Usages { map: HashMap::new(), order: Vec::new() };
    let mut largest_page = (0.0f64, 0.0f64);
    for (index, page_id) in doc::page_ids(d).into_iter().enumerate() {
        let media = doc::media_box(d, page_id);
        let unit = d.get_dictionary(page_id).ok().and_then(|p| p.get(b"UserUnit").ok()).and_then(number).filter(|u| *u > 0.0).unwrap_or(1.0);
        let page_size = ((media[2] - media[0]) as f64 * unit, (media[3] - media[1]) as f64 * unit);
        largest_page = (largest_page.0.max(page_size.0), largest_page.1.max(page_size.1));
        let content = d.get_page_content(page_id);
        for draw in walker.draws(&content, page_resources(d, page_id)) {
            if masks.contains(&draw.image) {
                continue;
            }
            let (w, h) = match draw.mat {
                Some(m) => (m[0].hypot(m[1]) * unit, m[2].hypot(m[3]) * unit),
                None => page_size,
            };
            match usages.map.get_mut(&draw.image) {
                Some(u) => {
                    u.w_pt = u.w_pt.max(w);
                    u.h_pt = u.h_pt.max(h);
                }
                None => {
                    usages.map.insert(draw.image, Usage { w_pt: w, h_pt: h, page: index });
                    usages.order.push(draw.image);
                }
            }
        }
    }
    // Images used some other way (patterns, annotations, fonts): assume a full page.
    if largest_page.0 <= 0.0 || largest_page.1 <= 0.0 {
        largest_page = (612.0, 792.0);
    }
    for (id, object) in &d.objects {
        if is_image(object) && !masks.contains(id) && !usages.map.contains_key(id) {
            usages.map.insert(*id, Usage { w_pt: largest_page.0, h_pt: largest_page.1, page: usize::MAX });
            usages.order.push(*id);
        }
    }
    usages
}

// ---------------------------------------------------------------------------
// Decoding images
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Gray,
    Rgb,
}

struct ColourSpace {
    kind: Kind,
    /// For an indexed image: the palette, in the base space, and the base space itself.
    palette: Option<(Vec<u8>, Object)>,
}

fn colour_space(d: &Document, obj: &Object, allow_indexed: bool) -> Option<ColourSpace> {
    let plain = |kind| Some(ColourSpace { kind, palette: None });
    match doc::deref(d, obj) {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => plain(Kind::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => plain(Kind::Rgb),
            _ => None,
        },
        Object::Array(items) => match doc::deref(d, items.first()?).as_name().ok()? {
            b"CalGray" | b"DeviceGray" => plain(Kind::Gray),
            b"CalRGB" | b"DeviceRGB" => plain(Kind::Rgb),
            b"ICCBased" => match doc::deref(d, items.get(1)?).as_stream().ok()?.dict.get(b"N").and_then(Object::as_i64).ok()? {
                1 => plain(Kind::Gray),
                3 => plain(Kind::Rgb),
                _ => None,
            },
            b"Indexed" | b"I" if allow_indexed => {
                let base_obj = items.get(1)?;
                let base = colour_space(d, base_obj, false)?;
                let lookup = match doc::deref(d, items.get(3)?) {
                    Object::String(bytes, _) => bytes.clone(),
                    Object::Stream(s) => s.get_plain_content().ok()?,
                    _ => return None,
                };
                Some(ColourSpace { kind: base.kind, palette: Some((lookup, base_obj.clone())) })
            }
            _ => None,
        },
        _ => None,
    }
}

/// Undo the first `count` filters of a stream. `None` when one of them is not supported.
fn unfilter(d: &Document, stream: &Stream, filters: &[Vec<u8>], count: usize) -> Option<Vec<u8>> {
    let parms = stream.dict.get(b"DecodeParms").or_else(|_| stream.dict.get(b"DP")).ok().map(|o| doc::deref(d, o));
    let mut data = stream.content.clone();
    for (i, filter) in filters.iter().take(count).enumerate() {
        if !matches!(filter.as_slice(), b"FlateDecode" | b"LZWDecode" | b"ASCII85Decode" | b"ASCIIHexDecode" | b"RunLengthDecode") {
            return None;
        }
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(filter.clone()));
        let own = match parms {
            Some(Object::Dictionary(p)) if i == 0 => Some(p.clone()),
            Some(Object::Array(list)) => list.get(i).and_then(|o| doc::deref(d, o).as_dict().ok()).cloned(),
            _ => None,
        };
        if let Some(mut p) = own {
            // Predictor parameters may themselves be indirect.
            for (_, v) in p.iter_mut() {
                *v = doc::deref(d, v).clone();
            }
            dict.set("DecodeParms", Object::Dictionary(p));
        }
        data = Stream::new(dict, data).decompressed_content().ok()?;
    }
    Some(data)
}

fn filter_names(stream: &Stream) -> Vec<Vec<u8>> {
    stream.filters().map(|f| f.into_iter().map(<[u8]>::to_vec).collect()).unwrap_or_default()
}

/// Samples of `bits` bits each, one byte per sample, rows padded to whole bytes.
fn unpack(data: &[u8], width: usize, height: usize, comps: usize, bits: usize) -> Option<Vec<u8>> {
    let per_row = width * comps;
    let row_bytes = (per_row * bits).div_ceil(8);
    if row_bytes == 0 || data.len() < row_bytes * height {
        return None;
    }
    let mut out = Vec::with_capacity(per_row * height);
    for row in data.chunks_exact(row_bytes).take(height) {
        match bits {
            8 => out.extend_from_slice(&row[..per_row]),
            16 => out.extend(row.chunks_exact(2).take(per_row).map(|pair| pair[0])),
            1 | 2 | 4 => {
                let mask = (1u8 << bits) - 1;
                for i in 0..per_row {
                    let bit = i * bits;
                    let shift = 8 - bits - (bit % 8);
                    out.push((row[bit / 8] >> shift) & mask);
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Decoded {
    /// `ImageLuma8` or `ImageRgb8`.
    image: DynamicImage,
    /// Few distinct colours are likely (low bit depth or a palette), so a lossless
    /// encoding is worth trying next to JPEG.
    flat: bool,
    /// The colour space the output must be tagged with when it differs from the stored
    /// one (an indexed image becomes an image in its base space).
    base_space: Option<Object>,
    stored_as_jpeg: bool,
}

/// Decode an image XObject to 8-bit grey or RGB. `None` for anything that cannot be
/// reproduced exactly from the result, in which case the image must be left alone.
fn decode_image(d: &Document, stream: &Stream) -> Option<Decoded> {
    let dict = &stream.dict;
    let int = |key: &[u8]| dict.get(key).ok().and_then(|o| doc::deref(d, o).as_i64().ok());
    if matches!(dict.get(b"ImageMask").map(|o| doc::deref(d, o)), Ok(Object::Boolean(true))) {
        return None;
    }
    if matches!(dict.get(b"Mask").map(|o| doc::deref(d, o)), Ok(Object::Array(_))) {
        return None; // colour-key masking depends on exact sample values
    }
    let width = usize::try_from(int(b"Width")?).ok().filter(|w| *w > 0)?;
    let height = usize::try_from(int(b"Height")?).ok().filter(|h| *h > 0)?;
    if width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    let space = colour_space(d, dict.get(b"ColorSpace").ok()?, true)?;
    let comps = if space.palette.is_some() || space.kind == Kind::Gray { 1 } else { 3 };

    // /Decode: the default is fine, a plain inversion can be applied, anything else cannot.
    let mut invert = false;
    if let Ok(Object::Array(pairs)) = dict.get(b"Decode").map(|o| doc::deref(d, o)) {
        let values: Vec<f64> = pairs.iter().filter_map(|o| number(doc::deref(d, o))).collect();
        if values.len() != pairs.len() || values.len() != comps * 2 {
            return None;
        }
        let bits = int(b"BitsPerComponent").unwrap_or(8);
        let top = if space.palette.is_some() { ((1i64 << bits.clamp(1, 16)) - 1) as f64 } else { 1.0 };
        let is = |lo: f64, hi: f64| values.chunks(2).all(|p| (p[0] - lo).abs() < 1e-6 && (p[1] - hi).abs() < 1e-6);
        if is(0.0, top) {
        } else if is(1.0, 0.0) && space.palette.is_none() {
            invert = true;
        } else {
            return None;
        }
    }

    let filters = filter_names(stream);
    let jpeg = filters.last().is_some_and(|f| f.as_slice() == b"DCTDecode");
    let data = unfilter(d, stream, &filters, if jpeg { filters.len() - 1 } else { filters.len() })?;

    let mut image = if jpeg {
        if space.palette.is_some() {
            return None;
        }
        let decoded = image::load_from_memory_with_format(&data, image::ImageFormat::Jpeg).ok()?;
        if decoded.width() as usize != width || decoded.height() as usize != height {
            return None;
        }
        // A JPEG whose channels do not match the declared colour space (CMYK, or a
        // colour JPEG tagged grey) is not something to guess about.
        match (decoded.color(), space.kind) {
            (image::ColorType::L8, Kind::Gray) | (image::ColorType::Rgb8, Kind::Rgb) => decoded,
            _ => return None,
        }
    } else {
        let bits = usize::try_from(int(b"BitsPerComponent")?).ok()?;
        let samples = unpack(&data, width, height, comps, bits)?;
        match &space.palette {
            Some((lookup, _)) => {
                let n = if space.kind == Kind::Gray { 1 } else { 3 };
                let entries = lookup.len() / n;
                if entries == 0 {
                    return None;
                }
                let mut pixels = Vec::with_capacity(samples.len() * n);
                for index in samples {
                    let at = (index as usize).min(entries - 1) * n;
                    pixels.extend_from_slice(&lookup[at..at + n]);
                }
                if n == 1 {
                    DynamicImage::ImageLuma8(GrayImage::from_raw(width as u32, height as u32, pixels)?)
                } else {
                    DynamicImage::ImageRgb8(RgbImage::from_raw(width as u32, height as u32, pixels)?)
                }
            }
            None => {
                let samples = match bits {
                    1 | 2 | 4 => {
                        let top = (1u32 << bits) - 1;
                        samples.into_iter().map(|v| (v as u32 * 255 / top) as u8).collect()
                    }
                    _ => samples,
                };
                if comps == 1 {
                    DynamicImage::ImageLuma8(GrayImage::from_raw(width as u32, height as u32, samples)?)
                } else {
                    DynamicImage::ImageRgb8(RgbImage::from_raw(width as u32, height as u32, samples)?)
                }
            }
        }
    };
    if invert {
        image.invert();
    }
    let bits = int(b"BitsPerComponent").unwrap_or(8);
    Some(Decoded { image, flat: !jpeg && (bits < 8 || space.palette.is_some()), base_space: space.palette.map(|(_, base)| base), stored_as_jpeg: jpeg })
}

// ---------------------------------------------------------------------------
// Re-encoding
// ---------------------------------------------------------------------------

fn deflate(data: &[u8], level: u32) -> Vec<u8> {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(data.len() / 2 + 64), Compression::new(level));
    // Writing into a Vec cannot fail.
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

fn jpeg(image: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    match image {
        DynamicImage::ImageLuma8(g) => enc.encode_image(g).ok()?,
        DynamicImage::ImageRgb8(c) => enc.encode_image(c).ok()?,
        _ => return None,
    }
    Some(out)
}

struct MaskPlan {
    id: ObjectId,
    data: Vec<u8>,
}

struct Plan {
    data: Vec<u8>,
    filter: &'static str,
    width: u32,
    height: u32,
    /// The `/ColorSpace` to write, or `None` to keep the one the image has.
    space: Option<Object>,
    mask: Option<MaskPlan>,
}

/// How large a stream is on disk today, counting raw streams as if Flate-compressed
/// (they would be by the time the file is saved).
fn stored_size(stream: &Stream) -> usize {
    if stream.dict.has(b"Filter") { stream.content.len() } else { deflate(&stream.content, 6).len().min(stream.content.len()) }
}

/// Work out a smaller replacement for an image, without touching the document.
fn plan_image(d: &Document, id: ObjectId, usage: &Usage, settings: &Settings, done_masks: &mut HashSet<ObjectId>) -> Option<Plan> {
    let stream = d.get_object(id).ok()?.as_stream().ok()?;
    let mask = match stream.dict.get(b"SMask") {
        Ok(o) => {
            let mask_id = o.as_reference().ok()?;
            let mask = d.get_object(mask_id).ok()?.as_stream().ok()?;
            if mask.dict.has(b"Matte") {
                return None; // colours are pre-blended against the matte: resampling would fringe
            }
            Some((mask_id, mask))
        }
        Err(_) => None,
    };
    let decoded = decode_image(d, stream)?;
    let (width, height) = (decoded.image.width(), decoded.image.height());

    // Resolution on the page. The lower of the two axes decides, so nothing is ever
    // taken below the cap.
    let dpi_x = width as f64 * 72.0 / usage.w_pt.max(0.01);
    let dpi_y = height as f64 * 72.0 / usage.h_pt.max(0.01);
    let dpi = dpi_x.min(dpi_y);
    let scale = if dpi > settings.max_dpi * 1.1 { settings.max_dpi / dpi } else { 1.0 };
    let new_w = ((width as f64 * scale).round() as u32).clamp(1, width);
    let new_h = ((height as f64 * scale).round() as u32).clamp(1, height);
    let resized = (new_w, new_h) != (width, height);

    let mut image = decoded.image;
    let to_gray = settings.grayscale && matches!(image, DynamicImage::ImageRgb8(_));
    if resized {
        image = image.resize_exact(new_w, new_h, FilterType::CatmullRom);
    }
    if to_gray {
        image = DynamicImage::ImageLuma8(image.to_luma8());
    }

    let mut best = (jpeg(&image, settings.quality)?, "DCTDecode");
    if decoded.flat {
        let lossless = deflate(image.as_bytes(), 9);
        if lossless.len() <= best.0.len() {
            best = (lossless, "FlateDecode");
        }
    }
    // Recompressing a JPEG at the same size loses a little quality every time, so it has
    // to pay for itself; in every other case smaller is enough.
    let stored = stored_size(stream);
    let limit = if decoded.stored_as_jpeg && !resized { stored as f64 * 0.9 } else { stored as f64 };
    if best.0.len() as f64 >= limit {
        return None;
    }

    let space = if to_gray {
        Some(Object::Name(b"DeviceGray".to_vec()))
    } else {
        decoded.base_space
    };

    // The soft mask follows the image down, stored Flate, when that makes it smaller.
    let mut mask_plan = None;
    if let Some((mask_id, mask)) = mask {
        if resized && !done_masks.contains(&mask_id) {
            let same_size = |key: &[u8], want: u32| mask.dict.get(key).ok().and_then(|o| doc::deref(d, o).as_i64().ok()) == Some(want as i64);
            if same_size(b"Width", width) && same_size(b"Height", height) {
                if let Some(Decoded { image: DynamicImage::ImageLuma8(alpha), .. }) = decode_image(d, mask) {
                    let small = image::imageops::resize(&alpha, new_w, new_h, FilterType::Triangle);
                    let data = deflate(small.as_raw(), 9);
                    if data.len() < stored_size(mask) {
                        mask_plan = Some(MaskPlan { id: mask_id, data });
                    }
                }
            }
        }
        done_masks.insert(mask_id);
    }

    Some(Plan { data: best.0, filter: best.1, width: new_w, height: new_h, space, mask: mask_plan })
}

fn set_image_stream(stream: &mut Stream, data: Vec<u8>, filter: &str, width: u32, height: u32) {
    stream.dict.set("Width", width as i64);
    stream.dict.set("Height", height as i64);
    stream.dict.set("BitsPerComponent", 8);
    stream.dict.set("Filter", Object::Name(filter.as_bytes().to_vec()));
    for key in [&b"DecodeParms"[..], b"DP", b"Decode"] {
        stream.dict.remove(key);
    }
    stream.set_content(data);
    stream.allows_compression = false;
}

fn apply_plan(d: &mut Document, id: ObjectId, plan: Plan) {
    if let Some(mask) = plan.mask {
        if let Ok(Object::Stream(stream)) = d.get_object_mut(mask.id) {
            set_image_stream(stream, mask.data, "FlateDecode", plan.width, plan.height);
            stream.dict.set("ColorSpace", "DeviceGray");
        }
    }
    if let Ok(Object::Stream(stream)) = d.get_object_mut(id) {
        set_image_stream(stream, plan.data, plan.filter, plan.width, plan.height);
        if let Some(space) = plan.space {
            stream.dict.set("ColorSpace", space);
        }
    }
}

// ---------------------------------------------------------------------------
// Streams, metadata and duplicates
// ---------------------------------------------------------------------------

/// Flate-compress a raw stream, or recompress a Flate stream at the highest level, when
/// that makes it smaller. The decoded bytes are identical either way.
fn squeeze(stream: &mut Stream) {
    if !stream.dict.has(b"Filter") {
        if stream.allows_compression {
            let _ = stream.compress();
        }
        return;
    }
    let single_flate = match stream.dict.get(b"Filter") {
        Ok(Object::Name(n)) => n == b"FlateDecode",
        Ok(Object::Array(a)) => a.len() == 1 && matches!(a.first(), Some(Object::Name(n)) if n == b"FlateDecode"),
        _ => false,
    };
    if !single_flate || stream.content.len() < 64 {
        return;
    }
    // Only the zlib layer is redone; a predictor, if any, stays as it is.
    const LIMIT: u64 = 1 << 30;
    let mut raw = Vec::new();
    let mut reader = ZlibDecoder::new(stream.content.as_slice()).take(LIMIT + 1);
    if reader.read_to_end(&mut raw).is_err() || raw.len() as u64 > LIMIT || raw.is_empty() {
        return;
    }
    let packed = deflate(&raw, 9);
    if !packed.is_empty() && packed.len() < stream.content.len() {
        stream.set_content(packed);
    }
}

fn strip_metadata(d: &mut Document) {
    if let Some(Object::Reference(id)) = d.trailer.remove(b"Info") {
        d.objects.remove(&id);
    }
    for object in d.objects.values_mut() {
        let dict = match object {
            Object::Dictionary(dict) => dict,
            Object::Stream(s) => &mut s.dict,
            _ => continue,
        };
        for key in [&b"Metadata"[..], b"PieceInfo", b"LastModified"] {
            dict.remove(key);
        }
    }
}

/// A byte string that is equal for equal objects, used to find duplicates.
fn canonical(obj: &Object, out: &mut Vec<u8>) {
    match obj {
        Object::Null => out.push(b'n'),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"t" } else { b"f" }),
        Object::Integer(i) => out.extend_from_slice(format!("i{i};").as_bytes()),
        Object::Real(r) => out.extend_from_slice(format!("r{};", r.to_bits()).as_bytes()),
        Object::Name(n) => {
            out.extend_from_slice(format!("/{}:", n.len()).as_bytes());
            out.extend_from_slice(n);
        }
        Object::String(s, _) => {
            out.extend_from_slice(format!("s{}:", s.len()).as_bytes());
            out.extend_from_slice(s);
        }
        Object::Array(items) => {
            out.push(b'[');
            for item in items {
                canonical(item, out);
            }
            out.push(b']');
        }
        Object::Dictionary(dict) => canonical_dict(dict, out),
        Object::Stream(s) => {
            canonical_dict(&s.dict, out);
            out.extend_from_slice(format!("S{}:", s.content.len()).as_bytes());
        }
        Object::Reference(id) => out.extend_from_slice(format!("R{} {};", id.0, id.1).as_bytes()),
    }
}

fn canonical_dict(dict: &Dictionary, out: &mut Vec<u8>) {
    let mut entries: Vec<(&Vec<u8>, &Object)> = dict.iter().filter(|(k, _)| k.as_slice() != b"Length").collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    out.push(b'<');
    for (key, value) in entries {
        out.extend_from_slice(format!("/{}:", key.len()).as_bytes());
        out.extend_from_slice(key);
        canonical(value, out);
    }
    out.push(b'>');
}

fn remap(obj: &mut Object, map: &HashMap<ObjectId, ObjectId>) {
    match obj {
        Object::Reference(id) => {
            if let Some(to) = map.get(id) {
                *id = *to;
            }
        }
        Object::Array(items) => items.iter_mut().for_each(|o| remap(o, map)),
        Object::Dictionary(dict) => dict.iter_mut().for_each(|(_, o)| remap(o, map)),
        Object::Stream(s) => s.dict.iter_mut().for_each(|(_, o)| remap(o, map)),
        _ => {}
    }
}

/// Point every reference to a duplicate at the first copy and drop the rest. Streams are
/// merged when their dictionaries and data are identical; with `fonts`, identical font
/// dictionaries, descriptors and encodings are merged too. Runs a few rounds, because
/// merging one layer (font files) makes the next (descriptors, then fonts) identical.
fn merge_duplicates(d: &mut Document, fonts: bool) -> usize {
    use std::hash::{Hash, Hasher};
    let mut merged = 0;
    for _ in 0..4 {
        let mut seen: HashMap<(u64, usize), Vec<ObjectId>> = HashMap::new();
        let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
        for (id, object) in &d.objects {
            let eligible = match object {
                Object::Stream(_) => true,
                Object::Dictionary(dict) => fonts && matches!(dict.get(b"Type").and_then(Object::as_name), Ok(b"Font" | b"FontDescriptor" | b"Encoding")),
                _ => false,
            };
            if !eligible {
                continue;
            }
            let mut key = Vec::new();
            canonical(object, &mut key);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut hasher);
            let content_len = match object {
                Object::Stream(s) => {
                    s.content.hash(&mut hasher);
                    s.content.len()
                }
                _ => 0,
            };
            let bucket = seen.entry((hasher.finish(), content_len)).or_default();
            // A matching hash is only a hint: compare for real before merging.
            let twin = bucket.iter().copied().find(|other| match (d.objects.get(other), object) {
                (Some(Object::Stream(a)), Object::Stream(b)) => a.content == b.content && same_dict(&a.dict, &b.dict),
                (Some(Object::Dictionary(a)), Object::Dictionary(b)) => same_dict(a, b),
                _ => false,
            });
            match twin {
                Some(first) => {
                    map.insert(*id, first);
                }
                None => bucket.push(*id),
            }
        }
        if map.is_empty() {
            break;
        }
        merged += map.len();
        for id in map.keys() {
            d.objects.remove(id);
        }
        for object in d.objects.values_mut() {
            remap(object, &map);
        }
        let mut trailer = Object::Dictionary(std::mem::take(&mut d.trailer));
        remap(&mut trailer, &map);
        if let Object::Dictionary(t) = trailer {
            d.trailer = t;
        }
    }
    merged
}

fn same_dict(a: &Dictionary, b: &Dictionary) -> bool {
    let (mut x, mut y) = (Vec::new(), Vec::new());
    canonical_dict(a, &mut x);
    canonical_dict(b, &mut y);
    x == y
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    #[test]
    fn presets_set_quality_and_dpi() {
        let mut o = Options::default();
        assert_eq!(o.effective(), (60, 150));
        o.preset = Preset::Light;
        assert_eq!(o.effective(), (85, 220));
        o.preset = Preset::Strong;
        assert_eq!(o.effective(), (35, 96));
        o.preset = Preset::Custom;
        o.image_quality = 72;
        o.max_dpi = 300;
        assert_eq!(o.effective(), (72, 300));
        let parsed: Options = serde_json::from_str(r#"{"preset":"strong","grayscale":true}"#).unwrap();
        assert_eq!(parsed.preset, Preset::Strong);
        assert!(parsed.grayscale);
    }

    #[test]
    fn matrices_compose_in_pdf_order() {
        // Scale to 200 x 100 points, then move: the image ends up 200 x 100.
        let ctm = mul([200.0, 0.0, 0.0, 100.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0, 50.0, 60.0]);
        assert_eq!(ctm, [200.0, 0.0, 0.0, 100.0, 50.0, 60.0]);
        // A rotation keeps the lengths of the axes.
        let rotated = mul(ctm, [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        assert!((rotated[0].hypot(rotated[1]) - 200.0).abs() < 1e-9);
        assert!((rotated[2].hypot(rotated[3]) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn low_bit_depths_unpack() {
        assert_eq!(unpack(&[0b1010_0000, 0b0100_0000], 3, 2, 1, 1).unwrap(), vec![1, 0, 1, 0, 1, 0]);
        assert_eq!(unpack(&[0b1101_0010], 4, 1, 1, 2).unwrap(), vec![3, 1, 0, 2]);
        assert_eq!(unpack(&[0xA5, 0x0F], 3, 1, 1, 4).unwrap(), vec![10, 5, 0]);
        assert!(unpack(&[0], 9, 1, 1, 1).is_none());
    }

    #[test]
    fn indexed_and_bilevel_images_decode() {
        let d = Document::with_version("1.7");
        let palette = vec![255, 0, 0, 0, 0, 255];
        let indexed = Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 2, "Height" => 1, "BitsPerComponent" => 1,
                "ColorSpace" => vec![Object::Name(b"Indexed".to_vec()), Object::Name(b"DeviceRGB".to_vec()), 1.into(), Object::string_literal(palette)],
            },
            vec![0b0100_0000],
        );
        let decoded = decode_image(&d, &indexed).unwrap();
        assert_eq!(decoded.image.as_bytes(), &[255, 0, 0, 0, 0, 255]);
        assert!(decoded.flat && decoded.base_space.is_some());

        let inverted = Stream::new(
            dictionary! {
                "Subtype" => "Image", "Width" => 2, "Height" => 1, "BitsPerComponent" => 1, "ColorSpace" => "DeviceGray",
                "Decode" => vec![1.into(), 0.into()],
            },
            vec![0b1000_0000],
        );
        assert_eq!(decode_image(&d, &inverted).unwrap().image.as_bytes(), &[0, 255]);

        let cmyk = Stream::new(dictionary! { "Subtype" => "Image", "Width" => 1, "Height" => 1, "BitsPerComponent" => 8, "ColorSpace" => "DeviceCMYK" }, vec![0, 0, 0, 0]);
        assert!(decode_image(&d, &cmyk).is_none());
        let stencil = Stream::new(dictionary! { "Subtype" => "Image", "Width" => 8, "Height" => 1, "BitsPerComponent" => 1, "ImageMask" => true }, vec![0]);
        assert!(decode_image(&d, &stencil).is_none());
    }

    #[test]
    fn sampled_estimate_is_close_to_the_real_size() {
        let mut d = crate::fixtures::photos_document().unwrap();
        let bytes = doc::to_bytes(&mut d).unwrap();
        let tmp = TempDir::new("estimate-test").unwrap();
        for preset in [Preset::Light, Preset::Balanced, Preset::Strong] {
            let opts = Options { preset, ..Default::default() };
            let real = build(&bytes, None, &opts, &Ctx::none(), &tmp.path().join("real.pdf")).unwrap().size;
            let guess = estimate_sampled(&bytes, None, &opts).unwrap();
            let off = (guess as f64 - real as f64).abs() / real as f64;
            println!("{preset:?}: real {real}, sampled estimate {guess}");
            assert!(off < 0.1, "{preset:?}: real {real}, sampled estimate {guess}");
        }
    }

    #[test]
    fn duplicate_streams_are_merged() {
        let mut d = Document::with_version("1.7");
        let a = d.add_object(Stream::new(dictionary! { "Kind" => "X" }, b"same bytes".to_vec()));
        let b = d.add_object(Stream::new(dictionary! { "Kind" => "X" }, b"same bytes".to_vec()));
        let c = d.add_object(Stream::new(dictionary! { "Kind" => "X" }, b"other data".to_vec()));
        let holder = d.add_object(dictionary! { "A" => a, "B" => b, "C" => c });
        d.trailer.set("Root", holder);
        assert_eq!(merge_duplicates(&mut d, false), 1);
        let h = d.get_dictionary(holder).unwrap();
        assert_eq!(h.get(b"A").unwrap(), h.get(b"B").unwrap());
        assert_ne!(h.get(b"A").unwrap(), h.get(b"C").unwrap());
        assert_eq!(d.objects.len(), 3);
    }
}
