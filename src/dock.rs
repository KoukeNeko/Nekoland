//! The Dock along the bottom, drawn by Nekoland on glass: a button for all applications,
//! the ones kept there, and after a line the others that have windows open. A dot under an
//! icon says it runs, a longer one that it is the active application. Pointing at an icon
//! lifts it and shows its name; clicking starts the application or brings its windows.

use cairo::{Context, Format, ImageSurface};
use smithay::{
    backend::renderer::element::Id,
    desktop::Window,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    config,
    menubar::Overlay,
    text::{self, Picture},
    windows::{is_managed, window_names, LAUNCHER_APP_ID},
    Nekoland,
};

const ICON: i32 = 48;
const GAP: i32 = 8;
const PADDING: i32 = 8;
const RADIUS: i32 = 18;
const MARGIN: i32 = 8;
const LIFT: i32 = 4;
const DOT: f64 = 4.0;
const SEPARATOR_WIDTH: i32 = 1;
/// The height the Dock keeps at the bottom of the output for itself.
pub const DOCK_SPACE: i32 = ICON + PADDING * 2 + MARGIN;

#[derive(Clone, PartialEq)]
enum Tile {
    Apps,
    App {
        app_id: String,
        name: String,
        icon: Option<String>,
        running: bool,
        active: bool,
    },
    Separator,
}

/// What the Dock shows; it is drawn again when any of it changes.
#[derive(Clone, PartialEq)]
struct Drawn {
    tiles: Vec<Tile>,
    hovered: Option<usize>,
    dark: bool,
}

#[derive(Default)]
pub struct Dock {
    tiles: Vec<Tile>,
    /// Where each tile is, across the Dock.
    spans: Vec<(i32, i32)>,
    hovered: Option<usize>,
    picture: Option<Picture>,
    label: Option<(usize, Picture)>,
    drawn: Option<Drawn>,
    ids: Option<(Id, Id)>,
}

type Rgb = (f64, f64, f64);

fn ink(dark: bool) -> Rgb {
    if dark {
        (0.945, 0.937, 0.918)
    } else {
        (0.090, 0.090, 0.102)
    }
}

fn on_ink(dark: bool) -> Rgb {
    if dark {
        (0.090, 0.090, 0.102)
    } else {
        (0.984, 0.980, 0.973)
    }
}

/// The button for all applications: nine dots in a square, in ink.
fn draw_apps_button(cr: &Context, x: f64, y: f64, dark: bool) {
    let (r, g, b) = ink(dark);
    text::rounded_rectangle(cr, x, y, f64::from(ICON), f64::from(ICON), f64::from(ICON) * 0.23);
    cr.set_source_rgba(r, g, b, 0.10);
    let _ = cr.fill();
    cr.set_source_rgb(r, g, b);
    let step = f64::from(ICON) / 4.0;
    for row in 1..=3 {
        for column in 1..=3 {
            cr.arc(
                x + step * f64::from(column),
                y + step * f64::from(row),
                3.2,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.fill();
        }
    }
}

/// An application without an icon: a tile with its initial.
fn draw_initial(cr: &Context, x: f64, y: f64, name: &str, dark: bool) {
    let (r, g, b) = ink(dark);
    text::rounded_rectangle(cr, x, y, f64::from(ICON), f64::from(ICON), f64::from(ICON) * 0.23);
    cr.set_source_rgba(r, g, b, 0.16);
    let _ = cr.fill();
    let initial: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(&pango::FontDescription::from_string(
        "Ubuntu Sans, Noto Sans CJK TC Bold 18",
    )));
    layout.set_text(&initial);
    let (width, height) = layout.pixel_size();
    cr.set_source_rgb(r, g, b);
    cr.move_to(
        x + f64::from(ICON - width) / 2.0,
        y + f64::from(ICON - height) / 2.0,
    );
    pangocairo::functions::show_layout(cr, &layout);
}

impl Nekoland {
    /// The Dock's tiles: all applications, the ones kept, and the others running.
    fn dock_tiles(&mut self) -> Vec<Tile> {
        let active = self
            .focused_window()
            .filter(is_managed)
            .map(|window| window_names(&window).1);
        let mut running: Vec<String> = Vec::new();
        for window in self.all_windows().filter(|window| is_managed(window)) {
            let app_id = window_names(window).1;
            if !app_id.is_empty() && app_id != LAUNCHER_APP_ID && !running.contains(&app_id) {
                running.push(app_id);
            }
        }
        let same = |one: &str, other: &str| one.eq_ignore_ascii_case(other);

        let mut tile = |app_id: &str, is_running: bool| {
            let info = self.apps.info(app_id).clone();
            Tile::App {
                app_id: app_id.to_string(),
                name: info
                    .name
                    .unwrap_or_else(|| app_id.rsplit('.').next().unwrap_or(app_id).to_string()),
                icon: info.icon_name,
                running: is_running,
                active: active.as_deref().is_some_and(|active| same(active, app_id)),
            }
        };
        let kept = self.config.dock.clone();
        let mut tiles = vec![Tile::Apps];
        for app_id in &kept {
            let is_running = running.iter().any(|running| same(running, app_id));
            tiles.push(tile(app_id, is_running));
        }
        let others: Vec<String> = running
            .into_iter()
            .filter(|app_id| !kept.iter().any(|kept| same(kept, app_id)))
            .collect();
        if !others.is_empty() {
            tiles.push(Tile::Separator);
            for app_id in others {
                tiles.push(tile(&app_id, true));
            }
        }
        tiles
    }

    fn dock_shown(&self) -> bool {
        self.fullscreen_window().is_none()
    }

    /// Where the Dock is on the output, if it is there.
    fn dock_rect(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        let area = self.space.output_geometry(output)?;
        let width = self.dock.spans.last().map_or(0, |(start, width)| start + width) + PADDING;
        let height = ICON + PADDING * 2;
        Some(Rectangle::new(
            (
                area.loc.x + (area.size.w - width) / 2,
                area.loc.y + area.size.h - MARGIN - height,
            )
                .into(),
            (width, height).into(),
        ))
    }

    /// Brings the Dock up to date before a frame.
    pub fn refresh_dock(&mut self) {
        let tiles = self.dock_tiles();
        let dark = self.menu_bar_dark();
        let drawn = Drawn {
            tiles: tiles.clone(),
            hovered: self.dock.hovered,
            dark,
        };
        if self.dock.drawn.as_ref() == Some(&drawn) && self.dock.picture.is_some() {
            return;
        }
        if self.dock.tiles != tiles {
            self.dock.hovered = None;
        }
        self.dock.tiles = tiles;
        self.dock.drawn = Some(Drawn {
            hovered: self.dock.hovered,
            ..drawn
        });
        self.draw_dock(dark);
    }

    fn draw_dock(&mut self, dark: bool) {
        // Where each tile goes.
        let mut spans = Vec::with_capacity(self.dock.tiles.len());
        let mut x = PADDING;
        for tile in &self.dock.tiles {
            let width = if *tile == Tile::Separator {
                SEPARATOR_WIDTH
            } else {
                ICON
            };
            spans.push((x, width));
            x += width + GAP;
        }
        self.dock.spans = spans;
        let width = x - GAP + PADDING;
        let height = ICON + PADDING * 2;

        let Ok(surface) = ImageSurface::create(Format::ARgb32, width.max(1), height) else {
            return;
        };
        let Ok(cr) = Context::new(&surface) else {
            return;
        };
        // The glass's own tint, as the design's floating surfaces have.
        let tint = if dark {
            (0.133, 0.133, 0.149, 0.45)
        } else {
            (0.984, 0.980, 0.973, 0.55)
        };
        text::rounded_rectangle(
            &cr,
            0.5,
            0.5,
            f64::from(width) - 1.0,
            f64::from(height) - 1.0,
            f64::from(RADIUS),
        );
        cr.set_source_rgba(tint.0, tint.1, tint.2, tint.3);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, if dark { 0.16 } else { 0.35 });
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        let (r, g, b) = ink(dark);
        for (index, (tile, (x, _))) in self.dock.tiles.iter().zip(&self.dock.spans).enumerate() {
            let lift = if self.dock.hovered == Some(index) { LIFT } else { 0 };
            let (x, y) = (f64::from(*x), f64::from(PADDING - lift));
            match tile {
                Tile::Apps => draw_apps_button(&cr, x, y, dark),
                Tile::Separator => {
                    cr.set_source_rgba(r, g, b, 0.18);
                    cr.rectangle(x, f64::from(PADDING + 6), 1.0, f64::from(ICON - 12));
                    let _ = cr.fill();
                }
                Tile::App {
                    name,
                    icon,
                    running,
                    active,
                    ..
                } => {
                    let drawn = icon
                        .as_deref()
                        .and_then(|icon| text::named_icon(icon, "", ICON, (r, g, b)))
                        .and_then(|icon| {
                            let scale = f64::from(ICON) / f64::from(icon.width().max(icon.height()).max(1));
                            cr.save().ok()?;
                            cr.translate(x, y);
                            cr.scale(scale, scale);
                            cr.set_source_surface(&icon, 0.0, 0.0).ok()?;
                            cr.paint().ok()?;
                            cr.restore().ok()
                        })
                        .is_some();
                    if !drawn {
                        draw_initial(&cr, x, y, name, dark);
                    }
                    if *running {
                        let dot_width = if *active { 14.0 } else { DOT };
                        let center = x + f64::from(ICON) / 2.0;
                        text::rounded_rectangle(
                            &cr,
                            center - dot_width / 2.0,
                            f64::from(height) - 6.0,
                            dot_width,
                            DOT,
                            DOT / 2.0,
                        );
                        cr.set_source_rgb(r, g, b);
                        let _ = cr.fill();
                    }
                }
            }
        }
        drop(cr);
        self.dock.picture = text::picture(surface);

        // The name of the pointed-at application, above it.
        self.dock.label = self.dock.hovered.and_then(|index| {
            let name = match self.dock.tiles.get(index)? {
                Tile::Apps => "應用程式".to_string(),
                Tile::App { name, .. } => name.clone(),
                Tile::Separator => return None,
            };
            Some((index, label_picture(&name, dark)?))
        });
    }

    /// What the Dock puts on screen, front first.
    pub fn dock_overlays(&self) -> Vec<Overlay<'_>> {
        if !self.dock_shown() {
            return Vec::new();
        }
        let (Some(rect), Some(picture), Some((dock_id, label_id))) =
            (self.dock_rect(), &self.dock.picture, &self.dock.ids)
        else {
            return Vec::new();
        };
        let mut overlays = Vec::new();
        if let Some((index, label)) = &self.dock.label {
            if let Some((x, width)) = self.dock.spans.get(*index) {
                let center = rect.loc.x + x + width / 2;
                overlays.push(Overlay {
                    id: label_id,
                    rect: Rectangle::new(
                        (center - label.size.w / 2, rect.loc.y - label.size.h - 8).into(),
                        label.size,
                    ),
                    glass: None,
                    alpha: 1.0,
                    picture: label,
                });
            }
        }
        overlays.push(Overlay {
            id: dock_id,
            rect,
            glass: Some(RADIUS),
            alpha: 1.0,
            picture,
        });
        overlays
    }

    /// Makes the ids the Dock's elements go by; called once the renderer is up.
    pub fn dock_ids(&mut self) {
        if self.dock.ids.is_none() {
            self.dock.ids = Some((Id::new(), Id::new()));
        }
    }

    pub fn dock_covers(&self, pos: Point<f64, Logical>) -> bool {
        self.dock_shown() && self.dock_rect().is_some_and(|rect| rect.to_f64().contains(pos))
    }

    fn dock_tile_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let rect = self.dock_rect()?;
        if !rect.to_f64().contains(pos) {
            return None;
        }
        let x = pos.x as i32 - rect.loc.x;
        self.dock
            .spans
            .iter()
            .position(|(start, width)| x >= *start - GAP / 2 && x < start + width + GAP / 2)
            .filter(|&index| self.dock.tiles[index] != Tile::Separator)
    }

    /// The pointer moved: the icon under it lifts and shows its name.
    pub fn dock_motion(&mut self, pos: Point<f64, Logical>) {
        let hovered = if self.dock_shown() {
            self.dock_tile_at(pos)
        } else {
            None
        };
        self.dock.hovered = hovered;
    }

    /// A button went down. Returns whether the Dock took it.
    pub fn dock_press(&mut self, pos: Point<f64, Logical>) -> bool {
        if !self.dock_covers(pos) {
            return false;
        }
        if let Some(tile) = self
            .dock_tile_at(pos)
            .and_then(|index| self.dock.tiles.get(index).cloned())
        {
            match tile {
                Tile::Apps => config::spawn(&self.config.launcher),
                Tile::App { app_id, running, .. } => {
                    if running {
                        self.bring_app(&app_id);
                    } else {
                        self.launch_app(&app_id);
                    }
                }
                Tile::Separator => {}
            }
        }
        true
    }

    /// Starts an application from its desktop entry, or by its ID when it has none.
    pub fn launch_app(&mut self, app_id: &str) {
        let exec = self.apps.info(app_id).exec.clone();
        config::spawn(exec.as_deref().unwrap_or(app_id));
    }

    /// Brings an application's windows forward, on whichever desktop they are, and gives
    /// the keyboard to the one on top.
    fn bring_app(&mut self, app_id: &str) {
        let same = |window: &Window| window_names(window).1.eq_ignore_ascii_case(app_id);
        let on_screen: Vec<Window> = self
            .space
            .elements()
            .filter(|window| same(window))
            .cloned()
            .collect();
        if let Some(top) = on_screen.last().cloned() {
            for window in &on_screen {
                self.space.raise_element(window, false);
            }
            self.focus_window(&top);
            return;
        }
        let elsewhere = self.all_windows().find(|window| same(window)).cloned();
        if let Some(window) = elsewhere {
            self.reveal_window(&window);
        }
    }
}

fn label_picture(name: &str, dark: bool) -> Option<Picture> {
    let font = pango::FontDescription::from_string("Ubuntu Sans, Noto Sans CJK TC Medium 9.5");
    let probe = Context::new(ImageSurface::create(Format::ARgb32, 1, 1).ok()?).ok()?;
    let layout = pangocairo::functions::create_layout(&probe);
    layout.set_font_description(Some(&font));
    layout.set_text(name);
    let (width, height) = layout.pixel_size();
    let (pad_x, pad_y) = (8, 4);
    let surface = ImageSurface::create(Format::ARgb32, width + pad_x * 2, height + pad_y * 2).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        let (r, g, b) = ink(dark);
        text::rounded_rectangle(
            &cr,
            0.0,
            0.0,
            f64::from(width + pad_x * 2),
            f64::from(height + pad_y * 2),
            6.0,
        );
        cr.set_source_rgb(r, g, b);
        cr.fill().ok()?;
        let layout = pangocairo::functions::create_layout(&cr);
        layout.set_font_description(Some(&font));
        layout.set_text(name);
        let (r, g, b) = on_ink(dark);
        cr.set_source_rgb(r, g, b);
        cr.move_to(f64::from(pad_x), f64::from(pad_y));
        pangocairo::functions::show_layout(&cr, &layout);
    }
    text::picture(surface)
}
