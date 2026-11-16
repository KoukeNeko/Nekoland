//! The Dock along the bottom, drawn by Nekoland on glass: a button for all applications,
//! the ones kept there, after a line the others that have windows open, and after another
//! the windows that are minimized. A dot under an icon says it runs, a longer one that it is
//! the active application.
//!
//! As on the Mac: pointing at an icon lifts it and names it; a click starts the application
//! or brings its windows; the icon breathes until a started application shows a window; a
//! right click opens a menu of its windows and what can be done with it; kept icons are
//! dragged to reorder them, out of the Dock to take them off, and a running one dragged in
//! among them is kept. At the right end are the kept folders, which open as a fan, a grid or
//! a list, and the Trash; a folder dragged in from a file manager is kept there, files
//! dropped on the Trash go to it, and files dropped on an application open in it. As on
//! Windows, resting on a running application shows a picture of each of its windows, to
//! pick one or close it.

use std::{
    io::Read,
    os::fd::{AsFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use cairo::{Context, Format, ImageSurface};
use gio::prelude::*;
use smithay::{
    backend::renderer::element::Id,
    desktop::Window,
    reexports::wayland_server::protocol::wl_data_source::WlDataSource,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::selection::data_device::with_source_metadata,
};

use crate::{
    config::{self, Show, Sort, Stack, View},
    menubar::{close, Command, Entry, Overlay},
    stacks::{self, Fan, Grid, Item},
    text::{self, Picture},
    windows::{is_managed, window_names, LAUNCHER_APP_ID},
    Nekoland,
};

/// Every icon's art is trimmed of the margin its theme drew around it and fitted to this,
/// as the Mac's icon template makes every icon the same size; icon themes on Linux leave
/// different margins, and the same padding then looks cramped around some.
const ART: i32 = 40;
const GAP: i32 = 12;
/// The same all around the art, the dot of a running application sitting in the bottom.
const SIDE: i32 = 12;
const TOP: i32 = 12;
const BOTTOM: i32 = 12;
const HEIGHT: i32 = TOP + ART + BOTTOM;
const RADIUS: i32 = 20;
const MARGIN: i32 = 8;
/// How far over the Dock what opens from it stands: names, window pictures and menus.
const POPUP_GAP: i32 = 10;
const LIFT: f64 = 4.0;
const DOT: f64 = 4.0;
const ACTIVE_DOT: f64 = 14.0;
const SEPARATOR_HEIGHT: f64 = 32.0;
/// A round icon looks bigger than a square one of the same width.
const ROUND_SCALE: f64 = 0.94;
const BADGE: i32 = 18;
/// The height the Dock keeps at the bottom of the output for itself.
pub const DOCK_SPACE: i32 = HEIGHT + MARGIN;

const LIFT_TIME: f64 = 0.12;
const MOVE_TIME: f64 = 0.18;
const PRESS_TIME: f64 = 0.09;
const BOUNCE_TIME: f64 = 0.18;
const PRESSED_SCALE: f64 = 0.92;
const BREATHE_TIME: f64 = 1.2;
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);
const PREVIEW_DELAY: Duration = Duration::from_millis(400);
const PREVIEW_LINGER: Duration = Duration::from_millis(300);
const DRAG_THRESHOLD: f64 = 6.0;
/// How far over the Dock a kept application is dragged to be taken off it.
const REMOVE_DISTANCE: f64 = 56.0;
const HELD_SCALE: f64 = 1.08;
/// How long a minimized window takes to shrink into its tile.
const MINIMIZE_TIME: f64 = 0.24;
/// How long names and window pictures take to fade in, and an icon let go off the Dock
/// to fade away.
const FADE_IN_TIME: f64 = 0.14;
/// How long a fan takes to come out of its folder.
const FAN_TIME: f64 = 0.22;
/// How often a kept folder and the Trash are looked at again for changes.
const FOLDER_CHECK: Duration = Duration::from_secs(1);
/// How many of a folder's items its list shows, each subfolder's too.
const LIST_MAX: usize = 30;
const FADE_OUT_TIME: f64 = 0.2;

const CARD_WIDTH: i32 = 220;
const CARD_PADDING: i32 = 6;
const CARD_HEADER: i32 = 24;
const SHOT_HEIGHT: i32 = 132;
const PREVIEW_PADDING: i32 = 6;
const CARD_GAP: i32 = 4;
const PREVIEW_RADIUS: i32 = 12;
const CLOSE: i32 = 20;
const LABEL_HEIGHT: i32 = 26;
const LABEL_RADIUS: i32 = 8;
const FONT: &str = "Ubuntu Sans, Noto Sans CJK TC";
const CONFIRM_APP_ID: &str = "nekoland-confirm";

#[derive(Clone, PartialEq)]
enum Tile {
    Apps,
    App {
        app_id: String,
        name: String,
        icon: Option<String>,
        running: bool,
        active: bool,
        kept: bool,
    },
    Separator(&'static str),
    /// A minimized window.
    Window {
        window: Window,
        title: String,
        icon: Option<String>,
    },
    /// A kept folder, by its place among them, with the items its icon piles up.
    Stack {
        index: usize,
        path: PathBuf,
        name: String,
        show: Show,
        covers: Vec<Item>,
    },
    Trash {
        full: bool,
    },
}

/// What stays the same of a tile from frame to frame, for its animation.
#[derive(Clone, PartialEq)]
enum Key {
    Apps,
    App(String),
    Separator(&'static str),
    Window(Window),
    Stack(PathBuf),
    Trash,
}

impl Tile {
    fn key(&self) -> Key {
        match self {
            Tile::Apps => Key::Apps,
            Tile::App { app_id, .. } => Key::App(app_id.clone()),
            Tile::Separator(name) => Key::Separator(name),
            Tile::Window { window, .. } => Key::Window(window.clone()),
            Tile::Stack { path, .. } => Key::Stack(path.clone()),
            Tile::Trash { .. } => Key::Trash,
        }
    }

    fn width(&self) -> i32 {
        if matches!(self, Tile::Separator(_)) {
            1
        } else {
            ART
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Tile::Apps => Some("應用程式"),
            Tile::App { name, .. } => Some(name),
            Tile::Window { title, .. } => Some(title),
            Tile::Stack { name, .. } => Some(name),
            Tile::Trash { .. } => Some("垃圾桶"),
            Tile::Separator(_) => None,
        }
    }
}

/// What the Dock's menu for an application does.
#[derive(Clone, PartialEq)]
pub enum DockCommand {
    NewWindow(String),
    TogglePin(String),
    Hide(String),
    Quit(String),
    Open(PathBuf),
    StackSort(usize, Sort),
    StackShow(usize, Show),
    StackView(usize, View),
    RemoveStack(usize),
    RevealStack(usize),
    OpenTrash,
    EmptyTrash,
}

/// What is in a kept folder, as last read.
struct Folder {
    path: PathBuf,
    sort: Sort,
    /// When the folder last changed, and when that was looked at.
    changed: i64,
    checked: Instant,
    items: Vec<Item>,
}

/// A kept folder opened over the Dock.
struct OpenStack {
    folder: PathBuf,
    name: String,
    items: Vec<Item>,
    view: Opened,
    rect: Rectangle<i32, Logical>,
    opened: Instant,
    hovered: Option<usize>,
    button_hovered: bool,
    picture: Option<Picture>,
    drawn: Option<(Option<usize>, bool, bool, usize, u8)>,
}

enum Opened {
    /// The items rising from the folder, newest nearest, and a line to open the folder;
    /// `anchor` is the top middle of the folder's icon.
    Fan {
        fan: Fan,
        anchor: Point<f64, Logical>,
        labels: Vec<String>,
    },
    /// A page of the items at a time, `scroll` rows down.
    Grid { grid: Grid, scroll: usize },
}

/// Where a tile is drawn now, on its way to where it goes.
struct Motion {
    key: Key,
    x: f64,
    lift: f64,
}

struct Press {
    key: Key,
    tile: Tile,
    start: Point<f64, Logical>,
    at: Instant,
}

/// An icon being dragged within the Dock or out of it.
struct Drag {
    what: Dragged,
    /// Whether letting it go away from the Dock takes it off.
    kept: bool,
    /// Where in the icon it was taken.
    grab: Point<f64, Logical>,
    /// Among its kind, where it would go if let go now; none when it would be taken off
    /// the Dock instead.
    slot: Option<usize>,
}

enum Dragged {
    /// A kept or running application, which goes among the kept ones.
    App { app_id: String, icon: Option<String> },
    /// A kept folder, which goes among the folders.
    Stack { index: usize, path: PathBuf },
}

impl Dragged {
    fn key(&self) -> Key {
        match self {
            Dragged::App { app_id, .. } => Key::App(app_id.clone()),
            Dragged::Stack { path, .. } => Key::Stack(path.clone()),
        }
    }
}

/// The pictures of a running application's windows, over its icon.
struct Preview {
    app_id: String,
    windows: Vec<Window>,
    rect: Rectangle<i32, Logical>,
    cards: Vec<Rectangle<i32, Logical>>,
    /// Since when the pointer has been away from the icon and the pictures.
    away: Option<Instant>,
    opened: Instant,
    picture: Option<Picture>,
    drawn: Option<(Vec<String>, Option<usize>, bool, bool)>,
}

impl Preview {
    fn shot(card: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        Rectangle::new(
            card.loc + Point::from((CARD_PADDING, CARD_PADDING + CARD_HEADER + 6)),
            Size::from((CARD_WIDTH - CARD_PADDING * 2, SHOT_HEIGHT)),
        )
    }

    fn close_button(card: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        Rectangle::new(
            card.loc
                + Point::from((
                    card.size.w - CARD_PADDING - CLOSE,
                    CARD_PADDING + (CARD_HEADER - CLOSE) / 2,
                )),
            Size::from((CLOSE, CLOSE)),
        )
    }
}

/// What the Dock was last drawn as; it is drawn again when any of it changes.
#[derive(PartialEq)]
struct Drawn {
    dark: bool,
    width: i32,
    tiles: Vec<(Tile, i32, u8, u8, u8)>,
}

struct Ids {
    dock: Id,
    label: Id,
    preview: Id,
    stack: Id,
    held: Id,
    tag: Id,
}

#[derive(Default)]
pub struct Dock {
    /// What is shown, the dragged icon left out.
    tiles: Vec<Tile>,
    /// Where each tile goes across the Dock, and how wide the Dock is to be.
    spans: Vec<(i32, i32)>,
    width: i32,
    shown_width: f64,
    motions: Vec<Motion>,
    hovered: Option<usize>,
    hovered_since: Option<Instant>,
    press: Option<Press>,
    bounce: Option<(Key, Instant)>,
    drag: Option<Drag>,
    /// Applications started from the Dock that have no window yet.
    launching: Vec<(String, Instant)>,
    /// Windows on their way into the Dock, from where they were.
    minimizing: Vec<(Window, Rectangle<i32, Logical>, Instant)>,
    preview: Option<Preview>,
    picture: Option<Picture>,
    label: Option<(Key, bool, Picture, Instant)>,
    held: Option<Picture>,
    /// An icon let go off the Dock, fading away where it was let go.
    poof: Option<(Picture, Point<i32, Logical>, Instant)>,
    tag: Option<(bool, Picture)>,
    icons: Vec<(String, i32, bool, Option<ImageSurface>)>,
    badges: Vec<(Option<String>, bool, Option<Picture>)>,
    drawn: Option<Drawn>,
    ids: Option<Ids>,
    last_tick: Option<Instant>,
    folders: Vec<Folder>,
    trash: Option<(bool, Instant)>,
    stack: Option<OpenStack>,
    /// Pictures of files and folders, by path.
    file_icons: Vec<(PathBuf, bool, Option<ImageSurface>)>,
    /// What a file manager is dragging, once it has said: the paths of the files.
    dnd: Option<Arc<Mutex<Option<Vec<PathBuf>>>>>,
    /// Where the pointer last moved to.
    pointer: Option<Point<f64, Logical>>,
}

type Rgb = (f64, f64, f64);

pub(crate) fn ink(dark: bool) -> Rgb {
    if dark {
        (0.945, 0.937, 0.918)
    } else {
        (0.090, 0.090, 0.102)
    }
}

fn ink_2(dark: bool) -> Rgb {
    if dark {
        (0.663, 0.655, 0.671)
    } else {
        (0.365, 0.361, 0.380)
    }
}

/// The glass's own tint, as the design's floating surfaces have, and the tint of what
/// opens over it, which is a little more solid to read on.
fn tint(dark: bool, solid: bool) -> (f64, f64, f64, f64) {
    match (dark, solid) {
        (true, false) => (0.133, 0.133, 0.149, 0.45),
        (true, true) => (0.133, 0.133, 0.149, 0.78),
        (false, false) => (0.984, 0.980, 0.973, 0.55),
        (false, true) => (0.984, 0.980, 0.973, 0.80),
    }
}

/// A rounded pane of tinted glass with its thin rim, the size of the surface.
pub(crate) fn paint_pane(cr: &Context, width: f64, height: f64, radius: f64, dark: bool, solid: bool) {
    let (r, g, b, a) = tint(dark, solid);
    text::rounded_rectangle(cr, 0.5, 0.5, width - 1.0, height - 1.0, radius);
    cr.set_source_rgba(r, g, b, a);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, if dark { 0.16 } else { 0.35 });
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Overshoots a little before it settles, for the icon coming back up after a press.
fn ease_out_back(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.7 * t.powi(3) + 1.7 * t.powi(2)
}

/// An icon's art without the empty margin around it, fitted to a square of `size`.
fn trimmed(mut surface: ImageSurface, size: i32) -> Option<ImageSurface> {
    let (width, height) = (surface.width(), surface.height());
    let stride = surface.stride() as usize;
    let (bounds, round) = {
        let data = surface.data().ok()?;
        let alpha = |x: i32, y: i32| {
            let at = y as usize * stride + x as usize * 4;
            u32::from_ne_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) >> 24
        };
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for y in 0..height {
            for x in 0..width {
                if alpha(x, y) > 24 {
                    left = left.min(x);
                    right = right.max(x + 1);
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        if right <= left || bottom <= top {
            return None;
        }
        let (w, h) = (right - left, bottom - top);
        // A circle leaves its corners empty a tenth of the way in; a rounded square does not.
        let corners = [(1, 1), (9, 1), (1, 9), (9, 9)].map(|(cx, cy)| {
            alpha(
                (left + w * cx / 10).min(width - 1),
                (top + h * cy / 10).min(height - 1),
            )
        });
        let square = (w as f64 / h as f64 - 1.0).abs() < 0.1;
        let round = square && corners.iter().all(|&a| a < 24);
        ((left, top, w, h), round)
    };
    let (left, top, w, h) = bounds;
    let fit = f64::from(size) * if round { ROUND_SCALE } else { 1.0 };
    let scale = fit / f64::from(w.max(h));
    let out = ImageSurface::create(Format::ARgb32, size, size).ok()?;
    {
        let cr = Context::new(&out).ok()?;
        cr.translate(
            (f64::from(size) - f64::from(w) * scale) / 2.0,
            (f64::from(size) - f64::from(h) * scale) / 2.0,
        );
        cr.scale(scale, scale);
        cr.set_source_surface(&surface, -f64::from(left), -f64::from(top))
            .ok()?;
        cr.source().set_filter(cairo::Filter::Good);
        cr.paint().ok()?;
    }
    Some(out)
}

/// An application without an icon: a tile with its initial.
fn draw_initial(cr: &Context, x: f64, y: f64, size: f64, name: &str, dark: bool) {
    let (r, g, b) = ink(dark);
    text::rounded_rectangle(cr, x, y, size, size, size * 0.23);
    cr.set_source_rgba(r, g, b, 0.16);
    let _ = cr.fill();
    let initial: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(&pango::FontDescription::from_string(&format!(
        "{FONT} Bold {}",
        (size * 0.36).round()
    ))));
    layout.set_text(&initial);
    let (width, height) = layout.pixel_size();
    cr.set_source_rgb(r, g, b);
    cr.move_to(
        x + (size - f64::from(width)) / 2.0,
        y + (size - f64::from(height)) / 2.0,
    );
    pangocairo::functions::show_layout(cr, &layout);
}

/// A name on a small pane of glass, as shown over a pointed-at icon.
fn label_picture(name: &str, height: i32, radius: i32, points: f64, dark: bool) -> Option<Picture> {
    let font = pango::FontDescription::from_string(&format!("{FONT} {points}"));
    let probe = Context::new(ImageSurface::create(Format::ARgb32, 1, 1).ok()?).ok()?;
    let layout = pangocairo::functions::create_layout(&probe);
    layout.set_font_description(Some(&font));
    layout.set_text(name);
    let (text_width, text_height) = layout.pixel_size();
    let width = text_width + 20;
    let surface = ImageSurface::create(Format::ARgb32, width, height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        paint_pane(
            &cr,
            f64::from(width),
            f64::from(height),
            f64::from(radius),
            dark,
            true,
        );
        let layout = pangocairo::functions::create_layout(&cr);
        layout.set_font_description(Some(&font));
        layout.set_text(name);
        let (r, g, b) = ink(dark);
        cr.set_source_rgb(r, g, b);
        cr.move_to(10.0, f64::from(height - text_height) / 2.0);
        pangocairo::functions::show_layout(&cr, &layout);
    }
    text::picture(surface)
}

impl Nekoland {
    /// The Dock's tiles: all applications, the ones kept, the others running, and the
    /// minimized windows.
    fn dock_tiles(&mut self) -> Vec<Tile> {
        let active = self
            .focused_window()
            .filter(is_managed)
            .map(|window| window_names(&window).1);
        let mut running: Vec<String> = Vec::new();
        for window in self.all_windows().filter(|window| is_managed(window)) {
            let app_id = window_names(window).1;
            // Nekoland's own launcher and dialogs are part of the desktop, not applications.
            let own = app_id == LAUNCHER_APP_ID || app_id == CONFIRM_APP_ID;
            if !app_id.is_empty() && !own && !running.contains(&app_id) {
                running.push(app_id);
            }
        }
        let same = |one: &str, other: &str| one.eq_ignore_ascii_case(other);

        let mut tile = |app_id: &str, is_running: bool, kept: bool| {
            let info = self.apps.info(app_id).clone();
            Tile::App {
                app_id: app_id.to_string(),
                name: info
                    .name
                    .unwrap_or_else(|| app_id.rsplit('.').next().unwrap_or(app_id).to_string()),
                icon: info.icon_name,
                running: is_running,
                active: active.as_deref().is_some_and(|active| same(active, app_id)),
                kept,
            }
        };
        let kept = self.config.dock.clone();
        let mut tiles = vec![Tile::Apps];
        for app_id in &kept {
            let is_running = running.iter().any(|running| same(running, app_id));
            tiles.push(tile(app_id, is_running, true));
        }
        let others: Vec<String> = running
            .into_iter()
            .filter(|app_id| !kept.iter().any(|kept| same(kept, app_id)))
            .collect();
        if !others.is_empty() {
            tiles.push(Tile::Separator("running"));
            for app_id in others {
                tiles.push(tile(&app_id, true, false));
            }
        }

        // The right end: kept folders, minimized windows, and the Trash last.
        tiles.push(Tile::Separator("right"));
        for (index, stack) in self.config.stacks.clone().into_iter().enumerate() {
            let path = stack.folder();
            let covers = self
                .folder_items(&path, stack.sort)
                .iter()
                .take(3)
                .cloned()
                .collect();
            tiles.push(Tile::Stack {
                index,
                name: stacks::folder_name(&path),
                show: stack.show,
                covers,
                path,
            });
        }
        self.minimized.retain(|(window, _)| window.alive());
        let minimized: Vec<Window> = self.minimized.iter().map(|(window, _)| window.clone()).collect();
        {
            for window in minimized {
                let (title, app_id) = window_names(&window);
                let info = self.apps.info(&app_id).clone();
                tiles.push(Tile::Window {
                    title: if title.is_empty() {
                        info.name.unwrap_or(app_id)
                    } else {
                        title
                    },
                    icon: info.icon_name,
                    window,
                });
            }
        }
        tiles.push(Tile::Trash {
            full: self.trash_full(),
        });
        tiles
    }

    /// What is in a kept folder, read again when it has changed since it was last read.
    fn folder_items(&mut self, path: &Path, sort: Sort) -> &[Item] {
        let index = match self.dock.folders.iter().position(|folder| folder.path == path) {
            Some(index) => index,
            None => {
                self.dock.folders.push(Folder {
                    path: path.to_path_buf(),
                    sort,
                    changed: i64::MIN,
                    checked: Instant::now() - FOLDER_CHECK,
                    items: Vec::new(),
                });
                self.dock.folders.len() - 1
            }
        };
        let folder = &mut self.dock.folders[index];
        if folder.sort != sort || folder.checked.elapsed() >= FOLDER_CHECK {
            use std::os::unix::fs::MetadataExt;
            let changed =
                std::fs::metadata(path).map_or(0, |meta| meta.mtime_nsec() + meta.mtime() * 1_000_000_000);
            if folder.sort != sort || changed != folder.changed {
                folder.items = stacks::read_folder(path, sort);
                folder.sort = sort;
                folder.changed = changed;
            }
            folder.checked = Instant::now();
        }
        &self.dock.folders[index].items
    }

    /// Whether anything is in the Trash, looked at again now and then.
    fn trash_full(&mut self) -> bool {
        if let Some((full, at)) = self.dock.trash {
            if at.elapsed() < FOLDER_CHECK {
                return full;
            }
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let full = std::fs::read_dir(format!("{home}/.local/share/Trash/files"))
            .is_ok_and(|mut entries| entries.next().is_some());
        self.dock.trash = Some((full, Instant::now()));
        full
    }

    /// A file's or folder's picture, kept for the next frame.
    fn file_icon(&mut self, item: &Item, dark: bool) -> Option<ImageSurface> {
        if let Some((.., icon)) = self
            .dock
            .file_icons
            .iter()
            .find(|(path, cached_dark, _)| *path == item.path && *cached_dark == dark)
        {
            return icon.clone();
        }
        // shortcut: forgets every picture once there are many, rather than the oldest.
        if self.dock.file_icons.len() > 400 {
            self.dock.file_icons.clear();
        }
        let icon = stacks::item_icon(item, 64, ink(dark));
        self.dock.file_icons.push((item.path.clone(), dark, icon.clone()));
        icon
    }

    /// A folder's own icon, as its file manager shows it, trimmed as an application's is.
    fn folder_icon(&mut self, path: &Path, size: i32, dark: bool) -> Option<ImageSurface> {
        let names = stacks::icon_names(path);
        names
            .iter()
            .chain(std::iter::once(&"folder".to_string()))
            .find_map(|name| self.dock_icon(name, size, dark))
    }

    fn dock_shown(&self) -> bool {
        self.fullscreen_window().is_none()
    }

    fn output_rect(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        self.space.output_geometry(output)
    }

    /// Where the Dock is on the output, `width` wide.
    fn dock_rect_of(&self, width: i32) -> Option<Rectangle<i32, Logical>> {
        let area = self.output_rect()?;
        Some(Rectangle::new(
            (
                area.loc.x + (area.size.w - width) / 2,
                area.loc.y + area.size.h - MARGIN - HEIGHT,
            )
                .into(),
            (width, HEIGHT).into(),
        ))
    }

    /// Where the Dock is drawn now.
    fn dock_rect(&self) -> Option<Rectangle<i32, Logical>> {
        self.dock_rect_of(self.dock.shown_width.round() as i32)
    }

    /// Where a tile is drawn now, art and all, lifted if it is.
    fn tile_rect(&self, index: usize) -> Option<Rectangle<i32, Logical>> {
        let dock = self.dock_rect()?;
        let motion = self.dock.motions.get(index)?;
        let width = self.dock.tiles.get(index)?.width();
        Some(Rectangle::new(
            (
                dock.loc.x + motion.x.round() as i32,
                dock.loc.y + TOP - (motion.lift * LIFT).round() as i32,
            )
                .into(),
            (width, ART).into(),
        ))
    }

    /// An icon by name, its art trimmed and fitted to `size`, kept for the next frame.
    fn dock_icon(&mut self, name: &str, size: i32, dark: bool) -> Option<ImageSurface> {
        if let Some((.., icon)) = self
            .dock
            .icons
            .iter()
            .find(|(cached, at, cached_dark, _)| cached == name && *at == size && *cached_dark == dark)
        {
            return icon.clone();
        }
        let icon =
            text::named_icon(name, "", (size * 3).max(96), ink(dark)).and_then(|icon| trimmed(icon, size));
        self.dock.icons.push((name.to_string(), size, dark, icon.clone()));
        icon
    }

    /// Lays the tiles out across the Dock, with room left at `gap` for a dragged icon.
    fn lay_out(tiles: &[Tile], gap: Option<usize>) -> (Vec<(i32, i32)>, i32) {
        let mut spans = Vec::with_capacity(tiles.len());
        let mut x = SIDE;
        for (index, tile) in tiles.iter().enumerate() {
            if gap == Some(index) {
                x += ART + GAP;
            }
            spans.push((x, tile.width()));
            x += tile.width() + GAP;
        }
        if gap == Some(tiles.len()) {
            x += ART + GAP;
        }
        (spans, x - GAP + SIDE)
    }

    /// Brings the Dock up to date before a frame: what it shows, where everything is on
    /// its way, and what opens over it.
    pub fn refresh_dock(&mut self) {
        let now = Instant::now();
        let dt = self
            .dock
            .last_tick
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64().min(0.1));
        self.dock.last_tick = Some(now);
        let pointer = self.seat.get_pointer().map(|pointer| pointer.current_location());
        let dark = self.menu_bar_dark();
        let menu_open = self.menu_bar.open.is_some();

        let mut tiles = self.dock_tiles();
        let running = |tiles: &[Tile], app_id: &str| {
            tiles.iter().any(|tile| {
                matches!(tile, Tile::App { app_id: id, running: true, .. } if id.eq_ignore_ascii_case(app_id))
            })
        };
        self.dock
            .launching
            .retain(|(app_id, at)| !running(&tiles, app_id) && at.elapsed() < LAUNCH_TIMEOUT);
        self.dock
            .minimizing
            .retain(|(window, _, at)| window.alive() && at.elapsed().as_secs_f64() < MINIMIZE_TIME);

        // A dragged icon leaves its place, and the others of its kind make room where it
        // would go: kept applications after the button for all of them, folders after the
        // line at the right end.
        let mut gap = None;
        if let Some(drag) = &self.dock.drag {
            let key = drag.what.key();
            tiles.retain(|tile| tile.key() != key);
            let (start, alike): (usize, fn(&Tile) -> bool) = match drag.what {
                Dragged::App { .. } => (1, |tile| matches!(tile, Tile::App { kept: true, .. })),
                Dragged::Stack { .. } => (
                    tiles
                        .iter()
                        .position(|tile| *tile == Tile::Separator("right"))
                        .map_or(tiles.len(), |at| at + 1),
                    |tile| matches!(tile, Tile::Stack { .. }),
                ),
            };
            let count = tiles[start..].iter().take_while(|tile| alike(tile)).count();
            let (spans, width) = Self::lay_out(&tiles, None);
            let slot = match (pointer, self.dock_rect_of(width)) {
                (Some(pos), Some(rect)) if pos.y >= f64::from(rect.loc.y) - REMOVE_DISTANCE => {
                    let x = pos.x - f64::from(rect.loc.x);
                    Some(
                        spans[start..start + count]
                            .iter()
                            .filter(|(start, width)| f64::from(start + width / 2) < x)
                            .count(),
                    )
                }
                _ => None,
            };
            gap = slot.map(|slot| slot + start);
            if let Some(drag) = self.dock.drag.as_mut() {
                drag.slot = slot;
            }
        }
        if self
            .dock
            .tiles
            .iter()
            .map(Tile::key)
            .ne(tiles.iter().map(Tile::key))
        {
            self.dock.hovered = None;
            self.dock.press = None;
        }
        let (spans, width) = Self::lay_out(&tiles, gap);

        // Everything moves toward where it goes.
        let approach = 1.0 - (-dt / (MOVE_TIME / 3.0)).exp();
        if self.dock.shown_width == 0.0 {
            self.dock.shown_width = f64::from(width);
        }
        self.dock.shown_width += (f64::from(width) - self.dock.shown_width) * approach;
        if (self.dock.shown_width - f64::from(width)).abs() < 0.5 {
            self.dock.shown_width = f64::from(width);
        }
        let hovered_key = self
            .dock
            .hovered
            .filter(|_| !menu_open && self.dock.drag.is_none())
            .and_then(|index| self.dock.tiles.get(index))
            .map(Tile::key);
        let old = std::mem::take(&mut self.dock.motions);
        self.dock.motions = tiles
            .iter()
            .zip(&spans)
            .map(|(tile, (target, _))| {
                let key = tile.key();
                let lift_to = if Some(&key) == hovered_key.as_ref() {
                    1.0
                } else {
                    0.0
                };
                match old.iter().find(|motion| motion.key == key) {
                    Some(motion) => {
                        let mut x = motion.x + (f64::from(*target) - motion.x) * approach;
                        if (x - f64::from(*target)).abs() < 0.5 {
                            x = f64::from(*target);
                        }
                        let step = dt / LIFT_TIME;
                        let lift = if lift_to > motion.lift {
                            (motion.lift + step).min(1.0)
                        } else {
                            (motion.lift - step).max(0.0)
                        };
                        Motion { key, x, lift }
                    }
                    None => Motion {
                        key,
                        x: f64::from(*target),
                        lift: 0.0,
                    },
                }
            })
            .collect();
        self.dock.tiles = tiles;
        self.dock.spans = spans;
        self.dock.width = width;

        if self.dock.stack.is_some() && (menu_open || !self.dock_shown()) {
            self.dock.stack = None;
        }
        self.refresh_preview(pointer, dark, menu_open, now);
        self.refresh_stack(pointer, dark);
        self.draw_dock(dark, now);
        self.refresh_dock_popups(dark, menu_open);
    }

    /// How big a tile is drawn now, and how solid: smaller while pressed, coming back with
    /// a little bounce, and breathing while its application starts.
    fn tile_scale(&self, tile: &Tile) -> (f64, f64) {
        let key = tile.key();
        if let Some(press) = self.dock.press.as_ref().filter(|press| press.key == key) {
            let t = press.at.elapsed().as_secs_f64() / PRESS_TIME;
            return (1.0 - (1.0 - PRESSED_SCALE) * ease_out(t), 1.0);
        }
        if let Some((_, at)) = self.dock.bounce.as_ref().filter(|(bounced, _)| *bounced == key) {
            let t = at.elapsed().as_secs_f64() / BOUNCE_TIME;
            if t < 1.0 {
                return (PRESSED_SCALE + (1.0 - PRESSED_SCALE) * ease_out_back(t), 1.0);
            }
        }
        if let Tile::App { app_id, .. } = tile {
            if let Some((_, at)) = self.dock.launching.iter().find(|(id, _)| id == app_id) {
                let phase = at.elapsed().as_secs_f64() / BREATHE_TIME * std::f64::consts::TAU;
                let breath = (1.0 - phase.cos()) / 2.0;
                return (1.0 - 0.1 * breath, 1.0 - 0.4 * breath);
            }
        }
        (1.0, 1.0)
    }

    fn draw_dock(&mut self, dark: bool, now: Instant) {
        if self
            .dock
            .bounce
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at).as_secs_f64() > BOUNCE_TIME)
        {
            self.dock.bounce = None;
        }
        let width = self.dock.shown_width.round() as i32;
        let drawn = Drawn {
            dark,
            width,
            tiles: self
                .dock
                .tiles
                .iter()
                .zip(&self.dock.motions)
                .map(|(tile, motion)| {
                    let (scale, alpha) = self.tile_scale(tile);
                    (
                        tile.clone(),
                        (motion.x * 2.0).round() as i32,
                        (motion.lift * 32.0).round() as u8,
                        (scale * 200.0).round() as u8,
                        (alpha * 100.0).round() as u8,
                    )
                })
                .collect(),
        };
        if self.dock.drawn.as_ref() == Some(&drawn) && self.dock.picture.is_some() {
            return;
        }

        let Ok(surface) = ImageSurface::create(Format::ARgb32, width.max(1), HEIGHT) else {
            return;
        };
        {
            let Ok(cr) = Context::new(&surface) else {
                return;
            };
            paint_pane(
                &cr,
                f64::from(width),
                f64::from(HEIGHT),
                f64::from(RADIUS),
                dark,
                false,
            );
            let (r, g, b) = ink(dark);
            for (tile, x, lift, scale, alpha) in &drawn.tiles {
                let x = f64::from(*x) / 2.0;
                let y = f64::from(TOP) - f64::from(*lift) / 32.0 * LIFT;
                let (scale, alpha) = (f64::from(*scale) / 200.0, f64::from(*alpha) / 100.0);
                let art = f64::from(ART);
                // Pressed and breathing icons shrink around their middle.
                let inset = art * (1.0 - scale) / 2.0;
                let (ax, ay, size) = (x + inset, y + inset, art * scale);
                match tile {
                    Tile::Apps => {
                        cr.set_source_rgba(r, g, b, alpha);
                        for row in -1..=1 {
                            for column in -1..=1 {
                                cr.arc(
                                    x + art / 2.0 + f64::from(column) * 12.0 * scale,
                                    y + art / 2.0 + f64::from(row) * 12.0 * scale,
                                    3.0 * scale,
                                    0.0,
                                    std::f64::consts::TAU,
                                );
                                let _ = cr.fill();
                            }
                        }
                    }
                    Tile::Separator(_) => {
                        cr.set_source_rgba(r, g, b, 0.18);
                        cr.rectangle(
                            x,
                            f64::from(TOP) + (art - SEPARATOR_HEIGHT) / 2.0,
                            1.0,
                            SEPARATOR_HEIGHT,
                        );
                        let _ = cr.fill();
                    }
                    Tile::App {
                        name,
                        icon,
                        running,
                        active,
                        ..
                    } => {
                        let image = icon.as_deref().and_then(|icon| self.dock_icon(icon, ART, dark));
                        match image {
                            Some(image) => {
                                let _ = cr.save();
                                cr.translate(ax, ay);
                                cr.scale(scale, scale);
                                let _ = cr.set_source_surface(&image, 0.0, 0.0);
                                let _ = cr.paint_with_alpha(alpha);
                                let _ = cr.restore();
                            }
                            None => draw_initial(&cr, ax, ay, size, name, dark),
                        }
                        if *running {
                            let dot = if *active { ACTIVE_DOT } else { DOT };
                            text::rounded_rectangle(
                                &cr,
                                x + art / 2.0 - dot / 2.0,
                                f64::from(HEIGHT) - 4.0 - DOT,
                                dot,
                                DOT,
                                DOT / 2.0,
                            );
                            cr.set_source_rgb(r, g, b);
                            let _ = cr.fill();
                        }
                    }
                    // Behind the window's picture, which is drawn over it.
                    Tile::Window { .. } => {
                        text::rounded_rectangle(&cr, ax, ay, size, size, 6.0);
                        cr.set_source_rgba(r, g, b, 0.10);
                        let _ = cr.fill();
                    }
                    Tile::Stack {
                        path, show, covers, ..
                    } => {
                        // As a stack, its newest items piled up, the newest on top.
                        let images: Vec<ImageSurface> = if *show == Show::Stack {
                            covers
                                .iter()
                                .filter_map(|item| self.file_icon(item, dark))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        if images.is_empty() {
                            if let Some(icon) = self.folder_icon(path, ART, dark) {
                                let _ = cr.save();
                                cr.translate(ax, ay);
                                stacks::paint_fitted(&cr, &icon, size, alpha);
                                let _ = cr.restore();
                            }
                        }
                        for (index, image) in images.iter().enumerate().rev() {
                            let (turn, shrink) = [(0.0, 1.0), (-0.08, 0.9), (0.08, 0.82)][index];
                            let piece = size * shrink;
                            let _ = cr.save();
                            cr.translate(ax + size / 2.0, ay + size / 2.0 - index as f64 * 2.0);
                            cr.rotate(turn);
                            cr.translate(-piece / 2.0, -piece / 2.0);
                            stacks::paint_fitted(&cr, image, piece, alpha);
                            let _ = cr.restore();
                        }
                    }
                    Tile::Trash { full } => {
                        let name = if *full { "user-trash-full" } else { "user-trash" };
                        if let Some(icon) = self.dock_icon(name, ART, dark) {
                            let _ = cr.save();
                            cr.translate(ax, ay);
                            cr.scale(scale, scale);
                            let _ = cr.set_source_surface(&icon, 0.0, 0.0);
                            let _ = cr.paint_with_alpha(alpha);
                            let _ = cr.restore();
                        }
                    }
                }
            }
        }
        self.dock.picture = text::picture(surface);
        self.dock.drawn = Some(drawn);
    }

    /// Opens, keeps or closes the pictures of the pointed-at application's windows.
    fn refresh_preview(
        &mut self,
        pointer: Option<Point<f64, Logical>>,
        dark: bool,
        menu_open: bool,
        now: Instant,
    ) {
        let quiet = !menu_open
            && self.dock.press.is_none()
            && self.dock.drag.is_none()
            && self.dock.stack.is_none()
            && self.dock.dnd.is_none()
            && self.overview.is_none()
            && self.dock_shown();
        let candidate =
            self.dock
                .hovered
                .filter(|_| quiet)
                .and_then(|index| match self.dock.tiles.get(index) {
                    Some(Tile::App {
                        app_id,
                        running: true,
                        ..
                    }) => Some((index, app_id.clone())),
                    _ => None,
                });
        let showing = self.dock.preview.as_ref().map(|preview| preview.app_id.clone());
        let dock_top = f64::from(self.dock_rect().map_or(0, |rect| rect.loc.y));
        let target = match (&candidate, showing) {
            _ if !quiet => None,
            // Resting long enough on an icon, or moving to another while pictures show.
            (Some((_, app_id)), showing)
                if showing.is_some()
                    || self
                        .dock
                        .hovered_since
                        .is_some_and(|since| now.duration_since(since) >= PREVIEW_DELAY) =>
            {
                Some(app_id.clone())
            }
            (_, Some(showing)) => {
                let Some(preview) = self.dock.preview.as_mut() else {
                    return;
                };
                // Over the pictures, or on the way down to the icon.
                let inside = pointer.is_some_and(|pos| {
                    let rect = preview.rect.to_f64();
                    pos.x >= rect.loc.x
                        && pos.x < rect.loc.x + rect.size.w
                        && pos.y >= rect.loc.y
                        && pos.y < dock_top
                });
                if inside {
                    preview.away = None;
                    Some(showing)
                } else {
                    let away = *preview.away.get_or_insert(now);
                    (now.duration_since(away) < PREVIEW_LINGER).then_some(showing)
                }
            }
            _ => None,
        };
        let Some(app_id) = target else {
            self.dock.preview = None;
            return;
        };
        // Moving to another icon while pictures show changes them without fading in again.
        let opened = self.dock.preview.as_ref().map_or(now, |preview| preview.opened);
        if self
            .dock
            .preview
            .as_ref()
            .is_some_and(|preview| preview.app_id != app_id)
        {
            self.dock.preview = None;
        }

        let windows: Vec<Window> = self
            .all_windows()
            .filter(|window| is_managed(window) && window_names(window).1 == app_id)
            .cloned()
            .collect();
        let (Some(output), Some(index)) = (
            self.output_rect(),
            self.dock
                .tiles
                .iter()
                .position(|tile| matches!(tile, Tile::App { app_id: id, .. } if *id == app_id)),
        ) else {
            self.dock.preview = None;
            return;
        };
        let fits =
            ((output.size.w - 24 - PREVIEW_PADDING * 2 + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).max(1) as usize;
        let windows: Vec<Window> = windows.into_iter().take(fits).collect();
        if windows.is_empty() {
            self.dock.preview = None;
            return;
        }
        let count = windows.len() as i32;
        let size = Size::from((
            PREVIEW_PADDING * 2 + count * CARD_WIDTH + (count - 1) * CARD_GAP,
            PREVIEW_PADDING * 2 + CARD_PADDING * 2 + CARD_HEADER + 6 + SHOT_HEIGHT,
        ));
        let (Some(dock), Some((start, tile_width))) =
            (self.dock_rect_of(self.dock.width), self.dock.spans.get(index))
        else {
            return;
        };
        let center = dock.loc.x + start + tile_width / 2;
        let x = (center - size.w / 2)
            .min(output.loc.x + output.size.w - 12 - size.w)
            .max(output.loc.x + 12);
        let rect = Rectangle::new((x, dock.loc.y - POPUP_GAP - size.h).into(), size);
        let cards: Vec<Rectangle<i32, Logical>> = (0..count)
            .map(|i| {
                Rectangle::new(
                    (
                        rect.loc.x + PREVIEW_PADDING + i * (CARD_WIDTH + CARD_GAP),
                        rect.loc.y + PREVIEW_PADDING,
                    )
                        .into(),
                    (CARD_WIDTH, size.h - PREVIEW_PADDING * 2).into(),
                )
            })
            .collect();
        let hovered = pointer.and_then(|pos| cards.iter().position(|card| card.to_f64().contains(pos)));
        let close_hovered = hovered
            .zip(pointer)
            .is_some_and(|(index, pos)| Preview::close_button(cards[index]).to_f64().contains(pos));
        let titles: Vec<String> = windows.iter().map(|window| window_names(window).0).collect();
        let key = (titles.clone(), hovered, close_hovered, dark);
        let icon = self.apps.info(&app_id).icon_name.clone();
        let stale = self
            .dock
            .preview
            .as_ref()
            .is_none_or(|preview| preview.drawn.as_ref() != Some(&key) || preview.picture.is_none());
        let picture = if stale {
            let header_icon = icon.as_deref().and_then(|icon| self.dock_icon(icon, 16, dark));
            draw_preview(
                size,
                &cards,
                rect.loc,
                &titles,
                header_icon.as_ref(),
                hovered,
                close_hovered,
                dark,
            )
        } else {
            self.dock
                .preview
                .as_mut()
                .and_then(|preview| preview.picture.take())
        };
        let away = self.dock.preview.as_ref().and_then(|preview| preview.away);
        self.dock.preview = Some(Preview {
            app_id,
            windows,
            rect,
            cards,
            away: if candidate.is_some() { None } else { away },
            opened,
            picture,
            drawn: Some(key),
        });
    }

    /// The name over the pointed-at icon, and the icon being dragged with its tag.
    fn refresh_dock_popups(&mut self, dark: bool, menu_open: bool) {
        let named = self
            .dock
            .hovered
            .filter(|_| {
                !menu_open
                    && self.dock.drag.is_none()
                    && self.dock.preview.is_none()
                    && self.dock.stack.is_none()
            })
            .and_then(|index| self.dock.tiles.get(index))
            .and_then(|tile| Some((tile.key(), tile.name()?.to_string())));
        self.dock.label = match named {
            Some((key, name)) => match self.dock.label.take() {
                Some(label) if label.0 == key && label.1 == dark => Some(label),
                // From one icon's name to the next's, without fading in again.
                previous => {
                    let opened = previous.map_or_else(Instant::now, |label| label.3);
                    label_picture(&name, LABEL_HEIGHT, LABEL_RADIUS, 9.75, dark)
                        .map(|picture| (key, dark, picture, opened))
                }
            },
            None => None,
        };
        if self
            .dock
            .poof
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed().as_secs_f64() >= FADE_OUT_TIME)
        {
            self.dock.poof = None;
        }

        let Some(drag) = &self.dock.drag else {
            self.dock.held = None;
            self.dock.tag = None;
            return;
        };
        let removing = drag.slot.is_none() && drag.kept;
        if self.dock.held.is_none() {
            let size = (f64::from(ART) * HELD_SCALE).round() as i32;
            let (image, name) = match &drag.what {
                Dragged::App { app_id, icon } => {
                    let (icon, name) = (icon.clone(), app_id.clone());
                    (
                        icon.as_deref().and_then(|icon| self.dock_icon(icon, size, dark)),
                        name,
                    )
                }
                Dragged::Stack { path, .. } => {
                    let path = path.clone();
                    (self.folder_icon(&path, size, dark), stacks::folder_name(&path))
                }
            };
            self.dock.held = ImageSurface::create(Format::ARgb32, size, size)
                .ok()
                .and_then(|surface| {
                    {
                        let cr = Context::new(&surface).ok()?;
                        match &image {
                            Some(image) => {
                                cr.set_source_surface(image, 0.0, 0.0).ok()?;
                                cr.paint_with_alpha(0.9).ok()?;
                            }
                            None => draw_initial(&cr, 0.0, 0.0, f64::from(size), &name, dark),
                        }
                    }
                    text::picture(surface)
                });
        }
        self.dock.tag = match self.dock.tag.take() {
            Some(tag) if removing && tag.0 == dark => Some(tag),
            _ if removing => label_picture("移除", 24, 6, 9.0, dark).map(|picture| (dark, picture)),
            _ => None,
        };
    }

    /// What the Dock puts on screen, front first.
    pub fn dock_overlays(&self) -> Vec<Overlay<'_>> {
        if !self.dock_shown() {
            return Vec::new();
        }
        let (Some(rect), Some(picture), Some(ids)) = (self.dock_rect(), &self.dock.picture, &self.dock.ids)
        else {
            return Vec::new();
        };
        let mut overlays = Vec::new();
        if let Some((stack, picture)) = self
            .dock
            .stack
            .as_ref()
            .and_then(|stack| Some((stack, stack.picture.as_ref()?)))
        {
            let grid = matches!(stack.view, Opened::Grid { .. });
            overlays.push(Overlay {
                fade: if grid {
                    ease_out(stack.opened.elapsed().as_secs_f64() / FADE_IN_TIME) as f32
                } else {
                    1.0
                },
                id: &ids.stack,
                rect: stack.rect,
                glass: grid.then_some(stacks::GRID_RADIUS),
                alpha: 1.0,
                picture,
            });
        }
        let pointer = self.seat.get_pointer().map(|pointer| pointer.current_location());
        if let (Some(drag), Some(held), Some(pos)) = (&self.dock.drag, &self.dock.held, pointer) {
            let at = Point::from((
                (pos.x - drag.grab.x * HELD_SCALE).round() as i32,
                (pos.y - drag.grab.y * HELD_SCALE).round() as i32,
            ));
            if let Some((_, tag)) = &self.dock.tag {
                overlays.push(Overlay {
                    fade: 1.0,
                    id: &ids.tag,
                    rect: Rectangle::new(
                        (at.x + held.size.w / 2 - tag.size.w / 2, at.y - 8 - tag.size.h).into(),
                        tag.size,
                    ),
                    glass: Some(6),
                    alpha: 1.0,
                    picture: tag,
                });
            }
            overlays.push(Overlay {
                fade: 1.0,
                id: &ids.held,
                rect: Rectangle::new(at, held.size),
                glass: None,
                alpha: 1.0,
                picture: held,
            });
        }
        let fade_in = |opened: &Instant| ease_out(opened.elapsed().as_secs_f64() / FADE_IN_TIME) as f32;
        if let Some((picture, at, gone)) = &self.dock.poof {
            overlays.push(Overlay {
                fade: (1.0 - gone.elapsed().as_secs_f64() / FADE_OUT_TIME).max(0.0) as f32,
                id: &ids.held,
                rect: Rectangle::new(*at, picture.size),
                glass: None,
                alpha: 1.0,
                picture,
            });
        }
        if let Some((key, _, label, opened)) = &self.dock.label {
            if let Some(tile) = self
                .dock
                .tiles
                .iter()
                .position(|tile| tile.key() == *key)
                .and_then(|index| self.tile_rect(index))
            {
                overlays.push(Overlay {
                    fade: fade_in(opened),
                    id: &ids.label,
                    rect: Rectangle::new(
                        (
                            tile.loc.x + tile.size.w / 2 - label.size.w / 2,
                            rect.loc.y - POPUP_GAP - label.size.h,
                        )
                            .into(),
                        label.size,
                    ),
                    glass: Some(LABEL_RADIUS),
                    alpha: 1.0,
                    picture: label,
                });
            }
        }
        if let Some((preview, picture)) = self
            .dock
            .preview
            .as_ref()
            .and_then(|preview| Some((preview, preview.picture.as_ref()?)))
        {
            overlays.push(Overlay {
                fade: fade_in(&preview.opened),
                id: &ids.preview,
                rect: preview.rect,
                glass: Some(PREVIEW_RADIUS),
                alpha: 1.0,
                picture,
            });
        }
        overlays.push(Overlay {
            fade: 1.0,
            id: &ids.dock,
            rect,
            glass: Some(RADIUS),
            alpha: 1.0,
            picture,
        });
        overlays
    }

    /// The windows pictured on the Dock, live: each minimized one in its tile and, while
    /// they show, the pointed-at application's in their cards. Each is fitted to its rectangle.
    pub fn dock_thumbnails(&self) -> Vec<(Window, Rectangle<i32, Logical>)> {
        if !self.dock_shown() {
            return Vec::new();
        }
        let mut thumbnails = Vec::new();
        if let Some(preview) = &self.dock.preview {
            for (window, card) in preview.windows.iter().zip(&preview.cards) {
                thumbnails.push((window.clone(), Preview::shot(*card)));
            }
        }
        for (index, tile) in self.dock.tiles.iter().enumerate() {
            if let (Tile::Window { window, .. }, Some(rect)) = (tile, self.tile_rect(index)) {
                let inset = 3;
                let mut rect = Rectangle::new(
                    rect.loc + Point::from((inset, inset)),
                    rect.size - Size::from((inset * 2, inset * 2)),
                );
                // A window just minimized shrinks from where it was into its tile.
                if let Some((_, from, at)) = self
                    .dock
                    .minimizing
                    .iter()
                    .find(|(minimizing, ..)| minimizing == window)
                {
                    rect = mix_rect(*from, rect, ease_out(at.elapsed().as_secs_f64() / MINIMIZE_TIME));
                }
                thumbnails.push((window.clone(), rect));
            }
        }
        thumbnails
    }

    /// The application's icon at the corner of each minimized window's tile, drawn over
    /// its picture.
    pub fn dock_badges(&mut self) {
        let dark = self.menu_bar_dark();
        let wanted: Vec<Option<String>> = self
            .dock
            .tiles
            .iter()
            .filter_map(|tile| match tile {
                Tile::Window { icon, .. } => Some(icon.clone()),
                _ => None,
            })
            .collect();
        for icon in wanted {
            if self
                .dock
                .badges
                .iter()
                .any(|(cached, cached_dark, _)| *cached == icon && *cached_dark == dark)
            {
                continue;
            }
            // A picture takes the surface's pixels, which cairo gives out only for a surface
            // nothing else holds; the cached icon is held by the cache.
            let picture = icon
                .as_deref()
                .and_then(|icon| self.dock_icon(icon, BADGE, dark))
                .and_then(|icon| {
                    let copy = ImageSurface::create(Format::ARgb32, icon.width(), icon.height()).ok()?;
                    {
                        let cr = Context::new(&copy).ok()?;
                        cr.set_source_surface(&icon, 0.0, 0.0).ok()?;
                        cr.paint().ok()?;
                    }
                    text::picture(copy)
                });
            self.dock.badges.push((icon, dark, picture));
        }
    }

    /// The badges [`Self::dock_badges`] made, and where they go.
    pub fn dock_badge_pictures(&self) -> Vec<(&Picture, Point<i32, Logical>)> {
        if !self.dock_shown() {
            return Vec::new();
        }
        let dark = self.menu_bar_dark();
        self.dock
            .tiles
            .iter()
            .enumerate()
            .filter_map(|(index, tile)| {
                let Tile::Window { icon, .. } = tile else {
                    return None;
                };
                let rect = self.tile_rect(index)?;
                let picture = self
                    .dock
                    .badges
                    .iter()
                    .find(|(cached, cached_dark, _)| cached == icon && *cached_dark == dark)?
                    .2
                    .as_ref()?;
                Some((
                    picture,
                    rect.loc + Point::from((rect.size.w - BADGE + 4, rect.size.h - BADGE + 4)),
                ))
            })
            .collect()
    }

    /// Makes the ids the Dock's elements go by; called once the renderer is up.
    pub fn dock_ids(&mut self) {
        if self.dock.ids.is_none() {
            self.dock.ids = Some(Ids {
                dock: Id::new(),
                label: Id::new(),
                preview: Id::new(),
                stack: Id::new(),
                held: Id::new(),
                tag: Id::new(),
            });
        }
        self.dock_badges();
    }

    /// Whether the Dock, or the pictures over it, are at a position, so that it is not a
    /// window's.
    pub fn dock_covers(&self, pos: Point<f64, Logical>) -> bool {
        self.dock_shown()
            && (self.dock_rect().is_some_and(|rect| rect.to_f64().contains(pos))
                || self
                    .dock
                    .preview
                    .as_ref()
                    .is_some_and(|preview| preview.rect.to_f64().contains(pos))
                || self
                    .dock
                    .stack
                    .as_ref()
                    .is_some_and(|stack| stack.rect.to_f64().contains(pos)))
    }

    fn dock_tile_at(&self, pos: Point<f64, Logical>) -> Option<usize> {
        let rect = self.dock_rect_of(self.dock.width)?;
        if !rect.to_f64().contains(pos) {
            return None;
        }
        let x = pos.x as i32 - rect.loc.x;
        self.dock
            .spans
            .iter()
            .position(|(start, width)| x >= *start - GAP / 2 && x < start + width + GAP / 2)
            .filter(|&index| !matches!(self.dock.tiles[index], Tile::Separator(_)))
    }

    /// The pointer moved: the icon under it lifts and is named, a press far enough becomes
    /// a drag.
    pub fn dock_motion(&mut self, pos: Point<f64, Logical>) {
        self.dock.pointer = Some(pos);
        let hovered = if self.dock_shown() && self.menu_bar.open.is_none() {
            self.dock_tile_at(pos)
        } else {
            None
        };
        if hovered != self.dock.hovered {
            self.dock.hovered_since = Some(Instant::now());
        }
        self.dock.hovered = hovered;

        let Some(press) = &self.dock.press else {
            return;
        };
        let moved = (pos.x - press.start.x).hypot(pos.y - press.start.y) > DRAG_THRESHOLD;
        let what = match &press.tile {
            Tile::App {
                app_id, icon, kept, ..
            } => Some((
                Dragged::App {
                    app_id: app_id.clone(),
                    icon: icon.clone(),
                },
                *kept,
            )),
            Tile::Stack { index, path, .. } => Some((
                Dragged::Stack {
                    index: *index,
                    path: path.clone(),
                },
                true,
            )),
            _ => None,
        };
        if let (true, Some((what, kept))) = (moved, what) {
            let grab = self
                .dock
                .tiles
                .iter()
                .position(|tile| tile.key() == press.key)
                .and_then(|index| self.tile_rect(index))
                .map_or(
                    Point::from((f64::from(ART) / 2.0, f64::from(ART) / 2.0)),
                    |rect| press.start - rect.loc.to_f64(),
                );
            self.dock.drag = Some(Drag {
                what,
                kept,
                grab,
                slot: None,
            });
            self.dock.press = None;
            self.dock.preview = None;
        }
    }

    /// A button went down. Returns whether the Dock took it.
    pub fn dock_press(&mut self, pos: Point<f64, Logical>, right: bool) -> bool {
        if !self.dock_shown() {
            return false;
        }
        // An open folder: an item opens and the folder closes; anywhere else only closes it.
        if let Some(stack) = self.dock.stack.take() {
            if stack.rect.to_f64().contains(pos) && !right {
                let open = match &stack.view {
                    Opened::Fan { fan, anchor, .. } => fan.hit(pos - *anchor).map(|index| {
                        stack
                            .items
                            .get(index)
                            .filter(|_| index < stacks::FAN_MAX)
                            .map_or(stack.folder.clone(), |item| item.path.clone())
                    }),
                    Opened::Grid { grid, scroll } => {
                        let local = (pos - stack.rect.loc.to_f64()).to_i32_round();
                        if grid.button(&open_label(&stack.name)).contains(local) {
                            Some(stack.folder.clone())
                        } else {
                            (0..grid.columns * grid.rows)
                                .find(|&index| grid.cell(index).contains(local))
                                .and_then(|index| stack.items.get(scroll * grid.columns + index))
                                .map(|item| item.path.clone())
                        }
                    }
                };
                match open {
                    Some(path) => open_path(&path),
                    // Between the items of a fan, or on the grid's glass: it stays open.
                    None if matches!(stack.view, Opened::Grid { .. }) => self.dock.stack = Some(stack),
                    None => {}
                }
                return true;
            }
            // A click on the folder's own icon only closes it.
            let own = self
                .dock_tile_at(pos)
                .and_then(|index| self.dock.tiles.get(index))
                .is_some_and(|tile| matches!(tile, Tile::Stack { path, .. } if *path == stack.folder));
            if own || !self.dock_covers(pos) {
                return true;
            }
        }
        // On the pictures of the windows: the close button, or a picture to go to it.
        if let Some(preview) = self
            .dock
            .preview
            .as_ref()
            .filter(|preview| preview.rect.to_f64().contains(pos))
        {
            let picked = preview
                .cards
                .iter()
                .position(|card| card.to_f64().contains(pos))
                .map(|index| {
                    let on_close = Preview::close_button(preview.cards[index]).to_f64().contains(pos);
                    (preview.windows[index].clone(), on_close)
                });
            match picked {
                Some((window, true)) => close(&window),
                Some((window, false)) if !right => {
                    self.dock.preview = None;
                    self.reveal_window(&window);
                }
                _ => {}
            }
            return true;
        }
        if !self.dock_covers(pos) {
            return false;
        }
        let Some(index) = self.dock_tile_at(pos) else {
            return true;
        };
        let tile = self.dock.tiles[index].clone();
        self.dock.preview = None;
        if right {
            if let Tile::App {
                app_id,
                running,
                kept,
                ..
            } = &tile
            {
                let entries = self.dock_menu(app_id, *running, *kept);
                self.open_tile_menu(index, entries);
            }
            if let Tile::Stack { index: stack, .. } = &tile {
                let entries = self.stack_menu(*stack);
                self.open_tile_menu(index, entries);
            }
            if let Tile::Trash { full } = &tile {
                let entries = vec![
                    Entry::item("打開", Command::Dock(DockCommand::OpenTrash)),
                    Entry::separator(),
                    Entry::item("清空垃圾桶", Command::Dock(DockCommand::EmptyTrash)).enabled(*full),
                ];
                self.open_tile_menu(index, entries);
            }
            return true;
        }
        self.dock.press = Some(Press {
            key: tile.key(),
            tile,
            start: pos,
            at: Instant::now(),
        });
        true
    }

    /// A button came up: a press let go where it went down is a click; a dragged icon is
    /// put where it was let go, or taken off the Dock.
    pub fn dock_release(&mut self, pos: Point<f64, Logical>) {
        if let Some(drag) = self.dock.drag.take() {
            // Let go away from the Dock: a kept one comes off it, fading where it was let go;
            // another stays as it was.
            if drag.slot.is_none() {
                if !drag.kept {
                    return;
                }
                let at = Point::from((
                    (pos.x - drag.grab.x * HELD_SCALE).round() as i32,
                    (pos.y - drag.grab.y * HELD_SCALE).round() as i32,
                ));
                self.dock.poof = self.dock.held.take().map(|held| (held, at, Instant::now()));
            }
            match drag.what {
                Dragged::App { app_id, .. } => {
                    let mut kept: Vec<String> = self
                        .config
                        .dock
                        .iter()
                        .filter(|kept| !kept.eq_ignore_ascii_case(&app_id))
                        .cloned()
                        .collect();
                    if let Some(slot) = drag.slot {
                        kept.insert(slot.min(kept.len()), app_id);
                    }
                    if kept != self.config.dock {
                        self.config.dock = kept;
                        config::save_dock(&self.config.dock);
                    }
                }
                Dragged::Stack { index, .. } if index < self.config.stacks.len() => {
                    let mut stacks = self.config.stacks.clone();
                    let stack = stacks.remove(index);
                    if let Some(slot) = drag.slot {
                        stacks.insert(slot.min(stacks.len()), stack);
                    }
                    if stacks != self.config.stacks {
                        self.config.stacks = stacks;
                        config::save_stacks(&self.config.stacks);
                    }
                }
                Dragged::Stack { .. } => {}
            }
            return;
        }
        let Some(press) = self.dock.press.take() else {
            return;
        };
        let here = self
            .dock_tile_at(pos)
            .and_then(|index| self.dock.tiles.get(index))
            .is_some_and(|tile| tile.key() == press.key);
        if !here {
            return;
        }
        self.dock.bounce = Some((press.key, Instant::now()));
        match press.tile {
            Tile::Apps => config::spawn(&self.config.launcher),
            Tile::App { app_id, running, .. } => {
                if running {
                    self.bring_app(&app_id);
                } else {
                    self.launch_app(&app_id);
                    self.dock.launching.retain(|(id, _)| *id != app_id);
                    self.dock.launching.push((app_id, Instant::now()));
                }
            }
            Tile::Window { window, .. } => self.reveal_window(&window),
            Tile::Stack { index, .. } => self.open_stack(index),
            Tile::Trash { .. } => open_uri("trash:///"),
            Tile::Separator(_) => {}
        }
    }

    /// The menu for an application in the Dock: its windows, then what can be done with it.
    fn dock_menu(&mut self, app_id: &str, running: bool, kept: bool) -> Vec<Entry> {
        let focused = self.focused_window();
        let windows: Vec<Window> = self
            .all_windows()
            .filter(|window| is_managed(window) && window_names(window).1 == app_id)
            .cloned()
            .collect();
        let mut entries = Vec::new();
        if !windows.is_empty() {
            entries.push(Entry::note("視窗"));
            for window in windows {
                let title = window_names(&window).0;
                let title = if title.is_empty() {
                    self.apps
                        .info(app_id)
                        .name
                        .clone()
                        .unwrap_or_else(|| app_id.to_string())
                } else {
                    title
                };
                let checked = focused.as_ref() == Some(&window);
                entries.push(Entry::item(title, Command::Focus(window)).checked(checked));
            }
            entries.push(Entry::separator());
        }
        let id = app_id.to_string();
        if running {
            entries.push(Entry::item(
                "新增視窗",
                Command::Dock(DockCommand::NewWindow(id.clone())),
            ));
        }
        entries.push(
            Entry::item("釘選到 Dock", Command::Dock(DockCommand::TogglePin(id.clone()))).checked(kept),
        );
        if running {
            entries.push(Entry::separator());
            entries.push(Entry::item("隱藏", Command::Dock(DockCommand::Hide(id.clone()))));
            entries.push(Entry::item("結束", Command::Dock(DockCommand::Quit(id))));
        }
        entries
    }

    pub fn run_dock_command(&mut self, command: DockCommand) {
        match command {
            DockCommand::NewWindow(app_id) => self.launch_app(&app_id),
            DockCommand::TogglePin(app_id) => {
                let before = self.config.dock.len();
                self.config
                    .dock
                    .retain(|kept| !kept.eq_ignore_ascii_case(&app_id));
                if self.config.dock.len() == before {
                    self.config.dock.push(app_id);
                }
                config::save_dock(&self.config.dock);
            }
            DockCommand::Hide(app_id) => {
                for window in self.app_windows(&app_id) {
                    self.minimize(&window);
                }
            }
            DockCommand::Quit(app_id) => self.quit_app(&app_id),
            DockCommand::Open(path) => open_path(&path),
            DockCommand::StackSort(index, sort) => self.change_stack(index, |stack| stack.sort = sort),
            DockCommand::StackShow(index, show) => self.change_stack(index, |stack| stack.show = show),
            DockCommand::StackView(index, view) => self.change_stack(index, |stack| stack.view = view),
            DockCommand::RemoveStack(index) => {
                if index < self.config.stacks.len() {
                    self.config.stacks.remove(index);
                    config::save_stacks(&self.config.stacks);
                }
            }
            DockCommand::RevealStack(index) => {
                if let Some(stack) = self.config.stacks.get(index) {
                    let uri = gio::File::for_path(stack.folder()).uri();
                    // The file manager shows the folder selected in the one it is in.
                    config::spawn(&format!(
                        "gdbus call --session --dest org.freedesktop.FileManager1 \
                         --object-path /org/freedesktop/FileManager1 \
                         --method org.freedesktop.FileManager1.ShowItems \"['{uri}']\" ''"
                    ));
                }
            }
            DockCommand::OpenTrash => open_uri("trash:///"),
            // Asked first, in a dialog of its own: what is in the Trash cannot come back.
            DockCommand::EmptyTrash => config::spawn("nekoland-confirm empty-trash"),
        }
    }

    /// A window was minimized from `rect`; it is drawn shrinking into the Dock.
    pub fn dock_minimizing(&mut self, window: &Window, rect: Rectangle<i32, Logical>) {
        self.dock
            .minimizing
            .retain(|(minimizing, ..)| minimizing != window);
        self.dock.minimizing.push((window.clone(), rect, Instant::now()));
    }

    fn change_stack(&mut self, index: usize, change: impl FnOnce(&mut Stack)) {
        if let Some(stack) = self.config.stacks.get_mut(index) {
            change(stack);
            config::save_stacks(&self.config.stacks);
        }
    }

    /// Opens a menu over a tile of the Dock, from the Dock's top edge.
    fn open_tile_menu(&mut self, index: usize, entries: Vec<Entry>) {
        if let (Some(tile), Some(dock)) = (self.tile_rect(index), self.dock_rect()) {
            let anchor = Rectangle::new((tile.loc.x, dock.loc.y).into(), (tile.size.w, HEIGHT).into());
            self.open_menu_above(entries, anchor, POPUP_GAP);
        }
    }

    /// The menu of a kept folder: how its items are sorted, how it shows in the Dock and
    /// how it opens, and the way to open it.
    fn stack_menu(&mut self, index: usize) -> Vec<Entry> {
        let Some(stack) = self.config.stacks.get(index).cloned() else {
            return Vec::new();
        };
        let item = |label: &str, command: DockCommand, checked: bool| {
            Entry::item(label, Command::Dock(command)).checked(checked)
        };
        let mut entries = vec![Entry::note("排序方式")];
        for (sort, label) in [
            (Sort::Name, "名稱"),
            (Sort::Added, "加入日期"),
            (Sort::Modified, "修改日期"),
            (Sort::Created, "建立日期"),
            (Sort::Kind, "種類"),
        ] {
            entries.push(item(
                label,
                DockCommand::StackSort(index, sort),
                stack.sort == sort,
            ));
        }
        entries.push(Entry::separator());
        entries.push(Entry::note("顯示為"));
        for (show, label) in [(Show::Folder, "檔案夾"), (Show::Stack, "疊放")] {
            entries.push(item(
                label,
                DockCommand::StackShow(index, show),
                stack.show == show,
            ));
        }
        entries.push(Entry::separator());
        entries.push(Entry::note("檢視內容為"));
        for (view, label) in [
            (View::Fan, "扇形"),
            (View::Grid, "格狀"),
            (View::List, "列表"),
            (View::Auto, "自動"),
        ] {
            entries.push(item(
                label,
                DockCommand::StackView(index, view),
                stack.view == view,
            ));
        }
        entries.push(Entry::separator());
        entries.push(Entry::submenu(
            "選項",
            vec![
                Entry::item("從 Dock 移除", Command::Dock(DockCommand::RemoveStack(index))),
                Entry::item("在檔案中顯示", Command::Dock(DockCommand::RevealStack(index))),
            ],
        ));
        entries.push(Entry::separator());
        let folder = stack.folder();
        entries.push(Entry::item(
            open_label(&stacks::folder_name(&folder)),
            Command::Dock(DockCommand::Open(folder)),
        ));
        entries
    }

    /// Opens a kept folder over its icon, as its view asks.
    fn open_stack(&mut self, index: usize) {
        let Some(stack) = self.config.stacks.get(index).cloned() else {
            return;
        };
        let folder = stack.folder();
        let items = self.folder_items(&folder, stack.sort).to_vec();
        let name = stacks::folder_name(&folder);
        let Some(tile) = self
            .dock
            .tiles
            .iter()
            .position(|tile| matches!(tile, Tile::Stack { index: at, .. } if *at == index))
        else {
            return;
        };
        let (Some(dock), Some(output), Some((start, width))) = (
            self.dock_rect_of(self.dock.width),
            self.output_rect(),
            self.dock.spans.get(tile).copied(),
        ) else {
            return;
        };
        self.dock.preview = None;
        let view = match stack.view {
            View::Auto if items.len() <= stacks::AUTO_FAN => View::Fan,
            View::Auto => View::Grid,
            view => view,
        };
        let center = dock.loc.x + start + width / 2;
        let (opened, rect) = match view {
            View::List => {
                let entries = self.list_entries(&items, &folder, &name, stack.sort);
                self.open_tile_menu(tile, entries);
                return;
            }
            View::Fan => {
                let shown = items.len().min(stacks::FAN_MAX);
                let mut labels: Vec<String> = items[..shown].iter().map(|item| item.name.clone()).collect();
                labels.push(if items.len() > shown {
                    format!(
                        "在檔案中打開其他 {} 個項目",
                        stacks::count_text(items.len() - shown)
                    )
                } else {
                    "在檔案中打開".to_string()
                });
                let fan = Fan::new(&labels);
                let anchor = Point::from((f64::from(center), f64::from(dock.loc.y + TOP)));
                let rect = Rectangle::new((anchor + fan.origin).to_i32_round(), fan.size);
                (Opened::Fan { fan, anchor, labels }, rect)
            }
            View::Grid | View::Auto => {
                let grid = Grid::new(items.len());
                let x = (center - grid.size.w / 2)
                    .min(output.loc.x + output.size.w - 12 - grid.size.w)
                    .max(output.loc.x + 12);
                let rect = Rectangle::new((x, dock.loc.y - POPUP_GAP - grid.size.h).into(), grid.size);
                (Opened::Grid { grid, scroll: 0 }, rect)
            }
        };
        self.dock.stack = Some(OpenStack {
            folder,
            name,
            items,
            view: opened,
            rect,
            opened: Instant::now(),
            hovered: None,
            button_hovered: false,
            picture: None,
            drawn: None,
        });
    }

    /// A folder's items as menu entries with their icons, a subfolder's as its submenu.
    fn list_entries(&mut self, items: &[Item], folder: &Path, name: &str, sort: Sort) -> Vec<Entry> {
        let mut entries: Vec<Entry> = items
            .iter()
            .take(LIST_MAX)
            .map(|item| {
                let icon = stacks::icon_names(&item.path);
                if item.is_dir {
                    let mut children = vec![
                        Entry::item(
                            open_label(&item.name),
                            Command::Dock(DockCommand::Open(item.path.clone())),
                        ),
                        Entry::separator(),
                    ];
                    children.extend(
                        stacks::read_folder(&item.path, sort)
                            .into_iter()
                            .take(LIST_MAX)
                            .map(|child| {
                                Entry::item(
                                    child.name.clone(),
                                    Command::Dock(DockCommand::Open(child.path.clone())),
                                )
                                .with_icon(stacks::icon_names(&child.path))
                            }),
                    );
                    Entry::submenu(item.name.clone(), children).with_icon(icon)
                } else {
                    Entry::item(
                        item.name.clone(),
                        Command::Dock(DockCommand::Open(item.path.clone())),
                    )
                    .with_icon(icon)
                }
            })
            .collect();
        if !entries.is_empty() {
            entries.push(Entry::separator());
        }
        let rest = items.len().saturating_sub(LIST_MAX);
        let open = if rest > 0 {
            format!("在檔案中打開其他 {} 個項目", stacks::count_text(rest))
        } else {
            open_label(name)
        };
        entries.push(Entry::item(
            open,
            Command::Dock(DockCommand::Open(folder.to_path_buf())),
        ));
        entries
    }

    /// Lights what the pointer is on in the open folder, and draws it again when that, or
    /// how far the fan has come out, changed.
    fn refresh_stack(&mut self, pointer: Option<Point<f64, Logical>>, dark: bool) {
        let Some(stack) = self.dock.stack.as_mut() else {
            return;
        };
        let progress = ease_out(stack.opened.elapsed().as_secs_f64() / FAN_TIME);
        let (hovered, button_hovered, scroll) = match &stack.view {
            Opened::Fan { fan, anchor, .. } => (pointer.and_then(|pos| fan.hit(pos - *anchor)), false, 0),
            Opened::Grid { grid, scroll } => {
                let local = pointer.map(|pos| (pos - stack.rect.loc.to_f64()).to_i32_round());
                let cell = local.and_then(|local| {
                    (0..grid.columns * grid.rows)
                        .find(|&index| grid.cell(index).contains(local))
                        .filter(|index| scroll * grid.columns + index < stack.items.len())
                });
                let on_button =
                    local.is_some_and(|local| grid.button(&open_label(&stack.name)).contains(local));
                (cell, on_button, *scroll)
            }
        };
        stack.hovered = hovered;
        stack.button_hovered = button_hovered;
        let key = (
            hovered,
            button_hovered,
            dark,
            scroll,
            (progress * 40.0).round() as u8,
        );
        if stack.drawn == Some(key) && stack.picture.is_some() {
            return;
        }
        stack.drawn = Some(key);
        let shown: Vec<Item> = match &stack.view {
            Opened::Fan { labels, .. } => stack.items.iter().take(labels.len() - 1).cloned().collect(),
            Opened::Grid { grid, scroll } => stack
                .items
                .iter()
                .skip(scroll * grid.columns)
                .take(grid.columns * grid.rows)
                .cloned()
                .collect(),
        };
        let icons: Vec<Option<ImageSurface>> = shown.iter().map(|item| self.file_icon(item, dark)).collect();
        let Some(stack) = self.dock.stack.as_mut() else {
            return;
        };
        stack.picture = match &stack.view {
            Opened::Fan { fan, labels, .. } => fan.draw(labels, &icons, progress, hovered, dark),
            Opened::Grid { grid, .. } => {
                let cells: Vec<(String, Option<ImageSurface>)> =
                    shown.iter().map(|item| item.name.clone()).zip(icons).collect();
                grid.draw(
                    &stack.name,
                    &open_label(&stack.name),
                    &cells,
                    hovered,
                    button_hovered,
                    dark,
                )
            }
        };
    }

    /// The wheel over an open grid turns its pages, a row at a time. Returns whether it
    /// was taken.
    pub fn dock_scroll(&mut self, pos: Point<f64, Logical>, amount: f64) -> bool {
        let Some(stack) = self
            .dock
            .stack
            .as_mut()
            .filter(|stack| stack.rect.to_f64().contains(pos))
        else {
            return false;
        };
        if let Opened::Grid { grid, scroll } = &mut stack.view {
            let rows = stack.items.len().div_ceil(grid.columns);
            let last = rows.saturating_sub(grid.rows);
            *scroll = if amount > 0.0 {
                (*scroll + 1).min(last)
            } else if amount < 0.0 {
                scroll.saturating_sub(1)
            } else {
                *scroll
            };
        }
        true
    }

    /// Closes an open folder. Returns whether one was open.
    pub fn close_dock_stack(&mut self) -> bool {
        self.dock.stack.take().is_some()
    }

    /// A file manager started dragging: what it drags is asked for at once, as a list of
    /// files, so that the Dock knows it if it is dropped there.
    pub fn dock_drag_started(&mut self, source: Option<&WlDataSource>) {
        self.dock.dnd = None;
        let Some(source) = source else {
            return;
        };
        let offers = with_source_metadata(source, |meta| {
            meta.mime_types.iter().any(|mime| mime == "text/uri-list")
        })
        .unwrap_or(false);
        if !offers {
            return;
        }
        let mut fds = [0; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            tracing::warn!("pipe: {}", std::io::Error::last_os_error());
            return;
        }
        // SAFETY: pipe2 just made these two descriptors, which nothing else owns.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        source.send("text/uri-list".into(), write.as_fd());
        drop(write);
        let found = Arc::new(Mutex::new(None));
        let into = found.clone();
        // The program writes it when it reads its socket, after this frame's events go out.
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Err(err) = std::fs::File::from(read).read_to_string(&mut text) {
                tracing::warn!("reading a drag's files: {err}");
            }
            if let Ok(mut found) = into.lock() {
                *found = Some(uri_paths(&text));
            }
        });
        self.dock.dnd = Some(found);
    }

    /// A drag ended. Dropped on the Dock, where no program took it: on an application, the
    /// files open in it; on the Trash, they go there; elsewhere, folders are kept.
    pub fn dock_dropped(&mut self, validated: bool) {
        let Some(found) = self.dock.dnd.take() else {
            return;
        };
        // Where the pointer was last seen: the drag holds the pointer while this runs, and
        // asking it where it is now would wait on itself.
        let Some(pos) = self.dock.pointer else {
            return;
        };
        if validated || !self.dock_covers(pos) {
            return;
        }
        let paths = found
            .lock()
            .ok()
            .and_then(|found| found.clone())
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        let tile = self
            .dock_tile_at(pos)
            .and_then(|index| self.dock.tiles.get(index))
            .cloned();
        match tile {
            Some(Tile::App { app_id, .. }) => {
                let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
                match gio::DesktopAppInfo::new(&format!("{app_id}.desktop")) {
                    Some(app) => {
                        if let Err(err) = app.launch(&files, None::<&gio::AppLaunchContext>) {
                            tracing::warn!("{app_id}: {err}");
                        }
                    }
                    None => tracing::warn!("{app_id}: no desktop entry to open files with"),
                }
            }
            Some(Tile::Trash { .. }) => {
                for path in &paths {
                    if let Err(err) = gio::File::for_path(path).trash(gio::Cancellable::NONE) {
                        tracing::warn!("{}: {err}", path.display());
                    }
                }
                self.dock.trash = None;
            }
            _ => {
                let home = std::env::var("HOME").unwrap_or_default();
                let before = self.config.stacks.len();
                for path in paths.iter().filter(|path| path.is_dir()) {
                    let written = match path.strip_prefix(&home) {
                        Ok(rest) if !home.is_empty() => format!("~/{}", rest.display()),
                        _ => path.display().to_string(),
                    };
                    if !self.config.stacks.iter().any(|stack| stack.folder() == *path) {
                        self.config.stacks.push(Stack::new(written));
                    }
                }
                if self.config.stacks.len() != before {
                    config::save_stacks(&self.config.stacks);
                }
            }
        }
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

fn mix_rect(from: Rectangle<i32, Logical>, to: Rectangle<i32, Logical>, t: f64) -> Rectangle<i32, Logical> {
    let mix = |a: i32, b: i32| (f64::from(a) + f64::from(b - a) * t).round() as i32;
    Rectangle::new(
        (mix(from.loc.x, to.loc.x), mix(from.loc.y, to.loc.y)).into(),
        (mix(from.size.w, to.size.w), mix(from.size.h, to.size.h)).into(),
    )
}

/// The menu entry and button that open a folder in the file manager.
fn open_label(name: &str) -> String {
    format!("打開「{name}」")
}

/// Opens a file or folder in the program the desktop opens it with.
fn open_path(path: &Path) {
    open_uri(&gio::File::for_path(path).uri());
}

fn open_uri(uri: &str) {
    if let Err(err) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        tracing::warn!("{uri}: {err}");
    }
}

/// The local files in a `text/uri-list`, as dragged out of a file manager.
fn uri_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|uri| gio::File::for_uri(uri).path())
        .collect()
}

/// The pane of window pictures: a card for each window with its icon and title, the card
/// under the pointer shaded with its close button showing. The pictures themselves are
/// drawn live over it.
#[allow(clippy::too_many_arguments)]
fn draw_preview(
    size: Size<i32, Logical>,
    cards: &[Rectangle<i32, Logical>],
    origin: Point<i32, Logical>,
    titles: &[String],
    icon: Option<&ImageSurface>,
    hovered: Option<usize>,
    close_hovered: bool,
    dark: bool,
) -> Option<Picture> {
    let surface = ImageSurface::create(Format::ARgb32, size.w, size.h).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        paint_pane(
            &cr,
            f64::from(size.w),
            f64::from(size.h),
            f64::from(PREVIEW_RADIUS),
            dark,
            true,
        );
        let (r, g, b) = ink(dark);
        let font = pango::FontDescription::from_string(&format!("{FONT} 9.75"));
        for (index, (card, title)) in cards.iter().zip(titles).enumerate() {
            let card = Rectangle::<i32, Logical>::new(card.loc - origin, card.size);
            let (x, y) = (f64::from(card.loc.x), f64::from(card.loc.y));
            let lit = hovered == Some(index);
            if lit {
                text::rounded_rectangle(&cr, x, y, f64::from(card.size.w), f64::from(card.size.h), 8.0);
                cr.set_source_rgba(r, g, b, 0.10);
                cr.fill().ok()?;
            }
            let header_y = y + f64::from(CARD_PADDING);
            if let Some(icon) = icon {
                cr.set_source_surface(icon, x + f64::from(CARD_PADDING) + 2.0, header_y + 4.0)
                    .ok()?;
                cr.paint().ok()?;
            }
            let layout = pangocairo::functions::create_layout(&cr);
            layout.set_font_description(Some(&font));
            layout.set_text(title);
            layout.set_width((CARD_WIDTH - CARD_PADDING * 2 - 16 - 12 - CLOSE - 6) * pango::SCALE);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            let text_height = layout.pixel_size().1;
            cr.set_source_rgb(r, g, b);
            cr.move_to(
                x + f64::from(CARD_PADDING + 2 + 16 + 6),
                header_y + f64::from(CARD_HEADER - text_height) / 2.0,
            );
            pangocairo::functions::show_layout(&cr, &layout);

            let shot = Preview::shot(card);
            text::rounded_rectangle(
                &cr,
                f64::from(shot.loc.x),
                f64::from(shot.loc.y),
                f64::from(shot.size.w),
                f64::from(shot.size.h),
                6.0,
            );
            cr.set_source_rgba(0.0, 0.0, 0.0, if dark { 0.25 } else { 0.10 });
            cr.fill().ok()?;

            if lit {
                let button = Preview::close_button(card);
                let (cx, cy) = (
                    f64::from(button.loc.x) + f64::from(CLOSE) / 2.0,
                    f64::from(button.loc.y) + f64::from(CLOSE) / 2.0,
                );
                cr.arc(cx, cy, f64::from(CLOSE) / 2.0, 0.0, std::f64::consts::TAU);
                cr.set_source_rgba(r, g, b, if close_hovered { 0.22 } else { 0.10 });
                cr.fill().ok()?;
                let (r2, g2, b2) = ink_2(dark);
                cr.set_source_rgb(r2, g2, b2);
                cr.set_line_width(1.6);
                cr.set_line_cap(cairo::LineCap::Round);
                cr.move_to(cx - 3.5, cy - 3.5);
                cr.line_to(cx + 3.5, cy + 3.5);
                cr.move_to(cx + 3.5, cy - 3.5);
                cr.line_to(cx - 3.5, cy + 3.5);
                cr.stroke().ok()?;
            }
        }
    }
    text::picture(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_laid_out_with_room_for_a_dragged_icon() {
        let tiles = [Tile::Apps, Tile::Separator("running"), Tile::Apps];
        let (spans, width) = Nekoland::lay_out(&tiles, None);
        assert_eq!(
            spans,
            [
                (SIDE, ART),
                (SIDE + ART + GAP, 1),
                (SIDE + ART + GAP * 2 + 1, ART)
            ]
        );
        assert_eq!(width, SIDE * 2 + ART * 2 + 1 + GAP * 2);
        let (spans, width_with_gap) = Nekoland::lay_out(&tiles, Some(1));
        assert_eq!(spans[1].0, SIDE + (ART + GAP) * 2);
        assert_eq!(width_with_gap, width + ART + GAP);
    }

    #[test]
    fn dragged_files_are_read_from_their_uris() {
        let text =
            "# from a file manager\r\nfile:///home/neko/%E4%B8%8B%E8%BC%89\r\nfile:///tmp/a%20b.txt\r\n";
        assert_eq!(
            uri_paths(text),
            [PathBuf::from("/home/neko/下載"), PathBuf::from("/tmp/a b.txt")]
        );
    }

    #[test]
    fn an_icon_is_trimmed_to_its_art() {
        let surface = ImageSurface::create(Format::ARgb32, 100, 100).unwrap();
        {
            let cr = Context::new(&surface).unwrap();
            cr.set_source_rgb(1.0, 0.0, 0.0);
            cr.rectangle(30.0, 40.0, 40.0, 20.0);
            cr.fill().unwrap();
        }
        let mut out = trimmed(surface, 40).unwrap();
        let stride = out.stride() as usize;
        let data = out.data().unwrap();
        let alpha = |x: usize, y: usize| data[y * stride + x * 4 + 3];
        // 40 by 20 of art fills the width and sits in the middle.
        assert!(alpha(0, 20) > 200 && alpha(39, 20) > 200);
        assert!(alpha(20, 5) < 10 && alpha(20, 34) < 10);
    }
}
