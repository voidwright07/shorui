//! What the command palette lists for a query. Pure logic, so it can be tested without a window.

use crate::catalog::{self, TOOLS, Tool};
use crate::fuzzy::fuzzy;
use shorui_core::tools::Group;
use std::path::PathBuf;

/// The modifier this platform uses for shortcuts, as printed on a key cap.
pub fn mod_label() -> &'static str {
    if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Switch mode. Loaded files stay when the tool accepts them.
    OpenTool(&'static str),
    /// Switch mode and set one option first.
    OpenToolWith { tool: &'static str, key: &'static str, value: &'static str },
    AddFiles,
    GoHome,
    ToggleTheme,
    ToggleSidebar,
    RunCurrent,
    ClearQueue,
    /// Show the most recent result in the system's file browser.
    ShowResult,
    ToggleFavorite,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    Suggested,
    Modes,
    Actions,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub section: Section,
    pub icon: &'static str,
    pub title: String,
    /// Byte ranges of `title` to highlight.
    pub title_hits: Vec<(usize, usize)>,
    pub subtitle: String,
    /// Byte ranges of `subtitle` to highlight (when the match was on a keyword).
    pub subtitle_hits: Vec<(usize, usize)>,
    pub group: Option<Group>,
    pub keys: Vec<String>,
    pub command: Command,
    score: i32,
}

/// What the palette needs to know about the app.
#[derive(Debug, Clone, Default)]
pub struct Situation {
    pub recent: Vec<String>,
    pub active_tool: Option<&'static str>,
    pub files: Vec<PathBuf>,
    pub dark: bool,
    pub favorites: Vec<String>,
}

/// What each tool is also known as, and a one-line description shown beside the name.
fn about(id: &str) -> (&'static str, &'static [&'static str]) {
    match id {
        "merge" => ("Combine files into one", &["combine", "join", "append", "concatenate"]),
        "split" => ("Break a file into several", &["separate", "extract pages", "divide", "burst"]),
        "pages" => ("Reorder, rotate and delete pages", &["reorder", "rotate", "delete pages", "rearrange", "organise pages"]),
        "crop" => ("Trim margins or change page size", &["trim", "margins", "page size", "scale"]),
        "nup" => ("Several pages per sheet", &["imposition", "2-up", "4-up", "handout", "booklet"]),
        "img2pdf" => ("One image per page", &["jpg to pdf", "png to pdf", "photos", "scan to pdf"]),
        "pdf2img" => ("Save pages as PNG or JPEG", &["export images", "pdf to jpg", "pdf to png", "rasterise"]),
        "office" => ("Word, Excel and PowerPoint", &["word", "excel", "powerpoint", "docx", "xlsx", "pptx"]),
        "html" => ("Print a web page", &["web page", "website", "url", "print page"]),
        "csv2pdf" => ("Print a spreadsheet as a table", &["csv", "tsv", "spreadsheet", "table to pdf", "data"]),
        "pdfa" => ("Archival compliance", &["archive", "long-term", "iso 19005"]),
        "tables" => ("Tables as CSV", &["csv", "spreadsheet", "data", "excel"]),
        "edit" => ("Fill forms and add text", &["form", "fill in", "add text", "type on pdf", "annotate"]),
        "sign" => ("Place a signature", &["signature", "initials", "autograph"]),
        "watermark" => ("Stamp text or an image", &["stamp", "confidential", "draft", "overlay"]),
        "numbers" => ("Numbers, headers and Bates", &["page numbers", "header", "footer", "bates", "numbering"]),
        "flatten" => ("Make forms and comments permanent", &["lock form", "merge annotations", "bake"]),
        "protect" => ("Add a password", &["encrypt", "password", "lock", "restrict"]),
        "unlock" => ("Remove a password", &["decrypt", "remove password", "unprotect"]),
        "redact" => ("Permanently remove content", &["black out", "censor", "hide text", "remove sensitive"]),
        "strip" => ("Remove hidden information", &["metadata", "author", "exif", "clean", "sanitise", "privacy"]),
        "compress" => ("Shrink file size", &["reduce size", "smaller", "optimise", "shrink"]),
        "ocr" => ("Make scans searchable", &["searchable", "recognise text", "scan", "text layer"]),
        "repair" => ("Rebuild a damaged file", &["fix", "recover", "corrupt", "broken"]),
        "compare" => ("Find differences between two files", &["diff", "differences", "changes", "versions"]),
        _ => ("", &[]),
    }
}

/// A small nudge so that, between equally good matches, the tools people reach for most
/// come first ("comp" gives Compress before Compare).
fn popularity(id: &str) -> i32 {
    match id {
        "merge" | "compress" | "split" | "pages" => 8,
        "sign" | "redact" | "ocr" | "protect" | "edit" => 4,
        _ => 0,
    }
}

fn chord(tool: &Tool) -> Vec<String> {
    vec!["G".into(), tool.chord.to_uppercase()]
}

fn mode_entry(tool: &'static Tool, section: Section) -> Entry {
    Entry {
        section,
        icon: tool.icon,
        title: tool.name.to_string(),
        title_hits: Vec::new(),
        subtitle: String::new(),
        subtitle_hits: Vec::new(),
        group: Some(tool.group),
        keys: chord(tool),
        command: Command::OpenTool(tool.id),
        score: 0,
    }
}

fn file_name(path: &PathBuf) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

struct ActionDef {
    icon: &'static str,
    title: String,
    subtitle: String,
    group: Option<Group>,
    keys: Vec<String>,
    command: Command,
}

fn actions(s: &Situation) -> Vec<ActionDef> {
    let m = mod_label().to_string();
    let mut out = Vec::new();
    let mut add = |icon, title: String, subtitle: &str, group, keys: Vec<String>, command| out.push(ActionDef { icon, title, subtitle: subtitle.to_string(), group, keys, command });
    let pdfs = s.files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))).count();

    if let Some(tool) = s.active_tool.and_then(catalog::tool) {
        add(tool.icon, format!("{}: run now", tool.name), "", Some(tool.group), vec![m.clone(), "Enter".into()], Command::RunCurrent);
        let favorite = s.favorites.iter().any(|f| f == tool.id);
        add(if favorite { "star-fill" } else { "star" }, if favorite { format!("Remove {} from favorites", tool.name) } else { format!("Add {} to favorites", tool.name) }, "", Some(tool.group), vec![], Command::ToggleFavorite);
    }
    if pdfs > 0 {
        let what = if pdfs == 1 { file_name(&s.files[0]) } else { format!("the {pdfs} open files") };
        add("compress", format!("Compress {what}"), "Balanced preset", Some(Group::Optimise), vec![m.clone(), "Shift".into(), "C".into()], Command::OpenToolWith { tool: "compress", key: "preset", value: "balanced" });
    }
    add("compress", "Compress with the Strong preset".into(), "", Some(Group::Optimise), vec![], Command::OpenToolWith { tool: "compress", key: "preset", value: "strong" });
    add("compress", "Compress with the Light preset".into(), "", Some(Group::Optimise), vec![], Command::OpenToolWith { tool: "compress", key: "preset", value: "light" });
    add("nup", "Make a booklet".into(), "Fold and staple", Some(Group::Organise), vec![], Command::OpenToolWith { tool: "nup", key: "booklet", value: "true" });
    add("numbers", "Add Bates numbers".into(), "", Some(Group::Edit), vec![], Command::OpenToolWith { tool: "numbers", key: "mode", value: "bates" });
    add("pdf2img", "Export pages as JPEG".into(), "", Some(Group::Convert), vec![], Command::OpenToolWith { tool: "pdf2img", key: "format", value: "jpg" });
    add("plus", "Add files".into(), "", None, vec![m.clone(), "O".into()], Command::AddFiles);
    add("x", "Clear the queue".into(), "", None, vec![], Command::ClearQueue);
    add("home", "Go to Home".into(), "", None, vec![m.clone(), "H".into()], Command::GoHome);
    add(if s.dark { "sun" } else { "moon" }, format!("Switch to the {} theme", if s.dark { "light" } else { "dark" }), "", None, vec![m.clone(), "Shift".into(), "L".into()], Command::ToggleTheme);
    add("sidebar", "Collapse or expand the sidebar".into(), "", None, vec![m.clone(), "B".into()], Command::ToggleSidebar);
    add("folder", "Show the last result in its folder".into(), "", None, vec![], Command::ShowResult);
    add("x", "Quit Shorui".into(), "", None, vec![m, "Q".into()], Command::Quit);
    out
}

/// The rows to show, already ordered and grouped by section.
pub fn entries(query: &str, s: &Situation) -> Vec<Entry> {
    let query = query.trim();
    if query.is_empty() {
        return empty_query(s);
    }
    let mut modes: Vec<Entry> = Vec::new();
    for tool in TOOLS {
        let (desc, keywords) = about(tool.id);
        let mut best: Option<Entry> = None;
        if let Some(m) = fuzzy(query, tool.name).filter(|m| compact(&m.ranges, tool.name)) {
            let mut e = mode_entry(tool, Section::Modes);
            e.title_hits = m.ranges;
            e.subtitle = desc.to_string();
            e.score = m.score + 40 + popularity(tool.id);
            best = Some(e);
        }
        for text in std::iter::once(desc).chain(keywords.iter().copied()) {
            if let Some(m) = fuzzy(query, text) {
                // Scattered letters across a description are noise: ask for one run.
                let tight = m.ranges.len() == 1;
                if tight && best.as_ref().is_none_or(|b| m.score > b.score) {
                    let mut e = mode_entry(tool, Section::Modes);
                    e.subtitle = capitalise(text);
                    e.subtitle_hits = m.ranges;
                    e.score = m.score;
                    best = Some(e);
                }
            }
        }
        modes.extend(best);
    }
    modes.sort_by(|a, b| b.score.cmp(&a.score));
    modes.truncate(6);

    let mut acts: Vec<Entry> = actions(s)
        .into_iter()
        .filter_map(|a| {
            let m = fuzzy(query, &a.title).filter(|m| compact(&m.ranges, &a.title))?;
            Some(Entry {
                section: Section::Actions,
                icon: a.icon,
                title: a.title,
                title_hits: m.ranges,
                subtitle: a.subtitle,
                subtitle_hits: Vec::new(),
                group: a.group,
                keys: a.keys,
                command: a.command,
                score: m.score,
            })
        })
        .collect();
    acts.sort_by(|a, b| b.score.cmp(&a.score));
    acts.truncate(6);
    modes.extend(acts);
    modes
}

/// A match reads as intended when it is one run, or when every run starts a word
/// ("sm" for Strip Metadata). Letters scattered through the middle of words are noise.
fn compact(ranges: &[(usize, usize)], text: &str) -> bool {
    if ranges.len() <= 1 {
        return true;
    }
    let bytes = text.as_bytes();
    ranges.iter().all(|(start, _)| *start == 0 || !bytes[*start - 1].is_ascii_alphanumeric())
}

fn capitalise(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn empty_query(s: &Situation) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut recent: Vec<&'static Tool> = s.recent.iter().filter_map(|id| catalog::tool(id)).filter(|t| Some(t.id) != s.active_tool).collect();
    if recent.is_empty() {
        recent = ["merge", "compress", "pages"].iter().filter_map(|id| catalog::tool(id)).filter(|t| Some(t.id) != s.active_tool).collect();
    }
    for tool in recent.into_iter().take(4) {
        out.push(mode_entry(tool, Section::Recent));
    }
    if let Some(file) = s.files.first() {
        let name = file_name(file);
        let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        for id in catalog::suggested_for(&ext).iter().filter(|id| Some(**id) != s.active_tool).take(4) {
            let Some(tool) = catalog::tool(id) else { continue };
            let title = match *id {
                "ocr" => format!("Make {name} searchable"),
                "pages" => format!("Reorder pages of {name}"),
                "img2pdf" => format!("Turn {name} into a PDF"),
                "office" => format!("Convert {name}"),
                "html" | "csv2pdf" => format!("Print {name} to PDF"),
                _ => format!("{} {name}", tool.name),
            };
            let mut e = mode_entry(tool, Section::Suggested);
            e.title = title;
            e.keys = Vec::new();
            out.push(e);
        }
    } else {
        for a in actions(s).into_iter().filter(|a| matches!(a.command, Command::AddFiles | Command::ToggleTheme)) {
            out.push(Entry { section: Section::Suggested, icon: a.icon, title: a.title, title_hits: vec![], subtitle: a.subtitle, subtitle_hits: vec![], group: a.group, keys: a.keys, command: a.command, score: 0 });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(query: &str, s: &Situation) -> Vec<String> {
        entries(query, s).into_iter().map(|e| e.title).collect()
    }

    #[test]
    fn comp_ranks_compress_first() {
        let s = Situation::default();
        let rows = entries("comp", &s);
        assert_eq!(rows[0].title, "Compress");
        assert_eq!(rows[0].command, Command::OpenTool("compress"));
        assert_eq!(rows[0].title_hits, vec![(0, 4)]);
        assert_eq!(rows[1].title, "Compare");
        let pdfa = rows.iter().find(|e| e.title == "PDF/A").expect("PDF/A matches on 'compliance'");
        assert_eq!(pdfa.subtitle, "Archival compliance");
        assert_eq!(pdfa.subtitle_hits, vec![(9, 13)]);
        assert!(rows.iter().any(|e| e.section == Section::Actions && e.title.starts_with("Compress with")));
    }

    #[test]
    fn keywords_find_tools() {
        let s = Situation::default();
        assert_eq!(titles("encrypt", &s)[0], "Protect");
        assert_eq!(titles("searchable", &s)[0], "OCR");
        assert_eq!(titles("combine", &s)[0], "Merge");
        assert_eq!(titles("bates", &s)[0], "Page Numbers & Bates");
        assert!(titles("zzzz", &s).is_empty());
    }

    #[test]
    fn empty_query_offers_recent_and_suggestions() {
        let s = Situation { recent: vec!["redact".into(), "merge".into()], files: vec![PathBuf::from("Q3-report.pdf")], active_tool: Some("merge"), ..Default::default() };
        let rows = entries("", &s);
        assert_eq!(rows[0].title, "Redact");
        assert_eq!(rows[0].section, Section::Recent);
        assert!(rows.iter().all(|e| e.command != Command::OpenTool("merge")), "the active tool is not offered");
        let suggested: Vec<&str> = rows.iter().filter(|e| e.section == Section::Suggested).map(|e| e.title.as_str()).collect();
        assert_eq!(suggested[0], "Compress Q3-report.pdf");
        assert!(suggested.contains(&"Split Q3-report.pdf"));
        assert!(suggested.contains(&"Make Q3-report.pdf searchable"));
    }

    #[test]
    fn theme_action_reflects_state() {
        let dark = Situation { dark: true, ..Default::default() };
        assert!(titles("theme", &dark).iter().any(|t| t == "Switch to the light theme"));
        let light = Situation::default();
        assert!(titles("theme", &light).iter().any(|t| t == "Switch to the dark theme"));
    }
}
