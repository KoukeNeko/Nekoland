//! Where the window commands put a window, as Rectangle does on the Mac: halves, quarters,
//! thirds, fourths and sixths of the usable area, its height, its middle, a little larger
//! or smaller, or against one of its edges; and where a window dragged to the display's
//! edge goes.
//!
//! Only arithmetic on rectangles, so that it can be tried without a compositor.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// A part of the usable area, as the fractions of it its left, top, right and bottom edges
/// are at.
pub type Part = [f64; 4];

/// What a window command does to a window's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arrangement {
    /// Over a part of the usable area.
    Part(Part),
    /// The whole usable area, as the window's own maximize does.
    Maximize,
    /// The usable area's full height, where the window already is across.
    MaximizeHeight,
    /// In the middle of the usable area, its size kept.
    Center,
    /// Larger by this many points on each side, or smaller when negative.
    Resize(i32),
    /// Against an edge of the usable area, its size kept.
    Move(Direction),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

const THIRD: f64 = 1.0 / 3.0;
const TWO_THIRDS: f64 = 2.0 / 3.0;

/// How far Rectangle's 「Make Larger」 and 「Make Smaller」 move each edge.
const RESIZE_STEP: i32 = 30;
/// No smaller than this does a window get made.
const MIN_SIZE: i32 = 120;
/// The widest gap left between windows, and around them.
pub const MAX_GAP: i32 = 64;

/// What each window command does, by the name the launcher, the menu bar and the shortcuts
/// give it.
pub fn arrangement(command: &str) -> Option<Arrangement> {
    let part = |part: Part| Some(Arrangement::Part(part));
    match command {
        "left" => part([0.0, 0.0, 0.5, 1.0]),
        "right" => part([0.5, 0.0, 1.0, 1.0]),
        "top" => part([0.0, 0.0, 1.0, 0.5]),
        "bottom" => part([0.0, 0.5, 1.0, 1.0]),
        "center-half" => part([0.25, 0.0, 0.75, 1.0]),
        "top-left" => part([0.0, 0.0, 0.5, 0.5]),
        "top-right" => part([0.5, 0.0, 1.0, 0.5]),
        "bottom-left" => part([0.0, 0.5, 0.5, 1.0]),
        "bottom-right" => part([0.5, 0.5, 1.0, 1.0]),
        "first-third" => part([0.0, 0.0, THIRD, 1.0]),
        "center-third" => part([THIRD, 0.0, TWO_THIRDS, 1.0]),
        "last-third" => part([TWO_THIRDS, 0.0, 1.0, 1.0]),
        "first-two-thirds" => part([0.0, 0.0, TWO_THIRDS, 1.0]),
        "center-two-thirds" => part([1.0 / 6.0, 0.0, 5.0 / 6.0, 1.0]),
        "last-two-thirds" => part([THIRD, 0.0, 1.0, 1.0]),
        "first-fourth" => part([0.0, 0.0, 0.25, 1.0]),
        "second-fourth" => part([0.25, 0.0, 0.5, 1.0]),
        "third-fourth" => part([0.5, 0.0, 0.75, 1.0]),
        "last-fourth" => part([0.75, 0.0, 1.0, 1.0]),
        "first-three-fourths" => part([0.0, 0.0, 0.75, 1.0]),
        "last-three-fourths" => part([0.25, 0.0, 1.0, 1.0]),
        "top-left-sixth" => part([0.0, 0.0, THIRD, 0.5]),
        "top-center-sixth" => part([THIRD, 0.0, TWO_THIRDS, 0.5]),
        "top-right-sixth" => part([TWO_THIRDS, 0.0, 1.0, 0.5]),
        "bottom-left-sixth" => part([0.0, 0.5, THIRD, 1.0]),
        "bottom-center-sixth" => part([THIRD, 0.5, TWO_THIRDS, 1.0]),
        "bottom-right-sixth" => part([TWO_THIRDS, 0.5, 1.0, 1.0]),
        "almost-maximize" => part([0.05, 0.05, 0.95, 0.95]),
        "maximize" => Some(Arrangement::Maximize),
        "maximize-height" => Some(Arrangement::MaximizeHeight),
        "center" => Some(Arrangement::Center),
        "larger" => Some(Arrangement::Resize(RESIZE_STEP)),
        "smaller" => Some(Arrangement::Resize(-RESIZE_STEP)),
        "move-left" => Some(Arrangement::Move(Direction::Left)),
        "move-right" => Some(Arrangement::Move(Direction::Right)),
        "move-up" => Some(Arrangement::Move(Direction::Up)),
        "move-down" => Some(Arrangement::Move(Direction::Down)),
        _ => None,
    }
}

/// The sizes a half or a quarter goes through when its command is given again, as
/// Rectangle's do: a half, two thirds, a third, and a half again.
const CYCLE: [f64; 3] = [0.5, TWO_THIRDS, THIRD];

/// `part` the `step`th time its command is given in a row. A half or a quarter against a
/// side changes how far it reaches from that side; other parts stay as they are.
pub fn cycled(part: Part, step: usize) -> Part {
    let reach = CYCLE[step % CYCLE.len()];
    let [left, top, right, bottom] = part;
    let is_half = |from: f64, to: f64| ((to - from) - 0.5).abs() < f64::EPSILON;
    if is_half(left, right) {
        if left == 0.0 {
            return [0.0, top, reach, bottom];
        }
        if right == 1.0 {
            return [1.0 - reach, top, 1.0, bottom];
        }
    } else if left == 0.0 && right == 1.0 && is_half(top, bottom) {
        if top == 0.0 {
            return [left, 0.0, right, reach];
        }
        if bottom == 1.0 {
            return [left, 1.0 - reach, right, 1.0];
        }
    }
    part
}

/// Where `part` of `area` is, with `gap` points left around the area's edge and half of it
/// on each side of an edge inside it, so that windows side by side are `gap` apart.
pub fn part_rect(area: Rectangle<i32, Logical>, part: Part, gap: i32) -> Rectangle<i32, Logical> {
    let [left, top, right, bottom] = part;
    let at = |fraction: f64, length: i32| (fraction * f64::from(length)).round() as i32;
    let inset = |fraction: f64| {
        if fraction <= 0.0 || fraction >= 1.0 {
            gap
        } else {
            gap / 2
        }
    };
    let x0 = area.loc.x + at(left, area.size.w) + inset(left);
    let y0 = area.loc.y + at(top, area.size.h) + inset(top);
    let x1 = area.loc.x + at(right, area.size.w) - inset(right);
    let y1 = area.loc.y + at(bottom, area.size.h) - inset(bottom);
    Rectangle::new(
        Point::from((x0, y0)),
        Size::from(((x1 - x0).max(1), (y1 - y0).max(1))),
    )
}

/// The area windows are arranged in, less `gap` on every side.
pub fn inset(area: Rectangle<i32, Logical>, gap: i32) -> Rectangle<i32, Logical> {
    part_rect(area, [0.0, 0.0, 1.0, 1.0], gap)
}

/// A window of `size` in the middle of `area`, its top left kept inside it.
pub fn centered(area: Rectangle<i32, Logical>, size: Size<i32, Logical>) -> Rectangle<i32, Logical> {
    let x = area.loc.x + (area.size.w - size.w).max(0) / 2;
    let y = area.loc.y + (area.size.h - size.h).max(0) / 2;
    Rectangle::new(Point::from((x, y)), size)
}

/// `frame` as tall as `area`, left where it is across.
pub fn full_height(area: Rectangle<i32, Logical>, frame: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        Point::from((frame.loc.x, area.loc.y)),
        Size::from((frame.size.w, area.size.h)),
    )
}

/// `frame` larger by `by` points on each side, or smaller when it is negative, inside
/// `area` and no smaller than a window is useful. An edge already against the area stays
/// there, so the window grows the other way, as Rectangle's does.
pub fn resized(
    area: Rectangle<i32, Logical>,
    frame: Rectangle<i32, Logical>,
    by: i32,
) -> Rectangle<i32, Logical> {
    let along = |from: i32, to: i32, low: i32, high: i32| {
        let (from, to) = if by > 0 {
            grow_span(from, to, (low, high), by)
        } else {
            shrink_span(from, to, (low, high), -by)
        };
        (from.max(low), to.min(high))
    };
    let (x0, x1) = along(
        frame.loc.x,
        frame.loc.x + frame.size.w,
        area.loc.x,
        area.loc.x + area.size.w,
    );
    let (y0, y1) = along(
        frame.loc.y,
        frame.loc.y + frame.size.h,
        area.loc.y,
        area.loc.y + area.size.h,
    );
    Rectangle::new(Point::from((x0, y0)), Size::from((x1 - x0, y1 - y0)))
}

/// A span `by` longer at each end; what one end has no room for goes to the other.
fn grow_span(from: i32, to: i32, (low, high): (i32, i32), by: i32) -> (i32, i32) {
    let room_before = (from - low).max(0);
    let room_after = (high - to).max(0);
    let before = by.min(room_before);
    let after = by.min(room_after);
    let extra_before = (by - after).min(room_before - before);
    let extra_after = (by - before).min(room_after - after);
    (from - before - extra_before, to + after + extra_after)
}

/// A span `by` shorter at each end, or at the far end alone when the other is against a
/// side, and never shorter than `MIN_SIZE`.
fn shrink_span(from: i32, to: i32, (low, high): (i32, i32), by: i32) -> (i32, i32) {
    let spare = (to - from - MIN_SIZE).max(0);
    match (from <= low, to >= high) {
        (true, false) => (from, to - by.min(spare)),
        (false, true) => (from + by.min(spare), to),
        _ => {
            let each = by.min(spare / 2);
            (from + each, to - each)
        }
    }
}

/// `frame` against one edge of `area`, as large as it was or as fits.
pub fn moved(
    area: Rectangle<i32, Logical>,
    frame: Rectangle<i32, Logical>,
    direction: Direction,
) -> Rectangle<i32, Logical> {
    let size = Size::from((frame.size.w.min(area.size.w), frame.size.h.min(area.size.h)));
    let clamp = |at: i32, length: i32, low: i32, span: i32| at.clamp(low, low + span - length);
    let mut loc = Point::from((
        clamp(frame.loc.x, size.w, area.loc.x, area.size.w),
        clamp(frame.loc.y, size.h, area.loc.y, area.size.h),
    ));
    match direction {
        Direction::Left => loc.x = area.loc.x,
        Direction::Right => loc.x = area.loc.x + area.size.w - size.w,
        Direction::Up => loc.y = area.loc.y,
        Direction::Down => loc.y = area.loc.y + area.size.h - size.h,
    }
    Rectangle::new(loc, size)
}

/// How far along an edge from a corner the pointer counts as in that corner.
const CORNER: f64 = 60.0;

/// The command for where a window let go of with the pointer at `pos` goes, as Rectangle's
/// snap areas: a corner to that quarter, the left or right edge to that half, the top to the
/// whole area, and the bottom to the third the pointer is over. None away from the edges,
/// which the pointer cannot pass.
pub fn snap_command(display: Rectangle<f64, Logical>, pos: Point<f64, Logical>) -> Option<&'static str> {
    let (left, top) = (display.loc.x, display.loc.y);
    let (right, bottom) = (left + display.size.w, top + display.size.h);
    let at_left = pos.x < left + 1.0;
    let at_right = pos.x >= right - 1.0;
    let at_top = pos.y < top + 1.0;
    let at_bottom = pos.y >= bottom - 1.0;
    let near_left = pos.x < left + CORNER;
    let near_right = pos.x >= right - CORNER;
    let near_top = pos.y < top + CORNER;
    let near_bottom = pos.y >= bottom - CORNER;
    let command = if (at_left && near_top) || (at_top && near_left) {
        "top-left"
    } else if (at_right && near_top) || (at_top && near_right) {
        "top-right"
    } else if (at_left && near_bottom) || (at_bottom && near_left) {
        "bottom-left"
    } else if (at_right && near_bottom) || (at_bottom && near_right) {
        "bottom-right"
    } else if at_top {
        "maximize"
    } else if at_left {
        "left"
    } else if at_right {
        "right"
    } else if at_bottom {
        let across = (pos.x - left) / display.size.w;
        if across < THIRD {
            "first-third"
        } else if across < TWO_THIRDS {
            "center-third"
        } else {
            "last-third"
        }
    } else {
        return None;
    };
    Some(command)
}

/// Where a window `size` wide that was dragged by `pointer`, while it was `frame`, goes as
/// it comes out of a half or the whole display at its earlier size: under the pointer at
/// the same share of its width, so the title bar stays in hand.
pub fn lifted(
    frame: Rectangle<i32, Logical>,
    pointer: Point<f64, Logical>,
    size: Size<i32, Logical>,
) -> Point<i32, Logical> {
    let share = if frame.size.w > 0 {
        ((pointer.x - f64::from(frame.loc.x)) / f64::from(frame.size.w)).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let x = pointer.x - share * f64::from(size.w);
    Point::from((x.round() as i32, frame.loc.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    /// A 1200 by 900 area() under a 30-point menu bar.
    fn area() -> Rectangle<i32, Logical> {
        rect(0, 30, 1200, 900)
    }

    #[test]
    fn every_named_part_lies_inside_the_area() {
        for command in [
            "left",
            "right",
            "top",
            "bottom",
            "center-half",
            "top-left",
            "bottom-right",
            "first-third",
            "last-two-thirds",
            "center-two-thirds",
            "first-fourth",
            "last-three-fourths",
            "top-center-sixth",
            "bottom-right-sixth",
            "almost-maximize",
        ] {
            let Some(Arrangement::Part(part)) = arrangement(command) else {
                panic!("{command} is not a part");
            };
            let placed = part_rect(area(), part, 0);
            assert!(area().contains_rect(placed), "{command}: {placed:?}");
        }
        assert_eq!(arrangement("nothing"), None);
    }

    #[test]
    fn halves_cycle_through_two_thirds_and_a_third() {
        let left = [0.0, 0.0, 0.5, 1.0];
        assert_eq!(cycled(left, 0), left);
        assert_eq!(cycled(left, 1), [0.0, 0.0, TWO_THIRDS, 1.0]);
        assert_eq!(cycled(left, 2), [0.0, 0.0, THIRD, 1.0]);
        assert_eq!(cycled(left, 3), left);
        assert_eq!(cycled([0.5, 0.0, 1.0, 1.0], 1), [1.0 - TWO_THIRDS, 0.0, 1.0, 1.0]);
        assert_eq!(cycled([0.0, 0.5, 1.0, 1.0], 2), [0.0, 1.0 - THIRD, 1.0, 1.0]);
        assert_eq!(cycled([0.5, 0.5, 1.0, 1.0], 1), [1.0 - TWO_THIRDS, 0.5, 1.0, 1.0]);
    }

    #[test]
    fn parts_off_the_sides_do_not_cycle() {
        assert_eq!(cycled([0.25, 0.0, 0.75, 1.0], 1), [0.25, 0.0, 0.75, 1.0]);
        assert_eq!(cycled([0.0, 0.0, THIRD, 1.0], 1), [0.0, 0.0, THIRD, 1.0]);
    }

    #[test]
    fn gaps_are_whole_at_the_edge_and_shared_between_windows() {
        let left = part_rect(area(), [0.0, 0.0, 0.5, 1.0], 10);
        let right = part_rect(area(), [0.5, 0.0, 1.0, 1.0], 10);
        assert_eq!(left, rect(10, 40, 585, 880));
        assert_eq!(right, rect(605, 40, 585, 880));
        assert_eq!(right.loc.x - (left.loc.x + left.size.w), 10);
        assert_eq!(part_rect(area(), [0.0, 0.0, 1.0, 1.0], 0), area());
    }

    #[test]
    fn larger_grows_away_from_edges_it_is_against() {
        let frame = rect(0, 30, 600, 900);
        assert_eq!(resized(area(), frame, 30), rect(0, 30, 660, 900));
        let middle = rect(300, 300, 400, 300);
        assert_eq!(resized(area(), middle, 30), rect(270, 270, 460, 360));
        assert_eq!(resized(area(), middle, -30), rect(330, 330, 340, 240));
    }

    #[test]
    fn smaller_stops_at_a_useful_size() {
        let small = rect(300, 300, 130, 130);
        let shrunk = resized(area(), small, -30);
        assert_eq!(shrunk.size, Size::from((MIN_SIZE, MIN_SIZE)));
    }

    #[test]
    fn smaller_keeps_a_window_against_the_edge_it_is_at() {
        let left_half = rect(0, 30, 600, 900);
        let shrunk = resized(area(), left_half, -30);
        assert_eq!(shrunk.loc.x, 0);
        assert_eq!(shrunk.size.w, 570);
    }

    #[test]
    fn moving_keeps_the_size_and_meets_the_edge() {
        let frame = rect(300, 300, 400, 300);
        assert_eq!(moved(area(), frame, Direction::Left), rect(0, 300, 400, 300));
        assert_eq!(moved(area(), frame, Direction::Right), rect(800, 300, 400, 300));
        assert_eq!(moved(area(), frame, Direction::Up), rect(300, 30, 400, 300));
        assert_eq!(moved(area(), frame, Direction::Down), rect(300, 630, 400, 300));
        let too_wide = rect(-50, 300, 1500, 300);
        assert_eq!(moved(area(), too_wide, Direction::Up), rect(0, 30, 1200, 300));
    }

    #[test]
    fn edges_and_corners_snap_to_their_parts() {
        let display = Rectangle::new((0.0, 0.0).into(), (1920.0, 1080.0).into());
        let at = |x, y| snap_command(display, (x, y).into());
        assert_eq!(at(0.0, 500.0), Some("left"));
        assert_eq!(at(1919.0, 500.0), Some("right"));
        assert_eq!(at(800.0, 0.0), Some("maximize"));
        assert_eq!(at(0.0, 10.0), Some("top-left"));
        assert_eq!(at(30.0, 0.0), Some("top-left"));
        assert_eq!(at(1919.0, 1079.0), Some("bottom-right"));
        assert_eq!(at(1900.0, 0.0), Some("top-right"));
        assert_eq!(at(0.0, 1060.0), Some("bottom-left"));
        assert_eq!(at(300.0, 1079.0), Some("first-third"));
        assert_eq!(at(960.0, 1079.0), Some("center-third"));
        assert_eq!(at(1500.0, 1079.0), Some("last-third"));
        assert_eq!(at(800.0, 500.0), None);
    }

    #[test]
    fn a_lifted_window_stays_under_the_pointer_at_the_same_share() {
        let half = rect(0, 30, 1000, 900);
        let at = lifted(half, (250.0, 40.0).into(), Size::from((400, 300)));
        assert_eq!(at, Point::from((150, 30)));
    }

    #[test]
    fn centering_keeps_a_large_window_at_the_top_left() {
        assert_eq!(centered(area(), Size::from((400, 300))), rect(400, 330, 400, 300));
        assert_eq!(centered(area(), Size::from((2000, 300))).loc.x, 0);
        assert_eq!(
            full_height(area(), rect(100, 200, 300, 300)),
            rect(100, 30, 300, 900)
        );
        assert_eq!(inset(area(), 8), rect(8, 38, 1184, 884));
    }
}
