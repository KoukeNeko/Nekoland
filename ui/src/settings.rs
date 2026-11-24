//! `nekoland-settings`: Nekoland's settings, in GTK and libadwaita as GNOME's own settings
//! are, so that it looks as the system's GTK style says. What the compositor keeps it asks
//! for and changes through `nekoland msg`, so the compositor alone reads and writes its file
//! and puts each change in effect; the rest is the system's, through its own tools.

mod alert;
mod apt;
mod program;
mod backup;
mod bluetooth;
mod display;
mod edid;
mod focus;
mod imagegen;
mod ipc;
mod keys;
mod network;
mod printers;
mod region;
mod secrets;
mod selfupdate;
mod themes;
mod users;
mod web;
mod wifi;
mod windows;
#[path = "settings/lock_screen.rs"]
mod lock_screen;
#[path = "settings/hover.rs"]
mod hover;
#[path = "settings/fonts.rs"]
mod fonts;
#[path = "settings/appearance.rs"]
mod appearance;
#[path = "settings/keyboard.rs"]
mod keyboard;
#[path = "settings/sound.rs"]
mod sound;
#[path = "settings/sharing.rs"]
mod sharing;
#[path = "settings/updates.rs"]
mod updates;
#[path = "settings/design.rs"]
mod design;
#[path = "settings/vpn.rs"]
mod vpn;
#[path = "settings/sunshine.rs"]
mod sunshine;
#[path = "settings/storage.rs"]
mod storage;
#[path = "settings/devices.rs"]
mod devices;
// Market's: what is installed and how it is removed.
#[allow(dead_code)]
#[path = "store/sources.rs"]
mod sources;
mod removal;
// Where the Dock finds an app's icon.
#[allow(dead_code)]
#[path = "launcher/apps.rs"]
mod apps;

use lock_screen::*;
use hover::*;
use fonts::*;
use appearance::*;
use keyboard::*;
use sound::*;
use sharing::*;
use updates::*;
use design::*;

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Settings";
const APPS_VIEWS: [&str; 4] = ["overview", "launchpad", "shelf", "sidebar"];
const DOCK_POSITIONS: [&str; 3] = ["left", "bottom", "right"];
const MINIMIZE_EFFECTS: [&str; 2] = ["genie", "scale"];
const HOVER_STYLES: [&str; 2] = ["full", "tooltip"];
/// The parts of the hover panel that can be left out, as `hover_hide` names them.
const HOVER_SECTIONS: [(&str, &str); 8] = [
    ("檔案圖示", "icon"),
    ("類型資訊", "type"),
    ("下載來源", "source"),
    ("日期", "dates"),
    ("檔案資訊", "file"),
    ("位置", "location"),
    ("備註", "notes"),
    ("標籤", "tags"),
];
/// What the hover panel leaves out unless told otherwise, as the compositor has it.
const HOVER_HIDDEN: [&str; 1] = ["file/屬性修改日期"];
const WALLPAPER_MODES: [&str; 3] = ["fill", "fit", "center"];
/// Where pictures for the desktop are looked for, and how many are offered.
const WALLPAPER_DIRS: [&str; 3] = [
    "~/.local/share/backgrounds",
    "~/Pictures/Wallpapers",
    "/usr/share/backgrounds",
];
const WALLPAPERS: usize = 8;
/// How often a slider being dragged passes its value on.
const SLIDER_EVERY: std::time::Duration = std::time::Duration::from_millis(100);
const SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// What PipeWire calls the sound a program is playing.
const PLAYBACK_STREAM: &str = "Stream/Output/Audio";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

/// What a program printed, or None when it could not be run or failed.
fn output(program: &str, args: &[&str]) -> Option<String> {
    if let ("gsettings", ["set" | "reset", schema, key, ..]) = (program, args) {
        WROTE.with_borrow_mut(|wrote| wrote.insert(format!("{schema} {key}")));
    }
    // Straight over the socket rather than through a `nekoland msg` process per message.
    if let ("nekoland", ["msg", message]) = (program, args) {
        return ipc::msg(message);
    }
    let result = Command::new(program)
        .args(args)
        .output()
        .inspect_err(|err| eprintln!("{program}: {err}"))
        .ok()?;
    // GNOME's desktop settings are what X11 programs are given, by the compositor.
    if let ("gsettings", ["set" | "reset", schema, ..]) = (program, args) {
        if schema.starts_with("org.gnome.desktop.") && result.status.success() {
            output("nekoland", &["msg", "xsettings"]);
        }
    }
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).into_owned())
}

fn run(program: &str, args: &[&str]) {
    if output(program, args).is_none() {
        eprintln!("{program} {} failed", args.join(" "));
    }
}

/// `text` as a TOML string, for a setting: Rust's own quoting writes some characters as TOML
/// does not read them, such as accents of their own.
fn toml_string(text: &str) -> String {
    toml::Value::from(text).to_string()
}

/// Changes one of the compositor's settings; it answers only when the change is refused.
fn set(line: &str) {
    change(&format!("set {line}"));
}

/// Puts one of the compositor's settings back at its default.
fn unset(key: &str) {
    change(&format!("unset {key}"));
}

fn change(command: &str) {
    let key = command.split_once(' ').map_or("", |(_, rest)| rest);
    let key = key.split_once(" = ").map_or(key, |(key, _)| key);
    WROTE.with_borrow_mut(|wrote| wrote.insert(key.to_string()));
    if let Some(reply) = output("nekoland", &["msg", command]) {
        if !reply.trim().is_empty() {
            eprintln!("{}", reply.trim());
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

/// The pictures offered for the desktop: the one in use first, then those found.
fn wallpapers(current: Option<&Path>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = current.into_iter().map(Path::to_path_buf).collect();
    for dir in WALLPAPER_DIRS {
        let Ok(entries) = std::fs::read_dir(expand(dir)) else {
            continue;
        };
        let mut pictures: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(extension.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp")
                    })
            })
            .collect();
        pictures.sort();
        for picture in pictures {
            if !found.contains(&picture) {
                found.push(picture);
            }
        }
    }
    found.truncate(WALLPAPERS);
    found
}

/// The value of `key` in a file of `KEY=VALUE` or `KEY: VALUE` lines.
fn field(text: &str, key: &str, separator: char) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(separator)?;
        (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
    })
}

fn facts() -> Vec<(&'static str, String)> {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    let mut facts = Vec::new();
    let mut add = |title: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            facts.push((title, value));
        }
    };
    add("電腦名稱", Some(read("/etc/hostname").trim().to_string()));
    add("系統", field(&read("/etc/os-release"), "PRETTY_NAME", '='));
    add(
        "核心",
        Some(read("/proc/sys/kernel/osrelease").trim().to_string()),
    );
    add("處理器", field(&read("/proc/cpuinfo"), "model name", ':'));
    let memory = field(&read("/proc/meminfo"), "MemTotal", ':')
        .and_then(|value| value.trim_end_matches("kB").trim().parse::<f64>().ok());
    add(
        "記憶體",
        memory.map(|kib| format!("{:.1} GB", kib / 1024.0 / 1024.0)),
    );
    // The first line is the mode in use, as WIDTHxHEIGHT@HZ.
    let mode = output("nekoland", &["msg", "modes"]).and_then(|modes| {
        let line = modes.lines().next()?.to_string();
        let (size, rate) = line.split_once('@')?;
        let (width, height) = size.split_once('x')?;
        Some(format!("{width} × {height}，{rate} Hz"))
    });
    add("顯示器", mode);
    facts
}

/// A default device's volume, 0 to 100, and whether it is muted; None without one.
fn volume(device: &str) -> Option<(f32, bool)> {
    let text = output("wpctl", &["get-volume", device])?;
    let level: f32 = text.split_whitespace().nth(1)?.parse().ok()?;
    Some((level * 100.0, text.contains("[MUTED]")))
}

/// The settings in effect, with what the file leaves out at the compositor's defaults.
struct Config(toml::Table);

impl Config {
    fn load() -> Self {
        Self(
            output("nekoland", &["msg", "config"])
                .and_then(|text| {
                    toml::from_str(&text)
                        .inspect_err(|err| eprintln!("nekoland msg config: {err}"))
                        .ok()
                })
                .unwrap_or_default(),
        )
    }

    fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(toml::Value::as_bool).unwrap_or_default()
    }

    fn number(&self, key: &str, default: f64) -> f64 {
        match self.0.get(key) {
            Some(toml::Value::Integer(number)) => *number as f64,
            value => value.and_then(toml::Value::as_float).unwrap_or(default),
        }
    }

    fn word(&self, key: &str) -> &str {
        self.0.get(key).and_then(toml::Value::as_str).unwrap_or_default()
    }

    /// Which of `names` the setting is, the first when it is none of them.
    fn choice(&self, names: &[&str], key: &str) -> u32 {
        names.iter().position(|name| *name == self.word(key)).unwrap_or(0) as u32
    }

    fn autostart(&self) -> Vec<String> {
        self.0
            .get("autostart")
            .and_then(toml::Value::as_array)
            .map(|commands| {
                commands
                    .iter()
                    .filter_map(|command| command.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
}

const A11Y_APPLICATIONS: &str = "org.gnome.desktop.a11y.applications";

/// unattended-upgrades, which installs security updates by itself, and apt's switch for it.
const UNATTENDED: &str = "/usr/bin/unattended-upgrade";
const AUTO_UPGRADES: &str = "/etc/apt/apt.conf.d/20auto-upgrades";
/// A drop-in that keeps automatic updates off while a pause lasts.
const PAUSE_FILE: &str = "/etc/apt/apt.conf.d/99nekoland-pause";
const APT_HISTORY: &str = "/var/log/apt/history.log";
/// How many of the latest apt runs 更新記錄 lists.
const HISTORY_ENTRIES: usize = 20;
/// The timer that turns automatic updates back on when a pause ends, kept as files so that
/// it survives a restart.
const RESUME_TIMER: &str = "/etc/systemd/system/nekoland-resume-updates.timer";
const RESUME_SERVICE: &str = "/etc/systemd/system/nekoland-resume-updates.service";
/// The pauses 暫停更新 offers, in weeks; 0 is none.
const PAUSE_WEEKS: [u32; 4] = [0, 1, 2, 5];

/// The page a restarted Settings opens on.
const PAGE_ENV: &str = "NEKOLAND_SETTINGS_PAGE";

type OpenPage = Box<dyn Fn(&str) -> bool>;

thread_local! {
    /// Set when Settings is to start again once it has closed, on the page named.
    static RESTART: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    /// Nekoland's `glass_opacity`, which the look gives GTK's menus and sidebars too.
    static GLASS_OPACITY: Cell<f64> = const { Cell::new(0.8) };
    /// Nekoland's `font`, which the look gives GTK's text too.
    static UI_FONT: RefCell<String> = RefCell::new("Ubuntu Sans".into());
    /// The settings Settings changed itself, which its pages already show.
    static WROTE: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Every page one level down, with the row that opens it, for search to look in.
    static SUBPAGES: RefCell<Vec<(glib::WeakRef<adw::ActionRow>, adw::NavigationPage)>> = RefCell::default();
    /// Shows the page named in the open window; false for a name it has no page for.
    static OPEN_PAGE: RefCell<Option<OpenPage>> = RefCell::default();
    /// The `pw-dump --monitor` the sound page follows, stopped as Settings ends.
    static SOUND_MONITOR: RefCell<Option<gio::Subprocess>> = const { RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    // The backup timer's service runs this, with no window.
    if std::env::args().nth(1).as_deref() == Some(backup::BACKUP_FLAG) {
        return backup::back_up_now();
    }
    // Run as root, by Settings and by systemd, to let what is shared through the firewall.
    if std::env::args().nth(1).as_deref() == Some(sharing::FIREWALL_FLAG) {
        return sharing::sync_firewall(std::env::args().nth(2).as_deref());
    }
    // A launcher of Windows' programs runs this, with no window of its own.
    if std::env::args().nth(1).as_deref() == Some(windows::OPEN_FLAG) {
        let (entry, file) = (std::env::args().nth(2), std::env::args().nth(3));
        return windows::open(entry.as_deref(), file.as_deref());
    }
    // Files opens a Windows program with this, in Wine, with no window of its own.
    if std::env::args().nth(1).as_deref() == Some(windows::WINE_FLAG) {
        return windows::run(std::env::args().nth(2).as_deref());
    }
    // The compositor runs this when X11 programs are to draw at another density.
    if std::env::args().nth(1).as_deref() == Some(windows::DPI_FLAG) {
        return windows::match_all_dpi();
    }
    // Files asks this what Windows' programs add to a file's menu, with no window.
    if std::env::args().nth(1).as_deref() == Some(windows::MENU_FLAG) {
        return windows::menu(&std::env::args().skip(2).collect::<Vec<_>>());
    }
    // libadwaita asks the portal for dark or light once, as it starts, and Settings can open
    // at login before the portal can start, as for a VPN asking for its password: it would
    // stay light. GNOME's setting, which Settings itself writes, is read and followed instead.
    std::env::set_var("ADW_DISABLE_PORTAL", "1");
    // The environment comes along so that a page asked for while Settings is open is shown.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::SEND_ENVIRONMENT)
        .build();
    app.connect_activate(build);
    app.connect_command_line(|app, line| {
        if let Some(page) = line.getenv(PAGE_ENV) {
            std::env::set_var(PAGE_ENV, page);
        }
        app.activate();
        0
    });
    let code = app.run();
    for monitor in SOUND_MONITOR.take().into_iter().chain(wifi::NETWORK_MONITORS.take()) {
        monitor.force_exit();
    }
    if let Some(page) = RESTART.with_borrow(|page| *page) {
        use std::os::unix::process::CommandExt;
        // The new program takes the application's name on the bus as this one's goes.
        let err = std::env::current_exe()
            .map(|program| Command::new(program).env(PAGE_ENV, page).exec())
            .unwrap_or_else(|err| err);
        eprintln!("restarting: {err}");
    }
    code
}

/// Scrolls the page a setting is on to it, and focuses it.
fn bring_into_view(target: &gtk::Widget) {
    if let Some(viewport) = target.ancestor(gtk::Viewport::static_type()) {
        let scroll = viewport.parent().and_downcast::<gtk::ScrolledWindow>();
        let content = viewport.downcast_ref::<gtk::Viewport>().and_then(|v| v.child());
        if let (Some(scroll), Some(content)) = (scroll, content) {
            if let Some(bounds) = target.compute_bounds(&content) {
                scroll.vadjustment().set_value((f64::from(bounds.y()) - 24.0).max(0.0));
            }
        }
    }
    target.grab_focus();
}

/// The title of each setting on a page and on the pages its rows open, with the row that
/// shows it.
fn setting_rows(widget: &gtk::Widget, found: &mut Vec<(String, gtk::Widget)>) {
    if let Some(row) = widget.downcast_ref::<adw::PreferencesRow>() {
        if widget.is_visible() && !row.title().is_empty() {
            found.push((row.title().to_string(), widget.clone()));
        }
    }
    if let Some(subpage) = subpage_of(widget) {
        setting_rows(subpage.upcast_ref(), found);
    }
    let mut child = widget.first_child();
    while let Some(next) = child {
        setting_rows(&next, found);
        child = next.next_sibling();
    }
}

/// A row opening `page` one level down, under `title`; its subtitle, from `summary`, tells
/// how the page is set and is read again as the page is left.
fn subpage_row(title: &str, summary: impl Fn() -> String + 'static, page: impl IsA<gtk::Widget>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(summary())
        .activatable(true)
        .build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let subpage = adw::NavigationPage::builder().title(title).child(&view).build();
    subpage.connect_hidden(glib::clone!(
        #[weak]
        row,
        move |_| row.set_subtitle(&summary())
    ));
    row.connect_activated(glib::clone!(
        #[strong]
        subpage,
        move |row| {
            let navigation = row.ancestor(adw::NavigationView::static_type()).and_downcast::<adw::NavigationView>();
            // Already shown, it is not opened twice.
            if let Some(navigation) = navigation.filter(|_| subpage.parent().is_none()) {
                navigation.push(&subpage);
            }
        }
    ));
    SUBPAGES.with_borrow_mut(|subpages| {
        subpages.retain(|(row, _)| row.upgrade().is_some());
        subpages.push((row.downgrade(), subpage));
    });
    row
}

fn subpage_of(widget: &gtk::Widget) -> Option<adw::NavigationPage> {
    let row = widget.downcast_ref::<adw::ActionRow>()?;
    SUBPAGES.with_borrow(|subpages| {
        subpages
            .iter()
            .find(|(opener, _)| opener.upgrade().as_ref() == Some(row))
            .map(|(_, subpage)| subpage.clone())
    })
}

fn open_subpage(row: &adw::ActionRow) {
    row.emit_by_name::<()>("activated", &[]);
}

/// Opens the page one level down that `widget` is on, if it is on one.
fn open_subpage_holding(widget: &gtk::Widget) {
    let opener = SUBPAGES.with_borrow(|subpages| {
        subpages
            .iter()
            .find(|(_, subpage)| widget.is_ancestor(subpage))
            .and_then(|(row, _)| row.upgrade())
    });
    if let Some(row) = opener {
        open_subpage(&row);
    }
}

/// Pages that are a row of another, by their old names or their own, as programs open them.
const MOVED_PAGES: [(&str, &str); 9] = [
    ("設計語言", "關於"),
    ("桌面", "桌布"),
    ("視窗", "桌面與 Dock"),
    ("字型", "外觀"),
    ("預設應用程式", "一般"),
    ("啟動項目", "一般"),
    ("語言與地區", "一般"),
    ("日期與時間", "一般"),
    ("系統監控", "選單列"),
];

fn build(app: &adw::Application) {
    // Opened again, it shows the window it has.
    if let Some(window) = app.active_window() {
        if let Ok(asked) = std::env::var(PAGE_ENV) {
            std::env::remove_var(PAGE_ENV);
            OPEN_PAGE.with_borrow(|open_page| open_page.as_ref().map(|open_page| open_page(&asked)));
        }
        window.present();
        return;
    }
    let config = Config::load();
    remember_look(&config);
    // A look turned on before keeps up with this version of it.
    if look_read_at_start() {
        if let Err(err) = set_gtk_look(true) {
            eprintln!("GTK look: {err}");
        }
        set_gtk3_theme(true);
    }
    let autostart = Rc::new(RefCell::new(config.autostart()));
    // GTK reads its user stylesheet as an application starts: other applications take the
    // look's changes when opened again, and Settings shows them at once with this.
    let look = gtk::CssProvider::new();
    let styles = adw::StyleManager::default();
    look.load_from_string(&look_for(styles.is_dark()));
    styles.connect_dark_notify(glib::clone!(
        #[weak]
        look,
        move |styles| look.load_from_string(&look_for(styles.is_dark()))
    ));
    // On already, it is over what GTK read at start, so the glass's opacities show at once.
    if let Some(display) = gdk::Display::default().filter(|_| look_read_at_start()) {
        gtk::style_context_add_provider_for_display(&display, &look, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
    let make_page = {
        let (look, autostart) = (look.clone(), autostart.clone());
        Rc::new(move |name: &str, config: &Config| -> gtk::Widget {
            match name {
                "Wi-Fi" => wifi::wifi_page().upcast(),
                "藍牙" => bluetooth::bluetooth_page().upcast(),
                "網路" => network::network_page().upcast(),
                "VPN" => vpn::vpn_page().upcast(),
                "外觀" => appearance_page(config, &look).upcast(),
                "協助工具" => accessibility_page(config).upcast(),
                "隱私" => privacy_page(config).upcast(),
                "桌布" => wallpaper_page(config, &autostart).upcast(),
                "桌面與 Dock" => desktop_page(config).upcast(),
                "選單列" => menu_bar_page(config).upcast(),
                "鎖定畫面" => lock_page(config).upcast(),
                "懸停資訊" => hover_page(config).upcast(),
                "通知" => notifications_page(config).upcast(),
                "顯示" => display::display_page(config).upcast(),
                "電源" => power_page(config).upcast(),
                "裝置管理員" => devices::devices_page().upcast(),
                "聲音" => sound_page().upcast(),
                "鍵盤" => keyboard_page(config).upcast(),
                "滑鼠與觸控板" => mouse_page(config).upcast(),
                "使用者" => users::users_page().upcast(),
                "一般" => general_page(config, &autostart).upcast(),
                "印表機" => printers::printers_page().upcast(),
                "軟體更新" => updates_page().upcast(),
                "儲存空間" => storage::storage_page().upcast(),
                "備份" => backup::backup_page().upcast(),
                "Windows App" => windows::windows_page().upcast(),
                "共享" => sharing_page().upcast(),
                "關於" => about_page().upcast(),
                _ => unreachable!("no page named {name}"),
            }
        })
    };
    // The pages in groups, each under its name; 網路 has the proxy and the firewall too,
    // which are not NetworkManager's.
    let groups: Vec<Vec<(&str, &str)>> = vec![
        wifi::has_wifi()
            .then_some(("Wi-Fi", "network-wireless-symbolic"))
            .into_iter()
            .chain(bluetooth::has_bluetooth().then_some(("藍牙", "bluetooth-symbolic")))
            .chain([("網路", "network-wired-symbolic")])
            .chain(has_vpn().then_some(("VPN", "network-vpn-symbolic")))
            .chain([("共享", "preferences-desktop-remote-desktop-symbolic")])
            .collect(),
        vec![
            ("外觀", "preferences-desktop-appearance-symbolic"),
            ("桌布", "preferences-desktop-wallpaper-symbolic"),
            ("桌面與 Dock", "user-desktop-symbolic"),
            ("選單列", "preferences-desktop-multitasking-symbolic"),
            ("鎖定畫面", "system-lock-screen-symbolic"),
            ("懸停資訊", "document-properties-symbolic"),
        ],
        vec![
            ("顯示", "video-display-symbolic"),
            ("聲音", "audio-speakers-symbolic"),
            ("鍵盤", "input-keyboard-symbolic"),
            ("滑鼠與觸控板", "input-mouse-symbolic"),
            ("印表機", "printer-symbolic"),
            ("電源", "battery-level-100-symbolic"),
            ("裝置管理員", "computer-symbolic"),
        ],
        vec![
            ("一般", "preferences-system-symbolic"),
            ("通知", "preferences-system-notifications-symbolic"),
            ("軟體更新", "software-update-available-symbolic"),
            ("儲存空間", "drive-harddisk-symbolic"),
            ("備份", "document-save-symbolic"),
            ("Windows App", "application-x-executable-symbolic"),
        ],
        vec![
            ("使用者", "system-users-symbolic"),
            ("隱私", "preferences-system-privacy-symbolic"),
            ("協助工具", "preferences-desktop-accessibility-symbolic"),
        ],
        vec![("關於", "help-about-symbolic")],
    ];
    const GROUP_NAMES: [&str; 6] = ["連線", "個人化", "裝置", "系統", "帳號與安全", "關於"];
    let crumbs: Vec<String> = groups
        .iter()
        .zip(GROUP_NAMES)
        .flat_map(|(group, name)| group.iter().map(move |(page, _)| format!("{name} › {page}")))
        .collect();
    let starts: Vec<usize> = groups
        .iter()
        .scan(0, |at, group| {
            let start = *at;
            *at += group.len();
            Some(start)
        })
        .collect();
    let pages: Vec<(&str, &str)> = groups.into_iter().flatten().collect();

    // A page's name has its glyphs half a pixel under the middle of its line, where its
    // icon, placed on whole pixels, cannot follow but by a transform.
    // A look's tile has the same room on every side, as a row has.
    let sidebar_css = gtk::CssProvider::new();
    sidebar_css.load_from_string(
        ".nk-sidebar-icon { -gtk-icon-transform: translateY(0.5px); } button.nk-look { padding: 12px; }",
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &sidebar_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    // A page is built the first time it is shown or searched: built all at once, the pages
    // Settings is not opened for cost about as much memory as the window itself.
    let page_of = {
        let make_page = make_page.clone();
        Rc::new(move |stack: &gtk::Stack, name: &str| -> gtk::Widget {
            stack.child_by_name(name).unwrap_or_else(|| {
                let page = make_page(name, &Config::load());
                stack.add_named(&page, Some(name));
                page
            })
        })
    };
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    sidebar.set_header_func(move |row, _| {
        let index = row.index() as usize;
        let title = starts.iter().position(|start| *start == index).map(|group| {
            gtk::Label::builder()
                .label(GROUP_NAMES[group])
                .xalign(0.0)
                .margin_top(if group == 0 { 4 } else { 14 })
                .margin_bottom(4)
                .margin_start(10)
                .css_classes(["dim-label", "caption-heading"])
                .build()
        });
        row.set_header(title.as_ref());
    });
    for (name, icon) in &pages {
        let icon = gtk::Image::builder()
            .icon_name(*icon)
            .pixel_size(16)
            .css_classes(["nk-sidebar-icon"])
            .build();
        let label = gtk::Label::builder().label(*name).xalign(0.0).build();
        sidebar.append(&line(8, &[icon.upcast_ref(), label.upcast_ref()]));
    }

    let content = adw::NavigationPage::builder().title(pages[0].0).build();
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&adw::HeaderBar::new());
    content_view.set_content(Some(&stack));
    // Pages one level down are pushed over the page shown, with a way back to it.
    let shown_page = adw::NavigationPage::builder()
        .title(pages[0].0)
        .tag(SHOWN_PAGE)
        .child(&content_view)
        .build();
    let navigation = adw::NavigationView::new();
    navigation.add(&shown_page);
    content.set_child(Some(&navigation));

    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    let scrolled = |child: &gtk::ListBox| {
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(child)
            .build()
    };
    let results = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let side = gtk::Stack::new();
    side.add_named(&scrolled(&sidebar), Some("pages"));
    side.add_named(&scrolled(&results), Some("results"));
    side.add_named(
        &adw::StatusPage::builder()
            .icon_name("edit-find-symbolic")
            .title("沒有結果")
            .css_classes(["compact"])
            .build(),
        Some("empty"),
    );
    let search = gtk::SearchEntry::builder()
        .placeholder_text("搜尋設定")
        .margin_start(8)
        .margin_end(8)
        .margin_bottom(6)
        .build();
    sidebar_view.add_top_bar(&search);
    sidebar_view.set_content(Some(&side));
    let split = adw::NavigationSplitView::builder()
        .sidebar(
            &adw::NavigationPage::builder()
                .title("設定")
                .child(&sidebar_view)
                .build(),
        )
        .content(&content)
        .min_sidebar_width(280.0)
        .max_sidebar_width(340.0)
        .sidebar_width_fraction(0.3)
        .build();

    let names: Vec<&str> = pages.iter().map(|(name, ..)| *name).collect();
    let page_names = names.clone();
    let asked_names = names.clone();
    sidebar.connect_row_selected(glib::clone!(
        #[weak]
        stack,
        #[weak]
        content,
        #[weak]
        shown_page,
        #[weak]
        navigation,
        #[strong]
        page_of,
        move |_, row| {
            if let Some(name) = row.and_then(|row| names.get(row.index() as usize)) {
                navigation.pop_to_tag(SHOWN_PAGE);
                page_of(&stack, name);
                stack.set_visible_child_name(name);
                content.set_title(name);
                shown_page.set_title(name);
            }
        }
    ));
    let open_page: OpenPage = Box::new(glib::clone!(
        #[weak]
        sidebar,
        #[weak]
        stack,
        #[weak]
        navigation,
        #[weak]
        search,
        #[upgrade_or]
        false,
        move |asked: &str| {
            // A page that moved is opened where it is now.
            let moved = MOVED_PAGES.iter().find(|(old, _)| *old == asked);
            let page = moved.map_or(asked, |(_, now)| *now);
            let Some(at) = asked_names.iter().position(|name| *name == page) else {
                return false;
            };
            search.set_text("");
            navigation.pop_to_tag(SHOWN_PAGE);
            sidebar.select_row(sidebar.row_at_index(at as i32).as_ref());
            if let (Some((old, _)), Some(page)) = (moved, stack.visible_child()) {
                let mut rows = Vec::new();
                setting_rows(&page, &mut rows);
                if let Some(row) = rows.iter().find(|(title, _)| title == old).and_then(|(_, row)| row.downcast_ref::<adw::ActionRow>()) {
                    let row = row.clone();
                    glib::idle_add_local_once(move || open_subpage(&row));
                }
            }
            true
        }
    ));
    let asked = std::env::var(PAGE_ENV).ok();
    std::env::remove_var(PAGE_ENV);
    if !asked.is_some_and(|asked| open_page(&asked)) {
        sidebar.select_row(sidebar.row_at_index(0).as_ref());
    }
    OPEN_PAGE.set(Some(open_page));
    // The VPN helper's profiles that ask for a password are asked about on whichever page is shown.
    if vpn::installed() {
        page_of(&stack, "VPN");
    }

    // Search: the pages' names and every setting's title, over the pages as they are now.
    let hits: Rc<RefCell<Vec<(usize, Option<gtk::Widget>)>>> = Rc::default();
    search.connect_search_changed(glib::clone!(
        #[weak]
        results,
        #[weak]
        stack,
        #[weak]
        side,
        #[strong]
        hits,
        #[strong]
        page_of,
        move |entry| {
            while let Some(old) = results.first_child() {
                results.remove(&old);
            }
            hits.borrow_mut().clear();
            let query = entry.text().to_lowercase();
            if query.is_empty() {
                side.set_visible_child_name("pages");
                return;
            }
            let mut found = Vec::new();
            for (at, name) in page_names.iter().enumerate() {
                if name.to_lowercase().contains(&query) {
                    found.push((name.to_string(), at, None));
                }
                // Searched, every page is built, so that its settings can be found.
                let page = page_of(&stack, name);
                let mut rows = Vec::new();
                setting_rows(&page, &mut rows);
                for (title, row) in rows {
                    if title.to_lowercase().contains(&query) {
                        found.push((title, at, Some(row)));
                    }
                }
            }
            for (title, at, target) in found {
                let label = gtk::Label::builder().label(&title).xalign(0.0).build();
                let place = gtk::Label::builder()
                    .label(&crumbs[at])
                    .xalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .build();
                results.append(&column(2, &[label.upcast_ref(), place.upcast_ref()]));
                hits.borrow_mut().push((at, target));
            }
            side.set_visible_child_name(if hits.borrow().is_empty() { "empty" } else { "results" });
        }
    ));
    search.connect_activate(glib::clone!(
        #[weak]
        results,
        move |_| {
            if let Some(first) = results.row_at_index(0) {
                first.activate();
            }
        }
    ));
    results.connect_row_activated(glib::clone!(
        #[weak]
        sidebar,
        #[weak]
        split,
        #[weak]
        search,
        #[strong]
        hits,
        move |_, row| {
            let Some((at, target)) = hits.borrow().get(row.index() as usize).cloned() else {
                return;
            };
            search.set_text("");
            sidebar.select_row(sidebar.row_at_index(at as i32).as_ref());
            split.set_show_content(true);
            if let Some(target) = target {
                open_subpage_holding(&target);
                // The page is laid out once it is shown, so the row is brought into view
                // again as it settles.
                for wait in [100, 400] {
                    let target = target.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(wait), move || {
                        bring_into_view(&target)
                    });
                }
            }
        }
    ));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("設定")
        .default_width(980)
        .default_height(720)
        .width_request(640)
        .height_request(480)
        .content(&split)
        .build();
    let find = gtk::ShortcutController::new();
    find.set_scope(gtk::ShortcutScope::Global);
    find.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>f"),
        Some(gtk::CallbackAction::new(glib::clone!(
            #[weak]
            search,
            #[weak]
            split,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _| {
                split.set_show_content(false);
                search.grab_focus();
                glib::Propagation::Stop
            }
        ))),
    ));
    window.add_controller(find);
    // Settings changed elsewhere show on the pages: GNOME's as they change, the compositor's
    // looked for as the window comes and goes and every few seconds while it is in front.
    // Its own changes rebuild nothing.
    let watched: Vec<(&str, gio::Settings)> = [
        INTERFACE,
        WM_PREFERENCES,
        A11Y_INTERFACE,
        "org.gnome.desktop.privacy",
    ]
    .into_iter()
    .filter(|schema| {
        gio::SettingsSchemaSource::default().is_some_and(|source| source.lookup(schema, true).is_some())
    })
    .map(|schema| (schema, gio::Settings::new(schema)))
    .collect();
    let seen = RefCell::new(None);
    let refresh = Rc::new(glib::clone!(
        #[strong]
        watched,
        move |config: &str| {
            let now = outside_settings(config, &watched);
            let own = WROTE.take();
            // Until the first answer there is nothing to compare with.
            let Some(before) = seen.replace(Some(now)) else {
                return;
            };
            if !changed_elsewhere(&before, seen.borrow().as_ref().unwrap(), &own) {
                return;
            }
            let config = Config::load();
            remember_look(&config);
            // The Dock adds and takes login items too; the list kept here is written back whole.
            autostart.replace(config.autostart());
            let shown = stack.visible_child_name();
            let scrolled = shown
                .as_ref()
                .and_then(|name| stack.child_by_name(name))
                .and_then(|page| page_scroll(&page))
                .map(|scroll| scroll.value());
            let transition = stack.transition_type();
            stack.set_transition_type(gtk::StackTransitionType::None);
            for name in REBUILT_PAGES {
                if let Some(old) = stack.child_by_name(name) {
                    stack.remove(&old);
                    let page = make_page(name, &config);
                    stack.add_named(&page, Some(name));
                    if shown.as_deref() == Some(name) {
                        if let (Some(offset), Some(scroll)) = (scrolled, page_scroll(&page)) {
                            scroll_once_laid_out(&scroll, offset);
                        }
                    }
                }
            }
            if let Some(shown) = shown {
                stack.set_visible_child_name(&shown);
            }
            stack.set_transition_type(transition);
        }
    ));
    // The compositor is asked off the main thread, once at a time; what GNOME has is read
    // as its answer comes back, so a change made meanwhile is in it.
    let asking = Rc::new(Cell::new(false));
    let look_again = Rc::new(move || {
        if asking.replace(true) {
            return;
        }
        let refresh = refresh.clone();
        let asking = asking.clone();
        glib::spawn_future_local(async move {
            let config = gio::spawn_blocking(|| ipc::msg("config")).await.ok().flatten();
            asking.set(false);
            refresh(&config.unwrap_or_default());
        });
    });
    for (_, settings) in &watched {
        settings.connect_changed(
            None,
            glib::clone!(
                #[strong]
                look_again,
                move |_, _| look_again()
            ),
        );
    }
    window.connect_is_active_notify(glib::clone!(
        #[strong]
        look_again,
        move |_| look_again()
    ));
    let in_front = window.downgrade();
    glib::timeout_add_seconds_local(
        2,
        glib::clone!(
            #[strong]
            look_again,
            move || {
                let Some(window) = in_front.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if window.is_active() {
                    look_again();
                }
                glib::ControlFlow::Continue
            }
        ),
    );
    look_again();
    window.present();
    focus::let_fields_go(&window);
}

/// The pages that show settings another program can change. 聲音 follows its devices on
/// its own, and the rest show nothing that changes.
const REBUILT_PAGES: [&str; 14] = [
    "外觀",
    "協助工具",
    "隱私",
    "桌布",
    "桌面與 Dock",
    "選單列",
    "鎖定畫面",
    "懸停資訊",
    "通知",
    "顯示",
    "電源",
    "鍵盤",
    "滑鼠與觸控板",
    "一般",
];

/// The tag of the page the sidebar shows, under the pages one level down.
const SHOWN_PAGE: &str = "shown";

/// Whether the design language's look was on as Settings started, when GTK read it.
fn look_read_at_start() -> bool {
    static AT_START: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AT_START.get_or_init(gtk_look_on)
}

/// The vertical scroll of a page, inside the scrolled window it keeps.
fn page_scroll(page: &gtk::Widget) -> Option<gtk::Adjustment> {
    let mut child = page.first_child();
    while let Some(widget) = child {
        if let Some(scrolled) = widget.downcast_ref::<gtk::ScrolledWindow>() {
            return Some(scrolled.vadjustment());
        }
        if let Some(found) = page_scroll(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

/// Scrolls a page just made to `offset`, or as far as it goes; until it is laid out it has
/// no height to scroll.
fn scroll_once_laid_out(scroll: &gtk::Adjustment, offset: f64) {
    let handler = Rc::new(RefCell::new(None));
    let id = scroll.connect_changed(glib::clone!(
        #[strong]
        handler,
        move |scroll| {
            if scroll.page_size() == 0.0 {
                return;
            }
            scroll.set_value(offset);
            if let Some(id) = handler.take() {
                scroll.disconnect(id);
            }
        }
    ));
    handler.replace(Some(id));
}

/// The compositor's settings, from `nekoland msg config`, and the GNOME ones the pages show,
/// by name: `key` for the compositor's, `schema key` for GNOME's, as Settings names them
/// when it changes them.
fn outside_settings(config: &str, gnome: &[(&str, gio::Settings)]) -> BTreeMap<String, String> {
    let mut all = BTreeMap::new();
    for line in config.lines() {
        if let Some((key, value)) = line.split_once(" = ") {
            all.insert(key.to_string(), value.to_string());
        }
    }
    for (schema, settings) in gnome {
        let Some(keys) = settings.settings_schema().map(|found| found.list_keys()) else {
            continue;
        };
        for key in keys {
            all.insert(format!("{schema} {key}"), settings.value(&key).print(true).into());
        }
    }
    all
}

/// Whether a setting changed that Settings did not change itself.
fn changed_elsewhere(
    seen: &BTreeMap<String, String>,
    now: &BTreeMap<String, String>,
    own: &HashSet<String>,
) -> bool {
    seen.keys()
        .chain(now.keys())
        .any(|key| !own.contains(key) && seen.get(key) != now.get(key))
}

/// Keeps what the custom look's stylesheet is drawn with up with the settings.
fn remember_look(config: &Config) {
    GLASS_OPACITY.set(config.number("glass_opacity", 0.8));
    if !config.word("font").is_empty() {
        UI_FONT.set(config.word("font").to_string());
    }
}

/// A row that switches a setting of the compositor's.
fn switch_row(config: &Config, title: &str, key: &'static str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .active(config.flag(key))
        .build();
    row.connect_active_notify(move |row| set(&format!("{key} = {}", row.is_active())));
    row
}

/// A row that picks one of `labels` for a setting of the compositor's, kept as `names`.
fn choice_row(
    config: &Config,
    title: &str,
    key: &'static str,
    names: &'static [&'static str],
    labels: &[&str],
) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .selected(config.choice(names, key))
        .build();
    row.connect_selected_notify(move |row| {
        if let Some(name) = names.get(row.selected() as usize) {
            set(&format!("{key} = {name:?}"));
        }
    });
    row
}

/// A row with a slider on it, from `low` to `high` in `step`s, its value at its left as
/// `shown` words it.
fn scale_row(
    title: &str,
    value: f64,
    (low, high, step): (f64, f64, f64),
    shown: fn(f64) -> String,
    changed: impl Fn(f64) + 'static,
) -> adw::ActionRow {
    let label = gtk::Label::builder()
        .label(shown(value))
        .xalign(1.0)
        .width_chars(shown(low).chars().count().max(shown(high).chars().count()) as i32)
        .css_classes(["dim-label", "numeric"])
        .build();
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, low, high, step);
    scale.set_value(value);
    scale.set_width_request(220);
    scale.set_valign(gtk::Align::Center);
    let shown_label = label.clone();
    let changed = Rc::new(changed);
    // The value a drag has reached and not yet passed on: a drag applies its value at most
    // every `SLIDER_EVERY`, each change being a reload of the compositor's settings.
    let unsent: Rc<Cell<Option<f64>>> = Rc::default();
    scale.connect_value_changed(move |scale| {
        // A click on the trough lands between steps; it is put on the nearest one.
        let snapped = low + ((scale.value() - low) / step).round() * step;
        if (snapped - scale.value()).abs() > step / 1000.0 {
            scale.set_value(snapped);
            return;
        }
        shown_label.set_label(&shown(scale.value()));
        if unsent.replace(Some(scale.value())).is_none() {
            let (changed, unsent) = (changed.clone(), unsent.clone());
            glib::timeout_add_local_once(SLIDER_EVERY, move || {
                if let Some(value) = unsent.take() {
                    changed(value);
                }
            });
        }
    });
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&line(8, &[label.upcast_ref(), scale.upcast_ref()]));
    row
}

/// A value from 0 to 1 as a percentage.
fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

/// The choices of how long, in seconds, the computer waits idle before it locks the screen,
/// turns the displays off or sleeps.
const IDLE_AFTER: [i64; 6] = [0, 60, 300, 900, 1800, 3600];

/// A row choosing how long the computer waits idle before what `key` is for.
fn idle_row(config: &Config, title: &str, key: &'static str) -> adw::ComboRow {
    let current = config.0.get(key).and_then(toml::Value::as_integer).unwrap_or(0);
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&["永不", "1 分鐘", "5 分鐘", "15 分鐘", "30 分鐘", "1 小時"]))
        .selected(IDLE_AFTER.iter().position(|seconds| *seconds == current).unwrap_or(0) as u32)
        .build();
    row.connect_selected_notify(move |row| {
        if let Some(seconds) = IDLE_AFTER.get(row.selected() as usize) {
            set(&format!("{key} = {seconds}"));
        }
    });
    row
}

/// A row choosing one of `values` for a whole-number setting, each as its label says.
fn integer_choice_row(
    config: &Config,
    title: &str,
    key: &'static str,
    (values, labels): (&'static [i64], &[&str]),
    default: i64,
) -> adw::ComboRow {
    let current = config.0.get(key).and_then(toml::Value::as_integer).unwrap_or(default);
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .selected(values.iter().position(|value| *value == current).unwrap_or(0) as u32)
        .build();
    row.connect_selected_notify(move |row| {
        if let Some(value) = values.get(row.selected() as usize) {
            set(&format!("{key} = {value}"));
        }
    });
    row
}

/// A row with a line of text for a setting, kept when the entry is applied.
fn text_row(config: &Config, title: &str, key: &'static str) -> adw::EntryRow {
    let row = adw::EntryRow::builder()
        .title(title)
        .text(config.word(key))
        .show_apply_button(true)
        .build();
    row.connect_apply(move |row| set(&format!("{key} = {}", toml_string(row.text().as_str()))));
    row
}

const CLOCK_STYLES: [&str; 3] = ["plain", "opaque", "outline"];
const EVENT_DOTS: [&str; 4] = ["none", "neutral", "highlighted", "multiple"];
/// How many minutes before an event the clock calls attention to it.
const ATTENTION_MINUTES: [i64; 6] = [1, 3, 5, 10, 15, 30];
const WEEK_COUNTS: [i64; 3] = [4, 5, 6];
const FIRST_WEEKDAYS: [i64; 7] = [0, 1, 2, 3, 4, 5, 6];

/// What the clock in the menu bar shows and does about the events.
fn clock_group(config: &Config) -> adw::PreferencesGroup {
    let clock = group("時鐘");
    clock.add(&text_row(config, "自訂格式", "clock_format"));
    clock.add(&switch_row(config, "農曆", "clock_lunar"));
    clock.add(&switch_row(config, "圖示", "clock_icon"));
    clock.add(&choice_row(config, "樣式", "clock_style", &CLOCK_STYLES, &["一般", "實底", "外框"]));
    clock.add(&switch_row(config, "滑過時開啟", "clock_open_on_hover"));
    clock.add(&switch_row(config, "下一個行程", "clock_next_event"));
    clock.add(&switch_row(config, "行程標題", "clock_next_event_title"));
    clock.add(&switch_row(config, "行程將至時閃爍", "clock_attention"));
    clock.add(&integer_choice_row(
        config,
        "提早",
        "clock_attention_minutes",
        (&ATTENTION_MINUTES, &["1 分鐘", "3 分鐘", "5 分鐘", "10 分鐘", "15 分鐘", "30 分鐘"]),
        5,
    ));
    clock.add(&switch_row(config, "提示音", "clock_attention_sound"));
    clock.add(&switch_row(config, "全螢幕提醒", "clock_fullscreen_alert"));
    clock
}

/// The month in the clock's panel.
fn calendar_month_group(config: &Config) -> adw::PreferencesGroup {
    let month = group("月曆");
    month.add(&switch_row(config, "週數", "calendar_week_numbers"));
    month.add(&switch_row(config, "月份外框", "calendar_month_outline"));
    month.add(&switch_row(config, "農曆", "calendar_lunar"));
    month.add(&switch_row(config, "節日", "calendar_holidays"));
    month.add(&choice_row(
        config,
        "行程標記",
        "calendar_event_dots",
        &EVENT_DOTS,
        &["無", "單色", "行事曆顏色", "每個行事曆"],
    ));
    month.add(&integer_choice_row(config, "顯示週數", "calendar_weeks", (&WEEK_COUNTS, &["4 週", "5 週", "6 週"]), 6));
    month.add(&integer_choice_row(
        config,
        "每週第一天",
        "calendar_first_weekday",
        (&FIRST_WEEKDAYS, &["週日", "週一", "週二", "週三", "週四", "週五", "週六"]),
        0,
    ));
    month.add(&switch_row(config, "記住選擇的日期", "calendar_preserve_date"));
    month.add(&switch_row(config, "Alt 預覽日期", "calendar_peek"));
    month
}

/// The day's list in the clock's panel.
fn calendar_day_group(config: &Config) -> adw::PreferencesGroup {
    let day = group("當天行程");
    for (title, key) in [
        ("已結束的行程", "calendar_show_finished"),
        ("尚未開始的行程", "calendar_show_future"),
        ("行程數量", "calendar_show_summary"),
        ("重複標記", "calendar_show_recurrence"),
        ("逾期的提醒事項", "calendar_show_overdue"),
        ("整天行程", "calendar_show_all_day"),
        ("整天行程的詳細資料", "calendar_show_all_day_details"),
    ] {
        day.add(&switch_row(config, title, key));
    }
    day
}

/// A row stating a fact, which can be selected to copy.
fn fact_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&fact_label(value));
    row
}

/// A fact row's value, for one that changes.
fn fact_label(value: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(value)
        .css_classes(["dim-label"])
        .selectable(true)
        .build()
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

/// The applications, the Dock, the menu bar and the wallpaper, a group each.
/// 桌面與 Dock: the applications' view and the Dock, with the Dock's less used switches, the
/// windows and the hot corners a page each one level down.
fn desktop_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let apps = group("應用程式");
    apps.add(&choice_row(
        config,
        "樣式",
        "apps_view",
        &APPS_VIEWS,
        &["概覽", "全螢幕", "分類", "側欄"],
    ));
    page.add(&apps);

    let dock = group("Dock");
    dock.add(&scale_row(
        "大小",
        config.number("dock_size", 40.0),
        (24.0, 96.0, 1.0),
        |size| format!("{size:.0} px"),
        |size| set(&format!("dock_size = {size:.0}")),
    ));
    dock.add(&choice_row(
        config,
        "位置",
        "dock_position",
        &DOCK_POSITIONS,
        &["左", "下", "右"],
    ));
    dock.add(&switch_row(config, "自動隱藏", "dock_autohide"));
    let magnify = switch_row(config, "放大", "dock_magnify");
    dock.add(&magnify);
    let scale = scale_row(
        "放大程度",
        config.number("dock_magnify_scale", 1.6),
        (1.2, 2.0, 0.1),
        |scale| format!("{scale:.1}×"),
        |scale| set(&format!("dock_magnify_scale = {scale:.2}")),
    );
    magnify
        .bind_property("active", &scale, "visible")
        .sync_create()
        .build();
    dock.add(&scale);
    dock.add(&choice_row(
        config,
        "最小化效果",
        "dock_minimize_effect",
        &MINIMIZE_EFFECTS,
        &["精靈效果", "縮放效果"],
    ));
    let others = group("");
    others.add(&switch_row(config, "毛玻璃", "dock_glass"));
    others.add(&switch_row(config, "執行中指示燈", "dock_indicators"));
    others.add(&switch_row(config, "開啟動畫", "dock_bounce"));
    others.add(&switch_row(config, "最小化至應用程式圖示", "dock_minimize_into_app"));
    others.add(&switch_row(config, "最近使用的應用程式", "dock_recents"));
    others.add(&switch_row(config, "只顯示目前桌面的視窗", "dock_this_desktop"));
    dock.add(&subpage_row("其他選項", String::new, page_of_groups(&[others])));
    page.add(&dock);

    let corners = group("");
    for (title, key) in HOT_CORNER_KEYS {
        corners.add(&choice_row(config, title, key, &HOT_CORNERS, &HOT_CORNER_NAMES));
    }
    let more = group("");
    more.add(&subpage_row("視窗", String::new, windows_page(config)));
    more.add(&subpage_row("熱點", hot_corners_summary, page_of_groups(&[corners])));
    page.add(&more);
    page
}

/// The corners set to do something, or 無.
fn hot_corners_summary() -> String {
    let config = Config::load();
    let set: Vec<&str> = HOT_CORNER_KEYS
        .iter()
        .filter(|(_, key)| !matches!(config.word(key), "" | "none"))
        .map(|(title, _)| *title)
        .collect();
    if set.is_empty() { "無".into() } else { set.join("、") }
}

/// Whether there is a VPN to set: the VPN helper's, or NetworkManager's.
fn has_vpn() -> bool {
    vpn::installed() || glib::find_program_in_path("nmcli").is_some()
}

/// A page holding `groups`, for a row to open one level down.
fn page_of_groups(groups: &[adw::PreferencesGroup]) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    for group in groups {
        page.add(group);
    }
    page
}

/// 選單列: which status icons it shows, with its clock, month and day a page each one level
/// down.
fn menu_bar_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let icons = group("狀態圖示");
    icons.add(&switch_row(config, "網路", "menu_bar_network"));
    if vpn::installed() {
        icons.add(&switch_row(config, "VPN", "menu_bar_vpn"));
    }
    icons.add(&switch_row(config, "音量", "menu_bar_sound"));
    if !batteries().is_empty() {
        icons.add(&switch_row(config, "電池", "menu_bar_battery"));
        icons.add(&switch_row(config, "電池百分比", "menu_bar_battery_percent"));
    }
    icons.add(&scale_row(
        "間距",
        config.number("menu_bar_spacing", 8.0),
        (0.0, 32.0, 2.0),
        |gap| format!("{gap:.0} px"),
        |gap| set(&format!("menu_bar_spacing = {gap:.0}")),
    ));
    page.add(&icons);

    let bar = group("");
    let hours = adw::ComboRow::builder()
        .title("時制")
        .model(&gtk::StringList::new(&["24 小時", "12 小時"]))
        .selected(if config.flag("clock_24h") { 0 } else { 1 })
        .build();
    hours.connect_selected_notify(|row| set(&format!("clock_24h = {}", row.selected() == 0)));
    bar.add(&hours);
    bar.add(&switch_row(config, "日期", "clock_date"));
    bar.add(&switch_row(config, "星期", "clock_weekday"));
    bar.add(&switch_row(config, "秒數", "clock_seconds"));
    bar.add(&switch_row(config, "閃爍時間分隔符號", "clock_flash"));
    // Each page's title is its groups' too.
    let untitled = |group: adw::PreferencesGroup| {
        group.set_title("");
        group
    };
    let clock = page_of_groups(&[bar, untitled(clock_group(config))]);
    let more = group("");
    more.add(&subpage_row("時鐘", menu_bar_clock_summary, clock));
    more.add(&subpage_row("系統監控", stats_summary, stats_page(config)));
    more.add(&subpage_row("月曆", String::new, page_of_groups(&[untitled(calendar_month_group(config))])));
    more.add(&subpage_row("當天行程", String::new, page_of_groups(&[untitled(calendar_day_group(config))])));
    page.add(&more);
    page
}

/// The system's readings the menu bar can show, each with its setting, as the panels title them.
const STATS: [(&str, &str); 7] = [
    ("CPU", "stats_cpu"),
    ("GPU", "stats_gpu"),
    ("記憶體", "stats_ram"),
    ("磁碟", "stats_disk"),
    ("網路", "stats_network"),
    ("感應器", "stats_sensors"),
    ("功耗", "stats_power"),
];
const STATS_STYLES: [&str; 6] = ["value", "chart", "bars", "ring", "gauge", "pie"];

/// Whether the GPU has a busy figure to read, as the compositor reads it: amdgpu's, or
/// nvidia-smi's.
fn has_gpu_reading() -> bool {
    let amd = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .any(|card| card.path().join("device/gpu_busy_percent").exists());
    amd || glib::find_program_in_path("nvidia-smi").is_some()
}

/// Whether a fan here can be set.
fn has_fan_control() -> bool {
    std::fs::read_dir("/sys/class/hwmon")
        .into_iter()
        .flatten()
        .flatten()
        .any(|chip| (1..=7).any(|index| chip.path().join(format!("pwm{index}_enable")).exists()))
}

fn has_temperatures() -> bool {
    std::fs::read_dir("/sys/class/hwmon")
        .into_iter()
        .flatten()
        .flatten()
        .any(|chip| chip.path().join("temp1_input").exists())
}

/// 系統監控: which of the system's readings the menu bar shows, and how.
fn stats_page(config: &Config) -> adw::PreferencesPage {
    let shown = group("");
    for (title, key) in STATS {
        let readable = match key {
            "stats_gpu" => has_gpu_reading(),
            "stats_sensors" => has_temperatures(),
            // RAPL's, a battery's or the GPU's, as the compositor adds them up.
            "stats_power" => {
                Path::new("/sys/class/powercap/intel-rapl:0").exists()
                    || Path::new("/sys/class/power_supply/BAT0").exists()
                    || has_gpu_reading()
            }
            _ => true,
        };
        if readable {
            shown.add(&switch_row(config, title, key));
        }
    }
    let look = group("外觀");
    look.add(&choice_row(config, "樣式", "stats_style", &STATS_STYLES, &["數值", "折線圖", "長條圖", "圓環", "儀表", "圓餅圖"]));
    look.add(&switch_row(config, "名稱", "stats_labels"));
    look.add(&switch_row(config, "合併顯示", "stats_combined"));
    look.add(&text_row(config, "自訂文字", "stats_text"));
    let reading = group("");
    reading.add(&integer_choice_row(
        config,
        "更新頻率",
        "stats_interval",
        (&[1, 2, 3, 5], &["每秒", "每 2 秒", "每 3 秒", "每 5 秒"]),
        2,
    ));
    reading.add(&integer_choice_row(
        config,
        "程序數量",
        "stats_processes",
        (&[0, 3, 5, 8], &["不顯示", "3", "5", "8"]),
        5,
    ));
    let units = adw::ComboRow::builder()
        .title("溫度單位")
        .model(&gtk::StringList::new(&["攝氏", "華氏"]))
        .selected(u32::from(config.flag("stats_fahrenheit")))
        .build();
    units.connect_selected_notify(|row| set(&format!("stats_fahrenheit = {}", row.selected() == 1)));
    reading.add(&units);
    reading.add(&switch_row(config, "公用 IP", "stats_public_ip"));
    if has_fan_control() {
        reading.add(&switch_row(config, "風扇控制", "stats_fan_control"));
    }
    let alerts = group("通知");
    alerts.add(&integer_choice_row(
        config,
        "使用率",
        "stats_alert",
        (&[0, 80, 90, 95], &["關閉", "80%", "90%", "95%"]),
        0,
    ));
    alerts.add(&integer_choice_row(
        config,
        "溫度",
        "stats_alert_temperature",
        (&[0, 80, 90, 95], &["關閉", "80 °C", "90 °C", "95 °C"]),
        0,
    ));
    page_of_groups(&[shown, look, reading, alerts])
}

/// The readings shown, or 無.
fn stats_summary() -> String {
    let config = Config::load();
    let shown: Vec<&str> = STATS.iter().filter(|(_, key)| config.flag(key)).map(|(title, _)| *title).collect();
    if shown.is_empty() { "無".into() } else { shown.join("・") }
}

fn menu_bar_clock_summary() -> String {
    let config = Config::load();
    let mut parts = vec![if config.flag("clock_24h") { "24 小時" } else { "12 小時" }];
    for (key, name) in [("clock_date", "日期"), ("clock_weekday", "星期"), ("clock_seconds", "秒數")] {
        if config.flag(key) {
            parts.push(name);
        }
    }
    parts.join("・")
}

/// 桌布: the desktop as it looks, the pictures to pick from, how one fills the desktop, and
/// the desktop's items.
fn wallpaper_page(config: &Config, autostart: &Rc<RefCell<Vec<String>>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    // Without a wallpaper of Nekoland's own, the one swaybg is started with at login.
    let current = config
        .0
        .get("wallpaper")
        .and_then(toml::Value::as_str)
        .map(expand)
        .or_else(|| {
            autostart.borrow().iter().find_map(|command| {
                let mut words = command.strip_prefix("swaybg ")?.split_whitespace();
                words.find(|word| *word == "-i")?;
                words.next().map(expand)
            })
        });
    let pictures = Rc::new(RefCell::new(wallpapers(current.as_deref())));

    let mode = Rc::new(Cell::new(config.choice(&WALLPAPER_MODES, "wallpaper_mode")));
    let (preview, show) = wallpaper_preview(&mode);
    if let Some(path) = &current {
        show(path.clone());
    }
    let shown = group("");
    shown.add(&preview);
    page.add(&shown);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .max_children_per_line(8)
        .min_children_per_line(4)
        .column_spacing(6)
        .row_spacing(6)
        .homogeneous(true)
        .css_classes(["wallpapers"])
        .build();
    for path in pictures.borrow().iter() {
        flow.append(&wallpaper_thumbnail(path));
    }
    if current.is_some() {
        flow.select_child(
            &flow
                .child_at_index(0)
                .expect("the wallpaper in use is listed first"),
        );
    }
    let autostart = autostart.clone();
    let use_wallpaper = Rc::new(move |path: &Path| {
        set(&format!("wallpaper = {}", toml_string(&path.to_string_lossy())));
        show(path.to_path_buf());
        // A wallpaper started at login would stay under this one; it is not started again.
        // shortcut: the one running now stays until the session ends; stopping it needs its process.
        let mut commands = autostart.borrow_mut();
        let before = commands.len();
        commands.retain(|command| !command.starts_with("swaybg"));
        if commands.len() != before {
            save_autostart(&commands);
        }
    });
    flow.connect_child_activated({
        let (pictures, use_wallpaper) = (pictures.clone(), use_wallpaper.clone());
        move |_, child| {
            if let Some(path) = pictures.borrow().get(child.index() as usize) {
                use_wallpaper(path);
            }
        }
    });

    // Another picture is added as the last tile, after the ones offered.
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("其他圖片")
        .css_classes(["card", "activatable"])
        .width_request(96)
        .height_request(60)
        .build();
    let add_tile = gtk::FlowBoxChild::builder().child(&add).focusable(false).build();
    flow.append(&add_tile);
    add.connect_clicked({
        let flow = flow.clone();
        move |button| {
            let images = gtk::FileFilter::new();
            // What swaybg reads, through the same gdk-pixbuf.
            images.add_pixbuf_formats();
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&images);
            let dialog = gtk::FileDialog::builder()
                .title("選擇圖片")
                .filters(&filters)
                .build();
            let (flow, pictures, use_wallpaper) = (flow.clone(), pictures.clone(), use_wallpaper.clone());
            let window = button.root().and_downcast::<gtk::Window>();
            dialog.open(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
                let path = match chosen.map(|file| file.path()) {
                    Ok(Some(path)) => path,
                    Ok(None) => return eprintln!("wallpaper: the file chosen has no local path"),
                    Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
                    Err(err) => return eprintln!("wallpaper: {err}"),
                };
                let index = pictures.borrow().iter().position(|picture| *picture == path);
                let index = index.unwrap_or_else(|| {
                    let index = pictures.borrow().len();
                    flow.insert(&wallpaper_thumbnail(&path), index as i32);
                    pictures.borrow_mut().push(path.clone());
                    index
                });
                if let Some(child) = flow.child_at_index(index as i32) {
                    flow.select_child(&child);
                }
                use_wallpaper(&path);
            });
        }
    });
    let pick = group("");
    pick.add(&flow);
    page.add(&pick);

    let options = group("");
    let fill = segmented(&["填滿", "符合", "置中"], mode.get());
    fill.set_valign(gtk::Align::Center);
    fill.connect_notify_local(Some("active"), {
        let preview = preview.clone();
        move |fill, _| {
            let chosen = fill.property::<u32>("active");
            if let Some(name) = WALLPAPER_MODES.get(chosen as usize) {
                set(&format!("wallpaper_mode = {name:?}"));
                mode.set(chosen);
                preview.queue_draw();
            }
        }
    });
    let fill_row = adw::ActionRow::builder().title("填滿方式").build();
    fill_row.add_suffix(&fill);
    options.add(&fill_row);
    options.add(&switch_row(config, "桌面項目", "desktop_icons"));
    page.add(&options);
    page
}

/// The desktop as the first display shows its wallpaper, in that display's shape, and the
/// way to show another picture on it.
fn wallpaper_preview(mode: &Rc<Cell<u32>>) -> (gtk::AspectFrame, Rc<dyn Fn(PathBuf)>) {
    let (screen_width, screen_height) = lock_preview_size();
    let (screen_width, screen_height) = (f64::from(screen_width), f64::from(screen_height));
    // The picture read small, with its own size.
    let shown: Rc<RefCell<Option<(Pixbuf, f64, f64)>>> = Rc::default();
    let area = gtk::DrawingArea::builder().css_classes(["card"]).build();
    area.set_overflow(gtk::Overflow::Hidden);
    area.set_draw_func({
        let (mode, shown) = (mode.clone(), shown.clone());
        move |_, cr, width, height| {
            let Some((pixbuf, own_width, own_height)) = &*shown.borrow() else {
                return;
            };
            // As the compositor puts it: scaled for the mode, centred, over black.
            let scale = match WALLPAPER_MODES.get(mode.get() as usize) {
                Some(&"fit") => (screen_width / own_width).min(screen_height / own_height),
                Some(&"center") => 1.0,
                _ => (screen_width / own_width).max(screen_height / own_height),
            } * f64::from(width)
                / screen_width;
            cr.set_source_rgb(0.0, 0.0, 0.0);
            let _ = cr.paint();
            cr.translate(
                (f64::from(width) - own_width * scale) / 2.0,
                (f64::from(height) - own_height * scale) / 2.0,
            );
            cr.scale(
                own_width * scale / f64::from(pixbuf.width()),
                own_height * scale / f64::from(pixbuf.height()),
            );
            cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
            let _ = cr.paint();
        }
    });
    let frame = gtk::AspectFrame::builder()
        .ratio((screen_width / screen_height) as f32)
        .obey_child(false)
        .child(&area)
        .height_request(280)
        .build();
    let show = Rc::new(move |path: PathBuf| {
        let (area, shown) = (area.downgrade(), shown.clone());
        glib::spawn_future_local(async move {
            let own = Pixbuf::file_info(&path).map(|(_, width, height)| (f64::from(width), f64::from(height)));
            // Twice the preview's points, for a display at double scale.
            let small = read_scaled(path, 1120, 700).await;
            if let (Some(own), Some(small), Some(area)) = (own, small, area.upgrade()) {
                *shown.borrow_mut() = Some((small, own.0, own.1));
                area.queue_draw();
            }
        });
    });
    (frame, show)
}


const NOTIFICATION_SECONDS: [i64; 4] = [3, 5, 10, 20];
const HOT_CORNERS: [&str; 4] = ["none", "overview", "apps", "lock"];
const HOT_CORNER_NAMES: [&str; 4] = ["無", "所有視窗", "應用程式", "鎖定螢幕"];
const HOT_CORNER_KEYS: [(&str, &str); 4] = [
    ("左上角", "hot_corner_top_left"),
    ("右上角", "hot_corner_top_right"),
    ("左下角", "hot_corner_bottom_left"),
    ("右下角", "hot_corner_bottom_right"),
];
const WM_PREFERENCES: &str = "org.gnome.desktop.wm.preferences";
/// What clicking a GTK title bar does, as GNOME names it.
const TITLEBAR_ACTIONS: [&str; 3] = ["toggle-maximize", "minimize", "none"];

/// A wallpaper as the 桌布 group shows it to be chosen.
fn wallpaper_thumbnail(path: &Path) -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(96)
        .height_request(60)
        .css_classes(["card"])
        .build();
    picture.set_overflow(gtk::Overflow::Hidden);
    show_thumbnail(&picture, path.to_path_buf());
    picture
}

/// Puts a picture in small once it is read, so that reading it does not hold the window up.
fn show_thumbnail(picture: &gtk::Picture, path: PathBuf) {
    let picture = picture.downgrade();
    glib::spawn_future_local(async move {
        if let (Some(small), Some(picture)) = (read_scaled(path, 320, 200).await, picture.upgrade()) {
            picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&small)));
        }
    });
}

/// A picture read on a thread of its own, made to fit `width` by `height`.
async fn read_scaled(path: PathBuf, width: i32, height: i32) -> Option<Pixbuf> {
    // A Pixbuf stays on the thread that read it; its pixels come back.
    let read = gio::spawn_blocking(move || {
        Pixbuf::from_file_at_scale(&path, width, height, true).map(|pixbuf| {
            (
                pixbuf.read_pixel_bytes(),
                pixbuf.has_alpha(),
                pixbuf.width(),
                pixbuf.height(),
                pixbuf.rowstride(),
            )
        })
    })
    .await;
    match read {
        Ok(Ok((pixels, alpha, width, height, stride))) => Some(Pixbuf::from_bytes(
            &pixels,
            gtk::gdk_pixbuf::Colorspace::Rgb,
            alpha,
            8,
            width,
            height,
            stride,
        )),
        Ok(Err(err)) => {
            eprintln!("{err}");
            None
        }
        Err(_) => None,
    }
}

/// GNOME's interface settings, which GTK reads through the settings portal, and Qt and
/// others where they follow GNOME.
const INTERFACE: &str = "org.gnome.desktop.interface";
const TEXT_SCALES: [f64; 6] = [1.0, 1.1, 1.25, 1.5, 1.75, 2.0];
/// GTK's own choice, then the antialiasing it is told to use.
const ANTIALIASING: [&str; 4] = ["automatic", "grayscale", "rgba", "none"];
const HINTING: [&str; 4] = ["none", "slight", "medium", "full"];

/// One of GNOME's interface settings, without the quotes gsettings prints a string in.
fn interface_setting(key: &str) -> String {
    output("gsettings", &["get", INTERFACE, key])
        .map(|value| value.trim().trim_matches('\'').to_string())
        .unwrap_or_default()
}

fn set_interface(key: &str, value: &str) {
    run("gsettings", &["set", INTERFACE, key, value]);
}

/// Fontconfig's generic families, whose own aliases already reach Chinese.
const GENERIC_FAMILIES: [&str; 5] = ["sans", "sans-serif", "serif", "monospace", "system-ui"];

/// Sets `label` in `font`, with the Chinese Nekoland's own text falls back to.
fn write_in(label: &gtk::Label, font: &gtk::pango::FontDescription) {
    let mut font = font.clone();
    // A family named after a generic one would lose to the fallback, which fontconfig
    // binds more strongly than the generic family's alias.
    if let Some(family) = font
        .family()
        .filter(|family| !GENERIC_FAMILIES.contains(&family.to_lowercase().as_str()))
    {
        font.set_family(&format!("{family},Noto Sans CJK TC"));
    }
    let attrs = gtk::pango::AttrList::new();
    attrs.insert(gtk::pango::AttrFontDesc::new(&font));
    label.set_attributes(Some(&attrs));
}

fn mouse_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let pointer = group("");
    pointer.add(&scale_row(
        "指標速度",
        config.number("pointer_speed", 0.0),
        (-1.0, 1.0, 0.1),
        // Adding 0 turns the -0 a step just under 0 rounds to into 0.
        |speed| format!("{:.1}", (speed * 10.0).round() / 10.0 + 0.0),
        |speed| set(&format!("pointer_speed = {speed:.2}")),
    ));
    pointer.add(&scale_row(
        "捲動速度",
        config.number("scroll_speed", 1.0),
        (0.5, 3.0, 0.1),
        |speed| format!("{speed:.1}×"),
        |speed| set(&format!("scroll_speed = {speed:.1}")),
    ));
    pointer.add(&switch_row(config, "滑鼠加速", "pointer_acceleration"));
    let primary = adw::ComboRow::builder()
        .title("主要按鈕")
        .model(&gtk::StringList::new(&["左", "右"]))
        .selected(u32::from(config.flag("left_handed")))
        .build();
    primary.connect_selected_notify(|row| set(&format!("left_handed = {}", row.selected() == 1)));
    pointer.add(&primary);
    pointer.add(&switch_row(config, "自然捲動", "natural_scroll"));
    pointer.add(&gsettings_switch("中鍵貼上", INTERFACE, "gtk-enable-primary-paste"));
    page.add(&pointer);
    let touchpad = group("觸控板");
    // On unless turned off, as the compositor has it.
    let using = adw::SwitchRow::builder()
        .title("使用觸控板")
        .active(config.0.get("touchpad").and_then(toml::Value::as_bool).unwrap_or(true))
        .build();
    using.connect_active_notify(|row| set(&format!("touchpad = {}", row.is_active())));
    touchpad.add(&using);
    touchpad.add(&switch_row(config, "輕觸以點按", "tap_to_click"));
    touchpad.add(&switch_row(config, "打字時停用", "disable_while_typing"));
    page.add(&touchpad);
    page
}

/// What starts at login, each to be taken off, and a field to add another.
/// GNOME's cursor sizes, which GTK draws its own cursors at too.
const CURSOR_SIZES: [i64; 5] = [24, 32, 48, 64, 96];
const A11Y_INTERFACE: &str = "org.gnome.desktop.a11y.interface";

/// Orca, which reads aloud what GTK programs show; GNOME's setting says whether it runs, and
/// the compositor starts it at login when it does.
fn screen_reader_row() -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title("螢幕閱讀器")
        .active(output("gsettings", &["get", A11Y_APPLICATIONS, "screen-reader-enabled"]).is_some_and(|value| value.trim() == "true"))
        .build();
    row.connect_active_notify(|row| {
        run("gsettings", &["set", A11Y_APPLICATIONS, "screen-reader-enabled", &row.is_active().to_string()]);
        if row.is_active() {
            if let Err(err) = Command::new("orca").arg("--replace").spawn() {
                eprintln!("orca: {err}");
            }
        } else if let Err(err) = Command::new("pkill").args(["-x", "orca"]).status() {
            eprintln!("pkill orca: {err}");
        }
    });
    row
}

fn accessibility_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let seeing = group("");
    let current = config.number("cursor_size", 24.0) as i64;
    let size = adw::ComboRow::builder()
        .title("游標大小")
        .model(&gtk::StringList::new(&["預設", "中", "大", "更大", "最大"]))
        .selected(CURSOR_SIZES.iter().position(|size| *size == current).unwrap_or(0) as u32)
        .build();
    size.connect_selected_notify(|row| {
        if let Some(size) = CURSOR_SIZES.get(row.selected() as usize) {
            set(&format!("cursor_size = {size}"));
            set_interface("cursor-size", &size.to_string());
        }
    });
    seeing.add(&size);
    seeing.add(&gsettings_switch("高對比", A11Y_INTERFACE, "high-contrast"));
    seeing.add(&switch_row(config, "反轉顏色", "invert_colors"));
    seeing.add(&choice_row(
        config,
        "色彩濾鏡",
        "color_filter",
        &COLOR_FILTERS,
        &["無", "灰階", "紅色盲", "綠色盲", "藍黃色盲"],
    ));
    let motion = adw::SwitchRow::builder()
        .title("減少動態效果")
        .active(interface_setting("enable-animations") == "false")
        .build();
    // GTK's own motion and the compositor's go together.
    motion.connect_active_notify(|row| {
        let animated = !row.is_active();
        set_interface("enable-animations", &animated.to_string());
        set(&format!("animations = {animated}"));
    });
    seeing.add(&motion);
    if glib::find_program_in_path("orca").is_some() {
        seeing.add(&screen_reader_row());
    }
    // Shares its limits with the compositor's Super Ctrl = and -.
    seeing.add(&scale_row(
        "螢幕縮放",
        config.number("zoom", 1.0),
        (1.0, 8.0, 0.5),
        |zoom| if zoom <= 1.0 { "關閉".into() } else { format!("{zoom:.1}×") },
        |zoom| set(&format!("zoom = {zoom:.1}")),
    ));
    page.add(&seeing);
    let typing = group("");
    typing.add(&integer_choice_row(
        config,
        "篩選鍵",
        "bounce_keys_ms",
        (&BOUNCE_KEYS, &["關閉", "0.25 秒", "0.5 秒", "1 秒"]),
        0,
    ));
    typing.add(&integer_choice_row(
        config,
        "慢速鍵",
        "slow_keys_ms",
        (&SLOW_KEYS, &["關閉", "0.3 秒", "0.5 秒", "1 秒"]),
        0,
    ));
    page.add(&typing);
    page
}

const COLOR_FILTERS: [&str; 5] = ["none", "grayscale", "protanopia", "deuteranopia", "tritanopia"];

/// The times, in milliseconds, a key is held before it counts.
const SLOW_KEYS: [i64; 4] = [0, 300, 500, 1000];

/// The times, in milliseconds, within which a key pressed again is ignored.
const BOUNCE_KEYS: [i64; 4] = [0, 250, 500, 1000];

/// A row that switches one of GNOME's settings, which GTK reads through the portal.
fn gsettings_switch(title: &str, schema: &'static str, key: &'static str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .active(output("gsettings", &["get", schema, key]).is_some_and(|value| value.trim() == "true"))
        .build();
    row.connect_active_notify(move |row| run("gsettings", &["set", schema, key, &row.is_active().to_string()]));
    row
}

/// What opens each kind of file or link, by the types it is the default for; the first
/// lists the choices.
const DEFAULT_APPS: [(&str, &[&str]); 8] = [
    ("網頁瀏覽器", &web::WEB_TYPES),
    ("郵件", &["x-scheme-handler/mailto"]),
    ("檔案", &["inode/directory"]),
    ("文字", &["text/plain"]),
    ("圖片", &["image/png", "image/jpeg", "image/gif", "image/webp"]),
    ("PDF", &["application/pdf"]),
    ("音樂", &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"]),
    ("影片", &["video/mp4", "video/webm", "video/x-matroska"]),
];

/// 一般: settings that are a page each, one level down, as the Mac's General has them.
fn general_page(config: &Config, autostart: &Rc<RefCell<Vec<String>>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let pages = group("");
    for (title, icon, subpage) in [
        ("日期與時間", "preferences-system-time-symbolic", date_time_page()),
        ("語言與地區", "preferences-desktop-locale-symbolic", region::region_page()),
        ("預設應用程式", "preferences-desktop-apps-symbolic", default_apps_page(config)),
        ("啟動項目", "system-run-symbolic", startup_page(autostart)),
    ] {
        let row = subpage_row(title, String::new, subpage);
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        pages.add(&row);
    }
    page.add(&pages);
    page
}

fn default_apps_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let apps = group("");
    for (title, types) in DEFAULT_APPS {
        let choices = gio::AppInfo::all_for_type(types[0]);
        if choices.is_empty() {
            // Nekoland comes with no browser; Market has them.
            if *types == web::WEB_TYPES {
                apps.add(&missing_browser_row(title));
            }
            continue;
        }
        let current = gio::AppInfo::default_for_type(types[0], false).and_then(|app| app.id());
        let row = app_row(title, &choices, |app| app.id().is_some_and(|id| Some(id) == current));
        row.connect_selected_notify(move |row| {
            let Some(app) = choices.get(row.selected() as usize) else {
                return;
            };
            for kind in types {
                if let Err(err) = app.set_as_default_for_type(kind) {
                    eprintln!("{kind}: {err}");
                }
            }
        });
        apps.add(&row);
    }
    // The terminal is Nekoland's own setting, a command, for its shortcut.
    let terminals: Vec<gio::AppInfo> = gio::AppInfo::all()
        .into_iter()
        .filter(|app| {
            app.downcast_ref::<gio::DesktopAppInfo>()
                .and_then(|app| app.categories())
                .is_some_and(|categories| categories.split(';').any(|kind| kind == "TerminalEmulator"))
        })
        .collect();
    if !terminals.is_empty() {
        let command = config.word("terminal").to_string();
        let row = app_row("終端機", &terminals, |app| app.executable().as_os_str() == command.as_str());
        row.connect_selected_notify(move |row| {
            if let Some(app) = terminals.get(row.selected() as usize) {
                set(&format!("terminal = {}", toml_string(&app.executable().to_string_lossy())));
            }
        });
        apps.add(&row);
    }
    page.add(&apps);
    page
}

/// The browser row while there is none: a way to Market's 瀏覽器 page.
fn missing_browser_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle("未安裝").build();
    let get = gtk::Button::builder()
        .label("取得")
        .valign(gtk::Align::Center)
        .build();
    get.connect_clicked(|_| {
        if let Err(err) = Command::new("nekoland-store")
            .env("NEKOLAND_STORE_PAGE", "browsers")
            .spawn()
        {
            eprintln!("nekoland-store: {err}");
        }
    });
    row.add_suffix(&get);
    row.set_activatable_widget(Some(&get));
    row
}

/// A row choosing one of `apps`, by name, with the first that `is_current` selected.
fn app_row(title: &str, apps: &[gio::AppInfo], is_current: impl Fn(&gio::AppInfo) -> bool) -> adw::ComboRow {
    let names: Vec<String> = apps.iter().map(|app| app.display_name().to_string()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&names))
        .selected(apps.iter().position(is_current).unwrap_or(0) as u32)
        .build()
}

fn startup_page(autostart: &Rc<RefCell<Vec<String>>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let commands = group("");
    let added = adw::EntryRow::builder()
        .title("指令")
        .show_apply_button(true)
        .build();
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    commands.add(&added);
    list_commands(&commands, &added, &rows, autostart);
    added.connect_apply(glib::clone!(
        #[weak]
        commands,
        #[strong]
        rows,
        #[strong]
        autostart,
        move |field| {
            let command = field.text().trim().to_string();
            if !command.is_empty() {
                autostart.borrow_mut().push(command);
                save_autostart(&autostart.borrow());
                field.set_text("");
                list_commands(&commands, field, &rows, &autostart);
            }
        }
    ));
    page.add(&commands);
    page
}

/// Lists the commands above the field for another one, again after each change.
fn list_commands(
    group: &adw::PreferencesGroup,
    added: &adw::EntryRow,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    autostart: &Rc<RefCell<Vec<String>>>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    group.remove(added);
    for (index, command) in autostart.borrow().iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(command))
            .build();
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("移除")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        remove.connect_clicked(glib::clone!(
            #[weak]
            group,
            #[weak]
            added,
            #[strong]
            rows,
            #[strong]
            autostart,
            move |_| {
                autostart.borrow_mut().remove(index);
                save_autostart(&autostart.borrow());
                list_commands(&group, &added, &rows, &autostart);
            }
        ));
        row.add_suffix(&remove);
        group.add(&row);
        rows.borrow_mut().push(row);
    }
    group.add(added);
}

/// Where colord and people keep ICC profiles.
fn colour_profile_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/color/icc"), PathBuf::from("/usr/local/share/color/icc")];
    dirs.push(glib::user_data_dir().join("icc"));
    dirs.push(home().join(".color/icc"));
    dirs
}

/// The .icc and .icm files under `dir`, by their names and paths.
fn colour_profiles(dir: &Path, found: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            colour_profiles(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("icc") || extension.eq_ignore_ascii_case("icm"))
        {
            found.push((colour_profile_name(&path), path.to_string_lossy().into_owned()));
        }
    }
}

/// A profile's own description, its file's name when it has none.
fn colour_profile_name(path: &Path) -> String {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| profile_description(&bytes))
        .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned())
}

/// The `desc` tag of an ICC profile: ASCII in version 2, the first of its UTF-16 strings in 4.
fn profile_description(bytes: &[u8]) -> Option<String> {
    let u32_at = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize);
    if bytes.get(36..40)? != b"acsp" {
        return None;
    }
    let (offset, size) = (0..u32_at(128)?.min(1024)).find_map(|index| {
        let entry = 132 + index * 12;
        (bytes.get(entry..entry + 4)? == b"desc").then(|| Some((u32_at(entry + 4)?, u32_at(entry + 8)?)))?
    })?;
    let tag = bytes.get(offset..offset.checked_add(size)?)?;
    let u32_in = |at: usize| Some(u32::from_be_bytes(tag.get(at..at + 4)?.try_into().ok()?) as usize);
    let name = match tag.get(0..4)? {
        b"desc" => {
            let text = tag.get(12..12 + u32_in(8)?)?;
            String::from_utf8_lossy(text.split(|byte| *byte == 0).next()?).into_owned()
        }
        b"mluc" => {
            let (length, start) = (u32_in(20)?, u32_in(24)?);
            let units: Vec<u16> = tag
                .get(start..start.checked_add(length)?)?
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// The ICC profile whose calibration the compositor gives the displays' gamma, with a button to
/// choose one from anywhere.
fn colour_profile_row(config: &Config) -> adw::ComboRow {
    let mut profiles = Vec::new();
    for dir in colour_profile_dirs() {
        colour_profiles(&dir, &mut profiles);
    }
    profiles.sort();
    profiles.dedup_by(|a, b| a.1 == b.1);
    let current = config.word("color_profile").to_owned();
    if !current.is_empty() && !profiles.iter().any(|(_, path)| *path == current) {
        profiles.push((colour_profile_name(Path::new(&current)), current.clone()));
    }
    let names = gtk::StringList::new(&["預設"]);
    for (name, _) in &profiles {
        names.append(name);
    }
    let selected = profiles.iter().position(|(_, path)| *path == current).map_or(0, |index| index + 1);
    let row = adw::ComboRow::builder()
        .title("色彩描述檔")
        .model(&names)
        .selected(selected as u32)
        .build();
    let paths = Rc::new(RefCell::new(profiles.into_iter().map(|(_, path)| path).collect::<Vec<_>>()));
    row.connect_selected_notify(glib::clone!(
        #[strong]
        paths,
        move |row| match row.selected().checked_sub(1) {
            None => unset("color_profile"),
            Some(index) => {
                if let Some(path) = paths.borrow().get(index as usize) {
                    set(&format!("color_profile = {path:?}"));
                }
            }
        }
    ));

    let choose = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("選擇描述檔…")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    choose.connect_clicked(glib::clone!(
        #[weak]
        row,
        move |button| {
            let profiles = gtk::FileFilter::new();
            profiles.set_name(Some("色彩描述檔"));
            for pattern in ["*.icc", "*.icm", "*.ICC", "*.ICM"] {
                profiles.add_pattern(pattern);
            }
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&profiles);
            let dialog = gtk::FileDialog::builder().title("選擇描述檔").filters(&filters).build();
            let window = button.root().and_downcast::<gtk::Window>();
            let (paths, names) = (paths.clone(), names.clone());
            dialog.open(window.as_ref(), gio::Cancellable::NONE, move |chosen| {
                let path = match chosen.map(|file| file.path()) {
                    Ok(Some(path)) => path,
                    Ok(None) => return eprintln!("colour profile: the file chosen has no local path"),
                    Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
                    Err(err) => return eprintln!("colour profile: {err}"),
                };
                let text = path.to_string_lossy().into_owned();
                let index = paths.borrow().iter().position(|known| *known == text);
                let index = index.unwrap_or_else(|| {
                    names.append(&colour_profile_name(&path));
                    paths.borrow_mut().push(text);
                    paths.borrow().len() - 1
                });
                row.set_selected(index as u32 + 1);
            });
        }
    ));
    row.add_suffix(&choose);
    row
}

/// A row choosing an hour of the day for a setting kept as `HH:MM`, `default` until it is set,
/// as the compositor has it; a time of the file's own is kept among the choices.
fn time_row(config: &Config, title: &str, key: &'static str, default: &str) -> adw::ComboRow {
    let mut times: Vec<String> = (0..24).map(|hour| format!("{hour:02}:00")).collect();
    let current = Some(config.word(key)).filter(|word| !word.is_empty()).unwrap_or(default).to_string();
    if !current.is_empty() && !times.contains(&current) {
        times.push(current.clone());
        times.sort();
    }
    let labels: Vec<&str> = times.iter().map(String::as_str).collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&labels))
        .selected(times.iter().position(|time| *time == current).unwrap_or(0) as u32)
        .build();
    row.connect_selected_notify(move |row| {
        if let Some(time) = times.get(row.selected() as usize) {
            set(&format!("{key} = {time:?}"));
        }
    });
    row
}

/// power-profiles-daemon's profiles, from saving the battery to the most speed, and their names.
const POWER_MODES: [(&str, &str); 3] = [("power-saver", "省電"), ("balanced", "平衡"), ("performance", "效能")];

/// The profiles `powerprofilesctl list` offers, and the one in use, marked with "*".
fn power_modes(listed: &str) -> (Vec<&'static str>, Option<&'static str>) {
    let mut offered = Vec::new();
    let mut current = None;
    for line in listed.lines().filter(|line| line.trim_end().ends_with(':')) {
        let name = line.trim().trim_start_matches('*').trim().trim_end_matches(':');
        if let Some((known, _)) = POWER_MODES.iter().find(|(known, _)| *known == name) {
            offered.push(*known);
            if line.trim_start().starts_with('*') {
                current = Some(*known);
            }
        }
    }
    offered.sort_by_key(|name| POWER_MODES.iter().position(|(known, _)| known == name));
    (offered, current)
}

/// 電源模式, where power-profiles-daemon runs.
fn power_mode_row() -> Option<adw::ComboRow> {
    let (offered, current) = power_modes(&output("powerprofilesctl", &["list"])?);
    if offered.len() < 2 {
        return None;
    }
    let labels: Vec<&str> = offered
        .iter()
        .filter_map(|name| POWER_MODES.iter().find(|(known, _)| known == name).map(|(_, label)| *label))
        .collect();
    let row = adw::ComboRow::builder()
        .title("電源模式")
        .model(&gtk::StringList::new(&labels))
        .selected(current.and_then(|current| offered.iter().position(|name| *name == current)).unwrap_or(0) as u32)
        .build();
    row.connect_selected_notify(move |row| {
        if let Some(name) = offered.get(row.selected() as usize) {
            run("powerprofilesctl", &["set", name]);
        }
    });
    Some(row)
}

const POWER_BUTTON: [&str; 3] = ["shutdown", "sleep", "ignore"];
const LID_CLOSE: [&str; 3] = ["sleep", "lock", "ignore"];

/// What happens when the computer is idle, what its power button and lid do, and how
/// worn its batteries are.
fn power_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let idle = group("");
    idle.add(&idle_row(config, "閒置後關閉螢幕", "screen_off_after"));
    idle.add(&idle_row(config, "閒置後睡眠", "sleep_after"));
    if let Some(mode) = power_mode_row() {
        idle.add(&mode);
    }
    page.add(&idle);

    let batteries = batteries();
    let buttons = group("");
    buttons.add(&choice_row(
        config,
        "電源按鈕",
        "power_button",
        &POWER_BUTTON,
        &["關機⋯", "睡眠", "不動作"],
    ));
    // A computer with a lid is told by its battery.
    if !batteries.is_empty() {
        buttons.add(&choice_row(
            config,
            "闔上螢幕",
            "lid_close",
            &LID_CLOSE,
            &["睡眠", "鎖定螢幕", "不動作"],
        ));
    }
    page.add(&buttons);

    for battery in &batteries {
        let title = if batteries.len() > 1 { battery.model.as_str() } else { "電池" };
        let rows = group(title);
        if let Some(health) = battery.health {
            rows.add(&fact_row("最大容量", &format!("{health}%")));
        }
        if let Some(cycles) = battery.cycles {
            rows.add(&fact_row("循環次數", &cycles.to_string()));
        }
        page.add(&rows);
    }
    page
}

/// What `timedatectl show` says: the time zone, and whether the clock is set over the network
/// and can be.
fn time_settings() -> (String, bool, bool) {
    let shown = output("timedatectl", &["show"]).unwrap_or_default();
    let get = |key: &str| field(&shown, key, '=').unwrap_or_default();
    (get("Timezone"), get("NTP") == "yes", get("CanNTP") == "yes")
}

/// The time zone and whether the clock sets itself, both systemd's, changed as root.
fn date_time_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let rows = group("");
    page.add(&rows);
    let automatic = adw::SwitchRow::builder().title("自動設定日期與時間").build();
    let zones: Vec<String> = output("timedatectl", &["list-timezones"])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let labels: Vec<&str> = zones.iter().map(String::as_str).collect();
    let zone = adw::ComboRow::builder()
        .title("時區")
        .model(&gtk::StringList::new(&labels))
        .enable_search(true)
        .build();
    zone.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        gtk::Expression::NONE,
        "string",
    )));
    rows.add(&automatic);
    rows.add(&zone);
    let zones = Rc::new(zones);

    // Set while the rows are made to show what systemd says, which is not a change to make.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[strong]
        zones,
        #[strong]
        showing,
        #[weak]
        automatic,
        #[weak]
        zone,
        move || {
            let (current, ntp, can_ntp) = time_settings();
            showing.set(true);
            automatic.set_active(ntp);
            automatic.set_visible(can_ntp);
            if let Some(index) = zones.iter().position(|known| *known == current) {
                zone.set_selected(index as u32);
            }
            automatic.set_sensitive(true);
            zone.set_sensitive(true);
            showing.set(false);
        }
    ));
    show();
    // Turned down or failed, the rows go back to what systemd says.
    let change = {
        let show = show.clone();
        move |reason: &str, args: &[&str]| {
            let show = show.clone();
            let what = args.join(" ");
            match sudo(reason, args, gio::SubprocessFlags::NONE) {
                Ok(sudo) => sudo.wait_check_async(gio::Cancellable::NONE, move |result| {
                    if let Err(err) = result {
                        eprintln!("{what}: {err}");
                    }
                    show();
                }),
                Err(err) => {
                    eprintln!("sudo: {err}");
                    show();
                }
            }
        }
    };
    let change = Rc::new(change);
    automatic.connect_active_notify(glib::clone!(
        #[strong]
        showing,
        #[strong]
        change,
        move |row| {
            if showing.get() {
                return;
            }
            row.set_sensitive(false);
            let on = if row.is_active() { "true" } else { "false" };
            change("變更日期與時間的設定方式", &["timedatectl", "set-ntp", on]);
        }
    ));
    zone.connect_selected_notify(move |row| {
        if showing.get() {
            return;
        }
        let Some(name) = zones.get(row.selected() as usize) else {
            return;
        };
        row.set_sensitive(false);
        change("變更時區", &["timedatectl", "set-timezone", name]);
    });
    page
}

/// The disks that hold the system and people's files, with how full each is; the boot and
/// EFI partitions are left out.
fn storage_group() -> Option<adw::PreferencesGroup> {
    let listed = output(
        "df",
        &["-B1", "--output=target,size,used", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs", "-x", "overlay", "-x", "efivarfs"],
    )?;
    let disks = disks(&listed);
    if disks.is_empty() {
        return None;
    }
    let rows = group("儲存空間");
    for (place, size, used) in disks {
        let title = if place == "/" { "系統磁碟".to_string() } else { place.clone() };
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(format!("已使用 {}，共 {}", gb(used), gb(size)))
            .activatable(true)
            .build();
        // What takes the space is shown by 磁碟用量.
        row.connect_activated(move |_| {
            let program = program::path("nekoland-disk");
            if let Err(err) = gio::Subprocess::newv(&[program.as_os_str(), std::ffi::OsStr::new(&place)], gio::SubprocessFlags::NONE) {
                eprintln!("nekoland-disk: {err}");
            }
        });
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let bar = gtk::LevelBar::builder()
            .value(used as f64 / size.max(1) as f64)
            .width_request(160)
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&bar);
        rows.add(&row);
    }
    Some(rows)
}

/// df's "Mounted on, size, used" lines, past its heading, of the disks storage_group shows.
fn disks(listed: &str) -> Vec<(String, u64, u64)> {
    listed
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (place, size, used) = (words.next()?, words.next()?.parse().ok()?, words.next()?.parse().ok()?);
            let shown = ["/", "/home"].contains(&place)
                || ["/home/", "/media/", "/mnt/", "/run/media/"].iter().any(|start| place.starts_with(start));
            shown.then(|| (place.to_string(), size, used))
        })
        .collect()
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Runs `args` as root through `sudo -A`, whose password Nekoland's askpass asks for,
/// saying `reason`; `flags` as STDERR_PIPE keep what sudo and `args` say to show it.
fn sudo(reason: &str, args: &[&str], flags: gio::SubprocessFlags) -> Result<gio::Subprocess, glib::Error> {
    let askpass = program::path("nekoland-askpass");
    let launcher = gio::SubprocessLauncher::new(flags);
    if flags.contains(gio::SubprocessFlags::STDERR_PIPE) {
        // What is said is read for why it failed, so it is said in the words looked for.
        launcher.setenv("LC_MESSAGES", "C", true);
    }
    launcher.setenv("SUDO_ASKPASS", askpass, true);
    launcher.setenv("NEKOLAND_ASKPASS_REASON", reason, true);
    let argv: Vec<&std::ffi::OsStr> = ["sudo", "-A", "--"].iter().chain(args).map(std::ffi::OsStr::new).collect();
    launcher.spawn(&argv)
}

/// Runs `args` as root, asking for the password with `reason`, and feeds it `input`; `then`
/// learns whether it worked. What it said when it did not is logged.
fn as_root(reason: &str, args: &[&str], input: Option<String>, then: impl FnOnce(bool) + 'static) {
    let what = args.first().copied().unwrap_or_default().to_string();
    let flags = gio::SubprocessFlags::STDERR_PIPE
        | if input.is_some() { gio::SubprocessFlags::STDIN_PIPE } else { gio::SubprocessFlags::NONE };
    let child = match sudo(reason, args, flags) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("sudo {what}: {err}");
            return then(false);
        }
    };
    let waited = child.clone();
    child.communicate_utf8_async(input, gio::Cancellable::NONE, move |result| {
        let worked = match result {
            Ok((_, said)) if !waited.is_successful() => {
                eprintln!("sudo {what}: {}", said.unwrap_or_default().trim());
                false
            }
            Ok(_) => true,
            Err(err) => {
                eprintln!("sudo {what}: {err}");
                false
            }
        };
        then(worked);
    });
}

/// Asks which image service makes the wallpaper, with its key or its sign-in, and what it
/// shows; `then` gets the service and the description, the choice and the key kept.
fn ask_for_wallpaper(parent: &impl IsA<gtk::Widget>, describe: bool, then: impl Fn(String, String) + 'static) {
    let names: Vec<&str> = imagegen::SERVICES.iter().map(|(_, name)| *name).collect();
    let kept = imagegen::read("service");
    let service = adw::ComboRow::builder()
        .title("服務")
        .model(&gtk::StringList::new(&names))
        .selected(imagegen::SERVICES.iter().position(|(id, _)| Some(*id) == kept.as_deref()).unwrap_or(0) as u32)
        .build();
    let key = adw::PasswordEntryRow::builder().title("API 金鑰").build();
    let account = adw::ActionRow::builder().title("ChatGPT 帳號").build();
    let sign_in = gtk::Button::builder().label("登入").valign(gtk::Align::Center).build();
    account.add_suffix(&sign_in);
    let description = adw::EntryRow::builder().title("描述").build();
    let rows = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .width_request(alert::FIELD_WIDTH)
        .build();
    for row in [service.upcast_ref::<gtk::Widget>(), key.upcast_ref(), account.upcast_ref(), description.upcast_ref()] {
        rows.append(row);
    }
    // Cutting a subject out needs no description.
    description.set_visible(describe);
    let described = move |description: &adw::EntryRow| !describe || !description.text().trim().is_empty();
    let (title, action) = if describe { ("AI 生成桌布", "生成") } else { ("去背", "去背") };
    let alert = adw::AlertDialog::new(Some(title), Some(&format!("每次{action}都由所選服務計費。")));
    alert.set_extra_child(Some(&rows));
    alert.add_responses(&[("cancel", "取消"), ("do", action)]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("do"));
    alert.set_close_response("cancel");
    let chosen = {
        let service = service.clone();
        move || imagegen::SERVICES[service.selected() as usize % imagegen::SERVICES.len()].0
    };
    // What 生成 needs: a description, and the chosen service's key or its sign-in.
    let ready = Rc::new(glib::clone!(
        #[weak]
        alert,
        #[weak]
        key,
        #[weak]
        account,
        #[weak]
        sign_in,
        #[weak]
        description,
        #[strong]
        chosen,
        move || {
            let id = chosen();
            let chatgpt = id == imagegen::CHATGPT;
            key.set_visible(!chatgpt);
            account.set_visible(chatgpt);
            let state = if chatgpt { imagegen::chatgpt_signed_in() } else { None };
            let signed_in = state == Some(true);
            if chatgpt {
                sign_in.set_visible(state == Some(false));
                account.set_subtitle(match state {
                    None => "未安裝 Codex",
                    Some(true) => "已登入",
                    Some(false) => "未登入",
                });
            } else {
                key.set_text(&imagegen::read(id).unwrap_or_default());
            }
            let has_key = !chatgpt && !key.text().trim().is_empty();
            alert.set_response_enabled("do", described(&description) && (signed_in || has_key));
        }
    ));
    ready();
    service.connect_selected_notify(glib::clone!(
        #[strong]
        ready,
        move |_| ready()
    ));
    key.connect_changed(glib::clone!(
        #[weak]
        alert,
        #[weak]
        description,
        move |key| alert.set_response_enabled("do", described(&description) && !key.text().trim().is_empty())
    ));
    description.connect_changed(glib::clone!(
        #[weak]
        alert,
        #[weak]
        key,
        #[strong]
        chosen,
        move |description| {
            let given = if chosen() == imagegen::CHATGPT {
                imagegen::chatgpt_signed_in() == Some(true)
            } else {
                !key.text().trim().is_empty()
            };
            alert.set_response_enabled("do", described(description) && given);
        }
    ));
    // Codex signs in through the browser and keeps the sign-in itself.
    sign_in.connect_clicked(glib::clone!(
        #[strong]
        ready,
        move |button| {
            let codex = imagegen::codex().unwrap_or_else(|| PathBuf::from("codex"));
            match gio::Subprocess::newv(&[codex.as_os_str(), std::ffi::OsStr::new("login")], gio::SubprocessFlags::NONE) {
                Ok(child) => {
                    button.set_sensitive(false);
                    let (button, ready) = (button.clone(), ready.clone());
                    child.wait_async(gio::Cancellable::NONE, move |_| {
                        button.set_sensitive(true);
                        ready();
                    });
                }
                Err(err) => {
                    eprintln!("codex login: {err}");
                    alert::notice(button, "無法登入 ChatGPT", Some("需要先安裝 Codex。"));
                }
            }
        }
    ));
    alert.choose(parent, gio::Cancellable::NONE, move |response| {
        if response != "do" {
            return;
        }
        let id = chosen();
        if let Err(err) = imagegen::write("service", id) {
            eprintln!("image-services: {err}");
        }
        if id != imagegen::CHATGPT {
            if let Err(err) = imagegen::write(id, &key.text()) {
                eprintln!("image-services: {err}");
            }
        }
        then(id.to_string(), description.text().trim().to_string());
    });
}

/// fprintd's names for the fingers, in the order new ones are taken, and what they are called.
const FINGERS: [(&str, &str); 10] = [
    ("right-index-finger", "右手食指"),
    ("left-index-finger", "左手食指"),
    ("right-thumb", "右手拇指"),
    ("left-thumb", "左手拇指"),
    ("right-middle-finger", "右手中指"),
    ("left-middle-finger", "左手中指"),
    ("right-ring-finger", "右手無名指"),
    ("left-ring-finger", "左手無名指"),
    ("right-little-finger", "右手小指"),
    ("left-little-finger", "左手小指"),
];

/// The fingers enrolled with fprintd, and one more to add, with whether the lock takes them;
/// hidden where there is no reader. `unlock` is the switch for the lock.
fn fingerprint_group(unlock: adw::SwitchRow) -> adw::PreferencesGroup {
    let fingers = group("指紋");
    fingers.set_visible(false);
    fill_fingers(&fingers, &Rc::default(), &unlock);
    fingers
}

/// Lists the fingers again, off the window's thread, as fprintd's answer can be slow.
fn fill_fingers(fingers: &adw::PreferencesGroup, rows: &Rc<RefCell<Vec<gtk::Widget>>>, unlock: &adw::SwitchRow) {
    let (weak, rows, unlock) = (fingers.downgrade(), rows.clone(), unlock.clone());
    glib::spawn_future_local(async move {
        let user = std::env::var("USER").unwrap_or_default();
        let listed = gio::spawn_blocking({
            let user = user.clone();
            move || output("fprintd-list", &[&user]).map(|listed| enrolled_fingers(&listed))
        })
        .await
        .ok()
        .flatten();
        let Some(fingers) = weak.upgrade() else { return };
        for row in rows.take() {
            fingers.remove(&row);
        }
        let Some(enrolled) = listed else {
            fingers.set_visible(false);
            return;
        };
        fingers.set_visible(true);
        let mut shown: Vec<gtk::Widget> = Vec::new();
        let refill = {
            let (weak, rows, unlock) = (fingers.downgrade(), rows.clone(), unlock.clone());
            Rc::new(move || {
                if let Some(fingers) = weak.upgrade() {
                    fill_fingers(&fingers, &rows, &unlock);
                }
            })
        };
        for finger in enrolled.iter().cloned() {
            let name = FINGERS.iter().find(|(id, _)| *id == finger).map_or(finger.as_str(), |(_, name)| name);
            let row = adw::ActionRow::builder().title(name).build();
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("移除")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let (user, refill) = (user.clone(), refill.clone());
            remove.connect_clicked(move |remove| {
                let (user, finger, refill, parent) = (user.clone(), finger.clone(), refill.clone(), remove.clone());
                alert::confirm(remove, "移除這個指紋？", None, "移除", move || {
                    // fprintd lets only an authorised session change fingers, and Nekoland
                    // has no polkit agent: root may.
                    as_root("移除指紋", &["fprintd-delete", &user, "-f", &finger], None, move |worked| {
                        if !worked {
                            alert::notice(&parent, "無法移除指紋", None);
                        }
                        refill();
                    });
                });
            });
            row.add_suffix(&remove);
            fingers.add(&row);
            shown.push(row.upcast());
        }
        if let Some((next, _)) = FINGERS.iter().find(|(id, _)| !enrolled.iter().any(|finger| finger == id)) {
            let add = adw::ActionRow::builder().title("新增指紋").activatable(true).build();
            add.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let refill = refill.clone();
            add.connect_activated(move |add| enroll_finger(add, next, refill.clone()));
            fingers.add(&add);
            shown.push(add.upcast());
        }
        unlock.set_sensitive(!enrolled.is_empty());
        fingers.add(&unlock);
        shown.push(unlock.clone().upcast());
        *rows.borrow_mut() = shown;
    });
}

/// The fingers `fprintd-list` lists, as fprintd names them.
fn enrolled_fingers(listed: &str) -> Vec<String> {
    listed
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- #")?.split_once(": ").map(|(_, finger)| finger.trim().to_string()))
        .collect()
}

/// Enrolls `finger` through `fprintd-enroll`, as root for want of a polkit agent, saying how
/// far it has come; `done` once it has ended either way.
fn enroll_finger(parent: &impl IsA<gtk::Widget>, finger: &'static str, done: Rc<dyn Fn()>) {
    let user = std::env::var("USER").unwrap_or_default();
    let child = match sudo("新增指紋", &["fprintd-enroll", "-f", finger, &user], gio::SubprocessFlags::STDOUT_PIPE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("fprintd-enroll: {err}");
            return alert::notice(parent, "無法新增指紋", None);
        }
    };
    let alert = adw::AlertDialog::new(Some("新增指紋"), Some("將手指放在感應器上，抬起後再放上，直到完成。"));
    alert.add_response("cancel", "取消");
    alert.set_close_response("cancel");
    let (stopped, cancelled) = (child.clone(), Rc::new(Cell::new(false)));
    alert.connect_response(
        None,
        glib::clone!(
            #[strong]
            cancelled,
            move |_, _| {
                cancelled.set(true);
                stopped.force_exit();
            }
        ),
    );
    alert.present(Some(parent));
    let Some(said) = child.stdout_pipe() else { return };
    let lines = gio::DataInputStream::new(&said);
    let parent = parent.as_ref().clone();
    glib::spawn_future_local(async move {
        let mut scans = 0;
        let mut ended = None;
        while let Ok(Some(line)) = lines.read_line_utf8_future(glib::Priority::DEFAULT).await {
            let Some(result) = line.trim().strip_prefix("Enroll result: ") else { continue };
            match result {
                "enroll-stage-passed" => {
                    scans += 1;
                    alert.set_body(&format!("已掃描 {scans} 次"));
                }
                "enroll-completed" => ended = Some(None),
                "enroll-duplicate" => ended = Some(Some("這個指紋已經新增過")),
                "enroll-retry-scan" | "enroll-swipe-too-short" | "enroll-finger-not-centered" | "enroll-remove-and-retry" => {
                    alert.set_body("請再掃描一次");
                }
                _ => ended = Some(Some("無法新增指紋")),
            }
            if ended.is_some() {
                break;
            }
        }
        // Closed by 取消, or sudo's password was not given: nothing to tell.
        alert.force_close();
        if let (Some(Some(why)), false) = (ended, cancelled.get()) {
            alert::notice(&parent, why, None);
        }
        done();
    });
}

/// Whether xrdp is running, or None when it is not installed.
fn xrdp_running() -> Option<bool> {
    let shown = output("systemctl", &["show", "xrdp", "--property=LoadState,ActiveState"])?;
    let value = |key: &str| field(&shown, key, '=');
    (value("LoadState").as_deref() == Some("loaded"))
        .then(|| value("ActiveState").as_deref() == Some("active"))
}

/// The computer's IPv4 addresses, or its name when it has none.
fn remote_address() -> String {
    let mut hosts: Vec<String> = output("hostname", &["-I"])
        .unwrap_or_default()
        .split_whitespace()
        .filter(|address| address.contains('.'))
        .map(String::from)
        .collect();
    if hosts.is_empty() {
        hosts.extend(output("hostname", &[]).map(|name| name.trim().to_string()));
    }
    hosts.join("、")
}

/// The port xrdp listens on, from its `[Globals]`, which come first in its file.
fn xrdp_port(ini: &str) -> u16 {
    ini.lines()
        .take_while(|line| !line.trim().starts_with('[') || line.trim() == "[Globals]")
        .find_map(|line| line.trim().strip_prefix("port=")?.trim().parse().ok())
        .unwrap_or(RDP_PORT)
}

fn xsession_is_nekoland() -> bool {
    std::fs::read_to_string(home().join(".xsession")).is_ok_and(|text| text.starts_with(XSESSION_HEAD))
}

/// Gives xrdp sessions Nekoland through `~/.xsession`, which xrdp's session script runs,
/// keeping one written by hand to put back when they are given the system's again.
fn set_xsession(nekoland: bool) -> std::io::Result<()> {
    let path = home().join(".xsession");
    let saved = glib::user_config_dir().join(SAVED_XSESSION_FILE);
    if nekoland == xsession_is_nekoland() {
        return Ok(());
    }
    if !nekoland {
        return if saved.exists() {
            std::fs::rename(&saved, &path)
        } else {
            std::fs::remove_file(&path)
        };
    }
    if path.exists() {
        std::fs::create_dir_all(saved.parent().unwrap_or(Path::new("/")))?;
        std::fs::rename(&path, &saved)?;
    }
    let program = nekoland_program().to_string_lossy().replace('\'', "'\\''");
    write_atomic(&path, &format!("{XSESSION_HEAD}exec '{program}'\n"))
}

/// The compositor's program: the one that started Settings put itself first on the PATH.
fn nekoland_program() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("nekoland"))
                .find(|program| program.exists())
        })
        .unwrap_or_else(|| PathBuf::from("nekoland"))
}

#[derive(Debug, PartialEq)]
struct Battery {
    model: String,
    /// How much it holds when full, as a percentage of what it was made to hold.
    health: Option<u64>,
    cycles: Option<u64>,
}

fn batteries() -> Vec<Battery> {
    let Ok(supplies) = std::fs::read_dir("/sys/class/power_supply") else {
        return Vec::new();
    };
    supplies
        .filter_map(|supply| std::fs::read_to_string(supply.ok()?.path().join("uevent")).ok())
        .filter_map(|uevent| parse_battery(&uevent))
        .collect()
}

/// A power supply's `uevent`, if it is a battery. It counts in energy or in charge,
/// whichever its driver knows.
fn parse_battery(uevent: &str) -> Option<Battery> {
    let field = |key: &str| {
        uevent
            .lines()
            .find_map(|line| line.strip_prefix("POWER_SUPPLY_")?.strip_prefix(key)?.strip_prefix('='))
    };
    let number = |key: &str| field(key)?.parse::<u64>().ok();
    if field("TYPE")? != "Battery" {
        return None;
    }
    let full = number("ENERGY_FULL").zip(number("ENERGY_FULL_DESIGN"));
    let health = full
        .or_else(|| number("CHARGE_FULL").zip(number("CHARGE_FULL_DESIGN")))
        .filter(|(_, design)| *design > 0)
        .map(|(full, design)| (full * 100 + design / 2) / design);
    Some(Battery {
        model: field("MODEL_NAME").unwrap_or_default().to_string(),
        health,
        // Drivers that do not count cycles say 0.
        cycles: number("CYCLE_COUNT").filter(|cycles| *cycles > 0),
    })
}

const PERMISSION_STORE: &str = "org.freedesktop.impl.portal.PermissionStore";
const PERMISSION_STORE_PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";

/// What the portals let each application use, by the table and entry the permission store
/// keeps it under. Only sandboxed applications, as Flatpak's, are asked and held to it.
const PERMISSIONS: [(&str, &str, &str, &str); 7] = [
    ("相機", "devices", "camera", "camera-web-symbolic"),
    ("麥克風", "devices", "microphone", "audio-input-microphone-symbolic"),
    ("喇叭", "devices", "speakers", "audio-speakers-symbolic"),
    ("定位服務", "location", "location", "find-location-symbolic"),
    ("螢幕擷取", "screenshot", "screenshot", "applets-screenshooter-symbolic"),
    ("背景執行", "background", "background", "system-run-symbolic"),
    ("通知", "notifications", "notification", "preferences-system-notifications-symbolic"),
];

/// The applications that asked for each permission, with whether they have it.
/// How many minutes the launcher's clipboard history keeps what was copied; -1 without end.
const CLIPBOARD_HISTORY: [i64; 5] = [0, 30, 480, 10080, -1];

fn privacy_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .inspect_err(|err| eprintln!("session bus: {err}"))
        .ok();
    // shortcut: read once, as the page is made; an application that asks while Settings is
    // open shows when it opens again.
    // Each permission a row telling how many applications asked for it; they are listed one
    // level down.
    let permissions = group("");
    for (title, table, id, icon) in PERMISSIONS {
        let rows = group("");
        let apps = bus.as_ref().map(|bus| app_permissions(bus, table, id)).unwrap_or_default();
        let asked = if apps.is_empty() { "沒有應用程式".to_string() } else { format!("{} 個應用程式", apps.len()) };
        if apps.is_empty() {
            rows.add(&adw::ActionRow::builder().title("沒有應用程式").css_classes(["dim-label"]).build());
        }
        for (app, permissions) in apps {
            let info = gio::DesktopAppInfo::new(&format!("{app}.desktop"));
            let row = adw::SwitchRow::builder()
                .title(info.as_ref().map_or(app.clone(), |info| info.display_name().to_string()))
                .active(allowed(table, &permissions))
                .build();
            if let Some(icon) = info.and_then(|info| info.icon()) {
                row.add_prefix(&gtk::Image::builder().gicon(&icon).pixel_size(32).build());
            }
            let bus = bus.clone();
            row.connect_active_notify(move |row| {
                let Some(bus) = &bus else { return };
                let changed = with_allowed(table, &permissions, row.is_active());
                bus.call(
                    Some(PERMISSION_STORE),
                    PERMISSION_STORE_PATH,
                    PERMISSION_STORE,
                    "SetPermission",
                    Some(&(table, true, id, app.as_str(), changed).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    -1,
                    gio::Cancellable::NONE,
                    |result| {
                        if let Err(err) = result {
                            eprintln!("permission store: {err}");
                        }
                    },
                );
            });
            rows.add(&row);
        }
        let row = subpage_row(title, move || asked.clone(), page_of_groups(&[rows]));
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        permissions.add(&row);
    }
    // Programs that read the clipboard in the background, by the name the compositor asked
    // about them by.
    let readers = config.0.get("clipboard_readers").and_then(toml::Value::as_table).cloned().unwrap_or_default();
    let asked = if readers.is_empty() { "沒有應用程式".to_string() } else { format!("{} 個應用程式", readers.len()) };
    let rows = group("");
    if readers.is_empty() {
        rows.add(&adw::ActionRow::builder().title("沒有應用程式").css_classes(["dim-label"]).build());
    }
    let kept = Rc::new(RefCell::new(readers.clone()));
    for (name, allowed) in readers {
        let row = adw::SwitchRow::builder().title(name.as_str()).active(allowed.as_bool().unwrap_or(false)).build();
        let kept = kept.clone();
        row.connect_active_notify(move |row| {
            kept.borrow_mut().insert(name.clone(), row.is_active().into());
            set(&format!("clipboard_readers = {}", toml::Value::Table(kept.borrow().clone())));
        });
        rows.add(&row);
    }
    let row = subpage_row("剪貼簿", move || asked.clone(), page_of_groups(&[rows]));
    row.add_prefix(&gtk::Image::from_icon_name("edit-paste-symbolic"));
    permissions.add(&row);
    page.add(&permissions);
    let history = group("");
    history.add(&integer_choice_row(
        config,
        "剪貼簿紀錄",
        "clipboard_history",
        (&CLIPBOARD_HISTORY, &["不記錄", "30 分鐘", "8 小時", "7 天", "不限"]),
        -1,
    ));
    history.add(&gsettings_switch(
        "記錄最近使用的檔案",
        "org.gnome.desktop.privacy",
        "remember-recent-files",
    ));
    page.add(&history);
    page
}

/// Each application's permissions for one entry, by application ID; none when nothing has
/// asked for it yet.
fn app_permissions(bus: &gio::DBusConnection, table: &str, id: &str) -> Vec<(String, Vec<String>)> {
    let reply = bus.call_sync(
        Some(PERMISSION_STORE),
        PERMISSION_STORE_PATH,
        PERMISSION_STORE,
        "Lookup",
        Some(&(table, id).to_variant()),
        glib::VariantTy::new("(a{sas}v)").ok(),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    );
    let reply = match reply {
        Ok(reply) => reply,
        Err(err) => {
            if gio::DBusError::remote_error(&err).as_deref() != Some("org.freedesktop.portal.Error.NotFound") {
                eprintln!("permission store: {err}");
            }
            return Vec::new();
        }
    };
    let mut apps: Vec<_> = reply
        .child_value(0)
        .get::<std::collections::HashMap<String, Vec<String>>>()
        .unwrap_or_default()
        .into_iter()
        // Programs outside a sandbox are kept under no ID; they are never asked.
        .filter(|(app, _)| !app.is_empty())
        .collect();
    apps.sort();
    apps
}

/// Whether an application may use it: `yes` for a device, any accuracy but `NONE` for
/// the location.
fn allowed(table: &str, permissions: &[String]) -> bool {
    match (table, permissions.first().map(String::as_str)) {
        ("location", Some(accuracy)) => accuracy != "NONE",
        (_, first) => first == Some("yes"),
    }
}

/// The permissions with the first, the one that allows or not, set; the location's
/// others, when it was last used, are kept.
fn with_allowed(table: &str, permissions: &[String], on: bool) -> Vec<String> {
    let first = match (table, on) {
        ("location", true) => "EXACT",
        ("location", false) => "NONE",
        (_, true) => "yes",
        (_, false) => "no",
    };
    std::iter::once(first.to_string()).chain(permissions.iter().skip(1).cloned()).collect()
}

/// The computer's name, as the network and the prompt show it, changed through systemd.
fn computer_name_row(name: &str) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title("電腦名稱").text(name).show_apply_button(true).build();
    let saved = Rc::new(RefCell::new(name.to_string()));
    row.connect_apply(move |row| {
        let typed = row.text().trim().to_string();
        // A hostname is letters, digits and hyphens, not starting or ending with one.
        let valid = !typed.is_empty()
            && typed.len() <= 63
            && typed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !typed.starts_with('-')
            && !typed.ends_with('-');
        if !valid {
            row.set_text(&saved.borrow());
            return alert::notice(row, "無法使用這個名稱", Some("請只用英文字母、數字和連字號，且不以連字號開頭或結尾。"));
        }
        let (row, saved) = (row.clone(), saved.clone());
        let name = typed.clone();
        as_root("變更電腦名稱", &["hostnamectl", "set-hostname", &typed], None, move |worked| {
            if worked {
                saved.replace(name);
            } else {
                row.set_text(&saved.borrow());
                alert::notice(&row, "無法變更電腦名稱", Some("請確認密碼正確後再試一次。"));
            }
        });
    });
    row
}

/// When banners show, how long and with what, and which programs may show them.
fn notifications_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let quiet = group("勿擾");
    let scheduled = switch_row(config, "排程", "quiet_scheduled");
    let from = time_row(config, "開始", "quiet_from", "22:00");
    let to = time_row(config, "結束", "quiet_to", "07:00");
    scheduled.bind_property("active", &from, "visible").sync_create().build();
    scheduled.bind_property("active", &to, "visible").sync_create().build();
    quiet.add(&scheduled);
    quiet.add(&from);
    quiet.add(&to);
    page.add(&quiet);

    let notices = group("橫幅");
    let current = config.0.get("notification_seconds").and_then(toml::Value::as_integer).unwrap_or(5);
    let banner = adw::ComboRow::builder()
        .title("橫幅顯示時間")
        .model(&gtk::StringList::new(&["3 秒", "5 秒", "10 秒", "20 秒"]))
        .selected(NOTIFICATION_SECONDS.iter().position(|seconds| *seconds == current).unwrap_or(1) as u32)
        .build();
    banner.connect_selected_notify(|row| {
        if let Some(seconds) = NOTIFICATION_SECONDS.get(row.selected() as usize) {
            set(&format!("notification_seconds = {seconds}"));
        }
    });
    notices.add(&banner);
    notices.add(&switch_row(config, "顯示預覽", "notification_previews"));
    page.add(&notices);

    let apps = group("應用程式");
    let off: Vec<String> = config
        .0
        .get("notifications_off")
        .and_then(toml::Value::as_array)
        .map(|names| names.iter().filter_map(|name| name.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let off = Rc::new(RefCell::new(off));
    // Where the compositor writes the names programs send notifications under.
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state"))
        .join("nekoland/notifiers");
    let mut notifiers: Vec<String> =
        std::fs::read_to_string(&state).unwrap_or_default().lines().map(str::to_string).collect();
    notifiers.sort_by_key(|name| name.to_lowercase());
    notifiers.dedup();
    if notifiers.is_empty() {
        apps.add(&adw::ActionRow::builder().title("沒有應用程式").css_classes(["dim-label"]).build());
    }
    let installed = gio::AppInfo::all();
    for name in notifiers {
        let app = notifier_app(&installed, &name);
        let row = adw::SwitchRow::builder()
            .title(app.as_ref().map_or(name.clone(), |app| app.display_name().to_string()))
            .use_markup(false)
            .active(!off.borrow().contains(&name))
            .build();
        let icon = app.and_then(|app| app.icon());
        let image = match &icon {
            Some(icon) => gtk::Image::from_gicon(icon),
            None => gtk::Image::from_icon_name("preferences-system-notifications-symbolic"),
        };
        image.set_pixel_size(32);
        row.add_prefix(&image);
        let off = off.clone();
        row.connect_active_notify(move |row| {
            let mut off = off.borrow_mut();
            off.retain(|other| *other != name);
            if !row.is_active() {
                off.push(name.clone());
            }
            let quoted: Vec<String> = off.iter().map(|name| format!("{name:?}")).collect();
            set(&format!("notifications_off = [{}]", quoted.join(", ")));
        });
        apps.add(&row);
    }
    page.add(&apps);
    page
}

/// The application a program sends notifications as `name` is: the one of that name, or
/// whose desktop file or program is called so, as `nekoland-files` is Files'. One shown in
/// the applications comes before a hidden helper of the same name, as Sunshine has.
fn notifier_app(installed: &[gio::AppInfo], name: &str) -> Option<gio::AppInfo> {
    let name = name.to_lowercase();
    let mut installed: Vec<&gio::AppInfo> = installed.iter().collect();
    installed.sort_by_key(|app| !app.should_show());
    installed.iter().copied().find(|app| app.name().to_lowercase() == name).or_else(|| {
        installed.iter().copied().find(|app| {
            let id = app.id().map(|id| id.trim_end_matches(".desktop").to_lowercase());
            // Not `executable()`, which some entries have none of and the binding reads anyway.
            let program = app.commandline().and_then(|line| {
                let line = line.to_string_lossy().into_owned();
                let program = line.split_whitespace().next()?;
                Some(Path::new(program).file_name()?.to_string_lossy().to_lowercase())
            });
            id.as_deref() == Some(name.as_str()) || program.as_deref() == Some(name.as_str())
        })
    }).cloned()
}

fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let computer = group("");
    for (title, value) in facts() {
        if title == "電腦名稱" {
            computer.add(&computer_name_row(&value));
        } else {
            computer.add(&fact_row(title, &value));
        }
    }
    page.add(&computer);
    if let Some(storage) = storage_group() {
        page.add(&storage);
    }
    let design = group("");
    design.add(&subpage_row("設計語言", String::new, design_page()));
    page.add(&design);
    page
}

fn save_autostart(autostart: &[String]) {
    let quoted: Vec<String> = autostart.iter().map(|command| toml_string(command)).collect();
    set(&format!("autostart = [{}]", quoted.join(", ")));
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_setting_keeps_accents_and_emoji_of_their_own() {
        for text in ["/home/me/cafe\u{301}.jpg", "相片 ❤\u{fe0f}", "say \"hi\" \\ there"] {
            let line = format!("wallpaper = {}", super::toml_string(text));
            let table: toml::Table = toml::from_str(&line).unwrap();
            assert_eq!(table["wallpaper"].as_str(), Some(text), "{line}");
        }
    }

    #[test]
    fn apt_history_lists_the_runs_that_changed_packages_newest_first() {
        let text = "Start-Date: 2026-11-01  10:00:00\nCommandline: apt install foo\nInstall: foo:arm64 (1.0), libfoo:arm64 (1.0, automatic)\nEnd-Date: 2026-11-01  10:00:05\n\n\
                    Start-Date: 2026-11-02  09:00:00\nCommandline: apt update\nEnd-Date: 2026-11-02  09:00:01\n\n\
                    Start-Date: 2026-11-03  08:00:00\nUpgrade: bar:arm64 (1, 2)\nRemove: baz:arm64 (3)\nEnd-Date: 2026-11-03  08:00:09\n";
        let runs = super::parse_apt_history(text);
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].date.as_str(), runs[0].upgraded, runs[0].removed), ("2026-11-03", 1, 1));
        assert_eq!((runs[1].installed, runs[1].packages.clone()), (2, vec!["foo".to_string(), "libfoo".to_string()]));
    }

    use super::*;

    #[test]
    fn the_enrolled_fingers_are_read_from_fprintd_list() {
        let listed = "found 1 devices\nDevice at /net/reactivated/Fprint/Device/0\nUsing device /net/reactivated/Fprint/Device/0\nFingerprints for user doeshing on Virtual device (press):\n - #0: right-index-finger\n - #1: left-thumb\n";
        assert_eq!(enrolled_fingers(listed), ["right-index-finger", "left-thumb"]);
        assert!(enrolled_fingers("User doeshing has no fingers enrolled for Virtual device.\n").is_empty());
    }

    #[test]
    fn power_modes_are_read() {
        let listed = "  performance:\n    CpuDriver:\tintel_pstate\n\n* balanced:\n    CpuDriver:\tintel_pstate\n\n  power-saver:\n    Degraded:   no\n";
        assert_eq!(power_modes(listed), (vec!["power-saver", "balanced", "performance"], Some("balanced")));
    }

    #[test]
    fn a_profile_is_named_by_its_description() {
        let profile = |tag: &[u8]| {
            let mut bytes = vec![0; 128];
            bytes[36..40].copy_from_slice(b"acsp");
            bytes.extend(1u32.to_be_bytes());
            bytes.extend(b"desc");
            bytes.extend(144u32.to_be_bytes());
            bytes.extend((tag.len() as u32).to_be_bytes());
            bytes.extend(tag);
            bytes
        };
        let mut v2 = b"desc\0\0\0\0".to_vec();
        v2.extend(6u32.to_be_bytes());
        v2.extend(b"sRGB\0\0");
        assert_eq!(profile_description(&profile(&v2)).as_deref(), Some("sRGB"));
        let mut v4 = b"mluc\0\0\0\0".to_vec();
        v4.extend([0, 0, 0, 1, 0, 0, 0, 12]);
        v4.extend(b"enUS");
        v4.extend(8u32.to_be_bytes());
        v4.extend(28u32.to_be_bytes());
        v4.extend([0, b'P', 0, b'3', 0, b'2', 0, b'7']);
        assert_eq!(profile_description(&profile(&v4)).as_deref(), Some("P327"));
        assert_eq!(profile_description(b"no profile"), None);
    }

    #[test]
    fn disks_are_the_system_and_people_s() {
        let listed = "Mounted on     1B-blocks        Used\n/ 32021266432 22749257728\n/boot 2040373248 128360448\n/boot/efi 1124995072 6840320\n/home 100 40\n/media/usb 10 5\n/homework 1 1\n";
        assert_eq!(
            disks(listed),
            [("/".into(), 32021266432, 22749257728), ("/home".into(), 100, 40), ("/media/usb".into(), 10, 5)]
        );
    }

    #[test]
    fn xrdp_port_is_read_from_its_globals_only() {
        let ini = "[Globals]\nport=3390\n\n[Xvnc]\nport=-1\n";
        assert_eq!(xrdp_port(ini), 3390);
        assert_eq!(
            xrdp_port("[Globals]\nport=tcp://:3391\n[vnc-any]\nport=5900\n"),
            RDP_PORT
        );
    }

    #[test]
    fn only_settings_changed_elsewhere_count() {
        let settings = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        let seen = settings(&[
            ("key_repeat", "true"),
            ("org.gnome.desktop.interface cursor-blink-time", "1200"),
        ]);
        let own: HashSet<String> = ["key_repeat".to_string()].into();
        let toggled = settings(&[
            ("key_repeat", "false"),
            ("org.gnome.desktop.interface cursor-blink-time", "1200"),
        ]);
        assert!(!changed_elsewhere(&seen, &toggled, &own));
        let both = settings(&[
            ("key_repeat", "false"),
            ("org.gnome.desktop.interface cursor-blink-time", "900"),
        ]);
        assert!(changed_elsewhere(&seen, &both, &own));
        assert!(changed_elsewhere(&seen, &toggled, &HashSet::new()));
        let added = settings(&[
            ("key_repeat", "true"),
            ("org.gnome.desktop.interface cursor-blink-time", "1200"),
            ("clock_date", "false"),
        ]);
        assert!(changed_elsewhere(&seen, &added, &own));
        assert!(!changed_elsewhere(&seen, &seen, &HashSet::new()));
    }

    #[test]
    fn a_family_is_quoted_for_css_without_what_would_end_it() {
        assert_eq!(css_family("Ubuntu Sans"), "\"Ubuntu Sans\"");
        assert_eq!(css_family("A\"; } * { color: red"), "\"A; } * { color: red\"");
        assert!(gtk_look().contains("--nekoland-font: \"Ubuntu Sans\", \"Noto Sans CJK TC\", sans-serif;"));
    }

    #[test]
    fn a_permission_is_allowed_and_set_as_its_table_writes_it() {
        let words = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert!(allowed("devices", &words(&["yes"])));
        assert!(!allowed("devices", &words(&["no"])));
        assert!(!allowed("devices", &[]));
        assert!(allowed("location", &words(&["CITY", "1700000000"])));
        assert!(!allowed("location", &words(&["NONE", "1700000000"])));
        assert_eq!(with_allowed("devices", &words(&["no"]), true), words(&["yes"]));
        assert_eq!(
            with_allowed("location", &words(&["CITY", "1700000000"]), false),
            words(&["NONE", "1700000000"])
        );
        assert_eq!(with_allowed("location", &[], true), words(&["EXACT"]));
    }

    #[test]
    fn a_battery_is_read_in_energy_or_in_charge() {
        let energy = "POWER_SUPPLY_NAME=BAT0\nPOWER_SUPPLY_TYPE=Battery\nPOWER_SUPPLY_CYCLE_COUNT=312\n\
            POWER_SUPPLY_ENERGY_FULL_DESIGN=57000000\nPOWER_SUPPLY_ENERGY_FULL=52440000\n\
            POWER_SUPPLY_MODEL_NAME=5B10W13930\n";
        assert_eq!(
            parse_battery(energy),
            Some(Battery { model: "5B10W13930".into(), health: Some(92), cycles: Some(312) })
        );
        let charge = "POWER_SUPPLY_TYPE=Battery\nPOWER_SUPPLY_CHARGE_FULL=4000\n\
            POWER_SUPPLY_CHARGE_FULL_DESIGN=5000\nPOWER_SUPPLY_CYCLE_COUNT=0\n";
        assert_eq!(parse_battery(charge), Some(Battery { model: String::new(), health: Some(80), cycles: None }));
        assert_eq!(parse_battery("POWER_SUPPLY_TYPE=Mains\nPOWER_SUPPLY_ONLINE=1\n"), None);
    }

    #[test]
    fn metadata_values_are_read_from_pw_metadata() {
        let text = "Found \"settings\" metadata 32\nupdate: id:0 key:'clock.rate' value:'44100' type:''\n";
        assert_eq!(metadata_value(text).as_deref(), Some("44100"));
        assert_eq!(metadata_value("Found \"settings\" metadata 32\n"), None);
    }

    #[test]
    fn multi_output_members_are_read_back_from_what_is_written() {
        let members = ["alsa_output.usb".to_string(), "bluez_output.\"quoted\"".to_string()];
        let conf = multi_output_conf(&members);
        assert!(conf.contains("libpipewire-module-combine-stream"));
        assert_eq!(multi_output_members(&conf), members);
    }

    #[test]
    fn the_running_clock_is_the_drivers_in_pw_tops_last_look() {
        let header = "S   ID  QUANT   RATE    WAIT    BUSY   W/Q   B/Q  ERR FORMAT           NAME \n";
        let idle = "S   39      0      0    ---     ---   ---   ---     0                  alsa_output.x\n";
        let playing = "R   39   1024  48000  11.0us  11.4us  0.00  0.00    0    S16LE 2 48000 alsa_output.x\n\
                       R   62    256  44100  37.1us  17.7us  0.00  0.00    0    S16LE 2 44100  = pw-play\n";
        assert_eq!(running_clock(&format!("{header}{playing}{header}{idle}")), None);
        assert_eq!(running_clock(&format!("{header}{idle}{header}{playing}")), Some((48000, 1024)));
        // A device open with nothing on it runs as a driver of nothing, at no rate.
        let unclocked = "R   60      0      0   0.0us   0.0us  ???   ???     0    S24LE 2 96000 alsa_input.mic\n";
        assert_eq!(running_clock(&format!("{header}{unclocked}{playing}")), Some((48000, 1024)));
        assert_eq!(running_clock(&format!("{header}{unclocked}")), None);
        assert_eq!(clock_text((48000, 1024)), "48000 Hz · 緩衝區 1024 個取樣（21.3 毫秒）");
    }

    #[test]
    fn the_default_clock_is_read_from_the_settings_metadata() {
        let dump = r#"[{"id": 32, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "settings"},
            "metadata": [{"key": "clock.rate", "value": 48000}, {"key": "clock.quantum", "value": "1024"}]}]"#;
        assert_eq!(default_clock(dump), Some((48000, 1024)));
        assert_eq!(default_clock("[]"), None);
    }

    #[test]
    fn clients_coming_and_going_are_not_a_change_to_pipewire() {
        let (mut clients, mut text) = (HashSet::new(), String::new());
        let client = "[\n  {\n    \"id\": 90,\n    \"type\": \"PipeWire:Interface:Client\",\n    \"info\": {}\n  }\n]\n";
        let gone = "[\n  {\n    \"id\": 90,\n    \"info\": null\n  }\n]\n";
        // Printed in pieces, as it is read.
        text.push_str(&client[..20]);
        assert!(!pipewire_changed(&mut text, &mut clients));
        text.push_str(&client[20..]);
        text.push_str(gone);
        assert!(!pipewire_changed(&mut text, &mut clients));
        assert!(text.is_empty() && clients.is_empty());

        text.push_str("[\n  {\n    \"id\": 58,\n    \"type\": \"PipeWire:Interface:Node\",\n    \"info\": {}\n  }\n]\n");
        assert!(pipewire_changed(&mut text, &mut clients));
        text.push_str("[\n  {\n    \"id\": 58,\n    \"info\": null\n  }\n]\n");
        assert!(pipewire_changed(&mut text, &mut clients));
    }

    #[test]
    fn sound_cards_are_read_with_the_profiles_they_have() {
        let dump = r#"[
            {"id": 46, "info": {"props": {"media.class": "Audio/Device", "device.description": "USB DAC"},
             "params": {"EnumProfile": [
                {"index": 0, "description": "Off", "available": "yes"},
                {"index": 1, "description": "Analog Stereo", "available": "unknown"},
                {"index": 2, "description": "Digital Stereo (HDMI)", "available": "no"},
                {"index": 3, "description": "Pro Audio", "available": "yes"}],
              "Profile": [{"index": 3}]}}},
            {"id": 47, "info": {"props": {"media.class": "Audio/Device", "device.name": "alsa_card.hdmi"},
             "params": {"EnumProfile": [{"index": 0, "description": "Off", "available": "yes"},
                {"index": 1, "description": "Digital Stereo (HDMI)", "available": "no"}],
              "Profile": [{"index": 1}]}}},
            {"id": 59, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.usb"}}}
        ]"#;
        let cards = parse_sound_cards(dump);
        assert_eq!(cards.len(), 2);
        let card = &cards[0];
        assert_eq!((card.id, card.description.as_str(), card.profile), (46, "USB DAC", 3));
        assert_eq!(card.profiles, [(0, "Off".into()), (1, "Analog Stereo".into()), (3, "Pro Audio".into())]);
        // An unplugged HDMI screen still shows the profile the card is in.
        assert_eq!(cards[1].description, "alsa_card.hdmi");
        assert_eq!(cards[1].profiles, [(0, "Off".into()), (1, "Digital Stereo (HDMI)".into())]);
    }

    #[test]
    fn playing_programs_are_named_by_application() {
        let dump = r#"[
            {"id": 61, "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "firefox",
              "application.name": "Firefox"}}},
            {"id": 62, "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "mpv"}}},
            {"id": 59, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.usb"}}}
        ]"#;
        assert_eq!(app_streams(dump), [(61, "Firefox".to_string()), (62, "mpv".to_string())]);
    }

    #[test]
    fn audio_nodes_are_read_with_their_formats_and_defaults() {
        let dump = r#"[
            {"id": 0, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
             "metadata": [{"key": "default.audio.sink", "value": {"name": "alsa_output.usb"}}]},
            {"id": 59, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.usb",
              "node.description": "USB DAC", "device.api": "alsa", "audio.rate": 96000, "device.id": 46},
             "params": {"EnumFormat": [{"format": {"default": "S16LE", "alt1": "S16LE", "alt2": "S24_32LE"},
              "rate": {"default": 48000, "min": 44100, "max": 96000}}],
              "Format": [{"format": "S24_32LE", "rate": 96000, "channels": 2}]}}},
            {"id": 60, "info": {"props": {"media.class": "Audio/Source", "node.name": "bluez_input.x",
              "device.api": "bluez5"}, "params": {"EnumFormat": [{"format": "S16LE",
              "rate": {"default": 16000, "alt1": 16000}}]}}},
            {"id": 61, "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "firefox"}}}
        ]"#;
        let devices = parse_audio_nodes(dump);
        assert_eq!((devices[0].card, devices[0].open_as.as_deref()), (Some(46), Some("S24_32LE 96000 Hz 2ch")));
        assert_eq!((devices[1].card, devices[1].open_as.as_deref()), (None, None));
        let summary: Vec<_> = devices
            .iter()
            .map(|d| (d.id, d.description.as_str(), d.default, d.alsa, d.rates.clone(), d.formats.join(" "), d.rate))
            .collect();
        assert_eq!(
            summary,
            [
                (59, "USB DAC", true, true, vec![44_100, 48_000, 88_200, 96_000], "S16LE S24_32LE".into(), 96_000),
                (60, "bluez_input.x", false, false, vec![16_000], "S16LE".into(), 0),
            ]
        );
    }

    #[test]
    fn xkb_layouts_are_each_followed_by_their_variants() {
        let list = "! model\n  pc105  Generic\n\n! layout\n  us   English (US)\n  fr   French\n\n\
                    ! variant\n  azerty   fr: French (AZERTY)\n  dvorak   us: English (Dvorak)\n\n\
                    ! option\n  ctrl:nocaps  Caps Lock as Ctrl\n";
        let names: Vec<_> = xkb_layouts(list)
            .into_iter()
            .map(|(layout, variant, description)| format!("{layout}/{variant}/{description}"))
            .collect();
        assert_eq!(
            names,
            ["us//English (US)", "us/dvorak/English (Dvorak)", "fr//French", "fr/azerty/French (AZERTY)"]
        );
    }

    /// Turning the look on and off keeps the user's own rules, and moves an earlier import.
    #[test]
    fn gtk_look_keeps_user_rules() {
        let config = std::env::temp_dir().join(format!("nekoland-gtk-{}", std::process::id()));
        let dir = config.join("gtk-4.0");
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &config);
        let user = dir.join("gtk.css");
        std::fs::write(&user, format!("{OLD_GTK_IMPORT}\nlabel {{ color: red; }}\n")).unwrap();

        set_gtk_look(true).unwrap();
        let on = std::fs::read_to_string(&user).unwrap();
        assert_eq!(on, format!("{GTK_IMPORT}\nlabel {{ color: red; }}\n"));
        assert!(gtk_look_on());
        assert_eq!(std::fs::read_to_string(dir.join(GTK_LOOK_FILE)).unwrap(), gtk_look());

        set_gtk_look(false).unwrap();
        assert_eq!(std::fs::read_to_string(&user).unwrap(), "label { color: red; }\n");
        assert!(!gtk_look_on() && !dir.join(GTK_LOOK_FILE).exists());
        std::fs::remove_dir_all(&config).unwrap();
    }

    /// The GTK look keeps the colours the Slint widgets take from theme.slint, light and dark.
    /// A scoped stylesheet sets the root's variables on the scope and keeps every other
    /// rule inside it, in a media query too, comments gone.
    #[test]
    fn a_stylesheet_is_kept_to_a_scope() {
        let css =
            "/* a { } */ :root { --a: 1; } b, c > d { e: f; } @media (x) { :root { --a: 2; } g { h: i; } }";
        assert_eq!(
            scoped(css, ".s").split_whitespace().collect::<Vec<_>>().join(" "),
            ".s { --a: 1; } .s b, .s c > d { e: f; } @media (x) { .s { --a: 2; } .s g { h: i; } }"
        );
    }

    /// Settings' own stylesheet ends with the dark colours, outside any media query, only
    /// when dark.
    #[test]
    fn the_look_settings_loads_itself_is_dark_when_dark() {
        assert_eq!(look_for(false), gtk_look());
        let added = &look_for(true)[gtk_look().len()..];
        assert!(added.contains("--nekoland-paper: #1b1b1e;"));
        assert!(!added.contains("@media"));
    }

    #[test]
    fn the_gtk_look_has_the_design_tokens() {
        let theme = include_str!("../ui/theme.slint");
        let (light, dark) = GTK_LOOK
            .split_once("@media (prefers-color-scheme: dark)")
            .expect("a dark part");
        for token in TOKENS
            .map(|(token, ..)| token)
            .into_iter()
            .chain(["on-ink", "on-danger"])
        {
            let line = theme
                .lines()
                .find(|line| line.contains(&format!("<color> {token}: dark ? ")))
                .unwrap_or_else(|| panic!("{token} in theme.slint"));
            let (_, values) = line.split_once("dark ? ").expect("dark and light");
            let (dark_value, light_value) = values
                .trim_end_matches(';')
                .split_once(" : ")
                .expect("two colours");
            assert!(
                light.contains(&format!("--nekoland-{token}: {light_value};")),
                "light {token}"
            );
            assert!(
                dark.contains(&format!("--nekoland-{token}: {dark_value};")),
                "dark {token}"
            );
        }
    }

    /// The GTK 3 theme names the same colours as the GTK 4 look, light and dark.
    #[test]
    fn the_gtk3_theme_has_the_gtk4_tokens() {
        let light = include_str!("../../shell/gtk3/Nekoland/gtk-3.0/gtk.css");
        let dark = include_str!("../../shell/gtk3/Nekoland/gtk-3.0/gtk-dark.css");
        for token in TOKENS
            .map(|(token, ..)| token)
            .into_iter()
            .chain(["on-ink", "on-danger"])
        {
            for (theme, is_dark) in [(light, false), (dark, true)] {
                let define = format!("@define-color nekoland_{} {};", token.replace('-', "_"), token_value(token, is_dark));
                assert!(theme.contains(&define), "{define}");
            }
        }
    }
}
