//! The keyboard paths that must never break, exercised in a headless window with real
//! key dispatch: the command palette, mode switching and the `G` chords.

use crate::palette_model::Command;
use crate::settings::Settings;
use crate::shell::Shell;
use crate::{palette, shell, view_special};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowOptions, px, size};
use std::path::PathBuf;
use std::time::Instant;

/// The palette shortcut on this platform.
fn open_key() -> &'static str {
    if cfg!(target_os = "macos") { "cmd-k" } else { "ctrl-k" }
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Shell>) {
    let (window, shell) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::apply_theme(true, cx);
        shell::bind_keys(cx);
        palette::bind_keys(cx);
        view_special::bind_keys(cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds { origin: Point::default(), size: size(px(1440.), px(900.)) })), ..Default::default() };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(Settings::default(), Instant::now(), window, cx))).expect("window")
    });
    cx.update_window(window, |_, window, cx| window.render_frame(cx)).unwrap();
    (window, shell)
}

/// The app with a tool open, where the shortcut opens the palette as an overlay.
fn open_off_home(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Shell>) {
    let (window, shell) = open(cx);
    cx.update_window(window, |_, window, cx| {
        shell.update(cx, |shell, cx| shell.open_tool("split", cx));
        window.render_frame(cx);
    })
    .unwrap();
    (window, shell)
}

fn titles(shell: &Entity<Shell>, cx: &gpui_kit::App) -> Vec<String> {
    shell.read(cx).palette.read(cx).entries().iter().map(|e| e.title.clone()).collect()
}

#[gpui_kit::test]
fn shortcut_opens_the_palette_and_enter_switches_mode(cx: &mut TestAppContext) {
    let (window, shell) = open_off_home(cx);
    cx.update_window(window, |_, window, cx| {
        assert!(!shell.read(cx).palette_open);
        window.press(open_key(), cx);
        assert!(shell.read(cx).palette_open, "the shortcut opens the palette");
        window.input("comp", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let rows = titles(&shell, cx);
        assert_eq!(rows[0], "Compress");
        assert_eq!(rows[1], "Compare");
        assert_eq!(shell.read(cx).palette.read(cx).query(cx), "comp");
        assert_eq!(shell.read(cx).palette.read(cx).selected(), 0);
        window.press("down", cx);
        assert_eq!(shell.read(cx).palette.read(cx).selected(), 1, "down moves the selection while the text field has focus");
        window.press("up", cx);
        assert_eq!(shell.read(cx).palette.read(cx).selected(), 0);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(!shell.read(cx).palette_open, "enter closes the palette");
        assert_eq!(shell.read(cx).state.mode, Some("compress"));
    });
}

#[gpui_kit::test]
fn arrows_wrap_and_escape_closes_and_returns_focus(cx: &mut TestAppContext) {
    let (window, shell) = open_off_home(cx);
    cx.update_window(window, |_, window, cx| {
        window.press(open_key(), cx);
        let count = shell.read(cx).palette.read(cx).entries().len();
        assert!(count >= 3, "an empty query lists recent modes and suggestions");
        window.press("up", cx);
        assert_eq!(shell.read(cx).palette.read(cx).selected(), count - 1, "up from the first row wraps to the last");
        window.press("down", cx);
        assert_eq!(shell.read(cx).palette.read(cx).selected(), 0);
        window.press("escape", cx);
    })
    .unwrap();
    // The palette reports "dismissed" as an event; it lands once the key press has been handled.
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(!shell.read(cx).palette_open);
        assert!(shell.read(cx).focus.is_focused(window), "focus goes back to the app");
        assert_eq!(shell.read(cx).state.mode, Some("split"), "escape changes nothing");
        // And it opens again straight away.
        window.press(open_key(), cx);
        assert!(shell.read(cx).palette_open);
        window.press(open_key(), cx);
        assert!(!shell.read(cx).palette_open, "the same shortcut closes it");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_palette_opens_with_a_fresh_query_each_time(cx: &mut TestAppContext) {
    let (window, shell) = open_off_home(cx);
    cx.update_window(window, |_, window, cx| {
        window.press(open_key(), cx);
        window.input("merge", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(titles(&shell, cx)[0], "Merge");
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press(open_key(), cx);
        assert_eq!(shell.read(cx).palette.read(cx).query(cx), "", "the old query is gone");
        assert!(titles(&shell, cx).len() >= 3);
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_shortcut_works_while_typing_in_an_options_field(cx: &mut TestAppContext) {
    let (window, shell) = open(cx);
    cx.update_window(window, |_, window, cx| {
        shell.update(cx, |shell, cx| shell.open_tool("merge", cx));
        window.render_frame(cx);
        // Put the keyboard in the Title field of the Merge options.
        let field = shell.read(cx).inputs.get("opt:merge:title").expect("the title field exists").state.clone();
        field.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
        window.input("gm report", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let field = shell.read(cx).inputs.get("opt:merge:title").unwrap().state.clone();
        assert_eq!(field.read(cx).value().to_string(), "gm report", "letters typed in a field are text, not tool shortcuts");
        assert_eq!(shell.read(cx).state.mode, Some("merge"));
        window.press(open_key(), cx);
        assert!(shell.read(cx).palette_open, "the palette opens from inside a text field");
        window.input("redact", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(shell.read(cx).state.mode, Some("redact")));
}

#[gpui_kit::test]
fn g_then_a_letter_switches_tool(cx: &mut TestAppContext) {
    let (window, shell) = open(cx);
    cx.update_window(window, |_, window, cx| {
        window.press("g", cx);
        window.press("m", cx);
        assert_eq!(shell.read(cx).state.mode, Some("merge"));
        window.press("g", cx);
        window.press("c", cx);
        assert_eq!(shell.read(cx).state.mode, Some("compress"));
        // A stray letter without the G does nothing.
        window.press("r", cx);
        assert_eq!(shell.read(cx).state.mode, Some("compress"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn switching_mode_keeps_the_loaded_files(cx: &mut TestAppContext) {
    let (window, shell) = open(cx);
    cx.update_window(window, |_, window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_tool("merge", cx);
            shell.add_files(vec![PathBuf::from("a.pdf"), PathBuf::from("b.pdf"), PathBuf::from("photo.png")], cx);
        });
        window.render_frame(cx);
        window.press(open_key(), cx);
        // With a file open, the empty palette suggests actions for it.
        assert!(titles(&shell, cx).iter().any(|t| t == "Compress a.pdf"), "{:?}", titles(&shell, cx));
        window.input("compress", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let s = &shell.read(cx).state;
        assert_eq!(s.mode, Some("compress"));
        assert_eq!(s.files.len(), 3, "nothing was dropped");
        assert_eq!(s.visible().len(), 2, "Compress shows the two PDFs");
    });
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.open_tool("img2pdf", cx))).unwrap();
    cx.update(|cx| assert_eq!(shell.read(cx).state.visible().len(), 1, "Images to PDF shows the image"));
}

#[gpui_kit::test]
fn palette_actions_run(cx: &mut TestAppContext) {
    let (window, shell) = open_off_home(cx);
    cx.update_window(window, |_, window, cx| {
        window.press(open_key(), cx);
        window.input("light theme", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let palette = shell.read(cx).palette.read(cx);
        assert_eq!(palette.entries()[0].command, Command::ToggleTheme);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(!shell.read(cx).settings.dark);
        assert!(!crate::ui::pal(cx).dark, "the palette of colours switched too");
    });
}

// ---------------------------------------------------------------------------
// The canvas and grid tools, end to end: open a file, act on the page, run, read the result.
// ---------------------------------------------------------------------------

use crate::state::RowStatus;
use crate::tool_runs::fixtures_dir;
use gpui_kit::{Pixels, point};
use shorui_core::text::TextReader;
use std::time::Duration;

fn open_with(tool: &'static str, file: &str, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Shell>, PathBuf) {
    let (window, shell) = open(cx);
    // One folder per test: several tests use the same tool at the same time.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("shorui-ui-out-{}-{tool}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let path = fixtures_dir().join(file);
    cx.update_window(window, |_, _, cx| {
        shell.update(cx, |shell, cx| {
            shell.settings.save_dir = Some(out.clone());
            shell.open_tool(tool, cx);
            shell.add_files(vec![path], cx);
        })
    })
    .unwrap();
    settle(window, cx);
    (window, shell, out)
}

/// Let background reads (page counts, page images, marks, form fields) finish and repaint.
fn settle(window: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx)).unwrap();
    }
}

/// Start the run and wait for the job thread to finish. Returns the outputs of the active file.
fn run_and_wait(window: AnyWindowHandle, shell: &Entity<Shell>, cx: &mut TestAppContext) -> Vec<PathBuf> {
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    answer_save_dialog(&shell, "saved.pdf", cx);
    for _ in 0..1500 {
        if !cx.update(|cx| shell.read(cx).state.running) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
    }
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert!(!shell.state.running, "the run did not finish");
        match &shell.state.files[shell.state.active_file].status {
            RowStatus::Done { outputs, .. } => outputs.clone(),
            other => panic!("the run ended as {other:?}; toast: {:?}", shell.toast.as_ref().map(|t| (&t.title, &t.body))),
        }
    })
}

/// Let a started run finish: its progress is pumped on a timer, so the clock has to move.
fn wait_for_run(shell: &Entity<Shell>, cx: &mut TestAppContext) {
    for _ in 0..1500 {
        if !cx.update(|cx| shell.read(cx).state.running) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
    }
    panic!("the run did not finish");
}

/// Answer the save dialog a run opens: a file name in the folder the dialog starts in, or
/// that folder itself when the run asks for a folder.
fn answer_save_dialog(shell: &Entity<Shell>, name: &str, cx: &mut TestAppContext) {
    cx.run_until_parked();
    let dir = cx.update(|cx| shell.read(cx).settings.save_dir.clone()).expect("the test sets a folder");
    if cx.did_prompt_for_new_path() {
        let name = name.to_string();
        cx.simulate_new_path_selection(move |start| Some(start.join(name)));
    } else if cx.did_prompt_for_paths() {
        cx.simulate_path_prompt_response(move |_| Some(vec![dir]));
    }
    cx.run_until_parked();
}

fn page_text(path: &std::path::Path) -> Vec<String> {
    let reader = TextReader::open_path(path, None).expect("output opens");
    (0..reader.page_count()).map(|i| reader.page(i).expect("text").plain()).collect()
}

/// The offset inside the page sheet of a point given in page points.
fn sheet_offset(shell: &Entity<Shell>, x: f32, y: f32, cx: &gpui_kit::App) -> gpui_kit::Point<Pixels> {
    let canvas = &shell.read(cx).canvas;
    let bounds = canvas.bounds.get();
    let (pw, ph) = canvas.page_size.expect("the page size is known");
    point(bounds.size.width * (x / pw), bounds.size.height * (y / ph))
}

#[gpui_kit::test]
fn redact_search_marks_then_removes_the_text(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("redact", "contacts.pdf", cx);
    cx.update_window(window, |_, _, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_redact_search("Mara Lindqvist", cx);
            shell.state.set_option("redact", "patterns", serde_json::json!(["email"]));
        })
    })
    .unwrap();
    settle(window, cx);
    cx.update_window(window, |_, _, cx| {
        shell.update(cx, |shell, _| {
            assert_eq!(shell.canvas.found.iter().filter(|m| m.source == "search").count(), 3);
            assert_eq!(shell.active_marks().len(), 5, "three names and two email addresses");
            assert_eq!(shell.chip_count(crate::catalog::tool("redact").unwrap(), "phone"), Some(1));
        })
    })
    .unwrap();
    // Drag out an area over the address line as well.
    cx.update_window(window, |_, window, cx| {
        let from = sheet_offset(&shell, 150., 132., cx);
        let to = sheet_offset(&shell, 300., 148., cx);
        let origin = shell.read(cx).canvas.bounds.get().origin;
        window.drag(origin + from, origin + to, cx);
    })
    .unwrap();
    settle(window, cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.areas.len(), 1, "the drag made an area mark"));
    let out = run_and_wait(window, &shell, cx);
    let text = page_text(&out[0]).join("\n");
    assert!(!text.contains("Mara") && !text.contains("example.com") && !text.contains("Alder Row"), "{text}");
}

#[gpui_kit::test]
fn redact_refuses_to_run_with_nothing_marked(cx: &mut TestAppContext) {
    let (window, shell, out) = open_with("redact", "contacts.pdf", cx);
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert!(!shell.state.running);
        assert_eq!(shell.toast.as_ref().map(|t| t.title.as_str()), Some("Not ready yet"));
    });
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing was written");
}

#[gpui_kit::test]
fn sign_places_where_clicked_and_saves(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("sign", "form.pdf", cx);
    cx.update_window(window, |_, _, cx| {
        shell.update(cx, |shell, cx| {
            shell.state.set_option("sign", "typed", serde_json::json!("Priya Raman"));
            shell.options_changed("sign", cx);
        })
    })
    .unwrap();
    settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        assert!(shell.read(cx).canvas.placed.is_none());
        let at = sheet_offset(&shell, 180., 310., cx);
        window.click_at("page-sheet", at, cx);
    })
    .unwrap();
    settle(window, cx);
    let (page, x, y) = cx.update(|cx| shell.read(cx).canvas.placed.expect("the click placed the signature"));
    assert_eq!(page, 0);
    // The signature is centred on the click.
    let (w, h) = cx.update(|cx| shell.read(cx).canvas.sign_size.unwrap());
    assert!((x + w / 2. - 180.).abs() < 2. && (y + h / 2. - 310.).abs() < 2., "{x},{y} size {w}x{h}");
    let out = run_and_wait(window, &shell, cx);
    assert!(page_text(&out[0])[0].contains("Priya Raman"));
}

/// Click a point of the page, given in page points.
fn click_page(window: AnyWindowHandle, shell: &Entity<Shell>, x: f32, y: f32, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        let at = sheet_offset(shell, x, y, cx);
        window.click_at("page-sheet", at, cx);
    })
    .unwrap();
    settle(window, cx);
}

fn type_text(window: AnyWindowHandle, text: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.input(text, cx)).unwrap();
    settle(window, cx);
}

fn press(window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| window.press(key, cx)).unwrap();
    settle(window, cx);
}

fn undo_key() -> &'static str {
    if cfg!(target_os = "macos") { "cmd-z" } else { "ctrl-z" }
}

#[gpui_kit::test]
fn edit_fills_fields_and_types_on_the_page(cx: &mut TestAppContext) {
    use crate::view_canvas::{EditTool, Editing};
    let (window, shell, _) = open_with("edit", "form.pdf", cx);
    let (agree, name) = cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.fields.len(), 3, "the form's fields were read");
        assert_eq!(edit.tool, EditTool::Text, "the text tool is ready from the start");
        let rect = |n: &str| edit.fields.iter().find(|f| f.name == n).unwrap().rect;
        (rect("agree"), rect("full_name"))
    });

    // A click on the check box ticks it; nothing is placed.
    click_page(window, &shell, (agree.x0 + agree.x1) / 2., (agree.y0 + agree.y1) / 2., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.values.get("agree").map(String::as_str), Some("true"));
        assert!(edit.items.is_empty() && edit.editing.is_none());
    });

    // A click on a text field opens it for typing where it sits.
    click_page(window, &shell, name.x0 + 20., (name.y0 + name.y1) / 2., cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.editing, Some(Editing::Field("full_name".into()))));
    type_text(window, "Priya Raman", cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.values.get("full_name").map(String::as_str), Some("Priya Raman"), "typing goes straight into the field"));
    press(window, "enter", cx);
    cx.update_window(window, |_, window, cx| {
        let shell = shell.read(cx);
        assert!(shell.canvas.edit.editing.is_none(), "enter finishes typing");
        assert!(shell.focus.is_focused(window), "and the keyboard goes back to the app");
    })
    .unwrap();

    // A click on empty paper starts a line of text right there.
    click_page(window, &shell, 60., 420., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.editing, Some(Editing::Item(0)));
        assert_eq!(edit.items.len(), 1);
    });
    type_text(window, "Collected by hand", cx);
    // Escape also keeps what was typed.
    press(window, "escape", cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert!(edit.editing.is_none());
        assert_eq!(edit.items[0].text, "Collected by hand");
        assert!((edit.items[0].rect.x0 - 60.).abs() < 1.5, "{:?}", edit.items[0].rect);
        assert!(edit.items[0].rect.width() > 60., "the box follows the text: {:?}", edit.items[0].rect);
    });

    // A click on the text opens it again, with the caret at the end.
    click_page(window, &shell, 70., 420., cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.editing, Some(Editing::Item(0))));
    type_text(window, " today", cx);
    press(window, "enter", cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.items.len(), 1);
        assert_eq!(edit.items[0].text, "Collected by hand today");
    });

    // A tick from the bar above the page.
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.set_edit_tool(EditTool::Check, cx))).unwrap();
    click_page(window, &shell, 300., 300., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.items.len(), 2);
        let r = edit.items[1].rect;
        assert!(((r.x0 + r.x1) / 2. - 300.).abs() < 1.5 && ((r.y0 + r.y1) / 2. - 300.).abs() < 1.5, "the tick is centred on the click: {r:?}");
        assert_eq!(edit.selected, None, "a new mark is not selected, so its handle stays out of the way");
    });

    let out = run_and_wait(window, &shell, cx);
    let fields = shorui_core::tools::edit::list_fields(&out[0], None).unwrap();
    assert_eq!(fields.iter().find(|f| f.name == "full_name").unwrap().value, "Priya Raman");
    assert_eq!(fields.iter().find(|f| f.name == "agree").unwrap().value, "true");
    let reader = TextReader::open_path(&out[0], None).unwrap();
    let page = reader.page(0).unwrap();
    let line = page.lines.iter().find(|l| l.text().contains("Collected by hand today")).expect("the typed line is in the file");
    assert!((line.rect.x0 - 60.).abs() < 2., "the text starts where it was clicked: {:?}", line.rect);
    assert!(line.rect.y0 < 420. && line.rect.y1 > 420., "and sits on the clicked height: {:?}", line.rect);
}

#[gpui_kit::test]
fn edit_items_move_delete_and_undo(cx: &mut TestAppContext) {
    use crate::view_canvas::EditTool;
    let (window, shell, out) = open_with("edit", "report.pdf", cx);
    cx.update(|cx| assert!(shell.read(cx).canvas.edit.fields.is_empty()));

    // A click with the text tool and nothing typed leaves nothing behind.
    click_page(window, &shell, 100., 500., cx);
    press(window, "enter", cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert!(edit.items.is_empty() && edit.history.is_empty() && edit.editing.is_none());
    });
    // And there is nothing to save yet.
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    cx.update(|cx| assert_eq!(shell.read(cx).toast.as_ref().map(|t| t.title.as_str()), Some("Not ready yet")));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing was written");

    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.set_edit_tool(EditTool::Cross, cx))).unwrap();
    click_page(window, &shell, 200., 300., cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.items.len(), 1));
    // A click on the cross picks it up rather than adding another.
    click_page(window, &shell, 201., 301., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.items.len(), 1);
        assert_eq!(edit.selected, Some(0));
    });

    // Drag it somewhere else.
    cx.update_window(window, |_, window, cx| {
        let origin = shell.read(cx).canvas.bounds.get().origin;
        let (from, to) = (sheet_offset(&shell, 200., 300., cx), sheet_offset(&shell, 260., 340., cx));
        window.drag(origin + from, origin + to, cx);
    })
    .unwrap();
    settle(window, cx);
    cx.update(|cx| {
        let r = shell.read(cx).canvas.edit.items[0].rect;
        assert!(((r.x0 + r.x1) / 2. - 260.).abs() < 2. && ((r.y0 + r.y1) / 2. - 340.).abs() < 2., "the cross followed the pointer: {r:?}");
    });

    // A highlight is dragged out.
    cx.update_window(window, |_, window, cx| {
        shell.update(cx, |shell, cx| shell.set_edit_tool(EditTool::Highlight, cx));
        window.render_frame(cx);
        let origin = shell.read(cx).canvas.bounds.get().origin;
        let (from, to) = (sheet_offset(&shell, 56., 60., cx), sheet_offset(&shell, 250., 80., cx));
        window.drag(origin + from, origin + to, cx);
    })
    .unwrap();
    settle(window, cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.items.len(), 2);
        assert_eq!(edit.items[1].tool, EditTool::Highlight);
        assert!(edit.items[1].rect.width() > 180.);
    });
    // A click on the highlight selects it.
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.set_edit_tool(EditTool::Select, cx))).unwrap();
    click_page(window, &shell, 150., 70., cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.selected, Some(1)));

    // Delete removes what is selected; undo steps back through everything.
    press(window, "delete", cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.items.len(), 1));
    press(window, undo_key(), cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.items.len(), 2, "undo brings the highlight back"));
    press(window, undo_key(), cx);
    press(window, undo_key(), cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.items.len(), 1);
        let r = edit.items[0].rect;
        assert!(((r.x0 + r.x1) / 2. - 200.).abs() < 2., "undo puts the cross back where it was placed: {r:?}");
    });
    press(window, undo_key(), cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert!(edit.items.is_empty() && edit.history.is_empty());
    });
}

#[gpui_kit::test]
fn edit_radio_buttons_and_lists_are_picked_on_the_page(cx: &mut TestAppContext) {
    use crate::view_canvas::Editing;
    let (window, shell, _) = open_with("edit", "choices.pdf", cx);
    let (red, blue, size, first) = cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        let field = |n: &str| edit.fields.iter().find(|f| f.name == n).unwrap();
        let colour = field("colour");
        assert_eq!(colour.widgets.len(), 2, "each radio button has its own place on the page");
        (colour.widgets[0].rect, colour.widgets[1].rect, field("size").rect, field("person.first").rect)
    });
    let mid = |r: &shorui_core::text::Rect4| ((r.x0 + r.x1) / 2., (r.y0 + r.y1) / 2.);
    let colour = |cx: &mut TestAppContext| cx.update(|cx| shell.read(cx).canvas.edit.values.get("colour").cloned().unwrap_or_default());

    click_page(window, &shell, mid(&red).0, mid(&red).1, cx);
    assert_eq!(colour(cx), "red");
    click_page(window, &shell, mid(&blue).0, mid(&blue).1, cx);
    assert_eq!(colour(cx), "blue", "choosing another button moves the choice");
    click_page(window, &shell, mid(&blue).0, mid(&blue).1, cx);
    assert_eq!(colour(cx), "", "a second click on the chosen button clears the group");
    click_page(window, &shell, mid(&blue).0, mid(&blue).1, cx);

    // A list field opens its list under the field; a click on a row picks it.
    click_page(window, &shell, mid(&size).0, mid(&size).1, cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.choice_open.as_deref(), Some("size"));
        assert!(edit.editing.is_none());
    });
    cx.update_window(window, |_, window, cx| {
        let canvas = &shell.read(cx).canvas;
        let scale = f32::from(canvas.bounds.get().size.width) / canvas.page_size.unwrap().0;
        // Second row: border and padding, one 28 px row and its gap, then half a row.
        let at = point(px(size.x0 * scale + 20.), px(size.y1 * scale + 2. + 5. + 29. + 14.));
        window.click_at("page-sheet", at, cx);
    })
    .unwrap();
    settle(window, cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.values.get("size").map(String::as_str), Some("l"));
        assert!(edit.choice_open.is_none(), "picking closes the list");
        assert!(edit.items.is_empty(), "the click on the list placed nothing on the page");
    });

    click_page(window, &shell, mid(&first).0, mid(&first).1, cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.editing, Some(Editing::Field("person.first".into()))));
    type_text(window, "Jonas", cx);
    // A click elsewhere on the page finishes the field.
    click_page(window, &shell, 400., 600., cx);
    press(window, "escape", cx);

    let out = run_and_wait(window, &shell, cx);
    let after = shorui_core::tools::edit::list_fields(&out[0], None).unwrap();
    assert_eq!(after.iter().map(|f| f.value.as_str()).collect::<Vec<_>>(), ["Jonas", "Okafor", "blue", "l"]);
    assert!(!page_text(&out[0])[0].contains("Jonas Jonas"));
}

fn zoom_key(key: &str) -> String {
    if cfg!(target_os = "macos") { format!("cmd-{key}") } else { format!("ctrl-{key}") }
}

fn sheet_bounds(shell: &Entity<Shell>, cx: &mut TestAppContext) -> gpui_kit::Bounds<Pixels> {
    cx.update(|cx| shell.read(cx).canvas.bounds.get())
}

#[gpui_kit::test]
fn edit_caret_sits_on_the_typed_line(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("edit", "report.pdf", cx);
    for (zoom, y) in [(1., 420.), (2., 470.)] {
        cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.set_canvas_zoom(zoom, cx))).unwrap();
        settle(window, cx);
        click_page(window, &shell, 300., y, cx);
        type_text(window, "Hello", cx);
        cx.update(|cx| {
            let shell = shell.read(cx);
            let b = shell.canvas.bounds.get();
            let scale = f32::from(b.size.width) / shell.canvas.page_size.unwrap().0;
            let item = shell.canvas.edit.items.last().unwrap();
            let (caret, line_height) = shell.inputs.get("edit:inline").expect("typing on the page").state.read(cx).cursor_layout().expect("the caret is laid out");
            // The static text is centred in its line box; the caret must be too.
            let line_mid = f32::from(b.origin.y) + (item.rect.y0 - 0.33 * item.size + 0.7 * item.size) * scale;
            let caret_mid = f32::from(caret.origin.y) + f32::from(caret.size.height) / 2.;
            assert!((caret_mid - line_mid).abs() < 1.5, "zoom {zoom}: caret middle {caret_mid}, line middle {line_mid}");
            // The caret is as tall as the text, so the text size reached the field.
            let want = item.size * scale * 1.2;
            assert!((f32::from(line_height) - want).abs() < 1.5, "zoom {zoom}: line height {line_height:?}, want {want}");
            // Letter widths differ in the windowless test fonts, so only check the caret is past the start.
            assert!(f32::from(caret.origin.x) > f32::from(b.origin.x) + item.rect.x0 * scale, "zoom {zoom}: {caret:?}");
        });
        press(window, "enter", cx);
    }
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.items.len(), 2));
}

#[gpui_kit::test]
fn edit_marks_go_where_clicked_even_on_fields(cx: &mut TestAppContext) {
    use crate::view_canvas::EditTool;
    let (window, shell, _) = open_with("edit", "form.pdf", cx);
    let (name, agree) = cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        let rect = |n: &str| edit.fields.iter().find(|f| f.name == n).unwrap().rect;
        (rect("full_name"), rect("agree"))
    });
    let set = |tool: EditTool, cx: &mut TestAppContext| cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.set_edit_tool(tool, cx))).unwrap();

    // A tick on a text field is a tick, not typing.
    set(EditTool::Check, cx);
    click_page(window, &shell, name.x0 + 40., (name.y0 + name.y1) / 2., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert!(edit.editing.is_none(), "the field did not open for typing");
        assert_eq!(edit.items.len(), 1);
        assert_eq!(edit.values.get("full_name").map(String::as_str), Some(""));
        let r = edit.items[0].rect;
        assert!(((r.x0 + r.x1) / 2. - (name.x0 + 40.)).abs() < 1.5, "the tick is where it was clicked: {r:?}");
    });
    // The tick tool still ticks a real tick box.
    click_page(window, &shell, (agree.x0 + agree.x1) / 2., (agree.y0 + agree.y1) / 2., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.values.get("agree").map(String::as_str), Some("true"));
        assert_eq!(edit.items.len(), 1);
    });
    // Other marks go on top of a tick box.
    set(EditTool::Cross, cx);
    click_page(window, &shell, (agree.x0 + agree.x1) / 2., (agree.y0 + agree.y1) / 2., cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.items.len(), 2));
    // Marks can sit close together: only a click right on one picks it up.
    set(EditTool::Check, cx);
    click_page(window, &shell, 300., 300., cx);
    click_page(window, &shell, 309., 300., cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.edit.items.len(), 4));
    click_page(window, &shell, 309., 300., cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        assert_eq!(edit.items.len(), 4, "a click on a mark picks it up instead of stacking another");
        assert_eq!(edit.selected, Some(3));
    });
}

#[gpui_kit::test]
fn zoom_keys_and_wheel_scale_the_page_and_it_scrolls(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("redact", "report.pdf", cx);
    let fit = sheet_bounds(&shell, cx);
    press(window, &zoom_key("="), cx);
    let bigger = sheet_bounds(&shell, cx);
    assert!((f32::from(bigger.size.width) / f32::from(fit.size.width) - 1.25).abs() < 0.01, "{fit:?} -> {bigger:?}");
    press(window, &zoom_key("-"), cx);
    press(window, &zoom_key("-"), cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.zoom.0, 0.75));
    press(window, &zoom_key("0"), cx);
    assert_eq!(sheet_bounds(&shell, cx).size, fit.size, "Ctrl 0 shows the whole page again");

    // Ctrl with the wheel zooms too: one notch, one step.
    cx.update_window(window, |_, window, cx| {
        let b = shell.read(cx).canvas.bounds.get();
        let event = gpui_kit::ScrollWheelEvent { position: b.center(), delta: gpui_kit::ScrollDelta::Lines(point(0., 3.)), modifiers: gpui_kit::Modifiers::secondary_key(), ..Default::default() };
        window.dispatch_event(gpui_kit::InputEvent::to_platform_input(event), cx);
    })
    .unwrap();
    settle(window, cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.zoom.0, 1.25));

    // Zoomed past the stage, the page starts at the stage's edge and scrolls.
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.set_canvas_zoom(3., cx))).unwrap();
    settle(window, cx);
    cx.update(|cx| shell.read(cx).canvas_scroll.set_offset(point(px(0.), px(0.))));
    settle(window, cx);
    let start = sheet_bounds(&shell, cx);
    assert!(f32::from(start.size.width) > 1200., "{start:?}");
    // Sidebar 220, the card's edge 1, the stage's padding 20: nothing is cut off on the left.
    assert!((f32::from(start.origin.x) - 242.).abs() < 2., "the left edge can be reached: {start:?}");
    cx.update(|cx| shell.read(cx).canvas_scroll.set_offset(point(px(-150.), px(-200.))));
    settle(window, cx);
    let moved = sheet_bounds(&shell, cx);
    assert!((f32::from(start.origin.x - moved.origin.x) - 150.).abs() < 1. && (f32::from(start.origin.y - moved.origin.y) - 200.).abs() < 1., "{start:?} -> {moved:?}");
    // Clicks still land on the right spot of the page when it is zoomed and scrolled.
    cx.update(|cx| {
        let shell = shell.read(cx);
        let b = shell.canvas.bounds.get();
        let (x, y) = shell.to_page_point(b.origin + point(b.size.width * 0.5, b.size.height * 0.25)).unwrap();
        let (pw, ph) = shell.canvas.page_size.unwrap();
        assert!((x - pw * 0.5).abs() < 0.5 && (y - ph * 0.25).abs() < 0.5);
    });
}

#[gpui_kit::test]
fn images_to_pdf_rows_show_the_pictures(cx: &mut TestAppContext) {
    use crate::thumbs::Thumb;
    let (window, shell, _) = open_with("img2pdf", "images/photo.jpg", cx);
    let png = fixtures_dir().join("images").join("diagram.png");
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.add_files(vec![png], cx))).unwrap();
    settle(window, cx);
    cx.update(|cx| {
        let shell = shell.read(cx);
        for (name, (w, h)) in [("photo.jpg", (128, 85)), ("diagram.png", (128, 85))] {
            let path = shell.state.files.iter().find(|f| f.name == name).unwrap().path.clone();
            match shell.thumbs.get(&(path, 0, 128)) {
                Some(Thumb::Ready(image)) => {
                    let size = image.size(0);
                    assert_eq!((size.width.0, size.height.0), (w, h), "{name}");
                }
                _ => panic!("no picture for {name}"),
            }
        }
    });
}

#[gpui_kit::test]
fn edit_a_date_typed_in_one_place_stays_there(cx: &mut TestAppContext) {
    use crate::view_edit::place_key;
    let (window, shell, _) = open_with("edit", "shared.pdf", cx);
    let (date, keys) = cx.update(|cx| {
        let edit = &shell.read(cx).canvas.edit;
        let date = edit.fields.iter().find(|f| f.name == "Date").unwrap().clone();
        let keys: Vec<String> = date.widgets.iter().map(|w| place_key(&date, w)).collect();
        (date, keys)
    });
    assert_eq!(keys.len(), 3);
    let second = date.widgets[1].rect;
    click_page(window, &shell, second.x0 + 20., (second.y0 + second.y1) / 2., cx);
    type_text(window, "2026-10-03", cx);
    press(window, "enter", cx);
    cx.update(|cx| {
        let values = &shell.read(cx).canvas.edit.values;
        assert_eq!(values.get(&keys[1]).map(String::as_str), Some("2026-10-03"));
        assert_eq!(values.get(&keys[0]).map(String::as_str), Some(""), "the other places are left alone");
        assert_eq!(values.get(&keys[2]).map(String::as_str), Some(""));
    });
    let out = run_and_wait(window, &shell, cx);
    let fields = shorui_core::tools::edit::list_fields(&out[0], None).unwrap();
    let value = |n: &str| fields.iter().find(|f| f.name == n).map(|f| f.value.clone());
    assert_eq!(value("Date 2").as_deref(), Some("2026-10-03"));
    assert_eq!(value("Date").as_deref(), Some(""));
    assert_eq!(fields.iter().find(|f| f.name == "Date").unwrap().widgets.len(), 2);
}

#[gpui_kit::test]
fn a_run_asks_where_to_save_one_result(cx: &mut TestAppContext) {
    let (window, shell, out) = open_with("compress", "letter.pdf", cx);
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path(), "one result: a save dialog for the file");
    // A name typed without its extension gets it.
    let chosen = out.join("smaller letter");
    let pick = chosen.clone();
    cx.simulate_new_path_selection(move |start| {
        assert_eq!(start, pick.parent().unwrap(), "the dialog opens in the folder last saved into");
        Some(pick)
    });
    cx.run_until_parked();
    wait_for_run(&shell, cx);
    let written: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    let status = cx.update(|cx| format!("{:?}", shell.read(cx).state.files[0].status));
    assert!(out.join("smaller letter.pdf").exists(), "saved where the dialog said, with .pdf added: {written:?} {status}");
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.settings.save_dir.as_deref(), Some(out.as_path()), "the folder is remembered");
        assert_eq!(shell.last_outputs, vec![out.join("smaller letter.pdf")], "Show in folder knows the result");
    });
}

#[gpui_kit::test]
fn cancelling_the_save_dialog_runs_nothing(cx: &mut TestAppContext) {
    let (window, shell, out) = open_with("compress", "letter.pdf", cx);
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    cx.run_until_parked();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    cx.update(|cx| assert!(!shell.read(cx).state.running));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing was written");
}

#[gpui_kit::test]
fn the_file_being_read_is_never_saved_over(cx: &mut TestAppContext) {
    let (window, shell, out) = open_with("compress", "letter.pdf", cx);
    let original = fixtures_dir().join("letter.pdf");
    let before = std::fs::read(&original).unwrap();
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    cx.run_until_parked();
    let target = original.clone();
    cx.simulate_new_path_selection(move |_| Some(target));
    cx.run_until_parked();
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert!(!shell.state.running);
        assert_eq!(shell.toast.as_ref().map(|t| t.title.as_str()), Some("Choose another name"));
    });
    assert_eq!(std::fs::read(&original).unwrap(), before);
    let _ = out;
}

#[gpui_kit::test]
fn several_results_ask_for_a_folder(cx: &mut TestAppContext) {
    let (window, shell, out) = open_with("compress", "letter.pdf", cx);
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.add_files(vec![fixtures_dir().join("appendix.pdf")], cx))).unwrap();
    settle(window, cx);
    let folder = out.join("chosen");
    std::fs::create_dir_all(&folder).unwrap();
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_current(window, cx))).unwrap();
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path() && cx.did_prompt_for_paths(), "two results: a folder dialog");
    let pick = folder.clone();
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.directories && !options.files);
        Some(vec![pick])
    });
    cx.run_until_parked();
    wait_for_run(&shell, cx);
    let mut names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["appendix-compressed.pdf", "letter-compressed.pdf"]);
}

#[gpui_kit::test]
fn favorites_are_starred_and_opened_by_number(cx: &mut TestAppContext) {
    let (window, shell) = open(cx);
    cx.update(|cx| assert!(shell.read(cx).settings.favorites.is_empty(), "no favourites until the user stars some"));
    cx.update_window(window, |_, window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_tool("ocr", cx);
            shell.run_command(Command::ToggleFavorite, window, cx);
            shell.open_tool("merge", cx);
            shell.toggle_favorite("merge", cx);
            shell.go_home(cx);
        });
    })
    .unwrap();
    cx.update(|cx| assert_eq!(shell.read(cx).settings.favorites, vec!["ocr", "merge"]));
    press(window, &zoom_key("2"), cx);
    cx.update(|cx| assert_eq!(shell.read(cx).state.mode, Some("merge"), "Ctrl 2 opens the second favourite"));
    cx.update_window(window, |_, window, cx| shell.update(cx, |shell, cx| shell.run_command(Command::ToggleFavorite, window, cx))).unwrap();
    cx.update(|cx| assert_eq!(shell.read(cx).settings.favorites, vec!["ocr"], "and the same command takes it off again"));
}

#[gpui_kit::test]
fn tool_options_come_back_next_time_but_passwords_do_not(cx: &mut TestAppContext) {
    let (window, shell) = open(cx);
    cx.update_window(window, |_, _, cx| {
        shell.update(cx, |shell, _| {
            shell.state.set_option("compress", "preset", serde_json::json!("strong"));
            shell.state.set_option("protect", "user_password", serde_json::json!("hunter2"));
            shell.canvas.edit.ink = crate::view_canvas::INKS[1].1;
            shell.save_prefs();
        })
    })
    .unwrap();
    let saved = cx.update(|cx| shell.read(cx).settings.clone());
    assert!(!serde_json::to_string(&saved).unwrap().contains("hunter2"));
    assert_eq!(saved.edit_ink, Some(crate::view_canvas::INKS[1].1));

    // A new window built from those settings starts where the last one left off.
    let (_, again) = cx.update(|cx| {
        let options = WindowOptions::default();
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(saved.clone(), Instant::now(), window, cx))).expect("window")
    });
    cx.update(|cx| {
        let shell = again.read(cx);
        assert_eq!(shell.state.options.get("compress").and_then(|o| o.get("preset")), Some(&serde_json::json!("strong")));
        assert_ne!(shell.state.options.get("protect").and_then(|o| o.get("user_password")), Some(&serde_json::json!("hunter2")));
        assert_eq!(shell.canvas.edit.ink, crate::view_canvas::INKS[1].1);
    });
}

#[gpui_kit::test]
fn a_touchpad_pinch_zooms_smoothly(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("edit", "report.pdf", cx);
    let fit = sheet_bounds(&shell, cx);
    let pinch = |delta: f32, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            let b = shell.read(cx).canvas.bounds.get();
            let event = gpui_kit::PinchEvent { position: b.center(), delta, modifiers: Default::default(), phase: Default::default() };
            window.dispatch_event(gpui_kit::InputEvent::to_platform_input(event), cx);
        })
        .unwrap();
        settle(window, cx);
    };
    pinch(0.1, cx);
    pinch(0.1, cx);
    cx.update(|cx| assert!((shell.read(cx).canvas.zoom.0 - 1.21).abs() < 1e-3, "two small pinches: {}", shell.read(cx).canvas.zoom.0));
    let bigger = sheet_bounds(&shell, cx);
    assert!((f32::from(bigger.size.width) / f32::from(fit.size.width) - 1.21).abs() < 0.01);
    pinch(-0.9, cx);
    cx.update(|cx| assert_eq!(shell.read(cx).canvas.zoom.0, 0.5, "never smaller than half"));
}

#[gpui_kit::test]
fn repair_takes_the_damaged_file_other_tools_leave_out(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("repair", "broken.pdf", cx);
    cx.update(|cx| {
        let s = &shell.read(cx).state;
        assert!(s.files[0].problem.is_some(), "the file is known to be damaged");
        assert_eq!(s.run_set(), vec![0], "and Repair includes it");
    });
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.open_tool("compress", cx))).unwrap();
    cx.update(|cx| assert!(shell.read(cx).state.run_set().is_empty(), "Compress leaves it out"));
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.open_tool("repair", cx))).unwrap();
    let out = run_and_wait(window, &shell, cx);
    assert!(shorui_core::doc::quick_info(&out[0]).unwrap().pages > 0);
}

#[gpui_kit::test]
fn pages_grid_keys_edit_then_save(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("pages", "report.pdf", cx);
    cx.update_window(window, |_, window, cx| {
        let focus = shell.read(cx).grid_focus.clone();
        window.focus(&focus, cx);
        window.render_frame(cx);
        assert_eq!(shell.read(cx).state.pages_edit.slots.len(), 6);
        window.press("right", cx); // cursor on page 2
        window.press("shift-right", cx); // select 2-3
        window.press("r", cx); // rotate both
        window.press("right", cx); // page 4 alone
        window.press("delete", cx);
    })
    .unwrap();
    settle(window, cx);
    cx.update(|cx| {
        let edit = &shell.read(cx).state.pages_edit;
        assert_eq!(edit.slots.iter().map(|s| s.source).collect::<Vec<_>>(), vec![1, 2, 3, 5, 6]);
        assert_eq!(edit.slots[1].turn, 90);
        assert_eq!(edit.slots[2].turn, 90);
    });
    let out = run_and_wait(window, &shell, cx);
    let text = page_text(&out[0]);
    assert_eq!(text.len(), 5);
    assert!(text[3].contains("MARK-REPORT-P5"));
    let doc = shorui_core::doc::load(&out[0], None).unwrap();
    let ids = shorui_core::doc::page_ids(&doc);
    assert_eq!(shorui_core::doc::rotation(&doc, ids[1]), 90);
    assert_eq!(shorui_core::doc::rotation(&doc, ids[0]), 0);
}

#[gpui_kit::test]
fn a_batch_run_reports_each_file_and_keeps_going_after_a_failure(cx: &mut TestAppContext) {
    let (window, shell, _) = open_with("compress", "photos.pdf", cx);
    cx.update_window(window, |_, _, cx| shell.update(cx, |shell, cx| shell.add_files(vec![fixtures_dir().join("broken.pdf"), fixtures_dir().join("letter.pdf")], cx))).unwrap();
    settle(window, cx);
    cx.update(|cx| {
        let s = &shell.read(cx).state;
        assert!(s.files[1].problem.is_some(), "the damaged file is flagged as soon as it is added");
        assert!(!s.files[1].included, "and left out of the batch");
        assert_eq!(s.run_set(), vec![0, 2]);
    });
    run_and_wait(window, &shell, cx);
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert!(matches!(shell.state.files[0].status, RowStatus::Done { .. }));
        assert!(matches!(shell.state.files[2].status, RowStatus::Done { .. }));
        let toast = shell.toast.as_ref().expect("a toast reports the result");
        assert!(toast.ok && toast.title == "2 files done", "{} / {}", toast.title, toast.body);
        assert!(toast.body.contains("smaller. Saved in "), "{}", toast.body);
    });
}
