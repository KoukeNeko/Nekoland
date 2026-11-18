//! What the lock looks like, in one of three styles: the wallpaper under ink; the time, the
//! date and the widgets; the notices that came in while locked; the user's name and a field
//! for the password. Colours are Nekoland's dark paper and ink, one taken from the wallpaper,
//! and its one red for a wrong password.

use std::f64::consts::PI;

use cairo::{Context, Format, ImageSurface, LinearGradient, RadialGradient, SurfacePattern};
use gdk_pixbuf::{InterpType, Pixbuf};

const PAPER: (f64, f64, f64) = (0x1b as f64 / 255.0, 0x1b as f64 / 255.0, 0x1e as f64 / 255.0);
const INK: (f64, f64, f64) = (0xf1 as f64 / 255.0, 0xef as f64 / 255.0, 0xea as f64 / 255.0);
const DANGER: (f64, f64, f64) = (1.0, 0x6b as f64 / 255.0, 0x5f as f64 / 255.0);
/// How much nearer the wallpaper comes while locked.
const NEARER: f64 = 1.06;
/// How far the bottom edge of the ink fades as it lifts.
pub const FEATHER: f64 = 160.0;
const FIELD_WIDTH: f64 = 248.0;
/// The accent where the wallpaper gives none: a warm sand.
pub const SAND: Rgb = (0.95, 0.70, 0.54);
const FIELD_HEIGHT: f64 = 36.0;

/// Which of the three layouts the lock has.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    /// The time in the middle, thin, with a row of widgets under it.
    Quiet,
    /// The hours over the minutes, bold, on the left, over the wallpaper as it is.
    Poster,
    /// A clock with hands, its widgets as small dials on its face.
    Dial,
}

/// Where the time stands across the screen, in the styles that centre it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// How the lock goes once the password is given.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Effect {
    /// Taken off upwards, its foot fading.
    Lift,
    Fade,
    /// Pushed off to the left.
    Slide,
    /// Coming nearer as it fades.
    Expand,
}

/// How much of the notifications that came in while locked the lock shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Notices {
    Hidden,
    Count,
    Full,
}

/// What the configuration says the lock shows.
pub struct Look {
    pub wallpaper: Option<Pixbuf>,
    pub blur: bool,
    pub font: String,
    pub clock_24h: bool,
    pub name: String,
    pub style: Style,
    /// The clock's own colour, where one was chosen; else the style's.
    pub clock_colour: Option<Rgb>,
    /// A colour of the wallpaper's, light enough to read on the ink.
    pub accent: Rgb,
    pub clock_weight: pango::Weight,
    /// The family the time is set in; else `font`'s.
    pub clock_font: String,
    /// How much larger than the style's own the time is.
    pub clock_scale: f64,
    pub clock_align: Align,
    pub clock_seconds: bool,
    pub show_date: bool,
    pub battery: bool,
    pub playing: bool,
    /// Today's next event from the Calendar.
    pub calendar: bool,
    /// The time zone of a second clock, as `Asia/Tokyo`, and the city it is shown as.
    pub world_clock: Option<String>,
    pub world_clock_name: String,
    /// Where to ask the weather of, and the city it is shown as.
    pub weather: Option<(f64, f64)>,
    pub weather_name: String,
    /// Whether the screen, left a while, shows the time dimly on black rather than going
    /// dark, as an always-on display does; and after how long.
    pub always_on: bool,
    /// Whether a finger may unlock, where fprintd has one enrolled.
    pub fingerprint: bool,
    pub doze_after: std::time::Duration,
    /// How much darker than the style's own ink the wallpaper is, from 0 to 1.
    pub dim: f64,
    pub lock_icon: bool,
    pub unlock_effect: Effect,
    pub unlock_duration: std::time::Duration,
    /// A picture placed on the lock: its centre as shares of the width and the height, and
    /// its longer side in points.
    pub sticker: Option<(ImageSurface, (f64, f64), f64)>,
    /// The wallpaper's subject cut out, framed as the wallpaper is, laid over the time as
    /// One UI's depth effect lays it.
    pub subject: Option<ImageSurface>,
    /// Whether the shortcuts stand in a row over the power buttons rather than in the corners.
    pub shortcuts_row: bool,
    pub notices: Notices,
    /// What the person asked to be shown while locked, such as whom to call.
    pub message: String,
    pub show_name: bool,
    pub picture: Option<Pixbuf>,
    /// The apps at the foot, left then right or in a row, with the icons the compositor drew.
    pub shortcuts: Vec<(String, Option<ImageSurface>)>,
    /// Whether they open over the lock at once, rather than once the password is given.
    pub shortcuts_unlocked: bool,
    pub power_buttons: bool,
}

/// What can be pressed on the lock.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Button {
    Shortcut(usize),
    Sleep,
    Restart,
    PowerOff,
    /// The player's.
    Previous,
    PlayPause,
    Next,
}

/// A button's centre and radius, in points.
pub type Hit = (Button, f64, f64, f64);

/// As many shortcuts as a row has room for, and the corners.
pub const ROW_SHORTCUTS: usize = 6;
const CORNER_SHORTCUTS: usize = 2;

/// The power buttons, with what is written under each.
const POWER: [(Button, &str); 3] = [(Button::Sleep, "睡眠"), (Button::Restart, "重新開機"), (Button::PowerOff, "關機")];
const SHORTCUT_RADIUS: f64 = 26.0;
const POWER_RADIUS: f64 = 18.0;
/// The room at the screen's edges, and between the field, its note under it, and the
/// buttons at the foot.
const EDGE: f64 = 32.0;
const GAP: f64 = 32.0;
const NOTE_ROOM: f64 = 28.0;
/// A power button's label, under it.
const LABEL_GAP: f64 = 6.0;
const LABEL_HEIGHT: f64 = 18.0;

/// The lowest the password field's top may be: the field and its note clear of the buttons
/// at the foot by the same gap in every style.
fn lowest_field(look: &Look, width: f64, height: f64) -> f64 {
    let foot = buttons(look, width, height)
        .into_iter()
        .map(|(_, _, y, r)| y - r)
        .fold(height - EDGE, f64::min);
    foot - GAP - NOTE_ROOM - FIELD_HEIGHT
}

/// Where each button is on a lock `width` by `height` points: its centre and its radius.
/// The shortcuts stand in the corners at the foot, as One UI has them; the power buttons
/// in a row between, as a login window has them.
pub fn buttons(look: &Look, width: f64, height: f64) -> Vec<Hit> {
    let mut found = Vec::new();
    let power_y = height - EDGE - LABEL_HEIGHT - LABEL_GAP - POWER_RADIUS;
    if look.power_buttons {
        const GAP: f64 = 76.0;
        for (i, (button, _)) in POWER.iter().enumerate() {
            found.push((*button, width / 2.0 + (i as f64 - 1.0) * GAP, power_y, POWER_RADIUS));
        }
    }
    let shown = |limit: usize| look.shortcuts.iter().enumerate().take(limit).filter(|(_, (app, _))| !app.is_empty());
    if look.shortcuts_row {
        const STEP: f64 = 2.0 * SHORTCUT_RADIUS + 20.0;
        let above = if look.power_buttons { power_y - POWER_RADIUS - 20.0 } else { height - EDGE };
        let y = above - SHORTCUT_RADIUS;
        let count = shown(ROW_SHORTCUTS).count() as f64;
        for (i, (at, _)) in shown(ROW_SHORTCUTS).enumerate() {
            found.push((Button::Shortcut(at), width / 2.0 + (i as f64 - (count - 1.0) / 2.0) * STEP, y, SHORTCUT_RADIUS));
        }
    } else {
        let foot = height - EDGE - SHORTCUT_RADIUS;
        // A slot left empty keeps its side: the second shortcut is always on the right.
        for (at, _) in shown(CORNER_SHORTCUTS) {
            let x = if at == 0 { EDGE + SHORTCUT_RADIUS } else { width - EDGE - SHORTCUT_RADIUS };
            found.push((Button::Shortcut(at), x, foot, SHORTCUT_RADIUS));
        }
    }
    found
}

/// The button at a point, if any.
pub fn button_at(look: &Look, width: f64, height: f64, x: f64, y: f64) -> Option<Button> {
    buttons(look, width, height)
        .into_iter()
        .find(|(_, cx, cy, r)| (x - cx).hypot(y - cy) <= *r)
        .map(|(button, ..)| button)
}

/// The shortcuts and the power buttons, over whichever style.
fn draw_buttons(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    for (button, cx, cy, r) in buttons(look, width, height) {
        cr.new_path();
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        let chosen = matches!(button, Button::Shortcut(at) if moment.chosen == Some(at));
        set(cr, if chosen { look.accent } else { INK }, if chosen { 0.34 } else { 0.13 });
        let _ = cr.fill_preserve();
        set(cr, if chosen { look.accent } else { (1.0, 1.0, 1.0) }, if chosen { 1.0 } else { 0.22 });
        cr.set_line_width(if chosen { 2.0 } else { 1.0 });
        let _ = cr.stroke();
        match button {
            Button::Shortcut(at) => {
                let (app, icon) = &look.shortcuts[at];
                let drawn = icon.as_ref().and_then(|icon| {
                    let side = 30.0;
                    let scale = side / f64::from(icon.width().max(icon.height()).max(1));
                    cr.save().ok()?;
                    cr.translate(cx - side / 2.0, cy - side / 2.0);
                    cr.scale(scale, scale);
                    cr.set_source_surface(icon, 0.0, 0.0).ok()?;
                    cr.paint().ok()?;
                    cr.restore().ok()
                });
                if drawn.is_none() {
                    // No icon to be had: the app's initial.
                    let initial: String = app.rsplit('.').next().and_then(|name| name.chars().next()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    set(cr, INK, 0.9);
                    let layout = text(cr, look, &initial, 18.0, MEDIUM);
                    let h = f64::from(layout.pixel_extents().1.height());
                    centred(cr, &layout, cx, cy - h / 2.0);
                }
            }
            power => {
                set(cr, INK, 0.9);
                power_icon(cr, power, cx, cy);
                let label = POWER.iter().find(|(b, _)| *b == power).map_or("", |(_, label)| label);
                set(cr, INK, 0.74);
                centred(cr, &text(cr, look, label, 12.0, MEDIUM), cx, cy + r + LABEL_GAP);
            }
        }
    }
}

/// A power button's sign, 16 points across, drawn in the source colour.
fn power_icon(cr: &Context, button: Button, cx: f64, cy: f64) {
    cr.new_path();
    cr.set_line_width(1.6);
    cr.set_line_cap(cairo::LineCap::Round);
    match button {
        Button::Sleep => {
            // A crescent: a disc with a smaller one taken from its upper right, kept within
            // the disc.
            cr.save().ok();
            cr.arc(cx, cy, 7.0, 0.0, 2.0 * PI);
            cr.clip();
            cr.arc(cx, cy, 7.0, 0.0, 2.0 * PI);
            cr.new_sub_path();
            cr.arc(cx + 3.6, cy - 3.0, 5.6, 0.0, 2.0 * PI);
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            let _ = cr.fill();
            cr.restore().ok();
        }
        Button::Restart => {
            cr.arc(cx, cy, 6.5, -PI * 0.35, PI * 1.45);
            let _ = cr.stroke();
            // The arrowhead at the arc's open end.
            let (ex, ey) = (cx + 6.5 * (PI * 1.45).cos(), cy + 6.5 * (PI * 1.45).sin());
            cr.move_to(ex - 3.4, ey - 1.2);
            cr.line_to(ex + 0.4, ey - 0.2);
            cr.line_to(ex - 0.6, ey + 3.4);
            let _ = cr.stroke();
        }
        _ => {
            cr.arc(cx, cy + 0.6, 6.5, -PI * 0.3, PI * 1.3);
            let _ = cr.stroke();
            cr.move_to(cx, cy - 7.5);
            cr.line_to(cx, cy - 0.5);
            let _ = cr.stroke();
        }
    }
}

pub type Rgb = (f64, f64, f64);

/// What the widgets show.
#[derive(Default, PartialEq, Clone)]
pub struct Facts {
    /// The charge, and whether it is charging.
    pub battery: Option<(u8, bool)>,
    pub playing: Option<Playing>,
    /// Today's next event, as `10:00 Title`.
    pub event: Option<String>,
    /// The second clock's city and its time.
    pub world: Option<(String, String)>,
    /// The weather's city, and how it is there, as `18° 多雲`.
    pub weather: Option<(String, String)>,
    /// The program and the summary of each notification that came in while locked.
    pub notices: Vec<(String, String)>,
}

/// What a player plays, and which player it is, for its buttons.
#[derive(Default, PartialEq, Clone, Debug)]
pub struct Playing {
    pub title: String,
    pub artist: String,
    pub paused: bool,
    /// Its name on the session bus.
    pub bus: String,
}

impl Playing {
    fn words(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} · {}", self.title, self.artist)
        }
    }
}

/// What is on the lock at a moment.
pub struct Moment<'a> {
    pub time: &'a str,
    pub date: &'a str,
    /// The hour, 0 to 23, and the minute, for the hands.
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub facts: &'a Facts,
    /// How many characters have been typed.
    pub typed: usize,
    pub checking: bool,
    pub caps_lock: bool,
    /// What went wrong with the last password, and how far into the shake it is, from 0 to 1.
    pub error: Option<(&'a str, f64)>,
    /// The shortcut to open once the password is given.
    pub chosen: Option<usize>,
    /// Whether the fingerprint reader waits for a finger.
    pub finger: bool,
}

/// The wallpaper covering `width` by `height` pixels, `nearer` times larger, blurred or not;
/// the paper alone without one.
pub fn backdrop(look: &Look, width: i32, height: i32, nearer: bool, blur: bool) -> Option<ImageSurface> {
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let cr = Context::new(&surface).ok()?;
    cr.set_source_rgb(PAPER.0, PAPER.1, PAPER.2);
    cr.paint().ok()?;
    let Some(wallpaper) = &look.wallpaper else {
        return Some(surface);
    };
    let grow = if nearer { NEARER } else { 1.0 };
    let (w, h) = (f64::from(wallpaper.width()), f64::from(wallpaper.height()));
    let cover = (f64::from(width) / w).max(f64::from(height) / h) * grow;
    let (cw, ch) = ((w * cover).round() as i32, (h * cover).round() as i32);
    let scaled = if blur {
        // Down to a twentieth averages the picture; up again in two steps keeps it smooth.
        let small = wallpaper.scale_simple((cw / 20).max(1), (ch / 20).max(1), InterpType::Hyper)?;
        let middle = small.scale_simple((cw / 4).max(1), (ch / 4).max(1), InterpType::Bilinear)?;
        middle.scale_simple(cw, ch, InterpType::Bilinear)?
    } else {
        wallpaper.scale_simple(cw, ch, InterpType::Hyper)?
    };
    let picture = pixbuf_surface(&scaled)?;
    cr.set_source_surface(
        &picture,
        f64::from(width - cw) / 2.0,
        f64::from(height - ch) / 2.0,
    )
    .ok()?;
    cr.paint().ok()?;
    surface.flush();
    Some(surface)
}

/// Ink over the backdrop, as the style has it. In points, on a surface already scaled.
pub fn veil(cr: &Context, look: &Look, width: f64, height: f64) {
    if look.dim > 0.0 {
        cr.set_source_rgba(PAPER.0, PAPER.1, PAPER.2, look.dim);
        let _ = cr.paint();
    }
    match look.style {
        // A little everywhere, more behind the time and the field.
        Style::Quiet => {
            cr.set_source_rgba(PAPER.0, PAPER.1, PAPER.2, 0.24);
            let _ = cr.paint();
            for (y, alpha) in [(0.32, 0.34), (0.74, 0.4)] {
                let (rx, ry) = (width * 0.34, height * 0.26);
                cr.save().ok();
                cr.translate(width / 2.0, height * y);
                cr.scale(rx, ry);
                let glow = RadialGradient::new(0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
                glow.add_color_stop_rgba(0.0, PAPER.0, PAPER.1, PAPER.2, alpha);
                glow.add_color_stop_rgba(0.7, PAPER.0, PAPER.1, PAPER.2, 0.0);
                cr.set_source(&glow).ok();
                cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
                let _ = cr.fill();
                cr.restore().ok();
            }
        }
        // Only on the left, where the words are, and a little on the right for the notices.
        Style::Poster => {
            let side = LinearGradient::new(0.0, 0.0, width, 0.0);
            side.add_color_stop_rgba(0.0, 0.08, 0.07, 0.08, 0.72);
            side.add_color_stop_rgba(0.34, 0.08, 0.07, 0.08, 0.46);
            side.add_color_stop_rgba(0.62, 0.08, 0.07, 0.08, 0.0);
            side.add_color_stop_rgba(1.0, 0.08, 0.07, 0.08, 0.18);
            cr.set_source(&side).ok();
            let _ = cr.paint();
        }
        // The wallpaper recedes, so the dial carries the screen.
        Style::Dial => {
            cr.set_source_rgba(0.08, 0.075, 0.086, 0.58);
            let _ = cr.paint();
        }
    }
}

fn text(cr: &Context, look: &Look, words: &str, size: f64, weight: pango::Weight) -> pango::Layout {
    text_in(cr, &look.font, words, size, weight)
}

/// The time, in the family chosen for it.
fn clock_text(cr: &Context, look: &Look, words: &str, size: f64) -> pango::Layout {
    let family = if look.clock_font.is_empty() { &look.font } else { &look.clock_font };
    text_in(cr, family, words, size, look.clock_weight)
}

fn text_in(cr: &Context, family: &str, words: &str, size: f64, weight: pango::Weight) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    // As Nekoland draws text: unhinted, with grey edges.
    if let Ok(mut options) = cairo::FontOptions::new() {
        options.set_antialias(cairo::Antialias::Gray);
        options.set_hint_style(cairo::HintStyle::None);
        options.set_hint_metrics(cairo::HintMetrics::Off);
        pangocairo::functions::context_set_font_options(&layout.context(), Some(&options));
        layout.context_changed();
    }
    let mut font = pango::FontDescription::from_string(family);
    font.set_weight(weight);
    font.set_absolute_size(size * f64::from(pango::SCALE));
    layout.set_font_description(Some(&font));
    layout.set_text(words);
    layout
}

/// Shows `layout` centred on `x`, its top at `y`; its height.
fn centred(cr: &Context, layout: &pango::Layout, x: f64, y: f64) -> f64 {
    let (_, logical) = layout.pixel_extents();
    cr.move_to(x - f64::from(logical.width()) / 2.0, y);
    pangocairo::functions::show_layout(cr, layout);
    f64::from(logical.height())
}

fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// How far the field is off centre `t` into a shake: three swings, each smaller.
fn shake(t: f64) -> f64 {
    if t >= 1.0 {
        return 0.0;
    }
    -9.0 * (1.0 - t) * (t * 3.0 * 2.0 * PI).sin()
}

const MEDIUM: pango::Weight = pango::Weight::Medium;

fn set(cr: &Context, colour: Rgb, alpha: f64) {
    cr.set_source_rgba(colour.0, colour.1, colour.2, alpha);
}

/// Shows `layout` with its left at `x` and its top at `y`; its width and height.
fn at(cr: &Context, layout: &pango::Layout, x: f64, y: f64) -> (f64, f64) {
    let (_, logical) = layout.pixel_extents();
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, layout);
    (f64::from(logical.width()), f64::from(logical.height()))
}

/// A line no wider than `width`, ending in an ellipsis when it is.
fn line(cr: &Context, look: &Look, words: &str, size: f64, weight: pango::Weight, width: f64) -> pango::Layout {
    let layout = text(cr, look, words, size, weight);
    layout.set_width((width * f64::from(pango::SCALE)) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout
}

/// A pane of glass, as the field and the widgets are.
fn glass(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64, alpha: f64) {
    rounded(cr, x, y, w, h, r);
    set(cr, INK, alpha);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

/// The person's picture in a circle `size` across, or their initial on the accent.
fn picture(cr: &Context, look: &Look, x: f64, y: f64, size: f64) {
    cr.save().ok();
    cr.arc(x + size / 2.0, y + size / 2.0, size / 2.0, 0.0, 2.0 * PI);
    cr.clip();
    let shown = look.picture.as_ref().and_then(|picture| {
        let (w, h) = (f64::from(picture.width()), f64::from(picture.height()));
        let cover = (size / w).max(size / h);
        // At twice the size, for screens with two pixels to a point; centred on the circle.
        let scaled = picture.scale_simple((w * cover * 2.0).ceil() as i32, (h * cover * 2.0).ceil() as i32, InterpType::Hyper)?;
        let surface = pixbuf_surface(&scaled)?;
        let over = |side: i32| (f64::from(side) / 2.0 - size) / 2.0;
        cr.translate(x - over(scaled.width()), y - over(scaled.height()));
        cr.scale(0.5, 0.5);
        cr.set_source_surface(&surface, 0.0, 0.0).ok()?;
        cr.paint().ok()
    });
    if shown.is_none() {
        set(cr, look.accent, 1.0);
        let _ = cr.paint();
        set(cr, PAPER, 1.0);
        let initial: String = look.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let layout = text(cr, look, &initial, size * 0.4, pango::Weight::Bold);
        let (_, extent) = layout.pixel_extents();
        centred(cr, &layout, x + size / 2.0, y + (size - f64::from(extent.height())) / 2.0);
    }
    cr.restore().ok();
}

/// The field for the password, its left at `x` and its top at `y`, with what went wrong or
/// the caps lock under it, from `note_x` or centred on the field.
fn field(cr: &Context, look: &Look, moment: &Moment, x: f64, y: f64, width: f64, note_left: bool) {
    let x = x + moment.error.map_or(0.0, |(_, t)| shake(t));
    rounded(cr, x, y, width, FIELD_HEIGHT, FIELD_HEIGHT / 2.0);
    set(cr, INK, 0.18);
    let _ = cr.fill_preserve();
    match moment.error {
        Some(_) => {
            set(cr, DANGER, 1.0);
            cr.set_line_width(1.5);
        }
        None => {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.4);
            cr.set_line_width(1.0);
        }
    }
    let _ = cr.stroke();

    let middle = y + FIELD_HEIGHT / 2.0;
    if moment.typed == 0 && !moment.checking {
        set(cr, INK, 0.5);
        let hint = text(cr, look, "密碼", 14.0, MEDIUM);
        let (_, extent) = hint.pixel_extents();
        at(cr, &hint, x + 16.0, middle - f64::from(extent.height()) / 2.0);
    } else {
        // The last dots that fit, as a field scrolls to what is being typed.
        let room = ((width - 36.0 - if moment.finger { FINGER_ROOM } else { 0.0 }) / 12.0) as usize;
        set(cr, INK, if moment.checking { 0.5 } else { 1.0 });
        for i in 0..moment.typed.min(room) {
            cr.arc(x + 20.0 + 12.0 * i as f64, middle, 3.5, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
    }

    if moment.finger {
        // A finger on the reader unlocks too.
        set(cr, if moment.error.is_some() { DANGER } else { INK }, 0.8);
        fingerprint(cr, x + width - FINGER_ROOM / 2.0 - 6.0, middle, 9.0);
    }

    let note = match moment.error {
        Some((why, _)) => Some((why, DANGER, 1.0)),
        None if moment.caps_lock => Some(("大寫鎖定", INK, 0.72)),
        None => None,
    };
    if let Some((words, colour, alpha)) = note {
        set(cr, colour, alpha);
        let layout = text(cr, look, words, 13.0, MEDIUM);
        if note_left {
            at(cr, &layout, x + 16.0, y + FIELD_HEIGHT + 10.0);
        } else {
            centred(cr, &layout, x + width / 2.0, y + FIELD_HEIGHT + 10.0);
        }
    }
}

/// How much of the field's right end the fingerprint takes.
const FINGER_ROOM: f64 = 24.0;

/// A fingerprint `radius` points across from its middle, drawn in the source colour: ridges
/// as nested arcs open at the foot, broken at different places as a finger's are.
fn fingerprint(cr: &Context, cx: f64, cy: f64, radius: f64) {
    cr.new_path();
    cr.set_line_width((radius / 7.0).max(1.2));
    cr.set_line_cap(cairo::LineCap::Round);
    for ridge in 0..4 {
        let r = radius * (0.22 + 0.26 * f64::from(ridge));
        let open = 0.55 - 0.08 * f64::from(ridge);
        let start = PI / 2.0 + open;
        let end = PI / 2.0 + 2.0 * PI - open;
        // Each ridge is broken once, further round for the outer ones.
        let gap = start + (end - start) * (0.3 + 0.17 * f64::from(ridge));
        cr.new_sub_path();
        cr.arc(cx, cy, r, start, gap - 0.18);
        cr.new_sub_path();
        cr.arc(cx, cy, r, gap + 0.18, end);
    }
    let _ = cr.stroke();
}

/// The battery as a ring `size` across, filled as far as it is charged.
fn ring(cr: &Context, cx: f64, cy: f64, radius: f64, line: f64, filled: f64, colour: Rgb) {
    cr.set_line_width(line);
    cr.set_line_cap(cairo::LineCap::Round);
    set(cr, INK, 0.22);
    cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
    let _ = cr.stroke();
    set(cr, colour, 1.0);
    let start = -PI / 2.0;
    cr.arc(cx, cy, radius, start, start + 2.0 * PI * filled.clamp(0.0, 1.0));
    let _ = cr.stroke();
}

fn battery_words((percent, charging): (u8, bool)) -> String {
    if charging {
        format!("{percent}% 充電中")
    } else {
        format!("{percent}%")
    }
}

/// One of the widgets the person asked for, with something to show in it.
enum Widget<'a> {
    Battery(u8, bool),
    Playing(&'a Playing),
    Event(&'a str),
    World(&'a str, &'a str),
    Weather(&'a str, &'a str),
}

fn widgets<'a>(look: &Look, facts: &'a Facts) -> Vec<Widget<'a>> {
    let mut found = Vec::new();
    if let Some((percent, charging)) = facts.battery.filter(|_| look.battery) {
        found.push(Widget::Battery(percent, charging));
    }
    if let Some(playing) = facts.playing.as_ref().filter(|_| look.playing) {
        found.push(Widget::Playing(playing));
    }
    if let Some(event) = facts.event.as_deref().filter(|_| look.calendar) {
        found.push(Widget::Event(event));
    }
    if let Some((city, time)) = facts.world.as_ref().filter(|_| look.world_clock.is_some()) {
        found.push(Widget::World(city, time));
    }
    if let Some((city, now)) = facts.weather.as_ref().filter(|_| look.weather.is_some()) {
        found.push(Widget::Weather(city, now));
    }
    found
}

fn widget_words(widget: &Widget) -> String {
    match widget {
        Widget::Battery(percent, charging) => battery_words((*percent, *charging)),
        Widget::Playing(playing) => playing.words(),
        Widget::Event(event) => (*event).to_string(),
        Widget::World(city, time) | Widget::Weather(city, time) => format!("{city} {time}"),
    }
}

/// A widget's sign, about 16 points across, centred on a point, in the source colour.
fn glyph(cr: &Context, widget: &Widget, cx: f64, cy: f64) {
    cr.new_path();
    cr.set_line_width(1.6);
    match widget {
        Widget::Battery(percent, _) => ring(cr, cx, cy, 8.0, 3.0, f64::from(*percent) / 100.0, INK),
        // A note.
        Widget::Playing(_) => {
            cr.arc(cx - 3.0, cy + 5.0, 3.5, 0.0, 2.0 * PI);
            let _ = cr.fill();
            cr.rectangle(cx - 0.5, cy - 8.0, 1.6, 13.0);
            let _ = cr.fill();
        }
        // A calendar's page.
        Widget::Event(_) => {
            rounded(cr, cx - 7.0, cy - 6.5, 14.0, 13.0, 3.0);
            let _ = cr.stroke();
            cr.rectangle(cx - 7.0, cy - 6.5, 14.0, 4.0);
            let _ = cr.fill();
        }
        // A cloud, with rain or snow under it as there is.
        Widget::Weather(_, now) if !now.contains('晴') && !now.ends_with('°') => {
            let top = if now.contains('雨') || now.contains('雪') { cy - 3.5 } else { cy };
            cr.arc(cx - 3.0, top + 1.0, 3.2, PI * 0.5, PI * 1.5);
            cr.arc(cx + 0.5, top - 1.5, 4.0, PI, PI * 1.9);
            cr.arc(cx + 4.0, top + 1.0, 3.2, PI * 1.5, PI * 0.5);
            cr.close_path();
            let _ = cr.stroke();
            for x in [cx - 2.5, cx + 2.5] {
                if now.contains('雨') {
                    cr.move_to(x + 1.0, top + 6.5);
                    cr.line_to(x - 0.5, top + 10.0);
                    let _ = cr.stroke();
                } else if now.contains('雪') {
                    cr.arc(x, top + 8.0, 1.3, 0.0, 2.0 * PI);
                    let _ = cr.fill();
                }
            }
        }
        // A sun.
        Widget::Weather(..) => {
            cr.arc(cx, cy, 3.6, 0.0, 2.0 * PI);
            let _ = cr.fill();
            for ray in 0..8 {
                let angle = f64::from(ray) * PI / 4.0;
                cr.move_to(cx + 5.8 * angle.cos(), cy + 5.8 * angle.sin());
                cr.line_to(cx + 8.0 * angle.cos(), cy + 8.0 * angle.sin());
            }
            let _ = cr.stroke();
        }
        // A clock's face.
        Widget::World(..) => {
            cr.arc(cx, cy, 7.0, 0.0, 2.0 * PI);
            let _ = cr.stroke();
            cr.move_to(cx, cy - 4.5);
            cr.line_to(cx, cy);
            cr.line_to(cx + 3.5, cy + 1.5);
            let _ = cr.stroke();
        }
    }
}

const MEDIA_RADIUS: f64 = 13.0;
const MEDIA_STEP: f64 = 30.0;
/// How wide the player's three buttons are together.
const MEDIA_WIDTH: f64 = 2.0 * MEDIA_RADIUS + 2.0 * MEDIA_STEP;

/// The player's buttons in a row from `x`, centred on `cy`, and where each is, for pressing.
fn media(cr: &Context, playing: &Playing, x: f64, cy: f64) -> Vec<Hit> {
    // A triangle with its tip at `tip`, pointing right for 1 and left for -1.
    let triangle = |tip: f64, towards: f64| {
        cr.move_to(tip, cy);
        cr.line_to(tip - towards * 6.5, cy - 5.0);
        cr.line_to(tip - towards * 6.5, cy + 5.0);
        cr.close_path();
        let _ = cr.fill();
    };
    let mut hits = Vec::new();
    for (i, button) in [Button::Previous, Button::PlayPause, Button::Next].into_iter().enumerate() {
        let cx = x + MEDIA_RADIUS + i as f64 * MEDIA_STEP;
        cr.new_path();
        cr.arc(cx, cy, MEDIA_RADIUS, 0.0, 2.0 * PI);
        set(cr, INK, 0.12);
        let _ = cr.fill();
        set(cr, INK, 0.92);
        match button {
            Button::Previous => {
                cr.rectangle(cx - 5.0, cy - 5.0, 1.8, 10.0);
                let _ = cr.fill();
                triangle(cx - 2.6, -1.0);
            }
            Button::Next => {
                cr.rectangle(cx + 3.2, cy - 5.0, 1.8, 10.0);
                let _ = cr.fill();
                triangle(cx + 2.6, 1.0);
            }
            _ if playing.paused => triangle(cx + 3.8, 1.0),
            _ => {
                cr.rectangle(cx - 3.8, cy - 5.0, 2.4, 10.0);
                cr.rectangle(cx + 1.4, cy - 5.0, 2.4, 10.0);
                let _ = cr.fill();
            }
        }
        hits.push((button, cx, cy, MEDIA_RADIUS));
    }
    hits
}

/// A padlock at the top of the screen, as a phone's lock has.
fn padlock(cr: &Context, cx: f64, top: f64) {
    set(cr, INK, 0.86);
    cr.new_path();
    cr.set_line_width(1.8);
    cr.arc(cx, top + 6.0, 4.5, PI, 0.0);
    cr.line_to(cx + 4.5, top + 9.0);
    cr.move_to(cx - 4.5, top + 9.0);
    cr.line_to(cx - 4.5, top + 6.0);
    let _ = cr.stroke();
    rounded(cr, cx - 7.0, top + 9.0, 14.0, 11.0, 2.5);
    let _ = cr.fill();
}

/// The picture placed on the lock, under the words.
fn sticker(cr: &Context, look: &Look, width: f64, height: f64) {
    let Some((picture, (x, y), size)) = &look.sticker else { return };
    let scale = size / f64::from(picture.width().max(picture.height()).max(1));
    let (w, h) = (f64::from(picture.width()) * scale, f64::from(picture.height()) * scale);
    cr.save().ok();
    cr.translate(x * width - w / 2.0, y * height - h / 2.0);
    cr.scale(scale, scale);
    let _ = cr.set_source_surface(picture, 0.0, 0.0);
    let _ = cr.paint();
    cr.restore().ok();
}

/// The middle of the column the time stands in, as wide as `half` either side of it.
fn column(look: &Look, width: f64, half: f64) -> f64 {
    let side = 64.0 + half;
    match look.clock_align {
        Align::Left => side,
        Align::Center => width / 2.0,
        Align::Right => width - side,
    }
}

/// The notices as the person asked to see them: none, how many from each program, or each.
fn notice_lines(look: &Look, facts: &Facts) -> Vec<(String, String)> {
    match look.notices {
        Notices::Hidden => Vec::new(),
        Notices::Full => facts.notices.iter().rev().take(3).cloned().collect(),
        Notices::Count => {
            let mut counted: Vec<(String, usize)> = Vec::new();
            for (app, _) in &facts.notices {
                match counted.iter_mut().find(|(seen, _)| seen == app) {
                    Some((_, count)) => *count += 1,
                    None => counted.push((app.clone(), 1)),
                }
            }
            counted.into_iter().map(|(app, count)| (app, format!("{count} 則通知"))).collect()
        }
    }
}

/// The words, the widgets and the field, in points, on a surface `width` by `height` points;
/// where the player's buttons are, which only the styles know.
pub fn words(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) -> Vec<Hit> {
    sticker(cr, look, width, height);
    if look.lock_icon {
        padlock(cr, width / 2.0, EDGE - 8.0);
    }
    let hits = match look.style {
        Style::Quiet => quiet(cr, look, moment, width, height),
        Style::Poster => poster(cr, look, moment, width, height),
        Style::Dial => dial(cr, look, moment, width, height),
    };
    subject(cr, look, width, height);
    draw_buttons(cr, look, moment, width, height);
    hits
}

/// The cut-out subject over the time, where the wallpaper has it: covering the screen as
/// the locked backdrop does, and fading out above the field, which stays in reach.
fn subject(cr: &Context, look: &Look, width: f64, height: f64) {
    let Some(picture) = &look.subject else { return };
    let (w, h) = (f64::from(picture.width()), f64::from(picture.height()));
    let cover = (width / w).max(height / h) * NEARER;
    let fade = LinearGradient::new(0.0, height * 0.5, 0.0, height * 0.64);
    fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 1.0);
    fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
    cr.save().ok();
    let pattern = SurfacePattern::create(picture);
    // From the screen to the picture: centred, `cover` times larger.
    let (x, y) = ((width - w * cover) / 2.0, (height - h * cover) / 2.0);
    pattern.set_matrix(cairo::Matrix::new(1.0 / cover, 0.0, 0.0, 1.0 / cover, -x / cover, -y / cover));
    cr.set_source(&pattern).ok();
    let _ = cr.mask(&fade);
    cr.restore().ok();
}

fn quiet(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) -> Vec<Hit> {
    const CARD_W: f64 = 380.0;
    let cx = column(look, width, CARD_W / 2.0);
    let mut y = height * 0.16;
    if look.show_date {
        set(cr, INK, 0.74);
        y += centred(cr, &text(cr, look, moment.date, 19.0, MEDIUM), cx, y);
    }
    set(cr, look.clock_colour.unwrap_or(INK), 1.0);
    let size = (width * 0.105).min(height * 0.17).min(150.0) * look.clock_scale;
    y += centred(cr, &clock_text(cr, look, moment.time, size), cx, y);

    // The widgets in rows of pills under the time, the player's buttons in its own.
    let mut hits = Vec::new();
    const PILL: f64 = 40.0;
    const SPACE: f64 = 10.0;
    let mut rows: Vec<Vec<(Widget, pango::Layout, f64)>> = Vec::new();
    let mut used = f64::INFINITY;
    for widget in widgets(look, moment.facts) {
        let layout = line(cr, look, &widget_words(&widget), 14.0, MEDIUM, 260.0);
        let after = if matches!(widget, Widget::Playing(_)) { SPACE + MEDIA_WIDTH + 7.0 } else { 16.0 };
        let w = 40.0 + f64::from(layout.pixel_extents().1.width()) + after;
        if used + SPACE + w > width - 2.0 * EDGE {
            rows.push(Vec::new());
            used = -SPACE;
        }
        used += SPACE + w;
        if let Some(row) = rows.last_mut() {
            row.push((widget, layout, w));
        }
    }
    let mut top = y + 22.0;
    for row in rows {
        let total: f64 = row.iter().map(|(_, _, w)| w).sum::<f64>() + SPACE * (row.len() - 1) as f64;
        let mut x = (cx - total / 2.0).clamp(EDGE, (width - EDGE - total).max(EDGE));
        for (widget, layout, w) in &row {
            glass(cr, x, top, *w, PILL, PILL / 2.0, 0.13);
            set(cr, INK, 0.9);
            glyph(cr, widget, x + 24.0, top + PILL / 2.0);
            set(cr, INK, 1.0);
            let (_, extent) = layout.pixel_extents();
            at(cr, layout, x + 40.0, top + (PILL - f64::from(extent.height())) / 2.0);
            if let Widget::Playing(playing) = widget {
                hits.extend(media(cr, playing, x + 40.0 + f64::from(extent.width()) + SPACE, top + PILL / 2.0));
            }
            x += w + SPACE;
        }
        y = top + PILL;
        top += PILL + SPACE;
    }

    // The notices, as cards stacked under them, as many as there is room for above the field.
    let field_top = (height * 0.82).min(lowest_field(look, width, height));
    let lowest = field_top - if look.show_name { 120.0 } else { 24.0 };
    let mut top = y + 28.0;
    for (app, summary) in notice_lines(look, moment.facts) {
        const CARD_H: f64 = 56.0;
        if top + CARD_H > lowest {
            break;
        }
        let x = cx - CARD_W / 2.0;
        glass(cr, x, top, CARD_W, CARD_H, 16.0, 0.16);
        set(cr, INK, 1.0);
        at(cr, &line(cr, look, &app, 14.0, pango::Weight::Bold, CARD_W - 32.0), x + 16.0, top + 9.0);
        set(cr, INK, 0.74);
        at(cr, &line(cr, look, &summary, 13.0, MEDIUM, CARD_W - 32.0), x + 16.0, top + 30.0);
        top += CARD_H + 8.0;
    }

    if look.show_name {
        set(cr, INK, 1.0);
        let name = text(cr, look, &look.name, 15.0, MEDIUM);
        let name_height = f64::from(name.pixel_extents().1.height());
        let name_top = field_top - 12.0 - name_height;
        centred(cr, &name, cx, name_top);
        picture(cr, look, cx - 28.0, name_top - 10.0 - 56.0, 56.0);
    }
    field(cr, look, moment, cx - FIELD_WIDTH / 2.0, field_top, FIELD_WIDTH, false);

    if !look.message.is_empty() {
        set(cr, INK, 0.74);
        // In the top left: the corners at the foot are the shortcuts'.
        let message = line(cr, look, &look.message, 13.0, MEDIUM, width * 0.4);
        at(cr, &message, 32.0, 28.0);
    }
    hits
}

fn poster(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) -> Vec<Hit> {
    let left = (width * 0.067).max(48.0);
    let mut y = height * 0.12;
    if look.show_date {
        let (weekday, date) = moment.date.rsplit_once(' ').map_or((moment.date, ""), |(date, weekday)| (weekday, date));
        set(cr, look.accent, 1.0);
        let (w, h) = at(cr, &text(cr, look, weekday, 20.0, MEDIUM), left, y);
        set(cr, INK, 0.76);
        at(cr, &text(cr, look, date, 20.0, MEDIUM), left + w + 16.0, y);
        y += h + 6.0;
    }

    let mut parts = moment.time.split(':');
    let (hours, minutes, seconds) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next());
    let size = (height * 0.235).min(width * 0.15).min(212.0) * look.clock_scale;
    for (i, part) in [hours, minutes].into_iter().enumerate() {
        let colour = if i == 0 { INK } else { look.clock_colour.unwrap_or(look.accent) };
        set(cr, colour, 1.0);
        let layout = clock_text(cr, look, part, size);
        // The lines of figures overlap a little, as a poster sets them.
        let (ink, extent) = layout.pixel_extents();
        let x = left - size * 0.04;
        cr.move_to(x, y);
        pangocairo::functions::show_layout(cr, &layout);
        if let Some(seconds) = seconds.filter(|_| i == 1) {
            // Small, at the foot of the minutes.
            let small = clock_text(cr, look, seconds, size * 0.28);
            let (small_ink, _) = small.pixel_extents();
            let foot = y + f64::from(ink.y() + ink.height());
            at(cr, &small, x + f64::from(ink.x() + ink.width()) + 14.0, foot - f64::from(small_ink.y() + small_ink.height()));
        }
        y += f64::from(extent.height()) * 0.76;
    }
    y += 40.0;

    let mut hits = Vec::new();
    let facts: Vec<(String, String, Option<&Playing>)> = widgets(look, moment.facts)
        .into_iter()
        .map(|widget| match widget {
            Widget::Battery(percent, charging) => ("電池".to_string(), battery_words((percent, charging)), None),
            Widget::Playing(playing) => ("播放".to_string(), playing.words(), Some(playing)),
            Widget::Event(event) => ("行程".to_string(), event.to_string(), None),
            Widget::World(city, time) | Widget::Weather(city, time) => (city.to_string(), time.to_string(), None),
        })
        .collect();
    let labels: Vec<pango::Layout> = facts.iter().map(|(label, ..)| text(cr, look, label, 15.0, MEDIUM)).collect();
    let label_width = labels.iter().map(|label| f64::from(label.pixel_extents().1.width())).fold(0.0, f64::max);
    for ((_, value, playing), label) in facts.iter().zip(&labels) {
        set(cr, INK, 0.52);
        let (_, h) = at(cr, label, left, y);
        set(cr, INK, 1.0);
        let (w, _) = at(cr, &line(cr, look, value, 15.0, MEDIUM, 360.0), left + label_width + 14.0, y);
        if let Some(playing) = playing {
            hits.extend(media(cr, playing, left + label_width + 14.0 + w + 14.0, y + h / 2.0));
        }
        y += h + 10.0;
    }

    // The field at the foot of the column, the person's picture before it.
    let field_top = (height - (height * 0.1).max(56.0) - FIELD_HEIGHT).min(lowest_field(look, width, height));
    let mut x = left;
    if look.show_name {
        picture(cr, look, x, field_top - 4.0, 44.0);
        x += 58.0;
    }
    field(cr, look, moment, x, field_top, 260.0, true);

    // The notices down the right.
    let lines = notice_lines(look, moment.facts);
    const LIST: f64 = 340.0;
    let right = width - (width * 0.05).max(40.0) - LIST;
    let mut top = height * 0.12;
    for (app, summary) in lines {
        rounded(cr, right, top, LIST, 66.0, 14.0);
        cr.set_source_rgba(0.09, 0.086, 0.094, 0.5);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.14);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        set(cr, INK, 0.52);
        at(cr, &line(cr, look, &app, 12.0, MEDIUM, LIST - 32.0), right + 16.0, top + 11.0);
        set(cr, INK, 1.0);
        at(cr, &line(cr, look, &summary, 14.0, MEDIUM, LIST - 32.0), right + 16.0, top + 31.0);
        top += 74.0;
    }

    if !look.message.is_empty() {
        set(cr, INK, 0.74);
        // Over the notices, on ink as they are, the wallpaper there being bare: the corners
        // at the foot are the shortcuts'.
        let message = line(cr, look, &look.message, 13.0, MEDIUM, LIST - 24.0);
        let (_, extent) = message.pixel_extents();
        let (w, h) = (f64::from(extent.width()) + 24.0, 30.0);
        let (x, y) = (width - (width * 0.05).max(40.0) - w, height * 0.12 - 12.0 - h);
        rounded(cr, x, y, w, h, h / 2.0);
        cr.set_source_rgba(0.09, 0.086, 0.094, 0.5);
        let _ = cr.fill();
        set(cr, INK, 0.86);
        at(cr, &message, x + 12.0, y + (h - f64::from(extent.height())) / 2.0);
    }
    hits
}

fn dial(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) -> Vec<Hit> {
    // As large as asked, but no larger than leaves room under it for the date, the other
    // widgets and the name above the field, which the shortcuts' row may push up.
    let found = widgets(look, moment.facts);
    let below_dial = 24.0
        + if look.show_date { 34.0 } else { 0.0 }
        + if found.iter().any(|widget| matches!(widget, Widget::Event(_) | Widget::World(..) | Widget::Weather(..))) { 22.0 } else { 0.0 }
        + if look.show_name { 40.0 } else { 8.0 };
    let room = lowest_field(look, width, height) - height * 0.1 - below_dial;
    let radius = ((height * 0.24).min(width * 0.2).min(220.0) * look.clock_scale).min(room / 2.0).max(60.0);
    let (cx, cy) = (column(look, width, radius.max(FIELD_WIDTH / 2.0)), height * 0.1 + radius);

    set(cr, INK, 0.06);
    cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
    let _ = cr.fill_preserve();
    set(cr, INK, 0.18);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    cr.set_line_cap(cairo::LineCap::Round);
    for hour in 0..12 {
        let quarter = hour % 3 == 0;
        let angle = f64::from(hour) * PI / 6.0;
        let (outer, inner) = (radius * 0.93, radius * if quarter { 0.86 } else { 0.89 });
        set(cr, INK, if quarter { 0.7 } else { 0.32 });
        cr.set_line_width(if quarter { 3.0 } else { 2.0 });
        cr.move_to(cx + outer * angle.sin(), cy - outer * angle.cos());
        cr.line_to(cx + inner * angle.sin(), cy - inner * angle.cos());
        let _ = cr.stroke();
    }

    // The widgets as small dials: the battery above the middle, what plays below it; the
    // others under the date.
    let mut hits = Vec::new();
    let mut others = Vec::new();
    for widget in found {
        match widget {
            Widget::Battery(percent, charging) => {
                let (bx, by) = (cx, cy - radius * 0.42);
                ring(cr, bx, by, radius * 0.17, 5.0, f64::from(percent) / 100.0, if charging { look.accent } else { INK });
                set(cr, INK, 1.0);
                let layout = text(cr, look, &format!("{percent}%"), 16.0, MEDIUM);
                let (_, extent) = layout.pixel_extents();
                centred(cr, &layout, bx, by - f64::from(extent.height()) / 2.0);
            }
            Widget::Playing(playing) => {
                let by = cy + radius * 0.3;
                set(cr, INK, 0.5);
                let label = text(cr, look, "播放中", 12.0, MEDIUM);
                let h = centred(cr, &label, cx, by);
                set(cr, INK, 1.0);
                let name = line(cr, look, &playing.title, 16.0, MEDIUM, radius * 0.9);
                let (_, extent) = name.pixel_extents();
                at(cr, &name, cx - f64::from(extent.width()) / 2.0, by + h + 2.0);
                let buttons_y = by + h + 2.0 + f64::from(extent.height()) + 8.0 + MEDIA_RADIUS;
                hits.extend(media(cr, playing, cx - MEDIA_WIDTH / 2.0, buttons_y));
            }
            other => others.push(widget_words(&other)),
        }
    }

    // The hands.
    let minute = f64::from(moment.minute);
    let hour = f64::from(moment.hour % 12) + minute / 60.0;
    let hand = |angle: f64, length: f64, line: f64, colour: Rgb| {
        set(cr, colour, 1.0);
        cr.set_line_width(line);
        cr.move_to(cx - 0.08 * length * angle.sin(), cy + 0.08 * length * angle.cos());
        cr.line_to(cx + length * angle.sin(), cy - length * angle.cos());
        let _ = cr.stroke();
    };
    let accent = look.clock_colour.unwrap_or(look.accent);
    hand(hour * PI / 6.0, radius * 0.5, 9.0, INK);
    hand(minute * PI / 30.0, radius * 0.8, 5.0, accent);
    if look.clock_seconds {
        hand(f64::from(moment.second) * PI / 30.0, radius * 0.86, 1.6, accent);
    }
    set(cr, accent, 1.0);
    cr.arc(cx, cy, 7.0, 0.0, 2.0 * PI);
    let _ = cr.fill();
    set(cr, PAPER, 1.0);
    cr.arc(cx, cy, 2.5, 0.0, 2.0 * PI);
    let _ = cr.fill();

    let mut below = cy + radius + 24.0;
    if look.show_date {
        set(cr, INK, 0.76);
        below += centred(cr, &text(cr, look, moment.date, 18.0, MEDIUM), cx, below) + 8.0;
    }
    if !others.is_empty() {
        set(cr, INK, 0.6);
        let layout = line(cr, look, &others.join("　·　"), 15.0, MEDIUM, width * 0.6);
        let (w, h) = (f64::from(layout.pixel_extents().1.width()), f64::from(layout.pixel_extents().1.height()));
        at(cr, &layout, cx - w / 2.0, below);
        below += h;
    }

    let field_top = (height * 0.8).max(below + 70.0).min(lowest_field(look, width, height));
    if look.show_name {
        set(cr, INK, 1.0);
        let name = text(cr, look, &look.name, 15.0, MEDIUM);
        let h = f64::from(name.pixel_extents().1.height());
        centred(cr, &name, cx, field_top - 12.0 - h);
    }
    field(cr, look, moment, cx - FIELD_WIDTH / 2.0, field_top, FIELD_WIDTH, false);

    // The notices as pills in the top left, the message in the top right.
    let mut x = 32.0;
    for (app, summary) in notice_lines(look, moment.facts) {
        let words = format!("{app}　{summary}");
        let layout = line(cr, look, &words, 13.0, MEDIUM, 300.0);
        let (_, extent) = layout.pixel_extents();
        let w = f64::from(extent.width()) + 24.0;
        rounded(cr, x, 28.0, w, 30.0, 15.0);
        set(cr, INK, 0.1);
        let _ = cr.fill();
        set(cr, INK, 0.8);
        at(cr, &layout, x + 12.0, 28.0 + (30.0 - f64::from(extent.height())) / 2.0);
        x += w + 8.0;
    }
    if !look.message.is_empty() {
        set(cr, INK, 0.6);
        let message = line(cr, look, &look.message, 13.0, MEDIUM, width * 0.4);
        let (_, extent) = message.pixel_extents();
        at(cr, &message, width - 32.0 - f64::from(extent.width()), 34.0);
    }
    hits
}

/// The lock dozing, as an always-on display: the time and the date dim on black, moved a
/// little each minute so they do not burn into the screen.
pub fn doze(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    cr.set_source_rgb(0.0, 0.0, 0.0);
    let _ = cr.paint();
    let drift = |step: u32, span: f64| (f64::from(step % 7) / 6.0 - 0.5) * span;
    let (cx, mut y) = (width / 2.0 + drift(moment.minute, 80.0), height * 0.3 + drift(moment.minute / 7, 60.0));
    set(cr, look.clock_colour.unwrap_or(INK), 0.62);
    // Without seconds: they would keep it awake.
    let time = moment.time.splitn(3, ':').take(2).collect::<Vec<_>>().join(":");
    y += centred(cr, &clock_text(cr, look, &time, (height * 0.12).min(110.0)), cx, y);
    if look.show_date {
        set(cr, INK, 0.42);
        centred(cr, &text(cr, look, moment.date, 17.0, MEDIUM), cx, y + 4.0);
    }
    if moment.finger {
        // Where a finger wakes and unlocks it, as a phone's always-on display shows.
        set(cr, INK, 0.36);
        fingerprint(cr, width / 2.0, height * 0.84, 16.0);
    }
}

/// A colour of the picture's to set the clock in: its most colourful, made light enough to
/// read on ink.
pub fn accent_of(picture: &Pixbuf) -> Option<Rgb> {
    let small = picture.scale_simple(16, 16, InterpType::Bilinear)?;
    let bytes = small.read_pixel_bytes();
    let (channels, stride) = (small.n_channels() as usize, small.rowstride() as usize);
    let mut best: Option<(f64, Rgb)> = None;
    for y in 0..16 {
        for x in 0..16 {
            let pixel = &bytes[y * stride + x * channels..];
            let rgb = (f64::from(pixel[0]) / 255.0, f64::from(pixel[1]) / 255.0, f64::from(pixel[2]) / 255.0);
            let (h, s, l) = hsl(rgb);
            // Colourful counts most; near-black and near-white do not count at all.
            let score = s * (1.0 - (l - 0.5).abs() * 2.0);
            if best.is_none_or(|(seen, _)| score > seen) {
                best = Some((score, (h, s, l)));
            }
        }
    }
    let (score, (h, s, _)) = best?;
    (score > 0.08).then(|| from_hsl(h, s.clamp(0.35, 0.7), 0.74))
}

fn hsl((r, g, b): Rgb) -> Rgb {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let channel = |t: f64| {
        let t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// The locked layer `t` of the way gone, from 0 to 1, over `under`, as the effect has it.
/// Lifted, its bottom edge fades, so it reads as a sheet taken off rather than a curtain.
/// In pixels.
pub fn reveal(cr: &Context, effect: Effect, under: &ImageSurface, layer: &ImageSurface, t: f64, scale: f64) {
    let _ = cr.set_source_surface(under, 0.0, 0.0);
    let _ = cr.paint();
    let (width, height) = (f64::from(layer.width()), f64::from(layer.height()));
    let eased = 1.0 - (1.0 - t).powi(3);
    match effect {
        Effect::Lift => {
            let feather = FEATHER * scale;
            let offset = -eased * (height + feather);
            let fade = LinearGradient::new(0.0, offset + height - feather, 0.0, offset + height);
            fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 1.0);
            fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
            let _ = cr.set_source_surface(layer, 0.0, offset);
            let _ = cr.mask(&fade);
        }
        Effect::Fade => {
            let _ = cr.set_source_surface(layer, 0.0, 0.0);
            let _ = cr.paint_with_alpha(1.0 - eased);
        }
        Effect::Slide => {
            let _ = cr.set_source_surface(layer, -eased * width, 0.0);
            let _ = cr.paint();
        }
        Effect::Expand => {
            let grow = 1.0 + 0.16 * eased;
            cr.save().ok();
            cr.translate(width / 2.0, height / 2.0);
            cr.scale(grow, grow);
            cr.translate(-width / 2.0, -height / 2.0);
            let _ = cr.set_source_surface(layer, 0.0, 0.0);
            let _ = cr.paint_with_alpha(1.0 - eased);
            cr.restore().ok();
        }
    }
}

/// A picture as Cairo draws it: premultiplied, in native byte order.
pub fn pixbuf_surface(pixbuf: &Pixbuf) -> Option<ImageSurface> {
    let (width, height) = (pixbuf.width(), pixbuf.height());
    let channels = pixbuf.n_channels() as usize;
    let rowstride = pixbuf.rowstride() as usize;
    let bytes = pixbuf.read_pixel_bytes();
    let mut surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().ok()?;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let pixel = &bytes[y * rowstride + x * channels..];
                let a = if channels == 4 { pixel[3] } else { 255 };
                let premultiply = |channel: u8| (u16::from(channel) * u16::from(a) / 255) as u8;
                let native = u32::from_be_bytes([
                    a,
                    premultiply(pixel[0]),
                    premultiply(pixel[1]),
                    premultiply(pixel[2]),
                ]);
                let at = y * stride + x * 4;
                data[at..at + 4].copy_from_slice(&native.to_ne_bytes());
            }
        }
    }
    Some(surface)
}

#[cfg(test)]
mod tests {
    use super::{from_hsl, hsl, reveal, shake, Effect};
    use cairo::{Context, Format, ImageSurface};

    #[test]
    fn every_unlock_effect_starts_on_the_lock_and_ends_on_the_desktop() {
        // As tall as a screen: the lift's fade at its foot is taller than a small one.
        let filled = |rgb: f64| {
            let surface = ImageSurface::create(Format::ARgb32, 64, 600).unwrap();
            let cr = Context::new(&surface).unwrap();
            cr.set_source_rgb(rgb, rgb, rgb);
            cr.paint().unwrap();
            drop(cr);
            surface
        };
        let (under, layer) = (filled(0.0), filled(1.0));
        for effect in [Effect::Lift, Effect::Fade, Effect::Slide, Effect::Expand] {
            for (t, expected) in [(0.0, 255), (1.0, 0)] {
                let mut target = ImageSurface::create(Format::ARgb32, 64, 600).unwrap();
                let cr = Context::new(&target).unwrap();
                reveal(&cr, effect, &under, &layer, t, 1.0);
                drop(cr);
                // The middle pixel's blue byte, white from the lock and black from the desktop.
                let stride = target.stride() as usize;
                let data = target.data().unwrap();
                assert_eq!(data[300 * stride + 32 * 4], expected, "{effect:?} at {t}");
            }
        }
    }

    #[test]
    fn colours_come_back_from_hue_saturation_and_lightness() {
        let sand = (0.95, 0.70, 0.54);
        let (h, s, l) = hsl(sand);
        let back = from_hsl(h, s, l);
        assert!((back.0 - sand.0).abs() < 1e-9 && (back.1 - sand.1).abs() < 1e-9 && (back.2 - sand.2).abs() < 1e-9);
    }

    #[test]
    fn the_shake_settles_where_it_started() {
        assert_eq!(shake(0.0), 0.0);
        assert_eq!(shake(1.0), 0.0);
        assert!(shake(0.08).abs() > 5.0);
        assert!(shake(0.9).abs() < 1.0);
    }
}
