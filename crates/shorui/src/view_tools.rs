//! The parts of the canvas and the options panel that belong to one tool:
//! Redact, Sign and Pages. Edit & Fill has its own module, `view_edit`.

use crate::catalog::Tool;
use crate::shell::Shell;
use crate::tokens::*;
use crate::ui::{self, Btn, pal};
use crate::view_canvas::Mark;
use crate::view_panel::InputTarget;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Map, Value, json};
use shorui_core::text::Rect4;
use shorui_core::tools::{redact, sign};
use std::path::PathBuf;

const ALL_PATTERNS: [&str; 5] = ["email", "phone", "iban", "date", "card"];

impl Shell {
    // ------------------------------------------------------------------ shared

    /// Load whatever the canvas tool needs to know about the active file, once per file.
    pub fn sync_canvas_file(&mut self, tool: &'static Tool, cx: &mut Context<Self>) {
        let Some(path) = self.state.active().map(|f| f.path.clone()) else { return };
        if self.canvas.loaded_for.as_ref() == Some(&path) {
            return;
        }
        self.canvas.reset();
        self.canvas.loaded_for = Some(path.clone());
        self.canvas.whole_words = false;
        self.canvas_size_for = None;
        self.reset_inputs();
        match tool.id {
            "redact" => self.refresh_marks(cx),
            "edit" => self.load_fields(path, cx),
            "sign" => self.refresh_sign_size(cx),
            _ => {}
        }
    }

    fn enabled_patterns(&mut self) -> Vec<String> {
        self.state.options_mut("redact").get("patterns").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    /// The marks that will be applied on a run: drawn areas, search hits, and hits of the
    /// patterns that are switched on.
    pub fn active_marks(&mut self) -> Vec<Mark> {
        let patterns = self.enabled_patterns();
        self.canvas.areas.iter().cloned().chain(self.canvas.found.iter().filter(|m| m.source == "search" || patterns.contains(&m.pattern)).cloned()).collect()
    }

    // ------------------------------------------------------------------ redact

    pub fn set_redact_search(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.canvas.search != text {
            self.canvas.search = text.to_string();
            self.refresh_marks(cx);
        }
    }

    /// Find search and pattern hits off the UI thread.
    pub fn refresh_marks(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.state.active().map(|f| f.path.clone()) else { return };
        self.canvas.generation += 1;
        let generation = self.canvas.generation;
        let text = self.canvas.search.trim().to_string();
        let opts = redact::Options {
            search: if text.chars().count() >= 2 { vec![redact::Search { text, match_case: self.canvas.match_case, whole_words: self.canvas.whole_words }] } else { vec![] },
            patterns: ALL_PATTERNS.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        };
        let task = cx.background_spawn(async move { redact::find_marks(&path, None, &opts) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.canvas.generation != generation {
                    return;
                }
                this.canvas.found = result
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|m| m.source != redact::Source::Area)
                    .map(|m| Mark { page: m.page.saturating_sub(1), rect: m.rect, source: if m.source == redact::Source::Search { "search".into() } else { "pattern".into() }, label: m.label, pattern: m.pattern })
                    .collect();
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------ sign

    fn sign_options(&mut self) -> sign::Options {
        serde_json::from_value(Value::Object(self.state.options_of("sign"))).unwrap_or_default()
    }

    pub fn refresh_sign_size(&mut self, cx: &mut Context<Self>) {
        let opts = self.sign_options();
        self.canvas.sign_size = sign::placed_size(&opts).ok();
        cx.notify();
    }

    // ------------------------------------------------------------------ run-time options

    /// Add what the canvas holds to the options of a run. `Err` explains what is missing.
    pub fn canvas_options(&mut self, tool: &'static Tool, options: &mut Map<String, Value>) -> Result<(), String> {
        match tool.id {
            "redact" => {
                let areas: Vec<Value> = self.canvas.areas.iter().map(|m| json!({ "page": m.page + 1, "rect": m.rect })).collect();
                let text = self.canvas.search.trim().to_string();
                let search = if text.chars().count() >= 2 { vec![json!({ "text": text, "match_case": self.canvas.match_case, "whole_words": self.canvas.whole_words })] } else { vec![] };
                if self.active_marks().is_empty() {
                    return Err("Mark something first: drag over the page, search for text, or switch on a pattern.".into());
                }
                options.insert("areas".into(), Value::Array(areas));
                options.insert("search".into(), Value::Array(search));
            }
            "sign" => {
                let has_image = options.get("image").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
                let has_name = options.get("typed").and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
                if !has_image && !has_name {
                    return Err("Choose a signature image, or type your name, in the options.".into());
                }
                let Some((page, x, y)) = self.canvas.placed else {
                    return Err("Click the page where the signature should go.".into());
                };
                options.insert("page".into(), json!(page + 1));
                options.insert("x".into(), json!(x));
                options.insert("y".into(), json!(y));
            }
            "edit" => self.edit_options(options)?,
            _ => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------------ canvas overlays

    /// Marks, placed items and the pointer handling for the page canvas.
    pub fn canvas_overlays(&mut self, tool: &'static Tool, mut sheet: Stateful<Div>, w: f32, _h: f32, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *p;
        let page = self.canvas.page;
        let Some((pw, _ph)) = self.canvas.page_size else { return sheet };
        let scale = w / pw;
        let locked = self.state.running;

        match tool.id {
            "redact" => {
                for mark in self.active_marks().into_iter().filter(|m| m.page == page) {
                    let area = mark.source == "area";
                    let mut el = Shell::mark_box(&mark.rect, scale).bg(p.blackout).border_1().border_color(p.stamp_fill).opacity(0.86);
                    if area && !locked {
                        let rect = mark.rect;
                        el = el.child(
                            div().id(("unmark", (rect.x0 * 10.) as usize * 10_000 + (rect.y0 * 10.) as usize)).absolute().top(px(-9.)).right(px(-9.)).size(px(18.)).rounded_full().bg(p.sleeve).border_1().border_color(p.rule_strong).flex().items_center().justify_center().cursor_pointer().occlude().child(ui::icon("x", 10., p.toner)).on_click(cx.listener(move |this, _, _, cx| {
                                this.canvas.areas.retain(|m| !(m.page == page && m.rect == rect));
                                cx.notify();
                            })),
                        );
                    }
                    sheet = sheet.child(el);
                }
            }
            "sign" => {
                if let (Some((pg, x, y)), Some((sw, sh))) = (self.canvas.placed, self.canvas.sign_size) {
                    let all = self.state.options_mut("sign").get("all_pages").and_then(Value::as_bool).unwrap_or(false);
                    if pg == page || all {
                        let rect = Rect4::new(x, y, x + sw, y + sh);
                        let image = self.state.options_mut("sign").get("image").and_then(Value::as_str).filter(|s| !s.is_empty()).map(PathBuf::from);
                        let typed = self.state.options_mut("sign").get("typed").and_then(Value::as_str).unwrap_or("").to_string();
                        let mut el = Shell::mark_box(&rect, scale).border_1().border_color(p.stock_select).flex().items_center().justify_center();
                        el = match image {
                            Some(path) => el.child(img(path).size_full().object_fit(ObjectFit::Contain)),
                            None => el.child(div().text_color(p.stock_ink).text_size(px((sh * scale * 0.6).max(8.))).child(SharedString::from(typed))),
                        };
                        sheet = sheet.child(el);
                    }
                }
            }
            _ => {}
        }

        // The box being dragged out.
        if let Some((a, b)) = self.canvas.drag {
            if let (Some((ax, ay)), Some((bx, by))) = (self.to_page_point(a), self.to_page_point(b)) {
                let rect = Rect4::new(ax, ay, bx, by);
                sheet = sheet.child(Shell::mark_box(&rect, scale).border_1().border_color(p.cyan).bg(p.cyan_wash));
            }
        }
        if locked {
            return sheet;
        }

        let id = tool.id;
        let boxes = id == "redact";
        sheet
            .cursor(if boxes { CursorStyle::Crosshair } else { CursorStyle::PointingHand })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    if boxes {
                        this.canvas.drag = Some((e.position, e.position));
                    } else {
                        this.canvas_click(id, e.position, cx);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                if let Some((start, _)) = this.canvas.drag {
                    if e.pressed_button == Some(MouseButton::Left) {
                        this.canvas.drag = Some((start, e.position));
                    } else {
                        this.canvas.drag = None;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseUpEvent, _, cx| {
                    if let Some((start, _)) = this.canvas.drag.take() {
                        if let (Some((ax, ay)), Some((bx, by))) = (this.to_page_point(start), this.to_page_point(e.position)) {
                            let rect = Rect4::new(ax, ay, bx, by);
                            if rect.width() >= 3. && rect.height() >= 3. {
                                let page = this.canvas.page;
                                this.canvas.areas.push(Mark { page, rect, source: "area".into(), label: String::new(), pattern: String::new() });
                            }
                        }
                        cx.notify();
                    }
                }),
            )
    }

    fn canvas_click(&mut self, tool: &'static str, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((x, y)) = self.to_page_point(at) else { return };
        let page = self.canvas.page;
        match tool {
            "sign" => {
                if self.canvas.sign_size.is_none() {
                    self.refresh_sign_size(cx);
                }
                match self.canvas.sign_size {
                    // Centre the signature on the click.
                    Some((w, h)) => self.canvas.placed = Some((page, (x - w / 2.).max(0.), (y - h / 2.).max(0.))),
                    None => self.show_toast(false, "Choose a signature first", "Pick an image or type your name in the options, then click the page.", cx),
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ panel

    /// Shown above the schema fields.
    pub fn panel_intro(&mut self, tool: &'static Tool, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = pal(cx);
        let section = |title: &'static str| ui::col().px(px(16.)).pt(px(8.)).pb(px(12.)).border_t_1().border_color(p.rule_soft).child(ui::row().h(px(28.)).child(ui::section_label(title, &p)));
        match tool.id {
            "redact" => {
                let marks = self.active_marks();
                let pages: std::collections::BTreeSet<usize> = marks.iter().map(|m| m.page).collect();
                let hits = self.canvas.found.iter().filter(|m| m.source == "search").count();
                let match_case = self.canvas.match_case;
                let whole_words = self.canvas.whole_words;
                let search = self.text_field("redact:search", "Text to find", &self.canvas.search.clone(), false, InputTarget::RedactSearch, window, cx);
                let warning = div().px(px(12.)).pb(px(12.)).child(
                    ui::row()
                        .items_start()
                        .px(px(12.))
                        .py(px(10.))
                        .gap(px(10.))
                        .rounded(px(R_MD))
                        .border_1()
                        .border_color(p.stamp_rule)
                        .bg(p.stamp_wash)
                        .child(div().pt(px(2.)).child(ui::icon("alert", 14., p.stamp)))
                        .child(ui::col().flex_1().min_w_0().gap(px(2.)).child(div().font_weight(FontWeight::SEMIBOLD).child("Redaction is permanent")).child(div().text_size(px(TEXT_SM)).line_height(px(18.)).text_color(p.toner_2).child("Marked pages are rebuilt as images, so the removed content cannot be recovered. The original file is not changed."))),
                );
                let find = section("Search to redact")
                    .child(div().pt(px(2.)).pb(px(6.)).child(search))
                    .child(
                        ui::row()
                            .gap(px(16.))
                            .min_h(px(28.))
                            .child(ui::row().gap(px(8.)).child(ui::checkbox("match-case", match_case, &p).on_click(cx.listener(|this, _, _, cx| {
                                this.canvas.match_case = !this.canvas.match_case;
                                this.refresh_marks(cx);
                            }))).child("Match case"))
                            .child(ui::row().gap(px(8.)).child(ui::checkbox("whole-words", whole_words, &p).on_click(cx.listener(|this, _, _, cx| {
                                this.canvas.whole_words = !this.canvas.whole_words;
                                this.refresh_marks(cx);
                            }))).child("Whole words")),
                    )
                    .child(ui::row().min_h(px(28.)).text_color(p.toner_2).child(SharedString::from(if self.canvas.search.trim().chars().count() < 2 { "Type at least two characters.".to_string() } else { format!("{hits} {} found", if hits == 1 { "match" } else { "matches" }) })));
                let summary = section("Marks").child(
                    ui::row()
                        .min_h(px(28.))
                        .justify_between()
                        .child(SharedString::from(format!("{} {} on {} {}", marks.len(), if marks.len() == 1 { "mark" } else { "marks" }, pages.len(), if pages.len() == 1 { "page" } else { "pages" })))
                        .when(!self.canvas.areas.is_empty(), |s| {
                            s.child(ui::button("clear-areas", Btn::Ghost, 24., true, &p).child("Clear drawn areas").on_click(cx.listener(|this, _, _, cx| {
                                this.canvas.areas.clear();
                                cx.notify();
                            })))
                        }),
                );
                Some(ui::col().child(warning).child(find).child(summary).into_any_element())
            }
            "pages" => {
                let edit = self.state.pages_edit.clone();
                let changes = edit.changes();
                let mut pending = section("Pending changes");
                if changes.is_empty() {
                    pending = pending.child(div().py(px(4.)).text_size(px(TEXT_SM)).text_color(p.toner_3).child("None yet. Select pages, then rotate, delete, duplicate or drag them."));
                }
                for change in changes {
                    pending = pending.child(ui::row().min_h(px(28.)).child(SharedString::from(change)));
                }
                let quick = section("Select").child(
                    ui::row()
                        .flex_wrap()
                        .gap(px(6.))
                        .py(px(2.))
                        .child(ui::toggle_chip("sel-odd", "Odd", false, None, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.select_where(|i, _| i % 2 == 0);
                            cx.notify();
                        })))
                        .child(ui::toggle_chip("sel-even", "Even", false, None, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.select_where(|i, _| i % 2 == 1);
                            cx.notify();
                        })))
                        .child(ui::toggle_chip("sel-all2", "All", false, None, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.select_all();
                            cx.notify();
                        })))
                        .child(ui::toggle_chip("sel-none", "None", false, None, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.selected.clear();
                            cx.notify();
                        }))),
                );
                let selection = if edit.selected.is_empty() { "Nothing selected".to_string() } else { format!("Selected: {}", edit.selection_text()) };
                let rotate = section("Rotate selection").child(
                    ui::segmented(&p)
                        .w_full()
                        .child(ui::segment("rot-left", "90° left", false, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.rotate(-90);
                            cx.notify();
                        })))
                        .child(ui::segment("rot-180", "180°", false, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.rotate(180);
                            cx.notify();
                        })))
                        .child(ui::segment("rot-right", "90° right", false, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.state.pages_edit.rotate(90);
                            cx.notify();
                        }))),
                );
                let keys = section("Keys")
                    .child(ui::row().min_h(px(26.)).justify_between().text_color(p.toner_2).child("Move").child(ui::kbd(&["Up", "Down"], &p, ui::KeyOn::Surface)))
                    .child(ui::row().min_h(px(26.)).justify_between().text_color(p.toner_2).child("Rotate").child(ui::kbd(&["R"], &p, ui::KeyOn::Surface)))
                    .child(ui::row().min_h(px(26.)).justify_between().text_color(p.toner_2).child("Delete").child(ui::kbd(&["Del"], &p, ui::KeyOn::Surface)))
                    .child(ui::row().min_h(px(26.)).justify_between().text_color(p.toner_2).child("Select all").child(ui::kbd(&[crate::palette_model::mod_label(), "A"], &p, ui::KeyOn::Surface)));
                Some(ui::col().child(ui::col().px(px(16.)).pb(px(12.)).child(ui::row().min_h(px(28.)).font_family(FONT_MONO).text_size(px(TEXT_SM)).text_color(p.toner_2).child(SharedString::from(selection)))).child(quick).child(rotate).child(pending).child(keys).into_any_element())
            }
            "edit" => Some(self.edit_panel(&p, cx)),
            _ => None,
        }
    }

    /// Shown below the schema fields.
    pub fn panel_outro(&mut self, tool: &'static Tool, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = pal(cx);
        match tool.id {
            "sign" => {
                let text = match self.canvas.placed {
                    Some((page, x, y)) => format!("Placed on page {} at {:.0}, {:.0} pt.", page + 1, x, y),
                    None => "Not placed yet. Click the page where it should go.".to_string(),
                };
                Some(ui::col().px(px(16.)).pt(px(8.)).pb(px(12.)).border_t_1().border_color(p.rule_soft).child(ui::row().h(px(28.)).child(ui::section_label("Position", &p))).child(div().text_color(p.toner_2).child(SharedString::from(text))).into_any_element())
            }
            _ => None,
        }
    }

    /// The number shown on a chip, when there is one to show.
    pub fn chip_count(&self, tool: &'static Tool, value: &str) -> Option<usize> {
        if tool.id == "redact" { Some(self.canvas.found.iter().filter(|m| m.pattern == value).count()) } else { None }
    }

    /// Called after an option changes in a way a preview depends on.
    pub fn options_changed(&mut self, tool: &'static str, cx: &mut Context<Self>) {
        self.prefs_changed(cx);
        if tool == "sign" {
            self.refresh_sign_size(cx);
        }
        cx.notify();
    }
}
