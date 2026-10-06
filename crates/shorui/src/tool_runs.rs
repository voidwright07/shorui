//! Every tool, driven the way the window drives it: options start from the tool's
//! defaults, are changed through the panel's schema, the run is planned and executed on
//! the job thread, and the output is read back. One test per tool, on generated PDFs.

use crate::catalog::{self, Tool};
use crate::jobs::{self, Event};
use crate::state::{PagesEdit, RowStatus, State};
use serde_json::{Value, json};
use shorui_core::text::TextReader;
use shorui_core::{doc, fixtures};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) fn fixtures_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("shorui-app-fixtures-{}", std::process::id()));
        fixtures::write_all(&dir).expect("fixtures");
        dir
    })
}

fn out_dir(tool: &str) -> PathBuf {
    // One folder per run: several tests drive the same tool at the same time.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("shorui-app-out-{}-{tool}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

struct Run {
    state: State,
    tool: &'static Tool,
    out: PathBuf,
}

impl Run {
    fn new(tool: &'static str, files: &[&str]) -> Run {
        let tool = catalog::tool(tool).expect("tool");
        let mut state = State::default();
        state.mode = Some(tool.id);
        state.add_files(files.iter().map(|f| fixtures_dir().join(f)).collect());
        for f in &mut state.files {
            if f.is_pdf() {
                if let Ok(info) = doc::quick_info(&f.path) {
                    f.pages = Some(info.pages);
                    f.locked = info.locked;
                }
            }
        }
        state.fix_active();
        Run { state, tool, out: out_dir(tool.id) }
    }

    /// Change an option the way the panel does: by field, from text.
    fn set(&mut self, key: &str, text: &str) -> &mut Self {
        let field = self.tool.fields.iter().find(|f| f.key == key).unwrap_or_else(|| panic!("{} has no field {key}", self.tool.id));
        self.state.set_option_text(self.tool.id, field, text);
        self
    }

    /// Set an option the canvas or grid supplies directly.
    fn raw(&mut self, key: &str, value: Value) -> &mut Self {
        self.state.set_option(self.tool.id, key, value);
        self
    }

    /// Plan and run, returning each row's final status. `None` when a helper program is missing.
    fn go(&mut self) -> Option<Vec<RowStatus>> {
        let rows = self.state.run_set();
        let files: Vec<PathBuf> = rows.iter().map(|i| self.state.files[*i].path.clone()).collect();
        let options = self.state.options_of(self.tool.id);
        let mut items = jobs::plan(self.tool, &files, &self.out, &options).expect("plan");
        for item in &mut items {
            item.row = rows.get(item.row).copied().unwrap_or(0);
        }
        let touched: Vec<usize> = items.iter().map(|i| i.row).collect();
        let handle = jobs::start(self.tool.id, items, options);
        let mut progress = 0;
        loop {
            match handle.events.recv().expect("job thread ended without finishing") {
                Event::Started { row } => {
                    if let Some(f) = self.state.files.get_mut(row) {
                        f.status = RowStatus::Running { fraction: 0.0, what: String::new() };
                    }
                }
                Event::Progress { .. } => progress += 1,
                Event::Done { row, outcome } => {
                    if self.state.files.is_empty() {
                        // HTML from a URL has no queue row; keep the outcome anyway.
                        self.state.add_files(vec![PathBuf::from("url.html")]);
                    }
                    self.state.apply_outcome(row, outcome);
                }
                Event::Failed { row, message } => {
                    if message.contains("was not found on this machine") {
                        eprintln!("skipped {}: {message}", self.tool.id);
                        return None;
                    }
                    if let Some(f) = self.state.files.get_mut(row) {
                        f.status = RowStatus::Failed { message };
                    }
                }
                Event::Finished { cancelled } => {
                    assert!(!cancelled);
                    break;
                }
            }
        }
        let _ = progress;
        Some(touched.into_iter().map(|row| self.state.files[row].status.clone()).collect())
    }

    /// Run and expect every row to succeed. Returns the outputs of the first row.
    fn ok(&mut self) -> Option<Vec<PathBuf>> {
        let statuses = self.go()?;
        for s in &statuses {
            if let RowStatus::Failed { message } = s {
                panic!("{} failed: {message}", self.tool.id);
            }
        }
        match &statuses[0] {
            RowStatus::Done { outputs, .. } => {
                for o in outputs {
                    assert!(o.exists(), "{} reported {} but it is not there", self.tool.id, o.display());
                }
                Some(outputs.clone())
            }
            other => panic!("{} ended as {other:?}", self.tool.id),
        }
    }
}

fn pages(path: &Path) -> usize {
    doc::page_count(&doc::load(path, None).expect("output loads"))
}

fn text(path: &Path) -> Vec<String> {
    let reader = TextReader::open_path(path, None).expect("output opens");
    (0..reader.page_count()).map(|i| reader.page(i).expect("page text").plain()).collect()
}

#[test]
fn merge_combines_in_queue_order() {
    let mut run = Run::new("merge", &["report.pdf", "letter.pdf", "appendix.pdf"]);
    run.state.move_file(1, 0);
    run.raw("ranges", json!(["", "1-2", ""]));
    let out = run.ok().unwrap();
    let t = text(&out[0]);
    assert_eq!(t.len(), 1 + 2 + 3);
    assert!(t[0].contains("MARK-LETTER-P1"));
    assert!(t[1].contains("MARK-REPORT-P1") && t[2].contains("MARK-REPORT-P2"));
    assert!(t[5].contains("MARK-APPENDIX-P3"));
}

#[test]
fn split_writes_one_file_per_range() {
    let mut run = Run::new("split", &["report.pdf"]);
    run.set("mode", "ranges").set("ranges", "1-2; 3-6");
    let out = run.ok().unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(pages(&out[0]), 2);
    assert_eq!(pages(&out[1]), 4);
}

#[test]
fn pages_applies_the_grid_edit() {
    let mut run = Run::new("pages", &["report.pdf"]);
    let mut edit = PagesEdit::new(6);
    edit.click(5, false, false);
    edit.move_to(0);
    edit.click(2, false, false);
    assert!(edit.delete());
    edit.click(0, false, false);
    edit.rotate(90);
    for (k, v) in edit.to_options() {
        run.raw(&k, v);
    }
    let out = run.ok().unwrap();
    let t = text(&out[0]);
    assert_eq!(t.len(), 5);
    assert!(t[0].contains("MARK-REPORT-P6") && t[1].contains("MARK-REPORT-P1") && t[2].contains("MARK-REPORT-P3"));
    let d = doc::load(&out[0], None).unwrap();
    assert_eq!(doc::rotation(&d, doc::page_ids(&d)[0]), 90);
}

#[test]
fn crop_trims_margins() {
    let mut run = Run::new("crop", &["report.pdf"]);
    run.set("top", "40").set("left", "20");
    let out = run.ok().unwrap();
    let d = doc::load(&out[0], None).unwrap();
    let (w, h) = doc::visible_size(&d, doc::page_ids(&d)[0]);
    assert!((w - 575.).abs() < 0.5 && (h - 802.).abs() < 0.5, "{w} x {h}");
}

#[test]
fn nup_puts_four_pages_on_a_sheet() {
    let mut run = Run::new("nup", &["report.pdf"]);
    run.set("per_sheet", "4");
    let out = run.ok().unwrap();
    assert_eq!(pages(&out[0]), 2);
    let all = text(&out[0]).join("\n");
    for n in 1..=6 {
        assert!(all.contains(&format!("MARK-REPORT-P{n}")));
    }
}

#[test]
fn img2pdf_makes_a_page_per_image() {
    let mut run = Run::new("img2pdf", &["images/photo.jpg", "images/diagram.png"]);
    let out = run.ok().unwrap();
    assert_eq!(pages(&out[0]), 2);
}

#[test]
fn pdf2img_exports_the_chosen_pages() {
    let mut run = Run::new("pdf2img", &["report.pdf"]);
    run.set("format", "jpg").set("dpi", "72").set("pages", "2-3");
    let out = run.ok().unwrap();
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|p| p.extension().is_some_and(|e| e == "jpg")));
}

#[test]
fn office_converts_a_pdf_to_word() {
    let mut run = Run::new("office", &["letter.pdf"]);
    if let Some(out) = run.ok() {
        assert_eq!(out[0].extension().and_then(|e| e.to_str()), Some("docx"));
        assert!(std::fs::read(&out[0]).unwrap().starts_with(b"PK"));
    }
}

#[test]
fn html_prints_a_local_page() {
    let mut run = Run::new("html", &["sample.html"]);
    if let Some(out) = run.ok() {
        assert!(text(&out[0]).join("\n").contains("MARK-HTML-P1"));
    }
}

#[test]
fn csv2pdf_lays_out_the_rows() {
    let csv = fixtures_dir().join("app-orders.csv");
    std::fs::write(&csv, "Item;Qty\nBolts;40\nNuts;\"1 200\"\n").unwrap();
    let mut run = Run::new("csv2pdf", &["app-orders.csv"]);
    run.set("orientation", "landscape");
    let out = run.ok().unwrap();
    assert_eq!(out[0].file_name().unwrap(), "app-orders.pdf");
    let t = text(&out[0]).join("\n");
    for word in ["Item", "Qty", "Bolts", "40", "Nuts", "1 200"] {
        assert!(t.contains(word), "{word} missing from {t}");
    }
}

#[test]
fn pdfa_marks_the_file_as_archival() {
    let mut run = Run::new("pdfa", &["report.pdf"]);
    let out = run.ok().unwrap();
    let bytes = std::fs::read(&out[0]).unwrap();
    let d = doc::load_bytes(&bytes, None).unwrap();
    assert_eq!(doc::page_count(&d), 6);
    assert!(d.catalog().unwrap().has(b"OutputIntents"));
}

#[test]
fn tables_finds_the_table() {
    let mut run = Run::new("tables", &["table.pdf"]);
    let out = run.ok().unwrap();
    let csv = std::fs::read_to_string(&out[0]).unwrap();
    assert!(csv.contains("Region,Units,Revenue,Margin"), "{csv}");
    assert!(csv.contains("Total,4439,177560.00,30%"), "{csv}");
}

#[test]
fn edit_fills_fields_and_places_items() {
    let mut run = Run::new("edit", &["form.pdf"]);
    run.raw("fields", json!({ "full_name": "Priya Raman", "agree": "true" }));
    run.raw("items", json!([{ "kind": "text", "page": 1, "x": 60.0, "y": 400.0, "w": 0.0, "h": 0.0, "text": "Placed by the test" }, { "kind": "check", "page": 1, "x": 300.0, "y": 300.0, "w": 12.0, "h": 12.0, "text": "" }]));
    let out = run.ok().unwrap();
    let fields = shorui_core::tools::edit::list_fields(&out[0], None).unwrap();
    assert_eq!(fields.iter().find(|f| f.name == "full_name").unwrap().value, "Priya Raman");
    assert_eq!(fields.iter().find(|f| f.name == "agree").unwrap().value, "true");
    assert!(text(&out[0])[0].contains("Placed by the test"));
}

#[test]
fn sign_places_a_typed_name_and_date() {
    let mut run = Run::new("sign", &["form.pdf"]);
    run.set("typed", "Priya Raman").set("add_date", "true");
    run.raw("page", json!(1)).raw("x", json!(60.0)).raw("y", json!(290.0));
    let out = run.ok().unwrap();
    let t = text(&out[0])[0].clone();
    assert!(t.contains("Priya Raman"), "{t}");
    assert!(t.contains(&shorui_core::tools::edit::today()), "{t}");
}

#[test]
fn watermark_stamps_every_page() {
    let mut run = Run::new("watermark", &["report.pdf"]);
    run.set("text", "DRAFT");
    let out = run.ok().unwrap();
    assert!(text(&out[0]).iter().all(|p| p.contains("DRAFT")));
}

#[test]
fn numbers_prints_page_numbers() {
    let mut run = Run::new("numbers", &["report.pdf"]);
    run.set("format", "Page {n} of {total}");
    let out = run.ok().unwrap();
    assert!(text(&out[0])[2].contains("Page 3 of 6"));
}

#[test]
fn flatten_removes_the_form() {
    let mut run = Run::new("flatten", &["form.pdf"]);
    let out = run.ok().unwrap();
    assert!(shorui_core::tools::edit::list_fields(&out[0], None).unwrap().is_empty());
}

#[test]
fn protect_then_unlock_round_trips() {
    let mut run = Run::new("protect", &["report.pdf"]);
    run.set("user_password", "open-sesame");
    let locked = run.ok().unwrap();
    assert!(matches!(doc::load(&locked[0], None), Err(shorui_core::Error::PasswordRequired)));

    // The protected file goes into Unlock as a queue row, password typed in the panel.
    let tool = catalog::tool("unlock").unwrap();
    let mut state = State::default();
    state.mode = Some(tool.id);
    state.add_files(vec![locked[0].clone()]);
    let mut unlock = Run { state, tool, out: out_dir("unlock") };
    unlock.set("@password", "open-sesame");
    let open = unlock.ok().unwrap();
    assert!(text(&open[0])[0].contains("MARK-REPORT-P1"));

    let mut wrong = Run { state: State::default(), tool, out: out_dir("unlock-wrong") };
    wrong.state.mode = Some(tool.id);
    wrong.state.add_files(vec![locked[0].clone()]);
    wrong.set("@password", "nope");
    let statuses = wrong.go().unwrap();
    assert!(matches!(&statuses[0], RowStatus::Failed { message } if message == "That password is not correct."), "{statuses:?}");
}

#[test]
fn redact_removes_searched_text() {
    let mut run = Run::new("redact", &["contacts.pdf"]);
    run.raw("search", json!([{ "text": "Mara Lindqvist", "match_case": false, "whole_words": false }]));
    run.raw("patterns", json!(["email"]));
    let out = run.ok().unwrap();
    let all = text(&out[0]).join("\n");
    assert!(!all.contains("Mara Lindqvist") && !all.contains("example.com"), "{all}");
    assert!(!String::from_utf8_lossy(&std::fs::read(&out[0]).unwrap()).contains("Lindqvist"));
}

#[test]
fn strip_clears_the_document_information() {
    let mut run = Run::new("strip", &["metadata.pdf"]);
    let out = run.ok().unwrap();
    let raw = String::from_utf8_lossy(&std::fs::read(&out[0]).unwrap()).to_string();
    assert!(!raw.contains("Priya Raman") && !raw.contains("Acme Writer"));
}

#[test]
fn compress_shrinks_the_photos() {
    let mut run = Run::new("compress", &["photos.pdf", "letter.pdf"]);
    let statuses = run.go().unwrap();
    match &statuses[0] {
        RowStatus::Done { bytes_out, .. } => assert!(*bytes_out < run.state.files[0].bytes / 5, "{bytes_out}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(statuses[1], RowStatus::Done { .. }));
}

#[test]
fn ocr_adds_a_text_layer() {
    let mut run = Run::new("ocr", &["scan.pdf"]);
    if let Some(out) = run.ok() {
        assert!(text(&out[0])[0].contains("Delivery Note"));
    }
}

#[test]
fn repair_recovers_the_broken_file() {
    let mut run = Run::new("repair", &["broken.pdf"]);
    // The app unticks unreadable files in batch tools; Repair is where they are meant to go.
    run.state.files[0].included = true;
    let out = run.ok().unwrap();
    assert!(text(&out[0])[2].contains("MARK-BROKEN-P3"));
}

#[test]
fn compare_reports_the_changed_pages() {
    let mut run = Run::new("compare", &["report.pdf", "report-v2.pdf"]);
    let out = run.ok().unwrap();
    let report = out.iter().find(|p| p.file_name().is_some_and(|n| n == "report.json")).expect("report.json");
    let json: Value = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    let changed: Vec<u64> = json["pages"].as_array().unwrap().iter().filter(|p| p["text_changed"] == json!(true)).map(|p| p["page"].as_u64().unwrap()).collect();
    assert_eq!(changed, vec![2, 4]);
}

#[test]
fn a_failing_file_does_not_stop_the_batch() {
    let mut run = Run::new("compress", &["broken.pdf", "letter.pdf"]);
    run.state.files[0].included = true;
    let statuses = run.go().unwrap();
    assert!(matches!(&statuses[0], RowStatus::Failed { message } if message.contains("Repair")), "{statuses:?}");
    assert!(matches!(statuses[1], RowStatus::Done { .. }));
}
