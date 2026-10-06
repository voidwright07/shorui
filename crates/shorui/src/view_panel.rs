//! The options panel: an inspector drawn from each tool's field schema.

use crate::catalog::{Control, Field, Tool};
use crate::shell::Shell;
use crate::state::value_text;
use crate::tokens::*;
use crate::ui::{self, Btn, pal};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};
use std::path::PathBuf;

/// Where a text field writes what is typed into it.
#[derive(Clone, Debug, PartialEq)]
pub enum InputTarget {
    Option { tool: &'static str, key: &'static str },
    Range { path: PathBuf },
    RedactSearch,
}

pub struct InputSlot {
    pub state: Entity<InputState>,
    _subscription: Subscription,
}

impl InputSlot {
    pub fn new(state: Entity<InputState>, subscription: Subscription) -> Self {
        InputSlot { state, _subscription: subscription }
    }
}

impl Shell {
    /// A text field bound to `target`. The field's state is created on first use and kept
    /// under `key`, so typing survives re-renders.
    pub fn text_field(&mut self, key: &str, placeholder: &str, value: &str, mono: bool, target: InputTarget, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        if !self.inputs.contains_key(key) {
            let placeholder = placeholder.to_string();
            let initial = value.to_string();
            let masked = key.ends_with("password");
            let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(initial).masked(masked));
            let subscription = cx.subscribe_in(&state, window, move |this: &mut Self, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    this.apply_input(&target, &text, cx);
                }
            });
            self.inputs.insert(key.to_string(), InputSlot { state, _subscription: subscription });
        }
        let state = self.inputs[key].state.clone();
        div()
            .h(px(CONTROL_H))
            .w_full()
            .flex()
            .items_center()
            .rounded(px(R_SM))
            .border_1()
            .border_color(p.control_rule)
            .bg(p.well)
            .when(mono, |s| s.font_family(FONT_MONO).text_size(px(TEXT_SM)))
            .child(Input::new(&state).appearance(false))
            .into_any_element()
    }

    fn apply_input(&mut self, target: &InputTarget, text: &str, cx: &mut Context<Self>) {
        match target {
            InputTarget::Option { tool, key } => {
                let Some(t) = crate::catalog::tool(tool) else { return };
                match t.fields.iter().find(|f| f.key == *key) {
                    Some(field) => self.state.set_option_text(tool, field, text),
                    None => self.state.set_option(tool, key, Value::String(text.to_string())),
                }
                self.options_changed(tool, cx);
                self.prefs_changed(cx);
            }
            InputTarget::Range { path } => {
                if let Some(f) = self.state.files.iter_mut().find(|f| &f.path == path) {
                    f.range = text.to_string();
                }
            }
            InputTarget::RedactSearch => self.set_redact_search(text, cx),
        }
        cx.notify();
    }

    /// Forget every text field so the next render rebuilds them from the options.
    pub fn reset_inputs(&mut self) {
        // Text being typed on the page is kept: its field goes away with the rest.
        self.finish_edit();
        self.inputs.clear();
    }

    pub fn render_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tool = self.state.tool()?;
        let p = pal(cx);
        let locked = self.state.running;
        let id = tool.id;

        let head = ui::row()
            .h(px(TOPBAR_H))
            .pl(px(16.))
            .pr(px(8.))
            .justify_between()
            .flex_shrink_0()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Options"))
            .child(if locked {
                div().pr(px(8.)).text_size(px(TEXT_SM)).text_color(p.toner_3).child("Locked while running").into_any_element()
            } else {
                ui::button("reset", Btn::Ghost, 24., true, &p)
                    .child("Reset")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.options.remove(id);
                        this.reset_inputs();
                        this.prefs_changed(cx);
                        cx.notify();
                    }))
                    .into_any_element()
            });

        let mut body = div().id("panel-body").flex().flex_col().flex_1().min_h_0().overflow_y_scroll().when(locked, |s| s.opacity(0.45));
        if let Some(extra) = self.panel_intro(tool, window, cx) {
            body = body.child(extra);
        }

        let mut section: Option<&'static str> = None;
        let mut current: Option<Div> = None;
        for field in tool.fields {
            if !self.state.field_visible(id, field) {
                continue;
            }
            if section != Some(field.section) {
                if let Some(done) = current.take() {
                    body = body.child(done);
                }
                section = Some(field.section);
                current = Some(ui::col().px(px(16.)).pt(px(8.)).pb(px(12.)).border_t_1().border_color(p.rule_soft).child(ui::row().h(px(28.)).child(ui::section_label(field.section, &p))));
            }
            let row = self.field_row(tool, field, locked, window, cx);
            current = current.map(|c| c.child(row));
        }
        if let Some(done) = current.take() {
            body = body.child(done);
        }
        if let Some(extra) = self.panel_outro(tool, window, cx) {
            body = body.child(extra);
        }

        Some(ui::col().w(px(PANEL_W)).h_full().flex_shrink_0().border_l_1().border_color(p.rule).child(head).child(body).into_any_element())
    }

    fn field_row(&mut self, tool: &'static Tool, field: &'static Field, locked: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = pal(cx);
        let id = tool.id;
        let key = field.key;
        let value = self.state.options_mut(id).get(key).cloned().unwrap_or(Value::Null);
        let text = value_text(&value);
        let set = move |this: &mut Shell, v: Value, cx: &mut Context<Shell>| {
            this.state.set_option(id, key, v);
            this.prefs_changed(cx);
            cx.notify();
        };
        let label = field.label;
        let ix = field_index(tool, field);

        match field.control {
            Control::Note => div().py(px(4.)).text_size(px(TEXT_SM)).line_height(px(18.)).text_color(p.toner_3).child(label).into_any_element(),
            Control::Switch => {
                let on = value.as_bool().unwrap_or(false);
                let mut sw = ui::switch(("sw", ix), on, &p);
                if !locked {
                    sw = sw.on_click(cx.listener(move |this, _, _, cx| set(this, json!(!on), cx)));
                }
                ui::prop_row(label, &p).child(sw).into_any_element()
            }
            Control::Text { placeholder, mono } => {
                let input = self.text_field(&format!("opt:{id}:{key}"), placeholder, &text, mono, InputTarget::Option { tool: id, key }, window, cx);
                ui::col().gap(px(6.)).py(px(4.)).child(div().text_color(p.toner_2).child(label)).child(input).into_any_element()
            }
            Control::Password { placeholder } => {
                let input = self.text_field(&format!("opt:{id}:{key}"), placeholder, &text, true, InputTarget::Option { tool: id, key }, window, cx);
                ui::col().gap(px(6.)).py(px(4.)).child(div().text_color(p.toner_2).child(label)).child(input).into_any_element()
            }
            Control::Number { unit, .. } => {
                let input = self.text_field(&format!("opt:{id}:{key}"), "", &text, true, InputTarget::Option { tool: id, key }, window, cx);
                ui::prop_row(label, &p).child(ui::row().gap(px(6.)).child(div().w(px(72.)).child(input)).when(!unit.is_empty(), |s| s.child(ui::mono(unit).text_color(p.toner_3).w(px(22.))))).into_any_element()
            }
            Control::Slider { min, max, step, unit } => {
                let current = value.as_f64().unwrap_or(min);
                let fraction = ((current - min) / (max - min)).clamp(0.0, 1.0) as f32;
                let shown = if step < 1.0 { format!("{current:.2}") } else { format!("{current:.0}") };
                let integer = value.is_i64() || value.is_u64();
                let slot = format!("slider:{id}:{key}");
                let bounds = self.slider_bounds.clone();
                let bounds_for_canvas = bounds.clone();
                let slot_for_canvas = slot.clone();
                let pick = move |this: &mut Shell, x: Pixels, cx: &mut Context<Shell>| {
                    let Some(b) = bounds.borrow().get(&slot).copied() else { return };
                    let f = ((x - b.origin.x) / b.size.width).clamp(0.0, 1.0) as f64;
                    let raw = min + f * (max - min);
                    let stepped = ((raw / step).round() * step).clamp(min, max);
                    set(this, if integer { json!(stepped.round() as i64) } else { json!(stepped) }, cx);
                };
                let pick_move = pick.clone();
                let track = div()
                    .id(("slider", ix))
                    .relative()
                    .w(px(112.))
                    .h(px(16.))
                    .cursor_pointer()
                    .child(canvas(move |b, _, _| { bounds_for_canvas.borrow_mut().insert(slot_for_canvas.clone(), b); }, |_, _, _, _| {}).absolute().size_full())
                    .child(div().absolute().left_0().right_0().top(px(6.)).h(px(4.)).rounded(px(2.)).bg(p.plate_3).child(div().h_full().w(relative(fraction)).rounded(px(2.)).bg(p.cyan)))
                    .child(div().absolute().top(px(2.)).left(relative(fraction)).ml(px(-6.)).size(px(12.)).rounded_full().bg(p.thumb).border_1().border_color(p.rule_strong))
                    .when(!locked, |s| {
                        s.on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| pick(this, e.position.x, cx))).on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                            if e.pressed_button == Some(MouseButton::Left) {
                                pick_move(this, e.position.x, cx);
                            }
                        }))
                    });
                ui::prop_row(label, &p).child(ui::row().gap(px(10.)).child(track).child(ui::mono(format!("{shown}{unit}")).w(px(44.)).text_right())).into_any_element()
            }
            Control::Segmented(options) => {
                let mut seg = ui::segmented(&p).w_full();
                for (i, (v, l)) in options.iter().enumerate() {
                    let v: &'static str = v;
                    let mut part = ui::segment(("seg", ix * 32 + i), *l, text == v, &p);
                    if !locked {
                        part = part.on_click(cx.listener(move |this, _, _, cx| {
                            set(this, json!(v), cx);
                            this.reset_inputs();
                        }));
                    }
                    seg = seg.child(part);
                }
                ui::col().gap(px(6.)).py(px(4.)).when(!label.is_empty(), |s| s.child(div().text_color(p.toner_2).child(label))).child(seg).into_any_element()
            }
            Control::Select(options) => {
                let shown = options.iter().find(|(v, _)| *v == text).map(|(_, l)| *l).unwrap_or("Choose");
                let slot = format!("{id}:{key}");
                let open = self.open_select.as_deref() == Some(slot.as_str());
                let slot_toggle = slot.clone();
                let numeric = value.is_number();
                let trigger = div()
                    .id(("select", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .w(px(148.))
                    .h(px(CONTROL_H))
                    .px(px(8.))
                    .rounded(px(R_SM))
                    .border_1()
                    .border_color(p.control_rule)
                    .bg(p.plate)
                    .cursor_pointer()
                    .tab_index(0)
                    .hover(move |s| s.bg(p.plate_2))
                    .focus_visible(move |s| ui::ring(s, &p))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(shown))
                    .child(ui::icon("chev-down", 12., p.toner_3))
                    .when(!locked, |s| {
                        s.on_click(cx.listener(move |this, _, _, cx| {
                            this.open_select = if this.open_select.as_deref() == Some(slot_toggle.as_str()) { None } else { Some(slot_toggle.clone()) };
                            cx.notify();
                        }))
                    });
                let mut holder = div().relative().child(trigger);
                if open {
                    let mut menu = ui::col().absolute().top(px(CONTROL_H + 4.)).right_0().w(px(184.)).p(px(4.)).gap(px(1.)).rounded(px(R_MD)).border_1().border_color(p.rule_strong).bg(p.sleeve).occlude();
                    for (i, (v, l)) in options.iter().enumerate() {
                        let v: &'static str = v;
                        let on = text == v;
                        menu = menu.child(
                            div()
                                .id(("select-item", ix * 32 + i))
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .h(px(28.))
                                .px(px(8.))
                                .rounded(px(R_SM))
                                .cursor_pointer()
                                .when(on, |s| s.bg(p.plate_2))
                                .hover(move |s| s.bg(p.plate_2))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let v_json = if numeric { v.parse::<f64>().map(|n| if n.fract() == 0.0 { json!(n as i64) } else { json!(n) }).unwrap_or(json!(v)) } else { json!(v) };
                                    this.open_select = None;
                                    set(this, v_json, cx);
                                    this.reset_inputs();
                                }))
                                .child(*l)
                                .when(on, |s| s.child(ui::icon("check", 12., p.cyan))),
                        );
                    }
                    holder = holder.child(deferred(menu).with_priority(1));
                }
                ui::prop_row(label, &p).child(holder).into_any_element()
            }
            Control::Radio(options) => {
                let mut list = ui::col().gap(px(2.)).p(px(4.)).mx(px(-4.)).rounded(px(R_MD)).border_1().border_color(p.rule);
                for (i, (v, l, d)) in options.iter().enumerate() {
                    let v: &'static str = v;
                    let on = text == v;
                    let mut item = div()
                        .id(("radio", ix * 32 + i))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.))
                        .h(px(44.))
                        .px(px(10.))
                        .rounded(px(R_SM))
                        .cursor_pointer()
                        .tab_index(0)
                        .when(on, |s| s.bg(p.plate_2))
                        .when(!on, |s| s.hover(move |s| s.bg(p.plate)))
                        .focus_visible(move |s| ui::ring(s, &p))
                        .child(div().flex().items_center().justify_center().size(px(14.)).rounded_full().flex_shrink_0().when(on, |s| s.bg(p.cyan).child(div().size(px(6.)).rounded_full().bg(p.cyan_ink))).when(!on, |s| s.bg(p.well).border_1().border_color(p.control_rule)))
                        .child(ui::col().flex_1().min_w_0().child(div().font_weight(FontWeight::MEDIUM).child(*l)).child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child(*d)));
                    if !locked {
                        item = item.on_click(cx.listener(move |this, _, _, cx| {
                            set(this, json!(v), cx);
                            this.reset_inputs();
                        }));
                    }
                    list = list.child(item);
                }
                ui::col().gap(px(6.)).py(px(2.)).when(!label.is_empty(), |s| s.child(div().text_color(p.toner_2).child(label))).child(list).into_any_element()
            }
            Control::Chips(options) => {
                let chosen: Vec<String> = value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let mut wrap = ui::row().flex_wrap().gap(px(6.)).py(px(2.));
                for (i, (v, l)) in options.iter().enumerate() {
                    let v: &'static str = v;
                    let on = chosen.iter().any(|c| c == v);
                    let count = self.chip_count(tool, v);
                    let mut chip = ui::toggle_chip(("chip", ix * 32 + i), *l, on, count, &p);
                    if !locked {
                        let chosen = chosen.clone();
                        chip = chip.on_click(cx.listener(move |this, _, _, cx| {
                            let mut next = chosen.clone();
                            if on {
                                next.retain(|c| c != v);
                            } else {
                                next.push(v.to_string());
                            }
                            set(this, json!(next), cx);
                            this.options_changed(id, cx);
                        }));
                    }
                    wrap = wrap.child(chip);
                }
                ui::col().gap(px(6.)).py(px(2.)).when(!label.is_empty(), |s| s.child(div().text_color(p.toner_2).child(label))).child(wrap).into_any_element()
            }
            Control::Color(swatches) => {
                let current: Vec<u8> = value.as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect()).unwrap_or_default();
                let mut dots = ui::row().gap(px(10.)).pr(px(2.));
                for (i, (_name, rgb)) in swatches.iter().enumerate() {
                    let rgb = *rgb;
                    let on = current.as_slice() == rgb;
                    let colour: Hsla = gpui_kit::rgb(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32).into();
                    let mut dot = div().id(("swatch", ix * 32 + i)).size(px(18.)).rounded_full().bg(colour).border_1().border_color(p.rule_strong).cursor_pointer().tab_index(0).when(on, |s| ui_ring(s, &p)).focus_visible(move |s| ui::ring(s, &p));
                    if !locked {
                        dot = dot.on_click(cx.listener(move |this, _, _, cx| set(this, json!([rgb[0], rgb[1], rgb[2]]), cx)));
                    }
                    dots = dots.child(dot);
                }
                ui::prop_row(label, &p).child(dots).into_any_element()
            }
            Control::File { extensions, placeholder } => {
                let chosen = value.as_str().filter(|s| !s.is_empty()).map(|s| PathBuf::from(s).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                let has = chosen.is_some();
                let mut pick = ui::button(("file", ix), Btn::Default, 28., !locked, &p).flex_1().min_w_0().justify_start().child(ui::icon("image", 14., p.toner_3)).child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(if has { p.toner } else { p.toner_3 }).child(SharedString::from(chosen.unwrap_or_else(|| placeholder.to_string()))));
                if !locked {
                    pick = pick.on_click(cx.listener(move |this, _, _, cx| {
                        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
                        cx.spawn(async move |this, cx| {
                            if let Ok(Ok(Some(paths))) = paths.await {
                                if let Some(path) = paths.into_iter().next() {
                                    let ok = path.extension().is_some_and(|e| extensions.contains(&e.to_string_lossy().to_lowercase().as_str()));
                                    let _ = this.update(cx, |this, cx| {
                                        if ok {
                                            this.state.set_option(id, key, json!(path.to_string_lossy()));
                                            this.options_changed(id, cx);
                                        } else {
                                            this.show_toast(false, "That file type cannot be used here", &format!("Choose one of: {}.", extensions.join(", ")), cx);
                                        }
                                        cx.notify();
                                    });
                                }
                            }
                        })
                        .detach();
                        let _ = this;
                    }));
                }
                let mut line = ui::row().gap(px(6.)).child(pick);
                if has && !locked {
                    line = line.child(ui::icon_button(("file-clear", ix), "x", 24., &p).on_click(cx.listener(move |this, _, _, cx| {
                        set(this, Value::Null, cx);
                        this.options_changed(id, cx);
                    })));
                }
                ui::col().gap(px(6.)).py(px(4.)).child(div().text_color(p.toner_2).child(label)).child(line).into_any_element()
            }
        }
    }
}

/// The selected-swatch ring.
fn ui_ring(s: Stateful<Div>, p: &Palette) -> Stateful<Div> {
    s.border_2().border_color(p.cyan)
}

fn field_index(tool: &'static Tool, field: &'static Field) -> usize {
    let t = crate::catalog::TOOLS.iter().position(|t| t.id == tool.id).unwrap_or(0);
    let f = tool.fields.iter().position(|x| std::ptr::eq(x, field)).unwrap_or(0);
    t * 64 + f
}
