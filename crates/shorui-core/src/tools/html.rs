//! HTML/URL to PDF: Print a web page or HTML file to PDF.
//!
//! The printing is done by a Chromium-family browser that is already on the machine
//! (Chrome, Edge, Chromium or Brave), run headless. Chromium takes the paper size,
//! orientation and margins from the page's own CSS rather than from the command line, so
//! for a local file Shorui prints a temporary copy with a small `@page` stylesheet added.
//! The copy lives in the system temp folder and carries a `<base>` pointing back at the
//! original folder, so images and stylesheets next to the file still load.
//!
//! A local file is printed offline: the browser is pointed at a proxy that does not exist,
//! so nothing is fetched from the internet unless `allow_remote` is set. A web address
//! given in `url` is the one case in Shorui where the network is used.

use crate::{Ctx, Error, Outcome, Result, doc, helpers};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Paper {
    #[default]
    A4,
    Letter,
    A3,
    Legal,
}

impl Paper {
    /// Portrait width and height as CSS lengths.
    fn css(self) -> (&'static str, &'static str) {
        match self {
            Paper::A4 => ("210mm", "297mm"),
            Paper::Letter => ("8.5in", "11in"),
            Paper::A3 => ("297mm", "420mm"),
            Paper::Legal => ("8.5in", "14in"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Web address or `file:` URL to print when no input file is given. Only `http`,
    /// `https` and `file` are accepted.
    pub url: Option<String>,
    pub paper: Paper,
    pub landscape: bool,
    /// Print background colours and images.
    pub background: bool,
    /// `true` keeps the browser's normal print margins; `false` prints edge to edge.
    pub margins: bool,
    /// How long scripts on the page get to finish before printing, in milliseconds of
    /// the browser's virtual time.
    pub wait_ms: u32,
    /// Print the browser's header and footer (date, title, address, page number).
    pub header_footer: bool,
    /// Let a local HTML file load images, fonts and styles from the internet. Off by
    /// default, because Shorui works offline. Has no effect on `url`.
    pub allow_remote: bool,
    /// Give up and stop the browser after this many seconds.
    pub timeout_secs: u32,
    /// Use this browser instead of looking for one. Must be a Chromium-family browser.
    pub browser_path: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            url: None,
            paper: Paper::A4,
            landscape: false,
            background: true,
            margins: true,
            wait_ms: 2000,
            header_footer: false,
            allow_remote: false,
            timeout_secs: 120,
            browser_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Running a helper with a time limit (shared with the Office tool)
// ---------------------------------------------------------------------------

pub(crate) struct Finished {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Finished {
    /// The last line the helper printed, for an error message.
    pub fn detail(&self) -> String {
        let last = |s: &str| s.lines().map(str::trim).filter(|l| !l.is_empty()).next_back().map(str::to_string);
        last(&self.stderr).or_else(|| last(&self.stdout)).unwrap_or_else(|| match self.code {
            Some(code) => format!("it stopped with exit code {code} and gave no details"),
            None => "it stopped and gave no details".into(),
        })
    }
}

/// Stop a process and everything it started.
pub(crate) fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Run a helper to completion without a console window, like `helpers::run`, but stop it
/// when it takes longer than `timeout` or when the user cancels. Output is collected in
/// `scratch` (a temp folder) rather than through pipes, so a chatty helper cannot stall.
/// A helper that exits with an error is returned as `Finished { success: false, .. }`.
pub(crate) fn run_limited(command: &mut Command, what: &str, timeout: Duration, ctx: &Ctx, scratch: &Path) -> Result<Finished> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static RUN: AtomicU32 = AtomicU32::new(0);
    let n = RUN.fetch_add(1, Ordering::Relaxed);
    let out_path = scratch.join(format!("helper-{n}.out"));
    let err_path = scratch.join(format!("helper-{n}.err"));
    let out_file = std::fs::File::create(&out_path).map_err(|e| Error::write(&out_path, e))?;
    let err_file = std::fs::File::create(&err_path).map_err(|e| Error::write(&err_path, e))?;
    command.stdin(Stdio::null()).stdout(out_file).stderr(err_file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| Error::External(format!("{what} could not be started: {e}")))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_tree(&mut child);
                return Err(Error::External(format!("{what} could not be watched: {e}")));
            }
        }
        if ctx.check().is_err() {
            kill_tree(&mut child);
            return Err(Error::Cancelled);
        }
        if started.elapsed() > timeout {
            kill_tree(&mut child);
            return Err(Error::External(format!("{what} did not finish within {} seconds and was stopped.", timeout.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let read = |p: &Path| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    Ok(Finished { success: status.success(), code: status.code(), stdout: read(&out_path), stderr: read(&err_path) })
}

// ---------------------------------------------------------------------------
// file: URLs
// ---------------------------------------------------------------------------

fn percent_encode(text: &[u8], out: &mut String) {
    for &b in text {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

/// Build a `file:` URL from an absolute path given as text. `windows` selects the
/// Windows rules (drive letters, backslashes, UNC shares) whatever the host is.
fn file_url_from(absolute: &[u8], windows: bool) -> String {
    let mut url = String::from("file://");
    if windows {
        let text = String::from_utf8_lossy(absolute).replace('\\', "/");
        let text = text.strip_prefix("//?/UNC/").map(|rest| format!("//{rest}")).unwrap_or_else(|| text.strip_prefix("//?/").unwrap_or(&text).to_string());
        match text.strip_prefix("//") {
            // \\server\share\file  ->  file://server/share/file
            Some(unc) => percent_encode(unc.as_bytes(), &mut url),
            None => {
                url.push('/');
                percent_encode(text.trim_start_matches('/').as_bytes(), &mut url);
            }
        }
    } else {
        percent_encode(absolute, &mut url);
    }
    url
}

/// The `file:` URL of a path, with spaces and other special characters escaped.
pub fn file_url(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path).map_err(|e| Error::read(path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(file_url_from(absolute.as_os_str().as_bytes(), false))
    }
    #[cfg(not(unix))]
    {
        Ok(file_url_from(absolute.to_string_lossy().as_bytes(), cfg!(windows)))
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit) && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit) {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
            out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// The local path a `file:` URL names, or `None` when it is not a plain local file URL.
fn path_from_file_url(url: &str, windows: bool) -> Option<PathBuf> {
    let rest = url.get(..7).filter(|p| p.eq_ignore_ascii_case("file://")).map(|_| &url[7..])?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let decoded = String::from_utf8(percent_decode(rest)).ok()?;
    if windows {
        let text = match decoded.strip_prefix('/') {
            Some(local) => local.to_string(),               // file:///C:/dir/file
            None if decoded.starts_with("localhost/") => decoded["localhost/".len()..].to_string(),
            None => format!("//{decoded}"),                  // file://server/share/file
        };
        Some(PathBuf::from(text.replace('/', "\\")))
    } else {
        let text = decoded.strip_prefix("localhost").unwrap_or(&decoded);
        text.starts_with('/').then(|| PathBuf::from(text))
    }
}

// ---------------------------------------------------------------------------
// The injected stylesheet
// ---------------------------------------------------------------------------

/// The stylesheet that carries the paper size, orientation, margins and background choice.
fn print_css(opts: &Options) -> String {
    let (w, h) = opts.paper.css();
    let (w, h) = if opts.landscape { (h, w) } else { (w, h) };
    let mut css = format!("@page {{ size: {w} {h} !important;");
    if !opts.margins {
        css.push_str(" margin: 0 !important;");
    }
    css.push_str(" }");
    if opts.background {
        css.push_str(" :root, * { -webkit-print-color-adjust: exact !important; print-color-adjust: exact !important; }");
    } else {
        // Headless Chromium always prints backgrounds, so take them away instead.
        css.push_str(" *, *::before, *::after { background: transparent none !important; box-shadow: none !important; }");
    }
    css
}

fn find_ci(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Where an opening tag such as `<head ...>` ends, if the document has one.
fn after_open_tag(html: &[u8], tag: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(at) = find_ci(html, tag, from) {
        let next = html.get(at + tag.len()).copied();
        if matches!(next, Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'/')) {
            return html[at..].iter().position(|&b| b == b'>').map(|p| at + p + 1);
        }
        from = at + tag.len();
    }
    None
}

/// Where a closing tag such as `</head>` starts. `</header>` is not `</head>`.
fn close_tag(html: &[u8], tag: &[u8], from: usize) -> Option<usize> {
    let mut from = from;
    while let Some(at) = find_ci(html, tag, from) {
        if matches!(html.get(at + tag.len()), Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')) {
            return Some(at);
        }
        from = at + tag.len();
    }
    None
}

/// A copy of an HTML document with a `<base>` (so relative links keep working from another
/// folder) and a print stylesheet added. Works on bytes, so any ASCII-compatible encoding
/// passes through untouched; UTF-16 is converted to UTF-8 first.
fn inject(html: &[u8], base_href: &str, css: &str) -> Vec<u8> {
    let mut charset = "";
    let converted: Vec<u8>;
    let html: &[u8] = if html.starts_with(&[0xFF, 0xFE]) || html.starts_with(&[0xFE, 0xFF]) {
        let little = html[0] == 0xFF;
        let units: Vec<u16> = html[2..].chunks_exact(2).map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        converted = String::from_utf16_lossy(&units).into_bytes();
        charset = "<meta charset=\"utf-8\">";
        &converted
    } else {
        html
    };

    let has_base = after_open_tag(html, b"<base").is_some();
    let base = if has_base { String::new() } else { format!("<base href=\"{base_href}\">") };
    let head_start = format!("{charset}{base}");
    let style = format!("<style>{css}</style>");

    let start = after_open_tag(html, b"<head")
        .or_else(|| after_open_tag(html, b"<html"))
        .or_else(|| find_ci(html, b"<!doctype", 0).and_then(|at| html[at..].iter().position(|&b| b == b'>').map(|p| at + p + 1)))
        .unwrap_or(if html.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 });
    // The stylesheet goes last so that it wins over the page's own @page rules.
    let end = close_tag(html, b"</head", start).or_else(|| close_tag(html, b"</body", start)).unwrap_or(html.len());

    let mut out = Vec::with_capacity(html.len() + head_start.len() + style.len());
    out.extend_from_slice(&html[..start]);
    out.extend_from_slice(head_start.as_bytes());
    out.extend_from_slice(&html[start..end]);
    out.extend_from_slice(style.as_bytes());
    out.extend_from_slice(&html[end..]);
    out
}

/// Whether the page asks for files from the internet.
fn refers_to_remote(html: &[u8]) -> bool {
    let lower = html.to_ascii_lowercase();
    let patterns: [&[u8]; 12] = [
        b"src=\"http", b"src='http", b"src=http", b"src=\"//", b"src='//", b"url(http", b"url(\"http", b"url('http", b"url(//", b"@import \"http", b"@import 'http", b"rel=\"stylesheet\" href=\"http",
    ];
    patterns.iter().any(|p| find_ci(&lower, p, 0).is_some())
}

// ---------------------------------------------------------------------------
// The tool
// ---------------------------------------------------------------------------

fn missing_browser() -> Error {
    Error::MissingHelper {
        tool: "A Chromium-based browser".into(),
        hint: "Install Google Chrome, Microsoft Edge, Chromium or Brave, then try again. Shorui uses it to print web pages to PDF.".into(),
    }
}

fn browser(opts: &Options) -> Result<PathBuf> {
    match opts.browser_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(given) => {
            let path = PathBuf::from(given);
            if path.is_file() {
                Ok(path)
            } else {
                Err(Error::MissingHelper { tool: format!("The browser at {given}"), hint: "Check the path, or leave it empty to let Shorui look for Chrome, Edge, Chromium or Brave.".into() })
            }
        }
        None => helpers::find_browser().ok_or_else(missing_browser),
    }
}

/// What will be printed.
enum Source {
    /// A local HTML file.
    File(PathBuf),
    /// A web address.
    Web(String),
}

fn source(inputs: &[PathBuf], opts: &Options) -> Result<Source> {
    match inputs {
        [file] => {
            let ext = helpers::ext(file);
            if !matches!(ext.as_str(), "html" | "htm" | "xhtml") {
                return Err(Error::invalid("Choose an .html or .htm file, or give a web address instead."));
            }
            Ok(Source::File(file.clone()))
        }
        [] => {
            let url = opts.url.as_deref().map(str::trim).filter(|u| !u.is_empty()).ok_or_else(|| Error::invalid("Choose an HTML file or enter a web address to print."))?;
            let lower = url.to_ascii_lowercase();
            if lower.starts_with("file://") {
                let path = path_from_file_url(url, cfg!(windows)).ok_or_else(|| Error::invalid("That file address could not be understood. Choose the HTML file instead."))?;
                Ok(Source::File(path))
            } else if (lower.starts_with("http://") || lower.starts_with("https://")) && !url.chars().any(|c| c.is_whitespace() || c.is_control()) {
                Ok(Source::Web(url.to_string()))
            } else {
                Err(Error::invalid("Enter an address that starts with http://, https:// or file://."))
            }
        }
        _ => Err(Error::invalid("HTML to PDF prints one file at a time.")),
    }
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let source = source(inputs, opts)?;
    let browser = browser(opts)?;
    let browser_name = browser_name(&browser);
    let temp = helpers::TempDir::new("html")?;
    let mut notes = Vec::new();
    let mut bytes_in = 0u64;

    ctx.report(0.05, "Preparing the page");
    let (target, offline) = match &source {
        Source::File(path) => {
            let html = doc::read_file(path)?;
            bytes_in = html.len() as u64;
            let folder = std::path::absolute(path).map_err(|e| Error::read(path, e))?.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut base = file_url(&folder)?;
            if !base.ends_with('/') {
                base.push('/');
            }
            if !opts.allow_remote && refers_to_remote(&html) {
                notes.push("This page refers to files on the internet. They were not loaded, because Shorui prints local files offline. Turn on \"allow remote content\" to load them.".to_string());
            }
            let copy = temp.path().join(format!("{}.html", safe_name(&helpers::stem(path))));
            doc::write_file(&copy, &inject(&html, &base, &print_css(opts)))?;
            (file_url(&copy)?, !opts.allow_remote)
        }
        Source::Web(url) => {
            notes.push(
                "Paper size, orientation, margins and the background setting cannot be passed to the browser for a web address, so the page's own print layout was used (US Letter, portrait, unless the page says otherwise). Save the page as an HTML file and print that to choose them."
                    .to_string(),
            );
            (url.clone(), false)
        }
    };

    let pdf = temp.path().join("page.pdf");
    let profile = temp.path().join("profile");
    let timeout = Duration::from_secs(opts.timeout_secs.clamp(5, 3600) as u64) + Duration::from_millis(opts.wait_ms as u64);
    let mut last_failure = String::new();
    let mut printed = false;
    for headless in ["--headless=new", "--headless"] {
        ctx.check()?;
        ctx.report(0.2, &format!("Printing with {browser_name}"));
        let _ = std::fs::remove_file(&pdf);
        let mut command = Command::new(&browser);
        command.arg(headless).args([
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-sync",
            "--disable-default-apps",
            "--disable-component-update",
            "--disable-background-networking",
            "--no-pings",
            "--hide-scrollbars",
            "--run-all-compositor-stages-before-draw",
        ]);
        if !opts.header_footer {
            // The second spelling is what browsers older than Chromium 112 know.
            command.args(["--no-pdf-header-footer", "--print-to-pdf-no-header"]);
        }
        if offline {
            command.args(["--proxy-server=127.0.0.1:9", "--proxy-bypass-list=<-loopback>"]);
        }
        command.arg(format!("--virtual-time-budget={}", opts.wait_ms.min(600_000)));
        command.arg(format!("--user-data-dir={}", profile.display()));
        command.arg(format!("--print-to-pdf={}", pdf.display()));
        command.arg(&target);
        let finished = run_limited(&mut command, &browser_name, timeout, ctx, temp.path())?;
        let ok = std::fs::read(&pdf).map(|b| b.starts_with(b"%PDF")).unwrap_or(false);
        if ok {
            printed = true;
            break;
        }
        last_failure = load_failure(&finished.stderr).unwrap_or_else(|| strip_log_prefix(&finished.detail()));
        // A page that cannot be loaded will not load in the other headless mode either.
        if load_failure(&finished.stderr).is_some() {
            break;
        }
    }
    if !printed {
        return Err(Error::External(format!("{browser_name} could not print the page: {}.", last_failure.trim_end_matches('.'))));
    }

    ctx.report(0.9, "Saving the PDF");
    let bytes = doc::read_file(&pdf)?;
    let pages = doc::load_bytes(&bytes, None).map(|d| doc::page_count(&d)).unwrap_or(0);
    doc::write_file(out, &bytes)?;
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), pages, bytes_in);
    outcome.notes = notes;
    Ok(outcome)
}

/// Chromium log lines start with `[pid:thread:date/time:LEVEL:file:line]`.
fn strip_log_prefix(line: &str) -> String {
    match line.strip_prefix('[').and_then(|rest| rest.split_once("] ")) {
        Some((_, message)) => message.trim().to_string(),
        None => line.trim().to_string(),
    }
}

/// Why the page could not be loaded, in plain words, when the browser's log says so.
fn load_failure(log: &str) -> Option<String> {
    let at = log.find("net::ERR_")?;
    let code: String = log[at + 5..].chars().take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_').collect();
    let reason = match code.as_str() {
        "ERR_NAME_NOT_RESOLVED" => "the address could not be found. Check the spelling and your internet connection",
        "ERR_INTERNET_DISCONNECTED" | "ERR_NETWORK_CHANGED" | "ERR_ADDRESS_UNREACHABLE" => "there is no internet connection",
        "ERR_CONNECTION_REFUSED" | "ERR_CONNECTION_RESET" | "ERR_CONNECTION_CLOSED" => "the site refused the connection",
        "ERR_CONNECTION_TIMED_OUT" | "ERR_TIMED_OUT" => "the site took too long to answer",
        "ERR_FILE_NOT_FOUND" => "the file does not exist",
        "ERR_ACCESS_DENIED" => "the file could not be opened",
        "ERR_CERT_AUTHORITY_INVALID" | "ERR_CERT_DATE_INVALID" | "ERR_CERT_COMMON_NAME_INVALID" => "the site's security certificate is not valid",
        _ => return Some(format!("the page could not be loaded ({code})")),
    };
    Some(format!("the page could not be loaded, because {reason}"))
}

/// What to call the browser in messages.
fn browser_name(path: &Path) -> String {
    let stem = helpers::stem(path).to_lowercase();
    if stem.contains("edge") {
        "Microsoft Edge".into()
    } else if stem.contains("brave") {
        "Brave".into()
    } else if stem.contains("chromium") {
        "Chromium".into()
    } else if stem.contains("chrome") {
        "Google Chrome".into()
    } else {
        "The browser".into()
    }
}

/// A file name the browser and every file system accept.
fn safe_name(stem: &str) -> String {
    let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
    if clean.is_empty() { "page".into() } else { clean }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_file_urls() {
        assert_eq!(file_url_from(br"C:\Users\Ana Maria\My Docs\a b#1.html", true), "file:///C:/Users/Ana%20Maria/My%20Docs/a%20b%231.html");
        assert_eq!(file_url_from(br"\\server\share\x y.html", true), "file://server/share/x%20y.html");
        assert_eq!(file_url_from(br"\\?\C:\long\path.html", true), "file:///C:/long/path.html");
        assert_eq!(file_url_from(br"\\?\UNC\server\share\a.html", true), "file://server/share/a.html");
        assert_eq!(file_url_from("/home/zoë/r%port.html".as_bytes(), false), "file:///home/zo%C3%AB/r%25port.html");
        assert_eq!(file_url_from(b"/Users/me/Web Pages/a.html", false), "file:///Users/me/Web%20Pages/a.html");
    }

    #[test]
    fn reads_file_urls() {
        assert_eq!(path_from_file_url("file:///C:/My%20Docs/a.html", true), Some(PathBuf::from(r"C:\My Docs\a.html")));
        assert_eq!(path_from_file_url("file://server/share/a.html", true), Some(PathBuf::from(r"\\server\share\a.html")));
        assert_eq!(path_from_file_url("FILE:///home/me/a%20b.html#top", false), Some(PathBuf::from("/home/me/a b.html")));
        assert_eq!(path_from_file_url("file://localhost/home/me/a.html", false), Some(PathBuf::from("/home/me/a.html")));
        assert_eq!(path_from_file_url("https://example.com/", false), None);
    }

    #[test]
    fn injects_base_and_style() {
        let css = "@page { size: 210mm 297mm !important; }";
        let out = String::from_utf8(inject(b"<!DOCTYPE html><HTML><HEAD lang=en><title>t</title></HEAD><body><header>x</header></body></HTML>", "file:///d/", css)).unwrap();
        assert_eq!(out, format!("<!DOCTYPE html><HTML><HEAD lang=en><base href=\"file:///d/\"><title>t</title><style>{css}</style></HEAD><body><header>x</header></body></HTML>"));

        // No head: the base goes first, the style before </body>.
        let out = String::from_utf8(inject(b"<body><header>x</header><p>hi</p></body>", "file:///d/", css)).unwrap();
        assert_eq!(out, format!("<base href=\"file:///d/\"><body><header>x</header><p>hi</p><style>{css}</style></body>"));

        // A page with its own <base> keeps it.
        let out = String::from_utf8(inject(b"<html><head><base href=\"https://example.com/\"></head></html>", "file:///d/", css)).unwrap();
        assert!(!out.contains("file:///d/"));
        assert!(out.contains("<style>@page"));

        // UTF-16 is converted.
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "<html><head></head><body>\u{017B}</body></html>".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        let out = String::from_utf8(inject(&utf16, "file:///d/", css)).unwrap();
        assert!(out.contains("<meta charset=\"utf-8\"><base href=\"file:///d/\">") && out.contains('\u{017B}'));
    }

    #[test]
    fn css_follows_options() {
        let css = print_css(&Options { paper: Paper::Letter, landscape: true, margins: false, background: false, ..Default::default() });
        assert!(css.contains("size: 11in 8.5in !important; margin: 0 !important;"));
        assert!(css.contains("background: transparent none !important"));
        let css = print_css(&Options::default());
        assert!(css.contains("size: 210mm 297mm !important; }") && css.contains("print-color-adjust: exact"));
    }

    #[test]
    fn explains_load_failures() {
        let log = r"[1764:2444:1002/194246.467:ERROR:components\headless\command_handler\headless_command_handler.cc:403] Page load failed: net::ERR_NAME_NOT_RESOLVED.";
        assert!(load_failure(log).unwrap().contains("could not be found"));
        assert_eq!(load_failure("x net::ERR_SOMETHING_NEW y").unwrap(), "the page could not be loaded (ERR_SOMETHING_NEW)");
        assert_eq!(load_failure("all fine"), None);
        assert_eq!(strip_log_prefix(log), "Page load failed: net::ERR_NAME_NOT_RESOLVED.");
        assert_eq!(strip_log_prefix("plain message"), "plain message");
    }

    #[test]
    fn spots_remote_references() {
        assert!(refers_to_remote(b"<img SRC=\"https://example.com/a.png\">"));
        assert!(refers_to_remote(b"<style>body{background:url(//cdn.example.com/a.png)}</style>"));
        assert!(!refers_to_remote(b"<a href=\"https://example.com\">link</a><img src=\"a.png\">"));
    }
}
