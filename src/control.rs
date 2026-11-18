//! The control center, as the Mac's: a panel of the menus' glass under its icon in the menu
//! bar, with what is recording; the network, Bluetooth, Do Not Disturb, light or dark, Night
//! Shift and a screenshot; the display's brightness, the volume and the microphone, where the
//! sound plays and is recorded from, and what is playing. A tile that is on is solid ink, as
//! anything chosen is in Nekoland.

use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use cairo::{Context, ImageSurface};
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    config,
    glass::Tier,
    menubar::{
        colors, font, network_icon, show_text, sound_icon, Colors, Overlay, MENU_BAR_HEIGHT, MENU_RADIUS,
    },
    privacy::{self, InUse},
    system::{self, AudioDevice, Network, Player, Volume, Wifi},
    text::{self, Picture},
    Nekoland,
};

const WIDTH: i32 = 320;
const PADDING: i32 = 12;
const GAP: i32 = 8;
const TILE_HEIGHT: i32 = 52;
const GROUP_HEIGHT: i32 = 64;
/// A group's second slider, under its first.
const SLIDER_ROW: i32 = 32;
const IN_USE_ROW: i32 = 22;
/// The output's row in its group, and the group's padding around it.
const OUTPUT_ROW: i32 = 36;
const INSET: i32 = 4;
/// A row of the lists that open, the networks and the outputs, and the rule above the last.
const LIST_ROW: i32 = 30;
const LIST_RULE: i32 = 9;
const PLAYER_HEIGHT: i32 = 64;
const ART: i32 = 40;
const BUTTON: i32 = 28;
const ICON: i32 = 16;
/// A slider's knob, its radius.
const KNOB: i32 = 7;
/// How long the panel takes to fade in, as the Notification Center does too.
pub(crate) const OPEN_TIME: f64 = 0.16;
const EXPAND_TIME: f64 = 0.2;
/// How much a tile shrinks while pressed.
const PRESSED_SCALE: f64 = 0.97;
/// How often a dragged slider tells the system, which takes a process each time.
const SET_EVERY: Duration = Duration::from_millis(50);
/// How often what the panel shows is read again while it is open.
const READ_EVERY: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    InUse,
    Network,
    Bluetooth,
    Quiet,
    Dark,
    Night,
    Screenshot,
    Wifi(usize),
    WifiPower,
    WifiSettings,
    Brightness,
    Mute,
    Volume,
    MicrophoneMute,
    Microphone,
    Output,
    Sink(usize),
    Input,
    Source(usize),
    Player,
    Previous,
    PlayPause,
    Next,
}

impl Part {
    fn is_slider(self) -> bool {
        matches!(self, Part::Brightness | Part::Volume | Part::Microphone)
    }

    fn is_tile(self) -> bool {
        matches!(
            self,
            Part::Network | Part::Bluetooth | Part::Quiet | Part::Dark | Part::Night | Part::Screenshot
        )
    }
}

/// The lists that open in the panel, one at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum List {
    Wifi,
    Output,
    Input,
}

/// What is read off the compositor's thread, when the panel opens and every `READ_EVERY`.
struct Found {
    brightness: Option<u32>,
    /// The sound outputs and inputs.
    devices: (Vec<AudioDevice>, Vec<AudioDevice>),
    wifi: Option<Wifi>,
    microphone: Option<Volume>,
    player: Option<Player>,
}

/// What the panel shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
struct Shown {
    dark: bool,
    quiet: bool,
    night: bool,
    in_use: InUse,
    network: Option<Network>,
    wifi: Option<Wifi>,
    volume: Option<Volume>,
    microphone: Option<Volume>,
    brightness: Option<u32>,
    sinks: Vec<AudioDevice>,
    sources: Vec<AudioDevice>,
    player: Option<Player>,
    /// How far each list is open, in thousandths.
    wifi_open: u32,
    output_open: u32,
    input_open: u32,
    pressed: Option<Part>,
    hovered: Option<Part>,
}

pub struct ControlCenter {
    /// Its left edge, from the output's.
    x: i32,
    opened: Instant,
    found: Option<Receiver<Found>>,
    // What decides where everything is, as of the last refresh.
    in_use: InUse,
    night_available: bool,
    has_volume: bool,
    bluetooth: bool,
    brightness: Option<u32>,
    sinks: Vec<AudioDevice>,
    sources: Vec<AudioDevice>,
    wifi: Option<Wifi>,
    microphone: Option<Volume>,
    player: Option<Player>,
    /// The cover of what plays, by the path it was read from.
    art: Option<(String, Option<ImageSurface>)>,
    open: Option<List>,
    /// The list closing as `open` opens, and when that began.
    closing: Option<List>,
    toggled: Option<Instant>,
    pressed: Option<Part>,
    hovered: Option<Part>,
    /// The slider being dragged, and when it last told the system.
    dragging: Option<(Part, Instant)>,
    shown: Option<Shown>,
    picture: Option<Picture>,
    id: Id,
}

/// The wells the panel's parts sit in, each with its title, and the parts, all from the
/// panel's corner.
struct Layout {
    height: i32,
    groups: Vec<(Rectangle<i32, Logical>, Option<&'static str>)>,
    parts: Vec<(Part, Rectangle<i32, Logical>)>,
}

impl Layout {
    fn find(&self, part: Part) -> Option<Rectangle<i32, Logical>> {
        self.parts
            .iter()
            .find(|(found, _)| *found == part)
            .map(|(_, rect)| *rect)
    }
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

pub(crate) fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// What is recording, in the order the bar's dots are, with each one's colour.
fn recording(in_use: InUse) -> Vec<(&'static str, (f64, f64, f64))> {
    [
        (in_use.microphone, "麥克風", privacy::MICROPHONE),
        (in_use.camera, "相機", privacy::CAMERA),
        (in_use.screen, "螢幕錄製", privacy::SCREEN),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, name, color)| (name, color))
    .collect()
}

fn read_found() -> Found {
    Found {
        brightness: system::read_brightness(),
        devices: system::read_audio_devices(),
        wifi: system::read_wifi(),
        microphone: system::read_volume(system::DEFAULT_SOURCE),
        player: system::read_player(),
    }
}

impl ControlCenter {
    pub(crate) fn dragging(&self) -> bool {
        self.dragging.is_some()
    }

    fn new(anchor: Rectangle<i32, Logical>, output_width: i32) -> Self {
        let (send, found) = mpsc::channel();
        // It stops once the panel closes and nobody takes what it reads.
        let spawned = std::thread::Builder::new()
            .name("nekoland-control".into())
            .spawn(move || {
                while send.send(read_found()).is_ok() {
                    std::thread::sleep(READ_EVERY);
                }
            });
        if let Err(err) = &spawned {
            tracing::warn!("control center: {err}");
        }
        Self {
            x: (anchor.loc.x + anchor.size.w / 2 - WIDTH / 2)
                .min(output_width - WIDTH - 6)
                .max(6),
            opened: Instant::now(),
            found: spawned.is_ok().then_some(found),
            in_use: InUse::default(),
            night_available: false,
            has_volume: false,
            bluetooth: system::has_bluetooth(),
            brightness: None,
            sinks: Vec::new(),
            sources: Vec::new(),
            wifi: None,
            microphone: None,
            player: None,
            art: None,
            open: None,
            closing: None,
            toggled: None,
            pressed: None,
            hovered: None,
            dragging: None,
            shown: None,
            picture: None,
            id: Id::new(),
        }
    }

    /// How far `list` is open, from 0 to 1.
    fn expansion(&self, list: List) -> f64 {
        let progress = self.toggled.map_or(1.0, |toggled| {
            ease_out(toggled.elapsed().as_secs_f64() / EXPAND_TIME)
        });
        if self.open == Some(list) {
            progress
        } else if self.closing == Some(list) {
            1.0 - progress
        } else {
            0.0
        }
    }

    /// Opens `list`, or closes it if it is open, closing any other.
    fn toggle(&mut self, list: List) {
        self.closing = self.open;
        self.open = (self.open != Some(list)).then_some(list);
        self.toggled = Some(Instant::now());
        self.hovered = None;
    }

    fn layout(&self) -> Layout {
        let inner = WIDTH - PADDING * 2;
        let mut groups = Vec::new();
        let mut parts = Vec::new();
        let mut y = PADDING;
        let row = |y| rect(PADDING + INSET, y, inner - INSET * 2, LIST_ROW);

        let recording = recording(self.in_use).len() as i32;
        if recording > 0 {
            let height = 26 + IN_USE_ROW * recording + 6;
            groups.push((rect(PADDING, y, inner, height), Some("使用中")));
            parts.push((Part::InUse, rect(PADDING, y + 26, inner, IN_USE_ROW * recording)));
            y += height + GAP;
        }

        let tile = (inner - GAP) / 2;
        let tiles: Vec<Part> = [Part::Network, Part::Bluetooth, Part::Quiet, Part::Dark]
            .into_iter()
            .chain(self.night_available.then_some(Part::Night))
            .chain([Part::Screenshot])
            .collect();
        for (index, part) in tiles.iter().enumerate() {
            let (column, line) = (index as i32 % 2, index as i32 / 2);
            parts.push((
                *part,
                rect(
                    PADDING + column * (tile + GAP),
                    y + line * (TILE_HEIGHT + GAP),
                    tile,
                    TILE_HEIGHT,
                ),
            ));
        }
        y += (tiles.len() as i32 + 1) / 2 * (TILE_HEIGHT + GAP);

        // A list that opens: the well grows to it, and what is in it is there once it has.
        let mut list = |y: &mut i32, open: f64, full: i32, fill: &mut dyn FnMut(i32)| {
            if open <= 0.0 {
                return;
            }
            let height = (f64::from(full) * open).round() as i32;
            groups.push((rect(PADDING, *y, inner, height), None));
            if open >= 1.0 {
                fill(*y + INSET);
            }
            *y += height + (f64::from(GAP) * open).round() as i32;
        };
        if let Some(wifi) = &self.wifi {
            let networks = wifi.networks.len() as i32;
            let rule = if networks > 0 { LIST_RULE } else { 0 };
            let full = INSET * 2 + LIST_ROW * (networks + 2) + rule;
            list(&mut y, self.expansion(List::Wifi), full, &mut |top| {
                for index in 0..wifi.networks.len() {
                    parts.push((Part::Wifi(index), row(top + LIST_ROW * index as i32)));
                }
                parts.push((Part::WifiPower, row(top + LIST_ROW * networks + rule)));
                parts.push((Part::WifiSettings, row(top + LIST_ROW * (networks + 1) + rule)));
            });
        }

        // A slider: its icon at the left, its value, then its track; the part is the track,
        // where the knob can go, which stays within the line. A group's padding, then the
        // design language's 8 either side of the widest value.
        let start = 12 + ICON + 8 + value_width() + 8 + KNOB;
        let slider = |parts: &mut Vec<_>, icon: Option<Part>, track: Part, row: i32| {
            if let Some(icon) = icon {
                parts.push((icon, rect(PADDING + 8, row, 24, 24)));
            }
            // The line stops a group's padding from the well's edge.
            parts.push((track, rect(PADDING + start, row, inner - start - 12 - KNOB, 24)));
        };
        if self.brightness.is_some() {
            groups.push((rect(PADDING, y, inner, GROUP_HEIGHT), Some("顯示器")));
            slider(&mut parts, None, Part::Brightness, y + 30);
            y += GROUP_HEIGHT + GAP;
        }
        let sliders = [
            (self.has_volume, Part::Mute, Part::Volume),
            (self.microphone.is_some(), Part::MicrophoneMute, Part::Microphone),
        ];
        let shown = sliders.iter().filter(|(shown, ..)| *shown).count() as i32;
        if shown > 0 {
            let height = GROUP_HEIGHT + SLIDER_ROW * (shown - 1);
            groups.push((rect(PADDING, y, inner, height), Some("聲音")));
            let mut at = y + 30;
            for (_, icon, track) in sliders.iter().filter(|(shown, ..)| *shown) {
                slider(&mut parts, Some(*icon), *track, at);
                at += SLIDER_ROW;
            }
            y += height + GAP;
        }

        let devices = [
            (
                List::Output,
                &self.sinks,
                Part::Output,
                Part::Sink as fn(usize) -> Part,
            ),
            (List::Input, &self.sources, Part::Input, Part::Source),
        ];
        for (list, devices, current, device) in devices {
            if devices.is_empty() {
                continue;
            }
            let open = self.expansion(list);
            let full = LIST_RULE + LIST_ROW * devices.len() as i32;
            let height = OUTPUT_ROW + INSET * 2 + (f64::from(full) * open).round() as i32;
            groups.push((rect(PADDING, y, inner, height), None));
            parts.push((
                current,
                rect(PADDING + INSET, y + INSET, inner - INSET * 2, OUTPUT_ROW),
            ));
            if open >= 1.0 {
                let top = y + INSET + OUTPUT_ROW + LIST_RULE;
                for index in 0..devices.len() {
                    parts.push((device(index), row(top + LIST_ROW * index as i32)));
                }
            }
            y += height + GAP;
        }

        if self.player.is_some() {
            groups.push((rect(PADDING, y, inner, PLAYER_HEIGHT), None));
            let buttons = [Part::Previous, Part::PlayPause, Part::Next];
            let first = PADDING + inner - 8 - BUTTON * buttons.len() as i32;
            parts.push((
                Part::Player,
                rect(
                    PADDING + 12,
                    y + (PLAYER_HEIGHT - ART) / 2,
                    first - PADDING - 20,
                    ART,
                ),
            ));
            for (index, part) in buttons.into_iter().enumerate() {
                parts.push((
                    part,
                    rect(
                        first + BUTTON * index as i32,
                        y + (PLAYER_HEIGHT - BUTTON) / 2,
                        BUTTON,
                        BUTTON,
                    ),
                ));
            }
            y += PLAYER_HEIGHT + GAP;
        }

        Layout {
            height: y - GAP + PADDING,
            groups,
            parts,
        }
    }

    fn part_at(&self, local: Point<i32, Logical>) -> Option<Part> {
        let layout = self.layout();
        layout.parts.iter().find_map(|(part, rect)| {
            // A slider takes the pointer a little past its ends, and its whole row.
            let rect = if part.is_slider() {
                Rectangle::new(rect.loc - Point::from((8, 2)), rect.size + (16, 4).into())
            } else {
                *rect
            };
            rect.contains(local).then_some(*part)
        })
    }

    /// The cover of what plays, read again only when it is another picture.
    fn art(&mut self) -> Option<&ImageSurface> {
        let path = self.player.as_ref()?.art.clone()?;
        if self.art.as_ref().map(|(read, _)| read) != Some(&path) {
            let scale = text::picture_scale();
            let surface = gdk_pixbuf::Pixbuf::from_file_at_scale(&path, ART * scale, ART * scale, true)
                .inspect_err(|err| tracing::debug!("{path}: {err}"))
                .ok()
                .and_then(|pixbuf| text::pixbuf_surface(&pixbuf, None))
                .inspect(|surface| surface.set_device_scale(f64::from(scale), f64::from(scale)));
            self.art = Some((path, surface));
        }
        self.art.as_ref()?.1.as_ref()
    }

    fn draw(&mut self, shown: &Shown) {
        let colors = colors(shown.dark);
        let layout = self.layout();
        let Ok(surface) = text::canvas(WIDTH, layout.height) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        draw_panel(&cr, &colors, shown.dark, (WIDTH, layout.height));

        let well =
            |cr: &Context, area: Rectangle<i32, Logical>, radius: f64| draw_well(cr, &colors, area, radius);
        let small = font("", 9.0);
        for (group, title) in &layout.groups {
            if group.size.h < 2 {
                continue;
            }
            well(&cr, *group, 8.0);
            if let Some(title) = title {
                show_text(
                    &cr,
                    title,
                    &small,
                    f64::from(group.loc.x + 12),
                    f64::from(group.loc.y + 18),
                    colors.ink_3,
                );
            }
        }

        let label = font("", 10.0);
        // A row of a list, lit in ink under the pointer as a menu's item is.
        let list_row = |cr: &Context, part: Part, area: Rectangle<i32, Logical>| {
            let lit = shown.hovered == Some(part);
            if lit {
                let (x, y, w, h) = to_f64(area);
                text::rounded_rectangle(cr, x, y, w, h, 6.0);
                cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                let _ = cr.fill();
            }
            if lit {
                colors.on_ink
            } else {
                colors.ink
            }
        };
        for (part, area) in &layout.parts {
            let (x, y, w, h) = to_f64(*area);
            let center = y + h / 2.0;
            match *part {
                Part::InUse => {
                    for (index, (name, (r, g, b))) in recording(shown.in_use).into_iter().enumerate() {
                        let center = y + f64::from(IN_USE_ROW) * (index as f64 + 0.5);
                        cr.arc(x + 16.0, center, 3.5, 0.0, std::f64::consts::TAU);
                        cr.set_source_rgb(r, g, b);
                        let _ = cr.fill();
                        show_text(&cr, name, &label, x + 30.0, center, colors.ink);
                    }
                }
                tile if tile.is_tile() => {
                    let (on, icon, title, detail) = tile_face(tile, shown, self.bluetooth);
                    cr.save().ok();
                    if shown.pressed == Some(tile) {
                        cr.translate(x + w / 2.0, center);
                        cr.scale(PRESSED_SCALE, PRESSED_SCALE);
                        cr.translate(-(x + w / 2.0), -center);
                    }
                    if on {
                        text::rounded_rectangle(&cr, x, y, w, h, 8.0);
                        cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                        let _ = cr.fill();
                    } else {
                        well(&cr, *area, 8.0);
                    }
                    let color = if on { colors.on_ink } else { colors.ink };
                    draw_icon(&cr, icon, x + 12.0, center, color);
                    match detail {
                        Some(detail) => {
                            show_text(&cr, title, &label, x + 38.0, center - 8.0, color);
                            cr.push_group();
                            let width = w - 38.0 - if tile == Part::Network { 22.0 } else { 8.0 };
                            show_line(&cr, &detail, &small, (x + 38.0, center + 9.0, width), color);
                            let _ = cr.pop_group_to_source();
                            let _ = cr.paint_with_alpha(0.7);
                        }
                        None => show_text(&cr, title, &label, x + 38.0, center, color),
                    }
                    if tile == Part::Network && shown.wifi.is_some() {
                        let turn = f64::from(shown.wifi_open) / 1000.0;
                        draw_chevron(&cr, x + w - 14.0, center, turn, color);
                    }
                    cr.restore().ok();
                }
                Part::Wifi(index) => {
                    let Some(network) = shown.wifi.as_ref().and_then(|wifi| wifi.networks.get(index)) else {
                        continue;
                    };
                    let color = list_row(&cr, *part, *area);
                    if network.active {
                        show_text(&cr, "✓", &label, x + 10.0, center, color);
                    }
                    let icon = match network.signal {
                        0..=24 => "network-wireless-signal-weak-symbolic",
                        25..=49 => "network-wireless-signal-ok-symbolic",
                        50..=74 => "network-wireless-signal-good-symbolic",
                        _ => "network-wireless-signal-excellent-symbolic",
                    };
                    let signal_x = x + w - 10.0 - f64::from(ICON);
                    let lock = if network.secure { f64::from(ICON) + 4.0 } else { 0.0 };
                    show_line(&cr, &network.ssid, &label, (x + 34.0, center, w - 70.0 - lock), color);
                    if network.secure {
                        draw_icon(&cr, "changes-prevent-symbolic", signal_x - lock, center, color);
                    }
                    draw_icon(&cr, icon, signal_x, center, color);
                }
                Part::WifiPower => {
                    if shown.wifi.as_ref().is_some_and(|wifi| !wifi.networks.is_empty()) {
                        rule(&cr, &colors, x, y - f64::from(LIST_RULE) / 2.0, w);
                    }
                    let color = list_row(&cr, *part, *area);
                    let on = shown.wifi.as_ref().is_some_and(|wifi| wifi.on);
                    let text = if on { "關閉 Wi-Fi" } else { "開啟 Wi-Fi" };
                    show_text(&cr, text, &label, x + 34.0, center, color);
                }
                Part::WifiSettings => {
                    let color = list_row(&cr, *part, *area);
                    show_text(&cr, "Wi-Fi 設定…", &label, x + 34.0, center, color);
                }
                Part::Mute => {
                    let icon = shown.volume.map_or("audio-volume-high-symbolic", sound_icon);
                    draw_icon(&cr, icon, x + 4.0, center, colors.ink);
                }
                Part::MicrophoneMute => {
                    let muted = shown
                        .microphone
                        .is_some_and(|volume| volume.muted || volume.percent == 0);
                    let icon = if muted {
                        "microphone-disabled-symbolic"
                    } else {
                        "microphone-sensitivity-high-symbolic"
                    };
                    draw_icon(&cr, icon, x + 4.0, center, colors.ink);
                }
                slider if slider.is_slider() => {
                    if slider == Part::Brightness {
                        let left = f64::from(PADDING + 12);
                        draw_icon(&cr, "display-brightness-symbolic", left, center, colors.ink);
                    }
                    let (level, muted) = match slider {
                        Part::Brightness => (shown.brightness.unwrap_or(0), false),
                        Part::Volume => shown
                            .volume
                            .map_or((0, false), |volume| (volume.percent, volume.muted)),
                        _ => shown
                            .microphone
                            .map_or((0, false), |volume| (volume.percent, volume.muted)),
                    };
                    let level = if muted {
                        0.0
                    } else {
                        f64::from(level.min(100)) / 100.0
                    };
                    draw_slider(&cr, &colors, shown.dark, (x, center, w), level);
                    let value = value_layout(&cr, &format!("{:.0}%", level * 100.0));
                    let (width, height) = value.pixel_size();
                    cr.set_source_rgb(colors.ink_3.0, colors.ink_3.1, colors.ink_3.2);
                    cr.move_to(x - f64::from(KNOB + 8 + width), center - f64::from(height) / 2.0);
                    pangocairo::functions::show_layout(&cr, &value);
                }
                Part::Output | Part::Input => {
                    let (devices, open) = match part {
                        Part::Output => (&shown.sinks, shown.output_open),
                        _ => (&shown.sources, shown.input_open),
                    };
                    let current = devices.iter().find(|device| device.default).or(devices.first());
                    if *part == Part::Output {
                        draw_speaker(&cr, x + 8.0, center, colors.ink);
                    } else {
                        draw_icon(
                            &cr,
                            "audio-input-microphone-symbolic",
                            x + 8.0,
                            center,
                            colors.ink,
                        );
                    }
                    if let Some(device) = current {
                        show_line(
                            &cr,
                            &device.description,
                            &label,
                            (x + 34.0, center, w - 64.0),
                            colors.ink,
                        );
                    }
                    let turn = f64::from(open) / 1000.0;
                    draw_chevron(&cr, x + w - 16.0, center, turn, colors.ink_3);
                    if open > 0 {
                        rule(&cr, &colors, x, y + h + f64::from(LIST_RULE / 2), w);
                    }
                }
                Part::Sink(index) | Part::Source(index) => {
                    let devices = if matches!(part, Part::Sink(_)) {
                        &shown.sinks
                    } else {
                        &shown.sources
                    };
                    let Some(device) = devices.get(index) else {
                        continue;
                    };
                    let color = list_row(&cr, *part, *area);
                    if device.default {
                        show_text(&cr, "✓", &label, x + 10.0, center, color);
                    }
                    show_line(
                        &cr,
                        &device.description,
                        &label,
                        (x + 34.0, center, w - 44.0),
                        color,
                    );
                }
                Part::Player => {
                    let Some(player) = shown.player.as_ref() else {
                        continue;
                    };
                    let art = f64::from(ART);
                    match self.art() {
                        Some(cover) => {
                            let (cover_w, cover_h) = text::points(cover);
                            cr.save().ok();
                            text::rounded_rectangle(&cr, x, y, art, art, 6.0);
                            cr.clip();
                            let _ = cr.set_source_surface(
                                cover,
                                x + (art - cover_w) / 2.0,
                                y + (art - cover_h) / 2.0,
                            );
                            let _ = cr.paint();
                            cr.restore().ok();
                        }
                        None => {
                            well(&cr, rect(x as i32, y as i32, ART, ART), 6.0);
                            let middle = x + (art - f64::from(ICON)) / 2.0;
                            draw_icon(&cr, "audio-x-generic-symbolic", middle, center, colors.ink_3);
                        }
                    }
                    let left = x + art + 10.0;
                    let width = w - art - 10.0;
                    show_line(
                        &cr,
                        &player.title,
                        &label,
                        (left, center - 8.0, width),
                        colors.ink,
                    );
                    let by = if player.artist.is_empty() {
                        &player.app
                    } else {
                        &player.artist
                    };
                    show_line(&cr, by, &small, (left, center + 9.0, width), colors.ink_3);
                }
                button @ (Part::Previous | Part::PlayPause | Part::Next) => {
                    if shown.hovered == Some(button) {
                        well(&cr, *area, 6.0);
                    }
                    let playing = shown.player.as_ref().is_some_and(|player| player.playing);
                    let icon = match button {
                        Part::Previous => "media-skip-backward-symbolic",
                        Part::Next => "media-skip-forward-symbolic",
                        _ if playing => "media-playback-pause-symbolic",
                        _ => "media-playback-start-symbolic",
                    };
                    draw_icon(&cr, icon, x + (w - f64::from(ICON)) / 2.0, center, colors.ink);
                }
                _ => {}
            }
        }
        drop(cr);
        self.picture = text::picture(surface);
    }
}

/// A tile's look: whether it is on, its icon, its title and what it says under the title.
fn tile_face(
    tile: Part,
    shown: &Shown,
    bluetooth: bool,
) -> (bool, &'static str, &'static str, Option<String>) {
    match tile {
        Part::Network => {
            let network = shown.network.as_ref();
            let title = match network {
                Some(network) if network.wireless => "Wi-Fi",
                Some(_) => "乙太網路",
                None => "網路",
            };
            let connected = network.is_some_and(|network| network.connected);
            let icon = network.map_or("network-offline-symbolic", network_icon);
            let active = shown
                .wifi
                .as_ref()
                .and_then(|wifi| wifi.networks.iter().find(|network| network.active));
            let (on, detail) = match (&shown.wifi, active) {
                (Some(wifi), _) if !wifi.on => (false, "已關閉".to_string()),
                (Some(_), Some(active)) => (true, active.ssid.clone()),
                _ => (connected, if connected { "已連線" } else { "未連線" }.to_string()),
            };
            (on, icon, title, Some(detail))
        }
        Part::Bluetooth => {
            let (icon, detail) = if bluetooth {
                ("bluetooth-symbolic", "未連線")
            } else {
                ("bluetooth-disabled-symbolic", "沒有裝置")
            };
            (false, icon, "藍牙", Some(detail.to_string()))
        }
        Part::Quiet => (shown.quiet, "notifications-disabled-symbolic", "勿擾", None),
        Part::Dark => (shown.dark, "weather-clear-night-symbolic", "深色", None),
        Part::Night => (shown.night, "night-light-symbolic", "夜覽", None),
        _ => (false, "applets-screenshooter-symbolic", "截圖", None),
    }
}

/// A panel of the menus' glass, `size` in points from the context's origin: its tint on the
/// menus' corners, and its edge.
pub(crate) fn draw_panel(cr: &Context, colors: &Colors, dark: bool, (width, height): (i32, i32)) {
    text::rounded_rectangle(
        cr,
        0.5,
        0.5,
        f64::from(width) - 1.0,
        f64::from(height) - 1.0,
        f64::from(MENU_RADIUS),
    );
    let (r, g, b, a) = crate::glass::tint(dark, Tier::Menu);
    cr.set_source_rgba(r, g, b, a);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(colors.edge.0, colors.edge.1, colors.edge.2, colors.edge.3);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

/// A well of a panel, a little ink over its glass, that a group of its parts sits in.
pub(crate) fn draw_well(cr: &Context, colors: &Colors, area: Rectangle<i32, Logical>, radius: f64) {
    let (x, y, w, h) = to_f64(area);
    text::rounded_rectangle(cr, x, y, w, h, radius);
    cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.07);
    let _ = cr.fill();
}

pub(crate) fn to_f64(area: Rectangle<i32, Logical>) -> (f64, f64, f64, f64) {
    (
        f64::from(area.loc.x),
        f64::from(area.loc.y),
        f64::from(area.size.w),
        f64::from(area.size.h),
    )
}

/// One line of text, cut short with an ellipsis where it would pass `width`.
pub(crate) fn show_line(
    cr: &Context,
    line: &str,
    font: &pango::FontDescription,
    (x, center, width): (f64, f64, f64),
    color: (f64, f64, f64),
) {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(font));
    layout.set_text(line);
    layout.set_width((width.max(0.0) * f64::from(pango::SCALE)) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    let (_, height) = layout.pixel_size();
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.move_to(x, center - f64::from(height) / 2.0);
    pangocairo::functions::show_layout(cr, &layout);
}

/// The rule between a list's row and the rows under it.
fn rule(cr: &Context, colors: &Colors, x: f64, y: f64, width: f64) {
    cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.12);
    cr.rectangle(x + 4.0, y.floor(), width - 8.0, 1.0);
    let _ = cr.fill();
}

/// A symbolic icon from the theme, its left edge at `x`, centred on `center`.
fn draw_icon(cr: &Context, name: &str, x: f64, center: f64, color: (f64, f64, f64)) {
    if let Some(icon) = text::named_icon(name, "", ICON, color) {
        let _ = cr.set_source_surface(&icon, x, center - f64::from(ICON) / 2.0);
        let _ = cr.paint();
    }
}

/// A speaker box, apart from the volume's icon, for the device the sound plays on.
fn draw_speaker(cr: &Context, x: f64, center: f64, color: (f64, f64, f64)) {
    let y = center - f64::from(ICON) / 2.0;
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(1.5);
    text::rounded_rectangle(cr, x + 4.0, y + 1.75, 8.0, 12.5, 1.5);
    let _ = cr.stroke();
    cr.arc(x + 8.0, y + 9.5, 2.0, 0.0, std::f64::consts::TAU);
    let _ = cr.stroke();
    cr.arc(x + 8.0, y + 4.6, 0.75, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
}

/// A slider's value, its digits all as wide.
fn value_layout(cr: &Context, value: &str) -> pango::Layout {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(&font("", 10.0)));
    layout.set_text(value);
    let digits = pango::AttrList::new();
    digits.insert(pango::AttrFontFeatures::new("tnum 1"));
    layout.set_attributes(Some(&digits));
    layout
}

/// How wide a slider's widest value, "100%", is.
fn value_width() -> i32 {
    let Ok(surface) = ImageSurface::create(cairo::Format::ARgb32, 1, 1) else {
        return 0;
    };
    let Ok(cr) = Context::new(&surface) else {
        return 0;
    };
    value_layout(&cr, "100%").pixel_size().0
}

/// A track, filled in ink as far as `level`, with its knob there. `x` and `width` are where
/// the knob's centre travels; the line runs a knob further each way, out to its far side,
/// as Settings' sliders do.
fn draw_slider(cr: &Context, colors: &Colors, dark: bool, (x, center, width): (f64, f64, f64), level: f64) {
    let at = x + width * level;
    let knob = f64::from(KNOB);
    let x = x - knob;
    text::rounded_rectangle(cr, x, center - 2.0, width + knob * 2.0, 4.0, 2.0);
    // Light ink on dark glass shows less, so the empty track takes more of it there.
    let empty = if dark { 0.25 } else { 0.16 };
    cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, empty);
    let _ = cr.fill();
    text::rounded_rectangle(cr, x, center - 2.0, at - x, 4.0, 2.0);
    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
    let _ = cr.fill();
    cr.arc(at, center, f64::from(KNOB), 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(colors.on_ink.0, colors.on_ink.1, colors.on_ink.2);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

/// The menus' submenu arrow, pointing right, turned down as far as `turn`, from 0 to 1.
fn draw_chevron(cr: &Context, x: f64, center: f64, turn: f64, color: (f64, f64, f64)) {
    let Some(arrow) = text::named_icon("nekoland-submenu-symbolic", "", 16, color) else {
        return;
    };
    cr.save().ok();
    cr.translate(x, center);
    cr.rotate(turn * std::f64::consts::FRAC_PI_2);
    let _ = cr.set_source_surface(&arrow, -8.0, -8.0);
    let _ = cr.paint();
    cr.restore().ok();
}

/// The control center's icon in the bar: two switches, one on each side.
pub(crate) fn draw_bar_icon(cr: &Context, x: f64, y: f64, size: f64, color: (f64, f64, f64)) {
    let s = size / 16.0;
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(s, s);
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(1.5);
    for (top, knob) in [(2.75, 11.75), (8.75, 4.25)] {
        text::rounded_rectangle(cr, 1.75, top, 12.5, 4.5, 2.25);
        let _ = cr.stroke();
        cr.arc(knob, top + 2.25, 1.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
    cr.restore().ok();
}

/// Takes a picture of the whole screen into Pictures, once the panel is gone from it, and
/// says where it went.
const SCREENSHOT: &str = r#"dir=$(xdg-user-dir PICTURES 2>/dev/null || echo "$HOME")
mkdir -p "$dir"
file="$dir/截圖 $(date '+%Y-%m-%d %H.%M.%S').png"
sleep 0.2
grim "$file" && gdbus call --session --dest org.freedesktop.Notifications \
  --object-path /org/freedesktop/Notifications --method org.freedesktop.Notifications.Notify \
  截圖 0 "$file" 已儲存截圖 "$(basename "$file")" '[]' '{}' 5000 >/dev/null"#;

impl Nekoland {
    /// Opens the control center under its icon in the bar, or closes it.
    pub(crate) fn toggle_control_center(&mut self, anchor: Rectangle<i32, Logical>) {
        self.menu_bar.open = None;
        self.menu_bar.notification_center = None;
        self.menu_bar.control = match self.menu_bar.control {
            Some(_) => None,
            None => {
                let width = self.output_area().map_or(0, |area| area.size.w);
                Some(ControlCenter::new(anchor, width))
            }
        };
    }

    /// Takes in what was read for it, and draws it again when anything it shows changed.
    pub(crate) fn refresh_control_center(&mut self) {
        let volume = self.menu_bar.sound;
        let dark = self.menu_bar.dark;
        let network = self.menu_bar.network.clone();
        let in_use = self.menu_bar.in_use;
        let quiet = self.notices.quiet;
        let (night, night_available) = (self.night_shift, self.night_shift_available);
        let Some(control) = self.menu_bar.control.as_mut() else {
            return;
        };
        if let Some(found) = control.found.as_ref().and_then(|found| found.try_iter().last()) {
            let dragging = control.dragging.map(|(part, _)| part);
            if dragging != Some(Part::Brightness) {
                control.brightness = found.brightness;
            }
            if dragging != Some(Part::Microphone) {
                control.microphone = found.microphone;
            }
            (control.sinks, control.sources) = found.devices;
            control.wifi = found.wifi;
            control.player = found.player;
            if control.wifi.is_none() && control.open == Some(List::Wifi) {
                control.open = None;
            }
        }
        control.in_use = in_use;
        control.night_available = night_available;
        control.has_volume = volume.is_some();
        let shown = Shown {
            dark,
            quiet,
            night,
            in_use,
            network,
            wifi: control.wifi.clone(),
            volume,
            microphone: control.microphone,
            brightness: control.brightness,
            sinks: control.sinks.clone(),
            sources: control.sources.clone(),
            player: control.player.clone(),
            wifi_open: (control.expansion(List::Wifi) * 1000.0).round() as u32,
            output_open: (control.expansion(List::Output) * 1000.0).round() as u32,
            input_open: (control.expansion(List::Input) * 1000.0).round() as u32,
            pressed: control.pressed,
            hovered: control.hovered,
        };
        if control.shown.as_ref() != Some(&shown) || control.picture.is_none() {
            control.draw(&shown);
            control.shown = Some(shown);
        }
    }

    pub(crate) fn control_center_overlay(&self) -> Option<Overlay<'_>> {
        let output = self.output_area()?;
        let control = self.menu_bar.control.as_ref()?;
        let picture = control.picture.as_ref()?;
        let size = picture.size;
        Some(Overlay {
            id: &control.id,
            rect: Rectangle::new(output.loc + Point::from((control.x, MENU_BAR_HEIGHT + 4)), size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: ease_out(control.opened.elapsed().as_secs_f64() / OPEN_TIME) as f32,
            picture,
        })
    }

    /// Where a point of the output is on the panel, if it is on it.
    fn control_local(&self, local: Point<i32, Logical>) -> Option<Point<i32, Logical>> {
        let control = self.menu_bar.control.as_ref()?;
        let size = control.picture.as_ref()?.size;
        let at = local - Point::from((control.x, MENU_BAR_HEIGHT + 4));
        Rectangle::new((0, 0).into(), size).contains(at).then_some(at)
    }

    /// A press on the output, at `local`. Returns whether it was on the panel.
    pub(crate) fn control_press(&mut self, local: Point<i32, Logical>) -> bool {
        let Some(at) = self.control_local(local) else {
            return false;
        };
        let Some(control) = self.menu_bar.control.as_mut() else {
            return false;
        };
        let part = control.part_at(at);
        match part {
            Some(slider) if slider.is_slider() => {
                control.dragging = Some((slider, Instant::now() - SET_EVERY));
                self.control_drag(at, false);
            }
            // Nothing to do there: what is recording, Bluetooth, and what plays. The network
            // has its list only with NetworkManager to list it.
            Some(Part::InUse | Part::Bluetooth | Part::Player) => {}
            Some(Part::Network) if control.wifi.is_none() => {}
            _ => control.pressed = part,
        }
        true
    }

    pub(crate) fn control_motion(&mut self, local: Point<i32, Logical>) {
        let at = local
            - self
                .menu_bar
                .control
                .as_ref()
                .map_or_else(Point::default, |control| {
                    Point::from((control.x, MENU_BAR_HEIGHT + 4))
                });
        if self
            .menu_bar
            .control
            .as_ref()
            .is_some_and(|control| control.dragging.is_some())
        {
            self.control_drag(at, false);
            return;
        }
        let inside = self.control_local(local).is_some();
        if let Some(control) = self.menu_bar.control.as_mut() {
            control.hovered = inside.then(|| control.part_at(at)).flatten();
        }
    }

    pub(crate) fn control_release(&mut self, local: Point<i32, Logical>) {
        let Some(control) = self.menu_bar.control.as_ref() else {
            return;
        };
        let at = local - Point::from((control.x, MENU_BAR_HEIGHT + 4));
        if control.dragging.is_some() {
            self.control_drag(at, true);
            if let Some(control) = self.menu_bar.control.as_mut() {
                control.dragging = None;
            }
            return;
        }
        let under = self.control_local(local).and_then(|at| control.part_at(at));
        let pressed = self
            .menu_bar
            .control
            .as_mut()
            .and_then(|control| control.pressed.take());
        if let Some(part) = pressed.filter(|&part| Some(part) == under) {
            self.control_activate(part);
        }
    }

    /// Moves the dragged slider to the pointer, and tells the system every so often, and
    /// when it is let go, `last`.
    fn control_drag(&mut self, at: Point<i32, Logical>, last: bool) {
        let volume = self.menu_bar.sound;
        let Some(control) = self.menu_bar.control.as_mut() else {
            return;
        };
        let Some((slider, told)) = control.dragging else {
            return;
        };
        let Some(track) = control.layout().find(slider) else {
            return;
        };
        let level = (f64::from(at.x - track.loc.x) / f64::from(track.size.w)).clamp(0.0, 1.0);
        let percent = (level * 100.0).round() as u32;
        let tell = last || told.elapsed() >= SET_EVERY;
        if tell {
            control.dragging = Some((slider, Instant::now()));
        }
        match slider {
            Part::Brightness => {
                control.brightness = Some(percent);
                if tell {
                    config::spawn(&format!("brightnessctl --class=backlight set {percent}%"));
                }
            }
            Part::Microphone => {
                let muted = control.microphone.is_some_and(|volume| volume.muted);
                control.microphone = Some(Volume {
                    percent,
                    muted: false,
                });
                if tell {
                    system::run(
                        "wpctl",
                        &["set-volume", system::DEFAULT_SOURCE, &format!("{percent}%")],
                    );
                    if muted {
                        system::run("wpctl", &["set-mute", system::DEFAULT_SOURCE, "0"]);
                    }
                }
            }
            _ => {
                if tell {
                    // As on the Mac, sliding the volume up unmutes it.
                    let unmute = if volume.is_some_and(|volume| volume.muted) {
                        " && wpctl set-mute @DEFAULT_AUDIO_SINK@ 0"
                    } else {
                        ""
                    };
                    config::spawn(&format!(
                        "wpctl set-volume @DEFAULT_AUDIO_SINK@ {percent}%{unmute}"
                    ));
                }
                self.menu_bar.sound = Some(Volume {
                    percent,
                    muted: false,
                });
                self.menu_bar.items_revision += 1;
            }
        }
    }

    fn control_activate(&mut self, part: Part) {
        let Some(control) = self.menu_bar.control.as_mut() else {
            return;
        };
        match part {
            Part::Network => {
                // The list read when the panel opens is what was last heard; it is heard again.
                if control.open != Some(List::Wifi) && control.wifi.as_ref().is_some_and(|wifi| wifi.on) {
                    system::run("nmcli", &["device", "wifi", "rescan"]);
                }
                control.toggle(List::Wifi);
            }
            Part::Wifi(index) => {
                let Some(network) = control.wifi.as_ref().and_then(|wifi| wifi.networks.get(index)) else {
                    return;
                };
                if !network.active {
                    system::join_wifi(&network.ssid);
                }
                control.toggle(List::Wifi);
            }
            Part::WifiSettings => {
                config::spawn("NEKOLAND_SETTINGS_PAGE=Wi-Fi nekoland-settings");
                control.toggle(List::Wifi);
            }
            Part::WifiPower => {
                let Some(wifi) = control.wifi.as_mut() else {
                    return;
                };
                wifi.on = !wifi.on;
                if !wifi.on {
                    wifi.networks.clear();
                }
                system::run("nmcli", &["radio", "wifi", if wifi.on { "on" } else { "off" }]);
                control.hovered = None;
            }
            Part::Quiet => self.notices.quiet = !self.notices.quiet,
            Part::Dark => {
                let dark = !self.menu_bar.dark;
                let scheme = if dark { "prefer-dark" } else { "default" };
                config::spawn(&format!(
                    "gsettings set org.gnome.desktop.interface color-scheme {scheme}"
                ));
                // The portal says so too, a moment later.
                self.menu_bar.dark = dark;
            }
            Part::Night => self.night_shift = !self.night_shift,
            Part::Screenshot => {
                self.menu_bar.control = None;
                config::spawn(SCREENSHOT);
            }
            Part::Mute => {
                config::spawn(&self.config.volume_mute);
                if let Some(volume) = self.menu_bar.sound.as_mut() {
                    volume.muted = !volume.muted;
                    self.menu_bar.items_revision += 1;
                }
            }
            Part::MicrophoneMute => {
                system::run("wpctl", &["set-mute", system::DEFAULT_SOURCE, "toggle"]);
                if let Some(volume) = control.microphone.as_mut() {
                    volume.muted = !volume.muted;
                }
            }
            Part::Output => control.toggle(List::Output),
            Part::Input => control.toggle(List::Input),
            Part::Sink(index) | Part::Source(index) => {
                let (devices, list) = match part {
                    Part::Sink(_) => (&mut control.sinks, List::Output),
                    _ => (&mut control.sources, List::Input),
                };
                if let Some(id) = devices.get(index).map(|device| device.id) {
                    system::run("wpctl", &["set-default", &id.to_string()]);
                    for (other, device) in devices.iter_mut().enumerate() {
                        device.default = other == index;
                    }
                }
                control.toggle(list);
            }
            Part::Previous | Part::PlayPause | Part::Next => {
                let Some(player) = control.player.as_mut() else {
                    return;
                };
                let method = match part {
                    Part::Previous => "Previous",
                    Part::Next => "Next",
                    _ => {
                        player.playing = !player.playing;
                        "PlayPause"
                    }
                };
                let bus = player.bus.clone();
                std::thread::spawn(move || system::player_command(&bus, method));
            }
            Part::InUse
            | Part::Bluetooth
            | Part::Brightness
            | Part::Volume
            | Part::Microphone
            | Part::Player => {}
        }
    }
}
