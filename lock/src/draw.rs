//! What the lock looks like, in one of three styles: the wallpaper under ink; the time, the
//! date and the widgets; the notices that came in while locked; the user's name and a field
//! for the password. Colours are Nekoland's dark paper and ink, one taken from the wallpaper,
//! and its one red for a wrong password.

use std::f64::consts::PI;

use cairo::{Context, Format, ImageSurface, LinearGradient, RadialGradient};
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
    pub battery: bool,
    pub playing: bool,
    pub notices: Notices,
    /// What the person asked to be shown while locked, such as whom to call.
    pub message: String,
    pub show_name: bool,
    pub picture: Option<Pixbuf>,
}

pub type Rgb = (f64, f64, f64);

/// What the widgets show.
#[derive(Default, PartialEq, Clone)]
pub struct Facts {
    /// The charge, and whether it is charging.
    pub battery: Option<(u8, bool)>,
    /// The title and the artist of what plays.
    pub playing: Option<(String, String)>,
    /// The program and the summary of each notification that came in while locked.
    pub notices: Vec<(String, String)>,
}

/// What is on the lock at a moment.
pub struct Moment<'a> {
    pub time: &'a str,
    pub date: &'a str,
    /// The hour, 0 to 23, and the minute, for the hands.
    pub hour: u32,
    pub minute: u32,
    pub facts: &'a Facts,
    /// How many characters have been typed.
    pub typed: usize,
    pub checking: bool,
    pub caps_lock: bool,
    /// What went wrong with the last password, and how far into the shake it is, from 0 to 1.
    pub error: Option<(&'a str, f64)>,
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
pub fn veil(cr: &Context, style: Style, width: f64, height: f64) {
    match style {
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
    let layout = pangocairo::functions::create_layout(cr);
    // As Nekoland draws text: unhinted, with grey edges.
    if let Ok(mut options) = cairo::FontOptions::new() {
        options.set_antialias(cairo::Antialias::Gray);
        options.set_hint_style(cairo::HintStyle::None);
        options.set_hint_metrics(cairo::HintMetrics::Off);
        pangocairo::functions::context_set_font_options(&layout.context(), Some(&options));
        layout.context_changed();
    }
    let mut font = pango::FontDescription::from_string(&look.font);
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
        let room = ((width - 36.0) / 12.0) as usize;
        set(cr, INK, if moment.checking { 0.5 } else { 1.0 });
        for i in 0..moment.typed.min(room) {
            cr.arc(x + 20.0 + 12.0 * i as f64, middle, 3.5, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
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

/// The widgets the person asked for, as there is something to show in them.
fn widgets<'a>(look: &Look, facts: &'a Facts) -> (Option<(u8, bool)>, Option<&'a (String, String)>) {
    (
        facts.battery.filter(|_| look.battery),
        facts.playing.as_ref().filter(|_| look.playing),
    )
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

/// The words, the widgets and the field, in points, on a surface `width` by `height` points.
pub fn words(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    match look.style {
        Style::Quiet => quiet(cr, look, moment, width, height),
        Style::Poster => poster(cr, look, moment, width, height),
        Style::Dial => dial(cr, look, moment, width, height),
    }
}

fn quiet(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    let cx = width / 2.0;
    let mut y = height * 0.16;
    set(cr, INK, 0.74);
    y += centred(cr, &text(cr, look, moment.date, 19.0, MEDIUM), cx, y);
    set(cr, look.clock_colour.unwrap_or(INK), 1.0);
    let clock = text(cr, look, moment.time, (width * 0.105).min(height * 0.17).min(150.0), look.clock_weight);
    y += centred(cr, &clock, cx, y);

    // The widgets in a row of pills under the time.
    let (battery, playing) = widgets(look, moment.facts);
    let mut pills: Vec<(pango::Layout, f64, Option<(u8, bool)>)> = Vec::new();
    if let Some(battery) = battery {
        let layout = text(cr, look, &battery_words(battery), 14.0, MEDIUM);
        let w = f64::from(layout.pixel_extents().1.width()) + 56.0;
        pills.push((layout, w, Some(battery)));
    }
    if let Some((title, artist)) = playing {
        let words = if artist.is_empty() { title.clone() } else { format!("{title} · {artist}") };
        let layout = line(cr, look, &words, 14.0, MEDIUM, 260.0);
        let w = f64::from(layout.pixel_extents().1.width()) + 56.0;
        pills.push((layout, w, None));
    }
    if !pills.is_empty() {
        const PILL: f64 = 40.0;
        let total: f64 = pills.iter().map(|(_, w, _)| w).sum::<f64>() + 10.0 * (pills.len() - 1) as f64;
        let mut x = cx - total / 2.0;
        let top = y + 22.0;
        for (layout, w, battery) in &pills {
            glass(cr, x, top, *w, PILL, PILL / 2.0, 0.13);
            match battery {
                Some((percent, _)) => ring(cr, x + 24.0, top + PILL / 2.0, 8.0, 3.0, f64::from(*percent) / 100.0, INK),
                // A note, for what plays.
                None => {
                    set(cr, INK, 0.9);
                    cr.arc(x + 21.0, top + 25.0, 3.5, 0.0, 2.0 * PI);
                    let _ = cr.fill();
                    cr.rectangle(x + 23.5, top + 12.0, 1.6, 13.0);
                    let _ = cr.fill();
                }
            }
            set(cr, INK, 1.0);
            let (_, extent) = layout.pixel_extents();
            at(cr, layout, x + 40.0, top + (PILL - f64::from(extent.height())) / 2.0);
            x += w + 10.0;
        }
        y = top + PILL;
    }

    // The notices, as cards stacked under them, as many as there is room for above the field.
    let field_top = height * 0.82;
    let lowest = field_top - if look.show_name { 120.0 } else { 24.0 };
    let mut top = y + 28.0;
    for (app, summary) in notice_lines(look, moment.facts) {
        const CARD_W: f64 = 380.0;
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
        let message = line(cr, look, &look.message, 13.0, MEDIUM, width * 0.4);
        let h = f64::from(message.pixel_extents().1.height());
        at(cr, &message, 32.0, height - 28.0 - h);
    }
}

fn poster(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    let left = (width * 0.067).max(48.0);
    let mut y = height * 0.12;
    let (weekday, date) = moment.date.rsplit_once(' ').map_or((moment.date, ""), |(date, weekday)| (weekday, date));
    set(cr, look.accent, 1.0);
    let (w, h) = at(cr, &text(cr, look, weekday, 20.0, MEDIUM), left, y);
    set(cr, INK, 0.76);
    at(cr, &text(cr, look, date, 20.0, MEDIUM), left + w + 16.0, y);
    y += h + 6.0;

    let (hours, minutes) = moment.time.split_once(':').unwrap_or((moment.time, ""));
    let size = (height * 0.235).min(width * 0.15).min(212.0);
    for (i, part) in [hours, minutes].into_iter().enumerate() {
        let colour = if i == 0 { INK } else { look.clock_colour.unwrap_or(look.accent) };
        set(cr, colour, 1.0);
        let layout = text(cr, look, part, size, look.clock_weight);
        // The lines of figures overlap a little, as a poster sets them.
        let (_, extent) = layout.pixel_extents();
        cr.move_to(left - size * 0.04, y);
        pangocairo::functions::show_layout(cr, &layout);
        y += f64::from(extent.height()) * 0.76;
    }
    y += 40.0;

    let (battery, playing) = widgets(look, moment.facts);
    let mut facts: Vec<(&str, String)> = Vec::new();
    if let Some(battery) = battery {
        facts.push(("電池", battery_words(battery)));
    }
    if let Some((title, artist)) = playing {
        facts.push(("播放", if artist.is_empty() { title.clone() } else { format!("{title} · {artist}") }));
    }
    for (label, value) in facts {
        set(cr, INK, 0.52);
        let (_, h) = at(cr, &text(cr, look, label, 15.0, MEDIUM), left, y);
        set(cr, INK, 1.0);
        at(cr, &line(cr, look, &value, 15.0, MEDIUM, 360.0), left + 52.0, y);
        y += h + 10.0;
    }

    // The field at the foot of the column, the person's picture before it.
    let field_top = height - (height * 0.1).max(56.0) - FIELD_HEIGHT;
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
        let message = line(cr, look, &look.message, 13.0, MEDIUM, LIST);
        let (_, extent) = message.pixel_extents();
        at(cr, &message, width - (width * 0.05).max(40.0) - f64::from(extent.width()), height - 40.0 - f64::from(extent.height()));
    }
}

fn dial(cr: &Context, look: &Look, moment: &Moment, width: f64, height: f64) {
    let radius = (height * 0.24).min(width * 0.2).min(220.0);
    let (cx, cy) = (width / 2.0, height * 0.1 + radius);

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

    // The widgets as small dials: the battery above the middle, what plays below it.
    let (battery, playing) = widgets(look, moment.facts);
    if let Some((percent, charging)) = battery {
        let (bx, by) = (cx, cy - radius * 0.42);
        ring(cr, bx, by, radius * 0.17, 5.0, f64::from(percent) / 100.0, if charging { look.accent } else { INK });
        set(cr, INK, 1.0);
        let layout = text(cr, look, &format!("{percent}%"), 16.0, MEDIUM);
        let (_, extent) = layout.pixel_extents();
        centred(cr, &layout, bx, by - f64::from(extent.height()) / 2.0);
    }
    if let Some((title, _)) = playing {
        let by = cy + radius * 0.36;
        set(cr, INK, 0.5);
        let label = text(cr, look, "播放中", 12.0, MEDIUM);
        let h = centred(cr, &label, cx, by);
        set(cr, INK, 1.0);
        let name = line(cr, look, title, 16.0, MEDIUM, radius * 0.9);
        let (_, extent) = name.pixel_extents();
        at(cr, &name, cx - f64::from(extent.width()) / 2.0, by + h + 2.0);
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
    hand(hour * PI / 6.0, radius * 0.5, 9.0, INK);
    hand(minute * PI / 30.0, radius * 0.8, 5.0, look.clock_colour.unwrap_or(look.accent));
    set(cr, look.clock_colour.unwrap_or(look.accent), 1.0);
    cr.arc(cx, cy, 7.0, 0.0, 2.0 * PI);
    let _ = cr.fill();
    set(cr, PAPER, 1.0);
    cr.arc(cx, cy, 2.5, 0.0, 2.0 * PI);
    let _ = cr.fill();

    set(cr, INK, 0.76);
    let date_top = cy + radius + 24.0;
    let date_height = centred(cr, &text(cr, look, moment.date, 18.0, MEDIUM), cx, date_top);

    let field_top = (height * 0.8).max(date_top + date_height + 70.0);
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

/// The locked layer lifted `lifted` of the way, from 0 to 1, over `under`; its bottom edge
/// fades, so it reads as a sheet taken off rather than a curtain. In pixels.
pub fn lift(cr: &Context, under: &ImageSurface, layer: &ImageSurface, lifted: f64, scale: f64) {
    let _ = cr.set_source_surface(under, 0.0, 0.0);
    let _ = cr.paint();
    let height = f64::from(layer.height());
    let feather = FEATHER * scale;
    let eased = 1.0 - (1.0 - lifted).powi(3);
    let offset = -eased * (height + feather);
    let fade = LinearGradient::new(0.0, offset + height - feather, 0.0, offset + height);
    fade.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 1.0);
    fade.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
    let _ = cr.set_source_surface(layer, 0.0, offset);
    let _ = cr.mask(&fade);
}

/// A picture as Cairo draws it: premultiplied, in native byte order.
fn pixbuf_surface(pixbuf: &Pixbuf) -> Option<ImageSurface> {
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
    use super::{from_hsl, hsl, shake};

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
