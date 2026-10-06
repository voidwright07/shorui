//! Edit & Fill. Everything is done on the page itself: pick a tool above the page, click
//! where it should go, and type in place. Form fields are filled where they sit.

use crate::shell::Shell;
use crate::tokens::*;
use crate::ui;
use crate::view_canvas::{EditTool, Editing, INKS, Moving, Pending, Placed};
use crate::view_panel::InputSlot;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Map, Value, json};
use shorui_core::doc::StdFont;
use shorui_core::text::Rect4;
use shorui_core::tools::edit::{self, FieldInfo, FieldType};
use std::path::PathBuf;

/// Key of the one text field that lives on the page.
const INLINE: &str = "edit:inline";

/// The box a line of typed text takes up: left edge and top of the capitals at `x`, `y`.
fn text_rect(x: f32, y: f32, text: &str, size: f32) -> Rect4 {
    let width = StdFont::Helvetica.width(text, size).max(size * 0.5);
    Rect4::new(x, y, x + width, y + size)
}

/// Top and height, in points, of the line box that puts the capitals of `size` text at `y`.
/// The saved file sets the top of the capitals at `y`; a line centred in this box matches.
fn line_box(y: f32, size: f32) -> (f32, f32) {
    (y - 0.33 * size, 1.4 * size)
}

/// Font size used inside a form field of this height, in points.
fn field_font(height: f32) -> f32 {
    (height * 0.62).clamp(6., 11.)
}

fn inside(r: &Rect4, x: f32, y: f32) -> bool {
    x >= r.x0 && x <= r.x1 && y >= r.y0 && y <= r.y1
}

fn ink(c: [u8; 3]) -> Hsla {
    rgb(((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32).into()
}

/// The wash that shows where a form field can be filled.
fn field_tint() -> Hsla {
    hsla(0.6, 0.9, 0.5, 0.10)
}

/// The key a place of a form field keeps its value under. Text and list fields that show
/// in several places get one value per place, so filling one place leaves the others
/// alone. Tick boxes and radio buttons share one value, as a group must.
pub fn place_key(field: &FieldInfo, widget: &shorui_core::tools::edit::WidgetInfo) -> String {
    if field.widgets.len() > 1 && matches!(field.kind, FieldType::Text | FieldType::Choice) { format!("{}\u{1f}{}", field.name, widget.index) } else { field.name.clone() }
}

/// Every place's value as the file has it.
fn initial_values(fields: &[FieldInfo]) -> std::collections::BTreeMap<String, String> {
    fields.iter().flat_map(|f| {
        let keys: Vec<String> = if f.widgets.is_empty() { vec![f.name.clone()] } else { f.widgets.iter().map(|w| place_key(f, w)).collect() };
        keys.into_iter().map(move |k| (k, f.value.clone()))
    }).collect()
}

/// A field counts as filled when any of its places holds something.
fn is_filled(field: &FieldInfo, values: &std::collections::BTreeMap<String, String>) -> bool {
    let filled = |k: &String| values.get(k).is_some_and(|v| !v.is_empty() && v != "false");
    filled(&field.name) || field.widgets.iter().any(|w| filled(&place_key(field, w)))
}

fn fillable(field: &FieldInfo) -> bool {
    !field.read_only && matches!(field.kind, FieldType::Text | FieldType::Checkbox | FieldType::Radio | FieldType::Choice)
}

#[derive(Clone, Copy)]
enum Shape {
    Check,
    Cross,
    Oval,
    Line,
}

/// Strokes that fill the element they are put in, drawn the way the saved file draws them.
fn strokes(shape: Shape, color: Hsla, width: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b: Bounds<Pixels>, _, window: &mut Window, _| {
            let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
            let at = |fx: f32, fy: f32| point(px(x + w * fx), px(y + h * fy));
            let mut path = PathBuilder::stroke(px(width.max(0.75)));
            match shape {
                Shape::Check => {
                    path.move_to(at(0.15, 0.5));
                    path.line_to(at(0.4, 0.8));
                    path.line_to(at(0.85, 0.15));
                }
                Shape::Cross => {
                    path.move_to(at(0.15, 0.15));
                    path.line_to(at(0.85, 0.85));
                    path.move_to(at(0.15, 0.85));
                    path.line_to(at(0.85, 0.15));
                }
                Shape::Line => {
                    if w >= h {
                        path.move_to(at(0., 0.5));
                        path.line_to(at(1., 0.5));
                    } else {
                        path.move_to(at(0.5, 0.));
                        path.line_to(at(0.5, 1.));
                    }
                }
                Shape::Oval => {
                    let (cx, cy) = (x + w / 2., y + h / 2.);
                    let (rx, ry) = (((w - width) / 2.).max(0.5), ((h - width) / 2.).max(0.5));
                    let (kx, ky) = (rx * 0.5523, ry * 0.5523);
                    let pt = |px_: f32, py_: f32| point(px(px_), px(py_));
                    path.move_to(pt(cx + rx, cy));
                    path.cubic_bezier_to(pt(cx, cy + ry), pt(cx + rx, cy + ky), pt(cx + kx, cy + ry));
                    path.cubic_bezier_to(pt(cx - rx, cy), pt(cx - kx, cy + ry), pt(cx - rx, cy + ky));
                    path.cubic_bezier_to(pt(cx, cy - ry), pt(cx - rx, cy - ky), pt(cx - kx, cy - ry));
                    path.cubic_bezier_to(pt(cx + rx, cy), pt(cx + kx, cy - ry), pt(cx + rx, cy - ky));
                    path.close();
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .size_full()
}

/// A square button in the bar above the page.
pub(crate) fn bar_button(id: impl Into<ElementId>, icon: &'static str, label: &'static str, on: bool, enabled: bool, p: &Palette) -> Stateful<Div> {
    let p = *p;
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .size(px(26.))
        .rounded(px(R_SM))
        .when(on, |s| s.bg(p.cyan_wash))
        .when(enabled, |s| s.cursor_pointer().tab_index(0).focus_visible(move |s| ui::ring(s, &p)))
        .when(enabled && !on, |s| s.hover(move |s| s.bg(p.plate)).active(move |s| s.bg(p.plate_2)))
        .child(ui::icon(icon, 15., if on { p.cyan } else if enabled { p.toner_2 } else { p.toner_4 }));
    ui::tip(button, label)
}

impl Shell {
    // ------------------------------------------------------------------ state

    pub fn load_fields(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let task = cx.background_spawn({
            let path = path.clone();
            async move { edit::list_fields(&path, None) }
        });
        cx.spawn(async move |this, cx| {
            let fields = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.canvas.loaded_for.as_ref() == Some(&path) {
                    this.canvas.edit.values = initial_values(&fields);
                    this.canvas.edit.fields = fields;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Remember the page as it is now, so Undo can come back to it.
    fn snapshot(&mut self) {
        let e = &mut self.canvas.edit;
        e.history.push((e.items.clone(), e.values.clone()));
        if e.history.len() > 200 {
            e.history.remove(0);
        }
    }

    pub fn edit_undo(&mut self, cx: &mut Context<Self>) {
        if self.state.mode != Some("edit") || self.state.running {
            return;
        }
        self.finish_edit();
        let e = &mut self.canvas.edit;
        if let Some((items, values)) = e.history.pop() {
            e.items = items;
            e.values = values;
            e.selected = None;
            e.moving = None;
        }
        cx.notify();
    }

    /// Remove the selected item. Returns false when there was nothing to remove.
    pub fn edit_delete_selected(&mut self, cx: &mut Context<Self>) -> bool {
        if self.state.mode != Some("edit") || self.state.running || self.canvas.edit.editing.is_some() {
            return false;
        }
        let Some(ix) = self.canvas.edit.selected.filter(|ix| *ix < self.canvas.edit.items.len()) else { return false };
        self.snapshot();
        self.canvas.edit.items.remove(ix);
        self.canvas.edit.selected = None;
        cx.notify();
        true
    }

    /// Escape: stop typing, then close an open list, then let go of the selection.
    /// Returns false when there was nothing to step back from.
    pub fn edit_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if self.state.mode != Some("edit") {
            return false;
        }
        if self.canvas.edit.editing.is_some() {
            self.commit_edit(cx);
            return true;
        }
        let e = &mut self.canvas.edit;
        if e.choice_open.take().is_some() || e.selected.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// Stop typing on the page and keep what was typed.
    pub fn finish_edit(&mut self) {
        let Some(target) = self.canvas.edit.editing.take() else { return };
        self.inputs.remove(INLINE);
        let e = &mut self.canvas.edit;
        if let Editing::Item(ix) = target {
            if e.items.get(ix).is_some_and(|i| i.text.trim().is_empty()) {
                e.items.remove(ix);
                e.selected = None;
            }
        }
        // Typing that changed nothing leaves no step to undo.
        if e.history.last().is_some_and(|(items, values)| *items == e.items && *values == e.values) {
            e.history.pop();
        }
    }

    pub fn commit_edit(&mut self, cx: &mut Context<Self>) {
        if self.canvas.edit.editing.is_some() {
            self.finish_edit();
            cx.notify();
        }
    }

    /// Put a text field on the page for `target` and give it the keyboard.
    fn begin_edit(&mut self, target: Editing, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_edit();
        let e = &self.canvas.edit;
        let initial = match &target {
            Editing::Item(ix) => match e.items.get(*ix) {
                Some(item) => item.text.clone(),
                None => return,
            },
            Editing::Field(name) => e.values.get(name).cloned().unwrap_or_default(),
        };
        let state = cx.new(|cx| InputState::new(window, cx));
        state.update(cx, |state, cx| {
            state.set_value(initial, window, cx);
            state.focus(window, cx);
        });
        let subscription = cx.subscribe_in(&state, window, |this: &mut Self, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let text = state.read(cx).value().to_string();
                this.edit_typed(&text, cx);
            }
            InputEvent::PressEnter { .. } => {
                this.commit_edit(cx);
                window.focus(&this.focus, cx);
            }
            InputEvent::Blur => this.commit_edit(cx),
            _ => {}
        });
        self.inputs.insert(INLINE.to_string(), InputSlot::new(state, subscription));
        self.canvas.edit.editing = Some(target);
        cx.notify();
    }

    fn edit_typed(&mut self, text: &str, cx: &mut Context<Self>) {
        let e = &mut self.canvas.edit;
        match e.editing.clone() {
            Some(Editing::Item(ix)) => {
                if let Some(item) = e.items.get_mut(ix) {
                    item.text = text.to_string();
                    item.rect = text_rect(item.rect.x0, item.rect.y0, text, item.size);
                }
            }
            Some(Editing::Field(name)) => {
                e.values.insert(name, text.to_string());
            }
            None => {}
        }
        cx.notify();
    }

    pub fn set_edit_tool(&mut self, tool: EditTool, cx: &mut Context<Self>) {
        self.finish_edit();
        let e = &mut self.canvas.edit;
        e.tool = tool;
        e.selected = None;
        e.choice_open = None;
        e.pending = None;
        e.moving = None;
        self.canvas.drag = None;
        cx.notify();
    }

    /// Make the selected item, or the next one placed, larger or smaller.
    fn edit_resize(&mut self, step: f32, cx: &mut Context<Self>) {
        self.prefs_changed(cx);
        let e = &self.canvas.edit;
        let target = e.selected.filter(|ix| e.items.get(*ix).is_some_and(|i| i.tool.is_text() || i.tool.is_mark()));
        match target {
            Some(ix) => {
                self.snapshot();
                let e = &mut self.canvas.edit;
                let item = &mut e.items[ix];
                if item.tool.is_text() {
                    item.size = (item.size + step).clamp(6., 72.);
                    item.rect = text_rect(item.rect.x0, item.rect.y0, &item.text, item.size);
                    e.text_size = item.size;
                } else {
                    let side = (item.rect.width() + step * 2.).clamp(4., 72.);
                    let (cx_, cy_) = ((item.rect.x0 + item.rect.x1) / 2., (item.rect.y0 + item.rect.y1) / 2.);
                    item.rect = Rect4::new(cx_ - side / 2., cy_ - side / 2., cx_ + side / 2., cy_ + side / 2.);
                    e.mark_size = if item.tool == EditTool::Dot { side * 2. } else { side };
                }
            }
            None => {
                let e = &mut self.canvas.edit;
                if e.tool.is_mark() {
                    e.mark_size = (e.mark_size + step * 2.).clamp(6., 72.);
                } else {
                    e.text_size = (e.text_size + step).clamp(6., 72.);
                }
            }
        }
        cx.notify();
    }

    /// The size the stepper shows: of the selected item, or of the next one placed.
    fn edit_size_shown(&self) -> f32 {
        let e = &self.canvas.edit;
        match e.selected.and_then(|ix| e.items.get(ix)) {
            Some(item) if item.tool.is_text() => item.size,
            Some(item) if item.tool.is_mark() => item.rect.width(),
            _ if e.tool.is_mark() => {
                if e.tool == EditTool::Dot {
                    e.mark_size * 0.5
                } else {
                    e.mark_size
                }
            }
            _ => e.text_size,
        }
    }

    fn set_ink(&mut self, color: [u8; 3], cx: &mut Context<Self>) {
        let e = &self.canvas.edit;
        let target = e.selected.filter(|ix| e.items.get(*ix).is_some_and(|i| !matches!(i.tool, EditTool::Highlight | EditTool::Whiteout) && i.color != color));
        if let Some(ix) = target {
            self.snapshot();
            self.canvas.edit.items[ix].color = color;
        }
        self.canvas.edit.ink = color;
        self.prefs_changed(cx);
        cx.notify();
    }

    /// What Edit & Fill hands to the run. `Err` explains what is missing.
    pub fn edit_options(&mut self, options: &mut Map<String, Value>) -> Result<(), String> {
        self.finish_edit();
        let e = &self.canvas.edit;
        let mut changed: Map<String, Value> = Map::new();
        let mut places: Vec<Value> = Vec::new();
        for f in &e.fields {
            let shared = f.widgets.first().is_none_or(|w| place_key(f, w) == f.name);
            if shared {
                if let Some(v) = e.values.get(&f.name).filter(|v| **v != f.value) {
                    changed.insert(f.name.clone(), Value::String(v.clone()));
                }
            } else {
                // Filled place by place: each changed place becomes a field of its own.
                for w in &f.widgets {
                    if let Some(v) = e.values.get(&place_key(f, w)).filter(|v| **v != f.value) {
                        places.push(json!({ "field": f.name, "widget": w.index, "value": v }));
                    }
                }
            }
        }
        let items: Vec<Value> = e
            .items
            .iter()
            .map(|i| {
                let mut item = json!({ "kind": i.tool.kind(), "page": i.page + 1, "x": i.rect.x0, "y": i.rect.y0 });
                if i.tool.is_text() {
                    item["text"] = json!(i.text);
                    item["size"] = json!(i.size);
                } else {
                    item["w"] = json!(i.rect.width());
                    item["h"] = json!(i.rect.height());
                }
                if !matches!(i.tool, EditTool::Highlight | EditTool::Whiteout) {
                    item["color"] = json!(i.color);
                }
                item
            })
            .collect();
        if changed.is_empty() && places.is_empty() && items.is_empty() {
            return Err("Nothing to save yet. Click the page to type or place a mark, or click a form field to fill it in.".into());
        }
        options.insert("fields".into(), Value::Object(changed));
        options.insert("places".into(), Value::Array(places));
        options.insert("items".into(), Value::Array(items));
        Ok(())
    }

    // ------------------------------------------------------------------ pointer

    /// The form field under a page point: its index and which of its places was hit.
    fn hit_widget(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let page = self.canvas.page + 1;
        self.canvas.edit.fields.iter().enumerate().filter(|(_, f)| fillable(f)).find_map(|(fi, f)| f.widgets.iter().position(|w| w.page == page && inside(&w.rect, x, y)).map(|wi| (fi, wi)))
    }

    /// The topmost placed item under a page point. `loose` allows a little room around
    /// each item, for the Select tool; the placing tools only pick up an item when the click
    /// is right on it, so marks can go close to each other.
    fn hit_item(&self, x: f32, y: f32, loose: bool) -> Option<usize> {
        let page = self.canvas.page;
        let room = if loose { 3. } else { 0. };
        self.canvas.edit.items.iter().enumerate().rev().find(|(_, i)| i.page == page && inside(&i.rect.grow(room), x, y)).map(|(ix, _)| ix)
    }

    fn edit_down(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        // A press anywhere on the page first finishes what was being typed.
        self.finish_edit();
        let e = &mut self.canvas.edit;
        e.choice_open = None;
        e.pending = None;
        e.moving = None;
        cx.notify();
        let Some((x, y)) = self.to_page_point(at) else { return };
        let tool = self.canvas.edit.tool;
        if !tool.is_box() {
            let fills = |kind: FieldType| matches!(tool, EditTool::Select | EditTool::Text) || (tool == EditTool::Check && matches!(kind, FieldType::Checkbox | FieldType::Radio));
            if let Some((fi, wi)) = self.hit_widget(x, y).filter(|(fi, _)| fills(self.canvas.edit.fields[*fi].kind)) {
                let field = self.canvas.edit.fields[fi].clone();
                let current = self.canvas.edit.values.get(&field.name).cloned().unwrap_or_default();
                self.canvas.edit.selected = None;
                match field.kind {
                    FieldType::Checkbox => {
                        self.snapshot();
                        self.canvas.edit.values.insert(field.name, if current == "true" { "false".into() } else { "true".into() });
                    }
                    FieldType::Radio => {
                        // Clicking the chosen button again clears the group.
                        let state = field.widgets[wi].state.clone();
                        self.snapshot();
                        self.canvas.edit.values.insert(field.name, if current == state { String::new() } else { state });
                    }
                    FieldType::Choice if !field.options.is_empty() => self.canvas.edit.choice_open = Some(place_key(&field, &field.widgets[wi])),
                    _ => self.canvas.edit.pending = Some(Pending::Field(place_key(&field, &field.widgets[wi]))),
                }
                return;
            }
            if let Some(ix) = self.hit_item(x, y, tool == EditTool::Select) {
                let origin = self.canvas.edit.items[ix].rect;
                self.canvas.edit.selected = Some(ix);
                self.canvas.edit.moving = Some(Moving { index: ix, from: (x, y), origin, moved: false });
                return;
            }
        }
        self.canvas.edit.selected = None;
        match tool {
            EditTool::Select => {}
            t if t.is_box() => self.canvas.drag = Some((at, at)),
            _ => self.canvas.edit.pending = Some(Pending::Place(x, y)),
        }
    }

    fn edit_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            // The button came up somewhere off the page.
            if self.canvas.drag.take().is_some() | self.canvas.edit.moving.take().is_some() {
                cx.notify();
            }
            return;
        }
        if let Some((start, _)) = self.canvas.drag {
            self.canvas.drag = Some((start, event.position));
            cx.notify();
            return;
        }
        let Some(moving) = self.canvas.edit.moving.clone() else { return };
        let (Some((x, y)), Some((pw, ph))) = (self.to_page_point(event.position), self.canvas.page_size) else { return };
        let (dx, dy) = (x - moving.from.0, y - moving.from.1);
        if !moving.moved {
            if dx.abs().max(dy.abs()) < 1.5 {
                return;
            }
            self.snapshot();
            if let Some(m) = &mut self.canvas.edit.moving {
                m.moved = true;
            }
        }
        let (w, h) = (moving.origin.width(), moving.origin.height());
        let nx = (moving.origin.x0 + dx).clamp(0., (pw - w).max(0.));
        let ny = (moving.origin.y0 + dy).clamp(0., (ph - h).max(0.));
        if let Some(item) = self.canvas.edit.items.get_mut(moving.index) {
            item.rect = Rect4::new(nx, ny, nx + w, ny + h);
        }
        cx.notify();
    }

    fn edit_up(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(moving) = self.canvas.edit.moving.take() {
            // A click on typed text without dragging it opens it for typing again.
            if !moving.moved && self.canvas.edit.items.get(moving.index).is_some_and(|i| i.tool.is_text()) {
                self.snapshot();
                self.begin_edit(Editing::Item(moving.index), window, cx);
            }
            return;
        }
        if let Some((start, _)) = self.canvas.drag.take() {
            if let (Some((ax, ay)), Some((bx, by))) = (self.to_page_point(start), self.to_page_point(at)) {
                let rect = Rect4::new(ax, ay, bx, by);
                let tool = self.canvas.edit.tool;
                let enough = if tool == EditTool::Line { rect.width().max(rect.height()) >= 4. } else { rect.width() >= 3. && rect.height() >= 3. };
                if enough {
                    self.snapshot();
                    let e = &mut self.canvas.edit;
                    // Not selected: its outline and remove handle would sit where the next
                    // mark often goes. A click on it selects it.
                    e.items.push(Placed { tool, page: self.canvas.page, rect, text: String::new(), size: 0., color: e.ink });
                }
            }
            return;
        }
        match self.canvas.edit.pending.take() {
            Some(Pending::Field(name)) => {
                self.snapshot();
                self.begin_edit(Editing::Field(name), window, cx);
            }
            Some(Pending::Place(x, y)) => self.edit_place(x, y, window, cx),
            None => {}
        }
    }

    /// Put the current tool's item on the page at a click.
    fn edit_place(&mut self, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.canvas.page;
        let e = &self.canvas.edit;
        let (tool, color, size) = (e.tool, e.ink, e.text_size);
        let placed = match tool {
            // The click lands on the middle of the capitals.
            EditTool::Text => Placed { tool, page, rect: text_rect(x, y - size * 0.36, "", size), text: String::new(), size, color },
            EditTool::Date => {
                let text = edit::today();
                Placed { tool, page, rect: text_rect(x, y - size * 0.36, &text, size), text, size, color }
            }
            EditTool::Check | EditTool::Cross | EditTool::Dot => {
                let half = if tool == EditTool::Dot { e.mark_size * 0.25 } else { e.mark_size * 0.5 };
                Placed { tool, page, rect: Rect4::new(x - half, y - half, x + half, y + half), text: String::new(), size: 0., color }
            }
            _ => return,
        };
        self.snapshot();
        let e = &mut self.canvas.edit;
        e.items.push(placed);
        let ix = e.items.len() - 1;
        // Only text being typed counts as selected, so the size and ink buttons apply to it.
        // A placed mark stays unselected: its remove handle would sit where the next mark goes.
        if tool == EditTool::Text {
            e.selected = Some(ix);
            self.begin_edit(Editing::Item(ix), window, cx);
        }
    }

    // ------------------------------------------------------------------ the bar above the page

    pub fn edit_toolbar(&mut self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let p = *p;
        let enabled = !self.state.running;
        let e = &self.canvas.edit;
        let (current, current_ink, can_undo) = (e.tool, e.ink, !e.history.is_empty());
        let size = self.edit_size_shown();
        let rule = || div().mx(px(3.)).child(ui::vrule(&p, 16.));

        let mut bar = ui::row().gap(px(2.)).flex_shrink_0();
        for (i, tool) in EditTool::ALL.into_iter().enumerate() {
            let mut button = bar_button(("edit-tool", i), tool.icon(), tool.label(), tool == current, enabled, &p);
            if enabled {
                button = button.on_click(cx.listener(move |this, _, _, cx| this.set_edit_tool(tool, cx)));
            }
            bar = bar.child(button);
            if matches!(tool, EditTool::Select | EditTool::Date | EditTool::Dot) {
                bar = bar.child(rule());
            }
        }

        let mut smaller = bar_button("edit-smaller", "minus", "Smaller", false, enabled, &p);
        let mut larger = bar_button("edit-larger", "plus", "Larger", false, enabled, &p);
        if enabled {
            smaller = smaller.on_click(cx.listener(|this, _, _, cx| this.edit_resize(-1., cx)));
            larger = larger.on_click(cx.listener(|this, _, _, cx| this.edit_resize(1., cx)));
        }
        bar = bar.child(rule()).child(smaller).child(ui::mono(format!("{size:.0} pt")).w(px(36.)).flex_shrink_0().text_center().text_color(p.toner_2)).child(larger).child(rule());

        for (i, (label, color)) in INKS.into_iter().enumerate() {
            let on = color == current_ink;
            let mut swatch = div()
                .id(("edit-ink", i))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .size(px(20.))
                .rounded_full()
                .border_1()
                .border_color(if on { p.cyan } else { transparent_black() })
                .child(div().size(px(12.)).rounded_full().bg(ink(color)).border_1().border_color(p.rule_strong));
            if enabled {
                swatch = swatch.cursor_pointer().tab_index(0).focus_visible(move |s| ui::ring(s, &p)).on_click(cx.listener(move |this, _, _, cx| this.set_ink(color, cx)));
            }
            bar = bar.child(ui::tip(swatch, label));
        }

        let mut undo = bar_button("edit-undo", "undo", "Undo", false, enabled && can_undo, &p);
        if enabled && can_undo {
            undo = undo.on_click(cx.listener(|this, _, _, cx| this.edit_undo(cx)));
        }
        bar.child(rule()).child(undo)
    }

    // ------------------------------------------------------------------ the page

    /// How one placed item looks on the page. `scale` is pixels per point.
    fn item_shape(item: &Placed, scale: f32) -> Div {
        let color = ink(item.color);
        let el = Shell::mark_box(&item.rect, scale);
        let (w, h) = (item.rect.width(), item.rect.height());
        match item.tool {
            EditTool::Check => el.child(strokes(Shape::Check, color, (w.min(h) * 0.1).max(0.8) * scale)),
            EditTool::Cross => el.child(strokes(Shape::Cross, color, (w.min(h) * 0.1).max(0.8) * scale)),
            EditTool::Dot => el.rounded_full().bg(color),
            EditTool::Circle => el.child(strokes(Shape::Oval, color, edit::STROKE.min(w.min(h) / 2.) * scale)),
            EditTool::Line => el.child(strokes(Shape::Line, color, edit::STROKE * scale)),
            EditTool::Highlight => el.bg(hsla(0.15, 0.95, 0.55, 0.42)),
            EditTool::Whiteout => el.bg(white()).border_1().border_color(hsla(0., 0., 0., 0.14)),
            EditTool::Select | EditTool::Text | EditTool::Date => {
                let (top, height) = line_box(item.rect.y0, item.size);
                div()
                    .absolute()
                    .left(px(item.rect.x0 * scale))
                    .top(px(top * scale))
                    .h(px(height * scale))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .font_family(FONT_PAGE)
                    .text_color(color)
                    .text_size(px(item.size * scale))
                    .line_height(px(item.size * scale * 1.2))
                    .child(SharedString::from(item.text.clone()))
            }
        }
    }

    /// Form fields, placed items, the text field being typed into, and the pointer handling.
    pub fn edit_overlays(&mut self, mut sheet: Stateful<Div>, w: f32, h: f32, p: &Palette, _window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *p;
        let page = self.canvas.page;
        let Some((pw, _)) = self.canvas.page_size else { return sheet };
        let scale = w / pw;
        let locked = self.state.running;
        let input = self.inputs.get(INLINE).map(|slot| slot.state.clone());
        let e = &self.canvas.edit;
        let editing = e.editing.clone();
        let page_ink = ink(INKS[0].1);

        // Form fields, filled where they sit.
        let mut open_list: Option<(String, Rect4, Vec<String>, String)> = None;
        for field in e.fields.iter().filter(|f| fillable(f)) {
            let value = e.values.get(&field.name).cloned().unwrap_or_default();
            let changed = value != field.value;
            for widget in field.widgets.iter().filter(|w| w.page == page + 1) {
                let r = widget.rect;
                let side = r.width().min(r.height());
                let mut el = Shell::mark_box(&r, scale);
                match field.kind {
                    FieldType::Checkbox => {
                        el = if value == "true" {
                            el.bg(field_tint()).child(strokes(Shape::Check, page_ink, (side * 0.12).max(0.8) * scale))
                        } else if changed {
                            el.bg(white()).border_1().border_color(hsla(0., 0., 0., 0.55))
                        } else {
                            el.bg(field_tint())
                        };
                    }
                    FieldType::Radio => {
                        el = el.rounded_full();
                        el = if !value.is_empty() && value == widget.state {
                            el.bg(field_tint()).flex().items_center().justify_center().child(ui::dot(page_ink, side * scale * 0.5))
                        } else if changed {
                            el.bg(white()).border_1().border_color(hsla(0., 0., 0., 0.55))
                        } else {
                            el.bg(field_tint())
                        };
                    }
                    _ => {
                        // This place's own value: a date typed in one place stays there.
                        let key = place_key(field, widget);
                        let value = e.values.get(&key).cloned().unwrap_or_default();
                        let changed = value != field.value;
                        let font = field_font(r.height()) * scale;
                        let typing = editing.as_ref() == Some(&Editing::Field(key.clone()));
                        el = match (&input, typing) {
                            (Some(state), true) => el.bg(white()).border_1().border_color(p.stock_select).occlude().overflow_hidden().flex().items_center().child(
                                Input::new(state).appearance(false).px(px(2. * scale)).py(px(0.)).text_size(px(font)).line_height(px(font * 1.2)).font_family(FONT_PAGE).text_color(page_ink),
                            ),
                            _ if !value.is_empty() || changed => {
                                el.bg(white()).flex().items_center().px(px(2. * scale)).overflow_hidden().whitespace_nowrap().font_family(FONT_PAGE).text_color(page_ink).text_size(px(font)).line_height(px(font * 1.2)).child(SharedString::from(value.clone()))
                            }
                            _ => el.bg(field_tint()),
                        };
                        if e.choice_open.as_deref() == Some(key.as_str()) {
                            open_list = Some((key.clone(), r, field.options.clone(), value.clone()));
                        }
                    }
                }
                sheet = sheet.child(el);
            }
        }

        // What has been placed.
        for (ix, item) in e.items.iter().enumerate().filter(|(_, i)| i.page == page) {
            let typing = editing == Some(Editing::Item(ix));
            match (&input, typing) {
                (Some(state), true) => {
                    let (top, height) = line_box(item.rect.y0, item.size);
                    let room = (item.rect.width() + item.size * 6.).min((pw - item.rect.x0).max(item.size * 2.));
                    sheet = sheet.child(
                        div()
                            .id("edit-inline")
                            .absolute()
                            .left(px(item.rect.x0 * scale - 4.))
                            .top(px(top * scale - 1.))
                            .w(px(room * scale + 8.))
                            .h(px(height * scale + 2.))
                            .pl(px(3.))
                            .rounded(px(2.))
                            .border_1()
                            .border_color(p.stock_select)
                            .occlude()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .child(Input::new(state).appearance(false).px(px(0.)).py(px(0.)).text_size(px(item.size * scale)).line_height(px(item.size * scale * 1.2)).font_family(FONT_PAGE).text_color(ink(item.color))),
                    );
                }
                _ => sheet = sheet.child(Shell::item_shape(item, scale)),
            }
            if e.selected == Some(ix) && !typing && !locked {
                let outline = if item.tool.is_text() {
                    let (top, height) = line_box(item.rect.y0, item.size);
                    Rect4::new(item.rect.x0, top, item.rect.x1, top + height)
                } else {
                    item.rect
                };
                sheet = sheet.child(
                    div()
                        .absolute()
                        .left(px(outline.x0 * scale - 3.))
                        .top(px(outline.y0 * scale - 3.))
                        .w(px(outline.width() * scale + 6.))
                        .h(px(outline.height() * scale + 6.))
                        .rounded(px(2.))
                        .border_1()
                        .border_color(p.stock_select)
                        .child(
                            div()
                                .id(("edit-remove", ix))
                                .absolute()
                                .top(px(-9.))
                                .right(px(-9.))
                                .size(px(16.))
                                .rounded_full()
                                .bg(p.sleeve)
                                .border_1()
                                .border_color(p.rule_strong)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .occlude()
                                .child(ui::icon("x", 9., p.toner))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.edit_delete_selected(cx);
                                })),
                        ),
                );
            }
        }

        // The box being dragged out, shown as what it will become.
        if let Some((a, b)) = self.canvas.drag {
            if let (Some((ax, ay)), Some((bx, by))) = (self.to_page_point(a), self.to_page_point(b)) {
                let ghost = Placed { tool: e.tool, page, rect: Rect4::new(ax, ay, bx, by), text: String::new(), size: 0., color: e.ink };
                sheet = sheet.child(Shell::item_shape(&ghost, scale));
            }
        }

        // The list of a choice field.
        if let Some((name, r, options, value)) = open_list {
            let rows = options.len() as f32 * 29. + 10.;
            let mut list = ui::col().absolute().left(px(r.x0 * scale)).w(px((r.width() * scale).max(150.))).p(px(4.)).gap(px(1.)).rounded(px(R_MD)).border_1().border_color(p.rule_strong).bg(p.sleeve).text_color(p.toner).occlude();
            // Open upwards when there is no room below.
            list = if r.y1 * scale + rows > h { list.bottom(px(h - r.y0 * scale + 2.)) } else { list.top(px(r.y1 * scale + 2.)) };
            for (i, option) in options.into_iter().enumerate() {
                let on = option == value;
                let name = name.clone();
                let pick = option.clone();
                list = list.child(
                    div()
                        .id(("edit-choice", i))
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
                        .child(SharedString::from(option))
                        .when(on, |s| s.child(ui::icon("check", 12., p.cyan)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.snapshot();
                            this.canvas.edit.values.insert(name.clone(), pick.clone());
                            this.canvas.edit.choice_open = None;
                            cx.notify();
                        })),
                );
            }
            sheet = sheet.child(list);
        }

        if locked {
            return sheet;
        }
        let cursor = match e.tool {
            EditTool::Select => CursorStyle::Arrow,
            EditTool::Text => CursorStyle::IBeam,
            _ => CursorStyle::Crosshair,
        };
        sheet
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| this.edit_down(event.position, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.edit_move(event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, window, cx| this.edit_up(event.position, window, cx)))
    }

    // ------------------------------------------------------------------ the options panel

    /// A short account of what is on the page. The editing itself happens on the page.
    pub fn edit_panel(&mut self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let p = *p;
        let e = &self.canvas.edit;
        let section = |title: &'static str| ui::col().px(px(16.)).pt(px(8.)).pb(px(12.)).border_t_1().border_color(p.rule_soft).child(ui::row().h(px(28.)).child(ui::section_label(title, &p)));
        let line = |label: &'static str, value: String| ui::row().min_h(px(28.)).justify_between().gap(px(12.)).child(div().text_color(p.toner_2).child(label)).child(ui::mono(value));

        let fields: Vec<&FieldInfo> = e.fields.iter().filter(|f| fillable(f)).collect();
        let filled = fields.iter().filter(|f| is_filled(f, &e.values)).count();
        let changed = e.values != initial_values(&e.fields);
        let n = e.items.len();

        let mut page = section("On the page").child(div().pb(px(6.)).text_size(px(TEXT_SM)).line_height(px(18.)).text_color(p.toner_3).child(if fields.is_empty() {
            "Pick a tool above the page, then click where it should go. Text is typed right on the page."
        } else {
            "With the Text tool, click a field on the page to fill it in. Tick, cross, dot and date go exactly where you click, fields included."
        }));
        page = page.child(line("Added", format!("{n} {}", if n == 1 { "item" } else { "items" })));
        if !fields.is_empty() {
            page = page.child(line("Form fields", format!("{filled} of {} filled", fields.len()))).child(div().pt(px(4.)).pb(px(4.)).child(ui::progress(filled as f32 / fields.len() as f32, p.cyan, &p)));
        }
        if (n > 0 || changed) && !self.state.running {
            page = page.child(ui::row().pt(px(6.)).child(ui::button("edit-clear", ui::Btn::Ghost, 24., true, &p).ml(px(-8.)).child("Start over").on_click(cx.listener(|this, _, _, cx| {
                this.finish_edit();
                this.snapshot();
                let e = &mut this.canvas.edit;
                e.items.clear();
                e.values = initial_values(&e.fields);
                e.selected = None;
                cx.notify();
            }))));
        }

        let key = |label: &'static str, keys: &[&str]| ui::row().min_h(px(26.)).justify_between().text_color(p.toner_2).child(label).child(ui::kbd(keys, &p, ui::KeyOn::Surface));
        let keys = section("Keys")
            .child(key("Finish typing", &["Enter"]))
            .child(key("Delete the selected item", &["Del"]))
            .child(key("Undo", &[crate::palette_model::mod_label(), "Z"]))
            .child(key("Zoom in or out", &[crate::palette_model::mod_label(), "+ / \u{2212}"]))
            .child(key("Let go of the selection", &["Esc"]));

        ui::col().child(page).child(keys).into_any_element()
    }
}
