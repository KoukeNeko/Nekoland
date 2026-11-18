//! The holidays and festivals of Taiwan, which Calendar lists as a calendar of its own.

use crate::{
    date::{Date, Month},
    lunar,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holiday {
    pub name: &'static str,
    /// Whether work stops for it, rather than it being a day that is only kept.
    pub day_off: bool,
}

const fn off(name: &'static str) -> Holiday {
    Holiday { name, day_off: true }
}

const fn festival(name: &'static str) -> Holiday {
    Holiday { name, day_off: false }
}

/// Kept on a day of the Gregorian calendar: month, day.
const SOLAR: [(u32, u32, Holiday); 9] = [
    (1, 1, off("元旦")),
    (2, 14, festival("情人節")),
    (2, 28, off("和平紀念日")),
    (4, 4, off("兒童節")),
    (5, 1, off("勞動節")),
    (8, 8, festival("父親節")),
    (9, 28, festival("教師節")),
    (10, 10, off("國慶日")),
    (12, 25, festival("聖誕節")),
];

/// Kept on a day of the lunar calendar: month, day.
const LUNAR: [(u32, u32, Holiday); 10] = [
    (1, 1, off("春節")),
    (1, 2, off("春節")),
    (1, 3, off("春節")),
    (1, 15, festival("元宵節")),
    (5, 5, off("端午節")),
    (7, 7, festival("七夕")),
    (7, 15, festival("中元節")),
    (8, 15, off("中秋節")),
    (9, 9, festival("重陽節")),
    (12, 8, festival("臘八")),
];

/// The first year the solar term formula below holds from, and the last.
const TERM_YEARS: std::ops::RangeInclusive<i32> = 2001..=2100;

/// The day of April 清明 falls on, by the shortcut formula for the twenty-first century.
fn qingming(year: i32) -> Option<Date> {
    if !TERM_YEARS.contains(&year) {
        return None;
    }
    let last_two = f64::from(year % 100);
    let day = (last_two * 0.2422 + 4.81).floor() as i32 - (year % 100) / 4;
    Date::new(year, 4, day as u32)
}

/// The days kept on `date`, those that stop work first.
pub fn on(date: Date) -> Vec<Holiday> {
    let mut found: Vec<Holiday> = SOLAR
        .iter()
        .filter(|(month, day, _)| (*month, *day) == (date.month, date.day))
        .map(|(.., holiday)| *holiday)
        .collect();
    if qingming(date.year) == Some(date) {
        found.push(off("清明節"));
    }
    if date.month == 5 && Date::nth_weekday(Month { year: date.year, month: 5 }, 0, 2) == Some(date) {
        found.push(festival("母親節"));
    }
    if let Some(day) = lunar::from_solar(date).filter(|day| !day.leap) {
        found.extend(
            LUNAR
                .iter()
                .filter(|(month, number, _)| (*month, *number) == (day.month, day.day))
                .map(|(.., holiday)| *holiday),
        );
    }
    // The day before the lunar new year, which the lunar month it is in does not say.
    let eve_of_new_year = |year| lunar::new_year(year).is_some_and(|start| start.plus_days(-1) == date);
    if eve_of_new_year(date.year) || eve_of_new_year(date.year + 1) {
        found.push(off("除夕"));
    }
    found.sort_by_key(|holiday| !holiday.day_off);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(year: i32, month: u32, day: u32) -> Vec<&'static str> {
        on(Date { year, month, day }).iter().map(|holiday| holiday.name).collect()
    }

    #[test]
    fn days_of_the_solar_calendar_are_kept() {
        assert_eq!(names(2026, 10, 10), ["國慶日"]);
        assert_eq!(names(2026, 11, 18), Vec::<&str>::new());
    }

    #[test]
    fn days_of_the_lunar_calendar_are_kept() {
        assert_eq!(names(2026, 2, 17), ["春節"]);
        assert_eq!(names(2026, 2, 16), ["除夕"]);
        assert_eq!(names(2024, 9, 17), ["中秋節"]);
        assert_eq!(names(2026, 6, 19), ["端午節"]);
    }

    #[test]
    fn qingming_follows_the_sun() {
        assert_eq!(qingming(2024), Date::new(2024, 4, 4));
        assert_eq!(qingming(2026), Date::new(2026, 4, 5));
        assert_eq!(qingming(1999), None);
    }

    #[test]
    fn the_second_sunday_of_may_is_mothers_day() {
        assert_eq!(names(2026, 5, 10), ["母親節"]);
    }

    #[test]
    fn days_that_stop_work_come_first() {
        assert_eq!(names(2024, 4, 4), ["兒童節", "清明節"]);
    }
}
