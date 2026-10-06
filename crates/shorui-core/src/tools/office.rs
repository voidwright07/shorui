//! Office to/from PDF: Convert Word, Excel and PowerPoint files to PDF, or a PDF to Word.
//!
//! The conversion is done by an office suite that is already installed:
//!
//! - LibreOffice, run headless. Works on Windows, macOS and Linux.
//! - Microsoft Office, on Windows only, driven through COM automation from a PowerShell
//!   script. Word handles text documents and PDF to DOCX, Excel handles spreadsheets and
//!   PowerPoint handles presentations.
//!
//! Both run locally. The input file is opened read-only and the result is written to a
//! temp folder first, then copied to the output path.

use super::html::{Finished, file_url, run_limited};
use crate::{Ctx, Error, Outcome, Result, doc, helpers};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// LibreOffice when it is installed, otherwise Microsoft Office on Windows. A PDF
    /// never goes through Microsoft Word on this setting: without LibreOffice it is
    /// converted by Shorui itself, text only, because Word shows itself while it opens a PDF.
    #[default]
    Auto,
    Libreoffice,
    Msoffice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// `"pdf"` or `"docx"`. `None` picks PDF for office files and DOCX for a PDF.
    pub to: Option<String>,
    pub engine: Engine,
    /// Give up and stop the office program after this many seconds.
    pub timeout_secs: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { to: None, engine: Engine::Auto, timeout_secs: 90 }
    }
}

/// Which program opens a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Excel,
    PowerPoint,
    Pdf,
}

impl Kind {
    fn of(path: &Path) -> Option<Kind> {
        match helpers::ext(path).as_str() {
            "doc" | "docx" | "docm" | "dot" | "dotx" | "rtf" | "odt" | "txt" => Some(Kind::Word),
            "xls" | "xlsx" | "xlsm" | "xlsb" | "csv" | "ods" => Some(Kind::Excel),
            "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "odp" => Some(Kind::PowerPoint),
            "pdf" => Some(Kind::Pdf),
            _ => None,
        }
    }

    /// The Microsoft Office program for this kind: display name, executable, COM class.
    fn ms_app(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Kind::Word | Kind::Pdf => ("Microsoft Word", "WINWORD", "Word.Application"),
            Kind::Excel => ("Microsoft Excel", "EXCEL", "Excel.Application"),
            Kind::PowerPoint => ("Microsoft PowerPoint", "POWERPNT", "PowerPoint.Application"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Pdf,
    Docx,
}

impl Target {
    fn ext(self) -> &'static str {
        match self {
            Target::Pdf => "pdf",
            Target::Docx => "docx",
        }
    }
}

fn target(kind: Kind, to: Option<&str>) -> Result<Target> {
    let asked = match to.map(|t| t.trim().trim_start_matches('.').to_ascii_lowercase()) {
        None => None,
        Some(t) if t.is_empty() => None,
        Some(t) if t == "pdf" => Some(Target::Pdf),
        Some(t) if t == "docx" => Some(Target::Docx),
        Some(t) => return Err(Error::invalid(format!("Shorui cannot convert to \"{t}\". Choose PDF or DOCX."))),
    };
    match (kind, asked) {
        (Kind::Pdf, None | Some(Target::Docx)) => Ok(Target::Docx),
        (Kind::Pdf, Some(Target::Pdf)) => Err(Error::invalid("This file is already a PDF. Choose DOCX to turn it into a Word document.")),
        (_, None | Some(Target::Pdf)) => Ok(Target::Pdf),
        (_, Some(Target::Docx)) => Err(Error::invalid("Only a PDF can be converted to DOCX here. Office files are converted to PDF.")),
    }
}

// ---------------------------------------------------------------------------
// Finding the engines
// ---------------------------------------------------------------------------

/// Where a Microsoft Office program is installed, if it is. Always `None` off Windows.
fn ms_app_path(kind: Kind) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let (_, exe, class) = kind.ms_app();
    let mut places = Vec::new();
    for root in ["%ProgramFiles%", "%ProgramFiles(x86)%"] {
        for dir in ["Microsoft Office\\root\\Office16", "Microsoft Office\\Office16", "Microsoft Office\\root\\Office15", "Microsoft Office\\Office15", "Microsoft Office\\Office14"] {
            places.push(format!("{root}\\{dir}\\{exe}.EXE"));
        }
    }
    let places: Vec<&str> = places.iter().map(String::as_str).collect();
    if let Some(found) = helpers::find_program(&[exe], &places) {
        return Some(found);
    }
    // Installed somewhere unusual (for example from the Store): ask the registry
    // whether the automation class exists.
    let registered = helpers::run(Command::new("reg").args(["query", &format!("HKCR\\{class}\\CLSID")]), "The registry").is_ok();
    registered.then(|| PathBuf::from(format!("{exe}.EXE")))
}

/// The engines found on this machine, for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Engines {
    pub libreoffice: bool,
    /// Microsoft Word (Windows only). Excel and PowerPoint are checked when a file needs them.
    pub msoffice: bool,
}

/// Which conversion engines are installed.
pub fn engines() -> Engines {
    Engines { libreoffice: helpers::find_libreoffice().is_some(), msoffice: ms_app_path(Kind::Word).is_some() }
}

fn missing_libreoffice() -> Error {
    let hint = if cfg!(windows) {
        "Install LibreOffice (free, from libreoffice.org) or Microsoft Office, then try again."
    } else {
        "Install LibreOffice (free, from libreoffice.org or your package manager), then try again."
    };
    Error::MissingHelper { tool: "LibreOffice".into(), hint: hint.into() }
}

enum Chosen {
    Libre(PathBuf),
    Ms,
    /// Shorui's own text-only PDF to DOCX.
    Builtin,
}

fn choose(engine: Engine, kind: Kind) -> Result<Chosen> {
    let ms = || -> Result<Chosen> {
        if !cfg!(windows) {
            return Err(Error::Unsupported("Microsoft Office can only be driven on Windows. Choose LibreOffice instead.".into()));
        }
        let (name, _, _) = kind.ms_app();
        match ms_app_path(kind) {
            Some(_) => Ok(Chosen::Ms),
            None => Err(Error::MissingHelper { tool: name.into(), hint: "Install Microsoft Office or LibreOffice, then try again.".into() }),
        }
    };
    match engine {
        Engine::Libreoffice => helpers::find_libreoffice().map(Chosen::Libre).ok_or_else(missing_libreoffice),
        Engine::Msoffice => ms(),
        Engine::Auto => match helpers::find_libreoffice() {
            Some(path) => Ok(Chosen::Libre(path)),
            // Word cannot open a PDF without showing a notice on screen, so it is only used
            // for that when the user asks for it by name.
            None if kind == Kind::Pdf => Ok(Chosen::Builtin),
            None if cfg!(windows) && ms_app_path(kind).is_some() => Ok(Chosen::Ms),
            None => Err(missing_libreoffice()),
        },
    }
}

// ---------------------------------------------------------------------------
// LibreOffice
// ---------------------------------------------------------------------------

fn with_libreoffice(soffice: &Path, input: &Path, kind: Kind, to: Target, temp: &Path, timeout: Duration, ctx: &Ctx) -> Result<PathBuf> {
    let out_dir = temp.join("out");
    helpers::ensure_dir(&out_dir)?;
    // A profile of its own, so a LibreOffice window the user has open is not disturbed
    // and does not swallow the request.
    let profile = file_url(&temp.join("profile"))?;
    let mut command = Command::new(soffice);
    command.args(["--headless", "--norestore", "--nolockcheck", "--nodefault", "--nofirststartwizard"]);
    command.arg(format!("-env:UserInstallation={profile}"));
    match (kind, to) {
        (Kind::Pdf, _) => {
            command.arg("--infilter=writer_pdf_import").args(["--convert-to", "docx:MS Word 2007 XML"]);
        }
        _ => {
            command.args(["--convert-to", "pdf"]);
        }
    }
    command.arg("--outdir").arg(&out_dir).arg(input);
    let finished = run_limited(&mut command, "LibreOffice", timeout, ctx, temp)?;
    let produced = std::fs::read_dir(&out_dir).ok().and_then(|entries| entries.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| helpers::ext(p) == to.ext()));
    produced.ok_or_else(|| Error::External(format!("LibreOffice could not convert this file: {}.", finished.detail())))
}

// ---------------------------------------------------------------------------
// Microsoft Office
// ---------------------------------------------------------------------------

/// The PowerShell script that drives one Office program. Paths and the password arrive in
/// environment variables, so nothing from the user is ever part of the script text. The
/// script holds no double quotes, which keeps it intact as a single `-Command` argument.
///
/// The script notes which process it started (`$mine`: the one process of that program
/// that appeared while the application object was created) so that a stuck run can be
/// stopped without touching a copy of the program the user, or anything else, has open.
/// When more than one appeared at that moment it cannot tell which is its own, and then
/// it stops none of them.
fn ms_script(kind: Kind, to: Target) -> String {
    let (_, exe, class) = kind.ms_app();
    let work = match (kind, to) {
        (Kind::Word, _) => {
            "$app.Visible = $false\n    $app.DisplayAlerts = 0\n    try { $app.AutomationSecurity = 3 } catch {}\n    $docs = $app.Documents\n    \
             $doc = $docs.Open($env:SHORUI_IN, $false, $true, $false, $env:SHORUI_PW)\n    $doc.ExportAsFixedFormat($env:SHORUI_OUT, 17)"
        }
        (Kind::Pdf, _) => {
            "$app.Visible = $true\n    $app.DisplayAlerts = 0\n    try { $app.AutomationSecurity = 3 } catch {}\n    $docs = $app.Documents\n    \
             if ($mine.Count -eq 1) { $watch = [PowerShell]::Create(); [void]$watch.AddScript($confirm).AddArgument($mine[0]); $pending = $watch.BeginInvoke() }\n    \
             $doc = $docs.Open($env:SHORUI_IN, $false, $true, $false, $env:SHORUI_PW)\n    $app.Visible = $false\n    $doc.SaveAs2($env:SHORUI_OUT, 16)"
        }
        (Kind::Excel, _) => {
            "$app.Visible = $false\n    $app.DisplayAlerts = $false\n    try { $app.AutomationSecurity = 3 } catch {}\n    try { $app.AskToUpdateLinks = $false } catch {}\n    $docs = $app.Workbooks\n    \
             $doc = $docs.Open($env:SHORUI_IN, 0, $true, [Type]::Missing, $env:SHORUI_PW)\n    $doc.ExportAsFixedFormat(0, $env:SHORUI_OUT)"
        }
        (Kind::PowerPoint, _) => {
            "try { $app.DisplayAlerts = 1 } catch {}\n    $docs = $app.Presentations\n    \
             $doc = $docs.Open($env:SHORUI_IN, -1, 0, 0)\n    $doc.SaveAs($env:SHORUI_OUT, 32)"
        }
    };
    let close = match kind {
        Kind::Word | Kind::Pdf => "$doc.Close(0)",
        Kind::Excel => "$doc.Close($false)",
        Kind::PowerPoint => "$doc.Close()",
    };
    // PowerPoint is one shared instance: if the user already has it open, leave it open.
    let quit = match kind {
        // Word's Quit takes its arguments by reference: a plain `Quit(0)` throws in
        // PowerShell and leaves WINWORD.EXE running.
        Kind::Word | Kind::Pdf => "try { $app.Quit([ref]0) } catch { $app.Quit() }",
        Kind::Excel => "$app.Quit()",
        Kind::PowerPoint => "if ($mine.Count -gt 0 -or $app.Presentations.Count -eq 0) { $app.Quit() }",
    };
    // When Word opens a PDF it first shows a notice ("Word will now convert your PDF to an
    // editable Word document...") with OK and Cancel. Under automation `Documents.Open` does
    // not return until it is answered, and in a hidden Word nobody can see it. Tried on
    // Word 16 (Microsoft 365) and found not to suppress it: `ConfirmConversions = False`,
    // `DisplayAlerts = 0`, and the per-user registry value `DisableConvertPdfWarning = 1`
    // (set before Word started, still shown). So the script answers the notice itself:
    // Word is shown while the PDF opens, because a dialog of a hidden window cannot be
    // reached, and a second PowerShell thread presses the notice's OK button. It only
    // touches a dialog of the Word this script started, and only one shaped like the
    // notice: one check box ("Don't show this message again", left as it is), an OK and a
    // Cancel button and nowhere to type. Anything else is left alone and runs into the
    // time limit.
    // Nothing is written to the user's Word settings.
    let pre = if kind == Kind::Pdf { PDF_NOTICE_WATCHER } else { "" };
    format!(
        r#"$ErrorActionPreference = 'Stop'
$code = 0
$app = $null
$docs = $null
$doc = $null
$mine = @()
$watch = $null
{pre}
try {{
    $before = @(Get-Process -Name {exe} -ErrorAction SilentlyContinue | ForEach-Object {{ $_.Id }})
    $app = New-Object -ComObject {class}
    $mine = @(Get-Process -Name {exe} -ErrorAction SilentlyContinue | ForEach-Object {{ $_.Id }} | Where-Object {{ $before -notcontains $_ }})
    if ($mine.Count -gt 1) {{ $mine = @() }}
    if ($env:SHORUI_PIDFILE) {{ [IO.File]::WriteAllText($env:SHORUI_PIDFILE, ($mine -join ' ')) }}
    {work}
}} catch {{
    $e = $_.Exception.GetBaseException()
    [Console]::Error.WriteLine(('{{0:X8}}|{{1}}' -f $e.HResult, ($e.Message -replace '\s+', ' ')))
    $code = 1
}} finally {{
    if ($watch -ne $null) {{ try {{ $watch.Stop() }} catch {{}}; try {{ $watch.Dispose() }} catch {{}} }}
    if ($doc -ne $null) {{ try {{ {close} }} catch {{}}; try {{ [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($doc) }} catch {{}} }}
    if ($docs -ne $null) {{ try {{ [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($docs) }} catch {{}} }}
    if ($app -ne $null) {{ try {{ {quit} }} catch {{}}; try {{ [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app) }} catch {{}} }}
    $doc = $null
    $docs = $null
    $app = $null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
    foreach ($id in $mine) {{
        for ($i = 0; $i -lt 25 -and (Get-Process -Id $id -ErrorAction SilentlyContinue); $i++) {{ Start-Sleep -Milliseconds 200 }}
        try {{ Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }} catch {{}}
    }}
}}
exit $code
"#
    )
}

fn powershell() -> PathBuf {
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let builtin = PathBuf::from(system).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    if builtin.is_file() { builtin } else { PathBuf::from("powershell") }
}

/// Stop the Office processes a timed-out or cancelled run left behind.
fn kill_pids(pid_file: &Path) {
    let Ok(text) = std::fs::read_to_string(pid_file) else { return };
    for pid in text.split_whitespace().filter(|p| p.chars().all(|c| c.is_ascii_digit())) {
        let _ = helpers::run(Command::new("taskkill").args(["/PID", pid, "/F"]), "taskkill");
    }
}

fn ms_error(app: &str, finished: &Finished, has_password: bool) -> Error {
    let detail = finished.detail();
    let (code, message) = match detail.split_once('|') {
        Some((code, message)) if code.len() == 8 => (code.to_string(), message.trim().to_string()),
        _ => (String::new(), detail.clone()),
    };
    match code.as_str() {
        // Word: the password is missing or wrong.
        "800A1520" | "800A156D" => {
            if has_password {
                Error::WrongPassword
            } else {
                Error::PasswordRequired
            }
        }
        // The automation class is not registered: the program is not really installed.
        "80040154" => Error::MissingHelper { tool: app.into(), hint: "Install Microsoft Office or LibreOffice, then try again.".into() },
        _ => Error::External(format!("{app} could not convert this file: {}", if message.ends_with('.') { message } else { format!("{message}.") })),
    }
}

/// PowerShell: the watcher that confirms Word's PDF conversion notice. It runs on a second
/// thread while `Documents.Open` blocks the first. See the comment in `ms_script`.
const PDF_NOTICE_WATCHER: &str = r#"$confirm = {
    param($wordId)
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    $element = [System.Windows.Automation.AutomationElement]
    $scope = [System.Windows.Automation.TreeScope]
    $type = [System.Windows.Automation.ControlType]
    $ofWord = New-Object System.Windows.Automation.PropertyCondition($element::ProcessIdProperty, [int]$wordId)
    $isDialog = New-Object System.Windows.Automation.PropertyCondition($element::ClassNameProperty, 'NUIDialog')
    $isButton = New-Object System.Windows.Automation.PropertyCondition($element::ControlTypeProperty, $type::Button)
    $isCheck = New-Object System.Windows.Automation.PropertyCondition($element::ControlTypeProperty, $type::CheckBox)
    $isEdit = New-Object System.Windows.Automation.PropertyCondition($element::ControlTypeProperty, $type::Edit)
    while ($true) {
        Start-Sleep -Milliseconds 250
        try {
            foreach ($window in $element::RootElement.FindAll($scope::Children, $ofWord)) {
                $dialogs = @()
                if ($window.Current.ClassName -eq 'NUIDialog') { $dialogs += $window }
                foreach ($d in $window.FindAll($scope::Descendants, $isDialog)) { $dialogs += $d }
                foreach ($dialog in $dialogs) {
                    $buttons = @($dialog.FindAll($scope::Descendants, $isButton))
                    $checks = @($dialog.FindAll($scope::Descendants, $isCheck))
                    $edits = @($dialog.FindAll($scope::Descendants, $isEdit))
                    # The buttons carry the standard dialog ids in every language: 1 is OK, 2 is Cancel.
                    $ok = @($buttons | Where-Object { $_.Current.AutomationId -eq '1' })
                    $cancel = @($buttons | Where-Object { $_.Current.AutomationId -eq '2' })
                    if ($buttons.Count -eq 2 -and $ok.Count -eq 1 -and $cancel.Count -eq 1 -and $checks.Count -eq 1 -and $edits.Count -eq 0) {
                        $ok[0].GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
                        return
                    }
                }
            }
        } catch {}
    }
}"#;

/// One Office automation at a time in this process, so two runs never start the same
/// program in the same instant and mistake each other's process for their own.
static OFFICE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_msoffice(input: &Path, kind: Kind, to: Target, temp: &Path, timeout: Duration, ctx: &Ctx) -> Result<PathBuf> {
    let _one_at_a_time = OFFICE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (app, _, _) = kind.ms_app();
    let produced = temp.join(format!("converted.{}", to.ext()));
    let pid_file = temp.join("office.pid");
    let mut command = Command::new(powershell());
    command
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg(ms_script(kind, to))
        .env("SHORUI_IN", input)
        .env("SHORUI_OUT", &produced)
        .env("SHORUI_PIDFILE", &pid_file)
        // A password is always passed: with none, Office would stop and ask in a window
        // nobody can see. A wrong one is simply refused.
        .env("SHORUI_PW", ctx.password().unwrap_or("shorui-no-password"));
    let finished = match run_limited(&mut command, app, timeout, ctx, temp) {
        Ok(f) => f,
        Err(e) => {
            kill_pids(&pid_file);
            return Err(match e {
                Error::External(m) if m.contains("did not finish") => {
                    Error::External(format!("{m} The file may be asking a question when it opens, for example for a password. Open it in {app} to check."))
                }
                other => other,
            });
        }
    };
    if !finished.success || !produced.is_file() {
        // Word answers "Command failed" when its PDF notice is cancelled.
        if kind == Kind::Pdf && finished.detail().starts_with("800A1066") {
            return Err(Error::External(
                "Microsoft Word did not open the PDF. If Word showed a message about converting PDFs and it was cancelled, run the conversion again and choose OK.".into(),
            ));
        }
        return Err(ms_error(app, &finished, ctx.password().is_some()));
    }
    Ok(produced)
}

// ---------------------------------------------------------------------------
// The tool
// ---------------------------------------------------------------------------

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = match inputs {
        [one] => one,
        [] => return Err(Error::invalid("Choose a file to convert.")),
        _ => return Err(Error::invalid("Office conversion works on one file at a time.")),
    };
    let kind = Kind::of(input).ok_or_else(|| {
        Error::invalid("This file type is not supported. Shorui converts Word (doc, docx, rtf, odt, txt), Excel (xls, xlsx, csv, ods) and PowerPoint (ppt, pptx, odp) files to PDF, and PDF to DOCX.")
    })?;
    let to = target(kind, opts.to.as_deref())?;
    if !input.is_file() {
        return Err(Error::read(input, std::io::Error::new(std::io::ErrorKind::NotFound, "the file does not exist")));
    }
    let bytes_in = crate::ctx::file_size(input);
    let chosen = choose(opts.engine, kind)?;
    if matches!(chosen, Chosen::Builtin) {
        let (bytes, pages) = builtin_docx(input, ctx)?;
        doc::write_file(out, &bytes)?;
        ctx.report(1.0, "Done");
        return Ok(Outcome::single(out.to_path_buf(), pages, bytes_in)
            .note("Text only: the wording and page breaks are carried over, the layout and pictures are not.")
            .note("For a closer copy, install LibreOffice, or choose Microsoft Office in the options (Word shows itself briefly while it converts)."));
    }
    let temp = helpers::TempDir::new("office")?;
    let timeout = Duration::from_secs(opts.timeout_secs.clamp(10, 7200) as u64);

    // The office program works on a copy. The original is never locked, and if the program
    // has to be stopped, its crash records point at the copy rather than at the user's file.
    ctx.check()?;
    ctx.report(0.05, "Preparing the file");
    let copy = temp.path().join(format!("input.{}", helpers::ext(input)));
    std::fs::copy(input, &copy).map_err(|e| Error::read(input, e))?;

    let (produced, engine_name) = match &chosen {
        Chosen::Libre(soffice) => {
            ctx.report(0.1, "Converting with LibreOffice");
            (with_libreoffice(soffice, &copy, kind, to, temp.path(), timeout, ctx)?, "LibreOffice")
        }
        Chosen::Builtin => unreachable!("handled above"),
        Chosen::Ms => {
            let (app, _, _) = kind.ms_app();
            ctx.report(0.1, &format!("Converting with {app}"));
            (with_msoffice(&copy, kind, to, temp.path(), timeout, ctx)?, app)
        }
    };

    ctx.report(0.9, "Saving the result");
    let bytes = doc::read_file(&produced)?;
    let looks_right = match to {
        Target::Pdf => bytes.starts_with(b"%PDF"),
        Target::Docx => bytes.starts_with(b"PK"),
    };
    if !looks_right {
        return Err(Error::External(format!("{engine_name} did not produce a usable {} file.", to.ext().to_uppercase())));
    }
    // Pages: of the PDF written, or of the PDF read when the result is a Word document.
    let pages = match to {
        Target::Pdf => doc::load_bytes(&bytes, None).map(|d| doc::page_count(&d)).unwrap_or(0),
        Target::Docx => doc::load(input, ctx.password()).map(|d| doc::page_count(&d)).unwrap_or(0),
    };
    doc::write_file(out, &bytes)?;
    ctx.report(1.0, "Done");
    let mut outcome = Outcome::single(out.to_path_buf(), pages, bytes_in).note(format!("Converted with {engine_name}."));
    if to == Target::Docx {
        outcome.notes.push("The layout is rebuilt from the PDF, so check the result: complex pages can come out differently, and scanned pages arrive as pictures rather than text.".into());
    }
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Built-in PDF to DOCX: text only, no helper program, nothing shown on screen
// ---------------------------------------------------------------------------

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// A zip archive with the files stored as they are. A .docx is such an archive.
fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, body) in files {
        let mut crc = flate2::Crc::new();
        crc.update(body);
        let offset = out.len() as u32;
        let header = |signature: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&signature.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
            h.extend_from_slice(&0x0800u16.to_le_bytes()); // names are UTF-8
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&[0u8; 4]); // time and date
            h.extend_from_slice(&crc.sum().to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(body.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra length
            h
        };
        out.extend_from_slice(&header(0x0403_4b50, false));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body);
        central.extend_from_slice(&header(0x0201_4b50, true));
        central.extend_from_slice(&[0u8; 2 + 2 + 2 + 4]); // comment, disk, internal and external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// Turn the text of a PDF into a Word document: one paragraph per line, larger lines in
/// bold at their size, pieces of one row joined by tabs, a page break between pages.
/// Returns the file's bytes and the number of pages read.
pub fn builtin_docx(input: &Path, ctx: &Ctx) -> Result<(Vec<u8>, usize)> {
    let reader = crate::text::TextReader::open_path(input, ctx.password())?;
    let pages = reader.page_count();
    let mut body = String::new();
    let mut any_text = false;
    let mut page_size = (595.0f32, 842.0f32);
    for index in 0..pages {
        ctx.check()?;
        ctx.report(0.1 + 0.8 * index as f32 / pages.max(1) as f32, &format!("Reading text, page {} of {pages}", index + 1));
        let page = reader.page(index)?;
        if index == 0 {
            page_size = (page.width, page.height);
        } else {
            body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
        }
        let lines: Vec<&crate::text::Line> = page.lines.iter().filter(|l| l.angle == 0.0 && !l.words.is_empty()).collect();
        let mut sizes: Vec<f32> = lines.iter().map(|l| l.size).collect();
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let usual = sizes.get(sizes.len() / 2).copied().unwrap_or(11.0);
        let mut row_y: Option<f32> = None;
        let mut open = false;
        for line in lines {
            any_text = true;
            let y = (line.rect.y0 + line.rect.y1) / 2.0;
            let same_row = row_y.is_some_and(|prev| (y - prev).abs() < 0.4 * line.size);
            let heading = line.size > usual * 1.25;
            let props = format!("<w:rPr>{}<w:sz w:val=\"{}\"/></w:rPr>", if heading { "<w:b/>" } else { "" }, (line.size * 2.0).round().clamp(12.0, 144.0) as u32);
            let text = xml_escape(&line.text());
            if same_row && open {
                body.push_str(&format!("<w:r>{props}<w:tab/><w:t xml:space=\"preserve\">{text}</w:t></w:r>"));
            } else {
                if open {
                    body.push_str("</w:p>");
                }
                body.push_str(&format!("<w:p><w:r>{props}<w:t xml:space=\"preserve\">{text}</w:t></w:r>"));
                open = true;
            }
            row_y = Some(y);
        }
        if open {
            body.push_str("</w:p>");
        }
    }
    if !any_text {
        return Err(Error::invalid("This PDF has no text to carry over. If it is a scan, run OCR on it first, then convert it."));
    }
    ctx.report(0.92, "Writing the Word document");
    let twips = |points: f32| (points * 20.0).round().clamp(2880.0, 31680.0) as u32;
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"{}\" w:h=\"{}\"/><w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        twips(page_size.0),
        twips(page_size.1)
    );
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>";
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    Ok((stored_zip(&[("[Content_Types].xml", types.as_bytes()), ("_rels/.rels", rels.as_bytes()), ("word/document.xml", document.as_bytes())]), pages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_docx_carries_the_text_and_page_breaks() {
        let dir = helpers::TempDir::new("docx").unwrap();
        let path = dir.path().join("report.pdf");
        let mut d = crate::fixtures::text_document("Quarterly Report", "REPORT", 6);
        doc::save(&mut d, &path).unwrap();
        let (bytes, pages) = builtin_docx(&path, &Ctx::none()).unwrap();
        assert_eq!(pages, 6);
        assert!(bytes.starts_with(b"PK"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("word/document.xml"));
        assert!(text.contains("MARK-REPORT-P1") && text.contains("MARK-REPORT-P6"));
        assert_eq!(text.matches("w:type=\"page\"").count(), 5);
        // The title is larger than the body text, so it is bold.
        assert!(text.contains("<w:b/><w:sz w:val=\"44\"/></w:rPr><w:t xml:space=\"preserve\">Quarterly Report"));
    }

    #[test]
    fn builtin_docx_needs_text() {
        let dir = helpers::TempDir::new("docx-scan").unwrap();
        let path = dir.path().join("scan.pdf");
        let mut d = crate::fixtures::scan_document().unwrap();
        doc::save(&mut d, &path).unwrap();
        assert!(matches!(builtin_docx(&path, &Ctx::none()), Err(Error::Invalid(m)) if m.contains("OCR")));
    }

    #[test]
    fn a_pdf_never_goes_through_word_unless_asked() {
        if helpers::find_libreoffice().is_none() {
            assert!(matches!(choose(Engine::Auto, Kind::Pdf), Ok(Chosen::Builtin)));
        }
    }


    #[test]
    fn picks_direction() {
        assert_eq!(target(Kind::Word, None).unwrap(), Target::Pdf);
        assert_eq!(target(Kind::Pdf, None).unwrap(), Target::Docx);
        assert_eq!(target(Kind::Excel, Some(".PDF")).unwrap(), Target::Pdf);
        assert!(target(Kind::Pdf, Some("pdf")).is_err());
        assert!(target(Kind::Excel, Some("docx")).is_err());
        assert!(target(Kind::Word, Some("odt")).is_err());
        assert_eq!(Kind::of(Path::new("a.DOCX")), Some(Kind::Word));
        assert_eq!(Kind::of(Path::new("a.csv")), Some(Kind::Excel));
        assert_eq!(Kind::of(Path::new("a.png")), None);
    }

    #[test]
    fn script_has_no_double_quotes() {
        for (kind, to) in [(Kind::Word, Target::Pdf), (Kind::Pdf, Target::Docx), (Kind::Excel, Target::Pdf), (Kind::PowerPoint, Target::Pdf)] {
            let script = ms_script(kind, to);
            assert!(!script.contains('"'), "{kind:?}");
            assert!(script.contains("finally") && script.contains("Quit"));
        }
    }
}
