//! The clock's text from a format of the kind Calendr takes: letters for the parts of a date
//! and time (`HH:mm`, `M月d日 EEE`), text in single quotes as it is, and, after `|`, another
//! place's time written `HH:mm@GMT+2 'LT'`.

use crate::date::{local_parts, WEEKDAY_NAMES};

const SECONDS_IN_MINUTE: i64 = 60;
const SECONDS_IN_HOUR: i64 = 3600;
const SEGMENT_SEPARATOR: char = '|';
const ZONE_MARK: &str = "@GMT";
const NOON: u32 = 12;

/// A moment, taken apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub weekday: usize,
}

impl Moment {
    /// The moment `seconds` since the epoch is in a zone that is `offset` seconds from UTC.
    pub fn at_offset(seconds: i64, offset: i64) -> Self {
        let shifted = seconds + offset;
        let days = shifted.div_euclid(86_400);
        let second_of_day = shifted.rem_euclid(86_400) as u32;
        Self::from_parts(days, second_of_day)
    }

    /// The moment in the local zone.
    pub fn local(seconds: i64) -> Self {
        let (days, second_of_day) = local_parts(seconds);
        Self::from_parts(days, second_of_day)
    }

    fn from_parts(days: i64, second_of_day: u32) -> Self {
        let date = crate::date::Date::from_epoch_days(days);
        Self {
            year: date.year,
            month: date.month,
            day: date.day,
            hour: second_of_day / 3600,
            minute: second_of_day / 60 % 60,
            second: second_of_day % 60,
            weekday: date.weekday(),
        }
    }
}

/// How many seconds from UTC a `+2`, `-3` or `+5:30` is.
fn parse_offset(text: &str) -> Option<i64> {
    let sign = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let (hours, minutes) = text[1..].split_once(':').unwrap_or((&text[1..], "0"));
    let (hours, minutes): (i64, i64) = (hours.parse().ok()?, minutes.parse().ok()?);
    Some(sign * (hours * SECONDS_IN_HOUR + minutes * SECONDS_IN_MINUTE))
}

/// A segment with the zone it names taken out of it.
fn split_zone(segment: &str) -> (String, Option<i64>) {
    let Some(at) = segment.find(ZONE_MARK) else {
        return (segment.to_string(), None);
    };
    let after = &segment[at + ZONE_MARK.len()..];
    let length = after.find(|character: char| !(character.is_ascii_digit() || matches!(character, '+' | '-' | ':'))).unwrap_or(after.len());
    match parse_offset(&after[..length]) {
        Some(offset) => (format!("{}{}", &segment[..at], &after[length..]), Some(offset)),
        None => (segment.to_string(), None),
    }
}

fn run_of(characters: &[char], at: usize) -> usize {
    characters[at..].iter().take_while(|character| **character == characters[at]).count()
}

fn format_one(pattern: &str, moment: Moment) -> String {
    let characters: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < characters.len() {
        let character = characters[at];
        if character == '\'' {
            // Text as it is, up to the next quote; two quotes together are one.
            let mut end = at + 1;
            while end < characters.len() && characters[end] != '\'' {
                out.push(characters[end]);
                end += 1;
            }
            if end == at + 1 {
                out.push('\'');
            }
            at = end + 1;
            continue;
        }
        let count = run_of(&characters, at);
        let hour12 = (moment.hour + 11) % 12 + 1;
        let weekday = WEEKDAY_NAMES[moment.weekday];
        let text = match (character, count) {
            ('y', 2) => format!("{:02}", moment.year % 100),
            ('y', _) => format!("{:0width$}", moment.year, width = count.max(1)),
            ('M', 1) => moment.month.to_string(),
            ('M', 2) => format!("{:02}", moment.month),
            ('M', _) => format!("{}月", moment.month),
            ('d', 1) => moment.day.to_string(),
            ('d', _) => format!("{:02}", moment.day),
            ('E', 1..=3) => format!("週{weekday}"),
            ('E', _) => format!("星期{weekday}"),
            ('H', 1) => moment.hour.to_string(),
            ('H', _) => format!("{:02}", moment.hour),
            ('h', 1) => hour12.to_string(),
            ('h', _) => format!("{hour12:02}"),
            ('m', 1) => moment.minute.to_string(),
            ('m', _) => format!("{:02}", moment.minute),
            ('s', 1) => moment.second.to_string(),
            ('s', _) => format!("{:02}", moment.second),
            ('a', _) => (if moment.hour < NOON { "上午" } else { "下午" }).to_string(),
            _ => characters[at..at + count].iter().collect(),
        };
        out.push_str(&text);
        at += count;
    }
    out
}

/// The clock `pattern` makes of the moment `seconds` since the epoch is.
pub fn format_clock(pattern: &str, seconds: i64) -> String {
    pattern
        .split(SEGMENT_SEPARATOR)
        .map(|segment| {
            let (segment, offset) = split_zone(segment.trim());
            let moment = match offset {
                Some(offset) => Moment::at_offset(seconds, offset),
                None => Moment::local(seconds),
            };
            format_one(&segment, moment)
        })
        .collect::<Vec<_>>()
        .join(&format!(" {SEGMENT_SEPARATOR} "))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-18 15:04:05 UTC, a Wednesday.
    const SECONDS: i64 = 1_795_014_245;

    fn in_utc(pattern: &str) -> String {
        // A zone of its own is read the same wherever the clock is.
        format_clock(&format!("{pattern}@GMT+0"), SECONDS)
    }

    #[test]
    fn the_parts_of_a_moment_are_written() {
        assert_eq!(in_utc("HH:mm:ss"), "15:04:05");
        assert_eq!(in_utc("yyyy-MM-dd"), "2026-11-18");
        assert_eq!(in_utc("M月d日 EEE"), "11月18日 週三");
        assert_eq!(in_utc("EEEE"), "星期三");
        assert_eq!(in_utc("a h:mm"), "下午 3:04");
        assert_eq!(in_utc("yy"), "26");
    }

    #[test]
    fn quoted_text_is_kept_as_it_is() {
        assert_eq!(in_utc("HH 'LT' mm"), "15 LT 04");
        assert_eq!(in_utc("'HH'"), "HH");
        assert_eq!(in_utc("H''m"), "15'4");
    }

    #[test]
    fn other_zones_follow_the_pipe() {
        let text = format_clock("HH:mm@GMT+0 | HH:mm@GMT+2 'LT' | HH:mm@GMT-3 'BR'", SECONDS);
        assert_eq!(text, "15:04 | 17:04 LT | 12:04 BR");
    }

    #[test]
    fn a_zone_may_have_minutes() {
        assert_eq!(format_clock("HH:mm@GMT+5:30", SECONDS), "20:34");
        // A zone that cannot be read is no zone; the clock is still the local one.
        assert!(!format_clock("HH:mm@GMT+x", SECONDS).is_empty());
    }

    #[test]
    fn a_day_changes_across_zones() {
        assert_eq!(format_clock("d@GMT+10", SECONDS), "19");
        assert_eq!(format_clock("d@GMT-20", SECONDS), "17");
    }
}
