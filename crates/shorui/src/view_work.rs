//! The workspace: Home, and the file queue most tools use.

use crate::catalog::{self, Inputs, Tool, Workspace};
use crate::palette_model::{Command, Entry, Section, describe, entries, mod_label};
use crate::settings::human_size;
use crate::shell::Shell;
use crate::state::{FileEntry, RowStatus};
use crate::tokens::*;
use crate::ui::{self, Btn, KeyOn, Tone, pal};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Payload while a queue row is being dragged.
#[derive(Clone)]
pub struct DragRow {
    pub index: usize,
    pub name: SharedString,
}

/// What follows the pointer during a drag.
pub struct DragPreview {
    name: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        ui::row().h(px(32.)).px(px(10.)).gap(px(8.)).rounded(px(R_MD)).border_1().border_color(p.rule_strong).bg(p.plate_2).text_color(p.toner).font_family(FONT_UI).text_size(px(TEXT)).child(ui::icon("file", 14., p.toner_2)).child(self.name.clone())
    }
}

fn drop_zone(p: &Palette) -> Div {
    div().flex().flex_col().items_center().justify_center().gap(px(18.)).rounded(px(R_LG)).border_1().border_dashed().border_color(p.rule_strong).bg(p.well)
}

impl Shell {
    pub fn render_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.state.tool() {
            None => self.render_home(cx).into_any_element(),
            Some(tool) => match tool.workspace {
                Workspace::Queue => self.render_queue(tool, window, cx).into_any_element(),
                Workspace::Grid => self.render_grid(tool, window, cx).into_any_element(),
                Workspace::Canvas => self.render_canvas(tool, window, cx).into_any_element(),
            },
        }
    }

    /// The launcher: one search box that is the command palette, then favourites and the
    /// tools worth reaching for next.
    fn render_home(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let situation = self.situation();
        self.home_search.update(cx, |search, cx| search.set_situation(situation.clone(), cx));
        let searching = !self.home_search.read(cx).query(cx).trim().is_empty();
        let loaded = situation.files.len();
        let note = match loaded {
            0 => "Just start typing a task or a tool. Files can be dropped anywhere in this window.".to_string(),
            1 => "1 file loaded. Pick a tool for it.".to_string(),
            n => format!("{n} files loaded. Pick a tool for them."),
        };

        let mut page = ui::col()
            .w(px(720.))
            .max_w_full()
            .pt(px(104.))
            .pb(px(48.))
            .gap(px(24.))
            .child(ui::col().gap(px(6.)).child(div().text_size(px(22.)).line_height(px(28.)).font_weight(FontWeight::SEMIBOLD).child("What do you need to do?")).child(div().text_color(p.toner_3).child(SharedString::from(note))))
            .child(self.home_search.clone());
        if !searching {
            if let Some(favorites) = self.home_favorites(&p, cx) {
                page = page.child(favorites);
            }
            let rows = entries("", &situation);
            let recent: Vec<&Entry> = rows.iter().filter(|e| e.section == Section::Recent).collect();
            let suggested: Vec<&Entry> = if loaded > 0 { rows.iter().filter(|e| e.section == Section::Suggested).collect() } else { Vec::new() };
            let recent = self.home_list("home-recent", "Recent tools".into(), &recent, &p, cx);
            page = page.child(match situation.files.first().and_then(|f| f.file_name()).filter(|_| !suggested.is_empty()) {
                Some(name) => {
                    let title = format!("Suggested for {}", name.to_string_lossy());
                    div().grid().grid_cols(2).gap(px(32.)).child(recent).child(self.home_list("home-suggested", title, &suggested, &p, cx))
                }
                None => recent,
            });
        }
        div().id("home").size_full().overflow_y_scroll().child(div().flex().justify_center().px(px(16.)).child(page))
    }

    fn home_favorites(&self, p: &Palette, cx: &mut Context<Self>) -> Option<Div> {
        let p = *p;
        let favorites: Vec<(usize, &'static Tool)> = self.settings.favorites.iter().enumerate().filter_map(|(slot, id)| catalog::tool(id).map(|tool| (slot, tool))).collect();
        if favorites.is_empty() {
            return None;
        }
        let mut chips = ui::row().flex_wrap().gap(px(8.));
        for (slot, tool) in favorites {
            let id = tool.id;
            // Ctrl 1 to 3 open the first three slots, so only those carry key caps.
            let digit = (slot < 3).then(|| (slot + 1).to_string());
            chips = chips.child(
                div()
                    .id(("home-favorite", slot))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .pl(px(10.))
                    .pr(px(if digit.is_some() { 5. } else { 10. }))
                    .rounded(px(R_SM))
                    .border_1()
                    .border_color(p.control_rule)
                    .cursor_pointer()
                    .tab_index(0)
                    .font_weight(FontWeight::MEDIUM)
                    .hover(move |s| s.bg(p.plate))
                    .active(move |s| s.bg(p.plate_2))
                    .focus_visible(move |s| ui::ring(s, &p))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_tool(id, cx)))
                    .child(ui::dot(p.group(tool.group), 6.))
                    .child(tool.name)
                    .when_some(digit, |s, digit| s.child(ui::kbd(&[mod_label(), digit.as_str()], &p, KeyOn::Surface))),
            );
        }
        Some(ui::col().gap(px(8.)).child(ui::section_label("Favorites", &p)).child(chips))
    }

    /// A titled list of palette entries as 36px rows: dot, name, what the tool does, keys.
    fn home_list(&self, id: &'static str, title: String, rows: &[&Entry], p: &Palette, cx: &mut Context<Self>) -> Div {
        let p = *p;
        let mut list = ui::col().min_w_0().gap(px(2.)).child(ui::row().h(px(20.)).mb(px(4.)).child(ui::section_label(title, &p)));
        for (ix, entry) in rows.iter().enumerate() {
            let command = entry.command.clone();
            let detail = match entry.command {
                Command::OpenTool(tool) => describe(tool),
                _ => "",
            };
            list = list.child(
                div()
                    .id((id, ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .h(px(36.))
                    .px(px(10.))
                    .rounded(px(R_SM))
                    .cursor_pointer()
                    .tab_index(0)
                    .hover(move |s| s.bg(p.plate))
                    .active(move |s| s.bg(p.plate_2))
                    .focus_visible(move |s| ui::ring(s, &p))
                    .on_click(cx.listener(move |this, _, window, cx| this.run_command(command.clone(), window, cx)))
                    .when_some(entry.group, |s, g| s.child(ui::dot(p.group(g), 6.)))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).child(SharedString::from(entry.title.clone())))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(TEXT_SM)).text_color(p.toner_3).child(detail))
                    .when(!entry.keys.is_empty(), |s| s.child(ui::kbd(&entry.keys, &p, KeyOn::Surface))),
            );
        }
        list
    }

    /// A queue header: title, count and actions.
    fn queue_head(&self, tool: &'static Tool, count: usize, p: &Palette, cx: &mut Context<Self>) -> Div {
        let running = self.state.running;
        let mut head = ui::row().h(px(TOPBAR_H)).pl(px(16.)).pr(px(12.)).gap(px(10.)).flex_shrink_0().child(div().font_weight(FontWeight::SEMIBOLD).child("Queue")).child(ui::mono(format!("{count} {}", if count == 1 { "file" } else { "files" })).text_color(p.toner_3));
        if !running {
            head = head.child(ui::button("add-files", Btn::Default, 24., true, p).child(ui::icon("plus", 14., p.toner)).child("Add files").on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))));
        }
        head = head.child(ui::grow());
        if tool.id == "merge" && count > 1 {
            head = head.child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child("Drag a row to reorder"));
        }
        if tool.id == "compare" {
            head = head.child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child("Old version first, then the new one"));
        }
        if count > 0 && !running {
            head = head.child(ui::button("clear", Btn::Ghost, 24., true, p).child("Clear").on_click(cx.listener(|this, _, _, cx| {
                this.state.clear_files();
                cx.notify();
            })));
        }
        head
    }

    fn render_queue(&mut self, tool: &'static Tool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = pal(cx);
        let visible = self.state.visible();
        let ticks = matches!(tool.inputs, Inputs::Each | Inputs::Optional);
        let merge = tool.id == "merge";
        let any_status = self.state.files.iter().any(|f| !matches!(f.status, RowStatus::Idle));
        let status_w = 320.;

        if visible.is_empty() {
            let accepts = tool.accepts.iter().map(|e| e.to_uppercase()).collect::<Vec<_>>();
            let kinds = if accepts.len() > 4 { format!("{} and more", accepts[..4].join(", ")) } else { accepts.join(", ") };
            let optional = matches!(tool.inputs, Inputs::Optional);
            return ui::col()
                .size_full()
                .child(self.queue_head(tool, 0, &p, cx))
                .child(
                    div().flex_1().p(px(24.)).pt(px(0.)).child(
                        drop_zone(&p)
                            .size_full()
                            .child(ui::icon(tool.icon, 20., p.toner_3))
                            .child(ui::col().items_center().gap(px(6.)).child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(SharedString::from(format!("Drop files to {}", tool.name.to_lowercase())))).child(div().text_color(p.toner_2).child(SharedString::from(if optional { "HTML files, or type a web address in the options.".to_string() } else { format!("Accepts {kinds}.") }))))
                            .child(ui::button("browse-empty", Btn::Default, 28., true, &p).child("Browse files").child(ui::kbd(&[mod_label(), "O"], &p, KeyOn::Surface)).on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx)))),
                    ),
                )
                .into_any_element();
        }

        let all_included = visible.iter().all(|i| self.state.files[*i].included);
        let mut head = ui::row().h(px(28.)).pl(px(16.)).pr(px(12.)).gap(px(12.)).flex_shrink_0().border_t_1().border_b_1().border_color(p.rule_soft).text_size(px(TEXT_SM)).text_color(p.toner_3);
        if ticks {
            let vis = visible.clone();
            head = head.child(ui::checkbox("tick-all", all_included, &p).on_click(cx.listener(move |this, _, _, cx| {
                for &i in &vis {
                    this.state.files[i].included = !all_included;
                }
                cx.notify();
            })));
        } else {
            head = head.child(div().w(px(14.)));
        }
        head = head.child(div().w(px(20.)).child("#")).child(div().w(px(22.))).child(div().flex_1().child("File"));
        if any_status {
            head = head.child(div().w(px(status_w)).child("Status"));
        } else {
            head = head.child(div().w(px(56.)).text_right().child("Pages")).child(div().w(px(72.)).text_right().child("Size"));
            if merge {
                head = head.child(div().w(px(120.)).child("Page range"));
            }
        }
        head = head.child(div().w(px(24.)));

        let mut rows = div().id("queue-rows").flex().flex_col().flex_1().min_h_0().overflow_y_scroll();
        for (n, &i) in visible.iter().enumerate() {
            rows = rows.child(self.queue_row(tool, n, i, any_status, status_w, &p, _window, cx));
        }
        rows = rows.child(ui::row().h(px(40.)).px(px(16.)).gap(px(8.)).flex_shrink_0().text_size(px(TEXT_SM)).text_color(p.toner_3).child(ui::icon("plus", 12., p.toner_3)).child("Drop more files anywhere in this window"));

        let strip = if merge { Some(self.merge_strip(&visible, &p, cx)) } else { None };
        ui::col().size_full().child(self.queue_head(tool, visible.len(), &p, cx)).child(head).child(rows).when_some(strip, |s, strip| s.child(strip)).into_any_element()
    }

    /// The pages of every queued file in the order they will be merged.
    fn merge_strip(&mut self, visible: &[usize], p: &Palette, cx: &mut Context<Self>) -> Div {
        const W: f32 = 60.;
        const H: f32 = 82.;
        let p = *p;
        let mut groups = ui::row().gap(px(20.)).items_start().overflow_hidden();
        let mut total = 0usize;
        for (n, &i) in visible.iter().enumerate() {
            let file = self.state.files[i].clone();
            let pages = file.pages.unwrap_or(0);
            let chosen: Vec<usize> = shorui_core::range::parse(&file.range, pages).unwrap_or_else(|_| (1..=pages).collect());
            let show = chosen.len().min(4);
            let batch: Vec<usize> = chosen.iter().take(show).map(|pg| pg - 1).collect();
            let mut thumbs = ui::row().gap(px(8.)).items_start();
            for (k, pg) in chosen.iter().take(show).enumerate() {
                let sheet = match self.page_image(&file.path, pg - 1, 256, &batch, cx) {
                    Some(image) => div().w(px(W)).h(px(H)).flex_shrink_0().rounded(px(2.)).bg(p.stock).border_1().border_color(p.stock_edge).overflow_hidden().child(img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Contain)),
                    None => ui::paper_blank(W, H, &p),
                };
                thumbs = thumbs.child(ui::col().gap(px(4.)).items_center().child(sheet).child(ui::mono((total + k + 1).to_string()).text_size(px(10.)).text_color(p.toner_3)));
            }
            if chosen.len() > show {
                let rest = chosen.len() - show;
                thumbs = thumbs.child(
                    ui::col().gap(px(4.)).items_center().child(div().w(px(W)).h(px(H)).flex_shrink_0().flex().items_center().justify_center().rounded(px(2.)).border_1().border_dashed().border_color(p.rule_strong).child(ui::mono(format!("+{rest}")).text_color(p.toner_2))).child(ui::mono(format!("{}-{}", total + show + 1, total + chosen.len())).text_size(px(10.)).text_color(p.toner_3)),
                );
            }
            groups = groups.child(
                ui::col()
                    .gap(px(6.))
                    .flex_shrink_0()
                    .child(ui::row().gap(px(6.)).h(px(18.)).child(ui::mono(format!("{:02}", n + 1)).text_size(px(TEXT_XS)).text_color(p.toner_2)).child(div().max_w(px(180.)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(TEXT_SM)).text_color(p.toner_3).child(SharedString::from(file.name.clone()))))
                    .child(thumbs),
            );
            total += chosen.len();
        }
        ui::col().flex_shrink_0().border_t_1().border_color(p.rule).child(ui::row().h(px(36.)).px(px(16.)).gap(px(10.)).child(div().font_weight(FontWeight::SEMIBOLD).child("Combined order")).child(ui::mono(format!("{total} {}", if total == 1 { "page" } else { "pages" })).text_color(p.toner_3))).child(div().id("merge-strip").px(px(16.)).pt(px(4.)).pb(px(14.)).overflow_x_scroll().child(groups))
    }

    #[allow(clippy::too_many_arguments)]
    fn queue_row(&mut self, tool: &'static Tool, n: usize, i: usize, any_status: bool, status_w: f32, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *p;
        let file: FileEntry = self.state.files[i].clone();
        let ticks = matches!(tool.inputs, Inputs::Each | Inputs::Optional);
        let merge = tool.id == "merge";
        let running = self.state.running;
        let failed = matches!(file.status, RowStatus::Failed { .. });

        let mut row = div()
            .id(("row", i))
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .gap(px(12.))
            .min_h(px(ROW_H))
            .pl(px(16.))
            .pr(px(12.))
            .border_b_1()
            .border_color(p.rule_soft)
            .hover(move |s| s.bg(p.plate));
        if failed {
            row = row.py(px(10.)).items_start();
        }

        // Leading control: tick box for batch tools, drag handle for merge.
        if ticks {
            row = row.child(ui::checkbox(("tick", i), file.included, &p).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(f) = this.state.files.get_mut(i) {
                    f.included = !f.included;
                    cx.notify();
                }
            })));
        } else {
            row = row.child(div().w(px(14.)).flex_shrink_0().when(merge, |s| s.cursor_grab().child(ui::icon("grip", 14., p.toner_4))));
        }
        if merge || tool.id == "compare" || tool.id == "img2pdf" {
            let name: SharedString = file.name.clone().into();
            row = row
                .on_drag(DragRow { index: i, name: name.clone() }, |drag: &DragRow, _, _, cx| cx.new(|_| DragPreview { name: drag.name.clone() }))
                .drag_over::<DragRow>(move |s, _, _, _| s.border_t_2().border_color(p.cyan))
                .on_drop(cx.listener(move |this, drag: &DragRow, _, cx| {
                    this.state.move_file(drag.index, i);
                    cx.notify();
                }));
        }

        let dim = if file.included || !ticks { p.toner } else { p.toner_3 };
        row = row
            .child(ui::mono(format!("{:02}", n + 1)).w(px(20.)).flex_shrink_0().text_color(p.toner_3))
            .child(self.thumb_small(&file, &p, cx))
            .child(
                ui::col().flex_1().min_w_0().gap(px(2.)).child(ui::row().gap(px(8.)).min_w_0().child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().font_weight(FontWeight::MEDIUM).text_color(dim).child(SharedString::from(file.name.clone()))).when(file.locked, |s| s.child(ui::chip("Locked", Tone::Neutral, &p))).when(failed, |s| s.child(ui::chip("Failed", Tone::Danger, &p)))).when_some(
                    match (&file.status, &file.problem) {
                        (RowStatus::Failed { message }, _) => Some(message.clone()),
                        (_, Some(problem)) => Some(problem.clone()),
                        _ => None,
                    },
                    |s, text| s.child(div().text_size(px(TEXT_SM)).text_color(p.toner_2).child(SharedString::from(text))),
                ),
            );

        if any_status {
            row = row.child(div().w(px(status_w)).flex_shrink_0().child(self.status_cell(tool, &file, &p)));
        } else {
            row = row
                .child(ui::mono(file.pages.map(|n| n.to_string()).unwrap_or_else(|| if file.is_pdf() { "…".into() } else { "".into() })).w(px(56.)).flex_shrink_0().text_right().text_color(p.toner_2))
                .child(ui::mono(human_size(file.bytes)).w(px(72.)).flex_shrink_0().text_right().text_color(p.toner_2));
            if merge {
                row = row.child(div().w(px(120.)).flex_shrink_0().child(self.text_field(&format!("range:{}", file.path.display()), "all", &file.range, true, crate::view_panel::InputTarget::Range { path: file.path.clone() }, window, cx)));
            }
        }

        if (failed || file.problem.is_some()) && tool.id != "repair" && file.is_pdf() {
            row = row.child(ui::button(("repair", i), Btn::Default, 24., !running, &p).child(ui::icon("repair", 14., p.toner)).child("Open in Repair").on_click(cx.listener(|this, _, _, cx| this.open_tool("repair", cx))));
        }
        if let RowStatus::Done { outputs, .. } = &file.status {
            if let Some(first) = outputs.first().cloned() {
                row = row.child(ui::button(("open-result", i), Btn::Ghost, 24., true, &p).child("Open").on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&first))));
            }
        }
        if running {
            row = row.child(div().w(px(24.)).flex_shrink_0());
        } else {
            row = row.child(ui::icon_button(("remove", i), "x", 24., &p).on_click(cx.listener(move |this, _, _, cx| {
                this.state.remove_file(i);
                cx.notify();
            })));
        }
        row
    }

    fn status_cell(&self, tool: &'static Tool, file: &FileEntry, p: &Palette) -> Div {
        match &file.status {
            RowStatus::Idle => div(),
            RowStatus::Queued => ui::row().gap(px(8.)).child(ui::icon("circle", 14., p.toner_4)).child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child("Queued")),
            RowStatus::Running { fraction, what } => ui::col().gap(px(7.)).child(ui::row().justify_between().gap(px(8.)).text_size(px(TEXT_SM)).child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(p.toner_2).child(SharedString::from(what.clone()))).child(ui::mono(format!("{:.0}%", fraction * 100.)))).child(ui::progress(*fraction, p.cyan, p)),
            RowStatus::Done { bytes_out, pages, outputs, notes } => {
                let mut line = ui::row().gap(px(8.)).child(ui::icon("check", 14., p.proof));
                if tool.id == "compress" || (*bytes_out > 0 && outputs.len() == 1 && file.is_pdf()) {
                    let change = if file.bytes > 0 { (*bytes_out as f64 / file.bytes as f64 - 1.0) * 100.0 } else { 0.0 };
                    line = line.child(ui::mono(human_size(file.bytes)).text_color(p.toner_3)).child(ui::icon("arrow-right", 10., p.toner_4)).child(ui::mono(human_size(*bytes_out)));
                    if change.abs() >= 1.0 {
                        line = line.child(ui::mono(format!("{}{:.0}%", if change < 0.0 { "−" } else { "+" }, change.abs())).text_color(p.toner_3));
                    }
                } else if outputs.len() > 1 {
                    line = line.child(ui::mono(format!("{} files", outputs.len())).text_color(p.toner_2));
                } else if *pages > 0 {
                    line = line.child(ui::mono(format!("{pages} {}", if *pages == 1 { "page" } else { "pages" })).text_color(p.toner_2));
                } else {
                    line = line.child(div().text_size(px(TEXT_SM)).text_color(p.toner_2).child("Done"));
                }
                let mut cell = ui::col().gap(px(2.)).py(px(6.)).child(line);
                if let Some(note) = notes.first() {
                    cell = cell.child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child(SharedString::from(note.clone())));
                }
                cell
            }
            RowStatus::Failed { .. } => ui::row().gap(px(8.)).pt(px(6.)).child(ui::icon("alert", 14., p.stamp)).child(div().text_size(px(TEXT_SM)).text_color(p.toner_3).child("Nothing was written")),
        }
    }
}
