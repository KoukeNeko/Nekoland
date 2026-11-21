//! The widgets Super+W brings up at the left, under the bar, as Windows' board: the day,
//! what is on today, what plays and the battery; what plays and the battery only when
//! there are any.

use std::{sync::mpsc::Receiver, time::Instant};

use calendar_data::{date::WEEKDAY_NAMES, event::on_day, Date};
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    clockmenu::{when_text, CALENDAR_COMMAND},
    config,
    control::{draw_panel, ease_out, OPEN_TIME},
    glass::Tier,
    menubar::{colors, font, show_text, text_size, Overlay, MENU_BAR_HEIGHT, MENU_RADIUS},
    system::{self, Player},
    text::{self, Picture},
    Nekoland,
};

const WIDTH: i32 = 320;
const PADDING: i32 = 16;
const INSET: i32 = 8;
const ROW: i32 = 24;
/// Between one widget and the next.
const GAP: i32 = 16;
const EVENTS_SHOWN: usize = 5;

pub struct Widgets {
    /// What plays, read off the compositor's thread once it opens.
    player: Option<Player>,
    found: Option<Receiver<Option<Player>>>,
    id: Id,
    picture: Option<Picture>,
    drawn: Option<(Date, bool, Option<Player>)>,
    rect: Rectangle<i32, Logical>,
    /// Where the agenda is in the panel, for a click there to open the calendar.
    agenda: Option<Rectangle<i32, Logical>>,
    opened: Instant,
}

/// Cuts `text` to `room` pixels wide, with an ellipsis.
fn fit(text: &str, font: &pango::FontDescription, room: i32) -> String {
    if text_size(text, font).0 <= room {
        return text.to_string();
    }
    let mut cut = text.to_string();
    while !cut.is_empty() && text_size(&format!("{cut}…"), font).0 > room {
        cut.pop();
    }
    cut + "…"
}

impl Nekoland {
    pub fn toggle_widgets(&mut self) {
        if self.widgets.take().is_some() {
            return;
        }
        let (send, found) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("nekoland-widgets".into())
            .spawn(move || drop(send.send(system::read_player())));
        if let Err(err) = &spawned {
            tracing::warn!("widgets: {err}");
        }
        self.widgets = Some(Widgets {
            player: None,
            found: spawned.is_ok().then_some(found),
            id: Id::new(),
            picture: None,
            drawn: None,
            rect: Rectangle::default(),
            agenda: None,
            opened: Instant::now(),
        });
    }

    /// Esc closes the widgets. Returns whether it did.
    pub fn widgets_key(&mut self, sym: smithay::input::keyboard::Keysym) -> bool {
        if self.widgets.is_some() && sym == smithay::input::keyboard::Keysym::Escape {
            self.widgets = None;
            return true;
        }
        false
    }

    /// A click on the agenda opens the calendar; anywhere outside closes the widgets.
    /// Returns whether the click was theirs.
    pub fn widgets_press(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(widgets) = self.widgets.as_ref() else {
            return false;
        };
        let at = pos.to_i32_round();
        if !widgets.rect.contains(at) {
            self.widgets = None;
            return false;
        }
        if widgets
            .agenda
            .is_some_and(|agenda| agenda.contains(at - widgets.rect.loc))
        {
            config::spawn(&format!("{CALENDAR_COMMAND} --date {}", Date::today().key()));
            self.widgets = None;
        }
        true
    }

    pub fn refresh_widgets(&mut self) {
        let dark = self.menu_bar.dark;
        let Some(output) = self.bar_area() else {
            return;
        };
        let Some(widgets) = self.widgets.as_mut() else {
            return;
        };
        if let Some(player) = widgets.found.as_ref().and_then(|found| found.try_recv().ok()) {
            widgets.player = player;
            widgets.found = None;
        }
        let today = Date::today();
        let drawn = (today, dark, widgets.player.clone());
        if widgets.drawn.as_ref() == Some(&drawn) && widgets.picture.is_some() {
            return;
        }
        let agenda = &self.menu_bar.agenda;
        let events: Vec<(String, String)> = on_day(agenda.events(), today)
            .into_iter()
            .filter(|occurrence| agenda.shown(occurrence.event.calendar))
            .take(EVENTS_SHOWN)
            .map(|occurrence| (when_text(occurrence.event), occurrence.event.title.clone()))
            .collect();
        let battery = self.menu_bar.battery;

        let colors = colors(dark);
        let ink = colors.ink;
        let title = font("600", 10.0);
        let body = font("400", 10.5);
        let big = font("600", 20.0);
        let room = WIDTH - 2 * PADDING;

        // Each widget's rows: a heading, then what it says, measured before it is drawn.
        let mut height = PADDING + 36;
        // The agenda stays when the day is free, as the way into the calendar.
        let agenda_top = height + GAP;
        let agenda_rows = 1 + events.len().max(1) as i32;
        height = agenda_top + ROW * agenda_rows;
        let player_top = height + GAP;
        if widgets.player.is_some() {
            height = player_top + ROW * 3;
        }
        let battery_top = height + GAP;
        if battery.is_some() {
            height = battery_top + ROW * 2;
        }
        height += PADDING;

        let Some((surface, cr)) = draw_panel(WIDTH, height, dark, &colors) else {
            return;
        };
        let x = f64::from(PADDING);
        let center = |top: i32, row: i32| f64::from(top + ROW * row + ROW / 2);
        let heading = |top: i32, label: &str| show_text(&cr, label, &title, x, center(top, 0), colors.ink_3);
        let day = format!(
            "{}月{}日 週{}",
            today.month,
            today.day,
            WEEKDAY_NAMES[today.weekday()]
        );
        show_text(&cr, &day, &big, x, f64::from(PADDING + 18), ink);

        heading(agenda_top, "今天");
        if events.is_empty() {
            show_text(&cr, "沒有行程", &body, x, center(agenda_top, 1), ink);
        }
        for (row, (when, what)) in events.iter().enumerate() {
            let y = center(agenda_top, row as i32 + 1);
            let (when_width, _) = text_size(when, &body);
            show_text(&cr, when, &body, x, y, ink);
            let shown = fit(what, &body, room - when_width - 12);
            show_text(
                &cr,
                &shown,
                &font("500", 10.5),
                x + f64::from(when_width + 12),
                y,
                ink,
            );
        }
        if let Some(player) = &widgets.player {
            heading(
                player_top,
                if player.playing {
                    "正在播放"
                } else {
                    "已暫停"
                },
            );
            show_text(
                &cr,
                &fit(&player.title, &font("500", 10.5), room),
                &font("500", 10.5),
                x,
                center(player_top, 1),
                ink,
            );
            let by = if player.artist.is_empty() {
                &player.app
            } else {
                &player.artist
            };
            show_text(&cr, &fit(by, &body, room), &body, x, center(player_top, 2), colors.ink_3);
        }
        if let Some(battery) = battery {
            heading(battery_top, "電池");
            let state = if battery.charging { "，充電中" } else { "" };
            show_text(
                &cr,
                &format!("{}%{state}", battery.percent),
                &body,
                x,
                center(battery_top, 1),
                ink,
            );
        }
        drop(cr);
        widgets.picture = text::picture(surface);
        widgets.drawn = Some(drawn);
        widgets.agenda = Some(Rectangle::new(
            (0, agenda_top).into(),
            (WIDTH, ROW * agenda_rows).into(),
        ));
        widgets.rect = Rectangle::new(
            output.loc + Point::from((INSET, MENU_BAR_HEIGHT + INSET)),
            (WIDTH, height).into(),
        );
    }

    pub fn widgets_overlay(&self) -> Option<Overlay<'_>> {
        let widgets = self.widgets.as_ref()?;
        let picture = widgets.picture.as_ref()?;
        Some(Overlay {
            id: &widgets.id,
            rect: Rectangle::new(widgets.rect.loc, picture.size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: ease_out(widgets.opened.elapsed().as_secs_f64() / OPEN_TIME) as f32,
            picture,
        })
    }
}
