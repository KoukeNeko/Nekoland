//! `nekoland-settings`: Nekoland's settings, in GTK and libadwaita as GNOME's own settings
//! are, so that it looks as the system's GTK style says. What the compositor keeps it asks
//! for and changes through `nekoland msg`, so the compositor alone reads and writes its file
//! and puts each change in effect; the rest is the system's, through its own tools.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Settings";
const APPS_VIEWS: [&str; 4] = ["overview", "launchpad", "shelf", "sidebar"];
const WALLPAPER_MODES: [&str; 3] = ["fill", "fit", "center"];
/// Where pictures for the desktop are looked for, and how many are offered.
const WALLPAPER_DIRS: [&str; 3] = [
    "~/.local/share/backgrounds",
    "~/Pictures/Wallpapers",
    "/usr/share/backgrounds",
];
const WALLPAPERS: usize = 8;
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

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
    if let Some(reply) = output("nekoland", &["msg", &format!("set {line}")]) {
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

/// The default output's volume, 0 to 100, and whether it is muted; None without sound.
fn volume() -> Option<(f32, bool)> {
    let text = output("wpctl", &["get-volume", SINK])?;
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
        self.0.get(key).and_then(toml::Value::as_float).unwrap_or(default)
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
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build);
    let code = app.run();
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
    // A look turned on before keeps up with this version of it.
    if gtk_look_on() {
        if let Err(err) = set_gtk_look(true) {
            eprintln!("GTK look: {err}");
        }
    }
    let config = Config::load();
    let autostart = Rc::new(RefCell::new(config.autostart()));
    let pages: [(&str, gtk::Widget); 7] = [
        ("桌面", desktop_page(&config, &autostart).upcast()),
        ("外觀", appearance_page().upcast()),
        ("滑鼠與觸控板", mouse_page(&config).upcast()),
        ("聲音", sound_page().upcast()),
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

/// A row with a slider on it, from `low` to `high` in `step`s.
fn scale_row(
    title: &str,
    value: f64,
    (low, high, step): (f64, f64, f64),
    changed: impl Fn(f64) + 'static,
) -> adw::ActionRow {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, low, high, step);
    scale.set_value(value);
    scale.set_width_request(220);
    scale.set_valign(gtk::Align::Center);
    scale.connect_value_changed(move |scale| changed(scale.value()));
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&scale);
    row
}

/// The choices of how long, in seconds, the screen waits idle before it locks.
const LOCK_AFTER: [i64; 6] = [0, 60, 300, 900, 1800, 3600];

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
    dock.add(&switch_row(config, "毛玻璃", "dock_glass"));
    let magnify = switch_row(config, "放大", "dock_magnify");
    dock.add(&magnify);
    let scale = scale_row(
        "放大程度",
        config.number("dock_magnify_scale", 1.6),
        (1.2, 2.0, 0.1),
        |scale| set(&format!("dock_magnify_scale = {scale:.2}")),
    );
    magnify
        .bind_property("active", &scale, "visible")
        .sync_create()
        .build();
    dock.add(&scale);
    page.add(&dock);

    let bar = group("選單列");
    let hours = adw::ComboRow::builder()
        .title("時制")
        .model(&gtk::StringList::new(&["24 小時", "12 小時"]))
        .selected(if config.flag("clock_24h") { 0 } else { 1 })
        .build();
    hours.connect_selected_notify(|row| set(&format!("clock_24h = {}", row.selected() == 0)));
    bar.add(&hours);
    bar.add(&switch_row(config, "星期", "clock_weekday"));
    bar.add(&switch_row(config, "秒數", "clock_seconds"));
    page.add(&bar);

    let lock = group("鎖定螢幕");
    let current = config.0.get("lock_after").and_then(toml::Value::as_integer).unwrap_or(0);
    let idle = adw::ComboRow::builder()
        .title("閒置後鎖定")
        .model(&gtk::StringList::new(&["永不", "1 分鐘", "5 分鐘", "15 分鐘", "30 分鐘", "1 小時"]))
        .selected(LOCK_AFTER.iter().position(|seconds| *seconds == current).unwrap_or(0) as u32)
        .build();
    idle.connect_selected_notify(|row| {
        if let Some(seconds) = LOCK_AFTER.get(row.selected() as usize) {
            set(&format!("lock_after = {seconds}"));
        }
    });
    lock.add(&idle);
    page.add(&lock);

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
    let pictures = wallpapers(current.as_deref());
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .max_children_per_line(8)
        .min_children_per_line(4)
        .column_spacing(6)
        .row_spacing(6)
        .homogeneous(true)
        .build();
    for path in &pictures {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .width_request(96)
            .height_request(60)
            .css_classes(["card"])
            .build();
        picture.set_overflow(gtk::Overflow::Hidden);
        flow.append(&picture);
        show_thumbnail(&picture, path.clone());
    }
    if current.is_some() {
        flow.select_child(
            &flow
                .child_at_index(0)
                .expect("the wallpaper in use is listed first"),
        );
    }
    let autostart = autostart.clone();
    flow.connect_child_activated(move |_, child| {
        let Some(path) = pictures.get(child.index() as usize) else {
            return;
        };
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
    if flow.first_child().is_none() {
        wall.add(&adw::ActionRow::builder().title("沒有圖片").build());
    } else {
        wall.add(&flow);
    }
    let fill = choice_row(
        config,
        "填滿方式",
        "wallpaper_mode",
        &WALLPAPER_MODES,
        &["填滿", "符合", "置中"],
    );
    fill.set_margin_top(12);
    wall.add(&fill);
    page.add(&wall);
    page
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

fn appearance_page() -> adw::PreferencesPage {
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
    // GTK reads its user stylesheet as an application starts: other applications take the
    // change when opened again, and Settings shows it at once with a stylesheet of its own.
    let provider = gtk::CssProvider::new();
    let styles = adw::StyleManager::default();
    provider.load_from_string(&look_for(styles.is_dark()));
    styles.connect_dark_notify(glib::clone!(
        #[weak]
        provider,
        move |styles| provider.load_from_string(&look_for(styles.is_dark()))
    ));
    let read_at_start = gtk_look_on();
    look.connect_active_notify(move |row| {
        let on = row.is_active();
        if let Err(err) = set_gtk_look(on) {
            eprintln!("GTK look: {err}");
            return;
        }
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
    page.add(&style);
    page
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

/// The look for a stylesheet Settings loads itself, which GTK reads light whatever the
/// system is: when dark, its dark colours are put in outside their media query.
fn look_for(dark: bool) -> String {
    let dark_part = GTK_LOOK
        .split_once("@media (prefers-color-scheme: dark) {")
        .and_then(|(_, part)| part.trim_end().strip_suffix('}'))
        .filter(|_| dark)
        .unwrap_or_default();
    format!("{GTK_LOOK}\n{dark_part}")
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
        write_atomic(&look, GTK_LOOK)?;
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

fn mouse_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let pointer = group("");
    pointer.add(&scale_row(
        "指標速度",
        config.number("pointer_speed", 0.0),
        (-1.0, 1.0, 0.1),
        |speed| set(&format!("pointer_speed = {speed:.2}")),
    ));
    pointer.add(&switch_row(config, "自然捲動", "natural_scroll"));
    pointer.add(&switch_row(config, "輕觸以點按", "tap_to_click"));
    page.add(&pointer);
    page
}

fn sound_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let output_group = group("");
    match volume() {
        Some((level, muted)) => {
            output_group.add(&scale_row(
                "音量",
                f64::from(level.round()),
                (0.0, 100.0, 5.0),
                |level| run("wpctl", &["set-volume", SINK, &format!("{:.2}", level / 100.0)]),
            ));
            let mute = adw::SwitchRow::builder().title("靜音").active(muted).build();
            mute.connect_active_notify(|row| {
                run(
                    "wpctl",
                    &["set-mute", SINK, if row.is_active() { "1" } else { "0" }],
                )
            });
            output_group.add(&mute);
        }
        None => output_group.add(&adw::ActionRow::builder().title("沒有音訊輸出").build()),
    }
    page.add(&output_group);
    page
}

/// What starts at login, each to be taken off, and a field to add another.
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

fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let computer = group("");
    for (title, value) in facts() {
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(
            &gtk::Label::builder()
                .label(value)
                .css_classes(["dim-label"])
                .selectable(true)
                .build(),
        );
        computer.add(&row);
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
        assert_eq!(std::fs::read_to_string(dir.join(GTK_LOOK_FILE)).unwrap(), GTK_LOOK);

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
        assert_eq!(look_for(false), format!("{GTK_LOOK}\n"));
        let added = &look_for(true)[GTK_LOOK.len()..];
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
