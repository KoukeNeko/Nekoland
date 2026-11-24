//! The switcher Alt+Tab and ⌘Tab bring up: the applications, or on Windows the windows,
//! from the one used last, held open while the modifier is, or until Return for Ctrl+Alt+Tab.
//! Each is the window small, with the application's icon over its bottom edge, as in the
//! overview.

use std::time::Instant;

use smithay::{
    backend::renderer::element::Id,
    desktop::Window,
    input::keyboard::Keysym,
    utils::{IsAlive, Logical, Point, Rectangle},
};

use crate::{
    control::{draw_panel, ease_out, show_line, OPEN_TIME},
    menubar::{colors, font, show_text, text_size, Overlay, MENU_RADIUS},
    glass::Tier,
    text::{self, Picture},
    windows::{is_managed, window_names},
    Nekoland,
};

const PADDING: i32 = 16;
const ICON: i32 = 56;
/// The room a window is fitted into.
const THUMB_W: i32 = 192;
const THUMB_H: i32 = 128;
/// Around the window in its cell, which is lit when it is the one chosen.
const INSET: i32 = 12;
const CELL_W: i32 = THUMB_W + 2 * INSET;
/// The icon hangs half below the window.
const CELL_H: i32 = THUMB_H + ICON / 2 + 2 * INSET;
const LABEL_HEIGHT: i32 = 28;
/// The room left at each side of the display for the switcher.
const MARGIN: i32 = 48;

struct Entry {
    window: Window,
    label: String,
    icon: Option<String>,
}

pub struct Switcher {
    entries: Vec<Entry>,
    selected: usize,
    /// Whether letting go of the modifiers chooses, rather than Return or a click.
    held: bool,
    id: Id,
    picture: Option<Picture>,
    /// What the picture was drawn for: the one chosen, and whether it was dark.
    drawn: Option<(usize, bool)>,
    /// Where it is on the output, and each entry's cell in it.
    rect: Rectangle<i32, Logical>,
    cells: Vec<Rectangle<i32, Logical>>,
    /// Each window shown, where it is drawn, and its icon and where that goes; on the output.
    thumbnails: Vec<(Window, Rectangle<i32, Logical>, Option<Picture>, Point<i32, Logical>)>,
    /// The entry the first cell shows, when there are more than fit.
    first: usize,
    opened: Instant,
}

impl Nekoland {
    /// Brings up the switcher, or goes on to the next entry in it; `back` goes the other way.
    /// `held` is the Alt+Tab one, which chooses as its keys are let go.
    pub fn switch(&mut self, back: bool, held: bool) {
        if let Some(switcher) = self.switcher.as_mut() {
            let count = switcher.entries.len();
            switcher.selected = if back { (switcher.selected + count - 1) % count } else { (switcher.selected + 1) % count };
            return;
        }
        let entries = self.switcher_entries();
        if entries.is_empty() {
            return;
        }
        let count = entries.len();
        let selected = match (back, count) {
            (_, 1) => 0,
            (true, _) => count - 1,
            (false, _) => 1,
        };
        self.switcher = Some(Switcher {
            entries,
            selected,
            held,
            id: Id::new(),
            picture: None,
            drawn: None,
            rect: Rectangle::default(),
            cells: Vec::new(),
            thumbnails: Vec::new(),
            first: 0,
            opened: Instant::now(),
        });
    }

    /// The windows from the one in front back, minimized ones last, one for each
    /// application, as macOS's and Linux desktops' switchers show applications.
    fn switcher_entries(&mut self) -> Vec<Entry> {
        let stacked = self.space.elements().rev().filter(|window| is_managed(window)).cloned();
        let minimized = self.minimized.iter().map(|(window, _)| window.clone());
        let windows: Vec<Window> = stacked.chain(minimized).filter(|window| window.alive()).collect();
        let mut entries: Vec<Entry> = Vec::new();
        for window in windows {
            let app_id = window_names(&window).1;
            if crate::windows::UNFOCUSED_APP_IDS.contains(&app_id.as_str()) {
                continue;
            }
            if entries.iter().any(|entry| window_names(&entry.window).1 == app_id) {
                continue;
            }
            let info = self.apps.info(&app_id);
            let name = info.name.clone().unwrap_or_else(|| app_id.rsplit('.').next().unwrap_or(&app_id).to_string());
            entries.push(Entry {
                label: name,
                icon: info.icon_name.clone(),
                window,
            });
        }
        entries
    }

    /// Brings forward what the switcher has chosen, and closes it.
    fn switch_to_selected(&mut self) {
        let Some(switcher) = self.switcher.take() else {
            return;
        };
        if let Some(entry) = switcher.entries.into_iter().nth(switcher.selected) {
            self.unminimize(&entry.window);
            self.focus_window(&entry.window);
        }
    }

    /// Takes a key for the switcher while it is up: the arrows and Tab move, Return chooses,
    /// Esc closes it. Returns whether it is up.
    pub fn switcher_key(&mut self, sym: Keysym, shift: bool) -> bool {
        let Some(switcher) = self.switcher.as_mut() else {
            return false;
        };
        let count = switcher.entries.len();
        match sym {
            Keysym::Escape => self.switcher = None,
            Keysym::Return | Keysym::KP_Enter | Keysym::space => self.switch_to_selected(),
            Keysym::Right | Keysym::Down => switcher.selected = (switcher.selected + 1) % count,
            Keysym::Left | Keysym::Up => switcher.selected = (switcher.selected + count - 1) % count,
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                let back = shift || sym == Keysym::ISO_Left_Tab;
                switcher.selected = if back { (switcher.selected + count - 1) % count } else { (switcher.selected + 1) % count };
            }
            // The modifiers themselves, and the keys of the shortcut that moves it on.
            _ => return false,
        }
        true
    }

    /// Chooses once the modifiers that hold the switcher open are all let go.
    pub fn switcher_modifiers(&mut self, ctrl: bool, alt: bool, logo: bool) {
        if self.switcher.as_ref().is_some_and(|switcher| switcher.held) && !(ctrl || alt || logo) {
            self.switch_to_selected();
        }
    }

    /// A click on an entry chooses it; anywhere else closes the switcher. Returns whether
    /// it was up.
    pub fn switcher_press(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(switcher) = self.switcher.as_mut() else {
            return false;
        };
        let at = pos.to_i32_round() - switcher.rect.loc;
        match switcher.cells.iter().position(|cell| cell.contains(at)) {
            Some(index) => {
                switcher.selected = switcher.first + index;
                self.switch_to_selected();
            }
            None if !switcher.rect.contains(pos.to_i32_round()) => self.switcher = None,
            None => {}
        }
        true
    }

    /// Draws the switcher again when what it shows changed.
    pub fn refresh_switcher(&mut self) {
        let dark = self.menu_bar.dark;
        let Some(output) = self.bar_area() else {
            return;
        };
        let Some(switcher) = self.switcher.as_ref() else {
            return;
        };
        if switcher.drawn == Some((switcher.selected, dark)) && switcher.picture.is_some() {
            return;
        }
        // As many cells as fit across the display, the chosen one among them.
        let fit = ((output.size.w - 2 * MARGIN - 2 * PADDING) / CELL_W).max(1) as usize;
        let count = switcher.entries.len();
        let first = switcher.selected.saturating_sub(fit - 1).min(count.saturating_sub(fit));
        let shown: Vec<(Window, Option<String>, String)> = switcher
            .entries
            .iter()
            .skip(first)
            .take(fit)
            .map(|entry| (entry.window.clone(), entry.icon.clone(), entry.label.clone()))
            .collect();
        let colors = colors(dark);
        let waits = text::icon_waits();
        let icons: Vec<Option<Picture>> = shown
            .iter()
            .map(|(_, icon, label)| {
                let icon = icon.as_deref().and_then(|icon| self.dock_icon(icon, ICON, dark));
                icon_picture(icon.as_ref(), label, &colors)
            })
            .collect();
        let Some(switcher) = self.switcher.as_mut() else {
            return;
        };
        let width = 2 * PADDING + CELL_W * shown.len() as i32;
        let height = 2 * PADDING + CELL_H + LABEL_HEIGHT;
        let rect = Rectangle::new(
            output.loc + Point::from(((output.size.w - width) / 2, (output.size.h - height) / 2)),
            (width, height).into(),
        );
        let Some((surface, cr)) = draw_panel(width, height, dark, &colors) else {
            return;
        };
        let chosen = switcher.selected - first;
        switcher.cells.clear();
        switcher.thumbnails.clear();
        for (index, ((window, _, _), icon)) in shown.iter().zip(icons).enumerate() {
            let cell = Rectangle::new(Point::from((PADDING + CELL_W * index as i32, PADDING)), (CELL_W, CELL_H).into());
            if index == chosen {
                // A tile behind the window, ringed in ink.
                let (x, y, w, h) = crate::control::to_f64(cell);
                text::rounded_rectangle(&cr, x + 1.0, y + 1.0, w - 2.0, h - 2.0, f64::from(MENU_RADIUS) - 1.0);
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.16);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.9);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
            }
            // The window fitted to its room, at the room's foot, so the icon meets its edge.
            let size = window.geometry().size;
            let factor = (f64::from(THUMB_W) / f64::from(size.w.max(1))).min(f64::from(THUMB_H) / f64::from(size.h.max(1)));
            let (w, h) = ((f64::from(size.w) * factor).round() as i32, (f64::from(size.h) * factor).round() as i32);
            let room = rect.loc + cell.loc + Point::from((INSET, INSET));
            let thumbnail = Rectangle::new(room + Point::from(((THUMB_W - w) / 2, THUMB_H - h)), (w, h).into());
            let icon_at = room + Point::from(((THUMB_W - ICON) / 2, THUMB_H - ICON / 2));
            switcher.thumbnails.push((window.clone(), thumbnail, icon, icon_at));
            switcher.cells.push(cell);
        }
        // The name of the one chosen, under the row, cut to the panel's width.
        let label_font = font("500", 10.5);
        let label = &shown[chosen].2;
        let tw = text_size(label, &label_font).0.min(width - 2 * PADDING);
        let center_y = f64::from(PADDING + CELL_H + LABEL_HEIGHT / 2);
        let x = f64::from((width - tw) / 2);
        show_line(&cr, label, &label_font, (x, center_y, f64::from(width - PADDING) - x), colors.ink);
        drop(cr);
        switcher.first = first;
        switcher.picture = text::picture(surface);
        switcher.drawn = (text::icon_waits() == waits).then_some((switcher.selected, dark));
        switcher.rect = rect;
    }

    /// The windows the switcher shows and their icons, drawn in front of its panel.
    pub fn switcher_thumbnails(&self) -> &[(Window, Rectangle<i32, Logical>, Option<Picture>, Point<i32, Logical>)] {
        self.switcher.as_ref().map_or(&[], |switcher| &switcher.thumbnails)
    }

    pub fn switcher_overlay(&self) -> Option<Overlay<'_>> {
        let switcher = self.switcher.as_ref()?;
        let picture = switcher.picture.as_ref()?;
        Some(Overlay {
            id: &switcher.id,
            rect: Rectangle::new(switcher.rect.loc, picture.size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: ease_out(switcher.opened.elapsed().as_secs_f64() / OPEN_TIME) as f32,
            picture,
        })
    }
}

/// An application's icon, or for one without, a tile with its initial, as the Dock's.
fn icon_picture(icon: Option<&cairo::ImageSurface>, label: &str, colors: &crate::menubar::Colors) -> Option<Picture> {
    let surface = text::canvas(ICON, ICON).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    match icon {
        Some(icon) => {
            let (w, h) = text::points(icon);
            let scale = f64::from(ICON) / w.max(h).max(1.0);
            cr.scale(scale, scale);
            cr.set_source_surface(icon, 0.0, 0.0).ok()?;
            cr.paint().ok()?;
        }
        None => {
            text::rounded_rectangle(&cr, 0.0, 0.0, f64::from(ICON), f64::from(ICON), 14.0);
            cr.set_source_rgb(colors.sheet.0, colors.sheet.1, colors.sheet.2);
            cr.fill().ok()?;
            let initial: String = label.chars().take(1).collect();
            let big = font("600", 22.0);
            let (tw, _) = text_size(&initial, &big);
            show_text(&cr, &initial, &big, f64::from(ICON - tw) / 2.0, f64::from(ICON) / 2.0, colors.ink);
        }
    }
    drop(cr);
    text::picture(surface)
}
