//! The lunar calendar, 農曆, for the years 1901 to 2099: where each lunar year begins, which
//! month it adds a leap month after, and which months have thirty days.

use crate::date::Date;

/// A lunar year: the solar year it begins in, the month and day it begins on, the month it
/// has a leap month after (0 for none), and which of its months have thirty days, the first
/// as the lowest bit, the leap month in its place.
type Year = (i32, u32, u32, u32, u16);

const FIRST_YEAR: i32 = 1901;
const YEARS: [Year; 199] = include!("lunar_table.in");
const MONTH_NAMES: [&str; 12] = [
    "正月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "冬月", "臘月",
];
const TEN_DAYS: [&str; 4] = ["初", "十", "廿", "三"];
const DIGITS: [&str; 10] = ["十", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lunar {
    pub month: u32,
    pub day: u32,
    /// Whether the month is the leap one, the second of its number.
    pub leap: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LunarMonth {
    number: u32,
    leap: bool,
    length: u32,
}

fn year_entry(year: i32) -> Option<&'static Year> {
    YEARS.get(usize::try_from(year - FIRST_YEAR).ok()?)
}

fn new_year_of(entry: &Year) -> Date {
    Date {
        year: entry.0,
        month: entry.1,
        day: entry.2,
    }
}

/// The months of a lunar year in order, the leap month after the month it repeats.
fn months_of(entry: &Year) -> Vec<LunarMonth> {
    let leap_after = entry.3;
    let count = if leap_after > 0 { 13 } else { 12 };
    (0..count)
        .map(|index| {
            let leap = leap_after > 0 && index as u32 == leap_after;
            let number = match (leap_after > 0, index as u32) {
                (true, at) if at > leap_after => at,
                (true, at) if at == leap_after => leap_after,
                (_, at) => at + 1,
            };
            LunarMonth {
                number,
                leap,
                length: if entry.4 & (1 << index) != 0 { 30 } else { 29 },
            }
        })
        .collect()
}

/// The day a lunar year, named by the solar year it begins in, begins on.
pub fn new_year(year: i32) -> Option<Date> {
    year_entry(year).map(new_year_of)
}

pub fn from_solar(date: Date) -> Option<Lunar> {
    let year = [date.year, date.year - 1]
        .into_iter()
        .find(|&year| new_year(year).is_some_and(|start| start <= date))?;
    let entry = year_entry(year)?;
    let mut offset = new_year_of(entry).days_until(date) as u32;
    for month in months_of(entry) {
        if offset < month.length {
            return Some(Lunar {
                month: month.number,
                day: offset + 1,
                leap: month.leap,
            });
        }
        offset -= month.length;
    }
    // Past the last day the table knows of that year, which the next year's start would give.
    None
}

/// The solar day of a lunar one, in the lunar year that begins in `year`.
pub fn to_solar(year: i32, month: u32, day: u32, leap: bool) -> Option<Date> {
    let entry = year_entry(year)?;
    let mut offset = 0;
    for found in months_of(entry) {
        if found.number == month && found.leap == leap {
            return (1..=found.length)
                .contains(&day)
                .then(|| new_year_of(entry).plus_days(i64::from(offset + day - 1)));
        }
        offset += found.length;
    }
    None
}

pub fn month_name(month: u32, leap: bool) -> String {
    let name = MONTH_NAMES[(month.clamp(1, 12) - 1) as usize];
    if leap {
        format!("閏{name}")
    } else {
        name.to_string()
    }
}

/// A day as it is said: 初一 to 初十, 十一 to 二十, 廿一 to 廿九, and 三十.
pub fn day_name(day: u32) -> String {
    match day {
        10 => "初十".into(),
        20 => "二十".into(),
        30 => "三十".into(),
        _ => format!("{}{}", TEN_DAYS[((day - 1) / 10) as usize], DIGITS[(day % 10) as usize]),
    }
}

impl Lunar {
    /// What a calendar writes under a day: the month on its first day, else the day.
    pub fn label(self) -> String {
        if self.day == 1 {
            month_name(self.month, self.leap)
        } else {
            day_name(self.day)
        }
    }

    /// The whole date, as the menu bar and the day's heading say it.
    pub fn full(self) -> String {
        format!("{}{}", month_name(self.month, self.leap), day_name(self.day))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn the_new_year_is_the_first_of_the_first_month() {
        assert_eq!(
            from_solar(date(2026, 2, 17)),
            Some(Lunar { month: 1, day: 1, leap: false })
        );
        assert_eq!(from_solar(date(2027, 2, 6)).map(Lunar::full), Some("正月初一".into()));
        assert_eq!(from_solar(date(2026, 2, 16)).map(|day| (day.month, day.day)), Some((12, 29)));
    }

    #[test]
    fn a_day_is_found_in_its_lunar_month() {
        assert_eq!(from_solar(date(2026, 11, 18)).map(Lunar::full), Some("十月初十".into()));
        assert_eq!(from_solar(date(2024, 9, 17)).map(Lunar::full), Some("八月十五".into()));
    }

    #[test]
    fn a_leap_month_follows_the_month_it_repeats() {
        // 2025 had a second sixth month.
        let leap = to_solar(2025, 6, 1, true).expect("a leap sixth month");
        assert_eq!(from_solar(leap), Some(Lunar { month: 6, day: 1, leap: true }));
        assert_eq!(from_solar(leap.plus_days(-1)).map(|day| (day.month, day.leap)), Some((6, false)));
        assert_eq!(from_solar(leap).map(Lunar::label), Some("閏六月".into()));
    }

    #[test]
    fn lunar_and_solar_days_are_each_others_inverses() {
        for days in (0..4000).step_by(7) {
            let day = date(2020, 1, 1).plus_days(days);
            let lunar = from_solar(day).expect("within the table");
            let year = if day >= new_year(day.year).expect("in the table") { day.year } else { day.year - 1 };
            assert_eq!(to_solar(year, lunar.month, lunar.day, lunar.leap), Some(day), "{day}");
        }
    }

    #[test]
    fn days_are_named_as_they_are_said() {
        let names: Vec<String> = [1, 2, 10, 11, 20, 21, 29, 30].into_iter().map(day_name).collect();
        assert_eq!(names, ["初一", "初二", "初十", "十一", "二十", "廿一", "廿九", "三十"]);
    }

    #[test]
    fn outside_the_table_there_is_no_lunar_date() {
        assert_eq!(from_solar(date(1900, 1, 1)), None);
        assert_eq!(from_solar(date(2100, 6, 1)), None);
    }
}
