//! What the app knows: loaded files, the current tool, each tool's options, and the edits
//! made in the Pages grid. No rendering here.

use crate::catalog::{self, Control, Field, Inputs, Tool};
use crate::settings::human_size;
use serde_json::{Map, Value, json};
use shorui_core::Outcome;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum RowStatus {
    Idle,
    Queued,
    Running { fraction: f32, what: String },
    Done { bytes_out: u64, pages: usize, outputs: Vec<PathBuf>, notes: Vec<String> },
    Failed { message: String },
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub ext: String,
    pub bytes: u64,
    /// `None` until read, or for files that have no pages (images, Office files).
    pub pages: Option<usize>,
    pub locked: bool,
    /// Why the file could not be read, if it could not.
    pub problem: Option<String>,
    /// Ticked in batch queues.
    pub included: bool,
    /// Page range typed into the row (Merge).
    pub range: String,
    pub status: RowStatus,
}

impl FileEntry {
    pub fn new(path: PathBuf) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        FileEntry { path, name, ext, bytes, pages: None, locked: false, problem: None, included: true, range: String::new(), status: RowStatus::Idle }
    }

    pub fn is_pdf(&self) -> bool {
        self.ext == "pdf"
    }

    pub fn is_image(&self) -> bool {
        matches!(self.ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff" | "webp")
    }
}

/// One page in the Pages grid: which original page it is and the rotation added to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSlot {
    /// 1-based page number in the source file.
    pub source: usize,
    /// Degrees added, a multiple of 90 in 0..360.
    pub turn: i32,
}

/// The pending edit in the Pages tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PagesEdit {
    pub slots: Vec<PageSlot>,
    pub selected: BTreeSet<usize>,
    /// Keyboard cursor, an index into `slots`.
    pub cursor: usize,
    pub original_count: usize,
}

impl PagesEdit {
    pub fn new(page_count: usize) -> Self {
        PagesEdit { slots: (1..=page_count).map(|source| PageSlot { source, turn: 0 }).collect(), selected: BTreeSet::new(), cursor: 0, original_count: page_count }
    }

    pub fn is_changed(&self) -> bool {
        self.slots.len() != self.original_count || self.slots.iter().enumerate().any(|(i, s)| s.source != i + 1 || s.turn != 0)
    }

    /// The slots an action applies to: the selection, or the cursor when nothing is selected.
    pub fn targets(&self) -> Vec<usize> {
        if self.selected.is_empty() {
            if self.cursor < self.slots.len() { vec![self.cursor] } else { vec![] }
        } else {
            self.selected.iter().copied().filter(|i| *i < self.slots.len()).collect()
        }
    }

    pub fn rotate(&mut self, degrees: i32) {
        for i in self.targets() {
            self.slots[i].turn = (self.slots[i].turn + degrees).rem_euclid(360);
        }
    }

    /// Remove the targets. Refuses to remove every page.
    pub fn delete(&mut self) -> bool {
        let targets: BTreeSet<usize> = self.targets().into_iter().collect();
        if targets.is_empty() || targets.len() >= self.slots.len() {
            return false;
        }
        let mut i = 0;
        self.slots.retain(|_| {
            let keep = !targets.contains(&i);
            i += 1;
            keep
        });
        self.selected.clear();
        self.cursor = self.cursor.min(self.slots.len().saturating_sub(1));
        true
    }

    pub fn duplicate(&mut self) {
        let targets = self.targets();
        for &i in targets.iter().rev() {
            let copy = self.slots[i];
            self.slots.insert(i + 1, copy);
        }
        self.selected.clear();
    }

    /// Move the targets so they sit before the slot currently at `before` (or at the end).
    pub fn move_to(&mut self, before: usize) {
        let targets: Vec<usize> = self.targets();
        if targets.is_empty() {
            return;
        }
        let moving: Vec<PageSlot> = targets.iter().map(|i| self.slots[*i]).collect();
        let shift = targets.iter().filter(|i| **i < before).count();
        let set: BTreeSet<usize> = targets.into_iter().collect();
        let mut i = 0;
        self.slots.retain(|_| {
            let keep = !set.contains(&i);
            i += 1;
            keep
        });
        let at = (before - shift).min(self.slots.len());
        for (k, slot) in moving.iter().enumerate() {
            self.slots.insert(at + k, *slot);
        }
        self.selected = (at..at + moving.len()).collect();
        self.cursor = at;
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.slots.len()).collect();
    }

    pub fn select_where(&mut self, keep: impl Fn(usize, &PageSlot) -> bool) {
        self.selected = self.slots.iter().enumerate().filter(|(i, s)| keep(*i, s)).map(|(i, _)| i).collect();
    }

    /// Click behaviour: plain click selects one, with the modifier it toggles, with shift it
    /// extends from the cursor.
    pub fn click(&mut self, index: usize, toggle: bool, extend: bool) {
        if index >= self.slots.len() {
            return;
        }
        if extend {
            let (a, b) = if self.cursor <= index { (self.cursor, index) } else { (index, self.cursor) };
            self.selected.extend(a..=b);
        } else if toggle {
            if !self.selected.remove(&index) {
                self.selected.insert(index);
            }
            self.cursor = index;
        } else {
            self.selected.clear();
            self.selected.insert(index);
            self.cursor = index;
        }
    }

    /// The selection written as a range of positions, e.g. `3-4, 9`.
    pub fn selection_text(&self) -> String {
        let pages: Vec<usize> = self.selected.iter().map(|i| i + 1).collect();
        shorui_core::range::format(&pages)
    }

    /// A short description of each pending change, for the options panel.
    pub fn changes(&self) -> Vec<String> {
        let mut out = Vec::new();
        let kept: BTreeSet<usize> = self.slots.iter().map(|s| s.source).collect();
        let removed: Vec<usize> = (1..=self.original_count).filter(|p| !kept.contains(p)).collect();
        if !removed.is_empty() {
            out.push(format!("Delete {} {}", if removed.len() == 1 { "page" } else { "pages" }, shorui_core::range::format(&removed)));
        }
        let mut seen = BTreeSet::new();
        let copies = self.slots.iter().filter(|s| !seen.insert(s.source)).count();
        if copies > 0 {
            out.push(format!("Add {copies} {}", if copies == 1 { "copy" } else { "copies" }));
        }
        let sources: Vec<usize> = self.slots.iter().map(|s| s.source).collect();
        let mut sorted = sources.clone();
        sorted.sort_unstable();
        if sources != sorted {
            out.push("Change the page order".to_string());
        }
        for turn in [90, 180, 270] {
            let pages: BTreeSet<usize> = self.slots.iter().filter(|s| s.turn == turn).map(|s| s.source).collect();
            if !pages.is_empty() {
                let list: Vec<usize> = pages.into_iter().collect();
                out.push(format!("Rotate {} {} by {turn}°", if list.len() == 1 { "page" } else { "pages" }, shorui_core::range::format(&list)));
            }
        }
        out
    }

    /// Options for the core `pages` tool.
    pub fn to_options(&self) -> Map<String, Value> {
        let order: Vec<usize> = self.slots.iter().map(|s| s.source).collect();
        let mut rotate = Vec::new();
        for turn in [90, 180, 270] {
            let pages: BTreeSet<usize> = self.slots.iter().filter(|s| s.turn == turn).map(|s| s.source).collect();
            if !pages.is_empty() {
                let list: Vec<usize> = pages.into_iter().collect();
                rotate.push(json!({ "pages": shorui_core::range::format(&list), "degrees": turn }));
            }
        }
        let mut map = Map::new();
        map.insert("order".into(), Value::String(order.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")));
        map.insert("rotate".into(), Value::Array(rotate));
        map
    }
}

/// The value of an option as text, for `show_if` checks and for display.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if (f - f.round()).abs() < 1e-6 { format!("{f:.0}") } else { format!("{f:.2}").trim_end_matches('0').trim_end_matches('.').to_string() }
            }
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub mode: Option<&'static str>,
    pub files: Vec<FileEntry>,
    /// Options per tool id. Starts from the tool's own defaults.
    pub options: HashMap<&'static str, Map<String, Value>>,
    pub pages_edit: PagesEdit,
    /// Index into `files` of the file shown by one-file tools.
    pub active_file: usize,
    pub running: bool,
}

impl State {
    pub fn tool(&self) -> Option<&'static Tool> {
        self.mode.and_then(catalog::tool)
    }

    /// The options of a tool, created from its defaults on first use.
    pub fn options_mut(&mut self, tool: &'static str) -> &mut Map<String, Value> {
        self.options.entry(tool).or_insert_with(|| match shorui_core::tools::default_options(tool) {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        })
    }

    pub fn options_of(&mut self, tool: &'static str) -> Map<String, Value> {
        self.options_mut(tool).clone()
    }

    pub fn set_option(&mut self, tool: &'static str, key: &str, value: Value) {
        self.options_mut(tool).insert(key.to_string(), value);
    }

    /// Set an option from text typed or chosen in the panel, keeping the JSON type the tool expects.
    pub fn set_option_text(&mut self, tool: &'static str, field: &Field, text: &str) {
        let current = self.options_mut(tool).get(field.key).cloned();
        let value = match (&field.control, &current) {
            (Control::Switch, _) => Value::Bool(text == "true"),
            // An emptied number field goes back to "decide for me" where the tool allows that.
            (Control::Number { .. }, Some(Value::Null) | None) if text.trim().is_empty() => Value::Null,
            (Control::Number { .. } | Control::Slider { .. }, _) => match text.trim().parse::<f64>() {
                Ok(n) if matches!(current, Some(Value::Number(ref c)) if c.is_i64() || c.is_u64()) => json!(n.round() as i64),
                Ok(n) => json!(n),
                Err(_) => return,
            },
            (_, Some(Value::Number(c))) => match text.trim().parse::<f64>() {
                Ok(n) if c.is_i64() || c.is_u64() => json!(n.round() as i64),
                Ok(n) => json!(n),
                Err(_) => Value::String(text.to_string()),
            },
            (_, Some(Value::Bool(_))) => Value::Bool(text == "true"),
            (Control::File { .. }, _) | (_, Some(Value::Null)) if text.is_empty() => Value::Null,
            _ => Value::String(text.to_string()),
        };
        self.set_option(tool, field.key, value);
    }

    /// Whether a field should be shown, given the tool's current options.
    pub fn field_visible(&mut self, tool: &'static str, field: &Field) -> bool {
        match field.show_if {
            None => true,
            Some((key, values)) => {
                let text = self.options_mut(tool).get(key).map(value_text).unwrap_or_default();
                values.contains(&text.as_str())
            }
        }
    }

    /// Indices of the files the current tool can take, in queue order.
    pub fn visible(&self) -> Vec<usize> {
        match self.tool() {
            Some(tool) => (0..self.files.len()).filter(|i| tool.accepts.contains(&self.files[*i].ext.as_str())).collect(),
            None => (0..self.files.len()).collect(),
        }
    }

    /// The files a run would use: visible, and ticked where the queue has check boxes.
    pub fn run_set(&self) -> Vec<usize> {
        let Some(tool) = self.tool() else { return vec![] };
        let visible = self.visible();
        match tool.inputs {
            Inputs::Each | Inputs::Optional => visible.into_iter().filter(|i| self.files[*i].included).collect(),
            Inputs::Combine | Inputs::Pair => visible,
            Inputs::One => visible.into_iter().filter(|i| *i == self.active_file).chain(None).collect::<Vec<_>>(),
        }
    }

    /// Add files, skipping ones already loaded. Returns how many were added and how many
    /// the current tool cannot use.
    pub fn add_files(&mut self, paths: Vec<PathBuf>) -> (usize, usize) {
        let mut added = 0;
        let mut unusable = 0;
        for path in paths {
            if path.is_dir() {
                let mut inside: Vec<PathBuf> = std::fs::read_dir(&path).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default();
                inside.sort();
                let (a, u) = self.add_files(inside);
                added += a;
                unusable += u;
                continue;
            }
            if self.files.iter().any(|f| f.path == path) {
                continue;
            }
            let entry = FileEntry::new(path);
            if let Some(tool) = self.tool() {
                if !tool.accepts.contains(&entry.ext.as_str()) {
                    unusable += 1;
                }
            }
            self.files.push(entry);
            added += 1;
        }
        self.fix_active();
        (added, unusable)
    }

    pub fn remove_file(&mut self, index: usize) {
        if index < self.files.len() {
            self.files.remove(index);
        }
        self.fix_active();
    }

    pub fn clear_files(&mut self) {
        self.files.clear();
        self.active_file = 0;
        self.pages_edit = PagesEdit::default();
    }

    /// Move a file to a new position in the queue.
    pub fn move_file(&mut self, from: usize, to: usize) {
        if from < self.files.len() && to <= self.files.len() && from != to {
            let entry = self.files.remove(from);
            let at = if to > from { to - 1 } else { to };
            self.files.insert(at.min(self.files.len()), entry);
        }
    }

    /// Keep `active_file` pointing at a file the current tool can show.
    pub fn fix_active(&mut self) {
        let visible = self.visible();
        if !visible.contains(&self.active_file) {
            self.active_file = visible.first().copied().unwrap_or(0);
        }
    }

    pub fn active(&self) -> Option<&FileEntry> {
        let visible = self.visible();
        if visible.contains(&self.active_file) { self.files.get(self.active_file) } else { None }
    }

    pub fn reset_statuses(&mut self) {
        for f in &mut self.files {
            f.status = RowStatus::Idle;
        }
    }

    pub fn apply_outcome(&mut self, row: usize, outcome: Outcome) {
        if let Some(f) = self.files.get_mut(row) {
            f.status = RowStatus::Done { bytes_out: outcome.bytes_out, pages: outcome.pages, outputs: outcome.outputs, notes: outcome.notes };
        }
    }

    /// The left part of the run line: what goes in.
    pub fn input_summary(&self, tool: &Tool) -> String {
        let set = self.run_set();
        if set.is_empty() {
            return match tool.inputs {
                Inputs::Optional => "No files in the queue".into(),
                _ => "No files open".into(),
            };
        }
        let files = set.len();
        let pages: usize = set.iter().filter_map(|i| self.files[*i].pages).sum();
        let bytes: u64 = set.iter().map(|i| self.files[*i].bytes).sum();
        let mut parts = Vec::new();
        if !matches!(tool.inputs, Inputs::One) {
            parts.push(format!("{files} {}", if files == 1 { "file" } else { "files" }));
        }
        if pages > 0 {
            parts.push(format!("{pages} {}", if pages == 1 { "page" } else { "pages" }));
        }
        parts.push(human_size(bytes));
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_edit_round_trip() {
        let mut e = PagesEdit::new(6);
        assert!(!e.is_changed());
        e.click(2, false, false);
        e.click(3, false, true);
        assert_eq!(e.selection_text(), "3-4");
        e.rotate(90);
        e.move_to(0);
        assert_eq!(e.slots.iter().map(|s| s.source).collect::<Vec<_>>(), vec![3, 4, 1, 2, 5, 6]);
        assert_eq!(e.selection_text(), "1-2");
        e.click(5, false, false);
        assert!(e.delete());
        assert_eq!(e.slots.len(), 5);
        let opts = e.to_options();
        assert_eq!(opts["order"], json!("3, 4, 1, 2, 5"));
        assert_eq!(opts["rotate"], json!([{ "pages": "3-4", "degrees": 90 }]));
        let changes = e.changes();
        assert!(changes.contains(&"Delete page 6".to_string()), "{changes:?}");
        assert!(changes.contains(&"Rotate pages 3-4 by 90°".to_string()), "{changes:?}");
        assert!(changes.contains(&"Change the page order".to_string()));
    }

    #[test]
    fn cannot_delete_every_page() {
        let mut e = PagesEdit::new(2);
        e.select_all();
        assert!(!e.delete());
        assert_eq!(e.slots.len(), 2);
    }

    #[test]
    fn move_to_later_position_accounts_for_removed_slots() {
        let mut e = PagesEdit::new(5);
        e.click(0, false, false);
        e.move_to(3);
        assert_eq!(e.slots.iter().map(|s| s.source).collect::<Vec<_>>(), vec![2, 3, 1, 4, 5]);
    }

    #[test]
    fn tools_only_see_files_they_accept() {
        let mut s = State::default();
        s.add_files(vec![PathBuf::from("a.pdf"), PathBuf::from("b.png"), PathBuf::from("c.pdf"), PathBuf::from("a.pdf")]);
        assert_eq!(s.files.len(), 3);
        s.mode = Some("compress");
        assert_eq!(s.visible(), vec![0, 2]);
        s.files[2].included = false;
        assert_eq!(s.run_set(), vec![0]);
        s.mode = Some("img2pdf");
        s.fix_active();
        assert_eq!(s.visible(), vec![1]);
        assert_eq!(s.active_file, 1);
        s.mode = Some("merge");
        assert_eq!(s.run_set(), vec![0, 2], "merge takes every PDF, ticked or not");
    }

    #[test]
    fn show_if_follows_options() {
        let mut s = State::default();
        let tool = catalog::tool("crop").unwrap();
        s.set_option("crop", "mode", json!("resize"));
        let paper = tool.fields.iter().find(|f| f.key == "paper").unwrap();
        let top = tool.fields.iter().find(|f| f.key == "top").unwrap();
        assert!(s.field_visible("crop", paper));
        assert!(!s.field_visible("crop", top));
        s.set_option("nup", "booklet", json!(false));
        let per_sheet = catalog::tool("nup").unwrap().fields.iter().find(|f| f.key == "per_sheet").unwrap();
        assert!(s.field_visible("nup", per_sheet));
    }

    #[test]
    fn typed_values_keep_their_json_type() {
        let mut s = State::default();
        let tool = catalog::tool("nup").unwrap();
        let margin = tool.fields.iter().find(|f| f.key == "margin").unwrap();
        s.set_option("nup", "margin", json!(18.0));
        s.set_option_text("nup", margin, "24");
        assert_eq!(s.options_mut("nup")["margin"], json!(24.0));
        let per_sheet = tool.fields.iter().find(|f| f.key == "per_sheet").unwrap();
        s.set_option("nup", "per_sheet", json!(2));
        s.set_option_text("nup", per_sheet, "4");
        assert_eq!(s.options_mut("nup")["per_sheet"], json!(4));
    }
}
