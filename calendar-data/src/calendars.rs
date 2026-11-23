//! The calendars events are sorted into: a name, a colour, and whether what is in them is
//! shown. The holidays are the last of them and keep to what the holiday list says.

use std::{fs, io};

use crate::event::data_dir;

pub type Rgb = (f64, f64, f64);

/// The calendars Calendar has before anyone changes them, and their colours.
const DEFAULTS: [(&str, Rgb); 3] = [
    ("個人", (0.35, 0.60, 0.95)),
    ("工作", (0.95, 0.60, 0.25)),
    ("節日", (0.85, 0.35, 0.40)),
];
/// Where the holidays are among the defaults.
pub const HOLIDAYS: usize = 2;
const COLUMN_SEPARATOR: char = '\t';
const FILE_NAME: &str = "calendars.tsv";

#[derive(Clone, Debug, PartialEq)]
pub struct CalendarInfo {
    pub name: String,
    pub color: Rgb,
    pub shown: bool,
}

pub fn defaults() -> Vec<CalendarInfo> {
    DEFAULTS
        .iter()
        .map(|(name, color)| CalendarInfo {
            name: (*name).into(),
            color: *color,
            shown: true,
        })
        .collect()
}

fn hex(color: Rgb) -> String {
    let byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(color.0), byte(color.1), byte(color.2))
}

pub fn parse_hex(text: &str) -> Option<Rgb> {
    let digits = text.strip_prefix('#')?;
    // Slicing by byte below needs every digit to be one byte.
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok().map(|value| f64::from(value) / 255.0);
    Some((channel(0)?, channel(2)?, channel(4)?))
}

fn parse_line(line: &str) -> Option<CalendarInfo> {
    let mut columns = line.split(COLUMN_SEPARATOR);
    let name = columns.next().filter(|name| !name.is_empty())?;
    Some(CalendarInfo {
        name: name.to_string(),
        color: columns.next().and_then(parse_hex)?,
        shown: columns.next() != Some("0"),
    })
}

/// The calendars, the defaults where nothing is kept or what is kept has fewer than they.
pub fn load() -> Vec<CalendarInfo> {
    let kept: Vec<CalendarInfo> = data_dir()
        .and_then(|dir| fs::read_to_string(dir.join(FILE_NAME)).ok())
        .map(|text| text.lines().filter_map(parse_line).collect())
        .unwrap_or_default();
    let mut calendars = defaults();
    for (index, info) in kept.into_iter().enumerate() {
        match calendars.get_mut(index) {
            Some(slot) => *slot = info,
            None => calendars.push(info),
        }
    }
    calendars
}

pub fn save(calendars: &[CalendarInfo]) -> io::Result<()> {
    let dir = data_dir().ok_or(io::ErrorKind::NotFound)?;
    fs::create_dir_all(&dir)?;
    let text: String = calendars
        .iter()
        .map(|info| format!("{}\t{}\t{}\n", info.name.replace(['\t', '\n'], " "), hex(info.color), u8::from(info.shown)))
        .collect();
    let path = dir.join(FILE_NAME);
    let partial = path.with_extension("tsv.partial");
    fs::write(&partial, text)?;
    fs::rename(&partial, path)
}

/// The colour of a calendar, grey for one that is not there.
pub fn color_of(calendars: &[CalendarInfo], index: usize) -> Rgb {
    calendars.get(index).map_or((0.6, 0.6, 0.6), |info| info.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_written_and_read_as_hex() {
        assert_eq!(parse_hex("#ff8000"), Some((1.0, 128.0 / 255.0, 0.0)));
        assert_eq!(hex((1.0, 0.0, 0.0)), "#ff0000");
        assert_eq!(parse_hex("ff8000"), None);
        assert_eq!(parse_hex("#ff80"), None);
        assert_eq!(parse_hex("#1é234"), None);
    }

    #[test]
    fn a_calendar_is_read_from_its_line() {
        let info = parse_line("工作\t#f09a40\t0").expect("a line");
        assert_eq!((info.name.as_str(), info.shown), ("工作", false));
        assert_eq!(parse_line("\t#ffffff\t1"), None);
        assert_eq!(parse_line("名稱"), None);
    }

    #[test]
    fn the_defaults_come_with_the_holidays_last() {
        let calendars = defaults();
        assert_eq!(calendars[HOLIDAYS].name, "節日");
        assert!(calendars.iter().all(|info| info.shown));
    }
}
