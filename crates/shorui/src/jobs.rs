//! Running tools off the UI thread, and deciding where their output goes.

use crate::catalog::{Inputs, Tool};
use serde_json::{Map, Value};
use shorui_core::tools::{self, OutputKind};
use shorui_core::{Ctx, Outcome, helpers};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// One unit of work: usually one input file and its output.
#[derive(Debug, Clone)]
pub struct Item {
    pub inputs: Vec<PathBuf>,
    pub out: PathBuf,
    /// Index of the queue row this item reports against.
    pub row: usize,
}

#[derive(Debug)]
pub enum Event {
    Started { row: usize },
    Progress { row: usize, fraction: f32, what: String },
    Done { row: usize, outcome: Outcome },
    Failed { row: usize, message: String },
    /// Every item has been handled, or the run was cancelled.
    Finished { cancelled: bool },
}

pub struct Handle {
    cancel: Arc<AtomicBool>,
    pub events: Receiver<Event>,
}

impl Handle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Options keys starting with `@` are for the interface, not the tool.
pub fn split_options(options: &Map<String, Value>) -> (Value, Option<String>) {
    let mut tool_options = Map::new();
    let mut password = None;
    for (k, v) in options {
        if k == "@password" {
            password = v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        } else if !k.starts_with('@') {
            tool_options.insert(k.clone(), v.clone());
        }
    }
    (Value::Object(tool_options), password)
}

/// Start a run on a background thread. Items are processed in order; a failed item
/// does not stop the rest.
pub fn start(tool_id: &'static str, items: Vec<Item>, options: Map<String, Value>) -> Handle {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let flag = cancel.clone();
    std::thread::Builder::new()
        .name(format!("shorui-{tool_id}"))
        .spawn(move || run_all(tool_id, items, options, flag, tx))
        .expect("could not start a worker thread");
    Handle { cancel, events: rx }
}

fn run_all(tool_id: &'static str, items: Vec<Item>, options: Map<String, Value>, cancel: Arc<AtomicBool>, tx: Sender<Event>) {
    let (tool_options, password) = split_options(&options);
    let mut cancelled = false;
    for item in items {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let row = item.row;
        let _ = tx.send(Event::Started { row });
        let progress_tx = tx.clone();
        let progress = move |fraction: f32, what: &str| {
            let _ = progress_tx.send(Event::Progress { row, fraction, what: what.to_string() });
        };
        let ctx = Ctx::new(&progress, &cancel).with_password(password.as_deref());
        // A tool must never take the app down with it.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tools::run_json(tool_id, &item.inputs, &item.out, &tool_options, &ctx)));
        let event = match result {
            Ok(Ok(outcome)) => Event::Done { row, outcome },
            Ok(Err(shorui_core::Error::Cancelled)) => {
                cancelled = true;
                let _ = tx.send(Event::Failed { row, message: "Cancelled.".into() });
                break;
            }
            Ok(Err(e)) => Event::Failed { row, message: e.to_string() },
            Err(_) => Event::Failed { row, message: "Something went wrong inside this tool. The file was not changed.".into() },
        };
        let _ = tx.send(event);
    }
    let _ = tx.send(Event::Finished { cancelled });
}

/// The extension a tool writes for a given input.
fn output_ext(tool: &Tool, input: Option<&Path>, options: &Map<String, Value>) -> String {
    match tool.id {
        "office" => {
            let to = options.get("to").and_then(Value::as_str).unwrap_or("");
            if !to.is_empty() {
                to.to_string()
            } else if input.map(helpers::ext).as_deref() == Some("pdf") {
                "docx".into()
            } else {
                "pdf".into()
            }
        }
        _ => tools::info(tool.id).map(|i| i.ext).unwrap_or("pdf").to_string(),
    }
}

/// Work out the items of a run and where each writes. `files` are the queue rows in order.
/// Returns an explanation instead when the queue does not suit the tool.
pub fn plan(tool: &Tool, files: &[PathBuf], out_dir: &Path, options: &Map<String, Value>) -> Result<Vec<Item>, String> {
    let kind = tools::info(tool.id).map(|i| i.output).unwrap_or(OutputKind::File);
    let target = |input: Option<&Path>, stem: &str| -> PathBuf {
        match kind {
            OutputKind::File => helpers::unique_path(&out_dir.join(format!("{stem}{}.{}", tool.suffix, output_ext(tool, input, options)))),
            OutputKind::Dir => helpers::unique_path(&out_dir.join(format!("{stem}{}", tool.suffix))),
        }
    };
    let none = || format!("Add at least one file to {}.", tool.verb.to_lowercase());
    match tool.inputs {
        Inputs::Each => {
            if files.is_empty() {
                return Err(none());
            }
            Ok(files.iter().enumerate().map(|(row, f)| Item { inputs: vec![f.clone()], out: target(Some(f), &helpers::stem(f)), row }).collect())
        }
        Inputs::Combine => {
            let first = files.first().ok_or_else(none)?;
            let stem = match options.get("@name").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                Some(name) => name.trim().trim_end_matches(".pdf").to_string(),
                None => helpers::stem(first),
            };
            Ok(vec![Item { inputs: files.to_vec(), out: target(Some(first), &stem), row: 0 }])
        }
        Inputs::Pair => {
            if files.len() != 2 {
                return Err("Compare needs exactly two files: the old version, then the new one.".into());
            }
            Ok(vec![Item { inputs: files.to_vec(), out: target(Some(&files[0]), &helpers::stem(&files[1])), row: 0 }])
        }
        Inputs::One => {
            let active = options.get("@active").and_then(Value::as_u64).unwrap_or(0) as usize;
            let file = files.get(active).or_else(|| files.first()).ok_or_else(none)?;
            Ok(vec![Item { inputs: vec![file.clone()], out: target(Some(file), &helpers::stem(file)), row: active.min(files.len().saturating_sub(1)) }])
        }
        Inputs::Optional => {
            let url = options.get("url").and_then(Value::as_str).unwrap_or("").trim();
            if !url.is_empty() {
                let stem: String = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
                    .take(60)
                    .collect();
                let stem = stem.trim_matches('-').to_string();
                return Ok(vec![Item { inputs: vec![], out: target(None, if stem.is_empty() { "page" } else { &stem }), row: 0 }]);
            }
            if files.is_empty() {
                return Err("Add an HTML file, or type a web address in the options.".into());
            }
            Ok(files.iter().enumerate().map(|(row, f)| Item { inputs: vec![f.clone()], out: target(Some(f), &helpers::stem(f)), row }).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tool;

    fn files(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn plans_each_combine_and_pair() {
        let out = Path::new("out-that-does-not-exist");
        let none = Map::new();
        let each = plan(tool("compress").unwrap(), &files(&["a.pdf", "b.pdf"]), out, &none).unwrap();
        assert_eq!(each.len(), 2);
        assert_eq!(each[1].out, out.join("b-compressed.pdf"));
        assert_eq!(each[1].row, 1);

        let combine = plan(tool("merge").unwrap(), &files(&["a.pdf", "b.pdf"]), out, &none).unwrap();
        assert_eq!(combine.len(), 1);
        assert_eq!(combine[0].inputs.len(), 2);
        assert_eq!(combine[0].out, out.join("a-merged.pdf"));

        assert!(plan(tool("compare").unwrap(), &files(&["a.pdf"]), out, &none).is_err());
        let pair = plan(tool("compare").unwrap(), &files(&["a.pdf", "b.pdf"]), out, &none).unwrap();
        assert_eq!(pair[0].out, out.join("b-comparison"));

        let split = plan(tool("split").unwrap(), &files(&["a.pdf"]), out, &none).unwrap();
        assert_eq!(split[0].out, out.join("a-split"));
        assert!(plan(tool("compress").unwrap(), &[], out, &none).is_err());
    }

    #[test]
    fn office_and_html_pick_their_extension() {
        let out = Path::new("out-that-does-not-exist");
        let none = Map::new();
        let office = plan(tool("office").unwrap(), &files(&["a.docx", "b.pdf"]), out, &none).unwrap();
        assert_eq!(office[0].out, out.join("a.pdf"));
        assert_eq!(office[1].out, out.join("b.docx"));
        let mut opts = Map::new();
        opts.insert("url".into(), Value::String("https://example.com/a b".into()));
        let html = plan(tool("html").unwrap(), &[], out, &opts).unwrap();
        assert_eq!(html[0].out, out.join("example.com-a-b.pdf"));
        assert!(html[0].inputs.is_empty());
    }

    #[test]
    fn interface_keys_are_not_sent_to_tools() {
        let mut opts = Map::new();
        opts.insert("@password".into(), Value::String("secret".into()));
        opts.insert("@active".into(), Value::from(2));
        opts.insert("dpi".into(), Value::from(150));
        let (tool_opts, password) = split_options(&opts);
        assert_eq!(password.as_deref(), Some("secret"));
        assert_eq!(tool_opts, serde_json::json!({ "dpi": 150 }));
    }
}
