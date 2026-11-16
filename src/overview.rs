//! The overview, after Mission Control: the windows of the desktop spread apart where
//! they are, each named, under a bar that lists the desktops.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::element::{solid::SolidColorBuffer, Id},
    desktop::Window,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};

use crate::{
    text::{label, Picture},
    windows::{is_managed, window_names},
    Nekoland,
};

const DURATION: Duration = Duration::from_millis(240);
/// How dark the wallpaper gets behind the windows.
const DIM: f32 = 0.25;
pub const BAR_HEIGHT: i32 = 64;
const BAR_COLOR: [f32; 3] = [0.10, 0.10, 0.11];
const BAR_OPACITY: f32 = 0.55;
const BAR_SPACING: i32 = 6;
const MARGIN: i32 = 48;
/// Kept free around every window, for the name under it and to tell them apart.
const GAP: i32 = 64;
pub const ICON_SIZE: i32 = 56;
/// What the icon and the name take up below a window's bottom edge.
const BADGE_ROOM: i32 = 72;
/// How far two overlapping windows are moved apart in one pass.
const SPREAD_STEP: f64 = 12.0;
const SPREAD_PASSES: usize = 400;

pub struct Slot {
    pub window: Window,
    /// Where the window is on the desktop.
    pub from: Rectangle<i32, Logical>,
    /// Where it sits in the overview.
    to: Rectangle<i32, Logical>,
    /// Where it is drawn right now, between the two.
    pub now: Rectangle<i32, Logical>,
    /// The application's icon and name.
    pub badge: Option<Picture>,
}

#[derive(Clone, Copy)]
enum BarAction {
    Switch(usize),
    Add,
}

pub struct BarItem {
    pub rect: Rectangle<i32, Logical>,
    pub picture: Picture,
    action: BarAction,
}

pub struct Overview {
    started: Instant,
    closing: bool,
    /// The windows, bottom of the stack first.
    pub slots: Vec<Slot>,
    pub dim: SolidColorBuffer,
    pub bar_background: SolidColorBuffer,
    pub bar: Vec<BarItem>,
    /// The slot under the pointer.
    pub hovered: Option<usize>,
    /// The four sides of the outline around it.
    pub outline: [Id; 4],
}

impl Overview {
    /// 0 on the desktop, 1 in the overview, eased.
    pub fn progress(&self) -> f64 {
        let linear = (self.started.elapsed().as_secs_f64() / DURATION.as_secs_f64()).min(1.0);
        let linear = if self.closing { 1.0 - linear } else { linear };
        linear * linear * (3.0 - 2.0 * linear)
    }
}

/// Moves overlapping windows apart, then shrinks the lot to fit `area`, so that every
/// window ends up near where it was. The result is in the order of `windows`.
fn spread(
    windows: &[Rectangle<i32, Logical>],
    area: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    // Centers, and half sizes with the gap around them.
    let mut centers: Vec<(f64, f64)> = windows
        .iter()
        .map(|rect| {
            (
                f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0,
                f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0,
            )
        })
        .collect();
    let halves: Vec<(f64, f64)> = windows
        .iter()
        .map(|rect| {
            (
                f64::from(rect.size.w + GAP) / 2.0,
                f64::from(rect.size.h + GAP) / 2.0,
            )
        })
        .collect();

    let (width, height) = (f64::from(area.size.w.max(1)), f64::from(area.size.h.max(1)));
    for _ in 0..SPREAD_PASSES {
        let mut moved = false;
        for i in 0..centers.len() {
            for j in i + 1..centers.len() {
                let (dx, dy) = (centers[i].0 - centers[j].0, centers[i].1 - centers[j].1);
                if dx.abs() >= halves[i].0 + halves[j].0 || dy.abs() >= halves[i].1 + halves[j].1 {
                    continue;
                }
                // Parted in proportion to the area, so that a wide screen fills sideways
                // before it stacks. Windows right on top of each other have no direction
                // to part in; each pair gets its own.
                let (nx, ny) = (dx / width, dy / height);
                let length = nx.hypot(ny);
                let (ux, uy) = if length < 1e-3 {
                    let angle = (i * 7 + j * 3) as f64;
                    (angle.cos(), angle.sin())
                } else {
                    (nx / length, ny / length)
                };
                let (step_x, step_y) = (ux * SPREAD_STEP * width / height, uy * SPREAD_STEP);
                centers[i].0 += step_x;
                centers[i].1 += step_y;
                centers[j].0 -= step_x;
                centers[j].1 -= step_y;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }

    let bounds =
        centers
            .iter()
            .zip(&halves)
            .fold(None, |bounds: Option<(f64, f64, f64, f64)>, (center, half)| {
                let (left, top, right, bottom) = (
                    center.0 - half.0,
                    center.1 - half.1,
                    center.0 + half.0,
                    center.1 + half.1,
                );
                Some(match bounds {
                    None => (left, top, right, bottom),
                    Some(b) => (b.0.min(left), b.1.min(top), b.2.max(right), b.3.max(bottom)),
                })
            });
    let Some((left, top, right, bottom)) = bounds else {
        return Vec::new();
    };
    let scale = f64::min(
        f64::from(area.size.w) / (right - left).max(1.0),
        f64::from(area.size.h) / (bottom - top).max(1.0),
    )
    .min(1.0);
    let (bounds_x, bounds_y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    let (area_x, area_y) = (
        f64::from(area.loc.x) + f64::from(area.size.w) / 2.0,
        f64::from(area.loc.y) + f64::from(area.size.h) / 2.0,
    );

    windows
        .iter()
        .zip(&centers)
        .map(|(window, center)| {
            let size = Size::<i32, Logical>::from((
                (f64::from(window.size.w) * scale).round() as i32,
                (f64::from(window.size.h) * scale).round() as i32,
            ));
            let loc = Point::from((
                (area_x + (center.0 - bounds_x) * scale - f64::from(size.w) / 2.0).round() as i32,
                (area_y + (center.1 - bounds_y) * scale - f64::from(size.h) / 2.0).round() as i32,
            ));
            Rectangle::new(loc, size)
        })
        .collect()
}

fn lerp(from: Rectangle<i32, Logical>, to: Rectangle<i32, Logical>, t: f64) -> Rectangle<i32, Logical> {
    let mix = |a: i32, b: i32| (f64::from(a) + f64::from(b - a) * t).round() as i32;
    Rectangle::new(
        Point::from((mix(from.loc.x, to.loc.x), mix(from.loc.y, to.loc.y))),
        Size::from((mix(from.size.w, to.size.w), mix(from.size.h, to.size.h))),
    )
}

impl Nekoland {
    pub fn toggle_overview(&mut self) {
        match &self.overview {
            Some(overview) if overview.closing => {}
            Some(_) => self.close_overview(None),
            None => self.open_overview(),
        }
    }

    fn open_overview(&mut self) {
        let Some(output) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        self.overview = Some(Overview {
            started: Instant::now(),
            closing: false,
            slots: Vec::new(),
            dim: SolidColorBuffer::new(output.size, [0.0, 0.0, 0.0, 0.0]),
            bar_background: SolidColorBuffer::new((output.size.w, BAR_HEIGHT), [0.0, 0.0, 0.0, 0.0]),
            bar: Vec::new(),
            hovered: None,
            outline: [Id::new(), Id::new(), Id::new(), Id::new()],
        });
        self.rebuild_overview();
        // Nothing has the keyboard in the overview; what is typed starts a search.
        self.set_keyboard_focus(None);
    }

    /// Lays the overview out again for the desktop that is shown now.
    pub fn rebuild_overview(&mut self) {
        let Some(output) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        // Under the bar, above whatever a dock keeps free at the bottom.
        let bottom = self
            .usable_area()
            .map_or(output.loc.y + output.size.h, |area| area.loc.y + area.size.h);
        let top = output.loc.y + BAR_HEIGHT;
        let area = Rectangle::new(
            Point::from((output.loc.x + MARGIN, top + MARGIN / 2)),
            Size::from((output.size.w - MARGIN * 2, bottom - top - MARGIN)),
        );

        let windows: Vec<(Window, Rectangle<i32, Logical>)> = self
            .space
            .elements()
            .filter(|window| is_managed(window))
            .filter_map(|window| {
                let location = self.space.element_location(window)?;
                Some((window.clone(), Rectangle::new(location, window.geometry().size)))
            })
            .collect();
        // Spread with room under each window for its icon and name.
        let rects: Vec<_> = windows
            .iter()
            .map(|(_, rect)| Rectangle::new(rect.loc, rect.size + Size::from((0, BADGE_ROOM))))
            .collect();
        let slots = windows
            .into_iter()
            .zip(spread(&rects, area))
            .map(|((window, from), padded)| {
                let scale = f64::from(padded.size.w) / f64::from(from.size.w.max(1));
                let height = (f64::from(from.size.h) * scale).round() as i32;
                let to = Rectangle::new(padded.loc, Size::from((padded.size.w, height)));
                let (title, app_id) = window_names(&window);
                Slot {
                    badge: self.apps.badge(&app_id, &title, ICON_SIZE),
                    window,
                    from,
                    to,
                    now: to,
                }
            })
            .collect();

        let bar = self.overview_bar(output);
        if let Some(overview) = &mut self.overview {
            overview.slots = slots;
            overview.bar = bar;
            overview.hovered = None;
        }
        self.tick_overview();
    }

    /// The names of the desktops, centered, and the button that adds one, at the right.
    fn overview_bar(&self, output: Rectangle<i32, Logical>) -> Vec<BarItem> {
        let names: Vec<(Picture, BarAction)> = (0..self.workspaces.count())
            .filter_map(|index| {
                let active = index == self.workspaces.active;
                let picture = label(&format!("Desktop {}", index + 1), 12, 240, active.then_some(0.2))?;
                Some((picture, BarAction::Switch(index)))
            })
            .collect();
        let width: i32 =
            names.iter().map(|(picture, _)| picture.size.w).sum::<i32>() + BAR_SPACING * names.len() as i32;
        let centered = |picture: &Picture| output.loc.y + (BAR_HEIGHT - picture.size.h) / 2;

        let mut x = output.loc.x + (output.size.w - width) / 2;
        let mut bar: Vec<BarItem> = names
            .into_iter()
            .map(|(picture, action)| {
                let rect = Rectangle::new(Point::from((x, centered(&picture))), picture.size);
                x += picture.size.w + BAR_SPACING;
                BarItem {
                    rect,
                    picture,
                    action,
                }
            })
            .collect();
        if let Some(picture) = label("+", 20, 60, None) {
            let x = output.loc.x + output.size.w - picture.size.w - MARGIN / 2;
            bar.push(BarItem {
                rect: Rectangle::new(Point::from((x, centered(&picture))), picture.size),
                picture,
                action: BarAction::Add,
            });
        }
        bar
    }

    /// Leaves the overview, bringing `selected` to the front if there is one.
    pub fn close_overview(&mut self, selected: Option<Window>) {
        let Some(overview) = &mut self.overview else {
            return;
        };
        if overview.closing {
            return;
        }
        overview.closing = true;
        overview.started = Instant::now();
        overview.hovered = None;

        match selected {
            Some(window) => self.raise_and_focus(&window),
            None => {
                let top = self
                    .space
                    .elements()
                    .rev()
                    .find(|window| is_managed(window))
                    .cloned();
                match top {
                    Some(window) => self.raise_and_focus(&window),
                    None => self.set_keyboard_focus(None),
                }
            }
        }
    }

    /// Moves the overview's animation on; called before each frame.
    pub fn tick_overview(&mut self) {
        let pointer = self.seat.get_pointer().map(|pointer| pointer.current_location());
        let Some(overview) = &mut self.overview else {
            return;
        };
        let progress = overview.progress();
        if overview.closing && progress <= 0.0 {
            self.overview = None;
            return;
        }
        overview.slots.retain(|slot| slot.window.alive());
        for slot in &mut overview.slots {
            slot.now = lerp(slot.from, slot.to, progress);
        }
        let fade = progress as f32;
        overview.dim.set_color([0.0, 0.0, 0.0, DIM * fade]);
        let [r, g, b] = BAR_COLOR.map(|channel| channel * BAR_OPACITY * fade);
        overview.bar_background.set_color([r, g, b, BAR_OPACITY * fade]);

        let settled = !overview.closing && progress >= 1.0;
        overview.hovered = pointer.filter(|_| settled).and_then(|pos| {
            overview
                .slots
                .iter()
                .rposition(|slot| slot.now.to_f64().contains(pos))
        });
    }

    /// Whether a position is on the desktops' bar, which lies over a panel at the top.
    pub fn overview_covers(&self, pos: Point<f64, Logical>) -> bool {
        let top = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map_or(0, |output| output.loc.y);
        pos.y < f64::from(top + BAR_HEIGHT)
    }

    /// A click in the overview: on a desktop's name, the add button, a window, or nothing.
    pub fn overview_click(&mut self, pos: Point<f64, Logical>) {
        let Some(overview) = &self.overview else {
            return;
        };
        let action = overview
            .bar
            .iter()
            .find(|item| item.rect.to_f64().contains(pos))
            .map(|item| item.action);
        match action {
            Some(BarAction::Switch(index)) => self.switch_workspace(index),
            Some(BarAction::Add) => {
                self.add_workspace();
                self.rebuild_overview();
            }
            None => {
                let picked = overview
                    .slots
                    .iter()
                    .rev()
                    .find(|slot| slot.now.to_f64().contains(pos))
                    .map(|slot| slot.window.clone());
                self.close_overview(picked);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    #[test]
    fn spread_parts_stacked_windows_and_keeps_them_inside() {
        let area = rect(48, 88, 1824, 880);
        // Three windows opened one after another all sit in the middle of the screen.
        let windows = vec![
            rect(612, 270, 696, 500),
            rect(780, 214, 360, 616),
            rect(560, 240, 800, 560),
        ];
        let spread = spread(&windows, area);
        assert_eq!(spread.len(), 3);
        for (index, placed) in spread.iter().enumerate() {
            assert!(area.contains_rect(*placed), "window {index} at {placed:?}");
            assert!(spread[..index].iter().all(|other| !other.overlaps(*placed)));
        }
    }

    #[test]
    fn spread_leaves_a_lone_window_its_size() {
        let area = rect(48, 88, 1824, 880);
        let spread = spread(&[rect(100, 200, 640, 480)], area);
        assert_eq!(spread[0].size, Size::from((640, 480)));
    }

    #[test]
    fn spread_keeps_left_of_and_above() {
        let area = rect(0, 0, 1920, 1000);
        let spread = spread(&[rect(0, 0, 1200, 900), rect(700, 100, 1200, 900)], area);
        assert!(spread[0].loc.x < spread[1].loc.x);
        assert!(spread[0].loc.y <= spread[1].loc.y);
    }
}
