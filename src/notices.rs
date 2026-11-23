//! Notifications, shown by Nekoland itself as banners on glass at the top right, under the
//! menu bar, newest first. A banner goes away when its time is up, unless the pointer
//! rests on it; a click on it does what its program offers for that, or dismisses it.
//! What is not transient is kept after its banner for the Notification Center, as it is
//! when it comes during Do Not Disturb, without a banner; a program turned off in Settings
//! shows nowhere.

use std::time::{Duration, Instant};

use cairo::{Context, Format, ImageSurface};
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    dbus::{Event, Expiry, Notification, Request},
    glass::Tier,
    menubar::{Overlay, MENU_BAR_HEIGHT},
    text::{self, Picture},
    Nekoland,
};

pub(crate) const WIDTH: i32 = 360;
const PADDING: i32 = 12;
const ICON: i32 = 36;
const GAP: i32 = 8;
const RADIUS: i32 = 14;
/// How far the banners, and the Notification Center where they are, stay from the screen's
/// edges.
pub(crate) const MARGIN: i32 = 12;
const MAX_SHOWN: usize = 4;
const BODY_LINES: i32 = 3;
/// How many notifications the Notification Center keeps; past it, the oldest go.
pub(crate) const KEPT_MAX: usize = 50;
/// The room between the text of a card's first line and the time at its end.
const TIME_GAP: i32 = 8;
/// The room between the lines of a card.
const LINE_GAP: i32 = 2;
const TITLE_FONT: &str = "Bold 10";
const BODY_FONT: &str = "9.5";
/// The time's, and the program's name over the title.
const SMALL_FONT: &str = "8.5";

const SECONDS_A_MINUTE: u64 = 60;
const SECONDS_AN_HOUR: u64 = 60 * SECONDS_A_MINUTE;
const SECONDS_A_DAY: u64 = 24 * SECONDS_AN_HOUR;

// Why a notification went away, as the specification numbers it.
const EXPIRED: u32 = 1;
pub(crate) const DISMISSED: u32 = 2;
const CLOSED: u32 = 3;

struct Banner {
    notification: Notification,
    deadline: Option<Instant>,
    picture: Option<Picture>,
    drawn_dark: Option<bool>,
    id: Id,
    /// When it was first shown, to come in from then.
    came: Option<Instant>,
    /// How long it had left when the pointer came to rest on it, the time it has again
    /// once the pointer leaves.
    paused: Option<Duration>,
    /// Where it stands from the top, and where it was before it last moved and when, to
    /// glide there.
    place: Option<(i32, f64, Instant)>,
    /// When it was pressed, and when it was let go and how far it had sunk then.
    held: Option<(Instant, Option<(Instant, f64)>)>,
}

/// A banner gone, still going back out the way it came.
struct Leaving {
    picture: Picture,
    id: Id,
    rect: Rectangle<i32, Logical>,
    left: Instant,
    held: Option<(Instant, Option<(Instant, f64)>)>,
}

/// A notification kept after its banner, for the Notification Center.
pub(crate) struct Kept {
    pub(crate) notification: Notification,
    /// When it came, or when its program last replaced it.
    pub(crate) came: Instant,
    /// Its icon, once drawn, and whether that was for dark glass.
    pub(crate) icon: Option<(bool, Option<ImageSurface>)>,
}

#[derive(Default)]
pub struct Notices {
    /// Do Not Disturb, from the control center: no banners come up.
    pub(crate) quiet: bool,
    banners: Vec<Banner>,
    leaving: Vec<Leaving>,
    hovered: Option<u32>,
    /// What the Notification Center lists, newest first.
    pub(crate) kept: Vec<Kept>,
    /// Counts each change to `kept`, for the Notification Center to tell when to draw again.
    pub(crate) kept_revision: u64,
}

/// What a notification's card shows besides the notification itself.
pub(crate) struct Face<'a> {
    /// The program's name, over the title, as the Notification Center shows it.
    pub(crate) app: Option<&'a str>,
    /// When it came, at the end of the first line.
    pub(crate) time: &'a str,
    pub(crate) width: i32,
    /// Whether its text is shown, or its title alone.
    pub(crate) body: bool,
}

/// When a notification came, as the Mac words it: now, then minutes, hours or days ago.
pub(crate) fn relative_time(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        seconds if seconds < SECONDS_A_MINUTE => "現在".to_string(),
        seconds if seconds < SECONDS_AN_HOUR => format!("{} 分鐘前", seconds / SECONDS_A_MINUTE),
        seconds if seconds < SECONDS_A_DAY => format!("{} 小時前", seconds / SECONDS_AN_HOUR),
        seconds => format!("{} 天前", seconds / SECONDS_A_DAY),
    }
}

/// Keeps `notification`, newest first, in place of one it replaces. Returns the ids of the
/// oldest, pushed past `KEPT_MAX`.
fn keep(kept: &mut Vec<Kept>, notification: Notification, came: Instant) -> Vec<u32> {
    kept.retain(|known| known.notification.id != notification.id);
    kept.insert(
        0,
        Kept {
            notification,
            came,
            icon: None,
        },
    );
    kept.split_off(KEPT_MAX.min(kept.len()))
        .into_iter()
        .map(|old| old.notification.id)
        .collect()
}

/// Takes the notification `id` out of those kept. Returns whether it was there.
fn forget(kept: &mut Vec<Kept>, id: u32) -> bool {
    let before = kept.len();
    kept.retain(|known| known.notification.id != id);
    kept.len() != before
}

type Rgb = (f64, f64, f64);

/// The ink, the second ink, and the tint a banner stands on: as solid as the menu bar's,
/// over a blur as strong, the bar's look in Settings.
fn colors(dark: bool) -> (Rgb, Rgb, (f64, f64, f64, f64)) {
    if dark {
        (
            (0.945, 0.937, 0.918),
            (0.663, 0.655, 0.671),
            crate::glass::tint(dark, Tier::Bar),
        )
    } else {
        (
            (0.090, 0.090, 0.102),
            (0.365, 0.361, 0.380),
            crate::glass::tint(dark, Tier::Bar),
        )
    }
}

fn layout(cr: &Context, text: &str, font: &str, width: i32, lines: i32) -> pango::Layout {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(&text::font(font)));
    layout.set_text(text);
    layout.set_width(width * pango::SCALE);
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_height(-lines);
    layout
}

/// The lines of a notification's card, laid out, and how tall the card is for them.
struct CardLines {
    app: Option<pango::Layout>,
    title: pango::Layout,
    time: pango::Layout,
    body: Option<pango::Layout>,
    height: i32,
    /// Where the first line is: text shorter than the icon, a title alone, stands in the
    /// middle beside it.
    top: i32,
}

/// Where a card's text starts, right of its icon.
const TEXT_X: i32 = PADDING + ICON + 12;

fn card_lines(cr: &Context, notification: &Notification, face: &Face) -> CardLines {
    let text_width = face.width - TEXT_X - PADDING;
    let title_text = if notification.summary.is_empty() {
        &notification.app_name
    } else {
        &notification.summary
    };
    let time = layout(cr, face.time, SMALL_FONT, text_width, 1);
    // The time ends the first line, the program's name when there is one, else the title.
    let beside_time = text_width - time.pixel_size().0 - TIME_GAP;
    let app = face.app.map(|app| layout(cr, app, SMALL_FONT, beside_time, 1));
    let title_width = if app.is_some() { text_width } else { beside_time };
    let title = layout(cr, title_text, TITLE_FONT, title_width, 1);
    let body = (face.body && !notification.body.is_empty())
        .then(|| layout(cr, &notification.body, BODY_FONT, text_width, BODY_LINES));
    let heights: Vec<i32> = [app.as_ref(), Some(&title), body.as_ref()]
        .into_iter()
        .flatten()
        .map(|line| line.pixel_size().1)
        .collect();
    let text_height = heights.iter().sum::<i32>() + LINE_GAP * (heights.len() as i32 - 1);
    let height = (PADDING * 2 + text_height).max(PADDING * 2 + ICON);
    CardLines {
        app,
        title,
        time,
        body,
        height,
        top: (height - text_height) / 2,
    }
}

/// How tall a notification's card is.
pub(crate) fn card_height(notification: &Notification, face: &Face) -> Option<i32> {
    let probe = Context::new(ImageSurface::create(Format::ARgb32, 1, 1).ok()?).ok()?;
    Some(card_lines(&probe, notification, face).height)
}

/// What a notification's card shows, a banner's or a row of the Notification Center's, from
/// the context's origin: its icon, the program's name if the face has it, its title, the
/// time and its text. What it stands on is the caller's.
pub(crate) fn draw_card(
    cr: &Context,
    notification: &Notification,
    icon: Option<&ImageSurface>,
    dark: bool,
    face: &Face,
) -> Option<()> {
    let (ink, ink_2, _) = colors(dark);
    match icon {
        Some(icon) => {
            let (w, h) = text::points(icon);
            let scale = f64::from(ICON) / w.max(h).max(1.0);
            cr.save().ok()?;
            cr.translate(f64::from(PADDING), f64::from(PADDING));
            cr.scale(scale, scale);
            cr.set_source_surface(icon, 0.0, 0.0).ok()?;
            cr.paint().ok()?;
            cr.restore().ok()?;
        }
        None => {
            text::rounded_rectangle(
                cr,
                f64::from(PADDING),
                f64::from(PADDING),
                f64::from(ICON),
                f64::from(ICON),
                9.0,
            );
            cr.set_source_rgba(ink.0, ink.1, ink.2, 0.14);
            cr.fill().ok()?;
        }
    }

    let lines = card_lines(cr, notification, face);
    let show = |line: &pango::Layout, y: i32, (r, g, b): Rgb| {
        cr.set_source_rgb(r, g, b);
        cr.move_to(f64::from(TEXT_X), f64::from(y));
        pangocairo::functions::show_layout(cr, line);
    };
    let mut y = lines.top;
    let time_x = f64::from(face.width - PADDING - lines.time.pixel_size().0);
    let show_time = |y: i32| {
        cr.set_source_rgb(ink_2.0, ink_2.1, ink_2.2);
        cr.move_to(time_x, f64::from(y));
        pangocairo::functions::show_layout(cr, &lines.time);
    };
    if let Some(app) = &lines.app {
        show(app, y, ink_2);
        show_time(y);
        y += app.pixel_size().1 + LINE_GAP;
    }
    show(&lines.title, y, ink);
    if lines.app.is_none() {
        // The time's type is smaller than the title's; it sits a little lower to line up.
        show_time(y + 1);
    }
    y += lines.title.pixel_size().1 + LINE_GAP;
    if let Some(body) = &lines.body {
        show(body, y, ink_2);
    }
    Some(())
}

fn draw_banner(notification: &Notification, icon: Option<ImageSurface>, dark: bool, body: bool) -> Option<Picture> {
    let (_, _, tint) = colors(dark);
    let time = relative_time(Duration::ZERO);
    let face = Face {
        app: None,
        time: &time,
        width: WIDTH,
        body,
    };
    let height = card_height(notification, &face)?;

    let surface = text::canvas(WIDTH, height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        text::rounded_rectangle(
            &cr,
            0.5,
            0.5,
            f64::from(WIDTH) - 1.0,
            f64::from(height) - 1.0,
            f64::from(RADIUS),
        );
        cr.set_source_rgba(tint.0, tint.1, tint.2, tint.3);
        cr.fill_preserve().ok()?;
        cr.set_source_rgba(1.0, 1.0, 1.0, if dark { 0.16 } else { 0.35 });
        cr.set_line_width(1.0);
        cr.stroke().ok()?;
        draw_card(&cr, notification, icon.as_ref(), dark, &face)?;
    }
    text::picture(surface)
}

impl Nekoland {
    pub(crate) fn notification_icon(&mut self, notification: &Notification) -> Option<ImageSurface> {
        if let Some((width, height, rgba)) = &notification.image {
            // Its own picture, in RGBA: turned into the ARGB the icon helper takes.
            let argb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[3], pixel[0], pixel[1], pixel[2]])
                .collect();
            return text::pixmap_icon(*width, *height, &argb);
        }
        let tint = colors(self.menu_bar_dark()).0;
        let name = if notification.icon.is_empty() {
            self.apps.info(&notification.app_name).icon_name.clone()?
        } else {
            // A desktop entry's name stands for that application's icon.
            self.apps
                .info(&notification.icon)
                .icon_name
                .clone()
                .unwrap_or_else(|| notification.icon.clone())
        };
        text::named_icon(name.trim_start_matches("file://"), "", ICON, tint)
    }

    /// Takes a notification in, or takes one away.
    pub fn notice_event(&mut self, event: &Event) {
        match event {
            Event::Notify(notification) => {
                remember_notifier(&notification.app_name);
                if self.is_turned_off(&notification.app_name) {
                    return;
                }
                if !notification.transient {
                    self.keep_notice(notification.clone());
                }
                if self.is_quiet() {
                    return;
                }
                if self.lock.is_some() {
                    let said = match (notification.summary.as_str(), notification.body.as_str()) {
                        (summary, "") => summary.to_string(),
                        ("", body) => body.to_string(),
                        (summary, body) => format!("{summary}：{body}"),
                    };
                    crate::lock::tell_locker(&notification.app_name, &said);
                }
                let deadline = match notification.timeout {
                    Expiry::UntilDismissed => None,
                    Expiry::After(timeout) => Some(Instant::now() + timeout),
                    Expiry::AsSet => {
                        Some(Instant::now() + Duration::from_secs(self.config.notification_seconds))
                    }
                };
                match self
                    .notices
                    .banners
                    .iter_mut()
                    .find(|banner| banner.notification.id == notification.id)
                {
                    Some(banner) => {
                        banner.notification = notification.clone();
                        banner.deadline = deadline;
                        banner.drawn_dark = None;
                        if banner.paused.is_some() {
                            banner.paused = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
                        }
                    }
                    None => self.notices.banners.insert(
                        0,
                        Banner {
                            notification: notification.clone(),
                            deadline,
                            picture: None,
                            drawn_dark: None,
                            id: Id::new(),
                            came: None,
                            paused: None,
                            place: None,
                            held: None,
                        },
                    ),
                }
            }
            // Its program took it back: from the screen and from the Notification Center.
            Event::CloseNotification(id) => self.close_notice(*id, CLOSED),
            _ => {}
        }
    }

    /// Whether banners stay away now: Do Not Disturb, from the control center or by its
    /// schedule.
    fn is_quiet(&self) -> bool {
        let config = &self.config;
        self.notices.quiet
            || (config.quiet_scheduled
                && crate::night::in_window(crate::night::minute_of_day(), config.quiet_from, config.quiet_to))
    }

    /// Whether Settings turned `app`'s notifications off.
    fn is_turned_off(&self, app: &str) -> bool {
        self.config.notifications_off.iter().any(|off| off == app)
    }

    fn keep_notice(&mut self, notification: Notification) {
        let pushed_out = keep(&mut self.notices.kept, notification, Instant::now());
        self.notices.kept_revision += 1;
        // One still on screen is closed when its banner goes.
        for id in pushed_out {
            if !self.has_banner(id) {
                self.tell_closed(id, EXPIRED);
            }
        }
    }

    fn has_banner(&self, id: u32) -> bool {
        self.notices.banners.iter().any(|banner| banner.notification.id == id)
    }

    fn is_kept(&self, id: u32) -> bool {
        self.notices.kept.iter().any(|kept| kept.notification.id == id)
    }

    /// Tells the program that its notification `id` is gone, and why.
    fn tell_closed(&self, id: u32, reason: u32) {
        if let Some(bus) = &self.menu_bar.bus {
            bus.send(Request::NotificationClosed { id, reason });
        }
    }

    fn drop_banner(&mut self, id: u32) -> bool {
        let Some(index) = self.notices.banners.iter().position(|banner| banner.notification.id == id) else {
            return false;
        };
        let rect = self.notice_rects().into_iter().find(|(shown, _)| *shown == index).map(|(_, rect)| rect);
        let banner = self.notices.banners.remove(index);
        if let (Some(rect), Some(picture), true) = (rect, banner.picture, self.config.animations) {
            self.notices.leaving.push(Leaving {
                picture,
                id: banner.id,
                rect,
                left: Instant::now(),
                held: banner.held,
            });
        }
        true
    }

    /// A banner's time is up. Its notification is closed only if it is not kept, since the
    /// Notification Center can still act on it.
    fn expire_banner(&mut self, id: u32) {
        if self.drop_banner(id) && !self.is_kept(id) {
            self.tell_closed(id, EXPIRED);
        }
    }

    /// Takes a notification away, its banner and from the Notification Center.
    pub(crate) fn close_notice(&mut self, id: u32, reason: u32) {
        let had_banner = self.drop_banner(id);
        let was_kept = forget(&mut self.notices.kept, id);
        if was_kept {
            self.notices.kept_revision += 1;
        }
        if had_banner || was_kept {
            self.tell_closed(id, reason);
        }
    }

    /// Does what clicking a notification does, its banner or its row in the Notification
    /// Center: its program's default action, if it has one, and it goes.
    pub(crate) fn activate_notice(&mut self, id: u32) {
        let has_default = self
            .notices
            .banners
            .iter()
            .map(|banner| &banner.notification)
            .chain(self.notices.kept.iter().map(|kept| &kept.notification))
            .find(|notification| notification.id == id)
            .is_some_and(|notification| notification.actions.chunks(2).any(|pair| pair[0] == "default"));
        if has_default {
            if let Some(bus) = &self.menu_bar.bus {
                bus.send(Request::NotificationAction {
                    id,
                    key: "default".into(),
                });
            }
        }
        self.close_notice(id, DISMISSED);
    }

    /// Draws the icons of the kept notifications that have none yet for `dark` glass.
    pub(crate) fn refresh_kept_icons(&mut self, dark: bool) {
        for index in 0..self.notices.kept.len() {
            if self.notices.kept[index].icon.as_ref().is_some_and(|(drawn_dark, _)| *drawn_dark == dark) {
                continue;
            }
            let notification = self.notices.kept[index].notification.clone();
            let icon = self.notification_icon(&notification);
            self.notices.kept[index].icon = Some((dark, icon));
        }
    }

    /// Lets banners whose time is up go, and draws the ones that are new or changed.
    pub fn refresh_notices(&mut self) {
        let now = Instant::now();
        let hovered = self.notices.hovered;
        let expired: Vec<u32> = self
            .notices
            .banners
            .iter()
            .filter(|banner| Some(banner.notification.id) != hovered)
            .filter(|banner| banner.deadline.is_some_and(|deadline| deadline <= now))
            .map(|banner| banner.notification.id)
            .collect();
        for id in expired {
            self.expire_banner(id);
        }

        let dark = self.menu_bar_dark();
        let stale: Vec<usize> = self
            .notices
            .banners
            .iter()
            .enumerate()
            .take(MAX_SHOWN)
            .filter(|(_, banner)| banner.drawn_dark != Some(dark) || banner.picture.is_none())
            .map(|(index, _)| index)
            .collect();
        let previews = self.config.notification_previews;
        for index in stale {
            let notification = self.notices.banners[index].notification.clone();
            let icon = self.notification_icon(&notification);
            let banner = &mut self.notices.banners[index];
            banner.picture = draw_banner(&notification, icon, dark, previews);
            banner.drawn_dark = Some(dark);
        }

        // Each shown banner glides from where it is to where it now stands, as one above it
        // comes or goes.
        let now = Instant::now();
        let mut top = 0;
        for banner in self.notices.banners.iter_mut().take(MAX_SHOWN) {
            let Some(picture) = &banner.picture else {
                continue;
            };
            banner.came.get_or_insert(now);
            match banner.place {
                Some((place, _, _)) if place == top => {}
                Some(_) => banner.place = Some((top, shown_top(banner.place), now)),
                None => banner.place = Some((top, f64::from(top), now)),
            }
            top += picture.size.h + GAP;
        }
        self.notices.leaving.retain(|leaving| crate::motion::notice_out(leaving.left).is_some());
    }

    /// Where each shown banner is, newest at the top.
    fn notice_rects(&self) -> Vec<(usize, Rectangle<i32, Logical>)> {
        let Some(area) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return Vec::new();
        };
        let top = area.loc.y + MENU_BAR_HEIGHT + MARGIN;
        let right = area.loc.x + area.size.w - WIDTH - MARGIN;
        let animated = self.config.animations;
        let mut y = top;
        let mut rects = Vec::new();
        for (index, banner) in self.notices.banners.iter().enumerate().take(MAX_SHOWN) {
            let Some(picture) = &banner.picture else {
                continue;
            };
            let (x, y_shown) = match (animated, banner.came) {
                // Out past the side of the screen, by all of its width and the margin.
                (true, Some(came)) => {
                    let (come, _) = crate::motion::notice_in(came);
                    let away = (1.0 - come) * f64::from(WIDTH + MARGIN);
                    (right + away.round() as i32, top + shown_top(banner.place).round() as i32)
                }
                _ => (right, y),
            };
            rects.push((index, Rectangle::new((x, y_shown).into(), picture.size)));
            y += picture.size.h + GAP;
        }
        rects
    }

    /// What the banners put on screen.
    pub fn notice_overlays(&self) -> Vec<Overlay<'_>> {
        if !self.banners_shown() {
            return Vec::new();
        }
        let animated = self.config.animations;
        let leaving = self.notices.leaving.iter().filter_map(|leaving| {
            let out = crate::motion::notice_out(leaving.left)?;
            let away = (out * f64::from(WIDTH + MARGIN)).round() as i32;
            Some(Overlay {
                fade: (1.0 - out) as f32,
                id: &leaving.id,
                rect: Rectangle::new(leaving.rect.loc + Point::from((away, 0)), leaving.rect.size),
                glass: Some((Tier::Bar, RADIUS)),
                alpha: 1.0,
                picture: &leaving.picture,
            })
        });
        self.notice_rects()
            .into_iter()
            .filter_map(|(index, rect)| {
                let banner = &self.notices.banners[index];
                let fade = match (animated, banner.came) {
                    (true, Some(came)) => crate::motion::notice_in(came).1 as f32,
                    _ => 1.0,
                };
                Some(Overlay {
                    fade,
                    id: &banner.id,
                    rect,
                    glass: Some((Tier::Bar, RADIUS)),
                    alpha: 1.0,
                    picture: banner.picture.as_ref()?,
                })
            })
            .chain(leaving)
            .collect()
    }

    /// Whether banners are there to see: not over a fullscreen window, nor while the
    /// Notification Center, where they stand, is open.
    fn banners_shown(&self) -> bool {
        self.fullscreen_window().is_none() && self.menu_bar.notification_center.is_none()
    }

    fn notice_at(&self, pos: Point<f64, Logical>) -> Option<u32> {
        if !self.banners_shown() {
            return None;
        }
        self.notice_rects()
            .into_iter()
            .find(|(_, rect)| rect.to_f64().contains(pos))
            .map(|(index, _)| self.notices.banners[index].notification.id)
    }

    pub fn notice_covers(&self, pos: Point<f64, Logical>) -> bool {
        self.notice_at(pos).is_some()
    }

    /// The pointer resting on a banner keeps it: its time stops, and goes on from where it
    /// stopped once the pointer leaves.
    pub fn notice_motion(&mut self, pos: Point<f64, Logical>) {
        let hovered = self.notice_at(pos);
        if hovered == self.notices.hovered {
            return;
        }
        let now = Instant::now();
        for banner in &mut self.notices.banners {
            let id = Some(banner.notification.id);
            if id == self.notices.hovered {
                if let Some(left) = banner.paused.take() {
                    banner.deadline = Some(now + left);
                }
            } else if id == hovered {
                banner.paused = banner.deadline.map(|deadline| deadline.saturating_duration_since(now));
            }
        }
        self.notices.hovered = hovered;
    }

    /// A banner pressed sinks under the pointer until it is let go.
    pub fn notice_press(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(id) = self.notice_at(pos) else {
            return false;
        };
        if let Some(banner) = self.notices.banners.iter_mut().find(|banner| banner.notification.id == id) {
            banner.held = Some((Instant::now(), None));
        }
        true
    }

    /// A banner let go comes back up; let go over it, its program's default action is taken
    /// if it has one, and it goes.
    pub fn notice_release(&mut self, pos: Point<f64, Logical>) {
        let now = Instant::now();
        let mut held = self.notices.banners.iter_mut().filter_map(|banner| match banner.held {
            Some((pressed, None)) => Some((banner, pressed)),
            _ => None,
        });
        let Some((banner, pressed)) = held.next() else {
            return;
        };
        banner.held = Some((pressed, Some((now, crate::motion::notice_sunk(pressed)))));
        let id = banner.notification.id;
        if self.notice_at(pos) == Some(id) {
            self.notices.hovered = None;
            self.activate_notice(id);
        }
    }

    /// How large a banner held down or just let go is drawn, around its middle.
    pub fn notice_zoom_of(&self, id: &Id) -> Option<f64> {
        if !self.config.animations {
            return None;
        }
        let banner = self.notices.banners.iter().find(|banner| &banner.id == id).map(|banner| banner.held);
        let leaving = || self.notices.leaving.iter().find(|leaving| &leaving.id == id).map(|leaving| leaving.held);
        let (pressed, let_go) = banner.or_else(leaving)??;
        crate::motion::notice_held_zoom(pressed, let_go)
    }
}

/// Where a banner stands from the top of the banners now, gliding from where it was to its
/// place.
fn shown_top(place: Option<(i32, f64, Instant)>) -> f64 {
    let Some((place, from, moved)) = place else {
        return 0.0;
    };
    from + (f64::from(place) - from) * crate::motion::notice_moved(moved)
}

/// Where the names programs send notifications under are kept, one a line, for Settings to
/// list them to turn off.
fn notifiers_path() -> Option<std::path::PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(".local/state")))?;
    Some(state.join("nekoland/notifiers"))
}

fn remember_notifier(app: &str) {
    let Some(path) = notifiers_path() else { return };
    if app.is_empty() || app.contains('\n') {
        return;
    }
    let known = std::fs::read_to_string(&path).unwrap_or_default();
    if known.lines().any(|line| line == app) {
        return;
    }
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{app}")
    });
    if let Err(err) = written {
        tracing::warn!("{}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: u32, app: &str) -> Notification {
        Notification {
            id,
            app_name: app.into(),
            icon: String::new(),
            image: None,
            summary: format!("notification {id}"),
            body: String::new(),
            actions: Vec::new(),
            timeout: Expiry::AsSet,
            transient: false,
        }
    }

    fn ids(kept: &[Kept]) -> Vec<u32> {
        kept.iter().map(|kept| kept.notification.id).collect()
    }

    #[test]
    fn the_newest_notification_is_kept_first() {
        let mut kept = Vec::new();
        for id in 1..=3 {
            assert!(keep(&mut kept, notification(id, "Mail"), Instant::now()).is_empty());
        }
        assert_eq!(ids(&kept), [3, 2, 1]);
    }

    #[test]
    fn a_replaced_notification_takes_the_old_ones_place_at_the_top() {
        let mut kept = Vec::new();
        for id in 1..=3 {
            keep(&mut kept, notification(id, "Mail"), Instant::now());
        }
        let mut replacement = notification(1, "Mail");
        replacement.summary = "replaced".into();
        keep(&mut kept, replacement, Instant::now());
        assert_eq!(ids(&kept), [1, 3, 2]);
        assert_eq!(kept[0].notification.summary, "replaced");
    }

    #[test]
    fn past_the_cap_the_oldest_go_and_are_named() {
        let mut kept = Vec::new();
        for id in 0..KEPT_MAX as u32 {
            assert!(keep(&mut kept, notification(id, "Mail"), Instant::now()).is_empty());
        }
        let pushed_out = keep(&mut kept, notification(KEPT_MAX as u32, "Mail"), Instant::now());
        assert_eq!(pushed_out, [0]);
        assert_eq!(kept.len(), KEPT_MAX);
        assert_eq!(kept[0].notification.id, KEPT_MAX as u32);
    }

    #[test]
    fn forgetting_takes_out_that_notification_alone() {
        let mut kept = Vec::new();
        for id in 1..=3 {
            keep(&mut kept, notification(id, "Mail"), Instant::now());
        }
        assert!(forget(&mut kept, 2));
        assert_eq!(ids(&kept), [3, 1]);
        assert!(!forget(&mut kept, 2));
        assert_eq!(ids(&kept), [3, 1]);
    }

    #[test]
    fn a_banner_glides_from_where_it_was_to_its_place() {
        assert_eq!(shown_top(None), 0.0);
        let long_ago = Instant::now() - Duration::from_secs(5);
        assert!((shown_top(Some((120, 0.0, long_ago))) - 120.0).abs() < 0.5);
        let just_now = Instant::now() + Duration::from_secs(5);
        assert_eq!(shown_top(Some((120, 40.0, just_now))), 40.0);
    }

    #[test]
    fn the_time_reads_now_then_minutes_hours_and_days_ago() {
        let after = |seconds| relative_time(Duration::from_secs(seconds));
        assert_eq!(after(0), "現在");
        assert_eq!(after(59), "現在");
        assert_eq!(after(60), "1 分鐘前");
        assert_eq!(after(59 * 60 + 59), "59 分鐘前");
        assert_eq!(after(60 * 60), "1 小時前");
        assert_eq!(after(23 * 60 * 60), "23 小時前");
        assert_eq!(after(2 * 24 * 60 * 60), "2 天前");
    }
}
