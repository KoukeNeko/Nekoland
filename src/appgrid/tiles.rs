//! The pages' icons and names as sprites, and how they move.
//!
//! Each icon and each name is drawn once and kept, between one showing of the applications
//! and the next too; a frame only says where each goes. An entry glides to its place on a
//! spring whenever its place changes, so the others part for a dragged icon and close up
//! behind it, and an icon let go flies home from where it was held.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use cairo::Context;
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point},
};

use super::{paint_folder, text_layout, Cell, Entry, Lifts, CELL_HEIGHT, CELL_WIDTH, HELD_SIZE, ICON, ICON_TOP};
use crate::{
    motion::settle,
    sprite::{Placed, Sprite},
    text, Nekoland,
};

/// How large an icon's sprite is drawn: as large as it is ever shown, held or pointed at.
const DRAWN_ICON: i32 = HELD_SIZE;
/// Room around the icon on its sprite, for its corners as it turns.
const ICON_MARGIN: i32 = 4;
const LABEL_HEIGHT: i32 = 22;
/// How far under the top of its place a name is.
const LABEL_TOP: i32 = ICON + 20;
/// How far the plate under an icon another is aimed onto reaches around it.
const PLATE_GROWN: f64 = 1.3;
/// How fast an entry glides to its place, and the pages to the one shown: the rate of a
/// spring that just does not swing past, in radians a second.
pub(super) const GLIDE_RATE: f64 = 17.0;
/// How fast a held icon follows the pointer: near enough at once, but not stiffly.
pub(super) const FOLLOW_RATE: f64 = 55.0;
/// How large an entry that was not there a moment ago starts out.
const BORN_SIZE: f64 = 0.6;
/// Nearer its place and its size than this, an entry is there.
const AT_REST: f64 = 0.02;
/// How long a frame may spend drawing sprites that will be wanted, with nothing shown.
const WARM_BUDGET: Duration = Duration::from_millis(3);
/// How many sprites are kept before all are drawn anew.
const STOCK_LIMIT: usize = 600;

/// What a sprite of an entry is drawn from.
pub(super) struct Wanted {
    pub name: String,
    /// The names its icon goes by; for a folder, those of each of its first applications.
    pub icons: Vec<Vec<String>>,
    pub folder: bool,
}

impl Wanted {
    fn icon_key(&self) -> String {
        let names: Vec<String> = self.icons.iter().map(|names| names.join(",")).collect();
        format!("{}{}", if self.folder { "folder:" } else { "" }, names.join(";"))
    }
}

/// The sprites, by what they show. An icon nothing was found for is kept as none, so that
/// it is not looked for every frame.
#[derive(Default)]
pub struct Stock {
    icons: HashMap<String, Option<Sprite>>,
    labels: HashMap<String, Option<Sprite>>,
    plate: Option<Sprite>,
    /// The scale they were drawn for.
    scale: i32,
    /// Applications whose sprites are still to be drawn while nothing is shown, and
    /// whether all of them have been.
    pending: Option<Vec<Wanted>>,
    warm: bool,
}

impl Stock {
    /// Forgets every sprite, as when the icons' theme changes.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Has the applications looked at again when next nothing is shown: one may have been
    /// installed since.
    pub(super) fn cool(&mut self) {
        self.warm = false;
    }

    fn has(&self, wanted: &Wanted) -> bool {
        self.icons.contains_key(&wanted.icon_key()) && self.labels.contains_key(&wanted.name)
    }
}

/// Where an entry is on its way to its place, in the pages' own coordinates, and how many
/// times its size it is drawn.
pub(super) struct Glide {
    pub at: Point<f64, Logical>,
    speed: Point<f64, Logical>,
    pub size: f64,
    size_speed: f64,
    icon_id: Id,
    label_id: Id,
    plate_id: Id,
}

impl Glide {
    pub fn new(at: Point<f64, Logical>, size: f64) -> Self {
        Self {
            at,
            speed: Point::default(),
            size,
            size_speed: 0.0,
            icon_id: Id::new(),
            label_id: Id::new(),
            plate_id: Id::new(),
        }
    }

    fn step(&mut self, goal: Point<f64, Logical>, seconds: f64) {
        settle(&mut self.at.x, &mut self.speed.x, goal.x, GLIDE_RATE, seconds);
        settle(&mut self.at.y, &mut self.speed.y, goal.y, GLIDE_RATE, seconds);
        settle(&mut self.size, &mut self.size_speed, 1.0, GLIDE_RATE, seconds);
        if (self.at - goal).x.hypot((self.at - goal).y) < AT_REST && (self.size - 1.0).abs() < AT_REST / 10.0 {
            (self.at, self.size) = (goal, 1.0);
            (self.speed, self.size_speed) = (Point::default(), 0.0);
        }
    }
}

/// How the entries come to places that changed.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Arrival {
    /// On springs; one that was not there grows in.
    Gliding,
    /// All at once, as when what is shown is other entries altogether.
    AtOnce,
}

/// Moves every entry on toward the place `cells` gives it, `place` telling where each of
/// the cells is, and forgets those no longer shown.
pub(super) fn glide(
    glides: &mut HashMap<Entry, Glide>,
    cells: &[Cell],
    place: impl Fn(usize) -> Point<f64, Logical>,
    seconds: f64,
    arrival: Arrival,
) {
    let first = glides.is_empty();
    glides.retain(|entry, _| cells.contains(&Cell::Entry(*entry)));
    for (index, cell) in cells.iter().enumerate() {
        let Cell::Entry(entry) = cell else {
            continue;
        };
        let goal = place(index);
        match glides.get_mut(entry) {
            Some(glide) if arrival == Arrival::Gliding => glide.step(goal, seconds),
            Some(glide) => *glide = Glide::new(goal, 1.0),
            None => {
                let size = if first || arrival == Arrival::AtOnce { 1.0 } else { BORN_SIZE };
                glides.insert(*entry, Glide::new(goal, size));
            }
        }
    }
}

/// Where the place at `index` of a board `columns` wide is, each page of `per_page`
/// places `stride` further along.
pub(super) fn place(index: usize, columns: usize, per_page: usize, stride: f64) -> Point<f64, Logical> {
    let (page, slot) = (index / per_page.max(1), index % per_page.max(1));
    let (row, column) = (slot / columns.max(1), slot % columns.max(1));
    Point::from((
        page as f64 * stride + column as f64 * f64::from(CELL_WIDTH),
        row as f64 * f64::from(CELL_HEIGHT),
    ))
}

/// How far the icon at a place has shaken, in radians: each place a little out of step
/// with the next, so that they do not sway as one.
fn shaken(index: usize, shake: Option<f64>) -> f64 {
    shake.map_or(0.0, |seconds| {
        super::SHAKE * (seconds * super::SHAKE_RATE * std::f64::consts::TAU + index as f64 * 2.4).sin()
    })
}

/// Entries laid out in places: the pages, or an open folder.
pub(super) struct Board<'a> {
    /// Where the first place is on screen, the pages turned as far as they are.
    pub origin: Point<f64, Logical>,
    /// What of the screen, from side to side, a place has to touch to be drawn.
    pub seen: (f64, f64),
    pub cells: &'a [Cell],
    pub glides: &'a HashMap<Entry, Glide>,
    pub lifts: &'a Lifts,
    /// The place an entry is pressed on, which gives a little.
    pub pressed: Option<usize>,
    /// The place a dragged application is aimed onto.
    pub aimed: Option<usize>,
    /// For how many seconds the icons have been shaking, while they are.
    pub shake: Option<f64>,
    pub alpha: f32,
}

/// Where an entry is on its way in or out, for one whose icon's middle is at a point at
/// rest: that middle now, how many times its size it is, and how much it shows.
pub(super) type Flight<'f> = &'f dyn Fn(Point<f64, Logical>) -> (Point<f64, Logical>, f64, f32);

/// How much an icon gives while it is pressed.
const PRESSED_SIZE: f64 = 0.94;

impl Stock {
    /// The sprites of a board's entries where they are now, front first: on their way in
    /// or out by `flight`, if they are.
    pub(super) fn board<'a>(
        &'a self,
        board: &Board<'a>,
        flight: Option<Flight<'_>>,
        wanted: impl Fn(Entry) -> Wanted,
    ) -> Vec<Placed<'a>> {
        let mut placed: Vec<Placed<'a>> = Vec::new();
        for (index, cell) in board.cells.iter().enumerate() {
            let first = placed.len();
            let Cell::Entry(entry) = cell else {
                continue;
            };
            let Some(glide) = board.glides.get(entry) else {
                continue;
            };
            let corner = board.origin + glide.at;
            if corner.x + f64::from(CELL_WIDTH) < board.seen.0 || corner.x > board.seen.1 {
                continue;
            }
            let wanted = wanted(*entry);
            let turn = shaken(index, board.shake);
            let middle = corner + Point::from((f64::from(CELL_WIDTH) / 2.0, f64::from(ICON_TOP + ICON / 2)));
            let pressed = if board.pressed == Some(index) { PRESSED_SIZE } else { 1.0 };
            let size = glide.size * board.lifts.of(index) * pressed;
            if let Some(Some(icon)) = self.icons.get(&wanted.icon_key()) {
                placed.push(Placed {
                    sprite: icon,
                    id: &glide.icon_id,
                    rect: icon_rect(middle, f64::from(ICON) * size),
                    alpha: board.alpha,
                    turn,
                });
            }
            if let Some(Some(label)) = self.labels.get(&wanted.name) {
                // Turned with its icon, about the icon's middle.
                let below = f64::from(LABEL_TOP + LABEL_HEIGHT / 2 - ICON_TOP - ICON / 2);
                let swung = Point::from((-below * turn.sin(), below * (turn.cos() - 1.0)));
                placed.push(Placed {
                    sprite: label,
                    id: &glide.label_id,
                    rect: smithay::utils::Rectangle::new(
                        corner + Point::from((0.0, f64::from(LABEL_TOP))) + swung,
                        label.size().to_f64(),
                    ),
                    alpha: board.alpha,
                    turn,
                });
            }
            if let (true, Some(plate)) = (board.aimed == Some(index), &self.plate) {
                placed.push(Placed {
                    sprite: plate,
                    id: &glide.plate_id,
                    rect: icon_rect(middle, f64::from(ICON) * PLATE_GROWN),
                    alpha: board.alpha,
                    turn: 0.0,
                });
            }
            // The icon, its name and its plate fly as one.
            if let Some(flight) = flight {
                let (now, grown, shown) = flight(middle);
                for sprite in &mut placed[first..] {
                    let from_middle = sprite.rect.loc - middle;
                    sprite.rect = smithay::utils::Rectangle::new(
                        now + Point::from((from_middle.x * grown, from_middle.y * grown)),
                        smithay::utils::Size::from((sprite.rect.size.w * grown, sprite.rect.size.h * grown)),
                    );
                    sprite.alpha *= shown;
                }
            }
        }
        placed
    }

    /// An entry's icon alone, its middle at `middle`, `size` across: the one being dragged.
    pub(super) fn held<'a>(
        &'a self,
        wanted: &Wanted,
        id: &'a Id,
        middle: Point<f64, Logical>,
        size: f64,
    ) -> Option<Placed<'a>> {
        let icon = self.icons.get(&wanted.icon_key())?.as_ref()?;
        Some(Placed {
            sprite: icon,
            id,
            rect: icon_rect(middle, size),
            alpha: 0.92,
            turn: 0.0,
        })
    }
}

/// Where an icon's sprite goes for the icon to be `size` across around `middle`: the
/// sprite has room around the icon.
fn icon_rect(middle: Point<f64, Logical>, size: f64) -> smithay::utils::Rectangle<f64, Logical> {
    let whole = size * f64::from(DRAWN_ICON + ICON_MARGIN * 2) / f64::from(DRAWN_ICON);
    smithay::utils::Rectangle::new(
        middle - Point::from((whole / 2.0, whole / 2.0)),
        smithay::utils::Size::from((whole, whole)),
    )
}

fn draw_sprite(width: i32, height: i32, paint: impl FnOnce(&Context)) -> Option<Sprite> {
    let surface = text::canvas(width, height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        paint(&cr);
    }
    Sprite::new(surface)
}

/// A name in the middle of a place's width, cut short with an ellipsis where it is too long.
fn draw_label(name: &str) -> Option<Sprite> {
    draw_sprite(CELL_WIDTH, LABEL_HEIGHT, |cr| {
        let label = text_layout(cr, name, 10.0);
        label.set_width((CELL_WIDTH - 14) * pango::SCALE);
        label.set_ellipsize(pango::EllipsizeMode::End);
        let (width, _) = label.pixel_size();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to(f64::from(CELL_WIDTH - width.min(CELL_WIDTH - 14)) / 2.0, 0.0);
        pangocairo::functions::show_layout(cr, &label);
    })
}

/// What shows under an icon that a dragged one would make a folder with.
fn draw_plate() -> Option<Sprite> {
    let whole = DRAWN_ICON + ICON_MARGIN * 2;
    draw_sprite(whole, whole, |cr| {
        let (margin, size) = (f64::from(ICON_MARGIN), f64::from(DRAWN_ICON));
        text::rounded_rectangle(cr, margin, margin, size, size, size * 0.24);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
        let _ = cr.fill();
    })
}

impl Nekoland {
    /// Draws the sprites of an entry, unless they are there.
    pub(super) fn stock(&mut self, wanted: &Wanted) {
        let scale = text::picture_scale();
        if self.app_stock.scale != scale || self.app_stock.icons.len() > STOCK_LIMIT {
            self.app_stock.clear();
            self.app_stock.scale = scale;
        }
        if self.app_stock.plate.is_none() {
            self.app_stock.plate = draw_plate();
        }
        let key = wanted.icon_key();
        if !self.app_stock.icons.contains_key(&key) {
            let icon = self.draw_icon(wanted);
            self.app_stock.icons.insert(key, icon);
        }
        if !self.app_stock.labels.contains_key(&wanted.name) {
            self.app_stock.labels.insert(wanted.name.clone(), draw_label(&wanted.name));
        }
    }

    fn draw_icon(&mut self, wanted: &Wanted) -> Option<Sprite> {
        let whole = DRAWN_ICON + ICON_MARGIN * 2;
        let (margin, size) = (f64::from(ICON_MARGIN), f64::from(DRAWN_ICON));
        if wanted.folder {
            let mini_size = (DRAWN_ICON - 14) / 3 - 2;
            let minis: Vec<_> = wanted
                .icons
                .iter()
                .map(|names| names.iter().find_map(|name| self.dock_icon(name, mini_size, true)))
                .collect();
            return draw_sprite(whole, whole, |cr| paint_folder(cr, margin, margin, size, &minis));
        }
        let icon = wanted
            .icons
            .first()?
            .iter()
            .find_map(|name| self.dock_icon(name, DRAWN_ICON, true))?;
        draw_sprite(whole, whole, |cr| {
            cr.translate(margin, margin);
            crate::stacks::paint_fitted(cr, &icon, size, 1.0);
        })
    }

    /// With nothing shown, draws a few of the sprites the applications will be shown with,
    /// a little each frame, so that showing them does not have to wait for any.
    pub(super) fn warm_stock(&mut self) {
        if self.app_stock.warm {
            return;
        }
        let started = Instant::now();
        let mut pending = self.app_stock.pending.take().unwrap_or_else(|| {
            super::installed()
                .into_iter()
                .map(|app| Wanted {
                    name: app.name,
                    icons: vec![app.icons],
                    folder: false,
                })
                .collect()
        });
        while started.elapsed() < WARM_BUDGET {
            let Some(wanted) = pending.pop() else {
                self.app_stock.warm = true;
                return;
            };
            if !self.app_stock.has(&wanted) {
                self.stock(&wanted);
            }
        }
        self.app_stock.pending = Some(pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spring_comes_to_its_goal_without_passing_it() {
        let (mut value, mut speed) = (0.0, 0.0);
        let mut most = 0.0f64;
        for _ in 0..120 {
            settle(&mut value, &mut speed, 100.0, GLIDE_RATE, 1.0 / 60.0);
            most = most.max(value);
        }
        assert!(most <= 100.0 + 1e-9, "went to {most}");
        assert!((value - 100.0).abs() < 0.01, "ended at {value}");
    }

    #[test]
    fn a_spring_goes_as_far_in_one_step_as_in_many() {
        let (mut once, mut once_speed) = (0.0, 0.0);
        settle(&mut once, &mut once_speed, 1.0, GLIDE_RATE, 0.1);
        let (mut many, mut many_speed) = (0.0, 0.0);
        for _ in 0..10 {
            settle(&mut many, &mut many_speed, 1.0, GLIDE_RATE, 0.01);
        }
        assert!((once - many).abs() < 1e-9);
    }

    #[test]
    fn entries_glide_to_new_places_and_new_ones_grow_in() {
        let mut glides = HashMap::new();
        let at = |index| place(index, 4, 8, 1000.0);
        let both = [Cell::Entry(Entry::App(0)), Cell::Entry(Entry::App(1))];
        glide(&mut glides, &both, at, 0.016, Arrival::Gliding);
        assert_eq!(glides[&Entry::App(1)].at, at(1));
        assert_eq!(glides[&Entry::App(1)].size, 1.0);

        // Room kept before both: they move over, a little at a time.
        let parted = [Cell::Gap, Cell::Entry(Entry::App(0)), Cell::Entry(Entry::App(1)), Cell::Entry(Entry::App(2))];
        glide(&mut glides, &parted, at, 0.016, Arrival::Gliding);
        let moved = glides[&Entry::App(0)].at.x;
        assert!(moved > at(0).x && moved < at(1).x, "at {moved}");
        assert_eq!(glides[&Entry::App(2)].size, BORN_SIZE);
        for _ in 0..120 {
            glide(&mut glides, &parted, at, 0.016, Arrival::Gliding);
        }
        assert_eq!(glides[&Entry::App(0)].at, at(1));
        assert_eq!(glides[&Entry::App(2)].size, 1.0);

        glide(&mut glides, &both[..1], at, 0.016, Arrival::Gliding);
        assert!(!glides.contains_key(&Entry::App(1)));
    }

    #[test]
    fn a_place_is_on_its_page() {
        assert_eq!(place(0, 4, 8, 1000.0), Point::from((0.0, 0.0)));
        assert_eq!(place(5, 4, 8, 1000.0), Point::from((f64::from(CELL_WIDTH), f64::from(CELL_HEIGHT))));
        assert_eq!(place(8, 4, 8, 1000.0), Point::from((1000.0, 0.0)));
    }
}
