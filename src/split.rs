//! The bar between windows put side by side, as in the Mac's Split View and between
//! Windows' snapped windows: under the pointer it shows a handle on glass, dragged it moves
//! the edges of every window against it, and a double click puts it back in the middle.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::element::Id,
    desktop::{Window, WindowSurface},
    input::pointer::{CursorIcon, CursorImageStatus, Focus, GrabStartData},
    output::Output,
    utils::{Logical, Point, Rectangle, Serial, Size},
};

use crate::{
    arrange::{self, Part, Seam},
    glass::Tier,
    grabs::DividerGrab,
    menubar::Overlay,
    text::{self, Picture},
    windows::is_managed,
    Nekoland,
};

/// How far either side of the line the pointer takes hold of the bar, when the windows
/// leave less room than that between them.
const HOLD: i32 = 5;
/// The handle shown on the bar, across it and along it.
const HANDLE_THICKNESS: i32 = 6;
const HANDLE_LENGTH: i32 = 56;
/// Two presses on the bar this close together are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// How long the bar waits for a window to draw the size it was last given before it gives
/// it the next one anyway, so that a slow program does not hold the others back.
const WAIT_FOR_DRAWN: Duration = Duration::from_millis(120);

/// A bar between windows: where it is, and the windows on either side with their parts.
#[derive(Clone)]
pub struct Divider {
    output: Output,
    area: Rectangle<i32, Logical>,
    seam: Seam,
    windows: Vec<Window>,
    parts: Vec<Part>,
}

pub struct Splits {
    /// The bar under the pointer, or being dragged.
    active: Option<Divider>,
    dragging: bool,
    /// Where the pointer is while the bar is dragged, until the windows can be given it.
    pending: Option<Point<f64, Logical>>,
    /// When the windows were last given a size while the bar was dragged.
    given: Option<Instant>,
    last_press: Option<Instant>,
    /// Whether the compositor set the cursor, to set it back.
    cursor: bool,
    id: Id,
    /// The handle, and whether it was drawn upright or across.
    handle: Option<(bool, Picture)>,
}

impl Default for Splits {
    fn default() -> Self {
        Self {
            active: None,
            dragging: false,
            pending: None,
            given: None,
            last_press: None,
            cursor: false,
            id: Id::new(),
            handle: None,
        }
    }
}

impl Divider {
    /// The line the windows meet along, as thick as the room between them, at least as
    /// thick as the pointer takes hold of.
    fn band(&self, gap: i32) -> Rectangle<i32, Logical> {
        let half = (gap / 2).max(HOLD);
        let area = self.area;
        let along = |from: f64, to: f64, start: i32, length: i32| {
            let at = |share: f64| start + (share * f64::from(length)).round() as i32;
            let edge = |share: f64| {
                if share <= 0.0 || share >= 1.0 {
                    gap
                } else {
                    gap / 2
                }
            };
            (at(from) + edge(from), at(to) - edge(to))
        };
        if self.seam.upright {
            let x = area.loc.x + (self.seam.at * f64::from(area.size.w)).round() as i32;
            let (y0, y1) = along(self.seam.from, self.seam.to, area.loc.y, area.size.h);
            Rectangle::new((x - half, y0).into(), (2 * half, (y1 - y0).max(1)).into())
        } else {
            let y = area.loc.y + (self.seam.at * f64::from(area.size.h)).round() as i32;
            let (x0, x1) = along(self.seam.from, self.seam.to, area.loc.x, area.size.w);
            Rectangle::new((x0, y - half).into(), ((x1 - x0).max(1), 2 * half).into())
        }
    }

    fn is_same_bar(&self, other: &Divider) -> bool {
        self.output == other.output && self.seam == other.seam && self.windows == other.windows
    }
}

impl Nekoland {
    /// The bars between the windows put in parts of `output`'s usable area.
    fn dividers_on(&self, output: &Output) -> Vec<Divider> {
        let Some(area) = self.usable_area_on(output) else {
            return Vec::new();
        };
        let (windows, parts): (Vec<Window>, Vec<Part>) = self
            .space
            .elements()
            .filter(|window| is_managed(window) && !self.is_fullscreen(window))
            .filter(|window| self.output_of(window).as_ref() == Some(output))
            .filter_map(|window| Some((window.clone(), self.arranged_part(window)?)))
            .unzip();
        arrange::seams(&parts)
            .into_iter()
            .map(|seam| {
                let members: Vec<usize> = seam.before.iter().chain(&seam.after).copied().collect();
                // Each window keeps its place in the list the seam names it by.
                let seam = Seam {
                    before: (0..seam.before.len()).collect(),
                    after: (seam.before.len()..members.len()).collect(),
                    ..seam
                };
                Divider {
                    output: output.clone(),
                    area,
                    seam,
                    windows: members.iter().map(|&index| windows[index].clone()).collect(),
                    parts: members.iter().map(|&index| parts[index]).collect(),
                }
            })
            .collect()
    }

    /// The bar the pointer at `pos` takes hold of: one not covered by another window or a
    /// panel, with nothing else open over the windows.
    fn divider_at(&self, pos: Point<f64, Logical>) -> Option<Divider> {
        if self.overview.is_some()
            || self.lock.is_some()
            || self.fullscreen_window().is_some()
            || self.switcher.is_some()
            || self.snap_assist.is_some()
            || self.menu_bar_covers(pos)
            || self.dock_covers(pos)
            || self.layer_under(pos, &crate::state::ABOVE_WINDOWS).is_some()
            // Not while a window is being dragged or resized across it.
            || self.seat.get_pointer().is_some_and(|pointer| pointer.is_grabbed())
        {
            return None;
        }
        let output = self.output_at(pos)?;
        let gap = self.gap_between_windows();
        let over = self.space.element_under(pos).map(|(window, _)| window.clone());
        self.dividers_on(&output).into_iter().find(|divider| {
            divider.band(gap).to_f64().contains(pos)
                && over
                    .as_ref()
                    .is_none_or(|window| divider.windows.contains(window))
        })
    }

    /// Whether the pointer at `pos` is on the bar, which then keeps it from the windows.
    pub fn divider_covers(&self, pos: Point<f64, Logical>) -> bool {
        let gap = self.gap_between_windows();
        self.splits.dragging
            || self
                .splits
                .active
                .as_ref()
                .is_some_and(|divider| divider.band(gap).to_f64().contains(pos))
    }

    /// The pointer moved: the bar under it shows its handle, and the cursor says it moves.
    pub fn divider_motion(&mut self, pos: Point<f64, Logical>) {
        if self.splits.dragging {
            return;
        }
        let found = self.divider_at(pos);
        let changed = match (&found, &self.splits.active) {
            (Some(found), Some(active)) => !found.is_same_bar(active),
            (None, None) => false,
            _ => true,
        };
        if !changed {
            // The windows may have moved under it: kept as they are now.
            self.splits.active = found;
            return;
        }
        self.cursor_status = match &found {
            Some(divider) if divider.seam.upright => CursorImageStatus::Named(CursorIcon::EwResize),
            Some(_) => CursorImageStatus::Named(CursorIcon::NsResize),
            None if self.splits.cursor => CursorImageStatus::default_named(),
            None => self.cursor_status.clone(),
        };
        self.splits.cursor = found.is_some();
        self.splits.active = found;
    }

    /// A left press on the bar starts dragging it, or on the second of a double click puts
    /// it back in the middle. Returns whether the press was on the bar.
    pub fn divider_press(&mut self, pos: Point<f64, Logical>, button: u32, serial: Serial) -> bool {
        if self.splits.active.is_none() || !self.divider_covers(pos) {
            return false;
        }
        let now = Instant::now();
        if self
            .splits
            .last_press
            .is_some_and(|last| now.duration_since(last) < DOUBLE_CLICK)
        {
            self.splits.last_press = None;
            self.center_divider();
            return true;
        }
        self.splits.last_press = Some(now);
        self.splits.dragging = true;
        self.splits.given = None;
        let start = GrabStartData {
            focus: None,
            button,
            location: pos,
        };
        let pointer = self.seat.get_pointer().unwrap();
        pointer.set_grab(self, DividerGrab::new(start), serial, Focus::Clear);
        true
    }

    /// The bar dragged to `pos`, as far as the windows on either side can go: given to them
    /// once each has drawn the size it was last given, or at once when `now`.
    pub fn drag_divider(&mut self, pos: Point<f64, Logical>, now: bool) {
        self.splits.pending = Some(pos);
        let Some(divider) = self.splits.active.as_ref() else {
            return;
        };
        let waited = self
            .splits
            .given
            .is_none_or(|given| given.elapsed() >= WAIT_FOR_DRAWN);
        if !now && !waited && !divider.windows.iter().all(has_drawn_size) {
            return;
        }
        let (area, upright) = (divider.area, divider.seam.upright);
        let share = if upright {
            (pos.x - f64::from(area.loc.x)) / f64::from(area.size.w.max(1))
        } else {
            (pos.y - f64::from(area.loc.y)) / f64::from(area.size.h.max(1))
        };
        self.splits.pending = None;
        self.move_divider(share);
    }

    /// Gives the windows where the bar was dragged to while they were drawing, once they
    /// have: each frame, while it is dragged.
    pub fn refresh_divider(&mut self) {
        if let Some(pos) = self.splits.pending.filter(|_| self.splits.dragging) {
            self.drag_divider(pos, false);
        }
        // Windows put elsewhere, or closed, take the bar between them away from under a
        // pointer that has not moved.
        if !self.splits.dragging && self.splits.active.is_some() {
            if let Some(pos) = self.seat.get_pointer().map(|pointer| pointer.current_location()) {
                self.divider_motion(pos);
            }
        }
        let Some(upright) = self.splits.active.as_ref().map(|divider| divider.seam.upright) else {
            return;
        };
        if self.splits.handle.as_ref().is_some_and(|(was, _)| *was == upright) {
            return;
        }
        self.splits.handle = handle_picture(upright).map(|picture| (upright, picture));
    }

    /// The drag is over: the windows get where the bar was let go.
    pub fn end_divider_drag(&mut self) {
        if let Some(pos) = self.splits.pending.take() {
            self.drag_divider(pos, true);
        }
        // The bar stays under the pointer, for a second click; the next move finds it again.
        self.splits.dragging = false;
    }

    /// Puts the bar halfway between the far edges of the windows on either side.
    fn center_divider(&mut self) {
        let Some(divider) = self.splits.active.as_ref() else {
            return;
        };
        let middle = arrange::seam_middle(&divider.seam, &divider.parts);
        self.move_divider(middle);
    }

    /// Moves the bar to `share` of the usable area, as near as the windows' sizes let it.
    fn move_divider(&mut self, share: f64) {
        let Some(mut divider) = self.splits.active.take() else {
            return;
        };
        let gap = self.gap_between_windows();
        let length = f64::from(
            if divider.seam.upright {
                divider.area.size.w
            } else {
                divider.area.size.h
            }
            .max(1),
        );
        let limits: Vec<(f64, f64)> = divider
            .windows
            .iter()
            .map(|window| {
                let (min, max) = crate::windows::size_limits(window);
                let (least, most) = if divider.seam.upright {
                    (min.w, max.w)
                } else {
                    (min.h, max.h)
                };
                let least = f64::from(least.max(arrange::MIN_SIZE) + gap) / length;
                let most = if most > 0 {
                    f64::from(most + gap) / length
                } else {
                    f64::INFINITY
                };
                (least, most)
            })
            .collect();
        let (low, high) = arrange::seam_range(&divider.seam, &divider.parts, &limits);
        // Whole points, as the windows are placed.
        let to = (share.clamp(low, high) * length).round() / length;
        if (to - divider.seam.at).abs() * length >= 0.5 {
            arrange::moved_seam(&mut divider.parts, &divider.seam, to);
            divider.seam.at = to;
            for (window, part) in divider.windows.iter().zip(&divider.parts) {
                self.put_in_part(window, divider.area, SPLIT_COMMAND, *part, 0);
                // Drawn where it is, not stretched toward its new size: the bar is under the pointer.
                self.stop_morph(window);
            }
            self.splits.given = Some(Instant::now());
        }
        self.splits.active = Some(divider);
    }

    /// The handle on the bar under the pointer or being dragged, in the middle of it.
    pub fn divider_overlay(&self) -> Option<Overlay<'_>> {
        let divider = self.splits.active.as_ref()?;
        let (upright, picture) = self.splits.handle.as_ref()?;
        if *upright != divider.seam.upright {
            return None;
        }
        let band = divider.band(self.gap_between_windows());
        let middle = band.loc + band.size.downscale(2).to_point();
        let size: Size<i32, Logical> = picture.size;
        Some(Overlay {
            id: &self.splits.id,
            rect: Rectangle::new(middle - size.downscale(2).to_point(), size),
            glass: Some((Tier::Menu, HANDLE_THICKNESS / 2)),
            alpha: 1.0,
            fade: 1.0,
            picture,
        })
    }
}

/// What a window the bar put in a part says it was put there by: no command, so the next
/// half or third given to it starts its sizes over.
const SPLIT_COMMAND: &str = "split";

/// Whether a window has drawn itself at the size it was last asked to be.
fn has_drawn_size(window: &Window) -> bool {
    match window.underlying_surface() {
        WindowSurface::Wayland(toplevel) => {
            let asked = toplevel.with_pending_state(|pending| pending.size);
            let drawn = toplevel.with_committed_state(|committed| committed.and_then(|state| state.size));
            asked == drawn
        }
        WindowSurface::X11(_) => true,
    }
}

/// The handle: a pill of the light sheet ringed in the light look's ink, upright or across.
/// It sits on the windows, not on the desktop, so it keeps one look that shows on windows
/// dark or light, whichever the system's is.
fn handle_picture(upright: bool) -> Option<Picture> {
    let (w, h) = if upright {
        (HANDLE_THICKNESS, HANDLE_LENGTH)
    } else {
        (HANDLE_LENGTH, HANDLE_THICKNESS)
    };
    let colors = crate::menubar::colors(false);
    let surface = text::canvas(w, h).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let radius = f64::from(HANDLE_THICKNESS) / 2.0;
    text::rounded_rectangle(
        &cr,
        0.5,
        0.5,
        f64::from(w) - 1.0,
        f64::from(h) - 1.0,
        radius - 0.5,
    );
    cr.set_source_rgba(colors.sheet.0, colors.sheet.1, colors.sheet.2, 0.95);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.45);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    drop(cr);
    text::picture(surface)
}
