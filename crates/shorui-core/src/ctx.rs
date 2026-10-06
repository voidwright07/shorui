use crate::{Error, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Passed to every tool so it can report progress and stop early.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    progress: Option<&'a (dyn Fn(f32, &str) + Send + Sync)>,
    cancel: Option<&'a AtomicBool>,
    password: Option<&'a str>,
}

impl<'a> Ctx<'a> {
    /// No progress reporting, never cancelled.
    pub const fn none() -> Ctx<'static> {
        Ctx { progress: None, cancel: None, password: None }
    }

    pub fn new(progress: &'a (dyn Fn(f32, &str) + Send + Sync), cancel: &'a AtomicBool) -> Self {
        Ctx { progress: Some(progress), cancel: Some(cancel), password: None }
    }

    /// The password to open encrypted inputs with.
    pub fn with_password(mut self, password: Option<&'a str>) -> Self {
        self.password = password;
        self
    }

    pub fn password(&self) -> Option<&'a str> {
        self.password
    }

    /// A context for one step of a larger job: its progress is mapped into `from..to`.
    /// Returns a closure-free copy when there is no progress sink.
    pub fn progress_span(&self, from: f32, to: f32, fraction: f32, what: &str) {
        self.report(from + (to - from) * fraction.clamp(0.0, 1.0), what);
    }

    /// `fraction` runs from 0.0 to 1.0. `what` is a short phrase such as
    /// "Recompressing images, page 11 of 18".
    pub fn report(&self, fraction: f32, what: &str) {
        if let Some(p) = self.progress {
            p(fraction.clamp(0.0, 1.0), what);
        }
    }

    /// Call between units of work. Returns `Err(Error::Cancelled)` once the user cancels.
    pub fn check(&self) -> Result<()> {
        match self.cancel {
            Some(c) if c.load(Ordering::Relaxed) => Err(Error::Cancelled),
            _ => Ok(()),
        }
    }
}

/// What a tool produced.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Outcome {
    /// Files written, in the order they were written.
    pub outputs: Vec<PathBuf>,
    /// Pages in the output (summed across files).
    pub pages: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// Plain-language remarks worth showing after the run, such as
    /// "2 pages had no text layer and were skipped".
    pub notes: Vec<String>,
}

impl Outcome {
    pub fn single(path: PathBuf, pages: usize, bytes_in: u64) -> Self {
        let bytes_out = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Outcome { outputs: vec![path], pages, bytes_in, bytes_out, notes: Vec::new() }
    }

    pub fn push(&mut self, path: PathBuf) {
        self.bytes_out += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.outputs.push(path);
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

pub fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
