//! The overview, after Mission Control: the windows of the desktop spread apart where
//! they are, each named, under a bar that lists the desktops.

use std::time::{Duration, Instant};

use smithay::{
    backend::renderer::element::{solid::SolidColorBuffer, Id},
    desktop::Window,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};

use crate::{
    appgrid::AppGrid,
    glass::Tier,
    menubar::{Overlay, MENU_BAR_HEIGHT},
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
const THUMBNAIL_WIDTH: i32 = 192;
const THUMBNAIL_GAP: i32 = 28;
const EXPANDED_PADDING: i32 = 14;
const LABEL_HEIGHT: i32 = 24;
/// How long the bar takes to open up or close.
const EXPAND_DURATION: Duration = Duration::from_millis(180);
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
    Add,
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
    strip: Option<Picture>,
    expanded: Option<Picture>,
    strip_items: Vec<BarItem>,
    expanded_items: Vec<BarItem>,
    /// Where each desktop's live picture goes when the bar is opened up.
    thumbnails: Vec<Rectangle<i32, Logical>>,
    /// The button that takes a desktop away, shown on the one the pointer is on.
    close: Option<Picture>,
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
                tint: crate::glass::tint(dark, Tier::Menu),
            }
        } else {
            Self {
                ink: (0.090, 0.090, 0.102),
                on_ink: (0.984, 0.980, 0.973),
                tint: crate::glass::tint(dark, Tier::Menu),
            }
        }
    }

    fn paint_glass(&self, cr: &cairo::Context, width: i32, height: i32) {
        cr.set_source_rgba(self.tint.0, self.tint.1, self.tint.2, self.tint.3);
        let _ = cr.paint();
        // A hairline along the bottom edge, where the bar ends.
        cr.set_source_rgba(self.ink.0, self.ink.1, self.ink.2, 0.10);
        cr.rectangle(0.0, f64::from(height) - 1.0, f64::from(width), 1.0);
        let _ = cr.fill();
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

fn thumbnail_height(output: Rectangle<i32, Logical>) -> i32 {
    (THUMBNAIL_WIDTH * output.size.h / output.size.w.max(1)).max(1)
}

/// How tall the desktops' bar is opened up, from the top of the screen.
pub(crate) fn expanded_height(output: Rectangle<i32, Logical>) -> i32 {
    MENU_BAR_HEIGHT + EXPANDED_PADDING * 2 + thumbnail_height(output) + LABEL_HEIGHT
}

fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
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
    let (width, height) = layout.pixel_size();
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.move_to(
        f64::from(rect.loc.x) + f64::from(rect.size.w - width) / 2.0,
        f64::from(rect.loc.y) + f64::from(rect.size.h - height) / 2.0,
    );
    pangocairo::functions::show_layout(cr, &layout);
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
    closing: bool,
    /// The windows, bottom of the stack first.
    pub slots: Vec<Slot>,
    pub dim: SolidColorBuffer,
    bar: SpacesBar,
    /// How far the desktops' bar is opened up, 0 to 1, and whether it is opening.
    expansion: f64,
    expanding: bool,
    last_tick: Instant,
    strip_id: Id,
    expanded_id: Id,
    drag: Option<Drag>,
    /// The desktop in the opened-up bar the pointer is on, which shows its close button.
    hovered_desktop: Option<usize>,
    /// The slot under the pointer.
    pub hovered: Option<usize>,
    /// The four sides of the outline around it.
    pub outline: [Id; 4],
    /// The applications, shown in place of the windows.
    pub apps: Option<AppGrid>,
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
            expanding: false,
            last_tick: Instant::now(),
            strip_id: Id::new(),
            expanded_id: Id::new(),
            drag: None,
            hovered_desktop: None,
            hovered: None,
            outline: [Id::new(), Id::new(), Id::new(), Id::new()],
            apps: None,
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
        let area_under = |top: i32| {
            Rectangle::new(
                Point::from((output.loc.x + MARGIN, top + MARGIN / 2)),
                Size::from((output.size.w - MARGIN * 2, bottom - top - MARGIN)),
            )
        };
        let area = area_under(output.loc.y + MENU_BAR_HEIGHT + BAR_GAP);
        let open_area = area_under(output.loc.y + expanded_height(output) + BAR_GAP);

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
        let names: Vec<String> = (0..count).map(|index| format!("桌面 {}", index + 1)).collect();
        let tile_height = thumbnail_height(output);
        let expanded_height = expanded_height(output);
        let font = text::font("Ubuntu Sans, Noto Sans CJK TC Medium 10");

        // The strip, where the menu bar was: names in a row, centered.
        let widths: Vec<i32> = names
            .iter()
            .map(|name| text_width(name, &font) + NAME_PADDING * 2)
            .collect();
        let row: i32 = widths.iter().sum::<i32>() + NAME_GAP * (count as i32 - 1).max(0);
        let mut strip_items = Vec::with_capacity(count + 1);
        let strip = draw(output.size.w, MENU_BAR_HEIGHT, |cr| {
            colors.paint_glass(cr, output.size.w, MENU_BAR_HEIGHT);
            let mut x = (output.size.w - row) / 2;
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

        // Opened up: the pictures with their names, and the add button a square, each
        // centered top to bottom.
        let row = count as i32 * THUMBNAIL_WIDTH + THUMBNAIL_GAP * (count as i32 - 1).max(0);
        let top = (expanded_height - tile_height - LABEL_HEIGHT) / 2;
        let mut thumbnails = Vec::with_capacity(count);
        let mut expanded_items = Vec::with_capacity(count + 1);
        let expanded = draw(output.size.w, expanded_height, |cr| {
            colors.paint_glass(cr, output.size.w, expanded_height);
            let mut x = (output.size.w - row) / 2;
            for (index, name) in names.iter().enumerate() {
                let tile = Rectangle::new(Point::from((x, top)), Size::from((THUMBNAIL_WIDTH, tile_height)));
                // Behind the live picture, which is drawn over it.
                colors.fill_tile(cr, tile);
                if index == active {
                    colors.ring(cr, tile);
                }
                let label = Rectangle::new(
                    Point::from((x, top + tile_height + 4)),
                    Size::from((THUMBNAIL_WIDTH, LABEL_HEIGHT - 4)),
                );
                show_centered(cr, name, &font, label, colors.ink);
                thumbnails.push(Rectangle::new(output.loc + tile.loc, tile.size));
                expanded_items.push(BarItem {
                    rect: Rectangle::new(
                        output.loc + tile.loc,
                        Size::from((THUMBNAIL_WIDTH, tile_height + LABEL_HEIGHT)),
                    ),
                    action: BarAction::Switch(index),
                });
                x += THUMBNAIL_WIDTH + THUMBNAIL_GAP;
            }
            let add = Rectangle::new(
                Point::from((
                    output.size.w - tile_height - 16,
                    (expanded_height - tile_height) / 2,
                )),
                Size::from((tile_height, tile_height)),
            );
            colors.fill_tile(cr, add);
            colors.draw_plus(cr, add, 18.0);
            expanded_items.push(BarItem {
                rect: Rectangle::new(output.loc + add.loc, add.size),
                action: BarAction::Add,
            });
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
            strip,
            expanded,
            strip_items,
            expanded_items,
            thumbnails,
            close,
        }
    }

    /// How far the desktops' bar is opened up, 0 to 1, eased.
    pub fn overview_expansion(&self) -> f64 {
        self.overview
            .as_ref()
            .map_or(0.0, |overview| ease(overview.expansion))
    }

    /// Where each desktop's live picture goes in the opened-up bar.
    pub fn overview_thumbnails(&self) -> Vec<(usize, Rectangle<i32, Logical>)> {
        self.overview.as_ref().map_or_else(Vec::new, |overview| {
            overview.bar.thumbnails.iter().copied().enumerate().collect()
        })
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
        let grown = f64::from(expanded_height(output) - MENU_BAR_HEIGHT) * ease(overview.expansion);
        Some(Rectangle::new(
            output.loc,
            Size::from((output.size.w, MENU_BAR_HEIGHT + grown.round() as i32)),
        ))
    }

    /// The desktops' bar: one pane of glass, the strip fading out on it as it grows down
    /// and the opened-up bar fading in, shown as far as the glass has come.
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
        let expansion = ease(overview.expansion);
        let mut overlays = Vec::new();
        if let Some(picture) = overview.bar.expanded.as_ref().filter(|_| expansion > 0.0) {
            overlays.push(Overlay {
                fade: 1.0,
                id: &overview.expanded_id,
                rect: panel,
                glass: None,
                alpha: expansion as f32,
                picture,
            });
        }
        if let Some(picture) = &overview.bar.strip {
            overlays.push(Overlay {
                fade: 1.0,
                id: &overview.strip_id,
                rect: panel,
                glass: Some((Tier::Menu, 0)),
                alpha: (1.0 - expansion) as f32,
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
        if overview.drag.as_ref().is_some_and(|drag| {
            overview
                .slots
                .get(drag.slot)
                .is_none_or(|slot| !slot.window.alive())
        }) {
            overview.drag = None;
        }
        overview.slots.retain(|slot| slot.window.alive());
        let fade = progress as f32;
        overview.dim.set_color([0.0, 0.0, 0.0, DIM * fade]);

        let settled = !overview.closing && progress >= 1.0;
        let dragging = overview.drag.as_ref().is_some_and(|drag| drag.moving);
        // The bar opens up when the pointer comes onto the strip or a window is dragged, and
        // closes when the pointer leaves the opened-up bar.
        if let (Some(pos), Some(output)) = (pointer, output) {
            let y = pos.y - f64::from(output.loc.y);
            // With the applications shown in the overview, the bar stays open over them; shown
            // any other way, it is not there.
            let apps = overview.apps.as_ref().map(|apps| apps.style());
            if apps.is_some_and(|style| style != crate::config::AppsView::Overview) {
                overview.expanding = false;
            } else if dragging || apps.is_some() || (settled && y < f64::from(MENU_BAR_HEIGHT)) {
                overview.expanding = true;
            } else if y > f64::from(expanded_height(output) + 8) || !settled {
                overview.expanding = false;
            }
        }
        let now = Instant::now();
        let step = now.duration_since(overview.last_tick).as_secs_f64() / EXPAND_DURATION.as_secs_f64();
        overview.last_tick = now;
        overview.expansion = if overview.expanding {
            (overview.expansion + step).min(1.0)
        } else {
            (overview.expansion - step).max(0.0)
        };

        // The windows make room as the bar opens up.
        let open = ease(overview.expansion);
        for slot in &mut overview.slots {
            let target = lerp(slot.to, slot.to_open, open);
            slot.now = lerp(slot.from, target, progress);
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
            .filter(|_| open >= 0.5 && desktops > 1 && !dragging)
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
        let Some(overview) = &self.overview else {
            return;
        };
        // The close button of a desktop, over the corner of its picture, comes first.
        if let (Some(index), Some((rect, _))) = (overview.hovered_desktop, self.overview_close_button()) {
            if rect.to_f64().contains(pos) {
                self.remove_workspace(index);
                return;
            }
        }
        let items = if ease(overview.expansion) >= 0.5 {
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
        match action {
            // As on the Mac, picking a desktop goes to it and leaves the overview.
            Some(BarAction::Switch(index)) => {
                self.switch_workspace(index);
                self.close_overview(None);
            }
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

    /// The pointer moved with a window held: far enough, and it is being dragged.
    pub fn overview_motion(&mut self, pos: Point<f64, Logical>) {
        self.apps_motion(pos);
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
            Some(BarAction::Switch(index)) => self.move_to_workspace(&window, index),
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
