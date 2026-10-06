//! Shorui: an offline, all-in-one PDF toolbox.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod assets;
mod catalog;
mod fuzzy;
mod jobs;
mod palette;
mod palette_model;
mod settings;
mod shell;
mod state;
mod thumbs;
mod tokens;
#[cfg(test)]
mod tool_runs;
#[cfg(test)]
mod ui_tests;
mod ui;
mod view_canvas;
mod view_edit;
mod view_chrome;
mod view_panel;
mod view_special;
mod view_tools;
mod view_work;

use gpui_kit::component::{Theme as KitTheme, ThemeMode, TitleBar};
use gpui_kit::*;
use settings::Settings;
use shell::Shell;
use std::time::Instant;

/// Switch every colour in the app. Views read the palette from a global each render.
pub fn apply_theme(dark: bool, cx: &mut App) {
    shell::set_theme(dark, cx);
    // gpui-kit's own components (text fields, scrollbars, tooltips) read its theme, so it
    // is kept in step with the tokens.
    let p = tokens::Palette::of(dark);
    KitTheme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    KitTheme::update(cx, |theme| {
        let c = &mut theme.colors;
        c.background = p.card;
        c.foreground = p.toner;
        c.border = p.rule;
        c.input = p.control_rule;
        c.ring = p.cyan;
        c.caret = p.cyan;
        c.selection = p.cyan_rule;
        c.primary = p.cyan;
        c.primary_hover = p.cyan_hi;
        c.primary_active = p.cyan_lo;
        c.primary_foreground = p.cyan_ink;
        c.secondary = p.plate;
        c.secondary_hover = p.plate_2;
        c.secondary_active = p.plate_3;
        c.muted = p.plate_3;
        c.muted_foreground = p.toner_3;
        c.accent = p.plate_2;
        c.accent_foreground = p.toner;
        c.popover = p.sleeve;
        c.popover_foreground = p.toner;
        c.scrollbar = gpui_kit::transparent_black();
        c.scrollbar_thumb = p.rule_strong;
        c.scrollbar_thumb_hover = p.toner_4;
        c.title_bar = p.frame;
        c.title_bar_border = p.rule;
        theme.font_family = tokens::FONT_UI.into();
        theme.font_size = px(tokens::TEXT);
        theme.mono_font_family = tokens::FONT_MONO.into();
        theme.mono_font_size = px(tokens::TEXT_SM);
        theme.radius = px(tokens::R_SM);
        theme.radius_lg = px(tokens::R_MD);
        theme.shadow = false;
    });
}

fn main() {
    let booted = Instant::now();
    let log = std::env::var_os("SHORUI_BOOT_LOG").is_some();
    let lap = move |what: &str| {
        if log {
            eprintln!("{:>4} ms  {what}", booted.elapsed().as_millis());
        }
    };
    let mut settings = Settings::load();
    // `shorui [--tool <id>] [--theme light|dark] [--size 1100x700] [files...]`
    let mut tool = None;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tool" => tool = args.next(),
            "--theme" => settings.dark = args.next().as_deref() != Some("light"),
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))) {
                    settings.window = Some((w.max(1100.), h.max(700.)));
                }
            }
            _ => files.push(std::path::PathBuf::from(arg)),
        }
    }
    lap("settings read");
    let app = application().with_assets(assets::Assets);
    lap("platform ready");
    app.run(move |cx| {
        lap("event loop running");
        init(cx);
        lap("gpui-kit initialised");
        // Fonts are embedded, so there is no file lookup on the way to the first frame.
        if let Err(e) = cx.text_system().add_fonts(assets::fonts()) {
            eprintln!("could not load the bundled fonts: {e}");
        }
        lap("fonts registered");
        apply_theme(settings.dark, cx);
        lap("theme applied");
        shell::bind_keys(cx);
        palette::bind_keys(cx);
        view_special::bind_keys(cx);

        let (w, h) = settings.window.unwrap_or((1440., 900.));
        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let options = WindowOptions {
            window_bounds: Some(if settings.maximized { WindowBounds::Maximized(bounds) } else { WindowBounds::Windowed(bounds) }),
            window_min_size: Some(size(px(1100.), px(700.))),
            // The top bar is the title bar: no separate caption strip.
            titlebar: Some(TitlebarOptions { title: Some("Shorui".into()), ..TitleBar::title_bar_options() }),
            app_id: Some("shorui".into()),
            focus: std::env::var_os("SHORUI_NO_ACTIVATE").is_none(),
            ..TitleBar::window_options()
        };
        let settings = settings.clone();
        open_window(options, cx, move |window, cx| {
            cx.new(|cx| {
                let mut shell = Shell::new(settings, booted, window, cx);
                if let Some(id) = tool.as_deref().and_then(catalog::tool).map(|t| t.id) {
                    shell.open_tool(id, cx);
                }
                if !files.is_empty() {
                    shell.add_files(files, cx);
                }
                shell
            })
        })
        .expect("could not open the window");
        lap("window open");
        // `SHORUI_NO_ACTIVATE` lets scripted runs open the window without taking the keyboard.
        if std::env::var_os("SHORUI_NO_ACTIVATE").is_none() {
            cx.activate(true);
        }
    });
}
