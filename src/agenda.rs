//! What Calendar has, kept up to date for the clock and its panel: the events and the
//! calendars they are in, read again when either file changes, and changed here (an event
//! done or taken away) by reading what is kept at that moment and writing it back.

use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use calendar_data::{
    calendars::{self, CalendarInfo},
    event::{self, Event},
};

/// How often the files are looked at for a change.
const CHECK_EVERY: Duration = Duration::from_secs(1);
const CALENDARS_FILE: &str = "calendars.tsv";

pub struct Agenda {
    events: Vec<Event>,
    calendars: Vec<CalendarInfo>,
    stamp: (Option<SystemTime>, Option<SystemTime>),
    checked: Option<Instant>,
    /// Counts each change, so what shows it is drawn again.
    pub revision: u64,
}

fn modified(path: Option<PathBuf>) -> Option<SystemTime> {
    std::fs::metadata(path?).and_then(|meta| meta.modified()).ok()
}

fn stamp() -> (Option<SystemTime>, Option<SystemTime>) {
    (
        modified(event::events_file()),
        modified(event::data_dir().map(|dir| dir.join(CALENDARS_FILE))),
    )
}

impl Default for Agenda {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            calendars: calendars::defaults(),
            stamp: (None, None),
            checked: None,
            revision: 0,
        }
    }
}

impl Agenda {
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn calendars(&self) -> &[CalendarInfo] {
        &self.calendars
    }

    /// Whether events of calendar `index` are shown.
    pub fn shown(&self, index: usize) -> bool {
        self.calendars.get(index).is_none_or(|info| info.shown)
    }

    /// Reads again what changed on disk, at most once a `CHECK_EVERY`. Returns whether anything did.
    pub fn refresh(&mut self) -> bool {
        if self.checked.is_some_and(|checked| checked.elapsed() < CHECK_EVERY) {
            return false;
        }
        self.checked = Some(Instant::now());
        let found = stamp();
        if self.revision > 0 && found == self.stamp {
            return false;
        }
        self.stamp = found;
        self.events = event::load();
        self.calendars = calendars::load();
        self.revision += 1;
        true
    }

    /// Changes the events as they are kept now, and keeps them.
    pub fn change(&mut self, change: impl FnOnce(&mut Vec<Event>)) {
        let mut events = event::load();
        change(&mut events);
        if let Err(err) = event::save(&events) {
            tracing::warn!("calendar: {err}");
            return;
        }
        self.events = events;
        self.stamp = stamp();
        self.revision += 1;
    }
}
