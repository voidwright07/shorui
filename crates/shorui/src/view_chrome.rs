//! Sidebar, top bar and run line.

use crate::catalog::{self, Inputs, Tool};
use crate::palette_model::mod_label;
use crate::shell::Shell;
use crate::state::RowStatus;
use crate::tokens::*;
use crate::ui::{self, Btn, KeyOn, pal};
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use shorui_core::tools::Group;

const APP_NAME: &str = "Shorui";

/// Makes an element part of the window's caption: dragging it moves the window and a
/// double click maximises it. Controls inside a caption must `occlude()`, or the caption
/// takes their clicks.
fn caption(el: Div) -> Div {
    let el = el.window_control_area(WindowControlArea::Drag);
    if cfg!(target_os = "windows") {
        // Windows does both itself for the area tagged as the caption.
        el
    } else {
        el.on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                if cfg!(target_os = "macos") { window.titlebar_double_click() } else { window.zoom_window() }
            } else {
                window.start_window_move();
            }
        })
    }
}

/// Minimise, maximise and close. macOS draws its own.
fn window_controls(window: &Window, p: &Palette) -> Div {
    if cfg!(target_os = "macos") {
        return div();
    }
    let p = *p;
    let control = |id: &'static str, icon: IconName, area: WindowControlArea| {
        let close = area == WindowControlArea::Close;
        let el = div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(46.))
            .h(px(TOPBAR_H))
            .flex_shrink_0()
            .text_color(p.toner_2)
            .hover(move |s| if close { s.bg(gpui_kit::rgb(0xC42B1C)).text_color(gpui_kit::white()) } else { s.bg(p.plate_2).text_color(p.toner) })
            .when(close, |s| s.rounded_tr(px(R_LG - 1.)))
            // Without this the caption around the buttons is what the pointer is over, and
            // the system treats a click on a button as the start of a window drag.
            .occlude()
            .window_control_area(area)
            .child(Icon::new(icon).small());
        if cfg!(target_os = "windows") {
            // The system acts on the tagged area itself, which keeps snap layouts working.
            el
        } else {
            el.on_click(move |_, window, _| match area {
                WindowControlArea::Min => window.minimize_window(),
                WindowControlArea::Max => window.zoom_window(),
                _ => window.remove_window(),
            })
        }
    };
    ui::row()
        .flex_shrink_0()
        .child(control("win-min", IconName::WindowMinimize, WindowControlArea::Min))
        .child(control("win-max", if window.is_maximized() { IconName::WindowRestore } else { IconName::WindowMaximize }, WindowControlArea::Max))
        .child(control("win-close", IconName::WindowClose, WindowControlArea::Close))
}

fn logo(p: &Palette) -> Div {
    div().flex().items_center().justify_center().size(px(20.)).rounded(px(5.)).bg(p.stock).flex_shrink_0().child(ui::icon("logo", 13., p.stock_ink))
}

fn nav_item(id: impl Into<ElementId>, icon: &str, label: &str, active: bool, hint: Option<String>, p: &Palette) -> Stateful<Div> {
    let p = *p;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(8.))
        .h(px(NAV_H))
        .px(px(8.))
        .rounded(px(R_SM))
        .cursor_pointer()
        .tab_index(0)
        .font_weight(FontWeight::MEDIUM)
        .text_color(if active { p.toner } else { p.toner_2 })
        .when(active, |s| s.bg(p.plate_2))
        .when(!active, |s| s.hover(move |s| s.bg(p.plate).text_color(p.toner)))
        .focus_visible(move |s| ui::ring(s, &p))
        .child(ui::icon(icon, 14., if active { p.toner } else { p.toner_3 }))
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(label.to_string())))
        .when_some(hint, |s, hint| s.child(ui::mono(hint).text_size(px(TEXT_XS)).text_color(p.toner_4)))
}

impl Shell {
    pub fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let narrow = window.viewport_size().width < px(1200.);
        if self.settings.sidebar_collapsed || narrow {
            return self.render_rail(&p, cx).into_any_element();
        }
        let active = self.state.mode;
        let label = |text: &'static str| ui::row().h(px(28.)).px(px(8.)).flex_shrink_0().child(ui::section_label(text, &p));
        let tool_item = |this: &Self, prefix: &'static str, tool: &'static Tool, hint: Option<String>, cx: &mut Context<Self>| {
            let id = tool.id;
            let favorite = this.settings.is_favorite(id);
            // A star on every tool: shown on hover, and always on favourites.
            let star = div()
                .id((if prefix == "fav" { "fav-star" } else if prefix == "recent" { "recent-star" } else { "tool-star" }, tool_index(tool)))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .size(px(20.))
                .rounded(px(R_XS))
                .cursor_pointer()
                .when(!favorite, |s| s.invisible().group_hover("nav-row", |s| s.visible()))
                .hover(move |s| s.bg(p.plate_2))
                .child(ui::icon(if favorite { "star-fill" } else { "star" }, 12., if favorite { p.toner_2 } else { p.toner_3 }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_favorite(id, cx);
                }));
            let star = ui::tip(star, if favorite { "Remove from favorites" } else { "Add to favorites" });
            nav_item((prefix, tool_index(tool)), tool.icon, &tool.name.replace("&amp;", "&"), active == Some(tool.id), hint, &p).group("nav-row").child(star).on_click(cx.listener(move |this, _, _, cx| this.open_tool(id, cx)))
        };

        let mut favorites = ui::col().gap(px(1.)).child(label("Favorites"));
        let starred: Vec<&'static Tool> = self.settings.favorites.iter().filter_map(|id| catalog::tool(id)).collect();
        if starred.is_empty() {
            favorites = favorites.child(ui::row().min_h(px(28.)).px(px(8.)).gap(px(6.)).text_size(px(TEXT_SM)).text_color(p.toner_3).child(ui::icon("star", 12., p.toner_4)).child("Star a tool to keep it here"));
        }
        for (i, tool) in starred.into_iter().enumerate() {
            let hint = if i < 3 { Some(format!("{} {}", mod_label(), i + 1)) } else { None };
            favorites = favorites.child(tool_item(self, "fav", tool, hint, cx));
        }
        let mut recent = ui::col().gap(px(1.)).child(label("Recent"));
        let recents: Vec<&'static Tool> = self.settings.recent.iter().filter_map(|id| catalog::tool(id)).filter(|t| !self.settings.is_favorite(t.id)).take(3).collect();
        for tool in &recents {
            recent = recent.child(tool_item(self, "recent", tool, None, cx));
        }

        let mut tools = ui::col().gap(px(1.)).child(label("Tools"));
        for group in Group::ALL {
            let open = self.open_groups.contains(&group);
            let count = catalog::in_group(group).count();
            tools = tools.child(
                div()
                    .id(("group", group as usize))
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(8.))
                    .h(px(NAV_H))
                    .px(px(8.))
                    .rounded(px(R_SM))
                    .cursor_pointer()
                    .tab_index(0)
                    .text_size(px(TEXT_SM))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.toner_2)
                    .hover(move |s| s.bg(p.plate).text_color(p.toner))
                    .focus_visible(move |s| ui::ring(s, &p))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.open_groups.remove(&group) {
                            this.open_groups.insert(group);
                        }
                        cx.notify();
                    }))
                    .child(ui::icon(if open { "chev-down" } else { "chev-right" }, 10., p.toner_3))
                    .child(ui::dot(p.group(group), 6.))
                    .child(div().flex_1().child(group.name()))
                    .child(ui::mono(count.to_string()).text_size(px(TEXT_XS)).text_color(p.toner_4)),
            );
            if open {
                let mut list = ui::col().gap(px(1.)).pl(px(10.));
                for tool in catalog::in_group(group) {
                    list = list.child(tool_item(self, "tool", tool, None, cx));
                }
                tools = tools.child(list);
            }
        }

        ui::col()
            .w(px(SIDEBAR_W))
            .h_full()
            .flex_shrink_0()
            .child(
                caption(ui::row())
                    .h(px(TOPBAR_H))
                    // Leave room for the traffic lights on macOS.
                    .pl(px(if cfg!(target_os = "macos") { 78. } else { 12. }))
                    .pr(px(8.))
                    .gap(px(8.))
                    .flex_shrink_0()
                    .child(logo(&p))
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(APP_NAME))
                    .child(ui::icon_button("collapse", "sidebar", 24., &p).occlude().on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(8.))
                    .py(px(4.))
                    .gap(px(12.))
                    .child(nav_item("home", "home", "Home", active.is_none(), None, &p).on_click(cx.listener(|this, _, _, cx| this.go_home(cx))))
                    .child(favorites)
                    .when(!recents.is_empty(), |s| s.child(recent))
                    .child(tools),
            )
            .child(
                ui::row()
                    .h(px(TOPBAR_H))
                    .px(px(8.))
                    .gap(px(8.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(p.rule_soft)
                    .child(ui::grow())
                    .child(self.theme_toggle(&p, cx)),
            )
            .into_any_element()
    }

    fn theme_toggle(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = self.settings.dark;
        let p = *p;
        let part = |id: &'static str, icon: &'static str, on: bool| {
            div().id(id).flex().items_center().justify_center().w(px(24.)).h_full().rounded(px(R_XS)).when(on, |s| s.bg(p.plate_2)).child(ui::icon(icon, 12., if on { p.toner } else { p.toner_3 }))
        };
        div()
            .id("theme")
            .flex()
            .flex_row()
            .gap(px(2.))
            .p(px(2.))
            .h(px(24.))
            .rounded(px(R_SM))
            .border_1()
            .border_color(p.rule)
            .bg(p.well)
            .cursor_pointer()
            .tab_index(0)
            .focus_visible(move |s| ui::ring(s, &p))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))
            .child(part("theme-light", "sun", !dark))
            .child(part("theme-dark", "moon", dark))
    }

    fn render_rail(&mut self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        let active = self.state.mode;
        let active_tool = active.and_then(catalog::tool);
        let rail_button = |id: ElementId, icon: &str, on: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(R_SM))
                .cursor_pointer()
                .tab_index(0)
                .when(on, |s| s.bg(p.plate_2))
                .when(!on, |s| s.hover(move |s| s.bg(p.plate)))
                .focus_visible(move |s| ui::ring(s, &p))
                .child(ui::icon(icon, 14., if on { p.toner } else { p.toner_2 }))
        };
        let mut body = ui::col().flex_1().min_h_0().items_center().py(px(4.)).gap(px(2.)).child(rail_button("rail-home".into(), "home", active.is_none()).on_click(cx.listener(|this, _, _, cx| this.go_home(cx))));
        for (i, tool) in self.settings.favorites.iter().filter_map(|id| catalog::tool(id)).take(6).enumerate() {
            let id = tool.id;
            body = body.child(rail_button(("rail-fav", i).into(), tool.icon, active == Some(tool.id)).on_click(cx.listener(move |this, _, _, cx| this.open_tool(id, cx))));
        }
        body = body.child(div().w(px(20.)).h(px(1.)).my(px(6.)).bg(p.rule_strong));
        for group in Group::ALL {
            let here = active_tool.filter(|t| t.group == group);
            let item = div()
                .id(("rail-group", group as usize))
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(R_SM))
                .cursor_pointer()
                .tab_index(0)
                .when(here.is_some(), |s| s.bg(p.plate_2))
                .when(here.is_none(), |s| s.hover(move |s| s.bg(p.plate)))
                .focus_visible(move |s| ui::ring(s, &p))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // Expanding shows the group's tools; the rail has no room for them.
                    this.settings.sidebar_collapsed = false;
                    this.open_groups.insert(group);
                    this.save_prefs();
                    cx.notify();
                }));
            body = body.child(match here {
                Some(tool) => item.child(ui::icon(tool.icon, 14., p.toner)),
                None => item.child(ui::dot(p.group(group), 8.)),
            });
        }
        ui::col()
            .w(px(RAIL_W))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .child(caption(ui::row()).h(px(TOPBAR_H)).w_full().justify_center().flex_shrink_0().when(!cfg!(target_os = "macos"), |s| s.child(logo(&p))))
            .child(body)
            .child(
                ui::col()
                    .items_center()
                    .py(px(8.))
                    .gap(px(2.))
                    .w_full()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(p.rule_soft)
                    .child(rail_button("rail-expand".into(), "sidebar", false).on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))))
                    .child(rail_button("rail-theme".into(), if self.settings.dark { "sun" } else { "moon" }, false).on_click(cx.listener(|this, _, _, cx| this.toggle_theme(cx)))),
            )
    }

    pub fn render_topbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let tool = self.state.tool();
        let compact = window.viewport_size().width < px(1240.);
        let visible = self.state.visible();
        let shown = if compact { 0 } else { 2.min(visible.len()) };

        let mut crumbs = ui::row().gap(px(8.)).flex_shrink_0();
        match tool {
            Some(tool) => {
                crumbs = crumbs
                    .child(ui::dot(p.group(tool.group), 6.))
                    .child(div().text_color(p.toner_3).child(tool.group.name()))
                    .child(ui::icon("chev-right", 10., p.toner_4))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(tool.name.replace("&amp;", "&"))))
                    .child({
                        let id = tool.id;
                        let favorite = self.settings.is_favorite(id);
                        let star = div()
                            .id("fav-current")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded(px(R_SM))
                            .cursor_pointer()
                            // The top bar moves the window; the star must take its own clicks.
                            .occlude()
                            .hover(move |s| s.bg(p.plate))
                            .child(ui::icon(if favorite { "star-fill" } else { "star" }, 13., if favorite { p.toner_2 } else { p.toner_3 }))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_favorite(id, cx)));
                        ui::tip(star, if favorite { "Remove from favorites" } else { "Add to favorites" })
                    });
            }
            None => crumbs = crumbs.child(div().font_weight(FontWeight::SEMIBOLD).child("Home")),
        }

        let mut files = ui::row().gap(px(6.)).flex_1().min_w_0().overflow_hidden();
        if !visible.is_empty() {
            files = files.child(ui::vrule(&p, 16.)).child(div().w(px(2.)));
            for &i in visible.iter().take(shown) {
                let name = self.state.files[i].name.clone();
                files = files.child(
                    ui::row()
                        .occlude()
                        .h(px(24.))
                        .pl(px(6.))
                        .pr(px(2.))
                        .gap(px(6.))
                        .rounded(px(R_SM))
                        .border_1()
                        .border_color(p.rule)
                        .text_color(p.toner_2)
                        .flex_shrink_0()
                        .child(ui::icon("file", 12., p.toner_3))
                        .child(ui::mono(name))
                        .child(
                            div()
                                .id(("chip-close", i))
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(18.))
                                .rounded(px(R_XS))
                                .cursor_pointer()
                                .hover(|s| s.bg(p.plate_2))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.state.running {
                                        this.state.remove_file(i);
                                        cx.notify();
                                    }
                                }))
                                .child(ui::icon("x", 10., p.toner_3)),
                        ),
                );
            }
            if visible.len() > shown {
                let more = if shown == 0 { format!("{} {}", visible.len(), if visible.len() == 1 { "file" } else { "files" }) } else { format!("+{}", visible.len() - shown) };
                files = files.child(ui::mono(more).text_color(p.toner_3).px(px(4.)));
            }
            files = files.child(ui::icon_button("topbar-add", "plus", 24., &p).occlude().on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))));
        }

        let search = div()
            .id("search")
            .occlude()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .gap(px(8.))
            .h(px(CONTROL_H))
            .pl(px(8.))
            .pr(px(5.))
            .rounded(px(R_SM))
            .border_1()
            .border_color(p.rule)
            .bg(p.well)
            .text_color(p.toner_3)
            .cursor_pointer()
            .tab_index(0)
            .hover(move |s| s.border_color(p.rule_strong))
            .focus_visible(move |s| ui::ring(s, &p))
            .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx)))
            .child(ui::icon("search", 14., p.toner_3))
            .when(!compact, |s| s.w(px(260.)).child(div().flex_1().child("Search tools and actions")))
            .child(ui::kbd(&crate::palette::open_keys(), &p, KeyOn::Surface));

        let local_open = self.local_open;
        let local = div()
            .id("local")
            .occlude()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .cursor_pointer()
            .tab_index(0)
            .focus_visible(move |s| ui::ring(s, &p))
            .on_click(cx.listener(|this, _, _, cx| {
                this.local_open = !this.local_open;
                cx.notify();
            }))
            .when(local_open, |s| {
                s.child(deferred(
                    ui::col()
                        .absolute()
                        .top(px(32.))
                        .right_0()
                        .w(px(280.))
                        .p(px(12.))
                        .gap(px(6.))
                        .rounded(px(R_MD))
                        .border_1()
                        .border_color(p.rule_strong)
                        .bg(p.sleeve)
                        .occlude()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("Runs on this machine"))
                        .child(div().text_size(px(TEXT_SM)).line_height(px(18.)).text_color(p.toner_2).child("Files are read and written on disk here. Nothing is uploaded. Only HTML/URL to PDF goes online, and only when you give it a web address."))
                        .child(ui::row().justify_between().mt(px(4.)).pt(px(8.)).border_t_1().border_color(p.rule_soft).child(div().text_size(px(TEXT_SM)).text_color(p.toner_2).child("Network access")).child(ui::mono("off"))),
                ).with_priority(2))
            })
            .rounded(px(R_SM))
            .h(px(24.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(R_SM))
            .border_1()
            .border_color(p.rule)
            .text_color(p.toner_2)
            .text_size(px(TEXT_SM))
            .font_weight(FontWeight::MEDIUM)
            .flex_shrink_0()
            .child(ui::dot(p.proof, 6.))
            .child("Local only");

        let controls = window_controls(window, &p);
        let trailing = if cfg!(target_os = "macos") { 12. } else { 0. };
        caption(ui::row()).h(px(TOPBAR_H)).pl(px(16.)).pr(px(trailing)).gap(px(12.)).flex_shrink_0().border_b_1().border_color(p.rule).child(crumbs).child(files).child(search).child(local).child(controls)
    }

    pub fn render_runline(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let compact = window.viewport_size().width < px(1240.);
        let tool = self.state.tool();
        let running = self.state.running;
        let keys = [mod_label(), "Enter"];
        let mut bar = ui::row().h(px(RUNLINE_H)).pl(px(16.)).pr(px(12.)).gap(px(10.)).flex_shrink_0().border_t_1().border_color(p.rule);

        if running {
            let total = self.state.files.iter().filter(|f| !matches!(f.status, RowStatus::Idle)).count().max(1);
            let done = self.state.files.iter().filter(|f| matches!(f.status, RowStatus::Done { .. })).count();
            let failed = self.state.files.iter().filter(|f| matches!(f.status, RowStatus::Failed { .. })).count();
            let partial: f32 = self.state.files.iter().map(|f| if let RowStatus::Running { fraction, .. } = &f.status { *fraction } else { 0.0 }).sum();
            let fraction = (done as f32 + failed as f32 + partial) / total as f32;
            let mut text = format!("{done} of {total} done");
            if failed > 0 {
                text.push_str(&format!(" · {failed} failed"));
            }
            return bar
                .child(ui::mono(text).text_color(p.toner_2))
                .child(ui::grow())
                .child(div().w(px(180.)).child(ui::progress(fraction, p.cyan, &p)))
                .child(div().w(px(4.)))
                .child(ui::button("cancel", Btn::Default, 32., true, &p).child("Cancel").child(ui::kbd(&["Esc"], &p, KeyOn::Surface)).on_click(cx.listener(|this, _, _, cx| this.cancel_run(cx))))
                .into_any_element();
        }

        bar = bar.child(ui::mono(self.state.input_summary()).text_color(p.toner_2).flex_shrink_0());
        let Some(tool) = tool else {
            return bar.child(ui::grow()).child(ui::button("run", Btn::Primary, 32., false, &p).child("Run").child(ui::kbd(&keys, &p, KeyOn::Muted))).into_any_element();
        };

        bar = bar.child(ui::icon("arrow-right", 12., p.toner_4)).child(ui::row().gap(px(6.)).flex_shrink_0().text_color(p.toner_3).child(ui::icon("folder", 12., p.toner_4)).child(ui::mono("you choose where to save")));
        let note = match tool.id {
            "redact" | "pages" | "edit" | "sign" => Some("saves a copy, original untouched"),
            _ => None,
        };
        if let (Some(note), false) = (note, compact) {
            bar = bar.child(ui::mono(note).text_color(p.toner_3).flex_shrink_0());
        }

        let ready = !self.state.run_set().is_empty() || matches!(tool.inputs, Inputs::Optional);
        let count = self.state.run_set().len();
        let label = match tool.inputs {
            Inputs::Each if count > 1 => format!("{} {count} files", tool.verb),
            Inputs::Combine if count > 1 => format!("{} {count} files", tool.verb),
            _ => tool.verb.to_string(),
        };
        let has_results = self.state.files.iter().any(|f| matches!(f.status, RowStatus::Done { .. }));
        bar = bar.child(ui::grow());
        if has_results {
            bar = bar.child(ui::button("show", Btn::Ghost, 32., true, &p).child(ui::icon("folder", 14., p.toner_2)).child("Show in folder").on_click(cx.listener(|this, _, window, cx| this.run_command(crate::palette_model::Command::ShowResult, window, cx))));
        }
        let kind = if tool.destructive { Btn::Danger } else { Btn::Primary };
        let key_on = if !ready { KeyOn::Muted } else if tool.destructive { KeyOn::Danger } else { KeyOn::Accent };
        let mut run = ui::button("run", kind, 32., ready, &p).child(SharedString::from(label)).child(ui::kbd(&keys, &p, key_on));
        if ready {
            run = run.on_click(cx.listener(|this, _, window, cx| this.run_current(window, cx)));
        }
        bar.child(run).into_any_element()
    }
}

fn tool_index(tool: &'static Tool) -> usize {
    catalog::TOOLS.iter().position(|t| t.id == tool.id).unwrap_or(0)
}
