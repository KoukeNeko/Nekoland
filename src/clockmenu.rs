//! The panel the clock in the menu bar opens, in the control center's glass: a month to pick
//! a day from, with its week numbers, its lunar days and holidays and what marks the days
//! that have events; what is on the day picked, which a click opens to its details and what
//! can be done with it; and the way into Calendar and the date and time settings.
//!
//! Every distance in it is one of three: `PADDING` from the panel's edge to a well and from a
//! well's edge to its text, `GAP` between wells and inside them, `INSET` between a row and
//! the well it is in.

use std::{process::Command, time::Instant};

use cairo::Context;
use calendar_data::{
    calendars::{self, color_of, Rgb},
    date::{Date, Month, DAYS_IN_WEEK, WEEKDAY_NAMES},
    event::{on_day, Event, Kind, Repeat},
    holidays::{self, Holiday},
    lunar,
};
use smithay::{
    backend::renderer::element::Id,
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    agenda::Agenda,
    config::{self, EventDots},
    control::{
        draw_chevron, draw_icon, draw_list_row, draw_panel, draw_well, ease_out, rect, show_line, to_f64, OPEN_TIME,
    },
    glass::Tier,
    menubar::{colors, font, show_text, Colors, Overlay, MENU_BAR_HEIGHT, MENU_RADIUS},
    text::{self, Picture},
    Nekoland,
};

const WIDTH: i32 = 320;
const PADDING: i32 = 12;
const GAP: i32 = 8;
const INSET: i32 = 4;
const HEADER_HEIGHT: i32 = 32;
const BUTTON: i32 = 28;
const TODAY_BUTTON_WIDTH: i32 = 44;
const WEEKDAY_ROW: i32 = 24;
const WEEK_COLUMN: i32 = 28;
const CELL_HEIGHT: i32 = 32;
/// A cell that has a line of lunar date or holiday under its day.
const CELL_HEIGHT_LABELLED: i32 = 42;
const MAX_WEEKS: usize = 6;
const MIN_WEEKS: usize = 4;
/// The disc a day sits on when it is today, picked or under the pointer.
const DISC: f64 = 28.0;
const DISC_CENTER: f64 = 14.0;
const LABEL_CENTER: f64 = 32.0;
const DOT_RADIUS: f64 = 1.75;
const DOT_SPACING: f64 = 6.0;
const MAX_DOTS: usize = 3;
const TITLE_ROW: i32 = 30;
const INFO_ROW: i32 = 16;
/// Where the day's heading is centred, from the top of its well.
const HEADING_CENTER: f64 = 17.0;
const ITEM_ROW: i32 = 38;
const EMPTY_ROW: i32 = 22;
const DETAIL_LINE: i32 = 18;
const ACTION_ROW: i32 = 32;
const LIST_ROW: i32 = 30;
const FOOTER_HEIGHT: i32 = INSET * 2 + LIST_ROW * 3;
/// The room the panel leaves under itself, from the bar.
const SCREEN_MARGIN: i32 = 8;
const MAX_ITEMS: usize = 5;
const MAX_NOTE_LINES: usize = 3;
/// What an item has at its left: the width of its mark, and the gap after it.
const MARK_WIDTH: i32 = 16;
const CHECK_RADIUS: f64 = 6.5;
const BAR_WIDTH: f64 = 4.0;
const BAR_HEIGHT: f64 = 24.0;
/// How much a button shrinks while pressed.
const PRESSED_SCALE: f64 = 0.92;
/// Turns of the submenu arrow, which points right, to point left.
const TURN_AROUND: f64 = 2.0;
const NO_EVENTS: &str = "沒有行程";
const ALL_DAY: &str = "整天";
const OVERDUE: &str = "逾期";
const REPEATS: &str = "↻";
pub(crate) const CALENDAR_COMMAND: &str = "nekoland-calendar";
const DATE_TIME_COMMAND: &str = "NEKOLAND_SETTINGS_PAGE=日期與時間 nekoland-settings";
const MINUTES_IN_HOUR: u32 = 60;
const SECONDS_IN_MINUTE: u32 = 60;
const GRID_OUTLINE: f64 = 0.2;

/// What the configuration says the panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    week_numbers: bool,
    outline: bool,
    lunar: bool,
    holidays: bool,
    dots: EventDots,
    weeks: usize,
    first_weekday: usize,
    peek: bool,
    finished: bool,
    future: bool,
    summary: bool,
    recurrence: bool,
    overdue: bool,
    all_day: bool,
    all_day_details: bool,
}

impl Options {
    pub fn from_config(config: &config::Config) -> Self {
        Self {
            week_numbers: config.calendar_week_numbers,
            outline: config.calendar_month_outline,
            lunar: config.calendar_lunar,
            holidays: config.calendar_holidays,
            dots: config.calendar_event_dots,
            weeks: (config.calendar_weeks.max(MIN_WEEKS as i32) as usize).min(MAX_WEEKS),
            first_weekday: config.calendar_first_weekday.rem_euclid(DAYS_IN_WEEK as i32) as usize,
            peek: config.calendar_peek,
            finished: config.calendar_show_finished,
            future: config.calendar_show_future,
            summary: config.calendar_show_summary,
            recurrence: config.calendar_show_recurrence,
            overdue: config.calendar_show_overdue,
            all_day: config.calendar_show_all_day,
            all_day_details: config.calendar_show_all_day_details,
        }
    }

    fn labelled(&self) -> bool {
        self.lunar || self.holidays
    }

    fn cell_height(&self) -> i32 {
        if self.labelled() {
            CELL_HEIGHT_LABELLED
        } else {
            CELL_HEIGHT
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Previous,
    Next,
    Today,
    Pin,
    Day(usize),
    /// A row of the list, which opens to its details.
    Item(usize),
    Check(usize),
    Edit(usize),
    Join(usize),
    Delete(usize),
    /// Taking away what the row says: all of it, or only its one day.
    ConfirmAll(usize),
    ConfirmThis(usize),
    Cancel,
    New,
    Calendar,
    DateTime,
}

/// What a row of the list is.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Holiday(Holiday),
    Event {
        /// Where the event is in the agenda.
        index: usize,
        overdue: bool,
        /// Whether what it was is over, as far as the day shown goes.
        finished: bool,
    },
}

/// What the panel shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
struct Shown {
    dark: bool,
    today: Date,
    month: Month,
    picked: Date,
    shown_day: Date,
    options: Options,
    agenda: u64,
    expanded: Option<String>,
    confirming: Option<String>,
    pinned: bool,
    pressed: Option<Part>,
    hovered: Option<Part>,
    minute: u32,
}

/// Where the parts are, from the panel's corner.
struct Layout {
    height: i32,
    grid_well: Rectangle<i32, Logical>,
    events_well: Rectangle<i32, Logical>,
    footer_well: Rectangle<i32, Logical>,
    parts: Vec<(Part, Rectangle<i32, Logical>)>,
    entries: Vec<(Entry, Rectangle<i32, Logical>)>,
    /// How many items did not fit, and where the line that says so is.
    overflow: Option<(usize, Rectangle<i32, Logical>)>,
    grid: Vec<Date>,
}

pub struct ClockPanel {
    /// Its left edge, from the output's.
    x: i32,
    /// How tall it may be, for the screen it is on.
    max_height: i32,
    opened: Instant,
    today: Date,
    month: Month,
    picked: Date,
    /// The day the pointer is on, held with Alt to look at it without picking it.
    hovered_day: Option<Date>,
    alt: bool,
    options: Options,
    minute: u32,
    expanded: Option<String>,
    confirming: Option<String>,
    pub(crate) pinned: bool,
    pressed: Option<Part>,
    hovered: Option<Part>,
    shown: Option<Shown>,
    picture: Option<Picture>,
    id: Id,
}

fn is_over(event: &Event, minute_of_day: u32) -> bool {
    let end = event.end.or(event.start).map(|(hour, minute)| hour * MINUTES_IN_HOUR + minute);
    end.is_some_and(|end| end < minute_of_day)
}

fn is_ahead(event: &Event, minute_of_day: u32) -> bool {
    event.start.is_some_and(|(hour, minute)| hour * MINUTES_IN_HOUR + minute > minute_of_day)
}

fn time_text((hour, minute): (u32, u32)) -> String {
    format!("{hour:02}:{minute:02}")
}

/// When an event is, as the list says it.
pub(crate) fn when_text(event: &Event) -> String {
    match (event.start, event.end) {
        (None, _) => ALL_DAY.to_string(),
        (Some(start), Some(end)) => format!("{} – {}", time_text(start), time_text(end)),
        (Some(start), None) => time_text(start),
    }
}

fn text_width(cr: &Context, text: &str, font: &pango::FontDescription) -> f64 {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(font));
    layout.set_text(text);
    f64::from(layout.pixel_size().0)
}

/// `text` with its middle at `center`.
fn show_centered(cr: &Context, text: &str, font: &pango::FontDescription, center: (f64, f64), color: Rgb) {
    let width = text_width(cr, text, font);
    show_text(cr, text, font, center.0 - width / 2.0, center.1, color);
}

pub(crate) fn show_right(cr: &Context, text: &str, font: &pango::FontDescription, (right, center): (f64, f64), color: Rgb) {
    let width = text_width(cr, text, font);
    show_text(cr, text, font, right - width, center, color);
}

fn disc(cr: &Context, x: f64, y: f64, color: Rgb, alpha: f64) {
    cr.new_sub_path();
    cr.arc(x, y, DISC / 2.0, 0.0, std::f64::consts::TAU);
    cr.set_source_rgba(color.0, color.1, color.2, alpha);
    let _ = cr.fill();
}

fn dot(cr: &Context, x: f64, y: f64, color: Rgb) {
    cr.new_sub_path();
    cr.arc(x, y, DOT_RADIUS, 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(color.0, color.1, color.2);
    let _ = cr.fill();
}

fn scale_if_pressed(cr: &Context, pressed: bool, x: f64, y: f64) {
    if pressed {
        cr.translate(x, y);
        cr.scale(PRESSED_SCALE, PRESSED_SCALE);
        cr.translate(-x, -y);
    }
}

impl ClockPanel {
    pub fn new(anchor: Rectangle<i32, Logical>, output: Size<i32, Logical>, options: Options, picked: Option<Date>) -> Self {
        let today = Date::today();
        let picked = picked.unwrap_or(today);
        Self {
            max_height: output.h - MENU_BAR_HEIGHT - 4 - SCREEN_MARGIN,
            x: (anchor.loc.x + anchor.size.w / 2 - WIDTH / 2)
                .min(output.w - WIDTH - 6)
                .max(6),
            opened: Instant::now(),
            today,
            month: picked.month_of(),
            picked,
            hovered_day: None,
            alt: false,
            options,
            minute: 0,
            expanded: None,
            confirming: None,
            pinned: false,
            pressed: None,
            hovered: None,
            shown: None,
            picture: None,
            id: Id::new(),
        }
    }

    /// The day whose events are listed: the one picked, or the one the pointer is on with Alt held.
    fn shown_day(&self) -> Date {
        match self.hovered_day {
            Some(day) if self.alt && self.options.peek => day,
            _ => self.picked,
        }
    }

    pub fn pick(&mut self, day: Date) {
        self.picked = day;
        self.month = day.month_of();
        self.expanded = None;
        self.confirming = None;
    }

    pub fn in_rect(&self, local: Point<i32, Logical>, height: i32) -> bool {
        Rectangle::new((self.x, MENU_BAR_HEIGHT + 4).into(), (WIDTH, height).into()).contains(local)
    }

    /// The days the month is shown in: the weeks it has, moved to hold the day picked.
    fn visible_grid(&self) -> Vec<Date> {
        let full = self.month.grid(self.options.first_weekday, MAX_WEEKS);
        let weeks = self.options.weeks;
        let picked_row = full
            .iter()
            .position(|day| *day == self.picked)
            .map_or(0, |at| at / DAYS_IN_WEEK);
        let first_row = (picked_row + 1).saturating_sub(weeks);
        full[first_row * DAYS_IN_WEEK..(first_row + weeks) * DAYS_IN_WEEK].to_vec()
    }

    fn week_column(&self) -> i32 {
        if self.options.week_numbers {
            WEEK_COLUMN
        } else {
            0
        }
    }

    fn cell_width(&self) -> i32 {
        (WIDTH - PADDING * 2 - GAP * 2 - self.week_column()) / DAYS_IN_WEEK as i32
    }

    /// What is listed for `day`: the holidays, the reminders that are late, then its events.
    fn entries(&self, agenda: &Agenda, day: Date) -> Vec<Entry> {
        let mut found = Vec::new();
        if self.options.holidays && agenda.shown(calendars::HOLIDAYS) {
            found.extend(holidays::on(day).into_iter().map(Entry::Holiday));
        }
        let is_today = day == self.today;
        if self.options.overdue && is_today {
            for (index, event) in agenda.events().iter().enumerate() {
                let late = event.kind == Kind::Reminder
                    && !event.done
                    && event.repeat == Repeat::Never
                    && event.date < self.today
                    && agenda.shown(event.calendar);
                if late {
                    found.push(Entry::Event { index, overdue: true, finished: false });
                }
            }
        }
        for occurrence in on_day(agenda.events(), day) {
            let event = occurrence.event;
            if !agenda.shown(event.calendar) {
                continue;
            }
            if event.all_day() && event.kind == Kind::Event && !self.options.all_day {
                continue;
            }
            let finished = is_today && is_over(event, self.minute);
            if (finished && !self.options.finished) || (is_today && is_ahead(event, self.minute) && !self.options.future) {
                continue;
            }
            found.push(Entry::Event { index: occurrence.index, overdue: false, finished });
        }
        found
    }

    /// The calendars of what is on `day`, for the dots under it.
    fn dot_calendars(&self, agenda: &Agenda, day: Date) -> Vec<usize> {
        let mut found: Vec<usize> = Vec::new();
        let mut add = |calendar: usize| {
            if !found.contains(&calendar) {
                found.push(calendar);
            }
        };
        for occurrence in on_day(agenda.events(), day) {
            if agenda.shown(occurrence.event.calendar) {
                add(occurrence.event.calendar);
            }
        }
        if self.options.holidays && agenda.shown(calendars::HOLIDAYS) && !holidays::on(day).is_empty() {
            add(calendars::HOLIDAYS);
        }
        found
    }

    /// The line under a day: the holiday it is, or its lunar date.
    fn cell_label(&self, agenda: &Agenda, day: Date) -> Option<(String, bool)> {
        if self.options.holidays && agenda.shown(calendars::HOLIDAYS) {
            if let Some(holiday) = holidays::on(day).first() {
                return Some((holiday.name.to_string(), holiday.day_off));
            }
        }
        self.options
            .lunar
            .then(|| lunar::from_solar(day).map(|found| (found.label(), false)))
            .flatten()
    }

    /// How tall an item is, opened or not, and what its details are.
    fn item_height(&self, agenda: &Agenda, entry: &Entry) -> i32 {
        let Entry::Event { index, .. } = entry else {
            return ITEM_ROW;
        };
        let event = &agenda.events()[*index];
        if self.expanded.as_deref() != Some(event.id.as_str()) {
            return ITEM_ROW;
        }
        ITEM_ROW + self.detail_lines(event).len() as i32 * DETAIL_LINE + INSET + ACTION_ROW
    }

    /// The lines under an open item: where it is, its link and its notes.
    fn detail_lines(&self, event: &Event) -> Vec<(&'static str, String)> {
        if event.all_day() && !self.options.all_day_details {
            return Vec::new();
        }
        let mut lines = Vec::new();
        if !event.location.is_empty() {
            lines.push(("地點", event.location.clone()));
        }
        if !event.url.is_empty() {
            lines.push(("連結", event.url.clone()));
        }
        lines.extend(
            event
                .notes
                .lines()
                .filter(|line| !line.trim().is_empty())
                .take(MAX_NOTE_LINES)
                .enumerate()
                .map(|(at, line)| (if at == 0 { "備註" } else { "" }, line.to_string())),
        );
        if event.repeat != Repeat::Never {
            lines.push(("重複", event.repeat.label().to_string()));
        }
        lines
    }

    fn layout(&self, agenda: &Agenda) -> Layout {
        let inner = WIDTH - PADDING * 2;
        let mut parts = Vec::new();
        let mut y = PADDING;

        // The buttons sit an inset inside the wells' right edge.
        let button_top = y + (HEADER_HEIGHT - BUTTON) / 2;
        let next = rect(PADDING + inner - INSET - BUTTON, button_top, BUTTON, BUTTON);
        let previous = rect(next.loc.x - BUTTON, button_top, BUTTON, BUTTON);
        parts.push((Part::Next, next));
        parts.push((Part::Previous, previous));
        let mut leftmost = previous.loc.x;
        if self.month != self.today.month_of() || self.picked != self.today {
            let today_button = rect(leftmost - INSET - TODAY_BUTTON_WIDTH, button_top, TODAY_BUTTON_WIDTH, BUTTON);
            leftmost = today_button.loc.x;
            parts.push((Part::Today, today_button));
        }
        parts.push((Part::Pin, rect(leftmost - INSET - BUTTON, button_top, BUTTON, BUTTON)));
        y += HEADER_HEIGHT + GAP;

        let grid = self.visible_grid();
        let cell_height = self.options.cell_height();
        let cell_width = self.cell_width();
        let grid_height = GAP * 2 + WEEKDAY_ROW + cell_height * self.options.weeks as i32;
        let grid_well = rect(PADDING, y, inner, grid_height);
        let cells_left = PADDING + GAP + self.week_column();
        let cells_top = y + GAP + WEEKDAY_ROW;
        for (cell, _) in grid.iter().enumerate() {
            let (column, row) = ((cell % DAYS_IN_WEEK) as i32, (cell / DAYS_IN_WEEK) as i32);
            parts.push((
                Part::Day(cell),
                rect(cells_left + column * cell_width, cells_top + row * cell_height, cell_width, cell_height),
            ));
        }
        y += grid_height + GAP;

        // The day's well: its heading, then its items, an open one taller.
        let well_top = y;
        let shown = self.shown_day();
        let has_info = self.options.lunar || self.options.holidays;
        let heading = TITLE_ROW + if has_info { INFO_ROW } else { 0 };
        let mut item_top = well_top + heading;
        let all = self.entries(agenda, shown);
        let mut entries = Vec::new();
        let mut overflow = None;
        // What the panel needs under the items: the line that counts the ones left out, the
        // footer and the margins.
        let reserve = EMPTY_ROW + GAP * 2 + FOOTER_HEIGHT + PADDING;
        for (at, entry) in all.iter().enumerate() {
            let height = self.item_height(agenda, entry);
            let fits = item_top + height + reserve <= self.max_height;
            if at >= MAX_ITEMS || (at > 0 && !fits) {
                let line = rect(PADDING, item_top, inner, EMPTY_ROW);
                overflow = Some((all.len() - at, line));
                item_top += EMPTY_ROW;
                break;
            }
            let area = rect(PADDING + INSET, item_top, inner - INSET * 2, height);
            entries.push((entry.clone(), area));
            self.item_parts(agenda, entry, at, area, &mut parts);
            item_top += height;
        }
        if all.is_empty() {
            item_top += EMPTY_ROW;
        }
        let events_height = item_top - well_top + GAP;
        let events_well = rect(PADDING, well_top, inner, events_height);
        y += events_height + GAP;

        let footer_well = rect(PADDING, y, inner, FOOTER_HEIGHT);
        let row = |index: i32| rect(PADDING + INSET, y + INSET + LIST_ROW * index, inner - INSET * 2, LIST_ROW);
        parts.push((Part::New, row(0)));
        parts.push((Part::Calendar, row(1)));
        parts.push((Part::DateTime, row(2)));
        y += footer_well.size.h;

        Layout {
            height: y + PADDING,
            grid_well,
            events_well,
            footer_well,
            parts,
            entries,
            overflow,
            grid,
        }
    }

    /// What can be pressed in an item: the row, the check of a reminder, and, when it is
    /// open, its buttons.
    fn item_parts(
        &self,
        agenda: &Agenda,
        entry: &Entry,
        at: usize,
        area: Rectangle<i32, Logical>,
        parts: &mut Vec<(Part, Rectangle<i32, Logical>)>,
    ) {
        let Entry::Event { index, .. } = entry else {
            return;
        };
        let event = &agenda.events()[*index];
        let open = self.expanded.as_deref() == Some(event.id.as_str());
        parts.push((Part::Item(at), rect(area.loc.x, area.loc.y, area.size.w, ITEM_ROW)));
        if event.kind == Kind::Reminder {
            parts.push((Part::Check(at), rect(area.loc.x, area.loc.y, PADDING + MARK_WIDTH, ITEM_ROW)));
        }
        if !open {
            return;
        }
        let lines = self.detail_lines(event).len() as i32;
        let top = area.loc.y + ITEM_ROW + lines * DETAIL_LINE + INSET;
        let confirming = self.confirming.as_deref() == Some(event.id.as_str());
        let buttons: Vec<Part> = if confirming && event.repeat != Repeat::Never {
            vec![Part::Cancel, Part::ConfirmThis(at), Part::ConfirmAll(at)]
        } else if confirming {
            vec![Part::Cancel, Part::ConfirmAll(at)]
        } else if event.url.is_empty() {
            vec![Part::Edit(at), Part::Delete(at)]
        } else {
            vec![Part::Edit(at), Part::Join(at), Part::Delete(at)]
        };
        let width = (area.size.w - PADDING * 2 - GAP * (buttons.len() as i32 - 1)) / buttons.len() as i32;
        for (position, part) in buttons.into_iter().enumerate() {
            parts.push((
                part,
                rect(
                    area.loc.x + PADDING + position as i32 * (width + GAP),
                    top,
                    width,
                    ACTION_ROW - INSET,
                ),
            ));
        }
    }

    fn part_at(&self, agenda: &Agenda, local: Point<i32, Logical>) -> Option<Part> {
        // A button of an open item is over the row it is in.
        self.layout(agenda)
            .parts
            .iter()
            .rev()
            .find_map(|(part, area)| area.contains(local).then_some(*part))
    }

    pub fn height(&self, agenda: &Agenda) -> i32 {
        self.layout(agenda).height
    }

    fn draw(&mut self, shown: &Shown, agenda: &Agenda) {
        let colors = colors(shown.dark);
        let layout = self.layout(agenda);
        let Some((surface, cr)) = draw_panel(WIDTH, layout.height, shown.dark, &colors) else {
            return;
        };

        self.draw_header(&cr, &colors, shown);
        for well in [layout.grid_well, layout.events_well, layout.footer_well] {
            draw_well(&cr, &colors, well, 8.0);
        }
        self.draw_grid(&cr, &colors, &layout, shown, agenda);
        self.draw_events(&cr, &colors, &layout, shown, agenda);

        let label = font("", 10.0);
        for (part, area) in &layout.parts {
            let (x, y, w, h) = to_f64(*area);
            let (center_x, center_y) = (x + w / 2.0, y + h / 2.0);
            let hovered = shown.hovered == Some(*part);
            let pressed = shown.pressed == Some(*part);
            match *part {
                Part::Previous | Part::Next => {
                    if hovered {
                        draw_well(&cr, &colors, *area, 6.0);
                    }
                    let turn = if *part == Part::Next { 0.0 } else { TURN_AROUND };
                    cr.save().ok();
                    scale_if_pressed(&cr, pressed, center_x, center_y);
                    draw_chevron(&cr, center_x, center_y, turn, colors.ink);
                    cr.restore().ok();
                }
                Part::Today => {
                    if hovered {
                        draw_well(&cr, &colors, *area, 6.0);
                    }
                    show_centered(&cr, "今天", &label, (center_x, center_y), colors.ink);
                }
                Part::Pin => {
                    let color = if shown.pinned {
                        text::rounded_rectangle(&cr, x, y, w, h, 6.0);
                        cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
                        let _ = cr.fill();
                        colors.on_ink
                    } else {
                        if hovered {
                            draw_well(&cr, &colors, *area, 6.0);
                        }
                        colors.ink
                    };
                    draw_icon(&cr, "view-pin-symbolic", center_x - 8.0, center_y, color);
                }
                Part::New | Part::Calendar | Part::DateTime => {
                    let color = draw_list_row(&cr, &colors, *area, hovered);
                    let text = match part {
                        Part::New => "新增行程⋯",
                        Part::Calendar => "開啟行事曆",
                        _ => "日期與時間設定⋯",
                    };
                    show_text(&cr, text, &label, x + f64::from(PADDING - INSET), center_y, color);
                }
                Part::Edit(_) | Part::Join(_) | Part::Delete(_) | Part::ConfirmAll(_) | Part::ConfirmThis(_) | Part::Cancel => {
                    let text = match part {
                        Part::Edit(_) => "編輯",
                        Part::Join(_) => "開啟連結",
                        Part::Delete(_) => "刪除",
                        Part::ConfirmAll(at) => self.confirm_all_label(agenda, &layout, *at),
                        Part::ConfirmThis(_) => "只刪除這次",
                        _ => "取消",
                    };
                    let color = draw_button(&cr, &colors, *area, hovered);
                    cr.save().ok();
                    scale_if_pressed(&cr, pressed, center_x, center_y);
                    show_centered(&cr, text, &label, (center_x, center_y), color);
                    cr.restore().ok();
                }
                Part::Day(_) | Part::Item(_) | Part::Check(_) => {}
            }
        }
        drop(cr);
        self.picture = text::picture(surface);
    }

    fn confirm_all_label(&self, agenda: &Agenda, layout: &Layout, at: usize) -> &'static str {
        let repeats = layout.entries.get(at).is_some_and(|(entry, _)| {
            matches!(entry, Entry::Event { index, .. } if agenda.events()[*index].repeat != Repeat::Never)
        });
        if repeats {
            "刪除之後全部"
        } else {
            "確定刪除"
        }
    }

    fn draw_header(&self, cr: &Context, colors: &Colors, shown: &Shown) {
        let title = font("Bold", 13.0);
        let center = f64::from(PADDING + HEADER_HEIGHT / 2);
        show_text(
            cr,
            &format!("{}年{}月", shown.month.year, shown.month.month),
            &title,
            f64::from(PADDING * 2),
            center,
            colors.ink,
        );
    }

    fn draw_grid(&self, cr: &Context, colors: &Colors, layout: &Layout, shown: &Shown, agenda: &Agenda) {
        let small = font("", 9.0);
        let tiny = font("", 8.0);
        let digits = font("", 11.0);
        let today_digits = font("Bold", 11.0);
        let cell_width = self.cell_width();
        let cell_height = self.options.cell_height();
        let cells_left = PADDING + GAP + self.week_column();
        let header_center = f64::from(layout.grid_well.loc.y + GAP + WEEKDAY_ROW / 2);

        if self.options.week_numbers {
            show_centered(cr, "週", &small, (f64::from(PADDING + GAP + WEEK_COLUMN / 2), header_center), colors.ink_3);
        }
        for column in 0..DAYS_IN_WEEK {
            let name = WEEKDAY_NAMES[(self.options.first_weekday + column) % DAYS_IN_WEEK];
            let center_x = f64::from(cells_left + cell_width * column as i32 + cell_width / 2);
            show_centered(cr, name, &small, (center_x, header_center), colors.ink_3);
        }

        let cells_top = layout.grid_well.loc.y + GAP + WEEKDAY_ROW;
        if self.options.week_numbers {
            for row in 0..self.options.weeks {
                let week = layout.grid[row * DAYS_IN_WEEK + 3].iso_week();
                let center = (
                    f64::from(PADDING + GAP + WEEK_COLUMN / 2),
                    f64::from(cells_top + row as i32 * cell_height) + DISC_CENTER,
                );
                show_centered(cr, &week.to_string(), &small, center, colors.ink_3);
            }
        }
        if self.options.outline {
            self.draw_outline(cr, colors, layout, (cells_left, cells_top), (cell_width, cell_height), shown.month);
        }

        for (part, area) in &layout.parts {
            let Part::Day(cell) = part else {
                continue;
            };
            let day = layout.grid[*cell];
            let (x, y, w, _) = to_f64(*area);
            let center_x = x + w / 2.0;
            let digit_y = y + DISC_CENTER;
            let in_month = day.month == shown.month.month;
            let is_today = day == shown.today;
            let is_picked = day == shown.picked;
            let hovered = shown.hovered == Some(*part);
            let ink = colors.ink;
            if is_today {
                disc(cr, center_x, digit_y, ink, 1.0);
            } else if hovered {
                disc(cr, center_x, digit_y, ink, 0.12);
            }
            if is_picked && !is_today {
                cr.new_sub_path();
                cr.arc(center_x, digit_y, DISC / 2.0 - 0.75, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(ink.0, ink.1, ink.2);
                cr.set_line_width(1.5);
                let _ = cr.stroke();
            }
            let number_color = match (is_today, in_month) {
                (true, _) => colors.on_ink,
                (false, true) => ink,
                (false, false) => colors.ink_3,
            };
            let number_font = if is_today { &today_digits } else { &digits };
            show_centered(cr, &day.day.to_string(), number_font, (center_x, digit_y - 1.0), number_color);

            if let Some((label, strong)) = self.cell_label(agenda, day) {
                let color = if strong && in_month { colors.ink } else { colors.ink_3 };
                cr.save().ok();
                cr.rectangle(x, y, w, f64::from(cell_height));
                cr.clip();
                let width = text_width(cr, &label, &tiny).min(w);
                show_line(cr, &label, &tiny, (center_x - width / 2.0, y + LABEL_CENTER, w), color);
                cr.restore().ok();
            }
            self.draw_dots(cr, colors, agenda, day, (center_x, y + f64::from(cell_height) - 3.0));
        }
    }

    /// A line round the days of the month, along the sides of its cells that touch another month.
    fn draw_outline(
        &self,
        cr: &Context,
        colors: &Colors,
        layout: &Layout,
        (left, top): (i32, i32),
        (cell_width, cell_height): (i32, i32),
        month: Month,
    ) {
        let inside = |row: i32, column: i32| {
            (0..self.options.weeks as i32).contains(&row)
                && (0..DAYS_IN_WEEK as i32).contains(&column)
                && layout.grid[row as usize * DAYS_IN_WEEK + column as usize].month_of() == month
        };
        cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, GRID_OUTLINE);
        cr.set_line_width(1.0);
        cr.set_line_cap(cairo::LineCap::Round);
        for row in 0..self.options.weeks as i32 {
            for column in 0..DAYS_IN_WEEK as i32 {
                if !inside(row, column) {
                    continue;
                }
                let x0 = f64::from(left + column * cell_width) + 0.5;
                let y0 = f64::from(top + row * cell_height) + 0.5;
                let (x1, y1) = (x0 + f64::from(cell_width) - 1.0, y0 + f64::from(cell_height) - 1.0);
                let sides = [
                    (inside(row - 1, column), (x0, y0, x1, y0)),
                    (inside(row + 1, column), (x0, y1, x1, y1)),
                    (inside(row, column - 1), (x0, y0, x0, y1)),
                    (inside(row, column + 1), (x1, y0, x1, y1)),
                ];
                for (neighbour, (ax, ay, bx, by)) in sides {
                    if !neighbour {
                        cr.move_to(ax, ay);
                        cr.line_to(bx, by);
                    }
                }
            }
        }
        let _ = cr.stroke();
    }

    fn draw_dots(&self, cr: &Context, colors: &Colors, agenda: &Agenda, day: Date, (center_x, y): (f64, f64)) {
        if self.options.dots == EventDots::None {
            return;
        }
        let found = self.dot_calendars(agenda, day);
        let Some(first) = found.first().copied() else {
            return;
        };
        match self.options.dots {
            EventDots::Neutral => dot(cr, center_x, y, colors.ink_3),
            EventDots::Highlighted => dot(cr, center_x, y, color_of(agenda.calendars(), first)),
            EventDots::Multiple | EventDots::None => {
                let shown: Vec<usize> = found.into_iter().take(MAX_DOTS).collect();
                let spread = DOT_SPACING * (shown.len() as f64 - 1.0) / 2.0;
                for (at, calendar) in shown.into_iter().enumerate() {
                    dot(cr, center_x - spread + DOT_SPACING * at as f64, y, color_of(agenda.calendars(), calendar));
                }
            }
        }
    }

    fn draw_events(&self, cr: &Context, colors: &Colors, layout: &Layout, shown: &Shown, agenda: &Agenda) {
        let well = layout.events_well;
        let (x, y, w, _) = to_f64(well);
        let small = font("", 9.0);
        let label = font("", 10.0);
        let day = shown.shown_day;
        let text_left = x + f64::from(PADDING);
        let text_right = x + w - f64::from(PADDING);
        let title = format!("{}月{}日 週{}", day.month, day.day, WEEKDAY_NAMES[day.weekday()]);
        show_text(cr, &title, &small, text_left, y + HEADING_CENTER, colors.ink_3);

        let counts = (
            layout.entries.iter().filter(|(entry, _)| matches!(entry, Entry::Event { index, .. } if agenda.events()[*index].kind == Kind::Event)).count(),
            layout.entries.iter().filter(|(entry, _)| matches!(entry, Entry::Event { index, .. } if agenda.events()[*index].kind == Kind::Reminder)).count(),
        );
        if self.options.summary && counts != (0, 0) {
            let mut parts = Vec::new();
            if counts.0 > 0 {
                parts.push(format!("{} 項行程", counts.0));
            }
            if counts.1 > 0 {
                parts.push(format!("{} 項提醒", counts.1));
            }
            show_right(cr, &parts.join(" · "), &small, (text_right, y + HEADING_CENTER), colors.ink_3);
        }
        if self.options.lunar || self.options.holidays {
            let mut line = Vec::new();
            if self.options.lunar {
                if let Some(found) = lunar::from_solar(day) {
                    line.push(format!("農曆{}", found.full()));
                }
            }
            if self.options.holidays && agenda.shown(calendars::HOLIDAYS) {
                line.extend(holidays::on(day).iter().map(|holiday| holiday.name.to_string()));
            }
            show_line(cr, &line.join(" · "), &small, (text_left, y + HEADING_CENTER + f64::from(INFO_ROW), text_right - text_left), colors.ink_3);
        }

        if layout.entries.is_empty() && layout.overflow.is_none() {
            let heading = TITLE_ROW + if self.options.lunar || self.options.holidays { INFO_ROW } else { 0 };
            show_text(cr, NO_EVENTS, &label, text_left, y + f64::from(heading + EMPTY_ROW / 2), colors.ink_3);
        }
        for (at, (entry, area)) in layout.entries.iter().enumerate() {
            self.draw_entry(cr, colors, shown, agenda, (at, entry, *area));
        }
        if let Some((more, line)) = layout.overflow {
            show_text(cr, &format!("另有 {more} 項"), &small, text_left, f64::from(line.loc.y + line.size.h / 2), colors.ink_3);
        }
    }

    fn draw_entry(&self, cr: &Context, colors: &Colors, shown: &Shown, agenda: &Agenda, (at, entry, area): (usize, &Entry, Rectangle<i32, Logical>)) {
        let (x, y, w, _) = to_f64(area);
        let label = font("", 10.0);
        let small = font("", 9.0);
        let center = y + f64::from(ITEM_ROW) / 2.0;
        let mark_x = x + f64::from(PADDING - INSET);
        let text_left = mark_x + f64::from(MARK_WIDTH);
        let text_right = x + w - f64::from(PADDING - INSET);
        let hovered = shown.hovered == Some(Part::Item(at));

        let (title, detail, calendar, finished, repeats) = match entry {
            Entry::Holiday(holiday) => (holiday.name.to_string(), String::new(), calendars::HOLIDAYS, false, false),
            Entry::Event { index, overdue, finished } => {
                let event = &agenda.events()[*index];
                let mut detail = if *overdue { OVERDUE.to_string() } else { when_text(event) };
                if !event.location.is_empty() {
                    detail = format!("{detail} · {}", event.location);
                }
                (event.title.clone(), detail, event.calendar, *finished || event.done, event.repeat != Repeat::Never)
            }
        };
        let tint = color_of(agenda.calendars(), calendar);
        let main = if finished { colors.ink_3 } else { colors.ink };
        if hovered && matches!(entry, Entry::Event { .. }) {
            draw_well(cr, colors, rect(area.loc.x, area.loc.y, area.size.w, ITEM_ROW), 6.0);
        }
        match entry {
            Entry::Event { index, .. } if agenda.events()[*index].kind == Kind::Reminder => {
                let done = agenda.events()[*index].done;
                cr.new_sub_path();
                cr.arc(mark_x + f64::from(MARK_WIDTH) / 2.0 - 2.0, center, CHECK_RADIUS, 0.0, std::f64::consts::TAU);
                cr.set_source_rgb(tint.0, tint.1, tint.2);
                if done {
                    let _ = cr.fill();
                } else {
                    cr.set_line_width(1.5);
                    let _ = cr.stroke();
                }
            }
            _ => {
                text::rounded_rectangle(cr, mark_x, center - BAR_HEIGHT / 2.0, BAR_WIDTH, BAR_HEIGHT, BAR_WIDTH / 2.0);
                cr.set_source_rgb(tint.0, tint.1, tint.2);
                let _ = cr.fill();
            }
        }
        let right = if repeats && self.options.recurrence { text_right - 14.0 } else { text_right };
        if detail.is_empty() {
            show_line(cr, &title, &label, (text_left, center, right - text_left), main);
        } else {
            show_line(cr, &title, &label, (text_left, center - 8.0, right - text_left), main);
            show_line(cr, &detail, &small, (text_left, center + 9.0, right - text_left), colors.ink_3);
        }
        if repeats && self.options.recurrence {
            show_right(cr, REPEATS, &label, (text_right, center), colors.ink_3);
        }

        let Entry::Event { index, .. } = entry else {
            return;
        };
        let event = &agenda.events()[*index];
        if self.expanded.as_deref() != Some(event.id.as_str()) {
            return;
        }
        let mut line_y = y + f64::from(ITEM_ROW) + f64::from(DETAIL_LINE) / 2.0;
        for (name, text) in self.detail_lines(event) {
            show_text(cr, name, &small, text_left, line_y, colors.ink_3);
            show_line(cr, &text, &small, (text_left + 36.0, line_y, text_right - text_left - 36.0), colors.ink);
            line_y += f64::from(DETAIL_LINE);
        }
    }
}

/// A button in an open item: a well of its own, lit in ink under the pointer.
fn draw_button(cr: &Context, colors: &Colors, area: Rectangle<i32, Logical>, hovered: bool) -> Rgb {
    if hovered {
        return draw_list_row(cr, colors, area, true);
    }
    draw_well(cr, colors, area, 6.0);
    colors.ink
}

fn open_url(url: &str) {
    let allowed = ["http://", "https://", "mailto:"].iter().any(|scheme| url.starts_with(scheme));
    if !allowed {
        tracing::warn!("calendar: not opening {url}");
        return;
    }
    if let Err(err) = Command::new("xdg-open").arg(url).spawn() {
        tracing::warn!("xdg-open {url}: {err}");
    }
}

impl Nekoland {
    /// Opens the clock's panel under the clock in the bar, or closes it.
    pub(crate) fn toggle_clock_panel(&mut self, anchor: Rectangle<i32, Logical>) {
        self.menu_bar.open = None;
        self.dismiss_stats_panel();
        self.menu_bar.control = None;
        if self.menu_bar.clock_panel.take().is_some() {
            return;
        }
        let Some(output) = self.bar_area() else {
            return;
        };
        let options = Options::from_config(&self.config);
        let picked = self.config.calendar_preserve_date.then_some(self.menu_bar.clock_picked).flatten();
        self.menu_bar.clock_panel = Some(ClockPanel::new(anchor, output.size, options, picked));
    }

    /// Opens the panel on `day`, as a link to a date does.
    pub fn show_clock_panel_on(&mut self, day: Date) {
        let Some(title) = self.clock_title_rect() else {
            return;
        };
        self.menu_bar.clock_panel = None;
        self.toggle_clock_panel(title);
        if let Some(panel) = self.menu_bar.clock_panel.as_mut() {
            panel.pick(day);
        }
    }

    /// Draws the panel again when anything it shows changed, as the day does at midnight.
    pub(crate) fn refresh_clock_panel(&mut self) {
        let dark = self.menu_bar.dark;
        let alt = self
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().alt);
        let options = Options::from_config(&self.config);
        let (_, second_of_day) = calendar_data::date::local_parts(calendar_data::date::now());
        let minute_of_day = second_of_day / SECONDS_IN_MINUTE;
        let revision = self.menu_bar.agenda.revision;
        let agenda = &self.menu_bar.agenda;
        let Some(panel) = self.menu_bar.clock_panel.as_mut() else {
            return;
        };
        panel.today = Date::today();
        panel.alt = alt;
        panel.options = options;
        panel.minute = minute_of_day;
        let shown = Shown {
            dark,
            today: panel.today,
            month: panel.month,
            picked: panel.picked,
            shown_day: panel.shown_day(),
            options: panel.options.clone(),
            agenda: revision,
            expanded: panel.expanded.clone(),
            confirming: panel.confirming.clone(),
            pinned: panel.pinned,
            pressed: panel.pressed,
            hovered: panel.hovered,
            minute: panel.minute,
        };
        if panel.shown.as_ref() != Some(&shown) || panel.picture.is_none() {
            panel.draw(&shown, agenda);
            panel.shown = Some(shown);
        }
        self.menu_bar.clock_picked = Some(panel.picked);
    }

    pub(crate) fn clock_panel_overlay(&self) -> Option<Overlay<'_>> {
        let output = self.bar_area()?;
        let panel = self.menu_bar.clock_panel.as_ref()?;
        let picture = panel.picture.as_ref()?;
        Some(Overlay {
            id: &panel.id,
            rect: Rectangle::new(output.loc + Point::from((panel.x, MENU_BAR_HEIGHT + 4)), picture.size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: ease_out(panel.opened.elapsed().as_secs_f64() / OPEN_TIME) as f32,
            picture,
        })
    }

    /// Where a point of the output is on the panel, if it is on it.
    fn clock_panel_local(&self, local: Point<i32, Logical>) -> Option<Point<i32, Logical>> {
        let panel = self.menu_bar.clock_panel.as_ref()?;
        let size = panel.picture.as_ref()?.size;
        let at = local - Point::from((panel.x, MENU_BAR_HEIGHT + 4));
        Rectangle::new((0, 0).into(), size).contains(at).then_some(at)
    }

    /// A press on the output, at `local`. Returns whether it was on the panel.
    pub(crate) fn clock_panel_press(&mut self, local: Point<i32, Logical>) -> bool {
        let Some(at) = self.clock_panel_local(local) else {
            return false;
        };
        let agenda = &self.menu_bar.agenda;
        if let Some(panel) = self.menu_bar.clock_panel.as_mut() {
            panel.pressed = panel.part_at(agenda, at);
        }
        true
    }

    pub(crate) fn clock_panel_motion(&mut self, local: Point<i32, Logical>) {
        let at = self.clock_panel_local(local);
        let agenda = &self.menu_bar.agenda;
        let Some(panel) = self.menu_bar.clock_panel.as_mut() else {
            return;
        };
        panel.hovered = at.and_then(|at| panel.part_at(agenda, at));
        panel.hovered_day = match (panel.hovered, panel.shown.as_ref()) {
            (Some(Part::Day(cell)), _) => panel.visible_grid().get(cell).copied(),
            _ => None,
        };
    }

    pub(crate) fn clock_panel_release(&mut self, local: Point<i32, Logical>) {
        let under = self.clock_panel_local(local);
        let agenda = &self.menu_bar.agenda;
        let Some(panel) = self.menu_bar.clock_panel.as_mut() else {
            return;
        };
        let under = under.and_then(|at| panel.part_at(agenda, at));
        let pressed = panel.pressed.take();
        if let Some(part) = pressed.filter(|&part| Some(part) == under) {
            self.clock_panel_activate(part);
        }
    }

    /// The entry of the list a part belongs to, as the event it is.
    fn clock_panel_event(&self, at: usize) -> Option<Event> {
        let panel = self.menu_bar.clock_panel.as_ref()?;
        let agenda = &self.menu_bar.agenda;
        match panel.entries(agenda, panel.shown_day()).get(at)? {
            Entry::Event { index, .. } => agenda.events().get(*index).cloned(),
            Entry::Holiday(_) => None,
        }
    }

    fn clock_panel_activate(&mut self, part: Part) {
        let event = match part {
            Part::Item(at) | Part::Check(at) | Part::Edit(at) | Part::Join(at) | Part::Delete(at) | Part::ConfirmAll(at) | Part::ConfirmThis(at) => {
                self.clock_panel_event(at)
            }
            _ => None,
        };
        let Some(panel) = self.menu_bar.clock_panel.as_mut() else {
            return;
        };
        let day = panel.shown_day();
        let grid = panel.visible_grid();
        match part {
            Part::Previous => panel.month = panel.month.shifted(-1),
            Part::Next => panel.month = panel.month.shifted(1),
            Part::Today => panel.pick(panel.today),
            Part::Pin => panel.pinned = !panel.pinned,
            Part::Day(cell) => {
                if let Some(day) = grid.get(cell).copied() {
                    panel.pick(day);
                }
            }
            Part::Item(_) => {
                let Some(event) = event else {
                    return;
                };
                let open = panel.expanded.as_deref() == Some(event.id.as_str());
                panel.expanded = (!open).then_some(event.id);
                panel.confirming = None;
            }
            Part::Check(_) => {
                if let Some(event) = event {
                    self.menu_bar.agenda.change(|events| {
                        if let Some(found) = events.iter_mut().find(|found| found.id == event.id) {
                            found.done = !found.done;
                        }
                    });
                }
            }
            Part::Edit(_) => {
                if let Some(event) = event {
                    self.menu_bar.clock_panel = None;
                    config::spawn(&format!("{CALENDAR_COMMAND} --event {}", event.id));
                }
            }
            Part::Join(_) => {
                if let Some(event) = event {
                    open_url(&event.url);
                }
            }
            Part::Delete(_) => panel.confirming = event.map(|event| event.id),
            Part::Cancel => panel.confirming = None,
            Part::ConfirmAll(_) | Part::ConfirmThis(_) => {
                panel.confirming = None;
                panel.expanded = None;
                if let Some(event) = event {
                    let this_only = matches!(part, Part::ConfirmThis(_));
                    self.menu_bar.agenda.change(|events| calendar_data::event::take_away(events, &event.id, day, this_only));
                }
            }
            Part::New => {
                self.menu_bar.clock_panel = None;
                config::spawn(&format!("{CALENDAR_COMMAND} --new {}", day.key()));
            }
            Part::Calendar => {
                self.menu_bar.clock_panel = None;
                config::spawn(&format!("{CALENDAR_COMMAND} --date {}", day.key()));
            }
            Part::DateTime => {
                self.menu_bar.clock_panel = None;
                config::spawn(DATE_TIME_COMMAND);
            }
        }
    }

    /// A key while the panel is open. Returns whether the panel took it.
    pub(crate) fn clock_panel_key(&mut self, sym: Keysym, typed: Option<char>, shift: bool) -> bool {
        let Some(panel) = self.menu_bar.clock_panel.as_mut() else {
            return false;
        };
        let picked = panel.picked;
        let moved = match sym {
            Keysym::Left => Some(picked.plus_days(-1)),
            Keysym::Right => Some(picked.plus_days(1)),
            Keysym::Up => Some(picked.plus_days(-(DAYS_IN_WEEK as i64))),
            Keysym::Down => Some(picked.plus_days(DAYS_IN_WEEK as i64)),
            Keysym::Page_Up => Some(if shift { picked.plus_years(-1) } else { picked.plus_months(-1) }),
            Keysym::Page_Down => Some(if shift { picked.plus_years(1) } else { picked.plus_months(1) }),
            Keysym::Home => Some(panel.today),
            _ if typed.is_some_and(|key| key.eq_ignore_ascii_case(&'t')) => Some(panel.today),
            _ => None,
        };
        if let Some(day) = moved {
            panel.pick(day);
            return true;
        }
        match (sym, typed) {
            (Keysym::Return | Keysym::KP_Enter, _) => {
                self.menu_bar.clock_panel = None;
                config::spawn(&format!("{CALENDAR_COMMAND} --date {}", picked.key()));
                true
            }
            (_, Some('p' | 'P')) => {
                panel.pinned = !panel.pinned;
                true
            }
            (_, Some('n' | 'N')) => {
                self.menu_bar.clock_panel = None;
                config::spawn(&format!("{CALENDAR_COMMAND} --new {}", picked.key()));
                true
            }
            _ => false,
        }
    }

    pub(crate) fn clock_panel_pinned(&self) -> bool {
        self.menu_bar.clock_panel.as_ref().is_some_and(|panel| panel.pinned)
    }

    /// Whether the point is where the panel is, from the panel's own height.
    pub(crate) fn clock_panel_holds(&self, local: Point<i32, Logical>) -> bool {
        let Some(panel) = self.menu_bar.clock_panel.as_ref() else {
            return false;
        };
        panel.in_rect(local, panel.height(&self.menu_bar.agenda))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn an_event_is_over_when_it_has_ended() {
        let mut event = Event::new(date(2026, 11, 18), "a");
        event.start = Some((9, 0));
        event.end = Some((10, 0));
        assert!(is_over(&event, 10 * 60 + 1) && !is_over(&event, 10 * 60));
        assert!(is_ahead(&event, 8 * 60) && !is_ahead(&event, 9 * 60));
        assert!(!is_over(&Event::new(date(2026, 11, 18), "whole"), 23 * 60));
    }

    #[test]
    fn an_event_is_told_as_when() {
        let mut event = Event::new(date(2026, 11, 18), "a");
        assert_eq!(when_text(&event), "整天");
        event.start = Some((9, 5));
        assert_eq!(when_text(&event), "09:05");
        event.end = Some((10, 0));
        assert_eq!(when_text(&event), "09:05 – 10:00");
    }
}
