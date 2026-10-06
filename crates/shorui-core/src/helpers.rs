//! Finding and running helper programs, and a few filesystem conveniences.
//!
//! Three tools cannot be done in pure Rust and lean on software that is already on the
//! machine: Office conversion, HTML to PDF, and OCR on Linux. Everything here is local.

use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Look for a program on PATH, then in the given absolute locations.
pub fn find_program(names: &[&str], locations: &[&str]) -> Option<PathBuf> {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".com", ".cmd", ".bat", ""] } else { &[""] };
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for name in names {
                for ext in exts {
                    let candidate = dir.join(format!("{name}{ext}"));
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
    }
    locations.iter().map(|l| expand_env(l)).find(|p| p.is_file())
}

fn expand_env(path: &str) -> PathBuf {
    let mut out = path.to_string();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA", "HOME"] {
        let token = format!("%{var}%");
        if out.contains(&token) {
            out = out.replace(&token, &std::env::var(var).unwrap_or_default());
        }
    }
    PathBuf::from(out)
}

/// A Chromium-family browser that can print to PDF headlessly.
pub fn find_browser() -> Option<PathBuf> {
    find_program(
        &["msedge", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome", "brave", "brave-browser"],
        &[
            "%ProgramFiles(x86)%\\Microsoft\\Edge\\Application\\msedge.exe",
            "%ProgramFiles%\\Microsoft\\Edge\\Application\\msedge.exe",
            "%ProgramFiles%\\Google\\Chrome\\Application\\chrome.exe",
            "%ProgramFiles(x86)%\\Google\\Chrome\\Application\\chrome.exe",
            "%LOCALAPPDATA%\\Google\\Chrome\\Application\\chrome.exe",
            "%ProgramFiles%\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
        ],
    )
}

pub fn find_libreoffice() -> Option<PathBuf> {
    find_program(
        &["soffice", "libreoffice"],
        &[
            "%ProgramFiles%\\LibreOffice\\program\\soffice.exe",
            "%ProgramFiles(x86)%\\LibreOffice\\program\\soffice.exe",
            "/Applications/LibreOffice.app/Contents/MacOS/soffice",
            "/usr/bin/soffice",
            "/usr/bin/libreoffice",
            "/snap/bin/libreoffice",
        ],
    )
}

pub fn find_tesseract() -> Option<PathBuf> {
    find_program(
        &["tesseract"],
        &["%ProgramFiles%\\Tesseract-OCR\\tesseract.exe", "/opt/homebrew/bin/tesseract", "/usr/local/bin/tesseract", "/usr/bin/tesseract"],
    )
}

/// Run a helper to completion without flashing a console window on Windows.
/// `what` names the job for the error message, e.g. "LibreOffice".
pub fn run(command: &mut Command, what: &str) -> Result<Output> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().map_err(|e| Error::External(format!("{what} could not be started: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr.trim().lines().last().or_else(|| stdout.trim().lines().last()).unwrap_or("no details given");
        return Err(Error::External(format!("{what} failed: {detail}")));
    }
    Ok(output)
}

/// A folder under the system temp directory, removed when dropped.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(prefix: &str) -> Result<Self> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let name = format!("shorui-{prefix}-{}-{stamp:x}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).map_err(|e| Error::write(&path, e))?;
        Ok(Self { path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// `report.pdf` becomes `report (2).pdf` when the name is taken.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(""));
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

/// File name without its extension.
pub fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "document".into())
}

pub fn ensure_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::write(dir, e))
}

/// The lowercase extension of a path, without the dot.
pub fn ext(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}
