//! The two workspaces that are not a queue: the page grid and the page canvas.

use crate::catalog::Tool;
use crate::shell::Shell;
use crate::state::{FileEntry, PagesEdit};
use crate::tokens::*;
use crate::ui::{self, Btn, KeyOn, Tone, pal};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

actions!(grid, [GridLeft, GridRight, GridUp, GridDown, GridSelectAll, GridDelete, GridRotateRight, GridRotateLeft, GridDuplicate, GridExtendLeft, GridExtendRight]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", GridLeft, Some("Grid")),
        KeyBinding::new("right", GridRight, Some("Grid")),
        KeyBinding::new("up", GridUp, Some("Grid")),
        KeyBinding::new("down", GridDown, Some("Grid")),
        KeyBinding::new("shift-left", GridExtendLeft, Some("Grid")),
        KeyBinding::new("shift-right", GridExtendRight, Some("Grid")),
        KeyBinding::new("secondary-a", GridSelectAll, Some("Grid")),
        KeyBinding::new("delete", GridDelete, Some("Grid")),
        KeyBinding::new("backspace", GridDelete, Some("Grid")),
        KeyBinding::new("r", GridRotateRight, Some("Grid")),
        KeyBinding::new("shift-r", GridRotateLeft, Some("Grid")),
        KeyBinding::new("secondary-d", GridDuplicate, Some("Grid")),
    ]);
}

/// Payload while pages are dragged in the grid.
#[derive(Clone)]
pub struct DragPages {
    pub count: usize,
}

pub struct DragPagesPreview {
    count: usize,
}

impl Render for DragPagesPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        div().relative().w(px(64.)).h(px(84.)).child(ui::paper_blank(56., 76., &p).absolute().left(px(6.)).top(px(6.)).opacity(0.7)).child(ui::paper_blank(56., 76., &p).absolute()).child(
            div().absolute().left(px(44.)).top(px(-8.)).flex().items_center().justify_center().min_w(px(20.)).h(px(20.)).px(px(6.)).rounded(px(10.)).bg(p.cyan).text_color(p.cyan_ink).font_family(FONT_MONO).text_size(px(TEXT_XS)).child(SharedString::from(self.count.to_string())),
        )
    }
}

const CELL_W: f32 = 136.;
const PAGE_W: f32 = 124.;
const PAGE_H: f32 = 172.;
const GRID_COLS: usize = 6;

impl Shell {
    /// A small page image for a queue row.
    pub fn thumb_small(&mut self, file: &FileEntry, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if file.is_image() && file.problem.is_none() {
            // The whole picture, letterboxed, so its shape and orientation show.
            let frame = div().size(px(36.)).flex_shrink_0().rounded(px(R_XS)).border_1().border_color(p.rule).bg(p.well).overflow_hidden().flex().items_center().justify_center();
            return match self.page_image(&file.path, 0, 128, &[0], cx) {
                Some(image) => frame.child(img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Contain)),
                None => frame.child(ui::icon("image", 14., p.toner_4)),
            }
            .into_any_element();
        }
        if !file.is_pdf() || file.locked || file.problem.is_some() {
            return div().w(px(22.)).h(px(30.)).flex_shrink_0().flex().items_center().justify_center().child(ui::icon(if file.is_pdf() { "protect" } else { "image" }, 14., p.toner_3)).into_any_element();
        }
        match self.page_image(&file.path, 0, 96, &[0], cx) {
            Some(image) => div().w(px(22.)).h(px(30.)).flex_shrink_0().rounded(px(2.)).border_1().border_color(p.stock_edge).overflow_hidden().child(img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Cover)).into_any_element(),
            None => ui::paper_blank(22., 30., p).into_any_element(),
        }
    }

    /// Make sure the grid edit matches the active file.
    fn sync_pages_edit(&mut self) {
        let pages = self.state.active().and_then(|f| f.pages).unwrap_or(0);
        if self.state.pages_edit.original_count != pages {
            self.state.pages_edit = PagesEdit::new(pages);
        }
    }

    fn grid_step(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let e = &mut self.state.pages_edit;
        if e.slots.is_empty() {
            return;
        }
        let next = (e.cursor as isize + delta).clamp(0, e.slots.len() as isize - 1) as usize;
        if extend {
            e.click(next, false, true);
            e.cursor = next;
        } else {
            e.click(next, false, false);
        }
        self.grid_scroll.scroll_to_item(next / GRID_COLS);
        cx.notify();
    }

    pub fn render_grid(&mut self, _tool: &'static Tool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        self.sync_pages_edit();
        let Some(file) = self.state.active().cloned() else {
            return self.single_file_empty("Drop a PDF to arrange its pages", &p, cx).into_any_element();
        };
        if file.locked {
            return self.single_file_message("This file is password protected", "Unlock it first, then come back to Pages.", &p).into_any_element();
        }
        let edit = self.state.pages_edit.clone();
        let selected = edit.selected.len();
        let busy = self.state.running;

        let act = |this: &mut Shell, f: &dyn Fn(&mut PagesEdit), cx: &mut Context<Shell>| {
            if !this.state.running {
                f(&mut this.state.pages_edit);
                cx.notify();
            }
        };
        let toolbar = ui::row()
            .h(px(TOPBAR_H))
            .px(px(16.))
            .gap(px(6.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(p.rule)
            .child(if selected > 0 { ui::chip(format!("{selected} selected"), Tone::Accent, &p) } else { ui::chip(format!("{} pages", edit.slots.len()), Tone::Neutral, &p) })
            .child(div().w(px(6.)))
            .child(ui::icon_button("rot-l", "rotate-ccw", 28., &p).on_click(cx.listener(move |this, _, _, cx| act(this, &|e| e.rotate(-90), cx))))
            .child(ui::icon_button("rot-r", "rotate-cw", 28., &p).on_click(cx.listener(move |this, _, _, cx| act(this, &|e| e.rotate(90), cx))))
            .child(ui::button("dup", Btn::Ghost, 28., !busy, &p).child(ui::icon("copy", 14., p.toner_2)).child("Duplicate").on_click(cx.listener(move |this, _, _, cx| act(this, &|e| e.duplicate(), cx))))
            .child(ui::button("del", Btn::Ghost, 28., !busy, &p).child(ui::icon("trash", 14., p.toner_2)).child("Delete").child(ui::kbd(&["Del"], &p, KeyOn::Surface)).on_click(cx.listener(|this, _, _, cx| this.grid_delete(cx))))
            .child(ui::grow())
            .child(ui::button("sel-all", Btn::Ghost, 28., !busy, &p).child("Select all").on_click(cx.listener(move |this, _, _, cx| act(this, &|e| e.select_all(), cx))))
            .when(edit.is_changed(), |s| {
                s.child(ui::button("revert", Btn::Ghost, 28., !busy, &p).child(ui::icon("undo", 14., p.toner_2)).child("Revert").on_click(cx.listener(|this, _, _, cx| {
                    let n = this.state.pages_edit.original_count;
                    this.state.pages_edit = PagesEdit::new(n);
                    cx.notify();
                })))
            });

        // Only pages near the viewport get an image; long documents stay light.
        let row_h = PAGE_H + 10. + 2. + 4. + 16. + 16.;
        let scrolled = (-f32::from(self.grid_scroll.offset().y)).max(0.);
        let view_h = f32::from(_window.viewport_size().height);
        let first_row = (scrolled / row_h).floor() as usize;
        let last_row = ((scrolled + view_h) / row_h).ceil() as usize + 1;
        let near = |ix: usize| {
            let r = ix / GRID_COLS;
            r + 1 >= first_row && r <= last_row
        };
        let batch: Vec<usize> = edit.slots.iter().enumerate().filter(|(ix, _)| near(*ix)).map(|(_, s)| s.source - 1).collect();
        let mut grid = div().flex().flex_row().flex_wrap().gap_x(px(8.)).gap_y(px(16.));
        for (ix, slot) in edit.slots.iter().enumerate() {
            let sel = edit.selected.contains(&ix);
            let cursor = edit.cursor == ix && self.grid_focus.is_focused(_window);
            let image = if near(ix) { self.page_image(&file.path, slot.source - 1, 256, &batch, cx) } else { None }.map(|image| self.thumbs.turned(&(file.path.clone(), slot.source - 1, 256), slot.turn, image));
            let turned = slot.turn % 180 != 0;
            let (w, h) = if turned { (PAGE_W, PAGE_W * PAGE_W / PAGE_H) } else { (PAGE_W, PAGE_H) };
            let sheet = match image {
                Some(image) => {
                    let picture = div().w(px(w)).h(px(h)).rounded(px(2.)).border_1().border_color(p.stock_edge).overflow_hidden().child(img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Contain));
                    div().w(px(PAGE_W)).h(px(PAGE_H)).flex().items_center().justify_center().child(picture)
                }
                None => div().w(px(PAGE_W)).h(px(PAGE_H)).flex().items_center().justify_center().child(ui::paper_blank(w, h, &p)),
            };
            let count = if sel { selected.max(1) } else { 1 };
            let cell = div()
                .id(("page", ix))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.))
                .w(px(CELL_W))
                .relative()
                .child(
                    div()
                        .p(px(5.))
                        .rounded(px(R_SM))
                        .border_1()
                        .border_color(if sel { p.cyan_rule } else { transparent_black() })
                        .when(sel, |s| s.bg(p.cyan_wash))
                        .when(cursor, |s| s.shadow(vec![BoxShadow { color: p.cyan, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(2.), inset: true }]))
                        .child(sheet),
                )
                .when(sel, |s| s.child(div().absolute().top(px(10.)).left(px(10.)).flex().items_center().justify_center().size(px(16.)).rounded_full().bg(p.cyan).child(ui::icon("check", 10., p.cyan_ink))))
                .child(ui::row().h(px(16.)).gap(px(6.)).child(ui::mono((ix + 1).to_string()).text_size(px(TEXT_XS)).text_color(if sel { p.cyan } else { p.toner_3 })).when(slot.turn != 0, |s| s.child(ui::mono(format!("{}°", slot.turn)).text_size(px(TEXT_XS)).text_color(p.toner_4))).when(slot.source != ix + 1, |s| s.child(ui::mono(format!("was {}", slot.source)).text_size(px(TEXT_XS)).text_color(p.toner_4))))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if this.state.running {
                        return;
                    }
                    let m = e.modifiers();
                    this.state.pages_edit.click(ix, m.secondary(), m.shift);
                    window.focus(&this.grid_focus, cx);
                    cx.notify();
                }))
                .on_drag(DragPages { count }, move |drag: &DragPages, _, _, cx| cx.new(|_| DragPagesPreview { count: drag.count }))
                .drag_over::<DragPages>(move |s, _, _, _| s.border_l_2().border_color(p.cyan))
                .on_drop(cx.listener(move |this, _: &DragPages, _, cx| {
                    if !this.state.running {
                        this.state.pages_edit.move_to(ix);
                        cx.notify();
                    }
                }));
            grid = grid.child(cell);
        }

        ui::col()
            .size_full()
            .child(toolbar)
            .child(
                div()
                    .id("grid")
                    .key_context("Grid")
                    .track_focus(&self.grid_focus)
                    .on_action(cx.listener(|this, _: &GridLeft, _, cx| this.grid_step(-1, false, cx)))
                    .on_action(cx.listener(|this, _: &GridRight, _, cx| this.grid_step(1, false, cx)))
                    .on_action(cx.listener(|this, _: &GridUp, _, cx| this.grid_step(-(GRID_COLS as isize), false, cx)))
                    .on_action(cx.listener(|this, _: &GridDown, _, cx| this.grid_step(GRID_COLS as isize, false, cx)))
                    .on_action(cx.listener(|this, _: &GridExtendLeft, _, cx| this.grid_step(-1, true, cx)))
                    .on_action(cx.listener(|this, _: &GridExtendRight, _, cx| this.grid_step(1, true, cx)))
                    .on_action(cx.listener(|this, _: &GridSelectAll, _, cx| {
                        this.state.pages_edit.select_all();
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &GridDelete, _, cx| this.grid_delete(cx)))
                    .on_action(cx.listener(|this, _: &GridRotateRight, _, cx| {
                        this.state.pages_edit.rotate(90);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &GridRotateLeft, _, cx| {
                        this.state.pages_edit.rotate(-90);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &GridDuplicate, _, cx| {
                        this.state.pages_edit.duplicate();
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| window.focus(&this.grid_focus, cx)))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.grid_scroll)
                    .p(px(24.))
                    .child(grid),
            )
            .into_any_element()
    }

    fn grid_delete(&mut self, cx: &mut Context<Self>) {
        if self.state.running {
            return;
        }
        if !self.state.pages_edit.delete() {
            self.show_toast(false, "A PDF needs at least one page", "Deselect a page, or use Split to take pages out.", cx);
        }
        cx.notify();
    }

    pub fn single_file_empty(&mut self, title: &'static str, p: &Palette, cx: &mut Context<Self>) -> Div {
        div().size_full().p(px(24.)).child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(16.))
                .rounded(px(R_LG))
                .border_1()
                .border_dashed()
                .border_color(p.rule_strong)
                .bg(p.well)
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(ui::button("browse-one", Btn::Default, 28., true, p).child("Browse files").child(ui::kbd(&[crate::palette_model::mod_label(), "O"], p, KeyOn::Surface)).on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx)))),
        )
    }

    pub fn single_file_message(&self, title: &'static str, body: &'static str, p: &Palette) -> Div {
        div().size_full().flex().flex_col().items_center().justify_center().gap(px(6.)).child(ui::icon("protect", 20., p.toner_3)).child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title)).child(div().text_color(p.toner_2).child(body))
    }
}
