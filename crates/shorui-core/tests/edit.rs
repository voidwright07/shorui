#![cfg(feature = "set-edit")]
//! Integration tests for the edit set: watermark, numbers, sign, edit, flatten, redact.
//! Every test runs a tool on generated fixtures and reads the result back.

use image::RgbaImage;
use lopdf::{Document, Object, ObjectId, dictionary};
use shorui_core::helpers::TempDir;
use shorui_core::render::Renderer;
use shorui_core::text::{Line, PageText, Rect4, TextReader};
use shorui_core::tools::{edit, flatten, numbers, redact, sign, watermark};
use shorui_core::{Ctx, Error, doc, fixtures};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The fixtures, written once per test run to a fixed folder under the system temp directory.
fn fx(name: &str) -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("shorui-set-edit-fixtures");
        fixtures::write_all(&dir).expect("fixtures");
        dir
    })
    .join(name)
}

fn out_dir() -> TempDir {
    TempDir::new("edit-test").expect("temp dir")
}

fn ctx() -> Ctx<'static> {
    Ctx::none()
}

fn pages(path: &Path) -> Vec<PageText> {
    TextReader::open_path(path, None).expect("open").all().expect("text")
}

fn line<'a>(page: &'a PageText, needle: &str) -> &'a Line {
    page.lines.iter().find(|l| l.text().contains(needle)).unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{}", page.plain()))
}

fn render(path: &Path, page: usize, scale: f32) -> RgbaImage {
    Renderer::open_path(path, None).expect("open").render(page, scale).expect("render")
}

/// The pixel at a display-space point.
fn pixel(img: &RgbaImage, x: f32, y: f32, scale: f32) -> [u8; 3] {
    let p = img.get_pixel(((x * scale) as u32).min(img.width() - 1), ((y * scale) as u32).min(img.height() - 1)).0;
    [p[0], p[1], p[2]]
}

/// How many pixels inside a display-space rectangle differ between two renders.
fn changed(a: &RgbaImage, b: &RgbaImage, r: Rect4, scale: f32) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    let mut n = 0;
    for y in (r.y0 * scale) as u32..((r.y1 * scale) as u32).min(a.height()) {
        for x in (r.x0 * scale) as u32..((r.x1 * scale) as u32).min(a.width()) {
            if a.get_pixel(x, y) != b.get_pixel(x, y) {
                n += 1;
            }
        }
    }
    n
}

/// Like `changed`, but ignores small differences such as JPEG noise.
fn differs(a: &RgbaImage, b: &RgbaImage, r: Rect4, scale: f32) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    let mut n = 0;
    for y in (r.y0 * scale) as u32..((r.y1 * scale) as u32).min(a.height()) {
        for x in (r.x0 * scale) as u32..((r.x1 * scale) as u32).min(a.width()) {
            let (p, q) = (a.get_pixel(x, y).0, b.get_pixel(x, y).0);
            if (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 90) {
                n += 1;
            }
        }
    }
    n
}

fn strings(obj: &Object, out: &mut Vec<Vec<u8>>) {
    match obj {
        Object::String(bytes, _) => out.push(bytes.clone()),
        Object::Array(items) => items.iter().for_each(|o| strings(o, out)),
        Object::Dictionary(d) => d.iter().for_each(|(_, o)| strings(o, out)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, o)| strings(o, out)),
        _ => {}
    }
}

/// True when `needle` can be found anywhere in the file: in the raw bytes, in any
/// stream after decompression, or in any string object, as plain bytes or UTF-16.
/// Independent of the text extractor.
fn occurs(path: &Path, needle: &str) -> bool {
    let raw = std::fs::read(path).expect("read");
    let doc = Document::load_mem(&raw).expect("load");
    let mut haystacks = vec![raw];
    for obj in doc.objects.values() {
        if let Ok(stream) = obj.as_stream() {
            haystacks.push(stream.get_plain_content().unwrap_or_else(|_| stream.content.clone()));
        }
        strings(obj, &mut haystacks);
    }
    let plain = needle.as_bytes().to_ascii_lowercase();
    let wide: Vec<u8> = needle.to_lowercase().encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
    haystacks.iter().any(|h| {
        let lower = h.to_ascii_lowercase();
        lower.windows(plain.len()).any(|w| w == plain) || lower.windows(wide.len()).any(|w| w == wide)
    })
}

fn image_xobjects(doc: &Document, page: ObjectId) -> usize {
    let Some(Object::Dictionary(res)) = doc::inherited(doc, page, b"Resources") else { return 0 };
    let Ok(xobjects) = res.get(b"XObject").map(|o| doc::deref(doc, o)).and_then(Object::as_dict) else { return 0 };
    xobjects
        .iter()
        .filter(|(_, v)| doc::deref(doc, v).as_stream().is_ok_and(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Image")))
        .count()
}

fn annot_count(doc: &Document) -> usize {
    doc::page_ids(doc).iter().map(|p| doc.get_page_annotations(*p).map(|a| a.len()).unwrap_or(0)).sum()
}

fn has_acroform(doc: &Document) -> bool {
    doc.catalog().unwrap().has(b"AcroForm")
}

fn field_dict<'a>(doc: &'a Document, name: &str) -> &'a lopdf::Dictionary {
    doc.objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .find(|d| d.get(b"T").and_then(Object::as_str).is_ok_and(|t| t == name.as_bytes()))
        .unwrap_or_else(|| panic!("no field {name}"))
}

fn appearance_text(doc: &Document, name: &str) -> String {
    let ap = field_dict(doc, name).get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
    String::from_utf8_lossy(&doc.get_object(ap).unwrap().as_stream().unwrap().get_plain_content().unwrap()).into_owned()
}

fn assert_invalid<T: std::fmt::Debug>(result: shorui_core::Result<T>) {
    match result {
        Err(Error::Invalid(message)) => assert!(!message.is_empty()),
        other => panic!("expected an Invalid error, got {other:?}"),
    }
}

fn nested_form(path: &Path) {
    doc::save(&mut fixtures::choices_document(), path).unwrap();
}

// ---------------------------------------------------------------------------
// Watermark
// ---------------------------------------------------------------------------

#[test]
fn watermark_adds_text_to_every_page_and_keeps_marks() {
    let tmp = out_dir();
    let out = tmp.path().join("wm.pdf");
    let opts = watermark::Options { text: "DRAFT COPY".into(), rotation: 0.0, ..Default::default() };
    let outcome = watermark::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(outcome.pages, 6);
    assert_eq!(outcome.outputs, vec![out.clone()]);
    assert!(outcome.bytes_in > 0 && outcome.bytes_out > 0);
    let text = pages(&out);
    assert_eq!(text.len(), 6);
    for (i, page) in text.iter().enumerate() {
        assert!(page.plain().contains(&format!("MARK-REPORT-P{}", i + 1)));
        let stamp = line(page, "DRAFT COPY");
        // Centred on the page, on its own bounding box.
        let cx = (stamp.rect.x0 + stamp.rect.x1) / 2.0;
        assert!((cx - page.width / 2.0).abs() < 0.02 * page.width, "centre x {cx}");
        let baseline = stamp.rect.y1 - 0.22 * stamp.size;
        let cap_centre = baseline - 0.359 * stamp.size;
        assert!((cap_centre - page.height / 2.0).abs() < 0.03 * stamp.size + 1.0, "centre y {cap_centre}");
        // Auto size keeps the text on the page.
        assert!(stamp.rect.x0 > 0.0 && stamp.rect.x1 < page.width);
    }
}

#[test]
fn watermark_is_upright_on_rotated_pages() {
    let tmp = out_dir();
    let out = tmp.path().join("wm-rotated.pdf");
    let opts = watermark::Options { text: "SAMPLE".into(), rotation: 0.0, size: 40.0, ..Default::default() };
    watermark::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    for (i, page) in pages(&out).iter().enumerate() {
        assert!(page.plain().contains(&format!("MARK-ROTATED-P{}", i + 1)));
        let stamp = line(page, "SAMPLE");
        assert_eq!(stamp.dir, 0, "page {} watermark runs left to right as displayed", i + 1);
        assert!((stamp.size - 40.0).abs() < 0.5);
        let cx = (stamp.rect.x0 + stamp.rect.x1) / 2.0;
        let cy = (stamp.rect.y0 + stamp.rect.y1) / 2.0;
        assert!((cx - page.width / 2.0).abs() < 3.0 && (cy - page.height / 2.0).abs() < 6.0, "page {} centre {cx},{cy} of {}x{}", i + 1, page.width, page.height);
    }
}

#[test]
fn watermark_default_is_diagonal_and_leaves_text_alone() {
    let tmp = out_dir();
    let out = tmp.path().join("wm-default.pdf");
    let before = pages(&fx("rotated.pdf"));
    watermark::run(&[fx("rotated.pdf")], &out, &watermark::Options::default(), &ctx()).unwrap();
    let after = pages(&out);
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        assert!(a.plain().contains(&format!("MARK-ROTATED-P{}", i + 1)));
        let count = |p: &PageText| p.plain().chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(count(a), count(b) + "CONFIDENTIAL".len(), "page {}", i + 1);
    }
    // Grey ink at 25% over white: the centre region is no longer pure white.
    let (plain, stamped) = (render(&fx("rotated.pdf"), 1, 1.0), render(&out, 1, 1.0));
    assert!(changed(&plain, &stamped, Rect4::new(200.0, 150.0, 640.0, 450.0), 1.0) > 2000);
}

#[test]
fn watermark_page_range_corners_tiles_and_images() {
    let tmp = out_dir();
    let out = tmp.path().join("wm-range.pdf");
    let opts = watermark::Options { text: "ONLY TWO".into(), rotation: 0.0, position: watermark::Position::TopRight, pages: "2".into(), ..Default::default() };
    watermark::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    let text = pages(&out);
    assert!(!text[0].plain().contains("ONLY TWO") && !text[2].plain().contains("ONLY TWO"));
    let stamp = line(&text[1], "ONLY TWO");
    assert!((stamp.rect.x1 - (text[1].width - 36.0)).abs() < 2.0, "right edge {}", stamp.rect.x1);
    assert!(stamp.rect.y0 < 60.0 && stamp.rect.y0 > 20.0, "top {}", stamp.rect.y0);

    let tiled = tmp.path().join("wm-tile.pdf");
    let opts = watermark::Options { text: "TILE".into(), rotation: 0.0, position: watermark::Position::Tile, under: true, ..Default::default() };
    let outcome = watermark::run(&[fx("letter.pdf")], &tiled, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("behind")));
    let page = &pages(&tiled)[0];
    assert!(page.plain().matches("TILE").count() > 8, "{}", page.plain());
    assert!(page.plain().contains("MARK-LETTER-P1"));

    let pictured = tmp.path().join("wm-image.pdf");
    let opts = watermark::Options { image: Some(fx("images/diagram.png")), rotation: 0.0, opacity: 0.5, ..Default::default() };
    watermark::run(&[fx("report.pdf")], &pictured, &opts, &ctx()).unwrap();
    let d = doc::load(&pictured, None).unwrap();
    for id in doc::page_ids(&d) {
        assert_eq!(image_xobjects(&d, id), 1);
    }
    assert!(pages(&pictured)[3].plain().contains("MARK-REPORT-P4"));
    // The circle of the diagram sits at the centre of the page, blue mixed with white.
    let p = pixel(&render(&pictured, 0, 1.0), 297.0, 421.0, 1.0);
    assert!(p[2] as i32 > p[0] as i32 + 40, "centre pixel {p:?}");
}

#[test]
fn watermark_rejects_bad_input() {
    let tmp = out_dir();
    let out = tmp.path().join("x.pdf");
    assert_invalid(watermark::run(&[fx("report.pdf")], &out, &watermark::Options { pages: "9".into(), ..Default::default() }, &ctx()));
    assert_invalid(watermark::run(&[fx("report.pdf")], &out, &watermark::Options { text: "  ".into(), ..Default::default() }, &ctx()));
    assert_invalid(watermark::run(&[], &out, &watermark::Options::default(), &ctx()));
    assert_invalid(watermark::run(&[fx("report.pdf")], &out, &watermark::Options { image: Some(fx("sample.html")), ..Default::default() }, &ctx()));
    assert!(matches!(watermark::run(&[fx("broken.pdf")], &out, &watermark::Options::default(), &ctx()), Err(Error::Damaged(_))));
    assert!(!out.exists());
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

#[test]
fn numbers_print_page_n_of_total() {
    let tmp = out_dir();
    let out = tmp.path().join("numbered.pdf");
    let opts = numbers::Options { format: "Sheet {n} of {total}".into(), position: numbers::Position::BottomRight, ..Default::default() };
    numbers::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    let text = pages(&out);
    for (i, page) in text.iter().enumerate() {
        let label = format!("Sheet {} of 6", i + 1);
        assert_eq!(numbers::preview_label(&opts, i + 1, 6, "report"), label);
        let l = line(page, &label);
        assert!(page.plain().contains(&format!("MARK-REPORT-P{}", i + 1)));
        // Right-aligned to the margin, feet of the letters one margin above the bottom edge.
        assert!((l.rect.x1 - (page.width - 28.0)).abs() < 1.5, "right edge {}", l.rect.x1);
        let baseline = l.rect.y1 - 0.22 * l.size;
        assert!((baseline - (page.height - 28.0)).abs() < 1.0, "baseline {baseline}");
        assert!((l.size - 10.0).abs() < 0.3);
    }
    // The default format is the bare number, bottom centre.
    let plain = tmp.path().join("plain.pdf");
    numbers::run(&[fx("appendix.pdf")], &plain, &numbers::Options::default(), &ctx()).unwrap();
    let text = pages(&plain);
    let l = text[2].lines.last().unwrap();
    assert_eq!(l.text(), "3");
    assert!(((l.rect.x0 + l.rect.x1) / 2.0 - text[2].width / 2.0).abs() < 1.5);
}

#[test]
fn numbers_bates_and_rotated_pages() {
    let tmp = out_dir();
    let out = tmp.path().join("bates.pdf");
    let opts = numbers::Options { mode: numbers::Mode::Bates, bates_prefix: "ABC".into(), position: numbers::Position::TopRight, ..Default::default() };
    let outcome = numbers::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("ABC000001") && n.contains("ABC000006")), "{:?}", outcome.notes);
    let text = pages(&out);
    assert!(text[2].plain().contains("ABC000003"));
    assert!(!text[2].plain().contains("ABC000002"));
    assert_eq!(numbers::preview_label(&opts, 3, 6, "report"), "ABC000003");

    let rotated = tmp.path().join("bates-rotated.pdf");
    let opts = numbers::Options { mode: numbers::Mode::Bates, bates_prefix: "R-".into(), bates_suffix: "-X".into(), bates_digits: 4, bates_start: 98, position: numbers::Position::TopRight, ..Default::default() };
    numbers::run(&[fx("rotated.pdf")], &rotated, &opts, &ctx()).unwrap();
    for (i, page) in pages(&rotated).iter().enumerate() {
        let l = line(page, &format!("R-{:04}-X", 98 + i));
        assert_eq!(l.dir, 0, "page {}", i + 1);
        assert!((l.rect.x1 - (page.width - 28.0)).abs() < 1.5, "page {} right edge {} of {}", i + 1, l.rect.x1, page.width);
        // Tops of the letters one margin below the top edge.
        let top = l.rect.y1 - 0.22 * l.size - 0.718 * l.size;
        assert!((top - 28.0).abs() < 1.0, "page {} top {top}", i + 1);
        assert!(page.plain().contains(&format!("MARK-ROTATED-P{}", i + 1)));
    }
}

#[test]
fn numbers_skip_first_ranges_and_headers() {
    let tmp = out_dir();
    let out = tmp.path().join("hf.pdf");
    let opts = numbers::Options {
        mode: numbers::Mode::HeaderFooter,
        header_left: "{file}".into(),
        header_right: "{date}".into(),
        footer_center: "No. {n} / {total}".into(),
        pages: "2-".into(),
        skip_first: true,
        date_text: Some("2026-01-05".into()),
        ..Default::default()
    };
    numbers::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    let text = pages(&out);
    // Page 1 is outside the range; page 2 is the first numbered page and is skipped but counted.
    for skipped in [0, 1] {
        assert!(!text[skipped].plain().contains("No. "), "{}", text[skipped].plain());
        assert!(!text[skipped].plain().contains("2026-01-05"));
    }
    assert!(text[2].plain().contains("No. 2 / 5"));
    assert!(text[5].plain().contains("No. 5 / 5"));
    let header = line(&text[2], "2026-01-05");
    assert!(header.text().contains("report"), "{}", header.text());
    assert!(header.rect.y0 < 40.0);
    assert_eq!(numbers::preview_label(&opts, 3, 6, "report"), "report | 2026-01-05 | No. 2 / 5");
    assert_eq!(numbers::preview_label(&opts, 2, 6, "report"), "");
    assert_eq!(numbers::preview_labels(&opts, 3, 6, "report").len(), 3);

    // {date} defaults to today.
    let dated = tmp.path().join("dated.pdf");
    numbers::run(&[fx("letter.pdf")], &dated, &numbers::Options { format: "Printed {date}".into(), ..Default::default() }, &ctx()).unwrap();
    assert!(pages(&dated)[0].plain().contains(&format!("Printed {}", edit::today())));

    assert_invalid(numbers::run(&[fx("report.pdf")], &out, &numbers::Options { pages: "0".into(), ..Default::default() }, &ctx()));
    assert_invalid(numbers::run(&[fx("report.pdf")], &out, &numbers::Options { mode: numbers::Mode::HeaderFooter, ..Default::default() }, &ctx()));
    assert_invalid(numbers::run(&[fx("report.pdf")], &out, &numbers::Options { size: 0.0, ..Default::default() }, &ctx()));
}

// ---------------------------------------------------------------------------
// Sign
// ---------------------------------------------------------------------------

#[test]
fn sign_places_an_image_and_the_date() {
    let tmp = out_dir();
    let out = tmp.path().join("signed.pdf");
    let opts = sign::Options { image: Some(fx("images/signature.png")), page: 1, x: 60.0, y: 255.0, width: 170.0, add_date: true, ..Default::default() };
    assert_eq!(sign::placed_size(&opts).unwrap(), (170.0, 51.0));
    let outcome = sign::run(&[fx("form.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("not a cryptographic")), "{:?}", outcome.notes);

    let before = doc::load(&fx("form.pdf"), None).unwrap();
    let after = doc::load(&out, None).unwrap();
    assert_eq!(image_xobjects(&before, doc::page_ids(&before)[0]), 0);
    assert_eq!(image_xobjects(&after, doc::page_ids(&after)[0]), 1);
    assert_eq!(annot_count(&after), 4, "the form is untouched");

    let page = &pages(&out)[0];
    assert!(page.plain().contains("MARK-FORM-P1"));
    let date = line(page, &edit::today());
    assert!((date.rect.x0 - 60.0).abs() < 1.0 && date.rect.y0 > 255.0 + 51.0 - 2.0 && date.rect.y0 < 255.0 + 51.0 + 12.0, "date at {:?}", date.rect);

    // Ink appears inside the signature box and nowhere else above the date.
    let (a, b) = (render(&fx("form.pdf"), 0, 2.0), render(&out, 0, 2.0));
    assert!(changed(&a, &b, Rect4::new(60.0, 255.0, 230.0, 306.0), 2.0) > 300);
    assert_eq!(changed(&a, &b, Rect4::new(0.0, 0.0, 595.0, 250.0), 2.0), 0);
    assert_eq!(changed(&a, &b, Rect4::new(240.0, 250.0, 595.0, 842.0), 2.0), 0);
    // The signature colour comes through, and its transparent background does not cover the page.
    let ink = (60..230).flat_map(|x| (255..306).map(move |y| (x, y))).map(|(x, y)| pixel(&b, x as f32, y as f32, 2.0)).filter(|p| p[2] as i32 > p[0] as i32 + 40 && p[0] < 90).count();
    assert!(ink > 100, "blue pixels: {ink}");
}

#[test]
fn sign_typed_name_on_every_page_upright() {
    let tmp = out_dir();
    let out = tmp.path().join("typed.pdf");
    let opts = sign::Options { typed: Some("Mara Lindqvist".into()), all_pages: true, x: 300.0, y: 100.0, add_date: true, date_text: Some("2 October 2026".into()), ..Default::default() };
    sign::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    let (w, h) = sign::placed_size(&opts).unwrap();
    for (i, page) in pages(&out).iter().enumerate() {
        let name = line(page, "Mara Lindqvist");
        assert_eq!(name.dir, 0, "page {}", i + 1);
        assert!((name.rect.x0 - 300.0).abs() < 1.0, "page {} x {}", i + 1, name.rect.x0);
        assert!(name.rect.y0 >= 95.0 && name.rect.y1 <= 100.0 + h + 8.0, "page {} at {:?}", i + 1, name.rect);
        assert!(name.rect.x1 <= 300.0 + w + 1.0);
        let date = line(page, "2 October 2026");
        assert!(date.rect.y0 > name.rect.y0 && (date.rect.x0 - 300.0).abs() < 1.0);
        assert!(page.plain().contains(&format!("MARK-ROTATED-P{}", i + 1)));
    }
    // Dark blue ink by default.
    let img = render(&out, 1, 2.0);
    let blue = (300..470).flat_map(|x| (100..135).map(move |y| (x, y))).map(|(x, y)| pixel(&img, x as f32, y as f32, 2.0)).filter(|p| p[2] as i32 > p[0] as i32 + 30).count();
    assert!(blue > 100, "blue pixels: {blue}");

    // Only the chosen page when all_pages is off.
    let one = tmp.path().join("one.pdf");
    sign::run(&[fx("report.pdf")], &one, &sign::Options { typed: Some("J. Okafor".into()), page: 4, x: 100.0, y: 500.0, ..Default::default() }, &ctx()).unwrap();
    let text = pages(&one);
    assert!(text[3].plain().contains("J. Okafor"));
    assert_eq!(text.iter().filter(|p| p.plain().contains("J. Okafor")).count(), 1);
}

#[test]
fn sign_rejects_bad_input() {
    let tmp = out_dir();
    let out = tmp.path().join("x.pdf");
    assert_invalid(sign::run(&[fx("report.pdf")], &out, &sign::Options::default(), &ctx()));
    assert_invalid(sign::run(&[fx("report.pdf")], &out, &sign::Options { typed: Some("A".into()), page: 9, ..Default::default() }, &ctx()));
    assert_invalid(sign::run(&[fx("report.pdf")], &out, &sign::Options { typed: Some("A".into()), width: 0.0, ..Default::default() }, &ctx()));
    assert!(matches!(sign::run(&[fx("report.pdf")], &out, &sign::Options { image: Some(tmp.path().join("missing.png")), ..Default::default() }, &ctx()), Err(Error::Read { .. })));
    assert!(!out.exists());
}

// ---------------------------------------------------------------------------
// Edit & Fill
// ---------------------------------------------------------------------------

fn fill_form(out: &Path, flatten: bool) -> shorui_core::Outcome {
    let fields: BTreeMap<String, String> = [("full_name", "Mara Lindqvist"), ("department", "Logistics (North)"), ("agree", "yes")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    edit::run(&[fx("form.pdf")], out, &edit::Options { fields, flatten, ..Default::default() }, &ctx()).unwrap()
}

#[test]
fn edit_lists_and_fills_form_fields() {
    let tmp = out_dir();
    let before = edit::list_fields(&fx("form.pdf"), None).unwrap();
    assert_eq!(before.iter().map(|f| (f.name.as_str(), f.kind.as_str(), f.value.as_str(), f.page)).collect::<Vec<_>>(), [("full_name", "text", "", 1), ("department", "text", "", 1), ("agree", "checkbox", "false", 1)]);
    // Field rectangles are in display space: full_name sits at y 676..696 from the bottom of an 842 pt page.
    assert_eq!(before[0].rect, Rect4::new(56.0, 146.0, 356.0, 166.0));
    assert_eq!(before[2].options, ["Yes"]);
    assert_eq!(serde_json::to_value(&before[2]).unwrap()["kind"], "checkbox");

    let out = tmp.path().join("filled.pdf");
    let outcome = fill_form(&out, false);
    assert!(outcome.notes.iter().any(|n| n.contains("Filled 3 form fields")), "{:?}", outcome.notes);
    let after = edit::list_fields(&out, None).unwrap();
    assert_eq!(after.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["Mara Lindqvist", "Logistics (North)", "true"]);

    let d = doc::load(&out, None).unwrap();
    // The value is drawn by an appearance stream, clipped to the field, in Helvetica at the size of /DA.
    let ap = appearance_text(&d, "full_name");
    assert!(ap.contains("/Tx BMC") && ap.contains("(Mara Lindqvist) Tj") && ap.contains(" 11 Tf") && ap.contains("re W n"), "{ap}");
    assert!(ap.contains("re S"), "the border the old appearance drew is kept: {ap}");
    assert!(appearance_text(&d, "department").contains("(Logistics \\(North\\)) Tj"));
    let agree = field_dict(&d, "agree");
    assert_eq!(agree.get(b"V").unwrap().as_name().unwrap(), b"Yes");
    assert_eq!(agree.get(b"AS").unwrap().as_name().unwrap(), b"Yes");
    let acro = d.catalog().unwrap().get(b"AcroForm").unwrap().as_reference().unwrap();
    assert!(d.get_dictionary(acro).unwrap().get(b"NeedAppearances").unwrap().as_bool().unwrap());
    assert_eq!(annot_count(&d), 4);

    // The value shows when the page is rendered: pixels inside the field changed, nothing else did.
    let (a, b) = (render(&fx("form.pdf"), 0, 2.0), render(&out, 0, 2.0));
    assert!(changed(&a, &b, before[0].rect, 2.0) > 200);
    assert!(changed(&a, &b, before[1].rect, 2.0) > 200);
    assert_eq!(changed(&a, &b, Rect4::new(0.0, 260.0, 595.0, 842.0), 2.0), 0);

    // Filling again replaces the value instead of drawing over it, and "no" clears the box.
    let again = tmp.path().join("again.pdf");
    let fields: BTreeMap<String, String> = [("full_name", "Jonas Okafor"), ("agree", "no")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    edit::run(&[out.clone()], &again, &edit::Options { fields, ..Default::default() }, &ctx()).unwrap();
    let d = doc::load(&again, None).unwrap();
    let ap = appearance_text(&d, "full_name");
    assert!(ap.contains("(Jonas Okafor) Tj") && !ap.contains("Mara"), "{ap}");
    assert_eq!(ap.matches("/Tx BMC").count(), 1);
    assert_eq!(field_dict(&d, "agree").get(b"AS").unwrap().as_name().unwrap(), b"Off");
    assert_eq!(edit::list_fields(&again, None).unwrap()[2].value, "false");
}

#[test]
fn edit_handles_nested_fields_radios_and_choices() {
    let tmp = out_dir();
    let form = tmp.path().join("nested.pdf");
    nested_form(&form);
    let fields = edit::list_fields(&form, None).unwrap();
    assert_eq!(fields.iter().map(|f| (f.name.as_str(), f.kind.as_str())).collect::<Vec<_>>(), [("person.first", "text"), ("person.last", "text"), ("colour", "radio"), ("size", "choice")]);
    assert_eq!(fields[1].value, "Okafor");
    assert_eq!(fields[2].options, ["red", "blue"]);
    assert_eq!(fields[2].value, "");
    assert_eq!(fields[3].options, ["s", "l"]);
    assert_eq!(fields[0].rect, Rect4::new(56.0, 122.0, 256.0, 142.0));

    let out = tmp.path().join("nested-filled.pdf");
    let values: BTreeMap<String, String> = [("person.first", "Jonas"), ("colour", "blue"), ("size", "Large")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    edit::run(&[form.clone()], &out, &edit::Options { fields: values, ..Default::default() }, &ctx()).unwrap();
    let after = edit::list_fields(&out, None).unwrap();
    assert_eq!(after.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["Jonas", "Okafor", "blue", "l"]);
    let d = doc::load(&out, None).unwrap();
    // Inherited /FT and /DA: size 0 means fit, colour blue.
    let ap = appearance_text(&d, "first");
    assert!(ap.contains("(Jonas) Tj") && ap.contains("0 0 1 rg") && ap.contains(" 12 Tf"), "{ap}");
    assert!(appearance_text(&d, "size").contains("(Large) Tj"));
    let states: Vec<Vec<u8>> = d.objects.values().filter_map(|o| o.as_dict().ok()).filter(|w| w.has(b"AS")).map(|w| w.get(b"AS").unwrap().as_name().unwrap().to_vec()).collect();
    assert_eq!(states.iter().filter(|s| s.as_slice() == b"blue").count(), 1);
    assert_eq!(states.iter().filter(|s| s.as_slice() == b"Off").count(), 1);

    let bad = |name: &str, value: &str| {
        let values: BTreeMap<String, String> = [(name.to_string(), value.to_string())].into_iter().collect();
        edit::run(&[form.clone()], &tmp.path().join("bad.pdf"), &edit::Options { fields: values, ..Default::default() }, &ctx())
    };
    assert_invalid(bad("person.middle", "x"));
    assert_invalid(bad("colour", "green"));
    assert_invalid(bad("size", "Huge"));
    assert_invalid(edit::run(&[fx("report.pdf")], &tmp.path().join("bad.pdf"), &edit::Options { fields: [("a".to_string(), "b".to_string())].into_iter().collect(), ..Default::default() }, &ctx()));
    assert_invalid(edit::run(&[fx("report.pdf")], &tmp.path().join("bad.pdf"), &edit::Options::default(), &ctx()));
}

#[test]
fn edit_draws_items_where_asked() {
    let tmp = out_dir();
    let out = tmp.path().join("items.pdf");
    let item = |kind, x, y, w, h| edit::Item { kind, page: 1, x, y, w, h, ..Default::default() };
    let items = vec![
        edit::Item { text: "Checked by J. Okafor\nsecond line".into(), size: 12.0, color: Some([160, 0, 0]), ..item(edit::ItemKind::Text, 360.0, 250.0, 0.0, 0.0) },
        item(edit::ItemKind::Check, 360.0, 300.0, 20.0, 20.0),
        edit::Item { color: Some([200, 0, 0]), ..item(edit::ItemKind::Cross, 390.0, 300.0, 20.0, 20.0) },
        item(edit::ItemKind::Date, 420.0, 304.0, 0.0, 0.0),
        item(edit::ItemKind::Highlight, 56.0, 90.0, 120.0, 14.0),
        item(edit::ItemKind::Whiteout, 80.0, 228.0, 60.0, 14.0),
        edit::Item { image: Some(fx("images/diagram.png")), ..item(edit::ItemKind::Image, 360.0, 340.0, 150.0, 0.0) },
    ];
    let outcome = edit::run(&[fx("form.pdf")], &out, &edit::Options { items, ..Default::default() }, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("Redact")), "{:?}", outcome.notes);

    let page = &pages(&out)[0];
    let first = line(page, "Checked by J. Okafor");
    // Top-left at (360, 250): the tops of the capitals touch y, the text starts at x.
    assert!((first.rect.x0 - 360.0).abs() < 0.6, "{:?}", first.rect);
    let cap_top = first.rect.y1 - 0.22 * first.size - 0.718 * first.size;
    assert!((cap_top - 250.0).abs() < 0.6, "cap top {cap_top}");
    assert!((first.size - 12.0).abs() < 0.2);
    let second = line(page, "second line");
    assert!((second.rect.y0 - first.rect.y0 - 14.4).abs() < 0.5);
    assert!(page.plain().contains(&edit::today()));
    assert!(page.plain().contains("MARK-FORM-P1"), "a highlight does not hide the text under it");

    let (a, b) = (render(&fx("form.pdf"), 0, 2.0), render(&out, 0, 2.0));
    // Highlight: white becomes yellow, the black text under it stays dark.
    let yellow = pixel(&b, 170.0, 97.0, 2.0);
    assert!(yellow[0] > 220 && yellow[1] > 200 && yellow[2] < 200, "highlight {yellow:?}");
    let dark = (56..150).flat_map(|x| (92..103).map(move |y| (x, y))).map(|(x, y)| pixel(&b, x as f32, y as f32, 2.0)).filter(|p| p[0] < 110).count();
    assert!(dark > 30, "text under the highlight: {dark}");
    // Whiteout: the start of "I have read..." is gone from the picture.
    let white = (81..139).flat_map(|x| (229..241).map(move |y| (x, y))).all(|(x, y)| pixel(&b, x as f32, y as f32, 2.0) == [255, 255, 255]);
    assert!(white);
    assert!(changed(&a, &b, Rect4::new(80.0, 228.0, 140.0, 242.0), 2.0) > 50);
    // Check and cross are strokes inside their boxes, in their colours.
    assert!(changed(&a, &b, Rect4::new(360.0, 300.0, 380.0, 320.0), 2.0) > 40);
    let red = (390..410).flat_map(|x| (300..320).map(move |y| (x, y))).map(|(x, y)| pixel(&b, x as f32, y as f32, 2.0)).filter(|p| p[0] > 150 && p[1] < 90).count();
    assert!(red > 20, "red cross pixels: {red}");
    assert_eq!(changed(&a, &b, Rect4::new(355.0, 322.0, 420.0, 338.0), 2.0), 0, "nothing spills below the boxes");
    // The image fills a 150 x 100 box (its own shape) with its top-left at (360, 340).
    let centre = pixel(&b, 435.0, 390.0, 2.0);
    assert!(centre[2] > 150 && centre[0] < 100, "image centre {centre:?}");
    assert_eq!(changed(&a, &b, Rect4::new(355.0, 442.0, 520.0, 500.0), 2.0), 0);
    let d = doc::load(&out, None).unwrap();
    assert_eq!(image_xobjects(&d, doc::page_ids(&d)[0]), 1);

    // On a rotated page the text still lands where asked, upright.
    let rotated = tmp.path().join("rotated-items.pdf");
    let items = (1..=4).map(|page| edit::Item { page, text: "Approved".into(), x: 100.0, y: 200.0, ..Default::default() }).collect();
    edit::run(&[fx("rotated.pdf")], &rotated, &edit::Options { items, ..Default::default() }, &ctx()).unwrap();
    for (i, page) in pages(&rotated).iter().enumerate() {
        let l = line(page, "Approved");
        assert_eq!(l.dir, 0);
        assert!((l.rect.x0 - 100.0).abs() < 0.6 && (l.rect.y1 - 0.22 * l.size - 0.718 * l.size - 200.0).abs() < 0.6, "page {}: {:?}", i + 1, l.rect);
    }

    assert_invalid(edit::run(&[fx("form.pdf")], &tmp.path().join("bad.pdf"), &edit::Options { items: vec![edit::Item { page: 7, text: "x".into(), ..Default::default() }], ..Default::default() }, &ctx()));
    assert_invalid(edit::run(&[fx("form.pdf")], &tmp.path().join("bad.pdf"), &edit::Options { items: vec![item(edit::ItemKind::Highlight, 1.0, 1.0, 0.0, 0.0)], ..Default::default() }, &ctx()));
    assert_invalid(edit::run(&[fx("form.pdf")], &tmp.path().join("bad.pdf"), &edit::Options { items: vec![item(edit::ItemKind::Image, 1.0, 1.0, 10.0, 10.0)], ..Default::default() }, &ctx()));
}

#[test]
fn edit_draws_dots_circles_and_lines() {
    let tmp = out_dir();
    let out = tmp.path().join("marks.pdf");
    let item = |kind, x, y, w, h| edit::Item { kind, page: 1, x, y, w, h, ..Default::default() };
    let items = vec![
        item(edit::ItemKind::Dot, 400.0, 300.0, 10.0, 10.0),
        edit::Item { color: Some([0, 60, 200]), ..item(edit::ItemKind::Circle, 400.0, 340.0, 60.0, 24.0) },
        item(edit::ItemKind::Line, 400.0, 400.0, 80.0, 10.0),
        item(edit::ItemKind::Line, 500.0, 300.0, 10.0, 80.0),
    ];
    edit::run(&[fx("form.pdf")], &out, &edit::Options { items, ..Default::default() }, &ctx()).unwrap();
    let (a, b) = (render(&fx("form.pdf"), 0, 2.0), render(&out, 0, 2.0));
    // The dot is solid at its centre and stays inside its box.
    assert!(pixel(&b, 405.0, 305.0, 2.0)[0] < 60);
    assert_eq!(changed(&a, &b, Rect4::new(380.0, 290.0, 399.0, 320.0), 2.0), 0);
    assert_eq!(changed(&a, &b, Rect4::new(411.0, 290.0, 430.0, 320.0), 2.0), 0);
    // The circle is an outline: blue on its edge, untouched in the middle and outside the box.
    let edge = pixel(&b, 400.6, 352.0, 2.0);
    assert!(edge[2] > 120 && edge[0] < 150, "circle edge {edge:?}");
    assert_eq!(pixel(&b, 430.0, 352.0, 2.0), [255, 255, 255]);
    assert_eq!(changed(&a, &b, Rect4::new(380.0, 330.0, 399.0, 372.0), 2.0), 0);
    assert_eq!(changed(&a, &b, Rect4::new(400.0, 365.0, 460.0, 372.0), 2.0), 0);
    // A wide box gives a level line through its middle, a tall box an upright one.
    assert!(pixel(&b, 440.0, 405.0, 2.0)[0] < 120);
    assert_eq!(pixel(&b, 440.0, 401.0, 2.0), [255, 255, 255]);
    assert!(pixel(&b, 505.0, 340.0, 2.0)[0] < 120);
    assert_eq!(pixel(&b, 501.0, 340.0, 2.0), [255, 255, 255]);

    assert_invalid(edit::run(&[fx("form.pdf")], &tmp.path().join("bad.pdf"), &edit::Options { items: vec![item(edit::ItemKind::Circle, 1.0, 1.0, 0.0, 0.0)], ..Default::default() }, &ctx()));
    assert_invalid(edit::run(&[fx("form.pdf")], &tmp.path().join("bad.pdf"), &edit::Options { items: vec![item(edit::ItemKind::Line, 1.0, 1.0, 0.0, 0.0)], ..Default::default() }, &ctx()));
}

#[test]
fn edit_lists_every_button_of_a_radio_group() {
    let tmp = out_dir();
    let form = tmp.path().join("nested.pdf");
    nested_form(&form);
    let fields = edit::list_fields(&form, None).unwrap();
    let colour = fields.iter().find(|f| f.name == "colour").unwrap();
    assert_eq!(colour.widgets.iter().map(|w| w.state.as_str()).collect::<Vec<_>>(), ["red", "blue"]);
    assert!(colour.widgets.iter().all(|w| w.page == 1 && w.rect.width() > 0.0));
    assert_ne!(colour.widgets[0].rect, colour.widgets[1].rect);
    assert_eq!(colour.rect, colour.widgets[0].rect);
    let first = fields.iter().find(|f| f.name == "person.first").unwrap();
    assert_eq!(first.widgets.len(), 1);
}

#[test]
fn edit_marks_over_a_field_end_up_on_top_of_it() {
    let tmp = out_dir();
    let out = tmp.path().join("over.pdf");
    let name = edit::list_fields(&fx("form.pdf"), None).unwrap().into_iter().find(|f| f.name == "full_name").unwrap().rect;
    let (mx, my) = ((name.x0 + name.x1) / 2.0, (name.y0 + name.y1) / 2.0);
    // A cross in the middle of the name field, and a tick well away from every field.
    let items = vec![
        edit::Item { kind: edit::ItemKind::Cross, page: 1, x: mx - 6.0, y: my - 6.0, w: 12.0, h: 12.0, ..Default::default() },
        edit::Item { kind: edit::ItemKind::Check, page: 1, x: 400.0, y: 500.0, w: 12.0, h: 12.0, ..Default::default() },
    ];
    let outcome = edit::run(&[fx("form.pdf")], &out, &edit::Options { items, ..Default::default() }, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("1 form field had something added on top")), "{:?}", outcome.notes);

    // The cross shows: there is ink at the centre of the field.
    let b = render(&out, 0, 3.0);
    let centre = pixel(&b, mx, my, 3.0);
    assert!(centre[0] < 120, "the cross is hidden under the field: {centre:?}");
    // The covered field is part of the page now; the others can still be filled.
    let left: Vec<String> = edit::list_fields(&out, None).unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(left, ["department", "agree"]);
    let again = tmp.path().join("again.pdf");
    let values: BTreeMap<String, String> = [("department".to_string(), "Stores".to_string())].into_iter().collect();
    edit::run(&[out.clone()], &again, &edit::Options { fields: values, ..Default::default() }, &ctx()).unwrap();
    assert_eq!(edit::list_fields(&again, None).unwrap()[0].value, "Stores");

    // Nothing is flattened when the marks keep clear of the fields.
    let clear = tmp.path().join("clear.pdf");
    let items = vec![edit::Item { kind: edit::ItemKind::Check, page: 1, x: 400.0, y: 500.0, w: 12.0, h: 12.0, ..Default::default() }];
    edit::run(&[fx("form.pdf")], &clear, &edit::Options { items, ..Default::default() }, &ctx()).unwrap();
    assert_eq!(edit::list_fields(&clear, None).unwrap().len(), 3);
}

#[test]
fn edit_fills_one_place_of_a_shared_field_on_its_own() {
    let tmp = out_dir();
    let out = tmp.path().join("places.pdf");
    let before = edit::list_fields(&fx("shared.pdf"), None).unwrap();
    let date = before.iter().find(|f| f.name == "Date").unwrap();
    assert_eq!(date.widgets.iter().map(|w| w.index).collect::<Vec<_>>(), [0, 1, 2]);

    let place = |field: &str, widget, value: &str| edit::PlaceValue { field: field.into(), widget, value: value.into() };
    let opts = edit::Options { places: vec![place("Date", 1, "2026-10-03"), place("Tick", 0, "X")], ..Default::default() };
    edit::run(&[fx("shared.pdf")], &out, &opts, &ctx()).unwrap();

    let after = edit::list_fields(&out, None).unwrap();
    let by_name = |n: &str| after.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("no field {n}: {:?}", after.iter().map(|f| &f.name).collect::<Vec<_>>()));
    // The filled places are fields of their own, where they were.
    assert_eq!(by_name("Date 2").value, "2026-10-03");
    assert_eq!(by_name("Date 2").widgets[0].rect, date.widgets[1].rect);
    assert_eq!(by_name("Tick 1").value, "X");
    // The other places still share the original, empty field.
    assert_eq!(by_name("Date").value, "");
    assert_eq!(by_name("Date").widgets.len(), 2);
    assert_eq!(by_name("Tick").widgets.len(), 1);
    // The value is drawn in its own place only.
    let d = doc::load(&out, None).unwrap();
    let drawn = d
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|w| w.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Widget"))
        .filter_map(|w| w.get(b"AP").ok()?.as_dict().ok()?.get(b"N").ok()?.as_reference().ok())
        .filter_map(|id| d.get_object(id).ok()?.as_stream().ok())
        .filter(|s| String::from_utf8_lossy(&s.decompressed_content().unwrap_or_else(|_| s.content.clone())).contains("2026-10-03"))
        .count();
    assert_eq!(drawn, 1, "only the filled place shows the date");

    // A place number that does not exist is refused.
    let bad = edit::Options { places: vec![place("Date", 9, "x")], ..Default::default() };
    assert_invalid(edit::run(&[fx("shared.pdf")], &tmp.path().join("bad.pdf"), &bad, &ctx()));
}

#[test]
fn edit_options_come_from_json() {
    let opts: edit::Options = serde_json::from_value(serde_json::json!({
        "fields": { "full_name": "A" },
        "items": [{ "kind": "whiteout", "page": 2, "x": 1, "y": 2, "w": 3, "h": 4 }, { "kind": "date", "color": [1, 2, 3] }],
        "flatten": true
    }))
    .unwrap();
    assert_eq!(opts.items[0].kind, edit::ItemKind::Whiteout);
    assert_eq!((opts.items[1].page, opts.items[1].size, opts.items[1].color), (1, 11.0, Some([1, 2, 3])));
    let w: watermark::Options = serde_json::from_value(serde_json::json!({ "position": "top-left" })).unwrap();
    assert_eq!((w.position, w.text.as_str(), w.opacity, w.rotation, w.color, w.scale), (watermark::Position::TopLeft, "CONFIDENTIAL", 0.25, 45.0, [128, 128, 128], 0.4));
    let n: numbers::Options = serde_json::from_value(serde_json::json!({ "mode": "header-footer", "position": "top-center", "font": "times" })).unwrap();
    assert_eq!((n.mode, n.position, n.start, n.bates_digits, n.size, n.margin), (numbers::Mode::HeaderFooter, numbers::Position::TopCenter, 1, 6, 10.0, 28.0));
    let r: redact::Options = serde_json::from_value(serde_json::json!({ "areas": [{ "page": 2, "rect": { "x0": 1, "y0": 2, "x1": 3, "y1": 4 } }], "search": [{ "text": "x", "whole_words": true }] })).unwrap();
    assert_eq!((r.dpi, r.color, r.padding, r.strip_metadata, r.jpeg_quality, r.areas[0].page), (200, [0, 0, 0], 1.0, true, 85, 2));
    let s: sign::Options = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!((s.page, s.width, s.color, s.date_size), (1, 170.0, [27, 42, 107], 10.0));
    let f: flatten::Options = serde_json::from_value(serde_json::json!({ "keep_links": false })).unwrap();
    assert!(f.forms && f.annotations && !f.keep_links);
}

// ---------------------------------------------------------------------------
// Flatten
// ---------------------------------------------------------------------------

#[test]
fn flatten_turns_a_filled_form_into_page_content() {
    let tmp = out_dir();
    let filled = tmp.path().join("filled.pdf");
    fill_form(&filled, false);
    let rects = edit::list_fields(&filled, None).unwrap();
    assert!(!pages(&filled)[0].plain().contains("Mara Lindqvist"), "before flattening the value is not page content");
    assert_eq!(serde_json::to_value(flatten::summary(&filled, None).unwrap()).unwrap(), serde_json::json!({ "fields": 3, "annotations": 1, "links": 0 }));

    let out = tmp.path().join("flat.pdf");
    let outcome = flatten::run(&[filled.clone()], &out, &flatten::Options::default(), &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n == "Flattened 3 form fields and 1 annotation."), "{:?}", outcome.notes);
    let d = doc::load(&out, None).unwrap();
    assert_eq!(annot_count(&d), 0);
    assert!(!has_acroform(&d));
    assert!(edit::list_fields(&out, None).unwrap().is_empty());
    // No field dictionaries are left lying around in the file either.
    assert!(!d.objects.values().filter_map(|o| o.as_dict().ok()).any(|o| o.has(b"FT")));

    let page = &pages(&out)[0];
    for (value, field) in [("Mara Lindqvist", &rects[0]), ("Logistics (North)", &rects[1])] {
        let l = line(page, value);
        let r = field.rect;
        assert!(l.rect.x0 >= r.x0 && l.rect.x1 <= r.x1 && l.rect.y0 >= r.y0 - 2.0 && l.rect.y1 <= r.y1 + 2.0, "{value} at {:?} is inside {:?}", l.rect, r);
    }
    // The free-text note is page content now, inside its old rectangle.
    let note = line(page, "Return by Friday");
    assert!(note.rect.x0 > 340.0 && note.rect.x1 < 540.0 && note.rect.y0 > 112.0 && note.rect.y1 < 142.0, "{:?}", note.rect);
    assert!(page.plain().contains("MARK-FORM-P1"));
    // The ticked box is drawn: dark stroke pixels inside the check box rectangle.
    let img = render(&out, 0, 3.0);
    let r = rects[2].rect;
    let dark = (0..14).flat_map(|dx| (0..14).map(move |dy| (dx, dy))).filter(|(dx, dy)| (2..12).contains(dx) && (2..12).contains(dy)).map(|(dx, dy)| pixel(&img, r.x0 + dx as f32, r.y0 + dy as f32, 3.0)).filter(|p| p[0] < 100).count();
    assert!(dark >= 8, "tick pixels: {dark}");

    // Edit can flatten in the same run.
    let direct = tmp.path().join("direct.pdf");
    let outcome = fill_form(&direct, true);
    assert!(outcome.notes.iter().any(|n| n.contains("Flattened 3 form fields")), "{:?}", outcome.notes);
    let d = doc::load(&direct, None).unwrap();
    assert!(!has_acroform(&d));
    assert_eq!(annot_count(&d), 1, "only the note is left, it is not a form field");
    assert!(pages(&direct)[0].plain().contains("Mara Lindqvist"));
}

#[test]
fn flatten_can_do_forms_or_annotations_alone() {
    let tmp = out_dir();
    let filled = tmp.path().join("filled.pdf");
    fill_form(&filled, false);

    let forms = tmp.path().join("forms.pdf");
    flatten::run(&[filled.clone()], &forms, &flatten::Options { annotations: false, ..Default::default() }, &ctx()).unwrap();
    let d = doc::load(&forms, None).unwrap();
    assert_eq!(annot_count(&d), 1);
    assert!(!has_acroform(&d));
    let text = pages(&forms)[0].plain();
    assert!(text.contains("Mara Lindqvist") && !text.contains("Return by Friday"));

    let notes = tmp.path().join("notes.pdf");
    flatten::run(&[filled.clone()], &notes, &flatten::Options { forms: false, ..Default::default() }, &ctx()).unwrap();
    let d = doc::load(&notes, None).unwrap();
    assert_eq!(annot_count(&d), 3);
    assert!(has_acroform(&d));
    assert_eq!(edit::list_fields(&notes, None).unwrap()[0].value, "Mara Lindqvist");
    let text = pages(&notes)[0].plain();
    assert!(!text.contains("Mara Lindqvist") && text.contains("Return by Friday"));

    assert_invalid(flatten::run(&[filled], &notes, &flatten::Options { forms: false, annotations: false, keep_links: true }, &ctx()));

    // A file with nothing to flatten comes through unchanged, with a note saying so.
    let plain = tmp.path().join("plain.pdf");
    let outcome = flatten::run(&[fx("report.pdf")], &plain, &flatten::Options::default(), &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("nothing to flatten")));
    assert_eq!(pages(&plain).len(), 6);
}

#[test]
fn flatten_generates_missing_appearances_and_keeps_links() {
    let tmp = out_dir();
    let form = tmp.path().join("nested.pdf");
    nested_form(&form);
    assert_eq!(serde_json::to_value(flatten::summary(&form, None).unwrap()).unwrap(), serde_json::json!({ "fields": 4, "annotations": 0, "links": 1 }));

    let out = tmp.path().join("flat.pdf");
    flatten::run(&[form.clone()], &out, &flatten::Options::default(), &ctx()).unwrap();
    let d = doc::load(&out, None).unwrap();
    assert!(!has_acroform(&d));
    let left = d.get_page_annotations(doc::page_ids(&d)[0]).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].get(b"Subtype").unwrap().as_name().unwrap(), b"Link");
    // "Okafor" had a value but nothing drawn for it; flattening draws it inside the field.
    let page = &pages(&out)[0];
    let l = line(page, "Okafor");
    assert!(l.rect.x0 >= 56.0 && l.rect.x1 <= 256.0 && l.rect.y0 >= 160.0 && l.rect.y1 <= 184.0, "{:?}", l.rect);
    // Both radio buttons were off: their outlines are drawn at their rectangles.
    let (a, b) = (render(&form, 0, 2.0), render(&out, 0, 2.0));
    assert!(changed(&a, &b, Rect4::new(56.0, 208.0, 70.0, 222.0), 2.0) > 20);
    assert!(changed(&a, &b, Rect4::new(96.0, 208.0, 110.0, 222.0), 2.0) > 20);

    let no_links = tmp.path().join("no-links.pdf");
    flatten::run(&[form], &no_links, &flatten::Options { keep_links: false, ..Default::default() }, &ctx()).unwrap();
    assert_eq!(annot_count(&doc::load(&no_links, None).unwrap()), 0);
}

// ---------------------------------------------------------------------------
// Redact
// ---------------------------------------------------------------------------

fn search(text: &str) -> redact::Search {
    redact::Search { text: text.into(), ..Default::default() }
}

#[test]
fn redact_removes_searched_text_for_good() {
    let tmp = out_dir();
    let input = fx("contacts.pdf");
    assert!(occurs(&input, "Mara Lindqvist"), "the detector sees the name in the source");
    assert!(occurs(&input, "Lindqvist") && occurs(&input, "Jonas Okafor"));

    let out = tmp.path().join("redacted.pdf");
    let opts = redact::Options { search: vec![search("mara lindqvist")], ..Default::default() };
    let marks = redact::find_marks(&input, None, &opts).unwrap();
    // Twice on page 1, once on page 2.
    assert_eq!(marks.iter().map(|m| m.page).collect::<Vec<_>>(), [1, 1, 2]);
    assert!(marks.iter().all(|m| m.source == redact::Source::Search && m.label == "mara lindqvist"));
    let outcome = redact::run(&[input.clone()], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes[0].starts_with("2 pages were converted to images at 200 dpi so the removed content cannot be recovered"), "{:?}", outcome.notes);
    assert!(outcome.notes[0].contains("no longer selectable"));
    assert_eq!(outcome.pages, 2);

    // Page 2 carries a mark as well, so both pages are images now: no text at all.
    let text = pages(&out);
    assert_eq!(text.len(), 2);
    assert!(text.iter().all(|p| p.is_empty()));
    for gone in ["Mara Lindqvist", "Lindqvist", "Mara", "Jonas Okafor", "Northfield", "example.com", "MARK-CONTACTS"] {
        assert!(!occurs(&out, gone), "{gone} is still in the file");
    }
    let d = doc::load(&out, None).unwrap();
    for id in doc::page_ids(&d) {
        let page = d.get_dictionary(id).unwrap();
        assert_eq!(image_xobjects(&d, id), 1);
        assert!(!page.has(b"Annots"));
        let res = page.get(b"Resources").unwrap().as_dict().unwrap();
        assert!(!res.has(b"Font"), "no fonts, so no text");
        assert_eq!(doc::visible_size(&d, id), (595.0, 842.0));
    }
    // Nothing but the catalog, the page tree, two pages, their contents and images, and the info dictionary.
    assert!(!d.objects.values().any(|o| o.as_dict().is_ok_and(|x| x.has_type(b"Font"))));

    // The picture shows black where the name was and the rest of the line as before.
    let before = render(&input, 0, 2.0);
    let after = render(&out, 0, 2.0);
    let m = marks[0].rect;
    assert_eq!(pixel(&after, (m.x0 + m.x1) / 2.0, (m.y0 + m.y1) / 2.0, 2.0), [0, 0, 0]);
    let covered = (m.x0 as u32 + 1..m.x1 as u32).flat_map(|x| (m.y0 as u32 + 1..m.y1 as u32).map(move |y| (x, y))).all(|(x, y)| pixel(&after, x as f32, y as f32, 2.0)[0] < 40);
    assert!(covered, "the whole mark is solid");
    let title = Rect4::new(50.0, 50.0, 300.0, 80.0);
    assert!(differs(&before, &after, title, 2.0) < 150, "the title is still there, give or take JPEG noise: {}", differs(&before, &after, title, 2.0));
    let ink = |img: &RgbaImage| (50..300).flat_map(|x| (50..80).map(move |y| (x, y))).filter(|(x, y)| pixel(img, *x as f32, *y as f32, 2.0)[0] < 100).count();
    assert!((ink(&before) as i64 - ink(&after) as i64).abs() < 200, "{} vs {}", ink(&before), ink(&after));
}

#[test]
fn redact_leaves_unmarked_pages_untouched() {
    let tmp = out_dir();
    let out = tmp.path().join("page1.pdf");
    let opts = redact::Options { search: vec![search("Northfield Supply")], ..Default::default() };
    let outcome = redact::run(&[fx("contacts.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes[0].starts_with("1 page was converted to an image at 200 dpi"), "{:?}", outcome.notes);
    let text = pages(&out);
    assert!(text[0].is_empty());
    let second = text[1].plain();
    for kept in ["MARK-CONTACTS-P2", "Signed for the Supplier: Jonas Okafor", "Signed for the Client: Mara Lindqvist", "accounts@example.org"] {
        assert!(second.contains(kept), "{kept} missing from:\n{second}");
    }
    assert!(!occurs(&out, "Northfield"));
    assert!(!occurs(&out, "Alder Row"), "the whole of page 1 is an image, not just the marked words");
    assert!(occurs(&out, "Jonas Okafor"));
    let d = doc::load(&out, None).unwrap();
    let ids = doc::page_ids(&d);
    assert_eq!((image_xobjects(&d, ids[0]), image_xobjects(&d, ids[1])), (1, 0));
}

#[test]
fn redact_patterns_find_personal_data() {
    let input = fx("contacts.pdf");
    let marks = |patterns: &[&str]| redact::find_marks(&input, None, &redact::Options { patterns: patterns.iter().map(|p| p.to_string()).collect(), ..Default::default() }).unwrap();
    let emails = marks(&["email"]);
    assert_eq!(emails.iter().map(|m| (m.page, m.label.as_str(), m.pattern.as_str())).collect::<Vec<_>>(), [(1, "mara.lindqvist@example.com", "email"), (2, "accounts@example.org", "email")]);
    assert!(emails.iter().all(|m| m.source == redact::Source::Pattern));
    // The rectangle covers the address and nothing else on its line.
    let text = pages(&input);
    let l = line(&text[0], "mara.lindqvist@example.com");
    let word = l.words.iter().find(|w| w.text.contains('@')).unwrap();
    let r = emails[0].rect;
    assert!((r.x0 - (word.rect.x0 - 1.0)).abs() < 0.5 && (r.x1 - (word.rect.x1 + 1.0)).abs() < 0.5, "{r:?} vs {:?}", word.rect);
    assert!(r.y0 <= l.rect.y0 && r.y1 >= l.rect.y1);

    assert_eq!(marks(&["phone"]).iter().map(|m| m.label.as_str()).collect::<Vec<_>>(), ["+44 7700 900142"]);
    assert_eq!(marks(&["iban"]).iter().map(|m| m.label.as_str()).collect::<Vec<_>>(), ["GB29 NWBK 6016 1331 9268 19"]);
    assert_eq!(marks(&["date"]).iter().map(|m| (m.page, m.label.as_str())).collect::<Vec<_>>(), [(2, "2026-10-02")]);
    assert!(marks(&["card"]).is_empty());
    assert_eq!(marks(&["email", "phone", "iban", "date"]).len(), 5);
    assert_invalid(redact::find_marks(&input, None, &redact::Options { patterns: vec!["postcode".into()], ..Default::default() }));

    let tmp = out_dir();
    let out = tmp.path().join("patterns.pdf");
    let opts = redact::Options { patterns: vec!["email".into(), "phone".into(), "iban".into()], label: Some("REDACTED".into()), dpi: 150, ..Default::default() };
    redact::run(&[input], &out, &opts, &ctx()).unwrap();
    for gone in ["example.com", "example.org", "7700", "NWBK", "REDACTED"] {
        assert!(!occurs(&out, gone), "{gone} is still in the file");
    }
    // The label is pixels: white letters inside the black mark, and no text.
    assert!(pages(&out).iter().all(|p| p.is_empty()));
    let img = render(&out, 0, 3.0);
    let r = emails[0].rect;
    let (mut white, mut black) = (0, 0);
    for x in r.x0 as u32 + 2..r.x1 as u32 - 2 {
        for y in r.y0 as u32 + 2..r.y1 as u32 - 2 {
            let p = pixel(&img, x as f32, y as f32, 3.0);
            white += (p[0] > 200) as usize;
            black += (p[0] < 50) as usize;
        }
    }
    assert!(white > 20 && black > 5 * white, "white {white}, black {black}");
}

#[test]
fn redact_areas_on_plain_and_rotated_pages() {
    let tmp = out_dir();
    let out = tmp.path().join("area.pdf");
    let area = Rect4::new(56.0, 130.0, 300.0, 150.0);
    let opts = redact::Options { areas: vec![redact::Area { page: 1, rect: area }], color: [200, 30, 30], padding: 0.0, dpi: 144, ..Default::default() };
    let marks = redact::find_marks(&fx("contacts.pdf"), None, &opts).unwrap();
    assert_eq!((marks.len(), marks[0].page, marks[0].rect, marks[0].source), (1, 1, area, redact::Source::Area));
    redact::run(&[fx("contacts.pdf")], &out, &opts, &ctx()).unwrap();
    let img = render(&out, 0, 2.0);
    let p = pixel(&img, 178.0, 140.0, 2.0);
    assert!(p[0] > 180 && p[1] < 60 && p[2] < 60, "inside the area: {p:?}");
    assert_eq!(pixel(&img, 178.0, 122.0, 2.0), [255, 255, 255], "just above the area is untouched");
    let d = doc::load(&out, None).unwrap();
    let image = d.objects.values().filter_map(|o| o.as_stream().ok()).find(|s| s.dict.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Image")).unwrap();
    assert_eq!(image.dict.get(b"Width").unwrap().as_i64().unwrap(), 1190, "595 pt at 144 dpi");
    assert!(pages(&out)[1].plain().contains("Mara Lindqvist"), "page 2 has no mark and keeps its text");

    // A page shown rotated: the mark found by search covers the text as displayed, and the new page keeps the displayed size.
    let rotated = tmp.path().join("rotated.pdf");
    let opts = redact::Options { search: vec![search("MARK-ROTATED-P2")], ..Default::default() };
    let marks = redact::find_marks(&fx("rotated.pdf"), None, &opts).unwrap();
    assert_eq!(marks.len(), 1);
    let m = marks[0].rect;
    assert!(m.height() > m.width(), "the text runs down the displayed page: {m:?}");
    redact::run(&[fx("rotated.pdf")], &rotated, &opts, &ctx()).unwrap();
    let d = doc::load(&rotated, None).unwrap();
    let ids = doc::page_ids(&d);
    assert_eq!(doc::visible_size(&d, ids[1]), (842.0, 595.0));
    assert_eq!(doc::rotation(&d, ids[1]), 0);
    assert_eq!(doc::rotation(&d, ids[2]), 270, "other pages are as they were");
    let (before, after) = (render(&fx("rotated.pdf"), 1, 2.0), render(&rotated, 1, 2.0));
    assert_eq!(before.dimensions(), after.dimensions());
    assert_eq!(pixel(&after, (m.x0 + m.x1) / 2.0, (m.y0 + m.y1) / 2.0, 2.0), [0, 0, 0]);
    // The heading next to it is still there and still in the same place.
    let heading = line(&pages(&fx("rotated.pdf"))[1], "Rotate 90").rect;
    let ink = |img: &RgbaImage| (heading.x0 as u32..heading.x1 as u32).flat_map(|x| (heading.y0 as u32..heading.y1 as u32).map(move |y| (x, y))).filter(|(x, y)| pixel(img, *x as f32, *y as f32, 2.0)[0] < 100).count();
    assert!(ink(&before) > 100 && (ink(&before) as i64 - ink(&after) as i64).abs() < 60, "{} vs {}", ink(&before), ink(&after));
    let text = pages(&rotated);
    assert!(text[1].is_empty());
    assert!(text[0].plain().contains("MARK-ROTATED-P1") && text[3].plain().contains("MARK-ROTATED-P4"));
    assert!(!occurs(&rotated, "MARK-ROTATED-P2") && occurs(&rotated, "MARK-ROTATED-P3"));
}

#[test]
fn redact_cleans_metadata_bookmarks_and_form_values() {
    let tmp = out_dir();

    // Document information and XMP repeat the title and the author.
    let input = fx("metadata.pdf");
    assert!(occurs(&input, "Priya Raman") && occurs(&input, "confidential draft"));
    let out = tmp.path().join("meta.pdf");
    let opts = redact::Options { search: vec![search("Board Minutes")], ..Default::default() };
    let outcome = redact::run(&[input.clone()], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("metadata were cleared")));
    for gone in ["Board Minutes", "Priya Raman", "confidential draft", "Acme Writer", "xmpmeta"] {
        assert!(!occurs(&out, gone), "{gone} is still in the file");
    }
    // Keeping the metadata is allowed, and the user is told the text is still in it.
    let kept = tmp.path().join("meta-kept.pdf");
    let outcome = redact::run(&[input], &kept, &redact::Options { strip_metadata: false, ..opts }, &ctx()).unwrap();
    assert!(occurs(&kept, "Priya Raman"));
    assert!(outcome.notes.iter().any(|n| n.contains("\"Board Minutes\" still appears")), "{:?}", outcome.notes);

    // A bookmark that repeats the name.
    let bookmarked = tmp.path().join("bookmarked.pdf");
    let mut d = doc::load(&fx("contacts.pdf"), None).unwrap();
    let first_page = doc::page_ids(&d)[0];
    let outlines = d.new_object_id();
    let item = d.add_object(dictionary! { "Title" => Object::string_literal("Agreement with Mara Lindqvist"), "Parent" => outlines, "Dest" => vec![Object::Reference(first_page), "Fit".into()] });
    d.objects.insert(outlines, Object::Dictionary(dictionary! { "Type" => "Outlines", "First" => item, "Last" => item, "Count" => 1 }));
    d.catalog_mut().unwrap().set("Outlines", outlines);
    doc::save(&mut d, &bookmarked).unwrap();
    let out = tmp.path().join("bookmarks.pdf");
    let outcome = redact::run(&[bookmarked], &out, &redact::Options { search: vec![search("Mara Lindqvist")], ..Default::default() }, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("1 bookmark")), "{:?}", outcome.notes);
    assert!(!occurs(&out, "Lindqvist"));
    assert!(occurs(&out, "[redacted]"));
    let d = doc::load(&out, None).unwrap();
    let dest = d.objects.values().filter_map(|o| o.as_dict().ok()).find(|o| o.has(b"Dest")).unwrap().get(b"Dest").unwrap().as_array().unwrap()[0].as_reference().unwrap();
    assert_eq!(dest, doc::page_ids(&d)[0], "the bookmark still points at the page");

    // A name typed into a form field is not page text, but it is found and removed with its field.
    let filled = tmp.path().join("filled.pdf");
    fill_form(&filled, false);
    assert!(occurs(&filled, "Mara Lindqvist"));
    let opts = redact::Options { search: vec![search("Mara Lindqvist")], ..Default::default() };
    let marks = redact::find_marks(&filled, None, &opts).unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].rect, Rect4::new(55.0, 145.0, 357.0, 167.0), "the field rectangle plus padding");
    let out = tmp.path().join("form.pdf");
    redact::run(&[filled.clone()], &out, &opts, &ctx()).unwrap();
    for gone in ["Mara Lindqvist", "Logistics", "full_name", "Return by Friday"] {
        assert!(!occurs(&out, gone), "{gone} is still in the file");
    }
    let d = doc::load(&out, None).unwrap();
    assert!(!has_acroform(&d));
    assert_eq!(annot_count(&d), 0);
    // The picture has the other field's value and the ticked box, and black over the redacted field.
    let img = render(&out, 0, 2.0);
    assert_eq!(pixel(&img, 200.0, 156.0, 2.0), [0, 0, 0]);
    let before = render(&filled, 0, 2.0);
    let department = Rect4::new(56.0, 196.0, 356.0, 216.0);
    assert!(differs(&before, &img, department, 2.0) < 150, "the department field looks the same: {}", differs(&before, &img, department, 2.0));
    let tick = (58..68).flat_map(|x| (230..240).map(move |y| (x, y))).filter(|(x, y)| pixel(&img, *x as f32, *y as f32, 2.0)[0] < 110).count();
    assert!(tick >= 4, "the ticked box is in the picture: {tick}");
}

#[test]
fn redact_refuses_when_there_is_nothing_to_do() {
    let tmp = out_dir();
    let out = tmp.path().join("none.pdf");
    assert_invalid(redact::run(&[fx("contacts.pdf")], &out, &redact::Options::default(), &ctx()));
    assert_invalid(redact::run(&[fx("contacts.pdf")], &out, &redact::Options { search: vec![search("Zebedee")], ..Default::default() }, &ctx()));
    assert_invalid(redact::run(&[fx("contacts.pdf")], &out, &redact::Options { areas: vec![redact::Area { page: 3, rect: Rect4::new(0.0, 0.0, 10.0, 10.0) }], ..Default::default() }, &ctx()));
    assert_invalid(redact::run(&[fx("contacts.pdf")], &out, &redact::Options { areas: vec![redact::Area { page: 1, rect: Rect4::new(5.0, 5.0, 5.0, 50.0) }], ..Default::default() }, &ctx()));
    assert!(!out.exists(), "no file is written when nothing was redacted");
    // Whole words and case are honoured.
    let whole = redact::Options { search: vec![redact::Search { text: "Mara".into(), match_case: true, whole_words: true }], ..Default::default() };
    assert_eq!(redact::find_marks(&fx("contacts.pdf"), None, &whole).unwrap().len(), 3);
    let cased = redact::Options { search: vec![redact::Search { text: "mara".into(), match_case: true, whole_words: false }], ..Default::default() };
    assert_eq!(redact::find_marks(&fx("contacts.pdf"), None, &cased).unwrap().len(), 1, "only the e-mail address is lower case");
    // The scanned fixture has no text layer, so a search finds nothing in it.
    assert_invalid(redact::run(&[fx("scan.pdf")], &out, &redact::Options { search: vec![search("Delivery")], ..Default::default() }, &ctx()));
}

#[test]
fn tools_stop_when_cancelled_and_report_progress() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let tmp = out_dir();
    let out = tmp.path().join("x.pdf");
    let calls = AtomicUsize::new(0);
    let progress = |f: f32, what: &str| {
        assert!((0.0..=1.0).contains(&f) && !what.is_empty());
        calls.fetch_add(1, Ordering::Relaxed);
    };
    let cancelled = AtomicBool::new(true);
    let stop = Ctx::new(&progress, &cancelled);
    assert!(matches!(watermark::run(&[fx("report.pdf")], &out, &watermark::Options::default(), &stop), Err(Error::Cancelled)));
    assert!(matches!(numbers::run(&[fx("report.pdf")], &out, &numbers::Options::default(), &stop), Err(Error::Cancelled)));
    assert!(matches!(sign::run(&[fx("report.pdf")], &out, &sign::Options { typed: Some("A".into()), ..Default::default() }, &stop), Err(Error::Cancelled)));
    assert!(matches!(flatten::run(&[fx("form.pdf")], &out, &flatten::Options::default(), &stop), Err(Error::Cancelled)));
    assert!(matches!(redact::run(&[fx("contacts.pdf")], &out, &redact::Options { search: vec![search("Mara")], ..Default::default() }, &stop), Err(Error::Cancelled)));
    assert!(matches!(edit::run(&[fx("form.pdf")], &out, &edit::Options { flatten: true, ..Default::default() }, &stop), Err(Error::Cancelled)));
    assert!(!out.exists());

    let running = AtomicBool::new(false);
    let go = Ctx::new(&progress, &running);
    let before = calls.load(Ordering::Relaxed);
    numbers::run(&[fx("report.pdf")], &out, &numbers::Options::default(), &go).unwrap();
    assert!(calls.load(Ordering::Relaxed) >= before + 6);
    // The input is never modified.
    let original = std::fs::read(fx("report.pdf")).unwrap();
    watermark::run(&[fx("report.pdf")], &out, &watermark::Options::default(), &go).unwrap();
    assert_eq!(std::fs::read(fx("report.pdf")).unwrap(), original);
}

// ---------------------------------------------------------------------------
// Encrypted input
// ---------------------------------------------------------------------------

fn encrypt(source: &Path, target: &Path, password: &str) {
    let mut d = Document::load(source).unwrap();
    d.trailer.set("ID", vec![Object::string_literal(b"shorui-test-id-0".to_vec()), Object::string_literal(b"shorui-test-id-0".to_vec())]);
    let version = lopdf::EncryptionVersion::V2 { document: &d, owner_password: "owner", user_password: password, key_length: 128, permissions: lopdf::Permissions::all() };
    let state = lopdf::EncryptionState::try_from(version).unwrap();
    d.encrypt(&state).unwrap();
    d.save(target).unwrap();
}

#[test]
fn encrypted_input_needs_its_password() {
    let tmp = out_dir();
    let locked = tmp.path().join("locked.pdf");
    encrypt(&fx("contacts.pdf"), &locked, "secret");
    assert!(!occurs_raw(&locked, "Mara Lindqvist"));
    let out = tmp.path().join("out.pdf");
    let wrong = ctx().with_password(Some("nope"));
    let right = ctx().with_password(Some("secret"));

    assert!(matches!(watermark::run(&[locked.clone()], &out, &watermark::Options::default(), &ctx()), Err(Error::PasswordRequired)));
    assert!(matches!(watermark::run(&[locked.clone()], &out, &watermark::Options::default(), &wrong), Err(Error::WrongPassword)));
    assert!(matches!(redact::find_marks(&locked, None, &redact::Options { patterns: vec!["email".into()], ..Default::default() }), Err(Error::PasswordRequired)));
    assert!(matches!(edit::list_fields(&locked, Some("nope")), Err(Error::WrongPassword)));
    assert!(matches!(redact::run(&[locked.clone()], &out, &redact::Options { search: vec![search("Mara")], ..Default::default() }, &ctx()), Err(Error::PasswordRequired)));
    assert!(!out.exists());

    // The result is written without a password, and the user is told.
    let outcome = watermark::run(&[locked.clone()], &out, &watermark::Options { text: "SEEN".into(), rotation: 0.0, ..Default::default() }, &right).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("password protected")), "{:?}", outcome.notes);
    let text = pages(&out)[0].plain();
    assert!(text.contains("SEEN") && text.contains("MARK-CONTACTS-P1"), "{text}");

    assert_eq!(redact::find_marks(&locked, Some("secret"), &redact::Options { patterns: vec!["email".into()], ..Default::default() }).unwrap().len(), 2);
    let redacted = tmp.path().join("redacted.pdf");
    let outcome = redact::run(&[locked], &redacted, &redact::Options { search: vec![search("Mara Lindqvist")], ..Default::default() }, &right).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("password protected")), "{:?}", outcome.notes);
    assert!(pages(&redacted).iter().all(|p| p.is_empty()));
    assert!(!occurs(&redacted, "Lindqvist"));
}

/// `needle` in the raw bytes of the file, without parsing it.
fn occurs_raw(path: &Path, needle: &str) -> bool {
    let raw = std::fs::read(path).unwrap();
    raw.windows(needle.len()).any(|w| w == needle.as_bytes())
}
