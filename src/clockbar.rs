//! The clock in the menu bar and what the calendar asks of it: its text from a format of the
//! user's or the usual one, the lunar date and the next event beside it, the flash that says
//! an event is near, the sound, the notification at the time an event named, and the card
//! over the whole screen that holds until it is dismissed.

use std::process::Command;

use cairo::Context;
use calendar_data::{
    date::{now, Date},
    format::{format_clock, Moment},
    lunar,
    upcoming::{alert_due, next_after, until_text},
};
use gio::prelude::*;
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Rectangle},
};

use crate::{
    agenda::Agenda,
    config::{self, Config},
    control::show_line,
    menubar::{font, show_text, Overlay},
    text::{self, Picture},
    Nekoland,
};

const SECONDS_IN_MINUTE: i64 = 60;
const SECONDS_IN_DAY: i64 = 86_400;
/// How long after its time an alert still counts as due, so that a frame that comes late
/// does not lose it.
const ALERT_WINDOW: i64 = 120;
/// How many alerts already given are remembered.
const REMEMBERED: usize = 64;
/// How far ahead an alert may be set, in days, from when the event is.
const ALERT_DAYS_AHEAD: i64 = 3;
const NOTIFICATION_TIMEOUT_MS: i32 = 10_000;
const NOTIFICATION_ICON: &str = "x-office-calendar";
const NOTIFICATION_APP: &str = "行事曆";
const SOUND_COMMAND: &str = "canberra-gtk-play -i alarm-clock-elapsed 2>/dev/null \
    || paplay /usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga 2>/dev/null";
const ALERT_CARD_WIDTH: i32 = 360;
const ALERT_CARD_HEIGHT: i32 = 112;
const ALERT_BACKDROP_ALPHA: f64 = 0.5;
const ALERT_CARD_RADIUS: f64 = 14.0;
const ALERT_PADDING: f64 = 24.0;
/// The mini calendar the bar's icon option draws.
pub(crate) const GLYPH_WIDTH: i32 = 14;
const GLYPH_HEIGHT: f64 = 13.0;
const GLYPH_BAND: f64 = 3.5;
pub(crate) const GLYPH_GAP: i32 = 5;

/// What the bar draws for the clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Changes when the text or the flash does, so the bar is drawn again.
    pub key: i64,
    pub text: String,
    /// The day of the month, when an icon is asked for.
    pub glyph_day: Option<u32>,
    /// Whether the flash is on, for an event that is near.
    pub flash: bool,
}

/// An alert held over the whole screen.
pub struct Alert {
    title: String,
    detail: String,
    picture: Option<Picture>,
    size: (i32, i32),
    id: Id,
}

#[derive(Default)]
pub struct ClockBar {
    /// The events whose alert was given, by id and when they begin.
    notified: Vec<(String, i64)>,
    /// The event whose coming the sound was made for.
    sounded: Option<(String, i64)>,
    pub(crate) alert: Option<Alert>,
}

fn remember(list: &mut Vec<(String, i64)>, entry: (String, i64)) {
    list.push(entry);
    if list.len() > REMEMBERED {
        list.remove(0);
    }
}

/// The clock as the bar has always had it: the date, the weekday and the time.
fn usual_text(config: &Config, moment: Moment) -> String {
    let date = if config.clock_date { format!("{}月{}日 ", moment.month, moment.day) } else { String::new() };
    let weekday = if config.clock_weekday {
        format!("週{} ", calendar_data::date::WEEKDAY_NAMES[moment.weekday])
    } else {
        String::new()
    };
    // A flashing separator is a punctuation space every other second, about as wide.
    let colon = if config.clock_flash && moment.second % 2 == 1 { '\u{2008}' } else { ':' };
    let time = if config.clock_24h {
        format!("{:02}{colon}{:02}", moment.hour, moment.minute)
    } else {
        let half = if moment.hour < 12 { "上午" } else { "下午" };
        format!("{half} {}{colon}{:02}", (moment.hour + 11) % 12 + 1, moment.minute)
    };
    let seconds = if config.clock_seconds { format!(":{:02}", moment.second) } else { String::new() };
    format!("{date}{weekday}{time}{seconds}")
}

/// Whether the clock changes every second, as these options make it.
fn ticks_each_second(config: &Config) -> bool {
    config.clock_seconds || config.clock_flash || config.clock_attention || pattern_has_seconds(&config.clock_format)
}

fn pattern_has_seconds(pattern: &str) -> bool {
    let mut quoted = false;
    for character in pattern.chars() {
        match character {
            '\'' => quoted = !quoted,
            's' if !quoted => return true,
            _ => {}
        }
    }
    false
}

/// What the clock says at `seconds`, with the lunar date and the next event beside it, and
/// whether the flash is lit.
pub fn face(config: &Config, agenda: &Agenda, seconds: i64) -> Face {
    let moment = Moment::local(seconds);
    let mut text = if config.clock_format.trim().is_empty() {
        usual_text(config, moment)
    } else {
        format_clock(&config.clock_format, seconds)
    };
    if config.clock_lunar {
        let today = Date { year: moment.year, month: moment.month, day: moment.day };
        if let Some(found) = lunar::from_solar(today) {
            text = format!("{} {text}", found.label());
        }
    }
    let next = next_after(agenda.events(), seconds, |event| !agenda.shown(event.calendar));
    if let Some(found) = next.as_ref().filter(|found| config.clock_next_event && found.begins - seconds < SECONDS_IN_DAY) {
        let wait = until_text(found.begins, seconds);
        let event = &agenda.events()[found.index];
        let line = if config.clock_next_event_title { format!("{} {wait}", event.title) } else { wait };
        text = format!("{line}  {text}");
    }
    let near = next.is_some_and(|found| {
        let ahead = found.begins - seconds;
        config.clock_attention && ahead >= 0 && ahead <= i64::from(config.clock_attention_minutes.max(1)) * SECONDS_IN_MINUTE
    });
    let flash = near && moment.second % 2 == 1;
    let key = if ticks_each_second(config) { seconds } else { seconds.div_euclid(SECONDS_IN_MINUTE) };
    Face {
        key: key * 2 + i64::from(flash),
        text,
        glyph_day: config.clock_icon.then_some(moment.day),
        flash,
    }
}

/// The mini calendar of the bar's icon option: a frame with a band on top and the day in it.
pub(crate) fn draw_glyph(cr: &Context, x: f64, center: f64, day: u32, color: (f64, f64, f64)) {
    let top = center - GLYPH_HEIGHT / 2.0;
    let width = f64::from(GLYPH_WIDTH);
    text::rounded_rectangle(cr, x + 0.5, top + 0.5, width - 1.0, GLYPH_HEIGHT - 1.0, 2.5);
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    cr.rectangle(x + 0.5, top + 0.5, width - 1.0, GLYPH_BAND);
    let _ = cr.fill();
    let digits = font("Bold", 6.5);
    let layout_width = {
        let layout = text::new_layout(cr);
        layout.set_font_description(Some(&digits));
        layout.set_text(&day.to_string());
        f64::from(layout.pixel_size().0)
    };
    show_text(cr, &day.to_string(), &digits, x + (width - layout_width) / 2.0, center + GLYPH_BAND / 2.0 + 0.5, color);
}

fn notify(title: &str, body: &str) {
    let spawned = Command::new("gdbus")
        .args(["call", "--session", "--dest", "org.freedesktop.Notifications"])
        .args(["--object-path", "/org/freedesktop/Notifications"])
        .args(["--method", "org.freedesktop.Notifications.Notify"])
        .arg(NOTIFICATION_APP.to_variant().print(false))
        .args(["0", NOTIFICATION_ICON])
        .arg(title.to_variant().print(false))
        .arg(body.to_variant().print(false))
        .args(["[]", "{}", &NOTIFICATION_TIMEOUT_MS.to_string()])
        .spawn();
    if let Err(err) = spawned {
        tracing::warn!("gdbus: {err}");
    }
}

fn detail_of(event: &calendar_data::Event, seconds: i64, begins: i64) -> String {
    let mut parts = vec![until_text(begins, seconds)];
    if let Some((hour, minute)) = event.start {
        parts.push(format!("{hour:02}:{minute:02}"));
    }
    if !event.location.is_empty() {
        parts.push(event.location.clone());
    }
    parts.join(" · ")
}

impl Nekoland {
    /// What the bar's clock is now, and what that sets off: the sound as an event comes near
    /// and the alerts that are due.
    pub(crate) fn clock_face(&mut self) -> Face {
        let seconds = now();
        let face = face(&self.config, &self.menu_bar.agenda, seconds);
        self.give_alerts(seconds);
        face
    }

    fn give_alerts(&mut self, seconds: i64) {
        let today = Date::from_epoch_days(calendar_data::date::local_parts(seconds).0);
        let agenda = &self.menu_bar.agenda;
        let mut due: Vec<(String, String, String, i64)> = Vec::new();
        for ahead in 0..ALERT_DAYS_AHEAD {
            let day = today.plus_days(ahead);
            for event in agenda.events() {
                let Some((hour, minute)) = event.start else {
                    continue;
                };
                if !event.occurs_on(day) || !agenda.shown(event.calendar) {
                    continue;
                }
                if !alert_due(event, day, seconds, ALERT_WINDOW) {
                    continue;
                }
                let begins = calendar_data::date::local_seconds(day, hour, minute);
                due.push((event.id.clone(), event.title.clone(), detail_of(event, seconds, begins), begins));
            }
        }
        for (id, title, detail, begins) in due {
            let key = (id, begins);
            if self.menu_bar.clock_bar.notified.contains(&key) {
                continue;
            }
            remember(&mut self.menu_bar.clock_bar.notified, key);
            notify(&title, &detail);
            if self.config.clock_attention_sound {
                config::spawn(SOUND_COMMAND);
            }
            if self.config.clock_fullscreen_alert {
                self.menu_bar.clock_bar.alert = Some(Alert { title, detail, picture: None, size: (0, 0), id: Id::new() });
            }
        }
        self.sound_when_near(seconds);
    }

    /// The sound, once for each event, as the minutes the flash starts at begin.
    fn sound_when_near(&mut self, seconds: i64) {
        if !(self.config.clock_attention && self.config.clock_attention_sound) {
            return;
        }
        let agenda = &self.menu_bar.agenda;
        let Some(found) = next_after(agenda.events(), seconds, |event| !agenda.shown(event.calendar)) else {
            return;
        };
        let ahead = found.begins - seconds;
        if ahead > i64::from(self.config.clock_attention_minutes.max(1)) * SECONDS_IN_MINUTE {
            return;
        }
        let key = (agenda.events()[found.index].id.clone(), found.begins);
        if self.menu_bar.clock_bar.sounded.as_ref() == Some(&key) {
            return;
        }
        self.menu_bar.clock_bar.sounded = Some(key);
        config::spawn(SOUND_COMMAND);
    }

    pub(crate) fn alert_shown(&self) -> bool {
        self.menu_bar.clock_bar.alert.is_some()
    }

    pub(crate) fn dismiss_alert(&mut self) {
        self.menu_bar.clock_bar.alert = None;
    }

    /// Draws the alert's card over a dimmed screen, once for the size the output has.
    pub(crate) fn refresh_alert(&mut self) {
        let Some(output) = self.output_area() else {
            return;
        };
        let dark = self.menu_bar.dark;
        let Some(alert) = self.menu_bar.clock_bar.alert.as_mut() else {
            return;
        };
        let size = (output.size.w, output.size.h);
        if alert.picture.is_some() && alert.size == size {
            return;
        }
        let Ok(surface) = text::canvas(size.0.max(1), size.1.max(1)) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        cr.set_source_rgba(0.0, 0.0, 0.0, ALERT_BACKDROP_ALPHA);
        let _ = cr.paint();
        let colors = crate::menubar::colors(dark);
        let card = Rectangle::<i32, Logical>::new(
            ((size.0 - ALERT_CARD_WIDTH) / 2, (size.1 - ALERT_CARD_HEIGHT) / 2).into(),
            (ALERT_CARD_WIDTH, ALERT_CARD_HEIGHT).into(),
        );
        let (x, y, w, h) = crate::control::to_f64(card);
        text::rounded_rectangle(&cr, x, y, w, h, ALERT_CARD_RADIUS);
        cr.set_source_rgb(colors.sheet.0, colors.sheet.1, colors.sheet.2);
        let _ = cr.fill();
        let inner = w - ALERT_PADDING * 2.0;
        show_line(&cr, &alert.title, &font("Bold", 16.0), (x + ALERT_PADDING, y + 42.0, inner), colors.ink);
        show_line(&cr, &alert.detail, &font("", 11.0), (x + ALERT_PADDING, y + 72.0, inner), colors.ink_3);
        drop(cr);
        alert.picture = text::picture(surface);
        alert.size = size;
    }

    pub(crate) fn alert_overlay(&self) -> Option<Overlay<'_>> {
        let output = self.output_area()?;
        let alert = self.menu_bar.clock_bar.alert.as_ref()?;
        let picture = alert.picture.as_ref()?;
        Some(Overlay {
            id: &alert.id,
            rect: Rectangle::new(output.loc, picture.size),
            glass: None,
            alpha: 1.0,
            fade: 1.0,
            picture,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_in_a_pattern_are_found_outside_quotes_only() {
        assert!(pattern_has_seconds("HH:mm:ss"));
        assert!(!pattern_has_seconds("HH:mm 'secs'"));
        assert!(!pattern_has_seconds(""));
    }

    #[test]
    fn the_usual_clock_has_what_the_options_ask() {
        let config = Config::default();
        let moment = Moment { year: 2026, month: 11, day: 8, hour: 15, minute: 4, second: 5, weekday: 0 };
        assert_eq!(usual_text(&config, moment), "11月8日 週日 15:04");
        let mut twelve = Config::default();
        twelve.clock_24h = false;
        twelve.clock_date = false;
        twelve.clock_weekday = false;
        twelve.clock_seconds = true;
        assert_eq!(usual_text(&twelve, moment), "下午 3:04:05");
    }

    #[test]
    fn alerts_already_given_are_remembered_up_to_a_limit() {
        let mut list = Vec::new();
        for at in 0..(REMEMBERED + 5) {
            remember(&mut list, (format!("e{at}"), 0));
        }
        assert_eq!(list.len(), REMEMBERED);
        assert_eq!(list[0].0, "e5");
    }
}
