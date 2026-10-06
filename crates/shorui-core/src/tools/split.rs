//! Split: Break one PDF into several.

use super::merge::{self, UNPROTECTED_NOTE, plural};
use crate::ctx::file_size;
use crate::{Ctx, Error, Outcome, Result, doc, helpers, range};
use lopdf::ObjectId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// One file per group in `ranges`.
    Ranges,
    /// A new file every `every` pages.
    Every,
    /// One file per page.
    #[default]
    Each,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub mode: Mode,
    /// Groups separated by `;` (or a new line). Each group is a page range and becomes
    /// one file: `1-3; 4-6; 7-`.
    pub ranges: String,
    /// Pages per file in `every` mode.
    pub every: usize,
    /// File name for each part, without `.pdf`. `{name}` is the input's name, `{n}` the
    /// part number (zero-padded), `{range}` the pages in the part.
    pub name_pattern: String,
    /// Carry over the bookmarks that point into each part.
    pub keep_bookmarks: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { mode: Mode::Each, ranges: String::new(), every: 1, name_pattern: "{name}-{n}".into(), keep_bookmarks: true }
    }
}

/// The page groups a split would produce for a file with `page_count` pages: one list
/// of 1-based page numbers per output file. The app uses this to preview a split.
pub fn plan(opts: &Options, page_count: usize) -> Result<Vec<Vec<usize>>> {
    if page_count == 0 {
        return Err(Error::damaged("This file has no pages, so there is nothing to split."));
    }
    match opts.mode {
        Mode::Each => Ok((1..=page_count).map(|p| vec![p]).collect()),
        Mode::Every => {
            if opts.every == 0 {
                return Err(Error::invalid("Pages per file must be at least 1."));
            }
            let all: Vec<usize> = (1..=page_count).collect();
            Ok(all.chunks(opts.every).map(<[usize]>::to_vec).collect())
        }
        Mode::Ranges => {
            let mut groups = Vec::new();
            for group in opts.ranges.split([';', '\n']) {
                if group.trim().is_empty() {
                    continue;
                }
                groups.push(range::parse(group, page_count)?);
            }
            if groups.is_empty() {
                return Err(Error::invalid("Enter the page ranges to split into, separated by semicolons, for example 1-3; 4-6; 7-."));
            }
            Ok(groups)
        }
    }
}

/// Make a string safe to use as a file name on Windows, macOS and Linux.
fn safe_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() { "part".into() } else { cleaned }
}

/// The file names (without folder, with `.pdf`) a split would write, in order.
pub fn part_names(opts: &Options, input: &Path, groups: &[Vec<usize>]) -> Vec<String> {
    let stem = helpers::stem(input);
    let mut pattern = if opts.name_pattern.trim().is_empty() { "{name}-{n}".to_string() } else { opts.name_pattern.trim().to_string() };
    if groups.len() > 1 && !pattern.contains("{n}") && !pattern.contains("{range}") {
        pattern.push_str("-{n}");
    }
    let width = groups.len().to_string().len();
    let mut used: HashSet<String> = HashSet::new();
    groups
        .iter()
        .enumerate()
        .map(|(i, pages)| {
            let number = format!("{:0width$}", i + 1);
            let pages_text = range::format(pages).replace(", ", ",");
            let base = safe_name(&pattern.replace("{name}", &stem).replace("{n}", &number).replace("{range}", &pages_text));
            let mut name = base.clone();
            let mut k = 2;
            while !used.insert(name.to_lowercase()) {
                name = format!("{base} ({k})");
                k += 1;
            }
            format!("{name}.pdf")
        })
        .collect()
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = merge::one_input(inputs, "Split")?;
    ctx.report(0.0, "Reading the file");
    let mut src = super::unlock::load(input, ctx.password())?;
    let ids = doc::page_ids(&src);
    let groups = plan(opts, ids.len())?;
    let names = part_names(opts, input, &groups);
    helpers::ensure_dir(out)?;

    let mut outcome = Outcome { bytes_in: file_size(input), ..Default::default() };
    let total = groups.len();
    for (i, (pages, name)) in groups.iter().zip(&names).enumerate() {
        ctx.check()?;
        ctx.report(i as f32 / total as f32, &format!("Writing part {} of {total}", i + 1));
        let chosen: Vec<ObjectId> = pages.iter().filter_map(|p| ids.get(p.wrapping_sub(1)).copied()).collect();

        let (mut part, root) = doc::new_document();
        let imported = merge::import(&mut part, &mut src, &chosen, true, opts.keep_bookmarks)?;
        doc::append_pages(&mut part, root, &imported.pages)?;
        if let Some(outline_root) = imported.outline_root {
            let valid: HashSet<ObjectId> = imported.pages.iter().copied().collect();
            let nodes = merge::read_outline(&part, outline_root, &valid, false);
            merge::write_outline(&mut part, &nodes)?;
        }
        merge::install_forms(&mut part, imported.form.into_iter().collect())?;
        merge::copy_info(&mut part, &src, &["Title", "Author", "Subject", "Keywords", "Creator", "CreationDate"])?;
        merge::prune(&mut part);

        let mut path = out.join(name);
        if merge::same_file(std::fs::canonicalize(&path).ok().as_deref(), &path, input) {
            path = helpers::unique_path(&path);
        }
        doc::save(&mut part, &path)?;
        outcome.pages += imported.pages.len();
        outcome.push(path);
    }

    if opts.mode == Mode::Ranges {
        let covered: HashSet<usize> = groups.iter().flatten().copied().collect();
        let missing: Vec<usize> = (1..=ids.len()).filter(|p| !covered.contains(p)).collect();
        if !missing.is_empty() {
            outcome.notes.push(format!(
                "{} {} {} not in any part.",
                if missing.len() == 1 { "Page" } else { "Pages" },
                range::format(&missing),
                if missing.len() == 1 { "is" } else { "are" }
            ));
        }
    }
    outcome.notes.insert(0, format!("Wrote {}.", plural(total, "file", "files")));
    if src.was_encrypted() {
        outcome.notes.push(UNPROTECTED_NOTE.replace("The result is", "The parts are"));
    }
    ctx.report(1.0, "Done");
    Ok(outcome)
}
