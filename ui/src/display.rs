//! Settings' 顯示 page, laid out as the Mac's Displays: the display, the sizes it can look
//! like, its brightness, refresh rate and rotation, and Night Shift in a dialog of its own.

use std::{
    cell::{Cell, RefCell},
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    path::Path,
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
const NIGHT_SHIFT: [&str; 4] = ["off", "on", "sunset", "scheduled"];
const NIGHT_SHIFT_LABELS: [&str; 4] = ["關閉", "開啟", "日落到日出", "自訂"];
const SUNSET: &str = "sunset";
const SCHEDULED: &str = "scheduled";
const DEFAULT_WARMTH: f64 = 0.7;
const NIGHT_SHIFT_WIDTH: i32 = 460;
const NIGHT_SHIFT_HEIGHT: i32 = 340;
const WARMTH_STEP: f64 = 0.05;
const DRM: &str = "/sys/class/drm";
/// The connector kinds of a panel built into the computer.
const BUILT_IN: [&str; 3] = ["eDP", "LVDS", "DSI"];
const VIRTUAL: &str = "Virtual";
const DISPLAY_ICON_SIZE: i32 = 96;
const SMALL_SUN: i32 = 12;
const LARGE_SUN: i32 = 18;
const SLIDER_WIDTH: i32 = 220;
/// The dimmest the slider goes: at 0 a backlight is off, which the Mac's slider never does.
const LEAST_BRIGHTNESS: f64 = 1.0;
const MOST_BRIGHTNESS: f64 = 100.0;
const BACKLIGHT: &str = "--class=backlight";
/// Where EDID's display descriptors are, each this long, and the kind and text of the one
/// that names the display.
const EDID_DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];
const EDID_DESCRIPTOR_LENGTH: usize = 18;
const EDID_NAME_TAG: u8 = 0xFC;
const EDID_TEXT_START: usize = 5;
/// Where EDID keeps the picture's width and height, in centimetres.
const EDID_WIDTH_CM: usize = 21;
const EDID_HEIGHT_CM: usize = 22;
const CM_AN_INCH: f64 = 2.54;

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
}

fn parse_display(text: &str) -> Option<Display> {
    let fields: HashMap<&str, &str> = text.lines().filter_map(|line| line.split_once(' ')).collect();
    Some(Display {
        current: parse_mode(fields.get("current")?)?,
        preferred: parse_mode(fields.get("preferred")?)?.0,
        scale: fields.get("scale")?.parse().ok()?,
        density_scale: fields.get("density_scale")?.parse().ok()?,
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
    let area = gtk::DrawingArea::builder()
        .content_width(88)
        .content_height(60)
        .halign(gtk::Align::Center)
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
fn apply_choice(choice: Choice, display: &Display, scales: &[(Size, f64)]) {
    if choice.mode == display.preferred {
        unset("mode");
    } else {
        set(&format!("mode = {:?}", choice.mode.setting()));
    }
    if (choice.scale - own_scale(display, scales, choice.mode)).abs() < SAME_SCALE {
        unset("scale");
    } else {
        set(&format!("scale = {:?}", choice.scale));
    }
}

/// The resolution list, with the refresh rate that follows the mode chosen in it.
struct Resolution {
    display: RefCell<Display>,
    modes: Vec<(Size, Vec<u32>)>,
    /// The scale each size is drawn at when the settings name none.
    scales: Vec<(Size, f64)>,
    group: adw::PreferencesGroup,
    /// The looks of the display's own mode, each a picture to press, as the Mac has them,
    /// in a group of their own over the list.
    pictures: adw::PreferencesGroup,
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
    fn new() -> Option<Rc<Self>> {
        let display = parse_display(&output("nekoland", &["msg", "display"])?)?;
        let modes = mode_rates(&output("nekoland", &["msg", "modes"]).unwrap_or_default());
        let scales = mode_scales(&output("nekoland", &["msg", "scales"]).unwrap_or_default());
        let resolution = Rc::new(Resolution {
            display: RefCell::new(display),
            modes,
            scales,
            group: group(""),
            rows: RefCell::default(),
            pictures: group(""),
            looks: gtk::Box::builder().spacing(12).homogeneous(true).build(),
            all: adw::ExpanderRow::builder().title("所有解析度").build(),
            rate: adw::ComboRow::builder().title("更新頻率").build(),
            filling_rates: Cell::new(false),
        });
        resolution.pictures.add(&resolution.looks);
        resolution.group.add(&resolution.all);
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
        self.pictures.set_visible(looks.len() > 1);
        let mut first: Option<gtk::ToggleButton> = None;
        // A look with no name keeps the line of one, so that the sizes under them line up.
        let names = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
        for (choice, label) in looks {
            let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
            content.append(&look_picture(choice.scale, choice.is(current)));
            let name = gtk::Label::new(Some(label));
            names.add_widget(&name);
            content.append(&name);
            // The size it looks like, under its name: the pictures alone do not tell two
            // near sizes apart.
            let size = choice.looks_like().label();
            content.append(&gtk::Label::builder().label(&size).css_classes(["dim-label", "caption"]).build());
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
        apply_choice(choice, &display, &self.scales);
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
        set(&format!("mode = \"{}@{hz}\"", mode.setting()));
        self.display.borrow_mut().current.1 = hz;
    }
}

/// The connected display's name and size, from its EDID; a display that gives none, as a
/// virtual machine's, is named by how it is connected.
// shortcut: the first connected display, which is the one the page sets while the settings
// are the same for every display.
fn display_identity() -> (String, Option<String>) {
    let connector = std::fs::read_dir(DRM).ok().and_then(|entries| {
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("status").is_file())
            .find(|path| std::fs::read_to_string(path.join("status")).is_ok_and(|status| status.trim() == "connected"))
    });
    let Some(connector) = connector else {
        return ("顯示器".into(), None);
    };
    let edid = std::fs::read(connector.join("edid")).unwrap_or_default();
    let name = edid_name(&edid).unwrap_or_else(|| kind_name(&connector).into());
    let size = edid_inches(&edid).map(|inches| format!("{inches:.0} 吋"));
    (name, size)
}

/// `card1-eDP-1` is an eDP panel's.
fn kind_name(connector: &Path) -> &'static str {
    let name = connector.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let kind = name
        .split_once('-')
        .map_or("", |(_, rest)| rest.rsplit_once('-').map_or(rest, |(kind, _)| kind));
    if BUILT_IN.contains(&kind) {
        "內建顯示器"
    } else if kind == VIRTUAL {
        "虛擬顯示器"
    } else {
        "外接顯示器"
    }
}

fn edid_name(edid: &[u8]) -> Option<String> {
    EDID_DESCRIPTORS.iter().find_map(|&start| {
        let descriptor = edid.get(start..start + EDID_DESCRIPTOR_LENGTH)?;
        if descriptor[..3] != [0, 0, 0] || descriptor[3] != EDID_NAME_TAG {
            return None;
        }
        let text = &descriptor[EDID_TEXT_START..];
        let end = text.iter().position(|byte| *byte == b'\n').unwrap_or(text.len());
        let name = String::from_utf8_lossy(&text[..end]).trim().to_string();
        (!name.is_empty()).then_some(name)
    })
}

fn edid_inches(edid: &[u8]) -> Option<f64> {
    let (width, height) = (*edid.get(EDID_WIDTH_CM)?, *edid.get(EDID_HEIGHT_CM)?);
    (width > 0 && height > 0).then(|| f64::from(width).hypot(f64::from(height)) / CM_AN_INCH)
}

/// The display's picture and name, over the page as the Mac's.
fn header() -> adw::PreferencesGroup {
    let (name, size) = display_identity();
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    column.append(
        &gtk::Image::builder()
            .icon_name("video-display-symbolic")
            .pixel_size(DISPLAY_ICON_SIZE)
            .build(),
    );
    column.append(&gtk::Label::builder().label(&name).css_classes(["title-3"]).build());
    if let Some(size) = size {
        column.append(&gtk::Label::builder().label(&size).css_classes(["dim-label"]).build());
    }
    let header = group("");
    header.add(&column);
    header
}

/// The displays left to right, as `nekoland msg displays` has them: name and size in points.
fn parse_displays(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (name, size) = (fields.next()?, Size::parse(fields.next()?)?);
            Some((name.to_string(), size.label()))
        })
        .collect()
}

/// The displays in the order they are side by side, to be moved left and right; the leftmost
/// is the main one, with the menu bar and the Dock. None while there is one.
/// The scales a display may be given on its own, as the Mac's list of looks has them.
const DISPLAY_SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

/// 各顯示器縮放: each display's own scale, or the one all share when it has none; none while
/// there is one display.
fn display_scales_group(config: &Config) -> Option<adw::PreferencesGroup> {
    let displays = parse_displays(&output("nekoland", &["msg", "displays"])?);
    if displays.len() < 2 {
        return None;
    }
    let chosen: BTreeMap<String, f64> = config
        .0
        .get("display_scales")
        .and_then(toml::Value::as_table)
        .map(|table| table.iter().filter_map(|(name, scale)| Some((name.clone(), scale.as_float()?))).collect())
        .unwrap_or_default();
    let chosen = Rc::new(RefCell::new(chosen));
    let scales = group("縮放");
    for (name, _) in displays {
        let labels: Vec<String> =
            std::iter::once("預設".to_string()).chain(DISPLAY_SCALES.iter().map(|scale| format!("{scale}×"))).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let current = chosen.borrow().get(&name).and_then(|scale| DISPLAY_SCALES.iter().position(|s| s == scale));
        let row = adw::ComboRow::builder()
            .title(name.as_str())
            .model(&gtk::StringList::new(&labels))
            .selected(current.map_or(0, |index| index as u32 + 1))
            .build();
        let chosen = chosen.clone();
        row.connect_selected_notify(move |row| {
            let mut chosen = chosen.borrow_mut();
            match (row.selected() as usize).checked_sub(1).and_then(|index| DISPLAY_SCALES.get(index)) {
                Some(scale) => chosen.insert(name.clone(), *scale),
                None => chosen.remove(&name),
            };
            if chosen.is_empty() {
                unset("display_scales");
            } else {
                let table: Vec<String> = chosen.iter().map(|(name, scale)| format!("{name:?} = {scale:?}")).collect();
                set(&format!("display_scales = {{ {} }}", table.join(", ")));
            }
        });
        scales.add(&row);
    }
    Some(scales)
}

fn arrangement_group(config: &Config) -> Option<adw::PreferencesGroup> {
    let displays = parse_displays(&output("nekoland", &["msg", "displays"])?);
    if displays.len() < 2 {
        return None;
    }
    let arrangement = group("排列");
    arrangement.add(&switch_row(config, "鏡像顯示", "mirror_displays"));
    let order = Rc::new(RefCell::new(displays));
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    fill_arrangement(&arrangement, &order, &rows);
    Some(arrangement)
}

fn fill_arrangement(
    arrangement: &adw::PreferencesGroup,
    order: &Rc<RefCell<Vec<(String, String)>>>,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
) {
    for row in rows.take() {
        arrangement.remove(&row);
    }
    let last = order.borrow().len() - 1;
    for (position, (name, size)) in order.borrow().iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(name.as_str())
            .subtitle(if position == 0 { format!("{size} · 主要") } else { size.clone() })
            .build();
        let moves = [
            ("go-previous-symbolic", "左移", position.checked_sub(1)),
            ("go-next-symbolic", "右移", Some(position + 1).filter(|to| *to <= last)),
        ];
        for (icon, tooltip, target) in moves {
            let button = gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .sensitive(target.is_some())
                .build();
            if let Some(to) = target {
                let (arrangement, order, rows) = (arrangement.clone(), order.clone(), rows.clone());
                button.connect_clicked(move |_| {
                    order.borrow_mut().swap(position, to);
                    let names: Vec<String> = order.borrow().iter().map(|(name, _)| format!("{name:?}")).collect();
                    set(&format!("display_order = [{}]", names.join(", ")));
                    fill_arrangement(&arrangement, &order, &rows);
                });
            }
            row.add_suffix(&button);
        }
        arrangement.add(&row);
        rows.borrow_mut().push(row);
    }
}

/// The backlight's slider, between a small sun and a large one; none without a backlight.
fn brightness_group() -> Option<adw::PreferencesGroup> {
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
    let brightness = group("");
    brightness.add(&row);
    Some(brightness)
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

fn apply_rotation(degrees: i64) {
    if degrees == 0 {
        unset("rotation");
    } else {
        set(&format!("rotation = {degrees}"));
    }
}

/// The picture's rotation; a new one is turned back unless it is kept in time.
fn rotation_row(config: &Config) -> adw::ComboRow {
    let current = config.number("rotation", 0.0) as i64;
    let labels: Vec<&str> = ROTATIONS.iter().map(|(label, _)| *label).collect();
    let kept = ROTATIONS.iter().position(|(_, degrees)| *degrees == current).unwrap_or(0) as u32;
    let row = adw::ComboRow::builder()
        .title("旋轉")
        .model(&gtk::StringList::new(&labels))
        .selected(kept)
        .build();
    let kept = Rc::new(Cell::new(kept));
    // Set while the row is put back, which is not a choice of a rotation.
    let reverting = Rc::new(Cell::new(false));
    row.connect_selected_notify(move |row| {
        if reverting.get() || row.selected() == kept.get() {
            return;
        }
        let Some(&(_, degrees)) = ROTATIONS.get(row.selected() as usize) else {
            return;
        };
        apply_rotation(degrees);
        ask_to_keep_rotation(row, kept.clone(), reverting.clone());
    });
    row
}

fn countdown_text(seconds: u32) -> String {
    format!("{seconds} 秒後還原。")
}

fn ask_to_keep_rotation(row: &adw::ComboRow, kept: Rc<Cell<u32>>, reverting: Rc<Cell<bool>>) {
    let alert = adw::AlertDialog::new(Some("保留這個旋轉方向？"), Some(&countdown_text(KEEP_ROTATION_WITHIN)));
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
            if let Some(&(_, degrees)) = ROTATIONS.get(kept.get() as usize) {
                apply_rotation(degrees);
            }
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

fn night_shift_dialog(parent: &gtk::Button) {
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
    adw::Dialog::builder()
        .title("夜覽")
        .content_width(NIGHT_SHIFT_WIDTH)
        // Tall enough for the custom hours too, so that choosing them does not push the
        // warmth out of sight.
        .content_height(NIGHT_SHIFT_HEIGHT)
        .child(&view)
        .build()
        .present(Some(parent));
}

/// The buttons under the page, at its right, as the Mac's.
fn buttons() -> adw::PreferencesGroup {
    let night_shift = gtk::Button::builder().label("夜覽⋯").build();
    night_shift.connect_clicked(night_shift_dialog);
    let row = gtk::Box::builder().halign(gtk::Align::End).spacing(8).build();
    row.append(&night_shift);
    let buttons = group("");
    buttons.add(&row);
    buttons
}

pub fn display_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.add(&header());
    if let Some(arrangement) = arrangement_group(config) {
        page.add(&arrangement);
    }
    if let Some(scales) = display_scales_group(config) {
        page.add(&scales);
    }
    let resolution = Resolution::new();
    if let Some(resolution) = &resolution {
        page.add(&resolution.pictures);
        page.add(&resolution.group);
    }
    if let Some(brightness) = brightness_group() {
        page.add(&brightness);
    }
    let picture = group("");
    if let Some(resolution) = &resolution {
        picture.add(&resolution.rate);
    }
    picture.add(&rotation_row(config));
    picture.add(&switch_row(config, "顯示 FPS", "show_fps"));
    picture.add(&super::colour_profile_row(config));
    page.add(&picture);
    page.add(&buttons());
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> Display {
        parse_display("current 3840x2160@60\npreferred 3840x2160@60\nscale 1.5\ndensity_scale 2\n").unwrap()
    }

    #[test]
    fn displays_are_read_left_to_right() {
        let text = "eDP-1\t1536x864\t0\nHDMI-A-1\t1920x1080\t1536\nbroken\n";
        assert_eq!(
            parse_displays(text),
            [("eDP-1".to_string(), "1536 × 864".to_string()), ("HDMI-A-1".to_string(), "1920 × 1080".to_string())]
        );
    }

    #[test]
    fn the_display_is_read_from_its_lines() {
        let display = display();
        assert_eq!(display.current, (Size { width: 3840, height: 2160 }, 60));
        assert_eq!(display.preferred, Size { width: 3840, height: 2160 });
        assert_eq!((display.scale, display.density_scale), (1.5, 2.0));
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
        };
        let heights: Vec<i32> = resolution_choices(&display, &[], &[], false)
            .iter()
            .map(|choice| choice.looks_like().height)
            .collect();
        assert_eq!(heights, [768, 614]);
    }

    #[test]
    fn the_name_comes_from_the_edid_else_the_connector() {
        let mut edid = vec![0u8; 128];
        edid[EDID_WIDTH_CM] = 60;
        edid[EDID_HEIGHT_CM] = 34;
        edid[72..77].copy_from_slice(&[0, 0, 0, EDID_NAME_TAG, 0]);
        edid[77..90].copy_from_slice(b"DELL U2723Q\n ");
        assert_eq!(edid_name(&edid).as_deref(), Some("DELL U2723Q"));
        assert_eq!(edid_inches(&edid).map(|inches| inches.round()), Some(27.0));
        assert_eq!(edid_name(&[]), None);
        assert_eq!(kind_name(Path::new("/sys/class/drm/card0-eDP-1")), "內建顯示器");
        assert_eq!(kind_name(Path::new("/sys/class/drm/card1-Virtual-1")), "虛擬顯示器");
        assert_eq!(kind_name(Path::new("/sys/class/drm/card0-HDMI-A-1")), "外接顯示器");
    }
}
