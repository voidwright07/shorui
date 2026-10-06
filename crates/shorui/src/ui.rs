//! Small building blocks drawn from the design tokens. Layout only: no state lives here.

use crate::assets::icon_path;
use crate::tokens::*;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// The active palette, stored as a global so any view can read it.
#[derive(Clone, Copy)]
pub struct Theme(pub Palette);
impl Global for Theme {}

pub fn pal(cx: &App) -> Palette {
    cx.try_global::<Theme>().map(|t| t.0).unwrap_or_else(Palette::dark)
}

pub fn row() -> Div {
    div().flex().flex_row().items_center()
}

pub fn col() -> Div {
    div().flex().flex_col()
}

pub fn grow() -> Div {
    div().flex_1()
}

/// A line icon at a fixed size, painted in `color`.
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg().path(icon_path(name)).size(px(size)).flex_shrink_0().text_color(color)
}

/// Text in the monospace face, for anything measured or typed.
pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(FONT_MONO).text_size(px(TEXT_SM)).child(text.into())
}

/// A small heading for a section of a panel or list.
pub fn section_label(text: impl Into<SharedString>, p: &Palette) -> Div {
    div().text_size(px(TEXT_SM)).font_weight(FontWeight::MEDIUM).text_color(p.toner_3).child(text.into())
}

pub fn dot(color: Hsla, size: f32) -> Div {
    div().size(px(size)).rounded_full().bg(color).flex_shrink_0()
}

pub fn vrule(p: &Palette, height: f32) -> Div {
    div().w(px(1.)).h(px(height)).bg(p.rule_strong).flex_shrink_0()
}

/// A two-pixel ring in the accent colour, used wherever keyboard focus lands. It is drawn
/// inside the element's edge so it also works on controls with no fill of their own.
pub fn ring(mut style: StyleRefinement, p: &Palette) -> StyleRefinement {
    style.box_shadow = Some(vec![BoxShadow { color: p.cyan, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(2.), inset: true }]);
    style
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyOn {
    /// On a neutral surface.
    Surface,
    /// Inside the primary button.
    Accent,
    /// Inside the destructive button.
    Danger,
    /// Inside a disabled button.
    Muted,
}

/// Key caps: `["Ctrl", "K"]`.
pub fn kbd<S: AsRef<str>>(keys: &[S], p: &Palette, on: KeyOn) -> Div {
    let (bg, border, fg) = match on {
        KeyOn::Surface => (p.key, p.rule, p.toner_2),
        KeyOn::Accent => (hsla(0., 0., 0., 0.16), transparent_black(), p.cyan_ink),
        KeyOn::Danger => (hsla(0., 0., 0., 0.16), transparent_black(), p.stamp_ink),
        KeyOn::Muted => (p.key, p.rule_soft, p.toner_4),
    };
    row().gap(px(3.)).flex_shrink_0().children(keys.iter().map(|k| {
        let label = k.as_ref();
        let cap = row().justify_center().min_w(px(18.)).h(px(18.)).px(px(4.)).rounded(px(R_XS)).bg(bg).border_1().border_color(border).text_color(fg).font_family(FONT_MONO).text_size(px(TEXT_XS)).line_height(px(16.));
        match label {
            "Up" => cap.child(icon("arrow-up", 10., fg)),
            "Down" => cap.child(icon("arrow-down", 10., fg)),
            other => cap.child(SharedString::from(other.to_string())),
        }
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Default,
    Primary,
    Ghost,
    Danger,
}

/// A button shell. Add children and `on_click`. `height` is 24, 28 or 32.
pub fn button(id: impl Into<ElementId>, kind: Btn, height: f32, enabled: bool, p: &Palette) -> Stateful<Div> {
    let p = *p;
    let base = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .gap(px(6.))
        .h(px(height))
        .px(px(if height >= 32. { 12. } else if height <= 24. { 8. } else { 10. }))
        .rounded(px(R_SM))
        .border_1()
        .text_size(px(if height <= 24. { TEXT_SM } else { TEXT }))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap();
    if !enabled {
        return match kind {
            Btn::Ghost => base.border_color(transparent_black()).text_color(p.toner_4),
            _ => base.bg(p.plate).border_color(p.rule_soft).text_color(p.toner_4),
        };
    }
    let base = base.cursor_pointer().tab_index(0).focus_visible(move |s| ring(s, &p));
    match kind {
        Btn::Default => base.bg(p.plate).border_color(p.control_rule).text_color(p.toner).hover(move |s| s.bg(p.plate_2)).active(move |s| s.bg(p.plate_3)),
        Btn::Primary => base.bg(p.cyan).border_color(transparent_black()).text_color(p.cyan_ink).hover(move |s| s.bg(p.cyan_hi)).active(move |s| s.bg(p.cyan_lo)),
        Btn::Danger => base.bg(p.stamp_fill).border_color(transparent_black()).text_color(p.stamp_ink).hover(move |s| s.bg(p.stamp_hi)).active(move |s| s.bg(p.stamp_lo)),
        Btn::Ghost => base.border_color(transparent_black()).text_color(p.toner_2).hover(move |s| s.bg(p.plate).text_color(p.toner)).active(move |s| s.bg(p.plate_2)),
    }
}

/// Adds a tooltip naming the control. Icon-only controls need one.
pub fn tip(el: Stateful<Div>, label: &'static str) -> Stateful<Div> {
    el.tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
}

/// The label an icon button gets from its icon, used for its tooltip.
fn icon_label(name: &str) -> Option<&'static str> {
    Some(match name {
        "x" => "Remove",
        "plus" => "Add files",
        "sidebar" => "Collapse or expand the sidebar",
        "rotate-cw" => "Rotate right",
        "rotate-ccw" => "Rotate left",
        "chev-left" => "Previous page",
        "chev-right" => "Next page",
        "undo" => "Undo",
        "trash" => "Delete",
        _ => return None,
    })
}

/// A square button holding one icon.
pub fn icon_button(id: impl Into<ElementId>, name: &str, size: f32, p: &Palette) -> Stateful<Div> {
    let button = icon_button_plain(id, name, size, p);
    match icon_label(name) {
        Some(label) => tip(button, label),
        None => button,
    }
}

fn icon_button_plain(id: impl Into<ElementId>, name: &str, size: f32, p: &Palette) -> Stateful<Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .size(px(size))
        .rounded(px(R_SM))
        .cursor_pointer()
        .tab_index(0)
        .text_color(p.toner_2)
        .hover(move |s| s.bg(p.plate).text_color(p.toner))
        .active(move |s| s.bg(p.plate_2))
        .focus_visible(move |s| ring(s, &p))
        .child(svg().path(icon_path(name)).size(px(14.)).flex_shrink_0().text_color(p.toner_2))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Danger,
}

pub fn chip(text: impl Into<SharedString>, tone: Tone, p: &Palette) -> Div {
    let (bg, border, fg) = match tone {
        Tone::Neutral => (transparent_black(), p.rule, p.toner_2),
        Tone::Accent => (p.cyan_wash, p.cyan_rule, p.toner),
        Tone::Danger => (p.stamp_wash, p.stamp_rule, p.stamp),
    };
    row().h(px(20.)).px(px(6.)).rounded(px(R_XS)).border_1().border_color(border).bg(bg).text_color(fg).text_size(px(TEXT_SM)).font_weight(FontWeight::MEDIUM).flex_shrink_0().whitespace_nowrap().child(text.into())
}

/// A switch. Add `on_click`.
pub fn switch(id: impl Into<ElementId>, on: bool, p: &Palette) -> Stateful<Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .w(px(26.))
        .h(px(16.))
        .px(px(2.))
        .rounded(px(8.))
        .cursor_pointer()
        .tab_index(0)
        .bg(if on { p.cyan } else { p.plate_3 })
        .when(on, |s| s.justify_end())
        .hover(move |s| s.bg(if on { p.cyan_hi } else { p.toner_4 }))
        .focus_visible(move |s| ring(s, &p))
        .child(div().size(px(12.)).rounded_full().bg(if on { p.cyan_ink } else { p.knob }))
}

/// A check box. Add `on_click`.
pub fn checkbox(id: impl Into<ElementId>, on: bool, p: &Palette) -> Stateful<Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .size(px(14.))
        .rounded(px(R_XS))
        .cursor_pointer()
        .tab_index(0)
        .border_1()
        .border_color(if on { transparent_black() } else { p.control_rule })
        .bg(if on { p.cyan } else { p.well })
        .hover(move |s| if on { s.bg(p.cyan_hi) } else { s.border_color(p.toner_3) })
        .focus_visible(move |s| ring(s, &p))
        .when(on, |s| s.child(icon("check", 10., p.cyan_ink)))
}

/// A toggle chip, optionally with a count. Add `on_click`.
pub fn toggle_chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool, count: Option<usize>, p: &Palette) -> Stateful<Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(6.))
        .h(px(24.))
        .px(px(8.))
        .rounded(px(R_SM))
        .cursor_pointer()
        .tab_index(0)
        .border_1()
        .border_color(if on { p.cyan_rule } else { p.control_rule })
        .when(on, |s| s.bg(p.cyan_wash))
        .text_color(if on { p.toner } else { p.toner_2 })
        .text_size(px(TEXT_SM))
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| if on { s.border_color(p.cyan) } else { s.bg(p.plate).text_color(p.toner) })
        .focus_visible(move |s| ring(s, &p))
        .child(label.into())
        .when_some(count, |s, n| s.child(mono(n.to_string()).text_size(px(TEXT_XS)).text_color(p.toner_3)))
}

/// One segment of a segmented control. Wrap several in `segmented`.
pub fn segment(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool, p: &Palette) -> Stateful<Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .h_full()
        .px(px(10.))
        .rounded(px(R_XS))
        .cursor_pointer()
        .tab_index(0)
        .text_size(px(TEXT_SM))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .text_color(if active { p.toner } else { p.toner_2 })
        .when(active, |s| s.bg(p.plate_2))
        .when(!active, |s| s.hover(move |s| s.bg(p.plate).text_color(p.toner)))
        .focus_visible(move |s| ring(s, &p))
        .child(label.into())
}

pub fn segmented(p: &Palette) -> Div {
    row().gap(px(2.)).p(px(2.)).h(px(CONTROL_H)).rounded(px(R_SM)).border_1().border_color(p.rule).bg(p.well).flex_shrink_0()
}

/// A thin progress bar. `fraction` runs 0 to 1.
pub fn progress(fraction: f32, color: Hsla, p: &Palette) -> Div {
    div().h(px(4.)).w_full().rounded(px(2.)).bg(p.plate_3).overflow_hidden().child(div().h_full().w(relative(fraction.clamp(0., 1.))).rounded(px(2.)).bg(color))
}

/// A page stand-in drawn while the real thumbnail is being rendered.
pub fn paper_blank(width: f32, height: f32, p: &Palette) -> Div {
    div().w(px(width)).h(px(height)).flex_shrink_0().rounded(px(2.)).bg(p.stock).border_1().border_color(p.stock_edge)
}

/// The label and value of one line in the options panel.
pub fn prop_row(label: impl Into<SharedString>, p: &Palette) -> Div {
    row().min_h(px(32.)).justify_between().gap(px(12.)).child(div().text_color(p.toner_2).flex_shrink_0().child(label.into()))
}
