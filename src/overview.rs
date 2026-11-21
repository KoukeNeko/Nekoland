//! The overview, after Mission Control: the windows of the desktop spread apart where
//! they are, each named, under a bar that lists the desktops.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::element::{solid::SolidColorBuffer, Id},
    desktop::Window,
    input::keyboard::Keysym,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};

use crate::{
    appgrid::AppGrid,
    glass::Tier,
    menubar::{Command, Entry, Overlay, MENU_BAR_HEIGHT},
    motion::lerp,
    text::{self, Picture},
    windows::{is_managed, window_names},
    Nekoland,
};

const DURATION: Duration = Duration::from_millis(240);
/// How dark the wallpaper gets behind the windows.
const DIM: f32 = 0.25;
/// Kept free under the desktops' bar, closed or opened up.
const BAR_GAP: i32 = 16;
const NAME_PADDING: i32 = 12;
const NAME_GAP: i32 = 8;
const NAME_PILL: i32 = 24;
/// How much of the screen's width a desktop's picture is in the opened-up bar: a tenth,
/// on a screen of any size, as the Mac's is the same share of any of its screens.
const THUMBNAIL_SHARE: i32 = 10;
/// Kept free at each end of the bar.
const BAR_EDGE: i32 = 48;
/// How far the add button stays in from the right edge.
const ADD_MARGIN: i32 = 24;
const THUMBNAIL_GAP: i32 = 32;
const EXPANDED_PADDING: i32 = 12;
const LABEL_HEIGHT: i32 = 24;
/// How fast a desktop glides to its place in the bar, as a spring's rate.
const BAR_GLIDE_RATE: f64 = 16.0;
/// The add button: a desktop's picture this much of the size of the others.
const ADD_SIZE: f64 = 0.62;
/// How dark the wallpaper is on the add button, under its plus.
const ADD_DIM: f64 = 0.45;
/// Around a desktop's picture on its frame, for the ring of the one shown.
const FRAME_MARGIN: i32 = 6;
/// How fast the bar opens up and closes, as springs' rates: it gets out of the way quicker
/// than it comes.
const BAR_OPEN_RATE: f64 = 30.0;
const BAR_CLOSE_RATE: f64 = 45.0;
/// How little the bar's spring is damped as it opens: it swings past and back, as a
/// phone's Dock does coming in.
const BAR_OPEN_DAMPING: f64 = 0.55;
/// How long the applications come in before the bar starts to open up over them.
const BAR_AFTER_APPS: Duration = Duration::from_millis(150);
/// How long the menu bar's content takes to slide up off the screen as the overview opens;
/// the desktops' names come in over its second half.
const MENU_SLIDE: Duration = Duration::from_millis(160);
/// How long the bar closes before the applications start to leave, by when most of it has.
const APPS_AFTER_BAR: Duration = Duration::from_millis(80);
const CLOSE_SIZE: i32 = 22;
/// How far the pointer moves with a button down before a click becomes a drag.
const DRAG_THRESHOLD: f64 = 6.0;
/// How wide a window being dragged gets over the desktops' bar, to show where it goes.
const DRAGGED_OVER_BAR_WIDTH: i32 = 160;
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
    /// Where it sits in the overview, and where when the desktops' bar is opened up and
    /// the windows make room for it.
    to: Rectangle<i32, Logical>,
    to_open: Rectangle<i32, Logical>,
    /// Where it is drawn right now, between the two.
    pub now: Rectangle<i32, Logical>,
    /// The application's icon and name.
    pub badge: Option<Picture>,
}

#[derive(Clone, Copy)]
pub(crate) enum BarAction {
    Switch(usize),
    /// A desktop's name under its picture, which is renamed where it is, as on Windows.
    Rename(usize),
    Add,
}

/// A desktop's name being typed in the bar.
pub(crate) struct Naming {
    index: usize,
    text: String,
    /// Whether the name is still all chosen, as when renaming starts, so that typing
    /// replaces it, as on Windows.
    chosen: bool,
    /// What the input method is composing, and what it offers to choose from.
    preedit: String,
    pub ime_focused: bool,
    candidates: Option<(crate::ime::Candidates, Picture)>,
    candidates_id: Id,
}

impl Naming {
    fn insert(&mut self, typed: &str) {
        if std::mem::take(&mut self.chosen) {
            self.text.clear();
        }
        self.text.push_str(typed);
    }
}

/// A desktop being dragged along the opened-up bar to another place.
struct DesktopDrag {
    index: usize,
    start: Point<f64, Logical>,
    /// How far from its picture's left edge it was taken, and where the pointer is now.
    grab: f64,
    x: f64,
    moving: bool,
}

pub struct BarItem {
    pub rect: Rectangle<i32, Logical>,
    action: BarAction,
}

/// A window being dragged in the overview, by the slot it is in.
struct Drag {
    slot: usize,
    start: Point<f64, Logical>,
    /// Where in the window it was taken, as a fraction of its size.
    grab: (f64, f64),
    moving: bool,
}

/// The desktops' bar, which takes the menu bar's place in the overview: closed, a strip of
/// their names as tall as the menu bar; opened up, the same glass grown down over pictures
/// of them. And where things are on each.
#[derive(Default)]
struct SpacesBar {
    /// The screen the bar is on.
    area: Rectangle<i32, Logical>,
    /// The names, without the glass under them.
    strip: Option<Picture>,
    /// The glass's tint, as tall as the bar opened up and shown as far as it has come.
    expanded: Option<Picture>,
    /// Nothing, to put the bar's glass under: its tint is drawn apart from it, short of
    /// the menu bar's content while that is still sliding away with a tint of its own.
    clear: Option<Picture>,
    strip_items: Vec<BarItem>,
    expanded_items: Vec<BarItem>,
    /// Where each desktop's live picture goes when the bar is opened up.
    thumbnails: Vec<Rectangle<i32, Logical>>,
    /// What goes around each: the ring of the one shown, and its name under it.
    frames: Vec<Picture>,
    /// The add button, which shows the wallpaper under its plus, and the plus.
    add: Rectangle<i32, Logical>,
    plus: Option<Picture>,
    /// The button that takes a desktop away, shown on the one the pointer is on.
    close: Option<Picture>,
    /// How far along the row of pictures is scrolled, when they do not all fit.
    scroll: f64,
    /// How far along it can be.
    scroll_end: f64,
}

struct BarColors {
    ink: (f64, f64, f64),
    on_ink: (f64, f64, f64),
    tint: (f64, f64, f64, f64),
}

impl BarColors {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                ink: (0.945, 0.937, 0.918),
                on_ink: (0.090, 0.090, 0.102),
                tint: crate::glass::tint(dark, Tier::Bar),
            }
        } else {
            Self {
                ink: (0.090, 0.090, 0.102),
                on_ink: (0.984, 0.980, 0.973),
                tint: crate::glass::tint(dark, Tier::Bar),
            }
        }
    }

    fn paint_glass(&self, cr: &cairo::Context) {
        cr.set_source_rgba(self.tint.0, self.tint.1, self.tint.2, self.tint.3);
        let _ = cr.paint();
    }

    fn fill_pill(&self, cr: &cairo::Context, rect: Rectangle<i32, Logical>) {
        text::rounded_rectangle(
            cr,
            f64::from(rect.loc.x),
            f64::from(rect.loc.y),
            f64::from(rect.size.w),
            f64::from(rect.size.h),
            6.0,
        );
        cr.set_source_rgb(self.ink.0, self.ink.1, self.ink.2);
        let _ = cr.fill();
    }

    fn fill_tile(&self, cr: &cairo::Context, rect: Rectangle<i32, Logical>) {
        text::rounded_rectangle(
            cr,
            f64::from(rect.loc.x),
            f64::from(rect.loc.y),
            f64::from(rect.size.w),
            f64::from(rect.size.h),
            6.0,
        );
        cr.set_source_rgba(self.ink.0, self.ink.1, self.ink.2, 0.10);
        let _ = cr.fill();
    }

    fn ring(&self, cr: &cairo::Context, rect: Rectangle<i32, Logical>) {
        text::rounded_rectangle(
            cr,
            f64::from(rect.loc.x) - 3.0,
            f64::from(rect.loc.y) - 3.0,
            f64::from(rect.size.w) + 6.0,
            f64::from(rect.size.h) + 6.0,
            8.0,
        );
        cr.set_source_rgb(self.ink.0, self.ink.1, self.ink.2);
        cr.set_line_width(2.5);
        let _ = cr.stroke();
    }

    /// A desktop's name being typed: in a field, with the caret after it.
    fn field(&self, cr: &cairo::Context, rect: Rectangle<i32, Logical>, typed: &str, chosen: bool, font: &pango::FontDescription) {
        text::rounded_rectangle(
            cr,
            f64::from(rect.loc.x) + 0.5,
            f64::from(rect.loc.y) + 0.5,
            f64::from(rect.size.w) - 1.0,
            f64::from(rect.size.h) - 1.0,
            4.0,
        );
        cr.set_source_rgba(self.ink.0, self.ink.1, self.ink.2, 0.10);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(self.ink.0, self.ink.1, self.ink.2);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        let inner = Rectangle::new(rect.loc + Point::from((6, 0)), rect.size - Size::from((14, 0)));
        let width = text_width(typed, font).min(inner.size.w);
        if chosen {
            cr.rectangle(
                f64::from(inner.loc.x + (inner.size.w - width) / 2) - 2.0,
                f64::from(rect.loc.y) + 3.0,
                f64::from(width) + 4.0,
                f64::from(rect.size.h) - 6.0,
            );
            cr.set_source_rgba(self.ink.0, self.ink.1, self.ink.2, 0.25);
            let _ = cr.fill();
            cr.set_source_rgb(self.ink.0, self.ink.1, self.ink.2);
        }
        show_centered(cr, typed, font, inner, self.ink);
        let caret = f64::from(inner.loc.x + (inner.size.w + width) / 2) + 1.5;
        cr.move_to(caret, f64::from(rect.loc.y) + 4.0);
        cr.line_to(caret, f64::from(rect.loc.y + rect.size.h) - 4.0);
        let _ = cr.stroke();
    }

    fn draw_plus(&self, cr: &cairo::Context, rect: Rectangle<i32, Logical>, size: f64) {
        let (x, y) = (
            f64::from(rect.loc.x) + f64::from(rect.size.w) / 2.0,
            f64::from(rect.loc.y) + f64::from(rect.size.h) / 2.0,
        );
        cr.set_source_rgb(self.ink.0, self.ink.1, self.ink.2);
        cr.set_line_width(1.75);
        cr.move_to(x - size / 2.0, y);
        cr.line_to(x + size / 2.0, y);
        cr.move_to(x, y - size / 2.0);
        cr.line_to(x, y + size / 2.0);
        let _ = cr.stroke();
    }
}

/// Where a desktop's close button sits: over the top left corner of its picture.
fn close_button_rect(thumbnail: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        thumbnail.loc - Point::from((CLOSE_SIZE / 3, CLOSE_SIZE / 3)),
        Size::from((CLOSE_SIZE, CLOSE_SIZE)),
    )
}

/// How wide a desktop's picture is in the opened-up bar with room for it.
fn full_width(screen_width: i32) -> i32 {
    (screen_width / THUMBNAIL_SHARE).max(1)
}

/// The narrowest the desktops' pictures get to fit more along the bar; past that, it scrolls.
fn least_width(screen_width: i32) -> i32 {
    full_width(screen_width) * 5 / 8
}

/// How high the line of a desktop's name is under its picture: larger on a screen wider
/// than 1280, by three fifths of what the screen is.
fn label_height(screen_width: i32) -> i32 {
    (LABEL_HEIGHT * (2 * 1280 + 3 * screen_width) / (5 * 1280)).max(LABEL_HEIGHT)
}

fn thumbnail_height(output: Rectangle<i32, Logical>) -> i32 {
    (full_width(output.size.w) * output.size.h / output.size.w.max(1)).max(1)
}

/// How tall the desktops' bar is opened up, from the top of the screen.
pub(crate) fn expanded_height(output: Rectangle<i32, Logical>) -> i32 {
    MENU_BAR_HEIGHT + EXPANDED_PADDING * 2 + thumbnail_height(output) + label_height(output.size.w)
}

fn text_width(text: &str, font: &pango::FontDescription) -> i32 {
    let Ok(surface) = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1) else {
        return 0;
    };
    let Ok(cr) = cairo::Context::new(&surface) else {
        return 0;
    };
    let layout = text::new_layout(&cr);
    layout.set_font_description(Some(font));
    layout.set_text(text);
    layout.pixel_size().0
}

fn show_centered(
    cr: &cairo::Context,
    text: &str,
    font: &pango::FontDescription,
    rect: Rectangle<i32, Logical>,
    color: (f64, f64, f64),
) {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(font));
    layout.set_text(text);
    layout.set_width(rect.size.w * pango::SCALE);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    let (width, height) = layout.pixel_size();
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.move_to(
        f64::from(rect.loc.x) + f64::from(rect.size.w - width) / 2.0,
        f64::from(rect.loc.y) + f64::from(rect.size.h - height) / 2.0,
    );
    pangocairo::functions::show_layout(cr, &layout);
}

/// How the opened-up bar lays out `count` desktops on a screen `screen_width` wide: as wide
/// as each picture is, where the first one starts, and how far along the row is scrolled,
/// out of how far it can be. They shrink to fit left of the add button, down to a width;
/// more than fit then scroll, at first as far as shows the one shown, `active`.
fn bar_row(screen_width: i32, count: usize, active: usize, scroll: Option<f64>) -> (i32, i32, f64, f64) {
    let count = count.max(1) as i32;
    let right = screen_width - add_width(screen_width) - ADD_MARGIN - THUMBNAIL_GAP;
    let room = right - BAR_EDGE;
    let fitted = (room - THUMBNAIL_GAP * (count - 1)) / count;
    let width = fitted.clamp(least_width(screen_width), full_width(screen_width));
    let row = (width + THUMBNAIL_GAP) * count - THUMBNAIL_GAP;
    if row <= room {
        return (width, ((screen_width - row) / 2).min(right - row), 0.0, 0.0);
    }
    let end = f64::from(row - room);
    let centered = f64::from((width + THUMBNAIL_GAP) * active as i32 + width / 2 - room / 2);
    let scroll = scroll.unwrap_or(centered).clamp(0.0, end);
    (width, BAR_EDGE - scroll.round() as i32, scroll, end)
}

/// The add button is a bit more than half a full-size desktop, and keeps that size however
/// small the desktops get.
fn add_width(screen_width: i32) -> i32 {
    (f64::from(full_width(screen_width)) * ADD_SIZE).round() as i32
}

/// Where the strip's row of names `row` wide starts on a screen `screen_width` wide:
/// centered, or, too long for that, as far along as shows the one shown, which is `at`
/// along the row and `width` wide.
fn strip_start(screen_width: i32, row: i32, at: i32, width: i32) -> i32 {
    // Clear of the add button at the right end, and as far from the left.
    let edge = MENU_BAR_HEIGHT + 16;
    if row <= screen_width - edge * 2 {
        return (screen_width - row) / 2;
    }
    let centered = screen_width / 2 - at - width / 2;
    centered.clamp(screen_width - edge - row, edge)
}

/// A wallpaper filling a desktop's picture `width` by `height`, cut to it as swaybg's fill
/// mode does.
fn wallpaper_tile(path: &str, width: i32, height: i32) -> Option<cairo::ImageSurface> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(crate::config::expand_home(path), -1, height * 2, true)
        .inspect_err(|err| tracing::warn!("{path}: {err}"))
        .ok()?;
    let picture = text::pixbuf_surface(&pixbuf, None)?;
    let tile = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    let cr = cairo::Context::new(&tile).ok()?;
    let scale = (f64::from(width) / f64::from(picture.width())).max(f64::from(height) / f64::from(picture.height()));
    cr.translate(
        (f64::from(width) - f64::from(picture.width()) * scale) / 2.0,
        (f64::from(height) - f64::from(picture.height()) * scale) / 2.0,
    );
    cr.scale(scale, scale);
    cr.set_source_surface(&picture, 0.0, 0.0).ok()?;
    cr.paint().ok()?;
    drop(cr);
    Some(tile)
}

/// The pictures offered as a desktop's own wallpaper, from where Settings finds them.
fn wallpaper_choices() -> Vec<String> {
    const DIRS: [&str; 3] = ["~/.local/share/backgrounds", "~/Pictures/Wallpapers", "/usr/share/backgrounds"];
    const MOST: usize = 12;
    let mut found = Vec::new();
    for dir in DIRS {
        let Ok(entries) = std::fs::read_dir(crate::config::expand_home(dir)) else {
            continue;
        };
        let mut pictures: Vec<String> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| matches!(extension.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp"))
            })
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        pictures.sort();
        found.extend(pictures);
    }
    found.truncate(MOST);
    found
}

/// A picture drawn by `paint` on a surface of that size.
fn draw(width: i32, height: i32, paint: impl FnOnce(&cairo::Context)) -> Option<Picture> {
    let surface = text::canvas(width.max(1), height.max(1)).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        paint(&cr);
    }
    text::picture(surface)
}

pub struct Overview {
    started: Instant,
    pub(crate) closing: bool,
    /// The windows, bottom of the stack first.
    pub slots: Vec<Slot>,
    pub dim: SolidColorBuffer,
    bar: SpacesBar,
    /// How far the desktops' bar is opened up, 0 to 1, how fast, and whether it is opening.
    expansion: f64,
    expansion_speed: f64,
    /// The same as it is drawn: a little past 1 while the bar swings at the end of its way.
    swing: f64,
    /// How much the desktops' names showed on the strip when the overview began to close,
    /// so that they only fade from there and do not come back as the bar closes.
    names_at_close: f64,
    /// How far the menu bar's content is off the screen, set once a frame: what is drawn
    /// by it is cut at one height, which reading the clock again for each would not give.
    menu_away: f64,
    expanding: bool,
    last_tick: Instant,
    strip_id: Id,
    expanded_id: Id,
    glass_id: Id,
    /// Where each desktop is along the opened-up bar on its way to its place, how fast,
    /// and what names its frame from one frame to the next.
    glides: Vec<(f64, f64, Id)>,
    drag: Option<Drag>,
    desktop_drag: Option<DesktopDrag>,
    /// How far along the bar's pictures are scrolled, once scrolled; till then, as far as
    /// shows the desktop shown.
    bar_scroll: Option<f64>,
    /// The desktop being renamed, and what is typed for it so far.
    pub(crate) renaming: Option<Naming>,
    /// The wallpapers of desktops that have their own, drawn at a width, as their pictures
    /// in the bar show them: read once while the overview is open.
    wallpaper_tiles: std::cell::RefCell<std::collections::HashMap<(String, i32), Option<cairo::ImageSurface>>>,
    /// The desktop in the opened-up bar the pointer is on, which shows its close button.
    hovered_desktop: Option<usize>,
    /// The slot under the pointer.
    pub hovered: Option<usize>,
    /// The four sides of the outline around it.
    pub outline: [Id; 4],
    /// The applications, shown in place of the windows.
    pub apps: Option<AppGrid>,
    /// Whether the overview came up with the applications, whose own glass covers the
    /// wallpaper: it is then not dimmed under them, which would blur it anew every frame.
    pub(crate) under_apps: bool,
    /// Only this application's windows, as the Dock's 顯示所有視窗 asks.
    only: Option<String>,
}

impl Overview {
    /// The slot of the window being dragged, which is drawn over the others.
    pub fn dragged(&self) -> Option<usize> {
        self.drag
            .as_ref()
            .filter(|drag| drag.moving)
            .map(|drag| drag.slot)
    }

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

impl Nekoland {
    pub fn toggle_overview(&mut self) {
        match &self.overview {
            Some(overview) if overview.closing => {}
            Some(_) => self.close_overview(None),
            None => self.open_overview(None),
        }
    }

    /// The overview of only `app_id`'s windows, as the Mac's App Exposé.
    pub fn show_app_windows(&mut self, app_id: &str) {
        if self.overview.is_none() {
            self.open_overview(Some(app_id.to_string()));
        }
    }

    fn open_overview(&mut self, only: Option<String>) {
        self.close_menu();
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
            bar: SpacesBar::default(),
            expansion: 0.0,
            expansion_speed: 0.0,
            swing: 0.0,
            names_at_close: 1.0,
            menu_away: 0.0,
            expanding: false,
            last_tick: Instant::now(),
            strip_id: Id::new(),
            expanded_id: Id::new(),
            glass_id: Id::new(),
            glides: Vec::new(),
            drag: None,
            desktop_drag: None,
            bar_scroll: None,
            renaming: None,
            wallpaper_tiles: Default::default(),
            hovered_desktop: None,
            hovered: None,
            outline: [Id::new(), Id::new(), Id::new(), Id::new()],
            apps: None,
            under_apps: false,
            only,
        });
        self.rebuild_overview();
        // Nothing has the keyboard in the overview; what is typed starts a search.
        self.set_keyboard_focus(None);
    }

    /// Lays the overview out again for the desktop that is shown now.
    pub fn rebuild_overview(&mut self) {
        let Some(main) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(output) = self.space.output_geometry(&main) else {
            return;
        };
        // Under the bar, above whatever a dock keeps free at the bottom.
        let bottom = self
            .usable_area_on(&main)
            .map_or(output.loc.y + output.size.h, |area| area.loc.y + area.size.h);
        let area_under = |top: i32| {
            Rectangle::new(
                Point::from((output.loc.x + MARGIN, top + MARGIN / 2)),
                Size::from((output.size.w - MARGIN * 2, bottom - top - MARGIN)),
            )
        };
        let area = area_under(output.loc.y + MENU_BAR_HEIGHT + BAR_GAP);
        let open_area = area_under(output.loc.y + expanded_height(output) + BAR_GAP);
        let only = self.overview.as_ref().and_then(|overview| overview.only.clone());

        let windows: Vec<(Window, Rectangle<i32, Logical>)> = self
            .space
            .elements()
            .filter(|window| is_managed(window))
            .filter(|window| {
                only.as_deref()
                    .is_none_or(|app_id| window_names(window).1.eq_ignore_ascii_case(app_id))
            })
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
        // The padded rectangle back to the window's own proportions.
        let unpad = |from: Rectangle<i32, Logical>, padded: Rectangle<i32, Logical>| {
            let scale = f64::from(padded.size.w) / f64::from(from.size.w.max(1));
            let height = (f64::from(from.size.h) * scale).round() as i32;
            Rectangle::new(padded.loc, Size::from((padded.size.w, height)))
        };
        let slots = windows
            .into_iter()
            .zip(spread(&rects, area).into_iter().zip(spread(&rects, open_area)))
            .map(|((window, from), (padded, padded_open))| {
                let to = unpad(from, padded);
                let (title, app_id) = window_names(&window);
                Slot {
                    badge: self.apps.badge(&app_id, &title, ICON_SIZE),
                    window,
                    from,
                    to,
                    to_open: unpad(from, padded_open),
                    now: to,
                }
            })
            .collect();

        let bar = self.spaces_bar(output);
        if let Some(overview) = &mut self.overview {
            overview.slots = slots;
            // A desktop more comes out of the add button; the first ones are in place.
            let first = overview.glides.is_empty();
            overview.glides.truncate(bar.thumbnails.len());
            for place in bar.thumbnails.iter().skip(overview.glides.len()) {
                let from = if first { place.loc.x } else { bar.add.loc.x };
                overview.glides.push((f64::from(from), 0.0, Id::new()));
            }
            overview.bar = bar;
            overview.hovered = None;
            overview.drag = None;
            overview.hovered_desktop = None;
        }
        self.tick_overview();
    }

    /// The desktops' bar, as on the Mac: a strip naming the desktops, the current one
    /// marked, with the add button at the right end; and the same bar opened up, each
    /// desktop shown small as it is, the current one ringed, named under its picture.
    fn spaces_bar(&self, output: Rectangle<i32, Logical>) -> SpacesBar {
        let dark = self.menu_bar_dark();
        let colors = BarColors::new(dark);
        let count = self.workspaces.count();
        let active = self.workspaces.active;
        let names: Vec<String> = (0..count).map(|index| self.workspaces.name(index)).collect();
        let renaming = (self.overview.as_ref())
            .and_then(|overview| overview.renaming.as_ref())
            .map(|naming| match naming.preedit.is_empty() {
                true => (naming.index, naming.text.clone(), naming.chosen),
                // What is being composed replaces a chosen name, as typing would.
                false if naming.chosen => (naming.index, naming.preedit.clone(), false),
                false => (naming.index, format!("{}{}", naming.text, naming.preedit), false),
            });
        let scroll = self.overview.as_ref().and_then(|overview| overview.bar_scroll);
        let (tile_width, first, scroll, scroll_end) = bar_row(output.size.w, count, active, scroll);
        let tile_height = (tile_width * output.size.h / output.size.w.max(1)).max(1);
        let expanded_height = expanded_height(output);
        let font = text::font("Medium 10");

        // The strip, where the menu bar was: names in a row, centered.
        let widths: Vec<i32> = names
            .iter()
            .map(|name| text_width(name, &font) + NAME_PADDING * 2)
            .collect();
        let row: i32 = widths.iter().sum::<i32>() + NAME_GAP * (count as i32 - 1).max(0);
        let active_at: i32 = widths.iter().take(active).map(|width| width + NAME_GAP).sum();
        let active_width = widths.get(active).copied().unwrap_or(0);
        let mut strip_items = Vec::with_capacity(count + 1);
        let strip = draw(output.size.w, MENU_BAR_HEIGHT, |cr| {
            let mut x = strip_start(output.size.w, row, active_at, active_width);
            for (index, (name, width)) in names.iter().zip(&widths).enumerate() {
                let pill = Rectangle::new(
                    Point::from((x, (MENU_BAR_HEIGHT - NAME_PILL) / 2)),
                    Size::from((*width, NAME_PILL)),
                );
                let ink = if index == active {
                    colors.fill_pill(cr, pill);
                    colors.on_ink
                } else {
                    colors.ink
                };
                show_centered(cr, name, &font, pill, ink);
                strip_items.push(BarItem {
                    rect: Rectangle::new(output.loc + pill.loc, pill.size),
                    action: BarAction::Switch(index),
                });
                x += width + NAME_GAP;
            }
            let plus = Rectangle::new(
                Point::from((output.size.w - MENU_BAR_HEIGHT - 8, 0)),
                Size::from((MENU_BAR_HEIGHT, MENU_BAR_HEIGHT)),
            );
            colors.draw_plus(cr, plus, 12.0);
            strip_items.push(BarItem {
                rect: Rectangle::new(output.loc + plus.loc, plus.size),
                action: BarAction::Add,
            });
        });

        // Opened up: the pictures with their names, each centered top to bottom, and the add
        // button at the right end, a smaller picture of the wallpaper under a plus.
        let label_height = label_height(output.size.w);
        // The names under the pictures, as much larger as their line is.
        let label_font = text::font(&format!("Medium {}", 10 * label_height / LABEL_HEIGHT));
        let top = (expanded_height - tile_height - label_height) / 2;
        let mut thumbnails = Vec::with_capacity(count);
        let mut expanded_items = Vec::with_capacity(count + 1);
        let mut frames = Vec::with_capacity(count);
        // With room under it for the bar to swing past its height as it opens.
        let expanded = draw(output.size.w, expanded_height + expanded_height / 4, |cr| {
            colors.paint_glass(cr);
        });
        // A desktop with a wallpaper other than the one drawn now shows its own picture of
        // it, which its frame carries, under its windows.
        let own_wallpapers: Vec<Option<cairo::ImageSurface>> = (0..count)
            .map(|index| {
                let path = self.overview_own_wallpaper(index)?;
                let overview = self.overview.as_ref()?;
                let mut tiles = overview.wallpaper_tiles.borrow_mut();
                tiles
                    .entry((path.clone(), tile_width))
                    .or_insert_with(|| wallpaper_tile(&path, tile_width, tile_height))
                    .clone()
            })
            .collect();
        let mut x = first;
        for (index, name) in names.iter().enumerate() {
            let tile = Rectangle::new(Point::from((x, top)), Size::from((tile_width, tile_height)));
            let label = Rectangle::new(
                Point::from((x, top + tile_height + 4)),
                Size::from((tile_width, label_height - 4)),
            );
            // Behind the live picture, which is drawn over it.
            let on_frame = Rectangle::new(Point::from((FRAME_MARGIN, FRAME_MARGIN)), tile.size);
            frames.extend(draw(
                tile_width + FRAME_MARGIN * 2,
                tile_height + label_height + FRAME_MARGIN * 2,
                |cr| {
                    colors.fill_tile(cr, on_frame);
                    if let Some(wallpaper) = &own_wallpapers[index] {
                        let _ = cr.save();
                        text::rounded_rectangle(
                            cr,
                            f64::from(on_frame.loc.x),
                            f64::from(on_frame.loc.y),
                            f64::from(on_frame.size.w),
                            f64::from(on_frame.size.h),
                            6.0,
                        );
                        cr.clip();
                        let _ = cr.set_source_surface(wallpaper, f64::from(on_frame.loc.x), f64::from(on_frame.loc.y));
                        let _ = cr.paint();
                        let _ = cr.restore();
                    }
                    if index == active {
                        colors.ring(cr, on_frame);
                    }
                    let on_label = Rectangle::new(
                        Point::from((FRAME_MARGIN, FRAME_MARGIN + tile_height + 4)),
                        label.size,
                    );
                    match renaming.as_ref().filter(|(at, ..)| *at == index) {
                        Some((_, typed, chosen)) => colors.field(cr, on_label, typed, *chosen, &label_font),
                        None => show_centered(cr, name, &label_font, on_label, colors.ink),
                    }
                },
            ));
            thumbnails.push(Rectangle::new(output.loc + tile.loc, tile.size));
            expanded_items.push(BarItem {
                rect: Rectangle::new(output.loc + tile.loc, tile.size),
                action: BarAction::Switch(index),
            });
            expanded_items.push(BarItem {
                rect: Rectangle::new(output.loc + label.loc, label.size),
                action: BarAction::Rename(index),
            });
            x += tile_width + THUMBNAIL_GAP;
        }
        let full_height = thumbnail_height(output);
        let add_size = Size::from((add_width(output.size.w), (f64::from(full_height) * ADD_SIZE).round() as i32));
        let add = Rectangle::new(
            output.loc + Point::from((output.size.w - add_size.w - ADD_MARGIN, top + (tile_height - add_size.h) / 2)),
            add_size,
        );
        let plus = draw(add.size.w, add.size.h, |cr| {
            cr.set_source_rgba(0.0, 0.0, 0.0, ADD_DIM);
            let _ = cr.paint();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(1.75);
            let (x, y, arm) = (f64::from(add.size.w) / 2.0, f64::from(add.size.h) / 2.0, 9.0);
            cr.move_to(x - arm, y);
            cr.line_to(x + arm, y);
            cr.move_to(x, y - arm);
            cr.line_to(x, y + arm);
            let _ = cr.stroke();
        });
        // First, over desktops scrolled under it.
        expanded_items.insert(0, BarItem {
            rect: add,
            action: BarAction::Add,
        });

        // A dark disc with a cross, as the Mac shows on a desktop to take it away.
        let close = draw(CLOSE_SIZE, CLOSE_SIZE, |cr| {
            let radius = f64::from(CLOSE_SIZE) / 2.0;
            cr.arc(radius, radius, radius - 0.5, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
            let _ = cr.fill();
            cr.set_source_rgb(colors.on_ink.0, colors.on_ink.1, colors.on_ink.2);
            cr.set_line_width(1.75);
            let arm = radius * 0.4;
            cr.move_to(radius - arm, radius - arm);
            cr.line_to(radius + arm, radius + arm);
            cr.move_to(radius + arm, radius - arm);
            cr.line_to(radius - arm, radius + arm);
            let _ = cr.stroke();
        });

        SpacesBar {
            area: output,
            clear: draw(1, 1, |_| {}),
            strip,
            expanded,
            strip_items,
            expanded_items,
            thumbnails,
            frames,
            add,
            plus,
            close,
            scroll,
            scroll_end,
        }
    }

    /// How far the desktops' bar is opened up, 0 to 1.
    pub fn overview_expansion(&self) -> f64 {
        self.overview.as_ref().map_or(0.0, |overview| overview.expansion)
    }

    /// Where each desktop's live picture is in the opened-up bar, on its way to its place:
    /// along the bar as desktops come and go, and down from above the screen as it opens up.
    pub fn overview_thumbnails(&self) -> Vec<(usize, Rectangle<i32, Logical>)> {
        self.overview.as_ref().map_or_else(Vec::new, |overview| {
            let away = 1.0 - overview.swing;
            let dragged = overview.desktop_drag.as_ref().filter(|drag| drag.moving);
            (overview.bar.thumbnails.iter().zip(&overview.glides).enumerate())
                .map(|(index, (place, (x, ..)))| {
                    let above = place.loc.y + place.size.h + label_height(overview.bar.area.size.w) + FRAME_MARGIN - overview.bar.area.loc.y;
                    let y = place.loc.y - (f64::from(above) * away).round() as i32;
                    let x = match dragged.filter(|drag| drag.index == index) {
                        Some(drag) => drag.x - drag.grab,
                        None => *x,
                    };
                    (index, Rectangle::new(Point::from((x.round() as i32, y)), place.size))
                })
                .collect()
        })
    }

    /// Where the add button shows the wallpaper, coming in from the right as the bar opens
    /// up, and the plus that goes over it.
    pub fn overview_add_button(&self) -> Option<(Rectangle<i32, Logical>, &Picture)> {
        let overview = self.overview.as_ref()?;
        let bar = &overview.bar;
        let right = bar.area.loc.x + bar.area.size.w;
        let away = f64::from(right - bar.add.loc.x) * (1.0 - overview.swing);
        let rect = Rectangle::new(bar.add.loc + Point::from((away.round() as i32, 0)), bar.add.size);
        Some((rect, bar.plus.as_ref()?))
    }

    /// Where the desktops' row ends, short of the add button, when the bar is opened up.
    pub fn overview_row_end(&self) -> Option<i32> {
        let add = self.overview.as_ref()?.bar.add;
        Some(add.loc.x - THUMBNAIL_GAP / 2)
    }

    /// The close button on the desktop the pointer is on, and where it is.
    pub fn overview_close_button(&self) -> Option<(Rectangle<i32, Logical>, &Picture)> {
        let overview = self.overview.as_ref()?;
        let index = overview.hovered_desktop?;
        let picture = overview.bar.close.as_ref()?;
        let (_, rect) = self
            .overview_thumbnails()
            .into_iter()
            .find(|(at, _)| *at == index)?;
        Some((close_button_rect(rect), picture))
    }

    /// Where the desktops' bar is right now: as tall as the menu bar closed, growing down
    /// as it opens up.
    pub fn overview_panel(&self) -> Option<Rectangle<i32, Logical>> {
        let overview = self.overview.as_ref()?;
        let output = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))?;
        let grown = f64::from(expanded_height(output) - MENU_BAR_HEIGHT) * overview.swing;
        Some(Rectangle::new(
            output.loc,
            Size::from((output.size.w, MENU_BAR_HEIGHT + grown.round() as i32)),
        ))
    }

    /// How far the menu bar's content has gone up off the screen, 0 to 1: away as the
    /// overview opens, and back as it ends, once the desktops' names have gone.
    pub fn overview_menu_away(&self) -> f64 {
        self.overview.as_ref().map_or(0.0, |overview| overview.menu_away)
    }

    /// The desktops' bar: the menu bar's pane of glass grown down, the names fading out on
    /// it, and the desktops' frames coming down with their pictures.
    pub fn overview_overlays(&self) -> Vec<Overlay<'_>> {
        let (Some(overview), Some(panel)) = (&self.overview, self.overview_panel()) else {
            return Vec::new();
        };
        // The applications shown other than in the overview cover it whole.
        if overview
            .apps
            .as_ref()
            .is_some_and(|apps| apps.style() != crate::config::AppsView::Overview)
        {
            return Vec::new();
        }
        let expansion = overview.expansion;
        let mut overlays = Vec::new();
        // What the input method offers for a name being typed, under its field.
        if let Some((naming, picture)) = overview
            .renaming
            .as_ref()
            .and_then(|naming| Some((naming, &naming.candidates.as_ref()?.1)))
        {
            let field = overview.bar.expanded_items.iter().find(|item| matches!(item.action, BarAction::Rename(index) if index == naming.index));
            if let Some(field) = field {
                let right = overview.bar.area.loc.x + overview.bar.area.size.w - picture.size.w;
                let x = (field.rect.loc.x + field.rect.size.w / 2 - picture.size.w / 2).clamp(overview.bar.area.loc.x, right);
                overlays.push(Overlay {
                    fade: 1.0,
                    id: &naming.candidates_id,
                    rect: Rectangle::new(Point::from((x, field.rect.loc.y + field.rect.size.h + 4)), picture.size),
                    glass: None,
                    alpha: 1.0,
                    picture,
                });
            }
        }
        // Each desktop's ring and name, where its picture is, cut off where the bar ends.
        let shown = panel.loc.y + panel.size.h;
        let row_end = self.overview_row_end().unwrap_or(i32::MAX);
        for ((_, place), (picture, (.., id))) in
            (self.overview_thumbnails().into_iter()).zip(overview.bar.frames.iter().zip(&overview.glides))
        {
            let loc = place.loc - Point::from((FRAME_MARGIN, FRAME_MARGIN));
            let height = picture.size.h.min(shown - loc.y);
            let width = picture.size.w.min(row_end - loc.x);
            if expansion > 0.0 && height > 0 && width > 0 {
                overlays.push(Overlay {
                    fade: 1.0,
                    id,
                    rect: Rectangle::new(loc, Size::from((width, height))),
                    glass: None,
                    alpha: expansion as f32,
                    picture,
                });
            }
        }
        // Over the applications the bar only opens up and closes: the names are not shown.
        let names = if overview.apps.is_some() {
            0.0
        } else if overview.closing {
            // Gone by halfway, when the menu bar's content starts back.
            overview.names_at_close * (overview.progress() * 2.0 - 1.0).max(0.0)
        } else {
            // In once the menu bar's content is halfway gone.
            let shown = overview.started.elapsed().as_secs_f64() / MENU_SLIDE.as_secs_f64() * 2.0 - 1.0;
            let shown = if self.config.animations { shown.clamp(0.0, 1.0) } else { 1.0 };
            (1.0 - expansion) * shown
        };
        if let Some(picture) = overview.bar.strip.as_ref().filter(|_| names > 0.0) {
            overlays.push(Overlay {
                fade: 1.0,
                id: &overview.strip_id,
                rect: panel,
                glass: None,
                alpha: names as f32,
                picture,
            });
        }
        // The menu bar's content brings its own tint while it is there.
        let menu = if self.config.menu_bar_background {
            // Cut where the menu bar's picture is, so that no line is left between them.
            MENU_BAR_HEIGHT - (f64::from(MENU_BAR_HEIGHT) * self.overview_menu_away()).round() as i32
        } else {
            0
        };
        if let Some(picture) = overview.bar.expanded.as_ref().filter(|_| panel.size.h > menu) {
            overlays.push(Overlay {
                fade: 1.0,
                id: &overview.expanded_id,
                rect: Rectangle::new(panel.loc + Point::from((0, menu)), panel.size - Size::from((0, menu))),
                glass: None,
                alpha: 1.0,
                picture,
            });
        }
        if let Some(picture) = &overview.bar.clear {
            overlays.push(Overlay {
                fade: 1.0,
                id: &overview.glass_id,
                rect: panel,
                glass: Some((Tier::Bar, 0)),
                alpha: 1.0,
                picture,
            });
        }
        overlays
    }

    /// What dropping something at a position on the opened-up bar would do.
    pub(crate) fn overview_bar_action(&self, pos: Point<f64, Logical>) -> Option<BarAction> {
        self.overview
            .as_ref()?
            .bar
            .expanded_items
            .iter()
            .find(|item| item.rect.to_f64().contains(pos))
            .map(|item| item.action)
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
        overview.names_at_close = 1.0 - overview.expansion;
        // The bar over the applications gets out of the way first, and they leave as it
        // nearly has: until then the overview is held where it is.
        let under_bar = (overview.apps.as_ref()).is_some_and(|apps| apps.style() == crate::config::AppsView::Overview);
        if under_bar && self.config.animations {
            overview.started += APPS_AFTER_BAR;
        }
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
        let output = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output));
        let desktops = self.workspaces.count();
        let thumbnails = self.overview_thumbnails();
        let Some(overview) = &mut self.overview else {
            return;
        };
        let progress = overview.progress();
        if overview.closing && progress <= 0.0 {
            self.overview = None;
            return;
        }
        overview.menu_away = if !self.config.animations {
            1.0
        } else if overview.closing {
            (progress * 2.0).min(1.0)
        } else {
            let t = (overview.started.elapsed().as_secs_f64() / MENU_SLIDE.as_secs_f64()).min(1.0);
            t * (2.0 - t)
        };
        // A window gone moves the ones after it up the list, where the drag and the
        // hovered slot are held by position: they are let go rather than point at another.
        let before = overview.slots.len();
        overview.slots.retain(|slot| slot.window.alive());
        if overview.slots.len() != before {
            overview.drag = None;
            overview.hovered = None;
        }
        let fade = if overview.under_apps { 0.0 } else { progress as f32 };
        overview.dim.set_color([0.0, 0.0, 0.0, DIM * fade]);

        let settled = !overview.closing && progress >= 1.0;
        let dragging = overview.drag.as_ref().is_some_and(|drag| drag.moving);
        // The bar opens up when the pointer comes onto the strip or a window is dragged, and
        // closes when the pointer leaves the opened-up bar.
        if let (Some(pos), Some(output)) = (pointer, output) {
            let y = pos.y - f64::from(output.loc.y);
            // With the applications shown in the overview, the bar stays open over them; shown
            // any other way, it is not there.
            let apps = overview.apps.as_ref();
            if overview.closing || apps.is_some_and(|apps| apps.style() != crate::config::AppsView::Overview) {
                overview.expanding = false;
            } else if dragging
                || overview.renaming.is_some()
                || overview.desktop_drag.as_ref().is_some_and(|drag| drag.moving)
                || (settled && y < f64::from(MENU_BAR_HEIGHT))
            {
                overview.expanding = true;
            } else if let Some(apps) = apps {
                // Over the applications it opens up once they have mostly come in.
                overview.expanding = apps.shown_for().is_some_and(|shown| shown >= BAR_AFTER_APPS);
            } else if y > f64::from(expanded_height(output) + 8) || !settled {
                overview.expanding = false;
            }
        }
        let now = Instant::now();
        let seconds = now.duration_since(overview.last_tick).as_secs_f64();
        overview.last_tick = now;
        let (goal, rate) = if overview.expanding {
            (1.0, BAR_OPEN_RATE)
        } else {
            (0.0, BAR_CLOSE_RATE)
        };
        if self.config.animations {
            // A frame late by much is not taken as a jump.
            let seconds = seconds.min(0.05);
            let (swing, speed) = (&mut overview.swing, &mut overview.expansion_speed);
            if overview.expanding {
                crate::motion::swing(swing, speed, goal, rate, BAR_OPEN_DAMPING, seconds);
            } else {
                crate::motion::settle(swing, speed, goal, rate, seconds);
            }
        }
        if !self.config.animations || ((overview.swing - goal).abs() < 0.002 && overview.expansion_speed.abs() < 0.05) {
            (overview.swing, overview.expansion_speed) = (goal, 0.0);
        }
        overview.swing = overview.swing.max(0.0);
        overview.expansion = overview.swing.min(1.0);

        for ((x, speed, _), place) in overview.glides.iter_mut().zip(&overview.bar.thumbnails) {
            let goal = f64::from(place.loc.x);
            if self.config.animations && seconds < 0.1 {
                crate::motion::settle(x, speed, goal, BAR_GLIDE_RATE, seconds);
            }
            if !self.config.animations || seconds >= 0.1 || ((*x - goal).abs() < 0.3 && speed.abs() < 1.0) {
                (*x, *speed) = (goal, 0.0);
            }
        }

        // The windows make room as the bar opens up; under the applications, whose glass
        // they show through blurred, they stay where they are.
        let open = overview.expansion;
        let spread = if overview.under_apps { 0.0 } else { progress };
        for slot in &mut overview.slots {
            let target = lerp(slot.to, slot.to_open, open);
            slot.now = lerp(slot.from, target, spread);
        }
        // A window being dragged follows the pointer, and shrinks over the bar.
        if let (Some(drag), Some(pos)) = (overview.drag.as_ref().filter(|drag| drag.moving), pointer) {
            if let Some(slot) = overview.slots.get_mut(drag.slot) {
                let over_bar =
                    output.is_some_and(|output| pos.y < f64::from(output.loc.y + expanded_height(output)));
                let size = if over_bar {
                    let height = DRAGGED_OVER_BAR_WIDTH * slot.to.size.h / slot.to.size.w.max(1);
                    Size::from((DRAGGED_OVER_BAR_WIDTH, height.max(1)))
                } else {
                    slot.now.size
                };
                let loc = Point::from((
                    pos.x - drag.grab.0 * f64::from(size.w),
                    pos.y - drag.grab.1 * f64::from(size.h),
                ));
                slot.now = Rectangle::new(loc.to_i32_round(), size);
            }
        }

        overview.hovered = pointer.filter(|_| settled && !dragging).and_then(|pos| {
            overview
                .slots
                .iter()
                .rposition(|slot| slot.now.to_f64().contains(pos))
        });
        // The desktop under the pointer in the opened-up bar shows its close button, when
        // there is another desktop for its windows to go to.
        overview.hovered_desktop = pointer
            .filter(|_| open >= 0.5 && desktops > 1 && !dragging && overview.desktop_drag.is_none())
            .and_then(|pos| {
                thumbnails
                    .iter()
                    .find(|(_, rect)| {
                        let reach = Rectangle::new(
                            rect.loc - Point::from((CLOSE_SIZE / 2, CLOSE_SIZE / 2)),
                            rect.size + Size::from((CLOSE_SIZE, CLOSE_SIZE)),
                        );
                        reach.to_f64().contains(pos)
                    })
                    .map(|(index, _)| *index)
            });
    }

    /// Whether a position is on the desktops' bar, which lies over a panel at the top.
    pub fn overview_covers(&self, pos: Point<f64, Logical>) -> bool {
        self.overview_panel()
            .is_some_and(|panel| pos.y < f64::from(panel.loc.y + panel.size.h))
    }

    /// A click in the overview: on a desktop's name or picture, the add button, a window, or nothing.
    pub fn overview_click(&mut self, pos: Point<f64, Logical>) {
        // A name being typed is taken as it is once anything else is clicked, as on Windows.
        self.finish_renaming(true);
        let Some(overview) = &self.overview else {
            return;
        };
        // The close button of a desktop, over the corner of its picture, comes first.
        if let (Some(index), Some((rect, _))) = (overview.hovered_desktop, self.overview_close_button()) {
            if rect.to_f64().contains(pos) {
                // The others close up over where it was.
                if let Some(overview) = self.overview.as_mut().filter(|overview| index < overview.glides.len()) {
                    overview.glides.remove(index);
                }
                self.remove_workspace(index);
                return;
            }
        }
        let items = if overview.expansion >= 0.5 {
            &overview.bar.expanded_items
        } else {
            &overview.bar.strip_items
        };
        // Off the bar, or with no bar, the applications take what they are shown over.
        let bar = overview
            .apps
            .as_ref()
            .is_none_or(|apps| apps.style() == crate::config::AppsView::Overview);
        if overview.apps.is_some() && !(bar && items.iter().any(|item| item.rect.to_f64().contains(pos))) {
            self.apps_press(pos);
            return;
        }
        let action = items
            .iter()
            .find(|item| item.rect.to_f64().contains(pos))
            .map(|item| item.action);
        let opened_up = overview.expansion >= 0.5;
        match action {
            // On the opened-up bar the press may become a drag of the desktop along it; let
            // go unmoved, it is picked then.
            Some(BarAction::Switch(index)) if opened_up => {
                let left = self
                    .overview_thumbnails()
                    .get(index)
                    .map_or(pos.x, |(_, rect)| f64::from(rect.loc.x));
                if let Some(overview) = &mut self.overview {
                    overview.desktop_drag = Some(DesktopDrag {
                        index,
                        start: pos,
                        grab: pos.x - left,
                        x: pos.x,
                        moving: false,
                    });
                }
            }
            Some(BarAction::Switch(index)) => self.pick_desktop(index),
            Some(BarAction::Rename(index)) => self.start_renaming(index),
            Some(BarAction::Add) => {
                self.add_workspace();
                self.rebuild_overview();
            }
            // On a window, the press may become a drag; it is picked when let go unmoved.
            None => match overview
                .slots
                .iter()
                .rposition(|slot| slot.now.to_f64().contains(pos))
            {
                Some(index) => {
                    let rect = overview.slots[index].now.to_f64();
                    let grab = (
                        (pos.x - rect.loc.x) / rect.size.w.max(1.0),
                        (pos.y - rect.loc.y) / rect.size.h.max(1.0),
                    );
                    if let Some(overview) = &mut self.overview {
                        overview.drag = Some(Drag {
                            slot: index,
                            start: pos,
                            grab,
                            moving: false,
                        });
                    }
                }
                None => self.close_overview(None),
            },
        }
    }

    /// As on the Mac, picking a desktop goes to it and leaves the overview, whose closing is
    /// the way there: the desktops do not slide past each other as well.
    fn pick_desktop(&mut self, index: usize) {
        self.show_workspace(index, false);
        self.close_overview(None);
    }

    /// The pointer moved with a window or a desktop held: far enough, and it is being
    /// dragged. A desktop dragged takes the place of the one it is over, which moves aside.
    pub fn overview_motion(&mut self, pos: Point<f64, Logical>) {
        self.apps_motion(pos);
        let Some(overview) = self.overview.as_mut() else {
            return;
        };
        if let Some(drag) = overview.desktop_drag.as_mut() {
            drag.x = pos.x;
            if !drag.moving && (pos.x - drag.start.x).hypot(pos.y - drag.start.y) > DRAG_THRESHOLD {
                drag.moving = true;
            }
            let from = drag.index;
            let over = (overview.bar.thumbnails.iter()).position(|place| {
                let reach = f64::from(THUMBNAIL_GAP) / 2.0;
                pos.x >= f64::from(place.loc.x) - reach && pos.x < f64::from(place.loc.x + place.size.w) + reach
            });
            if let Some(to) = over.filter(|&to| drag.moving && to != from) {
                drag.index = to;
                let glide = overview.glides.remove(from);
                overview.glides.insert(to, glide);
                self.move_workspace(from, to);
                self.refresh_overview_bar();
            }
        }
        if let Some(drag) = self.overview.as_mut().and_then(|overview| overview.drag.as_mut()) {
            let (dx, dy) = (pos.x - drag.start.x, pos.y - drag.start.y);
            if !drag.moving && dx.hypot(dy) > DRAG_THRESHOLD {
                drag.moving = true;
            }
        }
    }

    /// A button came up in the overview: a window let go unmoved is picked; one dragged
    /// onto a desktop goes there, onto the add button to a new desktop.
    pub fn overview_release(&mut self, pos: Point<f64, Logical>) {
        if let Some(drag) = self.overview.as_mut().and_then(|overview| overview.desktop_drag.take()) {
            if !drag.moving {
                self.pick_desktop(drag.index);
            }
            return;
        }
        if self.apps_shown() {
            self.apps_release(pos);
            return;
        }
        let Some(overview) = self.overview.as_mut() else {
            return;
        };
        let Some(drag) = overview.drag.take() else {
            return;
        };
        let Some(window) = overview.slots.get(drag.slot).map(|slot| slot.window.clone()) else {
            return;
        };
        if !drag.moving {
            self.close_overview(Some(window));
            return;
        }
        let target = overview
            .bar
            .expanded_items
            .iter()
            .find(|item| item.rect.to_f64().contains(pos))
            .map(|item| item.action);
        match target {
            Some(BarAction::Switch(index) | BarAction::Rename(index)) => self.move_to_workspace(&window, index),
            Some(BarAction::Add) => {
                let index = self.workspaces.count();
                self.add_workspace();
                self.move_to_workspace(&window, index);
            }
            // Let go anywhere else, it goes back to its place.
            None => return,
        }
        self.rebuild_overview();
    }
}

impl Nekoland {
    /// The wallpaper of a desktop, when it is not the one drawn now, which shows only the
    /// desktop shown.
    pub fn overview_own_wallpaper(&self, index: usize) -> Option<String> {
        let shown = self.desktop_wallpaper(self.workspaces.active);
        self.desktop_wallpaper(index).filter(|wallpaper| Some(wallpaper) != shown.as_ref())
    }

    /// Draws the desktops' bar again as it is now, where the desktops have not come or gone.
    fn refresh_overview_bar(&mut self) {
        let Some(output) = self.output_area() else {
            return;
        };
        let bar = self.spaces_bar(output);
        if let Some(overview) = &mut self.overview {
            overview.bar = bar;
        }
    }

    /// The wheel over the opened-up bar scrolls along its pictures, when they do not all fit.
    /// Returns whether it did.
    pub fn overview_scroll(&mut self, pos: Point<f64, Logical>, amount: f64) -> bool {
        let Some(overview) = self.overview.as_mut() else {
            return false;
        };
        let on_bar = overview.expansion >= 0.5
            && pos.y < f64::from(overview.bar.area.loc.y + expanded_height(overview.bar.area));
        if !on_bar || overview.bar.scroll_end <= 0.0 || amount == 0.0 {
            return false;
        }
        overview.bar_scroll = Some((overview.bar.scroll + amount * 3.0).clamp(0.0, overview.bar.scroll_end));
        self.refresh_overview_bar();
        true
    }

    /// A right click on a desktop's name or picture opens what can be done to it, as
    /// Windows' Task View does. Returns whether it was on one.
    pub fn overview_desktop_menu(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(overview) = &self.overview else {
            return false;
        };
        if overview.apps.as_ref().is_some_and(|apps| apps.style() != crate::config::AppsView::Overview) {
            return false;
        }
        let items = if overview.expansion >= 0.5 {
            &overview.bar.expanded_items
        } else {
            &overview.bar.strip_items
        };
        let Some(index) = items
            .iter()
            .find(|item| item.rect.to_f64().contains(pos))
            .and_then(|item| match item.action {
                BarAction::Switch(index) | BarAction::Rename(index) => Some(index),
                BarAction::Add => None,
            })
        else {
            return false;
        };
        self.finish_renaming(true);
        let count = self.workspaces.count();
        let own = self.workspaces.wallpaper(index).map(str::to_string);
        let mut choices = wallpaper_choices();
        if let Some(own) = own.as_ref().filter(|own| !choices.contains(own)) {
            choices.insert(0, own.clone());
        }
        let mut wallpapers = vec![
            Entry::item("與其他桌面相同", Command::Workspace(WorkspaceCommand::Wallpaper(index, None)))
                .checked(own.is_none()),
            Entry::separator(),
        ];
        wallpapers.extend(choices.into_iter().map(|path| {
            let name = std::path::Path::new(&path)
                .file_stem()
                .map_or_else(|| path.clone(), |stem| stem.to_string_lossy().into_owned());
            let checked = own.as_ref() == Some(&path);
            Entry::item(name, Command::Workspace(WorkspaceCommand::Wallpaper(index, Some(path)))).checked(checked)
        }));
        let entries = vec![
            Entry::item("重新命名", Command::Workspace(WorkspaceCommand::Rename(index))),
            Entry::submenu("桌布", wallpapers),
            Entry::separator(),
            Entry::item("向左移", Command::Workspace(WorkspaceCommand::Move(index, index.wrapping_sub(1))))
                .enabled(index > 0),
            Entry::item("向右移", Command::Workspace(WorkspaceCommand::Move(index, index + 1)))
                .enabled(index + 1 < count),
            Entry::separator(),
            Entry::item("關閉", Command::Workspace(WorkspaceCommand::Close(index))).enabled(count > 1),
        ];
        self.open_menu_at(entries, pos.to_i32_round());
        true
    }

    /// What the desktops' menu in the overview asked for.
    pub fn workspace_command(&mut self, command: WorkspaceCommand) {
        match command {
            WorkspaceCommand::Rename(index) => self.start_renaming(index),
            WorkspaceCommand::Move(from, to) => {
                if let Some(overview) = self.overview.as_mut().filter(|overview| from < overview.glides.len() && to < overview.glides.len()) {
                    let glide = overview.glides.remove(from);
                    overview.glides.insert(to, glide);
                }
                self.move_workspace(from, to);
                self.refresh_overview_bar();
            }
            WorkspaceCommand::Wallpaper(index, path) => {
                self.set_workspace_wallpaper(index, path);
                self.refresh_overview_bar();
            }
            WorkspaceCommand::Close(index) => {
                if let Some(overview) = self.overview.as_mut().filter(|overview| index < overview.glides.len()) {
                    overview.glides.remove(index);
                }
                self.remove_workspace(index);
            }
        }
    }

    /// Starts typing a desktop's name where it is shown, the bar held open meanwhile.
    fn start_renaming(&mut self, index: usize) {
        let name = self.workspaces.name(index);
        let Some(overview) = self.overview.as_mut() else {
            return;
        };
        overview.renaming = Some(Naming {
            index,
            text: name,
            chosen: true,
            preedit: String::new(),
            ime_focused: false,
            candidates: None,
            candidates_id: Id::new(),
        });
        overview.expanding = true;
        self.refresh_overview_bar();
    }

    /// Stops typing a desktop's name, taking what was typed or not.
    fn finish_renaming(&mut self, take: bool) {
        let Some(naming) = self.overview.as_mut().and_then(|overview| overview.renaming.take()) else {
            return;
        };
        if take {
            self.rename_workspace(naming.index, &naming.text);
        }
        self.refresh_overview_bar();
    }

    /// A key while a desktop's name is being typed: Return takes it, Escape leaves it as it
    /// was. Returns whether one was being typed.
    pub fn overview_rename_key(&mut self, sym: Keysym, typed: Option<char>) -> bool {
        let Some(naming) = self.overview.as_mut().and_then(|overview| overview.renaming.as_mut()) else {
            return false;
        };
        match sym {
            Keysym::Return | Keysym::KP_Enter | Keysym::Escape => {
                self.finish_renaming(sym != Keysym::Escape);
                return true;
            }
            Keysym::BackSpace => {
                if std::mem::take(&mut naming.chosen) {
                    naming.text.clear();
                } else {
                    naming.text.pop();
                }
            }
            // The arrows let the name go, to be added to rather than replaced.
            Keysym::Left | Keysym::Right | Keysym::Home | Keysym::End => naming.chosen = false,
            _ => match typed.filter(|character| !character.is_control()) {
                Some(character) => naming.insert(&character.to_string()),
                None => return true,
            },
        }
        self.refresh_overview_bar();
        true
    }

    /// What the input method says goes into a desktop's name being typed. Returns whether
    /// one was being typed.
    pub fn overview_rename_ime(&mut self, event: crate::ime::ImeEvent) -> bool {
        use crate::ime::ImeEvent;
        let Some(naming) = self.overview.as_mut().and_then(|overview| overview.renaming.as_mut()) else {
            return false;
        };
        match event {
            ImeEvent::Commit(text) => naming.insert(&text),
            ImeEvent::Preedit(text) => naming.preedit = text,
            ImeEvent::Candidates(candidates) => {
                naming.candidates = candidates.and_then(|candidates| {
                    let picture = crate::appgrid::draw_candidates(&candidates)?;
                    Some((candidates, picture))
                });
                return true;
            }
            ImeEvent::Key(raw) => {
                let sym = Keysym::from(raw);
                return self.overview_rename_key(sym, sym.key_char());
            }
        }
        self.refresh_overview_bar();
        true
    }
}

/// What can be done to a desktop from its menu in the overview.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkspaceCommand {
    Rename(usize),
    /// A wallpaper of its own, or, with none, the one of them all.
    Wallpaper(usize, Option<String>),
    /// From one place along the row to another.
    Move(usize, usize),
    Close(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_desktops_fit_left_of_the_add_button() {
        let (width, first, scroll, end) = bar_row(2000, 10, 0, None);
        let last_end = first + (width + THUMBNAIL_GAP) * 10 - THUMBNAIL_GAP;
        assert!(width < full_width(2000) && width >= least_width(2000));
        assert!(first >= BAR_EDGE && last_end <= 2000 - add_width(2000) - ADD_MARGIN - THUMBNAIL_GAP);
        assert_eq!((scroll, end), (0.0, 0.0));
    }

    #[test]
    fn a_few_desktops_keep_their_size_and_are_centered() {
        let (width, first, ..) = bar_row(2000, 2, 0, None);
        // The same share of a screen of any width.
        assert_eq!((width, full_width(1280)), (200, 128));
        assert_eq!(first, (2000 - (width * 2 + THUMBNAIL_GAP)) / 2);
    }

    #[test]
    fn too_many_desktops_scroll_to_the_one_shown_and_no_further_than_the_end() {
        let (width, first, scroll, end) = bar_row(1280, 30, 29, None);
        assert_eq!(width, least_width(1280));
        assert!(end > 0.0 && scroll == end);
        assert_eq!(first, BAR_EDGE - end.round() as i32);
        let (.., scroll, _) = bar_row(1280, 30, 29, Some(-50.0));
        assert_eq!(scroll, 0.0);
    }

    #[test]
    fn a_long_strip_keeps_the_desktop_shown_on_screen() {
        let start = strip_start(1280, 3000, 2900, 80);
        assert!(start + 2900 + 80 <= 1280 - MENU_BAR_HEIGHT - 16);
        assert_eq!(strip_start(1280, 400, 0, 80), 440);
    }

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
