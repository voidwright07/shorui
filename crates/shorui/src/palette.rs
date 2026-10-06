//! The command palette: a centred overlay that is the fastest way to do anything.
//! Opened with Cmd/Ctrl+K, driven entirely from the keyboard.

use crate::palette_model::{Command, Entry, Section, Situation, entries, mod_label};
use crate::tokens::*;
use crate::ui::{self, KeyOn, pal};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

actions!(palette, [PaletteUp, PaletteDown, PaletteConfirm, PaletteClose]);

pub const CONTEXT: &str = "Palette";

/// Key bindings. Call after `gpui_kit::init` so these win over the text input's own
/// bindings for the same keys while the palette is open.
pub fn bind_keys(cx: &mut App) {
    for context in ["Palette", "Palette > Input"] {
        cx.bind_keys([
            KeyBinding::new("up", PaletteUp, Some(context)),
            KeyBinding::new("down", PaletteDown, Some(context)),
            KeyBinding::new("ctrl-p", PaletteUp, Some(context)),
            KeyBinding::new("ctrl-n", PaletteDown, Some(context)),
            KeyBinding::new("enter", PaletteConfirm, Some(context)),
            KeyBinding::new("escape", PaletteClose, Some(context)),
        ]);
    }
}

pub enum PaletteEvent {
    Run(Command),
    Dismiss,
}

pub struct PaletteView {
    input: Entity<InputState>,
    entries: Vec<Entry>,
    selected: usize,
    situation: Situation,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<PaletteEvent> for PaletteView {}

impl PaletteView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search tools and actions"));
        let subscription = cx.subscribe_in(&input, window, |this: &mut Self, _, event: &InputEvent, _window, cx| match event {
            InputEvent::Change => this.refresh(cx),
            InputEvent::PressEnter { .. } => this.confirm(cx),
            _ => {}
        });
        PaletteView { input, entries: Vec::new(), selected: 0, situation: Situation::default(), scroll: ScrollHandle::new(), _subscription: subscription }
    }

    /// Show the palette for the current state of the app, with an empty query.
    pub fn open(&mut self, situation: Situation, window: &mut Window, cx: &mut Context<Self>) {
        self.situation = situation;
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.selected = 0;
        self.refresh(cx);
    }

    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.entries = entries(&query, &self.situation);
        self.selected = 0;
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries.get(self.selected) {
            cx.emit(PaletteEvent::Run(entry.command.clone()));
        }
    }

    fn up(&mut self, _: &PaletteUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }
    fn down(&mut self, _: &PaletteDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }
    fn enter(&mut self, _: &PaletteConfirm, _: &mut Window, cx: &mut Context<Self>) {
        self.confirm(cx);
    }
    fn close(&mut self, _: &PaletteClose, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Dismiss);
    }

    fn footer_note(&self) -> String {
        let n = self.entries.len();
        let files = self.situation.files.len();
        let results = if self.query_is_empty() { String::new() } else { format!("{n} {}", if n == 1 { "result" } else { "results" }) };
        let keep = match files {
            0 => String::new(),
            1 => "your file stays loaded".to_string(),
            k => format!("your {k} files stay loaded"),
        };
        [results, keep].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
    }

    fn query_is_empty(&self) -> bool {
        self.entries.iter().all(|e| matches!(e.section, Section::Recent | Section::Suggested))
    }
}

/// Text with the matched ranges picked out in the accent colour.
fn highlighted(text: &str, hits: &[(usize, usize)], base: Hsla, p: &Palette) -> Div {
    let mut out = div().flex().flex_row().flex_shrink_0().whitespace_nowrap();
    let mut at = 0;
    for &(a, b) in hits {
        if a > at && a <= text.len() {
            out = out.child(div().text_color(base).child(SharedString::from(text[at..a].to_string())));
        }
        if b <= text.len() && a < b {
            out = out.child(div().text_color(p.cyan).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(text[a..b].to_string())));
            at = b;
        }
    }
    if at < text.len() {
        out = out.child(div().text_color(base).child(SharedString::from(text[at..].to_string())));
    }
    out
}

impl Render for PaletteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let section_title = |section: Section, situation: &Situation| -> String {
            match section {
                Section::Recent => "Recent".into(),
                Section::Suggested => match situation.files.first().and_then(|f| f.file_name()) {
                    Some(name) => format!("Suggested for {}", name.to_string_lossy()),
                    None => "Suggested".into(),
                },
                Section::Modes => "Modes".into(),
                Section::Actions => "Actions".into(),
            }
        };

        let mut list = div().id("palette-list").flex().flex_col().gap(px(1.)).p(px(6.)).max_h(px(392.)).overflow_y_scroll().track_scroll(&self.scroll);
        let mut last: Option<Section> = None;
        for (ix, entry) in self.entries.iter().enumerate() {
            if last != Some(entry.section) {
                last = Some(entry.section);
                list = list.child(ui::row().h(px(28.)).px(px(10.)).flex_shrink_0().child(ui::section_label(section_title(entry.section, &self.situation), &p)));
            }
            let sel = ix == self.selected;
            let command = entry.command.clone();
            let group = entry.group;
            list = list.child(
                div()
                    .id(("palette-row", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(10.))
                    .h(px(36.))
                    .px(px(10.))
                    .rounded(px(R_SM))
                    .cursor_pointer()
                    .when(sel, |s| s.bg(p.plate_2))
                    .when(!sel, |s| s.hover(|s| s.bg(p.plate)))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(PaletteEvent::Run(command.clone()))))
                    .child(ui::icon(entry.icon, 14., if sel { p.toner } else { p.toner_3 }))
                    .child(highlighted(&entry.title, &entry.title_hits, p.toner, &p).font_weight(FontWeight::MEDIUM))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(highlighted(&entry.subtitle, &entry.subtitle_hits, p.toner_3, &p).text_size(px(TEXT_SM))))
                    .when_some(group, |s, g| s.child(ui::row().gap(px(6.)).flex_shrink_0().child(ui::dot(p.group(g), 6.)).child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child(g.name()))))
                    .child(ui::row().w(px(84.)).justify_end().flex_shrink_0().when(!entry.keys.is_empty(), |s| s.child(ui::kbd(&entry.keys, &p, KeyOn::Surface)))),
            );
        }
        if self.entries.is_empty() {
            list = list.child(ui::row().h(px(64.)).justify_center().text_color(p.toner_3).child("Nothing matches. Try a tool name such as merge or compress."));
        }

        let hint = |keys: &[&str], label: &'static str| ui::row().gap(px(6.)).child(ui::kbd(keys, &p, KeyOn::Surface)).child(label);
        let note = self.footer_note();

        div()
            .id("palette")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::close))
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(104.))
            .bg(p.scrim)
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismiss)))
            .child(
                div()
                    .id("palette-box")
                    .w(px(640.))
                    .flex()
                    .flex_col()
                    .rounded(px(R_LG))
                    .border_1()
                    .border_color(p.rule_strong)
                    .bg(p.sleeve)
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        ui::row()
                            .h(px(48.))
                            .pl(px(16.))
                            .pr(px(12.))
                            .gap(px(10.))
                            .border_b_1()
                            .border_color(p.rule)
                            .child(ui::icon("search", 16., p.toner_3))
                            .child(div().flex_1().min_w_0().text_size(px(15.)).child(Input::new(&self.input).appearance(false)))
                            .child(ui::kbd(&["Esc"], &p, KeyOn::Surface)),
                    )
                    .child(list)
                    .child(
                        ui::row()
                            .h(px(36.))
                            .px(px(12.))
                            .gap(px(16.))
                            .border_t_1()
                            .border_color(p.rule)
                            .text_size(px(TEXT_SM))
                            .text_color(p.toner_3)
                            .child(hint(&["Up", "Down"], "Move"))
                            .child(hint(&["Enter"], "Open"))
                            .child(hint(&["Esc"], "Close"))
                            .child(ui::grow())
                            .child(SharedString::from(note)),
                    ),
            )
    }
}

/// The label for the shortcut that opens the palette, as key caps.
pub fn open_keys() -> [&'static str; 2] {
    [mod_label(), "K"]
}
