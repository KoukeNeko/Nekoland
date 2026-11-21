//! `nekoland-settings`: Nekoland's settings, in GTK and libadwaita as GNOME's own settings
//! are, so that it looks as the system's GTK style says. What the compositor keeps it asks
//! for and changes through `nekoland msg`, so the compositor alone reads and writes its file
//! and puts each change in effect; the rest is the system's, through its own tools.

mod alert;
mod backup;
mod bluetooth;
mod display;
mod focus;
mod imagegen;
mod keys;
mod network;
mod printers;
mod region;
mod selfupdate;
mod themes;
mod users;
mod web;
mod wifi;

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
const SINK: &str = "@DEFAULT_AUDIO_SINK@";
/// What PipeWire calls the sound a program is playing.
const PLAYBACK_STREAM: &str = "Stream/Output/Audio";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

/// What a program printed, or None when it could not be run or failed.
fn output(program: &str, args: &[&str]) -> Option<String> {
    if let ("gsettings", ["set" | "reset", schema, key, ..]) = (program, args) {
        WROTE.with_borrow_mut(|wrote| wrote.insert(format!("{schema} {key}")));
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

/// Pages that are now a row of another, by their old names, as programs still open them.
const MOVED_PAGES: [(&str, &str); 8] = [
    ("設計語言", "關於"),
    ("桌面", "桌布"),
    ("視窗", "桌面與 Dock"),
    ("字型", "外觀"),
    ("預設應用程式", "一般"),
    ("啟動項目", "一般"),
    ("語言與地區", "一般"),
    ("日期與時間", "一般"),
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
                "外觀" => appearance_page(config, &look).upcast(),
                "協助工具" => accessibility_page(config).upcast(),
                "隱私" => privacy_page().upcast(),
                "桌布" => wallpaper_page(config, &autostart).upcast(),
                "桌面與 Dock" => desktop_page(config).upcast(),
                "選單列" => menu_bar_page(config).upcast(),
                "鎖定畫面" => lock_page(config).upcast(),
                "懸停資訊" => hover_page(config).upcast(),
                "通知" => notifications_page(config).upcast(),
                "顯示" => display::display_page(config).upcast(),
                "電源" => power_page(config).upcast(),
                "聲音" => sound_page().upcast(),
                "鍵盤" => keyboard_page(config).upcast(),
                "滑鼠與觸控板" => mouse_page(config).upcast(),
                "使用者" => users::users_page().upcast(),
                "一般" => general_page(config, &autostart).upcast(),
                "印表機" => printers::printers_page().upcast(),
                "軟體更新" => updates_page().upcast(),
                "備份" => backup::backup_page().upcast(),
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
            .chain([
                ("網路", "network-wired-symbolic"),
                ("共享", "preferences-desktop-remote-desktop-symbolic"),
            ])
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
        ],
        vec![
            ("一般", "preferences-system-symbolic"),
            ("通知", "preferences-system-notifications-symbolic"),
            ("軟體更新", "software-update-available-symbolic"),
            ("備份", "document-save-symbolic"),
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
    // Settings changed elsewhere show on the pages: looked for as the window comes and goes,
    // and every few seconds while it is in front. Its own changes rebuild nothing.
    let seen = RefCell::new(outside_settings());
    let refresh = Rc::new(move || {
        let now = outside_settings();
        let elsewhere = changed_elsewhere(&seen.borrow(), &now, &WROTE.take());
        seen.replace(now);
        if !elsewhere {
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
    });
    window.connect_is_active_notify(glib::clone!(
        #[strong]
        refresh,
        move |_| refresh()
    ));
    let in_front = window.downgrade();
    glib::timeout_add_seconds_local(2, move || {
        let Some(window) = in_front.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if window.is_active() {
            refresh();
        }
        glib::ControlFlow::Continue
    });
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

/// The compositor's settings and the GNOME ones the pages show, by name: `key` for the
/// compositor's, `schema key` for GNOME's, as Settings names them when it changes them.
fn outside_settings() -> BTreeMap<String, String> {
    let mut all = BTreeMap::new();
    for line in output("nekoland", &["msg", "config"]).unwrap_or_default().lines() {
        if let Some((key, value)) = line.split_once(" = ") {
            all.insert(key.to_string(), value.to_string());
        }
    }
    for schema in [
        INTERFACE,
        WM_PREFERENCES,
        A11Y_INTERFACE,
        "org.gnome.desktop.privacy",
    ] {
        for line in output("gsettings", &["list-recursively", schema])
            .unwrap_or_default()
            .lines()
        {
            if let [schema, key, value] = line.splitn(3, ' ').collect::<Vec<_>>()[..] {
                all.insert(format!("{schema} {key}"), value.to_string());
            }
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
    scale.connect_value_changed(move |scale| {
        // A click on the trough lands between steps; it is put on the nearest one.
        let snapped = low + ((scale.value() - low) / step).round() * step;
        if (snapped - scale.value()).abs() > step / 1000.0 {
            scale.set_value(snapped);
            return;
        }
        shown_label.set_label(&shown(scale.value()));
        changed(scale.value());
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
    row.connect_apply(move |row| set(&format!("{key} = {:?}", row.text().as_str())));
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
    row.add_suffix(
        &gtk::Label::builder()
            .label(value)
            .css_classes(["dim-label"])
            .selectable(true)
            .build(),
    );
    row
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
    icons.add(&switch_row(config, "音量", "menu_bar_sound"));
    if !batteries().is_empty() {
        icons.add(&switch_row(config, "電池", "menu_bar_battery"));
        icons.add(&switch_row(config, "電池百分比", "menu_bar_battery_percent"));
    }
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
    more.add(&subpage_row("月曆", String::new, page_of_groups(&[untitled(calendar_month_group(config))])));
    more.add(&subpage_row("當天行程", String::new, page_of_groups(&[untitled(calendar_day_group(config))])));
    page.add(&more);
    page
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

/// 桌布: the pictures to pick from, how one fills the desktop, and the desktop's items.
fn wallpaper_page(config: &Config, autostart: &Rc<RefCell<Vec<String>>>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let wall = group("");
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
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .max_children_per_line(8)
        .min_children_per_line(4)
        .column_spacing(6)
        .row_spacing(6)
        .homogeneous(true)
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
        set(&format!("wallpaper = {:?}", path.to_string_lossy()));
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
    // The pictures go under the heading, as a group's rows would come before them.
    flow.set_visible(flow.first_child().is_some());
    wall.add(&flow);
    wall.add(&switch_row(config, "桌面項目", "desktop_icons"));
    page.add(&wall);

    let options = adw::PreferencesGroup::new();
    options.add(&choice_row(
        config,
        "填滿方式",
        "wallpaper_mode",
        &WALLPAPER_MODES,
        &["填滿", "符合", "置中"],
    ));
    let choose = gtk::Button::builder()
        .label("選擇⋯")
        .valign(gtk::Align::Center)
        .build();
    choose.connect_clicked(move |button| {
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
                flow.append(&wallpaper_thumbnail(&path));
                flow.set_visible(true);
                pictures.borrow_mut().push(path.clone());
                pictures.borrow().len() - 1
            });
            if let Some(child) = flow.child_at_index(index as i32) {
                flow.select_child(&child);
            }
            use_wallpaper(&path);
        });
    });
    let other = adw::ActionRow::builder().title("其他圖片").build();
    other.add_suffix(&choose);
    other.set_activatable_widget(Some(&choose));
    options.add(&other);
    page.add(&options);
    page
}

/// A wallpaper as the 桌布 group shows it to be chosen.
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
        // A Pixbuf stays on the thread that read it; its pixels come back.
        let read = gio::spawn_blocking(move || {
            Pixbuf::from_file_at_scale(&path, 320, 200, true).map(|pixbuf| {
                let format = if pixbuf.has_alpha() {
                    gdk::MemoryFormat::R8g8b8a8
                } else {
                    gdk::MemoryFormat::R8g8b8
                };
                (
                    pixbuf.read_pixel_bytes(),
                    pixbuf.width(),
                    pixbuf.height(),
                    pixbuf.rowstride(),
                    format,
                )
            })
        })
        .await;
        match (read, picture.upgrade()) {
            (Ok(Ok((pixels, width, height, stride, format))), Some(picture)) => {
                let texture = gdk::MemoryTexture::new(width, height, format, &pixels, stride as usize);
                picture.set_paintable(Some(&texture));
            }
            (Ok(Err(err)), _) => eprintln!("{err}"),
            _ => {}
        }
    });
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

/// How nekoland-lock looks, and when the screen locks by itself.
const LOCK_STYLES: [&str; 3] = ["quiet", "poster", "dial"];
const LOCK_NOTICES: [&str; 3] = ["hidden", "count", "full"];
const LOCK_WEIGHTS: [&str; 3] = ["light", "regular", "bold"];
/// The clock's colours to pick from, after the style's own: white, and colours that read on
/// ink as the wallpaper's would.
const LOCK_CLOCK_COLOURS: [&str; 6] = ["#f1efea", "#f2b28a", "#9cc4e6", "#a8d8b9", "#ff8f6b", "#c3b1e1"];
const LOCK_CLOCK_SIZES: [&str; 3] = ["small", "medium", "large"];
const LOCK_CLOCK_ALIGNS: [&str; 3] = ["left", "center", "right"];
const LOCK_EFFECTS: [&str; 13] =
    ["lift", "fade", "slide", "expand", "spread", "wave", "warp", "ripple", "mosaic", "wiper", "paint", "glitch", "swirl"];
const LOCK_EFFECT_NAMES: [&str; 13] =
    ["往上掀起", "淡出", "滑開", "放大淡出", "擴散", "波浪", "拉伸", "漣漪", "馬賽克", "雨刷", "刷除", "故障", "漩渦"];
/// The effects that go one way, and those drawn in a colour of their own.
const LOCK_EFFECTS_ONE_WAY: [&str; 5] = ["slide", "wave", "wiper", "paint", "glitch"];
const LOCK_EFFECTS_COLOURED: [&str; 4] = ["spread", "ripple", "paint", "glitch"];
const LOCK_DIRECTIONS: [&str; 4] = ["left", "right", "up", "down"];
const LOCK_EASINGS: [&str; 3] = ["out", "inout", "linear"];
const LOCK_ICONS: [&str; 7] = ["none", "padlock", "shield", "key", "heart", "paw", "picture"];
const LOCK_NUMERALS: [&str; 3] = ["none", "arabic", "roman"];
const LOCK_HANDS: [&str; 3] = ["classic", "thin", "bold"];
const LOCK_SHORTCUT_LAYOUTS: [&str; 3] = ["corners", "row", "column"];
const LOCK_FINGER_EFFECTS: [&str; 3] = ["ripple", "glow", "none"];
/// How long unlocking takes, in milliseconds: quick, as it was, and slow.
const LOCK_DURATIONS: [i64; 3] = [320, 560, 900];
/// Seconds idle while locked before the displays go dark; 0 for as they otherwise do.
const LOCK_SCREEN_OFF: [i64; 4] = [0, 10, 30, 60];
/// The second clock's zones, and the city each is shown as.
const WORLD_CLOCKS: [(&str, &str); 10] = [
    ("Asia/Tokyo", "東京"),
    ("Asia/Seoul", "首爾"),
    ("Asia/Shanghai", "上海"),
    ("Asia/Hong_Kong", "香港"),
    ("Asia/Singapore", "新加坡"),
    ("Europe/London", "倫敦"),
    ("Europe/Paris", "巴黎"),
    ("America/New_York", "紐約"),
    ("America/Los_Angeles", "洛杉磯"),
    ("Australia/Sydney", "雪梨"),
];
/// The weather's places, as latitude and longitude.
const WEATHER_PLACES: [(&str, [f64; 2]); 14] = [
    ("臺北", [25.03, 121.57]),
    ("新北", [25.01, 121.47]),
    ("桃園", [24.99, 121.30]),
    ("新竹", [24.80, 120.97]),
    ("臺中", [24.15, 120.67]),
    ("嘉義", [23.48, 120.45]),
    ("臺南", [22.99, 120.21]),
    ("高雄", [22.63, 120.30]),
    ("花蓮", [23.99, 121.60]),
    ("東京", [35.68, 139.69]),
    ("香港", [22.32, 114.17]),
    ("新加坡", [1.35, 103.82]),
    ("倫敦", [51.51, -0.13]),
    ("紐約", [40.71, -74.01]),
];
/// As many shortcuts as the lock shows in a row.
const LOCK_ROW_SHORTCUTS: usize = 6;
/// The widgets the lock can show, by name, with their titles.
const LOCK_WIDGETS: [(&str, &str); 5] =
    [("battery", "電池"), ("playing", "正在播放"), ("calendar", "今天的行程"), ("world_clock", "世界時鐘"), ("weather", "天氣")];
/// How tall the clock's page draws the lock.
const LOCK_SUBPAGE_PREVIEW: i32 = 180;

/// A button that asks for a picture, and gives its path to `chosen`.
fn picture_button(chosen: impl Fn(std::path::PathBuf) + 'static) -> gtk::Button {
    let button = gtk::Button::builder().label("選擇⋯").valign(gtk::Align::Center).build();
    let chosen = Rc::new(chosen);
    button.connect_clicked(move |button| {
        let images = gtk::FileFilter::new();
        images.add_pixbuf_formats();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&images);
        let dialog = gtk::FileDialog::builder().title("選擇圖片").filters(&filters).build();
        let chosen = chosen.clone();
        let window = button.root().and_downcast::<gtk::Window>();
        dialog.open(window.as_ref(), gio::Cancellable::NONE, move |picked| match picked.map(|file| file.path()) {
            Ok(Some(path)) => chosen(path),
            Ok(None) => eprintln!("lock: the file chosen has no local path"),
            Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
            Err(err) => eprintln!("lock: {err}"),
        });
    });
    button
}

/// A file's name, to show which is chosen.
fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |name| name.to_string_lossy().into_owned())
}

/// `#rrggbb` for a colour picked.
fn hex_of(colour: &gdk::RGBA) -> String {
    let channel = |value: f32| (value * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", channel(colour.red()), channel(colour.green()), channel(colour.blue()))
}

/// A button that takes what a row chose away.
fn clear_button(tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

/// A row choosing the lock's colour for `key`; cleared, the lock's own, which the button
/// shows as `own`.
fn lock_colour_row(config: &Config, title: &str, key: &'static str, own: &'static str, refresh: &Rc<dyn Fn()>) -> adw::ActionRow {
    let current = gdk::RGBA::parse(config.word(key)).ok();
    let own = gdk::RGBA::parse(own).unwrap_or(gdk::RGBA::WHITE);
    let row = adw::ActionRow::builder().title(title).subtitle(if current.is_some() { "" } else { "自動" }).build();
    let button = gtk::ColorDialogButton::builder()
        .dialog(&gtk::ColorDialog::builder().title(title).with_alpha(false).build())
        .valign(gtk::Align::Center)
        .build();
    button.set_rgba(current.as_ref().unwrap_or(&own));
    let clear = clear_button("自動");
    clear.set_visible(current.is_some());
    let refresh = refresh.clone();
    // Put back to the lock's own when cleared, which is no colour chosen.
    let resetting = Rc::new(Cell::new(false));
    button.connect_rgba_notify(glib::clone!(
        #[weak]
        row,
        #[weak]
        clear,
        #[strong]
        refresh,
        #[strong]
        resetting,
        move |button| {
            if resetting.get() {
                return;
            }
            set(&format!("{key} = {:?}", hex_of(&button.rgba())));
            row.set_subtitle("");
            clear.set_visible(true);
            refresh();
        }
    ));
    clear.connect_clicked(glib::clone!(
        #[weak]
        row,
        #[weak]
        button,
        move |clear| {
            unset(key);
            resetting.set(true);
            button.set_rgba(&own);
            resetting.set(false);
            row.set_subtitle("自動");
            clear.set_visible(false);
            refresh();
        }
    ));
    row.add_suffix(&clear);
    row.add_suffix(&button);
    row
}

/// A row choosing a picture of the lock's for `key`, which shows its name.
fn lock_picture_row(config: &Config, title: &str, key: &'static str, refresh: &Rc<dyn Fn()>) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    let clear = clear_button("移除");
    let show = glib::clone!(
        #[weak]
        row,
        #[weak]
        clear,
        move |path: &str| {
            row.set_subtitle(&if path.is_empty() { "無".to_string() } else { file_name(path) });
            clear.set_visible(!path.is_empty());
        }
    );
    show(config.word(key));
    let refresh = refresh.clone();
    clear.connect_clicked(glib::clone!(
        #[strong]
        show,
        #[strong]
        refresh,
        move |_| {
            unset(key);
            show("");
            refresh();
        }
    ));
    row.add_suffix(&clear);
    row.add_suffix(&picture_button(move |path| {
        let path = path.to_string_lossy().into_owned();
        show(&path);
        set(&format!("{key} = {}", toml::Value::String(path)));
        refresh();
    }));
    row
}

/// A row putting what `key` is for where the person wants it, its middle as shares of the
/// screen; switched off, where the style puts it.
fn lock_place_row(config: &Config, title: &str, key: &'static str, default: [f64; 2], refresh: &Rc<dyn Fn()>) -> adw::ExpanderRow {
    let given: Option<Vec<f64>> = config
        .0
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|at| at.iter().filter_map(|n| n.as_float().or_else(|| n.as_integer().map(|n| n as f64))).collect())
        .filter(|at: &Vec<f64>| at.len() == 2);
    let row = adw::ExpanderRow::builder().title(title).show_enable_switch(true).enable_expansion(given.is_some()).build();
    let at = Rc::new(RefCell::new(given.unwrap_or_else(|| default.to_vec())));
    let refresh = refresh.clone();
    let write = Rc::new(glib::clone!(
        #[strong]
        at,
        #[strong]
        refresh,
        move || {
            set(&format!("{key} = {:?}", at.borrow()));
            refresh();
        }
    ));
    row.connect_enable_expansion_notify(glib::clone!(
        #[strong]
        write,
        move |row| {
            if row.enables_expansion() {
                write();
            } else {
                unset(key);
                refresh();
            }
        }
    ));
    for (axis, title) in [(0, "水平位置"), (1, "垂直位置")] {
        let share = at.borrow()[axis];
        row.add_row(&scale_row(title, share, (0.0, 1.0, 0.01), percent, glib::clone!(
            #[strong]
            at,
            #[strong]
            write,
            move |share| {
                at.borrow_mut()[axis] = share;
                write();
            }
        )));
    }
    row
}

/// The pictures placed on the lock, as LockStar's stickers: each file, where its middle
/// stands as shares of the screen, and how large it is in points.
struct Stickers {
    list: RefCell<Vec<(String, [f64; 2], f64)>>,
    rows: RefCell<Vec<gtk::Widget>>,
    /// Whether the one sticker Settings first kept is still written apart, to be folded in.
    apart: Cell<bool>,
    refresh: Rc<dyn Fn()>,
}

impl Stickers {
    fn read(config: &Config, refresh: &Rc<dyn Fn()>) -> Rc<Self> {
        let pair = |value: Option<&toml::Value>, default: [f64; 2]| {
            let at: Vec<f64> = value
                .and_then(toml::Value::as_array)
                .map(|at| at.iter().filter_map(|n| n.as_float().or_else(|| n.as_integer().map(|n| n as f64))).collect())
                .unwrap_or_default();
            if let [x, y] = at[..] { [x, y] } else { default }
        };
        let size = |value: Option<&toml::Value>| value.and_then(toml::Value::as_integer).map_or(160.0, |n| n as f64);
        let mut list = Vec::new();
        let first = config.word("lock_sticker");
        if !first.is_empty() {
            list.push((first.to_string(), pair(config.0.get("lock_sticker_at"), [0.78, 0.3]), size(config.0.get("lock_sticker_size"))));
        }
        for entry in config.0.get("lock_stickers").and_then(toml::Value::as_array).into_iter().flatten() {
            if let Some(file) = entry.get("file").and_then(toml::Value::as_str) {
                list.push((file.to_string(), pair(entry.get("at"), [0.78, 0.3]), size(entry.get("size"))));
            }
        }
        Rc::new(Self { list: RefCell::new(list), rows: RefCell::default(), apart: Cell::new(!first.is_empty()), refresh: refresh.clone() })
    }

    fn write(&self) {
        if self.apart.replace(false) {
            for key in ["lock_sticker", "lock_sticker_at", "lock_sticker_size"] {
                unset(key);
            }
        }
        let list = self.list.borrow();
        if list.is_empty() {
            unset("lock_stickers");
        } else {
            let value = toml::Value::Array(
                list.iter()
                    .map(|(file, [x, y], size)| {
                        let mut table = toml::Table::new();
                        table.insert("file".into(), toml::Value::String(file.clone()));
                        table.insert("at".into(), toml::Value::Array(vec![toml::Value::Float(*x), toml::Value::Float(*y)]));
                        table.insert("size".into(), toml::Value::Integer(*size as i64));
                        toml::Value::Table(table)
                    })
                    .collect(),
            );
            set(&format!("lock_stickers = {value}"));
        }
        (self.refresh)();
    }
}

/// Lists the stickers in `group` again, each with where it stands and how large, and a row
/// to add one.
fn fill_stickers(group: &adw::PreferencesGroup, stickers: &Rc<Stickers>) {
    for row in stickers.rows.take() {
        group.remove(&row);
    }
    let mut shown: Vec<gtk::Widget> = Vec::new();
    for (i, (file, at, size)) in stickers.list.borrow().iter().enumerate() {
        let row = adw::ExpanderRow::builder().title(file_name(file)).build();
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("移除")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        remove.connect_clicked(glib::clone!(
            #[weak]
            group,
            #[strong]
            stickers,
            move |_| {
                stickers.list.borrow_mut().remove(i);
                stickers.write();
                fill_stickers(&group, &stickers);
            }
        ));
        row.add_suffix(&remove);
        for (axis, title) in [(0, "水平位置"), (1, "垂直位置")] {
            row.add_row(&scale_row(title, at[axis], (0.0, 1.0, 0.01), percent, glib::clone!(
                #[strong]
                stickers,
                move |share| {
                    if let Some(sticker) = stickers.list.borrow_mut().get_mut(i) {
                        sticker.1[axis] = share;
                    }
                    stickers.write();
                }
            )));
        }
        row.add_row(&scale_row("大小", *size, (64.0, 480.0, 8.0), |points| format!("{points:.0} pt"), glib::clone!(
            #[strong]
            stickers,
            move |points| {
                if let Some(sticker) = stickers.list.borrow_mut().get_mut(i) {
                    sticker.2 = points;
                }
                stickers.write();
            }
        )));
        group.add(&row);
        shown.push(row.upcast());
    }
    let add = adw::ActionRow::builder().title("新增貼圖").build();
    add.add_suffix(&picture_button(glib::clone!(
        #[weak]
        group,
        #[strong]
        stickers,
        move |path| {
            stickers.list.borrow_mut().push((path.to_string_lossy().into_owned(), [0.5, 0.5], 160.0));
            stickers.write();
            fill_stickers(&group, &stickers);
        }
    )));
    group.add(&add);
    shown.push(add.upcast());
    *stickers.rows.borrow_mut() = shown;
}
/// How large the lock is drawn for its preview: the first display's points, as the lock
/// lays itself out by them.
fn lock_preview_size() -> (i32, i32) {
    gdk::Display::default()
        .and_then(|display| display.monitors().item(0))
        .and_then(|monitor| monitor.downcast::<gdk::Monitor>().ok())
        .map(|monitor| monitor.geometry())
        .map_or((1440, 900), |area| (area.width(), area.height()))
}

/// The lock as it looks now, or as `style` would look picked, drawn by nekoland-lock into a
/// picture, and drawn again with `refresh` once a setting changed.
fn lock_preview(style: Option<&'static str>, height: i32) -> (gtk::Picture, Rc<dyn Fn()>) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .height_request(height)
        .build();
    let name = style.map_or_else(|| "lock-preview.png".to_string(), |style| format!("lock-preview-{style}.png"));
    let file = glib::user_cache_dir().join("nekoland").join(name);
    if let Some(dir) = file.parent() {
        if let Err(err) = std::fs::create_dir_all(dir) {
            eprintln!("{}: {err}", dir.display());
        }
    }
    let refresh = Rc::new(glib::clone!(
        #[weak]
        picture,
        move || {
            let program = nekoland_program("nekoland-lock");
            let (width, height) = lock_preview_size();
            let (width, height) = (width.to_string(), height.to_string());
            let mut argv = vec![program.as_os_str(), "--preview".as_ref(), file.as_os_str(), width.as_ref(), height.as_ref()];
            argv.extend(style.map(std::ffi::OsStr::new));
            let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
                Ok(child) => child,
                Err(err) => return eprintln!("nekoland-lock --preview: {err}"),
            };
            let file = file.clone();
            child.wait_check_async(gio::Cancellable::NONE, glib::clone!(
                #[weak]
                picture,
                move |result| match result {
                    // Read anew: the same file name would show what was drawn before.
                    Ok(()) => match gdk::Texture::from_filename(&file) {
                        Ok(texture) => picture.set_paintable(Some(&texture)),
                        Err(err) => eprintln!("{}: {err}", file.display()),
                    },
                    Err(err) => eprintln!("nekoland-lock --preview: {err}"),
                }
            ));
        }
    ));
    refresh();
    (picture, refresh)
}

/// When the screen locks and how the lock looks, as the person makes it their own.
fn lock_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let (preview, refresh_preview) = lock_preview(None, 280);
    preview.add_css_class("card");
    let shown = group("");
    shown.add(&preview);
    page.add(&shown);
    // What is set less often, a page each one level down; the clock's has a preview of its own.
    let clock_page = adw::PreferencesPage::new();
    let (clock_preview, refresh_clock) = lock_preview(None, LOCK_SUBPAGE_PREVIEW);
    clock_preview.add_css_class("card");
    let clock_shown = group("");
    clock_shown.add(&clock_preview);
    clock_page.add(&clock_shown);
    let widgets_page = adw::PreferencesPage::new();
    let locked_page = adw::PreferencesPage::new();
    let always_page = adw::PreferencesPage::new();
    let background_page = adw::PreferencesPage::new();
    let unlock_page = adw::PreferencesPage::new();

    // Each style, drawn small as it would look picked: the way to pick one.
    let styles = gtk::Box::builder().spacing(12).homogeneous(true).build();
    let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
    let mut refreshes = vec![refresh_preview, refresh_clock];
    for (name, label) in LOCK_STYLES.into_iter().zip(["靜", "海報", "錶面"]) {
        let (thumbnail, refresh_thumbnail) = lock_preview(Some(name), 110);
        refreshes.push(refresh_thumbnail);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        content.append(&thumbnail);
        content.append(&gtk::Label::new(Some(label)));
        let button = gtk::ToggleButton::builder().child(&content).css_classes(["flat"]).build();
        if let Some(first) = buttons.first() {
            button.set_group(Some(first));
        }
        styles.append(&button);
        buttons.push(button);
    }
    // One change can write several settings at once, as a style does; they are drawn once,
    // after the last, so no two drawings write the same file.
    let refreshes = Rc::new(refreshes);
    let waiting: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(source) = waiting.take() {
            source.remove();
        }
        let (refreshes, waiting_then) = (refreshes.clone(), waiting.clone());
        let source = glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            waiting_then.take();
            refreshes.iter().for_each(|refresh| refresh());
        });
        *waiting.borrow_mut() = Some(source);
    });

    let when = group("");
    when.add(&idle_row(config, "閒置後鎖定", "lock_after"));
    when.add(&idle_row(config, "閒置後關閉螢幕", "screen_off_after"));
    let locked_off = config.0.get("lock_screen_off_after").and_then(toml::Value::as_integer).unwrap_or(0);
    let screen_off = adw::ComboRow::builder()
        .title("鎖定時關閉螢幕")
        .model(&gtk::StringList::new(&["與平時相同", "10 秒", "30 秒", "1 分鐘"]))
        .selected(LOCK_SCREEN_OFF.iter().position(|seconds| *seconds == locked_off).unwrap_or(0) as u32)
        .build();
    screen_off.connect_selected_notify(|row| {
        if let Some(seconds) = LOCK_SCREEN_OFF.get(row.selected() as usize) {
            set(&format!("lock_screen_off_after = {seconds}"));
        }
    });
    when.add(&screen_off);

    // Each change is drawn into the preview once it is written.
    let changed = |line: String, refresh: &Rc<dyn Fn()>| {
        set(&line);
        refresh();
    };

    // A group shows its rows before anything else, so the pictures get one of their own.
    let picker = group("");
    picker.add(&styles);
    page.add(&picker);
    page.add(&when);
    let look = group("");
    let selected = config.choice(&LOCK_STYLES, "lock_style");
    let weight = adw::ComboRow::builder()
        .title("時鐘粗細")
        .model(&gtk::StringList::new(&["細", "標準", "粗"]))
        .build();
    // Unset, the clock is as thick as its style sets it: thin, but the poster's bold.
    let style_weight = |style: u32| if style == 1 { 2 } else { 0 };
    let chosen_weight = LOCK_WEIGHTS.iter().position(|name| *name == config.word("lock_clock_weight"));
    weight.set_selected(chosen_weight.map_or(style_weight(selected), |at| at as u32));
    // The dial has hands, not figures.
    weight.set_visible(selected != 2);
    let blur = adw::SwitchRow::builder()
        .title("模糊桌布")
        .active(config.0.get("lock_blur").and_then(toml::Value::as_bool).unwrap_or(true))
        .build();
    // A choice or a switch of the lock's, drawn into the preview once written.
    let lock_choice = |title: &str, key: &'static str, names: &'static [&'static str], labels: &[&str], default: u32| {
        let current = names.iter().position(|name| *name == config.word(key)).map_or(default, |at| at as u32);
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(labels))
            .selected(current)
            .build();
        row.connect_selected_notify(glib::clone!(
            #[strong]
            refresh,
            move |row| {
                if let Some(name) = names.get(row.selected() as usize) {
                    changed(format!("{key} = {name:?}"), &refresh);
                }
            }
        ));
        row
    };
    let lock_switch = |title: &str, key: &'static str, default: bool| {
        let row = adw::SwitchRow::builder()
            .title(title)
            .active(config.0.get(key).and_then(toml::Value::as_bool).unwrap_or(default))
            .build();
        row.connect_active_notify(glib::clone!(
            #[strong]
            refresh,
            move |row| changed(format!("{key} = {}", row.is_active()), &refresh)
        ));
        row
    };
    let font_button = gtk::FontDialogButton::builder()
        .dialog(&gtk::FontDialog::builder().title("時鐘字型").build())
        .level(gtk::FontLevel::Family)
        .use_font(true)
        .valign(gtk::Align::Center)
        .build();
    let family = Some(config.word("lock_clock_font"))
        .filter(|family| !family.is_empty())
        .or(Some(config.word("font")).filter(|family| !family.is_empty()))
        .unwrap_or("Ubuntu Sans");
    font_button.set_font_desc(&gtk::pango::FontDescription::from_string(family));
    font_button.connect_font_desc_notify(glib::clone!(
        #[strong]
        refresh,
        move |button| {
            if let Some(family) = button.font_desc().and_then(|font| font.family()) {
                changed(format!("lock_clock_font = {}", toml::Value::String(family.to_string())), &refresh);
            }
        }
    ));
    let font = adw::ActionRow::builder().title("時鐘字型").build();
    font.add_suffix(&font_button);
    let size = lock_choice("時鐘大小", "lock_clock_size", &LOCK_CLOCK_SIZES, &["小", "中", "大"], 1);
    let align = lock_choice("時鐘位置", "lock_clock_align", &LOCK_CLOCK_ALIGNS, &["靠左", "置中", "靠右"], 1);
    // The poster's time is always on the left.
    align.set_visible(selected != 1);
    let seconds = lock_switch("顯示秒數", "lock_clock_seconds", false);
    let date = lock_switch("顯示日期", "lock_show_date", true);
    // Words and a picture laid on the clock, and the dial's face, as ClockFace's.
    let label = adw::EntryRow::builder()
        .title("時鐘文字")
        .text(config.word("lock_clock_text"))
        .show_apply_button(true)
        .build();
    label.connect_apply(glib::clone!(
        #[strong]
        refresh,
        move |row| {
            let words = row.text().replace(['\n', '\r'], " ");
            if words.trim().is_empty() {
                unset("lock_clock_text");
                refresh();
            } else {
                changed(format!("lock_clock_text = {}", toml::Value::String(words.trim().to_string())), &refresh);
            }
        }
    ));
    let badge = lock_picture_row(config, "時鐘圖片", "lock_clock_image", &refresh);
    let numerals = lock_choice("錶面數字", "lock_dial_numerals", &LOCK_NUMERALS, &["無", "阿拉伯數字", "羅馬數字"], 0);
    let hands = lock_choice("指針", "lock_dial_hands", &LOCK_HANDS, &["標準", "細", "粗"], 0);
    numerals.set_visible(selected == 2);
    hands.set_visible(selected == 2);

    buttons[selected as usize].set_active(true);
    for (at, button) in (0..).zip(&buttons) {
        button.connect_toggled(glib::clone!(
            #[weak]
            weight,
            #[weak]
            align,
            #[weak]
            blur,
            #[weak]
            numerals,
            #[weak]
            hands,
            #[strong]
            refresh,
            move |button| {
                if !button.is_active() {
                    return;
                }
                let name = LOCK_STYLES[at as usize];
                set(&format!("lock_style = {name:?}"));
                // A style comes with its own clock and wallpaper, as One UI's do; changed
                // after, they stay as changed.
                weight.set_selected(style_weight(at));
                weight.set_visible(at != 2);
                align.set_visible(at != 1);
                numerals.set_visible(at == 2);
                hands.set_visible(at == 2);
                blur.set_active(name != "poster");
                refresh();
            }
        ));
    }
    weight.connect_selected_notify(glib::clone!(
        #[strong]
        refresh,
        move |row| {
            if let Some(name) = LOCK_WEIGHTS.get(row.selected() as usize) {
                changed(format!("lock_clock_weight = {name:?}"), &refresh);
            }
        }
    ));

    let colours = adw::ActionRow::builder().title("時鐘顏色").build();
    let css = gtk::CssProvider::new();
    css.load_from_string(
        &(LOCK_CLOCK_COLOURS
            .iter()
            .enumerate()
            .map(|(i, colour)| format!(".nk-clock-{i} {{ background: {colour}; min-width: 24px; min-height: 24px; padding: 0; }}"))
            .collect::<String>()
            + ".nk-clock-rainbow { background: linear-gradient(90deg, #f2a8a8, #f2d78a, #a8d8b9, #9cc4e6, #c3b1e1); \
               min-width: 24px; min-height: 24px; padding: 0; }"),
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let current = config.word("lock_clock_color").to_string();
    let automatic = gtk::ToggleButton::builder()
        .label("自動")
        .valign(gtk::Align::Center)
        .tooltip_text("取自桌布")
        .active(current.is_empty())
        .build();
    let mut toggles = vec![automatic.clone()];
    automatic.connect_toggled(glib::clone!(
        #[strong]
        refresh,
        move |button| {
            if button.is_active() {
                unset("lock_clock_color");
                refresh();
            }
        }
    ));
    let swatches = gtk::Box::builder().spacing(8).valign(gtk::Align::Center).build();
    swatches.append(&automatic);
    for (i, colour) in LOCK_CLOCK_COLOURS.iter().enumerate() {
        let swatch = gtk::ToggleButton::builder()
            .group(&automatic)
            .active(current == *colour)
            .valign(gtk::Align::Center)
            .css_classes(["circular", &format!("nk-clock-{i}")])
            .tooltip_text(*colour)
            .build();
        swatch.connect_toggled(glib::clone!(
            #[strong]
            refresh,
            move |button| {
                if button.is_active() {
                    changed(format!("lock_clock_color = {colour:?}"), &refresh);
                }
            }
        ));
        swatches.append(&swatch);
        toggles.push(swatch);
    }
    // Through the colours, as ClockFace's rainbow.
    let rainbow = gtk::ToggleButton::builder()
        .group(&automatic)
        .active(current == "rainbow")
        .valign(gtk::Align::Center)
        .css_classes(["circular", "nk-clock-rainbow"])
        .tooltip_text("彩虹")
        .build();
    rainbow.connect_toggled(glib::clone!(
        #[strong]
        refresh,
        move |button| {
            if button.is_active() {
                changed("lock_clock_color = \"rainbow\"".to_string(), &refresh);
            }
        }
    ));
    swatches.append(&rainbow);
    toggles.push(rainbow);
    // Any other colour, as One UI's palette has it.
    let custom = gtk::Button::builder()
        .icon_name("color-select-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text("自訂")
        .css_classes(["circular", "flat"])
        .build();
    let picked = Rc::new(RefCell::new(
        Some(current.as_str())
            .filter(|colour| !colour.is_empty() && *colour != "rainbow" && !LOCK_CLOCK_COLOURS.contains(colour))
            .and_then(|colour| gdk::RGBA::parse(colour).ok()),
    ));
    custom.connect_clicked(glib::clone!(
        #[strong]
        refresh,
        move |button| {
            let dialog = gtk::ColorDialog::builder().title("時鐘顏色").with_alpha(false).build();
            let window = button.root().and_downcast::<gtk::Window>();
            let (toggles, picked, refresh) = (toggles.clone(), picked.clone(), refresh.clone());
            let initial = *picked.borrow();
            dialog.choose_rgba(window.as_ref(), initial.as_ref(), gio::Cancellable::NONE, move |chosen| {
                let colour = match chosen {
                    Ok(colour) => colour,
                    Err(err) if err.matches(gtk::DialogError::Dismissed) => return,
                    Err(err) => return eprintln!("lock clock colour: {err}"),
                };
                let channel = |value: f32| (value * 255.0).round() as u8;
                let hex = format!("#{:02x}{:02x}{:02x}", channel(colour.red()), channel(colour.green()), channel(colour.blue()));
                *picked.borrow_mut() = Some(colour);
                // None of the others is the clock's colour now.
                toggles.iter().for_each(|toggle| toggle.set_active(false));
                changed(format!("lock_clock_color = {hex:?}"), &refresh);
            });
        }
    ));
    swatches.append(&custom);
    colours.add_suffix(&swatches);
    look.add(&colours);
    look.add(&weight);
    look.add(&font);
    look.add(&size);
    look.add(&align);
    look.add(&seconds);
    look.add(&date);
    look.add(&numerals);
    look.add(&hands);
    look.add(&label);
    look.add(&badge);
    clock_page.add(&look);

    // Where the clock, the notices and the message stand, as LockStar lets them be moved.
    let places = group("自訂位置");
    places.add(&lock_place_row(config, "時鐘", "lock_clock_at", [0.5, 0.3], &refresh));
    places.add(&lock_place_row(config, "通知", "lock_notices_at", [0.5, 0.55], &refresh));
    places.add(&lock_place_row(config, "訊息", "lock_message_at", [0.5, 0.08], &refresh));
    clock_page.add(&places);

    let widgets = group("小工具");
    let listed = lock_widgets(config);
    let rows: Rc<Vec<(adw::SwitchRow, &str)>> = Rc::new(
        LOCK_WIDGETS
            .iter()
            .map(|&(name, title)| {
                let row = adw::SwitchRow::builder().title(title).active(listed.iter().any(|listed| listed == name)).build();
                (row, name)
            })
            .collect(),
    );
    let zone = adw::ComboRow::builder()
        .title("時鐘城市")
        .model(&gtk::StringList::new(&WORLD_CLOCKS.map(|(_, city)| city)))
        .selected(WORLD_CLOCKS.iter().position(|(zone, _)| *zone == config.word("lock_world_clock")).unwrap_or(0) as u32)
        .build();
    rows[3].0.bind_property("active", &zone, "visible").sync_create().build();
    let set_zone = |row: &adw::ComboRow| {
        if let Some((zone, city)) = WORLD_CLOCKS.get(row.selected() as usize) {
            set(&format!("lock_world_clock = {zone:?}"));
            set(&format!("lock_world_clock_name = {city:?}"));
        }
    };
    zone.connect_selected_notify(glib::clone!(
        #[strong]
        refresh,
        move |row| {
            set_zone(row);
            refresh();
        }
    ));
    let kept_place = config.word("lock_weather_name");
    let place = adw::ComboRow::builder()
        .title("天氣城市")
        .model(&gtk::StringList::new(&WEATHER_PLACES.map(|(city, _)| city)))
        .selected(WEATHER_PLACES.iter().position(|(city, _)| *city == kept_place).unwrap_or(0) as u32)
        .build();
    rows[4].0.bind_property("active", &place, "visible").sync_create().build();
    let set_place = |row: &adw::ComboRow| {
        if let Some((city, at)) = WEATHER_PLACES.get(row.selected() as usize) {
            set(&format!("lock_weather_at = {at:?}"));
            set(&format!("lock_weather_name = {city:?}"));
        }
    };
    place.connect_selected_notify(glib::clone!(
        #[strong]
        refresh,
        move |row| {
            set_place(row);
            refresh();
        }
    ));
    for (row, _) in rows.iter() {
        row.connect_active_notify(glib::clone!(
            #[strong]
            rows,
            #[weak]
            zone,
            #[weak]
            place,
            #[strong]
            refresh,
            move |_| {
                let names: Vec<&str> = rows.iter().filter(|(row, _)| row.is_active()).map(|(_, name)| *name).collect();
                if names.contains(&"world_clock") {
                    set_zone(&zone);
                }
                if names.contains(&"weather") {
                    set_place(&place);
                }
                changed(format!("lock_widgets = {names:?}"), &refresh);
            }
        ));
        widgets.add(row);
    }
    widgets.add(&zone);
    widgets.add(&place);
    widgets_page.add(&widgets);

    // Two apps at the lock's foot, left and right, as One UI has them, or a row of them, as
    // Good Lock's LockStar has.
    let shortcuts = group("捷徑");
    let mut apps: Vec<gio::AppInfo> = gio::AppInfo::all().into_iter().filter(gio::AppInfo::should_show).collect();
    apps.sort_by_key(|app| app.display_name().to_lowercase());
    let ids: Rc<Vec<String>> = Rc::new(
        apps.iter()
            .map(|app| app.id().map(|id| id.trim_end_matches(".desktop").to_string()).unwrap_or_default())
            .collect(),
    );
    let chosen: Vec<String> = config
        .0
        .get("lock_shortcuts")
        .and_then(toml::Value::as_array)
        .map(|listed| listed.iter().filter_map(toml::Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    let mut names = vec!["無".to_string()];
    names.extend(apps.iter().map(|app| app.display_name().to_string()));
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let layout = adw::ComboRow::builder()
        .title("排列")
        .model(&gtk::StringList::new(&["左右兩側", "一列", "一欄"]))
        .selected(LOCK_SHORTCUT_LAYOUTS.iter().position(|name| *name == config.word("lock_shortcuts_layout")).unwrap_or(0) as u32)
        .build();
    shortcuts.add(&layout);
    let sides: Vec<adw::ComboRow> = (0..LOCK_ROW_SHORTCUTS)
        .map(|at| {
            let current = chosen.get(at).and_then(|app| ids.iter().position(|id| id == app));
            adw::ComboRow::builder()
                .model(&gtk::StringList::new(&names))
                .selected(current.map_or(0, |at| at as u32 + 1))
                .build()
        })
        .collect();
    let sides = Rc::new(sides);
    // The corners' two are named by their side, a row's by their place in it.
    let name_sides = glib::clone!(
        #[strong]
        sides,
        move |row: bool| {
            for (at, side) in sides.iter().enumerate() {
                side.set_visible(row || at < 2);
                side.set_title(&if row { format!("第 {} 個", at + 1) } else if at == 0 { "左側".into() } else { "右側".into() });
            }
        }
    );
    name_sides(layout.selected() != 0);
    let write_sides = glib::clone!(
        #[strong]
        sides,
        #[strong]
        ids,
        #[weak]
        layout,
        #[strong]
        refresh,
        move || {
            // 無 is the first; the apps follow it.
            let mut picked: Vec<String> = sides
                .iter()
                .take(if layout.selected() != 0 { LOCK_ROW_SHORTCUTS } else { 2 })
                .map(|row| row.selected().checked_sub(1).and_then(|at| ids.get(at as usize)).cloned().unwrap_or_default())
                .collect();
            while picked.last().is_some_and(String::is_empty) {
                picked.pop();
            }
            if picked.is_empty() {
                unset("lock_shortcuts");
                refresh();
            } else {
                changed(format!("lock_shortcuts = {picked:?}"), &refresh);
            }
        }
    );
    let write_sides = Rc::new(write_sides);
    layout.connect_selected_notify(glib::clone!(
        #[strong]
        write_sides,
        move |layout| {
            name_sides(layout.selected() != 0);
            let name = LOCK_SHORTCUT_LAYOUTS[layout.selected() as usize % LOCK_SHORTCUT_LAYOUTS.len()];
            set(&format!("lock_shortcuts_layout = {name:?}"));
            write_sides();
        }
    ));
    for row in sides.iter() {
        row.connect_selected_notify(glib::clone!(
            #[strong]
            write_sides,
            move |_| write_sides()
        ));
        shortcuts.add(row);
    }
    let unlocked = adw::SwitchRow::builder()
        .title("不需密碼即可開啟")
        .subtitle("鎖定時任何人都能使用這些 App")
        .active(config.flag("lock_shortcuts_unlocked"))
        .build();
    unlocked.connect_active_notify(|row| set(&format!("lock_shortcuts_unlocked = {}", row.is_active())));
    shortcuts.add(&unlocked);
    widgets_page.add(&shortcuts);

    let locked = group("鎖定時");
    let notices = adw::ComboRow::builder()
        .title("通知")
        .model(&gtk::StringList::new(&["不顯示", "只顯示數量", "顯示內容"]))
        .selected(LOCK_NOTICES.iter().position(|name| *name == config.word("lock_notices")).unwrap_or(1) as u32)
        .build();
    notices.connect_selected_notify(glib::clone!(
        #[strong]
        refresh,
        move |row| {
            if let Some(name) = LOCK_NOTICES.get(row.selected() as usize) {
                changed(format!("lock_notices = {name:?}"), &refresh);
            }
        }
    ));
    let name = adw::SwitchRow::builder()
        .title("顯示使用者名稱與照片")
        .active(config.0.get("lock_show_name").and_then(toml::Value::as_bool).unwrap_or(true))
        .build();
    name.connect_active_notify(glib::clone!(
        #[strong]
        refresh,
        move |row| changed(format!("lock_show_name = {}", row.is_active()), &refresh)
    ));
    let message = adw::EntryRow::builder()
        .title("鎖定時顯示的訊息")
        .text(config.word("lock_message"))
        .show_apply_button(true)
        .build();
    message.connect_apply(glib::clone!(
        #[strong]
        refresh,
        move |row| {
            let words = row.text().replace(['\n', '\r'], " ");
            if words.trim().is_empty() {
                unset("lock_message");
                refresh();
            } else {
                changed(format!("lock_message = {}", toml::Value::String(words.trim().to_string())), &refresh);
            }
        }
    ));
    let power = adw::SwitchRow::builder()
        .title("顯示睡眠、重新開機和關機按鈕")
        .active(config.0.get("lock_power_buttons").and_then(toml::Value::as_bool).unwrap_or(true))
        .build();
    power.connect_active_notify(glib::clone!(
        #[strong]
        refresh,
        move |row| changed(format!("lock_power_buttons = {}", row.is_active()), &refresh)
    ));
    // The sign at the top, as LockStar's lock icons; Settings' first switch showed the padlock.
    let padlock = u32::from(config.0.get("lock_show_lock_icon").and_then(toml::Value::as_bool).unwrap_or(false));
    let icon = lock_choice("鎖定圖示", "lock_lock_icon", &LOCK_ICONS, &["不顯示", "鎖頭", "盾牌", "鑰匙", "愛心", "貓掌", "圖片"], padlock);
    let icon_picture = lock_picture_row(config, "圖示圖片", "lock_lock_icon_file", &refresh);
    let icon_colour = lock_colour_row(config, "圖示顏色", "lock_lock_icon_color", LOCK_CLOCK_COLOURS[0], &refresh);
    let show_icon = glib::clone!(
        #[weak]
        icon_picture,
        #[weak]
        icon_colour,
        move |at: u32| {
            icon_picture.set_visible(LOCK_ICONS[at as usize] == "picture");
            icon_colour.set_visible(!matches!(LOCK_ICONS[at as usize], "none" | "picture"));
        }
    );
    show_icon(icon.selected());
    icon.connect_selected_notify(move |row| show_icon(row.selected()));
    let message_size = scale_row(
        "訊息大小",
        config.number("lock_message_size", 13.0),
        (10.0, 40.0, 1.0),
        |points| format!("{points:.0} pt"),
        glib::clone!(
            #[strong]
            refresh,
            move |points| changed(format!("lock_message_size = {}", points as i64), &refresh)
        ),
    );
    let message_colour = lock_colour_row(config, "訊息顏色", "lock_message_color", LOCK_CLOCK_COLOURS[0], &refresh);
    let message_opacity = scale_row(
        "訊息不透明度",
        config.number("lock_message_opacity", 100.0) / 100.0,
        (0.1, 1.0, 0.05),
        percent,
        glib::clone!(
            #[strong]
            refresh,
            move |share| changed(format!("lock_message_opacity = {}", (share * 100.0).round() as i64), &refresh)
        ),
    );
    // What the always-on display shows besides the time, as LockStar's AOD options.
    let always_on = lock_switch("全天候顯示", "lock_always_on", false);
    let always_widgets = lock_switch("全天候顯示小工具", "lock_always_on_widgets", false);
    let always_stickers = lock_switch("全天候顯示貼圖", "lock_always_on_stickers", false);
    let always_bright = scale_row(
        "全天候顯示亮度",
        config.number("lock_always_on_brightness", 100.0) / 100.0,
        (0.2, 1.0, 0.05),
        percent,
        |share| set(&format!("lock_always_on_brightness = {}", (share * 100.0).round() as i64)),
    );
    for row in [always_widgets.upcast_ref::<gtk::Widget>(), always_stickers.upcast_ref(), always_bright.upcast_ref()] {
        always_on.bind_property("active", row, "sensitive").sync_create().build();
    }
    locked.add(&notices);
    locked.add(&name);
    locked.add(&icon);
    locked.add(&icon_picture);
    locked.add(&icon_colour);
    locked.add(&power);
    locked.add(&message);
    locked.add(&message_size);
    locked.add(&message_colour);
    locked.add(&message_opacity);
    locked_page.add(&locked);
    let always = group("");
    always.add(&always_on);
    always.add(&always_widgets);
    always.add(&always_stickers);
    always.add(&always_bright);
    always_page.add(&always);
    unlock_page.add(&fingerprint_group(lock_switch("以指紋解鎖", "lock_fingerprint", true)));

    let background = group("背景");
    let wallpaper = adw::SwitchRow::builder()
        .title("顯示桌布")
        .active(config.0.get("lock_wallpaper").and_then(toml::Value::as_bool).unwrap_or(true))
        .build();
    wallpaper.bind_property("active", &blur, "sensitive").sync_create().build();
    for (row, key) in [(&wallpaper, "lock_wallpaper"), (&blur, "lock_blur")] {
        row.connect_active_notify(glib::clone!(
            #[strong]
            refresh,
            move |row| changed(format!("{key} = {}", row.is_active()), &refresh)
        ));
    }
    background.add(&wallpaper);
    background.add(&blur);

    // A picture of the lock's own, rather than the desktop's.
    let own = adw::ActionRow::builder().title("鎖定畫面桌布").build();
    let clear_own = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("與桌面相同")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let show_own = glib::clone!(
        #[weak]
        own,
        #[weak]
        clear_own,
        move |path: &str| {
            own.set_subtitle(&if path.is_empty() { "與桌面相同".to_string() } else { file_name(path) });
            clear_own.set_visible(!path.is_empty());
        }
    );
    show_own(config.word("lock_wallpaper_file"));
    // A folder whose pictures take turns, one each time the screen locks, as Dynamic Lock
    // Screen's; the lock's own picture comes before it, so one clears the other.
    let folder = adw::ActionRow::builder().title("輪播資料夾").build();
    let clear_folder = clear_button("移除");
    let show_folder = glib::clone!(
        #[weak]
        folder,
        #[weak]
        clear_folder,
        move |path: &str| {
            folder.set_subtitle(&if path.is_empty() { "未設定".to_string() } else { file_name(path) });
            clear_folder.set_visible(!path.is_empty());
        }
    );
    show_folder(config.word("lock_wallpaper_folder"));
    let drop_folder = glib::clone!(
        #[strong]
        show_folder,
        #[weak]
        clear_folder,
        move || {
            if clear_folder.is_visible() {
                unset("lock_wallpaper_folder");
                show_folder("");
            }
        }
    );
    clear_own.connect_clicked(glib::clone!(
        #[strong]
        show_own,
        #[strong]
        refresh,
        move |_| {
            unset("lock_wallpaper_file");
            show_own("");
            refresh();
        }
    ));
    own.add_suffix(&clear_own);
    own.add_suffix(&picture_button(glib::clone!(
        #[strong]
        show_own,
        #[strong]
        refresh,
        #[strong]
        drop_folder,
        move |path| {
            let path = path.to_string_lossy().into_owned();
            show_own(&path);
            drop_folder();
            changed(format!("lock_wallpaper_file = {}", toml::Value::String(path)), &refresh);
        }
    )));
    wallpaper.bind_property("active", &own, "sensitive").sync_create().build();
    background.add(&own);
    clear_folder.connect_clicked(glib::clone!(
        #[strong]
        show_folder,
        #[strong]
        refresh,
        move |_| {
            unset("lock_wallpaper_folder");
            show_folder("");
            refresh();
        }
    ));
    let choose_folder = gtk::Button::builder().label("選擇⋯").valign(gtk::Align::Center).build();
    choose_folder.connect_clicked(glib::clone!(
        #[strong]
        show_folder,
        #[strong]
        show_own,
        #[strong]
        refresh,
        move |button| {
            let dialog = gtk::FileDialog::builder().title("選擇資料夾").build();
            let window = button.root().and_downcast::<gtk::Window>();
            let (show_folder, show_own, refresh) = (show_folder.clone(), show_own.clone(), refresh.clone());
            dialog.select_folder(window.as_ref(), gio::Cancellable::NONE, move |picked| match picked.map(|file| file.path()) {
                Ok(Some(path)) => {
                    let path = path.to_string_lossy().into_owned();
                    show_folder(&path);
                    unset("lock_wallpaper_file");
                    show_own("");
                    set(&format!("lock_wallpaper_folder = {}", toml::Value::String(path)));
                    refresh();
                }
                Ok(None) => eprintln!("lock: the folder chosen has no local path"),
                Err(err) if err.matches(gtk::DialogError::Dismissed) => {}
                Err(err) => eprintln!("lock: {err}"),
            });
        }
    ));
    folder.add_suffix(&clear_folder);
    folder.add_suffix(&choose_folder);
    wallpaper.bind_property("active", &folder, "sensitive").sync_create().build();
    background.add(&folder);
    let subject = adw::ActionRow::builder().title("前景主體").build();
    let clear_subject = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text("移除")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let show_subject = glib::clone!(
        #[weak]
        subject,
        #[weak]
        clear_subject,
        move |path: &str| {
            subject.set_subtitle(&if path.is_empty() { "未設定".to_string() } else { file_name(path) });
            clear_subject.set_visible(!path.is_empty());
        }
    );
    show_subject(config.word("lock_wallpaper_subject"));
    clear_subject.connect_clicked(glib::clone!(
        #[strong]
        show_subject,
        #[strong]
        refresh,
        move |_| {
            unset("lock_wallpaper_subject");
            show_subject("");
            refresh();
        }
    ));
    subject.add_suffix(&clear_subject);
    // Cut out of the wallpaper the lock shows by an image service, as One UI does by itself.
    let cut = gtk::Button::builder().label("去背").valign(gtk::Align::Center).build();
    cut.connect_clicked(glib::clone!(
        #[strong]
        show_subject,
        #[weak]
        subject,
        #[strong]
        refresh,
        move |cut| {
            // The lock's own picture as it is now, else the desktop's.
            let now = Config::load();
            let picture = Some(now.word("lock_wallpaper_file"))
                .filter(|path| !path.is_empty())
                .unwrap_or(now.word("wallpaper"))
                .replacen('~', &glib::home_dir().to_string_lossy(), 1);
            if picture.is_empty() {
                return alert::notice(cut, "無法去背", Some("沒有設定桌布。"));
            }
            let (show_subject, refresh, cut, subject) = (show_subject.clone(), refresh.clone(), cut.clone(), subject.clone());
            ask_for_wallpaper(&cut.clone(), false, move |service, _| {
                cut.set_sensitive(false);
                subject.set_subtitle("去背中");
                let (show_subject, refresh, cut, picture) = (show_subject.clone(), refresh.clone(), cut.clone(), picture.clone());
                glib::spawn_future_local(async move {
                    let made = gio::spawn_blocking(move || imagegen::cut_out(&service, std::path::Path::new(&picture)))
                        .await
                        .unwrap_or_else(|_| Err("無法去背".into()));
                    cut.set_sensitive(true);
                    match made {
                        Ok(path) => {
                            let path = path.to_string_lossy().into_owned();
                            show_subject(&path);
                            changed(format!("lock_wallpaper_subject = {}", toml::Value::String(path)), &refresh);
                        }
                        Err(why) => {
                            show_subject(Config::load().word("lock_wallpaper_subject"));
                            alert::notice(&cut, "無法去背", Some(&why));
                        }
                    }
                });
            });
        }
    ));
    subject.add_suffix(&cut);
    subject.add_suffix(&picture_button(glib::clone!(
        #[strong]
        show_subject,
        #[strong]
        refresh,
        move |path| {
            let path = path.to_string_lossy().into_owned();
            show_subject(&path);
            changed(format!("lock_wallpaper_subject = {}", toml::Value::String(path)), &refresh);
        }
    )));
    wallpaper.bind_property("active", &subject, "sensitive").sync_create().build();
    background.add(&subject);
    // A picture an image service makes from a description becomes the lock's own.
    let generated = adw::ActionRow::builder().title("AI 生成桌布").activatable(true).build();
    let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
    generated.add_suffix(&spinner);
    generated.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    generated.connect_activated(glib::clone!(
        #[strong]
        show_own,
        #[strong]
        drop_folder,
        #[strong]
        refresh,
        #[weak]
        spinner,
        move |row| {
            let (show_own, drop_folder, refresh, spinner, shown) =
                (show_own.clone(), drop_folder.clone(), refresh.clone(), spinner.clone(), row.clone());
            ask_for_wallpaper(row, true, move |service, description| {
                shown.set_sensitive(false);
                shown.set_subtitle("生成中");
                spinner.set_visible(true);
                let (show_own, drop_folder, refresh, spinner, shown) =
                    (show_own.clone(), drop_folder.clone(), refresh.clone(), spinner.clone(), shown.clone());
                glib::spawn_future_local(async move {
                    let made = gio::spawn_blocking(move || imagegen::generate(&service, &description))
                        .await
                        .unwrap_or_else(|_| Err("無法生成桌布".into()));
                    shown.set_sensitive(true);
                    shown.set_subtitle("");
                    spinner.set_visible(false);
                    match made {
                        Ok(path) => {
                            let path = path.to_string_lossy().into_owned();
                            show_own(&path);
                            drop_folder();
                            changed(format!("lock_wallpaper_file = {}", toml::Value::String(path)), &refresh);
                        }
                        Err(why) => alert::notice(&shown, "無法生成桌布", Some(&why)),
                    }
                });
            });
        }
    ));
    wallpaper.bind_property("active", &generated, "sensitive").sync_create().build();
    background.add(&generated);
    background.add(&scale_row(
        "調暗",
        config.number("lock_dim", 0.0) / 100.0,
        (0.0, 0.8, 0.05),
        percent,
        glib::clone!(
            #[strong]
            refresh,
            move |share| changed(format!("lock_dim = {}", (share * 100.0).round() as i64), &refresh)
        ),
    ));
    background_page.add(&background);

    // Pictures placed anywhere on the lock, as LockStar's stickers are.
    let sticker = group("貼圖");
    fill_stickers(&sticker, &Stickers::read(config, &refresh));
    background_page.add(&sticker);

    // How the lock goes once the password is given, as LockStar's unlock effects.
    let unlock = group("解鎖");
    let effect = lock_choice("效果", "lock_unlock_effect", &LOCK_EFFECTS, &LOCK_EFFECT_NAMES, 0);
    let direction = lock_choice("方向", "lock_unlock_direction", &LOCK_DIRECTIONS, &["向左", "向右", "向上", "向下"], 1);
    let easing = lock_choice("速度曲線", "lock_unlock_easing", &LOCK_EASINGS, &["漸慢", "慢快慢", "等速"], 0);
    let effect_colour = lock_colour_row(config, "顏色", "lock_unlock_color", LOCK_CLOCK_COLOURS[1], &refresh);
    // Only what the effect has: a way to go, a colour of its own.
    let show_effect = glib::clone!(
        #[weak]
        direction,
        #[weak]
        effect_colour,
        move |at: u32| {
            let name = LOCK_EFFECTS[at as usize % LOCK_EFFECTS.len()];
            direction.set_visible(LOCK_EFFECTS_ONE_WAY.contains(&name));
            effect_colour.set_visible(LOCK_EFFECTS_COLOURED.contains(&name));
        }
    );
    show_effect(effect.selected());
    effect.connect_selected_notify(move |row| show_effect(row.selected()));
    unlock.add(&effect);
    unlock.add(&direction);
    unlock.add(&easing);
    unlock.add(&effect_colour);
    let duration = config.0.get("lock_unlock_duration").and_then(toml::Value::as_integer).unwrap_or(560);
    let speed = adw::ComboRow::builder()
        .title("速度")
        .model(&gtk::StringList::new(&["快", "標準", "慢"]))
        .selected(LOCK_DURATIONS.iter().position(|ms| *ms == duration).unwrap_or(1) as u32)
        .build();
    speed.connect_selected_notify(|row| {
        if let Some(ms) = LOCK_DURATIONS.get(row.selected() as usize) {
            set(&format!("lock_unlock_duration = {ms}"));
        }
    });
    unlock.add(&speed);
    unlock.add(&lock_choice("指紋效果", "lock_fingerprint_effect", &LOCK_FINGER_EFFECTS, &["漣漪", "光暈", "無"], 0));
    unlock_page.add(&unlock);

    let more = group("");
    let clock = subpage_row("時鐘", lock_clock_summary, clock_page);
    more.add(&clock);
    more.add(&subpage_row("小工具與捷徑", lock_widgets_summary, widgets_page));
    more.add(&subpage_row("背景", lock_background_summary, background_page));
    more.add(&subpage_row("鎖定時", lock_notices_summary, locked_page));
    more.add(&subpage_row("全天候顯示", || on_off(Config::load().flag("lock_always_on")), always_page));
    more.add(&subpage_row("解鎖", lock_unlock_summary, unlock_page));
    page.add(&more);
    // The preview's clock leads to the clock's settings too.
    let press = gtk::GestureClick::new();
    press.connect_released(glib::clone!(
        #[weak]
        clock,
        move |_, _, _, _| open_subpage(&clock)
    ));
    preview.add_controller(press);
    preview.set_cursor_from_name(Some("pointer"));
    page
}

/// The widgets the lock shows, by name.
fn lock_widgets(config: &Config) -> Vec<String> {
    config
        .0
        .get("lock_widgets")
        .and_then(toml::Value::as_array)
        .map_or_else(|| vec!["battery".into(), "playing".into()], |listed| {
            listed.iter().filter_map(toml::Value::as_str).map(String::from).collect()
        })
}

fn lock_clock_summary() -> String {
    let config = Config::load();
    let label = |names: &[&str], labels: [&'static str; 3], key: &str| {
        names.iter().position(|name| *name == config.word(key)).map_or(labels[1], |at| labels[at])
    };
    let size = label(&LOCK_CLOCK_SIZES, ["小", "中", "大"], "lock_clock_size");
    let align = label(&LOCK_CLOCK_ALIGNS, ["靠左", "置中", "靠右"], "lock_clock_align");
    [config.word("lock_clock_font"), size, align].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("・")
}

fn lock_widgets_summary() -> String {
    let listed = lock_widgets(&Config::load());
    let titles: Vec<&str> = LOCK_WIDGETS
        .iter()
        .filter(|(name, _)| listed.iter().any(|listed| listed == name))
        .map(|(_, title)| *title)
        .collect();
    if titles.is_empty() { "無".into() } else { titles.join("、") }
}

fn lock_background_summary() -> String {
    let config = Config::load();
    let on = |key: &str| config.0.get(key).and_then(toml::Value::as_bool);
    match (on("lock_wallpaper").unwrap_or(true), on("lock_blur").unwrap_or(false)) {
        (false, _) => "不顯示桌布".into(),
        (true, true) => "桌布・模糊".into(),
        (true, false) => "桌布".into(),
    }
}

fn lock_notices_summary() -> String {
    let config = Config::load();
    let at = LOCK_NOTICES.iter().position(|name| *name == config.word("lock_notices")).unwrap_or(1);
    format!("通知{}", ["不顯示", "只顯示數量", "顯示內容"][at])
}

fn lock_unlock_summary() -> String {
    let config = Config::load();
    let at = LOCK_EFFECTS.iter().position(|name| *name == config.word("lock_unlock_effect")).unwrap_or(0);
    LOCK_EFFECT_NAMES[at].into()
}

fn on_off(on: bool) -> String {
    if on { "開啟" } else { "關閉" }.into()
}

fn windows_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let titlebar = group("標題列");
    for (title, key) in [
        ("按兩下", "action-double-click-titlebar"),
        ("按中鍵", "action-middle-click-titlebar"),
        ("按右鍵", "action-right-click-titlebar"),
    ] {
        let current = output("gsettings", &["get", WM_PREFERENCES, key]).unwrap_or_default();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&["縮放", "最小化", "無"]))
            // What Nekoland cannot do, GNOME's window menu or lowering, does nothing here.
            .selected(TITLEBAR_ACTIONS.iter().position(|action| current.contains(action)).unwrap_or(2) as u32)
            .build();
        row.connect_selected_notify(move |row| {
            if let Some(action) = TITLEBAR_ACTIONS.get(row.selected() as usize) {
                run("gsettings", &["set", WM_PREFERENCES, key, action]);
            }
        });
        titlebar.add(&row);
    }
    page.add(&titlebar);

    let tiling = group("並排");
    tiling.add(&switch_row(config, "拖到螢幕邊緣時並排", "edge_tiling"));
    tiling.add(&scale_row(
        "間距",
        config.number("window_gap", 0.0),
        (0.0, 32.0, 2.0),
        |gap| format!("{gap:.0} px"),
        |gap| set(&format!("window_gap = {gap:.0}")),
    ));
    page.add(&tiling);

    let focus = group("焦點");
    focus.add(&switch_row(config, "焦點跟隨游標", "focus_follows_pointer"));
    page.add(&focus);
    page
}

/// The hover panel: when it shows, where, how it looks and which fields, a group each, as
/// FinderHover's settings have them.
fn hover_place_summary() -> String {
    let config = Config::load();
    if config.flag("hover_follow") {
        return "跟隨指標".into();
    }
    let offset = |key: &str| config.number(key, 15.0);
    format!("偏移 {:.0}・{:.0} px", offset("hover_offset_x"), offset("hover_offset_y"))
}

fn hover_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let general = group("");
    let shown = switch_row(config, "顯示", "hover_info");
    general.add(&shown);
    let mut rows: Vec<gtk::Widget> = vec![
        scale_row(
            "延遲",
            config.number("hover_delay", 0.1),
            (0.1, 2.0, 0.1),
            |seconds| format!("{seconds:.1} 秒"),
            |seconds| set(&format!("hover_delay = {seconds:.1}")),
        )
        .upcast(),
        switch_row(config, "自動隱藏", "hover_auto_hide").upcast(),
        switch_row(config, "大型檔案保護", "hover_large_file_protection").upcast(),
    ];
    let hidden: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(
        config
            .0
            .get("hover_hide")
            .and_then(toml::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| HOVER_HIDDEN.map(String::from).to_vec()),
    ));
    let fields = adw::ActionRow::builder().title("欄位").activatable(true).build();
    fields.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    fields.connect_activated(move |row| hover_fields_dialog(row, hidden.clone()));
    rows.push(fields.upcast());
    for row in &rows {
        general.add(row);
    }
    page.add(&general);

    let place = group("");
    place.add(&switch_row(config, "跟隨指標", "hover_follow"));
    for (title, key) in [("水平偏移", "hover_offset_x"), ("垂直偏移", "hover_offset_y")] {
        place.add(&scale_row(
            title,
            config.number(key, 15.0),
            (0.0, 50.0, 5.0),
            |offset| format!("{offset:.0} px"),
            move |offset| set(&format!("{key} = {offset:.0}")),
        ));
    }

    let look = group("外觀");
    look.add(&choice_row(
        config,
        "樣式",
        "hover_style",
        &HOVER_STYLES,
        &["詳細", "工具提示"],
    ));
    let glass = switch_row(config, "毛玻璃", "hover_glass");
    let opacity = scale_row(
        "不透明度",
        config.number("hover_opacity", 0.98),
        (0.7, 1.0, 0.05),
        percent,
        |opacity| set(&format!("hover_opacity = {opacity:.2}")),
    );
    // As in FinderHover, opacity is for the plain panel; glass has its own.
    glass
        .bind_property("active", &opacity, "sensitive")
        .invert_boolean()
        .sync_create()
        .build();
    look.add(&glass);
    look.add(&opacity);
    look.add(&switch_row(config, "緊湊模式", "hover_compact"));
    look.add(&scale_row(
        "寬度",
        config.number("hover_width", 300.0),
        (300.0, 600.0, 20.0),
        |width| format!("{width:.0} px"),
        |width| set(&format!("hover_width = {width:.0}")),
    ));
    look.add(&scale_row(
        "文字大小",
        config.number("hover_font_size", 11.0),
        (9.0, 14.0, 1.0),
        |size| format!("{size:.0} 點"),
        |size| set(&format!("hover_font_size = {size:.1}")),
    ));
    page.add(&look);
    let more = group("");
    more.add(&subpage_row("位置", hover_place_summary, page_of_groups(&[place])));
    page.add(&more);

    // With the panel off, only the switch that brings it back is left.
    rows.extend([more.upcast(), look.upcast()]);
    for widget in rows {
        shown
            .bind_property("active", &widget, "visible")
            .sync_create()
            .build();
    }
    page
}

/// The hover panel's parts and every kind's fields, each to show or leave out, beside the
/// panel as the compositor draws it, as FinderHover's display settings.
fn hover_fields_dialog(parent: &adw::ActionRow, hidden: Rc<RefCell<Vec<String>>>) {
    const BASIC: &str = "基本資訊";
    // A line a part: its name, then its fields' labels, by tabs.
    let catalogue: Vec<(String, Vec<String>)> = output("nekoland", &["msg", "hover fields"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut words = line.split('\t');
            Some((words.next()?.to_string(), words.map(String::from).collect()))
        })
        .collect();
    let labels_of = |name: &str| {
        catalogue
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, labels)| labels.clone())
            .unwrap_or_default()
    };
    let basic = [
        ("dates", "日期", labels_of("dates")),
        ("file", "檔案資訊", labels_of("file")),
    ];
    let kinds: Vec<(String, Vec<String>)> = catalogue
        .iter()
        .filter(|(name, _)| name != "dates" && name != "file")
        .cloned()
        .collect();
    let is_hidden = {
        let hidden = hidden.clone();
        move |name: &str| hidden.borrow().iter().any(|hidden| hidden == name)
    };

    let picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();
    let selected = Rc::new(RefCell::new(BASIC.to_string()));
    let show_preview: Rc<dyn Fn()> = Rc::new({
        let (picture, selected) = (picture.clone(), selected.clone());
        move || {
            let command = format!("hover preview {}", selected.borrow());
            let Some(reply) = output("nekoland", &["msg", &command]) else {
                return;
            };
            let words: Vec<&str> = reply.trim().split('\t').collect();
            let [path, width, height] = words[..] else {
                return;
            };
            match gdk::Texture::from_filename(path) {
                Ok(texture) => {
                    picture.set_paintable(Some(&texture));
                    // In points: the PNG is drawn at the screen's scale.
                    picture.set_size_request(width.parse().unwrap_or(-1), height.parse().unwrap_or(-1));
                }
                Err(err) => eprintln!("{path}: {err}"),
            }
        }
    });

    // Each kind in the list says how many of its fields show, or that it is off.
    let summaries: Rc<Vec<(String, Vec<String>, gtk::Label)>> = Rc::new(
        kinds
            .iter()
            .map(|(name, labels)| {
                let label = gtk::Label::builder()
                    .css_classes(["dim-label", "numeric"])
                    .build();
                (name.clone(), labels.clone(), label)
            })
            .collect(),
    );
    let summarize: Rc<dyn Fn()> = Rc::new({
        let (summaries, is_hidden) = (summaries.clone(), is_hidden.clone());
        move || {
            for (name, labels, label) in summaries.iter() {
                if is_hidden(name) || is_hidden("type") {
                    label.set_label("關閉");
                } else {
                    let shown = labels
                        .iter()
                        .filter(|field| !is_hidden(&format!("{name}/{field}")))
                        .count();
                    label.set_label(&format!("{shown}/{}", labels.len()));
                }
            }
        }
    });
    let toggle: Rc<dyn Fn(&str, bool)> = Rc::new({
        let (hidden, summarize, show_preview) = (hidden.clone(), summarize.clone(), show_preview.clone());
        move |name, shown| {
            {
                let mut hidden = hidden.borrow_mut();
                hidden.retain(|hidden| hidden != name);
                if !shown {
                    hidden.push(name.to_string());
                }
                if *hidden == HOVER_HIDDEN {
                    unset("hover_hide");
                } else {
                    set(&format!("hover_hide = {hidden:?}"));
                }
            }
            summarize();
            show_preview();
        }
    });
    let switch = {
        let (toggle, is_hidden) = (toggle.clone(), is_hidden.clone());
        move |title: &str, name: String| {
            let row = adw::SwitchRow::builder()
                .title(title)
                .active(!is_hidden(&name))
                .build();
            let toggle = toggle.clone();
            row.connect_active_notify(move |row| toggle(&name, row.is_active()));
            row
        }
    };

    let inspector = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    let fill_inspector = {
        let (inspector, toggle, is_hidden) = (inspector.clone(), toggle.clone(), is_hidden.clone());
        let kinds = kinds.clone();
        move |name: &str| {
            while let Some(child) = inspector.first_child() {
                inspector.remove(&child);
            }
            if name == BASIC {
                let sections = group(BASIC);
                for (title, section) in HOVER_SECTIONS {
                    sections.add(&switch(title, section.to_string()));
                }
                inspector.append(&sections);
                for (part, title, labels) in &basic {
                    let fields = group(title);
                    for label in labels {
                        fields.add(&switch(label, format!("{part}/{label}")));
                    }
                    inspector.append(&fields);
                }
                return;
            }
            let Some((_, labels)) = kinds.iter().find(|(kind, _)| kind == name) else {
                return;
            };
            let fields = group(name);
            let master = gtk::Switch::builder()
                .active(!is_hidden(name))
                .valign(gtk::Align::Center)
                .build();
            master.update_property(&[gtk::accessible::Property::Label(name)]);
            fields.set_header_suffix(Some(&master));
            for label in labels {
                let row = switch(label, format!("{name}/{label}"));
                master
                    .bind_property("active", &row, "sensitive")
                    .sync_create()
                    .build();
                fields.add(&row);
            }
            let (toggle, kind) = (toggle.clone(), name.to_string());
            master.connect_active_notify(move |master| toggle(&kind, master.is_active()));
            inspector.append(&fields);
        }
    };

    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let names: Vec<String> = std::iter::once(BASIC.to_string())
        .chain(kinds.iter().map(|(name, _)| name.clone()))
        .collect();
    for name in &names {
        let title = gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&title);
        if let Some((.., summary)) = summaries.iter().find(|(kind, ..)| kind == name) {
            line.append(summary);
        }
        list.append(&line);
    }
    summarize();
    list.connect_row_selected(move |_, row| {
        let Some(name) = row.and_then(|row| names.get(row.index() as usize)) else {
            return;
        };
        selected.replace(name.clone());
        fill_inspector(name);
        show_preview();
    });

    let body = gtk::Box::builder().build();
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(false)
            .width_request(160)
            .child(&list)
            .build(),
    );
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(
        &gtk::ScrolledWindow::builder()
            .hexpand(true)
            .css_classes(["view"])
            .child(&picture)
            .build(),
    );
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(false)
            .width_request(260)
            .child(&inspector)
            .build(),
    );
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("欄位")
        .content_width(1040)
        .content_height(720)
        .child(&view)
        .build();
    dialog.present(Some(parent));
    list.select_row(list.row_at_index(0).as_ref());
}

/// A row naming its font in that font, which opens the font picker; `mono` picks a face and
/// size among the monospaced families, otherwise a family alone.
fn font_row(
    title: &str,
    mono: bool,
    font: &str,
    changed: impl Fn(&gtk::pango::FontDescription) + 'static,
) -> adw::ActionRow {
    let font = Rc::new(RefCell::new(gtk::pango::FontDescription::from_string(font)));
    let current = gtk::Label::builder().css_classes(["dim-label"]).build();
    let row = adw::ActionRow::builder().title(title).activatable(true).build();
    row.add_suffix(&current);
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let show = move |font: &gtk::pango::FontDescription| {
        current.set_label(&font.to_string());
        write_in(&current, font);
    };
    show(&font.borrow());
    let apply: Rc<dyn Fn(&gtk::pango::FontDescription)> = Rc::new({
        let font = font.clone();
        move |chosen| {
            show(chosen);
            font.replace(chosen.clone());
            changed(chosen);
        }
    });
    let title = format!("{title}字型");
    row.connect_activated(move |row| {
        let applied = font.borrow().clone();
        font_picker(row, &title, mono, applied, apply.clone());
    });
    row
}

/// The font picker: each family named in itself, and a page of Nekoland's own text in the
/// one shown. The arrow keys only show a family; clicking it or Enter sets it.
fn font_picker(
    parent: &adw::ActionRow,
    title: &str,
    mono: bool,
    applied: gtk::pango::FontDescription,
    apply: Rc<dyn Fn(&gtk::pango::FontDescription)>,
) {
    use gtk::pango::{self, prelude::*};
    let applied = Rc::new(RefCell::new(applied));
    let size = Rc::new(Cell::new(applied.borrow().size()));
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let context = list.pango_context();
    let mut families: Vec<pango::FontFamily> = context
        .list_families()
        .into_iter()
        .filter(|family| !mono || family.is_monospace())
        .collect();
    families.sort_by_key(|family| family.name().to_lowercase());
    // A family as the picker sets it: the interface's alone, the monospace one's
    // default face at the size chosen.
    let font_for = {
        let size = size.clone();
        move |family: &pango::FontFamily| {
            let mut font = pango::FontDescription::new();
            if mono {
                if let Some(face) = family.face(None) {
                    font = face.describe();
                }
                font.set_size(size.get());
            }
            font.set_family(&family.name());
            font
        }
    };

    let preview = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(120)
        .css_classes(["card"])
        .build();
    let pages = [preview_page(mono), preview_page(mono)];
    for (page, labels) in &pages {
        preview.add_child(page);
        labels.iter().for_each(|label| write_in(label, &applied.borrow()));
    }
    // The page out of sight takes the font, then fades in over the other.
    let show = Rc::new(glib::clone!(
        #[weak]
        preview,
        move |font: &pango::FontDescription| {
            let shown = preview.visible_child();
            let (page, labels) = if shown.as_ref() == Some(pages[0].0.upcast_ref()) {
                &pages[1]
            } else {
                &pages[0]
            };
            labels.iter().for_each(|label| write_in(label, font));
            preview.set_visible_child(page);
        }
    ));

    let mut rows = Vec::new();
    for family in &families {
        let name = family.name();
        let mut own = pango::FontDescription::new();
        own.set_family(&name);
        own.set_size(12 * pango::SCALE);
        let label = gtk::Label::builder()
            .label(name.as_str())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        let Some(font) = context.load_font(&own) else {
            continue;
        };
        // Icon and symbol fonts, and those of other scripts alone, cannot write a name.
        if !font.has_char('a') {
            continue;
        }
        write_in(&label, &own);
        let chinese = font.has_char('體');
        let tag = gtk::Label::builder()
            .label("中文")
            .css_classes(["dim-label", "caption"])
            .visible(chinese)
            .build();
        let check = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .visible(applied.borrow().family().as_deref() == Some(name.as_str()))
            .build();
        let line = gtk::Box::builder()
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        line.append(&label);
        line.append(&tag);
        line.append(&check);
        list.append(&line);
        rows.push((family.clone(), chinese, check));
    }
    let rows = Rc::new(rows);
    list.set_placeholder(Some(
        &gtk::Label::builder()
            .label("沒有符合的字型")
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build(),
    ));

    let search = gtk::SearchEntry::builder()
        .placeholder_text("搜尋")
        .hexpand(true)
        .build();
    let chinese_only = Rc::new(Cell::new(false));
    list.set_filter_func(glib::clone!(
        #[strong]
        rows,
        #[strong]
        chinese_only,
        #[weak]
        search,
        #[upgrade_or]
        true,
        move |row| {
            let (family, chinese, _) = &rows[row.index() as usize];
            (*chinese || !chinese_only.get())
                && family
                    .name()
                    .to_lowercase()
                    .contains(&search.text().to_lowercase())
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[weak]
        list,
        move |_| list.invalidate_filter()
    ));
    let tools = gtk::Box::builder().spacing(8).build();
    tools.append(&search);
    if !mono {
        let only = segmented(&["全部", "含中文"], 0);
        only.connect_notify_local(
            Some("active"),
            glib::clone!(
                #[weak]
                list,
                move |only, _| {
                    chinese_only.set(only.property::<u32>("active") == 1);
                    list.invalidate_filter();
                }
            ),
        );
        tools.append(&only);
    }

    // The monospace font's face, among its family's, and its size.
    let faces = Rc::new(RefCell::new(Vec::<pango::FontFace>::new()));
    let face_names = gtk::StringList::new(&[]);
    let face = gtk::DropDown::builder().model(&face_names).build();
    let filling = Rc::new(Cell::new(false));
    let fill_faces = Rc::new(glib::clone!(
        #[strong]
        faces,
        #[strong]
        applied,
        #[strong]
        filling,
        #[strong]
        rows,
        #[weak]
        face,
        move || {
            let applied = applied.borrow();
            let family = rows
                .iter()
                .find(|(family, ..)| applied.family().as_deref() == Some(family.name().as_str()));
            let list: Vec<pango::FontFace> =
                family.map(|(family, ..)| family.list_faces()).unwrap_or_default();
            let mut bare = applied.clone();
            bare.unset_fields(pango::FontMask::SIZE);
            filling.set(true);
            face_names.splice(0, face_names.n_items(), &[]);
            for one in &list {
                face_names.append(&one.face_name());
            }
            face.set_selected(list.iter().position(|one| one.describe() == bare).unwrap_or(0) as u32);
            face.set_visible(list.len() > 1);
            filling.set(false);
            faces.replace(list);
        }
    ));
    let points = gtk::SpinButton::with_range(6.0, 72.0, 1.0);
    points.set_value(f64::from(size.get() / pango::SCALE));
    let set_font = Rc::new(glib::clone!(
        #[strong]
        applied,
        #[strong]
        apply,
        #[strong]
        show,
        move |font: pango::FontDescription| {
            apply(&font);
            show(&font);
            applied.replace(font);
        }
    ));
    face.connect_selected_notify(glib::clone!(
        #[strong]
        faces,
        #[strong]
        filling,
        #[strong]
        size,
        #[strong]
        set_font,
        move |face| {
            let Some(chosen) = faces
                .borrow()
                .get(face.selected() as usize)
                .map(|one| one.describe())
            else {
                return;
            };
            if !filling.get() {
                let mut font = chosen;
                font.set_size(size.get());
                set_font(font);
            }
        }
    ));
    points.connect_value_changed(glib::clone!(
        #[strong]
        size,
        #[strong]
        applied,
        #[strong]
        set_font,
        move |points| {
            size.set(points.value() as i32 * pango::SCALE);
            let mut font = applied.borrow().clone();
            font.set_size(size.get());
            set_font(font);
        }
    ));
    let face_line = gtk::Box::builder().spacing(8).visible(mono).build();
    face_line.append(&face);
    face_line.append(&gtk::Box::builder().hexpand(true).build());
    face_line.append(&points);
    if mono {
        fill_faces();
    }

    list.connect_row_selected(glib::clone!(
        #[strong]
        rows,
        #[strong]
        show,
        #[strong]
        font_for,
        move |_, row| {
            if let Some((family, ..)) = row.and_then(|row| rows.get(row.index() as usize)) {
                show(&font_for(family));
            }
        }
    ));
    list.connect_row_activated(glib::clone!(
        #[strong]
        rows,
        move |_, row| {
            let Some((family, ..)) = rows.get(row.index() as usize) else {
                return;
            };
            set_font(font_for(family));
            for (other, _, check) in rows.iter() {
                check.set_visible(other.name() == family.name());
            }
            if mono {
                fill_faces();
            }
        }
    ));

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(24)
        .build();
    body.append(&preview);
    body.append(&face_line);
    body.append(&tools);
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .css_classes(["card"])
            .overflow(gtk::Overflow::Hidden)
            .child(&list)
            .build(),
    );
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(520)
        .content_height(640)
        .child(&view)
        .build();
    search.set_key_capture_widget(Some(&dialog));
    dialog.present(Some(parent));
    // Focus, which scrolls to the row, takes once the dialog is shown.
    let applied_name = applied.borrow().family().map(|family| family.to_string());
    let index = rows
        .iter()
        .position(|(family, ..)| Some(family.name().to_string()) == applied_name);
    glib::idle_add_local_once(move || {
        if let Some(row) = index.and_then(|index| list.row_at_index(index as i32)) {
            list.select_row(Some(&row));
            row.grab_focus();
        }
    });
}

/// A page of Nekoland's own text, for the font picker to show a font in, and its labels.
fn preview_page(mono: bool) -> (gtk::Box, Vec<gtk::Label>) {
    let mut labels = Vec::new();
    let mut text = |text: &str, classes: &[&str]| {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .css_classes(classes.to_vec())
            .build();
        labels.push(label.clone());
        label
    };
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    if mono {
        let code = text(
            "~/Nekoland ❯ cargo check\n\
             fn font(description: &str) -> FontDescription {\n    \
             // 沒有字族時用介面字型\n    \
             let mut font = from_string(description);\n    \
             font.set_size(11 * SCALE); // 0O1lI {}[]\n}",
            &[],
        );
        code.set_margin_top(14);
        code.set_margin_bottom(14);
        code.set_margin_start(16);
        code.set_margin_end(16);
        page.append(&code);
        return (page, labels);
    }
    let now = glib::DateTime::now_local().expect("local time");
    let weekday = ["一", "二", "三", "四", "五", "六", "日"][now.day_of_week() as usize - 1];
    let bar = gtk::Box::builder()
        .spacing(16)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(14)
        .margin_end(14)
        .build();
    bar.append(&text("文件檢視器", &["heading"]));
    for menu in ["檔案", "編輯", "檢視"] {
        bar.append(&text(menu, &[]));
    }
    let clock = text(
        &format!(
            "{}月{}日 週{weekday} {:02}:{:02}",
            now.month(),
            now.day_of_month(),
            now.hour(),
            now.minute()
        ),
        &["numeric"],
    );
    clock.set_hexpand(true);
    clock.set_xalign(1.0);
    bar.append(&clock);
    page.append(&bar);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let note = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(14)
        .margin_start(14)
        .margin_end(14)
        .css_classes(["card"])
        .build();
    let from = gtk::Box::builder()
        .margin_top(10)
        .margin_start(14)
        .margin_end(14)
        .build();
    from.append(&text("行事曆", &["caption", "dim-label"]));
    let when = text("現在", &["caption", "dim-label"]);
    when.set_hexpand(true);
    when.set_xalign(1.0);
    from.append(&when);
    note.append(&from);
    let title = text("設計評審 · 14:30", &["heading"]);
    let body = text("3A 會議室，帶 Q3 Roadmap 的 PDF 與 Dock 新圖示", &["dim-label"]);
    for line in [&title, &body] {
        line.set_margin_start(14);
        line.set_margin_end(14);
    }
    body.set_margin_bottom(10);
    note.append(&title);
    note.append(&body);
    page.append(&note);

    let window = text("季報-Q3.pdf — 文件檢視器", &["heading", "dim-label"]);
    window.set_xalign(0.5);
    window.set_margin_top(12);
    window.set_margin_bottom(14);
    page.append(&window);
    (page, labels)
}

/// The typefaces, for Nekoland and the applications, and how applications draw text.
/// Nekoland's own text keeps its size and the Mac's grey, unhinted edges.
fn fonts_page(look: &gtk::CssProvider) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let faces = group("字型");
    let family = UI_FONT.with_borrow(Clone::clone);
    let look = look.clone();
    faces.add(&font_row("介面", false, &family, move |font| {
        let Some(family) = font.family() else { return };
        set(&format!("font = {:?}", family.as_str()));
        // GNOME's setting keeps its size; GTK takes the family from it without the look.
        let mut name = gtk::pango::FontDescription::from_string(&interface_setting("font-name"));
        name.set_family(&family);
        if name.size() == 0 {
            name.set_size(11 * gtk::pango::SCALE);
        }
        set_interface("font-name", &name.to_string());
        UI_FONT.set(family.to_string());
        if gtk_look_on() {
            if let Err(err) = set_gtk_look(true) {
                eprintln!("GTK look: {err}");
            }
            look.load_from_string(&look_for(adw::StyleManager::default().is_dark()));
        }
    }));
    faces.add(&font_row(
        "等寬",
        true,
        &interface_setting("monospace-font-name"),
        |font| set_interface("monospace-font-name", &font.to_string()),
    ));
    page.add(&faces);

    let text = group("文字");
    let scale: f64 = interface_setting("text-scaling-factor").parse().unwrap_or(1.0);
    let size = adw::ComboRow::builder()
        .title("文字大小")
        .model(&gtk::StringList::new(&["100%", "110%", "125%", "150%", "175%", "200%"]))
        .selected(TEXT_SCALES.iter().position(|known| (known - scale).abs() < 0.01).unwrap_or(0) as u32)
        .build();
    size.connect_selected_notify(|row| {
        if let Some(scale) = TEXT_SCALES.get(row.selected() as usize) {
            set_interface("text-scaling-factor", &scale.to_string());
        }
    });
    text.add(&size);

    let automatic = interface_setting("font-rendering") == "automatic";
    let antialiasing = adw::ComboRow::builder()
        .title("反鋸齒")
        .model(&gtk::StringList::new(&["自動", "灰階", "次像素", "無"]))
        .selected(if automatic {
            0
        } else {
            let current = interface_setting("font-antialiasing");
            ANTIALIASING.iter().position(|known| *known == current).unwrap_or(0) as u32
        })
        .build();
    let hinting = adw::ComboRow::builder()
        .title("微調")
        .model(&gtk::StringList::new(&["無", "輕微", "中等", "完整"]))
        .selected({
            let current = interface_setting("font-hinting");
            HINTING.iter().position(|known| *known == current).unwrap_or(0) as u32
        })
        .visible(!automatic)
        .build();
    antialiasing.connect_selected_notify(glib::clone!(
        #[weak]
        hinting,
        move |row| {
            let Some(chosen) = ANTIALIASING.get(row.selected() as usize) else { return };
            let automatic = *chosen == "automatic";
            set_interface("font-rendering", if automatic { "automatic" } else { "manual" });
            if !automatic {
                set_interface("font-antialiasing", chosen);
            }
            hinting.set_visible(!automatic);
        }
    ));
    hinting.connect_selected_notify(|row| {
        if let Some(chosen) = HINTING.get(row.selected() as usize) {
            set_interface("font-hinting", chosen);
        }
    });
    text.add(&antialiasing);
    text.add(&hinting);
    page.add(&text);
    page
}

fn appearance_page(config: &Config, provider: &gtk::CssProvider) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let style = group("");
    let dark = output(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    )
    .is_some_and(|scheme| scheme.contains("dark"));
    let scheme = adw::ComboRow::builder()
        .title("配色")
        .model(&gtk::StringList::new(&["淺色", "深色"]))
        .selected(u32::from(dark))
        .build();
    scheme.connect_selected_notify(|row| {
        let value = if row.selected() == 1 {
            "prefer-dark"
        } else {
            "default"
        };
        run(
            "gsettings",
            &["set", "org.gnome.desktop.interface", "color-scheme", value],
        );
    });
    style.add(&scheme);

    let look = adw::SwitchRow::builder()
        .title("Nekoland 外觀")
        .subtitle("GTK app 重新開啟後套用")
        .active(gtk_look_on())
        .build();
    if gtk::minor_version() < GTK_LOOK_MINOR {
        look.set_sensitive(false);
        look.set_subtitle(&format!("需要 GTK 4.{GTK_LOOK_MINOR} 以上"));
    }
    let provider = provider.clone();
    let read_at_start = look_read_at_start();
    let glass_look = provider.clone();
    look.connect_active_notify(move |row| {
        let on = row.is_active();
        if let Err(err) = set_gtk_look(on) {
            eprintln!("GTK look: {err}");
            return;
        }
        set_gtk3_theme(on);
        let display = row.display();
        if on {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        } else {
            gtk::style_context_remove_provider_for_display(&display, &provider);
        }
        // What GTK read as Settings started cannot be taken back but by starting again.
        if !on && read_at_start {
            RESTART.set(Some("外觀"));
            if let Some(app) = row
                .root()
                .and_downcast::<gtk::Window>()
                .and_then(|window| window.application())
            {
                app.quit();
            }
        }
    });
    style.add(&look);
    let scroll_bars = adw::SwitchRow::builder()
        .title("一律顯示捲軸")
        .active(interface_setting("overlay-scrolling") == "false")
        .build();
    scroll_bars.connect_active_notify(|row| set_interface("overlay-scrolling", &(!row.is_active()).to_string()));
    style.add(&scroll_bars);
    page.add(&style);

    let themes = group("");
    themes.add(&theme_row(config, "圖示主題", "icon_theme", "icon-theme", false));
    themes.add(&converted_icons_row());
    themes.add(&own_icons_row());
    themes.add(&theme_row(config, "游標主題", "cursor_theme", "cursor-theme", true));
    page.add(&themes);

    let fonts = fonts_page(&glass_look);
    let bars = glass_group(config, "選單列與 Dock", ("bar", 0.5), |_| {});
    bars.add(&switch_row(config, "選單列背景", "menu_bar_background"));
    // GTK's menus and sidebars take their tint from the look, written again as it changes.
    let glass = glass_group(config, "選單、通知與視窗", ("glass", 0.8), move |opacity| {
        GLASS_OPACITY.set(opacity);
        if gtk_look_on() {
            if let Err(err) = set_gtk_look(true) {
                eprintln!("GTK look: {err}");
            }
            glass_look.load_from_string(&look_for(adw::StyleManager::default().is_dark()));
        }
    });
    let more = group("");
    more.add(&subpage_row("字型", || UI_FONT.with_borrow(Clone::clone), fonts));
    more.add(&subpage_row(
        "毛玻璃",
        || format!("不透明度 {:.0}%", GLASS_OPACITY.get() * 100.0),
        page_of_groups(&[bars, glass]),
    ));
    page.add(&more);
    page
}

/// A row choosing a theme for Nekoland's `key` and GNOME's `gsetting` together; 預設 leaves
/// both to their own choice.
fn theme_row(config: &Config, title: &str, key: &'static str, gsetting: &'static str, cursors: bool) -> adw::ComboRow {
    let themes: Vec<String> = themes::installed(cursors).into_iter().map(|theme| theme.name).collect();
    let labels: Vec<&str> = std::iter::once("預設").chain(themes.iter().map(String::as_str)).collect();
    let current = config.word(key);
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&labels))
        .selected(themes.iter().position(|theme| theme == current).map_or(0, |at| at + 1) as u32)
        .build();
    row.connect_selected_notify(move |row| match row.selected().checked_sub(1) {
        None => {
            unset(key);
            run("gsettings", &["reset", INTERFACE, gsetting]);
        }
        Some(at) => {
            if let Some(theme) = themes.get(at as usize) {
                set(&format!("{key} = {theme:?}"));
                set_interface(gsetting, &format!("'{theme}'"));
            }
        }
    });
    row
}

/// The hand-drawn icons of Nekoland's own icon theme, as install.sh puts them.
const DRAWN_ICONS: &str = "/usr/local/share/icons/Nekoland/scalable/apps";
/// What nekoland-conform-icons marks the icons it makes with.
const CONVERTED_MARK: &[u8] = b"<!-- conform-icons -->";

/// The SVG icons in `dir`, by name.
fn icons_in(dir: &Path) -> Vec<PathBuf> {
    let mut icons: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .collect();
    icons.sort();
    icons
}

/// The icons made from other applications' own, in the person's half of Nekoland's theme.
fn converted_icons() -> Vec<PathBuf> {
    use std::io::Read;
    icons_in(&themes::user_dir().join("Nekoland/scalable/apps"))
        .into_iter()
        .filter(|path| {
            let mut head = [0; 64];
            std::fs::File::open(path)
                .and_then(|mut file| file.read(&mut head))
                .is_ok_and(|read| head[..read].starts_with(CONVERTED_MARK))
        })
        .collect()
}

/// Whether other applications' icons are redrawn on Nekoland's tile; turning it on makes
/// them, off removes them, and either way the compositor looks its icons up again.
fn converted_icons_row() -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title("轉換其他應用程式的圖示")
        .active(!converted_icons().is_empty())
        .build();
    row.connect_active_notify(|row| {
        let remove = !row.is_active();
        row.set_sensitive(false);
        let row = row.clone();
        glib::spawn_future_local(async move {
            // Several seconds for a few hundred icons, so not on the main loop.
            let _ = gio::spawn_blocking(move || {
                run("nekoland-conform-icons", if remove { &["--remove"] } else { &[] });
                run("nekoland", &["msg", "reload"]);
            })
            .await;
            row.set_sensitive(true);
        });
    });
    row
}

/// Opens the list of Nekoland's icons: which are drawn by hand, which are converted.
fn own_icons_row() -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Nekoland 圖示").activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row.connect_activated(own_icons_dialog);
    row
}

fn own_icons_dialog(parent: &adw::ActionRow) {
    let page = adw::PreferencesPage::new();
    for (title, icons) in [
        ("手繪", icons_in(Path::new(DRAWN_ICONS))),
        ("自動演算", converted_icons()),
    ] {
        let icons_group = adw::PreferencesGroup::builder()
            .title(title)
            .description(icons.len().to_string())
            .build();
        let flow = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(12)
            .min_children_per_line(6)
            .column_spacing(6)
            .row_spacing(6)
            .homogeneous(true)
            .build();
        icons_group.add(&flow);
        page.add(&icons_group);
        // A few at a time: each is an SVG with filters, and there may be hundreds.
        let mut icons = icons.into_iter();
        glib::idle_add_local(move || {
            for path in icons.by_ref().take(8) {
                let name = path.file_stem().map(|stem| stem.to_string_lossy().into_owned());
                let image = gtk::Image::builder().file(path.to_string_lossy()).pixel_size(48).build();
                image.set_tooltip_text(name.as_deref());
                flow.append(&image);
            }
            if icons.len() == 0 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    adw::Dialog::builder()
        .title("Nekoland 圖示")
        .content_width(760)
        .content_height(640)
        .child(&view)
        .build()
        .present(Some(parent));
}

/// The opacity and blur of one kind of glass, Nekoland's `<key>_opacity` and `<key>_blur`;
/// `opacity_changed` is told each new opacity once Nekoland has it.
fn glass_group(
    config: &Config,
    title: &str,
    (key, opacity): (&'static str, f64),
    opacity_changed: impl Fn(f64) + 'static,
) -> adw::PreferencesGroup {
    let glass = group(title);
    glass.add(&scale_row(
        "不透明度",
        config.number(&format!("{key}_opacity"), opacity),
        (0.0, 1.0, 0.05),
        percent,
        move |opacity| {
            set(&format!("{key}_opacity = {opacity:.2}"));
            opacity_changed(opacity);
        },
    ));
    glass.add(&scale_row(
        "模糊程度",
        config.number(&format!("{key}_blur"), 0.5),
        (0.0, 1.0, 0.05),
        percent,
        move |blur| set(&format!("{key}_blur = {blur:.2}")),
    ));
    glass
}

/// Nekoland's design language for GTK 4 and libadwaita applications, as libadwaita's
/// variables; see the file for what it reaches.
const GTK_LOOK: &str = include_str!("../../shell/gtk/nekoland.css");
/// Where Nekoland's look goes, in a folder of its own beside GTK's user stylesheet.
const GTK_LOOK_FILE: &str = "nekoland/look.css";
/// What GTK's own user stylesheet imports it with, the rest of that file left as it is.
const GTK_IMPORT: &str = "@import url(\"nekoland/look.css\");";
/// The import of earlier versions, from beside that stylesheet; taken out for the one above.
const OLD_GTK_IMPORT: &str = "@import url(\"nekoland.css\");";
/// The first GTK whose stylesheets read the look's variables and its dark media query.
const GTK_LOOK_MINOR: u32 = 20;

/// The look as GTK's user stylesheet takes it, with Nekoland's glass opacity.
fn gtk_look() -> String {
    let opacity = GLASS_OPACITY.get() * 100.0;
    let font = UI_FONT.with_borrow(|font| css_family(font));
    format!(
        "{GTK_LOOK}\n:root {{ --nekoland-glass-opacity: {opacity:.0}%; \
         --nekoland-font: {font}, \"Noto Sans CJK TC\", sans-serif; }}\n"
    )
}

/// A family's name as CSS quotes it; quotes and backslashes in it are left out.
fn css_family(family: &str) -> String {
    format!("\"{}\"", family.replace(['"', '\\'], ""))
}

/// The look for a stylesheet Settings loads itself, which GTK reads light whatever the
/// system is: when dark, its dark colours are put in outside their media query.
fn look_for(dark: bool) -> String {
    let dark_part = GTK_LOOK
        .split_once("@media (prefers-color-scheme: dark) {")
        .and_then(|(_, part)| part.trim_end().strip_suffix('}'))
        .filter(|_| dark)
        .unwrap_or_default();
    format!("{}{dark_part}", gtk_look())
}

/// Where GTK 4 reads the user's stylesheet, and where Nekoland's goes beside it.
fn gtk_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("gtk-4.0")
}

fn gtk_look_on() -> bool {
    std::fs::read_to_string(gtk_dir().join("gtk.css")).is_ok_and(|text| {
        text.lines().any(|line| [GTK_IMPORT, OLD_GTK_IMPORT].contains(&line.trim()))
    })
}

/// Writes a file whole or not at all, so a crash midway leaves the one there before.
fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    // A stylesheet linked from a dotfiles folder is written there, the link kept.
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let partial = path.with_extension("css.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}

/// Imports Nekoland's look into GTK's user stylesheet, or takes it out again, keeping
/// whatever else is there.
fn set_gtk_look(on: bool) -> std::io::Result<()> {
    let dir = gtk_dir();
    let user = dir.join("gtk.css");
    let text = match std::fs::read_to_string(&user) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| ![GTK_IMPORT, OLD_GTK_IMPORT].contains(&line.trim()))
        .collect();
    let look = dir.join(GTK_LOOK_FILE);
    if on {
        std::fs::create_dir_all(look.parent().expect("a folder"))?;
        write_atomic(&look, &gtk_look())?;
        // An import counts only before any rule.
        lines.insert(0, GTK_IMPORT);
    } else if let Err(err) = std::fs::remove_file(&look) {
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err);
        }
    }
    if lines.is_empty() && !on {
        return match std::fs::remove_file(&user) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    write_atomic(&user, &(lines.join("\n") + "\n"))
}

/// The GTK 3 theme of the look, which install.sh puts in /usr/local/share/themes, is
/// GNOME's `gtk-theme` while the look is on: GTK 3 programs read it on Wayland, and the
/// compositor gives it to X11 ones as XSettings. Off, it is put back to the default, unless
/// another was chosen.
fn set_gtk3_theme(on: bool) {
    let current = interface_setting("gtk-theme");
    if on {
        if current != GTK3_THEME {
            set_interface("gtk-theme", &format!("'{GTK3_THEME}'"));
        }
    } else if current == GTK3_THEME {
        run("gsettings", &["reset", INTERFACE, "gtk-theme"]);
    }
}

const GTK3_THEME: &str = "Nekoland";

const XKB_LIST: &str = "/usr/share/X11/xkb/rules/base.lst";

/// The keyboard layouts XKB knows, each followed by its variants, as
/// `(layout, variant, description)`.
fn xkb_layouts(list: &str) -> Vec<(String, String, String)> {
    let (mut layouts, mut variants) = (Vec::new(), Vec::new());
    let mut section = "";
    for line in list.lines() {
        if let Some(name) = line.strip_prefix("! ") {
            section = name.trim();
            continue;
        }
        let Some((name, description)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let description = description.trim();
        match section {
            "layout" => layouts.push((name, description)),
            // A variant says its layout first: "dvorak          us: English (Dvorak)".
            "variant" => {
                if let Some((layout, description)) = description.split_once(": ") {
                    variants.push((layout, name, description));
                }
            }
            _ => {}
        }
    }
    let mut all = Vec::new();
    for (layout, description) in layouts {
        all.push((layout.into(), String::new(), description.into()));
        for (_, variant, description) in variants.iter().filter(|(of, ..)| *of == layout) {
            all.push((layout.into(), variant.to_string(), description.to_string()));
        }
    }
    all
}

/// What the Caps Lock key can do, as XKB options.
const CAPS_OPTIONS: [&str; 4] = ["", "ctrl:nocaps", "caps:escape", "ctrl:swapcaps"];

fn keyboard_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let layout = group("");
    let layouts = match std::fs::read_to_string(XKB_LIST) {
        Ok(list) => xkb_layouts(&list),
        Err(err) => {
            eprintln!("{XKB_LIST}: {err}");
            Vec::new()
        }
    };
    let descriptions: Vec<&str> = layouts.iter().map(|(.., text)| text.as_str()).collect();
    let current = (config.word("keyboard_layout"), config.word("keyboard_variant"));
    let row = adw::ComboRow::builder()
        .title("鍵盤排列")
        .model(&gtk::StringList::new(&descriptions))
        .enable_search(true)
        .build();
    let chosen = layouts.iter().position(|(name, variant, _)| (&**name, &**variant) == current);
    if let Some(at) = chosen {
        row.set_selected(at as u32);
    }
    row.connect_selected_notify(move |row| {
        if let Some((name, variant, _)) = layouts.get(row.selected() as usize) {
            // One setting at a time, each a layout and variant XKB has: the variant goes
            // first so that it is never one the old layout lacks.
            set("keyboard_variant = \"\"");
            set(&format!("keyboard_layout = {name:?}"));
            set(&format!("keyboard_variant = {variant:?}"));
        }
    });
    layout.add(&row);
    layout.add(&choice_row(
        config,
        "Caps Lock 鍵",
        "keyboard_options",
        &CAPS_OPTIONS,
        &["Caps Lock", "Control", "Esc", "與 Control 對調"],
    ));
    page.add(&layout);
    page.add(&input_method_group());

    let repeat = group("");
    let repeats = switch_row(config, "按鍵重複", "key_repeat");
    repeat.add(&repeats);
    let delay = scale_row(
        "重複延遲",
        config.number("repeat_delay", 200.0),
        (150.0, 1000.0, 50.0),
        |delay| format!("{delay:.0} 毫秒"),
        |delay| set(&format!("repeat_delay = {}", delay.round() as i32)),
    );
    let rate = scale_row(
        "重複速度",
        config.number("repeat_rate", 25.0),
        (10.0, 60.0, 5.0),
        |rate| format!("{rate:.0} 次/秒"),
        |rate| set(&format!("repeat_rate = {}", rate.round() as i32)),
    );
    for row in [&delay, &rate] {
        repeats.bind_property("active", row, "visible").sync_create().build();
        repeat.add(row);
    }
    page.add(&repeat);
    let typing = group("");
    typing.add(&switch_row(config, "黏著鍵", "sticky_keys"));
    let blink = gsettings_switch("文字游標閃爍", INTERFACE, "cursor-blink");
    typing.add(&blink);
    // GNOME's time is one whole blink, on and off.
    let blink_time = scale_row(
        "閃爍間隔",
        interface_setting("cursor-blink-time").parse().unwrap_or(1200.0),
        (100.0, 2500.0, 100.0),
        |time| format!("{time:.0} 毫秒"),
        |time| set_interface("cursor-blink-time", &(time.round() as i32).to_string()),
    );
    blink.bind_property("active", &blink_time, "visible").sync_create().build();
    typing.add(&blink_time);
    let (shortcuts, scheme) = keys::shortcuts_page(config);
    typing.add(&subpage_row("鍵盤快速鍵", scheme, shortcuts));
    page.add(&typing);
    page
}

const FCITX: &str = "org.fcitx.Fcitx5";
const FCITX_PATH: &str = "/controller";
const FCITX_CONTROLLER: &str = "org.fcitx.Fcitx.Controller1";

/// Calls Fcitx's controller, the one fcitx5-config-qt calls; None when Fcitx is not
/// running or refuses.
fn fcitx(
    bus: &gio::DBusConnection,
    method: &str,
    args: Option<&glib::Variant>,
    reply: &str,
) -> Option<glib::Variant> {
    bus.call_sync(
        Some(FCITX),
        FCITX_PATH,
        FCITX_CONTROLLER,
        method,
        args,
        glib::VariantTy::new(reply).ok(),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    )
    .inspect_err(|err| eprintln!("fcitx5 {method}: {err}"))
    .ok()
}

/// Fcitx's group of input methods in use: its layout, and each method's ID, name and
/// layout, in its order. The first is the one Fcitx turns to when it is off.
#[derive(Clone)]
struct InputMethods {
    group: String,
    layout: String,
    methods: Vec<(String, String, String)>,
}

/// An input method's name in its own language when Fcitx has one, as 酷音 for Chewing.
fn input_method_name(native: &str, name: &str) -> String {
    if native.is_empty() { name } else { native }.to_string()
}

fn input_methods(bus: &gio::DBusConnection) -> Option<InputMethods> {
    let group = fcitx(bus, "CurrentInputMethodGroup", None, "(s)")?
        .child_value(0)
        .str()?
        .to_string();
    let info = fcitx(
        bus,
        "FullInputMethodGroupInfo",
        Some(&(group.as_str(),).to_variant()),
        "(sssa{sv}a(sssssssbsa{sv}))",
    )?;
    let layout = info.child_value(2).str()?.to_string();
    let methods = info
        .child_value(4)
        .iter()
        .map(|method| {
            let text = |at| method.child_value(at).str().unwrap_or_default().to_string();
            (text(0), input_method_name(&text(2), &text(1)), text(8))
        })
        .collect();
    Some(InputMethods {
        group,
        layout,
        methods,
    })
}

/// Fcitx's input methods in its group in use, to add, remove and put in order.
fn input_method_group() -> adw::PreferencesGroup {
    let methods = group("輸入法");
    let add = adw::ActionRow::builder()
        .title("新增輸入法")
        .activatable(true)
        .build();
    add.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .inspect_err(|err| eprintln!("session bus: {err}"))
        .ok();
    methods.add(&add);
    let Some(bus) = bus else {
        add.set_visible(false);
        return methods;
    };
    list_input_methods(&methods, &add, &rows, &bus, input_methods(&bus));
    add.connect_activated(glib::clone!(
        #[weak]
        methods,
        #[strong]
        rows,
        #[strong]
        bus,
        move |add| {
            let Some(current) = input_methods(&bus) else {
                list_input_methods(&methods, add, &rows, &bus, None);
                return;
            };
            input_method_picker(
                add,
                &bus,
                current,
                glib::clone!(
                    #[weak]
                    methods,
                    #[weak]
                    add,
                    #[strong]
                    rows,
                    #[strong]
                    bus,
                    move |id: String, name: String| {
                        let Some(mut changed) = input_methods(&bus) else {
                            list_input_methods(&methods, &add, &rows, &bus, None);
                            return;
                        };
                        if !changed.methods.iter().any(|(added, ..)| *added == id) {
                            changed.methods.push((id, name, String::new()));
                        }
                        save_input_methods(&methods, &add, &rows, &bus, changed);
                    }
                ),
            );
        }
    ));
    methods
}

/// Gives Fcitx the group as changed, which it saves, and lists what it then has.
fn save_input_methods(
    group: &adw::PreferencesGroup,
    add: &adw::ActionRow,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    bus: &gio::DBusConnection,
    changed: InputMethods,
) {
    let entries: Vec<(String, String)> = changed
        .methods
        .into_iter()
        .map(|(id, _, layout)| (id, layout))
        .collect();
    fcitx(
        bus,
        "SetInputMethodGroupInfo",
        Some(&(changed.group.as_str(), changed.layout.as_str(), entries).to_variant()),
        "()",
    );
    list_input_methods(group, add, rows, bus, input_methods(bus));
}

/// Lists the input methods above the row that adds one, again after each change.
fn list_input_methods(
    group: &adw::PreferencesGroup,
    add: &adw::ActionRow,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    bus: &gio::DBusConnection,
    current: Option<InputMethods>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    group.remove(add);
    let Some(current) = current else {
        let row = adw::ActionRow::builder()
            .title("Fcitx 5 未執行")
            .css_classes(["dim-label"])
            .build();
        group.add(&row);
        rows.borrow_mut().push(row);
        return;
    };
    let count = current.methods.len();
    for (index, (id, name, _)) in current.methods.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(name))
            .build();
        let button = |icon: &str, tooltip: &str, sensitive: bool| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .sensitive(sensitive)
                .build()
        };
        let up = button("go-up-symbolic", "上移", index > 0);
        let remove = button("user-trash-symbolic", "移除", count > 1);
        for (button, moved) in [(&up, true), (&remove, false)] {
            let id = id.clone();
            button.connect_clicked(glib::clone!(
                #[weak]
                group,
                #[weak]
                add,
                #[strong]
                rows,
                #[strong]
                bus,
                move |_| {
                    // Fcitx's group as it is now, as it may have changed since it was listed.
                    let Some(mut changed) = input_methods(&bus) else {
                        list_input_methods(&group, &add, &rows, &bus, None);
                        return;
                    };
                    match changed.methods.iter().position(|(listed, ..)| *listed == id) {
                        Some(at) if moved && at > 0 => changed.methods.swap(at - 1, at),
                        Some(at) if !moved && changed.methods.len() > 1 => {
                            changed.methods.remove(at);
                        }
                        _ => {}
                    }
                    save_input_methods(&group, &add, &rows, &bus, changed);
                }
            ));
        }
        row.add_suffix(&up);
        row.add_suffix(&remove);
        row.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let dragged = id.clone();
        let grabbed = Rc::new(Cell::new((0, 0)));
        drag.connect_prepare(glib::clone!(
            #[strong]
            grabbed,
            move |_, x, y| {
                grabbed.set((x as i32, y as i32));
                Some(gdk::ContentProvider::for_value(&dragged.to_value()))
            }
        ));
        // The row is drawn anew on a card of its own to follow the pointer, as GNOME's
        // Settings does: a picture of the row itself would be clear where the list is.
        let name = name.clone();
        drag.connect_drag_begin(glib::clone!(
            #[weak]
            row,
            move |_, drag| {
                let copy = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&name))
                    .build();
                copy.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
                let card = gtk::ListBox::builder().css_classes(["boxed-list"]).build();
                card.set_size_request(row.width(), row.height());
                card.append(&copy);
                gtk::DragIcon::for_drag(drag).set_child(Some(&card));
                let (x, y) = grabbed.get();
                drag.set_hotspot(x, y);
            }
        ));
        row.add_controller(drag);
        // A row dropped on another takes its place.
        let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        let target = id.clone();
        drop.connect_drop(glib::clone!(
            #[weak]
            group,
            #[weak]
            add,
            #[strong]
            rows,
            #[strong]
            bus,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Ok(dragged) = value.get::<String>() else {
                    return false;
                };
                let Some(mut changed) = input_methods(&bus) else {
                    return false;
                };
                let at = |id: &str| changed.methods.iter().position(|(listed, ..)| listed == id);
                let (Some(from), Some(to)) = (at(&dragged), at(&target)) else {
                    return false;
                };
                let moved = changed.methods.remove(from);
                changed.methods.insert(to, moved);
                // The rows are made again once the drop is over, not while it runs on one.
                let (rows, bus) = (rows.clone(), bus.clone());
                glib::idle_add_local_once(move || save_input_methods(&group, &add, &rows, &bus, changed));
                true
            }
        ));
        row.add_controller(drop);
        group.add(&row);
        rows.borrow_mut().push(row);
    }
    group.add(add);
}

/// Fcitx's input methods not in the group, those of a language before its keyboard
/// layouts; clicking one or Enter adds it.
fn input_method_picker(
    parent: &adw::ActionRow,
    bus: &gio::DBusConnection,
    current: InputMethods,
    picked: impl Fn(String, String) + 'static,
) {
    let mut available: Vec<(String, String)> = fcitx(bus, "AvailableInputMethods", None, "(a(ssssssb))")
        .map(|reply| {
            reply
                .child_value(0)
                .iter()
                .map(|method| {
                    let text = |at| method.child_value(at).str().unwrap_or_default().to_string();
                    (text(0), input_method_name(&text(2), &text(1)))
                })
                .filter(|(id, _)| !current.methods.iter().any(|(added, ..)| added == id))
                .collect()
        })
        .unwrap_or_default();
    available.sort_by_cached_key(|(id, name)| (id.starts_with("keyboard-"), name.to_lowercase()));
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    for (_, name) in &available {
        list.append(
            &gtk::Label::builder()
                .label(name.as_str())
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .margin_top(4)
                .margin_bottom(4)
                .build(),
        );
    }
    list.set_placeholder(Some(
        &gtk::Label::builder()
            .label("沒有符合的輸入法")
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build(),
    ));
    let available = Rc::new(available);
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋").build();
    list.set_filter_func(glib::clone!(
        #[strong]
        available,
        #[weak]
        search,
        #[upgrade_or]
        true,
        move |row| {
            let wanted = search.text().to_lowercase();
            let (id, name) = &available[row.index() as usize];
            name.to_lowercase().contains(&wanted) || id.contains(&wanted)
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[weak]
        list,
        move |_| list.invalidate_filter()
    ));
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(24)
        .build();
    body.append(&search);
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .css_classes(["card"])
            .overflow(gtk::Overflow::Hidden)
            .child(&list)
            .build(),
    );
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("新增輸入法")
        .content_width(440)
        .content_height(560)
        .child(&view)
        .build();
    list.connect_row_activated(glib::clone!(
        #[weak]
        dialog,
        move |_, row| {
            if let Some((id, name)) = available.get(row.index() as usize) {
                picked(id.clone(), name.clone());
            }
            dialog.close();
        }
    ));
    search.set_key_capture_widget(Some(&dialog));
    dialog.present(Some(parent));
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

fn sound_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let shown = Rc::new(SoundPage::default());
    fill_sound_page(&page, shown.clone());
    follow_devices(&page, shown);
    page
}

/// Puts the devices on the page again when one comes or goes, as `pw-dump --monitor` says
/// PipeWire changed: once it has been quiet for half a second, so that a restarting
/// WirePlumber, whose devices go and come back, is looked at only once it is back. A
/// restarted PipeWire ends the monitor, which starts again while the page is there.
fn follow_devices(page: &adw::PreferencesPage, shown: Rc<SoundPage>) {
    let argv = ["pw-dump", "--monitor", "--no-colors"].map(std::ffi::OsStr::new);
    let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("pw-dump --monitor: {err}");
            return;
        }
    };
    let Some(changes) = child.stdout_pipe() else { return };
    SOUND_MONITOR.set(Some(child.clone()));
    let weak = page.downgrade();
    let quiet: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    glib::spawn_future_local(async move {
        while let Ok(bytes) = changes.read_bytes_future(64 * 1024, glib::Priority::DEFAULT).await {
            let Some(page) = weak.upgrade().filter(|_| !bytes.is_empty()) else { break };
            if let Some(waiting) = quiet.take() {
                waiting.remove();
            }
            let (shown, quiet_then) = (shown.clone(), quiet.clone());
            let waiting = glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                quiet_then.take();
                let dump = output("pw-dump", &[]).unwrap_or_default();
                let nodes = parse_audio_nodes(&dump);
                if page_names(&nodes, &app_streams(&dump)) != *shown.devices.borrow() {
                    fill_sound_page(&page, shown);
                } else {
                    show_status(&shown, &nodes, default_clock(&dump));
                }
            });
            *quiet.borrow_mut() = Some(waiting);
        }
        child.force_exit();
        if let Some(page) = weak.upgrade() {
            glib::timeout_add_seconds_local_once(1, move || follow_devices(&page, shown));
        }
    });
}

/// What the sound page shows: its groups, and the devices they were made for.
#[derive(Default)]
struct SoundPage {
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    devices: RefCell<Vec<(String, String)>>,
    /// The rows whose subtitles say what is going on: PipeWire's clock, and each card's.
    clock: RefCell<Option<adw::ActionRow>>,
    cards: RefCell<Vec<(u64, adw::ComboRow)>>,
}

/// Which devices there are, by kind and name, to tell when one comes or goes.
fn device_names(nodes: &[AudioDevice]) -> Vec<(String, String)> {
    let mut names: Vec<_> = nodes.iter().map(|device| (device.class.clone(), device.name.clone())).collect();
    names.sort();
    names
}

/// Puts each kind of sound device on the page, again after the default one changes and when
/// a device comes or goes.
fn fill_sound_page(page: &adw::PreferencesPage, shown: Rc<SoundPage>) {
    for group in shown.groups.take() {
        page.remove(&group);
    }
    let weak = page.downgrade();
    let again = {
        let shown = shown.clone();
        Rc::new(move || {
            if let Some(page) = weak.upgrade() {
                let shown = shown.clone();
                // Not while the row that changed is still handling it.
                glib::idle_add_local_once(move || fill_sound_page(&page, shown));
            }
        }) as Rc<dyn Fn()>
    };
    let dump = output("pw-dump", &[]).unwrap_or_default();
    let (nodes, cards) = (parse_audio_nodes(&dump), parse_sound_cards(&dump));
    let streams = app_streams(&dump);
    *shown.devices.borrow_mut() = page_names(&nodes, &streams);
    let clock_group = group("");
    let clock = adw::ActionRow::builder().title("PipeWire").build();
    clock_group.add(&clock);
    page.add(&clock_group);
    shown.groups.borrow_mut().push(clock_group);
    shown.clock.replace(Some(clock));
    for (title, class, default, none) in [
        ("輸出", "Audio/Sink", SINK, "沒有音訊輸出"),
        ("輸入", "Audio/Source", SOURCE, "沒有音訊輸入"),
    ] {
        let devices = nodes.iter().filter(|device| device.class == class).cloned().collect();
        let group = device_group(title, default, devices, none, again.clone());
        page.add(&group);
        shown.groups.borrow_mut().push(group);
    }
    if let Some(group) = app_volume_group(&streams) {
        page.add(&group);
        shown.groups.borrow_mut().push(group);
    }
    let outputs: Vec<AudioDevice> = nodes
        .iter()
        .filter(|device| device.class == "Audio/Sink" && device.name != MULTI_OUTPUT)
        .cloned()
        .collect();
    let mut more = vec![rates_group()];
    if !cards.is_empty() {
        more.insert(0, profile_group(cards, &shown));
    }
    if outputs.len() >= 2 {
        more.insert(0, multi_output_group(&outputs, again));
    }
    for group in more {
        page.add(&group);
        shown.groups.borrow_mut().push(group);
    }
    show_status(&shown, &nodes, default_clock(&dump));
}

/// Says under each card how its devices are open, or that it is idle, and under PipeWire
/// the rate and buffer it runs with: `clock`, its default, until `pw-top` has said, a second
/// later, what it runs with now.
fn show_status(shown: &SoundPage, nodes: &[AudioDevice], clock: Option<(u64, u64)>) {
    for (card, row) in shown.cards.borrow().iter() {
        let open: Vec<&str> =
            nodes.iter().filter(|node| node.card == Some(*card)).filter_map(|node| node.open_as.as_deref()).collect();
        row.set_subtitle(&if open.is_empty() { "閒置".to_string() } else { open.join(" · ") });
    }
    let Some(row) = shown.clock.borrow().clone() else { return };
    row.set_subtitle(&clock.map(clock_text).unwrap_or_default());
    let argv = ["pw-top", "--batch-mode", "--iterations", "2"].map(std::ffi::OsStr::new);
    let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("pw-top: {err}");
            return;
        }
    };
    let row = row.downgrade();
    glib::spawn_future_local(async move {
        match child.communicate_utf8_future(None).await {
            Ok((Some(text), _)) => {
                if let (Some(row), Some(clock)) = (row.upgrade(), running_clock(&text)) {
                    row.set_subtitle(&clock_text(clock));
                }
            }
            Ok((None, _)) => {}
            Err(err) => eprintln!("pw-top: {err}"),
        }
    });
}

/// A rate and a buffer, in samples: "48000 Hz · 緩衝區 1024 個取樣（21.3 毫秒）".
fn clock_text((rate, quantum): (u64, u64)) -> String {
    let ms = quantum as f64 * 1000.0 / rate.max(1) as f64;
    format!("{rate} Hz · 緩衝區 {quantum} 個取樣（{ms:.1} 毫秒）")
}

/// The rate and buffer PipeWire runs with when nothing asks for others, from its settings in
/// `pw-dump`'s output.
fn default_clock(dump: &str) -> Option<(u64, u64)> {
    let objects: Vec<serde_json::Value> = serde_json::from_str(dump).ok()?;
    let settings = objects.iter().find(|object| object["props"]["metadata.name"] == "settings")?;
    let value = |key: &str| {
        let entry = settings["metadata"].as_array()?.iter().find(|entry| entry["key"] == key)?;
        let value = &entry["value"];
        value.as_u64().or_else(|| value.as_str()?.parse().ok())
    };
    Some((value("clock.rate")?, value("clock.quantum")?))
}

/// The rate and buffer the first device driving PipeWire runs with, in `pw-top`'s last
/// look; None when nothing plays or records. A driver's line is the one not marked as
/// following another with "+" or "=".
fn running_clock(text: &str) -> Option<(u64, u64)> {
    let last = text.rsplit_once("QUANT")?.1;
    last.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"R") || fields.iter().any(|field| *field == "+" || *field == "=") {
            return None;
        }
        Some((fields.get(3)?.parse().ok()?, fields.get(2)?.parse().ok()?))
    })
}

/// What PipeWire knows the output that plays on several at once by.
const MULTI_OUTPUT: &str = "nekoland_multi_output";

fn pipewire_conf(name: &str) -> PathBuf {
    home().join(".config/pipewire/pipewire.conf.d").join(name)
}

/// Writes `text` to `path`, or removes it when `text` is None, gone already or not.
fn write_or_remove(path: &Path, text: Option<String>) -> std::io::Result<()> {
    match text {
        Some(text) => {
            std::fs::create_dir_all(path.parent().unwrap_or(path)).and_then(|()| std::fs::write(path, text))
        }
        None => std::fs::remove_file(path).or_else(|err| match err.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        }),
    }
}

/// The outputs that play together as one, as macOS's multi-output device: PipeWire's combine
/// stream, which there is while two or more are chosen.
fn multi_output_group(outputs: &[AudioDevice], changed: Rc<dyn Fn()>) -> adw::PreferencesGroup {
    let group = group("多重輸出裝置");
    let path = pipewire_conf("51-nekoland-multi-output.conf");
    // Those chosen that are not plugged in stay chosen.
    let members = Rc::new(RefCell::new(match std::fs::read_to_string(&path) {
        Ok(text) => multi_output_members(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            Vec::new()
        }
    }));
    for device in outputs {
        let row = adw::SwitchRow::builder()
            .title(&device.description)
            .active(members.borrow().contains(&device.name))
            .build();
        let (members, changed, path) = (members.clone(), changed.clone(), path.clone());
        let name = device.name.clone();
        row.connect_active_notify(move |row| {
            let mut members = members.borrow_mut();
            let was = members.len() >= 2;
            members.retain(|member| *member != name);
            if row.is_active() {
                members.push(name.clone());
            }
            let is = members.len() >= 2;
            if !was && !is {
                return;
            }
            if let Err(err) = write_or_remove(&path, is.then(|| multi_output_conf(&members))) {
                eprintln!("{}: {err}", path.display());
                return;
            }
            // PipeWire loads its modules only as it starts.
            run("systemctl", &["--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"]);
            when_multi_output_is(is, changed.clone());
        });
        group.add(&row);
    }
    group
}

/// Calls `then` once PipeWire is back with the multi-output device there or not, or after
/// five seconds.
fn when_multi_output_is(there: bool, then: Rc<dyn Fn()>) {
    let mut tries = 0;
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        tries += 1;
        let nodes = audio_nodes();
        let back = nodes.iter().any(|device| device.class == "Audio/Sink" && device.name != MULTI_OUTPUT);
        if (back && nodes.iter().any(|device| device.name == MULTI_OUTPUT) == there) || tries == 25 {
            then();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

fn multi_output_conf(members: &[String]) -> String {
    let matches: Vec<String> = members
        .iter()
        .map(|name| format!("            {{ media.class = \"Audio/Sink\" node.name = {} }}\n", quoted(name)))
        .collect();
    format!(
        "# Written by Nekoland's Settings.\n\
         context.modules = [\n  {{\n    name = libpipewire-module-combine-stream\n    args = {{\n      \
         combine.mode = sink\n      node.name = \"{MULTI_OUTPUT}\"\n      \
         node.description = \"多重輸出裝置\"\n      combine.latency-compensate = true\n      \
         combine.props = {{ audio.position = [ FL FR ] }}\n      stream.rules = [\n        {{\n          \
         matches = [\n{}          ]\n          actions = {{ create-stream = {{ }} }}\n        }}\n      \
         ]\n    }}\n  }}\n]\n",
        matches.concat()
    )
}

/// The outputs `multi_output_conf` was written with.
fn multi_output_members(conf: &str) -> Vec<String> {
    conf.lines()
        .filter_map(|line| {
            line.trim().strip_prefix("{ media.class = \"Audio/Sink\" node.name = ")?.strip_suffix(" }")
        })
        .filter_map(|name| serde_json::from_str(name).ok())
        .collect()
}

/// The value `pw-metadata` prints for a key: "update: id:0 key:'clock.rate' value:'48000' type:''".
fn metadata_value(text: &str) -> Option<String> {
    let value = text.lines().find_map(|line| line.split_once(" value:'"))?.1;
    Some(value.split_once('\'')?.0.to_string())
}

/// A string as PipeWire's configuration, a kind of JSON, takes it.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// Sample rates PipeWire may run at, following what plays, instead of always its default;
/// set at once, and kept for when PipeWire starts again.
fn rates_group() -> adw::PreferencesGroup {
    let group = group("");
    let path = pipewire_conf("51-nekoland-rates.conf");
    let row = adw::SwitchRow::builder().title("取樣率跟隨播放內容").active(path.exists()).build();
    row.connect_active_notify(move |row| {
        let rates: Vec<String> = COMMON_RATES.iter().map(u32::to_string).collect();
        let rates = format!("[ {} ]", rates.join(" "));
        let conf = row.is_active().then(|| {
            format!(
                "# Written by Nekoland's Settings.\n\
                 context.properties = {{ default.clock.allowed-rates = {rates} }}\n"
            )
        });
        if let Err(err) = write_or_remove(&path, conf) {
            eprintln!("{}: {err}", path.display());
            return;
        }
        // Off, it is the default rate alone, as PipeWire has it unless told otherwise; not
        // deleted, which PipeWire 1.6 crashes on.
        let rates = if row.is_active() {
            rates
        } else {
            let rate = output("pw-metadata", &["-n", "settings", "0", "clock.rate"])
                .and_then(|text| metadata_value(&text))
                .unwrap_or_else(|| "48000".into());
            format!("[ {rate} ]")
        };
        run("pw-metadata", &["-n", "settings", "0", "clock.allowed-rates", &rates]);
    });
    group.add(&row);
    group
}

/// One kind of sound device: which is the default, its volume and whether it is muted, and
/// the format a sound card is opened with.
fn device_group(
    title: &str,
    default: &'static str,
    devices: Vec<AudioDevice>,
    none: &str,
    changed: Rc<dyn Fn()>,
) -> adw::PreferencesGroup {
    let group = group(title);
    let Some(current) = devices.iter().find(|device| device.default).or(devices.first()).cloned() else {
        group.add(&adw::ActionRow::builder().title(none).build());
        return group;
    };
    let names: Vec<&str> = devices.iter().map(|device| device.description.as_str()).collect();
    let chosen = adw::ComboRow::builder()
        .title("裝置")
        .model(&gtk::StringList::new(&names))
        .selected(devices.iter().position(|device| device.name == current.name).unwrap_or(0) as u32)
        .build();
    chosen.connect_selected_notify(move |row| {
        let Some(name) = devices.get(row.selected() as usize).map(|device| &device.name) else {
            return;
        };
        // Looked up again: restarting WirePlumber numbers the devices anew.
        match audio_nodes().into_iter().find(|device| device.name == *name) {
            Some(device) => run("wpctl", &["set-default", &device.id.to_string()]),
            None => eprintln!("{name}: no longer there"),
        }
        changed();
    });
    group.add(&chosen);

    let (level, muted) = volume(default).unwrap_or_default();
    group.add(&scale_row(
        "音量",
        f64::from(level.round()),
        (0.0, 100.0, 5.0),
        |level| format!("{level:.0}%"),
        move |level| run("wpctl", &["set-volume", default, &format!("{:.2}", level / 100.0)]),
    ));
    let mute = adw::SwitchRow::builder().title("靜音").active(muted).build();
    mute.connect_active_notify(move |row| {
        run("wpctl", &["set-mute", default, if row.is_active() { "1" } else { "0" }])
    });
    group.add(&mute);

    if current.alsa {
        add_format_rows(&group, current);
    }
    group
}

/// The programs playing sound, as PipeWire's playback streams: the stream's ID and the name
/// of the program.
fn app_streams(dump: &str) -> Vec<(u32, String)> {
    let objects: Vec<serde_json::Value> = serde_json::from_str(dump).unwrap_or_default();
    objects
        .iter()
        .filter(|object| object["info"]["props"]["media.class"] == PLAYBACK_STREAM)
        .filter_map(|object| {
            let props = &object["info"]["props"];
            let name = props["application.name"].as_str().or(props["node.name"].as_str())?;
            Some((object["id"].as_u64()? as u32, name.to_string()))
        })
        .collect()
}

/// What the page was made for: the devices, and the programs playing, to tell when one
/// comes or goes.
fn page_names(nodes: &[AudioDevice], streams: &[(u32, String)]) -> Vec<(String, String)> {
    let mut names = device_names(nodes);
    names.extend(streams.iter().map(|(_, name)| (PLAYBACK_STREAM.to_string(), name.clone())));
    names
}

/// A volume for each program that is playing sound, as Windows' mixer has; none while
/// nothing plays. The page is made again as a program starts or stops.
fn app_volume_group(streams: &[(u32, String)]) -> Option<adw::PreferencesGroup> {
    if streams.is_empty() {
        return None;
    }
    let apps = group("應用程式");
    for (id, name) in streams {
        let id = id.to_string();
        let (level, _) = volume(&id).unwrap_or_default();
        apps.add(&scale_row(
            name,
            f64::from(level.round()),
            (0.0, 100.0, 5.0),
            |level| format!("{level:.0}%"),
            move |level| run("wpctl", &["set-volume", &id, &format!("{:.2}", level / 100.0)]),
        ));
    }
    Some(apps)
}

/// Bit depths a sound card may be opened with, as PipeWire names them.
const SAMPLE_FORMATS: [(&str, &str); 6] = [
    ("U8", "8 位元"),
    ("S16LE", "16 位元"),
    ("S24LE", "24 位元"),
    ("S24_32LE", "24 位元"),
    ("S32LE", "32 位元"),
    ("F32LE", "32 位元浮點"),
];

/// The sample rate and bit depth a sound card is opened with, each "自動" to leave it to
/// PipeWire, which resamples between it and the other devices.
fn add_format_rows(group: &adw::PreferencesGroup, device: AudioDevice) {
    let mut formats: Vec<(&str, &str)> =
        SAMPLE_FORMATS.into_iter().filter(|(name, _)| device.formats.iter().any(|f| f == name)).collect();
    // A card with both kinds of 24 bits is opened with the first.
    formats.dedup_by_key(|(_, label)| *label);
    let rates = device.rates.clone();
    let chosen = Rc::new(RefCell::new((device.rate, device.format.clone())));

    let mut labels = vec!["自動".to_string()];
    labels.extend(rates.iter().map(|rate| format!("{} kHz", f64::from(*rate) / 1000.0)));
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let rate_row = adw::ComboRow::builder()
        .title("取樣率")
        .model(&gtk::StringList::new(&labels))
        .selected(rates.iter().position(|rate| *rate == device.rate).map_or(0, |at| at as u32 + 1))
        .build();
    let name = device.name.clone();
    let rate_chosen = chosen.clone();
    rate_row.connect_selected_notify(move |row| {
        let rate = (row.selected() as usize).checked_sub(1).and_then(|at| rates.get(at));
        rate_chosen.borrow_mut().0 = rate.copied().unwrap_or(0);
        let (rate, format) = &*rate_chosen.borrow();
        save_device_format(&name, *rate, format);
    });
    group.add(&rate_row);

    let mut labels = vec!["自動"];
    labels.extend(formats.iter().map(|(_, label)| *label));
    let depth_row = adw::ComboRow::builder()
        .title("位元深度")
        .model(&gtk::StringList::new(&labels))
        .selected(formats.iter().position(|(name, _)| *name == device.format).map_or(0, |at| at as u32 + 1))
        .build();
    depth_row.connect_selected_notify(move |row| {
        let format = (row.selected() as usize).checked_sub(1).and_then(|at| formats.get(at));
        chosen.borrow_mut().1 = format.map(|(name, _)| name.to_string()).unwrap_or_default();
        let (rate, format) = &*chosen.borrow();
        save_device_format(&device.name, *rate, format);
    });
    group.add(&depth_row);
}

/// Keeps the format a sound card is opened with as a WirePlumber rule of its own, none when
/// both are left to PipeWire, and restarts WirePlumber, which reads its rules only then.
fn save_device_format(name: &str, rate: u32, format: &str) {
    let path = home()
        .join(".config/wireplumber/wireplumber.conf.d")
        .join(format!("51-nekoland-{}.conf", name.replace('/', "_")));
    let mut props = Vec::new();
    if rate != 0 {
        props.push(format!("audio.rate = {rate}"));
    }
    if !format.is_empty() {
        props.push(format!("audio.format = {}", quoted(format)));
    }
    let rule = (!props.is_empty()).then(|| {
        format!(
            "# Written by Nekoland's Settings.\n\
             monitor.alsa.rules = [\n  {{\n    matches = [ {{ node.name = {name} }} ]\n    \
             actions = {{ update-props = {{ {} }} }}\n  }}\n]\n",
            props.join(", "),
            name = quoted(name),
        )
    });
    match write_or_remove(&path, rule) {
        Ok(()) => run("systemctl", &["--user", "restart", "wireplumber"]),
        Err(err) => eprintln!("{}: {err}", path.display()),
    }
}

#[derive(Clone)]
struct AudioDevice {
    id: u32,
    /// What PipeWire knows it by, which stays the same across restarts.
    name: String,
    description: String,
    /// "Audio/Sink" or "Audio/Source".
    class: String,
    default: bool,
    /// Whether it is a sound card, whose format can be chosen.
    alsa: bool,
    /// The sample rates and formats it can be opened with, and the ones it is set to: 0 and
    /// "" for PipeWire's choice.
    rates: Vec<u32>,
    formats: Vec<String>,
    rate: u32,
    format: String,
    /// The sound card it is on, and how it is open now: "S16LE 48000 Hz 2ch".
    card: Option<u64>,
    open_as: Option<String>,
}

/// A sound card, and the profiles it can be used with, as PipeWire describes them: stereo,
/// surround, Pro Audio or off.
struct SoundCard {
    id: u64,
    description: String,
    /// By index, and the one it is in.
    profiles: Vec<(u64, String)>,
    profile: u64,
}

/// The sound cards in `pw-dump`'s output; one it cannot read, `parse_audio_nodes` says so.
fn parse_sound_cards(dump: &str) -> Vec<SoundCard> {
    use serde_json::Value;
    let objects: Vec<Value> = serde_json::from_str(dump).unwrap_or_default();
    objects
        .iter()
        .filter(|object| object["info"]["props"]["media.class"] == "Audio/Device")
        .filter_map(|object| {
            let (info, props) = (&object["info"], &object["info"]["props"]);
            let current = info["params"]["Profile"][0]["index"].as_u64()?;
            // Those PipeWire says cannot be used, unless it is in one.
            let profiles = info["params"]["EnumProfile"]
                .as_array()?
                .iter()
                .filter(|profile| profile["available"] != "no" || profile["index"] == current)
                .filter_map(|profile| {
                    Some((profile["index"].as_u64()?, profile["description"].as_str()?.into()))
                })
                .collect();
            Some(SoundCard {
                id: object["id"].as_u64()?,
                description: props["device.description"].as_str().or(props["device.name"].as_str())?.into(),
                profiles,
                profile: current,
            })
        })
        .collect()
}

/// Each sound card's profile, as macOS's Audio MIDI Setup and pavucontrol pick it.
fn profile_group(cards: Vec<SoundCard>, shown: &SoundPage) -> adw::PreferencesGroup {
    let group = group("裝置模式");
    let mut rows = Vec::new();
    for card in cards {
        let names: Vec<&str> = card.profiles.iter().map(|(_, name)| name.as_str()).collect();
        let row = adw::ComboRow::builder()
            .title(&card.description)
            .model(&gtk::StringList::new(&names))
            .selected(card.profiles.iter().position(|(index, _)| *index == card.profile).unwrap_or(0) as u32)
            .build();
        rows.push((card.id, row.clone()));
        // The page follows as the card's devices change.
        row.connect_selected_notify(move |row| {
            if let Some((index, _)) = card.profiles.get(row.selected() as usize) {
                run("wpctl", &["set-profile", &card.id.to_string(), &index.to_string()]);
            }
        });
        group.add(&row);
    }
    shown.cards.replace(rows);
    group
}

fn audio_nodes() -> Vec<AudioDevice> {
    output("pw-dump", &[]).map(|dump| parse_audio_nodes(&dump)).unwrap_or_default()
}

/// Sample rates offered when a card takes any in a range.
const COMMON_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// The audio devices in `pw-dump`'s output.
fn parse_audio_nodes(dump: &str) -> Vec<AudioDevice> {
    use serde_json::Value;
    let objects: Vec<Value> = match serde_json::from_str(dump) {
        Ok(objects) => objects,
        Err(err) => {
            eprintln!("pw-dump: {err}");
            return Vec::new();
        }
    };
    // The defaults are kept by name, as metadata.
    let default_of = |key: &str| {
        objects
            .iter()
            .filter(|object| object["props"]["metadata.name"] == "default")
            .flat_map(|object| object["metadata"].as_array().into_iter().flatten())
            .find(|entry| entry["key"] == key)
            .and_then(|entry| entry["value"]["name"].as_str())
            .map(String::from)
    };
    let defaults = [default_of("default.audio.sink"), default_of("default.audio.source")];
    // A choice is a plain value, or {"default", "alt1", ...}, or {"default", "min", "max"}.
    let choices = |value: &Value| -> Vec<Value> {
        match value.as_object() {
            None => vec![value.clone()],
            Some(choice) => {
                choice.iter().filter(|(key, _)| *key != "default").map(|(_, value)| value.clone()).collect()
            }
        }
    };
    let mut devices = Vec::new();
    for object in &objects {
        let info = &object["info"];
        let props = &info["props"];
        let class = props["media.class"].as_str().unwrap_or_default();
        if class != "Audio/Sink" && class != "Audio/Source" {
            continue;
        }
        let (Some(id), Some(name)) = (object["id"].as_u64(), props["node.name"].as_str()) else {
            continue;
        };
        let (mut rates, mut formats) = (Vec::new(), Vec::new());
        for format in info["params"]["EnumFormat"].as_array().into_iter().flatten() {
            let rate = &format["rate"];
            match (rate["min"].as_u64(), rate["max"].as_u64()) {
                (Some(min), Some(max)) => {
                    rates.extend(COMMON_RATES.into_iter().filter(|r| (min..=max).contains(&u64::from(*r))))
                }
                _ => rates.extend(choices(rate).iter().filter_map(Value::as_u64).map(|r| r as u32)),
            }
            formats.extend(choices(&format["format"]).iter().filter_map(Value::as_str).map(String::from));
        }
        rates.sort_unstable();
        rates.dedup();
        formats.sort_unstable();
        formats.dedup();
        devices.push(AudioDevice {
            id: id as u32,
            name: name.into(),
            description: props["node.description"].as_str().unwrap_or(name).into(),
            class: class.into(),
            default: defaults.iter().flatten().any(|default| default == name),
            alsa: props["device.api"] == "alsa",
            rates,
            formats,
            rate: props["audio.rate"].as_u64().unwrap_or(0) as u32,
            format: props["audio.format"].as_str().unwrap_or_default().into(),
            card: props["device.id"].as_u64(),
            open_as: info["params"]["Format"].get(0).and_then(|open| {
                let (format, rate, channels) =
                    (open["format"].as_str()?, open["rate"].as_u64()?, open["channels"].as_u64()?);
                Some(format!("{format} {rate} Hz {channels}ch"))
            }),
        });
    }
    devices
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
                set(&format!("terminal = {:?}", app.executable().to_string_lossy()));
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

/// Where Settings keeps the `~/.xsession` there was before it gave xrdp sessions Nekoland.
const SAVED_XSESSION_FILE: &str = "nekoland/xsession";
/// How an `~/.xsession` Settings wrote begins, to tell it from one written by hand.
const XSESSION_HEAD: &str = "# Written by Nekoland's Settings, for xrdp sessions.\n";
const XRDP_INI: &str = "/etc/xrdp/xrdp.ini";
const RDP_PORT: u16 = 3389;

/// What other computers may reach here: the desktop over RDP, through xrdp, with the session
/// they get, and a shell over SSH.
fn sharing_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let rows = group("");
    let service = adw::SwitchRow::builder().title("遠端桌面").build();
    let address = fact_row("位址", &remote_address());
    let port = std::fs::read_to_string(XRDP_INI).map_or(RDP_PORT, |ini| xrdp_port(&ini));
    let port = fact_row("連接埠", &port.to_string());
    let session = adw::ComboRow::builder()
        .title("遠端工作階段")
        .model(&gtk::StringList::new(&["Nekoland", "系統預設"]))
        .selected(if xsession_is_nekoland() { 0 } else { 1 })
        .build();
    rows.add(&service);
    rows.add(&address);
    rows.add(&port);
    rows.add(&session);
    page.add(&rows);

    // Set while the switch is made to show xrdp as it is, not turned by the person.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        service,
        #[weak]
        address,
        #[weak]
        port,
        #[strong]
        showing,
        move || {
            let running = xrdp_running();
            showing.set(true);
            service.set_active(running == Some(true));
            showing.set(false);
            // Turned off from an xrdp session, it would end the connection it is changed over.
            let remote = std::env::var_os("XRDP_SESSION").is_some();
            service.set_sensitive(running.is_some() && !remote);
            service.set_subtitle(match running {
                None => "未安裝 xrdp",
                Some(_) if remote => "遠端連線中",
                Some(_) => "",
            });
            address.set_visible(running == Some(true));
            port.set_visible(running == Some(true));
        }
    ));
    show();
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let (verb, reason) = if row.is_active() {
            ("enable", "開啟遠端桌面")
        } else {
            ("disable", "關閉遠端桌面")
        };
        row.set_sensitive(false);
        let show = show.clone();
        match sudo(reason, &["systemctl", verb, "--now", "xrdp"], gio::SubprocessFlags::NONE) {
            // Turned down or failed, the switch goes back to what xrdp is.
            Ok(sudo) => sudo.wait_check_async(gio::Cancellable::NONE, move |result| {
                if let Err(err) = result {
                    eprintln!("systemctl {verb} xrdp: {err}");
                }
                show();
            }),
            Err(err) => {
                eprintln!("sudo: {err}");
                show();
            }
        }
    });
    session.connect_selected_notify(|row| {
        if let Err(err) = set_xsession(row.selected() == 0) {
            eprintln!("~/.xsession: {err}");
        }
    });
    if let Some(ssh) = ssh_units() {
        page.add(&remote_login_group(ssh));
    }
    page.add(&file_sharing_group(&page));
    page
}

/// Turns on Samba and shares the Public folder as `$3`, with the account's own
/// Samba password: Samba keeps its own, so it is asked for as sharing is turned on.
const SHARE_FOLDER: &str = r#"set -e
usermod -aG sambashare "$1"
smbpasswd -a -s "$1"
# Ubuntu's smb.conf leaves the users' own shares off, its line for them commented out.
grep -Eq '^[[:space:]]*usershare max shares' /etc/samba/smb.conf ||
    sed -i '/^\[global\]/a\   usershare max shares = 100' /etc/samba/smb.conf
systemctl enable --now smbd
runuser -u "$1" -g sambashare -- net usershare add "$3" "$2" "" Everyone:F guest_ok=n
"#;

/// 檔案共享: the Public folder over SMB, as the Mac and Windows share it.
fn file_sharing_group(page: &adw::PreferencesPage) -> adw::PreferencesGroup {
    let sharing = group("");
    let user = std::env::var("USER").unwrap_or_default();
    // Installing Samba starts it; sharing is on once the folder was shared, which put the
    // person in `sambashare`, whose share list only its members can read.
    let state = glib::clone!(
        #[strong]
        user,
        move || {
            let shown = output("systemctl", &["show", "smbd", "--property=LoadState,ActiveState"])?;
            let value = |key: &str| field(&shown, key, '=');
            let member = output("getent", &["group", "sambashare"])
                .is_some_and(|line| line.trim().rsplit(':').next().is_some_and(|members| members.split(',').any(|m| m == user)));
            (value("LoadState").as_deref() == Some("loaded"))
                .then(|| member && value("ActiveState").as_deref() == Some("active"))
        }
    );
    let folder = glib::user_special_dir(glib::UserDirectory::PublicShare)
        .unwrap_or_else(|| glib::home_dir().join("Public"));
    // Samba takes no share named as a user.
    let share = format!("{}-{user}", folder.file_name().unwrap_or_default().to_string_lossy());
    let service = adw::SwitchRow::builder().title("檔案共享").build();
    let shared = fact_row("共享資料夾", &folder.display().to_string());
    sharing.add(&service);
    sharing.add(&shared);
    // Set while the switch is put back to what Samba is.
    let showing = Rc::new(Cell::new(false));
    let show = Rc::new(glib::clone!(
        #[weak]
        service,
        #[weak]
        shared,
        #[strong]
        showing,
        #[strong]
        share,
        move || {
            let running = state();
            showing.set(true);
            service.set_active(running == Some(true));
            showing.set(false);
            service.set_sensitive(running.is_some());
            service.set_subtitle(&match running {
                None => "未安裝 Samba".to_string(),
                Some(true) => format!("smb://{}/{share}", remote_address()),
                Some(false) => String::new(),
            });
            shared.set_visible(running == Some(true));
        }
    ));
    show();
    let page = page.clone();
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        row.set_sensitive(false);
        let show = show.clone();
        if !row.is_active() {
            return as_root("關閉檔案共享", &["systemctl", "disable", "--now", "smbd"], None, move |_| show());
        }
        if let Err(err) = std::fs::create_dir_all(&folder) {
            eprintln!("{}: {err}", folder.display());
            return show();
        }
        let (user, folder, share, page) = (user.clone(), folder.display().to_string(), share.clone(), page.clone());
        let cancelled: Rc<dyn Fn()> = show.clone();
        users::ask_password(&page.clone(), "檔案共享密碼", "開啟", cancelled, move |typed| {
            let args = ["sh", "-c", SHARE_FOLDER, "sh", user.as_str(), folder.as_str(), share.as_str()];
            as_root("開啟檔案共享", &args, Some(format!("{typed}\n{typed}\n")), move |worked| {
                if !worked {
                    alert::notice(&page, "無法開啟檔案共享", None);
                }
                show();
            });
        });
    });
    sharing
}

/// The units that start OpenSSH's server: a socket where it is started on demand, as Ubuntu
/// has it, else the service; None without it.
fn ssh_units() -> Option<&'static str> {
    let loaded = |unit: &str| {
        output("systemctl", &["show", unit, "--property=LoadState"])
            .is_some_and(|shown| field(&shown, "LoadState", '=').as_deref() == Some("loaded"))
    };
    if loaded("ssh.socket") {
        Some("ssh.socket")
    } else {
        loaded("ssh.service").then_some("ssh.service")
    }
}

/// 遠端登入, the Mac's name for letting others in over SSH.
fn remote_login_group(unit: &'static str) -> adw::PreferencesGroup {
    let login = group("");
    // `systemctl is-active` fails when any is not, so its lines are read rather than its status.
    let running = || {
        let states = Command::new("systemctl").args(["is-active", "ssh.socket", "ssh.service"]).output();
        states.is_ok_and(|states| String::from_utf8_lossy(&states.stdout).lines().any(|state| state == "active"))
    };
    let service = adw::SwitchRow::builder().title("遠端登入").active(running()).build();
    // What to type on the other computer, while it can be typed.
    let user = std::env::var("USER").unwrap_or_default();
    service
        .bind_property("active", &service, "subtitle")
        .transform_to(move |_, on: bool| Some(if on { format!("ssh {user}@{}", remote_address()) } else { String::new() }))
        .sync_create()
        .build();
    // Set while the switch is put back to what the service is.
    let showing = Rc::new(Cell::new(false));
    service.connect_active_notify(move |row| {
        if showing.get() {
            return;
        }
        let args: &[&str] = if row.is_active() {
            &["systemctl", "enable", "--now", unit]
        } else {
            &["systemctl", "disable", "--now", "ssh.socket", "ssh.service"]
        };
        let reason = if row.is_active() { "開啟遠端登入" } else { "關閉遠端登入" };
        row.set_sensitive(false);
        let (row, showing) = (row.clone(), showing.clone());
        as_root(reason, args, None, move |_| {
            showing.set(true);
            row.set_active(running());
            showing.set(false);
            row.set_sensitive(true);
        });
    });
    login.add(&service);
    login
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
            let program = nekoland_program("nekoland-disk");
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
    let askpass = nekoland_program("nekoland-askpass");
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

/// The absolute path of one of Nekoland's programs, installed beside Settings; sudo takes
/// only an absolute path.
fn nekoland_program(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
        .or_else(|| glib::find_program_in_path(name))
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin").join(name))
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
    // The compositor that started Settings put itself first on the PATH.
    let program = std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("nekoland"))
                .find(|program| program.exists())
        })
        .unwrap_or_else(|| PathBuf::from("nekoland"));
    let program = program.to_string_lossy().replace('\'', "'\\''");
    write_atomic(&path, &format!("{XSESSION_HEAD}exec '{program}'\n"))
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
fn privacy_page() -> adw::PreferencesPage {
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
    page.add(&permissions);
    let history = group("");
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

/// The software updates there are, counted by apt, which Market installs, and whether the
/// system installs security updates by itself.
fn updates_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let nekoland = group("");
    nekoland.add(&selfupdate::row(None));
    page.add(&nekoland);
    let updates = group("");
    let available = adw::ActionRow::builder().title("可用的更新").subtitle("正在檢查").build();
    let open = gtk::Button::builder().label("在 Market 中更新").valign(gtk::Align::Center).build();
    open.connect_clicked(|_| {
        if let Err(err) = Command::new("nekoland-store").env("NEKOLAND_STORE_PAGE", "updates").spawn() {
            eprintln!("nekoland-store: {err}");
        }
    });
    available.add_suffix(&open);
    updates.add(&available);
    match gio::Subprocess::newv(
        &["apt".as_ref(), "list".as_ref(), "--upgradable".as_ref()],
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    ) {
        Ok(apt) => apt.communicate_utf8_async(None, gio::Cancellable::NONE, glib::clone!(
            #[weak]
            available,
            move |result| {
                let count = result.ok().and_then(|(listed, _)| listed).map_or(0, |listed| upgradable(&listed));
                available.set_subtitle(&if count == 0 { "已是最新版本".to_string() } else { format!("{count} 個套件") });
            }
        )),
        Err(err) => {
            eprintln!("apt: {err}");
            available.set_subtitle("無法檢查");
        }
    }

    if Path::new(UNATTENDED).exists() {
        let automatic = adw::SwitchRow::builder()
            .title("自動安裝安全性更新")
            .active(
                std::fs::read_to_string(AUTO_UPGRADES)
                    .is_ok_and(|text| text.contains("Unattended-Upgrade \"1\"")),
            )
            .build();
        automatic.connect_active_notify(|row| {
            let on = if row.is_active() { "1" } else { "0" };
            let script = format!(
                "printf 'APT::Periodic::Update-Package-Lists \"1\";\\nAPT::Periodic::Unattended-Upgrade \"%s\";\\n' \"$1\" > {AUTO_UPGRADES}"
            );
            row.set_sensitive(false);
            let row = row.clone();
            as_root("變更自動更新", &["sh", "-c", &script, "sh", on], None, move |worked| {
                row.set_sensitive(true);
                if !worked {
                    alert::notice(&row, "無法變更自動更新", Some("請確認密碼正確後再試一次。"));
                }
            });
        });
        updates.add(&automatic);
        updates.add(&pause_row());
    }
    page.add(&updates);
    if let Some((history, latest)) = history_group() {
        let more = group("");
        more.add(&subpage_row("更新記錄", move || latest.clone(), page_of_groups(&[history])));
        page.add(&more);
    }
    page
}

/// The shell that takes a pause away: its drop-in, which keeps automatic updates off, and the
/// timer that ends it.
fn end_pause() -> String {
    format!(
        "systemctl disable --now nekoland-resume-updates.timer 2>/dev/null; rm -f {PAUSE_FILE} {RESUME_TIMER} {RESUME_SERVICE}; systemctl daemon-reload"
    )
}

/// The day a pause ends, as the timer it left says.
fn paused_until() -> Option<String> {
    let timer = std::fs::read_to_string(RESUME_TIMER).ok()?;
    timer.lines().find_map(|line| line.strip_prefix("OnCalendar=")).map(str::to_string)
}

/// 暫停更新: automatic updates kept off for some weeks by a drop-in a timer takes away
/// again, so that nothing but the system has to remember.
fn pause_row() -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title("暫停更新")
        .model(&gtk::StringList::new(&["不暫停", "1 週", "2 週", "5 週"]))
        .build();
    let until = |day: Option<String>| day.map(|day| format!("至 {day}")).unwrap_or_default();
    row.set_subtitle(&until(paused_until()));
    row.connect_selected_notify(move |row| {
        let Some(weeks) = PAUSE_WEEKS.get(row.selected() as usize).copied() else { return };
        if weeks == 0 && paused_until().is_none() {
            return;
        }
        let script = if weeks == 0 {
            end_pause()
        } else {
            format!(
                "set -e; day=$(date -d \"+$1 weeks\" +%Y-%m-%d); \
                 printf 'APT::Periodic::Unattended-Upgrade \"0\";\\n' > {PAUSE_FILE}; \
                 printf '[Unit]\\nDescription=End the pause of automatic updates\\n[Service]\\nType=oneshot\\nExecStart=/bin/rm -f {PAUSE_FILE}\\nExecStartPost=/usr/bin/systemctl disable nekoland-resume-updates.timer\\nExecStartPost=/bin/rm -f {RESUME_TIMER} {RESUME_SERVICE}\\n' > {RESUME_SERVICE}; \
                 printf '[Unit]\\nDescription=End the pause of automatic updates\\n[Timer]\\nOnCalendar=%s\\nPersistent=true\\n[Install]\\nWantedBy=timers.target\\n' \"$day\" > {RESUME_TIMER}; \
                 systemctl daemon-reload; systemctl enable --now nekoland-resume-updates.timer"
            )
        };
        row.set_sensitive(false);
        let row = row.clone();
        as_root("暫停更新", &["sh", "-c", &script, "sh", &weeks.to_string()], None, move |worked| {
            row.set_sensitive(true);
            if worked {
                row.set_subtitle(&until(paused_until()));
            } else {
                alert::notice(&row, "無法暫停更新", Some("請確認密碼正確後再試一次。"));
            }
        });
    });
    row
}

/// One run of apt, as its history lists it.
#[derive(Debug, PartialEq)]
struct AptRun {
    date: String,
    installed: usize,
    upgraded: usize,
    removed: usize,
    packages: Vec<String>,
}

/// The runs in apt's history that changed packages, newest first.
fn parse_apt_history(text: &str) -> Vec<AptRun> {
    let names = |list: &str| -> Vec<String> {
        list.split("), ")
            .filter_map(|package| package.split(" (").next())
            .map(|name| name.split(':').next().unwrap_or(name).to_string())
            .collect()
    };
    let mut runs: Vec<AptRun> = text
        .split("\n\n")
        .filter_map(|block| {
            let field = |key: &str| block.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": "));
            let date = field("Start-Date")?.split_whitespace().next()?.to_string();
            let (install, upgrade, remove) = (field("Install"), field("Upgrade"), field("Remove"));
            let count = |list: Option<&str>| list.map_or(0, |list| names(list).len());
            let packages = [install, upgrade, remove].into_iter().flatten().flat_map(names).collect();
            let run = AptRun {
                date,
                installed: count(install),
                upgraded: count(upgrade),
                removed: count(remove),
                packages,
            };
            (run.installed + run.upgraded + run.removed > 0).then_some(run)
        })
        .collect();
    runs.reverse();
    runs
}

/// 更新記錄: what apt installed, upgraded and removed, latest first; none without a history.
/// The latest apt runs, with the newest told in a line.
fn history_group() -> Option<(adw::PreferencesGroup, String)> {
    let runs = parse_apt_history(&std::fs::read_to_string(APT_HISTORY).ok()?);
    if runs.is_empty() {
        return None;
    }
    let history = group("");
    let mut latest = String::new();
    for run in runs.iter().take(HISTORY_ENTRIES) {
        let counts: Vec<String> = [("升級", run.upgraded), ("安裝", run.installed), ("移除", run.removed)]
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|(what, count)| format!("{what} {count} 個"))
            .collect();
        if latest.is_empty() {
            latest = format!("{}・{}", run.date, counts.join("、"));
        }
        let row = adw::ExpanderRow::builder().title(run.date.as_str()).subtitle(counts.join("、")).build();
        for package in &run.packages {
            row.add_row(&adw::ActionRow::builder().title(package.as_str()).build());
        }
        history.add(&row);
    }
    Some((history, latest))
}

/// How many packages `apt list --upgradable` lists, past its "Listing..." line.
fn upgradable(listed: &str) -> usize {
    listed.lines().filter(|line| line.contains("[upgradable from")).count()
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
    for name in notifiers {
        let row = adw::SwitchRow::builder()
            .title(name.as_str())
            .use_markup(false)
            .active(!off.borrow().contains(&name))
            .build();
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

/// The colours of the design language, by token, with their names and what they are for.
const TOKENS: [(&str, &str, &str); 8] = [
    ("paper", "紙", "視窗底色"),
    ("sheet", "頁", "卡片、清單"),
    ("ink", "墨", "文字、選中"),
    ("ink-2", "墨 2", "次要文字"),
    ("ink-3", "墨 3", "說明、停用"),
    ("line", "線", "分隔、外框"),
    ("fill", "填", "輸入框、未選中"),
    ("danger", "警示", "刪除、錯誤"),
];

/// A token's colour as the GTK look sets it, light or dark.
fn token_value(token: &str, dark: bool) -> &'static str {
    let (light, dark_part) = GTK_LOOK
        .split_once("@media (prefers-color-scheme: dark)")
        .unwrap_or((GTK_LOOK, ""));
    let part = if dark { dark_part } else { light };
    let key = format!("--nekoland-{token}: ");
    part.split_once(&key)
        .and_then(|(_, rest)| rest.split_once(';'))
        .map_or("", |(value, _)| value)
}

/// A stylesheet with every rule kept to what is inside `scope`, and what it sets on the
/// root set on `scope` itself: GTK has no way to give a stylesheet to one widget.
fn scoped(css: &str, scope: &str) -> String {
    let mut text = String::new();
    let mut rest = css;
    while let Some((before, comment)) = rest.split_once("/*") {
        text += before;
        rest = comment.split_once("*/").map_or("", |(_, after)| after);
    }
    text += rest;

    let mut out = String::new();
    let mut chunk = String::new();
    let mut in_declarations = false;
    for character in text.chars() {
        match character {
            '{' if !in_declarations => {
                let selectors = chunk.trim();
                // A media query: the rules in it follow.
                if selectors.starts_with('@') {
                    out += selectors;
                } else {
                    let kept: Vec<String> = selectors
                        .split(',')
                        .map(|selector| match selector.trim() {
                            ":root" => scope.to_string(),
                            selector => format!("{scope} {selector}"),
                        })
                        .collect();
                    out += &kept.join(", ");
                    in_declarations = true;
                }
                out += " { ";
                chunk.clear();
            }
            '}' => {
                out += &chunk;
                out += "}\n";
                chunk.clear();
                in_declarations = false;
            }
            _ => chunk.push(character),
        }
    }
    out
}

/// What the design language page is drawn with: Nekoland's look, kept to the page so that
/// it shows whether or not GTK applications have it, and what the page draws that GTK has
/// no widget for.
fn design_css(dark: bool) -> String {
    // Not the rules for sidebars on glass, which are the window's, not the page's.
    let look = look_for(dark);
    let (before, rest) = look
        .split_once("/* Sidebars on glass")
        .expect("the look has its sidebar rules");
    let after = &rest[rest.find("@media").expect("the look has a dark part")..];
    scoped(&format!("{before}{after}"), ".nk-design") + DESIGN_CSS
}

/// The parts of the design GTK has no widget for, and the page's own type and layout.
const DESIGN_CSS: &str = r#"
.nk-design { background-color: var(--nekoland-paper); color: var(--nekoland-ink); }
.nk-h1 { font-size: 32px; font-weight: 700; }
.nk-h2 { font-size: 20px; font-weight: 600; }
.nk-h3 { font-size: 12px; font-weight: 500; letter-spacing: 0.04em; color: var(--nekoland-ink-3); }
.nk-lead { color: var(--nekoland-ink-2); }
.nk-note { font-size: 12px; color: var(--nekoland-ink-3); }
.nk-small { font-size: 12px; color: var(--nekoland-ink-2); }
.nk-mono { font-family: "Ubuntu Sans Mono", monospace; }
/* The cards are laid out in a grid of cells; a cell is not something to point at or choose. */
.nk-design flowbox.nk-cards > flowboxchild, .nk-design flowbox.nk-cards > flowboxchild:hover, .nk-design flowbox.nk-cards > flowboxchild:active, .nk-design flowbox.nk-cards > flowboxchild:selected { padding: 0; background: none; box-shadow: none; outline: none; }
.nk-card { padding: 20px; border-radius: 12px; background-color: var(--nekoland-sheet); box-shadow: 0 0 0 1px var(--nekoland-line); }
.nk-rule { padding: 16px; }
.nk-rule-body { font-size: 13px; color: var(--nekoland-ink-2); }
.nk-swatch { min-height: 64px; border-radius: 8px; box-shadow: inset 0 0 0 1px var(--nekoland-line); }
.nk-type-row { padding: 10px 0; }
.nk-type-row.next { border-top: 1px solid var(--nekoland-line); }
.nk-spec { font-size: 12px; color: var(--nekoland-ink-3); }
.nk-step { background-color: var(--nekoland-ink); border-radius: 2px; }
.nk-corner { min-width: 72px; min-height: 56px; background-color: var(--nekoland-fill); box-shadow: inset 0 0 0 1.5px var(--nekoland-ink); }
.nk-design button.nk-large { min-height: 40px; padding: 0 20px; border-radius: 10px; }
.nk-label { font-size: 12px; font-weight: 500; }
.nk-err { font-size: 12px; color: var(--nekoland-danger); }
.nk-badge { min-height: 20px; padding: 0 8px; border-radius: 99px; font-size: 12px; font-weight: 500; background-color: var(--nekoland-fill); color: var(--nekoland-ink-2); }
.nk-badge.solid { background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); }
.nk-badge.danger { background-color: color-mix(in srgb, var(--nekoland-danger) 14%, transparent); color: var(--nekoland-danger); }
.nk-tip { padding: 4px 8px; border-radius: 6px; background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); font-size: 12px; font-weight: 500; }
.nk-toast { padding: 10px 10px 10px 16px; border-radius: 12px; background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); box-shadow: 0 12px 30px -10px rgb(0 0 0 / 40%); }
.nk-design .nk-toast button { min-height: 28px; background-color: color-mix(in srgb, var(--nekoland-on-ink) 14%, transparent); color: var(--nekoland-on-ink); }
.nk-stage { border-radius: 12px; background: linear-gradient(140deg, #f3b47a, #d4577b 42%, #4b3f8f 78%, #1f2a5c); }
/* As the compositor draws it, by render.rs's BAND_FILL_ALPHA and BAND_EDGE_ALPHA. */
.nk-band { background-color: rgb(255 255 255 / 15%); box-shadow: inset 0 0 0 1px rgb(255 255 255 / 60%); }
.nk-glass { background-color: color-mix(in srgb, var(--nekoland-sheet) 50%, transparent); color: var(--nekoland-ink); box-shadow: inset 0 0 0 1px rgb(255 255 255 / 20%), 0 10px 30px -10px rgb(0 0 0 / 45%); }
.nk-bar { min-height: 30px; padding: 0 10px; border-radius: 12px 12px 0 0; box-shadow: none; font-size: 13px; }
.nk-bar .title { font-weight: 600; }
.nk-menu { padding: 6px; border-radius: 12px; font-size: 13px; }
.nk-menu-item { min-height: 24px; padding: 0 10px; border-radius: 6px; }
.nk-menu-item.lit { background-color: var(--nekoland-ink); color: var(--nekoland-on-ink); }
.nk-menu-item.danger { color: var(--nekoland-danger); }
.nk-keys { font-family: "Ubuntu Sans Mono", monospace; font-size: 11px; opacity: 0.6; }
.nk-menu-line { min-height: 1px; margin: 5px 4px; background-color: color-mix(in srgb, var(--nekoland-ink) 12%, transparent); }
.nk-notice { padding: 12px; border-radius: 14px; }
.nk-notice-title { font-weight: 600; }
.nk-notice-body { font-size: 13px; color: var(--nekoland-ink-2); }
.nk-dock { padding: 12px 12px 4px 12px; border-radius: 20px; }
.nk-dot { min-width: 4px; min-height: 4px; border-radius: 2px; background-color: var(--nekoland-ink); }
.nk-dot.active { min-width: 14px; }
.nk-dock-line { min-width: 1px; min-height: 32px; margin: 4px 0 12px 0; background-color: color-mix(in srgb, var(--nekoland-ink) 18%, transparent); }
.nk-window { border-radius: 14px; background-color: var(--nekoland-paper); box-shadow: 0 0 0 1px rgb(0 0 0 / 12%), 0 24px 50px -20px rgb(0 0 0 / 50%); }
.nk-titlebar { min-height: 44px; padding: 0 8px 0 16px; }
.nk-titlebar .title { font-weight: 600; }
.nk-design .nk-titlebar button { min-width: 24px; min-height: 24px; padding: 0; }
.nk-window.inactive .nk-titlebar .title, .nk-design .nk-window.inactive .nk-titlebar button { color: var(--nekoland-ink-3); }
.nk-window-body { min-height: 120px; border-top: 1px solid var(--nekoland-line); border-radius: 0 0 14px 14px; background-color: var(--nekoland-sheet); }
.nk-scrim { padding: 32px; border-radius: 12px; background-color: rgb(0 0 0 / 40%); }
.nk-dialog { padding: 15px 20px 20px; border-radius: 14px; background-color: var(--nekoland-sheet); box-shadow: inset 0 0 0 1px var(--nekoland-line), 0 30px 60px -20px rgb(0 0 0 / 50%); }
.nk-dialog-title { font-size: 16px; font-weight: 600; }
.nk-dialog .nk-lead { font-size: 14px; line-height: 21px; }
.nk-dialog button { min-height: 32px; padding: 0 14px; }
"#;

fn classes(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// A widget that is only its style classes, of the size they give it.
fn shape(names: &[&str]) -> gtk::Box {
    gtk::Box::builder().css_classes(classes(names)).build()
}

fn text(label: &str, names: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .css_classes(classes(names))
        .build()
}

fn column(spacing: i32, children: &[&gtk::Widget]) -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(spacing)
        .build();
    for child in children {
        column.append(*child);
    }
    column
}

fn line(spacing: i32, children: &[&gtk::Widget]) -> gtk::Box {
    let line = gtk::Box::builder().spacing(spacing).build();
    for child in children {
        line.append(*child);
    }
    line
}

/// A card, under a small heading if it has one.
fn card(heading: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["nk-card"])
        .build();
    if !heading.is_empty() {
        card.append(&text(heading, &["nk-h3"]));
    }
    card.append(child);
    card
}

/// Cards side by side, each at least `width` wide and at most `most` to a line, as the
/// design's grids are.
fn cards(width: i32, most: u32, children: &[&gtk::Widget]) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["nk-cards"])
        .column_spacing(16)
        .row_spacing(16)
        .max_children_per_line(most)
        .homogeneous(true)
        .build();
    for child in children {
        child.set_size_request(width, -1);
        flow.append(*child);
    }
    // The cards are not to be chosen, only what is in them.
    let mut cell = flow.first_child();
    while let Some(child) = cell {
        child.set_focusable(false);
        cell = child.next_sibling();
    }
    flow
}

fn section(title: &str, lead: &str, children: &[&gtk::Widget]) -> gtk::Box {
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .build();
    let heading = column(4, &[text(title, &["nk-h2"]).upcast_ref()]);
    if !lead.is_empty() {
        let lead = text(lead, &["nk-lead"]);
        lead.set_wrap(true);
        heading.append(&lead);
    }
    section.append(&heading);
    for child in children {
        section.append(*child);
    }
    section
}

/// One of a few choices, the chosen one solid: libadwaita's toggle group.
// shortcut: made from its name, as the libadwaita crate in use stops at 1.5 and the group
// came with 1.7; its own type once the crate is moved up.
fn segmented(labels: &[&str], active: u32) -> gtk::Widget {
    let toggles: String = labels
        .iter()
        .map(|label| {
            format!(
                "<child><object class=\"AdwToggle\"><property name=\"label\">{}</property></object></child>",
                glib::markup_escape_text(label)
            )
        })
        .collect();
    let group: gtk::Widget = gtk::Builder::from_string(&format!(
        "<interface><object class=\"AdwToggleGroup\" id=\"group\">{toggles}</object></interface>"
    ))
    .object("group")
    .expect("the toggle group just described");
    group.set_property("active", active);
    group.set_halign(gtk::Align::Start);
    group
}

/// Nekoland's mark, as the menu bar draws it: a cat's head.
fn logo() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(16)
        .content_height(16)
        .valign(gtk::Align::Center)
        .build();
    area.set_draw_func(|area, cr, _, _| {
        cr.move_to(2.0, 6.5);
        cr.line_to(3.0, 1.0);
        cr.line_to(6.5, 4.0);
        cr.line_to(9.5, 4.0);
        cr.line_to(13.0, 1.0);
        cr.line_to(14.0, 6.5);
        cr.curve_to(15.5, 11.5, 12.0, 15.0, 8.0, 15.0);
        cr.curve_to(4.0, 15.0, 0.5, 11.5, 2.0, 6.5);
        cr.close_path();
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let _ = cr.fill();
    });
    area
}

/// Nekoland's design language, after its design, section by section: GTK's own widgets
/// in Nekoland's look where GTK has one, drawn here where it has none. Everything can be
/// tried, and the switch at the top shows it light or dark.
/// One of the page's rules: its name over a sentence or two.
fn rule_card(name: &str, body: &str) -> gtk::Widget {
    let name = text(name, &[]);
    name.set_markup(&format!("<b>{}</b>", name.label()));
    let body = text(body, &["nk-rule-body"]);
    body.set_wrap(true);
    // Narrow enough for three to a line.
    body.set_max_width_chars(20);
    let rule = column(4, &[name.upcast_ref(), body.upcast_ref()]);
    rule.set_css_classes(&["nk-card", "nk-rule"]);
    rule.upcast()
}

/// The construction of an icon, drawn as a sheet: the grid, the layers, the light, and
/// what a surface is and is not made of.
const ICON_SHEET: &[u8] = include_bytes!("icon-sheet.svg");

/// How Nekoland's icons are built, with the theme's own icons as the examples.
fn icons_section() -> gtk::Box {
    let sheet = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .height_request(416)
        .build();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON_SHEET)) {
        Ok(drawing) => sheet.set_paintable(Some(&drawing)),
        Err(err) => eprintln!("icon sheet: {err}"),
    }
    // The first of each kind, a line's worth: the whole list is in Appearance.
    let strip = |name: &str, icons: Vec<PathBuf>| {
        let spec = text(&format!("{name} {}", icons.len()), &["nk-spec"]);
        spec.set_size_request(120, -1);
        let samples = gtk::Box::builder().spacing(10).build();
        for path in icons.iter().take(12) {
            samples.append(&gtk::Image::builder().file(path.to_string_lossy()).pixel_size(48).build());
        }
        line(16, &[spec.upcast_ref(), samples.upcast_ref()])
    };
    section(
        "圖示",
        "手繪的和從其他應用程式轉換的圖示照同一張圖製作，合成一個圖示主題。",
        &[
            card("", &sheet).upcast_ref(),
            strip("手繪", icons_in(Path::new(DRAWN_ICONS))).upcast_ref(),
            strip("自動演算", converted_icons()).upcast_ref(),
        ],
    )
}

fn design_page() -> gtk::ScrolledWindow {
    let provider = gtk::CssProvider::new();
    let styles = adw::StyleManager::default();
    // Light or dark as chosen on the page; as the system is until then.
    let chosen: Rc<std::cell::Cell<Option<bool>>> = Rc::default();
    provider.load_from_string(&design_css(styles.is_dark()));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
    }

    // Colours: a swatch each, with the value it has now.
    let values: Vec<(gtk::Label, &str, &str)> = TOKENS
        .iter()
        .map(|(token, _, used)| (text("", &["nk-small"]), *token, *used))
        .collect();
    let show_values = {
        let values = values.clone();
        move |dark: bool| {
            for (label, token, used) in &values {
                label.set_markup(&format!(
                    "<span font_family='Ubuntu Sans Mono'>{}</span> · {used}",
                    token_value(token, dark)
                ));
            }
        }
    };
    show_values(styles.is_dark());
    let swatches: Vec<gtk::Widget> = TOKENS
        .iter()
        .zip(&values)
        .map(|((token, name, _), (value, ..))| {
            let swatch = shape(&["nk-swatch"]);
            // Each swatch takes its token from the page, so it turns with the page.
            let own = gtk::CssProvider::new();
            own.load_from_string(&format!(
                ".nk-swatch.nk-{token} {{ background-color: var(--nekoland-{token}); }}"
            ));
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &own,
                    gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
                );
            }
            swatch.add_css_class(&format!("nk-{token}"));
            let name = text(name, &[]);
            name.set_markup(&format!("<span weight='500'>{}</span>", name.label()));
            card(
                "",
                &column(
                    8,
                    &[
                        swatch.upcast_ref(),
                        column(0, &[name.upcast_ref(), value.upcast_ref()]).upcast_ref(),
                    ],
                ),
            )
            .upcast()
        })
        .collect();

    let theme = segmented(&["淺色", "深色"], u32::from(styles.is_dark()));
    theme.set_valign(gtk::Align::End);
    theme.connect_notify_local(
        Some("active"),
        glib::clone!(
            #[weak]
            provider,
            #[strong]
            chosen,
            #[strong]
            show_values,
            move |theme, _| {
                let dark = theme.property::<u32>("active") == 1;
                chosen.set(Some(dark));
                provider.load_from_string(&design_css(dark));
                show_values(dark);
            }
        ),
    );
    styles.connect_dark_notify(glib::clone!(
        #[weak]
        theme,
        #[strong]
        chosen,
        move |styles| {
            if chosen.get().is_none() {
                theme.set_property("active", u32::from(styles.is_dark()));
                chosen.set(None);
            }
        }
    ));

    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(56)
        .margin_top(40)
        .margin_bottom(80)
        .margin_start(32)
        .margin_end(32)
        .build();

    let intro = text(
        "介面只用墨色、紙色和灰階，選中就是實心墨色。顏色留給桌布、照片和視窗內容；需要警示時才出現紅色。",
        &["nk-lead"],
    );
    intro.set_wrap(true);
    intro.set_max_width_chars(40);
    let title = column(
        8,
        &[
            text("Nekoland 設計語言", &["nk-h1"]).upcast_ref(),
            intro.upcast_ref(),
        ],
    );
    title.set_hexpand(true);
    page.append(&line(24, &[title.upcast_ref(), &theme]));

    let rules: Vec<gtk::Widget> = [
        (
            "選中是墨色",
            "目前的分類、按下的選項、開著的開關，一律填實心墨色，不另用強調色。",
        ),
        (
            "顏色屬於內容",
            "桌布、App 圖示、照片和視窗是畫面上唯一的彩色。介面退到灰階，讓內容在前面。",
        ),
        (
            "一層玻璃",
            "只有浮在桌面上的東西用毛玻璃：選單列、Dock、通知、選單和側邊欄。視窗裡面是不透明的紙色。",
        ),
        (
            "動作有回彈",
            "按下時縮小一點，放開時回彈。開關的圓點按住會拉長，像被手指壓扁。",
        ),
    ]
    .iter()
    .map(|(name, body)| rule_card(name, body))
    .collect();
    page.append(&section(
        "原則",
        "",
        &[cards(320, 3, &rules.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    page.append(&section(
        "顏色",
        "每個名稱在淺色與深色各有一個值，切換右上角可以對照。",
        &[cards(240, 4, &swatches.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    page.append(&icons_section());

    let types = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    for (index, (spec, markup)) in [
        (
            "標題 32 / 700",
            "<span size='24pt' weight='bold'>桌面與 Dock</span>",
        ),
        (
            "頁面標題 20 / 600",
            "<span size='15pt' weight='600'>顯示器</span>",
        ),
        ("視窗標題 14 / 600", "<span weight='600'>設定</span>"),
        ("正文 14 / 400", "拖到螢幕邊緣時，視窗會貼齊到那一半。"),
        (
            "說明 12 / 400",
            "<span size='9pt' alpha='70%'>重新登入後生效</span>",
        ),
        (
            "群組標題 12 / 500",
            "<span size='9pt' weight='500' alpha='60%' letter_spacing='500'>系統</span>",
        ),
        (
            "數值 13 等寬",
            "<span font_family='Ubuntu Sans Mono' size='9.75pt'>6016 × 3384 · 60 Hz · Ctrl+Alt+T</span>",
        ),
    ]
    .iter()
    .enumerate()
    {
        let spec = text(spec, &["nk-spec"]);
        spec.set_size_request(120, -1);
        let sample = gtk::Label::builder()
            .label(*markup)
            .use_markup(true)
            .xalign(0.0)
            .build();
        let row = line(16, &[spec.upcast_ref(), sample.upcast_ref()]);
        row.set_css_classes(if index == 0 {
            &["nk-type-row"]
        } else {
            &["nk-type-row", "next"]
        });
        types.append(&row);
    }
    page.append(&section(
        "字體",
        "Ubuntu Sans 加 Noto Sans CJK TC；數值、路徑和快速鍵用 Ubuntu Sans Mono。",
        &[card("", &types).upcast_ref()],
    ));

    let steps = gtk::Box::builder().spacing(10).build();
    for step in [4, 8, 12, 16, 24, 32, 48] {
        let square = shape(&["nk-step"]);
        square.set_size_request(step, step);
        square.set_halign(gtk::Align::Center);
        square.set_valign(gtk::Align::End);
        square.set_vexpand(true);
        let number = text(&step.to_string(), &["nk-note"]);
        number.set_xalign(0.5);
        steps.append(&column(6, &[square.upcast_ref(), number.upcast_ref()]));
    }
    let corners = gtk::Box::builder().spacing(12).build();
    for (radius, used) in [
        ("6px", "6 · 小按鈕、提示"),
        ("8px", "8 · 按鈕、輸入框"),
        ("12px", "12 · 卡片、選單"),
        ("14px", "14 · 視窗、對話框"),
        ("99px", "全圓 · 開關、標籤"),
    ] {
        let corner = shape(&["nk-corner"]);
        corner.set_halign(gtk::Align::Start);
        let own = gtk::CssProvider::new();
        let class = format!("nk-corner-{}", radius.trim_end_matches("px"));
        own.load_from_string(&format!(".{class} {{ border-radius: {radius}; }}"));
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &own,
                gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
            );
        }
        corner.add_css_class(&class);
        corners.append(&column(
            12,
            &[corner.upcast_ref(), text(used, &["nk-note"]).upcast_ref()],
        ));
    }
    page.append(&section(
        "間距與圓角",
        "間距以 4 為單位；圓角依大小分級，越大的面越圓。",
        &[
            cards(240, 4, &[card("間距", &steps).upcast_ref()]).upcast_ref(),
            card("圓角", &corners).upcast_ref(),
        ],
    ));

    let button = |label: &str, names: &[&str]| {
        gtk::Button::builder()
            .label(label)
            .css_classes(classes(names))
            .build()
    };
    let disabled = button("儲存", &["suggested-action"]);
    disabled.set_sensitive(false);
    let add = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("新增")
                .build(),
        )
        .build();
    let kinds = line(
        12,
        &[
            button("儲存", &["suggested-action"]).upcast_ref(),
            button("取消", &[]).upcast_ref(),
            button("重設", &["flat"]).upcast_ref(),
            button("刪除", &["destructive-action"]).upcast_ref(),
            disabled.upcast_ref(),
            button("開始使用", &["suggested-action", "nk-large"]).upcast_ref(),
            add.upcast_ref(),
        ],
    );
    let mut kind = kinds.first_child();
    while let Some(widget) = kind {
        widget.set_valign(gtk::Align::Center);
        kind = widget.next_sibling();
    }
    let icon_button = |icon: &str, label: &str| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(label)
            .css_classes(["flat", "circular"])
            .build()
    };
    let icons = line(
        12,
        &[
            icon_button("system-search-symbolic", "搜尋").upcast_ref(),
            icon_button("view-more-horizontal-symbolic", "更多").upcast_ref(),
            icon_button("go-previous-symbolic", "返回").upcast_ref(),
        ],
    );
    let icon_card = card(
        "圖示按鈕",
        &column(
            12,
            &[
                icons.upcast_ref(),
                text("描邊 1.5，圓頭，16 格線。", &["nk-note"]).upcast_ref(),
            ],
        ),
    );
    page.append(&section(
        "按鈕",
        "一個畫面只有一個主要按鈕。按鈕寫出動作本身，不寫「確定」。",
        &[
            card("種類", &kinds).upcast_ref(),
            cards(240, 4, &[icon_card.upcast_ref()]).upcast_ref(),
        ],
    ));

    let field = |label: &str, widget: &gtk::Widget, error: &str| {
        let field = column(6, &[text(label, &["nk-label"]).upcast_ref(), widget]);
        if !error.is_empty() {
            field.append(&text(error, &["nk-err"]));
        }
        let card = card("", &field);
        card.upcast::<gtk::Widget>()
    };
    // Fields as wide as their cards, not as wide as GTK makes them by itself.
    let entry = |value: &str| {
        gtk::Entry::builder()
            .text(value)
            .width_chars(8)
            .max_width_chars(8)
            .build()
    };
    let wrong = entry("neko land");
    wrong.add_css_class("error");
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋").build();
    search.set_width_chars(8);
    search.set_max_width_chars(8);
    let zone = gtk::DropDown::from_strings(&["台北（GMT+8）", "東京（GMT+9）", "倫敦（GMT+0）"]);
    let fields = [
        field("裝置名稱", entry("nekoland").upcast_ref(), ""),
        field("主機名稱", wrong.upcast_ref(), "不能有空格"),
        field("搜尋", search.upcast_ref(), ""),
        field("時區", zone.upcast_ref(), ""),
    ];
    page.append(&section(
        "輸入",
        "",
        &[cards(240, 4, &fields.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    let switches = line(
        12,
        &[
            gtk::Switch::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build()
                .upcast_ref(),
            gtk::Switch::builder()
                .valign(gtk::Align::Center)
                .build()
                .upcast_ref(),
        ],
    );
    let centre = gtk::CheckButton::builder().label("置中").active(true).build();
    let pointer = gtk::CheckButton::builder().label("游標所在處").build();
    pointer.set_group(Some(&centre));
    let checks = column(
        6,
        &[
            gtk::CheckButton::builder()
                .label("開機時啟動")
                .active(true)
                .build()
                .upcast_ref(),
            gtk::CheckButton::builder()
                .label("在 Dock 顯示")
                .build()
                .upcast_ref(),
            centre.upcast_ref(),
            pointer.upcast_ref(),
        ],
    );
    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    slider.set_value(60.0);
    let level = text("60%", &["nk-note", "nk-mono"]);
    slider.connect_value_changed(glib::clone!(
        #[weak]
        level,
        move |slider| level.set_label(&format!("{}%", slider.value().round()))
    ));
    let choices = [
        card("開關", &switches).upcast::<gtk::Widget>(),
        card("核取方塊與單選", &checks).upcast(),
        card("分段選擇", &segmented(&["左側", "底部", "右側"], 1)).upcast(),
        card("滑桿", &column(8, &[slider.upcast_ref(), level.upcast_ref()])).upcast(),
    ];
    page.append(&section(
        "選擇",
        "",
        &[cards(240, 4, &choices.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    let rows = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    rows.append(&adw::SwitchRow::builder().title("毛玻璃").active(true).build());
    rows.append(
        &adw::SwitchRow::builder()
            .title("自動隱藏")
            .subtitle("游標移到螢幕底部時出現")
            .build(),
    );
    let resolution = adw::ActionRow::builder()
        .title("解析度")
        .activatable(true)
        .build();
    resolution.add_suffix(&text("6016 × 3384", &["nk-lead", "nk-mono"]));
    resolution.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    rows.append(&resolution);
    let bluetooth = adw::ActionRow::builder().title("藍牙").build();
    let closed = text("已關閉", &["nk-badge"]);
    closed.set_valign(gtk::Align::Center);
    bluetooth.add_suffix(&closed);
    rows.append(&bluetooth);
    let nav = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    for (name, detail) in [("桌面", ""), ("顯示器", "6016 × 3384"), ("輸入法", "Project Key")] {
        let name = text(name, &[]);
        name.set_hexpand(true);
        let detail = text(detail, &[]);
        detail.set_markup(&format!("<span size='9pt' alpha='60%'>{}</span>", detail.label()));
        nav.append(&line(8, &[name.upcast_ref(), detail.upcast_ref()]));
    }
    nav.select_row(nav.row_at_index(0).as_ref());
    page.append(&section(
        "清單與導覽",
        "",
        &[
            rows.upcast_ref(),
            cards(240, 4, &[card("側邊欄", &nav).upcast_ref()]).upcast_ref(),
        ],
    ));

    let badge = |label: &str, names: &[&str]| {
        let badge = text(label, names);
        badge.set_valign(gtk::Align::Center);
        badge
    };
    let badges = line(
        12,
        &[
            badge("3", &["nk-badge"]).upcast_ref(),
            badge("使用中", &["nk-badge", "solid"]).upcast_ref(),
            badge("離線", &["nk-badge", "danger"]).upcast_ref(),
        ],
    );
    let downloading = line(
        12,
        &[
            gtk::Spinner::builder().spinning(true).build().upcast_ref(),
            text("正在下載", &["nk-lead"]).upcast_ref(),
        ],
    );
    let progress = column(
        16,
        &[
            gtk::ProgressBar::builder().fraction(0.62).build().upcast_ref(),
            downloading.upcast_ref(),
        ],
    );
    let tip = text("Ctrl+Alt+T", &["nk-tip"]);
    tip.set_halign(gtk::Align::Start);
    let moved = text("已移到桌面 2", &[]);
    moved.set_hexpand(true);
    let toast = line(
        12,
        &[
            moved.upcast_ref(),
            gtk::Button::builder()
                .label("復原")
                .valign(gtk::Align::Center)
                .build()
                .upcast_ref(),
        ],
    );
    toast.add_css_class("nk-toast");
    let feedback = [
        card("標籤", &badges).upcast::<gtk::Widget>(),
        card("進度", &progress).upcast(),
        card("提示", &tip).upcast(),
        card("浮動訊息", &toast).upcast(),
    ];
    page.append(&section(
        "回饋",
        "",
        &[cards(240, 4, &feedback.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    page.append(&section(
        "浮在桌面上的",
        "選單列、選單、通知和 Dock 用同一層玻璃。模糊由合成器畫，這裡只有半透明。",
        &[desktop_glass().upcast_ref()],
    ));

    page.append(&section(
        "框選與並排預覽",
        "在桌面框選項目、把視窗拖到螢幕邊緣並排時，都畫同一種框：白色填色 15%，1 px 白邊 60%，直角。",
        &[desktop_bands().upcast_ref()],
    ));

    let window = |title: &str, active: bool| {
        let control = |icon: &str, label: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(label)
                .valign(gtk::Align::Center)
                .css_classes(["circular"])
                .build()
        };
        let name = text(title, &["title"]);
        name.set_hexpand(true);
        let bar = line(
            8,
            &[
                name.upcast_ref(),
                control("window-minimize-symbolic", "最小化").upcast_ref(),
                control("window-maximize-symbolic", "最大化").upcast_ref(),
                control("window-close-symbolic", "關閉").upcast_ref(),
            ],
        );
        bar.add_css_class("nk-titlebar");
        let window = column(0, &[bar.upcast_ref(), shape(&["nk-window-body"]).upcast_ref()]);
        window.set_css_classes(if active {
            &["nk-window"]
        } else {
            &["nk-window", "inactive"]
        });
        window.upcast::<gtk::Widget>()
    };
    let windows = [window("設定", true), window("檔案", false)];
    page.append(&section(
        "視窗",
        "標題列和內容同一張紙，沒有分隔的色帶。非作用中的視窗只把標題退成灰色。",
        &[cards(240, 4, &windows.iter().collect::<Vec<_>>()).upcast_ref()],
    ));

    // An alert as nekoland.css has libadwaita draw it: 20px from each row's letters or edges
    // to the next. A line of the question stands 5px above its letters and 3px below, one of
    // the consequence 4px and 3px, and the icon's symbol 2px inside its image, so the margins
    // are that much less; a field and the buttons are spaced from their edges.
    let alert = |icon: Option<&str>,
                 title: &str,
                 body: Option<&str>,
                 field: Option<gtk::Widget>,
                 responses: &[(&str, &[&str])]| {
        let dialog = column(0, &[]);
        dialog.add_css_class("nk-dialog");
        let mut gap = 0;
        if let Some(icon) = icon {
            let image = gtk::Image::builder()
                .icon_name(icon)
                .pixel_size(32)
                .margin_top(3)
                .build();
            dialog.append(&image);
            gap = 13;
        }
        let centred = |label: &str, names: &[&str]| {
            let label = text(label, names);
            label.set_xalign(0.5);
            label.set_justify(gtk::Justification::Center);
            label.set_wrap(true);
            label
        };
        let question = centred(title, &["nk-dialog-title"]);
        question.set_margin_top(gap);
        dialog.append(&question);
        if let Some(body) = body {
            let body = centred(body, &["nk-lead"]);
            body.set_margin_top(13);
            dialog.append(&body);
        }
        gap = 17;
        if let Some(field) = field {
            field.set_margin_top(gap);
            dialog.append(&field);
            gap = 20;
        }
        let actions = line(12, &[]);
        actions.set_homogeneous(true);
        actions.set_margin_top(gap);
        for (label, names) in responses {
            actions.append(&button(label, names));
        }
        dialog.append(&actions);
        // An alert's width, as alert::FIELD_WIDTH and the 20px each side make it.
        dialog.set_size_request(360, -1);
        let scrim = column(0, &[dialog.upcast_ref()]);
        scrim.add_css_class("nk-scrim");
        scrim.set_halign(gtk::Align::Start);
        scrim.set_valign(gtk::Align::Start);
        scrim.upcast::<gtk::Widget>()
    };
    let alerts = [
        alert(
            None,
            "刪除桌面 2？",
            Some("上面的 3 個視窗會移到桌面 1。"),
            None,
            &[("取消", &[]), ("刪除", &["destructive-action"])],
        ),
        alert(
            None,
            "新增檔案夾",
            None,
            Some(gtk::Entry::builder().text("未命名檔案夾").build().upcast()),
            &[("取消", &[]), ("建立", &["suggested-action"])],
        ),
        alert(
            None,
            "無法建立檔案夾",
            Some("沒有寫入這個檔案夾的權限。"),
            None,
            &[("關閉", &[])],
        ),
        alert(
            Some("system-lock-screen-symbolic"),
            "需要管理員密碼",
            Some("Market 要安裝 GIMP。"),
            Some(
                gtk::PasswordEntry::builder()
                    .show_peek_icon(true)
                    .build()
                    .upcast(),
            ),
            &[("取消", &[]), ("認證", &["suggested-action"])],
        ),
    ];
    // Each scrim only as large as its alert, so it is 32px on every side.
    let grid = cards(424, 2, &alerts.iter().collect::<Vec<_>>());
    grid.set_homogeneous(false);
    page.append(&section(
        "對話框",
        "問題置中，後果用灰字。四邊和每一列之間都隔 20px，從字算起；按鈕平分一列，相隔 12px。",
        &[grid.upcast_ref()],
    ));
    let clamp = adw::Clamp::builder()
        .maximum_size(1120)
        .tightening_threshold(1120)
        .child(&page)
        .build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .css_classes(["nk-design"])
        .child(&clamp)
        .build()
}

/// The box the desktop draws to choose items, and the same box where a dragged window will
/// go, each over a wallpaper.
fn desktop_bands() -> gtk::FlowBox {
    let stage = |name: &str, bands: &[&gtk::Widget]| {
        let stage = gtk::Box::builder()
            .css_classes(["nk-stage"])
            .height_request(160)
            .homogeneous(true)
            .overflow(gtk::Overflow::Hidden)
            .build();
        for band in bands {
            stage.append(*band);
        }
        column(8, &[stage.upcast_ref(), text(name, &["nk-note"]).upcast_ref()]).upcast::<gtk::Widget>()
    };
    let choosing = shape(&["nk-band"]);
    choosing.set_size_request(120, 72);
    choosing.set_halign(gtk::Align::Start);
    choosing.set_valign(gtk::Align::Start);
    choosing.set_margin_start(36);
    choosing.set_margin_top(28);
    // The left half, the other half left empty.
    let preview = stage("並排預覽",&[shape(&["nk-band"]).upcast_ref(), shape(&[]).upcast_ref()]);
    cards(240, 2, &[&stage("框選", &[choosing.upcast_ref()]), &preview])
}

/// What floats over the desktop, as Nekoland draws it: the menu bar across the top with a
/// menu open under it, a notification under the bar's right end, and the Dock. Drawn over
/// a wallpaper, with translucency standing in for the compositor's blur.
fn desktop_glass() -> gtk::Box {
    let stage = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["nk-stage"])
        .build();

    let bar = gtk::Box::builder()
        .spacing(20)
        .css_classes(["nk-glass", "nk-bar"])
        .build();
    bar.append(&logo());
    bar.append(&text("檔案", &["title"]));
    for menu in ["檔案", "編輯", "顯示方式", "視窗", "輔助說明"] {
        bar.append(&text(menu, &[]));
    }
    let status = gtk::Box::builder()
        .spacing(14)
        .hexpand(true)
        .halign(gtk::Align::End)
        .build();
    for icon in [
        "input-keyboard-symbolic",
        "audio-volume-high-symbolic",
        "network-wired-symbolic",
    ] {
        status.append(&gtk::Image::builder().icon_name(icon).pixel_size(16).build());
    }
    status.append(&text("11月16日 週一 09:41", &[]));
    bar.append(&status);
    stage.append(&bar);

    // Under the bar: a menu where it opens from its title, a notification at the right.
    let item = |label: &str, keys: &str, names: &[&str]| {
        let name = text(label, &[]);
        name.set_hexpand(true);
        let row = line(16, &[name.upcast_ref(), text(keys, &["nk-keys"]).upcast_ref()]);
        row.set_css_classes(&[&["nk-menu-item"], names].concat());
        row
    };
    let menu = column(
        0,
        &[
            item("開新視窗", "Ctrl N", &[]).upcast_ref(),
            item("釘選到 Dock", "", &["lit"]).upcast_ref(),
            item("移到桌面 2", "", &[]).upcast_ref(),
            shape(&["nk-menu-line"]).upcast_ref(),
            item("結束", "Ctrl Q", &["danger"]).upcast_ref(),
        ],
    );
    menu.set_css_classes(&["nk-glass", "nk-menu"]);
    menu.set_size_request(220, -1);
    menu.set_valign(gtk::Align::Start);
    // As wide as it is, though what is in it would spread.
    menu.set_hexpand(false);
    menu.set_margin_start(36);

    let body = text("「設計稿.png」已複製到下載項目", &["nk-notice-body"]);
    body.set_wrap(true);
    let words = column(
        0,
        &[text("檔案", &["nk-notice-title"]).upcast_ref(), body.upcast_ref()],
    );
    words.set_hexpand(true);
    let notice = line(
        10,
        &[
            gtk::Image::builder()
                .icon_name("org.gnome.Nautilus")
                .pixel_size(36)
                .valign(gtk::Align::Start)
                .build()
                .upcast_ref(),
            words.upcast_ref(),
        ],
    );
    notice.set_css_classes(&["nk-glass", "nk-notice"]);
    notice.set_size_request(360, -1);
    notice.set_valign(gtk::Align::Start);
    notice.set_halign(gtk::Align::End);
    notice.set_hexpand(true);
    notice.set_margin_end(12);
    let under = line(20, &[menu.upcast_ref(), notice.upcast_ref()]);
    under.set_margin_top(4);
    stage.append(&under);

    // The Dock: all applications, the kept ones, a line, the Trash; a dot under those that
    // run, a longer one under the active one.
    let dock = gtk::Box::builder()
        .spacing(12)
        .css_classes(["nk-glass", "nk-dock"])
        .halign(gtk::Align::Center)
        .margin_top(96)
        .margin_bottom(8)
        .build();
    for (icon, running) in [
        ("view-app-grid-symbolic", None),
        ("org.gnome.Nautilus", Some(true)),
        ("org.gnome.TextEditor", Some(false)),
        ("org.gnome.Calculator", None),
        ("utilities-terminal", Some(false)),
        ("", None),
        ("user-trash", None),
    ] {
        if icon.is_empty() {
            dock.append(&shape(&["nk-dock-line"]));
            continue;
        }
        let dot = match running {
            Some(true) => shape(&["nk-dot", "active"]),
            Some(false) => shape(&["nk-dot"]),
            None => shape(&[]),
        };
        dot.set_halign(gtk::Align::Center);
        dot.set_size_request(-1, 4);
        dock.append(&column(
            4,
            &[
                gtk::Image::builder()
                    .icon_name(icon)
                    .pixel_size(40)
                    .build()
                    .upcast_ref(),
                dot.upcast_ref(),
            ],
        ));
    }
    stage.append(&dock);
    stage
}

fn save_autostart(autostart: &[String]) {
    let quoted: Vec<String> = autostart.iter().map(|command| format!("{command:?}")).collect();
    set(&format!("autostart = [{}]", quoted.join(", ")));
}

#[cfg(test)]
mod tests {
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
