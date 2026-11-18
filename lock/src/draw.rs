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

/// How the lock goes once the password is given: LockStar's twelve, and the lift.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Effect {
    /// Taken off upwards, its foot fading.
    Lift,
    Fade,
    /// Pushed off, the way the direction says.
    Slide,
    /// Coming nearer as it fades.
    Expand,
    /// A hole opening from the middle, its edge in the effect's colour.
    Spread,
    /// Strips going off one after another, as a wave.
    Wave,
    /// Stretched wide and flat as it fades.
    Warp,
    /// A hole opening from the middle, rings of colour in it.
    Ripple,
    /// Tiles going one by one.
    Mosaic,
    /// Swept off around the middle, as a clock's hand.
    Wiper,
    /// Brushed off in bands, the brush in the effect's colour.
    Paint,
    /// Strips jumping aside as it fades.
    Glitch,
    /// Turned and drawn into the middle.
    Swirl,
}

/// The way an unlock effect goes, where it has one.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// How an unlock effect's pace changes as it goes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Easing {
    /// Quick, then slowing.
    Out,
    /// Slow, quick, slow.
    InOut,
    Linear,
}

/// The unlock effect as the person set it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Unlock {
    pub effect: Effect,
    /// Of the edges and brushes; the wallpaper's accent where none was chosen.
    pub colour: Option<Rgb>,
    pub direction: Direction,
    pub easing: Easing,
}

/// The sign at the top of the lock, as LockStar's lock icons.
pub enum Icon {
    Padlock,
    Shield,
    Key,
    Heart,
    Paw,
    Picture(ImageSurface),
}

/// Where the shortcuts stand.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shortcuts {
    /// Two, in the corners at the foot.
    Corners,
    /// Up to six, in a row over the power buttons.
    Row,
    /// Up to six, one over another at the right edge.
    Column,
}

/// The figures on the dial's face.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Numerals {
    None,
    Arabic,
    Roman,
}

/// How thick the dial's hands are.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hands {
    Classic,
    Thin,
    Bold,
}

/// What a finger on the reader shows around the fingerprint.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FingerEffect {
    None,
    /// Rings going out from it.
    Ripple,
    /// A glow fading from it.
    Glow,
}

/// A picture placed on the lock: its centre as shares of the width and the height, and its
/// longer side in points.
pub type Sticker = (ImageSurface, (f64, f64), f64);

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
    /// Whether the time goes through the colours, as ClockFace's rainbow.
    pub clock_rainbow: bool,
    /// Words and a small picture laid on the clock, as ClockFace's.
    pub clock_label: String,
    pub clock_badge: Option<ImageSurface>,
    pub numerals: Numerals,
    pub hands: Hands,
    /// Where the time, the notices and the message stand instead of where the style puts
    /// them: their middle, as shares of the width and the height.
    pub clock_at: Option<(f64, f64)>,
    pub notices_at: Option<(f64, f64)>,
    pub message_at: Option<(f64, f64)>,
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
    /// What the always-on display shows besides the time, and how bright, from 0 to 1.
    pub always_on_widgets: bool,
    pub always_on_stickers: bool,
    pub always_on_brightness: f64,
    /// Whether a finger may unlock, where fprintd has one enrolled.
    pub fingerprint: bool,
    pub finger_effect: FingerEffect,
    pub doze_after: std::time::Duration,
    /// How much darker than the style's own ink the wallpaper is, from 0 to 1.
    pub dim: f64,
    pub lock_icon: Option<Icon>,
    pub lock_icon_colour: Option<Rgb>,
    pub unlock: Unlock,
    pub unlock_duration: std::time::Duration,
    pub stickers: Vec<Sticker>,
    /// The wallpaper's subject cut out, framed as the wallpaper is, laid over the time as
    /// One UI's depth effect lays it.
    pub subject: Option<ImageSurface>,
    pub shortcuts_layout: Shortcuts,
    pub notices: Notices,
    /// What the person asked to be shown while locked, such as whom to call; how large, in
    /// what colour and how opaque.
    pub message: String,
    pub message_size: f64,
    pub message_colour: Option<Rgb>,
    pub message_opacity: f64,
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
    // A column of shortcuts stands at the edge, beside the field rather than under it.
    let beside = |button: &Hit| look.shortcuts_layout == Shortcuts::Column && matches!(button.0, Button::Shortcut(_));
    let foot = buttons(look, width, height)
        .into_iter()
        .filter(|button| !beside(button))
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
    const STEP: f64 = 2.0 * SHORTCUT_RADIUS + 20.0;
    match look.shortcuts_layout {
        Shortcuts::Row => {
            let above = if look.power_buttons { power_y - POWER_RADIUS - 20.0 } else { height - EDGE };
            let y = above - SHORTCUT_RADIUS;
            let count = shown(ROW_SHORTCUTS).count() as f64;
            for (i, (at, _)) in shown(ROW_SHORTCUTS).enumerate() {
                found.push((Button::Shortcut(at), width / 2.0 + (i as f64 - (count - 1.0) / 2.0) * STEP, y, SHORTCUT_RADIUS));
            }
        }
        Shortcuts::Column => {
            // Up the right edge from its foot, the first lowest, clear of the power buttons'
            // row, which is in the middle.
            let (x, foot) = (width - EDGE - SHORTCUT_RADIUS, height - EDGE - SHORTCUT_RADIUS);
            for (i, (at, _)) in shown(ROW_SHORTCUTS).enumerate() {
                found.push((Button::Shortcut(at), x, foot - i as f64 * STEP, SHORTCUT_RADIUS));
            }
        }
        Shortcuts::Corners => {
            let foot = height - EDGE - SHORTCUT_RADIUS;
            // A slot left empty keeps its side: the second shortcut is always on the right.
            for (at, _) in shown(CORNER_SHORTCUTS) {
                let x = if at == 0 { EDGE + SHORTCUT_RADIUS } else { width - EDGE - SHORTCUT_RADIUS };
                found.push((Button::Shortcut(at), x, foot, SHORTCUT_RADIUS));
            }
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
    /// How far, from 0 to 1, the effect of the last finger read has gone, and whether the
    /// reader knew it.
    pub finger_pulse: Option<(f64, bool)>,
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
    finger_pulse(cr, look, moment, x + width - FINGER_ROOM / 2.0 - 6.0, middle, 9.0);

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

/// What a finger just read shows around the fingerprint at `cx`, `cy`: rings going out from
/// it or a glow fading, in the accent where the reader knew the finger, else in red.
fn finger_pulse(cr: &Context, look: &Look, moment: &Moment, cx: f64, cy: f64, radius: f64) {
    let Some((t, known)) = moment.finger_pulse else { return };
    let colour = if known { look.accent } else { DANGER };
    let fade = 1.0 - t;
    match look.finger_effect {
        FingerEffect::None => {}
        FingerEffect::Ripple => {
            cr.set_line_width(2.0);
            for ring in 0..2 {
                let grown = (t - 0.25 * f64::from(ring)).max(0.0);
                cr.new_path();
                cr.arc(cx, cy, radius + 28.0 * grown, 0.0, 2.0 * PI);
                set(cr, colour, 0.8 * fade);
                let _ = cr.stroke();
            }
        }
        FingerEffect::Glow => {
            let reach = radius * 3.2;
            let glow = RadialGradient::new(cx, cy, 0.0, cx, cy, reach);
            glow.add_color_stop_rgba(0.0, colour.0, colour.1, colour.2, 0.7 * fade);
            glow.add_color_stop_rgba(1.0, colour.0, colour.1, colour.2, 0.0);
            cr.new_path();
            cr.arc(cx, cy, reach, 0.0, 2.0 * PI);
            let _ = cr.set_source(&glow);
            let _ = cr.fill();
        }
    }
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

/// The sign at the top of the screen, 20 points tall from `top`, as a phone's lock has.
fn lock_icon(cr: &Context, look: &Look, icon: &Icon, cx: f64, top: f64) {
    set(cr, look.lock_icon_colour.unwrap_or(INK), 0.86);
    cr.new_path();
    cr.set_line_width(1.8);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);
    match icon {
        Icon::Padlock => {
            cr.arc(cx, top + 6.0, 4.5, PI, 0.0);
            cr.line_to(cx + 4.5, top + 9.0);
            cr.move_to(cx - 4.5, top + 9.0);
            cr.line_to(cx - 4.5, top + 6.0);
            let _ = cr.stroke();
            rounded(cr, cx - 7.0, top + 9.0, 14.0, 11.0, 2.5);
            let _ = cr.fill();
        }
        Icon::Shield => {
            cr.move_to(cx, top);
            cr.line_to(cx + 8.0, top + 3.0);
            cr.curve_to(cx + 8.0, top + 12.0, cx + 4.0, top + 17.0, cx, top + 20.0);
            cr.curve_to(cx - 4.0, top + 17.0, cx - 8.0, top + 12.0, cx - 8.0, top + 3.0);
            cr.close_path();
            let _ = cr.fill();
        }
        Icon::Key => {
            cr.arc(cx - 4.0, top + 10.0, 4.5, 0.0, 2.0 * PI);
            let _ = cr.stroke();
            cr.move_to(cx + 0.5, top + 10.0);
            cr.line_to(cx + 9.0, top + 10.0);
            cr.move_to(cx + 6.0, top + 10.0);
            cr.line_to(cx + 6.0, top + 14.0);
            cr.move_to(cx + 9.0, top + 10.0);
            cr.line_to(cx + 9.0, top + 13.0);
            let _ = cr.stroke();
        }
        Icon::Heart => {
            cr.move_to(cx, top + 19.0);
            cr.curve_to(cx - 12.0, top + 11.0, cx - 8.0, top + 1.0, cx, top + 6.0);
            cr.curve_to(cx + 8.0, top + 1.0, cx + 12.0, top + 11.0, cx, top + 19.0);
            cr.close_path();
            let _ = cr.fill();
        }
        Icon::Paw => {
            // A cat's: the pad and four toes.
            cr.save().ok();
            cr.translate(cx, top + 14.5);
            cr.scale(1.0, 0.8);
            cr.arc(0.0, 0.0, 5.5, 0.0, 2.0 * PI);
            cr.restore().ok();
            let _ = cr.fill();
            for (dx, dy) in [(-7.0, 7.5), (-2.6, 3.5), (2.6, 3.5), (7.0, 7.5)] {
                cr.arc(cx + dx, top + dy, 2.4, 0.0, 2.0 * PI);
                let _ = cr.fill();
            }
        }
        Icon::Picture(picture) => {
            let side = 22.0;
            let scale = side / f64::from(picture.width().max(picture.height()).max(1));
            cr.save().ok();
            cr.translate(cx - f64::from(picture.width()) * scale / 2.0, top);
            cr.scale(scale, scale);
            let _ = cr.set_source_surface(picture, 0.0, 0.0);
            let _ = cr.paint();
            cr.restore().ok();
        }
    }
}

/// The pictures placed on the lock, under the words, as opaque as `alpha`.
fn stickers(cr: &Context, look: &Look, width: f64, height: f64, alpha: f64) {
    for (picture, (x, y), size) in &look.stickers {
        let scale = size / f64::from(picture.width().max(picture.height()).max(1));
        let (w, h) = (f64::from(picture.width()) * scale, f64::from(picture.height()) * scale);
        cr.save().ok();
        cr.translate(x * width - w / 2.0, y * height - h / 2.0);
        cr.scale(scale, scale);
        let _ = cr.set_source_surface(picture, 0.0, 0.0);
        let _ = cr.paint_with_alpha(alpha);
        cr.restore().ok();
    }
}

/// The message as the person styled it, at most `width` wide; its colour set, `alpha` as
/// opaque as the style draws it and the person's opacity together.
fn message_layout(cr: &Context, look: &Look, alpha: f64, width: f64) -> pango::Layout {
    set(cr, look.message_colour.unwrap_or(INK), alpha * look.message_opacity);
    line(cr, look, &look.message, look.message_size, MEDIUM, width)
}

/// The time's colour, from `x0` to `x1`: the person's, or `fallback`; through the colours,
/// where they asked for the rainbow.
fn clock_ink(cr: &Context, look: &Look, fallback: Rgb, alpha: f64, x0: f64, x1: f64) {
    if !look.clock_rainbow {
        return set(cr, look.clock_colour.unwrap_or(fallback), alpha);
    }
    let rainbow = LinearGradient::new(x0, 0.0, x1.max(x0 + 1.0), 0.0);
    for stop in 0..=6 {
        let share = f64::from(stop) / 6.0;
        let (r, g, b) = from_hsl(share * 0.83, 0.7, 0.72);
        rainbow.add_color_stop_rgba(share, r, g, b, alpha);
    }
    let _ = cr.set_source(&rainbow);
}

/// A layout's width, in points.
fn width_of(layout: &pango::Layout) -> f64 {
    f64::from(layout.pixel_extents().1.width())
}

/// The words and the small picture laid on the clock, centred on `cx` from `y`, or from `x`
/// where `cx` is not given; how tall, nothing if there is neither.
fn badge(cr: &Context, look: &Look, cx: Option<f64>, x: f64, y: f64) -> f64 {
    const SIDE: f64 = 28.0;
    let label = (!look.clock_label.is_empty()).then(|| text(cr, look, &look.clock_label, 15.0, MEDIUM));
    let picture_w = if look.clock_badge.is_some() { SIDE + if label.is_some() { 8.0 } else { 0.0 } } else { 0.0 };
    let label_w = label.as_ref().map_or(0.0, width_of);
    if picture_w == 0.0 && label.is_none() {
        return 0.0;
    }
    let left = cx.map_or(x, |cx| cx - (picture_w + label_w) / 2.0);
    if let Some(picture) = &look.clock_badge {
        let scale = SIDE / f64::from(picture.width().max(picture.height()).max(1));
        cr.save().ok();
        cr.translate(left, y);
        cr.scale(scale, scale);
        let _ = cr.set_source_surface(picture, 0.0, 0.0);
        let _ = cr.paint();
        cr.restore().ok();
    }
    if let Some(label) = &label {
        set(cr, look.clock_colour.unwrap_or(look.accent), 1.0);
        let h = f64::from(label.pixel_extents().1.height());
        at(cr, label, left + picture_w, y + (SIDE - h) / 2.0);
    }
    SIDE
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
    stickers(cr, look, width, height, 1.0);
    if let Some(icon) = &look.lock_icon {
        lock_icon(cr, look, icon, width / 2.0, EDGE - 8.0);
    }
    let hits = match look.style {
        Style::Quiet => quiet(cr, look, moment, width, height),
        Style::Poster => poster(cr, look, moment, width, height),
        Style::Dial => dial(cr, look, moment, width, height),
    };
    // What the person moved, where they put it rather than where the style does.
    if let (Some((x, y)), false) = (look.message_at, look.message.is_empty()) {
        let message = message_layout(cr, look, 0.86, width * 0.5);
        let h = f64::from(message.pixel_extents().1.height());
        centred(cr, &message, x * width, y * height - h / 2.0);
    }
    if let Some((x, y)) = look.notices_at {
        let lines = notice_lines(look, moment.facts);
        let tall = lines.len() as f64 * (CARD_H + CARD_GAP) - CARD_GAP;
        let top = (y * height - tall / 2.0).max(EDGE);
        cards(cr, look, &lines, (x * width).clamp(EDGE + CARD_W / 2.0, width - EDGE - CARD_W / 2.0), top, height - EDGE);
    }
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

const CARD_W: f64 = 380.0;
const CARD_H: f64 = 56.0;
const CARD_GAP: f64 = 8.0;

/// The notices as cards, one under another from `top` centred on `cx`, as many as fit above
/// `bottom`.
fn cards(cr: &Context, look: &Look, lines: &[(String, String)], cx: f64, mut top: f64, bottom: f64) {
    for (app, summary) in lines {
        if top + CARD_H > bottom {
            break;
        }
        let x = cx - CARD_W / 2.0;
        glass(cr, x, top, CARD_W, CARD_H, 16.0, 0.16);
        set(cr, INK, 1.0);
        at(cr, &line(cr, look, app, 14.0, pango::Weight::Bold, CARD_W - 32.0), x + 16.0, top + 9.0);
        set(cr, INK, 0.74);
        at(cr, &line(cr, look, summary, 13.0, MEDIUM, CARD_W - 32.0), x + 16.0, top + 30.0);
        top += CARD_H + CARD_GAP;
    }
}

fn quiet(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) -> Vec<Hit> {
    let cx = column(look, width, CARD_W / 2.0);
    let size = (width * 0.105).min(height * 0.17).min(150.0) * look.clock_scale;
    let date = look.show_date.then(|| text(cr, look, moment.date, 19.0, MEDIUM));
    let time = clock_text(cr, look, moment.time, size);
    let tall = |layout: &pango::Layout| f64::from(layout.pixel_extents().1.height());
    // Where the person put the time, its middle there; else the style's place.
    let (cx, mut y) = match look.clock_at {
        Some((x, y)) => (x * width, y * height - (date.as_ref().map_or(0.0, tall) + tall(&time)) / 2.0),
        None => (cx, height * 0.16),
    };
    if let Some(date) = &date {
        set(cr, INK, 0.74);
        y += centred(cr, date, cx, y);
    }
    let w = width_of(&time);
    clock_ink(cr, look, INK, 1.0, cx - w / 2.0, cx + w / 2.0);
    y += centred(cr, &time, cx, y);
    let badged = badge(cr, look, Some(cx), 0.0, y + 6.0);
    if badged > 0.0 {
        y += badged + 6.0;
    }

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
    let cx = column(look, width, CARD_W / 2.0);
    let field_top = (height * 0.82).min(lowest_field(look, width, height));
    let lowest = field_top - if look.show_name { 120.0 } else { 24.0 };
    if look.notices_at.is_none() {
        let top = if look.clock_at.is_some() { height * 0.16 } else { y + 28.0 };
        cards(cr, look, &notice_lines(look, moment.facts), cx, top, lowest);
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

    if !look.message.is_empty() && look.message_at.is_none() {
        // In the top left: the corners at the foot are the shortcuts'.
        let message = message_layout(cr, look, 0.74, width * 0.4);
        at(cr, &message, 32.0, 28.0);
    }
    hits
}

fn poster(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) -> Vec<Hit> {
    let field_left = (width * 0.067).max(48.0);
    let mut parts = moment.time.split(':');
    let (hours, minutes, seconds) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next());
    let size = (height * 0.235).min(width * 0.15).min(212.0) * look.clock_scale;
    let figures = [clock_text(cr, look, hours, size), clock_text(cr, look, minutes, size)];
    // Where the person put the time, its middle there; else the style's place.
    let (left, mut y) = match look.clock_at {
        Some((x, y)) => {
            let wide = figures.iter().map(width_of).fold(0.0, f64::max);
            let tall = figures.iter().map(|layout| f64::from(layout.pixel_extents().1.height()) * 0.76).sum::<f64>()
                + if look.show_date { 30.0 } else { 0.0 };
            (x * width - wide / 2.0, y * height - tall / 2.0)
        }
        None => (field_left, height * 0.12),
    };
    if look.show_date {
        let (weekday, date) = moment.date.rsplit_once(' ').map_or((moment.date, ""), |(date, weekday)| (weekday, date));
        set(cr, look.accent, 1.0);
        let (w, h) = at(cr, &text(cr, look, weekday, 20.0, MEDIUM), left, y);
        set(cr, INK, 0.76);
        at(cr, &text(cr, look, date, 20.0, MEDIUM), left + w + 16.0, y);
        y += h + 6.0;
    }

    for (i, layout) in figures.iter().enumerate() {
        // The lines of figures overlap a little, as a poster sets them.
        let (ink, extent) = layout.pixel_extents();
        let x = left - size * 0.04;
        if look.clock_rainbow {
            clock_ink(cr, look, INK, 1.0, x, x + f64::from(extent.width()));
        } else {
            set(cr, if i == 0 { INK } else { look.clock_colour.unwrap_or(look.accent) }, 1.0);
        }
        cr.move_to(x, y);
        pangocairo::functions::show_layout(cr, layout);
        if let Some(seconds) = seconds.filter(|_| i == 1) {
            // Small, at the foot of the minutes.
            let small = clock_text(cr, look, seconds, size * 0.28);
            let (small_ink, _) = small.pixel_extents();
            let foot = y + f64::from(ink.y() + ink.height());
            at(cr, &small, x + f64::from(ink.x() + ink.width()) + 14.0, foot - f64::from(small_ink.y() + small_ink.height()));
        }
        y += f64::from(extent.height()) * 0.76;
    }
    let badged = badge(cr, look, None, left, y + 8.0);
    y += 40.0 + if badged > 0.0 { badged + 8.0 } else { 0.0 };

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
    let mut x = field_left;
    if look.show_name {
        picture(cr, look, x, field_top - 4.0, 44.0);
        x += 58.0;
    }
    field(cr, look, moment, x, field_top, 260.0, true);

    // The notices down the right.
    let lines = if look.notices_at.is_none() { notice_lines(look, moment.facts) } else { Vec::new() };
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

    if !look.message.is_empty() && look.message_at.is_none() {
        // Over the notices, on ink as they are, the wallpaper there being bare: the corners
        // at the foot are the shortcuts'.
        let message = message_layout(cr, look, 0.86, LIST - 24.0);
        let (_, extent) = message.pixel_extents();
        let h = (f64::from(extent.height()) + 12.0).max(30.0);
        let w = f64::from(extent.width()) + 24.0;
        let (x, y) = (width - (width * 0.05).max(40.0) - w, height * 0.12 - 12.0 - h);
        rounded(cr, x, y, w, h, 15.0);
        cr.set_source_rgba(0.09, 0.086, 0.094, 0.5 * look.message_opacity);
        let _ = cr.fill();
        set(cr, look.message_colour.unwrap_or(INK), 0.86 * look.message_opacity);
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
        + if look.show_name { 40.0 } else { 8.0 }
        + if look.clock_label.is_empty() && look.clock_badge.is_none() { 0.0 } else { 36.0 };
    let room = lowest_field(look, width, height) - height * 0.1 - below_dial;
    let radius = ((height * 0.24).min(width * 0.2).min(220.0) * look.clock_scale).min(room / 2.0).max(60.0);
    let field_cx = column(look, width, radius.max(FIELD_WIDTH / 2.0));
    let (cx, cy) = look.clock_at.map_or((field_cx, height * 0.1 + radius), |(x, y)| (x * width, y * height));

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
    const ROMAN: [&str; 12] = ["XII", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI"];
    if look.numerals != Numerals::None {
        set(cr, INK, 0.72);
        for hour in 0..12 {
            let figure = match look.numerals {
                Numerals::Roman => ROMAN[hour].to_string(),
                _ => if hour == 0 { "12".to_string() } else { hour.to_string() },
            };
            let layout = text(cr, look, &figure, radius * 0.1, MEDIUM);
            let angle = hour as f64 * PI / 6.0;
            let (x, y) = (cx + radius * 0.74 * angle.sin(), cy - radius * 0.74 * angle.cos());
            let h = f64::from(layout.pixel_extents().1.height());
            centred(cr, &layout, x, y - h / 2.0);
        }
        // Text leaves the current point where it was set; what is drawn next starts afresh.
        cr.new_path();
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
    // The hour hand in ink; the others in the clock's colour, or through the colours.
    let hand = |angle: f64, length: f64, line: f64, ink: bool| {
        if ink {
            set(cr, INK, 1.0);
        } else {
            clock_ink(cr, look, look.accent, 1.0, cx - radius, cx + radius);
        }
        cr.set_line_width(line);
        cr.move_to(cx - 0.08 * length * angle.sin(), cy + 0.08 * length * angle.cos());
        cr.line_to(cx + length * angle.sin(), cy - length * angle.cos());
        let _ = cr.stroke();
    };
    let accent = look.clock_colour.unwrap_or(look.accent);
    let (hour_line, minute_line) = match look.hands {
        Hands::Classic => (9.0, 5.0),
        Hands::Thin => (4.0, 2.5),
        Hands::Bold => (13.0, 8.0),
    };
    hand(hour * PI / 6.0, radius * 0.5, hour_line, true);
    hand(minute * PI / 30.0, radius * 0.8, minute_line, false);
    if look.clock_seconds {
        hand(f64::from(moment.second) * PI / 30.0, radius * 0.86, 1.6, false);
    }
    set(cr, accent, 1.0);
    cr.arc(cx, cy, 7.0, 0.0, 2.0 * PI);
    let _ = cr.fill();
    set(cr, PAPER, 1.0);
    cr.arc(cx, cy, 2.5, 0.0, 2.0 * PI);
    let _ = cr.fill();

    let mut below = cy + radius + 24.0;
    let badged = badge(cr, look, Some(cx), 0.0, below - 8.0);
    if badged > 0.0 {
        below += badged + 4.0;
    }
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

    let below = if look.clock_at.is_some() { 0.0 } else { below };
    let field_top = (height * 0.8).max(below + 70.0).min(lowest_field(look, width, height));
    if look.show_name {
        set(cr, INK, 1.0);
        let name = text(cr, look, &look.name, 15.0, MEDIUM);
        let h = f64::from(name.pixel_extents().1.height());
        centred(cr, &name, field_cx, field_top - 12.0 - h);
    }
    field(cr, look, moment, field_cx - FIELD_WIDTH / 2.0, field_top, FIELD_WIDTH, false);

    // The notices as pills in the top left, the message in the top right.
    let mut x = 32.0;
    let lines = if look.notices_at.is_none() { notice_lines(look, moment.facts) } else { Vec::new() };
    for (app, summary) in lines {
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
    if !look.message.is_empty() && look.message_at.is_none() {
        let message = message_layout(cr, look, 0.6, width * 0.4);
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
    let bright = look.always_on_brightness;
    if look.always_on_stickers {
        stickers(cr, look, width, height, 0.5 * bright);
    }
    let (cx, mut y) = (width / 2.0 + drift(moment.minute, 80.0), height * 0.3 + drift(moment.minute / 7, 60.0));
    // Without seconds: they would keep it awake.
    let time = moment.time.splitn(3, ':').take(2).collect::<Vec<_>>().join(":");
    let time = clock_text(cr, look, &time, (height * 0.12).min(110.0));
    let w = width_of(&time);
    clock_ink(cr, look, INK, 0.62 * bright, cx - w / 2.0, cx + w / 2.0);
    y += centred(cr, &time, cx, y);
    if look.show_date {
        set(cr, INK, 0.42 * bright);
        y += 4.0 + centred(cr, &text(cr, look, moment.date, 17.0, MEDIUM), cx, y + 4.0);
    }
    if look.always_on_widgets {
        let found: Vec<String> = widgets(look, moment.facts).iter().map(widget_words).collect();
        if !found.is_empty() {
            set(cr, INK, 0.36 * bright);
            let layout = line(cr, look, &found.join("　·　"), 15.0, MEDIUM, width * 0.6);
            centred(cr, &layout, cx, y + 12.0);
        }
    }
    if moment.finger {
        // Where a finger wakes and unlocks it, as a phone's always-on display shows.
        set(cr, INK, 0.36 * bright);
        fingerprint(cr, width / 2.0, height * 0.84, 16.0);
        finger_pulse(cr, look, moment, width / 2.0, height * 0.84, 16.0);
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

/// A number from 0 to 1 that looks random but is the same for the same `i` and `j`.
fn noise(i: u32, j: u32) -> f64 {
    let mut x = i.wrapping_mul(0x9E37_79B1) ^ j.wrapping_mul(0x85EB_CA77);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    f64::from(x % 10_000) / 10_000.0
}

/// The locked layer `t` of the way gone, from 0 to 1, over `under`, as the person's effect
/// has it; its colour, where it has one, the wallpaper's `accent` unless they chose one.
/// Lifted, its bottom edge fades, so it reads as a sheet taken off rather than a curtain.
/// In pixels.
pub fn reveal(cr: &Context, unlock: &Unlock, accent: Rgb, under: &ImageSurface, layer: &ImageSurface, t: f64, scale: f64) {
    let _ = cr.set_source_surface(under, 0.0, 0.0);
    let _ = cr.paint();
    let (width, height) = (f64::from(layer.width()), f64::from(layer.height()));
    let t = t.clamp(0.0, 1.0);
    let e = match unlock.easing {
        Easing::Out => 1.0 - (1.0 - t).powi(3),
        Easing::InOut if t < 0.5 => 4.0 * t.powi(3),
        Easing::InOut => 1.0 - (2.0 - 2.0 * t).powi(3) / 2.0,
        Easing::Linear => t,
    };
    let colour = unlock.colour.unwrap_or(accent);
    let (cx, cy) = (width / 2.0, height / 2.0);
    let reach = cx.hypot(cy) + 2.0;
    let across = matches!(unlock.direction, Direction::Left | Direction::Right);
    let sign = if matches!(unlock.direction, Direction::Left | Direction::Up) { -1.0 } else { 1.0 };
    // The layer drawn through the current path only.
    let layer_through_path = |alpha: f64| {
        cr.save().ok();
        cr.clip();
        let _ = cr.set_source_surface(layer, 0.0, 0.0);
        let _ = cr.paint_with_alpha(alpha);
        cr.restore().ok();
    };
    // The layer drawn about the middle, turned, stretched and as opaque as given.
    let layer_about_middle = |turn: f64, sx: f64, sy: f64, alpha: f64| {
        cr.save().ok();
        cr.translate(cx, cy);
        cr.rotate(turn);
        cr.scale(sx, sy);
        cr.translate(-cx, -cy);
        let _ = cr.set_source_surface(layer, 0.0, 0.0);
        let _ = cr.paint_with_alpha(alpha);
        cr.restore().ok();
    };
    // Strips across the way the effect goes: their first and last pixel.
    let strips = |count: u32| {
        let span = if across { height } else { width };
        (0..count).map(move |i| {
            let from = (span * f64::from(i) / f64::from(count)).floor();
            let to = (span * f64::from(i + 1) / f64::from(count)).ceil();
            (i, from, to)
        })
    };
    let strip_rect = |from: f64, to: f64| {
        if across {
            cr.rectangle(0.0, from, width, to - from);
        } else {
            cr.rectangle(from, 0.0, to - from, height);
        }
    };
    match unlock.effect {
        Effect::Lift => {
            let feather = FEATHER * scale;
            let offset = -e * (height + feather);
            let fade = LinearGradient::new(0.0, offset + height - feather, 0.0, offset + height);
            fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 1.0);
            fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
            let _ = cr.set_source_surface(layer, 0.0, offset);
            let _ = cr.mask(&fade);
        }
        Effect::Fade => layer_about_middle(0.0, 1.0, 1.0, 1.0 - e),
        Effect::Slide => {
            let (x, y) = if across { (sign * e * width, 0.0) } else { (0.0, sign * e * height) };
            let _ = cr.set_source_surface(layer, x, y);
            let _ = cr.paint();
        }
        Effect::Expand => layer_about_middle(0.0, 1.0 + 0.16 * e, 1.0 + 0.16 * e, 1.0 - e),
        Effect::Warp => layer_about_middle(0.0, 1.0 + e, 1.0 - 0.9 * e, 1.0 - e),
        Effect::Swirl => layer_about_middle(e * PI / 2.0, 1.0 - 0.6 * e, 1.0 - 0.6 * e, 1.0 - e),
        Effect::Spread | Effect::Ripple => {
            let radius = e * reach;
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            cr.rectangle(0.0, 0.0, width, height);
            cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
            layer_through_path(1.0);
            cr.set_fill_rule(cairo::FillRule::Winding);
            if radius < 1.0 {
                return;
            }
            set(cr, colour, 1.0 - e);
            if unlock.effect == Effect::Spread {
                cr.set_line_width(6.0 * scale);
                cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
                let _ = cr.stroke();
            } else {
                cr.set_line_width(2.5 * scale);
                for share in [1.0, 0.78, 0.56] {
                    cr.new_sub_path();
                    cr.arc(cx, cy, radius * share, 0.0, 2.0 * PI);
                }
                let _ = cr.stroke();
            }
        }
        Effect::Wave => {
            for (i, from, to) in strips(24) {
                let local = ((e - 0.5 * f64::from(i) / 23.0) / 0.5).clamp(0.0, 1.0);
                let shift = sign * local * if across { width } else { height };
                cr.save().ok();
                strip_rect(from, to);
                cr.clip();
                let (x, y) = if across { (shift, 0.0) } else { (0.0, shift) };
                let _ = cr.set_source_surface(layer, x, y);
                let _ = cr.paint();
                cr.restore().ok();
            }
        }
        Effect::Mosaic => {
            const COLUMNS: u32 = 16;
            const ROWS: u32 = 10;
            let (w, h) = (width / f64::from(COLUMNS), height / f64::from(ROWS));
            for row in 0..ROWS {
                for column in 0..COLUMNS {
                    if 0.05 + 0.9 * noise(column, row) > e {
                        cr.rectangle((f64::from(column) * w).floor(), (f64::from(row) * h).floor(), w.ceil() + 1.0, h.ceil() + 1.0);
                    }
                }
            }
            layer_through_path(1.0);
        }
        Effect::Wiper => {
            // What the hand has not yet swept, clockwise from the top, or the other way.
            let top = -PI / 2.0;
            cr.move_to(cx, cy);
            if sign < 0.0 {
                cr.arc_negative(cx, cy, reach, top - e * 2.0 * PI, top - 2.0 * PI);
            } else {
                cr.arc(cx, cy, reach, top + e * 2.0 * PI, top + 2.0 * PI);
            }
            cr.close_path();
            layer_through_path(1.0);
        }
        Effect::Paint => {
            // Each band brushed off from one side to the other, a little after the last.
            const BANDS: u32 = 6;
            let span = if across { width } else { height };
            let brush = 14.0 * scale;
            for (i, from, to) in strips(BANDS) {
                let done = ((e - 0.4 * f64::from(i) / f64::from(BANDS - 1)) / 0.6).clamp(0.0, 1.0);
                // Where the brush is along the band.
                let edge = if sign > 0.0 { done * span } else { (1.0 - done) * span };
                let (start, end) = if sign > 0.0 { (edge, span) } else { (0.0, edge) };
                let rect = |a: f64, b: f64| {
                    if across {
                        cr.rectangle(a, from, b - a, to - from);
                    } else {
                        cr.rectangle(from, a, to - from, b - a);
                    }
                };
                rect(start, end);
                layer_through_path(1.0);
                if done > 0.0 && done < 1.0 {
                    set(cr, colour, 1.0);
                    rect(edge - brush / 2.0, edge + brush / 2.0);
                    let _ = cr.fill();
                }
            }
        }
        Effect::Glitch => {
            // Strips jumping aside, to new places a dozen times on the way, as it fades.
            let step = (e * 12.0).floor() as u32;
            let tint = (e * PI).sin() * 0.35;
            for (i, from, to) in strips(18) {
                let jump = (noise(i, step) - 0.5) * 2.0 * e * 0.3 * if across { width } else { height };
                cr.save().ok();
                strip_rect(from, to);
                cr.clip();
                let (x, y) = if across { (jump, 0.0) } else { (0.0, jump) };
                let _ = cr.set_source_surface(layer, x, y);
                let _ = cr.paint_with_alpha(1.0 - e);
                if noise(step, i) < 0.2 {
                    set(cr, colour, tint);
                    let _ = cr.paint();
                }
                cr.restore().ok();
            }
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
    use super::{from_hsl, hsl, reveal, shake, Direction, Easing, Effect, Unlock};
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
        let effects = [
            Effect::Lift,
            Effect::Fade,
            Effect::Slide,
            Effect::Expand,
            Effect::Spread,
            Effect::Wave,
            Effect::Warp,
            Effect::Ripple,
            Effect::Mosaic,
            Effect::Wiper,
            Effect::Paint,
            Effect::Glitch,
            Effect::Swirl,
        ];
        for effect in effects {
            for direction in [Direction::Left, Direction::Right, Direction::Up, Direction::Down] {
                for easing in [Easing::Out, Easing::InOut, Easing::Linear] {
                    // A grey of its own, so its edges and brushes show if they are left behind.
                    let unlock = Unlock { effect, colour: Some((0.5, 0.5, 0.5)), direction, easing };
                    for (t, expected) in [(0.0, 255), (1.0, 0)] {
                        let mut target = ImageSurface::create(Format::ARgb32, 64, 600).unwrap();
                        let cr = Context::new(&target).unwrap();
                        reveal(&cr, &unlock, (0.5, 0.5, 0.5), &under, &layer, t, 1.0);
                        drop(cr);
                        // The middle pixel's blue byte, white from the lock and black from the desktop.
                        let stride = target.stride() as usize;
                        let data = target.data().unwrap();
                        assert_eq!(data[300 * stride + 32 * 4], expected, "{effect:?} {direction:?} {easing:?} at {t}");
                    }
                }
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
