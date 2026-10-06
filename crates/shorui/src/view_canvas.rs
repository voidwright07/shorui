//! The single-page canvas used by Redact, Edit & Fill and Sign.

use crate::catalog::{Tool, Workspace};
use crate::shell::Shell;
use crate::tokens::*;
use crate::ui::{self, pal};
use gpui_kit::component::scroll::{ScrollableElement, ScrollbarAxis};
use gpui_kit::*;
use shorui_core::text::Rect4;
use std::cell::Cell;
use std::rc::Rc;

/// A rectangle on a page, in display space (points, origin top-left).
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// 0-based page.
    pub page: usize,
    pub rect: Rect4,
    /// "area", "search" or "pattern".
    pub source: String,
    pub label: String,
    /// For pattern marks: "email", "phone" and so on.
    pub pattern: String,
}

/// What a click or drag on the page does in Edit & Fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditTool {
    /// Pick up what is already on the page.
    Select,
    #[default]
    Text,
    Check,
    Cross,
    Dot,
    Circle,
    Line,
    Date,
    Highlight,
    Whiteout,
}

impl EditTool {
    pub const ALL: [EditTool; 10] =
        [EditTool::Select, EditTool::Text, EditTool::Date, EditTool::Check, EditTool::Cross, EditTool::Dot, EditTool::Circle, EditTool::Line, EditTool::Highlight, EditTool::Whiteout];

    /// The item kind the core tool knows this as.
    pub fn kind(self) -> &'static str {
        match self {
            EditTool::Select | EditTool::Text => "text",
            EditTool::Check => "check",
            EditTool::Cross => "cross",
            EditTool::Dot => "dot",
            EditTool::Circle => "circle",
            EditTool::Line => "line",
            EditTool::Date => "date",
            EditTool::Highlight => "highlight",
            EditTool::Whiteout => "whiteout",
        }
    }

    /// Named on the button's tooltip.
    pub fn label(self) -> &'static str {
        match self {
            EditTool::Select => "Select and move",
            EditTool::Text => "Text: click the page and type",
            EditTool::Check => "Tick",
            EditTool::Cross => "Cross",
            EditTool::Dot => "Dot",
            EditTool::Circle => "Circle: drag around something",
            EditTool::Line => "Line: drag along or down",
            EditTool::Date => "Today's date",
            EditTool::Highlight => "Highlight: drag over text",
            EditTool::Whiteout => "White out: drag to cover",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            EditTool::Select => "cursor",
            EditTool::Text => "text",
            EditTool::Check => "check",
            EditTool::Cross => "x",
            EditTool::Dot => "dot",
            EditTool::Circle => "oval",
            EditTool::Line => "line",
            EditTool::Date => "calendar",
            EditTool::Highlight => "highlight",
            EditTool::Whiteout => "whiteout",
        }
    }

    /// Drawn by dragging a box rather than by a single click.
    pub fn is_box(self) -> bool {
        matches!(self, EditTool::Circle | EditTool::Line | EditTool::Highlight | EditTool::Whiteout)
    }

    /// Typed text.
    pub fn is_text(self) -> bool {
        matches!(self, EditTool::Text | EditTool::Date)
    }

    /// A small mark placed with one click.
    pub fn is_mark(self) -> bool {
        matches!(self, EditTool::Check | EditTool::Cross | EditTool::Dot)
    }
}

/// Something placed on a page in Edit & Fill.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub tool: EditTool,
    /// 0-based page.
    pub page: usize,
    /// For text: left edge and the top of the capitals, as wide as the text and as tall as
    /// its size. For everything else: the box it is drawn in.
    pub rect: Rect4,
    pub text: String,
    /// Font size in points (text only).
    pub size: f32,
    pub color: [u8; 3],
}

/// What the text field on the page is writing into.
#[derive(Debug, Clone, PartialEq)]
pub enum Editing {
    /// The placed item at this index.
    Item(usize),
    /// The form field with this name.
    Field(String),
}

/// An item being dragged to a new place.
#[derive(Debug, Clone, PartialEq)]
pub struct Moving {
    pub index: usize,
    /// Where the drag started, in page points.
    pub from: (f32, f32),
    pub origin: Rect4,
    pub moved: bool,
}

/// What a press on the page will do when the button comes up.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    /// Place the current tool's item at this page point.
    Place(f32, f32),
    /// Start typing into this form field.
    Field(String),
}

/// The ink colours offered in Edit & Fill.
pub const INKS: [(&str, [u8; 3]); 3] = [("Black ink", [17, 17, 17]), ("Blue ink", [24, 72, 200]), ("Red ink", [196, 36, 36])];

/// Everything Edit & Fill keeps for the file on the canvas.
pub struct EditState {
    pub tool: EditTool,
    pub items: Vec<Placed>,
    pub fields: Vec<shorui_core::tools::edit::FieldInfo>,
    pub values: std::collections::BTreeMap<String, String>,
    pub selected: Option<usize>,
    pub editing: Option<Editing>,
    pub moving: Option<Moving>,
    pub pending: Option<Pending>,
    /// The choice field whose list is open.
    pub choice_open: Option<String>,
    pub ink: [u8; 3],
    /// Font size of new text, in points.
    pub text_size: f32,
    /// Side of a new tick, cross or dot, in points.
    pub mark_size: f32,
    /// Earlier states, newest last, for Undo.
    pub history: Vec<(Vec<Placed>, std::collections::BTreeMap<String, String>)>,
}

impl Default for EditState {
    fn default() -> Self {
        EditState {
            tool: EditTool::default(),
            items: Vec::new(),
            fields: Vec::new(),
            values: Default::default(),
            selected: None,
            editing: None,
            moving: None,
            pending: None,
            choice_open: None,
            ink: INKS[0].1,
            text_size: 11.,
            mark_size: 12.,
            history: Vec::new(),
        }
    }
}

/// How large the page is drawn: 1 shows the whole page in the stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom(pub f32);

impl Default for Zoom {
    fn default() -> Self {
        Zoom(1.)
    }
}

/// The zoom steps, as multiples of "whole page".
pub const ZOOMS: [f32; 10] = [0.5, 0.75, 1., 1.25, 1.5, 2., 2.5, 3., 4., 5.];

#[derive(Default)]
pub struct CanvasState {
    pub zoom: Zoom,
    /// Wheel movement with Ctrl or Cmd held, not yet turned into a zoom step.
    pub wheel: f32,
    /// 0-based page shown.
    pub page: usize,
    /// Where the page image sits on screen, recorded during paint.
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Rectangle being dragged out, in window coordinates.
    pub drag: Option<(Point<Pixels>, Point<Pixels>)>,
    /// Areas drawn by hand (Redact).
    pub areas: Vec<Mark>,
    /// Marks found from the search text and every pattern (Redact). Pattern marks only
    /// count once their pattern is switched on.
    pub found: Vec<Mark>,
    pub search: String,
    pub match_case: bool,
    pub whole_words: bool,
    /// Bumped on every search so a slow, stale result is ignored.
    pub generation: u64,
    /// The file the marks and form fields were read from.
    pub loaded_for: Option<std::path::PathBuf>,
    pub edit: EditState,
    /// Size of the signature box in points, when known.
    pub sign_size: Option<(f32, f32)>,
    /// Where the signature goes: page and top-left in display space.
    pub placed: Option<(usize, f32, f32)>,
    /// Size in points of the page being shown, when known.
    pub page_size: Option<(f32, f32)>,
}

impl CanvasState {
    pub fn reset(&mut self) {
        let bounds = self.bounds.clone();
        // The chosen tool, ink and sizes carry over to the next file.
        let keep = EditState { tool: self.edit.tool, ink: self.edit.ink, text_size: self.edit.text_size, mark_size: self.edit.mark_size, ..Default::default() };
        *self = CanvasState { bounds, edit: keep, ..Default::default() };
    }
}

impl Shell {
    /// Window point to display-space point on the page being shown.
    pub fn to_page_point(&self, at: Point<Pixels>) -> Option<(f32, f32)> {
        let b = self.canvas.bounds.get();
        let (pw, ph) = self.canvas.page_size?;
        if b.size.width <= px(0.) {
            return None;
        }
        let x = ((at.x - b.origin.x) / b.size.width).clamp(0., 1.) * pw;
        let y = ((at.y - b.origin.y) / b.size.height).clamp(0., 1.) * ph;
        Some((x, y))
    }

    pub fn canvas_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.finish_edit();
        let pages = self.state.active().and_then(|f| f.pages).unwrap_or(1).max(1);
        self.canvas.page = (self.canvas.page as isize + delta).clamp(0, pages as isize - 1) as usize;
        self.canvas.drag = None;
        self.canvas.edit.selected = None;
        self.canvas.edit.choice_open = None;
        // A new page starts at its top.
        let offset = self.canvas_scroll.offset();
        self.canvas_scroll.set_offset(point(offset.x, px(0.)));
        cx.notify();
    }

    /// One zoom step in (`1`) or out (`-1`).
    pub fn canvas_zoom_step(&mut self, direction: i32, cx: &mut Context<Self>) {
        let z = self.canvas.zoom.0;
        let next = if direction > 0 { ZOOMS.iter().copied().find(|s| *s > z + 1e-3) } else { ZOOMS.iter().rev().copied().find(|s| *s < z - 1e-3) };
        if let Some(next) = next {
            self.set_canvas_zoom(next, cx);
        }
    }

    pub fn set_canvas_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom_about(zoom, None, cx);
    }

    /// A two-finger pinch on a touchpad: smooth zoom around the fingers.
    fn canvas_pinch(&mut self, event: &PinchEvent, cx: &mut Context<Self>) {
        let zoom = (self.canvas.zoom.0 * (1. + event.delta)).clamp(ZOOMS[0], ZOOMS[ZOOMS.len() - 1]);
        self.zoom_about(zoom, Some(event.position), cx);
    }

    /// Zoom so that what is under `anchor` (a window point; the middle of the view when
    /// `None`) stays where it is.
    fn zoom_about(&mut self, zoom: f32, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        if !self.state.tool().is_some_and(|t| t.workspace == Workspace::Canvas) || self.state.active().is_none() {
            return;
        }
        let old = self.canvas.zoom.0;
        if (old - zoom).abs() < 1e-3 {
            return;
        }
        let view = self.canvas_scroll.bounds();
        let offset = self.canvas_scroll.offset();
        let at = anchor.map(|a| a - view.origin).unwrap_or(point(view.size.width / 2., view.size.height / 2.));
        let k = zoom / old;
        // Content under the anchor is `at - offset`; after zooming it is `k` times further in.
        let keep = |o: Pixels, a: Pixels| {
            let (o, a) = (f32::from(o), f32::from(a));
            px(-((a - o) * k - a).max(0.))
        };
        self.canvas_scroll.set_offset(point(keep(offset.x, at.x), keep(offset.y, at.y)));
        self.canvas.zoom = Zoom(zoom);
        self.canvas.drag = None;
        cx.notify();
    }

    /// Ctrl or Cmd with the wheel zooms; a mouse notch is one step, a trackpad adds up.
    fn canvas_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        self.canvas.wheel += f32::from(event.delta.pixel_delta(px(20.)).y);
        if self.canvas.wheel.abs() >= 40. {
            let direction = if self.canvas.wheel > 0. { 1 } else { -1 };
            self.canvas.wheel = 0.;
            self.canvas_zoom_step(direction, cx);
        }
    }

    /// The zoom control that floats over the bottom right of the stage.
    fn zoom_pill(&self, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let zoom = self.canvas.zoom.0;
        let (out_label, in_label, fit_label) = if cfg!(target_os = "macos") {
            ("Zoom out  Cmd \u{2212}", "Zoom in  Cmd +", "Whole page  Cmd 0")
        } else {
            ("Zoom out  Ctrl \u{2212}", "Zoom in  Ctrl +", "Whole page  Ctrl 0")
        };
        let mut out = crate::view_edit::bar_button("zoom-out", "minus", out_label, false, zoom > ZOOMS[0] + 1e-3, p);
        let mut grow = crate::view_edit::bar_button("zoom-in", "plus", in_label, false, zoom < ZOOMS[ZOOMS.len() - 1] - 1e-3, p);
        let mut fit = crate::view_edit::bar_button("zoom-fit", "fit", fit_label, false, (zoom - 1.).abs() > 1e-3, p);
        out = out.on_click(cx.listener(|this, _, _, cx| this.canvas_zoom_step(-1, cx)));
        grow = grow.on_click(cx.listener(|this, _, _, cx| this.canvas_zoom_step(1, cx)));
        fit = fit.on_click(cx.listener(|this, _, _, cx| this.set_canvas_zoom(1., cx)));
        div()
            .id("zoom")
            .absolute()
            .bottom(px(16.))
            .right(px(20.))
            .flex()
            .flex_row()
            .items_center()
            .h(px(34.))
            .px(px(3.))
            .gap(px(2.))
            .rounded(px(R_MD))
            .border_1()
            .border_color(p.rule_strong)
            .bg(p.sleeve)
            .occlude()
            .child(out)
            .child(ui::mono(format!("{:.0}%", zoom * 100.)).w(px(44.)).text_center().text_color(p.toner_2))
            .child(grow)
            .child(div().mx(px(2.)).child(ui::vrule(p, 16.)))
            .child(fit)
    }

    pub fn render_canvas(&mut self, tool: &'static Tool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        self.sync_canvas_file(tool, cx);
        let Some(file) = self.state.active().cloned() else {
            let title = match tool.id {
                "redact" => "Drop a PDF to redact it",
                "sign" => "Drop a PDF to sign it",
                _ => "Drop a PDF to write on it",
            };
            return self.single_file_empty(title, &p, cx).into_any_element();
        };
        if file.locked {
            return self.single_file_message("This file is password protected", "Unlock it first, then come back.", &p).into_any_element();
        }
        let pages = file.pages.unwrap_or(1).max(1);
        if self.canvas.page >= pages {
            self.canvas.page = pages - 1;
        }
        let page = self.canvas.page;

        // Zoom 1 fits the whole page in the stage; beyond that the stage scrolls.
        let viewport = window.viewport_size();
        let side = if self.settings.sidebar_collapsed || viewport.width < px(1200.) { RAIL_W } else { SIDEBAR_W };
        // The card takes the inset and its edge off the window.
        let stage_w = (f32::from(viewport.width) - side - INSET - 2. - PANEL_W - 40.).max(200.);
        let stage_h = (f32::from(viewport.height) - INSET * 2. - 2. - (TOPBAR_H * 2. + RUNLINE_H + 40.)).max(240.);
        let base = self.page_image(&file.path, page, 1400, &[page], cx);
        let (iw, ih) = base.as_ref().map(|i| { let s = i.size(0); (s.width.0 as f32, s.height.0 as f32) }).unwrap_or((595., 842.));
        let aspect = ih / iw;
        let mut w = stage_w;
        let mut h = w * aspect;
        if h > stage_h {
            h = stage_h;
            w = h / aspect;
        }
        let zoom = self.canvas.zoom.0;
        let (w, h) = (w * zoom, h * zoom);
        // Drawn larger than the usual rendering: render it sharper, keeping one such page.
        let longest = w.max(h) * window.scale_factor();
        let tier = if longest > 2400. { 3600 } else if longest > 1400. { 2400 } else { 1400 };
        let image = if tier == 1400 {
            base.clone()
        } else {
            for old in self.thumbs.evict_large(&(file.path.clone(), page, tier)) {
                cx.drop_image(old, Some(window));
            }
            self.page_image(&file.path, page, tier, &[page], cx).or(base.clone())
        };
        if self.canvas.page_size.is_none() || base.is_some() {
            // Thumbnails are rendered so the longer side is 1400 px; recover points from the file's page box lazily.
            self.ensure_page_size(&file.path, page, cx);
        }

        // Edit & Fill keeps its tools above the page; the others say what a click does.
        let lead = match tool.id {
            "edit" => self.edit_toolbar(&p, cx).into_any_element(),
            "redact" => div().text_color(p.toner_2).child("Drag over the page to mark an area").into_any_element(),
            _ => div().text_color(p.toner_2).child("Click the page to place the signature").into_any_element(),
        };
        let toolbar = ui::row()
            .h(px(TOPBAR_H))
            .pl(px(if tool.id == "edit" { 10. } else { 16. }))
            .pr(px(12.))
            .gap(px(4.))
            .flex_shrink_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(p.rule)
            .child(lead)
            .child(ui::grow())
            .child(ui::icon_button("pg-prev", "chev-left", 24., &p).on_click(cx.listener(|this, _, _, cx| this.canvas_step(-1, cx))))
            .child(ui::mono(format!("{} / {}", page + 1, pages)).px(px(6.)).flex_shrink_0().text_color(p.toner_2))
            .child(ui::icon_button("pg-next", "chev-right", 24., &p).on_click(cx.listener(|this, _, _, cx| this.canvas_step(1, cx))));

        let bounds_cell = self.canvas.bounds.clone();
        let mut sheet = div()
            .id("page-sheet")
            .relative()
            .w(px(w))
            .h(px(h))
            .flex_shrink_0()
            .rounded(px(2.))
            .bg(p.stock)
            .border_1()
            .border_color(p.stock_edge)
            .overflow_hidden()
            .child(canvas(move |b, _, _| bounds_cell.set(b), |_, _, _, _| {}).absolute().size_full());
        if let Some(image) = image {
            sheet = sheet.child(img(ImageSource::Render(image)).absolute().size_full().object_fit(ObjectFit::Fill));
        }
        sheet = if tool.id == "edit" { self.edit_overlays(sheet, w, h, &p, window, cx) } else { self.canvas_overlays(tool, sheet, w, h, &p, cx) };

        // The page sits in the middle of the stage while it fits, and scrolls when it does not:
        // the pad is at least as large as the stage and grows with the page.
        let scroll = self.canvas_scroll.clone();
        let stage = div()
            .relative()
            .flex_1()
            .min_h_0()
            .bg(p.well)
            .child(
                div().id("stage").absolute().inset_0().overflow_scroll().track_scroll(&scroll).flex().flex_col().items_start().child(
                    div()
                        .id("stage-pad")
                        .flex_shrink_0()
                        .min_w_full()
                        .min_h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(20.))
                        .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                            this.canvas_pinch(event, cx);
                            cx.stop_propagation();
                        }))
                        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                            if event.modifiers.secondary() {
                                this.canvas_wheel(event, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(sheet.test_support()),
                ),
            )
            .scrollbar(&scroll, ScrollbarAxis::Both)
            .child(self.zoom_pill(&p, cx));

        ui::col().size_full().child(toolbar).child(stage).into_any_element()
    }

    /// Look up the page's size in points once per page.
    fn ensure_page_size(&mut self, path: &std::path::Path, page: usize, cx: &mut Context<Self>) {
        if self.canvas_size_for == Some((path.to_path_buf(), page)) {
            return;
        }
        self.canvas_size_for = Some((path.to_path_buf(), page));
        self.canvas.page_size = None;
        let file = path.to_path_buf();
        let task = cx.background_spawn({
            let file = file.clone();
            async move { shorui_core::render::Renderer::open_path(&file, None).ok().and_then(|r| r.page_size(page)) }
        });
        cx.spawn(async move |this, cx| {
            let size = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.canvas_size_for == Some((file.clone(), page)) {
                    this.canvas.page_size = size;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A mark drawn over the page image. `scale` is pixels per point.
    pub fn mark_box(rect: &Rect4, scale: f32) -> Div {
        div().absolute().left(px(rect.x0 * scale)).top(px(rect.y0 * scale)).w(px((rect.x1 - rect.x0) * scale)).h(px((rect.y1 - rect.y0) * scale))
    }
}
