//! Days of the Gregorian calendar, with the arithmetic the panel needs.

use std::fmt;

pub const DAYS_IN_WEEK: usize = 7;
pub const WEEKDAY_NAMES: [&str; DAYS_IN_WEEK] = ["日", "一", "二", "三", "四", "五", "六"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    /// From 1 to 12.
    pub month: u32,
    pub day: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    pub year: i32,
    pub month: u32,
}

/// Days from 1970-01-01, by the civil-from-days arithmetic of Howard Hinnant.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> Date {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Date {
        year: year as i32,
        month: month as u32,
        day: day as u32,
    }
}

impl Date {
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        let valid = (1..=12).contains(&month) && day >= 1 && day <= Month { year, month }.length();
        valid.then_some(Self { year, month, day })
    }

    pub fn today() -> Self {
        Self::from_epoch_days(local_parts(now()).0)
    }

    pub fn from_epoch_days(days: i64) -> Self {
        civil_from_days(days)
    }

    pub fn epoch_days(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn month_of(self) -> Month {
        Month {
            year: self.year,
            month: self.month,
        }
    }

    /// From 0 on a Sunday.
    pub fn weekday(self) -> usize {
        (self.epoch_days() + 4).rem_euclid(7) as usize
    }

    pub fn plus_days(self, days: i64) -> Self {
        Self::from_epoch_days(self.epoch_days() + days)
    }

    /// The same day of another month, the last one that month has if it has no such day.
    pub fn plus_months(self, months: i32) -> Self {
        let month = self.month_of().shifted(months);
        Self {
            year: month.year,
            month: month.month,
            day: self.day.min(month.length()),
        }
    }

    pub fn plus_years(self, years: i32) -> Self {
        self.plus_months(years * 12)
    }

    pub fn days_until(self, other: Self) -> i64 {
        other.epoch_days() - self.epoch_days()
    }

    /// The week of the year by ISO 8601: the one with the year's first Thursday is the first.
    pub fn iso_week(self) -> u32 {
        let weekday_from_monday = (self.weekday() + 6) % 7;
        let thursday = self.plus_days(3 - weekday_from_monday as i64);
        let first = Self {
            year: thursday.year,
            month: 1,
            day: 1,
        };
        (first.days_until(thursday) / 7) as u32 + 1
    }

    /// The day as Calendar's file keeps it.
    pub fn key(self) -> String {
        self.to_string()
    }

    pub fn parse_key(key: &str) -> Option<Self> {
        let mut parts = key.trim().splitn(3, '-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        Self::new(year, month, day)
    }

    /// The nth of a weekday in a month, counted from 1: the second Sunday of May.
    pub fn nth_weekday(month: Month, weekday: usize, nth: u32) -> Option<Self> {
        let first = month.first_day();
        let lead = (weekday + DAYS_IN_WEEK - first.weekday()) % DAYS_IN_WEEK;
        Self::new(month.year, month.month, 1 + lead as u32 + (nth - 1) * 7)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl Month {
    pub fn length(self) -> u32 {
        match self.month {
            2 if self.year % 4 == 0 && (self.year % 100 != 0 || self.year % 400 == 0) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    pub fn shifted(self, by: i32) -> Self {
        let index = self.year * 12 + self.month as i32 - 1 + by;
        Self {
            year: index.div_euclid(12),
            month: index.rem_euclid(12) as u32 + 1,
        }
    }

    pub fn first_day(self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: 1,
        }
    }

    /// The days a month is shown in, `weeks` of them from the `first_weekday` on or before its
    /// first day, with the days of the months either side that fill them.
    pub fn grid(self, first_weekday: usize, weeks: usize) -> Vec<Date> {
        let lead = (self.first_day().weekday() + DAYS_IN_WEEK - first_weekday % DAYS_IN_WEEK) % DAYS_IN_WEEK;
        let start = self.first_day().plus_days(-(lead as i64));
        (0..weeks * DAYS_IN_WEEK)
            .map(|cell| start.plus_days(cell as i64))
            .collect()
    }
}

/// Seconds since the epoch.
pub fn now() -> i64 {
    // SAFETY: time writes nowhere when it is given a null pointer.
    i64::from(unsafe { libc::time(std::ptr::null_mut()) })
}

/// The local time of `seconds` since the epoch, as the day (in days since the epoch) and the
/// seconds into it.
pub fn local_parts(seconds: i64) -> (i64, u32) {
    // SAFETY: localtime_r only writes the tm it is given.
    let tm = unsafe {
        let at = seconds as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&at, &mut tm);
        tm
    };
    let days = days_from_civil(tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32);
    (days, (tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec) as u32)
}

/// The seconds since the epoch at a local day and time.
pub fn local_seconds(date: Date, hour: u32, minute: u32) -> i64 {
    // SAFETY: mktime only reads and normalises the tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = date.year - 1900;
        tm.tm_mon = date.month as i32 - 1;
        tm.tm_mday = date.day as i32;
        tm.tm_hour = hour as i32;
        tm.tm_min = minute as i32;
        tm.tm_isdst = -1;
        i64::from(libc::mktime(&mut tm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn weekdays_count_from_sunday() {
        assert_eq!(date(2026, 11, 18).weekday(), 3);
        assert_eq!(date(2000, 1, 1).weekday(), 6);
        assert_eq!(date(2024, 2, 29).weekday(), 4);
        assert_eq!(date(1970, 1, 1).weekday(), 4);
    }

    #[test]
    fn epoch_days_go_both_ways() {
        for days in [-1000, -1, 0, 1, 20_000, 24_000, 60_000] {
            assert_eq!(Date::from_epoch_days(days).epoch_days(), days);
        }
        assert_eq!(date(2026, 11, 18).plus_days(13), date(2026, 12, 1));
        assert_eq!(date(2026, 1, 1).plus_days(-1), date(2025, 12, 31));
    }

    #[test]
    fn months_have_the_days_they_have() {
        let february = |year| Month { year, month: 2 }.length();
        assert_eq!(
            [february(2024), february(2025), february(1900), february(2000)],
            [29, 28, 28, 29]
        );
        assert_eq!(date(2026, 1, 31).plus_months(1), date(2026, 2, 28));
        assert_eq!(date(2024, 2, 29).plus_years(1), date(2025, 2, 28));
    }

    #[test]
    fn months_shift_across_years() {
        let november = Month { year: 2026, month: 11 };
        assert_eq!(november.shifted(2), Month { year: 2027, month: 1 });
        assert_eq!(november.shifted(-11), Month { year: 2025, month: 12 });
    }

    #[test]
    fn weeks_are_numbered_by_iso_8601() {
        assert_eq!(date(2026, 1, 1).iso_week(), 1);
        assert_eq!(date(2026, 11, 18).iso_week(), 47);
        assert_eq!(date(2021, 1, 3).iso_week(), 53);
        assert_eq!(date(2024, 12, 30).iso_week(), 1);
    }

    #[test]
    fn the_grid_starts_on_the_chosen_weekday() {
        let november = Month { year: 2026, month: 11 };
        let sunday = november.grid(0, 6);
        assert_eq!((sunday[0], sunday.len()), (date(2026, 11, 1), 42));
        assert_eq!(november.grid(1, 6)[0], date(2026, 10, 26));
        assert_eq!(Month { year: 2026, month: 1 }.grid(0, 5)[0], date(2025, 12, 28));
    }

    #[test]
    fn the_nth_weekday_of_a_month_is_found() {
        let may = Month { year: 2026, month: 5 };
        assert_eq!(Date::nth_weekday(may, 0, 2), Some(date(2026, 5, 10)));
    }

    #[test]
    fn keys_are_read_back() {
        assert_eq!(Date::parse_key("2026-11-18"), Some(date(2026, 11, 18)));
        assert_eq!(Date::parse_key("2026-13-18"), None);
        assert_eq!(date(2026, 3, 4).key(), "2026-03-04");
    }
}
