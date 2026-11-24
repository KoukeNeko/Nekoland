//! Settings' 顯示 page: with more than one display, their picture and how they are used;
//! the chosen display's own settings, the sizes it can look like first as on the Mac; then
//! what every display shares, Night Shift in a dialog of its own.

use std::{
    cell::{Cell, RefCell},
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    rc::{Rc, Weak},
    time::Duration,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{group, line, output, run, set, switch_row, unset, Config};

/// The scales the resolution list offers on the display's own mode, the sizes it then looks
/// like running from the largest text to the most room.
const SCALES: [f64; 5] = [2.0, 1.75, 1.5, 1.25, 1.0];
/// Sizes shorter than this, in points, leave too little room to be offered.
const LEAST_HEIGHT: i32 = 600;
/// Scales closer than this are the same one.
const SAME_SCALE: f64 = 0.01;
const ROTATIONS: [(&str, i64); 4] = [("標準", 0), ("90°", 90), ("180°", 180), ("270°", 270)];
/// How long, in seconds, a new rotation waits to be kept before it is turned back, as on the
/// Mac, for a picture turned so that it cannot be read.
const KEEP_ROTATION_WITHIN: u32 = 15;
const COLOR_DEPTHS: [(&str, i64); 2] = [("8 位元", 8), ("10 位元", 10)];
const NIGHT_SHIFT: [&str; 4] = ["off", "on", "sunset", "scheduled"];
const NIGHT_SHIFT_LABELS: [&str; 4] = ["關閉", "開啟", "日落到日出", "自訂"];
const SUNSET: &str = "sunset";
const SCHEDULED: &str = "scheduled";
const DEFAULT_WARMTH: f64 = 0.7;
const NIGHT_SHIFT_WIDTH: i32 = 460;
const NIGHT_SHIFT_HEIGHT: i32 = 340;
const WARMTH_STEP: f64 = 0.05;
const SMALL_SUN: i32 = 12;
const LARGE_SUN: i32 = 18;
const SLIDER_WIDTH: i32 = 220;
/// The dimmest the slider goes: at 0 a backlight is off, which the Mac's slider never does.
const LEAST_BRIGHTNESS: f64 = 1.0;
const MOST_BRIGHTNESS: f64 = 100.0;
const BACKLIGHT: &str = "--class=backlight";

/// Where a display's mode, scale and rotation are kept: in the settings every display
/// shares while there is one, in its own entry of their tables while there are more.
#[derive(Clone, PartialEq, Debug)]
enum Target {
    All,
    Display(String),
}

impl Target {
    /// How `nekoland msg` is asked about the display.
    fn query(&self, what: &str) -> Option<String> {
        match self {
            Target::All => output("nekoland", &["msg", what]),
            Target::Display(name) => output("nekoland", &["msg", &format!("{what} {name}")]),
        }
    }

    /// Sets `key`, or for one display its entry of `table`, to a TOML value.
    fn set(&self, (key, table): (&str, &str), value: &str) {
        match self {
            Target::All => set(&format!("{key} = {value}")),
            Target::Display(name) => set_display_entry(table, name, Some(value)),
        }
    }

    fn unset(&self, (key, table): (&str, &str)) {
        match self {
            Target::All => unset(key),
            Target::Display(name) => set_display_entry(table, name, None),
        }
    }

    /// What the settings hold for the display: its own entry, else the shared setting.
    fn value(&self, config: &Config, (key, table): (&str, &str)) -> Option<toml::Value> {
        let own = match self {
            Target::All => None,
            Target::Display(name) => config.0.get(table).and_then(toml::Value::as_table).and_then(|entries| entries.get(name)),
        };
        own.or_else(|| config.0.get(key)).cloned()
    }
}

const MODE: (&str, &str) = ("mode", "display_modes");
const SCALE: (&str, &str) = ("scale", "display_scales");
const ROTATION: (&str, &str) = ("rotation", "display_rotations");
const COLOR_DEPTH: (&str, &str) = ("color_depth", "display_color_depths");
const VARIABLE_REFRESH: (&str, &str) = ("variable_refresh", "display_variable_refresh");

/// Sets one display's entry of a setting kept by display name, or takes it out.
fn set_display_entry(table: &str, name: &str, value: Option<&str>) {
    let mut entries: BTreeMap<String, String> = Config::load()
        .0
        .get(table)
        .and_then(toml::Value::as_table)
        .map(|entries| entries.iter().map(|(name, value)| (name.clone(), value.to_string())).collect())
        .unwrap_or_default();
    match value {
        Some(value) => entries.insert(name.to_string(), value.to_string()),
        None => entries.remove(name),
    };
    if entries.is_empty() {
        unset(table);
    } else {
        let entries: Vec<String> = entries.iter().map(|(name, value)| format!("{name:?} = {value}")).collect();
        set(&format!("{table} = {{ {} }}", entries.join(", ")));
    }
}

/// A size in pixels or in points.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Size {
    width: i32,
    height: i32,
}

impl Size {
    fn parse(text: &str) -> Option<Self> {
        let (width, height) = text.split_once('x')?;
        Some(Size {
            width: width.trim().parse().ok()?,
            height: height.trim().parse().ok()?,
        })
    }

    /// As the `mode` setting has it.
    fn setting(self) -> String {
        format!("{}x{}", self.width, self.height)
    }

    fn label(self) -> String {
        format!("{} × {}", self.width, self.height)
    }

    fn pixels(self) -> i64 {
        i64::from(self.width) * i64::from(self.height)
    }

    fn scaled_down(self, scale: f64) -> Size {
        let down = |length: i32| (f64::from(length) / scale).round() as i32;
        Size { width: down(self.width), height: down(self.height) }
    }
}

/// `WIDTHxHEIGHT@HZ`.
fn parse_mode(text: &str) -> Option<(Size, u32)> {
    let (size, rate) = text.split_once('@')?;
    Some((Size::parse(size)?, rate.trim().parse().ok()?))
}

/// The display as `nekoland msg display` tells it.
#[derive(Clone, PartialEq, Debug)]
struct Display {
    /// The mode in use, and its rate.
    current: (Size, u32),
    /// The display's own size, which its picture is sharpest at.
    preferred: Size,
    scale: f64,
    /// The scale it is drawn at when the settings name none.
    density_scale: f64,
    /// Whether the display and the card can take 10 bits a colour.
    ten_bit: bool,
    variable_refresh: bool,
}

fn parse_display(text: &str) -> Option<Display> {
    let fields: HashMap<&str, &str> = text.lines().filter_map(|line| line.split_once(' ')).collect();
    Some(Display {
        current: parse_mode(fields.get("current")?)?,
        preferred: parse_mode(fields.get("preferred")?)?.0,
        scale: fields.get("scale")?.parse().ok()?,
        density_scale: fields.get("density_scale")?.parse().ok()?,
        ten_bit: fields.get("ten_bit") == Some(&"true"),
        variable_refresh: fields.get("variable_refresh") == Some(&"true"),
    })
}

/// The display's sizes, largest first, each with its rates, fastest first, from what
/// `nekoland msg modes` prints after the mode in use.
fn mode_rates(text: &str) -> Vec<(Size, Vec<u32>)> {
    let mut sizes: Vec<(Size, Vec<u32>)> = Vec::new();
    for (size, rate) in text.lines().skip(1).filter_map(parse_mode) {
        match sizes.iter_mut().find(|(known, _)| *known == size) {
            Some((_, rates)) => rates.push(rate),
            None => sizes.push((size, vec![rate])),
        }
    }
    sizes.sort_by_key(|(size, _)| Reverse(size.pixels()));
    for (_, rates) in &mut sizes {
        rates.sort_unstable_by(|a, b| b.cmp(a));
        rates.dedup();
    }
    sizes
}

/// The scale each of the display's sizes is drawn at when the settings name none, from what
/// `nekoland msg scales` prints.
fn mode_scales(text: &str) -> Vec<(Size, f64)> {
    text.lines()
        .filter_map(|line| {
            let (size, scale) = line.split_once(' ')?;
            Some((Size::parse(size)?, scale.trim().parse().ok()?))
        })
        .collect()
}

/// The scale a mode is drawn at when the settings name none: a mode that is not the
/// display's own has its own, as a virtual display's larger ones do.
fn own_scale(display: &Display, scales: &[(Size, f64)], mode: Size) -> f64 {
    if mode == display.preferred {
        return display.density_scale;
    }
    scales.iter().find(|(size, _)| *size == mode).map_or(1.0, |(_, scale)| *scale)
}

/// One of the resolution list's choices: a mode, drawn at a scale.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Choice {
    mode: Size,
    scale: f64,
}

impl Choice {
    fn looks_like(self) -> Size {
        self.mode.scaled_down(self.scale)
    }

    fn is(self, other: Choice) -> bool {
        self.mode == other.mode && (self.scale - other.scale).abs() < SAME_SCALE
    }
}

fn current_choice(display: &Display) -> Choice {
    Choice { mode: display.current.0, scale: display.scale }
}

fn default_choice(display: &Display) -> Choice {
    Choice { mode: display.preferred, scale: display.density_scale }
}

/// The sizes the display can look like at its own mode, and with `all` each of its other
/// modes too, at the scale each has of its own and as they are, the most room first; the
/// one in use is always among them.
fn resolution_choices(
    display: &Display,
    modes: &[(Size, Vec<u32>)],
    scales: &[(Size, f64)],
    all: bool,
) -> Vec<Choice> {
    let mut choices: Vec<Choice> = SCALES
        .iter()
        .map(|&scale| Choice { mode: display.preferred, scale })
        .filter(|choice| choice.looks_like().height >= LEAST_HEIGHT)
        .collect();
    if all {
        for (size, _) in modes.iter().filter(|(size, _)| *size != display.preferred) {
            let own = own_scale(display, scales, *size);
            choices.push(Choice { mode: *size, scale: own });
            if (own - 1.0).abs() >= SAME_SCALE {
                choices.push(Choice { mode: *size, scale: 1.0 });
            }
        }
    }
    let current = current_choice(display);
    if !choices.iter().any(|choice| choice.is(current)) {
        choices.push(current);
    }
    choices.sort_by_key(|choice| Reverse(choice.looks_like().pixels()));
    choices
}

/// What a choice drawn at a scale really is, for under the size it looks like: its mode's
/// pixels and the scale. Two choices can look the same size, one of them four times as
/// sharp, and on a screen shown in a window nothing else tells them apart.
fn choice_pixels(choice: Choice) -> Option<String> {
    ((choice.scale - 1.0).abs() >= SAME_SCALE).then(|| format!("{} · {}×", choice.mode.label(), choice.scale))
}

/// The looks of the display's own mode, from the largest text to the most room, each with
/// what is written under its picture: the ends and the default are named, as on the Mac.
fn looks(display: &Display) -> Vec<(Choice, &'static str)> {
    let mut looks: Vec<Choice> = resolution_choices(display, &[], &[], false)
        .into_iter()
        .filter(|choice| choice.mode == display.preferred)
        .collect();
    looks.reverse();
    let last = looks.len().saturating_sub(1);
    looks
        .into_iter()
        .enumerate()
        .map(|(index, choice)| {
            let label = if choice.is(default_choice(display)) {
                "預設"
            } else if index == 0 {
                "較大文字"
            } else if index == last {
                "更多空間"
            } else {
                ""
            };
            (choice, label)
        })
        .collect()
}

/// A look drawn small: a window with its buttons and lines of text, as large as the look
/// makes them, and the one in use with a line of ink around it.
fn look_picture(scale: f64, chosen: bool) -> gtk::DrawingArea {
    // Narrow at least, then as wide as its share of the row: five at full size would hold the
    // page wider than the others.
    // As wide as its share of the row, and narrow at least: five at a fixed width would hold
    // the page wider than the others.
    let area = gtk::DrawingArea::builder()
        .content_width(36)
        .content_height(60)
        .margin_start(12)
        .margin_end(12)
        .hexpand(true)
        .build();
    area.set_draw_func(move |area, cr, width, height| {
        let (width, height) = (f64::from(width), f64::from(height));
        let ink = area.color();
        let paint = |alpha: f32| {
            cr.set_source_rgba(
                f64::from(ink.red()),
                f64::from(ink.green()),
                f64::from(ink.blue()),
                f64::from(ink.alpha() * alpha),
            );
        };
        // The window: a sheet with a line around it.
        let radius = 6.0;
        cr.new_sub_path();
        cr.arc(width - radius - 0.5, radius + 0.5, radius, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(width - radius - 0.5, height - radius - 0.5, radius, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(radius + 0.5, height - radius - 0.5, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(radius + 0.5, radius + 0.5, radius, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
        cr.close_path();
        paint(0.06);
        let _ = cr.fill_preserve();
        cr.clip_preserve();
        // Half of the line is inside the clip: twice as wide, it shows as wide as meant.
        paint(if chosen { 1.0 } else { 0.2 });
        cr.set_line_width(if chosen { 4.0 } else { 2.0 });
        let _ = cr.stroke();
        // Its buttons, then lines of text for as far down as there is room.
        let unit = 2.0 * scale;
        let (left, mut y) = (4.0 * scale, 4.0 * scale);
        paint(0.5);
        for dot in 0..3 {
            cr.arc(left + unit + f64::from(dot) * unit * 3.0, y + unit, unit, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        y += unit * 4.5;
        paint(0.35);
        let mut line = 0;
        while y < height {
            // Every third line ends short, as a paragraph's last does.
            let across = if line % 3 == 2 { 0.5 } else { 1.0 };
            cr.rectangle(left, y, (width - left * 2.0).max(unit) * across, unit);
            let _ = cr.fill();
            y += unit * 2.5;
            line += 1;
        }
    });
    area
}

/// A choice as the list shows it: the size it looks like, and whether it is the display's
/// default or a mode less sharp than its own.
fn choice_label(choice: Choice, display: &Display) -> String {
    let size = choice.looks_like().label();
    if choice.is(default_choice(display)) {
        format!("{size}（預設）")
    } else if choice.mode.pixels() < display.preferred.pixels() {
        format!("{size}（低解析度）")
    } else {
        size
    }
}

/// Puts a choice in effect: the display's own mode and a mode's own scale are left unset, to
/// follow the display.
fn apply_choice(target: &Target, choice: Choice, display: &Display, scales: &[(Size, f64)]) {
    if choice.mode == display.preferred {
        target.unset(MODE);
    } else {
        target.set(MODE, &format!("{:?}", choice.mode.setting()));
    }
    if (choice.scale - own_scale(display, scales, choice.mode)).abs() < SAME_SCALE {
        target.unset(SCALE);
    } else {
        target.set(SCALE, &format!("{:?}", choice.scale));
    }
}

/// The resolution list, with the refresh rate that follows the mode chosen in it.
struct Resolution {
    target: Target,
    display: RefCell<Display>,
    modes: Vec<(Size, Vec<u32>)>,
    /// The scale each size is drawn at when the settings name none.
    scales: Vec<(Size, f64)>,
    /// The looks of the display's own mode, each a picture to press, as the Mac has them,
    /// over the list.
    looks_row: gtk::ListBoxRow,
    looks: gtk::Box,
    /// Every size it can show, opened up under them.
    all: adw::ExpanderRow,
    rows: RefCell<Vec<adw::ActionRow>>,
    rate: adw::ComboRow,
    /// Set while the rates are filled in, which is not a choice of one.
    filling_rates: Cell<bool>,
}

thread_local! {
    /// The resolution list of the page built last, the one that can be on screen.
    static SHOWN: RefCell<Option<Rc<Resolution>>> = const { RefCell::new(None) };
}

impl Resolution {
    fn new(target: Target) -> Option<Rc<Self>> {
        let display = parse_display(&target.query("display")?)?;
        let modes = mode_rates(&target.query("modes").unwrap_or_default());
        let scales = mode_scales(&target.query("scales").unwrap_or_default());
        let resolution = Rc::new(Resolution {
            target,
            display: RefCell::new(display),
            modes,
            scales,
            rows: RefCell::default(),
            looks_row: card_row(),
            looks: gtk::Box::builder().spacing(8).homogeneous(true).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build(),
            all: adw::ExpanderRow::builder().title("所有解析度").build(),
            rate: adw::ComboRow::builder().title("更新頻率").build(),
            filling_rates: Cell::new(false),
        });
        resolution.looks_row.set_child(Some(&resolution.looks));
        let weak = Rc::downgrade(&resolution);
        resolution.rate.connect_selected_notify(move |row| {
            if let Some(resolution) = weak.upgrade() {
                resolution.choose_rate(row.selected());
            }
        });
        resolution.fill_rows();
        resolution.fill_rates();
        // Its widgets hold it only weakly, against a cycle through them.
        SHOWN.replace(Some(resolution.clone()));
        Some(resolution)
    }

    fn fill_rows(self: &Rc<Self>) {
        for row in self.rows.borrow_mut().drain(..) {
            self.all.remove(&row);
        }
        while let Some(look) = self.looks.first_child() {
            self.looks.remove(&look);
        }
        let display = self.display.borrow().clone();
        let current = current_choice(&display);
        let looks = looks(&display);
        // One look is no choice.
        self.looks_row.set_visible(looks.len() > 1);
        let mut first: Option<gtk::ToggleButton> = None;
        // A look with no name keeps the line of one, so that the sizes under them line up.
        let names = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
        for (choice, label) in looks {
            let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
            content.append(&look_picture(choice.scale, choice.is(current)));
            // The text ellipsizes rather than holding the page wider at a large font.
            let name = gtk::Label::builder().label(label).ellipsize(gtk::pango::EllipsizeMode::End).build();
            names.add_widget(&name);
            content.append(&name);
            // The size it looks like, under its name: the pictures alone do not tell two
            // near sizes apart.
            let size = choice.looks_like().label();
            content.append(&gtk::Label::builder().label(&size).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label", "caption"]).build());
            let button = gtk::ToggleButton::builder()
                .child(&content)
                .css_classes(["flat", "nk-look"])
                .active(choice.is(current))
                .build();
            button.update_property(&[gtk::accessible::Property::Label(&size)]);
            match &first {
                Some(first) => button.set_group(Some(first)),
                None => first = Some(button.clone()),
            }
            let weak: Weak<Self> = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(resolution) = weak.upgrade() {
                    resolution.choose(choice);
                }
            });
            self.looks.append(&button);
        }
        for choice in resolution_choices(&display, &self.modes, &self.scales, true) {
            let row = adw::ActionRow::builder()
                .title(choice_label(choice, &display))
                .subtitle(choice_pixels(choice).unwrap_or_default())
                .activatable(true)
                .build();
            row.add_suffix(
                &gtk::Image::builder()
                    .icon_name("object-select-symbolic")
                    .visible(choice.is(current))
                    .build(),
            );
            let weak: Weak<Self> = Rc::downgrade(self);
            row.connect_activated(move |_| {
                if let Some(resolution) = weak.upgrade() {
                    resolution.choose(choice);
                }
            });
            self.all.add_row(&row);
            self.rows.borrow_mut().push(row);
        }
    }

    fn choose(self: &Rc<Self>, choice: Choice) {
        let display = self.display.borrow().clone();
        if choice.is(current_choice(&display)) {
            return;
        }
        apply_choice(&self.target, choice, &display, &self.scales);
        {
            let mut display = self.display.borrow_mut();
            let fastest = self.rates_of(choice.mode).first().copied().unwrap_or(display.current.1);
            display.current = (choice.mode, fastest);
            display.scale = choice.scale;
        }
        self.fill_rows();
        self.fill_rates();
    }

    fn rates_of(&self, mode: Size) -> Vec<u32> {
        self.modes
            .iter()
            .find(|(size, _)| *size == mode)
            .map(|(_, rates)| rates.clone())
            .unwrap_or_default()
    }

    fn fill_rates(&self) {
        let (mode, rate) = self.display.borrow().current;
        let rates = self.rates_of(mode);
        let labels: Vec<String> = rates.iter().map(|hz| format!("{hz} Hz")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.filling_rates.set(true);
        self.rate.set_model(Some(&gtk::StringList::new(&labels)));
        self.rate
            .set_selected(rates.iter().position(|hz| *hz == rate).unwrap_or(0) as u32);
        self.filling_rates.set(false);
        // A display with one rate still shows it, as the frames a second it draws at most.
        self.rate.set_sensitive(rates.len() > 1);
    }

    fn choose_rate(&self, index: u32) {
        if self.filling_rates.get() {
            return;
        }
        let mode = self.display.borrow().current.0;
        let Some(&hz) = self.rates_of(mode).get(index as usize) else {
            return;
        };
        self.target.set(MODE, &format!("\"{}@{hz}\"", mode.setting()));
        self.display.borrow_mut().current.1 = hz;
    }
}

/// A connected display's name and size, from its EDID; a display that gives none, as a
/// virtual machine's, is named by how it is connected. The first connected one for `All`.
fn display_identity(target: &Target) -> (String, Option<String>) {
    let name = match target {
        Target::All => None,
        Target::Display(name) => Some(name.as_str()),
    };
    crate::edid::connector(name).map_or(("顯示器".into(), None), |connector| crate::edid::identity(&connector))
}

/// A display as `nekoland msg displays` tells it: where it is on the desktop, in points, and
/// whether the projection shows it.
#[derive(Clone, PartialEq, Debug)]
struct Screen {
    name: String,
    size: Size,
    x: i32,
    y: i32,
    on: bool,
}

impl Screen {
    fn overlaps(&self, other: &Screen) -> bool {
        self.x < other.x + other.size.width
            && other.x < self.x + self.size.width
            && self.y < other.y + other.size.height
            && other.y < self.y + self.size.height
    }
}

/// The displays, the main one first, as `nekoland msg displays` has them.
fn parse_displays(text: &str) -> Vec<Screen> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_string();
            let size = Size::parse(fields.next()?)?;
            let x = fields.next()?.parse().ok()?;
            let y = fields.next()?.parse().ok()?;
            let on = fields.next()? == "on";
            Some(Screen { name, size, x, y, on })
        })
        .collect()
}

/// Windows' four ways of using more than one display, as `projection` names them, in the
/// order its Win+P shows them.
const PROJECTIONS: [(&str, &str); 4] = [
    ("main-only", "僅主要顯示器"),
    ("duplicate", "同步顯示"),
    ("extend", "延伸"),
    ("second-only", "僅其他顯示器"),
];
const EXTEND: &str = "extend";
const DUPLICATE: &str = "duplicate";

/// The projection in effect: `mirror_displays`, as it was written before, duplicates.
fn projection(config: &Config) -> &str {
    match config.word("projection") {
        "" | EXTEND if config.flag("mirror_displays") => DUPLICATE,
        "" => EXTEND,
        projection => projection,
    }
}

/// Within how many points of lining up a dropped display's edges line up, as Windows snaps.
const LINE_UP_WITHIN: i32 = 64;
/// How much of an edge two displays side by side share at least, for the pointer to cross.
const LEAST_SHARED: i32 = 64;
const PICTURE_HEIGHT: i32 = 200;
const PICTURE_PADDING: f64 = 16.0;
/// How much of a display's picture its menu bar takes, and the sizes its name is drawn at and
/// needs.
const MENU_BAR_SHARE: f64 = 0.06;
const NAME_SIZE: f64 = 11.0;
const NAMED_HEIGHT: f64 = 70.0;

/// Where a display dropped at its place goes: against an edge of another, the nearest such
/// place where it covers none of them, its edges lined up with that one's when near.
fn snap(screens: &[Screen], index: usize) -> (i32, i32) {
    let moved = &screens[index];
    let others: Vec<&Screen> = screens.iter().enumerate().filter(|(other, _)| *other != index).map(|(_, screen)| screen).collect();
    let (w, h) = (moved.size.width, moved.size.height);
    let line_up = |at: i32, low: i32, high: i32| {
        if (at - low).abs() <= LINE_UP_WITHIN {
            low
        } else if (at - high).abs() <= LINE_UP_WITHIN {
            high
        } else {
            at
        }
    };
    let mut places = Vec::new();
    for other in &others {
        let (ox, oy, ow, oh) = (other.x, other.y, other.size.width, other.size.height);
        let across = |at: i32, start: i32, length: i32, own: i32| {
            let at = at.clamp(start - own + LEAST_SHARED.min(own), start + length - LEAST_SHARED.min(length));
            line_up(at, start, start + length - own)
        };
        let y = across(moved.y, oy, oh, h);
        let x = across(moved.x, ox, ow, w);
        places.extend([(ox + ow, y), (ox - w, y), (x, oy + oh), (x, oy - h)]);
    }
    let free = |&(x, y): &(i32, i32)| {
        let placed = Screen { x, y, ..moved.clone() };
        !others.iter().any(|other| placed.overlaps(other))
    };
    let distance = |&(x, y): &(i32, i32)| i64::from(x - moved.x).pow(2) + i64::from(y - moved.y).pow(2);
    places.into_iter().filter(free).min_by_key(distance).unwrap_or((moved.x, moved.y))
}

/// A display being dragged: which, where it was, and the drawing's scale and origin then,
/// kept while it moves so that the picture does not shift under the pointer.
#[derive(Clone, Copy)]
struct Drag {
    index: usize,
    from: (i32, i32),
    scale: f64,
    origin: (f64, f64),
}

/// Told of the display picked, by name, and whether with the keyboard.
type Picked = Box<dyn Fn(&str, bool)>;

/// The displays drawn where they are, numbered as `nekoland msg identify` numbers them, to be
/// picked and, while they extend the desktop, dragged to where they are beside each other.
struct Arrangement {
    screens: RefCell<Vec<Screen>>,
    /// Each display's own name, from its EDID, in the order of `screens`.
    names: Vec<String>,
    selected: RefCell<String>,
    area: gtk::DrawingArea,
    dragging: Cell<Option<Drag>>,
    /// Whether the display pressed is another than the one chosen before, told as it is let go.
    picked_another: Cell<bool>,
    movable: bool,
    picked: Picked,
}

impl Arrangement {
    fn new(screens: Vec<Screen>, selected: String, movable: bool, picked: impl Fn(&str, bool) + 'static) -> Rc<Self> {
        let names = screens.iter().map(|screen| display_identity(&Target::Display(screen.name.clone())).0).collect();
        let arrangement = Rc::new(Arrangement {
            screens: RefCell::new(screens),
            names,
            selected: RefCell::new(selected),
            area: gtk::DrawingArea::builder()
                .content_height(PICTURE_HEIGHT)
                .hexpand(true)
                .focusable(true)
                .accessible_role(gtk::AccessibleRole::Img)
                .build(),
            dragging: Cell::new(None),
            picked_another: Cell::new(false),
            movable,
            picked: Box::new(picked),
        });
        let weak = Rc::downgrade(&arrangement);
        arrangement.area.set_draw_func(move |area, cr, width, height| {
            if let Some(arrangement) = weak.upgrade() {
                arrangement.draw(area, cr, width, height);
            }
        });
        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(&arrangement);
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(arrangement) = weak.upgrade() {
                arrangement.begin(x, y);
            }
        });
        let weak = Rc::downgrade(&arrangement);
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some(arrangement) = weak.upgrade() {
                arrangement.drag(dx, dy);
            }
        });
        let weak = Rc::downgrade(&arrangement);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(arrangement) = weak.upgrade() {
                arrangement.release();
            }
        });
        arrangement.area.add_controller(drag);
        // The arrow keys pick the display before or after the one chosen.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&arrangement);
        keys.connect_key_pressed(move |_, key, _, _| {
            let step = match key {
                gtk::gdk::Key::Left | gtk::gdk::Key::Up => -1,
                gtk::gdk::Key::Right | gtk::gdk::Key::Down => 1,
                _ => return glib::Propagation::Proceed,
            };
            if let Some(arrangement) = weak.upgrade() {
                arrangement.step(step);
            }
            glib::Propagation::Stop
        });
        arrangement.area.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let area = arrangement.area.downgrade();
        focus.connect_enter(move |_| {
            if let Some(area) = area.upgrade() {
                area.queue_draw();
            }
        });
        let area = arrangement.area.downgrade();
        focus.connect_leave(move |_| {
            if let Some(area) = area.upgrade() {
                area.queue_draw();
            }
        });
        arrangement.area.add_controller(focus);
        arrangement.describe();
        arrangement
    }

    /// Tells a screen reader which display is chosen, by its number.
    fn describe(&self) {
        let screens = self.screens.borrow();
        let index = screens.iter().position(|screen| screen.name == *self.selected.borrow()).unwrap_or(0);
        let name = self.names.get(index).map_or("", String::as_str);
        self.area.update_property(&[gtk::accessible::Property::Label(&format!("顯示器 {} {name}", index + 1))]);
    }

    fn step(&self, step: isize) {
        let name = {
            let screens = self.screens.borrow();
            let count = screens.len() as isize;
            let index = screens.iter().position(|screen| screen.name == *self.selected.borrow()).unwrap_or(0) as isize;
            screens[(index + step).rem_euclid(count) as usize].name.clone()
        };
        self.selected.replace(name.clone());
        self.describe();
        self.area.queue_draw();
        (self.picked)(&name, true);
    }

    /// The scale the displays are drawn at and where the desktop's origin is drawn, to fit.
    fn fit(&self, width: f64, height: f64) -> (f64, (f64, f64)) {
        let screens = self.screens.borrow();
        let left = screens.iter().map(|screen| screen.x).min().unwrap_or(0);
        let top = screens.iter().map(|screen| screen.y).min().unwrap_or(0);
        let right = screens.iter().map(|screen| screen.x + screen.size.width).max().unwrap_or(1);
        let bottom = screens.iter().map(|screen| screen.y + screen.size.height).max().unwrap_or(1);
        let (across, down) = (f64::from((right - left).max(1)), f64::from((bottom - top).max(1)));
        let scale = ((width - PICTURE_PADDING * 2.0) / across).min((height - PICTURE_PADDING * 2.0) / down);
        let origin = (
            (width - across * scale) / 2.0 - f64::from(left) * scale,
            (height - down * scale) / 2.0 - f64::from(top) * scale,
        );
        (scale, origin)
    }

    fn transform(&self) -> (f64, (f64, f64)) {
        match self.dragging.get() {
            Some(drag) => (drag.scale, drag.origin),
            None => self.fit(f64::from(self.area.width()), f64::from(self.area.height())),
        }
    }

    fn draw(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let (scale, (ox, oy)) = match self.dragging.get() {
            Some(drag) => (drag.scale, drag.origin),
            None => self.fit(f64::from(width), f64::from(height)),
        };
        let ink = area.color();
        let paint = |alpha: f32| {
            cr.set_source_rgba(f64::from(ink.red()), f64::from(ink.green()), f64::from(ink.blue()), f64::from(ink.alpha() * alpha));
        };
        let selected = self.selected.borrow().clone();
        let screens = self.screens.borrow();
        // Displays showing the same picture share a place, and its number names them all.
        let mut drawn: Vec<((i32, i32), String)> = Vec::new();
        for (index, screen) in screens.iter().enumerate() {
            let number = (index + 1).to_string();
            match drawn.iter_mut().find(|(at, _)| *at == (screen.x, screen.y)) {
                Some((_, numbers)) => *numbers = format!("{numbers} | {number}"),
                None => drawn.push(((screen.x, screen.y), number)),
            }
        }
        for (index, screen) in screens.iter().enumerate() {
            let (x, y) = (ox + f64::from(screen.x) * scale, oy + f64::from(screen.y) * scale);
            let (w, h) = (f64::from(screen.size.width) * scale, f64::from(screen.size.height) * scale);
            let chosen = screen.name == selected;
            cr.rectangle(x + 2.0, y + 2.0, w - 4.0, h - 4.0);
            paint(match (chosen, screen.on) {
                (true, _) => 0.3,
                (false, true) => 0.12,
                (false, false) => 0.05,
            });
            let _ = cr.fill_preserve();
            paint(if chosen { 1.0 } else { 0.3 });
            cr.set_line_width(if chosen { 3.0 } else { 1.0 });
            let _ = cr.stroke();
            // The main display carries the menu bar, as on the Mac's picture.
            if index == 0 {
                cr.rectangle(x + 2.0, y + 2.0, w - 4.0, (h * MENU_BAR_SHARE).clamp(4.0, 8.0));
                paint(0.75);
                let _ = cr.fill();
            }
            let Some((_, numbers)) = drawn.iter().find(|(at, _)| *at == (screen.x, screen.y)) else {
                continue;
            };
            let number = text_layout(area, numbers, "Sans Bold", (h * 0.3).clamp(10.0, 40.0));
            // A place shared by more than one display, or one too small, goes without a name.
            let name = (!numbers.contains('|') && h > NAMED_HEIGHT)
                .then(|| text_layout(area, &self.names[index], "Sans", NAME_SIZE))
                .filter(|name| f64::from(name.pixel_size().0) < w - 8.0);
            let (number_w, number_h) = number.pixel_size();
            let name_h = name.as_ref().map_or(0, |name| name.pixel_size().1);
            let top = y + (h - f64::from(number_h + name_h)) / 2.0;
            cr.move_to(x + (w - f64::from(number_w)) / 2.0, top);
            paint(if screen.on { 0.9 } else { 0.4 });
            pangocairo::functions::show_layout(cr, &number);
            if let Some(name) = name {
                cr.move_to(x + (w - f64::from(name.pixel_size().0)) / 2.0, top + f64::from(number_h));
                paint(if screen.on { 0.7 } else { 0.35 });
                pangocairo::functions::show_layout(cr, &name);
            }
        }
        if area.has_focus() {
            cr.rectangle(1.0, 1.0, f64::from(width) - 2.0, f64::from(height) - 2.0);
            cr.set_line_width(2.0);
            paint(0.5);
            let _ = cr.stroke();
        }
    }

    fn at(&self, x: f64, y: f64) -> Option<usize> {
        let (scale, (ox, oy)) = self.transform();
        let (px, py) = ((x - ox) / scale, (y - oy) / scale);
        // Front first: the last drawn is over the others.
        self.screens.borrow().iter().rposition(|screen| {
            px >= f64::from(screen.x)
                && py >= f64::from(screen.y)
                && px < f64::from(screen.x + screen.size.width)
                && py < f64::from(screen.y + screen.size.height)
        })
    }

    fn begin(&self, x: f64, y: f64) {
        let Some(index) = self.at(x, y) else {
            return;
        };
        let name = self.screens.borrow()[index].name.clone();
        if *self.selected.borrow() != name {
            self.selected.replace(name);
            self.picked_another.set(true);
            self.describe();
        }
        if self.movable {
            let (scale, origin) = self.transform();
            let screen = &self.screens.borrow()[index];
            self.dragging.set(Some(Drag { index, from: (screen.x, screen.y), scale, origin }));
        }
        self.area.queue_draw();
    }

    fn drag(&self, dx: f64, dy: f64) {
        let Some(drag) = self.dragging.get() else {
            return;
        };
        let screen = &mut self.screens.borrow_mut()[drag.index];
        screen.x = drag.from.0 + (dx / drag.scale).round() as i32;
        screen.y = drag.from.1 + (dy / drag.scale).round() as i32;
        self.area.queue_draw();
    }

    /// Puts a dragged display where it goes and keeps the places, then tells of the display
    /// picked: the page is built again for it, which this picture goes with.
    fn release(&self) {
        if let Some(Drag { index, from, .. }) = self.dragging.take() {
            self.place(index, from);
        }
        if self.picked_another.take() {
            (self.picked)(&self.selected.borrow(), false);
        }
    }

    fn place(&self, index: usize, from: (i32, i32)) {
        let place = snap(&self.screens.borrow(), index);
        self.screens.borrow_mut()[index].x = place.0;
        self.screens.borrow_mut()[index].y = place.1;
        self.area.queue_draw();
        if place == from {
            return;
        }
        let places: Vec<String> = self
            .screens
            .borrow()
            .iter()
            .filter(|screen| screen.on)
            .map(|screen| format!("{:?} = [{}, {}]", screen.name, screen.x, screen.y))
            .collect();
        set(&format!("display_positions = {{ {} }}", places.join(", ")));
    }
}

/// Builds the page again, after a change to the displays has been put in effect.
fn refill_soon(page: &Weak<DisplayPage>) {
    let page = page.clone();
    // The compositor lays the displays out as it next draws.
    glib::timeout_add_local_once(Duration::from_millis(300), move || {
        if let Some(page) = page.upgrade() {
            page.fill();
        }
    });
}

/// 排列: the displays' picture, with the button that numbers them on their screens, and how
/// more than one of them is used.
fn arrangement_group(page: &Rc<DisplayPage>, config: &Config, screens: &[Screen], selected: &str) -> adw::PreferencesGroup {
    let group = group("排列");
    let identify = gtk::Button::builder().label("識別").valign(gtk::Align::Center).build();
    identify.connect_clicked(|_| run("nekoland", &["msg", "identify"]));
    group.set_header_suffix(Some(&identify));

    let current = projection(config);
    let weak = Rc::downgrade(page);
    let arrangement = Arrangement::new(screens.to_vec(), selected.to_string(), current == EXTEND, move |name, by_key| {
        if let Some(page) = weak.upgrade() {
            page.selected.replace(Some(name.to_string()));
            page.focus_picture.set(by_key);
            refill_soon(&weak);
        }
    });
    let picture = card_row();
    picture.set_child(Some(&arrangement.area));
    group.add(&picture);
    page.arrangement.replace(Some(arrangement));

    let labels: Vec<&str> = PROJECTIONS.iter().map(|(_, label)| *label).collect();
    let row = adw::ComboRow::builder()
        .title("多部顯示器")
        .model(&gtk::StringList::new(&labels))
        .selected(PROJECTIONS.iter().position(|(name, _)| *name == current).unwrap_or(2) as u32)
        .build();
    let weak = Rc::downgrade(page);
    keep_or_revert(&row, "保留這些顯示設定？", move |index| {
        let Some((name, _)) = PROJECTIONS.get(index as usize) else {
            return;
        };
        unset("mirror_displays");
        set(&format!("projection = {name:?}"));
        refill_soon(&weak);
    });
    group.add(&row);
    group
}

/// Makes the chosen display the main one; the main one itself has none to offer.
fn main_display_row(page: &Rc<DisplayPage>, screens: &[Screen], selected: &str) -> adw::SwitchRow {
    let main = adw::SwitchRow::builder().title("主要顯示器").build();
    let (weak, all, name) = (Rc::downgrade(page), screens.to_vec(), selected.to_string());
    main.connect_active_notify(move |row| {
        if !row.is_active() {
            return;
        }
        let order: Vec<String> = std::iter::once(&name)
            .chain(all.iter().map(|screen| &screen.name).filter(|other| **other != name))
            .map(|name| format!("{name:?}"))
            .collect();
        set(&format!("display_order = [{}]", order.join(", ")));
        refill_soon(&weak);
    });
    main
}

/// A row of a boxed list holding something other than a setting, as a picture.
fn card_row() -> gtk::ListBoxRow {
    gtk::ListBoxRow::builder().activatable(false).selectable(false).build()
}

fn text_layout(area: &gtk::DrawingArea, text: &str, font: &str, size: f64) -> gtk::pango::Layout {
    let layout = area.create_pango_layout(Some(text));
    let mut font = gtk::pango::FontDescription::from_string(font);
    font.set_absolute_size(size * f64::from(gtk::pango::SCALE));
    layout.set_font_description(Some(&font));
    layout
}

/// The page, built again as the displays and the one chosen change.
struct DisplayPage {
    page: adw::PreferencesPage,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    /// The display the settings under the picture are for, by name.
    selected: RefCell<Option<String>>,
    arrangement: RefCell<Option<Rc<Arrangement>>>,
    /// Whether the picture had the keyboard's focus as the page was built again, to keep it.
    focus_picture: Cell<bool>,
}

thread_local! {
    /// The page built last, the one that can be on screen; its widgets hold it only weakly.
    static PAGE: RefCell<Option<Rc<DisplayPage>>> = const { RefCell::new(None) };
}

impl DisplayPage {
    fn fill(self: &Rc<Self>) {
        for group in self.groups.take() {
            self.page.remove(&group);
        }
        let config = Config::load();
        let screens = parse_displays(&output("nekoland", &["msg", "displays"]).unwrap_or_default());
        let mut groups = Vec::new();
        let (target, display) = if screens.len() < 2 {
            let (name, size) = display_identity(&Target::All);
            let display = group(&name);
            display.set_description(size.as_deref());
            (Target::All, display)
        } else {
            let selected = self
                .selected
                .borrow()
                .clone()
                .filter(|name| screens.iter().any(|screen| screen.name == *name))
                .unwrap_or_else(|| screens[0].name.clone());
            groups.push(arrangement_group(self, &config, &screens, &selected));
            let target = Target::Display(selected.clone());
            let number = screens.iter().position(|screen| screen.name == selected).unwrap_or(0) + 1;
            let (name, size) = display_identity(&target);
            let display = group(&format!("顯示器 {number}"));
            display.set_description(Some(&[Some(name), size].into_iter().flatten().collect::<Vec<_>>().join("・")));
            (target, display)
        };
        if let Some(resolution) = Resolution::new(target.clone()) {
            display.add(&resolution.looks_row);
            display.add(&resolution.all);
            display.add(&resolution.rate);
            let abilities = resolution.display.borrow().clone();
            if abilities.ten_bit {
                display.add(&colour_depth_row(&config, &target));
            }
            if abilities.variable_refresh {
                display.add(&variable_refresh_row(&config, &target));
            }
        }
        display.add(&rotation_row(&config, &target));
        // The backlight is the built-in panel's.
        let built_in = match &target {
            Target::All => true,
            Target::Display(name) => crate::edid::BUILT_IN.iter().any(|kind| name.starts_with(kind)),
        };
        if let Some(brightness) = brightness_row().filter(|_| built_in) {
            display.add(&brightness);
        }
        // The built-in panel is the one that turns with the computer.
        if built_in {
            display.add(&switch_row(&config, "旋轉鎖定", "rotation_lock"));
        }
        if let Target::Display(name) = &target {
            if *name != screens[0].name {
                display.add(&main_display_row(self, &screens, name));
            }
        }
        groups.push(display);

        let shared = group("");
        shared.add(&night_shift_entry(&config));
        shared.add(&super::colour_profile_row(&config));
        shared.add(&switch_row(&config, "顯示 FPS", "show_fps"));
        groups.push(shared);
        for group in &groups {
            self.page.add(group);
        }
        self.groups.replace(groups);
        if self.focus_picture.take() {
            if let Some(arrangement) = self.arrangement.borrow().as_ref() {
                arrangement.area.grab_focus();
            }
        }
    }
}

/// The backlight's slider, between a small sun and a large one; none without a backlight.
fn brightness_row() -> Option<adw::ActionRow> {
    let info = output("brightnessctl", &[BACKLIGHT, "-m", "info"])?;
    // device,class,current,percent,max
    let percent: f64 = info.lines().next()?.split(',').nth(3)?.trim_end_matches('%').parse().ok()?;
    let sun = |size: i32| -> gtk::Widget {
        gtk::Image::builder()
            .icon_name("display-brightness-symbolic")
            .pixel_size(size)
            .build()
            .upcast()
    };
    let row = slider_row(
        "亮度",
        (LEAST_BRIGHTNESS, MOST_BRIGHTNESS, 1.0),
        percent.max(LEAST_BRIGHTNESS),
        (sun(SMALL_SUN), sun(LARGE_SUN)),
        |percent| run("brightnessctl", &[BACKLIGHT, "-q", "set", &format!("{}%", percent.round())]),
    );
    Some(row)
}

/// A row with a slider from `range`'s least to its most by its step, between two ends that
/// show which way is which, as the Mac's sliders.
fn slider_row(
    title: &str,
    (least, most, step): (f64, f64, f64),
    value: f64,
    (start, end): (gtk::Widget, gtk::Widget),
    changed: impl Fn(f64) + 'static,
) -> adw::ActionRow {
    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, least, most, step);
    slider.set_value(value);
    slider.set_width_request(SLIDER_WIDTH);
    slider.set_valign(gtk::Align::Center);
    slider.connect_value_changed(move |slider| changed(slider.value()));
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&line(8, &[&start, slider.upcast_ref(), &end]));
    row
}

fn apply_rotation(target: &Target, degrees: i64) {
    if degrees == 0 && *target == Target::All {
        target.unset(ROTATION);
    } else {
        target.set(ROTATION, &degrees.to_string());
    }
}

/// The picture's rotation; a new one is turned back unless it is kept in time.
fn rotation_row(config: &Config, target: &Target) -> adw::ComboRow {
    let current = target.value(config, ROTATION).and_then(|value| value.as_integer()).unwrap_or(0);
    let labels: Vec<&str> = ROTATIONS.iter().map(|(label, _)| *label).collect();
    let selected = ROTATIONS.iter().position(|(_, degrees)| *degrees == current).unwrap_or(0) as u32;
    let row = adw::ComboRow::builder()
        .title("旋轉")
        .model(&gtk::StringList::new(&labels))
        .selected(selected)
        .build();
    let target = target.clone();
    keep_or_revert(&row, "保留這個旋轉方向？", move |index| {
        if let Some(&(_, degrees)) = ROTATIONS.get(index as usize) {
            apply_rotation(&target, degrees);
        }
    });
    row
}

/// The bits each colour is sent with; a new depth, which a display may show as nothing, is
/// turned back unless it is kept in time.
fn colour_depth_row(config: &Config, target: &Target) -> adw::ComboRow {
    let current = target.value(config, COLOR_DEPTH).and_then(|value| value.as_integer()).unwrap_or(8);
    let labels: Vec<&str> = COLOR_DEPTHS.iter().map(|(label, _)| *label).collect();
    let selected = COLOR_DEPTHS.iter().position(|(_, depth)| *depth == current).unwrap_or(0) as u32;
    let row = adw::ComboRow::builder()
        .title("色彩深度")
        .model(&gtk::StringList::new(&labels))
        .selected(selected)
        .build();
    let target = target.clone();
    keep_or_revert(&row, "保留這個色彩深度？", move |index| {
        match COLOR_DEPTHS.get(index as usize) {
            Some(&(_, 8)) if target == Target::All => target.unset(COLOR_DEPTH),
            Some(&(_, depth)) => target.set(COLOR_DEPTH, &depth.to_string()),
            None => {}
        }
    });
    row
}

fn variable_refresh_row(config: &Config, target: &Target) -> adw::SwitchRow {
    let on = target.value(config, VARIABLE_REFRESH).and_then(|value| value.as_bool()).unwrap_or(false);
    let row = adw::SwitchRow::builder().title("可變更新頻率").active(on).build();
    let target = target.clone();
    row.connect_active_notify(move |row| match row.is_active() {
        false if target == Target::All => target.unset(VARIABLE_REFRESH),
        on => target.set(VARIABLE_REFRESH, &on.to_string()),
    });
    row
}

fn countdown_text(seconds: u32) -> String {
    format!("{seconds} 秒後還原。")
}

/// Puts in effect each choice made in `row`, turned back unless it is kept in time, as a
/// picture that cannot be read may follow it.
fn keep_or_revert(row: &adw::ComboRow, question: &'static str, apply: impl Fn(u32) + 'static) {
    let kept = Rc::new(Cell::new(row.selected()));
    // Set while the row is put back, which is not a choice.
    let reverting = Rc::new(Cell::new(false));
    let apply = Rc::new(apply);
    row.connect_selected_notify(move |row| {
        if reverting.get() || row.selected() == kept.get() {
            return;
        }
        apply(row.selected());
        ask_to_keep(row, question, kept.clone(), reverting.clone(), apply.clone());
    });
}

fn ask_to_keep(
    row: &adw::ComboRow,
    question: &str,
    kept: Rc<Cell<u32>>,
    reverting: Rc<Cell<bool>>,
    apply: Rc<dyn Fn(u32)>,
) {
    let alert = adw::AlertDialog::new(Some(question), Some(&countdown_text(KEEP_ROTATION_WITHIN)));
    alert.add_responses(&[("revert", "還原"), ("keep", "保留")]);
    alert.set_default_response(Some("keep"));
    alert.set_close_response("revert");
    let settled = Rc::new(Cell::new(false));
    let settle = Rc::new(glib::clone!(
        #[weak]
        row,
        move |keep: bool| {
            if settled.replace(true) {
                return;
            }
            if keep {
                kept.set(row.selected());
                return;
            }
            apply(kept.get());
            reverting.set(true);
            row.set_selected(kept.get());
            reverting.set(false);
        }
    ));
    let left = Cell::new(KEEP_ROTATION_WITHIN);
    let ticking = glib::timeout_add_local(
        Duration::from_secs(1),
        glib::clone!(
            #[weak]
            alert,
            #[strong]
            settle,
            #[upgrade_or]
            glib::ControlFlow::Break,
            move || {
                left.set(left.get().saturating_sub(1));
                if left.get() == 0 {
                    settle(false);
                    alert.force_close();
                    return glib::ControlFlow::Break;
                }
                alert.set_body(&countdown_text(left.get()));
                glib::ControlFlow::Continue
            }
        ),
    );
    let ticking = RefCell::new(Some(ticking));
    alert.choose(row, gio::Cancellable::NONE, move |response| {
        if let Some(ticking) = ticking.take() {
            ticking.remove();
        }
        settle(response == "keep");
    });
}

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

/// Whether the compositor knows where the sun sets: a time zone with a place, unlike UTC.
fn knows_sunset() -> bool {
    output("nekoland", &["msg", "sun"]).is_some_and(|text| !text.trim().is_empty())
}

/// Night Shift's schedule: its choices, with 日落到日出 only where the sun's hours are known
/// or it is already chosen.
fn night_shift_row(config: &Config) -> (adw::ComboRow, Vec<&'static str>) {
    let current = config.word("night_shift");
    let offered: Vec<usize> = (0..NIGHT_SHIFT.len())
        .filter(|&index| NIGHT_SHIFT[index] != SUNSET || current == SUNSET || knows_sunset())
        .collect();
    let names: Vec<&'static str> = offered.iter().map(|&index| NIGHT_SHIFT[index]).collect();
    let labels: Vec<&str> = offered.iter().map(|&index| NIGHT_SHIFT_LABELS[index]).collect();
    let row = adw::ComboRow::builder()
        .title("夜覽")
        .model(&gtk::StringList::new(&labels))
        .selected(names.iter().position(|name| *name == current).unwrap_or(0) as u32)
        .build();
    let chosen = names.clone();
    row.connect_selected_notify(move |row| {
        if let Some(name) = chosen.get(row.selected() as usize) {
            set(&format!("night_shift = {name:?}"));
        }
    });
    (row, names)
}

/// How warm Night Shift is, between the Mac's words for its ends.
fn warmth_row(config: &Config) -> adw::ActionRow {
    let end = |text: &str| -> gtk::Widget {
        gtk::Label::builder().label(text).css_classes(["dim-label", "caption"]).build().upcast()
    };
    slider_row(
        "色溫",
        (0.0, 1.0, WARMTH_STEP),
        config.number("night_shift_warmth", DEFAULT_WARMTH),
        (end("較不暖"), end("較暖")),
        |warmth| set(&format!("night_shift_warmth = {warmth:.2}")),
    )
}

/// 夜覽, with how it is set, opening its dialog.
fn night_shift_entry(config: &Config) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title("夜覽")
        .subtitle(night_shift_label(config))
        .activatable(true)
        .build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row.connect_activated(night_shift_dialog);
    row
}

fn night_shift_label(config: &Config) -> &'static str {
    let current = config.word("night_shift");
    NIGHT_SHIFT.iter().position(|name| *name == current).map_or(NIGHT_SHIFT_LABELS[0], |index| NIGHT_SHIFT_LABELS[index])
}

fn night_shift_dialog(parent: &adw::ActionRow) {
    let config = Config::load();
    let (shift, names) = night_shift_row(&config);
    let from = time_row(&config, "開始", "night_shift_from");
    let to = time_row(&config, "結束", "night_shift_to");
    let schedule = group("");
    schedule.add(&shift);
    schedule.add(&from);
    schedule.add(&to);
    let custom = move |row: &adw::ComboRow| {
        let on = names.get(row.selected() as usize) == Some(&SCHEDULED);
        from.set_visible(on);
        to.set_visible(on);
    };
    custom(&shift);
    shift.connect_selected_notify(custom);
    let warmth = group("");
    warmth.add(&warmth_row(&config));

    let page = adw::PreferencesPage::new();
    page.add(&schedule);
    page.add(&warmth);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title("夜覽")
        .content_width(NIGHT_SHIFT_WIDTH)
        // Tall enough for the custom hours too, so that choosing them does not push the
        // warmth out of sight.
        .content_height(NIGHT_SHIFT_HEIGHT)
        .child(&view)
        .build();
    dialog.connect_closed(glib::clone!(
        #[weak]
        parent,
        move |_| parent.set_subtitle(night_shift_label(&Config::load()))
    ));
    dialog.present(Some(parent));
}

pub fn display_page(_config: &Config) -> adw::PreferencesPage {
    let page = Rc::new(DisplayPage {
        page: adw::PreferencesPage::new(),
        groups: RefCell::default(),
        selected: RefCell::default(),
        arrangement: RefCell::default(),
        focus_picture: Cell::new(false),
    });
    page.fill();
    PAGE.replace(Some(page.clone()));
    page.page.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> Display {
        parse_display("current 3840x2160@60\npreferred 3840x2160@60\nscale 1.5\ndensity_scale 2\n").unwrap()
    }

    #[test]
    fn displays_are_read_main_first() {
        let text = "eDP-1\t1536x864\t0\t0\ton\nHDMI-A-1\t1920x1080\t1536\t-200\toff\nbroken\n";
        let size = |width, height| Size { width, height };
        assert_eq!(
            parse_displays(text),
            [
                Screen { name: "eDP-1".into(), size: size(1536, 864), x: 0, y: 0, on: true },
                Screen { name: "HDMI-A-1".into(), size: size(1920, 1080), x: 1536, y: -200, on: false },
            ]
        );
    }

    #[test]
    fn a_dropped_display_goes_against_the_nearest_edge() {
        let screen = |name: &str, x, y| Screen { name: name.into(), size: Size { width: 1920, height: 1080 }, x, y, on: true };
        // Dropped over the other, a little to the right: to its right, tops lined up.
        assert_eq!(snap(&[screen("A", 0, 0), screen("B", 1500, 30)], 1), (1920, 0));
        // Dropped far below: under it, sharing the edge.
        assert_eq!(snap(&[screen("A", 0, 0), screen("B", 300, 1500)], 1), (300, 1080));
        // Dropped off to the side: still against it, sharing enough of the edge to cross.
        assert_eq!(snap(&[screen("A", 0, 0), screen("B", 3000, 1050)], 1), (1920, 1080 - LEAST_SHARED));
    }

    #[test]
    fn the_display_is_read_from_its_lines() {
        let display = display();
        assert_eq!(display.current, (Size { width: 3840, height: 2160 }, 60));
        assert_eq!(display.preferred, Size { width: 3840, height: 2160 });
        assert_eq!((display.scale, display.density_scale), (1.5, 2.0));
        assert!(!display.ten_bit && !display.variable_refresh);
        let able = parse_display("current 3840x2160@60\npreferred 3840x2160@60\nscale 1.5\ndensity_scale 2\nten_bit true\nvariable_refresh true\n").unwrap();
        assert!(able.ten_bit && able.variable_refresh);
        assert_eq!(parse_display("current 3840x2160@60\n"), None);
    }

    #[test]
    fn modes_are_grouped_by_size_largest_first() {
        let text = "1920x1080@60\n1920x1080@60\n1280x720@60\n1920x1080@144\n2560x1440@60\n";
        let size = |width, height| Size { width, height };
        assert_eq!(
            mode_rates(text),
            [(size(2560, 1440), vec![60]), (size(1920, 1080), vec![144, 60]), (size(1280, 720), vec![60])]
        );
        assert_eq!(mode_rates(""), vec![]);
    }

    #[test]
    fn the_list_runs_from_most_room_to_largest_text() {
        let display = display();
        let labels: Vec<String> = resolution_choices(&display, &[], &[], false)
            .into_iter()
            .map(|choice| choice_label(choice, &display))
            .collect();
        assert_eq!(
            labels,
            ["3840 × 2160", "3072 × 1728", "2560 × 1440", "2194 × 1234", "1920 × 1080（預設）"]
        );
    }

    #[test]
    fn every_mode_shows_with_all_resolutions_and_the_one_in_use_always() {
        let display = Display {
            current: (Size { width: 1280, height: 720 }, 60),
            scale: 1.0,
            ..display()
        };
        let modes = mode_rates("x\n3840x2160@60\n1920x1080@60\n1280x720@60\n");
        let hidden = resolution_choices(&display, &modes, &[], false);
        assert!(hidden.iter().any(|choice| choice.is(current_choice(&display))));
        assert!(!hidden.iter().any(|choice| choice.mode.height == 1080));
        let all = resolution_choices(&display, &modes, &[], true);
        let low = all.iter().find(|choice| choice.mode.height == 1080).unwrap();
        assert_eq!(choice_label(*low, &display), "1920 × 1080（低解析度）");
    }

    #[test]
    fn a_mode_larger_than_the_displays_own_keeps_its_own_scale() {
        // A virtual display's own mode is small, and its 4K one is drawn at twice.
        let display = Display {
            current: (Size { width: 1024, height: 768 }, 60),
            preferred: Size { width: 1024, height: 768 },
            scale: 1.0,
            density_scale: 1.0,
            ten_bit: false,
            variable_refresh: false,
        };
        let four_k = Size { width: 3840, height: 2160 };
        let modes = mode_rates("x\n3840x2160@60\n1024x768@60\n");
        let scales = mode_scales("3840x2160 2\n1024x768 1\n");
        let labels: Vec<String> = resolution_choices(&display, &modes, &scales, true)
            .into_iter()
            .filter(|choice| choice.mode == four_k)
            .map(|choice| choice_label(choice, &display))
            .collect();
        assert_eq!(labels, ["3840 × 2160", "1920 × 1080"]);
        // What tells it from a mode of 1920 by 1080: its own pixels, and the scale.
        let own = Choice { mode: four_k, scale: 2.0 };
        assert_eq!(choice_pixels(own).as_deref(), Some("3840 × 2160 · 2×"));
        assert_eq!(choice_pixels(Choice { mode: four_k, scale: 1.0 }), None);
        // Chosen at twice, the settings name no scale; at one, they do.
        assert_eq!(own_scale(&display, &scales, four_k), 2.0);
        assert_eq!(own_scale(&display, &scales, display.preferred), 1.0);
    }

    #[test]
    fn the_looks_run_from_largest_text_to_most_room() {
        let display = display();
        let named: Vec<(String, &str)> =
            looks(&display).into_iter().map(|(choice, label)| (choice.looks_like().label(), label)).collect();
        assert_eq!(named.first(), Some(&("1920 × 1080".to_string(), "預設")));
        assert_eq!(named.last(), Some(&("3840 × 2160".to_string(), "更多空間")));
        assert!(named[1..named.len() - 1].iter().all(|(_, label)| label.is_empty()));
        // On a display whose default is not its largest text, that end is named too.
        let coarse = Display { density_scale: 1.0, ..display };
        assert_eq!(looks(&coarse).first().map(|(_, label)| *label), Some("較大文字"));
        assert_eq!(looks(&coarse).last().map(|(_, label)| *label), Some("預設"));
    }

    #[test]
    fn small_sizes_are_left_out() {
        let display = Display {
            current: (Size { width: 1366, height: 768 }, 60),
            preferred: Size { width: 1366, height: 768 },
            scale: 1.0,
            density_scale: 1.0,
            ten_bit: false,
            variable_refresh: false,
        };
        let heights: Vec<i32> = resolution_choices(&display, &[], &[], false)
            .iter()
            .map(|choice| choice.looks_like().height)
            .collect();
        assert_eq!(heights, [768, 614]);
    }
}
