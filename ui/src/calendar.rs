//! `nekoland-calendar`: a calendar of GTK's and libadwaita's own widgets, with no stylesheet
//! of its own, so that it looks as the system's GTK style says: it is how Nekoland's look
//! for GTK applications is tried on an application that knows nothing of it. The month, a
//! week, a day or what is coming, with the day chosen beside them and its events and
//! reminders, kept in the files the clock's panel reads, and an editor for them that
//! understands an event typed as a sentence. The grids are drawn by hand (`grid`), in the
//! theme's own colours.
//!
//! `--view day|week|month|agenda` opens on one of the views, `--date DAY` opens on a day, `--new [DAY]` opens the editor on one, `--event ID` opens an
//! event in it, `--add SENTENCE` keeps an event typed as a sentence and `--draft SENTENCE`
//! opens the editor with one typed.

#[path = "calendar/grid.rs"]
mod grid;

use std::{cell::{Cell, RefCell}, rc::Rc};

use adw::prelude::*;
use calendar_data::{
    calendars::{self, color_of, CalendarInfo, HOLIDAYS},
    date::{Date, WEEKDAY_NAMES},
    event::{self, on_day, Event, Kind, Repeat},
    holidays, lunar, natural,
};
use gtk::{gdk, gio, glib, pango};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Calendar";
const SEARCH_WIDTH: i32 = 400;
const PAGE_WIDTH: i32 = 640;
const SIDE_WIDTH: f64 = 340.0;
const WINDOW_WIDTH: i32 = 1200;
const WINDOW_HEIGHT: i32 = 760;
const MONTH_MIN_HEIGHT: i32 = 420;
const AGENDA_DAYS: i64 = 42;
const FIRST_HOUR_SHOWN: f64 = 7.0;
/// Within these hours the grid opens on the one before now; outside them, on the morning.
const WORK_HOURS: (u32, u32) = (8, 19);
const DAYS_IN_WEEK: i64 = 7;
const EDITOR_WIDTH: i32 = 460;
const EDITOR_HEIGHT: i32 = 640;
const MARGIN: i32 = 12;
const PAGE_MARGIN: i32 = 24;
const DOT_SIZE: i32 = 12;
const MINUTE_STEP: f64 = 5.0;
const DEFAULT_HOUR: f64 = 9.0;
const NO_ALERT: &str = "不提醒";
/// The alerts the editor offers, in minutes before an event, and how each reads.
const ALERTS: [(Option<u32>, &str); 9] = [
    (None, NO_ALERT),
    (Some(0), "事件開始時"),
    (Some(5), "5 分鐘前"),
    (Some(10), "10 分鐘前"),
    (Some(15), "15 分鐘前"),
    (Some(30), "30 分鐘前"),
    (Some(60), "1 小時前"),
    (Some(120), "2 小時前"),
    (Some(1440), "1 天前"),
];

thread_local! {
    /// The window's state, for what a second launch asks of the one that is running.
    static STATE: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
}

fn to_date(day: &glib::DateTime) -> Date {
    Date { year: day.year(), month: day.month() as u32, day: day.day_of_month() as u32 }
}

fn to_glib(day: Date) -> glib::DateTime {
    glib::DateTime::from_local(day.year, day.month as i32, day.day as i32, 0, 0, 0.0)
        .unwrap_or_else(|_| glib::DateTime::now_local().expect("the local time"))
}

fn day_text(day: Date) -> String {
    format!("{}年{}月{}日 週{}", day.year, day.month, day.day, WEEKDAY_NAMES[day.weekday()])
}

fn rgba_of((red, green, blue): (f64, f64, f64)) -> gdk::RGBA {
    gdk::RGBA::new(red as f32, green as f32, blue as f32, 1.0)
}

fn rgb_of(color: &gdk::RGBA) -> (f64, f64, f64) {
    (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()))
}

fn clock_text((hour, minute): (u32, u32)) -> String {
    format!("{hour:02}:{minute:02}")
}

/// When an event is, as its row says it.
fn when_text(event: &Event) -> String {
    match (event.start, event.end) {
        (None, _) => "整天".into(),
        (Some(start), Some(end)) => format!("{} – {}", clock_text(start), clock_text(end)),
        (Some(start), None) => clock_text(start),
    }
}

fn now_hour_minute() -> (u32, u32) {
    let (_, second_of_day) = calendar_data::date::local_parts(calendar_data::date::now());
    (second_of_day / 3600, second_of_day / 60 % 60)
}

/// What the window shows and changes.
struct State {
    events: RefCell<Vec<Event>>,
    calendars: RefCell<Vec<CalendarInfo>>,
    query: RefCell<String>,
    /// The day chosen, kept as a calendar that is not shown, for the dates it does.
    month: gtk::Calendar,
    list: gtk::ListBox,
    empty: adw::StatusPage,
    toasts: adw::ToastOverlay,
    window: RefCell<Option<adw::ApplicationWindow>>,
    view: Cell<View>,
    heading: gtk::Label,
    switcher: gtk::Box,
    side_title: gtk::Label,
    stack: gtk::Stack,
    month_area: gtk::DrawingArea,
    time_head: gtk::DrawingArea,
    time_area: gtk::DrawingArea,
    time_scroll: gtk::ScrolledWindow,
    agenda: gtk::Box,
    hits: RefCell<Vec<grid::Hit>>,
    head_hits: RefCell<Vec<grid::Hit>>,
}

/// How the days are laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Day,
    Week,
    Month,
    Agenda,
}

impl View {
    const ALL: [(View, &'static str); 4] =
        [(View::Day, "日"), (View::Week, "週"), (View::Month, "月"), (View::Agenda, "議程")];

    fn page(self) -> &'static str {
        match self {
            View::Day | View::Week => "time",
            View::Month => "month",
            View::Agenda => "agenda",
        }
    }
}

impl State {
    fn chosen(&self) -> Date {
        to_date(&self.month.date())
    }

    fn choose(&self, day: Date) {
        self.month.select_day(&to_glib(day));
    }

    fn shown(&self, calendar: usize) -> bool {
        self.calendars.borrow().get(calendar).is_none_or(|info| info.shown)
    }

    fn calendar_names(&self) -> Vec<String> {
        self.calendars.borrow().iter().map(|info| info.name.clone()).collect()
    }

    fn calendar_name(&self, index: usize) -> String {
        self.calendars.borrow().get(index).map(|info| info.name.clone()).unwrap_or_default()
    }

    /// Reads again what is kept, as after the clock's panel changed it.
    fn reload(self: &Rc<Self>) {
        *self.events.borrow_mut() = event::load();
        *self.calendars.borrow_mut() = calendars::load();
        self.refresh();
    }

    /// Changes the events as they are kept now, and keeps them.
    fn change(self: &Rc<Self>, change: impl FnOnce(&mut Vec<Event>)) {
        let mut events = event::load();
        change(&mut events);
        if let Err(err) = event::save(&events) {
            self.toasts.add_toast(adw::Toast::new(&format!("無法儲存：{err}")));
            return;
        }
        *self.events.borrow_mut() = events;
        self.refresh();
    }

    fn save_calendars(&self) {
        if let Err(err) = calendars::save(&self.calendars.borrow()) {
            self.toasts.add_toast(adw::Toast::new(&format!("無法儲存：{err}")));
        }
    }

    /// The events on the day chosen, or those the search finds on any day, earliest first.
    fn listed(&self) -> Vec<(Event, Date)> {
        let query = self.query.borrow().to_lowercase();
        let events = self.events.borrow();
        if query.is_empty() {
            let day = self.chosen();
            let mut found: Vec<(Event, Date)> = Vec::new();
            if day == Date::today() {
                found.extend(
                    events
                        .iter()
                        .filter(|event| is_late(event, day) && self.shown(event.calendar))
                        .map(|event| (event.clone(), event.date)),
                );
            }
            found.extend(
                on_day(&events, day)
                    .into_iter()
                    .filter(|found| self.shown(found.event.calendar))
                    .map(|found| (found.event.clone(), day)),
            );
            return found;
        }
        let mut found: Vec<(Event, Date)> = events
            .iter()
            .filter(|event| self.shown(event.calendar) && matches_query(event, &query))
            .map(|event| (event.clone(), event.date))
            .collect();
        found.sort_by_key(|(event, day)| (*day, event.start));
        found
    }

    /// Shows the chosen day's events beside the days, and the days again as they are laid out.
    fn refresh(self: &Rc<Self>) {
        let chosen = self.chosen();
        self.side_title.set_label(&format!("{} 月 {} 日 週{}", chosen.month, chosen.day, WEEKDAY_NAMES[chosen.weekday()]));
        self.heading.set_label(&self.heading_text(chosen));

        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let searching = !self.query.borrow().is_empty();
        if !searching {
            if let Some(row) = self.info_row(chosen) {
                self.list.append(&row);
            }
        }
        let listed = self.listed();
        for (event, day) in &listed {
            self.list.append(&self.event_row(event, *day, searching));
        }
        let has_rows = !listed.is_empty() || self.list.first_child().is_some();
        self.list.set_visible(has_rows);
        self.empty.set_visible(listed.is_empty());
        self.empty.set_title(if searching { "找不到行程" } else { "沒有行程" });

        self.stack.set_visible_child_name(self.view.get().page());
        match self.view.get() {
            View::Month => self.month_area.queue_draw(),
            View::Day | View::Week => {
                self.time_head.set_content_height(grid::head_height(&self.time_days()) as i32);
                self.time_head.queue_draw();
                self.time_area.queue_draw();
            }
            View::Agenda => self.fill_agenda(chosen),
        }
    }

    fn heading_text(&self, chosen: Date) -> String {
        match self.view.get() {
            View::Month | View::Agenda => format!("{} 年 {} 月", chosen.year, chosen.month),
            View::Day => format!("{} 年 {} 月 {} 日", chosen.year, chosen.month, chosen.day),
            View::Week => {
                let days = self.time_days();
                let (first, last) = (days[0].date, days[days.len() - 1].date);
                if first.month == last.month {
                    format!("{} 年 {} 月 {} – {} 日", first.year, first.month, first.day, last.day)
                } else {
                    format!("{} 月 {} 日 – {} 月 {} 日", first.month, first.day, last.month, last.day)
                }
            }
        }
    }

    /// The days the time grid shows: the day chosen, or the week it is in.
    fn time_days(&self) -> Vec<grid::Day> {
        let chosen = self.chosen();
        if self.view.get() == View::Day {
            return vec![self.day_data(chosen)];
        }
        let first = chosen.plus_days(-(chosen.weekday() as i64));
        (0..DAYS_IN_WEEK).map(|offset| self.day_data(first.plus_days(offset))).collect()
    }

    /// What is on a day as the grids draw it, and the line under its number.
    fn day_data(&self, day: Date) -> grid::Day {
        let events = self.events.borrow();
        let calendars = self.calendars.borrow();
        let items = on_day(&events, day)
            .into_iter()
            .filter(|found| self.shown(found.event.calendar))
            .map(|found| grid::Item {
                id: found.event.id.clone(),
                date: day,
                title: found.event.title.clone(),
                color: color_of(&calendars, found.event.calendar),
                start: found.event.start,
                end: found.event.end,
                done: found.event.done,
            })
            .collect();
        let holiday = if self.shown(HOLIDAYS) { holidays::on(day).first().map(|found| found.name.to_string()) } else { None };
        match holiday {
            Some(name) => grid::Day { date: day, items, note: name, holiday: true },
            None => {
                let note = lunar::from_solar(day).map(|found| found.label()).unwrap_or_default();
                grid::Day { date: day, items, note, holiday: false }
            }
        }
    }

    fn draw_month(&self, area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let chosen = self.chosen();
        let month = chosen.month_of();
        let lead = month.first_day().weekday();
        let weeks = (lead + month.length() as usize).div_ceil(7);
        let days: Vec<grid::Day> = month.grid(0, weeks).into_iter().map(|day| self.day_data(day)).collect();
        let layout = area.create_pango_layout(None);
        grid::draw_month(
            cr,
            &layout,
            (f64::from(width), f64::from(height)),
            &days,
            (chosen, Date::today(), month.month),
            &grid::Look::of(area),
            &mut self.hits.borrow_mut(),
        );
    }

    fn draw_time(&self, (area, part): (&gtk::DrawingArea, grid::Part), cr: &gtk::cairo::Context, width: i32, height: i32) {
        let days = self.time_days();
        let hits = if part == grid::Part::Head { &self.head_hits } else { &self.hits };
        let layout = area.create_pango_layout(None);
        grid::draw_time(
            cr,
            &layout,
            (f64::from(width), f64::from(height)),
            (part, &days),
            (self.chosen(), Date::today(), now_hour_minute()),
            &grid::Look::of(area),
            &mut hits.borrow_mut(),
        );
    }

    /// What a click on a grid does: a day is chosen, and what is on it opens when it is clicked
    /// twice, or once in the time grid, where it is big enough to be aimed at.
    fn clicked(self: &Rc<Self>, hits: &RefCell<Vec<grid::Hit>>, (x, y): (f64, f64), presses: i32) {
        let target = grid::hit_at(&hits.borrow(), x, y).cloned();
        let single_opens = self.view.get() != View::Month;
        match target {
            Some(grid::Target::Day(day)) => {
                self.choose(day);
                if presses >= 2 {
                    edit_event(self, None, day);
                }
            }
            Some(grid::Target::Item(item)) => {
                self.choose(item.date);
                if presses >= 2 || single_opens {
                    let found = self.events.borrow().iter().find(|event| event.id == item.id).cloned();
                    if let Some(event) = found {
                        edit_event(self, Some(event), item.date);
                    }
                }
            }
            None => {}
        }
    }

    /// The agenda: the days from the one chosen on that have anything, each with its events.
    fn fill_agenda(self: &Rc<Self>, from: Date) {
        while let Some(child) = self.agenda.first_child() {
            self.agenda.remove(&child);
        }
        let mut any = false;
        for offset in 0..AGENDA_DAYS {
            let day = from.plus_days(offset);
            let found: Vec<Event> = {
                let events = self.events.borrow();
                on_day(&events, day)
                    .into_iter()
                    .filter(|found| self.shown(found.event.calendar))
                    .map(|found| found.event.clone())
                    .collect()
            };
            if found.is_empty() {
                continue;
            }
            any = true;
            let today = if day == Date::today() { " · 今天" } else { "" };
            let title = gtk::Label::builder()
                .label(format!("{} 月 {} 日 週{}{today}", day.month, day.day, WEEKDAY_NAMES[day.weekday()]))
                .xalign(0.0)
                .css_classes(["heading"])
                .margin_top(MARGIN)
                .build();
            self.agenda.append(&title);
            let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
            for event in &found {
                list.append(&self.event_row(event, day, false));
            }
            self.agenda.append(&list);
        }
        if !any {
            self.agenda.append(&adw::StatusPage::builder().icon_name("x-office-calendar-symbolic").title("沒有行程").build());
        }
    }

    /// Switches to a view as its button does.
    fn show_view(&self, view: View) {
        let at = View::ALL.iter().position(|(found, _)| *found == view).unwrap_or_default();
        let mut child = self.switcher.first_child();
        for _ in 0..at {
            child = child.and_then(|child| child.next_sibling());
        }
        if let Some(button) = child.and_then(|child| child.downcast::<gtk::ToggleButton>().ok()) {
            button.set_active(true);
        }
    }

    /// Moves the day chosen by what the view shows at a time.
    fn step(&self, direction: i32) {
        let chosen = self.chosen();
        let next = match self.view.get() {
            View::Month => chosen.plus_months(direction),
            View::Day => chosen.plus_days(i64::from(direction)),
            View::Week | View::Agenda => chosen.plus_days(DAYS_IN_WEEK * i64::from(direction)),
        };
        self.choose(next);
    }

    /// The lunar date and holidays of the day, in a row that does nothing.
    fn info_row(&self, day: Date) -> Option<adw::ActionRow> {
        let lunar_text = lunar::from_solar(day).map(|found| format!("農曆{}", found.full()));
        let names: Vec<String> = if self.shown(HOLIDAYS) {
            holidays::on(day).iter().map(|holiday| holiday.name.to_string()).collect()
        } else {
            Vec::new()
        };
        let (title, subtitle) = match (lunar_text, names.is_empty()) {
            (Some(lunar_text), true) => (lunar_text, String::new()),
            (Some(lunar_text), false) => (names.join("、"), lunar_text),
            (None, false) => (names.join("、"), String::new()),
            (None, true) => return None,
        };
        let row = adw::ActionRow::builder().title(glib::markup_escape_text(&title)).build();
        if !subtitle.is_empty() {
            row.set_subtitle(&subtitle);
        }
        row.add_css_class("dim-label");
        Some(row)
    }

    fn event_row(self: &Rc<Self>, event: &Event, day: Date, searching: bool) -> adw::ActionRow {
        let mut parts = Vec::new();
        if searching || is_late(event, Date::today()) && day != self.chosen() {
            parts.push(day_text(day));
        }
        parts.push(when_text(event));
        if !event.location.is_empty() {
            parts.push(event.location.clone());
        }
        parts.push(self.calendar_name(event.calendar));
        if event.repeat != Repeat::Never {
            parts.push(event.repeat.label().to_string());
        }
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&event.title))
            .subtitle(glib::markup_escape_text(&parts.join(" · ")))
            .activatable(true)
            .build();
        if event.done {
            row.add_css_class("dim-label");
        }
        row.add_prefix(&self.event_mark(event));

        let edit = flat_button("document-edit-symbolic", "編輯");
        let delete = flat_button("user-trash-symbolic", "刪除");
        row.add_suffix(&edit);
        row.add_suffix(&delete);
        let open = {
            let (state, event) = (self.clone(), event.clone());
            move || edit_event(&state, Some(event.clone()), event.date)
        };
        edit.connect_clicked(glib::clone!(
            #[strong]
            open,
            move |_| open()
        ));
        row.connect_activated(move |_| open());
        let (state, event) = (self.clone(), event.clone());
        delete.connect_clicked(move |_| confirm_delete(&state, &event, day));
        row
    }

    /// What is before an event: the check of a reminder, or a dot in its calendar's colour.
    fn event_mark(self: &Rc<Self>, event: &Event) -> gtk::Widget {
        if event.kind == Kind::Reminder {
            let check = gtk::CheckButton::builder().active(event.done).valign(gtk::Align::Center).build();
            let (state, id) = (self.clone(), event.id.clone());
            check.connect_toggled(move |check| {
                let done = check.is_active();
                state.change(|events| {
                    if let Some(found) = events.iter_mut().find(|found| found.id == id) {
                        found.done = done;
                    }
                });
            });
            return check.upcast();
        }
        let color = color_of(&self.calendars.borrow(), event.calendar);
        let dot = gtk::DrawingArea::builder()
            .content_width(DOT_SIZE)
            .content_height(DOT_SIZE)
            .valign(gtk::Align::Center)
            .build();
        dot.set_draw_func(move |_, cr, width, height| {
            cr.arc(f64::from(width) / 2.0, f64::from(height) / 2.0, f64::from(width.min(height)) / 2.0, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(color.0, color.1, color.2);
            let _ = cr.fill();
        });
        dot.upcast()
    }
}

/// A reminder of an earlier day that is not done, still listed on today.
fn is_late(event: &Event, today: Date) -> bool {
    event.kind == Kind::Reminder && !event.done && event.repeat == Repeat::Never && event.date < today
}

fn matches_query(event: &Event, query: &str) -> bool {
    [&event.title, &event.location, &event.notes]
        .iter()
        .any(|field| field.to_lowercase().contains(query))
}

fn flat_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

/// Takes an event away. One that repeats asks whether it is this day only or those after it
/// too; either way it can be put back.
fn confirm_delete(state: &Rc<State>, event: &Event, day: Date) {
    if event.repeat == Repeat::Never {
        take_away(state, event, day, false);
        return;
    }
    let Some(window) = state.window.borrow().clone() else {
        return;
    };
    let dialog = adw::AlertDialog::new(Some(&format!("刪除「{}」？", event.title)), None);
    dialog.add_responses(&[("cancel", "取消"), ("this", "只刪除這次"), ("all", "刪除之後全部")]);
    dialog.set_response_appearance("all", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let (state, event) = (state.clone(), event.clone());
    dialog.connect_response(None, move |_, response| match response {
        "this" => take_away(&state, &event, day, true),
        "all" => take_away(&state, &event, day, false),
        _ => {}
    });
    dialog.present(Some(&window));
}

fn take_away(state: &Rc<State>, event: &Event, day: Date, this_only: bool) {
    let id = event.id.clone();
    state.change(|events| event::take_away(events, &id, day, this_only));
    let toast = adw::Toast::builder()
        .title(format!("已刪除「{}」", event.title))
        .button_label("復原")
        .build();
    let (state_again, original) = (state.clone(), event.clone());
    toast.connect_button_clicked(move |_| state_again.change(|events| event::restore(events, original.clone())));
    state.toasts.add_toast(toast);
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_activate(build);
    app.connect_command_line(|app, line| {
        app.activate();
        let arguments: Vec<String> = line
            .arguments()
            .iter()
            .skip(1)
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        if let Some(state) = STATE.with(|state| state.borrow().clone()) {
            follow_arguments(&state, &arguments);
        }
        0
    });
    app.run()
}

/// Reads a day given as 2026-12-25 or in words.
fn read_day(text: &str) -> Option<Date> {
    Date::parse_key(text).or_else(|| natural::parse(&format!("日 {text}"), Date::today(), &[], now_hour_minute()).date)
}

/// What the command line asks of the window that is open.
fn follow_arguments(state: &Rc<State>, arguments: &[String]) {
    let mut at = 0;
    while at < arguments.len() {
        let value = arguments.get(at + 1).filter(|value| !value.starts_with("--"));
        match (arguments[at].as_str(), value) {
            ("--date", Some(text)) => {
                if let Some(day) = read_day(text) {
                    state.choose(day);
                }
            }
            ("--view", Some(name)) => match View::ALL.iter().find(|(_, label)| view_name(label) == name.as_str()) {
                Some((view, _)) => state.show_view(*view),
                None => eprintln!("nekoland-calendar: {name} is not a view"),
            },
            ("--new", value) => {
                let day = value.and_then(|text| read_day(text)).unwrap_or_else(|| state.chosen());
                state.choose(day);
                edit_event(state, None, day);
            }
            ("--event", Some(id)) => open_event(state, id),
            ("--add", Some(sentence)) => add_sentence(state, sentence),
            ("--draft", Some(sentence)) => draft_event(state, sentence),
            (other, _) => eprintln!("nekoland-calendar: {other} is not understood"),
        }
        at += if value.is_some() { 2 } else { 1 };
    }
}

/// How a view is named on the command line.
fn view_name(label: &str) -> &'static str {
    match label {
        "日" => "day",
        "週" => "week",
        "月" => "month",
        _ => "agenda",
    }
}

fn open_event(state: &Rc<State>, id: &str) {
    let found = state.events.borrow().iter().find(|event| event.id == id).cloned();
    match found {
        Some(event) => {
            state.choose(event.date);
            edit_event(state, Some(event.clone()), event.date);
        }
        None => state.toasts.add_toast(adw::Toast::new("找不到這個行程")),
    }
}

/// Keeps an event typed as a sentence, on the day it names or today.
fn add_sentence(state: &Rc<State>, sentence: &str) {
    let parsed = natural::parse(sentence, Date::today(), &state.calendar_names(), now_hour_minute());
    let event = event_from_parsed(&parsed, sentence, state.chosen());
    state.choose(event.date);
    state.change(|events| events.push(event));
}

fn event_from_parsed(parsed: &natural::Parsed, sentence: &str, fallback: Date) -> Event {
    let title = if parsed.title.trim().is_empty() { sentence.trim() } else { parsed.title.trim() };
    let mut event = Event::new(parsed.date.unwrap_or(fallback), title);
    event.start = parsed.start.filter(|_| !parsed.all_day);
    event.end = parsed.end.filter(|_| event.start.is_some());
    event.calendar = parsed.calendar.unwrap_or(0);
    event
}

fn build(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }
    let state = Rc::new(State {
        events: RefCell::new(event::load()),
        calendars: RefCell::new(calendars::load()),
        query: RefCell::default(),
        month: gtk::Calendar::new(),
        list: gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
            .build(),
        empty: adw::StatusPage::builder().icon_name("x-office-calendar-symbolic").vexpand(true).build(),
        toasts: adw::ToastOverlay::new(),
        window: RefCell::new(None),
        view: Cell::new(View::Month),
        heading: gtk::Label::builder().css_classes(["title-3"]).build(),
        switcher: gtk::Box::builder().css_classes(["linked"]).build(),
        side_title: gtk::Label::builder().css_classes(["title-2"]).xalign(0.0).build(),
        stack: gtk::Stack::builder().vexpand(true).hexpand(true).build(),
        month_area: gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .content_height(MONTH_MIN_HEIGHT)
            .margin_start(MARGIN)
            .margin_end(MARGIN)
            .margin_bottom(MARGIN)
            .build(),
        time_head: gtk::DrawingArea::builder().hexpand(true).build(),
        time_area: gtk::DrawingArea::builder().hexpand(true).content_height(grid::body_height() as i32).build(),
        time_scroll: gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).build(),
        agenda: gtk::Box::builder().orientation(gtk::Orientation::Vertical).build(),
        hits: RefCell::default(),
        head_hits: RefCell::default(),
    });

    build_views(&state);
    let (side_view, add) = build_side(&state);
    let split = adw::OverlaySplitView::builder()
        .sidebar_position(gtk::PackType::End)
        .content(&build_main(&state))
        .sidebar(&side_view)
        .min_sidebar_width(SIDE_WIDTH)
        .max_sidebar_width(SIDE_WIDTH)
        .build();
    state.toasts.set_child(Some(&split));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("行事曆")
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .content(&state.toasts)
        .build();
    *state.window.borrow_mut() = Some(window.clone());

    connect_month(&state);
    add.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |_| edit_event(&state, None, state.chosen())
    ));
    add_actions(app, &state, &window);
    watch_files(&state);
    STATE.with(|slot| *slot.borrow_mut() = Some(state.clone()));

    state.refresh();
    scroll_to_hours(&state);
    window.present();
}

/// The pages of what is beside the side: the month, the hours, and what is coming.
fn build_views(state: &Rc<State>) {
    for (area, head) in [(&state.month_area, false), (&state.time_area, false), (&state.time_head, true)] {
        let click = gtk::GestureClick::new();
        click.connect_pressed(glib::clone!(
            #[strong]
            state,
            move |_, presses, x, y| {
                let hits = if head { &state.head_hits } else { &state.hits };
                state.clicked(hits, (x, y), presses);
            }
        ));
        area.add_controller(click);
    }
    state.month_area.set_draw_func(glib::clone!(
        #[strong]
        state,
        move |area, cr, width, height| state.draw_month(area, cr, width, height)
    ));
    for (area, part) in [(&state.time_head, grid::Part::Head), (&state.time_area, grid::Part::Body)] {
        area.set_draw_func(glib::clone!(
            #[strong]
            state,
            move |area, cr, width, height| state.draw_time((area, part), cr, width, height)
        ));
    }
    state.time_scroll.set_child(Some(&state.time_area));
    state.time_scroll.set_vexpand(true);
    let time_page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    time_page.append(&state.time_head);
    time_page.append(&state.time_scroll);
    let agenda = adw::Clamp::builder()
        .maximum_size(PAGE_WIDTH)
        .margin_top(MARGIN)
        .margin_bottom(PAGE_MARGIN)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .child(&state.agenda)
        .build();
    state.stack.add_named(&state.month_area, Some(View::Month.page()));
    state.stack.add_named(&time_page, Some(View::Day.page()));
    state.stack.add_named(
        &gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&agenda).build(),
        Some(View::Agenda.page()),
    );
}

/// Puts the time grid where the day's work begins, or where it is now.
fn scroll_to_hours(state: &Rc<State>) {
    let state = state.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
        let (hour, _) = now_hour_minute();
        let wanted = if (WORK_HOURS.0..WORK_HOURS.1).contains(&hour) { f64::from(hour) - 1.0 } else { FIRST_HOUR_SHOWN };
        state.time_scroll.vadjustment().set_value(grid::hour_y(wanted));
    });
}

/// The side: the day chosen with its events, and the calendars, each shown or not, named and
/// coloured.
fn build_side(state: &Rc<State>) -> (adw::ToolbarView, gtk::Button) {
    let group = adw::PreferencesGroup::builder().title("行事曆").build();
    let count = state.calendars.borrow().len();
    for index in 0..count {
        group.add(&calendar_row(state, index));
    }
    let add = gtk::Button::builder().label("新增行程").css_classes(["suggested-action", "pill"]).halign(gtk::Align::Center).build();
    let side = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(MARGIN)
        .margin_top(MARGIN)
        .margin_bottom(MARGIN)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    side.append(&state.side_title);
    side.append(&state.list);
    side.append(&state.empty);
    side.append(&group);
    side.append(&add);
    let bar = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .title_widget(&gtk::Label::new(None))
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&bar);
    view.set_content(Some(
        &gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&side).build(),
    ));
    (view, add)
}

fn calendar_row(state: &Rc<State>, index: usize) -> adw::EntryRow {
    let info = state.calendars.borrow()[index].clone();
    let row = adw::EntryRow::builder().title("名稱").text(&info.name).show_apply_button(true).build();
    let check = gtk::CheckButton::builder().active(info.shown).valign(gtk::Align::Center).tooltip_text("顯示").build();
    let color = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    color.set_rgba(&rgba_of(info.color));
    color.set_valign(gtk::Align::Center);
    color.set_tooltip_text(Some("顏色"));
    row.add_prefix(&check);
    row.add_suffix(&color);
    check.connect_toggled(glib::clone!(
        #[strong]
        state,
        move |check| {
            state.calendars.borrow_mut()[index].shown = check.is_active();
            state.save_calendars();
            state.refresh();
        }
    ));
    color.connect_rgba_notify(glib::clone!(
        #[strong]
        state,
        move |color| {
            state.calendars.borrow_mut()[index].color = rgb_of(&color.rgba());
            state.save_calendars();
            state.refresh();
        }
    ));
    row.connect_apply(glib::clone!(
        #[strong]
        state,
        move |row| {
            let name = row.text().trim().to_string();
            if name.is_empty() {
                row.set_text(&state.calendar_name(index));
                return;
            }
            state.calendars.borrow_mut()[index].name = name;
            state.save_calendars();
            state.refresh();
        }
    ));
    row
}

/// What is beside the side: how the days are laid out, a place to type an event as a
/// sentence, a search over all of them, and a menu.
fn build_main(state: &Rc<State>) -> adw::ToolbarView {
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋行程").build();
    let search_bar = gtk::SearchBar::builder()
        .child(&adw::Clamp::builder().maximum_size(SEARCH_WIDTH).child(&search).build())
        .build();
    search_bar.connect_entry(&search);
    let find = gtk::ToggleButton::builder().icon_name("system-search-symbolic").tooltip_text("搜尋").build();
    find.bind_property("active", &search_bar, "search-mode-enabled").bidirectional().build();
    search.connect_search_changed(glib::clone!(
        #[strong]
        state,
        move |search| {
            *state.query.borrow_mut() = search.text().to_string();
            state.refresh();
        }
    ));
    let menu = gio::Menu::new();
    menu.append(Some("今天"), Some("app.today"));
    menu.append(Some("刪除所有行程⋯"), Some("app.clear"));

    let previous = flat_button("go-previous-symbolic", "上一頁");
    let next = flat_button("go-next-symbolic", "下一頁");
    let today = gtk::Button::with_label("今天");
    previous.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |_| state.step(-1)
    ));
    next.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |_| state.step(1)
    ));
    today.connect_clicked(glib::clone!(
        #[strong]
        state,
        move |_| {
            state.choose(Date::today());
            state.refresh();
        }
    ));

    let bar = adw::HeaderBar::builder().show_end_title_buttons(false).title_widget(&view_switcher(state)).build();
    bar.pack_start(&previous);
    bar.pack_start(&next);
    bar.pack_start(&today);
    bar.pack_start(&state.heading);
    bar.pack_end(
        &gtk::MenuButton::builder().icon_name("open-menu-symbolic").tooltip_text("選單").menu_model(&menu).build(),
    );
    bar.pack_end(&find);

    let quick = gtk::Entry::builder()
        .placeholder_text("輸入一句話新增行程")
        .primary_icon_name("list-add-symbolic")
        .margin_top(MARGIN)
        .margin_bottom(MARGIN)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    quick.connect_activate(glib::clone!(
        #[strong]
        state,
        move |entry| {
            let sentence = entry.text().trim().to_string();
            if !sentence.is_empty() {
                add_sentence(&state, &sentence);
                entry.set_text("");
            }
        }
    ));

    let view = adw::ToolbarView::new();
    view.add_top_bar(&bar);
    view.add_top_bar(&search_bar);
    view.add_top_bar(&quick);
    view.set_content(Some(&state.stack));
    view
}

/// The days, the week, the month or what is coming, one at a time.
fn view_switcher(state: &Rc<State>) -> gtk::Box {
    let switcher = state.switcher.clone();
    let mut first: Option<gtk::ToggleButton> = None;
    for (view, label) in View::ALL {
        let button = gtk::ToggleButton::builder().label(label).active(view == state.view.get()).build();
        match &first {
            Some(first) => button.set_group(Some(first)),
            None => first = Some(button.clone()),
        }
        button.connect_toggled(glib::clone!(
            #[strong]
            state,
            move |button| {
                if button.is_active() {
                    state.view.set(view);
                    state.refresh();
                    if matches!(view, View::Day | View::Week) {
                        scroll_to_hours(&state);
                    }
                }
            }
        ));
        switcher.append(&button);
    }
    switcher
}

fn connect_month(state: &Rc<State>) {
    state.month.connect_day_selected(glib::clone!(
        #[strong]
        state,
        move |_| state.refresh()
    ));
}

fn add_actions(app: &adw::Application, state: &Rc<State>, window: &adw::ApplicationWindow) {
    let today = gio::SimpleAction::new("today", None);
    today.connect_activate(glib::clone!(
        #[strong]
        state,
        move |_, _| state.choose(Date::today())
    ));
    app.add_action(&today);
    let clear = gio::SimpleAction::new("clear", None);
    clear.connect_activate(glib::clone!(
        #[strong]
        state,
        #[weak]
        window,
        move |_, _| {
            let dialog = adw::AlertDialog::new(Some("刪除所有行程？"), Some("所有日期的行程都會刪除，無法復原。"));
            dialog.add_responses(&[("cancel", "取消"), ("delete", "刪除")]);
            dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            dialog.connect_response(
                Some("delete"),
                glib::clone!(
                    #[strong]
                    state,
                    move |_, _| state.change(Vec::clear)
                ),
            );
            dialog.present(Some(&window));
        }
    ));
    app.add_action(&clear);
}

/// Reads again when another program, such as the clock's panel, changes what is kept.
fn watch_files(state: &Rc<State>) {
    let Some(directory) = event::data_dir() else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(&directory) {
        eprintln!("{}: {err}", directory.display());
        return;
    }
    let monitor = gio::File::for_path(&directory).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE);
    let Ok(monitor) = monitor else {
        return;
    };
    monitor.connect_changed(glib::clone!(
        #[weak]
        state,
        move |_, file, _, _| {
            let name = file.basename().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if matches!(name.as_str(), "calendar.tsv" | "calendars.tsv") {
                state.reload();
            }
        }
    ));
    // The window is the monitor's owner for as long as it is open.
    std::mem::forget(monitor);
}

/// What the editor holds.
struct Editor {
    title: adw::EntryRow,
    day: DatePicker,
    reminder: adw::SwitchRow,
    all_day: adw::SwitchRow,
    start: TimeRows,
    has_end: adw::SwitchRow,
    end: TimeRows,
    location: adw::EntryRow,
    url: adw::EntryRow,
    notes: gtk::TextView,
    alert: adw::ComboRow,
    repeat: adw::ComboRow,
    has_until: adw::SwitchRow,
    until: DatePicker,
    calendar: adw::ComboRow,
}

struct TimeRows {
    hour: adw::SpinRow,
    minute: adw::SpinRow,
}

impl TimeRows {
    fn new(title: &str) -> Self {
        let hour = adw::SpinRow::with_range(0.0, 23.0, 1.0);
        hour.set_title(&format!("{title}時"));
        hour.set_value(DEFAULT_HOUR);
        let minute = adw::SpinRow::with_range(0.0, 59.0, MINUTE_STEP);
        minute.set_title(&format!("{title}分"));
        Self { hour, minute }
    }

    fn rows(&self) -> [&adw::SpinRow; 2] {
        [&self.hour, &self.minute]
    }

    fn get(&self) -> (u32, u32) {
        (self.hour.value() as u32, self.minute.value() as u32)
    }

    fn set(&self, (hour, minute): (u32, u32)) {
        self.hour.set_value(f64::from(hour));
        self.minute.set_value(f64::from(minute));
    }
}

/// A day chosen from a calendar that opens from a button.
#[derive(Clone)]
struct DatePicker {
    row: adw::ActionRow,
    day: Rc<Cell<Date>>,
    calendar: gtk::Calendar,
    button: gtk::MenuButton,
}

impl DatePicker {
    fn new(title: &str, day: Date) -> Self {
        let calendar = gtk::Calendar::new();
        let button = gtk::MenuButton::builder()
            .popover(&gtk::Popover::builder().child(&calendar).build())
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&button);
        let picker = Self { row, day: Rc::new(Cell::new(day)), calendar, button };
        picker.set(day);
        let shared = picker.clone();
        picker.calendar.connect_day_selected(move |calendar| shared.set(to_date(&calendar.date())));
        picker
    }

    fn get(&self) -> Date {
        self.day.get()
    }

    fn set(&self, day: Date) {
        self.day.set(day);
        self.button.set_label(&day_text(day));
        if to_date(&self.calendar.date()) != day {
            self.calendar.select_day(&to_glib(day));
        }
    }
}

fn switch_row(title: &str, active: bool) -> adw::SwitchRow {
    adw::SwitchRow::builder().title(title).active(active).build()
}

fn combo_row(title: &str, labels: &[&str]) -> adw::ComboRow {
    adw::ComboRow::builder().title(title).model(&gtk::StringList::new(labels)).build()
}

impl Editor {
    fn new(state: &State, existing: Option<&Event>, day: Date) -> Self {
        let names: Vec<String> = state.calendar_names();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let repeats: Vec<&str> = Repeat::ALL.iter().map(|repeat| repeat.label()).collect();
        let alerts: Vec<&str> = ALERTS.iter().map(|(_, label)| *label).collect();
        let editor = Self {
            title: adw::EntryRow::builder().title("標題").activates_default(true).build(),
            day: DatePicker::new("日期", day),
            reminder: switch_row("提醒事項", false),
            all_day: switch_row("整天", false),
            start: TimeRows::new("開始"),
            has_end: switch_row("結束時間", false),
            end: TimeRows::new("結束"),
            location: adw::EntryRow::builder().title("地點").build(),
            url: adw::EntryRow::builder().title("連結").build(),
            notes: gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .top_margin(8)
                .bottom_margin(8)
                .left_margin(12)
                .right_margin(12)
                .height_request(72)
                .build(),
            alert: combo_row("提醒", &alerts),
            repeat: combo_row("重複", &repeats),
            has_until: switch_row("結束重複", false),
            until: DatePicker::new("重複至", day.plus_months(1)),
            calendar: combo_row("行事曆", &name_refs),
        };
        editor.link_sensitivity();
        if let Some(event) = existing {
            editor.fill(event);
        }
        editor
    }

    /// Rows that mean nothing while what they depend on is off are dimmed.
    fn link_sensitivity(&self) {
        let timed = |row: &adw::SwitchRow, target: &gtk::Widget| {
            row.bind_property("active", target, "sensitive").invert_boolean().sync_create().build();
        };
        for row in self.start.rows() {
            timed(&self.all_day, row.upcast_ref());
        }
        timed(&self.all_day, self.has_end.upcast_ref());
        timed(&self.all_day, self.alert.upcast_ref());
        for row in self.end.rows() {
            self.has_end.bind_property("active", row, "sensitive").sync_create().build();
        }
        self.has_end
            .bind_property("active", &self.end.hour, "visible")
            .sync_create()
            .build();
        self.has_end
            .bind_property("active", &self.end.minute, "visible")
            .sync_create()
            .build();
        self.has_until
            .bind_property("active", &self.until.row, "visible")
            .sync_create()
            .build();
        let repeat = self.repeat.clone();
        let has_until = self.has_until.clone();
        let sync = move |repeat: &adw::ComboRow| has_until.set_visible(repeat.selected() != 0);
        sync(&repeat);
        repeat.connect_selected_notify(sync);
    }

    fn fill(&self, event: &Event) {
        self.title.set_text(&event.title);
        self.day.set(event.date);
        self.reminder.set_active(event.kind == Kind::Reminder);
        self.all_day.set_active(event.all_day());
        if let Some(start) = event.start {
            self.start.set(start);
        }
        if let Some(end) = event.end {
            self.has_end.set_active(true);
            self.end.set(end);
        }
        self.location.set_text(&event.location);
        self.url.set_text(&event.url);
        self.notes.buffer().set_text(&event.notes);
        let alert = ALERTS.iter().position(|(minutes, _)| *minutes == event.alert).unwrap_or(0);
        self.alert.set_selected(alert as u32);
        let repeat = Repeat::ALL.iter().position(|repeat| *repeat == event.repeat).unwrap_or(0);
        self.repeat.set_selected(repeat as u32);
        if let Some(until) = event.until {
            self.has_until.set_active(true);
            self.until.set(until);
        }
        self.calendar.set_selected(event.calendar as u32);
    }

    fn notes_text(&self) -> String {
        let buffer = self.notes.buffer();
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
    }

    /// The event the editor holds, built on `base` so that what the editor does not hold is kept.
    fn event(&self, base: Event) -> Event {
        let all_day = self.all_day.is_active();
        let repeat = Repeat::ALL.get(self.repeat.selected() as usize).copied().unwrap_or_default();
        Event {
            kind: if self.reminder.is_active() { Kind::Reminder } else { Kind::Event },
            date: self.day.get(),
            start: (!all_day).then(|| self.start.get()),
            end: (!all_day && self.has_end.is_active()).then(|| self.end.get()),
            calendar: self.calendar.selected() as usize,
            title: self.title.text().trim().to_string(),
            location: self.location.text().trim().to_string(),
            url: self.url.text().trim().to_string(),
            notes: self.notes_text().trim().to_string(),
            alert: if all_day { None } else { ALERTS.get(self.alert.selected() as usize).and_then(|(minutes, _)| *minutes) },
            repeat,
            until: (repeat != Repeat::Never && self.has_until.is_active()).then(|| self.until.get()),
            ..base
        }
    }

    /// Sets the rows a sentence in the title says something about, and marks the words.
    fn follow_sentence(&self, state: &State) {
        let sentence = self.title.text().to_string();
        let parsed = natural::parse(&sentence, Date::today(), &state.calendar_names(), now_hour_minute());
        if let Some(day) = parsed.date {
            self.day.set(day);
        }
        if parsed.all_day {
            self.all_day.set_active(true);
        } else if let Some(start) = parsed.start {
            self.all_day.set_active(false);
            self.start.set(start);
        }
        if let (Some(end), false) = (parsed.end, self.all_day.is_active()) {
            self.has_end.set_active(true);
            self.end.set(end);
        }
        if let Some(calendar) = parsed.calendar {
            self.calendar.set_selected(calendar as u32);
        }
        mark_words(&self.title, &parsed.marks);
    }

    /// The title without what the sentence said about the day and time.
    fn clean_title(&self, state: &State) -> String {
        let sentence = self.title.text().to_string();
        let parsed = natural::parse(&sentence, Date::today(), &state.calendar_names(), now_hour_minute());
        if parsed.title.trim().is_empty() {
            sentence.trim().to_string()
        } else {
            parsed.title.trim().to_string()
        }
    }
}

/// Underlines the words of a sentence that were taken as a day or a time.
fn mark_words(entry: &adw::EntryRow, marks: &[std::ops::Range<usize>]) {
    let Some(text) = entry.delegate().and_then(|delegate| delegate.downcast::<gtk::Text>().ok()) else {
        return;
    };
    let list = pango::AttrList::new();
    for mark in marks {
        let mut underline = pango::AttrInt::new_underline(pango::Underline::Single);
        underline.set_start_index(mark.start as u32);
        underline.set_end_index(mark.end as u32);
        list.insert(underline);
        let mut weight = pango::AttrInt::new_weight(pango::Weight::Bold);
        weight.set_start_index(mark.start as u32);
        weight.set_end_index(mark.end as u32);
        list.insert(weight);
    }
    text.set_attributes(Some(&list));
}

fn group_of(rows: &[&gtk::Widget]) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    for row in rows {
        group.add(*row);
    }
    group
}

fn notes_group(editor: &Editor) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("備註").build();
    let frame = gtk::Frame::builder().child(&editor.notes).build();
    group.add(&frame);
    group
}

/// The editor, for a new event on `day` or for `existing`.
fn edit_event(state: &Rc<State>, existing: Option<Event>, day: Date) {
    open_editor(state, existing, day, None);
}

/// The editor for a new event, with a sentence already typed in its title.
fn draft_event(state: &Rc<State>, sentence: &str) {
    open_editor(state, None, state.chosen(), Some(sentence));
}

fn open_editor(state: &Rc<State>, existing: Option<Event>, day: Date, draft: Option<&str>) {
    let Some(window) = state.window.borrow().clone() else {
        return;
    };
    let editor = Rc::new(Editor::new(state, existing.as_ref(), day));
    let is_new = existing.is_none();

    let page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(MARGIN).build();
    page.append(&group_of(&[editor.title.upcast_ref(), editor.day.row.upcast_ref(), editor.reminder.upcast_ref()]));
    page.append(&group_of(&[
        editor.all_day.upcast_ref(),
        editor.start.hour.upcast_ref(),
        editor.start.minute.upcast_ref(),
        editor.has_end.upcast_ref(),
        editor.end.hour.upcast_ref(),
        editor.end.minute.upcast_ref(),
    ]));
    page.append(&group_of(&[editor.location.upcast_ref(), editor.url.upcast_ref()]));
    page.append(&notes_group(&editor));
    page.append(&group_of(&[
        editor.alert.upcast_ref(),
        editor.repeat.upcast_ref(),
        editor.has_until.upcast_ref(),
        editor.until.row.upcast_ref(),
        editor.calendar.upcast_ref(),
    ]));
    if let Some(event) = existing.as_ref() {
        page.append(&delete_button(state, event));
    }

    let cancel = gtk::Button::with_label("取消");
    let save = gtk::Button::builder().label(if is_new { "新增" } else { "儲存" }).css_classes(["suggested-action"]).build();
    let bar = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
    bar.pack_start(&cancel);
    bar.pack_end(&save);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&bar);
    view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().margin_top(MARGIN).margin_bottom(MARGIN).margin_start(MARGIN).margin_end(MARGIN).child(&page).build())
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title(if is_new { "新增行程" } else { "編輯行程" })
        .content_width(EDITOR_WIDTH)
        .content_height(EDITOR_HEIGHT)
        .child(&view)
        .default_widget(&save)
        .build();

    save.set_sensitive(!editor.title.text().trim().is_empty());
    editor.title.connect_changed(glib::clone!(
        #[strong]
        editor,
        #[strong]
        state,
        #[weak]
        save,
        move |title| {
            save.set_sensitive(!title.text().trim().is_empty());
            if is_new {
                editor.follow_sentence(&state);
            }
        }
    ));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    save.connect_clicked(glib::clone!(
        #[strong]
        state,
        #[strong]
        editor,
        #[weak]
        dialog,
        move |_| {
            let base = existing.clone().unwrap_or_else(|| Event::new(editor.day.get(), ""));
            let mut event = editor.event(base);
            if is_new {
                event.title = editor.clean_title(&state);
            }
            let id = event.id.clone();
            state.change(|events| match events.iter_mut().find(|found| found.id == id) {
                Some(found) => *found = event.clone(),
                None => events.push(event.clone()),
            });
            state.choose(event.date);
            dialog.close();
        }
    ));
    dialog.present(Some(&window));
    editor.title.grab_focus();
    if let Some(sentence) = draft {
        editor.title.set_text(sentence);
        editor.title.set_position(-1);
    }
}

fn delete_button(state: &Rc<State>, event: &Event) -> gtk::Button {
    let button = gtk::Button::builder()
        .label("刪除")
        .halign(gtk::Align::Center)
        .css_classes(["destructive-action"])
        .build();
    let (state, event) = (state.clone(), event.clone());
    button.connect_clicked(move |button| {
        if let Some(dialog) = button.ancestor(adw::Dialog::static_type()).and_downcast::<adw::Dialog>() {
            dialog.close();
        }
        confirm_delete(&state, &event, event.date);
    });
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentence_makes_an_event_with_what_it_says() {
        let today = Date { year: 2026, month: 11, day: 18 };
        let parsed = natural::parse("牙醫 11/20 14:00-15:00", today, &[], (9, 0));
        let event = event_from_parsed(&parsed, "牙醫 11/20 14:00-15:00", today);
        assert_eq!(event.title, "牙醫");
        assert_eq!((event.date, event.start, event.end), (Date { year: 2026, month: 11, day: 20 }, Some((14, 0)), Some((15, 0))));
    }

    #[test]
    fn a_sentence_that_is_all_date_keeps_it_as_the_title() {
        let today = Date { year: 2026, month: 11, day: 18 };
        let parsed = natural::parse("明天", today, &[], (9, 0));
        let event = event_from_parsed(&parsed, "明天", today);
        assert!(!event.title.is_empty());
    }

    #[test]
    fn a_reminder_of_an_earlier_day_is_late_until_it_is_done() {
        let today = Date { year: 2026, month: 11, day: 18 };
        let mut reminder = Event::new(today.plus_days(-2), "繳電費");
        reminder.kind = Kind::Reminder;
        assert!(is_late(&reminder, today));
        reminder.done = true;
        assert!(!is_late(&reminder, today));
        assert!(!is_late(&Event::new(today.plus_days(-2), "開會"), today));
    }

    #[test]
    fn the_times_of_an_event_are_told_as_a_range() {
        let mut event = Event::new(Date { year: 2026, month: 11, day: 18 }, "a");
        assert_eq!(when_text(&event), "整天");
        event.start = Some((9, 5));
        assert_eq!(when_text(&event), "09:05");
        event.end = Some((10, 0));
        assert_eq!(when_text(&event), "09:05 – 10:00");
    }

    #[test]
    fn a_search_looks_in_the_title_the_place_and_the_notes() {
        let mut event = Event::new(Date { year: 2026, month: 11, day: 18 }, "Dentist");
        event.location = "小港".into();
        assert!(matches_query(&event, "dent") && matches_query(&event, "小港") && !matches_query(&event, "會議"));
    }
}
