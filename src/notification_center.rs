//! The Notification Center, as the Mac's: the notifications kept after their banners, on a
//! panel of the menus' glass under the clock, where the banners stand, opened and closed by
//! a click on the clock. Each program's are stacked under its newest, saying how many, and
//! spread out by a click on the stack; a click on one does what its banner would. The × on
//! a notification or a stack under the pointer, 清除 over a program's spread out and
//! 清除全部 at the top clear them.

use std::{f64::consts::TAU, time::Instant};

use cairo::Context;
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    control::{draw_panel, draw_well, ease_out, show_line, to_f64, OPEN_TIME},
    glass::Tier,
    menubar::{
        colors, font, show_text, text_size, Colors, Overlay, GAP_BELOW_BAR, MENU_BAR_HEIGHT, MENU_RADIUS,
    },
    notices::{self, card_height, draw_card, relative_time, Face, Kept, DISMISSED},
    text::{self, Picture},
    Nekoland,
};

/// The panel is as wide as a banner, and stands where the banners do.
const WIDTH: i32 = notices::WIDTH;
const PADDING: i32 = 12;
const CARD_WIDTH: i32 = WIDTH - PADDING * 2;
/// Between one program's notifications and the next program's.
const GAP: i32 = 8;
/// Between the notifications of a program spread out.
const CARD_GAP: i32 = 6;
/// The row of 清除全部 at the top, and the row with the program's name over its
/// notifications spread out.
const HEADER_HEIGHT: i32 = 26;
/// How tall the panel's middle is with nothing in it.
const EMPTY_HEIGHT: i32 = 64;
/// How much of each notification stacked under a program's newest shows below it, how much
/// narrower than the one over it it is on either side, and how many of them show.
const STACK_PEEK: i32 = 6;
const STACK_INSET: i32 = 10;
const STACK_SHOWN: usize = 2;
/// The × on a notification or a stack under the pointer: its radius, how far its centre is
/// in from the card's corner, and how far the arms of its cross reach from that centre.
const CLEAR_RADIUS: i32 = 9;
const CLEAR_INSET: i32 = 4;
const CROSS: f64 = 3.5;
/// A text button: how tall it is, the room around its label, its corners when lit, and its
/// label's size.
const BUTTON_HEIGHT: i32 = 20;
const BUTTON_PADDING: i32 = 6;
const BUTTON_RADIUS: f64 = 6.0;
const BUTTON_POINTS: f64 = 9.0;
/// The program's name over its notifications spread out, and what the panel says when empty.
const NAME_POINTS: f64 = 10.0;
const EMPTY_POINTS: f64 = 10.0;

/// The grid the bar's bell is drawn on, as the control center's icon is, and how thick
/// its line is on it.
const BELL_GRID: f64 = 16.0;
const BELL_LINE: f64 = 1.5;

const CLEAR_ALL: &str = "清除全部";
const CLEAR_APP: &str = "清除";
const SHOW_LESS: &str = "顯示較少";
const EMPTY: &str = "沒有通知";

#[derive(Clone, Debug, PartialEq)]
enum Part {
    ClearAll,
    /// A notification, by its ID: what its banner would do.
    Notice(u32),
    /// The × on a notification.
    Clear(u32),
    /// A program's notifications stacked under its newest, by the program's name: spreads
    /// them out.
    Stack(String),
    /// The × on a stack, or 清除 over a program's spread out: clears that program's.
    ClearApp(String),
    /// 顯示較少 over a program's spread out: stacks them again.
    Collapse(String),
}

/// A card in the panel: the kept notification it shows, by its place among them, how many
/// of its program's are stacked under it, the line over its title, and what its × and a
/// click on it do.
struct Card {
    index: usize,
    rect: Rectangle<i32, Logical>,
    stacked: usize,
    label: Option<String>,
    clear: Part,
    press: Part,
}

/// Where everything is, from the top of what the panel holds, scrolled or not.
#[derive(Default)]
struct Layout {
    height: i32,
    cards: Vec<Card>,
    /// The programs' names over their notifications spread out.
    headers: Vec<(String, Rectangle<i32, Logical>)>,
    buttons: Vec<(Part, &'static str, Rectangle<i32, Logical>)>,
    /// The cards' ×s and the cards, each × before its card, which it overlaps.
    parts: Vec<(Part, Rectangle<i32, Logical>)>,
}

/// What decides where everything is: what is kept, which programs' are spread out, and
/// whether notifications' text is shown.
#[derive(Clone, PartialEq)]
struct LayoutKey {
    revision: u64,
    expanded: Vec<String>,
    previews: bool,
}

/// What the panel shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
struct Shown {
    dark: bool,
    layout: LayoutKey,
    scroll: i32,
    hovered: Option<Part>,
    /// When each kept notification came, as it reads now.
    times: Vec<String>,
    /// How tall the panel can be on the screen.
    room: i32,
}

pub struct NotificationCenter {
    opened: Instant,
    /// The programs whose notifications are spread out.
    expanded: Vec<String>,
    /// How far down what it holds is scrolled, in points.
    scroll: i32,
    hovered: Option<Part>,
    pressed: Option<Part>,
    layout: Layout,
    laid_out: Option<LayoutKey>,
    shown: Option<Shown>,
    picture: Option<Picture>,
    id: Id,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// The kept notifications by program, as places among them: each program's together,
/// newest first, the programs in the order of their newest.
fn group_by_program(apps: &[&str]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, app) in apps.iter().enumerate() {
        match groups.iter_mut().find(|group| apps[group[0]] == *app) {
            Some(group) => group.push(index),
            None => groups.push(vec![index]),
        }
    }
    groups
}

/// The line over a card's title: the program's name, and how many there are when its
/// card stands for a stack of them.
fn card_label(app: &str, count: usize) -> Option<String> {
    match (app.is_empty(), count) {
        (true, 1) => None,
        (true, _) => Some(format!("{count} 則通知")),
        (false, 1) => Some(app.to_string()),
        (false, _) => Some(format!("{app} · {count} 則通知")),
    }
}

fn card_face<'a>(label: Option<&'a str>, time: &'a str, previews: bool) -> Face<'a> {
    Face {
        app: label,
        time,
        width: CARD_WIDTH,
        body: previews,
    }
}

/// Where the panel stands on the output: at its right, under the bar.
fn panel_origin(output_width: i32) -> Point<i32, Logical> {
    (output_width - WIDTH - notices::MARGIN, MENU_BAR_HEIGHT + GAP_BELOW_BAR).into()
}

/// A text button in a row at `y`, its right edge at `right`.
fn button_rect(label: &str, right: i32, y: i32) -> Rectangle<i32, Logical> {
    let width = text_size(label, &font("", BUTTON_POINTS)).0 + BUTTON_PADDING * 2;
    rect(right - width, y + (HEADER_HEIGHT - BUTTON_HEIGHT) / 2, width, BUTTON_HEIGHT)
}

/// Where a card's × is, around its corner.
fn clear_rect(card: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    rect(
        card.loc.x + CLEAR_INSET - CLEAR_RADIUS,
        card.loc.y + CLEAR_INSET - CLEAR_RADIUS,
        CLEAR_RADIUS * 2,
        CLEAR_RADIUS * 2,
    )
}

impl Layout {
    fn new(kept: &[Kept], expanded: &[String], previews: bool) -> Self {
        let mut layout = Layout::default();
        if kept.is_empty() {
            layout.height = PADDING * 2 + EMPTY_HEIGHT;
            return layout;
        }
        let mut y = PADDING;
        let right = WIDTH - PADDING;
        layout.buttons.push((Part::ClearAll, CLEAR_ALL, button_rect(CLEAR_ALL, right, y)));
        y += HEADER_HEIGHT;

        let apps: Vec<&str> = kept.iter().map(|kept| kept.notification.app_name.as_str()).collect();
        for group in group_by_program(&apps) {
            let app = apps[group[0]].to_string();
            if group.len() > 1 && expanded.contains(&app) {
                let clear = button_rect(CLEAR_APP, right, y);
                let less = button_rect(SHOW_LESS, clear.loc.x, y);
                layout.headers.push((app.clone(), rect(PADDING, y, less.loc.x - PADDING, HEADER_HEIGHT)));
                layout.buttons.push((Part::ClearApp(app.clone()), CLEAR_APP, clear));
                layout.buttons.push((Part::Collapse(app.clone()), SHOW_LESS, less));
                y += HEADER_HEIGHT;
                for &index in &group {
                    let id = kept[index].notification.id;
                    let parts = (Part::Clear(id), Part::Notice(id));
                    // The program's name is over them already.
                    y += layout.add_card(kept, index, y, (0, None), parts, previews) + CARD_GAP;
                }
                y += GAP - CARD_GAP;
            } else {
                let newest = group[0];
                let parts = if group.len() > 1 {
                    (Part::ClearApp(app.clone()), Part::Stack(app.clone()))
                } else {
                    let id = kept[newest].notification.id;
                    (Part::Clear(id), Part::Notice(id))
                };
                let stack = ((group.len() - 1).min(STACK_SHOWN), card_label(&app, group.len()));
                y += layout.add_card(kept, newest, y, stack, parts, previews) + GAP;
            }
        }
        layout.height = y - GAP + PADDING;
        layout
    }

    /// Puts a card at `y` for the kept notification at `index`, with how many are stacked
    /// under it and the line over its title; `clear` is what its × does and `press` what a
    /// click on it does. Returns how tall it is with those stacked under it.
    fn add_card(
        &mut self,
        kept: &[Kept],
        index: usize,
        y: i32,
        (stacked, label): (usize, Option<String>),
        (clear, press): (Part, Part),
        previews: bool,
    ) -> i32 {
        let notification = &kept[index].notification;
        let Some(height) = card_height(notification, &card_face(label.as_deref(), "", previews)) else {
            tracing::warn!("notification center: could not lay out notification {}", notification.id);
            return 0;
        };
        let card = rect(PADDING, y, CARD_WIDTH, height);
        let total = height + STACK_PEEK * stacked as i32;
        self.parts.push((clear.clone(), clear_rect(card)));
        self.parts.push((press.clone(), rect(PADDING, y, CARD_WIDTH, total)));
        self.cards.push(Card {
            index,
            rect: card,
            stacked,
            label,
            clear,
            press,
        });
        total
    }

    fn part_at(&self, at: Point<i32, Logical>) -> Option<Part> {
        let button = self.buttons.iter().map(|(part, _, area)| (part, area));
        let card = self.parts.iter().map(|(part, area)| (part, area));
        button
            .chain(card)
            .find(|(_, area)| area.contains(at))
            .map(|(part, _)| part.clone())
    }
}

impl NotificationCenter {
    fn new() -> Self {
        Self {
            opened: Instant::now(),
            expanded: Vec::new(),
            scroll: 0,
            hovered: None,
            pressed: None,
            layout: Layout::default(),
            laid_out: None,
            shown: None,
            picture: None,
            id: Id::new(),
        }
    }

    fn draw(&mut self, kept: &[Kept], shown: &Shown) {
        let colors = colors(shown.dark);
        let height = self.layout.height.min(shown.room);
        let Some((surface, cr)) = draw_panel(WIDTH, height, shown.dark, &colors) else {
            return;
        };
        // What is scrolled past is cut off at the panel's edge.
        text::rounded_rectangle(&cr, 0.0, 0.0, f64::from(WIDTH), f64::from(height), f64::from(MENU_RADIUS));
        cr.clip();
        cr.translate(0.0, -f64::from(shown.scroll));

        if kept.is_empty() {
            draw_empty(&cr, &colors);
        }
        let name_font = font("Bold", NAME_POINTS);
        for (name, area) in &self.layout.headers {
            let (x, y, w, h) = to_f64(*area);
            show_line(&cr, name, &name_font, (x, y + h / 2.0, w), colors.ink);
        }
        let button_font = font("", BUTTON_POINTS);
        for (part, label, area) in &self.layout.buttons {
            let lit = shown.hovered.as_ref() == Some(part);
            if lit {
                draw_well(&cr, &colors, *area, BUTTON_RADIUS);
            }
            let (x, y, _, h) = to_f64(*area);
            let color = if lit { colors.ink } else { colors.ink_3 };
            show_text(&cr, label, &button_font, x + f64::from(BUTTON_PADDING), y + h / 2.0, color);
        }
        for card in &self.layout.cards {
            draw_stacked(&cr, &colors, card);
            draw_well(&cr, &colors, card.rect, f64::from(MENU_RADIUS));
            let kept = &kept[card.index];
            let icon = kept.icon.as_ref().and_then(|(_, icon)| icon.as_ref());
            let time = shown.times.get(card.index).map_or("", String::as_str);
            let face = card_face(card.label.as_deref(), time, shown.layout.previews);
            cr.save().ok();
            cr.translate(f64::from(card.rect.loc.x), f64::from(card.rect.loc.y));
            if draw_card(&cr, &kept.notification, icon, shown.dark, &face).is_none() {
                let id = kept.notification.id;
                tracing::warn!("notification center: could not draw notification {id}");
            }
            cr.restore().ok();
            let hovered = shown.hovered.as_ref();
            if hovered == Some(&card.clear) || hovered == Some(&card.press) {
                draw_clear_button(&cr, &colors, card.rect);
            }
        }
        drop(cr);
        self.picture = text::picture(surface);
    }
}

fn draw_empty(cr: &Context, colors: &Colors) {
    let empty_font = font("", EMPTY_POINTS);
    let width = text_size(EMPTY, &empty_font).0;
    let x = f64::from((WIDTH - width) / 2);
    let center = f64::from(PADDING + EMPTY_HEIGHT / 2);
    show_text(cr, EMPTY, &empty_font, x, center, colors.ink_3);
}

/// The edges of the notifications stacked under a card, each a little narrower than the one
/// over it, showing below it.
fn draw_stacked(cr: &Context, colors: &Colors, card: &Card) {
    let bottom = card.rect.loc.y + card.rect.size.h;
    for step in 1..=card.stacked as i32 {
        let inset = STACK_INSET * step;
        let under = rect(
            card.rect.loc.x + inset,
            card.rect.loc.y + STACK_PEEK * step,
            card.rect.size.w - inset * 2,
            card.rect.size.h,
        );
        // The wells are see-through: only what shows below the one over it is drawn.
        cr.save().ok();
        let strip_top = bottom + STACK_PEEK * (step - 1);
        cr.rectangle(0.0, f64::from(strip_top), f64::from(WIDTH), f64::from(STACK_PEEK));
        cr.clip();
        draw_well(cr, colors, under, f64::from(MENU_RADIUS));
        cr.restore().ok();
    }
}

/// The Notification Center's icon in the bar: a bell, on the control center's own grid
/// and of its line.
pub(crate) fn draw_bar_icon(cr: &Context, x: f64, y: f64, size: f64, color: (f64, f64, f64)) {
    let unit = size / BELL_GRID;
    cr.save().ok();
    cr.translate(x, y);
    cr.scale(unit, unit);
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(BELL_LINE);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_cap(cairo::LineCap::Round);
    // The body: a dome over sides that flare out to the rim.
    cr.new_sub_path();
    cr.move_to(2.5, 11.25);
    cr.curve_to(4.0, 10.0, 4.0, 8.5, 4.0, 6.5);
    cr.curve_to(4.0, 4.0, 5.75, 2.25, 8.0, 2.25);
    cr.curve_to(10.25, 2.25, 12.0, 4.0, 12.0, 6.5);
    cr.curve_to(12.0, 8.5, 12.0, 10.0, 13.5, 11.25);
    cr.close_path();
    let _ = cr.stroke();
    // The clapper, showing under the rim.
    cr.new_sub_path();
    cr.arc(8.0, 12.25, 1.5, 0.0, std::f64::consts::PI);
    let _ = cr.stroke();
    cr.restore().ok();
}

/// The × at a card's corner, as the Mac shows on a notification under the pointer.
fn draw_clear_button(cr: &Context, colors: &Colors, card: Rectangle<i32, Logical>) {
    let x = f64::from(card.loc.x + CLEAR_INSET);
    let y = f64::from(card.loc.y + CLEAR_INSET);
    // On its own, not joined by a line to where what was drawn before it ended.
    cr.new_sub_path();
    cr.arc(x, y, f64::from(CLEAR_RADIUS), 0.0, TAU);
    cr.set_source_rgb(colors.sheet.0, colors.sheet.1, colors.sheet.2);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(colors.edge.0, colors.edge.1, colors.edge.2, colors.edge.3);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    cr.move_to(x - CROSS, y - CROSS);
    cr.line_to(x + CROSS, y + CROSS);
    cr.move_to(x + CROSS, y - CROSS);
    cr.line_to(x - CROSS, y + CROSS);
    cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}

impl Nekoland {
    /// Opens the Notification Center under the clock, or closes it.
    pub(crate) fn toggle_notification_center(&mut self) {
        self.menu_bar.open = None;
        self.menu_bar.control = None;
        self.menu_bar.notification_center = match self.menu_bar.notification_center {
            Some(_) => None,
            None => Some(NotificationCenter::new()),
        };
    }

    /// Draws the panel again when anything it shows changed.
    pub(crate) fn refresh_notification_center(&mut self) {
        if self.menu_bar.notification_center.is_none() {
            return;
        }
        let dark = self.menu_bar.dark;
        self.refresh_kept_icons(dark);
        let room = self.output_size().h - (MENU_BAR_HEIGHT + GAP_BELOW_BAR) - notices::MARGIN;
        let previews = self.config.notification_previews;
        let revision = self.notices.kept_revision;
        let kept = &self.notices.kept;
        let Some(center) = self.menu_bar.notification_center.as_mut() else {
            return;
        };
        let layout_key = LayoutKey {
            revision,
            expanded: center.expanded.clone(),
            previews,
        };
        if center.laid_out.as_ref() != Some(&layout_key) {
            center.layout = Layout::new(kept, &center.expanded, previews);
            center.laid_out = Some(layout_key.clone());
        }
        center.scroll = center.scroll.clamp(0, (center.layout.height - room).max(0));
        let shown = Shown {
            dark,
            layout: layout_key,
            scroll: center.scroll,
            hovered: center.hovered.clone(),
            times: kept.iter().map(|kept| relative_time(kept.came.elapsed())).collect(),
            room,
        };
        if center.shown.as_ref() != Some(&shown) || center.picture.is_none() {
            center.draw(kept, &shown);
            center.shown = Some(shown);
        }
    }

    pub(crate) fn notification_center_overlay(&self) -> Option<Overlay<'_>> {
        let output = self.output_area()?;
        let center = self.menu_bar.notification_center.as_ref()?;
        let picture = center.picture.as_ref()?;
        let fade = if self.config.animations {
            ease_out(center.opened.elapsed().as_secs_f64() / OPEN_TIME)
        } else {
            1.0
        };
        Some(Overlay {
            id: &center.id,
            rect: Rectangle::new(output.loc + panel_origin(output.size.w), picture.size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: fade as f32,
            picture,
        })
    }

    /// Where a point of the output is in what the panel holds, scrolled as it is, if it is
    /// on the panel.
    fn notification_center_at(&self, local: Point<i32, Logical>) -> Option<Point<i32, Logical>> {
        let center = self.menu_bar.notification_center.as_ref()?;
        let size = center.picture.as_ref()?.size;
        let at = local - panel_origin(self.output_size().w);
        Rectangle::new((0, 0).into(), size)
            .contains(at)
            .then(|| at + Point::from((0, center.scroll)))
    }

    /// A press on the output, at `local`. Returns whether it was on the panel.
    pub(crate) fn notification_center_press(&mut self, local: Point<i32, Logical>) -> bool {
        let Some(at) = self.notification_center_at(local) else {
            return false;
        };
        if let Some(center) = self.menu_bar.notification_center.as_mut() {
            center.pressed = center.layout.part_at(at);
        }
        true
    }

    pub(crate) fn notification_center_motion(&mut self, local: Point<i32, Logical>) {
        let at = self.notification_center_at(local);
        if let Some(center) = self.menu_bar.notification_center.as_mut() {
            center.hovered = at.and_then(|at| center.layout.part_at(at));
        }
    }

    pub(crate) fn notification_center_release(&mut self, local: Point<i32, Logical>) {
        let at = self.notification_center_at(local);
        let Some(center) = self.menu_bar.notification_center.as_mut() else {
            return;
        };
        let under = at.and_then(|at| center.layout.part_at(at));
        if let Some(part) = center.pressed.take().filter(|part| Some(part) == under.as_ref()) {
            self.notification_center_activate(part);
        }
    }

    /// The wheel over the panel scrolls what it holds. Returns whether it was over it.
    pub fn notification_center_scroll(&mut self, pos: Point<f64, Logical>, amount: f64) -> bool {
        let Some(local) = self.local(pos) else {
            return false;
        };
        if self.notification_center_at(local).is_none() || !self.menu_bar_shown() {
            return false;
        }
        // It is kept within what there is to scroll when the panel is drawn next.
        if let Some(center) = self.menu_bar.notification_center.as_mut() {
            center.scroll += amount.round() as i32;
        }
        true
    }

    /// The IDs of the kept notifications, all of them or `app`'s.
    fn kept_ids(&self, app: Option<&str>) -> Vec<u32> {
        self.notices
            .kept
            .iter()
            .map(|kept| &kept.notification)
            .filter(|notification| app.is_none_or(|app| notification.app_name == app))
            .map(|notification| notification.id)
            .collect()
    }

    fn notification_center_activate(&mut self, part: Part) {
        match part {
            // As on the Mac, the panel goes as the program comes forward.
            Part::Notice(id) => {
                self.menu_bar.notification_center = None;
                self.activate_notice(id);
                return;
            }
            Part::Clear(id) => self.close_notice(id, DISMISSED),
            Part::ClearAll => {
                for id in self.kept_ids(None) {
                    self.close_notice(id, DISMISSED);
                }
            }
            Part::ClearApp(ref app) => {
                for id in self.kept_ids(Some(app)) {
                    self.close_notice(id, DISMISSED);
                }
            }
            Part::Stack(_) | Part::Collapse(_) => {}
        }
        let Some(center) = self.menu_bar.notification_center.as_mut() else {
            return;
        };
        match part {
            Part::Stack(app) => center.expanded.push(app),
            Part::Collapse(app) | Part::ClearApp(app) => center.expanded.retain(|open| *open != app),
            Part::Notice(_) | Part::Clear(_) | Part::ClearAll => {}
        }
        // What was under the pointer may have gone; the next move finds what is there now.
        center.hovered = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_programs_notifications_group_under_its_newest_in_the_order_they_came() {
        let apps = ["Mail", "Calendar", "Mail", "Files", "Calendar", "Mail"];
        assert_eq!(group_by_program(&apps), [vec![0, 2, 5], vec![1, 4], vec![3]]);
        assert!(group_by_program(&[]).is_empty());
    }

    #[test]
    fn a_stack_says_how_many_it_holds_and_a_single_notification_its_programs_name() {
        assert_eq!(card_label("Mail", 1).as_deref(), Some("Mail"));
        assert_eq!(card_label("Mail", 3).as_deref(), Some("Mail · 3 則通知"));
        assert_eq!(card_label("", 1), None);
        assert_eq!(card_label("", 2).as_deref(), Some("2 則通知"));
    }
}
