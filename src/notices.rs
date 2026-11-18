//! Notifications, shown by Nekoland itself as banners on glass at the top right, under the
//! menu bar, newest first. A banner goes away when its time is up, unless the pointer
//! rests on it; a click on it does what its program offers for that, or dismisses it.

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

const WIDTH: i32 = 360;
const PADDING: i32 = 12;
const ICON: i32 = 36;
const GAP: i32 = 10;
const RADIUS: i32 = 14;
const MARGIN: i32 = 12;
const MAX_SHOWN: usize = 4;
const BODY_LINES: i32 = 3;

// Why a notification went away, as the specification numbers it.
const EXPIRED: u32 = 1;
const DISMISSED: u32 = 2;
const CLOSED: u32 = 3;

struct Banner {
    notification: Notification,
    deadline: Option<Instant>,
    picture: Option<Picture>,
    drawn_dark: Option<bool>,
    id: Id,
}

#[derive(Default)]
pub struct Notices {
    /// Do Not Disturb, from the control center: no banners come up.
    pub(crate) quiet: bool,
    banners: Vec<Banner>,
    hovered: Option<u32>,
}

type Rgb = (f64, f64, f64);

fn colors(dark: bool) -> (Rgb, Rgb, (f64, f64, f64, f64)) {
    if dark {
        (
            (0.945, 0.937, 0.918),
            (0.663, 0.655, 0.671),
            crate::glass::tint(dark, Tier::Menu),
        )
    } else {
        (
            (0.090, 0.090, 0.102),
            (0.365, 0.361, 0.380),
            crate::glass::tint(dark, Tier::Menu),
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

fn draw_banner(notification: &Notification, icon: Option<ImageSurface>, dark: bool) -> Option<Picture> {
    const TITLE: &str = "Bold 10";
    const BODY: &str = "9.5";
    const TIME: &str = "8.5";
    let (ink, ink_2, tint) = colors(dark);
    let text_x = PADDING + ICON + 12;
    let text_width = WIDTH - text_x - PADDING;
    let title = if notification.summary.is_empty() {
        notification.app_name.clone()
    } else {
        notification.summary.clone()
    };

    let probe = Context::new(ImageSurface::create(Format::ARgb32, 1, 1).ok()?).ok()?;
    let title_height = layout(&probe, &title, TITLE, text_width - 40, 1).pixel_size().1;
    let body_height = if notification.body.is_empty() {
        0
    } else {
        layout(&probe, &notification.body, BODY, text_width, BODY_LINES)
            .pixel_size()
            .1
    };
    let text_height = if body_height == 0 { title_height } else { title_height + 2 + body_height };
    let height = (PADDING * 2 + text_height).max(PADDING * 2 + ICON);
    // Text shorter than the icon, a title alone, stands in the middle beside it.
    let top = (height - text_height) / 2;

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

        match icon {
            Some(icon) => {
                let (w, h) = text::points(&icon);
                let scale = f64::from(ICON) / w.max(h).max(1.0);
                cr.save().ok()?;
                cr.translate(f64::from(PADDING), f64::from(PADDING));
                cr.scale(scale, scale);
                cr.set_source_surface(&icon, 0.0, 0.0).ok()?;
                cr.paint().ok()?;
                cr.restore().ok()?;
            }
            None => {
                text::rounded_rectangle(
                    &cr,
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

        cr.set_source_rgb(ink.0, ink.1, ink.2);
        cr.move_to(f64::from(text_x), f64::from(top));
        pangocairo::functions::show_layout(&cr, &layout(&cr, &title, TITLE, text_width - 40, 1));

        let time = layout(&cr, "現在", TIME, 40, 1);
        let time_width = time.pixel_size().0;
        cr.set_source_rgb(ink_2.0, ink_2.1, ink_2.2);
        cr.move_to(f64::from(WIDTH - PADDING - time_width), f64::from(top + 1));
        pangocairo::functions::show_layout(&cr, &time);

        if !notification.body.is_empty() {
            cr.move_to(f64::from(text_x), f64::from(top + title_height + 2));
            pangocairo::functions::show_layout(
                &cr,
                &layout(&cr, &notification.body, BODY, text_width, BODY_LINES),
            );
        }
    }
    text::picture(surface)
}

impl Nekoland {
    fn notification_icon(&mut self, notification: &Notification) -> Option<ImageSurface> {
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
            // shortcut: what comes in while quiet is not kept; keep it once there is a list of
            // past notifications to show it in.
            Event::Notify(notification) if self.is_quiet_for(&notification.app_name) => {
                remember_notifier(&notification.app_name);
            }
            Event::Notify(notification) => {
                remember_notifier(&notification.app_name);
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
                    }
                    None => self.notices.banners.insert(
                        0,
                        Banner {
                            notification: notification.clone(),
                            deadline,
                            picture: None,
                            drawn_dark: None,
                            id: Id::new(),
                        },
                    ),
                }
            }
            Event::CloseNotification(id) => self.dismiss_notice(*id, CLOSED),
            _ => {}
        }
    }

    /// Whether `app`'s banners stay away now: Do Not Disturb, from the control center or by
    /// its schedule, or Settings turning that program's off.
    fn is_quiet_for(&self, app: &str) -> bool {
        let config = &self.config;
        self.notices.quiet
            || config.notifications_off.iter().any(|off| off == app)
            || (config.quiet_scheduled
                && crate::night::in_window(crate::night::minute_of_day(), config.quiet_from, config.quiet_to))
    }

    fn dismiss_notice(&mut self, id: u32, reason: u32) {
        let before = self.notices.banners.len();
        self.notices.banners.retain(|banner| banner.notification.id != id);
        if self.notices.banners.len() != before {
            if let Some(bus) = &self.menu_bar.bus {
                bus.send(Request::NotificationClosed { id, reason });
            }
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
            self.dismiss_notice(id, EXPIRED);
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
        for index in stale {
            let mut notification = self.notices.banners[index].notification.clone();
            if !self.config.notification_previews {
                notification.body.clear();
            }
            let icon = self.notification_icon(&notification);
            let banner = &mut self.notices.banners[index];
            banner.picture = draw_banner(&notification, icon, dark);
            banner.drawn_dark = Some(dark);
        }
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
        let mut y = area.loc.y + MENU_BAR_HEIGHT + MARGIN;
        let mut rects = Vec::new();
        for (index, banner) in self.notices.banners.iter().enumerate().take(MAX_SHOWN) {
            let Some(picture) = &banner.picture else {
                continue;
            };
            let rect = Rectangle::new(
                (area.loc.x + area.size.w - WIDTH - MARGIN, y).into(),
                picture.size,
            );
            y += picture.size.h + GAP;
            rects.push((index, rect));
        }
        rects
    }

    /// What the banners put on screen.
    pub fn notice_overlays(&self) -> Vec<Overlay<'_>> {
        if self.fullscreen_window().is_some() {
            return Vec::new();
        }
        self.notice_rects()
            .into_iter()
            .filter_map(|(index, rect)| {
                let banner = &self.notices.banners[index];
                Some(Overlay {
                    fade: 1.0,
                    id: &banner.id,
                    rect,
                    glass: Some((Tier::Menu, RADIUS)),
                    alpha: 1.0,
                    picture: banner.picture.as_ref()?,
                })
            })
            .collect()
    }

    fn notice_at(&self, pos: Point<f64, Logical>) -> Option<u32> {
        if self.fullscreen_window().is_some() {
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

    /// The pointer resting on a banner keeps it.
    pub fn notice_motion(&mut self, pos: Point<f64, Logical>) {
        self.notices.hovered = self.notice_at(pos);
    }

    /// A click on a banner: its program's default action if it has one, and it goes.
    pub fn notice_press(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(id) = self.notice_at(pos) else {
            return false;
        };
        let has_default = self
            .notices
            .banners
            .iter()
            .find(|banner| banner.notification.id == id)
            .is_some_and(|banner| {
                banner
                    .notification
                    .actions
                    .chunks(2)
                    .any(|pair| pair[0] == "default")
            });
        if has_default {
            if let Some(bus) = &self.menu_bar.bus {
                bus.send(Request::NotificationAction {
                    id,
                    key: "default".into(),
                });
            }
        }
        self.notices.hovered = None;
        self.dismiss_notice(id, DISMISSED);
        true
    }
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
