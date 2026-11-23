//! Events and reminders as Calendar keeps them in one file, a line each, and how they
//! repeat. A line begins with the four columns older versions wrote (day, time, calendar and
//! title) and goes on with `key=value` columns for what came after, so that what an older
//! version wrote is still read, and its reading of a newer line only shows more title.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::date::Date;

const NO_TIME: &str = "-";
const COLUMN_SEPARATOR: char = '\t';
const BASE_COLUMNS: usize = 4;
/// What a title or a value may not hold stands in for it with a space or an escape.
const NEWLINE_ESCAPE: &str = "\\n";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Event,
    /// Something to do, which is done or is not rather than begun and ended.
    Reminder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Never,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Repeat {
    pub const ALL: [Repeat; 5] = [Repeat::Never, Repeat::Daily, Repeat::Weekly, Repeat::Monthly, Repeat::Yearly];

    pub fn name(self) -> &'static str {
        match self {
            Repeat::Never => "never",
            Repeat::Daily => "daily",
            Repeat::Weekly => "weekly",
            Repeat::Monthly => "monthly",
            Repeat::Yearly => "yearly",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Never => "不重複",
            Repeat::Daily => "每天",
            Repeat::Weekly => "每週",
            Repeat::Monthly => "每月",
            Repeat::Yearly => "每年",
        }
    }

    fn from_name(name: &str) -> Self {
        Self::ALL.into_iter().find(|repeat| repeat.name() == name).unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Which of the events this is, so that one can be named though the file is rewritten.
    pub id: String,
    pub kind: Kind,
    /// The day it is on, or begins on when it repeats.
    pub date: Date,
    /// When it begins and when it ends, as hour and minute; none for one that takes the day.
    pub start: Option<(u32, u32)>,
    pub end: Option<(u32, u32)>,
    pub calendar: usize,
    pub title: String,
    pub location: String,
    pub url: String,
    pub notes: String,
    /// How many minutes before it begins it says so.
    pub alert: Option<u32>,
    pub repeat: Repeat,
    /// The last day it repeats on.
    pub until: Option<Date>,
    /// A reminder that is done.
    pub done: bool,
    /// The days of a repeating event that are left out.
    pub skipped: Vec<Date>,
}

/// One day of an event: it, and the day it is on there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occurrence<'a> {
    /// Where the event is in the list it came from.
    pub index: usize,
    pub event: &'a Event,
    pub date: Date,
}

impl Event {
    pub fn new(date: Date, title: &str) -> Self {
        Self {
            id: new_id(),
            kind: Kind::Event,
            date,
            start: None,
            end: None,
            calendar: 0,
            title: title.to_string(),
            location: String::new(),
            url: String::new(),
            notes: String::new(),
            alert: None,
            repeat: Repeat::Never,
            until: None,
            done: false,
            skipped: Vec::new(),
        }
    }

    pub fn all_day(&self) -> bool {
        self.start.is_none()
    }

    /// Whether it is on `day`.
    pub fn occurs_on(&self, day: Date) -> bool {
        if day < self.date || self.until.is_some_and(|until| day > until) || self.skipped.contains(&day) {
            return false;
        }
        match self.repeat {
            Repeat::Never => day == self.date,
            Repeat::Daily => true,
            Repeat::Weekly => self.date.days_until(day) % 7 == 0,
            Repeat::Monthly => day.day == self.date.day.min(day.month_of().length()),
            Repeat::Yearly => {
                day.month == self.date.month && day.day == self.date.day.min(day.month_of().length())
            }
        }
    }

    /// A line of the file.
    pub fn line(&self) -> String {
        let time = self.start.map_or(NO_TIME.to_string(), clock_text);
        let mut columns = vec![
            self.date.key(),
            time,
            self.calendar.to_string(),
            clean(&self.title, " "),
            format!("id={}", self.id),
        ];
        let mut put = |key: &str, value: String| {
            if !value.is_empty() {
                columns.push(format!("{key}={value}"));
            }
        };
        put("end", self.end.map(clock_text).unwrap_or_default());
        put("loc", clean(&self.location, " "));
        put("url", clean(&self.url, " "));
        put("notes", clean(&self.notes, NEWLINE_ESCAPE));
        put("alert", self.alert.map(|minutes| minutes.to_string()).unwrap_or_default());
        put(
            "repeat",
            if self.repeat == Repeat::Never { String::new() } else { self.repeat.name().into() },
        );
        put("until", self.until.map(Date::key).unwrap_or_default());
        put("kind", if self.kind == Kind::Reminder { "reminder".into() } else { String::new() });
        put("done", if self.done { "1".into() } else { String::new() });
        put(
            "skip",
            self.skipped.iter().map(|day| day.key()).collect::<Vec<_>>().join(","),
        );
        columns.join(&COLUMN_SEPARATOR.to_string())
    }

    pub fn parse(line: &str) -> Option<Self> {
        let columns: Vec<&str> = line.split(COLUMN_SEPARATOR).collect();
        if columns.len() < BASE_COLUMNS {
            return None;
        }
        let mut event = Self::new(Date::parse_key(columns[0])?, columns[3]);
        // An event written before it had an id gets the same one each time it is read.
        event.id = format!("l{}", fingerprint(line));
        event.start = parse_clock(columns[1]);
        event.calendar = columns[2].parse().ok()?;
        for column in &columns[BASE_COLUMNS..] {
            let Some((key, value)) = column.split_once('=') else {
                continue;
            };
            match key {
                "id" => event.id = value.to_string(),
                "end" => event.end = parse_clock(value),
                "loc" => event.location = value.to_string(),
                "url" => event.url = value.to_string(),
                "notes" => event.notes = value.replace(NEWLINE_ESCAPE, "\n"),
                "alert" => event.alert = value.parse().ok(),
                "repeat" => event.repeat = Repeat::from_name(value),
                "until" => event.until = Date::parse_key(value),
                "kind" => event.kind = if value == "reminder" { Kind::Reminder } else { Kind::Event },
                "done" => event.done = value == "1",
                "skip" => event.skipped = value.split(',').filter_map(Date::parse_key).collect(),
                _ => {}
            }
        }
        Some(event)
    }
}

fn clean(text: &str, newline: &str) -> String {
    text.replace(['\t', '\r'], " ").replace('\n', newline)
}

fn clock_text((hour, minute): (u32, u32)) -> String {
    format!("{hour:02}:{minute:02}")
}

fn parse_clock(text: &str) -> Option<(u32, u32)> {
    let (hour, minute) = text.split_once(':')?;
    let (hour, minute): (u32, u32) = (hour.parse().ok()?, minute.parse().ok()?);
    (hour < 24 && minute < 60).then_some((hour, minute))
}

fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// An id no other event has: the time to the nanosecond, and a count for those made at once.
fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static MADE: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{nanos:x}{:x}", MADE.fetch_add(1, Ordering::Relaxed))
}

/// What is on `day`, those that take the day first, then by when they begin.
pub fn on_day(events: &[Event], day: Date) -> Vec<Occurrence<'_>> {
    let mut found: Vec<Occurrence<'_>> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.occurs_on(day))
        .map(|(index, event)| Occurrence { index, event, date: day })
        .collect();
    found.sort_by(|a, b| (a.event.start, a.event.kind).cmp(&(b.event.start, b.event.kind)));
    found
}

/// Takes an event away: all of it, or, of one that repeats, only `day` (`this_only`) or `day`
/// and every day after it, which ends it the day before. A day at or before where it begins
/// leaves nothing of it.
pub fn take_away(events: &mut Vec<Event>, id: &str, day: Date, this_only: bool) {
    let Some(at) = events.iter().position(|event| event.id == id) else {
        return;
    };
    let event = &mut events[at];
    if event.repeat == Repeat::Never || (!this_only && day <= event.date) {
        events.remove(at);
    } else if this_only {
        event.skipped.push(day);
    } else {
        event.until = Some(day.plus_days(-1));
    }
}

/// Puts an event back where it was taken from, or at the end if it is not there any more.
pub fn restore(events: &mut Vec<Event>, original: Event) {
    match events.iter_mut().find(|event| event.id == original.id) {
        Some(found) => *found = original,
        None => events.push(original),
    }
}

impl PartialOrd for Kind {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Kind {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

/// Where the events are kept.
pub fn events_file() -> Option<PathBuf> {
    Some(data_dir()?.join("calendar.tsv"))
}

pub fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))?;
    Some(base.join("nekoland"))
}

pub fn load() -> Vec<Event> {
    let Some(path) = events_file() else {
        return Vec::new();
    };
    load_from(&path)
}

pub fn load_from(path: &Path) -> Vec<Event> {
    match fs::read_to_string(path) {
        Ok(text) => text.lines().filter_map(Event::parse).collect(),
        // Calendar has not been used yet; there is nothing to show.
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            Vec::new()
        }
    }
}

pub fn save(events: &[Event]) -> io::Result<()> {
    let path = events_file().ok_or(io::ErrorKind::NotFound)?;
    save_to(&path, events)
}

/// Writes the file whole and moves it into place, so a reader sees the old one or the new one.
pub fn save_to(path: &Path, events: &[Event]) -> io::Result<()> {
    let text: String = events.iter().map(|event| event.line() + "\n").collect();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("tsv.partial");
    fs::write(&partial, text)?;
    fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn a_line_older_versions_wrote_is_read() {
        let event = Event::parse("2026-11-18\t09:30\t1\t產品週會").expect("a line");
        assert_eq!(
            (event.date, event.start, event.calendar, event.title.as_str()),
            (date(2026, 11, 18), Some((9, 30)), 1, "產品週會")
        );
        assert!(Event::parse("2026-11-18\t-\t0\t生日").is_some_and(|event| event.all_day()));
        assert_eq!(Event::parse("not an event"), None);
    }

    #[test]
    fn an_event_is_read_back_as_it_was_written() {
        let mut event = Event::new(date(2026, 11, 18), "週會\t討論");
        event.start = Some((9, 5));
        event.end = Some((10, 0));
        event.location = "會議室 A".into();
        event.url = "https://meet.example/abc".into();
        event.notes = "第一行\n第二行".into();
        event.alert = Some(15);
        event.repeat = Repeat::Weekly;
        event.until = Some(date(2027, 1, 1));
        event.skipped = vec![date(2026, 11, 25), date(2026, 12, 2)];
        let read = Event::parse(&event.line()).expect("a line");
        assert_eq!(read, Event { title: "週會 討論".into(), ..event });
    }

    fn repeating() -> Event {
        let mut event = Event::new(date(2026, 11, 1), "每週");
        event.repeat = Repeat::Weekly;
        event
    }

    #[test]
    fn an_event_that_does_not_repeat_is_removed() {
        let event = Event::new(date(2026, 11, 18), "a");
        let id = event.id.clone();
        let mut events = vec![event];
        take_away(&mut events, &id, date(2026, 11, 18), true);
        assert!(events.is_empty());
    }

    #[test]
    fn one_day_of_a_repeating_event_is_skipped() {
        let event = repeating();
        let id = event.id.clone();
        let mut events = vec![event];
        take_away(&mut events, &id, date(2026, 11, 8), true);
        assert_eq!(events[0].skipped, [date(2026, 11, 8)]);
        assert!(!events[0].occurs_on(date(2026, 11, 8)) && events[0].occurs_on(date(2026, 11, 15)));
    }

    #[test]
    fn the_days_after_one_are_taken_away_by_ending_the_event_before_it() {
        let event = repeating();
        let id = event.id.clone();
        let mut events = vec![event];
        take_away(&mut events, &id, date(2026, 11, 15), false);
        assert!(events[0].occurs_on(date(2026, 11, 8)) && !events[0].occurs_on(date(2026, 11, 15)));
        take_away(&mut events, &id, date(2026, 11, 1), false);
        assert!(events.is_empty());
    }

    #[test]
    fn an_event_taken_away_is_put_back_as_it_was() {
        let event = repeating();
        let id = event.id.clone();
        let mut events = vec![event.clone()];
        take_away(&mut events, &id, date(2026, 11, 8), true);
        restore(&mut events, event.clone());
        assert_eq!(events, [event.clone()]);
        events.clear();
        restore(&mut events, event.clone());
        assert_eq!(events, [event]);
    }

    #[test]
    fn a_reminder_is_done_or_not() {
        let mut reminder = Event::new(date(2026, 11, 18), "繳電費");
        reminder.kind = Kind::Reminder;
        reminder.done = true;
        assert_eq!(Event::parse(&reminder.line()).map(|read| (read.kind, read.done)), Some((Kind::Reminder, true)));
    }

    #[test]
    fn a_line_without_an_id_keeps_the_one_it_was_given() {
        let line = "2026-11-18\t09:30\t1\t產品週會";
        assert_eq!(Event::parse(line).map(|event| event.id), Event::parse(line).map(|event| event.id));
    }

    #[test]
    fn events_repeat_as_they_say() {
        let mut event = Event::new(date(2026, 1, 31), "月底");
        event.repeat = Repeat::Monthly;
        assert!(event.occurs_on(date(2026, 2, 28)));
        assert!(!event.occurs_on(date(2026, 2, 27)));
        assert!(!event.occurs_on(date(2025, 12, 31)));
        event.repeat = Repeat::Weekly;
        assert!(event.occurs_on(date(2026, 2, 7)) && !event.occurs_on(date(2026, 2, 8)));
        event.until = Some(date(2026, 2, 1));
        assert!(!event.occurs_on(date(2026, 2, 7)));
        event.repeat = Repeat::Yearly;
        event.until = None;
        assert!(event.occurs_on(date(2030, 1, 31)));
    }

    #[test]
    fn a_skipped_day_is_left_out() {
        let mut event = Event::new(date(2026, 11, 1), "每天");
        event.repeat = Repeat::Daily;
        event.skipped = vec![date(2026, 11, 3)];
        assert!(event.occurs_on(date(2026, 11, 2)) && !event.occurs_on(date(2026, 11, 3)));
    }

    #[test]
    fn a_day_lists_what_takes_it_first() {
        let mut timed = Event::new(date(2026, 11, 18), "會");
        timed.start = Some((9, 0));
        let whole = Event::new(date(2026, 11, 18), "生日");
        let events = [timed, whole];
        let titles: Vec<&str> = on_day(&events, date(2026, 11, 18)).iter().map(|found| found.event.title.as_str()).collect();
        assert_eq!(titles, ["生日", "會"]);
    }

    #[test]
    fn the_file_is_written_whole_and_read_back() {
        let path = std::env::temp_dir().join(format!("nekoland-calendar-test-{}.tsv", std::process::id()));
        let events = vec![Event::new(date(2026, 11, 18), "a"), Event::new(date(2026, 11, 19), "b")];
        save_to(&path, &events).expect("written");
        assert_eq!(load_from(&path), events);
        let _ = fs::remove_file(&path);
        assert_eq!(load_from(&path), Vec::new());
    }
}
