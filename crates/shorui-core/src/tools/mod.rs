//! The 25 tools. Every tool has the same shape:
//!
//! ```ignore
//! pub struct Options { .. }            // serde, every field defaulted
//! pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome>
//! ```
//!
//! `out` is a file path for tools whose `OutputKind` is `File`, and a folder for `Dir`.
//!
//! The tools are split into four cargo features (`set-organise`, `set-edit`, `set-convert`,
//! `set-optimise`), all on by default, so one set can be built and tested on its own.

use crate::{Ctx, Error, Outcome, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Group {
    Organise,
    Convert,
    Edit,
    Secure,
    Optimise,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::Organise, Group::Convert, Group::Edit, Group::Secure, Group::Optimise];
    pub fn name(self) -> &'static str {
        match self {
            Group::Organise => "Organise",
            Group::Convert => "Convert",
            Group::Edit => "Edit",
            Group::Secure => "Secure",
            Group::Optimise => "Optimise",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutputKind {
    /// `out` is the path of the single file to write.
    File,
    /// `out` is a folder; the tool names the files inside it.
    Dir,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ToolInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub group: Group,
    pub output: OutputKind,
    /// Usual extension of the output, without the dot.
    pub ext: &'static str,
    pub about: &'static str,
}

macro_rules! tools {
    ($(($id:literal, $module:ident, $feat:literal, $name:literal, $group:ident, $kind:expr, $ext:literal, $about:literal),)*) => {
        $(#[cfg(feature = $feat)] pub mod $module;)*

        pub const ALL: &[ToolInfo] = &[
            $(ToolInfo { id: $id, name: $name, group: Group::$group, output: $kind, ext: $ext, about: $about },)*
        ];

        /// Run a tool by id with options given as JSON. Missing options take their defaults.
        pub fn run_json(id: &str, inputs: &[PathBuf], out: &Path, options: &serde_json::Value, ctx: &Ctx) -> Result<Outcome> {
            let _ = (inputs, out, options, ctx);
            match id {
                $(#[cfg(feature = $feat)] $id => {
                    let opts: $module::Options = serde_json::from_value(options.clone())
                        .map_err(|e| Error::invalid(format!("These options are not valid for {}: {e}", $name)))?;
                    $module::run(inputs, out, &opts, ctx)
                })*
                other => Err(Error::invalid(format!("There is no tool called \"{other}\"."))),
            }
        }

        /// The default options of a tool, as JSON.
        pub fn default_options(id: &str) -> Option<serde_json::Value> {
            match id {
                $(#[cfg(feature = $feat)] $id => serde_json::to_value($module::Options::default()).ok(),)*
                _ => None,
            }
        }
    };
}

tools! {
    ("merge", merge, "set-organise", "Merge", Organise, OutputKind::File, "pdf", "Combine several files into one PDF."),
    ("split", split, "set-organise", "Split", Organise, OutputKind::Dir, "pdf", "Break one PDF into several."),
    ("pages", pages, "set-organise", "Pages", Organise, OutputKind::File, "pdf", "Reorder, rotate, duplicate and delete pages."),
    ("crop", crop, "set-organise", "Crop & Resize", Organise, OutputKind::File, "pdf", "Trim page margins or change the page size."),
    ("nup", nup, "set-organise", "N-up / Booklet", Organise, OutputKind::File, "pdf", "Put several pages on one sheet, or impose a booklet."),
    ("img2pdf", img2pdf, "set-convert", "Images to PDF", Convert, OutputKind::File, "pdf", "Turn images into a PDF, one image per page."),
    ("pdf2img", pdf2img, "set-convert", "PDF to Images", Convert, OutputKind::Dir, "png", "Save pages as PNG or JPEG images."),
    ("office", office, "set-convert", "Office to/from PDF", Convert, OutputKind::File, "pdf", "Convert Word, Excel and PowerPoint files to PDF, or a PDF to Word."),
    ("html", html, "set-convert", "HTML/URL to PDF", Convert, OutputKind::File, "pdf", "Print a web page or HTML file to PDF."),
    ("csv2pdf", csv2pdf, "set-convert", "CSV to PDF", Convert, OutputKind::File, "pdf", "Lay out a CSV file as a table in a PDF."),
    ("pdfa", pdfa, "set-convert", "PDF/A", Convert, OutputKind::File, "pdf", "Convert to the PDF/A archival format."),
    ("tables", tables, "set-convert", "Extract Tables", Convert, OutputKind::Dir, "csv", "Pull tables out of a PDF as CSV."),
    ("edit", edit, "set-edit", "Edit & Fill", Edit, OutputKind::File, "pdf", "Fill form fields and add text, check marks and dates."),
    ("sign", sign, "set-edit", "Sign", Edit, OutputKind::File, "pdf", "Place a signature image on a page."),
    ("watermark", watermark, "set-edit", "Watermark", Edit, OutputKind::File, "pdf", "Stamp text or an image across pages."),
    ("numbers", numbers, "set-edit", "Page Numbers & Bates", Edit, OutputKind::File, "pdf", "Add page numbers, headers, footers or Bates numbers."),
    ("flatten", flatten, "set-edit", "Flatten", Edit, OutputKind::File, "pdf", "Turn form fields and annotations into fixed page content."),
    ("protect", protect, "set-organise", "Protect", Secure, OutputKind::File, "pdf", "Encrypt a PDF with a password."),
    ("unlock", unlock, "set-organise", "Unlock", Secure, OutputKind::File, "pdf", "Remove the password from a PDF you can open."),
    ("redact", redact, "set-edit", "Redact", Secure, OutputKind::File, "pdf", "Permanently remove marked content."),
    ("strip", strip, "set-organise", "Strip Metadata", Secure, OutputKind::File, "pdf", "Remove author, title, dates and other hidden information."),
    ("compress", compress, "set-optimise", "Compress", Optimise, OutputKind::File, "pdf", "Make a PDF smaller."),
    ("ocr", ocr, "set-optimise", "OCR", Optimise, OutputKind::File, "pdf", "Add a searchable text layer to scanned pages."),
    ("repair", repair, "set-optimise", "Repair", Optimise, OutputKind::File, "pdf", "Rebuild a damaged PDF."),
    ("compare", compare, "set-optimise", "Compare", Optimise, OutputKind::Dir, "pdf", "Find the differences between two PDFs."),
}

pub fn info(id: &str) -> Option<&'static ToolInfo> {
    ALL.iter().find(|t| t.id == id)
}
