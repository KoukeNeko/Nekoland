//! What comes next, and how long until: the event the menu bar shows, and when one is close
//! enough to call attention to itself.

use crate::{
    date::{local_parts, local_seconds, now, Date},
    event::{Event, Kind},
};

const SECONDS_IN_MINUTE: i64 = 60;
const MINUTES_IN_HOUR: i64 = 60;
const HOURS_IN_DAY: i64 = 24;
/// How many days ahead the next event is looked for.
const LOOK_AHEAD_DAYS: i64 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upcoming {
    pub index: usize,
    pub day: Date,
    /// Seconds since the epoch when it begins.
    pub begins: i64,
}

fn begins_at(event: &Event, day: Date) -> Option<i64> {
    let (hour, minute) = event.start?;
    Some(local_seconds(day, hour, minute))
}

/// The event that begins next after `seconds`, with those not counted that `skip` says to.
pub fn next_after(events: &[Event], seconds: i64, skip: impl Fn(&Event) -> bool) -> Option<Upcoming> {
    let (days, _) = local_parts(seconds);
    let today = Date::from_epoch_days(days);
    (0..LOOK_AHEAD_DAYS)
        .map(|ahead| today.plus_days(ahead))
        .flat_map(|day| {
            events.iter().enumerate().filter_map(move |(index, event)| {
                let counts = event.kind == Kind::Event && event.occurs_on(day);
                let begins = begins_at(event, day).filter(|&begins| counts && begins >= seconds)?;
                Some(Upcoming { index, day, begins })
            })
        })
        .filter(|found| !skip(&events[found.index]))
        .min_by_key(|found| found.begins)
}

/// How long until `begins`, as the bar says it: 「25 分鐘後」「2 小時後」「明天」.
pub fn until_text(begins: i64, seconds: i64) -> String {
    let minutes = (begins - seconds).div_euclid(SECONDS_IN_MINUTE);
    if minutes < 1 {
        return "即將開始".into();
    }
    if minutes < MINUTES_IN_HOUR {
        return format!("{minutes} 分鐘後");
    }
    let hours = minutes / MINUTES_IN_HOUR;
    if hours < HOURS_IN_DAY {
        return format!("{hours} 小時後");
    }
    let (begin_day, _) = local_parts(begins);
    let (this_day, _) = local_parts(seconds);
    match begin_day - this_day {
        1 => "明天".into(),
        2 => "後天".into(),
        days => format!("{days} 天後"),
    }
}

/// The minutes of an event's alert that have come by `seconds`, and are within `window`
/// seconds of it: the one that says it is time.
pub fn alert_due(event: &Event, day: Date, seconds: i64, window: i64) -> bool {
    let Some(minutes) = event.alert else {
        return false;
    };
    let Some(begins) = begins_at(event, day) else {
        return false;
    };
    let at = begins - i64::from(minutes) * SECONDS_IN_MINUTE;
    (at..at + window).contains(&seconds)
}

/// The same instant the seconds count, as now.
pub fn current_seconds() -> i64 {
    now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: Date, hour: u32, minute: u32) -> i64 {
        local_seconds(day, hour, minute)
    }

    fn event(day: Date, hour: u32, minute: u32, title: &str) -> Event {
        let mut event = Event::new(day, title);
        event.start = Some((hour, minute));
        event
    }

    #[test]
    fn the_next_event_is_the_one_that_begins_soonest() {
        let day = Date { year: 2026, month: 11, day: 18 };
        let events = [event(day, 14, 0, "後"), event(day, 9, 30, "前"), event(day.plus_days(1), 8, 0, "明")];
        let found = next_after(&events, at(day, 9, 0), |_| false).expect("one");
        assert_eq!(events[found.index].title, "前");
        let found = next_after(&events, at(day, 10, 0), |_| false).expect("one");
        assert_eq!(events[found.index].title, "後");
        assert_eq!(next_after(&events, at(day, 20, 0), |_| false).map(|found| found.day), Some(day.plus_days(1)));
    }

    #[test]
    fn events_that_are_skipped_are_not_next() {
        let day = Date { year: 2026, month: 11, day: 18 };
        let events = [event(day, 9, 30, "a"), event(day, 10, 0, "b")];
        let found = next_after(&events, at(day, 9, 0), |event| event.title == "a").expect("one");
        assert_eq!(events[found.index].title, "b");
    }

    #[test]
    fn days_that_take_the_day_are_not_next() {
        let day = Date { year: 2026, month: 11, day: 18 };
        assert_eq!(next_after(&[Event::new(day, "生日")], at(day, 0, 0), |_| false), None);
    }

    #[test]
    fn the_wait_is_said_in_the_largest_unit() {
        let day = Date { year: 2026, month: 11, day: 18 };
        let now = at(day, 9, 0);
        assert_eq!(until_text(at(day, 9, 0), now), "即將開始");
        assert_eq!(until_text(at(day, 9, 25), now), "25 分鐘後");
        assert_eq!(until_text(at(day, 11, 10), now), "2 小時後");
        assert_eq!(until_text(at(day.plus_days(1), 9, 0), now), "明天");
        assert_eq!(until_text(at(day.plus_days(5), 9, 0), now), "5 天後");
    }

    #[test]
    fn an_alert_is_due_in_the_window_it_names() {
        let day = Date { year: 2026, month: 11, day: 18 };
        let mut timed = event(day, 9, 30, "會");
        timed.alert = Some(15);
        let due = at(day, 9, 15);
        assert!(alert_due(&timed, day, due, 30));
        assert!(!alert_due(&timed, day, due - 1, 30));
        assert!(!alert_due(&timed, day, due + 30, 30));
        timed.alert = None;
        assert!(!alert_due(&timed, day, due, 30));
    }
}
