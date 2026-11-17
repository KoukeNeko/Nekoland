//! Night Shift: warmer colours at night, as the Mac's, through the displays' gamma ramps.

use std::{
    f64::consts::{PI, TAU},
    sync::OnceLock,
};

use crate::{
    config::{NightShift, TimeOfDay},
    state::Nekoland,
};

/// The colour temperatures, in kelvin, Night Shift's warmth runs between.
const COOLEST: f64 = 6500.0;
const WARMEST: f64 = 2000.0;

/// Where the time zones' places are, as `ISO\t±DDMM[SS]±DDDMM[SS]\tZone`.
const ZONE_TABLE: &str = "/usr/share/zoneinfo/zone1970.tab";
const LOCAL_TIME: &str = "/etc/localtime";
const ZONE_ROOT: &str = "/usr/share/zoneinfo/";
/// The sun's centre is this far below the horizon as its top shows, through the air.
const SUNRISE_ZENITH: f64 = 90.833;
const MINUTES_A_DAY: f64 = 1440.0;
const MINUTES_A_DEGREE: f64 = 4.0;
const DAYS_A_YEAR: f64 = 365.0;

impl Nekoland {
    /// Turns Night Shift on or off as its setting and schedule say, as they come to say
    /// something else; the control center turns it on or off in between.
    pub fn follow_night_shift(&mut self) {
        let now = LocalTime::now();
        let wanted = match self.config.night_shift {
            NightShift::Off => false,
            NightShift::On => true,
            NightShift::Sunset => match sun_today(&now) {
                Some(Sun::Rises(rise, set)) => in_window(now.minute, set, rise),
                Some(Sun::Up) => false,
                Some(Sun::Down) => true,
                // A zone with no place, as UTC, keeps the hours set by hand.
                None => in_window(now.minute, self.config.night_shift_from, self.config.night_shift_to),
            },
            NightShift::Scheduled => in_window(now.minute, self.config.night_shift_from, self.config.night_shift_to),
        };
        if self.night_shift_wanted != Some(wanted) {
            self.night_shift_wanted = Some(wanted);
            self.night_shift = wanted;
        }
    }
}

/// Today's sunset and sunrise, as `HH:MM HH:MM`, or `up` or `down` for a day the sun does
/// not set or rise, for Settings to offer the schedule they make; empty where the time zone
/// has no place.
pub fn sun_text() -> String {
    match sun_today(&LocalTime::now()) {
        Some(Sun::Rises(rise, set)) => format!("{} {}", String::from(set), String::from(rise)),
        Some(Sun::Up) => "up".into(),
        Some(Sun::Down) => "down".into(),
        None => String::new(),
    }
}

/// Whether `minute` is from `from` up to `to`, through midnight when `to` is earlier.
pub fn in_window(minute: u16, from: TimeOfDay, to: TimeOfDay) -> bool {
    if from.0 <= to.0 {
        (from.0..to.0).contains(&minute)
    } else {
        minute >= from.0 || minute < to.0
    }
}

/// The local time, with what working out the sun's hours needs of it.
struct LocalTime {
    minute: u16,
    /// From 0, the first of January.
    day_of_year: u16,
    /// Ahead of UTC.
    offset_minutes: f64,
}

impl LocalTime {
    fn now() -> Self {
        // SAFETY: localtime_r only writes the tm it is given.
        let tm = unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&now, &mut tm);
            tm
        };
        LocalTime {
            minute: (tm.tm_hour * 60 + tm.tm_min) as u16,
            day_of_year: tm.tm_yday as u16,
            offset_minutes: tm.tm_gmtoff as f64 / 60.0,
        }
    }
}

/// The sun on a day: rising and setting at these local times, or up or down all of it, as
/// near the poles.
#[derive(Debug, PartialEq)]
enum Sun {
    Rises(TimeOfDay, TimeOfDay),
    Up,
    Down,
}

fn sun_today(now: &LocalTime) -> Option<Sun> {
    let (latitude, longitude) = (*PLACE.get_or_init(zone_place))?;
    Some(sun_on(now.day_of_year, latitude, longitude, now.offset_minutes))
}

/// Where the time zone is, in degrees north and east, looked up once: a zone changed while
/// Nekoland runs is taken up as it starts again.
static PLACE: OnceLock<Option<(f64, f64)>> = OnceLock::new();

fn zone_place() -> Option<(f64, f64)> {
    let target = std::fs::read_link(LOCAL_TIME).ok()?;
    let zone = target.to_str()?.split_once(ZONE_ROOT)?.1.to_string();
    let table = std::fs::read_to_string(ZONE_TABLE).ok()?;
    table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let mut fields = line.split('\t');
            let (_, place, name) = (fields.next()?, fields.next()?, fields.next()?);
            (name == zone).then(|| parse_place(place)).flatten()
        })
}

/// `±DDMM[SS]±DDDMM[SS]` as degrees north and east.
fn parse_place(text: &str) -> Option<(f64, f64)> {
    let split = text[1..].find(['+', '-'])? + 1;
    let (latitude, longitude) = text.split_at(split);
    Some((parse_degrees(latitude, 2)?, parse_degrees(longitude, 3)?))
}

fn parse_degrees(text: &str, degree_digits: usize) -> Option<f64> {
    let sign = if text.starts_with('-') { -1.0 } else { 1.0 };
    let digits = &text[1..];
    let part = |range: std::ops::Range<usize>| digits.get(range).map_or(Some(0.0), |part| part.parse::<f64>().ok());
    let degrees = digits.get(..degree_digits)?.parse::<f64>().ok()?;
    let minutes = part(degree_digits..degree_digits + 2)?;
    let seconds = part(degree_digits + 2..degree_digits + 4)?;
    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

/// The sun on `day_of_year` at a place, by NOAA's approximation, within a minute or two.
fn sun_on(day_of_year: u16, latitude: f64, longitude: f64, offset_minutes: f64) -> Sun {
    let year = TAU / DAYS_A_YEAR * f64::from(day_of_year);
    let equation_of_time = 229.18
        * (0.000075 + 0.001868 * year.cos()
            - 0.032077 * year.sin()
            - 0.014615 * (2.0 * year).cos()
            - 0.040849 * (2.0 * year).sin());
    let declination = 0.006918 - 0.399912 * year.cos() + 0.070257 * year.sin()
        - 0.006758 * (2.0 * year).cos()
        + 0.000907 * (2.0 * year).sin()
        - 0.002697 * (3.0 * year).cos()
        + 0.00148 * (3.0 * year).sin();
    let latitude = latitude.to_radians();
    let cos_hour_angle =
        SUNRISE_ZENITH.to_radians().cos() / (latitude.cos() * declination.cos()) - latitude.tan() * declination.tan();
    if cos_hour_angle > 1.0 {
        return Sun::Down;
    }
    if cos_hour_angle < -1.0 {
        return Sun::Up;
    }
    let hour_angle = cos_hour_angle.acos() * 180.0 / PI;
    let noon = MINUTES_A_DAY / 2.0 - MINUTES_A_DEGREE * longitude - equation_of_time + offset_minutes;
    let local = |minutes: f64| TimeOfDay(minutes.rem_euclid(MINUTES_A_DAY).round() as u16 % MINUTES_A_DAY as u16);
    Sun::Rises(
        local(noon - MINUTES_A_DEGREE * hour_angle),
        local(noon + MINUTES_A_DEGREE * hour_angle),
    )
}

/// The minute of the day, local time.
pub fn minute_of_day() -> u16 {
    LocalTime::now().minute
}

/// The red, green and blue gains of light at the colour temperature `warmth`, from 0 to 1,
/// stands for, after Tanner Helland's fit of black-body colours.
pub fn gains(warmth: f64) -> (f64, f64, f64) {
    let hundreds = (COOLEST - warmth.clamp(0.0, 1.0) * (COOLEST - WARMEST)) / 100.0;
    let green = (99.470_802_586_1 * hundreds.ln() - 161.119_568_166_1) / 255.0;
    let blue = (138.517_731_223_1 * (hundreds - 10.0).ln() - 305.044_792_730_7) / 255.0;
    (1.0, green.clamp(0.0, 1.0), blue.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_may_run_through_midnight() {
        let (night, morning) = (TimeOfDay(22 * 60), TimeOfDay(7 * 60));
        assert!(in_window(23 * 60, night, morning));
        assert!(in_window(3 * 60, night, morning));
        assert!(!in_window(7 * 60, night, morning));
        assert!(!in_window(12 * 60, night, morning));
        assert!(in_window(12 * 60, morning, night));
        assert!(!in_window(12 * 60, night, night));
    }

    #[test]
    fn warmer_takes_away_blue_before_green() {
        let (_, green, blue) = gains(0.0);
        assert!(green > 0.99 && blue > 0.97);
        let (red, green, blue) = gains(1.0);
        assert_eq!(red, 1.0);
        assert!(green < 0.6 && blue < 0.2 && blue < green);
    }

    #[test]
    fn zone_places_read_as_degrees() {
        let (latitude, longitude) = parse_place("+2503+12130").unwrap();
        assert!((latitude - 25.05).abs() < 1e-9 && (longitude - 121.5).abs() < 1e-9);
        let (latitude, longitude) = parse_place("+404251-0740023").unwrap();
        assert!((latitude - 40.714_17).abs() < 1e-4 && (longitude + 74.006_39).abs() < 1e-4);
        assert_eq!(parse_place("+25"), None);
    }

    /// Within a few minutes of the almanac's: Taipei on the 18th of November rises about
    /// 06:15 and sets about 17:05; London on the 21st of June about 04:43 and 21:21.
    #[test]
    fn the_sun_rises_and_sets_where_the_almanac_says() {
        let near = |time: TimeOfDay, hour: u16, minute: u16| (i32::from(time.0) - i32::from(hour * 60 + minute)).abs() <= 6;
        let Sun::Rises(rise, set) = sun_on(321, 25.05, 121.5, 480.0) else { panic!("Taipei has a night") };
        assert!(near(rise, 6, 15) && near(set, 17, 5), "{rise:?} {set:?}");
        let Sun::Rises(rise, set) = sun_on(171, 51.508, -0.125, 60.0) else { panic!("London has a night") };
        assert!(near(rise, 4, 43) && near(set, 21, 21), "{rise:?} {set:?}");
    }

    #[test]
    fn near_the_poles_the_sun_may_stay_up_or_down() {
        assert_eq!(sun_on(171, 78.2, 15.6, 120.0), Sun::Up);
        assert_eq!(sun_on(355, 78.2, 15.6, 60.0), Sun::Down);
    }
}
