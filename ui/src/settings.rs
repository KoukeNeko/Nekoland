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

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build);
    app.run()
}

fn build(app: &adw::Application) {
    let config = Config::load();
    let autostart = Rc::new(RefCell::new(config.autostart()));
    let pages = [
        ("桌面", desktop_page(&config, &autostart)),
        ("外觀", appearance_page()),
        ("滑鼠與觸控板", mouse_page(&config)),
        ("聲音", sound_page()),
        ("啟動項目", startup_page(&autostart)),
        ("關於", about_page()),
        ("設計語言", design_page()),
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
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

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
    page.add(&style);
    page
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

/// GTK's own widgets, as the system's style draws them: Nekoland's design language reaches
/// every GTK application through that style, so what changes there changes here too.
fn design_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let bar = |widgets: &[&gtk::Widget]| {
        let row = gtk::Box::builder()
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        for widget in widgets {
            row.append(*widget);
        }
        row
    };
    let button = |label: &str, class: Option<&str>| {
        let button = gtk::Button::with_label(label);
        if let Some(class) = class {
            button.add_css_class(class);
        }
        button
    };

    let buttons = group("按鈕");
    buttons.add(&bar(&[
        button("儲存", Some("suggested-action")).upcast_ref(),
        button("取消", None).upcast_ref(),
        button("略過", Some("flat")).upcast_ref(),
        button("刪除", Some("destructive-action")).upcast_ref(),
        button("新增", Some("pill")).upcast_ref(),
    ]));
    page.add(&buttons);

    let choices = group("選擇");
    choices.add(&adw::SwitchRow::builder().title("開關").active(true).build());
    let check = adw::ActionRow::builder().title("核取方塊").build();
    let checked = gtk::CheckButton::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    check.add_prefix(&checked);
    check.set_activatable_widget(Some(&checked));
    choices.add(&check);
    let first = gtk::CheckButton::with_label("底部");
    let second = gtk::CheckButton::with_label("左側");
    second.set_group(Some(&first));
    first.set_active(true);
    let radios = adw::ActionRow::builder().title("選項").build();
    radios.add_suffix(&bar(&[first.upcast_ref(), second.upcast_ref()]));
    choices.add(&radios);
    choices.add(
        &adw::ComboRow::builder()
            .title("下拉選單")
            .model(&gtk::StringList::new(&["自動", "淺色", "深色"]))
            .build(),
    );
    page.add(&choices);

    let inputs = group("輸入");
    inputs.add(&adw::EntryRow::builder().title("名稱").build());
    inputs.add(&adw::PasswordEntryRow::builder().title("密碼").build());
    let number = adw::SpinRow::with_range(0.0, 100.0, 1.0);
    number.set_title("數量");
    inputs.add(&number);
    page.add(&inputs);

    let values = group("數值與進度");
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_value(60.0);
    scale.set_hexpand(true);
    values.add(&scale);
    let progress = gtk::ProgressBar::builder()
        .fraction(0.6)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    scale
        .adjustment()
        .bind_property("value", &progress, "fraction")
        .transform_to(|_, value: f64| Some(value / 100.0))
        .build();
    values.add(&progress);
    values.add(&bar(&[gtk::Spinner::builder()
        .spinning(true)
        .build()
        .upcast_ref()]));
    page.add(&values);
    page
}

fn save_autostart(autostart: &[String]) {
    let quoted: Vec<String> = autostart.iter().map(|command| format!("{command:?}")).collect();
    set(&format!("autostart = [{}]", quoted.join(", ")));
}
