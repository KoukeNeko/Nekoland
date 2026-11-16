//! The control center, as the Mac's: a panel of the menus' glass under its icon in the menu
//! bar, with the network, light or dark, the display's brightness, the volume and where the
//! sound plays. A tile that is on is solid ink, as anything chosen is in Nekoland.

use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

use cairo::Context;
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
    system::{self, Network, Sink, Volume},
    text::{self, Picture},
    Nekoland,
};

const WIDTH: i32 = 300;
const PADDING: i32 = 12;
const GAP: i32 = 8;
const TILE_HEIGHT: i32 = 56;
const GROUP_HEIGHT: i32 = 64;
/// The output's row in its group, and the group's padding around it.
const OUTPUT_ROW: i32 = 36;
const OUTPUT_INSET: i32 = 4;
const SINK_ROW: i32 = 30;
const SINK_RULE: i32 = 9;
const ICON: i32 = 16;
const OPEN_TIME: f64 = 0.16;
const EXPAND_TIME: f64 = 0.2;
/// How much a tile shrinks while pressed.
const PRESSED_SCALE: f64 = 0.97;
/// How often a dragged slider tells the system, which takes a process each time.
const SET_EVERY: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Network,
    Dark,
    Brightness,
    Mute,
    Volume,
    Output,
    Sink(usize),
}

/// What is read off the compositor's thread when the panel opens.
struct Found {
    brightness: Option<u32>,
    sinks: Vec<Sink>,
}

/// What the panel shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
struct Shown {
    dark: bool,
    network: Option<Network>,
    volume: Option<Volume>,
    brightness: Option<u32>,
    sinks: Vec<Sink>,
    /// How far the outputs' list is open, in thousandths.
    expansion: u32,
    pressed: Option<Part>,
    hovered: Option<Part>,
}

pub struct ControlCenter {
    /// Its left edge, from the output's.
    x: i32,
    opened: Instant,
    found: Option<Receiver<Found>>,
    brightness: Option<u32>,
    sinks: Vec<Sink>,
    expanded: bool,
    /// When the outputs' list last opened or closed.
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

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

impl ControlCenter {
    pub(crate) fn dragging(&self) -> bool {
        self.dragging.is_some()
    }

    fn new(anchor: Rectangle<i32, Logical>, output_width: i32) -> Self {
        let (send, found) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("nekoland-control".into())
            .spawn(move || {
                let _ = send.send(Found {
                    brightness: system::read_brightness(),
                    sinks: system::read_sinks(),
                });
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
            brightness: None,
            sinks: Vec::new(),
            expanded: false,
            toggled: None,
            pressed: None,
            hovered: None,
            dragging: None,
            shown: None,
            picture: None,
            id: Id::new(),
        }
    }

    /// How far the outputs' list is open, from 0 to 1.
    fn expansion(&self) -> f64 {
        let progress = self.toggled.map_or(1.0, |toggled| {
            ease_out(toggled.elapsed().as_secs_f64() / EXPAND_TIME)
        });
        if self.expanded {
            progress
        } else {
            1.0 - progress
        }
    }

    fn layout(&self, volume: bool) -> Layout {
        let inner = WIDTH - PADDING * 2;
        let mut groups = Vec::new();
        let mut parts = Vec::new();
        let tile = (inner - GAP) / 2;
        parts.push((Part::Network, rect(PADDING, PADDING, tile, TILE_HEIGHT)));
        parts.push((Part::Dark, rect(PADDING + tile + GAP, PADDING, tile, TILE_HEIGHT)));
        let mut y = PADDING + TILE_HEIGHT + GAP;

        // A slider: its icon at the left, then its track; the part is the track.
        let mut slider = |title, icon: Option<Part>, track: Part, y: i32| {
            let group = rect(PADDING, y, inner, GROUP_HEIGHT);
            groups.push((group, Some(title)));
            let row = y + 30;
            if let Some(icon) = icon {
                parts.push((icon, rect(PADDING + 8, row, 24, 24)));
            }
            parts.push((track, rect(PADDING + 42, row, inner - 54, 24)));
        };
        if self.brightness.is_some() {
            slider("顯示器", None, Part::Brightness, y);
            y += GROUP_HEIGHT + GAP;
        }
        if volume {
            slider("聲音", Some(Part::Mute), Part::Volume, y);
            y += GROUP_HEIGHT + GAP;
        }
        if !self.sinks.is_empty() {
            let list = SINK_RULE + SINK_ROW * self.sinks.len() as i32;
            let expansion = self.expansion();
            let height = OUTPUT_ROW + OUTPUT_INSET * 2 + (f64::from(list) * expansion).round() as i32;
            groups.push((rect(PADDING, y, inner, height), None));
            let row = |y| rect(PADDING + OUTPUT_INSET, y, inner - OUTPUT_INSET * 2, OUTPUT_ROW);
            parts.push((Part::Output, row(y + OUTPUT_INSET)));
            if expansion >= 1.0 {
                let top = y + OUTPUT_INSET + OUTPUT_ROW + SINK_RULE;
                for index in 0..self.sinks.len() {
                    let at = top + SINK_ROW * index as i32;
                    parts.push((
                        Part::Sink(index),
                        rect(PADDING + OUTPUT_INSET, at, inner - OUTPUT_INSET * 2, SINK_ROW),
                    ));
                }
            }
            y += height + GAP;
        }
        Layout {
            height: y - GAP + PADDING,
            groups,
            parts,
        }
    }

    fn part_at(&self, local: Point<i32, Logical>, volume: bool) -> Option<Part> {
        let layout = self.layout(volume);
        layout.parts.iter().find_map(|(part, rect)| {
            // A slider takes the pointer a little past its ends, and its whole row.
            let rect = match part {
                Part::Brightness | Part::Volume => {
                    Rectangle::new(rect.loc - Point::from((8, 2)), rect.size + (16, 4).into())
                }
                _ => *rect,
            };
            rect.contains(local).then_some(*part)
        })
    }

    fn draw(&mut self, shown: &Shown) {
        let colors = colors(shown.dark);
        let layout = self.layout(shown.volume.is_some());
        let Ok(surface) = text::canvas(WIDTH, layout.height) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        text::rounded_rectangle(
            &cr,
            0.5,
            0.5,
            f64::from(WIDTH) - 1.0,
            f64::from(layout.height) - 1.0,
            f64::from(MENU_RADIUS),
        );
        let (r, g, b, a) = crate::glass::tint(shown.dark, Tier::Menu);
        cr.set_source_rgba(r, g, b, a);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(colors.edge.0, colors.edge.1, colors.edge.2, colors.edge.3);
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        let well = |cr: &Context, area: Rectangle<i32, Logical>| {
            let (x, y, w, h) = to_f64(area);
            text::rounded_rectangle(cr, x, y, w, h, 8.0);
            cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.07);
            let _ = cr.fill();
        };
        let small = font("", 9.0);
        for (group, title) in &layout.groups {
            well(&cr, *group);
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
        for (part, area) in &layout.parts {
            let (x, y, w, h) = to_f64(*area);
            let center = y + h / 2.0;
            match part {
                Part::Network | Part::Dark => {
                    let (on, icon, title, detail) = match part {
                        Part::Network => {
                            let network = shown.network.as_ref();
                            let title = match network {
                                Some(network) if network.wireless => "Wi-Fi",
                                Some(_) => "乙太網路",
                                None => "網路",
                            };
                            let connected = network.is_some_and(|network| network.connected);
                            let icon = network.map_or("network-offline-symbolic", network_icon);
                            (
                                connected,
                                icon,
                                title,
                                Some(if connected { "已連線" } else { "未連線" }),
                            )
                        }
                        _ => (shown.dark, "weather-clear-night-symbolic", "深色", None),
                    };
                    cr.save().ok();
                    if shown.pressed == Some(*part) {
                        cr.translate(x + w / 2.0, center);
                        cr.scale(PRESSED_SCALE, PRESSED_SCALE);
                        cr.translate(-(x + w / 2.0), -center);
                    }
                    if on {
                        text::rounded_rectangle(&cr, x, y, w, h, 8.0);
                        cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                        let _ = cr.fill();
                    } else {
                        well(&cr, *area);
                    }
                    let color = if on { colors.on_ink } else { colors.ink };
                    draw_icon(&cr, icon, x + 12.0, center, color);
                    match detail {
                        Some(detail) => {
                            show_text(&cr, title, &label, x + 38.0, center - 8.0, color);
                            cr.push_group();
                            show_text(&cr, detail, &small, x + 38.0, center + 9.0, color);
                            let _ = cr.pop_group_to_source();
                            let _ = cr.paint_with_alpha(0.7);
                        }
                        None => show_text(&cr, title, &label, x + 38.0, center, color),
                    }
                    cr.restore().ok();
                }
                Part::Mute => {
                    let icon = shown.volume.map_or("audio-volume-high-symbolic", sound_icon);
                    draw_icon(&cr, icon, x + 4.0, center, colors.ink);
                }
                Part::Brightness | Part::Volume => {
                    if *part == Part::Brightness {
                        draw_icon(&cr, "display-brightness-symbolic", x - 30.0, center, colors.ink);
                    }
                    let (level, muted) = match part {
                        Part::Brightness => (shown.brightness.unwrap_or(0), false),
                        _ => shown
                            .volume
                            .map_or((0, false), |volume| (volume.percent, volume.muted)),
                    };
                    draw_slider(
                        &cr,
                        &colors,
                        shown.dark,
                        (x, center, w),
                        if muted {
                            0.0
                        } else {
                            f64::from(level.min(100)) / 100.0
                        },
                    );
                }
                Part::Output => {
                    let current = shown
                        .sinks
                        .iter()
                        .find(|sink| sink.default)
                        .or(shown.sinks.first());
                    draw_speaker(&cr, x + 8.0, center, colors.ink);
                    if let Some(sink) = current {
                        show_text(&cr, &sink.description, &label, x + 34.0, center, colors.ink);
                    }
                    draw_chevron(
                        &cr,
                        x + w - 16.0,
                        center,
                        f64::from(shown.expansion) / 1000.0,
                        colors.ink_3,
                    );
                }
                Part::Sink(index) => {
                    let sink = &shown.sinks[*index];
                    let lit = shown.hovered == Some(*part);
                    if lit {
                        text::rounded_rectangle(&cr, x, y, w, h, 6.0);
                        cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                        let _ = cr.fill();
                    }
                    let color = if lit { colors.on_ink } else { colors.ink };
                    if sink.default {
                        show_text(&cr, "✓", &label, x + 10.0, center, color);
                    }
                    show_text(&cr, &sink.description, &label, x + 34.0, center, color);
                }
            }
        }
        // The rule between the output and the list, under the list as it opens.
        if let Some((_, output)) = layout.parts.iter().find(|(part, _)| *part == Part::Output) {
            if shown.expansion > 0 {
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.12);
                let y = output.loc.y + output.size.h + SINK_RULE / 2;
                cr.rectangle(
                    f64::from(output.loc.x + 4),
                    f64::from(y),
                    f64::from(output.size.w - 8),
                    1.0,
                );
                let _ = cr.fill();
            }
        }
        drop(cr);
        self.picture = text::picture(surface);
    }
}

fn to_f64(area: Rectangle<i32, Logical>) -> (f64, f64, f64, f64) {
    (
        f64::from(area.loc.x),
        f64::from(area.loc.y),
        f64::from(area.size.w),
        f64::from(area.size.h),
    )
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

/// A track, filled in ink as far as `level`, with its knob there.
fn draw_slider(cr: &Context, colors: &Colors, dark: bool, (x, center, width): (f64, f64, f64), level: f64) {
    let at = x + width * level;
    text::rounded_rectangle(cr, x, center - 2.0, width, 4.0, 2.0);
    // Light ink on dark glass shows less, so the empty track takes more of it there.
    let empty = if dark { 0.25 } else { 0.16 };
    cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, empty);
    let _ = cr.fill();
    text::rounded_rectangle(cr, x, center - 2.0, at - x, 4.0, 2.0);
    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
    let _ = cr.fill();
    cr.arc(at, center, 7.25, 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(colors.on_ink.0, colors.on_ink.1, colors.on_ink.2);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

/// A chevron pointing right, turned down as far as `turn`, from 0 to 1.
fn draw_chevron(cr: &Context, x: f64, center: f64, turn: f64, color: (f64, f64, f64)) {
    cr.save().ok();
    cr.translate(x, center);
    cr.rotate(turn * std::f64::consts::FRAC_PI_2);
    cr.move_to(-2.0, -4.5);
    cr.line_to(2.5, 0.0);
    cr.line_to(-2.0, 4.5);
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(1.5);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    let _ = cr.stroke();
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

impl Nekoland {
    /// Opens the control center under its icon in the bar, or closes it.
    pub(crate) fn toggle_control_center(&mut self, anchor: Rectangle<i32, Logical>) {
        self.menu_bar.open = None;
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
        let shown_volume = self.menu_bar.sound;
        let dark = self.menu_bar.dark;
        let network = self.menu_bar.network.clone();
        let Some(control) = self.menu_bar.control.as_mut() else {
            return;
        };
        if let Some(found) = control.found.as_ref().and_then(|found| found.try_recv().ok()) {
            control.brightness = found.brightness;
            control.sinks = found.sinks;
            control.found = None;
        }
        let shown = Shown {
            dark,
            network,
            volume: shown_volume,
            brightness: control.brightness,
            sinks: control.sinks.clone(),
            expansion: (control.expansion() * 1000.0).round() as u32,
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
        let volume = self.menu_bar.sound.is_some();
        let Some(control) = self.menu_bar.control.as_mut() else {
            return false;
        };
        let part = control.part_at(at, volume);
        match part {
            Some(slider @ (Part::Brightness | Part::Volume)) => {
                control.dragging = Some((slider, Instant::now() - SET_EVERY));
                self.control_drag(at, false);
            }
            // The wired network has nothing to turn off.
            Some(Part::Network)
                if !self
                    .menu_bar
                    .network
                    .as_ref()
                    .is_some_and(|network| network.wireless) => {}
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
        let volume = self.menu_bar.sound.is_some();
        let inside = self.control_local(local).is_some();
        if let Some(control) = self.menu_bar.control.as_mut() {
            control.hovered = inside.then(|| control.part_at(at, volume)).flatten();
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
        let volume = self.menu_bar.sound.is_some();
        let under = self
            .control_local(local)
            .and_then(|at| control.part_at(at, volume));
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
        let layout = control.layout(volume.is_some());
        let Some((_, track)) = layout.parts.iter().find(|(part, _)| *part == slider) else {
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
        match part {
            Part::Network => {
                let connected = self
                    .menu_bar
                    .network
                    .as_ref()
                    .is_some_and(|network| network.connected);
                config::spawn(&format!(
                    "nmcli radio wifi {}",
                    if connected { "off" } else { "on" }
                ));
            }
            Part::Dark => {
                let dark = !self.menu_bar.dark;
                let scheme = if dark { "prefer-dark" } else { "default" };
                config::spawn(&format!(
                    "gsettings set org.gnome.desktop.interface color-scheme {scheme}"
                ));
                // The portal says so too, a moment later.
                self.menu_bar.dark = dark;
            }
            Part::Mute => {
                config::spawn(&self.config.volume_mute);
                if let Some(volume) = self.menu_bar.sound.as_mut() {
                    volume.muted = !volume.muted;
                    self.menu_bar.items_revision += 1;
                }
            }
            Part::Output => {
                if let Some(control) = self.menu_bar.control.as_mut() {
                    control.expanded = !control.expanded;
                    control.toggled = Some(Instant::now());
                }
            }
            Part::Sink(index) => {
                if let Some(control) = self.menu_bar.control.as_mut() {
                    if let Some(id) = control.sinks.get(index).map(|sink| sink.id) {
                        config::spawn(&format!("wpctl set-default {id}"));
                        for (other, sink) in control.sinks.iter_mut().enumerate() {
                            sink.default = other == index;
                        }
                    }
                    control.expanded = false;
                    control.toggled = Some(Instant::now());
                    control.hovered = None;
                }
            }
            Part::Brightness | Part::Volume => {}
        }
    }
}
