//! `nekoland-settings`: Nekoland's settings, in GTK and libadwaita as GNOME's own settings
//! are, so that it looks as the system's GTK style says. What the compositor keeps it asks
//! for and changes through `nekoland msg`, so the compositor alone reads and writes its file
//! and puts each change in effect; the rest is the system's, through its own tools.

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Settings";
const APPS_VIEWS: [&str; 4] = ["overview", "launchpad", "shelf", "sidebar"];
const DOCK_POSITIONS: [&str; 3] = ["left", "bottom", "right"];
const WALLPAPER_MODES: [&str; 3] = ["fill", "fit", "center"];
/// Where pictures for the desktop are looked for, and how many are offered.
const WALLPAPER_DIRS: [&str; 3] = [
    "~/.local/share/backgrounds",
    "~/Pictures/Wallpapers",
    "/usr/share/backgrounds",
];
const WALLPAPERS: usize = 8;
const SINK: &str = "@DEFAULT_AUDIO_SINK@";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

/// What a program printed, or None when it could not be run or failed.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .inspect_err(|err| eprintln!("{program}: {err}"))
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
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

/// The page a restarted Settings opens on.
const PAGE_ENV: &str = "NEKOLAND_SETTINGS_PAGE";

thread_local! {
    /// Set when Settings is to start again once it has closed, on the page named.
    static RESTART: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    /// Nekoland's `glass_opacity`, which the look gives GTK's menus and sidebars too.
    static GLASS_OPACITY: Cell<f64> = const { Cell::new(0.8) };
    /// Nekoland's `font`, which the look gives GTK's text too.
    static UI_FONT: RefCell<String> = RefCell::new("Ubuntu Sans".into());
    /// The `pw-dump --monitor` the sound page follows, stopped as Settings ends.
    static SOUND_MONITOR: RefCell<Option<gio::Subprocess>> = const { RefCell::new(None) };
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build);
    let code = app.run();
    if let Some(monitor) = SOUND_MONITOR.take() {
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

fn build(app: &adw::Application) {
    // Opened again, it shows the window it has.
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let config = Config::load();
    GLASS_OPACITY.set(config.number("glass_opacity", 0.8));
    if !config.word("font").is_empty() {
        UI_FONT.set(config.word("font").to_string());
    }
    // A look turned on before keeps up with this version of it.
    if gtk_look_on() {
        if let Err(err) = set_gtk_look(true) {
            eprintln!("GTK look: {err}");
        }
        set_window_buttons(true);
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
    let pages: [(&str, gtk::Widget); 14] = [
        ("桌面", desktop_page(&config, &autostart).upcast()),
        ("外觀", appearance_page(&config, &look).upcast()),
        ("顯示", display_page(&config).upcast()),
        ("字型", fonts_page(&look).upcast()),
        ("鍵盤", keyboard_page(&config).upcast()),
        ("滑鼠與觸控板", mouse_page(&config).upcast()),
        ("聲音", sound_page().upcast()),
        ("電源", power_page(&config).upcast()),
        ("隱私", privacy_page().upcast()),
        ("協助工具", accessibility_page(&config).upcast()),
        ("預設應用程式", default_apps_page(&config).upcast()),
        ("啟動項目", startup_page(&autostart).upcast()),
        ("關於", about_page().upcast()),
        ("設計語言", design_page().upcast()),
    ];

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    for (name, page) in &pages {
        stack.add_named(page, Some(name));
        sidebar.append(&gtk::Label::builder().label(*name).xalign(0.0).build());
    }

    let content = adw::NavigationPage::builder().title(pages[0].0).build();
    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&adw::HeaderBar::new());
    content_view.set_content(Some(&stack));
    content.set_child(Some(&content_view));

    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    sidebar_view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&sidebar)
            .build(),
    ));
    let split = adw::NavigationSplitView::builder()
        .sidebar(
            &adw::NavigationPage::builder()
                .title("設定")
                .child(&sidebar_view)
                .build(),
        )
        .content(&content)
        .build();

    let names: Vec<&str> = pages.iter().map(|(name, _)| *name).collect();
    let first = std::env::var(PAGE_ENV)
        .ok()
        .and_then(|page| names.iter().position(|name| *name == page))
        .unwrap_or(0);
    std::env::remove_var(PAGE_ENV);
    sidebar.connect_row_selected(glib::clone!(
        #[weak]
        stack,
        #[weak]
        content,
        move |_, row| {
            if let Some(name) = row.and_then(|row| names.get(row.index() as usize)) {
                stack.set_visible_child_name(name);
                content.set_title(name);
            }
        }
    ));
    sidebar.select_row(sidebar.row_at_index(first as i32).as_ref());

    adw::ApplicationWindow::builder()
        .application(app)
        .title("設定")
        .default_width(980)
        .default_height(720)
        .width_request(640)
        .height_request(480)
        .content(&split)
        .build()
        .present();
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
fn desktop_page(config: &Config, autostart: &Rc<RefCell<Vec<String>>>) -> adw::PreferencesPage {
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
    dock.add(&choice_row(config, "位置", "dock_position", &DOCK_POSITIONS, &["左", "下", "右"]));
    dock.add(&switch_row(config, "自動隱藏", "dock_autohide"));
    dock.add(&switch_row(config, "毛玻璃", "dock_glass"));
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
    dock.add(&switch_row(config, "執行中指示燈", "dock_indicators"));
    dock.add(&switch_row(config, "開啟動畫", "dock_bounce"));
    page.add(&dock);

    let bar = group("選單列");
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
    bar.add(&switch_row(config, "網路", "menu_bar_network"));
    bar.add(&switch_row(config, "音量", "menu_bar_sound"));
    page.add(&bar);

    let lock = group("鎖定螢幕");
    lock.add(&idle_row(config, "閒置後鎖定", "lock_after"));
    page.add(&lock);

    let notices = group("通知");
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
    page.add(&notices);

    let windows = group("視窗");
    for (title, key) in [
        ("按兩下標題列", "action-double-click-titlebar"),
        ("按中鍵標題列", "action-middle-click-titlebar"),
    ] {
        let current = output("gsettings", &["get", WM_PREFERENCES, key]).unwrap_or_default();
        let titlebar = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&["縮放", "最小化", "無"]))
            .selected(TITLEBAR_ACTIONS.iter().position(|action| current.contains(action)).unwrap_or(0) as u32)
            .build();
        titlebar.connect_selected_notify(move |row| {
            if let Some(action) = TITLEBAR_ACTIONS.get(row.selected() as usize) {
                run("gsettings", &["set", WM_PREFERENCES, key, action]);
            }
        });
        windows.add(&titlebar);
    }
    windows.add(&switch_row(config, "焦點跟隨游標", "focus_follows_pointer"));
    windows.add(&switch_row(config, "拖到螢幕邊緣時並排", "edge_tiling"));
    page.add(&windows);

    let corners = group("熱點");
    for (title, key) in [
        ("左上角", "hot_corner_top_left"),
        ("右上角", "hot_corner_top_right"),
        ("左下角", "hot_corner_bottom_left"),
        ("右下角", "hot_corner_bottom_right"),
    ] {
        corners.add(&choice_row(config, title, key, &HOT_CORNERS, &["無", "所有視窗", "應用程式", "鎖定螢幕"]));
    }
    page.add(&corners);

    let wall = group("桌布");
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
const WM_PREFERENCES: &str = "org.gnome.desktop.wm.preferences";
/// What double-clicking or middle-clicking a GTK title bar does, as GNOME names it.
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

/// A row with a button that opens GTK's font chooser, at `level`.
fn font_row(
    title: &str,
    level: gtk::FontLevel,
    font: &str,
    changed: impl Fn(&gtk::pango::FontDescription) + 'static,
) -> adw::ActionRow {
    let button = gtk::FontDialogButton::builder()
        .dialog(&gtk::FontDialog::builder().title(title).build())
        .level(level)
        .font_desc(&gtk::pango::FontDescription::from_string(font))
        .valign(gtk::Align::Center)
        .build();
    button.connect_font_desc_notify(move |button| {
        if let Some(font) = button.font_desc() {
            changed(&font);
        }
    });
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&button);
    row.set_activatable_widget(Some(&button));
    row
}

/// The typefaces, for Nekoland and the applications, and how applications draw text.
/// Nekoland's own text keeps its size and the Mac's grey, unhinted edges.
fn fonts_page(look: &gtk::CssProvider) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let faces = group("字型");
    let family = UI_FONT.with_borrow(Clone::clone);
    let look = look.clone();
    faces.add(&font_row("介面", gtk::FontLevel::Family, &family, move |font| {
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
        gtk::FontLevel::Font,
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
    let read_at_start = gtk_look_on();
    // On already, it is over what GTK read at start, so the glass's opacities show at once.
    if let Some(display) = gdk::Display::default().filter(|_| read_at_start) {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
    let glass_look = provider.clone();
    look.connect_active_notify(move |row| {
        let on = row.is_active();
        if let Err(err) = set_gtk_look(on) {
            eprintln!("GTK look: {err}");
            return;
        }
        set_window_buttons(on);
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
    themes.add(&theme_row(config, "游標主題", "cursor_theme", "cursor-theme", true));
    page.add(&themes);

    let bars = glass_group(config, "選單列與 Dock", ("bar", 0.5), |_| {});
    bars.add(&switch_row(config, "選單列背景", "menu_bar_background"));
    page.add(&bars);
    // GTK's menus and sidebars take their tint from the look, written again as it changes.
    page.add(&glass_group(config, "毛玻璃", ("glass", 0.8), move |opacity| {
        GLASS_OPACITY.set(opacity);
        if gtk_look_on() {
            if let Err(err) = set_gtk_look(true) {
                eprintln!("GTK look: {err}");
            }
            glass_look.load_from_string(&look_for(adw::StyleManager::default().is_dark()));
        }
    }));
    page
}

/// The opacity and blur of one kind of glass, Nekoland's `<key>_opacity` and `<key>_blur`;
/// `opacity_changed` is told each new opacity once Nekoland has it.
/// The icon themes, or with `cursors` the cursor themes, installed, by folder name.
fn installed_themes(cursors: bool) -> Vec<String> {
    let mut dirs = vec![home().join(".icons"), home().join(".local/share/icons")];
    dirs.extend(glib::system_data_dirs().into_iter().map(|dir| dir.join("icons")));
    let mut names: Vec<String> = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|theme| {
            let path = theme.path();
            if cursors {
                path.join("cursors").is_dir()
            } else {
                std::fs::read_to_string(path.join("index.theme"))
                    .is_ok_and(|index| index.contains("Directories="))
            }
        })
        .filter_map(|theme| theme.file_name().into_string().ok())
        // The base every theme falls back to, not one of its own.
        .filter(|name| !["hicolor", "locolor", "default"].contains(&name.as_str()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// A row choosing a theme for Nekoland's `key` and GNOME's `gsetting` together; 預設 leaves
/// both to their own choice.
fn theme_row(config: &Config, title: &str, key: &'static str, gsetting: &'static str, cursors: bool) -> adw::ComboRow {
    let themes = installed_themes(cursors);
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

/// GNOME's setting for the buttons GTK puts in a window's title bar.
const BUTTON_LAYOUT: [&str; 2] = ["org.gnome.desktop.wm.preferences", "button-layout"];
/// The design language's title bar: minimize, maximize and close, at its right.
const LOOK_BUTTONS: &str = "':minimize,maximize,close'";
/// The layout from before the look was turned on, beside the look, to put back after.
const SAVED_BUTTONS_FILE: &str = "nekoland/button-layout";

/// Gives windows the design language's title bar buttons with the look, and puts back the
/// ones they had when it is turned off.
fn set_window_buttons(on: bool) {
    let [schema, key] = BUTTON_LAYOUT;
    let saved = gtk_dir().join(SAVED_BUTTONS_FILE);
    if on {
        if !saved.exists() {
            // Without the layout kept, it could not be put back: it is left as it is.
            let Some(before) = output("gsettings", &["get", schema, key]) else {
                return;
            };
            if let Err(err) = std::fs::write(&saved, before.trim()) {
                eprintln!("{}: {err}", saved.display());
                return;
            }
        }
        run("gsettings", &["set", schema, key, LOOK_BUTTONS]);
    } else if let Ok(before) = std::fs::read_to_string(&saved) {
        run("gsettings", &["set", schema, key, before.trim()]);
        if let Err(err) = std::fs::remove_file(&saved) {
            eprintln!("{}: {err}", saved.display());
        }
    }
}

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
    typing.add(&gsettings_switch("文字游標閃爍", INTERFACE, "cursor-blink"));
    page.add(&typing);
    page
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
                if device_names(&nodes) != *shown.devices.borrow() {
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
    *shown.devices.borrow_mut() = device_names(&nodes);
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
    let motion = adw::SwitchRow::builder()
        .title("減少動態效果")
        .active(interface_setting("enable-animations") == "false")
        .build();
    motion.connect_active_notify(|row| set_interface("enable-animations", &(!row.is_active()).to_string()));
    seeing.add(&motion);
    page.add(&seeing);
    page
}

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
const DEFAULT_APPS: [(&str, &[&str]); 7] = [
    ("網頁瀏覽器", &["x-scheme-handler/http", "x-scheme-handler/https", "text/html"]),
    ("郵件", &["x-scheme-handler/mailto"]),
    ("檔案", &["inode/directory"]),
    ("文字", &["text/plain"]),
    ("圖片", &["image/png", "image/jpeg", "image/gif", "image/webp"]),
    ("音樂", &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"]),
    ("影片", &["video/mp4", "video/webm", "video/x-matroska"]),
];

fn default_apps_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let apps = group("");
    for (title, types) in DEFAULT_APPS {
        let choices = gio::AppInfo::all_for_type(types[0]);
        if choices.is_empty() {
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

/// The display's sizes, as WIDTHxHEIGHT, largest first, each with its rates, fastest first,
/// and the mode in use, from what `nekoland msg modes` prints.
fn display_modes(text: &str) -> (Option<(String, u32)>, Vec<(String, Vec<u32>)>) {
    let parse = |line: &str| {
        let (size, rate) = line.split_once('@')?;
        Some((size.to_string(), rate.parse::<u32>().ok()?))
    };
    let mut lines = text.lines();
    let current = lines.next().and_then(parse);
    let mut sizes: Vec<(String, Vec<u32>)> = Vec::new();
    for (size, rate) in lines.filter_map(parse) {
        match sizes.iter_mut().find(|(known, _)| *known == size) {
            Some((_, rates)) => rates.push(rate),
            None => sizes.push((size, vec![rate])),
        }
    }
    let pixels = |size: &str| {
        let (width, height) = size.split_once('x')?;
        Some(width.parse::<u64>().ok()? * height.parse::<u64>().ok()?)
    };
    sizes.sort_by_key(|(size, _)| std::cmp::Reverse(pixels(size)));
    for (_, rates) in &mut sizes {
        rates.sort_unstable_by(|a, b| b.cmp(a));
        rates.dedup();
    }
    (current, sizes)
}

const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

fn display_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let display = adw::PreferencesGroup::new();
    page.add(&display);

    let (current, sizes) = display_modes(&output("nekoland", &["msg", "modes"]).unwrap_or_default());
    if !sizes.is_empty() {
        let labels: Vec<String> = sizes.iter().map(|(size, _)| size.replace('x', " × ")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let resolution = adw::ComboRow::builder()
            .title("解析度")
            .model(&gtk::StringList::new(&labels))
            .selected(
                current
                    .as_ref()
                    .and_then(|(size, _)| sizes.iter().position(|(known, _)| known == size))
                    .unwrap_or(0) as u32,
            )
            .build();
        let rate = adw::ComboRow::builder().title("更新頻率").build();
        let sizes = Rc::new(sizes);
        // Set while the rates are filled in, which is not a choice of one.
        let filling = Rc::new(Cell::new(false));
        let show_rates = {
            let (rate, sizes, filling) = (rate.clone(), sizes.clone(), filling.clone());
            move |index: u32, wanted: Option<u32>| {
                let Some((_, rates)) = sizes.get(index as usize) else {
                    return;
                };
                let labels: Vec<String> = rates.iter().map(|hz| format!("{hz} Hz")).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                filling.set(true);
                rate.set_model(Some(&gtk::StringList::new(&labels)));
                rate.set_selected(
                    wanted
                        .and_then(|wanted| rates.iter().position(|hz| *hz == wanted))
                        .unwrap_or(0) as u32,
                );
                filling.set(false);
                rate.set_visible(rates.len() > 1);
            }
        };
        show_rates(resolution.selected(), current.map(|(_, hz)| hz));
        resolution.connect_selected_notify(glib::clone!(
            #[strong]
            sizes,
            move |row| {
                if let Some((size, _)) = sizes.get(row.selected() as usize) {
                    show_rates(row.selected(), None);
                    set(&format!("mode = {size:?}"));
                }
            }
        ));
        rate.connect_selected_notify(glib::clone!(
            #[weak]
            resolution,
            move |row| {
                let Some((size, rates)) = sizes.get(resolution.selected() as usize) else {
                    return;
                };
                if let (false, Some(hz)) = (filling.get(), rates.get(row.selected() as usize)) {
                    set(&format!("mode = \"{size}@{hz}\""));
                }
            }
        ));
        display.add(&resolution);
        display.add(&rate);
    }

    // None is the density's own; a scale of the file's own is kept among the choices.
    let current_scale = config.0.get("scale").map(|_| config.number("scale", 1.0));
    let mut scales: Vec<Option<f64>> = std::iter::once(None).chain(SCALES.map(Some)).collect();
    if !scales.contains(&current_scale) {
        scales.push(current_scale);
    }
    let labels: Vec<String> = scales
        .iter()
        .map(|scale| scale.map_or("自動".into(), |scale| format!("{}%", (scale * 100.0).round())))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let scale = adw::ComboRow::builder()
        .title("縮放")
        .model(&gtk::StringList::new(&labels))
        .selected(scales.iter().position(|scale| *scale == current_scale).unwrap_or(0) as u32)
        .build();
    scale.connect_selected_notify(move |row| match scales.get(row.selected() as usize) {
        Some(Some(scale)) => set(&format!("scale = {scale:?}")),
        Some(None) => unset("scale"),
        None => {}
    });
    display.add(&scale);

    let night = adw::PreferencesGroup::new();
    let shift = choice_row(config, "夜覽", "night_shift", &NIGHT_SHIFT, &["關閉", "開啟", "排程"]);
    let from = time_row(config, "開始", "night_shift_from");
    let to = time_row(config, "結束", "night_shift_to");
    night.add(&shift);
    night.add(&from);
    night.add(&to);
    let scheduled = move |row: &adw::ComboRow| {
        let on = NIGHT_SHIFT.get(row.selected() as usize) == Some(&"scheduled");
        from.set_visible(on);
        to.set_visible(on);
    };
    scheduled(&shift);
    shift.connect_selected_notify(scheduled);
    night.add(&scale_row(
        "色溫",
        config.number("night_shift_warmth", 0.7),
        (0.0, 1.0, 0.05),
        percent,
        |warmth| set(&format!("night_shift_warmth = {warmth:.2}")),
    ));
    page.add(&night);
    page
}

const NIGHT_SHIFT: [&str; 3] = ["off", "on", "scheduled"];

/// A row choosing an hour of the day for a setting kept as `HH:MM`; a time of the file's own
/// is kept among the choices.
fn time_row(config: &Config, title: &str, key: &'static str) -> adw::ComboRow {
    let mut times: Vec<String> = (0..24).map(|hour| format!("{hour:02}:00")).collect();
    let current = config.word(key).to_string();
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

const POWER_BUTTON: [&str; 3] = ["shutdown", "sleep", "ignore"];
const LID_CLOSE: [&str; 3] = ["sleep", "lock", "ignore"];

/// What happens when the computer is idle, what its power button and lid do, and how
/// worn its batteries are.
fn power_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let idle = group("");
    idle.add(&idle_row(config, "閒置後關閉螢幕", "screen_off_after"));
    idle.add(&idle_row(config, "閒置後睡眠", "sleep_after"));
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
const PERMISSIONS: [(&str, &str, &str); 3] = [
    ("相機", "devices", "camera"),
    ("麥克風", "devices", "microphone"),
    ("定位服務", "location", "location"),
];

/// The applications that asked for each permission, with whether they have it.
fn privacy_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .inspect_err(|err| eprintln!("session bus: {err}"))
        .ok();
    // shortcut: read once, as the page is made; an application that asks while Settings is
    // open shows when it opens again.
    for (title, table, id) in PERMISSIONS {
        let rows = group(title);
        let apps = bus.as_ref().map(|bus| app_permissions(bus, table, id)).unwrap_or_default();
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
        page.add(&rows);
    }
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

fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let computer = group("");
    for (title, value) in facts() {
        computer.add(&fact_row(title, &value));
    }
    page.add(&computer);
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
.nk-dialog { padding: 24px; border-radius: 14px; background-color: var(--nekoland-sheet); box-shadow: 0 0 0 1px var(--nekoland-line), 0 30px 60px -20px rgb(0 0 0 / 50%); }
.nk-dialog-title { font-size: 16px; font-weight: 600; }
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
    .map(|(name, body)| {
        let name = text(name, &[]);
        name.set_markup(&format!("<b>{}</b>", name.label()));
        let body = text(body, &["nk-rule-body"]);
        body.set_wrap(true);
        // Narrow enough for three to a line.
        body.set_max_width_chars(20);
        let rule = column(4, &[name.upcast_ref(), body.upcast_ref()]);
        rule.set_css_classes(&["nk-card", "nk-rule"]);
        rule.upcast()
    })
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
    let body = text("上面的 3 個視窗會移到桌面 1。", &["nk-lead"]);
    let actions = line(
        8,
        &[
            gtk::Button::with_label("取消").upcast_ref(),
            button("刪除", &["destructive-action"]).upcast_ref(),
        ],
    );
    actions.set_halign(gtk::Align::End);
    actions.set_margin_top(14);
    let dialog = column(
        6,
        &[
            text("刪除桌面 2？", &["nk-dialog-title"]).upcast_ref(),
            body.upcast_ref(),
            actions.upcast_ref(),
        ],
    );
    dialog.add_css_class("nk-dialog");
    dialog.set_size_request(360, -1);
    dialog.set_halign(gtk::Align::Center);
    let scrim = column(0, &[dialog.upcast_ref()]);
    scrim.add_css_class("nk-scrim");
    let windows = [window("設定", true), window("檔案", false)];
    page.append(&section(
        "視窗與對話框",
        "標題列和內容同一張紙，沒有分隔的色帶。非作用中的視窗只把標題退成灰色。",
        &[
            cards(240, 4, &windows.iter().collect::<Vec<_>>()).upcast_ref(),
            scrim.upcast_ref(),
        ],
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
    use super::*;

    #[test]
    fn display_modes_are_grouped_by_size_largest_first() {
        let text = "1920x1080@60\n1920x1080@60\n1280x720@60\n1920x1080@144\n2560x1440@60\n";
        let (current, sizes) = display_modes(text);
        assert_eq!(current, Some(("1920x1080".to_string(), 60)));
        assert_eq!(
            sizes,
            [
                ("2560x1440".to_string(), vec![60]),
                ("1920x1080".to_string(), vec![144, 60]),
                ("1280x720".to_string(), vec![60]),
            ]
        );
        assert_eq!(display_modes(""), (None, vec![]));
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
}
