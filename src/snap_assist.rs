//! Snap Assist, as Windows': once a window is put in a half or in a part of a layout, what
//! is left beside it shows the other windows, small, on the menus' glass, as the Mac's Split
//! View shows them on its other side. The one picked fills that part, and the next part left
//! empty shows the windows still to place.

use std::time::Instant;

use smithay::{
    backend::renderer::element::Id,
    desktop::{Window, WindowSurface},
    input::keyboard::Keysym,
    output::Output,
    utils::{IsAlive, Logical, Point, Rectangle},
};

use crate::{
    arrange::{self, Arrangement, Part},
    control::{draw_panel, ease_out, show_line, OPEN_TIME},
    glass::Tier,
    menubar::{colors, font, Overlay, MENU_RADIUS},
    text::{self, Picture},
    windows::{app_id, is_managed, window_names, ALERT_APP_IDS, KEYBOARD_APP_ID},
    Nekoland,
};

/// The layouts a part is taken to be of, to tell what is left beside it: the first that
/// has it.
const LAYOUTS: [&[&str]; 4] = [
    &["left", "right"],
    &["top", "bottom"],
    &["top-left", "top-right", "bottom-left", "bottom-right"],
    &["first-third", "center-third", "last-third"],
];
/// Between the panel and the part it fills, and around the windows in it.
const INSET: i32 = 8;
const PADDING: i32 = 20;
const GAP: i32 = 8;
/// The room a window is fitted into, at most, under its icon and name.
const THUMB_W: i32 = 240;
const THUMB_H: i32 = 150;
/// Around a window and its name in its cell, which is lit when it is the one chosen.
const CELL_INSET: i32 = 10;
const HEADER: i32 = 28;
const ICON: i32 = 20;

struct Candidate {
    window: Window,
    label: String,
    icon: Option<String>,
}

pub struct SnapAssist {
    output: Output,
    /// The parts still to fill, the one shown first, each with the command that names it.
    zones: Vec<(&'static str, Part)>,
    candidates: Vec<Candidate>,
    selected: usize,
    id: Id,
    picture: Option<Picture>,
    /// What the picture was drawn for: the one chosen, the parts left, the windows, dark.
    drawn: Option<(usize, usize, usize, bool)>,
    /// Where the panel is on the output, and each cell in it, from its top left.
    rect: Rectangle<i32, Logical>,
    cells: Vec<Rectangle<i32, Logical>>,
    /// Each window shown and the room it is fitted into, on the output.
    thumbnails: Vec<(
        Window,
        Rectangle<i32, Logical>,
        Option<Picture>,
        Point<i32, Logical>,
    )>,
    opened: Instant,
}

/// The parts of the layout `command` is of, but its own, in the order they are filled.
fn rest_of_layout(command: &str, layout: Option<&[&'static str]>) -> Vec<&'static str> {
    let layout = layout.or_else(|| LAYOUTS.iter().copied().find(|layout| layout.contains(&command)));
    layout
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|part| *part != command)
        .collect()
}

impl Nekoland {
    /// Offers the other windows for what is left of the layout `window` was just put in a
    /// part of by `command`: that of `layout` when Snap Layouts named it, else the one it is
    /// taken to be of.
    pub fn offer_snap_assist(&mut self, window: &Window, command: &str, layout: Option<&[&'static str]>) {
        self.snap_assist = None;
        let Some(output) = self.output_of(window) else {
            return;
        };
        let rest: Vec<(&'static str, Part)> = rest_of_layout(command, layout)
            .into_iter()
            .filter_map(|command| match arrange::arrangement(command)? {
                Arrangement::Part(part) => Some((command, part)),
                _ => None,
            })
            .collect();
        // The parts of the layout other windows already fill are left as they are, and so
        // are those windows.
        let filling: Vec<(Window, Part)> = self
            .space
            .elements()
            .filter(|other| *other != window && self.output_of(other).as_ref() == Some(&output))
            .filter_map(|other| Some((other.clone(), self.arranged_part(other)?)))
            .filter(|(_, part)| rest.iter().any(|(_, zone)| zone == part))
            .collect();
        let zones: Vec<(&'static str, Part)> = rest
            .into_iter()
            .filter(|(_, zone)| !filling.iter().any(|(_, part)| part == zone))
            .collect();
        if zones.is_empty() {
            return;
        }
        let placed: Vec<Window> = std::iter::once(window.clone())
            .chain(filling.into_iter().map(|(other, _)| other))
            .collect();
        let candidates = self.snap_assist_candidates(&placed);
        if candidates.is_empty() {
            return;
        }
        self.snap_assist = Some(SnapAssist {
            output,
            zones,
            candidates,
            selected: 0,
            id: Id::new(),
            picture: None,
            drawn: None,
            rect: Rectangle::default(),
            cells: Vec::new(),
            thumbnails: Vec::new(),
            opened: Instant::now(),
        });
    }

    /// The windows that can fill a part, the one used last first, minimized ones last: not
    /// those `placed` in the layout already, nor ones of a single size, dialogs, or the
    /// system's own.
    fn snap_assist_candidates(&mut self, placed: &[Window]) -> Vec<Candidate> {
        let stacked = self.space.elements().rev().cloned();
        let minimized = self.minimized.iter().map(|(window, _)| window.clone());
        let windows: Vec<Window> = stacked
            .chain(minimized)
            .filter(|window| window.alive() && !placed.contains(window) && is_managed(window))
            .filter(|window| !self.is_fixed_size(window))
            .filter(|window| !self.config.glass.contains_key(&app_id(window)))
            .filter(|window| match window.underlying_surface() {
                WindowSurface::Wayland(toplevel) => toplevel.parent().is_none(),
                WindowSurface::X11(_) => true,
            })
            .collect();
        let mut candidates = Vec::new();
        for window in windows {
            let (title, app_id) = window_names(&window);
            if app_id == KEYBOARD_APP_ID || ALERT_APP_IDS.contains(&app_id.as_str()) {
                continue;
            }
            let info = self.apps.info(&app_id);
            let name = info
                .name
                .clone()
                .unwrap_or_else(|| app_id.rsplit('.').next().unwrap_or(&app_id).to_string());
            let title = crate::windows::title_without_app(&title, &name);
            candidates.push(Candidate {
                label: if title.is_empty() { name } else { title },
                icon: info.icon_name.clone(),
                window,
            });
        }
        candidates
    }

    /// Puts the window chosen in the part shown, and goes on to the next part left, if
    /// there are windows still to place.
    fn pick_snap_assist(&mut self) {
        let Some(mut assist) = self.snap_assist.take() else {
            return;
        };
        let Some(candidate) =
            (assist.selected < assist.candidates.len()).then(|| assist.candidates.remove(assist.selected))
        else {
            return;
        };
        let (command, part) = assist.zones.remove(0);
        let window = candidate.window;
        self.unminimize(&window);
        if let Some(area) = self.usable_area_on(&assist.output) {
            self.place_in_part(&window, area, command, part, 0);
        }
        self.raise_and_focus(&window);
        if assist.zones.is_empty() || assist.candidates.is_empty() {
            return;
        }
        assist.selected = 0;
        assist.drawn = None;
        assist.opened = Instant::now();
        self.snap_assist = Some(assist);
    }

    /// The arrows and Tab go from window to window, Return puts the one chosen in the part,
    /// Esc closes. Any other key closes it and goes on. Returns whether the key was taken.
    pub fn snap_assist_key(&mut self, sym: Keysym, shift: bool) -> bool {
        let Some(assist) = self.snap_assist.as_mut() else {
            return false;
        };
        let count = assist.cells.len().max(1);
        let next = (assist.selected + 1) % count;
        let previous = (assist.selected + count - 1) % count;
        match sym {
            Keysym::Escape => self.snap_assist = None,
            Keysym::Return | Keysym::KP_Enter | Keysym::space => self.pick_snap_assist(),
            Keysym::Right | Keysym::Down => assist.selected = next,
            Keysym::Left | Keysym::Up => assist.selected = previous,
            Keysym::Tab | Keysym::ISO_Left_Tab => {
                assist.selected = if shift || sym == Keysym::ISO_Left_Tab {
                    previous
                } else {
                    next
                };
            }
            _ => {
                self.snap_assist = None;
                return false;
            }
        }
        true
    }

    fn snap_assist_cell_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let assist = self.snap_assist.as_ref()?;
        let at = pos.to_i32_round() - assist.rect.loc;
        assist.cells.iter().position(|cell| cell.contains(at))
    }

    /// The window under the pointer is the one chosen.
    pub fn snap_assist_motion(&mut self, pos: Point<f64, Logical>) {
        if let Some(index) = self.snap_assist_cell_at(pos) {
            if let Some(assist) = self.snap_assist.as_mut() {
                assist.selected = index;
            }
        }
    }

    pub fn snap_assist_covers(&self, pos: Point<f64, Logical>) -> bool {
        self.snap_assist
            .as_ref()
            .is_some_and(|assist| assist.rect.contains(pos.to_i32_round()))
    }

    /// A press on a window puts it in the part; one off the panel closes it. Returns whether
    /// the panel took the press.
    pub fn snap_assist_press(&mut self, pos: Point<f64, Logical>) -> bool {
        if self.snap_assist.is_none() {
            return false;
        }
        match self.snap_assist_cell_at(pos) {
            Some(index) => {
                if let Some(assist) = self.snap_assist.as_mut() {
                    assist.selected = index;
                }
                self.pick_snap_assist();
                true
            }
            None if self.snap_assist_covers(pos) => true,
            None => {
                // The press closes it and goes on to what is under it, as Windows' does.
                self.snap_assist = None;
                false
            }
        }
    }

    /// Draws the panel again when what it shows changed, and closes it once there is
    /// nothing to show it for.
    pub fn refresh_snap_assist(&mut self) {
        let dark = self.menu_bar.dark;
        if self.overview.is_some() || self.lock.is_some() {
            self.snap_assist = None;
        }
        let Some(assist) = self.snap_assist.as_mut() else {
            return;
        };
        assist.candidates.retain(|candidate| candidate.window.alive());
        if assist.candidates.is_empty() {
            self.snap_assist = None;
            return;
        }
        assist.selected = assist.selected.min(assist.candidates.len() - 1);
        let key = (assist.selected, assist.zones.len(), assist.candidates.len(), dark);
        if assist.drawn == Some(key) && assist.picture.is_some() {
            return;
        }
        let (output, (_, part)) = (assist.output.clone(), assist.zones[0]);
        let Some(area) = self.usable_area_on(&output) else {
            self.snap_assist = None;
            return;
        };
        let zone = arrange::part_rect(area, part, self.gap_between_windows());
        let rect = Rectangle::new(
            zone.loc + Point::from((INSET, INSET)),
            (zone.size.w - 2 * INSET, zone.size.h - 2 * INSET).into(),
        );
        let (width, height) = (rect.size.w.max(1), rect.size.h.max(1));

        // As many cells as fit, a narrow part's windows smaller.
        let thumb_w = THUMB_W.min(width - 2 * PADDING - 2 * CELL_INSET).max(40);
        let thumb_h = THUMB_H * thumb_w / THUMB_W;
        let (cell_w, cell_h) = (thumb_w + 2 * CELL_INSET, HEADER + thumb_h + 2 * CELL_INSET);
        let columns = ((width - 2 * PADDING + GAP) / (cell_w + GAP)).max(1);
        let rows = ((height - 2 * PADDING + GAP) / (cell_h + GAP)).max(1);
        let Some(assist) = self.snap_assist.as_ref() else {
            return;
        };
        let shown: Vec<(Window, Option<String>, String)> = assist
            .candidates
            .iter()
            .take((columns * rows) as usize)
            .map(|candidate| {
                (
                    candidate.window.clone(),
                    candidate.icon.clone(),
                    candidate.label.clone(),
                )
            })
            .collect();
        let icons: Vec<Option<cairo::ImageSurface>> = shown
            .iter()
            .map(|(_, icon, _)| icon.as_deref().and_then(|icon| self.dock_icon(icon, ICON, dark)))
            .collect();
        let used_columns = columns.min(shown.len() as i32);
        let used_rows = (shown.len() as i32 + columns - 1) / columns;
        let grid_w = used_columns * cell_w + (used_columns - 1) * GAP;
        let grid_h = used_rows * cell_h + (used_rows - 1) * GAP;
        let origin =
            Point::<i32, Logical>::from(((width - grid_w) / 2, ((height - grid_h) / 2).max(PADDING)));

        let colors = colors(dark);
        let Some((surface, cr)) = draw_panel(width, height, dark, &colors) else {
            return;
        };
        let label_font = font("500", 10.5);
        let Some(assist) = self.snap_assist.as_mut() else {
            return;
        };
        assist.cells.clear();
        assist.thumbnails.clear();
        for (index, ((window, _, label), icon)) in shown.iter().zip(icons).enumerate() {
            let (column, row) = (index as i32 % columns, index as i32 / columns);
            let cell = Rectangle::new(
                origin + Point::from((column * (cell_w + GAP), row * (cell_h + GAP))),
                (cell_w, cell_h).into(),
            );
            let (x, y, w, h) = crate::control::to_f64(cell);
            if index == assist.selected {
                // A tile behind the window, ringed in ink, as the switcher's.
                text::rounded_rectangle(
                    &cr,
                    x + 1.0,
                    y + 1.0,
                    w - 2.0,
                    h - 2.0,
                    f64::from(MENU_RADIUS) - 1.0,
                );
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.16);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.9);
                cr.set_line_width(2.0);
                let _ = cr.stroke();
            } else {
                // Each window on a well of its own, so one as dark as the glass still shows.
                crate::control::draw_well(&cr, &colors, cell, f64::from(MENU_RADIUS));
            }
            // The icon and the name over the window, as Windows' are.
            let header_center = y + f64::from(CELL_INSET + HEADER / 2);
            let mut text_x = x + f64::from(CELL_INSET);
            if let Some(icon) = icon {
                let (iw, ih) = text::points(&icon);
                let scale = f64::from(ICON) / iw.max(ih).max(1.0);
                let _ = cr.save();
                cr.translate(text_x, header_center - f64::from(ICON) / 2.0);
                cr.scale(scale, scale);
                if cr.set_source_surface(&icon, 0.0, 0.0).is_ok() {
                    let _ = cr.paint();
                }
                let _ = cr.restore();
                text_x += f64::from(ICON + 8);
            }
            let text_width = x + w - f64::from(CELL_INSET) - text_x;
            show_line(
                &cr,
                label,
                &label_font,
                (text_x, header_center, text_width),
                colors.ink,
            );
            let room = Rectangle::new(
                rect.loc + cell.loc + Point::from((CELL_INSET, CELL_INSET + HEADER)),
                (thumb_w, thumb_h).into(),
            );
            assist
                .thumbnails
                .push((window.clone(), room, None, Point::default()));
            assist.cells.push(cell);
        }
        drop(cr);
        assist.picture = text::picture(surface);
        assist.drawn = Some(key);
        assist.rect = rect;
    }

    /// The windows the panel shows, drawn in front of it.
    pub fn snap_assist_thumbnails(
        &self,
    ) -> &[(
        Window,
        Rectangle<i32, Logical>,
        Option<Picture>,
        Point<i32, Logical>,
    )] {
        self.snap_assist.as_ref().map_or(&[], |assist| &assist.thumbnails)
    }

    pub fn snap_assist_overlay(&self) -> Option<Overlay<'_>> {
        let assist = self.snap_assist.as_ref()?;
        let picture = assist.picture.as_ref()?;
        Some(Overlay {
            id: &assist.id,
            rect: Rectangle::new(assist.rect.loc, picture.size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: ease_out(assist.opened.elapsed().as_secs_f64() / OPEN_TIME) as f32,
            picture,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_left_is_the_rest_of_the_layout_a_part_is_of() {
        assert_eq!(rest_of_layout("left", None), ["right"]);
        assert_eq!(rest_of_layout("bottom", None), ["top"]);
        assert_eq!(
            rest_of_layout("top-right", None),
            ["top-left", "bottom-left", "bottom-right"]
        );
        assert_eq!(
            rest_of_layout("center-third", None),
            ["first-third", "last-third"]
        );
        assert!(rest_of_layout("center-half", None).is_empty());
        let layout: &[&str] = &["first-two-thirds", "last-third"];
        assert_eq!(rest_of_layout("last-third", Some(layout)), ["first-two-thirds"]);
    }
}
