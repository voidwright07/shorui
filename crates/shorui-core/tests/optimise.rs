#![cfg(feature = "set-optimise")]
//! Compress, Repair, Compare and OCR, run on the generated fixtures.

use image::{DynamicImage, Rgba, RgbaImage};
use lopdf::{Dictionary, Object, Stream, dictionary};
use shorui_core::render::Renderer;
use shorui_core::text::TextReader;
use shorui_core::tools::{compare, compress, ocr, repair};
use shorui_core::{Ctx, Error, doc, fixtures, img};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

/// The fixtures, written once per test run into cargo's scratch folder for this crate.
fn fx() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("optimise-fixtures");
        let _ = std::fs::remove_dir_all(&dir);
        fixtures::write_all(&dir).expect("fixtures");
        dir
    })
}

/// A clean folder for one test's outputs.
fn out_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("optimise-out").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

fn page_texts(path: &Path) -> Vec<String> {
    let reader = TextReader::open_path(path, None).unwrap();
    (0..reader.page_count()).map(|i| reader.page(i).unwrap().plain()).collect()
}

/// Mean absolute difference per colour channel between two renders of the same size.
fn mean_abs_diff(a: &RgbaImage, b: &RgbaImage) -> f64 {
    assert_eq!(a.dimensions(), b.dimensions());
    let total: u64 = a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|c| p.0[c].abs_diff(q.0[c]) as u64).sum::<u64>()).sum();
    total as f64 / (a.width() as f64 * a.height() as f64 * 3.0)
}

/// `(width, height, filter, colour space)` of every image XObject in a file.
fn images(path: &Path) -> Vec<(i64, i64, String, String)> {
    let d = doc::load(path, None).unwrap();
    let mut out = Vec::new();
    for object in d.objects.values() {
        if let Object::Stream(s) = object {
            if matches!(s.dict.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")) {
                let name = |key: &[u8]| s.dict.get(key).and_then(Object::as_name).map(|n| String::from_utf8_lossy(n).to_string()).unwrap_or_default();
                out.push((s.dict.get(b"Width").unwrap().as_i64().unwrap(), s.dict.get(b"Height").unwrap().as_i64().unwrap(), name(b"Filter"), name(b"ColorSpace")));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Compress
// ---------------------------------------------------------------------------

#[test]
fn compress_shrinks_photos_and_keeps_pages_readable() {
    let input = fx().join("photos.pdf");
    let dir = out_dir("compress-photos");
    let out = dir.join("photos-small.pdf");
    let messages = std::sync::Mutex::new(Vec::<String>::new());
    let progress = |_: f32, what: &str| messages.lock().unwrap().push(what.to_string());
    let cancel = AtomicBool::new(false);
    let outcome = compress::run(&[input.clone()], &out, &compress::Options::default(), &Ctx::new(&progress, &cancel)).unwrap();

    assert_eq!(outcome.pages, 3);
    assert_eq!(outcome.bytes_in, size(&input));
    assert_eq!(outcome.bytes_out, size(&out));
    let saved = 1.0 - outcome.bytes_out as f64 / outcome.bytes_in as f64;
    println!("photos.pdf balanced: {} -> {} bytes, saved {:.1}%; notes {:?}", outcome.bytes_in, outcome.bytes_out, saved * 100.0, outcome.notes);
    assert!(saved > 0.9, "balanced should save well over 80%, saved {:.1}%", saved * 100.0);
    assert!(outcome.notes.iter().any(|n| n.contains("Recompressed 3 of 3 images")), "{:?}", outcome.notes);
    assert!(messages.lock().unwrap().iter().any(|m| m == "Recompressing images, page 2 of 3"));

    // Text survives.
    let texts = page_texts(&out);
    assert_eq!(texts.len(), 3);
    for (i, text) in texts.iter().enumerate() {
        assert!(text.contains(&format!("MARK-PHOTOS-P{}", i + 1)), "page {}: {text}", i + 1);
    }
    // The images were scaled to 150 dpi (483 pt wide) and stored as JPEG.
    let list = images(&out);
    assert_eq!(list.len(), 3);
    for (w, h, filter, space) in &list {
        assert_eq!((*w, *h, filter.as_str(), space.as_str()), (1006, 719, "DCTDecode", "DeviceRGB"));
    }
    // Every page still renders, and page 1 looks like it did.
    let before = Renderer::open_path(&input, None).unwrap();
    let after = Renderer::open_path(&out, None).unwrap();
    assert_eq!(after.page_count(), 3);
    for i in 0..3 {
        let diff = mean_abs_diff(&before.render(i, 0.5).unwrap(), &after.render(i, 0.5).unwrap());
        println!("page {} mean absolute pixel difference after compression: {diff:.3}", i + 1);
        assert!(diff < 4.0, "page {} changed too much: {diff}", i + 1);
    }
    // The input is untouched.
    assert_eq!(size(&input), outcome.bytes_in);
}

#[test]
fn compress_presets_order_and_estimate_matches() {
    let input = fx().join("photos.pdf");
    let dir = out_dir("compress-presets");
    let mut sizes = Vec::new();
    for preset in ["light", "balanced", "strong"] {
        let opts: compress::Options = serde_json::from_value(serde_json::json!({ "preset": preset })).unwrap();
        let out = dir.join(format!("{preset}.pdf"));
        let outcome = compress::run(&[input.clone()], &out, &opts, &Ctx::none()).unwrap();
        let estimate = compress::estimate(&input, None, &opts).unwrap();
        println!("{preset}: {} bytes, estimate {estimate}", outcome.bytes_out);
        let off = (estimate as f64 - outcome.bytes_out as f64).abs() / outcome.bytes_out as f64;
        assert!(off < 0.02, "estimate {estimate} vs actual {}", outcome.bytes_out);
        sizes.push(outcome.bytes_out);
    }
    assert!(sizes[0] > sizes[1] && sizes[1] > sizes[2], "{sizes:?}");

    // Custom values are used as given, and grayscale changes the colour space.
    let opts: compress::Options = serde_json::from_value(serde_json::json!({ "preset": "custom", "image_quality": 50, "max_dpi": 72, "grayscale": true })).unwrap();
    let out = dir.join("custom.pdf");
    compress::run(&[input], &out, &opts, &Ctx::none()).unwrap();
    for (w, h, filter, space) in images(&out) {
        assert_eq!((w, h, filter.as_str(), space.as_str()), (483, 345, "DCTDecode", "DeviceGray"));
    }
}

#[test]
fn compress_never_grows_a_file() {
    let dir = out_dir("compress-small");
    // A file that has already been through Compress has nothing left to give.
    let once = dir.join("once.pdf");
    compress::run(&[fx().join("report.pdf")], &once, &compress::Options::default(), &Ctx::none()).unwrap();
    assert!(size(&once) <= size(&fx().join("report.pdf")));
    let twice = dir.join("twice.pdf");
    let outcome = compress::run(&[once.clone()], &twice, &compress::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.notes, vec!["Already small, copied as is".to_string()]);
    assert_eq!(std::fs::read(&once).unwrap(), std::fs::read(&twice).unwrap());
    assert_eq!(outcome.bytes_out, outcome.bytes_in);
    assert_eq!(compress::estimate(&once, None, &compress::Options::default()).unwrap(), size(&once));

    // Every fixture comes out no larger, with its pages and marks intact.
    for (name, tag, pages) in [("report.pdf", "REPORT", 6), ("rotated.pdf", "ROTATED", 4), ("form.pdf", "FORM", 1), ("table.pdf", "TABLE", 1)] {
        let input = fx().join(name);
        let out = dir.join(name);
        let outcome = compress::run(&[input.clone()], &out, &compress::Options::default(), &Ctx::none()).unwrap();
        assert!(outcome.bytes_out <= size(&input), "{name} grew");
        let texts = page_texts(&out);
        assert_eq!(texts.len(), pages, "{name}");
        for (i, text) in texts.iter().enumerate() {
            assert!(text.contains(&format!("MARK-{tag}-P{}", i + 1)), "{name} page {}", i + 1);
        }
    }
    // The form is still a form.
    let form = doc::load(&dir.join("form.pdf"), None).unwrap();
    assert!(form.catalog().unwrap().has(b"AcroForm"));
}

#[test]
fn compress_strips_metadata_when_asked() {
    let dir = out_dir("compress-metadata");
    let out = dir.join("clean.pdf");
    let opts = compress::Options { strip_metadata: true, subset_fonts: true, ..Default::default() };
    compress::run(&[fx().join("metadata.pdf")], &out, &opts, &Ctx::none()).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    let d = doc::load_bytes(&bytes, None).unwrap();
    assert!(!d.trailer.has(b"Info"));
    assert!(!d.catalog().unwrap().has(b"Metadata"));
    let texts = page_texts(&out);
    assert!(texts[0].contains("MARK-METADATA-P1") && texts[1].contains("MARK-METADATA-P2"));

    // Without the option the author stays.
    let kept = dir.join("kept.pdf");
    compress::run(&[fx().join("metadata.pdf")], &kept, &compress::Options::default(), &Ctx::none()).unwrap();
    let d = doc::load(&kept, None).unwrap();
    let info = d.trailer.get(b"Info").and_then(Object::as_reference).unwrap();
    assert_eq!(d.get_dictionary(info).unwrap().get(b"Author").unwrap().as_str().unwrap(), b"Priya Raman");
}

/// An image with transparency, drawn small through a Form XObject under a scaled matrix.
#[test]
fn compress_follows_matrices_into_forms_and_keeps_soft_masks() {
    let dir = out_dir("compress-form");
    let (mut d, root) = doc::new_document();
    let photo = fixtures::photo(600, 400, 5);
    let rgba = RgbaImage::from_fn(600, 400, |x, y| {
        let p = photo.get_pixel(x, y).0;
        Rgba([p[0], p[1], p[2], (x * 255 / 599) as u8])
    });
    let image = img::add_image(&mut d, &DynamicImage::ImageRgba8(rgba), img::Encoding::Flate).unwrap();
    let form = d.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 400.into(), 400.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
        },
        b"q 288 0 0 192 20 20 cm /Im0 Do Q".to_vec(),
    ));
    let content = d.add_object(Stream::new(Dictionary::new(), b"q 0.5 0 0 0.5 100 100 cm /Fm0 Do Q".to_vec()));
    let page = d.add_object(dictionary! {
        "Type" => "Page", "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Resources" => dictionary! { "XObject" => dictionary! { "Fm0" => form } }, "Contents" => content,
    });
    doc::append_pages(&mut d, root, &[page]).unwrap();
    let input = dir.join("form-image.pdf");
    doc::save(&mut d, &input).unwrap();

    // Drawn 144 x 96 pt (288 x 192 halved): 600 px over 2 inches is 300 dpi, so 150 dpi halves it.
    let out = dir.join("small.pdf");
    let outcome = compress::run(&[input.clone()], &out, &compress::Options::default(), &Ctx::none()).unwrap();
    assert!(outcome.bytes_out < outcome.bytes_in / 3, "{} -> {}", outcome.bytes_in, outcome.bytes_out);
    let mut list = images(&out);
    list.sort();
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!(list[0], (300, 200, "DCTDecode".to_string(), "DeviceRGB".to_string()));
    assert_eq!(list[1], (300, 200, "FlateDecode".to_string(), "DeviceGray".to_string()), "the soft mask follows the image");
    let after = doc::load(&out, None).unwrap();
    assert!(after.objects.values().any(|o| matches!(o, Object::Stream(s) if s.dict.has(b"SMask"))));
    let diff = mean_abs_diff(&Renderer::open_path(&input, None).unwrap().render(0, 1.0).unwrap(), &Renderer::open_path(&out, None).unwrap().render(0, 1.0).unwrap());
    println!("form image mean absolute pixel difference: {diff:.3}");
    assert!(diff < 2.0, "{diff}");
}

#[test]
fn compress_reports_errors_plainly() {
    let dir = out_dir("compress-errors");
    let out = dir.join("x.pdf");
    assert!(matches!(compress::run(&[], &out, &compress::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
    let not_pdf = dir.join("notes.pdf");
    std::fs::write(&not_pdf, b"just some text").unwrap();
    assert!(matches!(compress::run(&[not_pdf], &out, &compress::Options::default(), &Ctx::none()), Err(Error::Damaged(_))));
    let cancel = AtomicBool::new(true);
    let progress = |_: f32, _: &str| {};
    let result = compress::run(&[fx().join("photos.pdf")], &out, &compress::Options::default(), &Ctx::new(&progress, &cancel));
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(!out.exists());
}

// ---------------------------------------------------------------------------
// Repair
// ---------------------------------------------------------------------------

#[test]
fn repair_rebuilds_a_wrecked_index() {
    let input = fx().join("broken.pdf");
    assert!(doc::load(&input, None).is_err(), "the fixture is meant not to open");
    let out = out_dir("repair-broken").join("fixed.pdf");
    let outcome = repair::run(&[input], &out, &repair::Options::default(), &Ctx::none()).unwrap();
    println!("broken.pdf: {:?}", outcome.notes);
    assert_eq!(outcome.pages, 3);
    assert!(outcome.notes[0].starts_with("Rebuilt the page index from ") && outcome.notes[0].contains("all 3 pages recovered"), "{:?}", outcome.notes);
    let d = doc::load(&out, None).unwrap();
    assert_eq!(doc::page_count(&d), 3);
    let texts = page_texts(&out);
    for (i, text) in texts.iter().enumerate() {
        assert!(text.contains(&format!("MARK-BROKEN-P{}", i + 1)), "page {}: {text}", i + 1);
    }
    assert_eq!(Renderer::open_path(&out, None).unwrap().page_count(), 3);
}

#[test]
fn repair_recovers_what_a_truncated_file_still_holds() {
    let dir = out_dir("repair-truncated");
    let whole = std::fs::read(fx().join("report.pdf")).unwrap();
    let input = dir.join("truncated.pdf");
    std::fs::write(&input, &whole[..whole.len() * 7 / 10]).unwrap();
    assert!(doc::load(&input, None).is_err());
    let out = dir.join("fixed.pdf");
    let outcome = repair::run(&[input], &out, &repair::Options::default(), &Ctx::none()).unwrap();
    println!("report.pdf cut to 70%: {} pages, {:?}", outcome.pages, outcome.notes);
    assert!(outcome.pages >= 1 && outcome.pages < 6, "{}", outcome.pages);
    assert!(outcome.notes[0].contains(&format!("{} of 6 pages recovered", outcome.pages)), "{:?}", outcome.notes);
    let texts = page_texts(&out);
    assert_eq!(texts.len(), outcome.pages);
    for (i, text) in texts.iter().enumerate() {
        assert!(text.contains(&format!("MARK-REPORT-P{}", i + 1)), "page {}: {text}", i + 1);
    }
}

#[test]
fn repair_drops_junk_before_the_header() {
    let dir = out_dir("repair-garbage");
    let whole = std::fs::read(fx().join("report.pdf")).unwrap();
    for (name, junk) in [("short", 40usize), ("long", 3000)] {
        let mut bytes: Vec<u8> = (0..junk).map(|i| (i * 7 % 251) as u8).collect();
        bytes.extend_from_slice(&whole);
        let input = dir.join(format!("{name}.pdf"));
        std::fs::write(&input, &bytes).unwrap();
        let out = dir.join(format!("{name}-fixed.pdf"));
        let outcome = repair::run(&[input], &out, &repair::Options::default(), &Ctx::none()).unwrap();
        println!("{junk} bytes of junk prepended: {:?}", outcome.notes);
        assert_eq!(outcome.pages, 6);
        assert!(outcome.notes.iter().any(|n| n.contains(&format!("Removed {junk} bytes"))), "{:?}", outcome.notes);
        assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF-"));
        let texts = page_texts(&out);
        for (i, text) in texts.iter().enumerate() {
            assert!(text.contains(&format!("MARK-REPORT-P{}", i + 1)));
        }
    }
}

#[test]
fn repair_unpacks_object_streams_when_the_index_is_gone() {
    let dir = out_dir("repair-objstm");
    let compact = dir.join("compact.pdf");
    let mut d = doc::load(&fx().join("report.pdf"), None).unwrap();
    doc::save_compact(&mut d, &compact).unwrap();
    let bytes = std::fs::read(&compact).unwrap();
    assert!(bytes.windows(7).any(|w| w == b"/ObjStm"), "the compact file should use object streams");
    // Cut the file inside its cross-reference stream: the index and trailer are gone.
    let at = bytes.windows(5).rposition(|w| w == b"/XRef").expect("xref stream");
    let input = dir.join("no-index.pdf");
    std::fs::write(&input, &bytes[..at]).unwrap();
    assert!(doc::load(&input, None).is_err());
    let out = dir.join("fixed.pdf");
    let outcome = repair::run(&[input], &out, &repair::Options::default(), &Ctx::none()).unwrap();
    println!("object streams without an index: {:?}", outcome.notes);
    assert_eq!(outcome.pages, 6);
    let texts = page_texts(&out);
    for (i, text) in texts.iter().enumerate() {
        assert!(text.contains(&format!("MARK-REPORT-P{}", i + 1)));
    }
}

/// Pages whose content is nonsense (as it is when a file is still encrypted, or when the
/// bytes belong to something else) must not be reported as recovered.
#[test]
fn repair_does_not_pass_off_unreadable_pages_as_recovered() {
    let dir = out_dir("repair-garbage-content");
    let garbage: Vec<u8> = (0..600u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8 | 0x80).collect();
    for (name, spoil, expect_pages) in [("one", 1usize, Some(5usize)), ("all", 6, None)] {
        let mut d = doc::load(&fx().join("report.pdf"), None).unwrap();
        for page in doc::page_ids(&d).into_iter().take(spoil) {
            for id in d.get_page_contents(page) {
                d.objects.insert(id, Object::Stream(Stream::new(dictionary! { "Filter" => "FlateDecode" }, garbage.clone())));
            }
        }
        let mut bytes = b"junk before the header ".to_vec();
        doc::save(&mut d, &dir.join("tmp.pdf")).unwrap();
        bytes.extend(std::fs::read(dir.join("tmp.pdf")).unwrap());
        let input = dir.join(format!("{name}.pdf"));
        std::fs::write(&input, bytes).unwrap();
        let out = dir.join(format!("{name}-fixed.pdf"));
        let result = repair::run(&[input], &out, &repair::Options::default(), &Ctx::none());
        match expect_pages {
            Some(pages) => {
                let outcome = result.unwrap();
                println!("one unreadable page: {:?}", outcome.notes);
                assert_eq!(outcome.pages, pages);
                assert!(outcome.notes[0].contains("5 of 6 pages recovered, 1 could not be read"), "{:?}", outcome.notes);
                let texts = page_texts(&out);
                assert!(texts[0].contains("MARK-REPORT-P2") && texts[4].contains("MARK-REPORT-P6"));
            }
            None => match result {
                Err(Error::Damaged(message)) => {
                    println!("every page unreadable: {message}");
                    assert!(message.contains("cannot be read"), "{message}");
                    assert!(!out.exists());
                }
                other => panic!("expected Damaged, got {other:?}"),
            },
        }
    }
}

#[test]
fn repair_says_so_when_a_file_is_fine_or_hopeless() {
    let dir = out_dir("repair-other");
    let out = dir.join("fine.pdf");
    let outcome = repair::run(&[fx().join("report.pdf")], &out, &repair::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 6);
    assert!(outcome.notes[0].contains("opened normally"), "{:?}", outcome.notes);

    let junk = dir.join("junk.pdf");
    std::fs::write(&junk, b"This is a text file that somebody renamed.".repeat(40)).unwrap();
    let hopeless = dir.join("hopeless.pdf");
    match repair::run(&[junk], &hopeless, &repair::Options::default(), &Ctx::none()) {
        Err(Error::Damaged(message)) => println!("not a PDF: {message}"),
        other => panic!("expected Damaged, got {other:?}"),
    }
    assert!(!hopeless.exists());

    let head = dir.join("head.pdf");
    std::fs::write(&head, &std::fs::read(fx().join("report.pdf")).unwrap()[..40]).unwrap();
    assert!(matches!(repair::run(&[head], &hopeless, &repair::Options::default(), &Ctx::none()), Err(Error::Damaged(_))));
    assert!(matches!(repair::run(&[], &hopeless, &repair::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
}

// ---------------------------------------------------------------------------
// Compare
// ---------------------------------------------------------------------------

fn read_report(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("report.json")).unwrap()).unwrap()
}

#[test]
fn compare_finds_exactly_the_changed_pages() {
    let dir = out_dir("compare-versions");
    let (a, b) = (fx().join("report.pdf"), fx().join("report-v2.pdf"));
    let outcome = compare::run(&[a.clone(), b.clone()], &dir, &compare::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.pages, 6);
    assert_eq!(outcome.notes[0], "2 pages differ: 2, 4.");

    let report = read_report(&dir);
    assert_eq!(report["pages_a"], 6);
    assert_eq!(report["pages_b"], 6);
    assert_eq!(report["identical"], false);
    let pages = report["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 6);
    for page in pages {
        let n = page["page"].as_u64().unwrap();
        let changed = n == 2 || n == 4;
        assert_eq!(page["text_changed"], changed, "page {n}");
        assert_eq!(page["visual_changed"], changed, "page {n}");
        assert_eq!(page["changed_ratio"].as_f64().unwrap() > 0.0, changed, "page {n}");
        assert_eq!(dir.join(format!("diff-p{n}.png")).exists(), changed, "page {n}");
    }
    assert_eq!(pages[1]["added"][0], "This sentence was rewritten in the second version of the report.");
    assert_eq!(pages[1]["removed"][0], "Inventory turnover improved after the warehouse layout was changed.");
    assert_eq!(pages[3]["added"][0], "An extra closing remark was added on page four.");
    assert_eq!(pages[3]["removed"].as_array().unwrap().len(), 0);

    let text = std::fs::read_to_string(dir.join("report.txt")).unwrap();
    assert!(text.contains("=== Page 2") && text.contains("=== Page 4") && !text.contains("=== Page 1") && !text.contains("=== Page 3"), "{text}");
    assert!(text.contains("-Inventory turnover improved after the warehouse layout was changed."));
    assert!(text.contains("+This sentence was rewritten in the second version of the report."));

    // The pictures: red where the old sentence was, green where the new one is.
    let picture = image::open(dir.join("diff-p2.png")).unwrap().to_rgb8();
    let (mut red, mut green) = (0, 0);
    for p in picture.pixels() {
        let [r, g, b] = p.0;
        if r > 150 && g < 110 && b < 110 {
            red += 1;
        }
        if g > 120 && r < 110 && b < 110 {
            green += 1;
        }
    }
    assert!(red > 50 && green > 50, "red {red}, green {green}");
    assert_eq!(doc::page_count(&doc::load(&dir.join("diff.pdf"), None).unwrap()), 2);
    assert_eq!(outcome.outputs.len(), 5);

    let quick = compare::summary(&a, &b, None).unwrap();
    assert_eq!(quick.changed_pages, vec![2, 4]);
    assert!(!quick.identical);
    assert_eq!((quick.added_lines, quick.removed_lines), (2, 1));
}

#[test]
fn compare_calls_a_file_identical_to_itself() {
    let dir = out_dir("compare-same");
    let a = fx().join("report.pdf");
    let outcome = compare::run(&[a.clone(), a.clone()], &dir, &compare::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.notes, vec!["The two files are identical.".to_string()]);
    let report = read_report(&dir);
    assert_eq!(report["identical"], true);
    assert!(report["pages"].as_array().unwrap().iter().all(|p| p["text_changed"] == false && p["visual_changed"] == false));
    assert!(!dir.join("diff.pdf").exists());
    assert_eq!(outcome.outputs.len(), 2);
    let quick = compare::summary(&a, &a, None).unwrap();
    assert!(quick.identical && quick.changed_pages.is_empty());
}

#[test]
fn compare_reports_extra_pages_and_honours_mode_and_range() {
    let dir = out_dir("compare-pages");
    // The same report with its last two pages removed.
    let mut short = doc::load(&fx().join("report.pdf"), None).unwrap();
    let ids = doc::page_ids(&short);
    doc::set_page_order(&mut short, &ids[..4]).unwrap();
    let short_path = dir.join("short.pdf");
    doc::save(&mut short, &short_path).unwrap();

    let removed = dir.join("removed");
    let outcome = compare::run(&[fx().join("report.pdf"), short_path.clone()], &removed, &compare::Options::default(), &Ctx::none()).unwrap();
    let report = read_report(&removed);
    assert_eq!((report["pages_a"].as_u64(), report["pages_b"].as_u64()), (Some(6), Some(4)));
    assert_eq!(report["identical"], false);
    let presence: Vec<&str> = report["pages"].as_array().unwrap().iter().map(|p| p["presence"].as_str().unwrap()).collect();
    assert_eq!(presence, ["both", "both", "both", "both", "removed", "removed"]);
    assert!(report["pages"][4]["removed"].as_array().unwrap().iter().any(|l| l == "MARK-REPORT-P5"));
    assert!(outcome.notes[0].contains("5-6"), "{:?}", outcome.notes);

    let added = dir.join("added");
    compare::run(&[short_path, fx().join("report.pdf")], &added, &compare::Options::default(), &Ctx::none()).unwrap();
    let report = read_report(&added);
    assert_eq!(report["pages"][5]["presence"], "added");
    assert!(added.join("diff-p6.png").exists());

    // Text mode writes no pictures; a range limits what is looked at.
    let text_only = dir.join("text");
    let opts: compare::Options = serde_json::from_value(serde_json::json!({ "mode": "text" })).unwrap();
    compare::run(&[fx().join("report.pdf"), fx().join("report-v2.pdf")], &text_only, &opts, &Ctx::none()).unwrap();
    assert!(!text_only.join("diff-p2.png").exists() && !text_only.join("diff.pdf").exists());
    let report = read_report(&text_only);
    assert_eq!(report["pages"][1]["text_changed"], true);
    assert_eq!(report["pages"][1]["visual_changed"], false);

    let ranged = dir.join("ranged");
    let opts = compare::Options { pages: "1, 3, 5-6".into(), ..Default::default() };
    let outcome = compare::run(&[fx().join("report.pdf"), fx().join("report-v2.pdf")], &ranged, &opts, &Ctx::none()).unwrap();
    assert_eq!(read_report(&ranged)["identical"], true);
    assert_eq!(outcome.pages, 4);

    // Visual mode sees the change without reading any text.
    let visual = dir.join("visual");
    let opts = compare::Options { mode: compare::Mode::Visual, pages: "2".into(), ..Default::default() };
    compare::run(&[fx().join("report.pdf"), fx().join("report-v2.pdf")], &visual, &opts, &Ctx::none()).unwrap();
    let report = read_report(&visual);
    assert_eq!(report["pages"][0]["visual_changed"], true);
    assert_eq!(report["pages"][0]["text_changed"], false);

    let one = [fx().join("report.pdf")];
    assert!(matches!(compare::run(&one, &dir.join("bad"), &compare::Options::default(), &Ctx::none()), Err(Error::Invalid(_))));
    let bad_range = compare::Options { pages: "9".into(), ..Default::default() };
    assert!(matches!(compare::run(&[fx().join("report.pdf"), fx().join("report-v2.pdf")], &dir.join("bad"), &bad_range, &Ctx::none()), Err(Error::Invalid(_))));
}

// ---------------------------------------------------------------------------
// OCR
// ---------------------------------------------------------------------------

/// An engine that "reads" the same two words on every page, to test the text layer
/// on machines with no OCR engine at all.
struct FixedEngine;

impl ocr::OcrEngine for FixedEngine {
    fn recognise(&self, _: &RgbaImage) -> shorui_core::Result<Vec<ocr::OcrWord>> {
        Ok(vec![
            ocr::OcrWord { text: "Zebra".into(), x: 200.0, y: 300.0, w: 150.0, h: 36.0, line: 0 },
            ocr::OcrWord { text: "Crossing".into(), x: 370.0, y: 300.0, w: 220.0, h: 46.0, line: 0 },
        ])
    }
    fn describe(&self) -> String {
        "a fixed test engine".into()
    }
}

#[test]
fn ocr_text_layer_lands_where_the_words_are_on_rotated_pages_too() {
    let dir = out_dir("ocr-layer");
    let input = fx().join("rotated.pdf");
    let bytes = std::fs::read(&input).unwrap();
    let mut d = doc::load_bytes(&bytes, None).unwrap();
    let reader = TextReader::open(bytes.clone(), None).unwrap();
    let renderer = Renderer::open(bytes, None).unwrap();
    // 144 dpi: two pixels per point.
    let opts = ocr::Options { dpi: 144, skip_text_pages: false, ..Default::default() };
    let notes = ocr::run_with_engine(&FixedEngine, &mut d, &reader, &renderer, &[1, 2, 3, 4], &opts, &Ctx::none()).unwrap();
    assert!(notes[0].contains("4 pages") && notes[0].contains("8 words"), "{notes:?}");
    let out = dir.join("layer.pdf");
    doc::save(&mut d, &out).unwrap();

    let before = Renderer::open_path(&input, None).unwrap();
    let after_render = Renderer::open_path(&out, None).unwrap();
    let after = TextReader::open_path(&out, None).unwrap();
    for index in 0..4 {
        let page = after.page(index).unwrap();
        assert!(page.plain().contains("Zebra Crossing"), "page {}: {}", index + 1, page.plain());
        assert!(page.plain().contains(&format!("MARK-ROTATED-P{}", index + 1)));
        // The word box was 200..350 px across and 300..336 px down: 100..175 pt and 150..168 pt.
        let hits = page.find("Zebra", true, true);
        assert_eq!(hits.len(), 1, "page {}", index + 1);
        let r = hits[0];
        assert!((r.x0 - 100.0).abs() < 2.0 && (r.x1 - 175.0).abs() < 4.0, "page {}: {r:?}", index + 1);
        assert!(r.y0 < 160.0 && r.y1 > 160.0 && r.y0 > 135.0 && r.y1 < 185.0, "page {}: {r:?}", index + 1);
        // Invisible means invisible.
        let diff = mean_abs_diff(&before.render(index, 1.0).unwrap(), &after_render.render(index, 1.0).unwrap());
        assert!(diff < 0.01, "page {} looks different: {diff}", index + 1);
    }
}

#[test]
fn ocr_reads_a_scan_with_the_engine_on_this_machine() {
    let Some(engine) = ocr::engine_name() else {
        println!("SKIPPED: no OCR engine on this machine (no system engine and no tesseract on PATH).");
        return;
    };
    println!("OCR engine: {engine}; languages: {:?}", ocr::available_languages());
    assert!(!ocr::available_languages().is_empty());
    let dir = out_dir("ocr-scan");
    let input = fx().join("scan.pdf");
    assert!(page_texts(&input).iter().all(|t| t.trim().is_empty()), "the scan fixture must have no text layer");
    let out = dir.join("scan-ocr.pdf");
    let outcome = ocr::run(&[input.clone()], &out, &ocr::Options::default(), &Ctx::none()).unwrap();
    println!("notes: {:?}", outcome.notes);
    assert_eq!(outcome.pages, 2);

    let reader = TextReader::open_path(&out, None).unwrap();
    for index in 0..2 {
        let page = reader.page(index).unwrap();
        let text = page.plain();
        let mark = text.lines().find(|l| l.contains("MARK") || l.contains("SCAN")).unwrap_or("(no MARK line recognised)");
        println!("page {}: MARK line recognised as \"{mark}\"; first lines: {:?}", index + 1, text.lines().take(3).collect::<Vec<_>>());
        assert!(text.contains("Delivery Note"), "page {}: {text}", index + 1);
        assert!(text.contains("Customer support"), "page {}: {text}", index + 1);
        // The title was drawn at x = 56 pt with its baseline 72 pt from the top, 22 pt tall.
        let hits = page.find("Delivery Note", true, false);
        assert!(!hits.is_empty());
        let r = hits[0];
        assert!((r.x0 - 56.0).abs() < 4.0 && r.y0 < 66.0 && r.y1 > 66.0 && r.y0 > 40.0 && r.y1 < 90.0, "title at {r:?}");
        assert!((r.width() - 140.0).abs() < 12.0, "title width {}", r.width());
    }
    // The pages look the same as before.
    let diff = mean_abs_diff(&Renderer::open_path(&input, None).unwrap().render(0, 1.0).unwrap(), &Renderer::open_path(&out, None).unwrap().render(0, 1.0).unwrap());
    assert!(diff < 0.01, "{diff}");
}

#[test]
fn ocr_leaves_pages_with_text_alone() {
    let Some(engine) = ocr::engine_name() else {
        println!("SKIPPED: no OCR engine on this machine (no system engine and no tesseract on PATH).");
        return;
    };
    let dir = out_dir("ocr-mixed");
    // Page 1 is a scan, page 2 has real text.
    let scan = doc::load(&fx().join("scan.pdf"), None).unwrap();
    let report = doc::load(&fx().join("report.pdf"), None).unwrap();
    let (mut mixed, root) = doc::new_document();
    let a = doc::import_pages(&mut mixed, &scan, &doc::page_ids(&scan)[..1]).unwrap();
    let b = doc::import_pages(&mut mixed, &report, &doc::page_ids(&report)[..1]).unwrap();
    doc::append_pages(&mut mixed, root, &[a[0], b[0]]).unwrap();
    let input = dir.join("mixed.pdf");
    doc::save(&mut mixed, &input).unwrap();

    let out = dir.join("mixed-ocr.pdf");
    let outcome = ocr::run(&[input.clone()], &out, &ocr::Options::default(), &Ctx::none()).unwrap();
    println!("{engine}: {:?}", outcome.notes);
    assert!(outcome.notes.iter().any(|n| n.contains("1 page already had text and was left alone")), "{:?}", outcome.notes);

    let before = doc::load(&input, None).unwrap();
    let after = doc::load(&out, None).unwrap();
    let (before_ids, after_ids) = (doc::page_ids(&before), doc::page_ids(&after));
    assert_eq!(before.get_page_content(before_ids[1]), after.get_page_content(after_ids[1]), "the text page's content must be byte-identical");
    assert_eq!(before.get_page_contents(before_ids[1]).len(), after.get_page_contents(after_ids[1]).len());
    assert_eq!(before.get_dictionary(before_ids[1]).unwrap().get(b"Resources").unwrap(), after.get_dictionary(after_ids[1]).unwrap().get(b"Resources").unwrap());
    assert_ne!(before.get_page_content(before_ids[0]), after.get_page_content(after_ids[0]));
    let texts = page_texts(&out);
    assert!(texts[0].contains("Delivery Note"));
    assert!(texts[1].contains("MARK-REPORT-P1"));

    // A document that is all text gets nothing added.
    let untouched = dir.join("report-ocr.pdf");
    let outcome = ocr::run(&[fx().join("report.pdf")], &untouched, &ocr::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.notes[0], "No text layer was added.");
    assert_eq!(page_texts(&untouched), page_texts(&fx().join("report.pdf")));

    // Running OCR on its own output adds nothing either: the layer counts as text.
    let again = dir.join("again.pdf");
    let outcome = ocr::run(&[out], &again, &ocr::Options::default(), &Ctx::none()).unwrap();
    assert_eq!(outcome.notes[0], "No text layer was added.");
}

#[test]
fn ocr_explains_missing_engines_and_languages() {
    let dir = out_dir("ocr-errors");
    let out = dir.join("x.pdf");
    let scan = [fx().join("scan.pdf")];
    if shorui_core::helpers::find_tesseract().is_none() {
        let opts = ocr::Options { engine: ocr::Engine::Tesseract, ..Default::default() };
        match ocr::run(&scan, &out, &opts, &Ctx::none()) {
            Err(e @ Error::MissingHelper { .. }) => println!("tesseract asked for but absent: {e}"),
            other => panic!("expected MissingHelper, got {other:?}"),
        }
    } else {
        println!("tesseract is installed; the missing-helper path was not exercised.");
    }
    if ocr::engine_name().is_some() {
        let opts = ocr::Options { language: "tlh-QO".into(), ..Default::default() };
        let result = ocr::run(&scan, &out, &opts, &Ctx::none());
        println!("unknown language: {:?}", result.as_ref().err().map(|e| e.to_string()));
        assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
    }
    if !cfg!(windows) {
        let opts = ocr::Options { engine: ocr::Engine::System, ..Default::default() };
        assert!(matches!(ocr::run(&scan, &out, &opts, &Ctx::none()), Err(Error::Unsupported(_))));
    }
    let bad = ocr::Options { dpi: 5, ..Default::default() };
    assert!(matches!(ocr::run(&scan, &out, &bad, &Ctx::none()), Err(Error::Invalid(_))));
    assert!(!out.exists());
    let parsed: ocr::Options = serde_json::from_str(r#"{"language":"en-US","engine":"tesseract","skip_text_pages":false}"#).unwrap();
    assert_eq!((parsed.engine, parsed.dpi, parsed.skip_text_pages), (ocr::Engine::Tesseract, 200, false));
}
