#![cfg(feature = "set-organise")]
//! Tests for the organise set: merge, split, pages, crop, nup, protect, unlock, strip.
//! Fixtures are generated once into cargo's test temp folder; every test writes its
//! outputs into its own folder there and reads them back.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use shorui_core::text::{Rect4, TextReader};
use shorui_core::tools::{crop, merge, nup, pages, protect, split, strip, unlock};
use shorui_core::{Ctx, Error, doc, fixtures};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
}

/// The fixture folder, written once per test run.
fn fx(name: &str) -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tmp().join("organise-fixtures");
        let _ = std::fs::remove_dir_all(&dir);
        fixtures::write_all(&dir).expect("fixtures");
        dir
    })
    .join(name)
}

/// A clean output folder for one test.
fn out_dir(test: &str) -> PathBuf {
    let dir = tmp().join("organise-out").join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("output folder");
    dir
}

fn ctx() -> Ctx<'static> {
    Ctx::none()
}

/// The `MARK-...` tokens on every page, in reading order.
fn marks_with(path: &Path, password: Option<&str>) -> Vec<Vec<String>> {
    let reader = TextReader::open_path(path, password).expect("open for text");
    (0..reader.page_count())
        .map(|i| reader.page(i).expect("page text").plain().split_whitespace().filter(|w| w.starts_with("MARK-")).map(String::from).collect())
        .collect()
}

fn marks(path: &Path) -> Vec<Vec<String>> {
    marks_with(path, None)
}

fn flat_marks(path: &Path) -> Vec<String> {
    marks(path).into_iter().flatten().collect()
}

fn report_marks(pages: &[usize]) -> Vec<String> {
    pages.iter().map(|p| format!("MARK-REPORT-P{p}")).collect()
}

/// Where a piece of text sits on a page, in display space (origin top-left, y down).
fn locate(path: &Path, page: usize, needle: &str) -> Rect4 {
    let reader = TextReader::open_path(path, None).expect("open for text");
    let hits = reader.page(page).expect("page text").find(needle, true, false);
    *hits.first().unwrap_or_else(|| panic!("{needle} not found on page {} of {}", page + 1, path.display()))
}

fn sizes(path: &Path) -> Vec<(f32, f32, i32)> {
    let d = doc::load(path, None).expect("load");
    doc::page_ids(&d).into_iter().map(|id| { let (w, h) = doc::visible_size(&d, id); (w, h, doc::rotation(&d, id)) }).collect()
}

fn near(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

fn catalog(d: &Document) -> &Dictionary {
    d.catalog().expect("catalog")
}

fn resolve<'a>(d: &'a Document, o: &'a Object) -> &'a Object {
    doc::deref(d, o)
}

fn text_of(o: &Object) -> String {
    lopdf::decode_text_string(o).expect("text string")
}

/// Top-level bookmarks as (title, 1-based page number or 0 when the target is not a page).
fn bookmarks(path: &Path) -> Vec<(String, usize)> {
    let d = doc::load(path, None).expect("load");
    let ids = doc::page_ids(&d);
    let mut out = Vec::new();
    let Ok(root) = catalog(&d).get(b"Outlines").and_then(Object::as_reference) else { return out };
    let mut next = d.get_dictionary(root).unwrap().get(b"First").and_then(Object::as_reference).ok();
    while let Some(id) = next {
        let item = d.get_dictionary(id).unwrap();
        assert_eq!(item.get(b"Parent").and_then(Object::as_reference).unwrap(), root, "bookmark parent");
        let page = item
            .get(b"Dest")
            .ok()
            .map(|o| resolve(&d, o))
            .and_then(|o| o.as_array().ok())
            .and_then(|a| a.first())
            .and_then(|o| o.as_reference().ok())
            .and_then(|target| ids.iter().position(|p| *p == target))
            .map(|i| i + 1)
            .unwrap_or(0);
        out.push((text_of(item.get(b"Title").unwrap()), page));
        next = item.get(b"Next").and_then(Object::as_reference).ok();
    }
    let count = d.get_dictionary(root).unwrap().get(b"Count").and_then(Object::as_i64).unwrap();
    assert_eq!(count as usize, out.len(), "outline count");
    out
}

/// Names of the top-level form fields.
fn field_names(d: &Document) -> Vec<String> {
    let Ok(form) = catalog(d).get(b"AcroForm") else { return Vec::new() };
    let form = resolve(d, form).as_dict().expect("form dictionary");
    let fields = resolve(d, form.get(b"Fields").expect("fields")).as_array().expect("fields array");
    fields.iter().map(|f| text_of(d.get_dictionary(f.as_reference().unwrap()).unwrap().get(b"T").unwrap())).collect()
}

fn annotation_subtypes(d: &Document, page: ObjectId) -> Vec<String> {
    match d.get_dictionary(page).unwrap().get(b"Annots").map(|a| resolve(d, a)) {
        Ok(Object::Array(items)) => items
            .iter()
            .map(|a| String::from_utf8_lossy(resolve(d, a).as_dict().unwrap().get(b"Subtype").unwrap().as_name().unwrap()).into_owned())
            .collect(),
        _ => Vec::new(),
    }
}

fn info_string(d: &Document, key: &str) -> Option<String> {
    let info = resolve(d, d.trailer.get(b"Info").ok()?).as_dict().ok()?;
    info.get(key.as_bytes()).ok().map(|o| text_of(resolve(d, o)))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn expect_invalid<T: std::fmt::Debug>(result: shorui_core::Result<T>, part: &str) {
    match result {
        Err(Error::Invalid(message)) => assert!(message.contains(part), "message was: {message}"),
        other => panic!("expected an invalid-input error mentioning {part:?}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// merge
// ---------------------------------------------------------------------------

#[test]
fn merge_keeps_every_page_in_order_with_a_bookmark_per_file() {
    let dir = out_dir("merge_basic");
    let inputs = [fx("report.pdf"), fx("letter.pdf"), fx("appendix.pdf")];
    let out = dir.join("merged.pdf");
    let outcome = merge::run(&inputs, &out, &merge::Options::default(), &ctx()).unwrap();
    assert_eq!(outcome.pages, 10);
    assert_eq!(outcome.outputs, vec![out.clone()]);
    assert_eq!(outcome.bytes_in, merge::estimate_size(&inputs));
    assert_eq!(outcome.bytes_out, std::fs::metadata(&out).unwrap().len());

    let mut expected = report_marks(&[1, 2, 3, 4, 5, 6]);
    expected.push("MARK-LETTER-P1".into());
    expected.extend((1..=3).map(|p| format!("MARK-APPENDIX-P{p}")));
    let got = marks(&out);
    assert_eq!(got.len(), 10);
    assert!(got.iter().all(|page| page.len() == 1), "one mark per page: {got:?}");
    assert_eq!(got.into_iter().flatten().collect::<Vec<_>>(), expected);
    assert_eq!(bookmarks(&out), vec![("report".to_string(), 1), ("letter".to_string(), 7), ("appendix".to_string(), 8)]);
    // The inputs are untouched.
    assert_eq!(flat_marks(&inputs[0]), report_marks(&[1, 2, 3, 4, 5, 6]));
}

#[test]
fn merge_ranges_contents_page_and_odd_starts() {
    let dir = out_dir("merge_layout");
    let out = dir.join("merged.pdf");
    let opts = merge::Options {
        ranges: vec!["1-2".into(), String::new(), "3, 1".into()],
        contents_page: true,
        start_on_odd: true,
        title: Some("Board pack".into()),
        ..Default::default()
    };
    let outcome = merge::run(&[fx("report.pdf"), fx("letter.pdf"), fx("appendix.pdf")], &out, &opts, &ctx()).unwrap();
    // contents, blank, report 1-2, letter, blank, appendix 3 and 1
    assert_eq!(outcome.pages, 8);
    assert!(outcome.notes.iter().any(|n| n.contains("2 blank pages")), "{:?}", outcome.notes);
    let got = marks(&out);
    let expected: Vec<Vec<&str>> =
        vec![vec![], vec![], vec!["MARK-REPORT-P1"], vec!["MARK-REPORT-P2"], vec!["MARK-LETTER-P1"], vec![], vec!["MARK-APPENDIX-P3"], vec!["MARK-APPENDIX-P1"]];
    assert_eq!(got, expected);

    let reader = TextReader::open_path(&out, None).unwrap();
    let contents = reader.page(0).unwrap();
    let lines: Vec<String> = contents.lines.iter().map(|l| l.text()).collect();
    assert!(lines.iter().any(|l| l == "Contents"), "{lines:?}");
    for (name, start) in [("report", "3"), ("letter", "5"), ("appendix", "7")] {
        let line = lines.iter().find(|l| l.starts_with(name)).unwrap_or_else(|| panic!("no contents line for {name}: {lines:?}"));
        assert!(line.trim_end().ends_with(start), "{line}");
    }
    assert!(reader.page(1).unwrap().is_empty(), "page 2 is the blank that makes the report start on page 3");

    assert_eq!(bookmarks(&out), vec![("Contents".to_string(), 1), ("report".to_string(), 3), ("letter".to_string(), 5), ("appendix".to_string(), 7)]);
    let d = doc::load(&out, None).unwrap();
    assert_eq!(info_string(&d, "Title").as_deref(), Some("Board pack"));
    // Each contents line links to its file.
    let first = doc::page_ids(&d)[0];
    assert_eq!(annotation_subtypes(&d, first), vec!["Link", "Link", "Link"]);
}

#[test]
fn merge_takes_title_and_author_from_the_first_file_unless_told_not_to() {
    let dir = out_dir("merge_metadata");
    let inputs = [fx("metadata.pdf"), fx("letter.pdf")];
    let with = dir.join("with.pdf");
    merge::run(&inputs, &with, &merge::Options::default(), &ctx()).unwrap();
    let d = doc::load(&with, None).unwrap();
    assert_eq!(info_string(&d, "Title").as_deref(), Some("Board Minutes, confidential draft"));
    assert_eq!(info_string(&d, "Author").as_deref(), Some("Priya Raman"));
    assert_eq!(info_string(&d, "Producer").as_deref(), Some("Shorui"));

    let without = dir.join("without.pdf");
    let opts = merge::Options { metadata_from_first: false, bookmarks: merge::Bookmarks::None, ..Default::default() };
    merge::run(&inputs, &without, &opts, &ctx()).unwrap();
    let d = doc::load(&without, None).unwrap();
    assert_eq!(info_string(&d, "Title"), None);
    assert_eq!(info_string(&d, "Author"), None);
    assert!(bookmarks(&without).is_empty());
    assert!(!catalog(&d).has(b"Outlines"));

    // A non-Latin title survives as Unicode.
    let unicode = dir.join("unicode.pdf");
    let opts = merge::Options { title: Some("報告書 2026".into()), ..Default::default() };
    merge::run(&inputs, &unicode, &opts, &ctx()).unwrap();
    assert_eq!(info_string(&doc::load(&unicode, None).unwrap(), "Title").as_deref(), Some("報告書 2026"));
}

#[test]
fn merge_builds_one_working_form_and_renames_clashing_fields() {
    let dir = out_dir("merge_forms");
    let out = dir.join("forms.pdf");
    let inputs = [fx("form.pdf"), fx("report.pdf"), fx("form.pdf")];
    let opts = merge::Options { ranges: vec![String::new(), "1".into()], ..Default::default() };
    let outcome = merge::run(&inputs, &out, &opts, &ctx()).unwrap();
    assert_eq!(outcome.pages, 3);
    assert!(outcome.notes.iter().any(|n| n.contains("3 form fields") && n.contains("renamed")), "{:?}", outcome.notes);

    let d = doc::load(&out, None).unwrap();
    assert_eq!(field_names(&d), vec!["full_name", "department", "agree", "full_name_2", "department_2", "agree_2"]);
    let form = resolve(&d, catalog(&d).get(b"AcroForm").unwrap()).as_dict().unwrap();
    assert!(form.has(b"DA"), "default appearance carried over");
    let fonts = resolve(&d, form.get(b"DR").unwrap()).as_dict().unwrap().get(b"Font").unwrap().as_dict().unwrap();
    let helv = fonts.get(b"Helv").unwrap().as_reference().unwrap();
    assert!(d.get_dictionary(helv).is_ok(), "the form's font resource exists in the merged file");

    // Every field in the form is a widget on the right page, and points back at it.
    let ids = doc::page_ids(&d);
    let fields = resolve(&d, form.get(b"Fields").unwrap()).as_array().unwrap().clone();
    for (i, field) in fields.iter().enumerate() {
        let id = field.as_reference().unwrap();
        let page = if i < 3 { ids[0] } else { ids[2] };
        assert_eq!(d.get_dictionary(id).unwrap().get(b"P").and_then(Object::as_reference).unwrap(), page);
        let on_page = d.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap();
        assert!(on_page.contains(field));
    }
    assert_eq!(annotation_subtypes(&d, ids[0]), vec!["Widget", "Widget", "Widget", "FreeText"]);

    // Without form fields: no form, no widgets, but the note annotation stays.
    let plain = dir.join("plain.pdf");
    let opts = merge::Options { keep_form_fields: false, ..Default::default() };
    let outcome = merge::run(&[fx("form.pdf"), fx("letter.pdf")], &plain, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("3 form fields were left out")), "{:?}", outcome.notes);
    let d = doc::load(&plain, None).unwrap();
    assert!(!catalog(&d).has(b"AcroForm"));
    assert_eq!(annotation_subtypes(&d, doc::page_ids(&d)[0]), vec!["FreeText"]);
    assert!(!contains(&std::fs::read(&plain).unwrap(), b"full_name"));
}

#[test]
fn merge_can_keep_the_bookmarks_files_already_have() {
    let dir = out_dir("merge_keep");
    let first = dir.join("first.pdf");
    merge::run(&[fx("report.pdf"), fx("letter.pdf")], &first, &merge::Options::default(), &ctx()).unwrap();
    assert_eq!(bookmarks(&first), vec![("report".to_string(), 1), ("letter".to_string(), 7)]);

    let keep = merge::Options { bookmarks: merge::Bookmarks::Keep, ..Default::default() };
    let second = dir.join("second.pdf");
    merge::run(&[fx("appendix.pdf"), first.clone()], &second, &keep, &ctx()).unwrap();
    assert_eq!(bookmarks(&second), vec![("report".to_string(), 4), ("letter".to_string(), 10)]);

    // A bookmark whose page is not taken along is dropped.
    let third = dir.join("third.pdf");
    let opts = merge::Options { ranges: vec![String::new(), "6-7".into()], ..keep };
    merge::run(&[fx("appendix.pdf"), first], &third, &opts, &ctx()).unwrap();
    assert_eq!(bookmarks(&third), vec![("letter".to_string(), 5)]);
    assert_eq!(flat_marks(&third)[3..], ["MARK-REPORT-P6".to_string(), "MARK-LETTER-P1".to_string()]);
}

#[test]
fn merge_refuses_bad_input() {
    let dir = out_dir("merge_errors");
    expect_invalid(merge::run(&[], &dir.join("a.pdf"), &merge::Options::default(), &ctx()), "at least one");
    let opts = merge::Options { ranges: vec![String::new(), "4".into()], ..Default::default() };
    expect_invalid(merge::run(&[fx("report.pdf"), fx("letter.pdf")], &dir.join("b.pdf"), &opts, &ctx()), "letter.pdf");
    assert!(!dir.join("b.pdf").exists());
    // Writing over an input is refused and the input stays as it was.
    let copy = dir.join("copy.pdf");
    std::fs::copy(fx("letter.pdf"), &copy).unwrap();
    expect_invalid(merge::run(&[fx("report.pdf"), copy.clone()], &copy, &merge::Options::default(), &ctx()), "overwrite");
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(fx("letter.pdf")).unwrap());
    // A damaged file is named.
    match merge::run(&[fx("report.pdf"), fx("broken.pdf")], &dir.join("c.pdf"), &merge::Options::default(), &ctx()) {
        Ok(_) => {} // lopdf may recover the file; either outcome is acceptable
        Err(Error::Damaged(m)) => assert!(m.contains("broken.pdf"), "{m}"),
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn merge_reports_cancellation() {
    let dir = out_dir("merge_cancel");
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let progress = |_: f32, _: &str| {};
    let ctx = Ctx::new(&progress, &cancel);
    let result = merge::run(&[fx("report.pdf")], &dir.join("a.pdf"), &merge::Options::default(), &ctx);
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(!dir.join("a.pdf").exists());
}

// ---------------------------------------------------------------------------
// split
// ---------------------------------------------------------------------------

#[test]
fn split_each_page_into_its_own_file() {
    let dir = out_dir("split_each");
    let outcome = split::run(&[fx("report.pdf")], &dir, &split::Options::default(), &ctx()).unwrap();
    assert_eq!(outcome.outputs.len(), 6);
    assert_eq!(outcome.pages, 6);
    for (i, path) in outcome.outputs.iter().enumerate() {
        assert_eq!(path, &dir.join(format!("report-{}.pdf", i + 1)));
        assert_eq!(marks(path), vec![vec![format!("MARK-REPORT-P{}", i + 1)]]);
    }
    assert_eq!(outcome.bytes_out, outcome.outputs.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum::<u64>());
}

#[test]
fn split_by_ranges_and_by_count() {
    let dir = out_dir("split_ranges");
    let opts = split::Options { mode: split::Mode::Ranges, ranges: "1-3; 5-".into(), name_pattern: "{name} p{range}".into(), ..Default::default() };
    let outcome = split::run(&[fx("report.pdf")], &dir, &opts, &ctx()).unwrap();
    assert_eq!(outcome.outputs, vec![dir.join("report p1-3.pdf"), dir.join("report p5-6.pdf")]);
    assert_eq!(flat_marks(&outcome.outputs[0]), report_marks(&[1, 2, 3]));
    assert_eq!(flat_marks(&outcome.outputs[1]), report_marks(&[5, 6]));
    assert_eq!(outcome.pages, 5);
    assert!(outcome.notes.iter().any(|n| n == "Page 4 is not in any part."), "{:?}", outcome.notes);

    let dir = out_dir("split_every");
    let opts = split::Options { mode: split::Mode::Every, every: 4, ..Default::default() };
    let outcome = split::run(&[fx("report.pdf")], &dir, &opts, &ctx()).unwrap();
    assert_eq!(outcome.outputs.len(), 2);
    assert_eq!(flat_marks(&outcome.outputs[0]), report_marks(&[1, 2, 3, 4]));
    assert_eq!(flat_marks(&outcome.outputs[1]), report_marks(&[5, 6]));

    // Twelve parts are numbered 01..12; a pattern with no number still gives distinct names.
    let twelve: Vec<Vec<usize>> = (1..=12).map(|p| vec![p]).collect();
    let names = split::part_names(&split::Options::default(), Path::new("x/big file.pdf"), &twelve);
    assert_eq!(names[0], "big file-01.pdf");
    assert_eq!(names[11], "big file-12.pdf");
    let names = split::part_names(&split::Options { name_pattern: "part: {name}".into(), ..Default::default() }, Path::new("a.pdf"), &twelve[..2]);
    assert_eq!(names, vec!["part_ a-1.pdf", "part_ a-2.pdf"]);

    expect_invalid(split::run(&[fx("report.pdf")], &dir, &split::Options { mode: split::Mode::Ranges, ..Default::default() }, &ctx()), "ranges");
    expect_invalid(split::run(&[fx("report.pdf")], &dir, &split::Options { mode: split::Mode::Ranges, ranges: "1-2; 9".into(), ..Default::default() }, &ctx()), "Page 9");
    expect_invalid(split::run(&[fx("report.pdf")], &dir, &split::Options { mode: split::Mode::Every, every: 0, ..Default::default() }, &ctx()), "at least 1");
    expect_invalid(split::run(&[fx("report.pdf"), fx("letter.pdf")], &dir, &split::Options::default(), &ctx()), "one PDF");
}

#[test]
fn split_parts_carry_only_what_they_need() {
    // Each photo page is about a third of the file; a part must not drag the others along.
    let dir = out_dir("split_lean");
    let whole = std::fs::metadata(fx("photos.pdf")).unwrap().len();
    let outcome = split::run(&[fx("photos.pdf")], &dir, &split::Options::default(), &ctx()).unwrap();
    assert_eq!(outcome.outputs.len(), 3);
    for path in &outcome.outputs {
        let size = std::fs::metadata(path).unwrap().len();
        assert!(size < whole / 2, "{} is {size} bytes of a {whole} byte original", path.display());
        assert!(size > whole / 6);
    }

    // A part with form fields still has a working form; bookmarks follow their pages.
    let dir = out_dir("split_form");
    let merged = dir.join("merged.pdf");
    merge::run(&[fx("report.pdf"), fx("form.pdf")], &merged, &merge::Options::default(), &ctx()).unwrap();
    let opts = split::Options { mode: split::Mode::Ranges, ranges: "1-6; 7".into(), ..Default::default() };
    let outcome = split::run(&[merged.clone()], &dir.join("parts"), &opts, &ctx()).unwrap();
    let first = doc::load(&outcome.outputs[0], None).unwrap();
    assert!(!catalog(&first).has(b"AcroForm"));
    assert!(!contains(&std::fs::read(&outcome.outputs[0]).unwrap(), b"full_name"));
    assert_eq!(bookmarks(&outcome.outputs[0]), vec![("report".to_string(), 1)]);
    let second = doc::load(&outcome.outputs[1], None).unwrap();
    assert_eq!(field_names(&second), vec!["full_name", "department", "agree"]);
    assert_eq!(bookmarks(&outcome.outputs[1]), vec![("form".to_string(), 1)]);
    assert_eq!(flat_marks(&outcome.outputs[1]), vec!["MARK-FORM-P1"]);

    // A part is never written over the input, even when the pattern produces its name.
    let opts = split::Options { mode: split::Mode::Every, every: 7, name_pattern: "{name}".into(), ..Default::default() };
    let before = std::fs::read(&merged).unwrap();
    let outcome = split::run(&[merged.clone()], &dir, &opts, &ctx()).unwrap();
    assert_ne!(outcome.outputs[0], merged);
    assert_eq!(std::fs::read(&merged).unwrap(), before);
}

// ---------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------

#[test]
fn pages_reorders_deletes_and_duplicates() {
    let dir = out_dir("pages_order");
    let run = |name: &str, opts: pages::Options| {
        let out = dir.join(name);
        let outcome = pages::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
        (out, outcome)
    };
    let (out, outcome) = run("order.pdf", pages::Options { order: "3, 1".into(), ..Default::default() });
    assert_eq!(flat_marks(&out), report_marks(&[3, 1]));
    assert_eq!(outcome.pages, 2);
    assert!(outcome.notes.iter().any(|n| n == "Removed 4 pages."), "{:?}", outcome.notes);

    let (out, _) = run("delete.pdf", pages::Options { delete: "2-5".into(), ..Default::default() });
    assert_eq!(flat_marks(&out), report_marks(&[1, 6]));

    let (out, _) = run("both.pdf", pages::Options { order: "6-1".into(), delete: "odd".into(), ..Default::default() });
    assert_eq!(flat_marks(&out), report_marks(&[6, 4, 2]));

    let (out, outcome) = run("twice.pdf", pages::Options { order: "1, 1, 2, 1".into(), ..Default::default() });
    assert_eq!(flat_marks(&out), report_marks(&[1, 1, 2, 1]));
    assert_eq!(outcome.pages, 4);

    let (out, outcome) = run("blank.pdf", pages::Options { insert_blank_after: vec![0, 2], ..Default::default() });
    assert_eq!(outcome.pages, 8);
    let got = marks(&out);
    assert!(got[0].is_empty() && got[3].is_empty(), "{got:?}");
    assert_eq!(got.into_iter().flatten().collect::<Vec<_>>(), report_marks(&[1, 2, 3, 4, 5, 6]));
    assert!(sizes(&out).iter().all(|s| near(s.0, 595.0, 0.1) && near(s.1, 842.0, 0.1)));

    let out = dir.join("none.pdf");
    expect_invalid(pages::run(&[fx("report.pdf")], &out, &pages::Options { delete: "1-6".into(), ..Default::default() }, &ctx()), "every page");
    assert!(!out.exists());
    expect_invalid(pages::run(&[fx("report.pdf")], &out, &pages::Options { order: "7".into(), ..Default::default() }, &ctx()), "Page 7");
    expect_invalid(pages::run(&[fx("report.pdf")], &out, &pages::Options { insert_blank_after: vec![9], ..Default::default() }, &ctx()), "after page 9");
}

#[test]
fn pages_rotation_adds_to_what_is_there() {
    let dir = out_dir("pages_rotate");
    let out = dir.join("turned.pdf");
    assert_eq!(sizes(&fx("rotated.pdf")).iter().map(|s| s.2).collect::<Vec<_>>(), vec![0, 90, 270, 0]);
    let opts = pages::Options { rotate: vec![pages::Rotate { pages: String::new(), degrees: 90 }], ..Default::default() };
    pages::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    let got = sizes(&out);
    assert_eq!(got.iter().map(|s| s.2).collect::<Vec<_>>(), vec![90, 180, 0, 90]);
    assert!(near(got[0].0, 842.0, 0.1) && near(got[0].1, 595.0, 0.1));
    assert!(near(got[1].0, 595.0, 0.1) && near(got[1].1, 842.0, 0.1));
    assert_eq!(flat_marks(&out), (1..=4).map(|p| format!("MARK-ROTATED-P{p}")).collect::<Vec<_>>());

    // Two rules on one page add up, negative turns left, and a copy turns with its original.
    let opts = pages::Options {
        order: "2, 2, 4".into(),
        rotate: vec![pages::Rotate { pages: "2".into(), degrees: -180 }, pages::Rotate { pages: "2, 4".into(), degrees: -90 }],
        ..Default::default()
    };
    pages::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(sizes(&out).iter().map(|s| s.2).collect::<Vec<_>>(), vec![180, 180, 270]);

    let opts = pages::Options { rotate: vec![pages::Rotate { pages: "1".into(), degrees: 45 }], ..Default::default() };
    expect_invalid(pages::run(&[fx("rotated.pdf")], &out, &opts, &ctx()), "90 degrees");
}

#[test]
fn pages_drops_fields_and_bookmarks_of_removed_pages() {
    let dir = out_dir("pages_tidy");
    let merged = dir.join("merged.pdf");
    merge::run(&[fx("form.pdf"), fx("report.pdf"), fx("letter.pdf")], &merged, &merge::Options::default(), &ctx()).unwrap();
    let out = dir.join("kept.pdf");
    pages::run(&[merged.clone()], &out, &pages::Options { order: "8, 2-3".into(), ..Default::default() }, &ctx()).unwrap();
    let d = doc::load(&out, None).unwrap();
    assert!(!catalog(&d).has(b"AcroForm"), "the form page is gone, so is the form");
    assert!(!contains(&std::fs::read(&out).unwrap(), b"full_name"));
    assert_eq!(bookmarks(&out), vec![("report".to_string(), 2), ("letter".to_string(), 1)]);

    // Keeping the form page keeps the form.
    pages::run(&[merged], &out, &pages::Options { delete: "2-8".into(), ..Default::default() }, &ctx()).unwrap();
    assert_eq!(field_names(&doc::load(&out, None).unwrap()), vec!["full_name", "department", "agree"]);
    assert_eq!(bookmarks(&out), vec![("form".to_string(), 1)]);
}

// ---------------------------------------------------------------------------
// crop
// ---------------------------------------------------------------------------

fn boxes(path: &Path) -> Vec<([f32; 4], [f32; 4])> {
    let d = doc::load(path, None).unwrap();
    doc::page_ids(&d).into_iter().map(|id| (doc::media_box(&d, id), doc::crop_box(&d, id))).collect()
}

fn same_box(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| near(*x, y, 0.05))
}

#[test]
fn crop_margins_follow_the_page_as_the_reader_sees_it() {
    let dir = out_dir("crop_margins");
    let out = dir.join("cropped.pdf");
    let opts = crop::Options { top: 100.0, right: 20.0, bottom: 10.0, left: 50.0, ..Default::default() };
    let outcome = crop::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("hidden, not deleted")), "{:?}", outcome.notes);
    let got = sizes(&out);
    assert!(near(got[0].0, 525.0, 0.1) && near(got[0].1, 732.0, 0.1), "{got:?}");
    for page in &got[1..] {
        assert!(near(page.0, 772.0, 0.1) && near(page.1, 485.0, 0.1), "{got:?}");
    }
    assert_eq!(got.iter().map(|s| s.2).collect::<Vec<_>>(), vec![0, 90, 270, 0]);
    let b = boxes(&out);
    assert!(same_box(b[0].1, [50.0, 10.0, 575.0, 742.0]));
    // Turned 90 degrees clockwise: the reader's top is the page's left edge, the reader's left its bottom.
    assert!(same_box(b[1].1, [100.0, 50.0, 585.0, 822.0]), "{:?}", b[1]);
    // Turned 270: the reader's top is the page's right edge, the reader's left its top.
    assert!(same_box(b[2].1, [10.0, 20.0, 495.0, 792.0]), "{:?}", b[2]);
    assert!(same_box(b[0].0, [0.0, 0.0, 595.0, 842.0]), "media box is kept");
    // The mark sits 98 points from the top of page 1; 100 points were trimmed, so it now
    // lies outside the visible page but is still in the file.
    assert_eq!(flat_marks(&out).len(), 4);

    // Only some pages, and with the media box cut as well.
    let opts = crop::Options { top: 30.0, pages: "4".into(), remove_hidden: true, ..Default::default() };
    let outcome = crop::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("still stored")), "{:?}", outcome.notes);
    let b = boxes(&out);
    assert!(same_box(b[3].0, [0.0, 0.0, 842.0, 565.0]) && same_box(b[3].1, b[3].0), "{:?}", b[3]);
    assert!(same_box(b[0].1, [0.0, 0.0, 595.0, 842.0]));
    // The text moved up with the new top edge.
    let before = locate(&fx("rotated.pdf"), 3, "MARK-ROTATED-P4");
    let after = locate(&out, 3, "MARK-ROTATED-P4");
    assert!(near(before.y0 - after.y0, 30.0, 0.5), "{before:?} {after:?}");

    // Negative margins add space by growing the page.
    let opts = crop::Options { top: -20.0, left: -10.0, pages: "1".into(), ..Default::default() };
    let outcome = crop::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
    let got = sizes(&out);
    assert!(near(got[0].0, 605.0, 0.1) && near(got[0].1, 862.0, 0.1), "{got:?}");
    assert!(same_box(boxes(&out)[0].0, [-10.0, 0.0, 595.0, 862.0]));
    let after = locate(&out, 0, "MARK-ROTATED-P1");
    let before = locate(&fx("rotated.pdf"), 0, "MARK-ROTATED-P1");
    assert!(near(after.x0 - before.x0, 10.0, 0.5) && near(after.y0 - before.y0, 20.0, 0.5));

    let opts = crop::Options { top: 500.0, bottom: 400.0, ..Default::default() };
    expect_invalid(crop::run(&[fx("report.pdf")], &dir.join("no.pdf"), &opts, &ctx()), "Nothing would be left");
    assert!(!dir.join("no.pdf").exists());
}

#[test]
fn crop_auto_trims_to_the_content() {
    let dir = out_dir("crop_auto");
    let out = dir.join("auto.pdf");
    let opts = crop::Options { mode: crop::Mode::Auto, auto_padding: 12.0, ..Default::default() };
    crop::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    // Text runs from x=56 to the end of the rule at 539, and from the footer near y=60 to the title near y=790.
    for (w, h, _) in sizes(&out) {
        assert!(w > 483.0 + 20.0 && w < 483.0 + 30.0, "width {w}");
        assert!(h > 700.0 && h < 780.0, "height {h}");
    }
    // What is left on each page reaches to within the padding of every edge.
    let renderer = shorui_core::render::Renderer::open_path(&out, None).unwrap();
    for i in 0..renderer.page_count() {
        let (w, h) = renderer.page_size(i).unwrap();
        let found = crop::detect_content_box(&renderer, i).unwrap().expect("content");
        for (gap, edge) in [(found[0], "left"), (found[1], "bottom"), (w - found[2], "right"), (h - found[3], "top")] {
            assert!((10.5..=13.5).contains(&gap), "page {} {edge} gap is {gap}", i + 1);
        }
    }
    assert_eq!(flat_marks(&out), report_marks(&[1, 2, 3, 4, 5, 6]));

    // Rotated pages: the crop follows what is shown.
    crop::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    let got = sizes(&out);
    assert!(got[0].0 > got[0].1, "page 1 text is wider than tall: {got:?}");
    assert!(got[1].1 > got[1].0 && got[2].1 > got[2].0, "sideways text is taller than wide: {got:?}");
    assert!(got.iter().all(|s| s.0 < 200.0 && s.1 < 200.0), "{got:?}");

    // One crop for all pages keeps them the same size.
    let uniform = crop::Options { uniform: true, ..opts.clone() };
    crop::run(&[fx("report.pdf")], &out, &uniform, &ctx()).unwrap();
    let got = sizes(&out);
    assert!(got.iter().all(|s| near(s.0, got[0].0, 0.01) && near(s.1, got[0].1, 0.01)), "{got:?}");

    // A blank page is left alone and mentioned.
    let with_blank = dir.join("with-blank.pdf");
    pages::run(&[fx("letter.pdf")], &with_blank, &pages::Options { insert_blank_after: vec![1], ..Default::default() }, &ctx()).unwrap();
    let outcome = crop::run(&[with_blank], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("1 page was blank")), "{:?}", outcome.notes);
    let got = sizes(&out);
    assert!(got[0].1 < 800.0 && near(got[1].1, 842.0, 0.1), "{got:?}");
}

#[test]
fn resize_scales_content_onto_the_new_paper() {
    let dir = out_dir("crop_resize");
    let out = dir.join("a5.pdf");
    let opts = crop::Options { mode: crop::Mode::Resize, paper: "a5".into(), ..Default::default() };
    crop::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    let ((a5w, a5h), _) = crop::paper_size("a5").unwrap();
    assert!(near(a5w, 419.53, 0.01) && near(a5h, 595.28, 0.01));
    assert!(sizes(&out).iter().all(|s| near(s.0, a5w, 0.01) && near(s.1, a5h, 0.01)));
    assert_eq!(flat_marks(&out), report_marks(&[1, 2, 3, 4, 5, 6]));
    // The mark line moved with the scale: fit keeps proportions and centres.
    let scale = (a5w / 595.0).min(a5h / 842.0);
    let (dx, dy) = ((a5w - 595.0 * scale) / 2.0, (a5h - 842.0 * scale) / 2.0);
    let before = locate(&fx("report.pdf"), 0, "MARK-REPORT-P1");
    let after = locate(&out, 0, "MARK-REPORT-P1");
    assert!(near(after.x0, before.x0 * scale + dx, 0.6), "{before:?} {after:?}");
    assert!(near(after.y1, before.y1 * scale + dy, 0.6), "{before:?} {after:?}");
    assert!(near(after.width(), before.width() * scale, 0.6));

    // Rotated and landscape pages keep their orientation, their rotation and their look.
    crop::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    let got = sizes(&out);
    assert!(near(got[0].0, a5w, 0.01) && near(got[0].1, a5h, 0.01));
    for page in &got[1..] {
        assert!(near(page.0, a5h, 0.01) && near(page.1, a5w, 0.01), "{got:?}");
    }
    assert_eq!(got.iter().map(|s| s.2).collect::<Vec<_>>(), vec![0, 90, 270, 0]);
    let source = TextReader::open_path(&fx("rotated.pdf"), None).unwrap();
    let result = TextReader::open_path(&out, None).unwrap();
    for i in 0..4 {
        let (a, b) = (source.page(i).unwrap(), result.page(i).unwrap());
        assert_eq!(a.plain(), b.plain());
        let (la, lb) = (&a.lines[0], &b.lines[0]);
        assert_eq!(la.dir, lb.dir, "text direction on page {}", i + 1);
        // Same relative place on the page.
        assert!(near(la.rect.x0 / a.width, lb.rect.x0 / b.width, 0.01), "page {}: {:?} {:?}", i + 1, la.rect, lb.rect);
        assert!(near(la.rect.y0 / a.height, lb.rect.y0 / b.height, 0.01), "page {}: {:?} {:?}", i + 1, la.rect, lb.rect);
    }

    // Stretch to an exact size, without turning the paper; annotations move with the page.
    let opts = crop::Options { mode: crop::Mode::Resize, paper: "612x792".into(), fit: crop::Fit::Stretch, ..Default::default() };
    let outcome = crop::run(&[fx("form.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("moved and scaled")), "{:?}", outcome.notes);
    assert_eq!(sizes(&out), vec![(612.0, 792.0, 0)]);
    let d = doc::load(&out, None).unwrap();
    assert_eq!(field_names(&d), vec!["full_name", "department", "agree"]);
    let page = doc::page_ids(&d)[0];
    let first = d.get_dictionary(page).unwrap().get(b"Annots").unwrap().as_array().unwrap()[0].as_reference().unwrap();
    let rect = doc::rect_of(&d, d.get_dictionary(first).unwrap().get(b"Rect").unwrap()).unwrap();
    let (sx, sy) = (612.0 / 595.0, 792.0 / 842.0);
    assert!(same_box(rect, [56.0 * sx, 676.0 * sy, 356.0 * sx, 696.0 * sy]), "{rect:?}");
    let label = locate(&out, 0, "Full name");
    assert!(near(label.x0, 56.0 * sx, 0.6));

    // Fill covers the page; a landscape target with keep_orientation off stays landscape.
    let opts = crop::Options { mode: crop::Mode::Resize, paper: "600x300".into(), fit: crop::Fit::Fill, ..Default::default() };
    crop::run(&[fx("letter.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(sizes(&out), vec![(600.0, 300.0, 0)]);
    let title = locate(&out, 0, "Cover Letter");
    assert!(near(title.x0, 56.0 * 600.0 / 595.0, 1.0), "fill scales by the larger factor: {title:?}");

    assert!(near(crop::paper_size("210x297mm").unwrap().0.0, 595.28, 0.01));
    assert_eq!(crop::paper_size(" Letter ").unwrap(), ((612.0, 792.0), true));
    assert_eq!(crop::paper_size("8.5x11in").unwrap(), ((612.0, 792.0), false));
    let opts = crop::Options { mode: crop::Mode::Resize, paper: "foolscap".into(), ..Default::default() };
    expect_invalid(crop::run(&[fx("letter.pdf")], &out, &opts, &ctx()), "not a paper size");
}

// ---------------------------------------------------------------------------
// nup
// ---------------------------------------------------------------------------

#[test]
fn nup_puts_four_pages_on_a_sheet() {
    let dir = out_dir("nup_four");
    let out = dir.join("4up.pdf");
    let opts = nup::Options { per_sheet: 4, border: true, ..Default::default() };
    let outcome = nup::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(outcome.pages, 2);
    let got = sizes(&out);
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|s| near(s.0, 595.28, 0.01) && near(s.1, 841.89, 0.01)), "portrait A4 wastes least for 2x2: {got:?}");
    let mut per_sheet = marks(&out);
    per_sheet.iter_mut().for_each(|m| m.sort());
    assert_eq!(per_sheet, vec![report_marks(&[1, 2, 3, 4]), report_marks(&[5, 6])]);

    // Row order: 1 2 / 3 4. Nothing is clipped: every mark lies inside the sheet margins.
    let at = |n: usize| locate(&out, (n - 1) / 4, &format!("MARK-REPORT-P{n}"));
    assert!(at(2).x0 > at(1).x1 && near(at(1).y0, at(2).y0, 0.5));
    assert!(at(3).y0 > at(1).y1 + 100.0 && near(at(1).x0, at(3).x0, 0.5));
    assert!(near(at(4).x0, at(2).x0, 0.5) && near(at(4).y0, at(3).y0, 0.5));
    assert!(near(at(5).x0, at(1).x0, 0.5) && near(at(5).y0, at(1).y0, 0.5));
    // Scale: a cell is (595.28 - 36 - 9) / 2 wide, the page 595 wide.
    let scale = ((595.28 - 36.0 - 9.0) / 2.0 / 595.0f32).min((841.89 - 36.0 - 9.0) / 2.0 / 842.0);
    let source = locate(&fx("report.pdf"), 0, "MARK-REPORT-P1");
    assert!(near(at(1).width(), source.width() * scale, 0.6));
    // The footer of the last row is still on the sheet.
    let footer = locate(&out, 0, "Quarterly Report / 3");
    assert!(footer.y1 < 841.89 - 18.0);

    // Column order: 1 3 / 2 4.
    let opts = nup::Options { per_sheet: 4, order: nup::Order::Column, ..Default::default() };
    nup::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    let at = |n: usize| locate(&out, (n - 1) / 4, &format!("MARK-REPORT-P{n}"));
    assert!(at(2).y0 > at(1).y1 + 100.0 && near(at(1).x0, at(2).x0, 0.5));
    assert!(at(3).x0 > at(1).x1 && near(at(1).y0, at(3).y0, 0.5));

    expect_invalid(nup::run(&[fx("report.pdf")], &out, &nup::Options { per_sheet: 3, ..Default::default() }, &ctx()), "2, 4, 6, 8, 9 or 16");
    expect_invalid(nup::run(&[fx("report.pdf")], &out, &nup::Options { margin: 400.0, ..Default::default() }, &ctx()), "no room");
}

#[test]
fn nup_picks_the_orientation_and_keeps_pages_upright() {
    let dir = out_dir("nup_orientation");
    let out = dir.join("2up.pdf");
    let outcome = nup::run(&[fx("report.pdf")], &out, &nup::Options::default(), &ctx()).unwrap();
    assert_eq!(outcome.pages, 3);
    assert!(sizes(&out).iter().all(|s| s.0 > s.1), "two portrait pages sit side by side on a landscape sheet");
    let p1 = locate(&out, 0, "MARK-REPORT-P1");
    let p2 = locate(&out, 0, "MARK-REPORT-P2");
    assert!(p1.x1 < 841.89 / 2.0 && p2.x0 > 841.89 / 2.0 && near(p1.y0, p2.y0, 0.5));

    // Forcing portrait stacks them instead.
    nup::run(&[fx("report.pdf")], &out, &nup::Options { landscape: Some(false), ..Default::default() }, &ctx()).unwrap();
    assert!(sizes(&out).iter().all(|s| s.0 < s.1));
    let p1 = locate(&out, 0, "MARK-REPORT-P1");
    let p2 = locate(&out, 0, "MARK-REPORT-P2");
    assert!(p2.y0 > p1.y1 && near(p1.x0, p2.x0, 0.5));

    // Sixteen-up and a page selection.
    let opts = nup::Options { per_sheet: 16, pages: "6, 1".into(), paper: "letter".into(), ..Default::default() };
    let outcome = nup::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(outcome.pages, 1);
    assert_eq!(flat_marks(&out).len(), 2);
    assert!(locate(&out, 0, "MARK-REPORT-P6").x1 < locate(&out, 0, "MARK-REPORT-P1").x0);

    // Rotated sources look the way a reader shows them: same text direction as displayed.
    let opts = nup::Options { per_sheet: 4, ..Default::default() };
    nup::run(&[fx("rotated.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(sizes(&out).len(), 1);
    let source = TextReader::open_path(&fx("rotated.pdf"), None).unwrap();
    let sheet = TextReader::open_path(&out, None).unwrap().page(0).unwrap();
    for i in 0..4 {
        let mark = format!("MARK-ROTATED-P{}", i + 1);
        let shown = source.page(i).unwrap();
        let from = shown.lines.iter().find(|l| l.text() == mark).unwrap();
        let to = sheet.lines.iter().find(|l| l.text() == mark).unwrap_or_else(|| panic!("{mark} missing on the sheet"));
        assert_eq!(from.dir, to.dir, "{mark} direction");
        assert!(to.rect.x0 >= 18.0 && to.rect.y0 >= 18.0 && to.rect.x1 <= sheet.width - 18.0 && to.rect.y1 <= sheet.height - 18.0, "{mark} inside the margins");
    }

    // Annotations cannot come along; the user is told.
    let outcome = nup::run(&[fx("form.pdf")], &out, &nup::Options::default(), &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("not carried onto the sheets")), "{:?}", outcome.notes);
    assert!(!catalog(&doc::load(&out, None).unwrap()).has(b"AcroForm"));
}

#[test]
fn booklet_orders_pages_for_folding() {
    assert_eq!(nup::booklet_order(8), vec![[Some(7), Some(0)], [Some(1), Some(6)], [Some(5), Some(2)], [Some(3), Some(4)]]);
    assert_eq!(nup::booklet_order(1), vec![[None, Some(0)], [None, None]]);
    assert_eq!(nup::booklet_order(0).len(), 0);

    let dir = out_dir("nup_booklet");
    let out = dir.join("booklet.pdf");
    let opts = nup::Options { booklet: true, per_sheet: 9, landscape: Some(false), ..Default::default() };
    let outcome = nup::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    // Six pages pad to eight: two sheets of paper, four printed sides.
    assert_eq!(outcome.pages, 4);
    assert!(outcome.notes.iter().any(|n| n.starts_with("2 sheets of paper")), "{:?}", outcome.notes);
    assert!(outcome.notes.iter().any(|n| n.contains("2 blank pages")), "{:?}", outcome.notes);
    assert!(sizes(&out).iter().all(|s| near(s.0, 841.89, 0.01) && near(s.1, 595.28, 0.01)));
    let mut sides = marks(&out);
    sides.iter_mut().for_each(|m| m.sort());
    assert_eq!(sides, vec![report_marks(&[1]), report_marks(&[2]), report_marks(&[3, 6]), report_marks(&[4, 5])]);
    let middle = 841.89 / 2.0;
    let left = |side: usize, n: usize| locate(&out, side, &format!("MARK-REPORT-P{n}")).x1 < middle;
    let right = |side: usize, n: usize| locate(&out, side, &format!("MARK-REPORT-P{n}")).x0 > middle;
    // Outer sheet: front has (blank 8 | 1), back has (2 | blank 7). Inner sheet: (6 | 3) and (4 | 5).
    assert!(right(0, 1));
    assert!(left(1, 2));
    assert!(left(2, 6) && right(2, 3));
    assert!(left(3, 4) && right(3, 5));
}

// ---------------------------------------------------------------------------
// protect and unlock
// ---------------------------------------------------------------------------

/// Every integer stored under `key` in the /Encrypt dictionary and its crypt filter, read
/// from the raw file (a loader would decrypt the document and drop the dictionary).
fn encrypt_ints(path: &Path, key: &str) -> Vec<i64> {
    let raw = std::fs::read(path).unwrap();
    let start = raw.windows(16).position(|w| w == b"/Filter/Standard").expect("an /Encrypt dictionary");
    let end = start + raw[start..].windows(6).position(|w| w == b"endobj").expect("end of the dictionary");
    let pattern = format!("/{key} ");
    let mut found = Vec::new();
    let mut from = start;
    while let Some(offset) = raw[from..end].windows(pattern.len()).position(|w| w == pattern.as_bytes()) {
        let at = from + offset + pattern.len();
        let digits: String = raw[at..end].iter().take_while(|b| b.is_ascii_digit() || **b == b'-').map(|b| *b as char).collect();
        found.extend(digits.parse::<i64>().ok());
        from = at;
    }
    found
}

fn encrypt_int(path: &Path, key: &str) -> i64 {
    *encrypt_ints(path, key).first().unwrap_or_else(|| panic!("no /{key} in the encryption dictionary"))
}

#[test]
fn protect_and_unlock_round_trip() {
    let dir = out_dir("protect_round_trip");
    for (encryption, name, bits, revision) in [(protect::Encryption::Aes256, "aes256.pdf", 256, 6), (protect::Encryption::Aes128, "aes128.pdf", 128, 4)] {
        let locked = dir.join(name);
        let opts = protect::Options { user_password: "open sesame".into(), owner_password: "boss".into(), encryption, allow_copy: false, allow_modify: false, ..Default::default() };
        let outcome = protect::run(&[fx("metadata.pdf")], &locked, &opts, &ctx()).unwrap();
        assert_eq!(outcome.pages, 2);
        assert!(outcome.notes[0].contains(&format!("AES-{bits}")));

        // Nothing readable is left in the file.
        let raw = std::fs::read(&locked).unwrap();
        assert!(contains(&std::fs::read(fx("metadata.pdf")).unwrap(), b"Priya Raman"));
        for secret in [&b"Priya Raman"[..], b"Board Minutes", b"xpacket"] {
            assert!(!contains(&raw, secret), "{} is readable in {name}", String::from_utf8_lossy(secret));
        }

        // The dictionary other readers rely on.
        assert_eq!(encrypt_int(&locked, "R"), revision);
        let mut lengths = encrypt_ints(&locked, "Length");
        lengths.sort_unstable();
        assert_eq!(lengths, vec![bits / 8, bits], "key length in bytes on the crypt filter, in bits on the dictionary");
        let p = encrypt_int(&locked, "P");
        assert!(p < 0, "reserved high bits are set");
        assert_eq!(p & (1 << 2), 1 << 2, "printing allowed");
        assert_eq!(p & (1 << 4), 0, "copying not allowed");
        assert_eq!(p & (1 << 3), 0, "changing not allowed");
        assert_eq!(p & (1 << 5), 1 << 5, "comments allowed");

        // Opening: no password, wrong password, the right one.
        assert!(matches!(doc::load(&locked, None), Err(Error::PasswordRequired)));
        assert!(matches!(doc::load(&locked, Some("wrong")), Err(Error::WrongPassword)));
        assert!(matches!(TextReader::open_path(&locked, None), Err(Error::PasswordRequired)));
        let d = doc::load(&locked, Some("open sesame")).unwrap();
        assert!(d.was_encrypted());
        assert_eq!(info_string(&d, "Author").as_deref(), Some("Priya Raman"));
        let source = TextReader::open_path(&fx("metadata.pdf"), None).unwrap();
        let opened = TextReader::open_path(&locked, Some("open sesame")).unwrap();
        for page in 0..2 {
            assert_eq!(source.page(page).unwrap().plain(), opened.page(page).unwrap().plain());
        }
        // The owner password opens it too. Checked through the loaders only for 256-bit:
        // for 128-bit files lopdf 0.45 derives a wrong key from an owner password and the
        // text reader refuses it. The tools work around that, see the unlock, strip and
        // crop runs with the owner password below.
        if encryption == protect::Encryption::Aes256 {
            assert_eq!(marks_with(&locked, Some("boss")), vec![vec!["MARK-METADATA-P1"], vec!["MARK-METADATA-P2"]]);
            assert_eq!(info_string(&doc::load(&locked, Some("boss")).unwrap(), "Author").as_deref(), Some("Priya Raman"));
        }

        // Unlock.
        let open = dir.join(format!("unlocked-{name}"));
        assert!(matches!(unlock::run(&[locked.clone()], &open, &unlock::Options::default(), &ctx()), Err(Error::PasswordRequired)));
        let wrong = ctx().with_password(Some("nope"));
        assert!(matches!(unlock::run(&[locked.clone()], &open, &unlock::Options::default(), &wrong), Err(Error::WrongPassword)));
        assert!(!open.exists());
        let right = ctx().with_password(Some("open sesame"));
        for (password, tag) in [("open sesame", "user"), ("boss", "owner")] {
            let open = dir.join(format!("unlocked-{tag}-{name}"));
            let outcome = unlock::run(&[locked.clone()], &open, &unlock::Options::default(), &ctx().with_password(Some(password))).unwrap();
            assert_eq!(outcome.pages, 2);
            let d = doc::load(&open, None).unwrap();
            assert!(!d.was_encrypted() && !d.trailer.has(b"Encrypt"));
            assert_eq!(info_string(&d, "Title").as_deref(), Some("Board Minutes, confidential draft"));
            assert_eq!(flat_marks(&open), vec!["MARK-METADATA-P1", "MARK-METADATA-P2"]);
            assert_eq!(info_string(&d, "Author").as_deref(), Some("Priya Raman"), "unlocked with the {tag} password");
            // The XMP stream decrypts as well.
            let xmp = catalog(&d).get(b"Metadata").and_then(Object::as_reference).and_then(|id| d.get_object(id)).unwrap().as_stream().unwrap();
            let xmp = xmp.decompressed_content().unwrap_or_else(|_| xmp.content.clone());
            assert!(contains(&xmp, b"<rdf:li>Priya Raman</rdf:li>"), "unlocked with the {tag} password");
            assert_eq!(unlock::status(&open).unwrap(), (false, true));
        }
        assert_eq!(unlock::status(&locked).unwrap(), (true, false));

        // Other tools open a protected file with the password from the context and say the result is open.
        let turned = dir.join(format!("turned-{name}"));
        let outcome = pages::run(&[locked.clone()], &turned, &pages::Options { order: "2".into(), ..Default::default() }, &right).unwrap();
        assert!(outcome.notes.iter().any(|n| n.contains("password protected")), "{:?}", outcome.notes);
        assert_eq!(flat_marks(&turned), vec!["MARK-METADATA-P2"]);
        assert!(matches!(pages::run(&[locked.clone()], &turned, &pages::Options::default(), &ctx()), Err(Error::PasswordRequired)));
        // ... with the owner password as well, including the tool that renders pages.
        let owner = ctx().with_password(Some("boss"));
        let outcome = strip::run(&[locked.clone()], &turned, &strip::Options::default(), &owner).unwrap();
        assert!(outcome.notes[0].starts_with("Removed title, author"), "{:?}", outcome.notes);
        assert_eq!(flat_marks(&turned), vec!["MARK-METADATA-P1", "MARK-METADATA-P2"]);
        let auto = crop::Options { mode: crop::Mode::Auto, ..Default::default() };
        crop::run(&[locked.clone()], &turned, &auto, &owner).unwrap();
        assert!(sizes(&turned).iter().all(|s| s.0 < 560.0 && s.1 < 800.0));

        // Protecting an already protected file replaces the password.
        let again = dir.join(format!("again-{name}"));
        let opts = protect::Options { user_password: "second".into(), encryption, ..Default::default() };
        protect::run(&[locked.clone()], &again, &opts, &right).unwrap();
        assert!(matches!(doc::load(&again, Some("open sesame")), Err(Error::WrongPassword)));
        assert_eq!(marks_with(&again, Some("second")).len(), 2);
    }
}

#[test]
fn protect_edge_cases() {
    let dir = out_dir("protect_edges");
    let out = dir.join("out.pdf");
    expect_invalid(protect::run(&[fx("report.pdf")], &out, &protect::Options::default(), &ctx()), "Enter a password");
    assert!(!out.exists());

    // Only an owner password: opens freely, restrictions recorded, Unlock needs no password.
    let opts = protect::Options { owner_password: "boss".into(), allow_print: false, allow_forms: false, allow_annotate: false, ..Default::default() };
    let outcome = protect::run(&[fx("form.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("opens without a password")), "{:?}", outcome.notes);
    let d = doc::load(&out, None).unwrap();
    assert!(d.was_encrypted());
    assert_eq!(field_names(&d), vec!["full_name", "department", "agree"], "field names decrypt");
    assert_eq!(flat_marks(&out), vec!["MARK-FORM-P1"]);
    let p = encrypt_int(&out, "P");
    assert_eq!(p & ((1 << 2) | (1 << 11) | (1 << 5) | (1 << 8)), 0, "print, comment and form bits are clear");
    assert_ne!(p & (1 << 4), 0);
    assert_eq!(unlock::status(&out).unwrap(), (true, true));
    let free = dir.join("free.pdf");
    // A wrong password does not get in the way when none is needed.
    unlock::run(&[out.clone()], &free, &unlock::Options::default(), &ctx().with_password(Some("guess"))).unwrap();
    assert!(!doc::load(&free, None).unwrap().was_encrypted());
    assert_eq!(field_names(&doc::load(&free, None).unwrap()), vec!["full_name", "department", "agree"]);

    // Same password for both and restrictions: works, with a warning.
    let opts = protect::Options { user_password: "pw".into(), allow_copy: false, ..Default::default() };
    let outcome = protect::run(&[fx("report.pdf")], &out, &opts, &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("separate owner password")), "{:?}", outcome.notes);

    // Unicode passwords with AES-256; refused with a reason for AES-128.
    let opts = protect::Options { user_password: "pässwörd-密码".into(), ..Default::default() };
    protect::run(&[fx("letter.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(marks_with(&out, Some("pässwörd-密码")), vec![vec!["MARK-LETTER-P1"]]);
    assert!(matches!(doc::load(&out, Some("password")), Err(Error::WrongPassword)));
    let opts = protect::Options { user_password: "pässwörd".into(), encryption: protect::Encryption::Aes128, ..Default::default() };
    expect_invalid(protect::run(&[fx("letter.pdf")], &out, &opts, &ctx()), "AES-256");

    // A file saved with object streams and a cross-reference stream encrypts cleanly too.
    let compact = dir.join("compact.pdf");
    let mut d = doc::load(&fx("report.pdf"), None).unwrap();
    doc::save_compact(&mut d, &compact).unwrap();
    let opts = protect::Options { user_password: "pw".into(), ..Default::default() };
    protect::run(&[compact], &out, &opts, &ctx()).unwrap();
    assert_eq!(marks_with(&out, Some("pw")).into_iter().flatten().collect::<Vec<_>>(), report_marks(&[1, 2, 3, 4, 5, 6]));

    // Options arrive as JSON with these exact names.
    let parsed: protect::Options = serde_json::from_str(r#"{"user_password":"a","encryption":"aes-128","allow_print":false}"#).unwrap();
    assert_eq!(parsed.encryption, protect::Encryption::Aes128);
    assert!(!parsed.allow_print && parsed.allow_copy);
}

#[test]
fn unlock_copies_a_file_that_was_never_locked() {
    let dir = out_dir("unlock_plain");
    let out = dir.join("copy.pdf");
    let outcome = unlock::run(&[fx("report.pdf")], &out, &unlock::Options::default(), &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("not password protected")), "{:?}", outcome.notes);
    assert_eq!(outcome.pages, 6);
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(fx("report.pdf")).unwrap());
}

// ---------------------------------------------------------------------------
// strip
// ---------------------------------------------------------------------------

#[test]
fn strip_removes_document_information_and_xmp() {
    let dir = out_dir("strip_basic");
    let out = dir.join("clean.pdf");
    let before = std::fs::read(fx("metadata.pdf")).unwrap();
    assert!(contains(&before, b"Priya Raman") && contains(&before, b"xpacket"));
    let found = strip::inspect(&fx("metadata.pdf"), None).unwrap();
    assert_eq!(found.info_fields, vec!["title", "author", "subject", "keywords", "creating program"]);
    assert!(found.xmp_packet);

    let outcome = strip::run(&[fx("metadata.pdf")], &out, &strip::Options::default(), &ctx()).unwrap();
    assert_eq!(outcome.notes[0], "Removed title, author, subject, keywords, creating program and the XMP packet.");
    let d = doc::load(&out, None).unwrap();
    let info = resolve(&d, d.trailer.get(b"Info").unwrap()).as_dict().unwrap();
    assert_eq!(info.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect::<Vec<_>>(), vec!["Producer"]);
    assert!(!catalog(&d).has(b"Metadata"));
    let raw = std::fs::read(&out).unwrap();
    for secret in [&b"Priya"[..], b"Board Minutes, confidential", b"Acme Writer", b"xpacket", b"/Metadata"] {
        assert!(!contains(&raw, secret), "{} is still in the file", String::from_utf8_lossy(secret));
    }
    assert_eq!(flat_marks(&out), vec!["MARK-METADATA-P1", "MARK-METADATA-P2"]);
    assert!(strip::inspect(&out, None).unwrap().is_empty());

    // Each option can be switched off.
    let opts = strip::Options { info: false, ..Default::default() };
    let outcome = strip::run(&[fx("metadata.pdf")], &out, &opts, &ctx()).unwrap();
    assert_eq!(outcome.notes[0], "Removed the XMP packet.");
    let d = doc::load(&out, None).unwrap();
    assert_eq!(info_string(&d, "Author").as_deref(), Some("Priya Raman"));
    assert!(!catalog(&d).has(b"Metadata"));
    let opts = strip::Options { xmp: false, ..Default::default() };
    strip::run(&[fx("metadata.pdf")], &out, &opts, &ctx()).unwrap();
    let d = doc::load(&out, None).unwrap();
    assert_eq!(info_string(&d, "Author"), None);
    assert!(catalog(&d).has(b"Metadata"));

    let outcome = strip::run(&[fx("report.pdf")], &out, &strip::Options::default(), &ctx()).unwrap();
    assert!(outcome.notes.iter().any(|n| n.contains("None of the chosen kinds")), "{:?}", outcome.notes);
}

/// A document carrying one of everything Strip looks for.
fn loaded_document() -> Document {
    let mut d = fixtures::text_document("Tender", "RICH", 2);
    let plain = |bytes: &[u8]| Stream::new(Dictionary::new(), bytes.to_vec()).with_compression(false);
    doc::set_info(&mut d, "Title", "Tender draft").unwrap();
    doc::set_info(&mut d, "Author", "Reviewer Bob").unwrap();
    doc::set_info(&mut d, "ModDate", "D:20260101120000Z").unwrap();
    let ids = doc::page_ids(&d);
    let (first, second) = (ids[0], ids[1]);

    let script = d.add_object(dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('named script')") });
    let on_close = d.add_object(dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('closing')") });
    let embedded = d.add_object(plain(b"ATTACHED-SECRET-ONE"));
    let filespec = d.add_object(dictionary! { "Type" => "Filespec", "F" => Object::string_literal("costs.txt"), "EF" => dictionary! { "F" => embedded } });
    let names = d.add_object(dictionary! {
        "JavaScript" => dictionary! { "Names" => vec![Object::string_literal("init"), script.into()] },
        "EmbeddedFiles" => dictionary! { "Names" => vec![Object::string_literal("costs.txt"), filespec.into()] },
    });
    let xmp = d.add_object(Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, b"<x:xmpmeta>XMP-DOC-SECRET</x:xmpmeta>".to_vec()).with_compression(false));
    let page_xmp = d.add_object(Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, b"<x:xmpmeta>XMP-PAGE-SECRET</x:xmpmeta>".to_vec()).with_compression(false));
    let private = d.add_object(plain(b"PRIVATE-APP-DATA"));
    let root = d.trailer.get(b"Root").and_then(Object::as_reference).unwrap();
    {
        let catalog = d.get_dictionary_mut(root).unwrap();
        catalog.set("Names", names);
        catalog.set("Metadata", xmp);
        catalog.set("OpenAction", dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('on open')") });
        catalog.set("AA", dictionary! { "WC" => on_close });
        catalog.set("PieceInfo", dictionary! { "Editor" => dictionary! { "LastModified" => Object::string_literal("D:2026"), "Private" => private } });
    }

    let thumb = d.add_object(plain(b"THUMBNAIL-PIXELS"));
    let attached = d.add_object(plain(b"ATTACHED-SECRET-TWO"));
    let note = d.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Text", "Rect" => vec![50.into(), 50.into(), 70.into(), 70.into()],
        "Contents" => Object::string_literal("Check clause 4"), "T" => Object::string_literal("Reviewer Bob"),
        "M" => Object::string_literal("D:20260102090000Z"), "CreationDate" => Object::string_literal("D:20260102080000Z"),
        "NM" => Object::string_literal("note-0001"),
    });
    let link = d.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![100.into(), 50.into(), 200.into(), 70.into()],
        "A" => dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('link script')") },
    });
    let web = d.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![100.into(), 80.into(), 200.into(), 100.into()],
        "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.org/") },
    });
    let clip = d.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FileAttachment", "Rect" => vec![300.into(), 50.into(), 320.into(), 70.into()],
        "FS" => dictionary! { "Type" => "Filespec", "F" => Object::string_literal("notes.txt"), "EF" => dictionary! { "F" => attached } },
        "T" => Object::string_literal("Reviewer Bob"),
    });
    let popup = d.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Popup", "Rect" => vec![330.into(), 50.into(), 430.into(), 120.into()], "Parent" => clip });
    {
        let page = d.get_dictionary_mut(first).unwrap();
        page.set("Thumb", thumb);
        page.set("Metadata", page_xmp);
        page.set("Annots", vec![Object::Reference(note), link.into(), web.into(), clip.into(), popup.into()]);
        page.set("AA", dictionary! { "O" => dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('page open')") } });
    }
    d.get_dictionary_mut(second).unwrap().set("LastModified", Object::string_literal("D:20260103"));
    d.trailer.set("ID", vec![Object::string_literal("ORIGINAL-FILE-ID-A"), Object::string_literal("ORIGINAL-FILE-ID-B")]);
    d
}

const SECRETS: [&str; 13] = [
    "Reviewer Bob", "Tender draft", "named script", "closing", "on open", "link script", "page open", "ATTACHED-SECRET-ONE", "ATTACHED-SECRET-TWO",
    "XMP-DOC-SECRET", "XMP-PAGE-SECRET", "THUMBNAIL-PIXELS", "PRIVATE-APP-DATA",
];

#[test]
fn strip_removes_scripts_attachments_thumbnails_and_private_data() {
    let dir = out_dir("strip_everything");
    let source = dir.join("loaded.pdf");
    let mut d = loaded_document();
    doc::save(&mut d, &source).unwrap();
    let raw = std::fs::read(&source).unwrap();
    for secret in SECRETS.iter().chain(["ORIGINAL-FILE-ID-A", "note-0001", "D:20260102090000Z"].iter()) {
        assert!(contains(&raw, secret.as_bytes()), "the test file should contain {secret}");
    }

    let out = dir.join("clean.pdf");
    let outcome = strip::run(&[source.clone()], &out, &strip::Options::default(), &ctx()).unwrap();
    let notes = outcome.notes.join(" | ");
    assert!(notes.contains("Removed title, author, 1 other field, the XMP packet and metadata on 1 page or image."), "{notes}");
    assert!(notes.contains("Removed 1 page thumbnail."), "{notes}");
    assert!(notes.contains("Removed 5 scripts."), "{notes}");
    assert!(notes.contains("Removed 2 attached files."), "{notes}");
    assert!(notes.contains("Cleared the author and date on 1 comment."), "{notes}");
    assert!(notes.contains("editing programs"), "{notes}");

    let raw = std::fs::read(&out).unwrap();
    for secret in SECRETS.iter().chain(["ORIGINAL-FILE-ID-A", "note-0001", "D:20260102090000Z", "JavaScript", "EmbeddedFiles", "PieceInfo", "LastModified", "/Thumb", "FileAttachment"].iter()) {
        assert!(!contains(&raw, secret.as_bytes()), "{secret} is still in the file");
    }
    let d = doc::load(&out, None).unwrap();
    let c = catalog(&d);
    for key in [&b"Metadata"[..], b"OpenAction", b"AA", b"PieceInfo"] {
        assert!(!c.has(key), "catalog still has {}", String::from_utf8_lossy(key));
    }
    let first = doc::page_ids(&d)[0];
    assert!(!d.get_dictionary(first).unwrap().has(b"AA"));
    // The comment and both links are still there; the web link still works.
    assert_eq!(annotation_subtypes(&d, first), vec!["Text", "Link", "Link"]);
    let annots = d.get_dictionary(first).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    let note = d.get_dictionary(annots[0].as_reference().unwrap()).unwrap();
    assert_eq!(text_of(note.get(b"Contents").unwrap()), "Check clause 4");
    assert!(!note.has(b"T") && !note.has(b"M") && !note.has(b"NM") && !note.has(b"CreationDate"));
    assert!(!d.get_dictionary(annots[1].as_reference().unwrap()).unwrap().has(b"A"));
    assert!(contains(&raw, b"https://example.org/"));
    let id = d.trailer.get(b"ID").unwrap().as_array().unwrap();
    assert_eq!(id.len(), 2);
    assert_eq!(id[0].as_str().unwrap().len(), 16);
    assert_eq!(flat_marks(&out), vec!["MARK-RICH-P1", "MARK-RICH-P2"]);

    // With everything switched off nothing is taken out.
    let kept = dir.join("kept.pdf");
    let none = strip::Options { info: false, xmp: false, thumbnails: false, javascript: false, attachments: false, annotations_meta: false, private_data: false };
    strip::run(&[source.clone()], &kept, &none, &ctx()).unwrap();
    let raw = std::fs::read(&kept).unwrap();
    for secret in SECRETS.iter().chain(["ORIGINAL-FILE-ID-A", "note-0001"].iter()) {
        assert!(contains(&raw, secret.as_bytes()), "{secret} should have been kept");
    }

    // Only scripts.
    let opts = strip::Options { javascript: true, ..none.clone() };
    strip::run(&[source.clone()], &kept, &opts, &ctx()).unwrap();
    let raw = std::fs::read(&kept).unwrap();
    assert!(!contains(&raw, b"JavaScript") && !contains(&raw, b"app.alert"));
    assert!(contains(&raw, b"ATTACHED-SECRET-ONE") && contains(&raw, b"Reviewer Bob") && contains(&raw, b"THUMBNAIL-PIXELS"));

    // Form field names are not mistaken for comment authors.
    strip::run(&[fx("form.pdf")], &kept, &strip::Options::default(), &ctx()).unwrap();
    assert_eq!(field_names(&doc::load(&kept, None).unwrap()), vec!["full_name", "department", "agree"]);
}

// ---------------------------------------------------------------------------
// through the JSON entry point
// ---------------------------------------------------------------------------

#[test]
fn tools_run_from_json_with_defaults() {
    let dir = out_dir("json");
    for id in ["merge", "split", "pages", "crop", "nup", "protect", "unlock", "strip"] {
        assert!(shorui_core::tools::default_options(id).is_some(), "{id} has default options");
    }
    let defaults = shorui_core::tools::default_options("merge").unwrap();
    assert_eq!(defaults["bookmarks"], "per-file");
    assert_eq!(defaults["keep_form_fields"], true);
    assert_eq!(shorui_core::tools::default_options("split").unwrap()["mode"], "each");
    assert_eq!(shorui_core::tools::default_options("split").unwrap()["name_pattern"], "{name}-{n}");
    assert_eq!(shorui_core::tools::default_options("crop").unwrap()["mode"], "margins");
    assert_eq!(shorui_core::tools::default_options("nup").unwrap()["per_sheet"], 2);
    assert_eq!(shorui_core::tools::default_options("nup").unwrap()["landscape"], serde_json::Value::Null);
    assert_eq!(shorui_core::tools::default_options("protect").unwrap()["encryption"], "aes-256");
    assert_eq!(shorui_core::tools::default_options("strip").unwrap()["annotations_meta"], true);

    let options = serde_json::json!({ "order": "2", "rotate": [{ "pages": "2", "degrees": 90 }], "insert_blank_after": [2] });
    let out = dir.join("pages.pdf");
    let outcome = shorui_core::tools::run_json("pages", &[fx("report.pdf")], &out, &options, &ctx()).unwrap();
    assert_eq!(outcome.pages, 2);
    assert_eq!(sizes(&out), vec![(842.0, 595.0, 90), (842.0, 595.0, 0)]);

    let options = serde_json::json!({ "mode": "resize", "paper": "a3", "fit": "fill" });
    shorui_core::tools::run_json("crop", &[fx("letter.pdf")], &dir.join("a3.pdf"), &options, &ctx()).unwrap();
    let options = serde_json::json!({ "bookmarks": "keep", "ranges": ["1"] });
    shorui_core::tools::run_json("merge", &[fx("report.pdf")], &dir.join("merge.pdf"), &options, &ctx()).unwrap();
    let options = serde_json::json!({ "mode": "every", "every": 2 });
    let outcome = shorui_core::tools::run_json("split", &[fx("report.pdf")], &dir.join("parts"), &options, &ctx()).unwrap();
    assert_eq!(outcome.outputs.len(), 3);
    let options = serde_json::json!({ "order": "column", "per_sheet": 6 });
    let outcome = shorui_core::tools::run_json("nup", &[fx("report.pdf")], &dir.join("6up.pdf"), &options, &ctx()).unwrap();
    assert_eq!(outcome.pages, 1);
}
