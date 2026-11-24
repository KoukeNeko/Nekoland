//! 系統監控: a page for each of the readings the menu bar can show, as GNOME-Stats sets them:
//! what it shows in the bar, how it is read, its panel, and what it notifies of.

use super::*;

/// A reading: its title, the name its settings are under, and what it can show in the bar.
struct Reading {
    title: &'static str,
    key: &'static str,
    widgets: &'static [&'static str],
}

const READINGS: [Reading; 6] = [
    Reading {
        title: "CPU",
        key: "cpu",
        widgets: &[
            "mini",
            "line_chart",
            "bar_chart",
            "cores",
            "pie_chart",
            "tachometer",
            "ring",
            "label",
            "text",
            "icon",
        ],
    },
    Reading {
        title: "GPU",
        key: "gpu",
        widgets: &[
            "mini",
            "line_chart",
            "bar_chart",
            "tachometer",
            "ring",
            "label",
            "text",
            "icon",
        ],
    },
    Reading {
        title: "記憶體",
        key: "ram",
        widgets: &[
            "mini",
            "line_chart",
            "bar_chart",
            "pie_chart",
            "memory",
            "tachometer",
            "ring",
            "state",
            "label",
            "text",
            "icon",
        ],
    },
    Reading {
        title: "磁碟",
        key: "disk",
        widgets: &[
            "mini",
            "memory",
            "speed",
            "network_chart",
            "bar_chart",
            "pie_chart",
            "ring",
            "label",
            "text",
            "icon",
        ],
    },
    Reading {
        title: "網路",
        key: "network",
        widgets: &[
            "speed",
            "network_chart",
            "bar_chart",
            "state",
            "label",
            "text",
            "icon",
        ],
    },
    Reading {
        title: "感應器",
        key: "sensors",
        widgets: &[
            "mini",
            "sensors",
            "line_chart",
            "bar_chart",
            "tachometer",
            "ring",
            "label",
            "text",
            "icon",
        ],
    },
];

/// Each widget's name in the bar's settings.
const WIDGET_NAMES: [(&str, &str); 15] = [
    ("label", "名稱"),
    ("mini", "數值"),
    ("line_chart", "折線圖"),
    ("bar_chart", "長條圖"),
    ("cores", "核心"),
    ("pie_chart", "圓餅圖"),
    ("tachometer", "儀表"),
    ("ring", "圓環"),
    ("memory", "用量"),
    ("speed", "速度"),
    ("network_chart", "流量圖"),
    ("sensors", "讀數"),
    ("text", "文字"),
    ("state", "狀態"),
    ("icon", "圖示"),
];

const COLORS: [&str; 2] = ["alert", "mono"];
const FRAMES: [&str; 3] = ["none", "box", "frame"];
const SCALES: [&str; 4] = ["linear", "square", "cube", "log"];
const SCALE_NAMES: [&str; 4] = ["線性", "平方根", "立方根", "對數"];
const ROWS: [&str; 2] = ["two", "one"];
const SYMBOLS: [&str; 4] = ["arrows", "dots", "letters", "none"];
const WIDGET_HISTORY: [i64; 5] = [12, 24, 36, 48, 60];
const INTERVALS: [i64; 5] = [1, 2, 3, 5, 10];
const INTERVAL_NAMES: [&str; 5] = ["每秒", "每 2 秒", "每 3 秒", "每 5 秒", "每 10 秒"];
const PROCESS_COUNTS: [i64; 4] = [0, 3, 5, 8];
const PROCESS_NAMES: [&str; 4] = ["不顯示", "3", "5", "8"];
const PANEL_HISTORY: [i64; 4] = [60, 120, 180, 300];
const USAGE_ALERTS: [i64; 5] = [0, 70, 80, 90, 95];
const USAGE_ALERT_NAMES: [&str; 5] = ["關閉", "70%", "80%", "90%", "95%"];
const TEMPERATURE_ALERTS: [i64; 5] = [0, 70, 80, 90, 95];
const TEMPERATURE_ALERT_NAMES: [&str; 5] = ["關閉", "70 °C", "80 °C", "90 °C", "95 °C"];
const POWER_ALERTS: [i64; 6] = [0, 100, 200, 300, 400, 500];
const POWER_ALERT_NAMES: [&str; 6] = ["關閉", "100 W", "200 W", "300 W", "400 W", "500 W"];

fn widget_name(kind: &str) -> &'static str {
    WIDGET_NAMES
        .iter()
        .find(|(name, _)| *name == kind)
        .map_or("", |(_, title)| title)
}

/// Whether the compositor has anything to read for it, as it reads it.
fn readable(key: &str) -> bool {
    match key {
        "gpu" => has_gpu_reading(),
        // Temperatures, or RAPL's, a battery's or the GPU's power, as the compositor adds it up.
        "sensors" => has_temperatures() || has_power(),
        _ => true,
    }
}

fn has_power() -> bool {
    Path::new("/sys/class/powercap/intel-rapl:0").exists()
        || Path::new("/sys/class/power_supply/BAT0").exists()
        || has_gpu_reading()
}

/// The names the compositor reads the sensors and the GPUs by, each with its kind.
fn reading_names() -> Vec<(String, String)> {
    output("nekoland", &["msg", "stats-names"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(kind, name)| (kind.to_string(), name.to_string()))
        })
        .collect()
}

/// 系統監控: a row for each reading, opening its page, and what they share.
pub(crate) fn stats_page(config: &Config) -> adw::PreferencesPage {
    let readings = group("");
    for reading in READINGS.iter().filter(|reading| readable(reading.key)) {
        let summary = move || {
            let config = Config::load();
            let widgets = config
                .0
                .get(&format!("stats_{}_widgets", reading.key))
                .and_then(toml::Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !config.flag(&format!("stats_{}", reading.key)) {
                return "關閉".into();
            }
            let names: Vec<&str> = widgets
                .iter()
                .filter_map(|widget| Some(widget_name(widget.get("kind")?.as_str()?)))
                .collect();
            names.join("・")
        };
        readings.add(&subpage_row(
            reading.title,
            summary,
            reading_page(config, reading),
        ));
    }
    let shared = group("");
    shared.add(&switch_row(config, "合併顯示", "stats_combined"));
    shared.add(&text_row(config, "自訂文字", "stats_text"));
    let units = adw::ComboRow::builder()
        .title("溫度單位")
        .model(&gtk::StringList::new(&["攝氏", "華氏"]))
        .selected(u32::from(config.flag("stats_fahrenheit")))
        .build();
    units.connect_selected_notify(|row| set(&format!("stats_fahrenheit = {}", row.selected() == 1)));
    shared.add(&units);
    page_of_groups(&[readings, shared])
}

/// The readings turned on, or 無.
pub(crate) fn stats_summary() -> String {
    let config = Config::load();
    let shown: Vec<&str> = READINGS
        .iter()
        .filter(|reading| config.flag(&format!("stats_{}", reading.key)))
        .map(|reading| reading.title)
        .collect();
    if shown.is_empty() {
        "無".into()
    } else {
        shown.join("・")
    }
}

/// A setting's key for the reading, as `stats_KEY_NAME`, kept for the life of the page.
fn key_of(reading: &Reading, name: &str) -> &'static str {
    let key = if name.is_empty() {
        format!("stats_{}", reading.key)
    } else {
        format!("stats_{}_{name}", reading.key)
    };
    Box::leak(key.into_boxed_str())
}

fn reading_page(config: &Config, reading: &'static Reading) -> adw::PreferencesPage {
    let key = |name: &str| key_of(reading, name);
    let bar = group("選單列");
    bar.add(&switch_row(config, "顯示", key("")));
    widget_rows(config, &bar, reading);
    if reading.key == "sensors" {
        bar.add(&shown_sensors_row(config));
    }

    let read = group("");
    read.add(&integer_choice_row(
        config,
        "更新頻率",
        key("interval"),
        (&INTERVALS, &INTERVAL_NAMES),
        2,
    ));
    if matches!(reading.key, "cpu" | "ram" | "disk" | "network") {
        read.add(&integer_choice_row(
            config,
            "程序數量",
            key("processes"),
            (&PROCESS_COUNTS, &PROCESS_NAMES),
            5,
        ));
    }
    match reading.key {
        "gpu" => {
            let gpus: Vec<String> = reading_names()
                .into_iter()
                .filter(|(kind, _)| kind == "gpu")
                .map(|(_, name)| name)
                .collect();
            read.add(&named_choice_row(
                config,
                "顯示的 GPU",
                "stats_gpu_shown",
                ("自動", ""),
                &gpus,
            ));
        }
        "disk" => {
            read.add(&named_choice_row(
                config,
                "磁碟",
                "stats_disk_mount",
                ("根目錄", "/"),
                &mounts(),
            ));
        }
        "network" => {
            let interfaces: Vec<String> = std::fs::read_dir("/sys/class/net")
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name != "lo")
                .collect();
            read.add(&named_choice_row(
                config,
                "介面",
                "stats_network_interface",
                ("自動", ""),
                &interfaces,
            ));
            let units = adw::ComboRow::builder()
                .title("速度單位")
                .model(&gtk::StringList::new(&["位元組", "位元"]))
                .selected(u32::from(config.flag("stats_network_bits")))
                .build();
            units
                .connect_selected_notify(|row| set(&format!("stats_network_bits = {}", row.selected() == 1)));
            read.add(&units);
            read.add(&integer_choice_row(
                config,
                "啟用門檻",
                "stats_network_threshold",
                (&[0, 1, 10, 100], &["無", "1 KB/s", "10 KB/s", "100 KB/s"]),
                0,
            ));
            read.add(&switch_row(config, "公用 IP", "stats_network_public_ip"));
        }
        "sensors" if has_fan_control() => read.add(&switch_row(config, "風扇控制", "stats_fan_control")),
        _ => {}
    }

    let panel = group("面板");
    panel.add(&integer_choice_row(
        config,
        "圖表筆數",
        key("history"),
        (&PANEL_HISTORY, &["60", "120", "180", "300"]),
        60,
    ));
    match reading.key {
        "cpu" => panel.add(&switch_row(config, "核心", "stats_cpu_cores")),
        "disk" => {
            panel.add(&choice_row(
                config,
                "圖表縮放",
                "stats_disk_chart_scale",
                &SCALES,
                &SCALE_NAMES,
            ));
            panel.add(&switch_row(config, "SMART", "stats_disk_smart"));
        }
        "network" => {
            panel.add(&choice_row(
                config,
                "圖表縮放",
                "stats_network_chart_scale",
                &SCALES,
                &SCALE_NAMES,
            ));
            panel.add(&switch_row(config, "上傳在前", "stats_network_reverse"));
        }
        _ => {}
    }

    let alerts = group("通知");
    let usage = |title: &str, name: &str| {
        integer_choice_row(config, title, key(name), (&USAGE_ALERTS, &USAGE_ALERT_NAMES), 0)
    };
    match reading.key {
        "cpu" => {
            alerts.add(&usage("使用率", "alert"));
            alerts.add(&usage("系統使用率", "alert_system"));
            alerts.add(&usage("使用者使用率", "alert_user"));
        }
        "gpu" => alerts.add(&usage("使用率", "alert")),
        "ram" => {
            alerts.add(&usage("使用量", "alert"));
            alerts.add(&integer_choice_row(
                config,
                "可用低於",
                key("alert_free"),
                (&[0, 5, 10, 20], &["關閉", "5%", "10%", "20%"]),
                0,
            ));
            alerts.add(&usage("置換空間", "alert_swap"));
            alerts.add(&integer_choice_row(
                config,
                "記憶體壓力",
                key("alert_pressure"),
                (&[0, 10, 30, 50], &["關閉", "10%", "30%", "50%"]),
                0,
            ));
        }
        "disk" => alerts.add(&usage("使用量", "alert")),
        "network" => {
            alerts.add(&switch_row(config, "連線", "stats_network_alert_connection"));
            alerts.add(&switch_row(config, "介面", "stats_network_alert_interface"));
            alerts.add(&switch_row(config, "本機 IP", "stats_network_alert_address"));
            alerts.add(&switch_row(config, "公用 IP", "stats_network_alert_public_ip"));
        }
        _ => sensor_alert_rows(config, &alerts),
    }
    page_of_groups(&[bar, read, panel, alerts])
}

/// The mounted filesystems of disks, by where they are mounted, the root's left to its own choice.
fn mounts() -> Vec<String> {
    std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let device = fields.next()?;
            let mount = fields.next()?.replace("\\040", " ");
            (device.starts_with("/dev/") && mount != "/" && !mount.starts_with("/boot")).then_some(mount)
        })
        .collect()
}

/// A row choosing one of `names` for a text setting, or `fallback`'s value under its title.
fn named_choice_row(
    config: &Config,
    title: &str,
    key: &'static str,
    fallback: (&str, &'static str),
    names: &[String],
) -> adw::ComboRow {
    let current = config.word(key);
    let mut labels = vec![fallback.0.to_string()];
    labels.extend(names.iter().cloned());
    // One set that is not there now is kept as a choice of its own.
    if !current.is_empty() && current != fallback.1 && !names.iter().any(|name| name == current) {
        labels.push(current.to_string());
    }
    let values: Vec<String> = std::iter::once(fallback.1.to_string())
        .chain(labels[1..].iter().cloned())
        .collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        ))
        .selected(values.iter().position(|value| value == current).unwrap_or(0) as u32)
        .build();
    row.connect_selected_notify(move |row| {
        if let Some(value) = values.get(row.selected() as usize) {
            set(&format!("{key} = {}", toml_string(value)));
        }
    });
    row
}

/// The widgets a reading shows in the bar, in order, each opening its options, above the row
/// that adds one; listed again after each change of their order.
#[derive(Clone)]
struct WidgetList {
    bar: adw::PreferencesGroup,
    add: adw::ActionRow,
    widgets: Rc<RefCell<Vec<toml::Table>>>,
    rows: Rc<RefCell<Vec<adw::ExpanderRow>>>,
    reading: &'static Reading,
}

impl WidgetList {
    fn save(&self) {
        let value = toml::Value::Array(
            self.widgets
                .borrow()
                .iter()
                .cloned()
                .map(toml::Value::Table)
                .collect(),
        );
        set(&format!("{} = {value}", key_of(self.reading, "widgets")));
    }

    fn show(&self) {
        for row in self.rows.borrow_mut().drain(..) {
            self.bar.remove(&row);
        }
        self.bar.remove(&self.add);
        let widgets = self.widgets.borrow().clone();
        for (index, widget) in widgets.iter().enumerate() {
            let kind = widget.get("kind").and_then(toml::Value::as_str).unwrap_or("mini");
            let row = adw::ExpanderRow::builder().title(widget_name(kind)).build();
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
            // One is kept at least, which is how the reading is in the bar.
            let remove = button("user-trash-symbolic", "移除", widgets.len() > 1);
            for (button, moved) in [(&up, true), (&remove, false)] {
                let list = self.clone();
                button.connect_clicked(move |_| {
                    if moved {
                        list.widgets.borrow_mut().swap(index - 1, index);
                    } else {
                        list.widgets.borrow_mut().remove(index);
                    }
                    list.save();
                    list.show();
                });
            }
            // An expander row puts the later suffix first.
            row.add_suffix(&remove);
            row.add_suffix(&up);
            let options = self.options(kind, widget, index);
            row.set_enable_expansion(!options.is_empty());
            for option in options {
                row.add_row(&option);
            }
            self.bar.add(&row);
            self.rows.borrow_mut().push(row);
        }
        self.bar.add(&self.add);
    }
    /// The rows of one widget's options, as its kind has them.
    fn options(&self, kind: &str, widget: &toml::Table, index: usize) -> Vec<gtk::Widget> {
        let update = {
            let list = self.clone();
            Rc::new(move |field: &str, value: toml::Value| {
                if let Some(widget) = list.widgets.borrow_mut().get_mut(index) {
                    widget.insert(field.into(), value);
                }
                list.save();
            })
        };
        let reading = self.reading;
        let flag = |field: &str| widget.get(field).and_then(toml::Value::as_bool);
        let word = |field: &str| {
            widget
                .get(field)
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let mut options: Vec<gtk::Widget> = Vec::new();
        let switch = |title: &str, field: &'static str, default: bool| {
            let row = adw::SwitchRow::builder()
                .title(title)
                .active(flag(field).unwrap_or(default))
                .build();
            let update = update.clone();
            row.connect_active_notify(move |row| update(field, row.is_active().into()));
            row.upcast::<gtk::Widget>()
        };
        let choice = |title: &str, field: &'static str, names: &'static [&'static str], labels: &[&str]| {
            let current = word(field);
            let row = adw::ComboRow::builder()
                .title(title)
                .model(&gtk::StringList::new(labels))
                .selected(names.iter().position(|name| *name == current).unwrap_or(0) as u32)
                .build();
            let update = update.clone();
            row.connect_selected_notify(move |row| {
                if let Some(name) = names.get(row.selected() as usize) {
                    update(field, (*name).into());
                }
            });
            row.upcast::<gtk::Widget>()
        };
        let labelled = [
            "mini",
            "line_chart",
            "bar_chart",
            "cores",
            "pie_chart",
            "tachometer",
            "ring",
            "network_chart",
        ];
        let colored = [
            "mini",
            "line_chart",
            "bar_chart",
            "cores",
            "pie_chart",
            "tachometer",
            "ring",
            "memory",
            "sensors",
        ];
        let speeds = matches!(reading.key, "disk" | "network");
        if labelled.contains(&kind) {
            options.push(switch("名稱", "label", true));
        }
        if colored.contains(&kind) {
            options.push(choice("顏色", "color", &COLORS, &["警示色", "單色"]));
        }
        if matches!(kind, "line_chart" | "network_chart") {
            options.push(choice("背景", "frame", &FRAMES, &["無", "底色", "外框"]));
            let current = widget
                .get("history")
                .and_then(toml::Value::as_integer)
                .unwrap_or(24);
            let row = adw::ComboRow::builder()
                .title("筆數")
                .model(&gtk::StringList::new(&["12", "24", "36", "48", "60"]))
                .selected(
                    WIDGET_HISTORY
                        .iter()
                        .position(|count| *count == current)
                        .unwrap_or(1) as u32,
                )
                .build();
            let update = update.clone();
            row.connect_selected_notify(move |row| {
                if let Some(count) = WIDGET_HISTORY.get(row.selected() as usize) {
                    update("history", (*count).into());
                }
            });
            options.push(row.upcast());
        }
        if kind == "line_chart" && !speeds {
            options.push(switch("數值", "value", false));
        }
        if kind == "network_chart" || kind == "line_chart" && speeds {
            options.push(choice("縮放", "scale", &SCALES, &SCALE_NAMES));
        }
        match kind {
            "network_chart" | "speed" => options.push(switch("順序對調", "reverse", false)),
            "memory" => options.push(switch("顯示可用", "reverse", false)),
            _ => {}
        }
        if matches!(kind, "speed" | "sensors") {
            options.push(choice("列數", "rows", &ROWS, &["雙列", "單列"]));
        }
        if kind == "speed" {
            options.push(choice(
                "符號",
                "symbols",
                &SYMBOLS,
                &["箭頭", "圓點", "字母", "無"],
            ));
        }
        if kind == "text" {
            let row = adw::EntryRow::builder()
                .title("文字")
                .text(word("text"))
                .show_apply_button(true)
                .build();
            let update = update.clone();
            row.connect_apply(move |row| update("text", row.text().as_str().into()));
            options.push(row.upcast());
        }
        options
    }
}

/// The reading's widgets, and a row that adds one, in the group of its bar.
fn widget_rows(config: &Config, bar: &adw::PreferencesGroup, reading: &'static Reading) {
    let widgets: Vec<toml::Table> = config
        .0
        .get(key_of(reading, "widgets"))
        .and_then(toml::Value::as_array)
        .map(|widgets| {
            widgets
                .iter()
                .filter_map(|widget| widget.as_table().cloned())
                .collect()
        })
        .unwrap_or_default();
    let choices = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let popover = gtk::Popover::builder().child(&choices).build();
    let menu = gtk::MenuButton::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("新增元件")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .popover(&popover)
        .build();
    let add = adw::ActionRow::builder().title("新增元件").build();
    add.add_suffix(&menu);
    add.set_activatable_widget(Some(&menu));
    bar.add(&add);
    let list = WidgetList {
        bar: bar.clone(),
        add,
        widgets: Rc::new(RefCell::new(widgets)),
        rows: Rc::default(),
        reading,
    };
    for kind in reading.widgets {
        let button = gtk::Button::builder()
            .label(widget_name(kind))
            .css_classes(["flat"])
            .build();
        let list = list.clone();
        button.connect_clicked(glib::clone!(
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                let mut widget = toml::Table::new();
                widget.insert("kind".into(), (*kind).into());
                list.widgets.borrow_mut().push(widget);
                list.save();
                list.show();
            }
        ));
        choices.append(&button);
    }
    list.show();
}

/// The sensors' readings the bar shows, in the order turned on: the hottest temperature, the
/// power, and each one by its name.
fn shown_sensors_row(config: &Config) -> adw::ExpanderRow {
    let row = adw::ExpanderRow::builder().title("顯示的讀數").build();
    let shown: Vec<String> = config
        .0
        .get("stats_sensors_shown")
        .and_then(toml::Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_else(|| vec!["hottest".into()]);
    let shown = Rc::new(RefCell::new(shown));
    let mut names: Vec<(String, String)> = vec![("hottest".into(), "最熱溫度".into())];
    if has_power() {
        names.push(("power".into(), "總功耗".into()));
    }
    names.extend(
        reading_names()
            .into_iter()
            .filter(|(kind, _)| matches!(kind.as_str(), "temperature" | "fan" | "voltage"))
            .map(|(_, name)| (name.clone(), name)),
    );
    row.set_subtitle(&summary_of(&shown.borrow(), &names));
    let names = Rc::new(names);
    for (name, title) in names.iter() {
        let switch = adw::SwitchRow::builder()
            .title(title)
            .active(shown.borrow().contains(name))
            .build();
        let (name, shown, names) = (name.clone(), shown.clone(), names.clone());
        switch.connect_active_notify(glib::clone!(
            #[weak]
            row,
            move |switch| {
                let mut list = shown.borrow_mut();
                list.retain(|shown| *shown != name);
                if switch.is_active() {
                    list.push(name.clone());
                }
                // One is always shown.
                if list.is_empty() {
                    list.push(name.clone());
                    drop(list);
                    switch.set_active(true);
                    return;
                }
                let value =
                    toml::Value::Array(list.iter().map(|name| toml::Value::from(name.as_str())).collect());
                set(&format!("stats_sensors_shown = {value}"));
                row.set_subtitle(&summary_of(&list, &names));
            }
        ));
        row.add_row(&switch);
    }
    row
}

fn summary_of(shown: &[String], names: &[(String, String)]) -> String {
    shown
        .iter()
        .filter_map(|name| {
            names
                .iter()
                .find(|(found, _)| found == name)
                .map(|(_, title)| title.as_str())
        })
        .collect::<Vec<_>>()
        .join("・")
}

/// A threshold for the hottest temperature, the power, and each temperature by its name.
fn sensor_alert_rows(config: &Config, alerts: &adw::PreferencesGroup) {
    let limits = config
        .0
        .get("stats_sensors_alerts")
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default();
    let limits = Rc::new(RefCell::new(limits));
    let mut names: Vec<(String, String, bool)> = vec![("hottest".into(), "最熱溫度".into(), false)];
    if has_power() {
        names.push(("power".into(), "總功耗".into(), true));
    }
    names.extend(
        reading_names()
            .into_iter()
            .filter(|(kind, _)| kind == "temperature")
            .map(|(_, name)| (name.clone(), name, false)),
    );
    for (name, title, power) in names {
        let (values, labels): (&[i64], &[&str]) = if power {
            (&POWER_ALERTS, &POWER_ALERT_NAMES)
        } else {
            (&TEMPERATURE_ALERTS, &TEMPERATURE_ALERT_NAMES)
        };
        let current = limits
            .borrow()
            .get(&name)
            .and_then(toml::Value::as_integer)
            .unwrap_or(0);
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(labels))
            .selected(values.iter().position(|value| *value == current).unwrap_or(0) as u32)
            .build();
        let limits = limits.clone();
        row.connect_selected_notify(move |row| {
            let Some(value) = values.get(row.selected() as usize) else {
                return;
            };
            let mut limits = limits.borrow_mut();
            if *value == 0 {
                limits.remove(&name);
            } else {
                limits.insert(name.clone(), (*value).into());
            }
            set(&format!(
                "stats_sensors_alerts = {}",
                toml::Value::Table(limits.clone())
            ));
        });
        alerts.add(&row);
    }
}
