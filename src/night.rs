//! Night Shift: warmer colours at night, as the Mac's, through the displays' gamma ramps.

use crate::{
    config::{NightShift, TimeOfDay},
    state::Nekoland,
};

/// The colour temperatures, in kelvin, Night Shift's warmth runs between.
const COOLEST: f64 = 6500.0;
const WARMEST: f64 = 2000.0;

impl Nekoland {
    /// Turns Night Shift on or off as its setting and schedule say, as they come to say
    /// something else; the control center turns it on or off in between.
    pub fn follow_night_shift(&mut self) {
        let wanted = match self.config.night_shift {
            NightShift::Off => false,
            NightShift::On => true,
            NightShift::Scheduled => {
                in_window(minute_of_day(), self.config.night_shift_from, self.config.night_shift_to)
            }
        };
        if self.night_shift_wanted != Some(wanted) {
            self.night_shift_wanted = Some(wanted);
            self.night_shift = wanted;
        }
    }
}

/// Whether `minute` is from `from` up to `to`, through midnight when `to` is earlier.
fn in_window(minute: u16, from: TimeOfDay, to: TimeOfDay) -> bool {
    if from.0 <= to.0 {
        (from.0..to.0).contains(&minute)
    } else {
        minute >= from.0 || minute < to.0
    }
}

fn minute_of_day() -> u16 {
    // SAFETY: localtime_r only writes the tm it is given.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    (tm.tm_hour * 60 + tm.tm_min) as u16
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
}
