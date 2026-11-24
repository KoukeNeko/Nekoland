//! The layouts Super+Z offers at the top right of the window in front, as Windows' do: a
//! part of one chosen puts the window there.

use std::time::Instant;

use smithay::{
    backend::renderer::element::Id,
    input::keyboard::Keysym,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    control::{draw_panel, ease_out, OPEN_TIME},
    glass::Tier,
    menubar::{colors, Overlay, MENU_RADIUS},
    text::{self, Picture},
    windows::is_managed,
    Nekoland,
};

/// A rectangle as x, y, width and height.
type Frac = (f64, f64, f64, f64);

/// Each layout's parts, as fractions of the display, and the window command for each.
const LAYOUTS: [&[(Frac, &str)]; 6] = [
    &[((0.0, 0.0, 0.5, 1.0), "left"), ((0.5, 0.0, 0.5, 1.0), "right")],
    &[((0.0, 0.0, 2.0 / 3.0, 1.0), "first-two-thirds"), ((2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0), "last-third")],
    &[((0.0, 0.0, 1.0 / 3.0, 1.0), "first-third"), ((1.0 / 3.0, 0.0, 2.0 / 3.0, 1.0), "last-two-thirds")],
    &[
        ((0.0, 0.0, 1.0 / 3.0, 1.0), "first-third"),
        ((1.0 / 3.0, 0.0, 1.0 / 3.0, 1.0), "center-third"),
        ((2.0 / 3.0, 0.0, 1.0 / 3.0, 1.0), "last-third"),
    ],
    &[
        ((0.0, 0.0, 0.5, 1.0), "left"),
        ((0.5, 0.0, 0.5, 0.5), "top-right"),
        ((0.5, 0.5, 0.5, 0.5), "bottom-right"),
    ],
    &[
        ((0.0, 0.0, 0.5, 0.5), "top-left"),
        ((0.5, 0.0, 0.5, 0.5), "top-right"),
        ((0.0, 0.5, 0.5, 0.5), "bottom-left"),
        ((0.5, 0.5, 0.5, 0.5), "bottom-right"),
    ],
];
const COLUMNS: usize = 3;
const PADDING: i32 = 12;
const GAP: i32 = 12;
const THUMB_W: i32 = 88;
const THUMB_H: i32 = 56;
/// Between the parts of a layout.
const PART_GAP: f64 = 3.0;
/// From the window's right edge, and from the display's edges.
const INSET: i32 = 8;
/// Below the window's top, past its title bar and the buttons in it.
const BELOW_TITLE: i32 = 52;

pub struct SnapLayouts {
    /// The layout and the part of it chosen.
    selected: (usize, usize),
    id: Id,
    picture: Option<Picture>,
    drawn: Option<((usize, usize), bool)>,
    rect: Rectangle<i32, Logical>,
    opened: Instant,
}

/// Where each part of each layout is drawn, in the panel.
fn part_rects() -> Vec<((usize, usize), Frac)> {
    let mut rects = Vec::new();
    for (layout, parts) in LAYOUTS.iter().enumerate() {
        let (column, row) = ((layout % COLUMNS) as i32, (layout / COLUMNS) as i32);
        let x = f64::from(PADDING + column * (THUMB_W + GAP));
        let y = f64::from(PADDING + row * (THUMB_H + GAP));
        for (part, ((px, py, pw, ph), _)) in parts.iter().enumerate() {
            let (w, h) = (f64::from(THUMB_W), f64::from(THUMB_H));
            rects.push((
                (layout, part),
                (
                    x + px * w + PART_GAP / 2.0,
                    y + py * h + PART_GAP / 2.0,
                    pw * w - PART_GAP,
                    ph * h - PART_GAP,
                ),
            ));
        }
    }
    rects
}

fn panel_size() -> (i32, i32) {
    let rows = LAYOUTS.len().div_ceil(COLUMNS) as i32;
    (
        2 * PADDING + COLUMNS as i32 * THUMB_W + (COLUMNS as i32 - 1) * GAP,
        2 * PADDING + rows * THUMB_H + (rows - 1) * GAP,
    )
}

impl Nekoland {
    /// Shows the layouts over the window in front, or closes them.
    pub fn toggle_snap_layouts(&mut self) {
        if self.snap_layouts.take().is_some() {
            return;
        }
        let Some(window) = self.focused_window().filter(is_managed) else {
            return;
        };
        let (Some(at), Some(output)) = (self.space.element_location(&window), self.output_of(&window)) else {
            return;
        };
        let Some(area) = self.space.output_geometry(&output) else {
            return;
        };
        let (w, h) = panel_size();
        let size = window.geometry().size;
        let x = (at.x + size.w - w - INSET).clamp(area.loc.x + INSET, area.loc.x + area.size.w - w - INSET);
        // Above the window where there is room, so it covers none of it; else under its title bar.
        let above = at.y - h - INSET;
        let top = area.loc.y + crate::menubar::MENU_BAR_HEIGHT + INSET;
        let y = if above >= top { above } else { at.y + BELOW_TITLE };
        let y = y.clamp(area.loc.y + INSET, area.loc.y + area.size.h - h - INSET);
        self.snap_layouts = Some(SnapLayouts {
            selected: (0, 0),
            id: Id::new(),
            picture: None,
            drawn: None,
            rect: Rectangle::new((x, y).into(), (w, h).into()),
            opened: Instant::now(),
        });
    }

    fn apply_snap_layout(&mut self, (layout, part): (usize, usize)) {
        self.snap_layouts = None;
        let Some(parts) = LAYOUTS.get(layout) else {
            return;
        };
        if let Some((_, command)) = parts.get(part) {
            let window = self.focused_window();
            self.window_command(command);
            // What is left of the layout chosen offers the other windows, part after part.
            let commands: Vec<&'static str> = parts.iter().map(|(_, command)| *command).collect();
            if let Some(window) = window {
                self.offer_snap_assist(&window, command, Some(&commands));
            }
        }
    }

    /// The arrows go from part to part, and a row up or down; Return puts the window in the
    /// part chosen, Esc closes. Returns whether the layouts are shown.
    pub fn snap_layouts_key(&mut self, sym: Keysym) -> bool {
        let Some(snap) = self.snap_layouts.as_mut() else {
            return false;
        };
        let parts: Vec<(usize, usize)> = part_rects().into_iter().map(|(at, _)| at).collect();
        let index = parts.iter().position(|at| *at == snap.selected).unwrap_or(0);
        let count = parts.len();
        match sym {
            Keysym::Escape => self.snap_layouts = None,
            Keysym::Return | Keysym::KP_Enter | Keysym::space => {
                let selected = snap.selected;
                self.apply_snap_layout(selected);
            }
            Keysym::Right | Keysym::Tab => snap.selected = parts[(index + 1) % count],
            Keysym::Left => snap.selected = parts[(index + count - 1) % count],
            Keysym::Down | Keysym::Up => {
                let (layout, part) = snap.selected;
                let row = if sym == Keysym::Down { COLUMNS } else { LAYOUTS.len() - COLUMNS };
                let other = (layout + row) % LAYOUTS.len();
                snap.selected = (other, part.min(LAYOUTS[other].len() - 1));
            }
            _ => self.snap_layouts = None,
        }
        true
    }

    fn snap_part_at(&self, pos: Point<f64, Logical>) -> Option<(usize, usize)> {
        let snap = self.snap_layouts.as_ref()?;
        let at = pos - snap.rect.loc.to_f64();
        part_rects()
            .into_iter()
            .find(|(_, (x, y, w, h))| at.x >= *x && at.x < x + w && at.y >= *y && at.y < y + h)
            .map(|(part, _)| part)
    }

    pub fn snap_layouts_motion(&mut self, pos: Point<f64, Logical>) {
        if let Some(part) = self.snap_part_at(pos) {
            if let Some(snap) = self.snap_layouts.as_mut() {
                snap.selected = part;
            }
        }
    }

    /// A press on a part puts the window there; anywhere else closes the layouts. Returns
    /// whether they were shown.
    pub fn snap_layouts_press(&mut self, pos: Point<f64, Logical>) -> bool {
        if self.snap_layouts.is_none() {
            return false;
        }
        match self.snap_part_at(pos) {
            Some(part) => self.apply_snap_layout(part),
            None => self.snap_layouts = None,
        }
        true
    }

    pub fn refresh_snap_layouts(&mut self) {
        let dark = self.menu_bar.dark;
        let Some(snap) = self.snap_layouts.as_mut() else {
            return;
        };
        if snap.drawn == Some((snap.selected, dark)) && snap.picture.is_some() {
            return;
        }
        let colors = colors(dark);
        let (w, h) = panel_size();
        let Some((surface, cr)) = draw_panel(w, h, dark, &colors) else {
            return;
        };
        // The part chosen is solid ink, as anything chosen is.
        for (part, (x, y, pw, ph)) in part_rects() {
            text::rounded_rectangle(&cr, x, y, pw, ph, 4.0);
            let alpha = if part == snap.selected { 1.0 } else { 0.18 };
            cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, alpha);
            let _ = cr.fill();
        }
        drop(cr);
        snap.picture = text::picture(surface);
        snap.drawn = Some((snap.selected, dark));
    }

    pub fn snap_layouts_overlay(&self) -> Option<Overlay<'_>> {
        let snap = self.snap_layouts.as_ref()?;
        let picture = snap.picture.as_ref()?;
        Some(Overlay {
            id: &snap.id,
            rect: Rectangle::new(snap.rect.loc, picture.size),
            glass: Some((Tier::Menu, MENU_RADIUS)),
            alpha: 1.0,
            fade: ease_out(snap.opened.elapsed().as_secs_f64() / OPEN_TIME) as f32,
            picture,
        })
    }
}
