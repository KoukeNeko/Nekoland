//! `nekoland-terminal`: Nekoland's terminal, made for running coding agents side by side
//! with shells. Each tab is grouped in the sidebar by what it is doing: waiting for the
//! user, working, done, failed, or an idle shell. Agents tell it through OSC 7501 or are
//! known by their program; shells through OSC 133 marks, the bell and notifications. The
//! count in the title bar, or Ctrl+Shift+U, goes to the next tab that waits.

use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

#[path = "terminal/agent.rs"]
mod agent;
mod ipc;
#[path = "terminal/api.rs"]
mod api;
#[path = "terminal/carry.rs"]
mod carry;
#[path = "terminal/badges.rs"]
mod badges;
#[path = "terminal/github.rs"]
mod github;
#[path = "terminal/ports.rs"]
mod ports;
#[path = "terminal/config.rs"]
mod config;
#[path = "terminal/context_card.rs"]
mod context_card;
#[path = "terminal/lang.rs"]
mod lang;
#[path = "terminal/layouts.rs"]
mod layouts;
#[path = "terminal/graphics_stream.rs"]
mod graphics_stream;
#[path = "terminal/hint_mode.rs"]
mod hint_mode;
#[path = "terminal/images.rs"]
mod images;
#[path = "terminal/hints.rs"]
mod hints;
#[path = "terminal/jobs.rs"]
mod jobs;
#[path = "terminal/osc.rs"]
mod osc;
#[path = "terminal/palette.rs"]
mod palette;
#[path = "terminal/page.rs"]
mod page;
#[path = "terminal/pane.rs"]
mod pane;
#[path = "terminal/preview.rs"]
mod preview;
#[allow(dead_code)]
#[path = "tasks/proc.rs"]
mod proc;
#[path = "terminal/profile_settings.rs"]
mod profile_settings;
#[path = "terminal/profiles.rs"]
mod profiles;
#[path = "terminal/push.rs"]
mod push;
#[path = "terminal/push_settings.rs"]
mod push_settings;
#[path = "terminal/restore.rs"]
mod restore;
#[path = "terminal/saved_settings.rs"]
mod saved_settings;
#[path = "terminal/remove_dialog.rs"]
mod remove_dialog;
#[path = "terminal/worktree.rs"]
mod worktree;
#[path = "terminal/scheme_import.rs"]
mod scheme_import;
#[path = "terminal/screen.rs"]
mod screen;
#[path = "terminal/session.rs"]
mod session;
#[path = "terminal/settings.rs"]
mod settings;
#[path = "terminal/shell_integration.rs"]
mod shell_integration;
#[path = "terminal/sidebar.rs"]
mod sidebar;
#[path = "terminal/strip_layout.rs"]
mod strip_layout;
#[path = "terminal/subagent_drawer.rs"]
mod subagent_drawer;
#[path = "terminal/key_input.rs"]
mod key_input;
#[path = "terminal/kitty_keyboard.rs"]
mod kitty_keyboard;
#[path = "terminal/launcher.rs"]
mod launcher;
#[path = "terminal/kitty_graphics.rs"]
mod kitty_graphics;
#[path = "terminal/ligatures.rs"]
mod ligatures;
#[path = "terminal/markup.rs"]
mod markup;
#[path = "terminal/screen_rows.rs"]
mod screen_rows;
#[path = "terminal/sixel.rs"]
mod sixel;
#[path = "terminal/sticky_header.rs"]
mod sticky_header;
#[path = "terminal/subagents.rs"]
mod subagents;
#[path = "terminal/subagents_card.rs"]
mod subagents_card;
#[path = "terminal/theme.rs"]
mod theme;
#[path = "terminal/title.rs"]
mod title;
#[path = "terminal/titlebar.rs"]
mod titlebar;
#[path = "terminal/trigger_settings.rs"]
mod trigger_settings;
#[path = "terminal/triggers.rs"]
mod triggers;
#[path = "terminal/usage.rs"]
mod usage;
#[path = "terminal/window.rs"]
mod window;

const APP_ID: &str = "io.github.koukeneko.Nekoland.Terminal";
/// How often each pane's foreground program and folder are read.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// The first argument that makes the program `nekoland-terminal cli`, the automation API's client, not a window.
const CLI_COMMAND: &str = "cli";
/// The first argument that shows or hides the drop-down window, and the app action that does.
const DROP_DOWN_COMMAND: &str = "drop-down";
/// The app action that becomes the copy installed now, every pane carried over.
const UPDATE_AND_RESTART: &str = "update-restart";

thread_local! {
    /// Set when the terminal was started by `drop-down`: its first window is the drop-down one.
    static STARTED_FOR_DROP_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Each command's keys and its name, for the accelerators and the shortcuts page of the settings.
const SHORTCUTS: [(&str, &[&str], &str); 27] = [
    ("win.new-tab", &["<Control><Shift>t"], "新增分頁"),
    ("win.new-window", &["<Control><Shift>n"], "新視窗"),
    ("win.close-tab", &["<Control><Shift>w"], "關閉分頁"),
    ("win.rename-tab", &["F2"], "重新命名分頁"),
    ("win.split-right", &["<Alt><Shift>d"], "向右分割"),
    ("win.split-down", &["<Alt><Shift>s"], "向下分割"),
    ("win.copy", &["<Control><Shift>c"], "複製"),
    ("win.paste", &["<Control><Shift>v"], "貼上"),
    ("win.find", &["<Control><Shift>f"], "尋找"),
    ("win.toggle-sidebar", &["<Control><Shift>b"], "切換側邊欄"),
    ("win.palette", &["<Control><Shift>p"], "命令選擇區"),
    ("win.search-tabs", &["<Control><Shift>g"], "搜尋所有分頁"),
    ("win.hints", &["<Control><Shift>e"], "提示選取（網址、路徑、雜湊）"),
    ("win.save-layout", &[], "儲存視窗配置"),
    ("win.new-worktree", &[], "在新的 git 工作樹開啟分頁"),
    ("win.remove-worktree", &[], "移除這個 git 工作樹"),
    ("win.import-schemes", &[], "從 Windows Terminal 匯入色彩配置"),
    ("win.import-iterm-colors", &[], "匯入 iTerm2 色彩配置（.itermcolors）"),
    ("win.zoom-in", &["<Control>plus", "<Control>equal"], "放大字型"),
    ("win.zoom-out", &["<Control>minus"], "縮小字型"),
    ("win.zoom-reset", &["<Control>0"], "重設字型大小"),
    ("app.next-waiting", &["<Control><Shift>u"], "下一個需要回應的分頁"),
    ("app.preferences", &["<Control><Shift>o", "<Control>comma"], "設定"),
    // Super+` is Nekoland's, which runs `nekoland-terminal drop-down`: no key of the window's own.
    ("app.drop-down", &[], "下拉式視窗（Super+`）"),
    ("app.update-restart", &[], "更新並重新啟動"),
    ("app.about", &[], "關於終端機"),
    ("app.quit", &["<Control><Shift>q"], "結束"),
];

/// What GTK has no class for: the round marks, the tinted count and its shake, the ways the sidebar lists the tabs, and
/// the parts of the settings page. Everything else is libadwaita's own, so the window follows the desktop's style and
/// accent.
const CSS: &str = "
.term-dot { min-width: 7px; min-height: 7px; border-radius: 4px; background: currentColor; }
.term-waiting { min-height: 0; padding: 3px 12px; background: alpha(@warning_color, 0.15); color: @warning_color; }
.term-waiting:hover { background: alpha(@warning_color, 0.25); }
.term-waiting.nudge { animation: term-nudge 300ms ease-out; }
@keyframes term-nudge { 30% { transform: translateX(-6px); } 60% { transform: translateX(4px); } }
.term-piece { margin: 0 6px; }
.term-bell { box-shadow: inset 0 0 0 3px var(--accent-color); }

.navigation-sidebar > row:not(:selected):not(:hover) .term-name:not(.urgent) { opacity: 0.55; }
.term-heading { padding: 10px 6px 2px; }
.term-repo { font-weight: bold; opacity: 1; }
.term-close { opacity: 0; min-width: 22px; min-height: 22px; padding: 0; }
row:hover .term-close { opacity: 1; }
row:hover .term-state { opacity: 0; }
.term-badge { padding: 0 4px; }
.term-colour-bar { min-width: 3px; border-radius: 2px; }
.term-list > row { min-height: 40px; }
.term-repos > row { min-height: 26px; padding-top: 2px; padding-bottom: 2px; }
.term-cards > row, .term-cards > row:selected, .term-cards > row:hover { background: none; padding: 3px 2px; }
.term-card { padding: 12px; border-radius: 8px; border: 1px solid alpha(currentColor, 0.12); background: alpha(currentColor, 0.03); }
.term-cards > row:hover .term-card { background: alpha(currentColor, 0.07); }
.term-cards > row:selected .term-card { border-color: var(--accent-bg-color); background: alpha(currentColor, 0.09); }
.term-pill { padding: 1px 8px; border-radius: 9px; background: alpha(currentColor, 0.16); }
.term-peek { padding: 6px 8px; border-radius: 6px; background: alpha(currentColor, 0.07); }
.term-icons > row, .term-icons > row:selected, .term-icons > row:hover { background: none; padding: 4px 0; margin: 0; }
.term-cell { min-width: 36px; min-height: 36px; border-radius: 10px; border: 1.5px solid transparent; }
.term-icons > row:hover .term-cell { background: alpha(currentColor, 0.07); }
.term-icons > row:selected .term-cell { background: alpha(currentColor, 0.12); }
.term-cell.ringed.warning { border-color: @warning_color; animation: term-pulse 1.4s ease-in-out infinite; }
.term-cell.ringed.accent { border-color: var(--accent-color); }
.term-cell.ringed.success { border-color: @success_color; }
.term-cell.ringed.error { border-color: @error_color; }
@keyframes term-pulse { 50% { border-color: alpha(@warning_color, 0.3); } }
.term-cell-spinner { min-width: 10px; min-height: 10px; }

.term-settings-nav { background: @sidebar_bg_color; }
.term-settings-heading { margin: 24px 0 12px; }
.term-picture { padding: 6px; border-radius: 10px; border: 1px solid alpha(currentColor, 0.12); }
.term-picture:checked { background: none; border: 2px solid var(--accent-bg-color); padding: 5px; }
.term-miniature { border-radius: 6px; background: @window_bg_color; }
.term-scheme { padding: 10px 12px; border-radius: 8px; min-height: 44px; }
.term-swatch { min-width: 16px; min-height: 16px; border-radius: 4px; }
.term-schemes > flowboxchild { padding: 2px; border-radius: 10px; }
.term-schemes > flowboxchild:selected { background: var(--accent-bg-color); }
.term-code { font-family: monospace; padding: 1px 8px; border-radius: 6px; background: alpha(currentColor, 0.08); }
.term-tag { padding: 2px 9px; border-radius: 9px; background: alpha(currentColor, 0.1); }
.term-tag.mark { background: alpha(@warning_color, 0.22); }
.term-tag.warn { background: alpha(@error_color, 0.22); }
.term-sample { font-family: monospace; padding: 8px 10px; border-radius: 6px; background: alpha(currentColor, 0.06); }
.term-swatch-button { padding: 4px; border-radius: 8px; }
.term-swatch-button:checked { box-shadow: inset 0 0 0 2px var(--accent-bg-color); background: none; }
.term-swatch-button .term-swatch { min-width: 20px; min-height: 20px; }
";

/// The bar down a row of the sidebar in the colour the user gave its tab.
fn tab_colour_css() -> String {
    page::TAB_COLOURS
        .iter()
        .map(|(id, _, hex)| format!(".term-colour-{id} {{ background: {hex}; }}\n"))
        .collect()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        "{CSS}{}{}{}{}{}{}{}{}",
        tab_colour_css(),
        trigger_settings::css(),
        context_card::css(),
        palette::css(),
        subagents_card::css(),
        subagent_drawer::css(),
        key_input::css(),
        badges::css()
    ));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        settings::show_scheme_css(&display);
    }
    theme::show_opacity(&config::current());
}

fn add_app_actions(app: &adw::Application) {
    let next_waiting = gio::SimpleAction::new("next-waiting", None);
    next_waiting.connect_activate(|_, _| window::show_next_waiting());
    app.add_action(&next_waiting);

    let show_tab = gio::SimpleAction::new("show-tab", Some(glib::VariantTy::UINT64));
    show_tab.connect_activate(|_, parameter| {
        if let Some(id) = parameter.and_then(|parameter| parameter.get::<u64>()) {
            window::show_page(id);
        }
    });
    app.add_action(&show_tab);

    let preferences = gio::SimpleAction::new("preferences", None);
    preferences.connect_activate(|_, _| {
        if let Some(window) = window::front_window() {
            window.open_settings(settings::Section::Appearance, None);
        }
    });
    app.add_action(&preferences);

    // The settings at a page named in the action, for a link to it: "triggers", or "profiles:bash" for what is on it.
    let section = gio::SimpleAction::new("settings", Some(glib::VariantTy::STRING));
    section.connect_activate(|_, parameter| {
        let place = parameter.and_then(|parameter| parameter.str()).unwrap_or_default();
        let (id, detail) = place.split_once(':').map_or((place, None), |(id, detail)| (id, Some(detail)));
        let section = settings::Section::from_id(id).unwrap_or(settings::Section::Appearance);
        if let Some(window) = window::front_window() {
            window.open_settings(section, detail);
        }
    });
    app.add_action(&section);

    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(|_, _| {
        if let Some(window) = window::front_window() {
            window.open_settings(settings::Section::About, None);
        }
    });
    app.add_action(&about);

    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(|_, _| {
        if let Some(window) = window::front_window() {
            window.window.close();
        }
    });
    app.add_action(&quit);

    let drop_down = gio::SimpleAction::new(DROP_DOWN_COMMAND, None);
    drop_down.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| window::toggle_drop_down(&app)
    ));
    app.add_action(&drop_down);

    let restart = gio::SimpleAction::new(UPDATE_AND_RESTART, None);
    restart.connect_activate(|_, _| window::update_and_restart());
    app.add_action(&restart);

    for (action, accels, _) in SHORTCUTS {
        app.set_accels_for_action(action, accels);
    }
}

/// Reads every pane's foreground program and folder, and redraws what changed.
fn start_polling() {
    glib::timeout_add_local(POLL_INTERVAL, || {
        let pages = window::all_pages();
        let changed = pages.iter().fold(false, |changed, page| page.poll() | changed) | badges::refresh(&pages);
        window::publish_agent_states(&pages);
        if changed {
            window::refresh_soon();
        }
        glib::ControlFlow::Continue
    });
}

fn main() -> glib::ExitCode {
    // Claude Code runs this program as its status line and hooks in a pane; that run is no window.
    if usage::run_as_command() {
        return glib::ExitCode::SUCCESS;
    }
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(String::as_str);
    if subcommand == Some(CLI_COMMAND) {
        return if api::run_cli(&args[2..]) { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE };
    }
    if carry::is_carrying() {
        carry::wait_until_released(APP_ID);
    }
    let app = adw::Application::builder().application_id(APP_ID).build();
    if subcommand == Some(DROP_DOWN_COMMAND) {
        return toggle_drop_down(&app, &args[..1]);
    }
    run_app(&app, &args)
}

/// `nekoland-terminal drop-down`: the running terminal shows or hides its drop-down window, or one starts with it.
fn toggle_drop_down(app: &adw::Application, program: &[String]) -> glib::ExitCode {
    connect_app(app);
    if let Err(error) = app.register(gio::Cancellable::NONE) {
        eprintln!("nekoland-terminal: could not reach the running terminal: {error}");
        return glib::ExitCode::FAILURE;
    }
    if app.is_remote() {
        app.activate_action(DROP_DOWN_COMMAND, None);
        // The call is only queued: it has to leave before the program ends.
        if let Some(Err(error)) = app.dbus_connection().map(|bus| bus.flush_sync(gio::Cancellable::NONE)) {
            eprintln!("nekoland-terminal: could not reach the running terminal: {error}");
            return glib::ExitCode::FAILURE;
        }
        return glib::ExitCode::SUCCESS;
    }
    STARTED_FOR_DROP_DOWN.set(true);
    app.run_with_args(program)
}

fn run_app(app: &adw::Application, args: &[String]) -> glib::ExitCode {
    connect_app(app);
    app.run_with_args(args)
}

fn connect_app(app: &adw::Application) {
    app.connect_startup(|app| {
        shell_integration::install();
        usage::clean_stale();
        lang::apply(config::current().language);
        load_css();
        add_app_actions(app);
        start_polling();
        api::start(window::answer_api);
    });
    app.connect_shutdown(|_| api::stop());
    app.connect_activate(|app| {
        if STARTED_FOR_DROP_DOWN.replace(false) {
            window::toggle_drop_down(app);
            return;
        }
        if let Some(carried) = carry::take() {
            window::open_carried(app, &carried);
            return;
        }
        match window::shown_window() {
            Some(window) => window.window.present(),
            None => window::open_first(app),
        }
    });
}
