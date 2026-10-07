//! The window: one shell shared by every mode. Sidebar, top bar, workspace, options panel
//! and the run line. The pieces are drawn in the `view_*` modules.

use crate::catalog::{self, Tool};
use crate::jobs;
use crate::palette::{PaletteEvent, PaletteView, Presentation};
use crate::palette_model::{Command, Situation};
use crate::settings::Settings;
use crate::state::{RowStatus, State};
use crate::tokens::*;
use crate::ui::{self, Theme, pal};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use shorui_core::tools::Group;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

actions!(shorui, [OpenPalette, AddFiles, RunCurrent, Dismiss, ToggleTheme, ToggleSidebar, GoHome, ClearQueue, Quit, Favorite1, Favorite2, Favorite3, CompressOpen, PageUndo, PageDelete, ZoomIn, ZoomOut, ZoomFit]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-k", OpenPalette, None),
        KeyBinding::new("secondary-o", AddFiles, None),
        KeyBinding::new("secondary-enter", RunCurrent, None),
        KeyBinding::new("escape", Dismiss, Some("Shell")),
        KeyBinding::new("secondary-shift-l", ToggleTheme, None),
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-shift-h", GoHome, None),
        KeyBinding::new("secondary-shift-c", CompressOpen, None),
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-1", Favorite1, None),
        KeyBinding::new("secondary-2", Favorite2, None),
        KeyBinding::new("secondary-3", Favorite3, None),
        // Edit & Fill. A text field that has the keyboard keeps these keys for itself.
        KeyBinding::new("secondary-z", PageUndo, Some("Shell")),
        KeyBinding::new("delete", PageDelete, Some("Shell")),
        KeyBinding::new("backspace", PageDelete, Some("Shell")),
        // Zoom on the page canvas. `=` is where `+` lives without Shift on most layouts.
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary-shift-=", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-0", ZoomFit, None),
    ]);
}

/// A short message shown bottom-right for a few seconds.
#[derive(Clone)]
pub struct Toast {
    pub ok: bool,
    pub title: String,
    pub body: String,
    pub id: u64,
}

pub struct Shell {
    pub state: State,
    pub settings: Settings,
    pub focus: FocusHandle,
    pub open_groups: HashSet<Group>,
    pub palette: Entity<PaletteView>,
    pub palette_open: bool,
    /// The same search as the palette, inline on Home.
    pub home_search: Entity<PaletteView>,
    pub job: Option<jobs::Handle>,
    pub toast: Option<Toast>,
    toast_counter: u64,
    /// Set while a `G` chord is waiting for its second key.
    pub chord_pending: bool,
    /// When the process started, for the boot-time log.
    pub booted: Option<std::time::Instant>,
    pub inputs: std::collections::HashMap<String, crate::view_panel::InputSlot>,
    pub open_select: Option<String>,
    pub slider_bounds: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, Bounds<Pixels>>>>,
    pub thumbs: crate::thumbs::Thumbs,
    pub grid_focus: FocusHandle,
    pub grid_scroll: ScrollHandle,
    pub canvas: crate::view_canvas::CanvasState,
    pub canvas_size_for: Option<(PathBuf, usize)>,
    /// Scroll position of the page canvas when the page is zoomed past the stage.
    pub canvas_scroll: ScrollHandle,
    /// The "Local only" details are showing.
    pub local_open: bool,
    /// How many files were loaded when page images were last tidied.
    thumbs_for: usize,
    /// A save of the preferences is already on its way.
    prefs_pending: bool,
    /// Where the most recent run saved, for "Show in folder".
    pub last_outputs: Vec<PathBuf>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(settings: Settings, booted: std::time::Instant, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let palette = cx.new(|cx| PaletteView::new(Presentation::Overlay, window, cx));
        let subscription = cx.subscribe_in(&palette, window, |this: &mut Self, _, event: &PaletteEvent, window, cx| match event {
            PaletteEvent::Dismiss => this.close_palette(window, cx),
            PaletteEvent::Run(command) => {
                this.close_palette(window, cx);
                this.run_command(command.clone(), window, cx);
            }
        });
        let home_search = cx.new(|cx| PaletteView::new(Presentation::Inline, window, cx));
        let home_subscription = cx.subscribe_in(&home_search, window, |this: &mut Self, search, event: &PaletteEvent, window, cx| match event {
            PaletteEvent::Dismiss => window.focus(&this.focus, cx),
            PaletteEvent::Run(command) => {
                search.update(cx, |search, cx| search.clear(window, cx));
                window.focus(&this.focus, cx);
                this.run_command(command.clone(), window, cx);
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Keep the keyboard inside the app so shortcuts always have somewhere to land.
        let restore = cx.on_focus_lost(window, |this: &mut Self, window, cx| window.focus(&this.focus, cx));
        let weak = cx.entity().downgrade();
        let chords = cx.intercept_keystrokes(move |event, window, cx| {
            let used = weak.update(cx, |shell, cx| shell.chord(&event.keystroke, window, cx)).unwrap_or(false);
            if used {
                cx.stop_propagation();
            }
        });
        // Remember the window and everything else for next time.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = this.update(cx, |shell, _| {
                shell.settings.maximized = window.is_maximized();
                if !window.is_maximized() && !window.is_fullscreen() {
                    let size = window.viewport_size();
                    shell.settings.window = Some((f32::from(size.width), f32::from(size.height)));
                }
                shell.save_prefs();
            });
            true
        });
        // Put back the options and drawing settings saved last time.
        let mut state = State::default();
        for (tool, saved) in &settings.options {
            let Some(tool) = catalog::tool(tool) else { continue };
            let options = state.options_mut(tool.id);
            for (key, value) in saved {
                // Only keys the tool still has, and never a password.
                if options.contains_key(key) && crate::settings::keep_option(key) {
                    options.insert(key.clone(), value.clone());
                }
            }
        }
        let mut canvas = crate::view_canvas::CanvasState::default();
        if let Some(ink) = settings.edit_ink {
            canvas.edit.ink = ink;
        }
        if let Some(size) = settings.edit_text_size.filter(|s| (6. ..=72.).contains(s)) {
            canvas.edit.text_size = size;
        }
        if let Some(size) = settings.edit_mark_size.filter(|s| (4. ..=72.).contains(s)) {
            canvas.edit.mark_size = size;
        }
        Shell {
            state,
            settings,
            focus,
            open_groups: HashSet::new(),
            palette,
            palette_open: false,
            home_search,
            job: None,
            toast: None,
            toast_counter: 0,
            chord_pending: false,
            booted: Some(booted),
            inputs: Default::default(),
            open_select: None,
            slider_bounds: Default::default(),
            thumbs: Default::default(),
            grid_focus: cx.focus_handle(),
            grid_scroll: ScrollHandle::new(),
            canvas,
            canvas_size_for: None,
            canvas_scroll: ScrollHandle::new(),
            local_open: false,
            thumbs_for: 0,
            prefs_pending: false,
            last_outputs: Vec::new(),
            _subscriptions: vec![subscription, home_subscription, restore, chords],
        }
    }

    // ----- palette

    pub fn situation(&self) -> Situation {
        Situation {
            recent: self.settings.recent.clone(),
            active_tool: self.state.mode,
            files: self.state.visible().into_iter().map(|i| self.state.files[i].path.clone()).collect(),
            dark: self.settings.dark,
            favorites: self.settings.favorites.clone(),
        }
    }

    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The same shortcut closes it again.
        if self.palette_open {
            self.close_palette(window, cx);
            return;
        }
        if self.state.tool().is_none() {
            self.home_search.update(cx, |search, cx| search.focus_all(window, cx));
            return;
        }
        let situation = self.situation();
        self.palette_open = true;
        self.palette.update(cx, |palette, cx| palette.open(situation, window, cx));
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    // ----- commands

    pub fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::OpenTool(id) => self.open_tool(id, cx),
            Command::OpenToolWith { tool, key, value } => {
                let json = match value {
                    "true" => serde_json::Value::Bool(true),
                    "false" => serde_json::Value::Bool(false),
                    other => serde_json::Value::String(other.to_string()),
                };
                self.state.set_option(tool, key, json);
                self.open_tool(tool, cx);
            }
            Command::AddFiles => self.pick_files(window, cx),
            Command::GoHome => self.go_home(cx),
            Command::ToggleTheme => self.toggle_theme(cx),
            Command::ToggleSidebar => self.toggle_sidebar(cx),
            Command::RunCurrent => self.run_current(window, cx),
            Command::ClearQueue => {
                if !self.state.running {
                    self.state.clear_files();
                    cx.notify();
                }
            }
            Command::ShowResult => match self.last_outputs.first() {
                Some(path) => cx.reveal_path(path),
                None => self.show_toast(false, "Nothing saved yet", "Run a tool first. You choose where its result goes.", cx),
            },
            Command::ToggleFavorite => {
                if let Some(id) = self.state.mode {
                    self.toggle_favorite(id, cx);
                }
            }
            Command::Quit => cx.quit(),
        }
    }

    pub fn open_tool(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if self.state.running {
            self.show_toast(false, "A run is in progress", "Wait for it to finish, or cancel it, before switching tools.", cx);
            return;
        }
        let Some(tool) = catalog::tool(id) else { return };
        self.state.mode = Some(tool.id);
        // Damaged files take part in Repair and sit out of every other tool.
        for file in &mut self.state.files {
            if file.problem.is_some() {
                file.included = tool.id == "repair";
            }
        }
        self.state.reset_statuses();
        self.state.fix_active();
        self.canvas.loaded_for = None;
        self.open_select = None;
        self.reset_inputs();
        self.open_groups.clear();
        self.open_groups.insert(tool.group);
        self.settings.touch_recent(tool.id);
        self.save_prefs();
        cx.notify();
    }

    pub fn go_home(&mut self, cx: &mut Context<Self>) {
        if !self.state.running {
            self.state.mode = None;
            cx.notify();
        }
    }

    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.settings.dark = !self.settings.dark;
        self.save_prefs();
        crate::apply_theme(self.settings.dark, cx);
        cx.refresh_windows();
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.settings.sidebar_collapsed = !self.settings.sidebar_collapsed;
        self.save_prefs();
        cx.notify();
    }

    // ----- files

    pub fn pick_files(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.add_files(paths, cx));
            }
        })
        .detach();
    }

    pub fn toggle_favorite(&mut self, id: &str, cx: &mut Context<Self>) {
        self.settings.toggle_favorite(id);
        self.save_prefs();
        cx.notify();
    }

    /// Write the preferences now: theme, sidebar, favourites, recent tools, window, the
    /// tools' options and the drawing settings.
    pub fn save_prefs(&mut self) {
        self.settings.remember_options(&self.state.options);
        let edit = &self.canvas.edit;
        self.settings.edit_ink = Some(edit.ink);
        self.settings.edit_text_size = Some(edit.text_size);
        self.settings.edit_mark_size = Some(edit.mark_size);
        self.settings.save();
    }

    /// Something worth remembering changed. Saves shortly after, once typing pauses.
    pub fn prefs_changed(&mut self, cx: &mut Context<Self>) {
        if self.prefs_pending {
            return;
        }
        self.prefs_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(700)).await;
            let _ = this.update(cx, |this, _| {
                this.prefs_pending = false;
                this.save_prefs();
            });
        })
        .detach();
    }

    /// Where a save dialog opens: the folder last saved into, else the first file's folder.
    fn save_start_dir(&self, files: &[PathBuf]) -> PathBuf {
        self.settings
            .save_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| files.first().and_then(|f| f.parent()).map(|d| d.to_path_buf()))
            .or_else(dirs::document_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn add_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let before = self.state.files.len();
        let (added, unusable) = self.state.add_files(paths);
        if added == 0 {
            return;
        }
        if unusable > 0 {
            if let Some(tool) = self.state.tool() {
                let what = if unusable == 1 { "1 file is".to_string() } else { format!("{unusable} files are") };
                self.show_toast(false, &format!("{what} not used by {}", tool.name), "They stay loaded for tools that can take them.", cx);
            }
        }
        // Page counts are read off the UI thread.
        for index in before..self.state.files.len() {
            let entry = &self.state.files[index];
            if !entry.is_pdf() {
                continue;
            }
            let path = entry.path.clone();
            let task = cx.background_spawn({
                let path = path.clone();
                async move { shorui_core::doc::quick_info(&path) }
            });
            cx.spawn(async move |this, cx| {
                let info = task.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(file) = this.state.files.iter_mut().find(|f| f.path == path) {
                        match info {
                            Ok(info) => {
                                file.pages = Some(info.pages);
                                file.locked = info.locked;
                                file.bytes = info.bytes;
                            }
                            Err(e) => {
                                // A file that cannot be read sits out of batch runs until it is
                                // repaired; in Repair it is exactly what the run is for.
                                file.problem = Some(e.to_string());
                                file.included = this.state.mode == Some("repair");
                            }
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    // ----- running

    pub fn run_current(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.state.running {
            return;
        }
        let Some(tool) = self.state.tool() else {
            self.show_toast(false, "Choose a tool first", &format!("Press {} K to search the tools.", crate::palette_model::mod_label()), cx);
            return;
        };
        let rows = self.state.run_set();
        let files: Vec<PathBuf> = rows.iter().map(|i| self.state.files[*i].path.clone()).collect();
        let mut options = self.state.options_of(tool.id);
        if tool.id == "pages" {
            for (k, v) in self.state.pages_edit.to_options() {
                options.insert(k, v);
            }
        }
        if let Err(why) = self.canvas_options(tool, &mut options) {
            self.show_toast(false, "Not ready yet", &why, cx);
            return;
        }
        if tool.id == "merge" {
            let ranges: Vec<serde_json::Value> = rows.iter().map(|i| serde_json::Value::String(self.state.files[*i].range.clone())).collect();
            options.insert("ranges".into(), serde_json::Value::Array(ranges));
        }
        // Plan in the folder the dialog will open in, so the suggested names are right.
        let start = self.save_start_dir(&files);
        let items = match jobs::plan(tool, &files, &start, &options) {
            Ok(items) => items,
            Err(why) => {
                self.show_toast(false, "Nothing to run yet", &why, cx);
                return;
            }
        };
        let one_file = items.len() == 1 && shorui_core::tools::info(tool.id).is_none_or(|i| i.output == shorui_core::tools::OutputKind::File);
        if one_file {
            // One result: ask for its name and place.
            let suggested = items[0].out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = items[0].out.extension().map(|e| e.to_string_lossy().into_owned());
            let answer = cx.prompt_for_new_path(&start, Some(&suggested));
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(mut path))) = answer.await {
                    if path.extension().is_none() {
                        if let Some(ext) = &ext {
                            path.set_extension(ext);
                        }
                    }
                    let _ = this.update(cx, |this, cx| {
                        let mut items = items;
                        items[0].out = path;
                        this.start_run(tool, rows, files, items, options, cx);
                    });
                }
            })
            .detach();
        } else {
            // Several results: ask for a folder and name them there.
            let answer = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Save here".into()) });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(dirs))) = answer.await {
                    if let Some(dir) = dirs.into_iter().next() {
                        let _ = this.update(cx, |this, cx| match jobs::plan(tool, &files, &dir, &options) {
                            Ok(items) => this.start_run(tool, rows, files, items, options, cx),
                            Err(why) => this.show_toast(false, "Nothing to run yet", &why, cx),
                        });
                    }
                }
            })
            .detach();
        }
    }

    /// Start a run once the user has said where its results go.
    fn start_run(&mut self, tool: &'static Tool, rows: Vec<usize>, files: Vec<PathBuf>, mut items: Vec<jobs::Item>, options: serde_json::Map<String, serde_json::Value>, cx: &mut Context<Self>) {
        // The tool or the files may have changed while the dialog was open.
        if self.state.running || self.state.mode != Some(tool.id) || rows.iter().any(|r| self.state.files.get(*r).is_none_or(|f| !files.contains(&f.path))) {
            return;
        }
        // The original is never written over: tools read it while they write.
        if let Some(item) = items.iter().find(|i| i.inputs.iter().any(|input| same_file(input, &i.out))) {
            let name = item.out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.show_toast(false, "Choose another name", &format!("{name} is the file being read, so it cannot be saved over. Save under a new name."), cx);
            return;
        }
        if let Some(dir) = items.first().and_then(|i| i.out.parent()) {
            self.settings.save_dir = Some(dir.to_path_buf());
            self.save_prefs();
        }
        // `plan` numbers rows within the files it was given; map them back to queue rows.
        for item in &mut items {
            item.row = rows.get(item.row).copied().unwrap_or(0);
        }
        self.state.reset_statuses();
        for item in &items {
            if let Some(f) = self.state.files.get_mut(item.row) {
                f.status = RowStatus::Queued;
            }
        }
        self.state.running = true;
        self.job = Some(jobs::start(tool.id, items, options));
        self.pump_job(tool, cx);
        cx.notify();
    }

    /// Escape: stop a run, or close whatever small thing is open.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.state.running {
            self.cancel_run(cx);
        } else if self.edit_dismiss(cx) {
        } else if self.open_select.take().is_some() || std::mem::take(&mut self.local_open) {
            cx.notify();
        } else if self.toast.take().is_some() {
            cx.notify();
        }
    }

    /// Release page images that belong to files no longer in the queue.
    fn tidy_thumbs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thumbs_for == self.state.files.len() {
            return;
        }
        self.thumbs_for = self.state.files.len();
        let paths: std::collections::HashSet<PathBuf> = self.state.files.iter().map(|f| f.path.clone()).collect();
        for image in self.thumbs.evict(&|path| paths.contains(path)) {
            cx.drop_image(image, Some(window));
        }
    }

    pub fn cancel_run(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &self.job {
            job.cancel();
        }
        cx.notify();
    }

    /// Drain job events on a short timer until the run finishes.
    fn pump_job(&mut self, tool: &'static Tool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(40)).await;
                let finished = this.update(cx, |this, cx| this.drain_job(tool, cx)).unwrap_or(true);
                if finished {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn drain_job(&mut self, tool: &'static Tool, cx: &mut Context<Self>) -> bool {
        let mut finished = false;
        let mut changed = false;
        let events: Vec<jobs::Event> = match &self.job {
            Some(job) => std::iter::from_fn(|| job.events.try_recv().ok()).collect(),
            None => return true,
        };
        for event in events {
            changed = true;
            match event {
                jobs::Event::Started { row } => {
                    if let Some(f) = self.state.files.get_mut(row) {
                        f.status = RowStatus::Running { fraction: 0.0, what: "Starting".into() };
                    }
                }
                jobs::Event::Progress { row, fraction, what } => {
                    if let Some(f) = self.state.files.get_mut(row) {
                        f.status = RowStatus::Running { fraction, what };
                    }
                }
                jobs::Event::Done { row, outcome } => self.state.apply_outcome(row, outcome),
                jobs::Event::Failed { row, message } => {
                    if let Some(f) = self.state.files.get_mut(row) {
                        f.status = RowStatus::Failed { message };
                    }
                }
                jobs::Event::Finished { cancelled } => {
                    finished = true;
                    self.finish_run(tool, cancelled, cx);
                }
            }
        }
        if changed {
            cx.notify();
        }
        finished
    }

    fn finish_run(&mut self, tool: &'static Tool, cancelled: bool, cx: &mut Context<Self>) {
        self.state.running = false;
        self.job = None;
        for f in &mut self.state.files {
            if matches!(f.status, RowStatus::Queued | RowStatus::Running { .. }) {
                f.status = RowStatus::Idle;
            }
        }
        let done: Vec<&crate::state::FileEntry> = self.state.files.iter().filter(|f| matches!(f.status, RowStatus::Done { .. })).collect();
        let failed = self.state.files.iter().filter(|f| matches!(f.status, RowStatus::Failed { .. })).count();
        if cancelled {
            self.show_toast(false, "Run cancelled", "Files already finished were kept.", cx);
            return;
        }
        if done.is_empty() {
            let reason = self.state.files.iter().find_map(|f| match &f.status {
                RowStatus::Failed { message } => Some(message.clone()),
                _ => None,
            });
            self.show_toast(false, &format!("{} failed", tool.name), &reason.unwrap_or_else(|| "See the queue for the reason.".into()), cx);
            return;
        }
        let bytes_in: u64 = done.iter().map(|f| f.bytes).sum();
        let bytes_out: u64 = done.iter().map(|f| if let RowStatus::Done { bytes_out, .. } = &f.status { *bytes_out } else { 0 }).sum();
        let n = done.len();
        let title = match tool.inputs {
            catalog::Inputs::Each | catalog::Inputs::Optional => format!("{n} {} done", if n == 1 { "file" } else { "files" }),
            _ => format!("{} finished", tool.name),
        };
        self.last_outputs = done.iter().filter_map(|f| if let RowStatus::Done { outputs, .. } = &f.status { outputs.first().cloned() } else { None }).collect();
        let folder = self.last_outputs.first().and_then(|p| p.parent()).map(crate::settings::display_path).unwrap_or_default();
        let mut body = if tool.id == "compress" && bytes_out < bytes_in {
            format!("{} smaller. Saved in {folder}.", crate::settings::human_size(bytes_in - bytes_out))
        } else {
            format!("Saved in {folder}.")
        };
        if failed > 0 {
            body.push_str(&format!(" {failed} {} failed, see the queue.", if failed == 1 { "file" } else { "files" }));
        }
        self.show_toast(true, &title, &body, cx);
    }

    pub fn show_toast(&mut self, ok: bool, title: &str, body: &str, cx: &mut Context<Self>) {
        self.toast_counter += 1;
        let id = self.toast_counter;
        self.toast = Some(Toast { ok, title: title.to_string(), body: body.to_string(), id });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(6)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast.as_ref().is_some_and(|t| t.id == id) {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open_favorite(&mut self, slot: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.settings.favorites.get(slot).and_then(|id| catalog::tool(id)).map(|t| t.id) {
            self.open_tool(id, cx);
        }
    }

    /// `G` then a letter switches tool. On Home, any other printable key starts a search in
    /// the inline box. Returns true when the key was used up.
    ///
    /// This runs before key bindings, so the chord also works while the page grid or a
    /// button has the keyboard. It stands aside while a text field is focused.
    fn chord(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use gpui_kit::component::WindowExt as _;
        let m = &keystroke.modifiers;
        let typing = window.focused_input(cx).is_some();
        if self.palette_open || typing || m.control || m.platform || m.alt || m.function || !self.focus.contains_focused(window, cx) {
            self.chord_pending = false;
            return false;
        }
        if self.chord_pending {
            self.chord_pending = false;
            if let Some(tool) = catalog::TOOLS.iter().find(|t| t.chord == keystroke.key) {
                self.open_tool(tool.id, cx);
                return true;
            }
            return self.type_on_home("g", keystroke, window, cx);
        }
        if keystroke.key == "g" && !m.shift {
            self.chord_pending = true;
            return true;
        }
        self.type_on_home("", keystroke, window, cx)
    }

    fn type_on_home(&mut self, prefix: &str, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // Space and Enter stay with whatever row or button has the keyboard.
        let printable = keystroke.key_char.as_deref().filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control() && !c.is_whitespace()));
        let (None, Some(text)) = (self.state.tool(), printable) else { return false };
        let text = format!("{prefix}{text}");
        self.home_search.update(cx, |search, cx| search.type_text(&text, window, cx));
        true
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(start) = self.booted.take() {
            if std::env::var_os("SHORUI_BOOT_LOG").is_some() {
                eprintln!("first frame after {} ms", start.elapsed().as_millis());
            }
        }
        let p = pal(cx);
        self.tidy_thumbs(window, cx);
        let sidebar = self.render_sidebar(window, cx).into_any_element();
        let topbar = self.render_topbar(window, cx).into_any_element();
        let workspace = self.render_workspace(window, cx).into_any_element();
        let panel = self.render_panel(window, cx);
        let runline = self.render_runline(window, cx).into_any_element();

        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)))
            .on_action(cx.listener(|this, _: &AddFiles, window, cx| this.pick_files(window, cx)))
            .on_action(cx.listener(|this, _: &RunCurrent, window, cx| this.run_current(window, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.dismiss(cx)))
            .on_action(cx.listener(|this, _: &PageUndo, _, cx| this.edit_undo(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.canvas_zoom_step(1, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.canvas_zoom_step(-1, cx)))
            .on_action(cx.listener(|this, _: &ZoomFit, _, cx| this.set_canvas_zoom(1., cx)))
            .on_action(cx.listener(|this, _: &PageDelete, _, cx| {
                this.edit_delete_selected(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &GoHome, _, cx| this.go_home(cx)))
            .on_action(cx.listener(|this, _: &ClearQueue, window, cx| this.run_command(Command::ClearQueue, window, cx)))
            .on_action(cx.listener(|this, _: &CompressOpen, window, cx| this.run_command(Command::OpenToolWith { tool: "compress", key: "preset", value: "balanced" }, window, cx)))
            .on_action(cx.listener(|this, _: &Favorite1, _, cx| this.open_favorite(0, cx)))
            .on_action(cx.listener(|this, _: &Favorite2, _, cx| this.open_favorite(1, cx)))
            .on_action(cx.listener(|this, _: &Favorite3, _, cx| this.open_favorite(2, cx)))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.add_files(paths.paths().to_vec(), cx)))
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(p.frame)
            .text_color(p.toner)
            .font_family(FONT_UI)
            .text_size(px(TEXT))
            .line_height(px(18.))
            .child(sidebar)
            .child(
                // The content card: inset from the window edges, rounded, one hairline edge.
                div().flex_1().min_w_0().h_full().py(px(INSET)).pr(px(INSET)).child(
                    ui::col()
                        .size_full()
                        .rounded(px(R_LG))
                        .border_1()
                        .border_color(p.rule)
                        .bg(p.card)
                        .overflow_hidden()
                        .child(topbar)
                        .child(div().flex().flex_row().flex_1().min_h_0().child(div().flex().flex_col().flex_1().min_w_0().h_full().overflow_hidden().child(workspace)).when_some(panel, |s, panel| s.child(panel)))
                        .child(runline),
                ),
            )
            .when_some(self.toast.clone(), |s, toast| s.child(self.render_toast(&toast, &p, cx)))
            .when(self.palette_open, |s| s.child(self.palette.clone()))
    }
}

impl Shell {
    fn render_toast(&self, toast: &Toast, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let right = if self.state.mode.is_some() { PANEL_W + 16. } else { 16. } + INSET;
        div().absolute().right(px(right)).bottom(px(RUNLINE_H + 16. + INSET)).child(
            ui::row()
                .items_start()
                .w(px(340.))
                .py(px(10.))
                .pl(px(12.))
                .pr(px(8.))
                .gap(px(10.))
                .rounded(px(R_MD))
                .border_1()
                .border_color(p.rule_strong)
                .bg(p.sleeve)
                .child(div().pt(px(2.)).child(ui::icon(if toast.ok { "check" } else { "alert" }, 14., if toast.ok { p.proof } else { p.stamp })))
                .child(ui::col().flex_1().min_w_0().gap(px(2.)).child(div().font_weight(FontWeight::MEDIUM).child(SharedString::from(toast.title.clone()))).child(div().text_size(px(TEXT_SM)).text_color(p.toner_2).child(SharedString::from(toast.body.clone()))))
                .child(ui::icon_button("toast-close", "x", 24., p).on_click(cx.listener(|this, _, _, cx| {
                    this.toast = None;
                    cx.notify();
                }))),
        )
    }
}

/// Install the palette as a global and tell gpui-kit's own components to match it.
pub fn set_theme(dark: bool, cx: &mut App) {
    cx.set_global(Theme(Palette::of(dark)));
}

/// The same file, even when written differently (case on Windows, `..`, links).
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}
