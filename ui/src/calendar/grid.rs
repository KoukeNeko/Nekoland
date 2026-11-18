//! The month and the time grids, drawn by hand: widgets could not put an event's colour on
//! a cell without a stylesheet. What is drawn is kept as rectangles too, so that a click is
//! looked up among them.

use calendar_data::date::{Date, WEEKDAY_NAMES};
#[allow(deprecated)]
use gtk::{cairo, gdk, pango, prelude::{StyleContextExt, WidgetExt}};

pub type Rgb = (f64, f64, f64);

/// The scale all the spacing here is on, as in the control center and the clock's panel.
const PADDING: f64 = 12.0;
const GAP: f64 = 8.0;
const INSET: f64 = 4.0;

const WEEKDAY_ROW: f64 = 32.0;
const DAY_NUMBER_SIZE: f64 = 24.0;
const CHIP_HEIGHT: f64 = 20.0;
const CHIP_STEP: f64 = CHIP_HEIGHT + INSET;
const CHIP_RADIUS: f64 = 5.0;
const CELL_HEAD: f64 = INSET + DAY_NUMBER_SIZE + INSET;

const HOUR_HEIGHT: f64 = 48.0;
const GUTTER: f64 = 56.0;
const DAY_HEAD: f64 = 72.0;
const ALL_DAY_STEP: f64 = 28.0;
const MIN_BLOCK: f64 = 20.0;
const DEFAULT_MINUTES: u32 = 60;
const MINUTES_IN_HOUR: f64 = 60.0;
const HOURS_IN_DAY: u32 = 24;
const BAR_WIDTH: f64 = 3.0;
const NOW_DOT: f64 = 4.0;
const CASCADE_STEP: f64 = 16.0;

const TEXT_SMALL: f64 = 9.0;
const TEXT_BODY: f64 = 10.5;
const TEXT_NUMBER: f64 = 12.0;

const LINE_ALPHA: f64 = 0.12;
const DIM_ALPHA: f64 = 0.45;
const SOFT_ALPHA: f64 = 0.62;
const BLOCK_ALPHA: f64 = 0.22;
const SELECTED_ALPHA: f64 = 0.10;
const OUTSIDE_SHADE: f64 = 0.04;
const BRIGHT: f64 = 0.6;
const HOLIDAY: Rgb = (0.85, 0.25, 0.18);
const FALLBACK_ACCENT: (f32, f32, f32) = (0.21, 0.52, 0.89);
const FALLBACK_DARK_PAGE: Rgb = (0.12, 0.12, 0.12);
const FALLBACK_LIGHT_PAGE: Rgb = (1.0, 1.0, 1.0);

/// One event on one day, as the grids draw it.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub date: Date,
    pub title: String,
    pub color: Rgb,
    pub start: Option<(u32, u32)>,
    pub end: Option<(u32, u32)>,
    pub done: bool,
}

/// A day, what is on it, and the line that says what day it is.
#[derive(Clone, Debug)]
pub struct Day {
    pub date: Date,
    pub items: Vec<Item>,
    pub note: String,
    pub holiday: bool,
}

/// How the theme says things are drawn.
pub struct Look {
    pub ink: gdk::RGBA,
    pub accent: gdk::RGBA,
    /// What the grids are drawn on.
    pub page: Rgb,
    pub dark: bool,
}

impl Look {
    pub fn of(widget: &impl WidgetExt) -> Self {
        #[allow(deprecated)]
        let accent = widget
            .style_context()
            .lookup_color("accent_bg_color")
            .unwrap_or_else(|| gdk::RGBA::new(FALLBACK_ACCENT.0, FALLBACK_ACCENT.1, FALLBACK_ACCENT.2, 1.0));
        let dark = adw::StyleManager::default().is_dark();
        #[allow(deprecated)]
        let page = widget.style_context().lookup_color("view_bg_color").map_or_else(
            || if dark { FALLBACK_DARK_PAGE } else { FALLBACK_LIGHT_PAGE },
            |color| (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue())),
        );
        Self { ink: widget.color(), accent, page, dark }
    }
}

#[derive(Clone, Debug)]
pub enum Target {
    Day(Date),
    Item(Item),
}

pub struct Hit {
    pub rect: (f64, f64, f64, f64),
    pub target: Target,
}

/// What is under a point; the events come after the days they are on, so they are found first.
pub fn hit_at(hits: &[Hit], x: f64, y: f64) -> Option<&Target> {
    hits.iter()
        .rev()
        .find(|hit| {
            let (left, top, width, height) = hit.rect;
            x >= left && x < left + width && y >= top && y < top + height
        })
        .map(|hit| &hit.target)
}

fn set_color(cr: &cairo::Context, color: &gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()), alpha);
}

fn rounded(cr: &cairo::Context, (x, y, width, height): (f64, f64, f64, f64), radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + radius, y + radius, radius, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

fn is_bright((red, green, blue): Rgb) -> bool {
    0.299 * red + 0.587 * green + 0.114 * blue > BRIGHT
}

#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Right,
    Center,
}

/// A line of text, as wide as `width` at most, with `…` where it is cut. Returns its height.
fn put(
    cr: &cairo::Context,
    layout: &pango::Layout,
    text: &str,
    (x, y, width): (f64, f64, f64),
    (points, bold, align): (f64, bool, Align),
) -> f64 {
    let mut font = layout.context().font_description().unwrap_or_default();
    font.set_size((points * f64::from(pango::SCALE)) as i32);
    font.set_weight(if bold { pango::Weight::Bold } else { pango::Weight::Normal });
    layout.set_font_description(Some(&font));
    layout.set_text(text);
    layout.set_width((width.max(0.0) * f64::from(pango::SCALE)) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_alignment(match align {
        Align::Left => pango::Alignment::Left,
        Align::Right => pango::Alignment::Right,
        Align::Center => pango::Alignment::Center,
    });
    let (_, height) = layout.pixel_size();
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, layout);
    f64::from(height)
}

fn clock_text((hour, minute): (u32, u32)) -> String {
    format!("{hour:02}:{minute:02}")
}

fn chip_text(item: &Item) -> String {
    match item.start {
        Some(start) => format!("{} {}", clock_text(start), item.title),
        None => item.title.clone(),
    }
}

/// The number of a day, on a disc of the accent when it is today.
fn day_number(cr: &cairo::Context, layout: &pango::Layout, look: &Look, (x, y): (f64, f64), day: Date, today: bool) {
    let disc = DAY_NUMBER_SIZE;
    if today {
        cr.arc(x + disc / 2.0, y + disc / 2.0, disc / 2.0, 0.0, std::f64::consts::TAU);
        set_color(cr, &look.accent, 1.0);
        let _ = cr.fill();
        cr.set_source_rgb(1.0, 1.0, 1.0);
    } else {
        set_color(cr, &look.ink, 1.0);
    }
    let text = day.day.to_string();
    let height = {
        let mut font = layout.context().font_description().unwrap_or_default();
        font.set_size((TEXT_NUMBER * f64::from(pango::SCALE)) as i32);
        font.set_weight(pango::Weight::Bold);
        layout.set_font_description(Some(&font));
        layout.set_text(&text);
        layout.set_width(-1);
        layout.set_ellipsize(pango::EllipsizeMode::None);
        f64::from(layout.pixel_size().1)
    };
    let (width, _) = layout.pixel_size();
    cr.move_to(x + (disc - f64::from(width)) / 2.0, y + (disc - height) / 2.0);
    pangocairo::functions::show_layout(cr, layout);
}

/// The weeks of a month, a cell to a day: its number, its lunar day or holiday, and what
/// is on it as bars of the calendars' colours.
pub fn draw_month(
    cr: &cairo::Context,
    layout: &pango::Layout,
    (width, height): (f64, f64),
    days: &[Day],
    (chosen, today, month): (Date, Date, u32),
    look: &Look,
    hits: &mut Vec<Hit>,
) {
    hits.clear();
    let weeks = days.len() / 7;
    if weeks == 0 {
        return;
    }
    let (cell_width, cell_height) = (width / 7.0, (height - WEEKDAY_ROW) / weeks as f64);
    for (index, name) in WEEKDAY_NAMES.iter().enumerate() {
        set_color(cr, &look.ink, DIM_ALPHA);
        put(
            cr,
            layout,
            name,
            (index as f64 * cell_width, (WEEKDAY_ROW - TEXT_SMALL * 1.6) / 2.0, cell_width),
            (TEXT_SMALL + 1.0, false, Align::Center),
        );
    }
    for (index, day) in days.iter().enumerate() {
        let (column, row) = ((index % 7) as f64, (index / 7) as f64);
        let (x, y) = (column * cell_width, WEEKDAY_ROW + row * cell_height);
        let outside = day.date.month != month;
        if outside {
            set_color(cr, &look.ink, OUTSIDE_SHADE);
            cr.rectangle(x, y, cell_width, cell_height);
            let _ = cr.fill();
        }
        if day.date == chosen {
            set_color(cr, &look.accent, SELECTED_ALPHA);
            cr.rectangle(x, y, cell_width, cell_height);
            let _ = cr.fill();
        }
        hits.push(Hit { rect: (x, y, cell_width, cell_height), target: Target::Day(day.date) });

        day_number(cr, layout, look, (x + GAP, y + INSET), day.date, day.date == today);
        if !day.note.is_empty() {
            if day.holiday {
                cr.set_source_rgb(HOLIDAY.0, HOLIDAY.1, HOLIDAY.2);
            } else {
                set_color(cr, &look.ink, if outside { DIM_ALPHA * 0.7 } else { DIM_ALPHA + 0.1 });
            }
            put(
                cr,
                layout,
                &day.note,
                (x + GAP + DAY_NUMBER_SIZE + INSET, y + INSET + (DAY_NUMBER_SIZE - TEXT_SMALL * 1.6) / 2.0, cell_width - GAP * 2.0 - DAY_NUMBER_SIZE - INSET),
                (TEXT_SMALL, false, Align::Right),
            );
        }

        let room = ((cell_height - CELL_HEAD - INSET) / CHIP_STEP).floor().max(0.0) as usize;
        let overflow = day.items.len() > room;
        let shown = if overflow { room.saturating_sub(1) } else { day.items.len() };
        for (slot, item) in day.items.iter().take(shown).enumerate() {
            let rect = (x + INSET, y + CELL_HEAD + slot as f64 * CHIP_STEP, cell_width - INSET * 2.0, CHIP_HEIGHT);
            draw_chip(cr, layout, rect, item);
            hits.push(Hit { rect, target: Target::Item(item.clone()) });
        }
        if overflow && room > 0 {
            set_color(cr, &look.ink, SOFT_ALPHA);
            put(
                cr,
                layout,
                &format!("還有 {} 項", day.items.len() - shown),
                (x + GAP, y + CELL_HEAD + shown as f64 * CHIP_STEP + 2.0, cell_width - GAP * 2.0),
                (TEXT_SMALL, false, Align::Left),
            );
        }
    }
    // The lines between days, drawn over the fills.
    set_color(cr, &look.ink, LINE_ALPHA);
    cr.set_line_width(1.0);
    for row in 0..=weeks {
        let y = (WEEKDAY_ROW + row as f64 * cell_height).round() + 0.5;
        cr.move_to(0.0, y);
        cr.line_to(width, y);
    }
    for column in 1..7 {
        let x = (column as f64 * cell_width).round() + 0.5;
        cr.move_to(x, WEEKDAY_ROW);
        cr.line_to(x, height);
    }
    let _ = cr.stroke();
    if let Some(index) = days.iter().position(|day| day.date == chosen) {
        let (x, y) = ((index % 7) as f64 * cell_width, WEEKDAY_ROW + (index / 7) as f64 * cell_height);
        set_color(cr, &look.accent, 1.0);
        cr.set_line_width(2.0);
        cr.rectangle(x + 1.0, y + 1.0, cell_width - 2.0, cell_height - 2.0);
        let _ = cr.stroke();
    }
}

/// An event as a bar of its calendar's colour, with its title in the one that reads on it.
fn draw_chip(cr: &cairo::Context, layout: &pango::Layout, rect: (f64, f64, f64, f64), item: &Item) {
    let (x, y, width, height) = rect;
    rounded(cr, rect, CHIP_RADIUS);
    cr.set_source_rgba(item.color.0, item.color.1, item.color.2, if item.done { 0.45 } else { 1.0 });
    let _ = cr.fill();
    if is_bright(item.color) {
        cr.set_source_rgb(0.1, 0.1, 0.1);
    } else {
        cr.set_source_rgb(1.0, 1.0, 1.0);
    }
    put(cr, layout, &chip_text(item), (x + GAP, y + (height - TEXT_BODY * 1.5) / 2.0, width - GAP * 2.0), (TEXT_SMALL + 1.0, false, Align::Left));
}

/// The two parts of the time grid: the days' heads with the events that take the day, which
/// stay where they are, and the hours under them, which scroll.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Head,
    Body,
}

/// How tall the heads are for the days they are over.
pub fn head_height(days: &[Day]) -> f64 {
    let all_day = days.iter().map(|day| day.items.iter().filter(|item| item.start.is_none()).count()).max().unwrap_or(0);
    DAY_HEAD + GAP + all_day as f64 * ALL_DAY_STEP + if all_day > 0 { GAP } else { 0.0 }
}

/// How tall the hours are.
pub fn body_height() -> f64 {
    f64::from(HOURS_IN_DAY) * HOUR_HEIGHT + PADDING
}

/// Where an hour is, from the top of the hours.
pub fn hour_y(hour: f64) -> f64 {
    hour * HOUR_HEIGHT
}

fn minutes_of((hour, minute): (u32, u32)) -> u32 {
    hour * 60 + minute
}

/// Gives each of a day's timed events a lane, so that those at the same time sit side by
/// side: `(item, lane, lanes in its group)`.
fn lanes(items: &[&Item]) -> Vec<(usize, usize, usize)> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&index| items[index].start);
    let end_of = |item: &Item| {
        let start = minutes_of(item.start.unwrap_or_default());
        item.end.map_or(start + DEFAULT_MINUTES, minutes_of).max(start + 1)
    };
    let mut placed: Vec<(usize, usize, usize)> = Vec::new();
    let mut group: Vec<(usize, usize)> = Vec::new();
    let mut lane_ends: Vec<u32> = Vec::new();
    let mut group_end = 0;
    let flush = |group: &mut Vec<(usize, usize)>, lane_ends: &mut Vec<u32>, placed: &mut Vec<(usize, usize, usize)>| {
        let lanes = lane_ends.len().max(1);
        placed.extend(group.drain(..).map(|(index, lane)| (index, lane, lanes)));
        lane_ends.clear();
    };
    for index in order {
        let start = minutes_of(items[index].start.unwrap_or_default());
        if start >= group_end && !group.is_empty() {
            flush(&mut group, &mut lane_ends, &mut placed);
        }
        let lane = lane_ends.iter().position(|&end| end <= start).unwrap_or(lane_ends.len());
        if lane == lane_ends.len() {
            lane_ends.push(0);
        }
        lane_ends[lane] = end_of(items[index]);
        group_end = group_end.max(end_of(items[index]));
        group.push((index, lane));
    }
    flush(&mut group, &mut lane_ends, &mut placed);
    placed
}

/// One or more days side by side: each day's head and those of its events that take the day
/// as bars under it (`Part::Head`), and the others as blocks where their time is against the
/// hours (`Part::Body`). The points in `hits` are the widget's own in both.
pub fn draw_time(
    cr: &cairo::Context,
    layout: &pango::Layout,
    (width, _height): (f64, f64),
    (part, days): (Part, &[Day]),
    (chosen, today, now): (Date, Date, (u32, u32)),
    look: &Look,
    hits: &mut Vec<Hit>,
) {
    hits.clear();
    if days.is_empty() {
        return;
    }
    let column_width = (width - GUTTER) / days.len() as f64;
    let head = head_height(days);
    let bottom = body_height();

    if part == Part::Head {
        for (index, day) in days.iter().enumerate() {
            let x = GUTTER + index as f64 * column_width;
            let is_today = day.date == today;
            hits.push(Hit { rect: (x, 0.0, column_width, head), target: Target::Day(day.date) });

            let weekday = format!("週{}", WEEKDAY_NAMES[day.date.weekday()]);
            if is_today {
                set_color(cr, &look.accent, 1.0);
            } else {
                set_color(cr, &look.ink, SOFT_ALPHA);
            }
            put(cr, layout, &weekday, (x, PADDING, column_width), (TEXT_SMALL + 1.0, is_today, Align::Center));
            let disc_x = x + (column_width - DAY_NUMBER_SIZE) / 2.0;
            let disc_y = PADDING + 20.0;
            day_number(cr, layout, look, (disc_x, disc_y), day.date, is_today);
            if day.date == chosen && !is_today {
                set_color(cr, &look.accent, 1.0);
                cr.set_line_width(2.0);
                cr.arc(disc_x + DAY_NUMBER_SIZE / 2.0, disc_y + DAY_NUMBER_SIZE / 2.0, DAY_NUMBER_SIZE / 2.0, 0.0, std::f64::consts::TAU);
                let _ = cr.stroke();
            }
            if !day.note.is_empty() {
                if day.holiday {
                    cr.set_source_rgb(HOLIDAY.0, HOLIDAY.1, HOLIDAY.2);
                } else {
                    set_color(cr, &look.ink, DIM_ALPHA + 0.1);
                }
                put(cr, layout, &day.note, (x, disc_y + DAY_NUMBER_SIZE + INSET, column_width), (TEXT_SMALL, false, Align::Center));
            }
            let all_day = day.items.iter().filter(|item| item.start.is_none());
            for (slot, item) in all_day.enumerate() {
                let rect = (x + INSET, DAY_HEAD + GAP + slot as f64 * ALL_DAY_STEP, column_width - INSET * 2.0, ALL_DAY_STEP - INSET);
                draw_chip(cr, layout, rect, item);
                hits.push(Hit { rect, target: Target::Item(item.clone()) });
            }
        }
        set_color(cr, &look.ink, LINE_ALPHA);
        cr.set_line_width(1.0);
        for index in 0..=days.len() {
            let x = (GUTTER + index as f64 * column_width).round() + 0.5;
            cr.move_to(x, DAY_HEAD - INSET);
            cr.line_to(x, head);
        }
        cr.move_to(0.0, head - 0.5);
        cr.line_to(width, head - 0.5);
        let _ = cr.stroke();
        return;
    }

    for (index, day) in days.iter().enumerate() {
        let x = GUTTER + index as f64 * column_width;
        if day.date == today {
            set_color(cr, &look.accent, SELECTED_ALPHA * 0.6);
            cr.rectangle(x, 0.0, column_width, bottom);
            let _ = cr.fill();
        }
        hits.push(Hit { rect: (x, 0.0, column_width, bottom), target: Target::Day(day.date) });

    }

    set_color(cr, &look.ink, LINE_ALPHA);
    cr.set_line_width(1.0);
    for hour in 0..=HOURS_IN_DAY {
        let y = (f64::from(hour) * HOUR_HEIGHT).round() + 0.5;
        cr.move_to(GUTTER, y);
        cr.line_to(width, y);
    }
    for index in 0..=days.len() {
        let x = (GUTTER + index as f64 * column_width).round() + 0.5;
        cr.move_to(x, 0.0);
        cr.line_to(x, bottom);
    }
    let _ = cr.stroke();
    for hour in 1..HOURS_IN_DAY {
        set_color(cr, &look.ink, DIM_ALPHA + 0.1);
        put(
            cr,
            layout,
            &clock_text((hour, 0)),
            (0.0, f64::from(hour) * HOUR_HEIGHT - TEXT_SMALL, GUTTER - GAP),
            (TEXT_SMALL, false, Align::Right),
        );
    }

    for (index, day) in days.iter().enumerate() {
        let x = GUTTER + index as f64 * column_width;
        let timed: Vec<&Item> = day.items.iter().filter(|item| item.start.is_some()).collect();
        for (at, lane, lane_count) in lanes(&timed) {
            let item = timed[at];
            let start = minutes_of(item.start.unwrap_or_default());
            let end = item.end.map_or(start + DEFAULT_MINUTES, minutes_of).max(start + 15);
            // Each event that overlaps one before it starts a step to the right of it and is
            // drawn over it, so that every title is in view, down to the step that is left of
            // the column's width when there are many.
            let step = if lane_count > 1 { CASCADE_STEP.min(column_width / 2.0 / (lane_count - 1) as f64) } else { 0.0 };
            let rect = (
                x + INSET + lane as f64 * step,
                f64::from(start) / MINUTES_IN_HOUR * HOUR_HEIGHT + 1.0,
                column_width - INSET * 2.0 - lane as f64 * step - 2.0,
                (f64::from(end - start) / MINUTES_IN_HOUR * HOUR_HEIGHT - 2.0).max(MIN_BLOCK),
            );
            draw_block(cr, layout, rect, item, look);
            hits.push(Hit { rect, target: Target::Item(item.clone()) });
        }
    }

    if let Some(index) = days.iter().position(|day| day.date == today) {
        let y = f64::from(minutes_of(now)) / MINUTES_IN_HOUR * HOUR_HEIGHT;
        let x = GUTTER + index as f64 * column_width;
        set_color(cr, &look.accent, 1.0);
        cr.set_line_width(2.0);
        cr.move_to(x, y);
        cr.line_to(x + column_width, y);
        let _ = cr.stroke();
        cr.arc(x, y, NOW_DOT, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    }
}

/// An event that has a time: a pale block of its calendar's colour with a bar down its edge.
fn draw_block(cr: &cairo::Context, layout: &pango::Layout, rect: (f64, f64, f64, f64), item: &Item, look: &Look) {
    let (x, y, width, height) = rect;
    // Solid under the tint, so that what a block is drawn over does not show through.
    rounded(cr, rect, CHIP_RADIUS);
    cr.set_source_rgb(look.page.0, look.page.1, look.page.2);
    let _ = cr.fill_preserve();
    let alpha = if look.dark { BLOCK_ALPHA + 0.1 } else { BLOCK_ALPHA };
    cr.set_source_rgba(item.color.0, item.color.1, item.color.2, if item.done { alpha * 0.6 } else { alpha });
    let _ = cr.fill();
    cr.set_source_rgb(item.color.0, item.color.1, item.color.2);
    rounded(cr, (x, y, BAR_WIDTH + 1.0, height), BAR_WIDTH);
    let _ = cr.fill();
    set_color(cr, &look.ink, if item.done { DIM_ALPHA } else { 1.0 });
    let used = put(cr, layout, &item.title, (x + BAR_WIDTH + GAP, y + INSET, width - BAR_WIDTH - GAP - INSET), (TEXT_BODY, true, Align::Left));
    if height > used + TEXT_SMALL * 2.2 {
        set_color(cr, &look.ink, SOFT_ALPHA);
        let when = match (item.start, item.end) {
            (Some(start), Some(end)) => format!("{} – {}", clock_text(start), clock_text(end)),
            (Some(start), None) => clock_text(start),
            _ => String::new(),
        };
        put(cr, layout, &when, (x + BAR_WIDTH + GAP, y + INSET + used + 1.0, width - BAR_WIDTH - GAP - INSET), (TEXT_SMALL, false, Align::Left));
    }
}
