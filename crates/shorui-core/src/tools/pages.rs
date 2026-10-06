//! Pages: Reorder, rotate, duplicate and delete pages.

use super::merge::{self, UNPROTECTED_NOTE, plural};
use crate::ctx::file_size;
use crate::{Ctx, Error, Outcome, Result, doc, range};
use lopdf::ObjectId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Rotate {
    /// Original page numbers, as a range (`1-3, 7`). Empty means every page.
    pub pages: String,
    /// Added to the rotation the page already has. Clockwise, a multiple of 90; negative turns left.
    pub degrees: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Options {
    /// The final order, written with ORIGINAL page numbers: `3, 1-2, 5`. Empty keeps the
    /// order. A page left out is removed; a page listed twice is duplicated.
    pub order: String,
    /// Original page numbers to drop from the order.
    pub delete: String,
    pub rotate: Vec<Rotate>,
    /// Original page numbers to follow with a blank page of the same size. `0` puts a
    /// blank page at the very front.
    pub insert_blank_after: Vec<usize>,
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = merge::one_input(inputs, "Pages")?;
    merge::guard_output(inputs, out)?;
    ctx.report(0.0, "Reading the file");
    let mut document = super::unlock::load(input, ctx.password())?;
    let ids = doc::page_ids(&document);
    let count = ids.len();
    if count == 0 {
        return Err(Error::damaged("This file has no pages."));
    }

    // The order, in original page numbers.
    let mut order = range::parse(&opts.order, count)?;
    if !opts.delete.trim().is_empty() {
        let drop: HashSet<usize> = range::parse(&opts.delete, count)?.into_iter().collect();
        order.retain(|p| !drop.contains(p));
    }
    if order.is_empty() {
        return Err(Error::invalid("That would remove every page. Keep at least one page."));
    }
    for &after in &opts.insert_blank_after {
        if after > count {
            return Err(Error::invalid(format!("Cannot add a blank page after page {after}: this file has {}.", plural(count, "page", "pages"))));
        }
    }

    // Rotation is applied to the original pages, so copies of a page turn with it.
    ctx.check()?;
    ctx.report(0.2, "Turning pages");
    for rule in &opts.rotate {
        if rule.degrees % 90 != 0 {
            return Err(Error::invalid(format!("Pages can only be turned in steps of 90 degrees, not {}.", rule.degrees)));
        }
        if rule.degrees.rem_euclid(360) == 0 {
            continue;
        }
        let targets: HashSet<usize> = range::parse(&rule.pages, count)?.into_iter().collect();
        for page in targets {
            let Some(&id) = ids.get(page.wrapping_sub(1)) else { continue };
            let turned = (doc::rotation(&document, id) + rule.degrees).rem_euclid(360);
            document.get_dictionary_mut(id)?.set("Rotate", turned as i64);
        }
    }

    // Build the final page list.
    ctx.check()?;
    ctx.report(0.4, "Arranging pages");
    let blank_after: HashSet<usize> = opts.insert_blank_after.iter().copied().collect();
    let mut used: HashSet<usize> = HashSet::new();
    let mut ordered: Vec<ObjectId> = Vec::with_capacity(order.len());
    let mut duplicated_with_annots = 0usize;
    let mut blanks = 0usize;
    if blank_after.contains(&0) {
        if let Some(&first) = order.first().and_then(|p| ids.get(p.wrapping_sub(1))) {
            let (w, h) = doc::visible_size(&document, first);
            ordered.push(doc::blank_page(&mut document, w, h));
            blanks += 1;
        }
    }
    for &page in &order {
        let Some(&id) = ids.get(page.wrapping_sub(1)) else { continue };
        if used.insert(page) {
            ordered.push(id);
        } else {
            if !merge::annots(&document, id).is_empty() {
                duplicated_with_annots += 1;
            }
            ordered.push(doc::duplicate_page(&mut document, id)?);
        }
        if blank_after.contains(&page) {
            let (w, h) = doc::visible_size(&document, id);
            ordered.push(doc::blank_page(&mut document, w, h));
            blanks += 1;
        }
    }
    let removed = count - used.len();
    doc::set_page_order(&mut document, &ordered)?;

    let mut notes = Vec::new();
    if removed > 0 {
        // Bookmarks and form fields that pointed at the removed pages go with them.
        merge::tidy_form(&mut document);
        if let Some(root) = document.catalog().ok().and_then(|c| c.get(b"Outlines").ok()).and_then(|o| o.as_reference().ok()) {
            let valid: HashSet<ObjectId> = ordered.iter().copied().collect();
            let nodes = merge::read_outline(&document, root, &valid, true);
            merge::write_outline(&mut document, &nodes)?;
        }
        notes.push(format!("Removed {}.", plural(removed, "page", "pages")));
    }
    let skipped: Vec<usize> = blank_after.iter().copied().filter(|p| *p != 0 && !used.contains(p)).collect();
    if blanks > 0 {
        notes.push(format!("Added {}.", plural(blanks, "blank page", "blank pages")));
    }
    if !skipped.is_empty() {
        let mut skipped = skipped;
        skipped.sort_unstable();
        notes.push(format!("No blank page was added after page {} because it is not in the result.", range::format(&skipped)));
    }
    if duplicated_with_annots > 0 {
        notes.push("A page that appears more than once keeps its links, comments and form fields only on its first copy.".into());
    }
    if document.was_encrypted() {
        notes.push(UNPROTECTED_NOTE.into());
    }
    merge::prune(&mut document);

    ctx.check()?;
    ctx.report(0.8, "Saving");
    doc::save(&mut document, out)?;
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), ordered.len(), file_size(input));
    outcome.notes = notes;
    Ok(outcome)
}
