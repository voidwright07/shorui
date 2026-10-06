# Building a tool in shorui-core

Shorui is an offline, cross-platform (Windows, macOS, Linux) desktop PDF toolbox. This crate holds
the PDF operations; the GPUI app in `crates/shorui` calls them. Nothing here may open a network
connection (the only exception is HTML/URL to PDF when the user gives a URL).

## The contract

Each tool is one file, `src/tools/<id>.rs`, already present as a stub:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]   // or a hand-written Default
#[serde(default)]
pub struct Options { /* every field has a default */ }

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome>
```

- `out` is the output **file** for `OutputKind::File` tools and an output **folder** for `OutputKind::Dir`
  tools (see the table in `src/tools/mod.rs`). Create parent folders; never write anywhere else.
  Never modify or delete an input file.
- `Options` is deserialised from JSON by the app and the CLI, so use plain serde types, `snake_case`
  field names, `#[serde(default)]`, and string enums with `#[serde(rename_all = "kebab-case")]`.
  Keep the field names listed in your brief: the UI binds to them. You may add fields.
- Open encrypted inputs with `ctx.password()`: `doc::load(path, ctx.password())`.
- Call `ctx.report(fraction, "short phrase")` as work proceeds and `ctx.check()?` between pages or
  files so the user can cancel.
- Return an `Outcome`: output paths, page count, `bytes_in` (sum of input sizes), `bytes_out`
  (`Outcome::single` and `Outcome::push` fill it), and `notes` for anything the user should know.
- Errors are shown to the user verbatim. Use `crate::Error` variants; write plain sentences that say
  what happened and what to do. No `unwrap`/`expect`/indexing that can panic on user data.
- You may expose extra `pub fn`s the UI will want (previews, estimates, lists of form fields).
  Document them with a doc comment.

## What already exists (read these files before writing code)

- `src/doc.rs`: `load`, `load_bytes`, `save`, `save_compact`, `to_bytes`, `write_file`; `page_ids`,
  `page_count`, `inherited`, `media_box`, `crop_box`, `rotation`, `visible_size`; `visible_to_page`
  and `page_to_visible` matrices (draw overlays in "visible space" and they land upright on rotated
  pages); `flatten_inherited`, `set_page_order`, `duplicate_page`, `new_document`, `append_pages`,
  `blank_page`, `import_pages` (deep copy between documents); `resources_mut`, `add_resource`,
  `overlay` (draw on top of or under a page safely), `page_as_xobject`; standard fonts (`StdFont`,
  `ensure_font`, `StdFont::width`), `ensure_alpha`, `ensure_xobject`, `pdf_string`, `fmt`, `cm`,
  `set_info`.
- `src/img.rs`: `add_image` (JPEG or Flate, keeps alpha), `add_jpeg_bytes`, `draw`.
- `src/render.rs`: `Renderer` (pure-Rust rasteriser): `open`, `page_count`, `page_size`, `render(index, scale)`,
  `thumbnail`. `dpi_to_scale`.
- `src/text.rs`: `TextReader::page(index) -> PageText` with lines, words and rectangles in display
  space (points, origin top-left, y down); `PageText::find`, `plain`. `Rect4::to_visible`.
- `src/range.rs`: `parse("1-3, 7", page_count)` gives 1-based page numbers; `format`.
- `src/helpers.rs`: `find_browser`, `find_libreoffice`, `find_tesseract`, `find_program`, `run`
  (runs a helper without a console window), `TempDir`, `unique_path`, `stem`, `ext`, `ensure_dir`.
- `src/fixtures.rs`: generated test documents. `fixtures::write_all(dir)` writes `report.pdf` (6 pages),
  `letter.pdf` (1), `appendix.pdf` (3), `contacts.pdf` (2), `table.pdf`, `rotated.pdf` (rotate 0/90/270 and a
  landscape page), `form.pdf` (AcroForm: text fields `full_name`, `department`, check box `agree`, a FreeText
  note), `metadata.pdf`, `photos.pdf` (10 MB of lossless photos), `scan.pdf` (image only, no text layer),
  `report-v2.pdf`, `broken.pdf` (wrecked xref), `images/photo.jpg`, `images/diagram.png` (has alpha),
  `images/signature.png`, `sample.html`. Every text page carries a line `MARK-<TAG>-P<n>` so tests can tell
  which source page ended up where.

Crates available: `lopdf 0.45`, `hayro 0.7` (+ `hayro-interpret`, `hayro-syntax`), `krilla 0.8`, `image 0.25`,
`flate2`, `serde`, `serde_json`, `similar`, `thiserror`. Their sources are in
`~/.cargo/registry/src/index.crates.io-*/`; read them instead of guessing an API.

## Rules for working alongside other agents

Four agents work in this crate at the same time, each on its own set of tools.

- Edit only: your tool files in `src/tools/`, your own test file `tests/<set>.rs`, and your marked
  section at the bottom of `Cargo.toml` (re-read the file right before editing it, keep the edit small).
- Do **not** edit `doc.rs`, `img.rs`, `render.rs`, `text.rs`, `range.rs`, `helpers.rs`, `fixtures.rs`,
  `ctx.rs`, `error.rs`, `lib.rs`, `tools/mod.rs` or another agent's files. If the foundation is missing
  something or has a bug, work around it inside your own file and say so in your final report.
- Build and test only your set, in your own target folder, so nobody blocks anybody:
  `CARGO_TARGET_DIR=target/<set> cargo test -p shorui-core --no-default-features --features set-<set>`
  (run from the repository root, `C:\Users\ASUS\Desktop\Code\shorui`). Start your test file with
  `#![cfg(feature = "set-<set>")]`.
- The CLI helps with manual checks:
  `CARGO_TARGET_DIR=target/<set> cargo run -p shorui-core --no-default-features --features set-<set> --bin shorui-cli -- run <tool> --out <path> --opt '<json>' <inputs...>`.
  Also `shorui-cli text <file>`, `info <file>`, `render <file> <page> <out.png>`.
  Fixtures are already written to `testdata/`. Write your outputs under `testdata/out/<tool>/`.
- This machine is Windows 11 with Git Bash. Poppler tools are on PATH (`pdfinfo`, `pdftotext`, `pdfimages`,
  `pdffonts`, `pdftocairo`) and are good for independent verification by hand, but tests must not depend on them.
- Do not download anything other than crates through cargo. Do not install software.

## Tests

Write integration tests in `tests/<set>.rs`: for every tool at least one test that runs it on fixtures
written to a temp folder and checks the result by reading the output back (page counts, `MARK-` lines via
`TextReader`, sizes, rotation, that text is gone after redaction, and so on). Test error paths that matter
(wrong password, bad range, missing helper). Tests that need a helper program (Office, browser, OCR engine)
must skip with a printed reason when the helper is absent rather than fail.

## Final report

End with: what each tool does and its final `Options` fields; what you verified and how (test names, manual
checks); anything that does not work, is partial, or could not be tested on this machine. Be exact: an
untested path is "untested", not "should work".
