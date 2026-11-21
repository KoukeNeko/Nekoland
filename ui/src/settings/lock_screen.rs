//! The lock screen page and the pages under it.

use super::*;

/// How nekoland-lock looks, and when the screen locks by itself.
pub(crate) const LOCK_STYLES: [&str; 3] = ["quiet", "poster", "dial"];
pub(crate) const LOCK_NOTICES: [&str; 3] = ["hidden", "count", "full"];
pub(crate) const LOCK_WEIGHTS: [&str; 3] = ["light", "regular", "bold"];
/// The clock's colours to pick from, after the style's own: white, and colours that read on
/// ink as the wallpaper's would.
pub(crate) const LOCK_CLOCK_COLOURS: [&str; 6] = ["#f1efea", "#f2b28a", "#9cc4e6", "#a8d8b9", "#ff8f6b", "#c3b1e1"];
pub(crate) const LOCK_CLOCK_SIZES: [&str; 3] = ["small", "medium", "large"];
pub(crate) const LOCK_CLOCK_ALIGNS: [&str; 3] = ["left", "center", "right"];
pub(crate) const LOCK_EFFECTS: [&str; 13] =
    ["lift", "fade", "slide", "expand", "spread", "wave", "warp", "ripple", "mosaic", "wiper", "paint", "glitch", "swirl"];
pub(crate) const LOCK_EFFECT_NAMES: [&str; 13] =
    ["往上掀起", "淡出", "滑開", "放大淡出", "擴散", "波浪", "拉伸", "漣漪", "馬賽克", "雨刷", "刷除", "故障", "漩渦"];
/// The effects that go one way, and those drawn in a colour of their own.
pub(crate) const LOCK_EFFECTS_ONE_WAY: [&str; 5] = ["slide", "wave", "wiper", "paint", "glitch"];
pub(crate) const LOCK_EFFECTS_COLOURED: [&str; 4] = ["spread", "ripple", "paint", "glitch"];
pub(crate) const LOCK_DIRECTIONS: [&str; 4] = ["left", "right", "up", "down"];
pub(crate) const LOCK_EASINGS: [&str; 3] = ["out", "inout", "linear"];
pub(crate) const LOCK_ICONS: [&str; 7] = ["none", "padlock", "shield", "key", "heart", "paw", "picture"];
pub(crate) const LOCK_NUMERALS: [&str; 3] = ["none", "arabic", "roman"];
pub(crate) const LOCK_HANDS: [&str; 3] = ["classic", "thin", "bold"];
pub(crate) const LOCK_SHORTCUT_LAYOUTS: [&str; 3] = ["corners", "row", "column"];
pub(crate) const LOCK_FINGER_EFFECTS: [&str; 3] = ["ripple", "glow", "none"];
/// How long unlocking takes, in milliseconds: quick, as it was, and slow.
pub(crate) const LOCK_DURATIONS: [i64; 3] = [320, 560, 900];
/// Seconds idle while locked before the displays go dark; 0 for as they otherwise do.
pub(crate) const LOCK_SCREEN_OFF: [i64; 4] = [0, 10, 30, 60];
/// The second clock's zones, and the city each is shown as.
pub(crate) const WORLD_CLOCKS: [(&str, &str); 10] = [
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
pub(crate) const WEATHER_PLACES: [(&str, [f64; 2]); 14] = [
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
pub(crate) const LOCK_ROW_SHORTCUTS: usize = 6;
/// The widgets the lock can show, by name, with their titles.
pub(crate) const LOCK_WIDGETS: [(&str, &str); 5] =
    [("battery", "電池"), ("playing", "正在播放"), ("calendar", "今天的行程"), ("world_clock", "世界時鐘"), ("weather", "天氣")];
/// How tall the clock's page draws the lock.
pub(crate) const LOCK_SUBPAGE_PREVIEW: i32 = 180;

/// A button that asks for a picture, and gives its path to `chosen`.
pub(crate) fn picture_button(chosen: impl Fn(std::path::PathBuf) + 'static) -> gtk::Button {
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
pub(crate) fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |name| name.to_string_lossy().into_owned())
}

/// `#rrggbb` for a colour picked.
pub(crate) fn hex_of(colour: &gdk::RGBA) -> String {
    let channel = |value: f32| (value * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", channel(colour.red()), channel(colour.green()), channel(colour.blue()))
}

/// A button that takes what a row chose away.
pub(crate) fn clear_button(tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

/// A row choosing the lock's colour for `key`; cleared, the lock's own, which the button
/// shows as `own`.
pub(crate) fn lock_colour_row(config: &Config, title: &str, key: &'static str, own: &'static str, refresh: &Rc<dyn Fn()>) -> adw::ActionRow {
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
pub(crate) fn lock_picture_row(config: &Config, title: &str, key: &'static str, refresh: &Rc<dyn Fn()>) -> adw::ActionRow {
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
pub(crate) fn lock_place_row(config: &Config, title: &str, key: &'static str, default: [f64; 2], refresh: &Rc<dyn Fn()>) -> adw::ExpanderRow {
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
pub(crate) struct Stickers {
    pub(crate) list: RefCell<Vec<(String, [f64; 2], f64)>>,
    pub(crate) rows: RefCell<Vec<gtk::Widget>>,
    /// Whether the one sticker Settings first kept is still written apart, to be folded in.
    pub(crate) apart: Cell<bool>,
    pub(crate) refresh: Rc<dyn Fn()>,
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
pub(crate) fn fill_stickers(group: &adw::PreferencesGroup, stickers: &Rc<Stickers>) {
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
pub(crate) fn lock_preview_size() -> (i32, i32) {
    gdk::Display::default()
        .and_then(|display| display.monitors().item(0))
        .and_then(|monitor| monitor.downcast::<gdk::Monitor>().ok())
        .map(|monitor| monitor.geometry())
        .map_or((1440, 900), |area| (area.width(), area.height()))
}

/// The lock as it looks now, or as `style` would look picked, drawn by nekoland-lock into a
/// picture, and drawn again with `refresh` once a setting changed.
pub(crate) fn lock_preview(style: Option<&'static str>, height: i32) -> (gtk::Picture, Rc<dyn Fn()>) {
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
            let program = program::path("nekoland-lock");
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
pub(crate) fn lock_page(config: &Config) -> adw::PreferencesPage {
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
pub(crate) fn lock_widgets(config: &Config) -> Vec<String> {
    config
        .0
        .get("lock_widgets")
        .and_then(toml::Value::as_array)
        .map_or_else(|| vec!["battery".into(), "playing".into()], |listed| {
            listed.iter().filter_map(toml::Value::as_str).map(String::from).collect()
        })
}

pub(crate) fn lock_clock_summary() -> String {
    let config = Config::load();
    let label = |names: &[&str], labels: [&'static str; 3], key: &str| {
        names.iter().position(|name| *name == config.word(key)).map_or(labels[1], |at| labels[at])
    };
    let size = label(&LOCK_CLOCK_SIZES, ["小", "中", "大"], "lock_clock_size");
    let align = label(&LOCK_CLOCK_ALIGNS, ["靠左", "置中", "靠右"], "lock_clock_align");
    [config.word("lock_clock_font"), size, align].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("・")
}

pub(crate) fn lock_widgets_summary() -> String {
    let listed = lock_widgets(&Config::load());
    let titles: Vec<&str> = LOCK_WIDGETS
        .iter()
        .filter(|(name, _)| listed.iter().any(|listed| listed == name))
        .map(|(_, title)| *title)
        .collect();
    if titles.is_empty() { "無".into() } else { titles.join("、") }
}

pub(crate) fn lock_background_summary() -> String {
    let config = Config::load();
    let on = |key: &str| config.0.get(key).and_then(toml::Value::as_bool);
    match (on("lock_wallpaper").unwrap_or(true), on("lock_blur").unwrap_or(false)) {
        (false, _) => "不顯示桌布".into(),
        (true, true) => "桌布・模糊".into(),
        (true, false) => "桌布".into(),
    }
}

pub(crate) fn lock_notices_summary() -> String {
    let config = Config::load();
    let at = LOCK_NOTICES.iter().position(|name| *name == config.word("lock_notices")).unwrap_or(1);
    format!("通知{}", ["不顯示", "只顯示數量", "顯示內容"][at])
}

pub(crate) fn lock_unlock_summary() -> String {
    let config = Config::load();
    let at = LOCK_EFFECTS.iter().position(|name| *name == config.word("lock_unlock_effect")).unwrap_or(0);
    LOCK_EFFECT_NAMES[at].into()
}

pub(crate) fn on_off(on: bool) -> String {
    if on { "開啟" } else { "關閉" }.into()
}
