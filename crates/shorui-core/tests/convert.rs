#![cfg(feature = "set-convert")]
//! Tests for the conversion tools: img2pdf, pdf2img, tables, csv2pdf, html, office, pdfa.
//!
//! Every test writes the fixtures to a temp folder of its own, runs a tool and reads the
//! result back. Tests that need a browser or an office suite print why and pass when the
//! machine has none.

use lopdf::{Document, Object, dictionary};
use shorui_core::fixtures::{self, Builder, text_op};
use shorui_core::helpers::{self, TempDir};
use shorui_core::render::Renderer;
use shorui_core::text::TextReader;
use shorui_core::tools::{csv2pdf, html, img2pdf, office, pdf2img, pdfa, tables};
use shorui_core::{Ctx, Error, doc};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

fn setup(name: &str) -> (TempDir, PathBuf) {
    let temp = TempDir::new(name).expect("temp folder");
    let dir = temp.path().join("in");
    fixtures::write_all(&dir).expect("fixtures");
    (temp, dir)
}

fn page_sizes(path: &Path) -> Vec<(f32, f32)> {
    let d = doc::load(path, None).expect("output loads");
    doc::page_ids(&d).iter().map(|id| doc::visible_size(&d, *id)).collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1.5
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// All text of a PDF with white space removed, so that word spacing cannot matter.
fn squeezed_text(path: &Path) -> String {
    let reader = TextReader::open_path(path, None).expect("text opens");
    let mut out = String::new();
    for page in reader.all().expect("text reads") {
        out.extend(page.plain().chars().filter(|c| !c.is_whitespace()));
    }
    out
}

/// Width, height and filter of every image in a PDF, soft masks left out.
fn images(d: &Document) -> Vec<(i64, i64, String, bool)> {
    let masks: Vec<_> = d.objects.values().filter_map(|o| o.as_stream().ok()).filter_map(|s| s.dict.get(b"SMask").and_then(Object::as_reference).ok()).collect();
    d.objects
        .iter()
        .filter(|(id, _)| !masks.contains(id))
        .filter_map(|(_, o)| o.as_stream().ok())
        .filter(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image"))
        .map(|s| {
            let n = |k: &[u8]| s.dict.get(k).and_then(Object::as_i64).unwrap_or(0);
            let filter = s.dict.get(b"Filter").and_then(Object::as_name).map(|f| String::from_utf8_lossy(f).to_string()).unwrap_or_default();
            (n(b"Width"), n(b"Height"), filter, s.dict.has(b"SMask"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// img2pdf
// ---------------------------------------------------------------------------

#[test]
fn img2pdf_fits_pages_and_keeps_jpeg_bytes() {
    let (temp, dir) = setup("img2pdf-fit");
    let out = temp.path().join("out/images.pdf");
    let inputs = [dir.join("images/photo.jpg"), dir.join("images/diagram.png")];
    let outcome = img2pdf::run(&inputs, &out, &img2pdf::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 2);
    assert_eq!(outcome.outputs, vec![out.clone()]);
    assert!(outcome.bytes_in > 0 && outcome.bytes_out > 0);

    // 1200 x 800 and 600 x 400 pixels at 150 dpi.
    let sizes = page_sizes(&out);
    assert_eq!(sizes.len(), 2);
    assert!(close(sizes[0].0, 576.0) && close(sizes[0].1, 384.0), "{sizes:?}");
    assert!(close(sizes[1].0, 288.0) && close(sizes[1].1, 192.0), "{sizes:?}");

    // The JPEG file is inside the PDF byte for byte.
    let jpeg = std::fs::read(&inputs[0]).unwrap();
    let pdf = std::fs::read(&out).unwrap();
    assert!(contains(&pdf, &jpeg), "the JPEG was re-encoded");

    // The PNG keeps its transparency and is stored losslessly.
    let d = doc::load(&out, None).unwrap();
    let found = images(&d);
    assert!(found.contains(&(1200, 800, "DCTDecode".into(), false)), "{found:?}");
    assert!(found.contains(&(600, 400, "FlateDecode".into(), true)), "{found:?}");
}

#[test]
fn img2pdf_paper_size_quality_and_title() {
    let (temp, dir) = setup("img2pdf-a4");
    let out = temp.path().join("a4.pdf");
    let inputs = [dir.join("images/photo.jpg"), dir.join("images/diagram.png"), dir.join("images/signature.png")];
    let opts: img2pdf::Options = serde_json::from_value(serde_json::json!({
        "page_size": "a4", "orientation": "portrait", "quality": 60, "title": "Zdj\u{119}cia 2026"
    }))
    .unwrap();
    let outcome = img2pdf::run(&inputs, &out, &opts, &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 3);
    for size in page_sizes(&out) {
        assert!(close(size.0, 595.28) && close(size.1, 841.89), "{size:?}");
    }
    // Re-encoded: the original JPEG bytes are gone and the file is smaller.
    let jpeg = std::fs::read(&inputs[0]).unwrap();
    let pdf = std::fs::read(&out).unwrap();
    assert!(!contains(&pdf, &jpeg));
    let d = doc::load(&out, None).unwrap();
    let found = images(&d);
    assert!(found.contains(&(1200, 800, "DCTDecode".into(), false)), "{found:?}");
    // Images with transparency stay lossless whatever the quality.
    assert!(found.contains(&(600, 400, "FlateDecode".into(), true)), "{found:?}");
    assert!(found.contains(&(400, 120, "FlateDecode".into(), true)), "{found:?}");
    // The title survives as Unicode.
    let info = d.trailer.get(b"Info").and_then(Object::as_reference).unwrap();
    let title = d.get_dictionary(info).unwrap().get(b"Title").unwrap().as_str().unwrap().to_vec();
    assert_eq!(&title[..2], &[0xFE, 0xFF]);
    let units: Vec<u16> = title[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16(&units).unwrap(), "Zdj\u{119}cia 2026");

    // Automatic orientation turns the sheet for wide images; the margin is kept free.
    let auto = temp.path().join("auto.pdf");
    let opts: img2pdf::Options = serde_json::from_value(serde_json::json!({ "page_size": "letter", "margin": 36.0 })).unwrap();
    img2pdf::run(&inputs[..1], &auto, &opts, &Ctx::none()).unwrap();
    let sizes = page_sizes(&auto);
    assert!(close(sizes[0].0, 792.0) && close(sizes[0].1, 612.0), "{sizes:?}");
    let image = Renderer::open_path(&auto, None).unwrap().render(0, 1.0).unwrap();
    assert_eq!(image.get_pixel(20, 306).0, [255, 255, 255, 255], "the margin is not empty");
    assert_ne!(image.get_pixel(396, 306).0, [255, 255, 255, 255], "the image is missing");
}

/// A JPEG, 80 x 40, red on the left and blue on the right, tagged with an EXIF orientation.
fn exif_jpeg(orientation: u8) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(80, 40, |x, _| if x < 40 { image::Rgb([230, 20, 20]) } else { image::Rgb([20, 20, 230]) });
    let mut plain = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut plain, 95).encode_image(&picture).unwrap();
    let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
    exif.extend_from_slice(&[0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, orientation, 0x00, 0x00, 0x00]);
    exif.extend_from_slice(&[0, 0, 0, 0]);
    let mut out = plain[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&exif);
    out.extend_from_slice(&plain[2..]);
    out
}

#[test]
fn img2pdf_honours_exif_orientation() {
    let temp = TempDir::new("img2pdf-exif").unwrap();
    let red = |p: &image::Rgba<u8>| p.0[0] > 150 && p.0[2] < 100;
    let blue = |p: &image::Rgba<u8>| p.0[2] > 150 && p.0[0] < 100;
    // Orientation 6: turn a quarter clockwise to view, so the left half ends up on top.
    // Orientation 3: upside down, so the left half ends up on the right.
    for (orientation, upright_size) in [(6u8, (40.0, 80.0)), (3u8, (80.0, 40.0))] {
        let input = temp.path().join(format!("turned-{orientation}.jpg"));
        std::fs::write(&input, exif_jpeg(orientation)).unwrap();
        for quality in [None, Some(95u8)] {
            let out = temp.path().join(format!("turned-{orientation}-{}.pdf", quality.is_some()));
            let opts = img2pdf::Options { dpi: 72.0, quality, ..Default::default() };
            img2pdf::run(std::slice::from_ref(&input), &out, &opts, &Ctx::none()).unwrap();
            let sizes = page_sizes(&out);
            assert!(close(sizes[0].0, upright_size.0) && close(sizes[0].1, upright_size.1), "orientation {orientation}: {sizes:?}");
            let image = Renderer::open_path(&out, None).unwrap().render(0, 2.0).unwrap();
            if orientation == 6 {
                assert!(red(image.get_pixel(40, 30)), "orientation 6, quality {quality:?}: top is {:?}", image.get_pixel(40, 30));
                assert!(blue(image.get_pixel(40, 130)), "orientation 6, quality {quality:?}: bottom is {:?}", image.get_pixel(40, 130));
            } else {
                assert!(blue(image.get_pixel(30, 40)), "orientation 3, quality {quality:?}: left is {:?}", image.get_pixel(30, 40));
                assert!(red(image.get_pixel(130, 40)), "orientation 3, quality {quality:?}: right is {:?}", image.get_pixel(130, 40));
            }
            // Kept as it is, the JPEG is still byte for byte in the file.
            if quality.is_none() {
                assert!(contains(&std::fs::read(&out).unwrap(), &std::fs::read(&input).unwrap()));
            }
        }
    }
}

#[test]
fn img2pdf_reads_every_supported_format() {
    let (temp, dir) = setup("img2pdf-formats");
    let diagram = image::open(dir.join("images/diagram.png")).unwrap();
    let mut inputs = Vec::new();
    for ext in ["bmp", "gif", "tiff", "webp", "jpeg"] {
        let path = temp.path().join(format!("diagram.{ext}"));
        if ext == "jpeg" {
            diagram.to_rgb8().save(&path).unwrap();
        } else {
            diagram.save(&path).unwrap();
        }
        inputs.push(path);
    }
    // A PNG whose name says something else is still read by its content.
    let disguised = temp.path().join("really-a-png.jpg");
    std::fs::copy(dir.join("images/signature.png"), &disguised).unwrap();
    inputs.push(disguised);
    let out = temp.path().join("formats.pdf");
    let outcome = img2pdf::run(&inputs, &out, &img2pdf::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 6);
    let sizes = page_sizes(&out);
    for size in &sizes[..5] {
        assert!(close(size.0, 288.0) && close(size.1, 192.0), "{sizes:?}");
    }
    assert!(close(sizes[5].0, 192.0) && close(sizes[5].1, 57.6), "{sizes:?}");
    // Every page shows the blue disc in the middle (the last one shows the signature).
    let renderer = Renderer::open_path(&out, None).unwrap();
    for index in 0..5 {
        let pixel = renderer.render(index, 1.0).unwrap().get_pixel(144, 96).0;
        assert!(pixel[2] > 150 && pixel[0] < 90, "page {}: {pixel:?}", index + 1);
    }
}

#[test]
fn img2pdf_refuses_what_is_not_an_image() {
    let (temp, dir) = setup("img2pdf-bad");
    let out = temp.path().join("bad.pdf");
    let err = img2pdf::run(&[dir.join("report.pdf")], &out, &img2pdf::Options::default(), &Ctx::none()).unwrap_err();
    assert!(matches!(err, Error::Invalid(ref m) if m.contains("report.pdf")), "{err}");
    assert!(!out.exists());
    assert!(matches!(img2pdf::run(&[], &out, &img2pdf::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
}

// ---------------------------------------------------------------------------
// pdf2img
// ---------------------------------------------------------------------------

#[test]
fn pdf2img_writes_one_png_per_page() {
    let (temp, dir) = setup("pdf2img-png");
    let out = temp.path().join("pages");
    let opts: pdf2img::Options = serde_json::from_value(serde_json::json!({ "pages": "1-2", "dpi": 72 })).unwrap();
    let steps = Mutex::new(Vec::new());
    let progress = |f: f32, what: &str| steps.lock().unwrap().push((f, what.to_string()));
    let cancel = AtomicBool::new(false);
    let outcome = pdf2img::run(&[dir.join("report.pdf")], &out, &opts, &Ctx::new(&progress, &cancel)).unwrap();
    assert_eq!(outcome.pages, 2);
    assert_eq!(outcome.outputs, vec![out.join("report-1.png"), out.join("report-2.png")]);
    for path in &outcome.outputs {
        let image = image::open(path).unwrap();
        assert_eq!((image.width(), image.height()), (595, 842));
        // Not a blank page: the text left dark pixels.
        assert!(image.to_luma8().pixels().any(|p| p.0[0] < 80));
    }
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2);
    let steps = steps.lock().unwrap();
    assert!(steps.iter().any(|(_, what)| what.contains("page 1")) && steps.iter().any(|(_, what)| what.contains("page 2")), "{steps:?}");
    assert!(steps.windows(2).all(|w| w[0].0 <= w[1].0), "progress went backwards: {steps:?}");
}

#[test]
fn pdf2img_jpeg_pattern_and_errors() {
    let (temp, dir) = setup("pdf2img-jpg");
    let out = temp.path().join("pages");
    let opts: pdf2img::Options = serde_json::from_value(serde_json::json!({ "format": "jpg", "dpi": 36, "pages": "6, 2", "quality": 70, "name_pattern": "p{n}-of-{total}" })).unwrap();
    let outcome = pdf2img::run(&[dir.join("report.pdf")], &out, &opts, &Ctx::none()).unwrap();
    assert_eq!(outcome.outputs, vec![out.join("p6-of-6.jpg"), out.join("p2-of-6.jpg")]);
    let image = image::open(&outcome.outputs[0]).unwrap();
    // Half of 595 x 842 points; which way the half pixel goes is up to the rasteriser.
    assert!(matches!(image.width(), 297 | 298) && image.height() == 421, "{} x {}", image.width(), image.height());
    assert_eq!(&std::fs::read(&outcome.outputs[0]).unwrap()[..2], &[0xFF, 0xD8]);

    // A landscape and rotated pages come out the way they are displayed.
    let rotated = temp.path().join("rotated");
    let opts: pdf2img::Options = serde_json::from_value(serde_json::json!({ "dpi": 72 })).unwrap();
    let outcome = pdf2img::run(&[dir.join("rotated.pdf")], &rotated, &opts, &Ctx::none()).unwrap();
    let dims: Vec<(u32, u32)> = outcome.outputs.iter().map(|p| image::image_dimensions(p).unwrap()).collect();
    assert_eq!(dims, vec![(595, 842), (842, 595), (842, 595), (842, 595)]);

    let bad_range: pdf2img::Options = serde_json::from_value(serde_json::json!({ "pages": "9" })).unwrap();
    assert!(matches!(pdf2img::run(&[dir.join("report.pdf")], &out, &bad_range, &Ctx::none()), Err(Error::Invalid(_))));
    // The renderer rebuilds a wrecked page index by itself, so a damaged file still converts.
    let broken = temp.path().join("broken");
    let opts: pdf2img::Options = serde_json::from_value(serde_json::json!({ "dpi": 36 })).unwrap();
    assert_eq!(pdf2img::run(&[dir.join("broken.pdf")], &broken, &opts, &Ctx::none()).unwrap().pages, 3);
    // Something that is not a PDF at all is refused.
    let not_pdf = temp.path().join("notes.pdf");
    std::fs::write(&not_pdf, "plain text").unwrap();
    assert!(matches!(pdf2img::run(&[not_pdf], &out, &pdf2img::Options::default(), &Ctx::none()), Err(Error::Damaged(_))));

    // Cancelling stops before anything is written.
    let cancelled = temp.path().join("cancelled");
    let cancel = AtomicBool::new(true);
    let progress = |_: f32, _: &str| {};
    let err = pdf2img::run(&[dir.join("report.pdf")], &cancelled, &pdf2img::Options::default(), &Ctx::new(&progress, &cancel)).unwrap_err();
    assert!(matches!(err, Error::Cancelled));
    assert_eq!(std::fs::read_dir(&cancelled).map(|d| d.count()).unwrap_or(0), 0);
    cancel.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------

fn expected_rows() -> Vec<Vec<String>> {
    fixtures::TABLE_ROWS.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect()
}

/// Parse CSV the way a spreadsheet would: quoted fields, doubled quotes.
fn parse_csv(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => row.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    rows
}

#[test]
fn tables_finds_the_table_and_nothing_else() {
    let (temp, dir) = setup("tables");
    let found = tables::detect(&dir.join("table.pdf"), None, &tables::Options::default()).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].page, 1);
    assert_eq!(found[0].rows, expected_rows());
    // The table sits below the heading and paragraph, inside the page.
    let rect = found[0].rect;
    assert!(rect.x0 > 56.0 && rect.x1 < 520.0 && rect.y0 > 160.0 && rect.y1 < 320.0, "{rect:?}");

    let out = temp.path().join("csv");
    let outcome = tables::run(&[dir.join("table.pdf")], &out, &tables::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.outputs, vec![out.join("table-p1-t1.csv")]);
    let text = std::fs::read_to_string(&outcome.outputs[0]).unwrap();
    assert!(text.starts_with('\u{FEFF}'));
    assert_eq!(parse_csv(text.trim_start_matches('\u{FEFF}'), ','), expected_rows());

    // Ordinary text is not a table.
    for name in ["report.pdf", "contacts.pdf", "letter.pdf", "form.pdf", "rotated.pdf"] {
        assert!(tables::detect(&dir.join(name), None, &tables::Options::default()).unwrap().is_empty(), "{name}");
    }
    let err = tables::run(&[dir.join("report.pdf")], &temp.path().join("none"), &tables::Options::default(), &Ctx::none()).unwrap_err();
    assert!(matches!(err, Error::Invalid(ref m) if m.contains("No tables were found")), "{err}");
    assert!(!temp.path().join("none").exists(), "an empty output folder was left behind");
    // A scan has no text at all, which the message says.
    let err = tables::run(&[dir.join("scan.pdf")], &temp.path().join("none"), &tables::Options::default(), &Ctx::none()).unwrap_err();
    assert!(matches!(err, Error::Invalid(ref m) if m.contains("OCR")), "{err}");
}

/// A page with a paragraph, a table that has an empty cell and cells of several words,
/// and a second, smaller table further down.
fn busy_tables_pdf(path: &Path) {
    let mut b = Builder::new();
    let mut c = text_op(56.0, 780.0, 11.0, false, "The order below was placed on the second of October and ships in two parts.");
    c += &text_op(56.0, 764.0, 11.0, false, "Prices are per unit, without tax, and quantities are in pieces unless stated.");
    let rows = [["Item name", "Qty", "Unit price", "Note"], ["Steel bolts M8", "40", "0.35", "in stock"], ["Washers, \"flat\"", "", "0.05", "back order"], ["Hex nuts", "120", "0.08", "in stock"]];
    let xs = [56.0, 220.0, 290.0, 400.0];
    for (r, row) in rows.iter().enumerate() {
        for (col, cell) in row.iter().enumerate() {
            c += &text_op(xs[col], 720.0 - r as f32 * 18.0, 11.0, r == 0, cell);
        }
    }
    c += &text_op(56.0, 600.0, 11.0, false, "Delivery addresses follow. Each line gives the depot and the day it accepts goods.");
    for (r, row) in [["Depot", "Day"], ["Leeds North", "Monday"], ["Hull", "Thursday"]].iter().enumerate() {
        c += &text_op(56.0, 560.0 - r as f32 * 18.0, 11.0, false, row[0]);
        c += &text_op(260.0, 560.0 - r as f32 * 18.0, 11.0, false, row[1]);
    }
    b.page(fixtures::A4, c);
    // Page 2: two columns of running text, like a newsletter. Not a table.
    let mut c = String::new();
    for line in 0..12 {
        let y = 760.0 - line as f32 * 16.0;
        c += &text_op(56.0, y, 10.0, false, "Revenue grew steadily across all three regions");
        c += &text_op(310.0, y, 10.0, false, "Customer support answered most requests on time");
    }
    b.page(fixtures::A4, c);
    let mut d = b.finish();
    doc::save(&mut d, path).unwrap();
}

#[test]
fn tables_handles_empty_cells_several_tables_and_text_columns() {
    let temp = TempDir::new("tables-busy").unwrap();
    let input = temp.path().join("order.pdf");
    busy_tables_pdf(&input);
    let found = tables::detect(&input, None, &tables::Options::default()).unwrap();
    assert_eq!(found.len(), 2, "{found:#?}");
    let s = |rows: &[&[&str]]| -> Vec<Vec<String>> { rows.iter().map(|r| r.iter().map(|c| c.to_string()).collect()).collect() };
    assert_eq!(
        found[0].rows,
        s(&[&["Item name", "Qty", "Unit price", "Note"], &["Steel bolts M8", "40", "0.35", "in stock"], &["Washers, \"flat\"", "", "0.05", "back order"], &["Hex nuts", "120", "0.08", "in stock"]])
    );
    assert_eq!(found[1].rows, s(&[&["Depot", "Day"], &["Leeds North", "Monday"], &["Hull", "Thursday"]]));
    assert!(found.iter().all(|t| t.page == 1), "the text columns on page 2 were taken for a table");

    // min_cols and min_rows filter; one file holds everything, tab separated.
    let wide: tables::Options = serde_json::from_value(serde_json::json!({ "min_cols": 3 })).unwrap();
    assert_eq!(tables::detect(&input, None, &wide).unwrap().len(), 1);
    let tall: tables::Options = serde_json::from_value(serde_json::json!({ "min_rows": 4 })).unwrap();
    assert_eq!(tables::detect(&input, None, &tall).unwrap().len(), 1);
    let only_two: tables::Options = serde_json::from_value(serde_json::json!({ "pages": "2" })).unwrap();
    assert!(tables::detect(&input, None, &only_two).unwrap().is_empty());

    let out = temp.path().join("csv");
    let opts: tables::Options = serde_json::from_value(serde_json::json!({ "single_file": true, "delimiter": "tab", "bom": false })).unwrap();
    let outcome = tables::run(std::slice::from_ref(&input), &out, &opts, &Ctx::none()).unwrap();
    assert_eq!(outcome.outputs, vec![out.join("order-tables.csv")]);
    let text = std::fs::read_to_string(&outcome.outputs[0]).unwrap();
    let rows = parse_csv(&text, '\t');
    assert_eq!(rows.len(), 4 + 1 + 3, "{text}");
    assert_eq!(rows[2], vec!["Washers, \"flat\"", "", "0.05", "back order"]);
    assert_eq!(rows[4], vec![""], "tables are separated by a blank line");
    assert_eq!(rows[5], vec!["Depot", "Day"]);

    // Separate files, comma separated: the quoted field survives a round trip.
    let out = temp.path().join("csv-each");
    let outcome = tables::run(std::slice::from_ref(&input), &out, &tables::Options { bom: false, ..Default::default() }, &Ctx::none()).unwrap();
    assert_eq!(outcome.outputs, vec![out.join("order-p1-t1.csv"), out.join("order-p1-t2.csv")]);
    let text = std::fs::read_to_string(&outcome.outputs[0]).unwrap();
    assert!(text.contains("\"Washers, \"\"flat\"\"\",,0.05,back order\r\n"), "{text}");
    assert_eq!(parse_csv(&text, ',')[2], vec!["Washers, \"flat\"", "", "0.05", "back order"]);
}

// ---------------------------------------------------------------------------
// csv2pdf
// ---------------------------------------------------------------------------

fn words(path: &Path) -> Vec<Vec<(String, f32, f32)>> {
    let reader = TextReader::open_path(path, None).expect("text opens");
    reader
        .all()
        .expect("text reads")
        .iter()
        .map(|page| page.lines.iter().flat_map(|l| l.words.iter()).map(|w| (w.text.clone(), w.rect.x1, w.rect.y0)).collect())
        .collect()
}

#[test]
fn csv2pdf_pages_a_long_table_and_repeats_the_header() {
    let temp = TempDir::new("csv2pdf-long").unwrap();
    let csv = temp.path().join("stock list.csv");
    let mut body = String::from("Code,Description,Units\n");
    for n in 1..=150 {
        body += &format!("ROW-{n:03},\"Part {n}, boxed\",{}\n", n * 7);
    }
    std::fs::write(&csv, &body).unwrap();
    let before = std::fs::read(&csv).unwrap();
    let out = temp.path().join("out/stock.pdf");
    let outcome = csv2pdf::run(std::slice::from_ref(&csv), &out, &csv2pdf::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(std::fs::read(&csv).unwrap(), before);
    assert!(outcome.pages >= 3, "{} pages", outcome.pages);
    assert_eq!(outcome.pages, doc::page_count(&doc::load(&out, None).unwrap()));
    for (w, h) in page_sizes(&out) {
        assert!(close(w, 595.3) && close(h, 841.9), "A4 portrait, got {w}x{h}");
    }

    let pages = words(&out);
    let mut seen = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        assert!(page.iter().any(|w| w.0 == "Code"), "header missing on page {}", i + 1);
        assert!(page.iter().any(|w| w.0 == "Page") && page.iter().any(|w| w.0 == format!("{}", i + 1)), "footer missing on page {}", i + 1);
        seen.extend(page.iter().filter(|w| w.0.starts_with("ROW-")).map(|w| w.0.clone()));
    }
    let expected: Vec<String> = (1..=150).map(|n| format!("ROW-{n:03}")).collect();
    assert_eq!(seen, expected, "every row once, in order");

    // The Units column holds numbers only, so it is right-aligned: every number ends at the same x.
    let ends: Vec<f32> = pages[0].iter().filter(|w| w.0.chars().all(|c| c.is_ascii_digit()) && w.0.len() >= 2 && w.0.parse::<u32>().is_ok_and(|n| n % 7 == 0 && n >= 14)).map(|w| w.1).collect();
    assert!(ends.len() > 10);
    assert!(ends.iter().all(|x| (x - ends[0]).abs() < 0.5), "{ends:?}");

    let d = doc::load(&out, None).unwrap();
    let info = d.trailer.get(b"Info").and_then(Object::as_reference).unwrap();
    assert_eq!(d.get_dictionary(info).unwrap().get(b"Title").unwrap().as_str().unwrap(), b"stock list");
}

#[test]
fn csv2pdf_reads_back_as_the_same_table() {
    let temp = TempDir::new("csv2pdf-roundtrip").unwrap();
    let csv = temp.path().join("regions.csv");
    std::fs::write(&csv, "Region;Units;Revenue\nNorth;1200;48000.00\nSouth;950;38000.00\nEast;1410;56400.00\nWest;879;35160.00\n").unwrap();
    let out = temp.path().join("regions.pdf");
    let opts: csv2pdf::Options = serde_json::from_value(serde_json::json!({ "grid": false, "stripes": false, "page_numbers": false })).unwrap();
    csv2pdf::run(std::slice::from_ref(&csv), &out, &opts, &Ctx::none()).unwrap();
    assert_eq!(page_sizes(&out).len(), 1);
    let tables_out = temp.path().join("tables");
    let outcome = tables::run(std::slice::from_ref(&out), &tables_out, &tables::Options::default(), &Ctx::none()).unwrap();
    let text = std::fs::read_to_string(&outcome.outputs[0]).unwrap();
    let rows = parse_csv(text.trim_start_matches('\u{feff}'), ',');
    assert_eq!(rows[0], ["Region", "Units", "Revenue"]);
    assert_eq!(rows[4], ["West", "879", "35160.00"]);
}

#[test]
fn csv2pdf_turns_wide_tables_landscape_and_wraps() {
    let temp = TempDir::new("csv2pdf-wide").unwrap();
    let csv = temp.path().join("wide.tsv");
    let head: Vec<String> = (1..=14).map(|c| format!("Column{c}")).collect();
    let long = "a very long note that cannot possibly fit on one line of a narrow column ".repeat(3);
    let row: Vec<String> = (1..=14).map(|c| if c == 3 { long.clone() } else { format!("value-{c}") }).collect();
    std::fs::write(&csv, format!("{}\n{}\n", head.join("\t"), row.join("\t"))).unwrap();
    let out = temp.path().join("wide.pdf");
    csv2pdf::run(std::slice::from_ref(&csv), &out, &csv2pdf::Options::default(), &Ctx::none()).unwrap();
    let (w, h) = page_sizes(&out)[0];
    assert!(w > h, "landscape expected, got {w}x{h}");
    let pages = words(&out);
    // Every header made it, nothing ran off the page, and the long note wrapped onto several lines.
    for c in 1..=14 {
        assert!(pages[0].iter().any(|x| x.0 == format!("Column{c}")), "Column{c}");
    }
    assert!(pages[0].iter().all(|x| x.1 <= w - 36.0 + 0.5), "text past the right margin");
    let lines: std::collections::BTreeSet<i32> = pages[0].iter().filter(|x| x.0 == "narrow").map(|x| x.2 as i32).collect();
    assert_eq!(lines.len(), 3, "the note wraps, so its three copies of \"narrow\" sit on different lines");

    // Forced portrait still fits, with smaller text.
    let opts = csv2pdf::Options { orientation: csv2pdf::Orientation::Portrait, ..Default::default() };
    let outcome = csv2pdf::run(std::slice::from_ref(&csv), &out, &opts, &Ctx::none()).unwrap();
    let (w, h) = page_sizes(&out)[0];
    assert!(w < h);
    assert!(words(&out)[0].iter().all(|x| x.1 <= w - 36.0 + 0.5), "text past the right margin");
    assert!(outcome.notes.iter().any(|n| n.contains("smaller")), "{:?}", outcome.notes);
}

#[test]
fn csv2pdf_handles_encodings_and_odd_rows() {
    let temp = TempDir::new("csv2pdf-enc").unwrap();
    // Windows-1252 bytes (not UTF-8), a short row, a quoted line break, and a character
    // the standard fonts cannot draw.
    let csv = temp.path().join("legacy.csv");
    let mut bytes = b"Name,City,Note\nJos\xE9,M\xFCnchen\nAnn,\"Oslo\",\"line one\nline two\"\n".to_vec();
    bytes.extend_from_slice("Li,\u{5317}\u{4eac},x\n".as_bytes());
    std::fs::write(&csv, &bytes).unwrap();
    let out = temp.path().join("legacy.pdf");
    // The file is not valid UTF-8 as a whole, so it is read as Windows-1252.
    let outcome = csv2pdf::run(std::slice::from_ref(&csv), &out, &csv2pdf::Options::default(), &Ctx::none()).unwrap();
    let all = squeezed_text(&out);
    for s in ["José", "München", "lineone", "linetwo", "Oslo"] {
        assert!(all.contains(s), "{s} missing from {all}");
    }
    assert!(outcome.notes.iter().any(|n| n.contains("3 rows and 3 columns")), "{:?}", outcome.notes);

    let utf8 = temp.path().join("utf8.csv");
    std::fs::write(&utf8, "\u{feff}Name,City\nLi,\u{5317}\u{4eac}\nZoë,Kraków\n").unwrap();
    let outcome = csv2pdf::run(std::slice::from_ref(&utf8), &out, &csv2pdf::Options::default(), &Ctx::none()).unwrap();
    assert!(squeezed_text(&out).contains("Zoë"));
    assert!(outcome.notes.iter().any(|n| n.contains("shown as \"?\"")), "{:?}", outcome.notes);
}

#[test]
fn csv2pdf_error_paths() {
    let temp = TempDir::new("csv2pdf-errors").unwrap();
    let out = temp.path().join("x.pdf");
    let o = csv2pdf::Options::default();
    assert!(matches!(csv2pdf::run(&[], &out, &o, &Ctx::none()), Err(Error::Invalid(_))));
    let empty = temp.path().join("empty.csv");
    std::fs::write(&empty, "\n\r\n").unwrap();
    assert!(matches!(csv2pdf::run(std::slice::from_ref(&empty), &out, &o, &Ctx::none()), Err(Error::Invalid(_))));
    assert!(matches!(csv2pdf::run(&[empty.clone(), empty.clone()], &out, &o, &Ctx::none()), Err(Error::Invalid(_))));
    let binary = temp.path().join("photo.csv");
    std::fs::write(&binary, [0x89, b'P', b'N', b'G', 0, 0, 0, 13]).unwrap();
    assert!(matches!(csv2pdf::run(std::slice::from_ref(&binary), &out, &o, &Ctx::none()), Err(Error::Invalid(_))));
    let missing = temp.path().join("nope.csv");
    assert!(matches!(csv2pdf::run(std::slice::from_ref(&missing), &out, &o, &Ctx::none()), Err(Error::Read { .. })));
    let ok = temp.path().join("ok.csv");
    std::fs::write(&ok, "a,b\n1,2\n").unwrap();
    for bad in [serde_json::json!({ "font_size": 2.0 }), serde_json::json!({ "margin": -1.0 }), serde_json::json!({ "margin": 400.0 })] {
        let opts: csv2pdf::Options = serde_json::from_value(bad.clone()).unwrap();
        assert!(matches!(csv2pdf::run(std::slice::from_ref(&ok), &out, &opts, &Ctx::none()), Err(Error::Invalid(_))), "{bad}");
    }
    assert!(!out.exists());
    let cancel = AtomicBool::new(true);
    let progress = |_: f32, _: &str| {};
    assert!(matches!(csv2pdf::run(std::slice::from_ref(&ok), &out, &o, &Ctx::new(&progress, &cancel)), Err(Error::Cancelled)));
    assert!(!out.exists());
}

// ---------------------------------------------------------------------------
// html
// ---------------------------------------------------------------------------

#[test]
fn html_prints_a_local_file() {
    if helpers::find_browser().is_none() {
        println!("skipped: no Chromium-based browser (Chrome, Edge, Chromium, Brave) on this machine");
        return;
    }
    let (temp, dir) = setup("html");
    let out = temp.path().join("sample.pdf");
    let entries_before = std::fs::read_dir(&dir).unwrap().count();
    let source_before = std::fs::read(dir.join("sample.html")).unwrap();
    let outcome = html::run(&[dir.join("sample.html")], &out, &html::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 1);
    assert!(squeezed_text(&out).contains("MARK-HTML-P1"));
    let sizes = page_sizes(&out);
    assert!(close(sizes[0].0, 595.0) && close(sizes[0].1, 842.0), "not A4: {sizes:?}");
    // The original file is untouched and nothing was written next to it.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries_before);
    assert_eq!(std::fs::read(dir.join("sample.html")).unwrap(), source_before);
}

#[test]
fn html_applies_paper_orientation_margins_and_background() {
    if helpers::find_browser().is_none() {
        println!("skipped: no Chromium-based browser (Chrome, Edge, Chromium, Brave) on this machine");
        return;
    }
    let (temp, dir) = setup("html-opts");
    // A folder and file name with spaces, a relative image, a coloured background.
    let site = temp.path().join("my site");
    std::fs::create_dir_all(site.join("art work")).unwrap();
    std::fs::copy(dir.join("images/signature.png"), site.join("art work/sig nature.png")).unwrap();
    let page = site.join("front page.html");
    std::fs::write(&page, "<!doctype html><html><head><meta charset=\"utf-8\"><title>t</title><style>@page{size:A5;margin:3cm}html,body{margin:0;background:#00c000}</style></head><body><p>MARK-HTMLOPT-P1 \u{17b}\u{f3}\u{142}w</p><img src=\"art%20work/sig%20nature.png\"></body></html>").unwrap();

    let out = temp.path().join("letter.pdf");
    let opts: html::Options = serde_json::from_value(serde_json::json!({ "paper": "letter", "landscape": true, "margins": false })).unwrap();
    let outcome = html::run(std::slice::from_ref(&page), &out, &opts, &Ctx::none()).unwrap();
    assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
    // Our paper size wins over the page's own @page rule.
    let sizes = page_sizes(&out);
    assert!(close(sizes[0].0, 792.0) && close(sizes[0].1, 612.0), "not Letter landscape: {sizes:?}");
    assert!(squeezed_text(&out).contains("MARK-HTMLOPT-P1"));
    // The relative image was found from the temp copy.
    let d = doc::load(&out, None).unwrap();
    assert!(images(&d).iter().any(|i| (i.0, i.1) == (400, 120)), "{:?}", images(&d));
    // No margins and backgrounds on: the corner of the sheet is green.
    let image = Renderer::open_path(&out, None).unwrap().render(0, 1.0).unwrap();
    let corner = image.get_pixel(4, 4).0;
    assert!(corner[1] > 150 && corner[0] < 80, "corner is {corner:?}");

    // Backgrounds off: the same corner is white.
    let plain = temp.path().join("plain.pdf");
    let opts: html::Options = serde_json::from_value(serde_json::json!({ "paper": "a3", "margins": false, "background": false })).unwrap();
    html::run(std::slice::from_ref(&page), &plain, &opts, &Ctx::none()).unwrap();
    let sizes = page_sizes(&plain);
    assert!(close(sizes[0].0, 842.0) && close(sizes[0].1, 1191.0), "not A3: {sizes:?}");
    let image = Renderer::open_path(&plain, None).unwrap().render(0, 1.0).unwrap();
    assert_eq!(&image.get_pixel(4, 4).0[..3], &[255, 255, 255]);

    // The same file through a file: URL.
    let by_url = temp.path().join("by-url.pdf");
    let opts = html::Options { url: Some(html::file_url(&page).unwrap()), ..Default::default() };
    assert!(opts.url.as_deref().unwrap().contains("my%20site/front%20page.html"));
    html::run(&[], &by_url, &opts, &Ctx::none()).unwrap();
    assert!(squeezed_text(&by_url).contains("MARK-HTMLOPT-P1"));

    // A page that asks for files from the internet is printed without them, and says so.
    let remote = site.join("remote.html");
    std::fs::write(&remote, "<html><body><p>MARK-REMOTE-P1</p><img src=\"https://example.invalid/pixel.png\"></body></html>").unwrap();
    let out = temp.path().join("remote.pdf");
    let outcome = html::run(std::slice::from_ref(&remote), &out, &html::Options::default(), &Ctx::none()).unwrap();
    assert!(squeezed_text(&out).contains("MARK-REMOTE-P1"));
    assert!(outcome.notes.iter().any(|n| n.contains("not loaded")), "{:?}", outcome.notes);
}

#[test]
fn html_error_paths() {
    let temp = TempDir::new("html-errors").unwrap();
    let out = temp.path().join("x.pdf");
    let none = html::run(&[], &out, &html::Options::default(), &Ctx::none()).unwrap_err();
    assert!(matches!(none, Error::Invalid(_)), "{none}");
    for url in ["javascript:alert(1)", "ftp://example.com/a.html", "chrome://settings", "example.com"] {
        let opts = html::Options { url: Some(url.to_string()), ..Default::default() };
        assert!(matches!(html::run(&[], &out, &opts, &Ctx::none()), Err(Error::Invalid(_))), "{url}");
    }
    let wrong = temp.path().join("notes.txt");
    std::fs::write(&wrong, "hello").unwrap();
    assert!(matches!(html::run(&[wrong], &out, &html::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
    // A browser that is not there is reported as a missing helper, with a hint.
    let page = temp.path().join("a.html");
    std::fs::write(&page, "<p>hi</p>").unwrap();
    let opts = html::Options { browser_path: Some(temp.path().join("no-such-browser").display().to_string()), ..Default::default() };
    let err = html::run(&[page], &out, &opts, &Ctx::none()).unwrap_err();
    assert!(matches!(err, Error::MissingHelper { .. }), "{err}");
    assert!(!out.exists());
}

// ---------------------------------------------------------------------------
// office
// ---------------------------------------------------------------------------

/// A zip archive with the files stored uncompressed. Enough for Word to open.
fn stored_zip(files: &[(&str, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, body) in files {
        let mut crc = flate2::Crc::new();
        crc.update(body.as_bytes());
        let offset = out.len() as u32;
        let header = |sig: u32, extra_version: bool| {
            let mut h = sig.to_le_bytes().to_vec();
            if extra_version {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
            h.extend_from_slice(&crc.sum().to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra length
            h
        };
        out.extend_from_slice(&header(0x0403_4b50, false));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body.as_bytes());
        central.extend_from_slice(&header(0x0201_4b50, true));
        central.extend_from_slice(&[0u8; 2 + 2 + 2 + 4]); // comment, disk, internal and external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn minimal_docx(lines: &[&str]) -> Vec<u8> {
    let paragraphs: String = lines.iter().map(|l| format!("<w:p><w:r><w:t>{l}</w:t></w:r></w:p>")).collect();
    stored_zip(&[
        (
            "[Content_Types].xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>",
        ),
        (
            "_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>",
        ),
        (
            "word/document.xml",
            &format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{paragraphs}</w:body></w:document>"),
        ),
    ])
}

#[test]
fn office_round_trip() {
    let found = office::engines();
    if !found.libreoffice && !found.msoffice {
        println!("skipped: neither LibreOffice nor Microsoft Office is installed on this machine");
        return;
    }
    let (temp, dir) = setup("office");
    // DOCX to PDF. The folder and file names have spaces and an apostrophe in them.
    let folder = temp.path().join("Ana's files");
    std::fs::create_dir_all(&folder).unwrap();
    let docx = folder.join("status report.docx");
    std::fs::write(&docx, minimal_docx(&["Office test", "MARK-OFFICE-P1", "Second paragraph."])).unwrap();
    let before = std::fs::read(&docx).unwrap();
    let pdf = temp.path().join("out/status report.pdf");
    let outcome = office::run(std::slice::from_ref(&docx), &pdf, &office::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 1);
    assert!(outcome.notes.iter().any(|n| n.starts_with("Converted with")), "{:?}", outcome.notes);
    assert!(squeezed_text(&pdf).contains("MARK-OFFICE-P1"), "{}", squeezed_text(&pdf));
    assert_eq!(std::fs::read(&docx).unwrap(), before, "the input was changed");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "something was written next to the input");

    // PDF to DOCX.
    let word = temp.path().join("out/letter.docx");
    let outcome = office::run(&[dir.join("letter.pdf")], &word, &office::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 1);
    let bytes = std::fs::read(&word).unwrap();
    assert_eq!(&bytes[..2], b"PK");
    assert!(contains(&bytes, b"word/document.xml"));
}

#[test]
fn office_spreadsheet_to_pdf() {
    let found = office::engines();
    if !found.libreoffice && !found.msoffice {
        println!("skipped: neither LibreOffice nor Microsoft Office is installed on this machine");
        return;
    }
    let temp = TempDir::new("office-csv").unwrap();
    let csv = temp.path().join("stock.csv");
    std::fs::write(&csv, "Item,Qty\r\nBolts,40\r\nMARKCSVP1,7\r\n").unwrap();
    let pdf = temp.path().join("stock.pdf");
    match office::run(std::slice::from_ref(&csv), &pdf, &office::Options::default(), &Ctx::none()) {
        Ok(outcome) => {
            assert_eq!(outcome.pages, 1);
            assert!(squeezed_text(&pdf).contains("MARKCSVP1"));
        }
        Err(Error::MissingHelper { tool, .. }) => println!("skipped: {tool} is not installed"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn office_error_paths() {
    let temp = TempDir::new("office-errors").unwrap();
    let out = temp.path().join("x.pdf");
    let picture = temp.path().join("a.png");
    std::fs::write(&picture, b"x").unwrap();
    assert!(matches!(office::run(&[picture], &out, &office::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
    let sheet = temp.path().join("a.csv");
    std::fs::write(&sheet, "a,b\r\n").unwrap();
    let to_docx: office::Options = serde_json::from_value(serde_json::json!({ "to": "docx" })).unwrap();
    assert!(matches!(office::run(std::slice::from_ref(&sheet), &out, &to_docx, &Ctx::none()), Err(Error::Invalid(_))));
    let to_odt: office::Options = serde_json::from_value(serde_json::json!({ "to": "odt" })).unwrap();
    assert!(matches!(office::run(std::slice::from_ref(&sheet), &out, &to_odt, &Ctx::none()), Err(Error::Invalid(_))));
    assert!(matches!(office::run(&[], &out, &office::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
    assert!(matches!(office::run(&[temp.path().join("missing.docx")], &out, &office::Options::default(), &Ctx::none()), Err(Error::Read { .. })));

    // An engine that is not installed is named, with a hint about what to install.
    if helpers::find_libreoffice().is_none() {
        let libre: office::Options = serde_json::from_value(serde_json::json!({ "engine": "libreoffice" })).unwrap();
        let err = office::run(std::slice::from_ref(&sheet), &out, &libre, &Ctx::none()).unwrap_err();
        assert!(matches!(err, Error::MissingHelper { ref tool, ref hint } if tool == "LibreOffice" && hint.contains("libreoffice.org")), "{err}");
    } else {
        println!("LibreOffice is installed: the missing-helper path was not exercised");
    }
    if !cfg!(windows) {
        let ms: office::Options = serde_json::from_value(serde_json::json!({ "engine": "msoffice" })).unwrap();
        assert!(matches!(office::run(std::slice::from_ref(&sheet), &out, &ms, &Ctx::none()), Err(Error::Unsupported(_))));
    }
    assert!(!out.exists());
}

// ---------------------------------------------------------------------------
// pdfa
// ---------------------------------------------------------------------------

/// The XMP packet and the output intent of a PDF/A file, checked and returned.
fn pdfa_parts(path: &Path) -> (String, Document) {
    let d = doc::load(path, None).expect("output loads");
    let catalog = d.catalog().unwrap();
    let metadata = d.get_object(catalog.get(b"Metadata").and_then(Object::as_reference).unwrap()).unwrap().as_stream().unwrap();
    assert!(metadata.dict.get(b"Filter").is_err(), "the XMP packet is compressed");
    let xmp = String::from_utf8(metadata.content.clone()).unwrap();
    assert!(xmp.contains("<pdfaid:part>2</pdfaid:part>"), "{xmp}");
    assert!(xmp.contains("<pdfaid:conformance>B</pdfaid:conformance>"), "{xmp}");
    assert!(xmp.contains("<pdf:Producer>Shorui</pdf:Producer>"), "{xmp}");
    assert!(xmp.starts_with("<?xpacket begin=") && xmp.trim_end().ends_with("<?xpacket end=\"w\"?>"));

    let intents = catalog.get(b"OutputIntents").unwrap().as_array().unwrap();
    assert_eq!(intents.len(), 1);
    let intent = d.get_dictionary(intents[0].as_reference().unwrap()).unwrap();
    assert_eq!(intent.get(b"S").unwrap().as_name().unwrap(), b"GTS_PDFA1");
    let profile = d.get_object(intent.get(b"DestOutputProfile").and_then(Object::as_reference).unwrap()).unwrap().as_stream().unwrap();
    assert_eq!(profile.dict.get(b"N").unwrap().as_i64().unwrap(), 3);
    let icc = profile.decompressed_content().unwrap_or_else(|_| profile.content.clone());
    assert_eq!(&icc[36..40], b"acsp");
    assert_eq!(u32::from_be_bytes([icc[0], icc[1], icc[2], icc[3]]) as usize, icc.len());

    // A file identifier, no encryption, version 1.7 with a binary comment line.
    let id = d.trailer.get(b"ID").unwrap().as_array().unwrap();
    assert_eq!(id.len(), 2);
    assert!(id.iter().all(|part| part.as_str().unwrap().len() >= 16));
    assert!(d.trailer.get(b"Encrypt").is_err());
    let raw = std::fs::read(path).unwrap();
    assert!(raw.starts_with(b"%PDF-1.7\n%"));
    assert!(raw[10..14].iter().all(|b| *b > 127));
    // Object numbers run without a gap (see `fill_gaps` in pdfa.rs for why that matters).
    let numbers: Vec<u32> = d.objects.keys().map(|id| id.0).collect();
    assert_eq!(numbers, (1..=numbers.len() as u32).collect::<Vec<u32>>());
    (xmp, d)
}

#[test]
fn pdfa_keeps_pages_that_qualify() {
    let (temp, dir) = setup("pdfa-keep");
    // The scan has no fonts at all, so nothing stands in the way.
    let report = pdfa::check(&dir.join("scan.pdf"), None).unwrap();
    assert!(report.can_preserve(), "{report:?}");
    let out = temp.path().join("scan-a.pdf");
    let outcome = pdfa::run(&[dir.join("scan.pdf")], &out, &pdfa::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 2);
    assert!(outcome.notes[0].starts_with("Kept every page"), "{:?}", outcome.notes);
    assert!(outcome.notes.iter().any(|n| n.contains("not a full PDF/A validator")), "{:?}", outcome.notes);
    let (_, d) = pdfa_parts(&out);
    // Same pages, same images: nothing was redrawn.
    let source = doc::load(&dir.join("scan.pdf"), None).unwrap();
    assert_eq!(images(&d), images(&source));
    assert_eq!(page_sizes(&out), page_sizes(&dir.join("scan.pdf")));
}

#[test]
fn pdfa_turns_pages_into_images_when_fonts_are_missing() {
    let (temp, dir) = setup("pdfa-image");
    let report = pdfa::check(&dir.join("metadata.pdf"), None).unwrap();
    assert!(!report.can_preserve());
    assert_eq!(report.fonts_not_embedded, vec!["Helvetica".to_string(), "Helvetica-Bold".to_string()]);

    let out = temp.path().join("minutes-a.pdf");
    let opts: pdfa::Options = serde_json::from_value(serde_json::json!({ "level": "2b", "mode": "auto", "dpi": 100 })).unwrap();
    let outcome = pdfa::run(&[dir.join("metadata.pdf")], &out, &opts, &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 2);
    assert!(outcome.notes[0].contains("turned into an image") && outcome.notes[0].contains("Helvetica"), "{:?}", outcome.notes);
    let (xmp, d) = pdfa_parts(&out);
    // The document information is carried over and mirrored in the XMP.
    assert!(xmp.contains("Board Minutes, confidential draft") && xmp.contains("<rdf:li>Priya Raman</rdf:li>"), "{xmp}");
    assert!(xmp.contains("<xmp:CreatorTool>Acme Writer 4.2</xmp:CreatorTool>"), "{xmp}");
    let info = d.get_dictionary(d.trailer.get(b"Info").and_then(Object::as_reference).unwrap()).unwrap();
    assert_eq!(info.get(b"Title").unwrap().as_str().unwrap(), b"Board Minutes, confidential draft");
    // Image-only pages: no fonts, no text, one picture per page at 100 dpi.
    assert!(!d.objects.values().any(|o| matches!(o, Object::Dictionary(dict) if dict.has_type(b"Font"))));
    assert!(squeezed_text(&out).is_empty());
    assert_eq!(images(&d).len(), 2);
    assert!(images(&d).iter().all(|i| (i.0, i.1) == (827, 1170) || (i.0, i.1) == (826, 1169) || (i.0, i.1) == (827, 1169)), "{:?}", images(&d));
    assert_eq!(page_sizes(&out), page_sizes(&dir.join("metadata.pdf")));
    // The page still shows its text as a picture.
    let image = Renderer::open_path(&out, None).unwrap().render(0, 1.0).unwrap();
    assert!(image.pixels().any(|p| p.0[0] < 80));

    // Asked to keep the pages anyway, it does, and says plainly that the file falls short.
    let kept = temp.path().join("minutes-kept.pdf");
    let opts: pdfa::Options = serde_json::from_value(serde_json::json!({ "mode": "preserve" })).unwrap();
    let outcome = pdfa::run(&[dir.join("metadata.pdf")], &kept, &opts, &Ctx::none()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("probably does not meet PDF/A-2b") && n.contains("not embedded")), "{:?}", outcome.notes);
    pdfa_parts(&kept);
    assert!(squeezed_text(&kept).contains("MARK-METADATA-P2"));
}

#[test]
fn pdfa_removes_what_the_standard_forbids() {
    let (temp, dir) = setup("pdfa-clean");
    // The scan again, with a script that runs on opening, an attached file, a hidden note,
    // and an interpolated image.
    let mut d = doc::load(&dir.join("scan.pdf"), None).unwrap();
    let script = d.add_object(dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('hi')") });
    let file = d.add_object(lopdf::Stream::new(dictionary! { "Type" => "EmbeddedFile" }, b"attached".to_vec()));
    let spec = d.add_object(dictionary! { "Type" => "Filespec", "F" => Object::string_literal("a.txt"), "EF" => dictionary! { "F" => file } });
    let names = dictionary! {
        "EmbeddedFiles" => dictionary! { "Names" => vec![Object::string_literal("a.txt"), Object::Reference(spec)] },
        "JavaScript" => dictionary! { "Names" => vec![Object::string_literal("s"), Object::Reference(script)] },
    };
    let catalog = d.trailer.get(b"Root").and_then(Object::as_reference).unwrap();
    {
        let c = d.get_dictionary_mut(catalog).unwrap();
        c.set("OpenAction", script);
        c.set("Names", names);
        c.set("AA", dictionary! { "WC" => script });
    }
    let page = doc::page_ids(&d)[0];
    let hidden = d.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()], "F" => 2, "Contents" => Object::string_literal("secret note"),
    });
    d.get_dictionary_mut(page).unwrap().set("Annots", vec![Object::Reference(hidden)]);
    let image_ids: Vec<_> = d.objects.iter().filter(|(_, o)| matches!(o, Object::Stream(s) if s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image"))).map(|(id, _)| *id).collect();
    for id in &image_ids {
        if let Ok(Object::Stream(s)) = d.get_object_mut(*id) {
            s.dict.set("Interpolate", true);
        }
    }
    let input = temp.path().join("dirty.pdf");
    doc::save(&mut d, &input).unwrap();
    assert!(contains(&std::fs::read(&input).unwrap(), b"JavaScript"));

    let out = temp.path().join("clean.pdf");
    let outcome = pdfa::run(std::slice::from_ref(&input), &out, &pdfa::Options::default(), &Ctx::none()).unwrap();
    assert!(outcome.notes[0].starts_with("Kept every page"), "{:?}", outcome.notes);
    let removed = outcome.notes.iter().find(|n| n.starts_with("Removed")).expect("a note about what was removed");
    assert!(removed.contains("script") && removed.contains("attached file") && removed.contains("annotation"), "{removed}");
    let (_, clean) = pdfa_parts(&out);
    let raw = std::fs::read(&out).unwrap();
    for gone in [&b"JavaScript"[..], b"app.alert", b"EmbeddedFile", b"secret note", b"OpenAction"] {
        assert!(!contains(&raw, gone), "{} is still in the file", String::from_utf8_lossy(gone));
    }
    assert!(clean.catalog().unwrap().get(b"AA").is_err());
    for object in clean.objects.values() {
        if let Object::Stream(s) = object {
            assert!(!s.dict.get(b"Interpolate").and_then(Object::as_bool).unwrap_or(false));
        }
    }
    assert_eq!(doc::page_count(&clean), 2);
}

#[test]
fn pdfa_allows_an_invisible_text_layer() {
    let (temp, dir) = setup("pdfa-ocr");
    // A scanned page with a searchable layer, the way an OCR tool leaves it: text in
    // rendering mode 3 (invisible) set in a standard font that is not embedded.
    let mut d = doc::load(&dir.join("scan.pdf"), None).unwrap();
    let page = doc::page_ids(&d)[0];
    let font = doc::ensure_font(&mut d, page, doc::StdFont::Helvetica).unwrap();
    doc::overlay(&mut d, page, format!("BT 3 Tr /{font} 11 Tf 56 722 Td (MARK-OCR-P1) Tj ET").into_bytes(), false).unwrap();
    let input = temp.path().join("searchable.pdf");
    doc::save(&mut d, &input).unwrap();

    let report = pdfa::check(&input, None).unwrap();
    assert!(report.can_preserve(), "{report:?}");
    assert_eq!(report.invisible_text_fonts, vec!["Helvetica".to_string()]);
    let out = temp.path().join("searchable-a.pdf");
    let outcome = pdfa::run(std::slice::from_ref(&input), &out, &pdfa::Options::default(), &Ctx::none()).unwrap();
    assert!(outcome.notes[0].starts_with("Kept every page"), "{:?}", outcome.notes);
    pdfa_parts(&out);
    assert!(squeezed_text(&out).contains("MARK-OCR-P1"), "the text layer was lost");

    // The same font drawing visible text does stand in the way.
    let mut d = doc::load(&dir.join("scan.pdf"), None).unwrap();
    let page = doc::page_ids(&d)[0];
    let font = doc::ensure_font(&mut d, page, doc::StdFont::Helvetica).unwrap();
    doc::overlay(&mut d, page, format!("BT /{font} 11 Tf 56 722 Td (Stamped) Tj ET").into_bytes(), false).unwrap();
    let visible = temp.path().join("stamped.pdf");
    doc::save(&mut d, &visible).unwrap();
    let report = pdfa::check(&visible, None).unwrap();
    assert_eq!(report.fonts_not_embedded, vec!["Helvetica".to_string()]);
    assert!(!report.can_preserve());
}

#[test]
fn pdfa_error_paths() {
    let (temp, dir) = setup("pdfa-errors");
    let out = temp.path().join("x.pdf");
    assert!(matches!(pdfa::run(&[], &out, &pdfa::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
    let low: pdfa::Options = serde_json::from_value(serde_json::json!({ "dpi": 5 })).unwrap();
    assert!(matches!(pdfa::run(&[dir.join("scan.pdf")], &out, &low, &Ctx::none()), Err(Error::Invalid(_))));
    assert!(serde_json::from_value::<pdfa::Options>(serde_json::json!({ "level": "1a" })).is_err());
    let not_pdf = temp.path().join("notes.pdf");
    std::fs::write(&not_pdf, "plain text").unwrap();
    assert!(matches!(pdfa::run(&[not_pdf], &out, &pdfa::Options::default(), &Ctx::none()), Err(Error::Damaged(_))));
    // Cancelling while pages are being drawn stops the job and writes nothing.
    let cancel = AtomicBool::new(true);
    let progress = |_: f32, _: &str| {};
    let image: pdfa::Options = serde_json::from_value(serde_json::json!({ "mode": "image" })).unwrap();
    let err = pdfa::run(&[dir.join("report.pdf")], &out, &image, &Ctx::new(&progress, &cancel)).unwrap_err();
    assert!(matches!(err, Error::Cancelled));
    assert!(!out.exists());
}

#[test]
fn options_have_defaults_and_keep_their_names() {
    use shorui_core::tools::default_options;
    let fields = |id: &str| -> Vec<String> {
        let mut keys: Vec<String> = default_options(id).unwrap().as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(fields("img2pdf"), ["dpi", "margin", "orientation", "page_size", "quality", "title"]);
    assert_eq!(fields("pdf2img"), ["dpi", "format", "name_pattern", "pages", "quality"]);
    assert_eq!(fields("tables"), ["bom", "delimiter", "min_cols", "min_rows", "pages", "single_file"]);
    assert_eq!(fields("html"), ["allow_remote", "background", "browser_path", "header_footer", "landscape", "margins", "paper", "timeout_secs", "url", "wait_ms"]);
    assert_eq!(fields("csv2pdf"), ["delimiter", "font_size", "grid", "header", "margin", "orientation", "page_numbers", "paper", "stripes", "title"]);
    assert_eq!(fields("office"), ["engine", "timeout_secs", "to"]);
    assert_eq!(fields("pdfa"), ["dpi", "level", "mode", "quality"]);
    let d = default_options("img2pdf").unwrap();
    assert_eq!((d["page_size"].as_str(), d["dpi"].as_f64(), d["orientation"].as_str()), (Some("fit"), Some(150.0), Some("auto")));
    let d = default_options("pdf2img").unwrap();
    assert_eq!((d["format"].as_str(), d["dpi"].as_u64(), d["quality"].as_u64(), d["name_pattern"].as_str()), (Some("png"), Some(150), Some(90), Some("{name}-{n}")));
    let d = default_options("tables").unwrap();
    assert_eq!((d["delimiter"].as_str(), d["min_rows"].as_u64(), d["min_cols"].as_u64(), d["single_file"].as_bool()), (Some("comma"), Some(2), Some(2), Some(false)));
    let d = default_options("html").unwrap();
    assert_eq!((d["paper"].as_str(), d["background"].as_bool(), d["margins"].as_bool(), d["wait_ms"].as_u64(), d["header_footer"].as_bool()), (Some("a4"), Some(true), Some(true), Some(2000), Some(false)));
    let d = default_options("office").unwrap();
    assert_eq!((d["engine"].as_str(), d["to"].is_null()), (Some("auto"), true));
    let d = default_options("pdfa").unwrap();
    assert_eq!((d["level"].as_str(), d["mode"].as_str()), (Some("2b"), Some("auto")));
}
