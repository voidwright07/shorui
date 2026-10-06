# gpui-kit 0.7.0 cheat sheet

Written from the crate sources (`gpui-kit` 0.7.0, `gpui-component` 0.7.0, `gpui-base` 0.7.0, `gpui-kit-assets` 0.7.0, `gpui-pre*` 0.3.7), not from memory of older gpui-component releases.

**Legend.** `[C]` = snippet compiled with `cargo +1.98.0 build` in a throwaway crate outside this repo (`gpui-kit = "0.7"`, `image = "0.25"`, `rust-embed = "8"`, Windows 11). `[S]` = signature or behaviour read from source, not compiled here. Signature blocks are copied from the sources. In addition a 2.5 s smoke run of that crate (no panics) actually executed: bootstrap, asset source, `add_fonts`, theme JSON install (variant A) + `sync_system_appearance`, all `KeyBinding::new` strings in section 8, and one rendered frame of the TitleBar, the palette view (section 8/9), every component in section 10 and the layout/scroll/virtual-list/overlay view of section 6. Compiled but **not executed**: image functions, theme variants B and C, the async/state code of section 7, and everything behind a click (dialog, sheet, notification, menu closures, drag and drop). Pixel sizes quoted as `h_8` etc. are **rem based**: `1rem = theme.font_size` (default 16 px), so `h_8` = 32 px only at the default font size.

## 0. Crate map and imports

| Path | Crate | Notes |
| --- | --- | --- |
| `gpui_kit::*` | gpui (`gpui-pre`) | everything gpui exports, plus kit's own `actions!`, `init`, `open_window`, `application` |
| `gpui_kit::platform` | `gpui-pre-platform` | `application()`, `headless()`, `current_platform()` |
| `gpui_kit::base` | `gpui-base` | unstyled behaviour layer, `Root`, motion, virtual list |
| `gpui_kit::component` | `gpui-component` | styled components, `Theme`, `TitleBar`, `Icon` |
| `gpui_kit::assets` | `gpui-kit-assets` | `Assets`, `AllAssets`, `IconName` (full Lucide), `icon_assets!` |

```rust
use gpui_kit::*;                                   // gpui: div, px, App, Window, Context, Entity, ...
use gpui_kit::prelude::FluentBuilder as _;         // .when / .when_some / .when_else / .when_none / .map  (NOT in the root glob)
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Selectable as _, Disableable as _,
    StyledExt as _, WindowExt as _, Colorize as _, h_flex, v_flex, Icon, IconName, Theme, TitleBar};
```

- Do **not** glob-import `gpui_kit::component::*` next to `gpui_kit::*`: both export `black()`, `white()`, `red`, `blue`, `green`, `yellow` (gpui: `fn red() -> Hsla`; component: `fn red(scale: usize) -> Hsla`).
- `gpui_kit::Result` is `anyhow::Result` and shadows the std prelude `Result` under `use gpui_kit::*` (still accepts two type args).
- Derives (`IntoElement`, `Action`, `Render`) and `actions!` resolve paths through the facade; no direct `gpui` dependency is needed.
- `image` is **not** re-exported. Add `image = { version = "0.25", default-features = false }` to build `RenderImage` frames (lock resolves to the same 0.25.x gpui uses).

## 1. Bootstrap `[C]`

```rust
pub fn application() -> Application                              // gpui_kit::application (desktop + web)
pub fn init(cx: &mut App)                                        // gpui_kit::init == gpui_component::init (+ gpui_base::init)
pub fn open_window<V: Render>(options: WindowOptions, cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> Result<(AnyWindowHandle, Entity<V>)>
// Application: .with_assets(impl AssetSource) .with_quit_mode(QuitMode) .with_http_client(..) .run(|cx: &mut App| ..)
```

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::*;

fn main() {
    application().with_assets(app_assets::AppAssets).run(|cx| {
        init(cx);                                   // must run before open_window
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
            window_min_size: Some(size(px(720.), px(480.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Shorui".into()),
                ..TitleBar::title_bar_options()     // appears_transparent: true, traffic lights at (9, 9)
            }),
            app_id: Some("dev.shorui.app".into()),
            kind: WindowKind::Normal,
            ..TitleBar::window_options()            // == Default + titlebar above + app_owns_titlebar_drag: true
        };
        let (window, _shell) = open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(window, cx)))
            .expect("failed to open window");
        window.update(cx, |_root: AnyView, window, cx| {
            window.set_window_title("Shorui");                       // runtime title change
            Theme::sync_system_appearance(Some(window), cx);         // optional: follow OS dark/light once
        }).ok();
        cx.activate(true);
    });
}
```

`WindowOptions` fields (all `pub`, `Default` in brackets):

| Field | Type | Notes |
| --- | --- | --- |
| `window_bounds` | `Option<WindowBounds>` [`None`] | `WindowBounds::{Windowed, Maximized, Fullscreen}(Bounds<Pixels>)`; `WindowBounds::centered(size, cx)`; `None` = cascade from active window / display default |
| `window_min_size` | `Option<Size<Pixels>>` [`None`] | enforced on Windows via `WM_GETMINMAXINFO` |
| `titlebar` | `Option<TitlebarOptions>` [`Some(default)`] | `{ title: Option<SharedString>, appears_transparent: bool, traffic_light_position: Option<Point<Pixels>> }`. On Windows `None` also hides the native title bar |
| `app_owns_titlebar_drag` | `bool` [`false`] | macOS only; set `true` with a custom title bar |
| `window_decorations` | `Option<WindowDecorations>` [`None`] | Linux: `Server` / `Client` |
| `window_background` | `WindowBackgroundAppearance` [`Opaque`] | `Transparent`, `Blurred`, `MicaBackdrop`, `MicaAltBackdrop` |
| `kind` | `WindowKind` [`Normal`] | `PopUp` (topmost tool window), `Floating`, `Dialog` (modal, disables parent on Windows), `AnchoredPopup(..)` (rejected on Windows) |
| `app_id` | `Option<String>` [`None`] | used on Linux; **not read by the Windows backend** |
| `focus`, `show` | `bool` [`true`] | |
| `is_movable`, `is_resizable`, `is_minimizable` | `bool` [`true`] | |
| `display_id`, `icon` (X11 only), `tabbing_identifier` (macOS), `inactive_frame_interval` [`Some(33ms)`] | | |

Quit: `QuitMode::Default` = quit on last window closed everywhere except macOS. `cx.quit()` quits explicitly. `cx.on_window_closed(|cx, window_id| ..) -> Subscription`.

**Root layer.** `gpui_kit::open_window` wraps your view in `gpui_kit::base::Root` (`cx.open_window` directly would skip it and `WindowExt` calls then panic with "component window state is missing"). `gpui_component::init` registers a per-window plugin on that Root which:
- mounts, above your content: sheet layer, dialog layer, notification layer, tooltip overlay, fallback menu overlay, touch-selection overlay;
- sets `window.set_rem_size(theme.font_size)` every frame, and root `font_family`, `bg(theme.background)`, `text_color(theme.foreground)`;
- wraps everything in `window_border()` (only visible on Linux client-side decorations).

Your view renders **no** layer elements; `Root::render_dialog_layer` / `render_notification_layer` do not exist in 0.7. Root also owns key context `"Root"` with `tab` / `shift-tab` (focus next/prev, honouring focus traps) and `ctrl-c` (copy window text selection). `Root::read(window, cx)`, `Root::update(window, cx, |root, window, cx| ..)`, `root.view() -> &AnyView`.

## 2. Custom title bar `[C]`

```rust
// gpui_kit::component::{TitleBar, TITLE_BAR_HEIGHT}        TITLE_BAR_HEIGHT = px(34.)
TitleBar::new() -> TitleBar                                 // impl Styled + ParentElement, RenderOnce
TitleBar::title_bar_options() -> TitlebarOptions
TitleBar::window_options() -> WindowOptions
.on_close_window(impl Fn(&ClickEvent, &mut Window, &mut App) + 'static)   // Linux only
```

```rust
v_flex().size_full()
    .child(TitleBar::new()
        .child(h_flex().gap_2().text_sm().child("Shorui"))    // 1st child: left
        .child(h_flex().pr_2().child("right side")))          // 2nd child: right (container is justify_between)
    .child(div().flex_1().min_h_0().child("content"))
```

- Layout: `h(34px)`, `pl(12px)` (80 px on macOS for traffic lights), bottom border `theme.title_bar_border`, background = vertical gradient built from `theme.title_bar` and `theme.background`. Style overrides via `Styled` (`.bg(..)`, `.h(..)`, `.border_b_0()`).
- Children are placed in an inner `h_flex` that **is the drag region** (`window_control_area(WindowControlArea::Drag)` -> `HTCAPTION` on Windows; `start_window_move()` on mouse drag elsewhere). Double-click: Windows native, macOS `titlebar_double_click()`, Linux `zoom_window()`.
- Windows controls: three 34 px wide divs (minimize, maximize/restore, close) drawn with `IconName::WindowMinimize/WindowMaximize/WindowRestore/WindowClose`, each tagged `window_control_area(Min/Max/Close)`; the OS performs the action (snap layouts work). Not drawn on macOS; on Linux only with client-side decorations.
- Clickable things inside the bar sit on top of the drag region. **Uncertain, not tested at runtime:** on Windows the hit test answers `HTCAPTION` wherever the Drag hitbox is in the hit-test stack, and neither a plain `div().id(..).on_click(..)` nor `Button` removes it (an enabled `Button` only calls `window.prevent_default()` on mouse-down; it neither occludes nor stops propagation), so whether a click survives the OS caption handling cannot be decided from source. Deterministic options derived from the Windows backend: wrap title-bar controls in `div().occlude()` (takes the Drag hitbox out of the hit test there, so Windows sees `HTCLIENT`), or call `cx.stop_propagation()` in an `.on_mouse_down(MouseButton::Left, ..)` on the control (gpui then reports the non-client mouse-down as handled and skips the default caption handling).
- Own drag region without `TitleBar`: `div().window_control_area(WindowControlArea::Drag)`; needs `titlebar.appears_transparent = true`.

## 3. Theme

```rust
// gpui_kit::component::{Theme, ThemeColor, ThemeMode, ThemeConfig, ThemeConfigColors, ThemeSet, ThemeRegistry, ActiveTheme, Colorize}
pub struct Theme {                       // Global; Deref/DerefMut -> ThemeColor
    pub colors: ThemeColor,              // solid Hsla per token
    pub tokens: ThemeTokens,             // same tokens as ThemeToken { color, background } (may hold gradients)
    pub highlight_theme: Arc<HighlightTheme>,
    pub light_theme: Rc<ThemeConfig>, pub dark_theme: Rc<ThemeConfig>,
    pub mode: ThemeMode,                 // Light | Dark
    pub font_family: SharedString,       // ".SystemUIFont"
    pub font_size: Pixels,               // 16px; ALSO the window rem size
    pub mono_font_family: SharedString,  // Consolas on Windows (Menlo / DejaVu Sans Mono)
    pub mono_font_size: Pixels,          // 13px
    pub radius: Pixels,                  // 6px   general
    pub radius_lg: Pixels,               // 8px   dialogs, notifications
    pub shadow: bool,                    // true
    pub focus_ring: bool,                // true; false = tinted border only (cannot be clipped)
    pub transparent: Hsla,
    pub scrollbar_mode: ScrollbarMode,   // Scrolling | Hover | Always (init syncs it with the OS setting)
    pub notification: NotificationSettings, pub list: ListSettings, pub sheet: SheetSettings, pub motion: MotionTokens,
}
impl Theme {
    pub fn global(cx: &App) -> &Theme
    pub fn global_mut(cx: &mut App) -> &mut Theme                     // raw edit: tokens + base projection NOT synced
    pub fn update<R>(cx: &mut App, edit: impl FnOnce(&mut Theme) -> R) -> R   // preferred write path, refreshes windows
    pub fn change(mode: impl Into<ThemeMode>, _window: Option<&mut Window>, cx: &mut App)
    pub fn sync_system_appearance(window: Option<&mut Window>, cx: &mut App)
    pub fn sync_scrollbar_appearance(cx: &mut App)
    pub fn set_scrollbar_mode(mode: ScrollbarMode, cx: &mut App)
    pub fn sync_base(cx: &mut App)                                    // call after global_mut edits, then cx.refresh_windows()
    pub fn apply_config(&mut self, config: &Rc<ThemeConfig>)          // installs as light/dark slot AND switches to its mode
    pub fn is_dark(&self) -> bool
    pub fn theme_name(&self) -> &SharedString
    pub fn input_background(&self) -> Hsla
    pub fn radius_full(&self) -> Pixels                               // 9999, or 0 when radius == 0
}
pub trait ActiveTheme { fn theme(&self) -> &Theme; }                  // impl for App: cx.theme().primary
```

How switching works:
- `init` installs the built-in "Default Light"/"Default Dark" configs and calls `Theme::change(ThemeMode::Light, ..)`. Nothing follows the OS automatically.
- `Theme::change(mode)` **reloads the `ThemeConfig` in `light_theme` / `dark_theme` for that mode** via `apply_config`: every colour is recomputed from the config (missing keys fall back to derived/default values), and `radius`, `radius_lg`, `shadow`, fonts are overwritten only when the config sets them. So directly assigned `theme.colors` are lost on the next `change`.
- Setting `theme.mode` inside `Theme::update` does the same reload. Edit colours in a second `update` after switching.
- Editing the `ThemeRegistry` global re-runs `Theme::change` (observer installed by `init`).

```rust
// A. [C] own light + dark sets from JSON (recommended: survives Theme::change)
ThemeRegistry::global_mut(cx).load_themes_from_str(include_str!("../themes/app.json")).expect("invalid theme json");
let registry = ThemeRegistry::global(cx);
let light = registry.themes().get("App Light").cloned().expect("light theme");   // Rc<ThemeConfig>
let dark = registry.themes().get("App Dark").cloned().expect("dark theme");
Theme::update(cx, |theme| { theme.light_theme = light; theme.dark_theme = dark; });
Theme::change(ThemeMode::Dark, None, cx);

// B. [C] ThemeConfig built in code (ThemeConfigColors has private fields: no struct-update syntax, mutate a default)
let mut colors = ThemeConfigColors::default();
colors.background = Some("#141413".into());
colors.primary = Some("#6B8CFF".into());
let config = Rc::new(ThemeConfig { name: "Code Dark".into(), mode: ThemeMode::Dark, radius: Some(8),
    radius_lg: Some(12), shadow: Some(false), font_family: Some("Arial".into()), font_size: Some(14.),
    colors, ..Default::default() });
Theme::update(cx, |theme| theme.apply_config(&config));

// C. [C] direct assignment (lost on the next Theme::change / mode switch: re-apply after it)
Theme::update(cx, |theme| {
    let mut colors: ThemeColor = *ThemeColor::dark();          // ThemeColor is Copy; ::light() / ::dark() -> Arc<ThemeColor>
    colors.background = rgb(0x141413).into();
    colors.primary_hover = colors.primary.lighten(0.1);
    theme.colors = colors;
    theme.radius = px(8.); theme.radius_lg = px(12.);
    theme.font_family = "Arial".into(); theme.font_size = px(14.);
    theme.mono_font_family = "Consolas".into(); theme.mono_font_size = px(13.);
    theme.shadow = false; theme.focus_ring = true; theme.scrollbar_mode = ScrollbarMode::Hover;
});

// follow the OS [C]
cx.observe_window_appearance(window, |_, window, cx| Theme::sync_system_appearance(Some(window), cx)).detach();
// toggle [C]
let next = if cx.theme().is_dark() { ThemeMode::Light } else { ThemeMode::Dark };
Theme::change(next, Some(window), cx);
```

Theme JSON (`ThemeSet`, verified by the smoke run). Unknown keys and unparsable colour strings are **silently ignored** (fallback is used), so typos do not fail.

```json
{ "name": "App", "author": "..", "url": "..",
  "themes": [ { "name": "App Light", "mode": "light", "is_default": false,
      "font.family": "Arial", "font.size": 14, "mono_font.family": "Consolas", "mono_font.size": 13,
      "radius": 8, "radius.lg": 12, "shadow": false,
      "colors": { "background": "#FBFBFA", "primary.background": "#2F5BEA", "border": "neutral-200" },
      "highlight": { } } ] }
```

Colour strings: `#RRGGBB`, `#RRGGBBAA`, Tailwind names `black`, `white`, `neutral`, `neutral-200`, `blue-600/40` (name[-scale][/opacity%]; scale defaults to 500). Keys marked **bg** below also accept `linear-gradient(135deg, #4F46E5, #06B6D4)` (two stops; solid colour = first stop). `radius` / `radius.lg` are integers.

### ThemeColor fields (`cx.theme().<field>`, all `Hsla`) — JSON key — meaning [fallback when the key is absent]

Core
- `background` — `background` **bg** — window/page background [default theme]
- `foreground` — `foreground` — default text [default theme]
- `border` — `border` — default border [default theme]
- `input` — `input.border` — border of Input/Select [`border`]
- `ring` — `ring` — focus ring / focused border [`blue`]
- `caret` — `caret` — text cursor [`primary`]
- `selection` — `selection.background` **bg** — text selection (alpha capped at 0.3) [`primary`]
- `overlay` — `overlay` **bg** — scrim behind dialogs/sheets [default theme]
- `window_border` — `window.border` — window frame, Linux only [`border`]
- `drag_border` — `drag.border` — drag indicator / active resize handle [`primary` at 0.65]
- `drop_target` — `drop_target.background` **bg** — drop target fill [`primary` at 0.2]

Semantic surfaces (each: base, `_hover`, `_active`, `_foreground`)
- `primary` — `primary.background` **bg** — primary fill [default theme]
- `primary_hover` — `primary.hover.background` **bg** [`background` blended with `primary` 0.9]
- `primary_active` — `primary.active.background` **bg** [`primary` darkened 10% (20% dark mode)]
- `primary_foreground` — `primary.foreground` — text on primary [`foreground`]
- `secondary` — `secondary.background` **bg** — secondary fill [default theme]
- `secondary_hover` — `secondary.hover.background` **bg** [blend 0.9]
- `secondary_active` — `secondary.active.background` **bg** [darkened]
- `secondary_foreground` — `secondary.foreground` — secondary text [`foreground`]
- `muted` — `muted.background` **bg** — muted fill (skeleton, switch off) [default theme]
- `muted_foreground` — `muted.foreground` — muted/disabled text [`muted` blended with `foreground` 0.7]
- `accent` — `accent.background` **bg** — hover fill of menu/list items [`secondary`]
- `accent_foreground` — `accent.foreground` — text on accent [`foreground`]
- `danger` — `danger.background` **bg** [`red`]
- `danger_hover` — `danger.hover.background` **bg** [blend 0.9]
- `danger_active` — `danger.active.background` **bg** [darkened]
- `danger_foreground` — `danger.foreground` [`primary_foreground`]
- `success` — `success.background` **bg** [`green`]
- `success_hover` — `success.hover.background` **bg** [blend 0.9]
- `success_active` — `success.active.background` **bg** [darkened]
- `success_foreground` — `success.foreground` [`primary_foreground`]
- `warning` — `warning.background` **bg** [`yellow`]
- `warning_hover` — `warning.hover.background` **bg** [blend 0.9]
- `warning_active` — `warning.active.background` **bg** [darkened]
- `warning_foreground` — `warning.foreground` [`primary_foreground`]
- `info` — `info.background` **bg** [`cyan`]
- `info_hover` — `info.hover.background` **bg** [blend 0.9]
- `info_active` — `info.active.background` **bg** [darkened]
- `info_foreground` — `info.foreground` [`primary_foreground`]
- `link` — `link` — link text [`primary`]
- `link_hover` — `link.hover` [`link`]
- `link_active` — `link.active` [`link`]

Buttons
- `button` — `button.background` **bg** — default Button fill [light: `background`; dark: `input` mixed 30% with transparent]
- `button_hover` — `button.hover.background` **bg** [`input` mixed 50%]
- `button_active` — `button.active.background` **bg** [`input` mixed 70%]
- `button_foreground` — `button.foreground` [`foreground`]
- `button_primary` — `button.primary.background` **bg** [`primary`]
- `button_primary_hover` — `button.primary.hover.background` **bg** [`primary_hover`]
- `button_primary_active` — `button.primary.active.background` **bg** [`primary_active`]
- `button_primary_foreground` — `button.primary.foreground` [`primary_foreground`]
- `button_secondary` — `button.secondary.background` **bg** [`secondary`]
- `button_secondary_hover` — `button.secondary.hover.background` **bg** [`secondary_hover`]
- `button_secondary_active` — `button.secondary.active.background` **bg** [`secondary_active`]
- `button_secondary_foreground` — `button.secondary.foreground` [`secondary_foreground`]
- `button_danger` — `button.danger.background` **bg** [`danger` mixed 20% (tinted, not solid)]
- `button_danger_hover` — `button.danger.hover.background` **bg** [30%]
- `button_danger_active` — `button.danger.active.background` **bg** [40%]
- `button_danger_foreground` — `button.danger.foreground` [`danger`]
- `button_success` / `_hover` / `_active` — `button.success[.hover|.active].background` **bg** [`success` mixed 20/30/40%]
- `button_success_foreground` — `button.success.foreground` [`success`]
- `button_warning` / `_hover` / `_active` — `button.warning[.hover|.active].background` **bg** [`warning` mixed 20/30/40%]
- `button_warning_foreground` — `button.warning.foreground` [`warning`]
- `button_info` / `_hover` / `_active` — `button.info[.hover|.active].background` **bg** [`info` mixed 20/30/40%]
- `button_info_foreground` — `button.info.foreground` [`info`]

Containers
- `popover` — `popover.background` **bg** — popover/menu/select surface [`background`]
- `popover_foreground` — `popover.foreground` [`foreground`]
- `accordion` — `accordion.background` **bg** [`background`]
- `group_box` — `group_box.background` **bg** [`background` blended with `secondary` 0.4 (0.3 dark)]
- `group_box_foreground` — `group_box.foreground` [`foreground`]
- `title_bar` — `title_bar.background` **bg** — TitleBar [`background`]
- `title_bar_border` — `title_bar.border` [`border`]
- `status_bar` — `status_bar.background` **bg** [`title_bar`]
- `status_bar_border` — `status_bar.border` [`title_bar_border`]
- `sidebar` — `sidebar.background` **bg** [`background` blended with `border` 0.15]
- `sidebar_foreground` — `sidebar.foreground` [`foreground`]
- `sidebar_border` — `sidebar.border` [`border`]
- `sidebar_accent` — `sidebar.accent.background` **bg** — hovered/active item [`accent`]
- `sidebar_accent_foreground` — `sidebar.accent.foreground` [`accent_foreground`]
- `sidebar_primary` — `sidebar.primary.background` **bg** [`primary`]
- `sidebar_primary_foreground` — `sidebar.primary.foreground` [`primary_foreground`]
- `description_list_label` — `description_list.label.background` **bg** [`background` blended with `border` 0.2]
- `description_list_label_foreground` — `description_list.label.foreground` [`muted_foreground`]

Lists, tables, tabs
- `list` — `list.background` **bg** [`background`]
- `list_hover` — `list.hover.background` **bg** [`accent` at 0.6]
- `list_active` — `list.active.background` **bg** — selected row (alpha capped at 0.2) [`primary` at 0.1 over `background`]
- `list_active_border` — `list.active.border` [`primary` at 0.6 over `background`]
- `list_even` — `list.even.background` **bg** — stripe [`list`]
- `list_head` — `list.head.background` **bg** [`list`]
- `table` — `table.background` **bg** [`list`]
- `table_hover` — `table.hover.background` **bg** [`list_hover`]
- `table_active` — `table.active.background` **bg** (alpha capped at 0.2) [`list_active`]
- `table_active_border` — `table.active.border` [`list_active_border`]
- `table_even` — `table.even.background` **bg** [`list_even`]
- `table_head` — `table.head.background` **bg** [`list_head`]
- `table_head_foreground` — `table.head.foreground` [`muted_foreground`]
- `table_foot` — `table.foot.background` **bg** [`list_head`]
- `table_foot_foreground` — `table.foot.foreground` [`muted_foreground`]
- `table_row_border` — `table.row.border` [`border`]
- `tab` — `tab.background` **bg** [`background`]
- `tab_foreground` — `tab.foreground` [`foreground`]
- `tab_active` — `tab.active.background` **bg** [`background`]
- `tab_active_foreground` — `tab.active.foreground` [`foreground`]
- `tab_bar` — `tab_bar.background` **bg** [`background`]
- `tab_bar_segmented` — `tab_bar.segmented.background` **bg** — segmented trough [`secondary`]

Controls
- `scrollbar` — `scrollbar.background` **bg** — track [`background`]
- `scrollbar_thumb` — `scrollbar.thumb.background` **bg** [`accent`]
- `scrollbar_thumb_hover` — `scrollbar.thumb.hover.background` **bg** [`scrollbar_thumb`]
- `skeleton` — `skeleton.background` **bg** [`secondary`]
- `slider_bar` — `slider.background` **bg** — filled range [`primary`]
- `slider_thumb` — `slider.thumb.background` **bg** [`primary_foreground`]
- `switch` — `switch.background` **bg** — unchecked track [`secondary_active`]
- `switch_thumb` — `switch.thumb.background` **bg** [`background`]
- `progress_bar` — `progress.bar.background` **bg** [`primary`]

Charts and base palette
- `chart_1` .. `chart_5` — `chart.1` .. `chart.5` [ramp of `blue`: lighten 0.4, 0.2, base, darken 0.2, 0.4]
- `chart_bullish` — `chart.bullish` [`green`]; `chart_bearish` — `chart.bearish` [`red`]; `chart_grid` — `chart.grid` [`border` at 0.6]
- `red`, `green`, `blue`, `yellow`, `magenta`, `cyan` — `base.red` .. `base.cyan` [default theme]
- `red_light`, `green_light`, `blue_light`, `yellow_light`, `magenta_light`, `cyan_light` — `base.red.light` .. [`background` blended with the base colour at 0.8]

### Colours `[C]`

```rust
rgb(0x141413) -> Rgba            rgba(0xffffff1f) -> Rgba            hsla(h, s, l, a) -> Hsla   // all components 0.0..=1.0
let c: Hsla = rgb(0x6B8CFF).into();                                   // setters take `impl Into<Hsla>` (bg takes `impl Into<Fill>`)
gpui_kit::{black(), white(), red(), green(), blue(), yellow(), transparent_black(), transparent_white(), opaque_grey(l, a)}
Hsla: .opacity(factor)   // MULTIPLIES alpha     .alpha(a)   // SETS alpha     .blend(other) .grayscale() .to_rgb() .is_transparent()
// gpui_kit::component::Colorize (trait on Hsla): .lighten(f) .darken(f) .mix(other, f) .mix_oklab(other, f) .invert() .hue(f) .saturation(f) .lightness(f) .to_hex() ; Hsla::parse_hex("#F8FAFC")
// gpui_kit::component::{hsl(h_deg, s_pct, l_pct), try_parse_color("blue-600/40"), neutral_200(), blue(600), ...}
linear_gradient(180., linear_color_stop(a, 0.), linear_color_stop(b, 1.)) -> Background
```

## 4. Fonts and text `[C]`

```rust
// TextSystem (cx.text_system(): &Arc<TextSystem>)
pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()>
pub fn all_font_names(&self) -> Vec<String>

cx.text_system()
    .add_fonts(vec![Cow::Borrowed(include_bytes!("../assets/fonts/demo.ttf").as_slice())])
    .expect("failed to register fonts");
Theme::update(cx, |theme| theme.font_family = "Inter".into());   // family NAME inside the font file
```

- Register fonts right after `init`, before the first window; then name the family on the theme (Root applies `theme.font_family` to the whole window). A family set explicitly is used as-is. A misspelt/unregistered name does not error: `TextSystem::resolve_font` silently falls back through a built-in stack (`.ZedMono`, `.ZedSans`, `Helvetica`, `Segoe UI`, `Ubuntu`, `Adwaita Sans`, `Cantarell`, `Noto Sans`, `DejaVu Sans`, `Arial`), so on Windows you get Segoe UI, and it panics only when none of those is installed. The failed lookup is retried on every text run, so a wrong name also costs time each frame.
- Weights are separate files of the same family; pick with `.font_weight(..)`.

```rust
div()
    .font_family("Arial")                    // impl Into<SharedString>
    .font_weight(FontWeight::SEMIBOLD)       // THIN 100, EXTRA_LIGHT, LIGHT, NORMAL 400, MEDIUM 500, SEMIBOLD 600, BOLD 700, EXTRA_BOLD, BLACK
    .text_size(px(13.))                      // impl Into<AbsoluteLength>; or text_xs (0.75rem) text_sm (0.875) text_base (1) text_lg (1.125) text_xl (1.25) text_2xl text_3xl
    .line_height(px(18.))                    // impl Into<DefiniteLength>: px(..), rems(..), relative(1.4)
    .text_color(cx.theme().muted_foreground) // impl Into<Hsla>
    .italic().underline().line_through().text_center()
    .child("text")
// StyledExt shortcuts: .font_medium() .font_semibold() .font_bold() ...
// truncation: .truncate() == overflow_hidden + whitespace_nowrap + text_ellipsis (needs a bounded width: add .min_w_0() on flex children)
//   .whitespace_nowrap() .whitespace_normal() .text_ellipsis() .text_ellipsis_start() .text_ellipsis_middle() .line_clamp(2)
```

Text styles inherit down the tree. Children can be `&'static str`, `String`, `SharedString`, or `StyledText::new(..)`.

## 5. Assets, icons, images

```rust
pub trait AssetSource: 'static + Send + Sync {                                    // gpui_kit::AssetSource
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}
```

- `gpui_kit::assets::Assets` embeds only the 104 default component icons (`icons/<name>.svg`); its `load` returns `Err` for unknown paths and `Ok(None)` for `""`.
- `gpui_kit::assets::AllAssets` embeds the full Lucide catalog (1830 SVGs, about +1 MiB).
- `gpui_kit::assets::icon_assets!(Name, [Variant, ..])` builds a source with selected catalog icons (borrowed static bytes).

```rust
// [C] own files (rust-embed = "8") + selected Lucide extras + default component icons
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct Embedded;

gpui_kit::assets::icon_assets!(ExtraIcons, [Scissors, Merge, FileImage]);

pub struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() { return Ok(None); }
        if let Some(file) = Embedded::get(path) { return Ok(Some(file.data)); }
        if let Some(bytes) = ExtraIcons.load(path)? { return Ok(Some(bytes)); }
        gpui_kit::assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths: Vec<SharedString> =
            Embedded::iter().filter(|p| p.starts_with(path)).map(|p| p.to_string().into()).collect();
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(gpui_kit::assets::Assets.list(path)?);
        paths.sort(); paths.dedup();
        Ok(paths)
    }
}
// [C] hand-written alternative: match path { "icons/merge.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../assets/icons/merge.svg")))), _ => gpui_kit::assets::Assets.load(path) }
```

Icons `[C]`

```rust
// gpui_kit::component::{Icon, IconName, IconNamed}
Icon::new(icon: impl Into<Icon>) -> Icon          // accepts component::IconName, assets::IconName, any IconNamed, an Icon
Icon::empty() / Icon::default()
.path(impl Into<SharedString>)                    // asset path, e.g. "icons/merge.svg"
.data(&[u8])                                      // raw SVG bytes, no asset path
.rotate(impl Into<Radians>)  .transform(Transformation)  .view(cx) -> Entity<Icon>
// Sizable: .xsmall() 12px  .small() 14px  .with_size(Size::Medium) 16px  .large() 24px  .with_size(px(18.))   (rem based)
// Styled: .text_color(..) sets the colour; .size_5() etc. also work. No size => current text size; no colour => current text colour.

Icon::new(IconName::Search).small().text_color(cx.theme().muted_foreground)
Icon::new(gpui_kit::assets::IconName::Scissors).size_5()      // needs AllAssets or icon_assets!
Icon::empty().path("icons/merge.svg").with_size(px(18.))
div().child(IconName::Check)                                   // IconName is itself an element
svg().path("icons/merge.svg").size_4().text_color(rgb(0x888888))   // raw gpui element; no size => zero sized
```

SVGs are rendered as alpha masks tinted with `text_color` (monochrome; use `stroke="currentColor"`). For coloured SVG use `img("path.svg")`.

`gpui_kit::component::IconName` variants (the only ones `Assets` can load): ALargeSmall, ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Asterisk, Ban, BatteryCharging, BatteryFull, BatteryLow, BatteryMedium, BatteryWarning, Battery, Bell, BookOpen, Bot, Building2, Calendar, CaseSensitive, ChartPie, Check, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, ChevronsUpDown, CircleAlert, CircleCheck, CircleUser, CircleX, Close, Copy, Cpu, Dash, Delete, EllipsisVertical, Ellipsis, ExternalLink, EyeOff, Eye, FileText, File, FolderClosed, FolderOpen, Folder, Frame, GalleryVerticalEnd, Github, Globe, HardDrive, HeartOff, Heart, Inbox, Info, Inspector, LayoutDashboard, LoaderCircle, Loader, Map, Maximize, MemoryStick, Menu, Minimize, Minus, Moon, Network, Palette, PanelBottomOpen, PanelBottom, PanelLeftClose, PanelLeftOpen, PanelLeft, PanelRightClose, PanelRightOpen, PanelRight, Pause, Play, Plus, Redo2, Redo, RefreshCw, Replace, ResizeCorner, RotateCw, Search, Settings2, Settings, SortAscending, SortDescending, SquareTerminal, StarFill, StarOff, Star, Sun, ThumbsDown, ThumbsUp, TriangleAlert, Undo2, Undo, User, WindowClose, WindowMaximize, WindowMinimize, WindowRestore. `gpui_kit::assets::IconName` has the whole Lucide set (PascalCase of the file stem, `IconName::ALL`).

Images `[C]`

```rust
pub fn img(source: impl Into<ImageSource>) -> Img
pub enum ImageSource { Resource(Resource), Render(Arc<RenderImage>), Image(Arc<Image>), Custom(Arc<dyn Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>>>) }
// From: &str / String / SharedString (URL if it parses as one, else AssetSource path), PathBuf / &Path / Arc<Path> (file), Arc<RenderImage>, Arc<Image>
// StyledImage: .object_fit(ObjectFit::{Fill, Contain (default), Cover, ScaleDown, None}) .grayscale(bool) .with_fallback(|| AnyElement) .with_loading(|| AnyElement)
RenderImage::new(data: impl Into<SmallVec<[image::Frame; 1]>>) -> RenderImage     // Vec<Frame> converts; doc: "in BGRA format"
Image::from_bytes(format: ImageFormat, bytes: Vec<u8>) -> Image                    // gpui_kit::ImageFormat::{Png, Jpeg, Webp, Gif, Svg, Bmp, Tiff, Ico, Pnm}
App::drop_image(&mut self, image: Arc<RenderImage>, current_window: Option<&mut Window>)
Window::drop_image(&mut self, data: Arc<RenderImage>) -> Result<()>
```

```rust
use image::{Frame, RgbaImage};
/// Straight-alpha RGBA8 -> RenderImage. Frames must hold **BGRA** bytes (gpui's own decoders swap R and B, no premultiply).
pub fn render_image_from_rgba(width: u32, height: u32, mut rgba: Vec<u8>) -> Option<Arc<RenderImage>> {
    for pixel in rgba.chunks_exact_mut(4) { pixel.swap(0, 2); }       // skip when the source is already BGRA
    let buffer = RgbaImage::from_raw(width, height, rgba)?;            // None if len != w*h*4
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Contain).rounded(px(6.))   // or img(image)
img(Arc::new(Image::from_bytes(ImageFormat::Png, bytes))).w(px(120.)).h(px(160.))             // encoded bytes, decoded lazily
img("images/logo.png").size(px(64.))                                                           // AssetSource path
img(std::path::PathBuf::from("C:/tmp/page.png")).max_w(px(320.))                               // file on disk

// replacing a bitmap: free the old GPU texture explicitly
if let Some(old) = std::mem::replace(&mut self.image, new_image) { cx.drop_image(old, Some(window)); }
```

- `img` with no size uses the bitmap's pixel size (divided by its scale factor); with only height set, width follows the aspect ratio.
- Each `RenderImage` gets a new `ImageId`; its texture lives in the window's sprite atlas **until `drop_image`** — dropping the `Arc` alone does not free it. Thumbnail grids must drop replaced/removed images.
- `Arc<Image>` sources are cached by content hash in the app asset cache; release with `image.remove_asset(cx)`. Path/URL sources: wrap a subtree with `div().image_cache(retain_all("id"))` or use the default asset cache.
- Max texture 16384 x 16384 device px on Windows (DirectX atlas limit).

## 6. Layout and styling `[C]`

All style methods come from `Styled` (Tailwind names). Length arguments: sizes take `impl Into<Length>` (`px(..)`, `rems(..)`, `relative(0.5)`, `auto()`), padding/gap `DefiniteLength`, borders/radii `AbsoluteLength`.

```rust
h_flex()  // == div().flex().flex_row().items_center()        gpui_kit::component::{h_flex, v_flex}
v_flex()  // == div().flex().flex_col()
// size: .w(px(240.)) .h(px(30.)) .size(px(16.)) .size_full() .w_full() .h_full() .min_w_0() .min_h_0() .max_w(px(..)) .w_1_2() .size_4()
// flex: .flex_1() .flex_none() .flex_grow_1() .flex_grow(2.) .flex_shrink_0() .flex_wrap() .items_start/center/end/stretch() .justify_start/center/end/between/around()
// space: .gap_2() .gap(px(6.)) .gap_x_1() .p_2() .px_3() .py_1p5() .pt(px(3.)) .m_2() .mx_auto() .ml(px(4.)) .mt_neg_1()      suffix n => n * 0.25rem
// box: .bg(color) .border_1() .border_b_1() .border(px(1.5)) .border_color(c) .border_dashed() .rounded(px(8.)) .rounded_md() .rounded_full() .rounded_t(px(8.))
//      .shadow_sm() .shadow_md() .shadow_lg() .shadow(vec![BoxShadow]) .opacity(0.5) .overflow_hidden() .overflow_x_hidden()
// position: .relative() .absolute() .inset_0() .top(px(8.)) .right_0() .bottom_2() .left(px(4.))
// visibility/cursor: .visible() .invisible() .hidden() .cursor_pointer() .cursor_default() .cursor_grab() .cursor_text() .cursor_not_allowed()
// children: .child(impl IntoElement) .children(impl IntoIterator<Item = impl IntoElement>)   (Option<T> is an iterator: .children(maybe_element))
// conditional (FluentBuilder): .when(cond, |this| ..) .when_some(option, |this, value| ..) .when_else(cond, f, g) .map(|this| ..)
```

`.id(..)` turns `Div` into `Stateful<Div>`; it is required for `on_click`, `on_hover`, `active`, `tooltip`, `on_drag`, `overflow_*_scroll`, `track_scroll`, `focusable`. `hover`, `on_mouse_down/up/move`, `on_drop`, `drag_over`, `on_action`, `key_context`, `track_focus`, `occlude` work without an id. `ElementId` from `&'static str`, `SharedString`, `String`, `usize`, `u64`, `(&'static str, usize)`, `(&'static str, EntityId)`, `(ElementId, impl Into<SharedString>)`. Ids only need to be unique among siblings.

```rust
div()
    .id(("row", ix))
    .group("row")                                                  // name a hover/active group
    .relative().h(px(30.)).px_2().flex().items_center().gap_2().rounded(px(6.))
    .cursor_pointer()
    .hover(|style| style.bg(muted))                                // FnOnce(StyleRefinement) -> StyleRefinement
    .active(|style| style.opacity(0.8))                            // while pressed (needs id)
    .when(ix == 0, |this| this.bg(accent).font_weight(FontWeight::MEDIUM))
    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
    .child(div().invisible().group_hover("row", |style| style.visible()).child("x"))
    .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {      // Fn(&ClickEvent, &mut Window, &mut App)
        if event.modifiers().secondary() { this.items.remove(ix); }          // also .click_count(), .position()
        cx.notify();
    }))
    .on_double_click(|_event, _window, _cx| {})                    // gpui_kit::component::InteractiveElementExt
    .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, _window, cx| {
        this.menu_at = Some(event.position);                       // fields: button, position, modifiers, click_count, first_mouse
        cx.notify();
    }))
    .on_hover(cx.listener(|_this, hovered: &bool, _window, cx| cx.notify()))
```

Inside listeners: `cx.stop_propagation()` stops mouse events reaching elements below; `window.prevent_default()` (e.g. stops focus-on-mouse-down). Capture `cx.theme()` colours into locals before closures (they are `Copy`).

Drag and drop `[C]`

```rust
fn on_drag<T: 'static, W: 'static + Render>(self, value: T, constructor: impl Fn(&T, Point<Pixels>, &mut Window, &mut App) -> Entity<W> + 'static) -> Self  // needs id
fn on_drop<T: 'static>(self, listener: impl Fn(&T, &mut Window, &mut App) + 'static) -> Self
fn drag_over<S: 'static>(self, f: impl 'static + Fn(StyleRefinement, &S, &mut Window, &mut App) -> StyleRefinement) -> Self
fn can_drop(self, predicate: impl Fn(&dyn Any, &mut Window, &mut App) -> bool + 'static) -> Self
fn on_drag_move<T: 'static>(self, listener: impl Fn(&DragMoveEvent<T>, &mut Window, &mut App) + 'static) -> Self

// in-app: payload type + a view that renders the drag ghost
.on_drag(DragItem { ix, label: label.clone() }, |item, _cursor_offset, _window, cx| cx.new(|_| DragGhost { label: item.label.clone() }))
.drag_over::<DragItem>(move |style, _item, _window, _cx| style.bg(accent))
.on_drop(cx.listener(move |this, item: &DragItem, _window, cx| { this.items.swap(ix, item.ix); cx.notify(); }))

// files from the OS: the payload type is gpui_kit::ExternalPaths (paths() -> &[PathBuf])
div().id("drop-zone").border_2().border_dashed().border_color(border).rounded(px(12.))
    .drag_over::<ExternalPaths>(move |style, _paths, _window, _cx| style.border_color(ring).bg(accent))
    .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
        this.files.extend(paths.paths().iter().cloned());
        cx.notify();
    }))
```

`cx.has_active_drag()`, `cx.stop_active_drag(window)`. A full-window drop target is just a `size_full` div with these handlers. (Compiled; OS file drops not exercised at runtime.)

Scrolling `[C]`

```rust
// plain gpui: needs id; in a flex column also give the scroller a bounded height (.flex_1().min_h_0() or .h(..))
div().id("list").size_full().overflow_y_scroll().track_scroll(&self.scroll)      // ScrollHandle::new(); also overflow_x_scroll / overflow_scroll
// ScrollHandle: .offset() .set_offset(point) .max_offset() .scroll_to_item(ix) .scroll_to_top_of_item(ix) .scroll_to_bottom() .bounds() .bounds_for_item(ix)

// gpui_kit::component::scroll::{ScrollableElement, Scrollbar, ScrollbarAxis, ScrollbarMode}
div().h(px(200.)).overflow_y_scrollbar().child(tall)          // -> Scrollable<Div>: scroll area + themed scrollbar, state keyed by call site
    // also .overflow_x_scrollbar() .overflow_scrollbar(); .id("..") on the result when one call site makes several
div().relative().h(px(200.))                                   // manual: own ScrollHandle + overlay scrollbar
    .child(div().id("list2").size_full().overflow_y_scroll().track_scroll(&self.scroll))
    .vertical_scrollbar(&self.scroll)                          // or .horizontal_scrollbar(&h) / .scrollbar(&h, ScrollbarAxis::Both)
div().absolute().inset_0().child(Scrollbar::vertical(&self.scroll))   // explicit element; Scrollbar::new(&h).axis(..).mode(ScrollbarMode::Always)
// ScrollbarHandle is implemented for ScrollHandle, UniformListScrollHandle, ListState, VirtualListScrollHandle
```

Overlays and z-order `[C]`

```rust
pub fn deferred(child: impl IntoElement) -> Deferred          // paints after all ancestors; .with_priority(usize) orders deferred layers
pub fn anchored() -> Anchored                                 // .position(Point<Pixels>) .anchor(Anchor) .offset(point) .snap_to_window() .snap_to_window_with_margin(px(8.)) .position_mode(AnchoredPositionMode::{Window, Local})
// Anchor::{TopLeft, TopRight, BottomLeft, BottomRight, TopCenter, BottomCenter, LeftCenter, RightCenter}   (replaces the old `Corner`)

.when_some(self.menu_at, |this, position| this.child(
    deferred(anchored().position(position).anchor(Anchor::TopLeft).snap_to_window_with_margin(px(8.)).child(
        div().occlude()                                        // blocks hover/clicks for everything underneath
            .w(px(200.)).p_1().bg(popover).border_1().border_color(border).rounded(px(8.)).shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| { this.menu_at = None; cx.notify(); }))
            .child("Menu"),
    )).with_priority(1)))
```

There is no z-index: paint order = tree order; later siblings are on top; `deferred` lifts above the whole window; `.occlude()` / `.block_mouse_except_scroll()` control hit testing. A scrim is `div().absolute().inset_0().occlude().bg(cx.theme().overlay)`. Elements are clipped by ancestors with `overflow_hidden`/scroll unless deferred.

Virtual lists `[C]`

```rust
// gpui_kit::component::{v_virtual_list, h_virtual_list, VirtualListScrollHandle}   (also in gpui_kit::base)
pub fn v_virtual_list<R: IntoElement, V: Render>(view: Entity<V>, id: impl Into<ElementId>, item_sizes: Rc<Vec<Size<Pixels>>>,
    f: impl 'static + Fn(&mut V, Range<usize>, &mut Window, &mut Context<V>) -> Vec<R>) -> VirtualList
// VirtualList: .track_scroll(&VirtualListScrollHandle) .with_sizing_behavior(..)      handle: .scroll_to_item(ix, ScrollStrategy::Top) .scroll_to_bottom()

let sizes = Rc::new(vec![size(px(240.), px(30.)); self.items.len()]);     // only height is used for v_; rows may differ
div().relative().h(px(300.)).child(
    v_virtual_list(cx.entity(), "rows", sizes, |this, range, _window, _cx| {
        range.map(|ix| div().id(ix).h(px(30.)).child(this.items[ix].clone())).collect::<Vec<_>>()
    }).track_scroll(&self.rows),
).vertical_scrollbar(&self.rows)

// gpui uniform_list (all rows the same height, measured from the first)
pub fn uniform_list<R: IntoElement>(id: impl Into<ElementId>, item_count: usize, f: impl 'static + Fn(Range<usize>, &mut Window, &mut App) -> Vec<R>) -> UniformList
uniform_list("uniform", self.items.len(), cx.processor(|this, range: Range<usize>, _window, _cx| {
    range.map(|ix| div().h(px(30.)).child(this.items[ix].clone())).collect::<Vec<_>>()
})).h(px(300.))                                                // .track_scroll(&UniformListScrollHandle)
```

## 7. State and async `[C]`

```rust
// entities
cx.new(|cx: &mut Context<T>| T { .. }) -> Entity<T>            // on App, Context, AsyncApp alike (no Result)
entity.read(cx) -> &T      entity.update(cx, |t, cx: &mut Context<T>| ..) -> R      entity.downgrade() -> WeakEntity<T>
weak.upgrade() -> Option<Entity<T>>      weak.update(cx, |t, cx| ..) -> Result<R>      weak.update_in(cx, |t, window, cx| ..) -> Result<R>
cx.entity() -> Entity<T>      cx.weak_entity() -> WeakEntity<T>      cx.notify()            // inside Context<T>
impl Render for T { fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement }
#[derive(IntoElement)] struct X; impl RenderOnce for X { fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement }
window.use_state(cx, |window, cx| S { .. }) -> Entity<S>       // element-local state inside RenderOnce (keyed by call site); use_keyed_state(id, cx, ..) in lists

// listeners (Context<T>)
pub fn listener<E: ?Sized>(&self, f: impl Fn(&mut T, &E, &mut Window, &mut Context<T>) + 'static) -> impl Fn(&E, &mut Window, &mut App) + 'static
pub fn processor<E, R>(&self, f: impl Fn(&mut T, E, &mut Window, &mut Context<T>) -> R + 'static) -> impl Fn(E, &mut Window, &mut App) -> R + 'static
window.listener_for(&entity, |t, event, window, cx| ..)        // same, for another entity (usable from RenderOnce)

// async
pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R> where AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R + 'static            // Context<T>
pub fn spawn_in<AsyncFn, R>(&self, window: &Window, f: AsyncFn) -> Task<R> where AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncWindowContext) -> R + 'static
pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R> where AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static                          // App / AsyncApp
fn background_spawn<R: Send + 'static>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>                                // AppContext trait
cx.background_executor().spawn(fut) -> Task<R>       cx.background_executor().timer(Duration) -> Task<()>
```

```rust
pub struct Job { pub progress: f32, task: Option<Task<()>> }
pub enum JobEvent { Done(u64) }
impl EventEmitter<JobEvent> for Job {}

impl Job {
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {                       // async closure syntax
            let result = cx.background_spawn(async move { heavy() }).await;    // thread pool; the future must be Send
            cx.background_executor().timer(Duration::from_millis(50)).await;
            this.update(cx, |this, cx| {                                        // Result: Err when the entity was dropped
                this.progress = 1.0;
                cx.emit(JobEvent::Done(result));
                cx.notify();
            }).ok();
            let _compact = cx.update(|cx| cx.global::<Settings>().compact);     // AsyncApp::update returns R directly
        }));
    }
    pub fn start_in_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let value = cx.background_spawn(async move { heavy() }).await;
            this.update_in(cx, |this, window, cx| { this.progress = value as f32; window.set_window_title("done"); cx.notify(); }).ok();
        }).detach();
    }
}
```

- **Dropping a `Task` cancels it.** Store it (replacing the field cancels the previous run) or `.detach()`. `Task::ready(v)`.
- Timer loop: `loop { cx.background_executor().timer(d).await; if this.update(cx, ..).is_err() { break; } }`.
- `AsyncWindowContext::update(|window, cx| ..) -> Result<R>`; `AsyncApp::update(|cx| ..) -> R`.
- `cx.defer(|cx: &mut App| ..)`, `cx.defer_in(window, |this, window, cx| ..)`, `cx.on_next_frame(window, |this, window, cx| ..)`, `window.request_animation_frame()`, `window.refresh()`, `cx.refresh_windows()`.

```rust
// subscriptions: keep the returned Subscription alive (field) or .detach()
let _subscriptions = vec![
    cx.subscribe(&job, |_this, _job: Entity<Job>, event: &JobEvent, cx| { let JobEvent::Done(_v) = event; cx.notify(); }),
    cx.observe(&job, |_this, _job, cx| cx.notify()),                            // fires on the job's cx.notify()
    cx.observe_global::<Settings>(|_this, cx| cx.notify()),
    cx.subscribe_in(&job, window, |_this, _job: &Entity<Job>, _event: &JobEvent, window, _cx| window.refresh()),
];
// also: cx.observe_in(&e, window, |this, e, window, cx| ..)  cx.observe_release(&e, |this, e, cx| ..)  cx.on_release(|this, cx| ..)
//       cx.observe_window_bounds / observe_window_activation / observe_window_appearance(window, |this, window, cx| ..)

// globals
#[derive(Default)] pub struct Settings { pub compact: bool }
impl Global for Settings {}
cx.set_global(Settings::default());
let compact = cx.global::<Settings>().compact;                                  // panics when unset; cx.try_global::<Settings>() -> Option<&Settings>
cx.global_mut::<Settings>().compact = true;                                     // notifies observers
cx.update_global::<Settings, _>(|settings, _cx| settings.compact = false);      // BorrowAppContext
cx.default_global::<Settings>();  cx.has_global::<Settings>();
cx.observe_global::<Settings>(|cx| cx.refresh_windows()).detach();              // App-level form
```

## 8. Actions and key bindings `[C]`

```rust
actions!(shorui, [OpenPalette, ClosePalette, SelectNext, SelectPrev, Confirm, Quit]);   // gpui_kit::actions! (pub unit structs, name "shorui::OpenPalette")

#[derive(Clone, PartialEq, Debug, Action)]                    // action with data; no_json avoids serde/schemars requirements
#[action(namespace = shorui, no_json)]
pub struct OpenTool { pub id: usize }

pub fn new<A: Action>(keystrokes: &str, action: A, context: Option<&str>) -> KeyBinding   // KeyBinding::new PANICS on a bad keystroke/context string

cx.bind_keys([
    KeyBinding::new("secondary-k", OpenPalette, None),                   // None = any context
    KeyBinding::new("escape", ClosePalette, Some("Palette")),
    KeyBinding::new("down", SelectNext, Some("Palette")),
    KeyBinding::new("enter", Confirm, Some("Palette")),
    KeyBinding::new("ctrl-1", OpenTool { id: 1 }, Some("Shell")),
    KeyBinding::new("alt-shift-t", OpenTool { id: 2 }, Some("Shell && !Palette")),
    KeyBinding::new("ctrl-k ctrl-o", OpenPalette, Some("Shell > Sidebar")),   // chord (space separated) + descendant predicate
]);
cx.on_action(|_: &Quit, cx: &mut App| cx.quit());                        // app-global handler
```

Keystroke syntax: `[secondary-][ctrl-][alt-][shift-][cmd-|super-|win-][fn-]key`. `secondary` = cmd on macOS, ctrl elsewhere. Key names (lowercase): letters/digits/punctuation, `enter`, `escape`, `tab`, `space`, `backspace`, `delete`, `insert`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `f1`..`f24`, `menu`. A single uppercase letter means shift+letter. Context predicates: `Name`, `A && B`, `A || B`, `!A`, `A > B` (B below A), `key == value`, `key != value`.

Element side:

```rust
v_flex()
    .id("palette")
    .key_context("Palette")                                              // contexts stack from the focused element up to the root
    .track_focus(&self.focus)
    .on_action(cx.listener(Self::open))                                  // fn open(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>)
    .on_action(cx.listener(|this, _: &SelectPrev, _window, cx| { this.selected = this.selected.saturating_sub(1); cx.notify(); }))
    .on_action(cx.listener(|_this, action: &OpenTool, _window, _cx| { let _ = action.id; }))
    .on_key_down(cx.listener(|_this, event: &KeyDownEvent, _window, cx| {
        let key = &event.keystroke;                                      // KeyDownEvent { keystroke, is_held, prefer_character_input }
        if key.key == "f2" && !key.modifiers.modified() && !event.is_held { cx.stop_propagation(); }
    }))                                                                  // Keystroke { modifiers: Modifiers { control, alt, shift, platform, function }, key: String, key_char: Option<String> }
// also .capture_action::<A>(..) .capture_key_down(..) .on_key_up(..) .on_modifiers_changed(..)

window.dispatch_action(Box::new(ClosePalette), cx);      // from the focused element (deferred to the end of the effect cycle)
cx.dispatch_action(&Quit);                               // App: same, on the active window
handle.dispatch_action(&OpenPalette, window, cx);        // from a specific FocusHandle's element
window.is_action_available(&OpenPalette, cx) -> bool
cx.intercept_keystrokes(|event: &KeystrokeEvent, _window, cx| { if event.keystroke.key == "f12" { cx.stop_propagation(); } })  // before bindings -> Subscription
cx.observe_keystrokes(|event, _window, _cx| { /* event.keystroke, event.action, event.context_stack */ })                     // after dispatch -> Subscription
```

Dispatch rules (from `window.rs`):
1. A key press matches bindings against the context stack of the **focused** element's path (root -> focused). All matching bindings are tried, deepest context first; each dispatches its action and stops at the first one whose handler does not call `cx.propagate()`.
2. An action runs: global `cx.on_action` listeners (capture) -> element `capture_action` root->focused -> element `on_action` focused->root (bubble; handling stops propagation unless the handler calls `cx.propagate()`) -> global `cx.on_action` (bubble, only if nothing handled it).
3. If no binding consumed the key, `on_key_down` listeners run on the same path, then text input goes to the focused input.
4. With **nothing focused** the path is only the window root node: `on_action` handlers on your own elements do not run. Use `cx.on_action` for truly global shortcuts, or keep a root `FocusHandle` focused: `track_focus` on the top element, focus it at startup, and restore it with `cx.on_focus_lost(window, move |_this, window, cx| window.focus(&root_handle, cx))` `[C]`.

Text input interaction: a focused `Input` adds context `"Input"`. Bindings are only skipped in favour of typing when the platform flags `prefer_character_input` (AltGr combos). So `KeyBinding::new("secondary-k", OpenPalette, None)` fires while an input is focused (no `-k` binding exists in the Input context; derived from source, not run). Keys the Input context binds on Windows/Linux (they win over a `None`-context binding when the input handles them): `enter`, `shift-enter`, `secondary-enter`, `escape`, `tab`, `shift-tab`, arrows (+shift/alt/ctrl variants), `home`, `end`, `pageup`, `pagedown`, `backspace`, `delete`, `ctrl-a/c/x/v/z/y/e/f/h`, `ctrl-.`, `ctrl-[`, `ctrl-]`, `ctrl-shift-a/e`. For a **single-line** `Input`:
- `up` / `down` / `tab` / `shift-tab` dispatch `input::MoveUp` / `MoveDown` / `IndentInline` / `OutdentInline`, which the single-line input does **not** handle, so the next matching binding (yours, in an ancestor context) runs; or listen for the input's own actions on an ancestor: `.on_action(cx.listener(|this, _: &gpui_kit::component::input::MoveDown, window, cx| ..))` `[C]`.
- `enter` emits `InputEvent::PressEnter` and propagates; `escape` propagates unless `clean_on_escape` is set; `backspace` on an empty input propagates.

## 9. Focus `[C]`

```rust
cx.focus_handle() -> FocusHandle                                  // App / Context; default: NOT a tab stop
handle.tab_stop(true).tab_index(0)                                // builder on the handle
window.focus(&handle, cx)        handle.focus(window, cx)         // pub fn focus(&mut self, handle: &FocusHandle, cx: &mut App)  <- takes cx in this version
window.blur(cx)   window.focused(cx) -> Option<FocusHandle>   window.focus_next(cx)   window.focus_prev(cx)
handle.is_focused(window) -> bool                                 // exactly this handle
handle.contains_focused(window, cx) -> bool                       // this or a descendant
handle.within_focused(window, cx) -> bool                         // this or an ancestor is focused
handle.downgrade() -> WeakFocusHandle (.upgrade())
pub trait Focusable: 'static { fn focus_handle(&self, cx: &App) -> FocusHandle; }      // Entity<V: Focusable> is Focusable too
cx.focus_self(window)  /  cx.focus_view(&entity, window)          // need Focusable
// element: .track_focus(&handle) .tab_index(isize) .tab_stop(bool) .tab_group() .focusable() (Stateful)
// styles:  .focus(|s| ..) .in_focus(|s| ..) .focus_visible(|s| ..)
// listeners (Context<T>, return Subscription):
cx.on_focus(&handle, window, |this, window, cx| ..)   cx.on_blur(&handle, window, |this, window, cx| ..)
cx.on_focus_in(&handle, window, ..)   cx.on_focus_out(&handle, window, |this, event: FocusOutEvent, window, cx| ..)   cx.on_focus_lost(window, |this, window, cx| ..)
```

```rust
// open an overlay and give focus back when it closes
pub fn open(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
    self.previous_focus = window.focused(cx);
    self.open = true;
    self.query.update(cx, |input, cx| input.focus(window, cx));   // InputState::focus
    cx.notify();
}
fn close(&mut self, _: &ClosePalette, window: &mut Window, cx: &mut Context<Self>) {
    self.open = false;
    if let Some(previous) = self.previous_focus.take() { window.focus(&previous, cx); } else { window.blur(cx); }
    cx.notify();
}
```

- `window.open_dialog` / `open_sheet` do this bookkeeping themselves (focus the dialog, restore the previous handle on close).
- Tab order: Root binds `tab`/`shift-tab` to `focus_next`/`focus_prev` over elements with `tab_stop(true)`, ordered by `tab_index` then tree order. Buttons, checkboxes, inputs etc. are tab stops by default (`.tab_stop(false)` / `.tab_index(n)` on each).
- Focus ring: `gpui_kit::component::ThemeStyled::focus_ring_style(self, window: &Window, cx: &App)` (element must be `ParentElement`; tints the border with `theme.ring` and paints a 3 px ring **outside** the element, clipped by `overflow_hidden` ancestors) — `.when(focused, |this| this.focus_ring_style(window, cx))`. Components: `.focus_ring(false)` (`FocusableExt`) per control, or `theme.focus_ring = false` globally. `.popover_style(cx)` and `.rounded_full_style(cx)` live on the same trait.
- Focus trap: `gpui_kit::component::FocusTrapElement::focus_trap(self, id, &handle)` makes Tab cycle inside the container (dialogs/sheets already do).
- A focused element removed from the tree leaves focus on nothing (see dispatch rule 4).

## 10. Components (`gpui_kit::component::..`)

`Sizable` (`.xsmall()`, `.small()`, `.large()`, `.with_size(Size::Medium | px(28.))`; there is **no `.medium()`**, Medium is the default). Heights at 16 px rem:

| | XSmall | Small | Medium | Large |
| --- | --- | --- | --- | --- |
| Button (label) | 20 (`h_5`) | 24 (`h_6`) | 32 (`h_8`) | 32 (`h_8`, wider padding) |
| Button (icon only) | 20 | 24 | 32 | 32 |
| Input / Select | 20 | 24 | 32 | 44 (`h_11`) |
| TabBar (underline) | 20 (26) | 24 (30) | 32 (36) | 36 (44) — fixed px |
| Progress | 4 | 6 | 8 | 10 — fixed px |
| Switch | 28x16 | 28x16 | 36x20 | 36x20 — fixed px |
| Icon | 12 | 14 | 16 | 24 |

Other traits: `Disableable::disabled(bool)`, `Selectable::selected(bool)` / `.open(bool)`, `FocusableExt::focus_ring(bool)`, `Collapsible::collapsed(bool)`. "State" = needs an `Entity` you create once with `cx.new` and keep in your view.

### Button — no state `[C]`

```rust
// button::{Button, ButtonVariants, ButtonVariant, ButtonCustomVariant, ButtonRounded, ButtonGroup, DropdownButton, Toggle, ToggleGroup}
Button::new(id: impl Into<ElementId>)             // impl Styled + ParentElement + InteractiveElement + Sizable + Disableable + Selectable
.label(impl Into<SharedString>)  .icon(impl Into<ButtonIcon>)  .child(..)        // icon only = no label and no children
.primary() .secondary() .danger() .warning() .success() .info() .ghost() .link() .text() .custom(ButtonCustomVariant) .with_variant(ButtonVariant)
.outline()  .compact()  .rounded(impl Into<ButtonRounded>)  .loading(bool)  .loading_icon(icon)  .dropdown_caret(bool)  .toggled(bool)
.tooltip(impl Into<SharedString>)  .tooltip_placement(Placement)  .tooltip_with_action(text, &dyn Action, context: Option<&str>)
.on_click(impl Fn(&ClickEvent, &mut Window, &mut App) + 'static)  .on_hover(impl Fn(&bool, &mut Window, &mut App) + 'static)
.tab_index(isize) .tab_stop(bool) .accessibility_label(..)
ButtonCustomVariant::new(cx: &App) .color(Hsla) .foreground(Hsla) .hover(Hsla) .active(Hsla) .shadow(bool)

Button::new("primary").primary().label("Primary").on_click(cx.listener(|this, _event: &ClickEvent, window, cx| this.submit(window, cx)))
Button::new("ghost").ghost().icon(IconName::Settings).tooltip("Settings")
Button::new("danger").danger().small().label("Delete")
Button::new("busy").primary().label("Export").loading(self.busy).disabled(self.busy)
Button::new("sel").ghost().compact().label("Toggle").selected(self.open)
Button::new("sized").with_size(px(28.)).rounded(px(14.)).label("28px")
Button::new("rich").ghost().w_full().justify_start().child(h_flex().gap_2().child(Icon::new(IconName::File).small()).child("Custom content"))
let custom = ButtonCustomVariant::new(cx).color(cx.theme().accent).foreground(cx.theme().accent_foreground)
    .hover(cx.theme().accent.opacity(0.8)).active(cx.theme().accent.opacity(0.6));
Button::new("custom").custom(custom).large().label("Custom")
ButtonGroup::new("group").child(Button::new("left").label("Left")).child(Button::new("right").label("Right"))
```

Default variant = bordered `theme.button`; `danger/success/warning/info` are **tinted** (20% fill, coloured text), not solid. Medium/Large label text is `text_base` (1rem).

### Input — state: `Entity<InputState>` `[C]`

```rust
// input::{Input, InputState, InputEvent, Textarea, TextareaState, NumberInput, OtpInput, MaskPattern, ...}
InputState::new(window: &mut Window, cx: &mut Context<Self>)                 // single line
  builders: .placeholder(..) .default_value(..) .masked(bool) .clean_on_escape() .pattern(regex::Regex) .validate(|&str, &mut App| -> bool) .mask_pattern(..) .context_menu(bool)
  read:  .value() -> SharedString   .text() -> &Rope   .unmask_value()   .selected_value()   .cursor()   .is_editable()
  write: .set_value(impl Into<InputContent>, window, cx)   // no Change event, clears undo
         .replace_all(text, window, cx)  .insert(text, window, cx)  .clean(window, cx)  .select_all(window, cx)
         .set_placeholder(text, window, cx)  .set_masked(bool, window, cx)  .set_disabled(bool, cx)  .set_readonly(bool, cx)  .set_loading(bool, window, cx)
         .focus(window, cx)                                                  // impl Focusable
pub enum InputEvent { Change, PressEnter { secondary: bool, shift: bool }, Focus, Blur }

Input::new(&Entity<InputState>)                    // Styled + Sizable
.prefix(el) .suffix(el) .cleanable(bool) .mask_toggle() .appearance(bool) .bordered(bool) .focus_bordered(bool)
.disabled(bool) .readonly(bool) .tab_index(isize) .h(..) .h_full() .id(..) .on_paste(|&ClipboardItem, &mut Window, &mut App| -> bool)
```

```rust
let name = cx.new(|cx| InputState::new(window, cx).placeholder("File name").default_value("merged.pdf"));
let password = cx.new(|cx| InputState::new(window, cx).masked(true));
self._subscriptions.push(cx.subscribe_in(&name, window, |this, input, event: &InputEvent, window, cx| match event {
    InputEvent::Change => { let _value: SharedString = input.read(cx).value(); cx.notify(); }
    InputEvent::PressEnter { .. } => this.submit(window, cx),
    InputEvent::Focus | InputEvent::Blur => {}
}));
// render
Input::new(&self.name).prefix(Icon::new(IconName::Search).small()).suffix(Kbd::new(Keystroke::parse("secondary-k").unwrap())).cleanable(true)
Input::new(&self.password).mask_toggle().small()
Input::new(&self.name).appearance(false).large().w(px(240.))          // no border/background/ring: for custom shells (palette search field)
// programmatic
self.name.update(cx, |input, cx| { input.set_value("", window, cx); input.focus(window, cx); });
let value = self.name.read(cx).value();
window.focused_input(cx) -> Option<AnyInputState>                       // WindowExt
```

`InputState::new` needs a `Window`: create it in a constructor that receives one (`open_window` builder, `cx.new` inside render/handlers), not in `App`-only code.

### Checkbox, Switch, Radio — no state `[C]`

```rust
Checkbox::new(id) .label(impl Into<Text>) .checked(bool) .on_click / .on_change(impl Fn(&bool, &mut Window, &mut App) + 'static) .tooltip(..) .disabled(bool)
Switch::new(id)   .label(..) .checked(bool) .on_click / .on_change(Fn(&bool, ..)) .color(impl Into<Hsla>) .tooltip(..) .disabled(bool)
Radio::new(id)    .label(..) .checked(bool) .on_click(Fn(&bool, ..)) .disabled(bool)
RadioGroup::horizontal(id) / ::vertical(id) .selected_index(Option<usize>) .child(impl Into<Radio>) .children(..) .on_click / .on_change(Fn(&usize, ..)) .disabled(bool)

Checkbox::new("agree").label("Keep bookmarks").checked(self.checked)
    .on_click(cx.listener(|this, checked: &bool, _window, cx| { this.checked = *checked; cx.notify(); }))     // &bool = NEW value
Switch::new("compress").label("Compress").checked(self.checked).small().on_click(cx.listener(|this, checked: &bool, _, cx| { this.checked = *checked; cx.notify(); }))
RadioGroup::horizontal("mode").selected_index(self.radio).child(Radio::new("fast").label("Fast")).child(Radio::new("best").label("Best"))
    .on_click(cx.listener(|this, ix: &usize, _window, cx| { this.radio = Some(*ix); cx.notify(); }))
```

All controlled: you store the value and pass it back each render.

### Slider — state: `Entity<SliderState>` `[C]`

```rust
// slider::{Slider, SliderState, SliderEvent, SliderValue, SliderScale}
SliderState::new() .min(f32) .max(f32) .step(f32) .scale(SliderScale) .default_value(impl Into<SliderValue>)     // f32 or (f32, f32) range
state.value() -> SliderValue (.start() .end() .is_range())      state.set_value(v, window, cx)
pub enum SliderEvent { Change(SliderValue), Release(SliderValue) }
Slider::new(&state) .horizontal() .vertical() .reverse() .disabled(bool)                                        // Styled

let quality = cx.new(|_| SliderState::new().min(0.).max(100.).step(1.).default_value(80.));
cx.subscribe(&quality, |_this, _slider, event: &SliderEvent, cx| { if let SliderEvent::Change(value) = event { let _v: f32 = value.start(); cx.notify(); } })
Slider::new(&self.quality).w(px(240.))
```

### Select — state: `Entity<SelectState<D>>` `[C]`

```rust
// select::{Select, SelectState, SelectEvent, SearchableVec, SelectGroup, SelectItem, SelectDelegate};  IndexPath from component root
SelectState::new(delegate: D, selected_index: Option<IndexPath>, window, cx) .searchable(bool)
  .selected_value() -> Option<&Value>   .selected_index(cx) -> Option<IndexPath>   .set_selected_index(Option<IndexPath>, window, cx)
  .set_selected_value(&value, window, cx)   .set_items(D, window, cx)   .focus(window, cx)
pub enum SelectEvent<D> { Confirm(Option<Value>) }                      // state also emits DismissEvent when the menu closes
Select::new(&state) .placeholder(..) .title_prefix(..) .icon(..) .cleanable(bool) .menu_width(..) .menu_max_h(..) .search_placeholder(..) .disabled(bool) .appearance(bool) .empty(..)  // Sizable + Styled
// items: Vec<T> / SearchableVec<T> / SearchableVec<SelectGroup<T>> where T: SelectItem (impl for &'static str, String, SharedString; custom: title(), value(), optional render())
IndexPath::new(row) .section(n) .row(n)        // fields: section, row, column

let format = cx.new(|cx| SelectState::new(SearchableVec::new(vec!["PDF", "PNG", "JPEG"]), Some(IndexPath::new(0)), window, cx).searchable(true));
cx.subscribe(&format, |_this, _select, event: &SelectEvent<SearchableVec<&'static str>>, cx| {
    let SelectEvent::Confirm(value) = event;                             // Option<&'static str>
    cx.notify();
})
Select::new(&self.format).placeholder("Format").w(px(200.)).small()
```

### Tabs — no state `[C]`

```rust
// tab::{Tab, TabBar, TabVariant}            TabVariant::{Tab (default), Outline, Pill, Segmented, Underline}
TabBar::new(id) .selected_index(usize) .child(impl Into<Tab>) .children(..) .on_click(Fn(&usize, &mut Window, &mut App))
    .segmented() .pill() .outline() .underline() .with_variant(..) .prefix(el) .suffix(el) .menu(bool) .track_scroll(&ScrollHandle)   // Sizable + Styled
Tab::new() .label(..) .icon(..) .prefix(el) .suffix(el) .disabled(bool) .on_click(Fn(&ClickEvent, ..))        // From<&'static str | String | SharedString | Icon | IconName>

TabBar::new("tabs").segmented().small().selected_index(self.tab)
    .child(Tab::new().label("Pages")).child(Tab::new().label("Outline").icon(IconName::File))
    .on_click(cx.listener(|this, ix: &usize, _window, cx| { this.tab = *ix; cx.notify(); }))
```

The segmented control is `TabBar::segmented()`. Setting `TabBar::on_click` disables the children's own `on_click`.

### Small pieces — no state `[C]`

```rust
Kbd::new(Keystroke::parse("ctrl-shift-p").unwrap())  .outline() .appearance(false)                // kbd::Kbd; Kbd::format(&keystroke) -> "Ctrl+Shift+P"
Kbd::binding_for_action(&OpenPalette, None, window) -> Option<Kbd>                               // first binding of an action, for hints
Tag::primary().small().child("New")   Tag::secondary().outline().child("Beta")                   // tag::Tag: new/primary/secondary/danger/success/warning/info/custom(color, fg, border)/color(ColorName)
Tag::custom(cx.theme().accent, cx.theme().accent_foreground, cx.theme().border).rounded_full().child("Custom")
Badge::new().count(3).child(Icon::new(IconName::Bell))   Badge::new().dot().child(..)            // badge::Badge: .max(99) .color(..) .icon(..)
Progress::new("progress").value(42.).h(px(4.))           // progress::{Progress, ProgressCircle}: value 0.0..=100.0, .loading(true) indeterminate, .color(..)
Spinner::new().small()                                    // spinner::Spinner: .color(Hsla) .icon(..)
Skeleton::new().w(px(120.)).h_4().rounded_md()            // skeleton::Skeleton: .secondary()
Separator::horizontal()  Separator::vertical()  Separator::horizontal().label("or")  // separator::Separator: .dashed() .color(..) ; vertical needs a parent height
div().id("tip").child("Hover me").tooltip(|window, cx| Tooltip::new("Plain tooltip").build(window, cx))   // tooltip::Tooltip: .action(&dyn Action, context) .key_binding(Option<Kbd>); Tooltip::element(|window, cx| el)
Accordion::new("faq").multiple(false)                     // accordion::Accordion: .bordered(bool) .disabled(bool), Sizable
    .item(|item| item.title("Options").open(true).child("Body"))
    .item(|item| item.title("Advanced").child("More"))
    .on_toggle_click(|open_indices: &[usize], _window, _cx| {})          // controlled: pass .open(..) per item yourself
Collapsible::new().open(self.open).child("Always visible").content(div().child("Shown when open"))       // collapsible::Collapsible; .motion_id(id) animates
```

Others present in 0.7 (not detailed): `alert::Alert`, `avatar`, `breadcrumb`, `clipboard::Clipboard`, `color_picker`, `combobox`, `date_picker` / `calendar` / `time_field`, `description_list`, `empty`, `form`, `group_box`, `hover_card`, `label::Label`, `link`, `pagination`, `rating`, `setting`, `sheet`, `status_bar`, `stepper`, `table` (DataTable), `text` (markdown/html `TextView`), `toolbar`, `tree`, `dock`, `chart`/`plot`.

### Popover — no state (keeps open state internally unless controlled) `[C]`

```rust
Popover::new(id) .trigger(T: Selectable + IntoElement)        // e.g. Button
    .content(|state: &mut PopoverState, window, cx: &mut Context<PopoverState>| el)   // called every render: do not create entities inside
    .anchor(impl Into<Anchor>)  .offset(px)  .arrow(bool)  .mouse_button(MouseButton)  .overlay_closable(bool)  .appearance(bool)
    .default_open(bool)  .open(bool) + .on_open_change(Fn(&bool, &mut Window, &mut App))   .track_focus(&FocusHandle)  .trigger_style(StyleRefinement)

Popover::new("popover").anchor(Anchor::TopLeft).trigger(Button::new("pop").label("Popover"))
    .content(|_state, _window, _cx| v_flex().w(px(220.)).gap_2().child("Popover body"))
Popover::new("controlled").open(self.open)
    .on_open_change(cx.listener(|this, open: &bool, _window, cx| { this.open = *open; cx.notify(); }))
    .trigger(Button::new("pop2").label("Controlled")).child("Static children also work")
```

`anchor` names the popover's own corner: `TopLeft` opens below the trigger, left aligned.

### Dialog, Sheet, Notification — via `WindowExt` (`use gpui_kit::component::WindowExt as _`) `[C]`

```rust
fn open_dialog<F: Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static>(&mut self, cx: &mut App, build: F)     // builder re-runs every frame
fn open_alert_dialog<F: Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static>(&mut self, cx: &mut App, build: F)
fn close_dialog(&mut self, cx: &mut App)   fn close_all_dialogs(&mut self, cx: &mut App)   fn has_active_dialog(&mut self, cx: &mut App) -> bool
fn open_sheet<F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static>(&mut self, cx: &mut App, build: F)         // open_sheet_at(Placement, cx, build); close_sheet(cx)
fn push_notification(&mut self, note: impl Into<Notification>, cx: &mut App)                                     // remove_notification::<T>(cx), clear_notifications(cx)
// dialog::{Dialog, AlertDialog, DialogFooter, DialogAction, DialogClose, DialogButtonProps, DialogHeader, DialogTitle, DialogDescription}
Dialog: .title(el) .child(..) .footer(el) .w(px)/.width(px) [448px] .max_w(px) .margin_top(px) .overlay(bool) .overlay_closable(bool) .keyboard(bool) .close_button(bool)
        .on_ok(Fn(&ClickEvent, &mut Window, &mut App) -> bool) .on_cancel(.. -> bool) .on_close(Fn(&ClickEvent, ..)) .button_props(DialogButtonProps) + Styled (.p_0() ..)

window.open_dialog(cx, move |dialog, _window, _cx| {
    let view = view.clone();
    dialog.title("Rename").w(px(420.)).child(Input::new(&name))
        .footer(DialogFooter::new()
            .child(DialogClose::new().trigger(|button| button.label("Cancel")))
            .child(DialogAction::new().child(Button::new("ok").primary().label("Rename"))))
        .on_ok(move |_event, window, cx| { view.update(cx, |this, cx| this.submit(window, cx)); true })   // true closes
        .on_cancel(|_event, _window, _cx| true)
});
window.open_alert_dialog(cx, |alert, _window, _cx| alert.confirm().title("Delete 3 pages?").description("This cannot be undone.")
    .on_ok(|_, window, cx| { window.push_notification("Deleted", cx); true }));
window.open_sheet(cx, |sheet, _window, _cx| sheet.title("Inspector").size(px(360.)).child("Details"));

// notification::{Notification, NotificationType}
struct ExportDone;                                                        // id type: pushing again replaces the previous one
window.push_notification(Notification::new().id::<ExportDone>().title("Export finished").message("merged.pdf (2.1 MB)")
    .with_type(NotificationType::Success).autohide(true)                 // autohide = 5 s
    .action(|_note, _window, _cx| Button::new("show").small().label("Show")), cx);
window.push_notification(Notification::success(format!("Saved {value}")), cx);   // ::info ::success ::warning ::error
window.push_notification((NotificationType::Error, "Failed"), cx);               // or just "text"
Theme::update(cx, |theme| theme.notification.placement = Anchor::BottomRight);   // NotificationSettings { placement [TopRight], margins [top = 34 + 16], max_items [10], width [382px], delivery }
```

The dialog closure must be `Fn + 'static` and is called on every render: capture entities (clone inside), never create state in it. Esc closes (unless `.keyboard(false)`), overlay click closes (unless `.overlay_closable(false)`), focus is trapped and restored on close. Dialogs stack.

### Menus — no state `[C]`

```rust
// menu::{PopupMenu, PopupMenuItem, DropdownMenu, ContextMenuExt, AppMenuBar}
Button::new("menu").icon(IconName::Ellipsis).dropdown_menu(|menu: PopupMenu, window, cx: &mut Context<PopupMenu>| {      // DropdownMenu trait (Button, SidebarHeader/Footer)
    menu.menu("Open palette", Box::new(OpenPalette))                                  // dispatches the action; shows its key binding
        .menu_with_icon("Tool 1", IconName::File, Box::new(OpenTool { id: 1 }))
        .menu_with_check("Checked", true, Box::new(OpenTool { id: 2 }))
        .separator()
        .item(PopupMenuItem::new("Closure item").on_click(|_, _window, _cx| {}))       // closure instead of an action
        .submenu("More", window, cx, |menu, _, _| menu.menu("Nested", Box::new(OpenPalette)))
})                                                                                     // .dropdown_menu_with_anchor(Anchor::TopRight, ..)
div().id("ctx").child("Right click me").context_menu(|menu, _window, _cx| menu.menu("Open palette", Box::new(OpenPalette)))  // ContextMenuExt on any element
// PopupMenu: .label(..) .link(..) .menu_with_disabled(label, action, bool) .menu_element(action, |window, cx| el) .min_w(px) .max_w(px) .max_h(px) .scrollable(bool) .action_context(FocusHandle)
```

Menu actions are dispatched to the element that was focused when the menu opened, so the `on_action` handler must be on that focus path (or global).

### List — state: `Entity<ListState<D>>` `[C]`

```rust
// list::{List, ListState, ListDelegate, ListItem, ListEvent}
pub trait ListDelegate: Sized + 'static {
    type Item: Selectable + IntoElement;                                              // usually ListItem
    fn items_count(&self, section: usize, cx: &App) -> usize;                         // required
    fn render_item(&mut self, ix: IndexPath, window: &mut Window, cx: &mut Context<ListState<Self>>) -> Option<Self::Item>;   // required
    fn set_selected_index(&mut self, ix: Option<IndexPath>, window: &mut Window, cx: &mut Context<ListState<Self>>);          // required
    // optional: sections_count, render_section_header/footer, perform_search(&mut self, query, window, cx) -> Task<()>, confirm(secondary, ..), cancel,
    //           render_empty, render_initial, loading, render_loading, has_more, load_more, set_right_clicked_index
}
ListState::new(delegate, window, cx) .searchable(bool) .selectable(bool);  .delegate() .delegate_mut() .selected_index() .set_selected_index(..) .set_query(..) .scroll_to_item(..) .focus(window, cx)
pub enum ListEvent { Select(IndexPath), Confirm(IndexPath), Cancel }
List::new(&state) .scrollbar_visible(bool) .search_placeholder(..)                    // Sizable + Styled
ListItem::new(id) .child(..) .selected(bool) .confirmed(bool) .disabled(bool) .suffix(|window, cx| el) .separator() .on_click(..) .check_icon(..)

let tools = cx.new(|cx| ListState::new(ToolList { items: vec!["Merge".into(), "Split".into()], selected: None }, window, cx).searchable(true));
cx.subscribe(&tools, |_this, _list, event: &ListEvent, cx| { if let ListEvent::Confirm(ix) = event { let _row = ix.row; cx.notify(); } })
List::new(&self.tools)
```

Virtualised (built on `v_virtual_list`), keyboard: up/down/enter/secondary-enter/escape in context `"List"`. `searchable_list` (`SearchableVec`, `SearchableListDelegate`, `SearchableListItem`) is the item model behind `Select`/`Combobox`; it is not a standalone widget.

### Command palette — state: `Entity<CommandState>` `[C]`

```rust
// command::{Command, CommandState, CommandItem, CommandGroup, CommandEntry}
CommandState::new(window, cx);  .query(cx) -> SharedString  .set_query(text, window, cx)  .selected_index() -> Option<IndexPath>  .set_selected_index(..)  .matched_count()  .focus(window, cx)  .set_loading(bool, window, cx)
Command::new(&state)
    .item(CommandItem) .items(iter) .group(CommandGroup) .separator()
    .placeholder(..) .max_h(impl Into<DefiniteLength>) [18.75rem] .bordered(bool) .searchable(bool) .filterable(bool)   // filterable(false): you answer the query yourself
    .header(|state: &CommandState, window, cx| el) .footer(|state, window, cx| el) .empty(|state, window, cx| el)
    .on_query(Fn(&str, &mut Window, &mut App)) .on_select(Fn(IndexPath, ..)) .on_confirm(Fn(IndexPath, ..)) .on_cancel(Fn(&mut Window, &mut App))
CommandItem::new() .label(..) .icon(..) .keywords(iter) .action(Box<dyn Action>) .checked(bool) .disabled(bool) .child(|window, cx| el)
CommandGroup::new() .label(..) .item(..) .items(iter)

Command::new(&self.command).placeholder("Type a command or search...").bordered(false).max_h(px(320.))
    .group(CommandGroup::new().label("Tools")
        .item(CommandItem::new().label("Merge PDFs").icon(IconName::File).keywords(["combine", "join"]).action(Box::new(OpenTool { id: 1 })))
        .item(CommandItem::new().label("Split").disabled(true))
        .item(CommandItem::new().label("Custom row").child(|_window, cx| h_flex().w_full().justify_between().child("Compress")
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child("PDF")))))
    .separator()
    .item(CommandItem::new().label("Quit").checked(true))
    .footer(|state, _window, cx| h_flex().px_3().py_2().gap_2().border_t_1().border_color(cx.theme().border)
        .child(Kbd::new(Keystroke::parse("enter").unwrap())).child(format!("{} results", state.matched_count())))
    .on_confirm(|ix: IndexPath, _window, _cx| { let _ = (ix.section, ix.row); })

// as a modal palette
window.open_dialog(cx, move |dialog, _window, _cx| dialog.close_button(false).p_0().w(px(560.))
    .child(Command::new(&command).bordered(false).item(CommandItem::new().label("Merge"))));
command.update(cx, |state, cx| state.focus(window, cx));
```

What it provides: search field + filtered, virtualised list; groups with headings (sections); separators; per-item icon, label, keywords, trailing key-binding hint resolved from the item's `Action` (or a check mark); fully custom rows via `CommandItem::child` (label/keywords still drive filtering; a custom row draws its own hints); custom header, footer and empty slots; `up`/`down`/`enter`/`escape` in context `"Command"`; confirm dispatches the item's action then calls `on_confirm`. `IndexPath` uses input-model coordinates (ungrouped items are section 0). Limits: substring match only (no fuzzy ranking or highlight of matches), row chrome is fixed, no built-in recents or nested pages — reasons to write your own.

### Sidebar — no state `[C]`

```rust
// sidebar::{Sidebar, SidebarHeader, SidebarFooter, SidebarGroup, SidebarMenu, SidebarMenuItem, SidebarToggleButton, SidebarCollapsible}   width 255px, collapsed 48px
Sidebar::new(id) .side(Side) .collapsed(bool) .collapsible(SidebarCollapsible::{Icon, Offcanvas, None} | bool) .header(el) .footer(el) .child(E) .children(iter)   // all children one type E: SidebarItem
SidebarMenuItem::new(label) .icon(..) .active(bool) .on_click(Fn(&ClickEvent, ..)) .children(iter) .default_open(bool) .suffix(|window, cx| el) .disable(bool) .context_menu(..)

Sidebar::new("sidebar").collapsed(self.collapsed).header(SidebarHeader::new().child("Shorui"))
    .child(SidebarGroup::new("Tools").child(SidebarMenu::new()
        .child(SidebarMenuItem::new("Merge").icon(IconName::File).active(self.tab == 0)
            .on_click(cx.listener(|this, _event, _window, cx| { this.tab = 0; cx.notify(); })))
        .child(SidebarMenuItem::new("Split").icon(IconName::Copy))))
SidebarToggleButton::new().collapsed(self.collapsed).on_click(cx.listener(|this, _event, _window, cx| { this.collapsed = !this.collapsed; cx.notify(); }))
```

### Resizable — no state required `[C]`

```rust
// gpui_kit::component::{h_resizable, v_resizable, resizable_panel, ResizablePanelGroup, ResizablePanel, ResizableState, ResizablePanelEvent}
h_resizable(id) / v_resizable(id) .child(impl Into<ResizablePanel>) .with_state(&Entity<ResizableState>) .on_resize(Fn(&Entity<ResizableState>, &mut Window, &mut App))
resizable_panel() .size(impl Into<Pixels>) .size_range(Range<Pixels>) .visible(bool) .child(..)       // ResizableState::sizes() -> &Vec<Pixels>

h_resizable("split")
    .child(resizable_panel().size(px(260.)).size_range(px(160.)..px(480.)).child(List::new(&self.tools)))
    .child(resizable_panel().child(palette))
```

## 11. gpui-base (`gpui_kit::base`) worth knowing

- `Root`, `RootPlugin` (`Root::register_plugin::<V>(cx, fn(&mut Window, &mut Context<V>) -> V)`: your own per-window overlay layer rendered above content, registered before windows are created).
- `is_mobile()` (`gpui_kit::is_mobile`): compile-time iOS/Android check.
- Unstyled primitives the styled components wrap: `base::{Button, Input, Checkbox, Switch, Dialog, Popover, Select, Slider, Tabs, Tooltip, Toast, Tree, Table, ..}` with `.styles(|s| ..)` state styling; use them for fully custom looks.
- Motion (call during render; state keyed by id) `[C]`: `transition(id, target: T, Transition::new(duration).easing(..), window, cx) -> T`, `spring(id, target, Spring::new(duration), window, cx)`, `animate_keyframes(..)`, `Presence::new(id, present).transition(..).sample(window, cx)` for enter/exit. `base::animation::{cubic_bezier, ease_out_cubic, ease_in_cubic, ease_in_out_cubic, EffectTransition}`. Theme durations/easings: `cx.theme().motion_tokens()`.
- gpui's own: `el.with_animation(id, Animation::new(Duration::from_millis(150)).with_easing(ease_out_quint()), |el, delta| el.opacity(delta))` (`AnimationExt`); `.repeat()` for loops.
- `FocusTrapElement`, `ElementExt::on_prepaint(|bounds, window, cx| ..)` (read layout bounds), `InteractiveElementExt::{on_double_click, lock_scroll_axis}`, `Positioner`, `Placement`, `Side`, `measure`.
- `base::actions::{Confirm { secondary }, Cancel, SelectUp, SelectDown, SelectLeft, SelectRight, SelectFirst, SelectLast, SelectPageUp, SelectPageDown}` (namespace `ui`): the actions List/Command/Dialog bind; reuse them for your own keyboard-driven widgets.
- Test support (feature `test-support`): `#[gpui_kit::test]`, `gpui_kit::test::{TestAppContextExt, TestWindowExt}` (`window.find("id")`, `window.click("id", cx)`, `window.within(..)`, `cx.wait_for(..)`), `.test_support()` on elements (`TestSupportExt`, inert otherwise).

## 12. Windows gotchas and startup cost

- `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` at the top of `main.rs`, otherwise a console window opens with the app.
- Window/taskbar icon: the Windows backend loads **icon resource ordinal 1** from the exe (`LoadImageW(module, 1, IMAGE_ICON, ..)`). Embed an `.ico` with id `1` through a resource script (`winresource`/`embed-resource` in `build.rs`). `WindowOptions.icon` is X11 only.
- `app_id` is ignored on Windows. `cx.set_app_identity(identifier, name)` exists for system notifications (AppUserModelID).
- Custom title bar: `appears_transparent: true` removes the caption but keeps the resizable frame, shadow and snap; resize hit-testing at the edges is done by gpui. Window controls must be elements tagged `window_control_area(..)` (TitleBar does this). `titlebar: None` also hides the native bar on Windows.
- DPI: everything is in logical pixels (`px`); per-monitor scale changes are handled (`WM_DPICHANGED`). `window.scale_factor()` for raster work: render bitmaps at `logical size * scale_factor` to stay sharp.
- `WindowKind::Dialog` disables the parent window while open; `WindowKind::AnchoredPopup` returns an error on Windows (components fall back to in-window popovers).
- Debug builds bind `ctrl-shift-i` (`cmd-alt-i` on macOS) globally to the element inspector; release builds do not unless feature `inspector` is on.
- Rem scaling: `theme.font_size` is the rem size; setting 14 shrinks every rem-based control (`h_8` = 28 px). Size with `px(..)` where exact pixels matter.
- A wrong font family name silently renders in Segoe UI (section 4). The theme probes only its built-in defaults (`.SystemUIFont`; Consolas -> Cascadia Mono -> Courier New).

Startup cost of `init` (in order of weight, from source):
- `theme::init` -> `Theme::change`: enumerates **all installed font families once** (`TextSystem::all_font_names`, cached per process; the source comments quote about 100 ms on macOS) to resolve the default UI and mono fonts, and parses `default-theme.json` (14 KB, twice) and `default-colors.json` (58 KB, lazily on first Tailwind colour lookup). Runs again cheaply on every later `Theme::change`.
- The remaining `init` calls only register key bindings, globals and observers (input, list, command, menu, dock, table, popover, sheet, notification, tooltip, date picker, carousel, questionnaire). No tree-sitter unless a `tree-sitter*` feature is enabled; `locales` i18n strings are compiled in.
- `open_window` draws one frame synchronously before returning. Fonts and glyphs are loaded lazily on first use; `add_fonts` only registers bytes.
- Binary/asset size: `Assets` = 104 SVGs (about 44 KiB); `AllAssets` adds about 1 MiB; `icon_assets!` adds only the listed icons. A debug build of the check crate is about 55 MB (unoptimised, no debuginfo).

## 13. Differences from common gpui / gpui-component knowledge

1. `open_window` is `gpui_kit::open_window(options, cx, |window, cx| -> Entity<V>)` returning `(AnyWindowHandle, Entity<V>)`; it creates the `Root` itself. No `cx.new(|cx| Root::new(view, window, cx))`, no `Root::render_*_layer` in your view.
2. `Theme::change` reloads the registered light/dark `ThemeConfig` and overwrites `theme.colors`; `Theme::update` is the synced write path; `Theme::global_mut` edits do not reach tokens or scrollbars until `Theme::sync_base`.
3. `theme.font_size` is the rem size; most component sizes scale with it.
4. `window.focus(&handle, cx)` and `handle.focus(window, cx)` take `cx`.
5. `AppContext::new` returns `Entity<T>` directly in async contexts too; `AsyncApp::update` returns `R` (no `Result`); `WeakEntity::update` still returns `Result`.
6. `cx.spawn(async move |this, cx| ..)` uses async closures: `(WeakEntity<T>, &mut AsyncApp)`; `cx.spawn_in(window, async move |this, cx| ..)` gets `&mut AsyncWindowContext`.
7. `Anchor` (8 positions) replaces `Corner` in `anchored()`, `Popover::anchor`, notification placement.
8. `drag_over` closure is `Fn(StyleRefinement, &T, &mut Window, &mut App) -> StyleRefinement`.
9. `Sizable` has no `.medium()`; Button Large is the same height as Medium.
10. Two `IconName` enums: `component::IconName` (104, embedded by `Assets`) and `assets::IconName` (full Lucide, needs `AllAssets` or `icon_assets!`). `Icon::new` accepts both.
11. `FluentBuilder` (`.when`) is only in `gpui_kit::prelude`, not in the root glob.
12. `InputState` lives in gpui-base (`InputBaseState<InputMode>`); multi-line is a separate `TextareaState` / `Textarea`, code editor `EditorState` / `Editor`. `InputEvent::PressEnter { secondary, shift }`.
13. A `command` palette component exists (groups, custom rows, footer, action key hints).
14. `RenderImage` wants BGRA bytes inside `image::Frame`, and `image` is not re-exported.
15. `KeyBinding::new` panics on invalid strings; theme JSON never errors on unknown keys or bad colours.
16. Nothing follows the OS appearance unless you call `Theme::sync_system_appearance` and observe `observe_window_appearance`.
