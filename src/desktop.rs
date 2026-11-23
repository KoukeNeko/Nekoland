//! The files on the desktop, as on the Mac: what is in the Desktop folder, in columns from
//! the top right corner down. A click chooses one, Shift or Ctrl adds to the choice, and
//! dragging over the desktop draws a box choosing what it touches; a double click opens.
//! What is chosen can be dragged into a folder, onto the Dock or into a window, renamed in
//! place, and opened, put in the Trash or shown in the file manager from the keyboard or
//! the menus a right click opens.

use std::{
    collections::HashSet,
    io::{Read, Write},
    os::fd::{FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use cairo::ImageSurface;
use gio::prelude::*;
use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, Kind},
        gles::GlesRenderer,
    },
    input::{
        dnd::{DnDGrab, DndAction, Source, SourceMetadata},
        keyboard::Keysym,
        keyboard::ModifiersState,
        pointer::{Focus, GrabStartData},
    },
    utils::{IsAlive, Logical, Point, Rectangle, Serial, Size},
    wayland::selection::{
        data_device::{
            current_data_device_selection_userdata, request_data_device_client_selection,
            set_data_device_selection, SelectionRequestError,
        },
        SelectionTarget,
    },
};

use crate::{
    appgrid::draw_candidates,
    config::{self, Sort},
    dock::{ask_file_manager, drop_into, move_into, open_path, reveal_path, uri_paths},
    ime::Candidates,
    menubar::{self, Command, Entry, MENU_BAR_HEIGHT},
    render::{SceneElement, BAND_EDGE_ALPHA, BAND_FILL_ALPHA, OUTLINE_COLOR},
    stacks::{self, Item},
    state::Nekoland,
    text::{self, Picture},
};

const CELL_WIDTH: i32 = 96;
const CELL_HEIGHT: i32 = 100;
const ICON: i32 = 56;
const ICON_TOP: i32 = 6;
const LABEL_TOP: i32 = ICON_TOP + ICON + 6;
const LABEL_POINTS: f64 = 9.5;
/// From the screen's right edge and from under the menu bar.
const MARGIN: i32 = 12;
/// Two clicks on an item this close together open it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// How far a press moves before it drags.
const DRAG_DISTANCE: f64 = 4.0;
/// Room under the last row for a name being typed that runs longer than two lines.
const RENAME_ROOM: i32 = 48;
/// The name a new folder is given, numbered after the first.
const NEW_FOLDER: &str = "未命名檔案夾";
const URI_LIST: &str = "text/uri-list";
/// How wide the badge counting what is dragged is; the picture leaves half of it above
/// the icon.
const DRAG_BADGE: f64 = 22.0;
/// How long, in seconds, what nothing took takes to go back.
const FLY_BACK_TIME: f64 = 0.3;
/// How long, in seconds, what a folder, the Trash or an application took takes to shrink
/// into it.
const SINK_TIME: f64 = 0.2;
/// Where the items were put, beside the configuration.
const PLACES: &str = "desktop-places.toml";
/// Quick Look's panel: its title strip, corners, the icon of what it has no picture of,
/// and how much of a text file it shows.
const LOOK_TITLE: i32 = 36;
const LOOK_RADIUS: f64 = 12.0;
const LOOK_ICON: i32 = 128;
const LOOK_TEXT_BYTES: u64 = 32 << 10;
const LOOK_TEXT_LINES: usize = 60;
/// How GNOME's file manager copies and cuts files: `copy` or `cut`, then a URI a line.
const COPIED_FILES: &str = "x-special/gnome-copied-files";
const PLAIN_TEXT: &str = "text/plain;charset=utf-8";
// evdev's BTN_LEFT
const BUTTON_LEFT: u32 = 0x110;

#[derive(Clone, Debug, PartialEq)]
pub enum DesktopCommand {
    Open(Vec<PathBuf>),
    /// With the application of this desktop entry.
    OpenWith(String, Vec<PathBuf>),
    Reveal(PathBuf),
    Trash(Vec<PathBuf>),
    Info(Vec<PathBuf>),
    Rename(PathBuf),
    /// Onto the clipboard, for pasting here or in the file manager.
    Copy(Vec<PathBuf>),
    /// What is on the clipboard, into the Desktop folder.
    Paste,
    /// A copy beside each one.
    Duplicate(Vec<PathBuf>),
    SelectAll,
    /// Shows the last item chosen in a panel, or puts the panel away.
    QuickLook,
    NewFolder,
    Sort(Sort),
    /// Items stay where they are dragged, starting from where they are now.
    KeepPlaces,
    Wallpaper,
}

#[derive(Default)]
pub struct Desktop {
    /// The items, as last read, and where each is.
    items: Vec<(Item, Rectangle<i32, Logical>)>,
    selected: Vec<PathBuf>,
    last_click: Option<(PathBuf, Instant)>,
    /// The items drawn, at the place of the first column's top, and what that picture
    /// showed, to know when to draw them again.
    picture: Option<(Picture, Point<i32, Logical>)>,
    drawn: Option<Drawn>,
    /// What the last refresh saw, so that a frame where none of it changed and no picture is
    /// being read does nothing.
    checked: Option<Checked>,
    /// The left button held down on the desktop.
    press: Option<Press>,
    /// The box being dragged out to choose items, and its picture.
    band: Option<(Rectangle<i32, Logical>, Picture)>,
    /// What Nekoland drags, off the desktop or out of a folder in the Dock, for whoever it
    /// is dropped on to ask for.
    dragged: Vec<PathBuf>,
    /// Whether Nekoland drags it now.
    dragging: bool,
    /// The icon following the pointer while dragging, and where it is from the pointer.
    drag_picture: Option<(Picture, Point<i32, Logical>)>,
    /// The folder on the desktop what is dragged would go into, marked as chosen.
    drop_target: Option<PathBuf>,
    pointer: Point<f64, Logical>,
    pub renaming: Option<Rename>,
    pub candidates: Option<Candidates>,
    candidates_picture: Option<(Candidates, Picture)>,
    /// Whether the last item chosen is shown in Quick Look, and its panel as drawn for
    /// that item, theme and output.
    looking: bool,
    quick_look: Option<(Looked, Picture)>,
    /// A press on the Quick Look panel, which drags out what it shows once the pointer goes
    /// far enough: where, and its serial.
    look_press: Option<(Point<f64, Logical>, Serial)>,
    /// A picture being read for Quick Look away from the compositor's thread, which a
    /// camera's largest photos would hold up for half a second.
    reading: Option<(Looked, stacks::Reading)>,
    /// Each item's icon as drawn, and the pictures being read for them on a thread, which
    /// would otherwise be read again at every click, holding the screen.
    icons: Vec<(Item, ImageSurface)>,
    icon_reads: Vec<(Item, stacks::Reading)>,
    /// Where the items were put, read once the desktop keeps them there.
    places: Option<Places>,
    /// Where the pointer was when the drag started, when it was off the desktop.
    drag_from: Option<Point<f64, Logical>>,
    /// Where the pointer was when Nekoland's drag started, for its icon to go back to.
    drag_start: Point<f64, Logical>,
    /// The icon of what nothing took, going back where it was taken from, and since when.
    flying_back: Option<(Picture, Point<f64, Logical>, Point<f64, Logical>, Instant)>,
    /// The icon of what something on the desktop or the Dock took, shrinking into it at
    /// the pointer: the picture, the pointer, the picture's offset from it, and since when.
    sinking: Option<(Picture, Point<f64, Logical>, Point<i32, Logical>, Instant)>,
}

type Looked = (PathBuf, bool, Size<i32, Logical>);

/// The folder's change and order, whether it and free places are shown, the choice, the
/// folder dropped on, the name typed and its candidates, Quick Look, the theme and the size.
type Checked = (
    i64,
    Sort,
    bool,
    bool,
    Vec<PathBuf>,
    Option<PathBuf>,
    Option<Rename>,
    Option<Candidates>,
    bool,
    bool,
    Size<i32, Logical>,
);

/// The cell each item was put in, by name, as column from the right and row from the top.
type Places = std::collections::BTreeMap<String, [i32; 2]>;

type Drawn = (
    Vec<Item>,
    Vec<PathBuf>,
    Option<Rename>,
    Size<i32, Logical>,
    Option<Places>,
);

struct Press {
    from: Point<f64, Logical>,
    item: Option<PathBuf>,
    serial: Serial,
    /// What was chosen before, which a box drawn with Shift or Ctrl adds to.
    kept: Vec<PathBuf>,
}

/// An item's name being typed in place.
#[derive(Clone, Debug, PartialEq)]
pub struct Rename {
    path: PathBuf,
    text: String,
    /// Where typing goes, in bytes.
    caret: usize,
    /// The end of the part chosen from the start, which typing replaces: at first, the
    /// name without its extension, as the Mac does.
    chosen: Option<usize>,
    pub preedit: String,
    pub ime_focused: bool,
}

impl Rename {
    fn insert(&mut self, typed: &str) {
        self.take_chosen();
        self.text.insert_str(self.caret, typed);
        self.caret += typed.len();
    }

    /// Removes what is chosen; returns whether there was something.
    fn take_chosen(&mut self) -> bool {
        let Some(end) = self.chosen.take() else {
            return false;
        };
        self.text.replace_range(..end, "");
        self.caret = 0;
        true
    }

    fn previous(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(at, _)| at)
    }

    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |character| self.caret + character.len_utf8())
    }

    /// A key that edits the name; false for one that does not.
    fn key(&mut self, sym: Keysym) -> bool {
        match sym {
            Keysym::BackSpace => {
                if !self.take_chosen() && self.caret > 0 {
                    let from = self.previous();
                    self.text.replace_range(from..self.caret, "");
                    self.caret = from;
                }
            }
            Keysym::Delete => {
                if !self.take_chosen() {
                    let to = self.next();
                    self.text.replace_range(self.caret..to, "");
                }
            }
            Keysym::Left => {
                self.caret = if self.chosen.take().is_some() {
                    0
                } else {
                    self.previous()
                };
            }
            Keysym::Right => {
                self.caret = match self.chosen.take() {
                    Some(end) => end,
                    None => self.next(),
                };
            }
            Keysym::Home | Keysym::Up => {
                self.chosen = None;
                self.caret = 0;
            }
            Keysym::End | Keysym::Down => {
                self.chosen = None;
                self.caret = self.text.len();
            }
            _ => return false,
        }
        true
    }
}

/// The user's Desktop folder; none when it would be the home folder itself, whose files
/// are not the desktop's.
fn desktop_folder() -> Option<PathBuf> {
    let folder = gio::glib::user_special_dir(gio::glib::UserDirectory::Desktop)?;
    (folder != gio::glib::home_dir()).then_some(folder)
}

/// Where each of `count` items goes on an output of `size`: down from the top right corner,
/// then down the column to the left of it.
fn cells(count: usize, size: Size<i32, Logical>) -> Vec<Rectangle<i32, Logical>> {
    let rows = grid(size)[1];
    (0..count as i32)
        .map(|index| cell_rect([index / rows, index % rows], size))
        .collect()
}

/// How many columns and rows of cells the desktop has room for.
fn grid(size: Size<i32, Logical>) -> [i32; 2] {
    [
        ((size.w - MARGIN * 2) / CELL_WIDTH).max(1),
        ((size.h - MENU_BAR_HEIGHT - MARGIN * 2) / CELL_HEIGHT).max(1),
    ]
}

/// The cell in `column` from the right and `row` from the top.
fn cell_rect([column, row]: [i32; 2], size: Size<i32, Logical>) -> Rectangle<i32, Logical> {
    let x = size.w - MARGIN - (column + 1) * CELL_WIDTH;
    let y = MENU_BAR_HEIGHT + MARGIN + row * CELL_HEIGHT;
    Rectangle::new(Point::from((x, y)), Size::from((CELL_WIDTH, CELL_HEIGHT)))
}

/// The column and row of the cell nearest to a cell's corner at `loc`.
fn cell_of(loc: Point<i32, Logical>, size: Size<i32, Logical>) -> [i32; 2] {
    let column = f64::from(size.w - MARGIN - CELL_WIDTH - loc.x) / f64::from(CELL_WIDTH);
    let row = f64::from(loc.y - MENU_BAR_HEIGHT - MARGIN) / f64::from(CELL_HEIGHT);
    [column.round() as i32, row.round() as i32]
}

/// Where items go when they stay where they were put: each in the cell kept for its name,
/// the rest in the free cells in order, in columns from the top right.
fn placed_cells(items: &[Item], places: &Places, size: Size<i32, Logical>) -> Vec<Rectangle<i32, Logical>> {
    let [columns, rows] = grid(size);
    let mut taken = std::collections::HashSet::new();
    let kept: Vec<Option<[i32; 2]>> = items
        .iter()
        .map(|item| {
            places
                .get(&item.name)
                .copied()
                .filter(|&[column, row]| column < columns && row < rows && taken.insert([column, row]))
        })
        .collect();
    let mut free = (0..)
        .map(|index| [index / rows, index % rows])
        .filter(|cell| !taken.contains(cell));
    kept.into_iter()
        .map(|cell| {
            cell_rect(
                cell.unwrap_or_else(|| free.next().expect("cells never run out")),
                size,
            )
        })
        .collect()
}

fn read_places() -> Places {
    config::config_file(PLACES)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| {
            toml::from_str(&text)
                .inspect_err(|err| tracing::warn!("{PLACES}: {err}"))
                .ok()
        })
        .unwrap_or_default()
}

fn write_places(places: &Places) {
    let Some(path) = config::config_file(PLACES) else {
        return;
    };
    let written = toml::to_string(places)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
            std::fs::write(&path, text)
        });
    if let Err(err) = written {
        tracing::warn!("{}: {err}", path.display());
    }
}

/// The part of a cell that is drawn on: the icon and the name under it.
fn body(cell: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        cell.loc + Point::from(((CELL_WIDTH - ICON) / 2 - 4, ICON_TOP)),
        Size::from((ICON + 8, CELL_HEIGHT - ICON_TOP - 8)),
    )
}

/// The box between two points.
fn band(from: Point<f64, Logical>, to: Point<f64, Logical>) -> Rectangle<i32, Logical> {
    let (from, to): (Point<i32, Logical>, Point<i32, Logical>) = (from.to_i32_round(), to.to_i32_round());
    let loc = Point::from((from.x.min(to.x), from.y.min(to.y)));
    Rectangle::new(loc, Size::from(((from.x - to.x).abs(), (from.y - to.y).abs())))
}

/// How far into a name its extension starts, which renaming leaves out of the choice;
/// the whole name for a folder or a name with none.
fn stem_end(name: &str, is_dir: bool) -> usize {
    match name.rfind('.') {
        Some(dot) if !is_dir && dot > 0 => dot,
        _ => name.len(),
    }
}

impl Nekoland {
    /// Reads the Desktop folder again when it has changed and draws its items when they,
    /// the choice among them, a name being typed or the output's size have.
    pub fn refresh_desktop(&mut self) {
        let folder = desktop_folder().filter(|_| self.config.desktop_icons);
        let sort = self.config.desktop_sort;
        let changed = match &folder {
            Some(folder) => self.folder_items(folder, sort).0,
            None => i64::MIN,
        };
        let size = self.output_size();
        let dark = self.menu_bar_dark();
        let free = self.config.desktop_free;
        let desktop = &self.desktop;
        let checked = (
            changed,
            sort,
            folder.is_some(),
            free,
            desktop.selected.clone(),
            desktop.drop_target.clone(),
            desktop.renaming.clone(),
            desktop.candidates.clone(),
            desktop.looking,
            dark,
            size,
        );
        if desktop.icon_reads.is_empty()
            && desktop.reading.is_none()
            && desktop.checked.as_ref() == Some(&checked)
        {
            return;
        }
        let items = match &folder {
            Some(folder) => self.folder_items(folder, sort).1.to_vec(),
            None => Vec::new(),
        };
        let desktop = &mut self.desktop;
        desktop.checked = Some(checked);
        if free && desktop.places.is_none() {
            desktop.places = Some(read_places());
        }
        // What went away is no longer chosen.
        let paths: HashSet<&PathBuf> = items.iter().map(|item| &item.path).collect();
        desktop.selected.retain(|path| paths.contains(path));
        refresh_icons(desktop, &items);
        let looked = desktop.selected.last().filter(|_| desktop.looking);
        desktop.looking = looked.is_some();
        match looked.and_then(|path| items.iter().find(|item| item.path == *path)) {
            None => desktop.quick_look = None,
            Some(item) => refresh_quick_look(desktop, item, (item.path.clone(), dark, size)),
        }
        if desktop.candidates.as_ref() != desktop.candidates_picture.as_ref().map(|(drawn, _)| drawn) {
            desktop.candidates_picture = desktop
                .candidates
                .as_ref()
                .and_then(|candidates| Some((candidates.clone(), draw_candidates(candidates)?)));
        }
        let marked = desktop
            .selected
            .iter()
            .chain(&desktop.drop_target)
            .cloned()
            .collect();
        let places = desktop.places.clone().filter(|_| free);
        let wanted = (items, marked, desktop.renaming.clone(), size, places);
        if desktop.drawn.as_ref() == Some(&wanted) {
            return;
        }
        let (items, selected, renaming, _, places) = &wanted;
        let places = match places {
            Some(places) => placed_cells(items, places, size),
            None => cells(items.len(), size),
        };
        desktop.items = items.iter().cloned().zip(places.iter().copied()).collect();
        desktop.picture = draw_items(&desktop.items, &desktop.icons, selected, renaming.as_ref());
        desktop.drawn = Some(wanted);
    }

    /// The desktop's items and the box choosing them, over the wallpaper and under every
    /// window, front to back.
    pub fn desktop_elements(
        &self,
        renderer: &mut GlesRenderer,
        output_loc: Point<i32, Logical>,
        scale: f64,
    ) -> Vec<SceneElement> {
        let candidates = self.desktop.candidates_picture.as_ref().and_then(|(_, picture)| {
            let path = &self.desktop.renaming.as_ref()?.path;
            let (_, cell) = self.desktop.items.iter().find(|(item, _)| item.path == *path)?;
            let x =
                (cell.loc.x + CELL_WIDTH / 2 - picture.size.w / 2).min(self.output_size().w - picture.size.w);
            Some((picture, Point::from((x, cell.loc.y + CELL_HEIGHT + RENAME_ROOM))))
        });
        candidates
            .into_iter()
            .chain(
                self.desktop
                    .band
                    .as_ref()
                    .map(|(rect, picture)| (picture, rect.loc)),
            )
            .chain(self.desktop.picture.as_ref().map(|(picture, at)| (picture, *at)))
            .filter_map(|(picture, at)| picture_element(renderer, picture, at - output_loc, scale))
            .collect()
    }

    /// The icon of what is being dragged off the desktop, and the Quick Look panel, over
    /// everything.
    pub fn desktop_front_elements(
        &self,
        renderer: &mut GlesRenderer,
        output_loc: Point<i32, Logical>,
        scale: f64,
    ) -> Vec<SceneElement> {
        let dragged = self
            .desktop
            .drag_picture
            .as_ref()
            .map(|(picture, offset)| (picture, self.desktop.pointer.to_i32_round() + *offset));
        let flying = self.desktop.flying_back.as_ref().and_then(|(picture, from, to, since)| {
            let t = since.elapsed().as_secs_f64() / FLY_BACK_TIME;
            let eased = crate::control::ease_out(t.min(1.0));
            (t < 1.0).then(|| (picture, (*from + (*to - *from).upscale(eased)).to_i32_round()))
        });
        let sinking = self.desktop.sinking.as_ref().and_then(|(picture, pointer, offset, since)| {
            let t = since.elapsed().as_secs_f64() / SINK_TIME;
            if t >= 1.0 {
                return None;
            }
            let left = 1.0 - crate::control::ease_out(t);
            let at = *pointer + offset.to_f64().upscale(left) - output_loc.to_f64();
            let size = picture.size.to_f64().upscale(left).to_i32_round();
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                at.to_physical(scale),
                &picture.buffer,
                Some(left as f32),
                None,
                Some(size),
                Kind::Unspecified,
            )
            .ok()
            .map(SceneElement::Picture)
        });
        let looked = self
            .desktop
            .quick_look
            .as_ref()
            .zip(self.quick_look_area())
            .map(|((_, picture), area)| (picture, area.loc));
        dragged
            .into_iter()
            .chain(flying)
            .chain(looked)
            .filter_map(|(picture, at)| picture_element(renderer, picture, at - output_loc, scale))
            .chain(sinking)
            .collect()
    }

    /// Where the Quick Look panel is: in the middle of the output.
    fn quick_look_area(&self) -> Option<Rectangle<i32, Logical>> {
        let (_, picture) = self.desktop.quick_look.as_ref()?;
        let output = self.output_area()?;
        let loc = output.loc
            + Point::from((
                (output.size.w - picture.size.w) / 2,
                (output.size.h - picture.size.h) / 2,
            ));
        Some(Rectangle::new(loc, picture.size))
    }

    /// A press anywhere ends typing a name, and, off the desktop's items and the panel,
    /// Quick Look.
    pub fn desktop_pointer_pressed(&mut self, pos: Point<f64, Logical>) {
        self.end_rename();
        let on_panel = self
            .quick_look_area()
            .is_some_and(|area| area.to_f64().contains(pos));
        let on_item = self.space.element_under(pos).is_none() && self.desktop_item_at(pos).is_some();
        if !on_panel && !on_item {
            self.desktop.looking = false;
        }
    }

    /// The item under `pos` while nothing goes on there: no window over it, no press, box
    /// or drag, no name being typed and no Quick Look.
    pub(crate) fn desktop_resting_item(&self, pos: Point<f64, Logical>) -> Option<Item> {
        let desktop = &self.desktop;
        let busy = desktop.press.is_some()
            || desktop.band.is_some()
            || desktop.drag_picture.is_some()
            || desktop.renaming.is_some()
            || desktop.looking;
        if busy || self.space.element_under(pos).is_some() {
            return None;
        }
        self.desktop_item_at(pos)
    }

    fn desktop_item_at(&self, pos: Point<f64, Logical>) -> Option<Item> {
        let pos = pos.to_i32_round();
        self.desktop
            .items
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(item, _)| item.clone())
    }

    pub(crate) fn modifiers(&self) -> ModifiersState {
        self.seat
            .get_keyboard()
            .map(|keyboard| keyboard.modifier_state())
            .unwrap_or_default()
    }

    /// A press on the desktop, where no window is: an item is chosen, opened on the second
    /// click, or given its menu; the desktop itself gets its own menu, or starts a box.
    /// Returns whether the desktop takes the keyboard: whenever it shows items or a menu.
    pub fn desktop_press(&mut self, pos: Point<f64, Logical>, right: bool, serial: Serial) -> bool {
        if !right && self.quick_look_area().is_some_and(|area| area.to_f64().contains(pos)) {
            self.desktop.look_press = Some((pos, serial));
            return true;
        }
        let item = self.desktop_item_at(pos);
        let modifiers = self.modifiers();
        let adding = (modifiers.shift || modifiers.ctrl) && !right;
        let selected = &mut self.desktop.selected;
        match &item {
            Some(item) if adding => match selected.iter().position(|path| *path == item.path) {
                Some(at) => {
                    selected.remove(at);
                }
                None => selected.push(item.path.clone()),
            },
            // Pressing on what is already chosen keeps the rest chosen with it, to drag.
            Some(item) if !selected.contains(&item.path) => *selected = vec![item.path.clone()],
            Some(_) => {}
            None if !adding => selected.clear(),
            None => {}
        }
        if right {
            let entries = match &item {
                Some(_) => self.item_menu(),
                None => self.desktop_menu(),
            };
            self.open_menu_at(entries, pos.to_i32_round());
            return true;
        }
        self.desktop.press = Some(Press {
            from: pos,
            item: item.as_ref().map(|item| item.path.clone()),
            serial,
            kept: if adding {
                self.desktop.selected.clone()
            } else {
                Vec::new()
            },
        });
        let Some(item) = item else {
            self.desktop.last_click = None;
            // The desktop takes the keyboard as the Mac's Finder does, for pasting there.
            return !self.desktop.items.is_empty();
        };
        let twice = self
            .desktop
            .last_click
            .take()
            .is_some_and(|(path, at)| path == item.path && at.elapsed() < DOUBLE_CLICK);
        if twice && !adding {
            self.run_desktop_command(DesktopCommand::Open(self.desktop.selected.clone()));
        } else {
            self.desktop.last_click = Some((item.path, Instant::now()));
        }
        true
    }

    /// The pointer moved: a press far enough on an item drags what is chosen, and on the
    /// desktop draws a box choosing what it touches.
    pub fn desktop_motion(&mut self, pos: Point<f64, Logical>) {
        self.desktop.pointer = pos;
        if self.dock_files_dragged_now() {
            self.desktop.drop_target = self.desktop_folder_at(pos).map(|folder| folder.path);
        }
        if let Some((from, serial)) = self.desktop.look_press {
            let moved = pos - from;
            if moved.x.hypot(moved.y) >= DRAG_DISTANCE {
                self.desktop.look_press = None;
                self.drag_out_of_quick_look(from, serial);
            }
            return;
        }
        let Some(press) = &self.desktop.press else {
            return;
        };
        let moved = pos - press.from;
        if moved.x.hypot(moved.y) < DRAG_DISTANCE && self.desktop.band.is_none() {
            return;
        }
        if press.item.is_some() {
            let press = self.desktop.press.take().expect("a press");
            self.start_desktop_drag(press);
            return;
        }
        let rect = band(press.from, pos);
        let mut selected = press.kept.clone();
        for (item, cell) in &self.desktop.items {
            if body(*cell).overlaps(rect) && !selected.contains(&item.path) {
                selected.push(item.path.clone());
            }
        }
        self.desktop.selected = selected;
        self.desktop.band = draw_band(rect.size).map(|picture| (rect, picture));
    }

    pub fn desktop_release(&mut self) {
        self.desktop.band = None;
        self.desktop.look_press = None;
        let Some(press) = self.desktop.press.take() else {
            return;
        };
        // A click on one of several chosen, not dragged, chooses it alone.
        let modifiers = self.modifiers();
        if let Some(path) = press.item.filter(|_| !(modifiers.shift || modifiers.ctrl)) {
            self.desktop.selected = vec![path];
        }
    }

    /// Drags what is chosen as files, for a window, the Dock or a folder on the desktop to
    /// take where it is dropped.
    fn start_desktop_drag(&mut self, press: Press) {
        let Some(path) = press.item else {
            return;
        };
        let Some((item, cell)) = self.desktop.items.iter().find(|(item, _)| item.path == path) else {
            return;
        };
        let paths = self.desktop.selected.clone();
        let icon_at = cell.loc + Point::from(((CELL_WIDTH - ICON) / 2, ICON_TOP));
        let icon = self.desktop.icons.iter().find(|(known, _)| known == item);
        let picture = icon
            .and_then(|(_, icon)| draw_dragged(icon, paths.len()))
            .map(|picture| (picture, icon_at - press.from.to_i32_round()));
        self.start_file_drag(paths, picture, press.from, press.serial);
        self.desktop.drag_from = Some(press.from);
    }

    /// What Quick Look shows dragged out of its panel, as a file, as on the Mac.
    fn drag_out_of_quick_look(&mut self, from: Point<f64, Logical>, serial: Serial) {
        let Some(((path, ..), _)) = &self.desktop.quick_look else {
            return;
        };
        let path = path.clone();
        let picture = (self.desktop.icons.iter())
            .find(|(item, _)| item.path == path)
            .and_then(|(_, icon)| dragged_alone(icon, Point::default()));
        self.start_file_drag(vec![path], picture, from, serial);
    }

    /// Drags `paths` as files from `from`, with `picture` under the pointer, for a window,
    /// the Dock or the desktop to take where they are let go of.
    pub(crate) fn start_file_drag(
        &mut self,
        paths: Vec<PathBuf>,
        picture: Option<(Picture, Point<i32, Logical>)>,
        from: Point<f64, Logical>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let start = GrabStartData {
            focus: None,
            button: BUTTON_LEFT,
            location: from,
        };
        self.desktop.dragged = paths.clone();
        self.desktop.drag_from = None;
        self.desktop.drag_start = from;
        self.desktop.flying_back = None;
        self.desktop.sinking = None;
        self.desktop.drag_picture = picture;
        self.desktop.dragging = true;
        self.dock_files_dragged(paths.clone());
        let grab = DnDGrab::new_pointer(&self.display_handle, start, DesktopDrag(paths), self.seat.clone());
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    pub fn desktop_dragging(&self) -> bool {
        self.desktop.dragging
    }

    /// A drag of files ended, Nekoland's or a program's, taken by a window or not.
    pub fn desktop_drag_ended(&mut self) {
        self.desktop.dragging = false;
        self.desktop.dragged.clear();
        self.desktop.drag_from = None;
        self.desktop.drag_picture = None;
        self.desktop.drop_target = None;
        self.dock_drag_ended();
    }

    /// No window took what Nekoland dragged: on the Dock, it goes where the Dock puts
    /// files; on the desktop, where `drop_files_on_desktop` puts them.
    pub fn desktop_dropped_here(&mut self) {
        let pos = self.desktop.pointer;
        let paths = self.desktop.dragged.clone();
        let on_folder = self.space.element_under(pos).is_none() && self.desktop_folder_at(pos).is_some();
        if self.drop_files_on_dock(pos, &paths) {
            self.sink();
        } else if self.drop_files_on_desktop(pos, &paths) {
            // Put about the desktop, they are already where they were let go of.
            if on_folder {
                self.sink();
            }
        } else {
            self.fly_back();
        }
    }

    /// What a folder, the Trash or an application took shrinks into it, as on the Mac.
    fn sink(&mut self) {
        let Some((picture, offset)) = self.desktop.drag_picture.take() else {
            return;
        };
        if self.config.animations {
            self.desktop.sinking = Some((picture, self.desktop.pointer, offset, Instant::now()));
        }
    }

    /// What nothing took goes back where it was taken from, as on the Mac.
    fn fly_back(&mut self) {
        let Some((picture, offset)) = self.desktop.drag_picture.take() else {
            return;
        };
        if !self.config.animations {
            return;
        }
        let offset = offset.to_f64();
        let (from, to) = (self.desktop.pointer + offset, self.desktop.drag_start + offset);
        self.desktop.flying_back = Some((picture, from, to, Instant::now()));
    }

    /// Files let go of on the desktop, as on the Mac's: on a folder there, into it; dragged
    /// about the desktop, where they were let go of if it keeps them where they are put;
    /// from anywhere else, into the Desktop folder. Returns whether they went anywhere.
    pub(crate) fn drop_files_on_desktop(&mut self, pos: Point<f64, Logical>, paths: &[PathBuf]) -> bool {
        if self.space.element_under(pos).is_some()
            || self.dock_covers(pos)
            || self.overview.is_some()
            || pos.y < f64::from(MENU_BAR_HEIGHT)
        {
            return false;
        }
        let copy = self.copying();
        if let Some(folder) = self.desktop_folder_at(pos) {
            drop_into(paths, &folder.path, copy);
        } else if let Some(from) = self.desktop.drag_from.filter(|_| self.config.desktop_free) {
            self.place_dragged((pos - from).to_i32_round());
        } else if let Some(folder) = desktop_folder() {
            // Already there, they only go back to their places.
            if !copy && paths.iter().all(|path| path.parent() == Some(folder.as_path())) {
                return false;
            }
            drop_into(paths, &folder, copy);
        } else {
            return false;
        }
        true
    }

    /// What was dragged stays as far from where it was as it was dragged, in the cells
    /// nearest, unless one of them is taken or off the desktop.
    fn place_dragged(&mut self, by: Point<i32, Logical>) {
        let size = self.output_size();
        let [columns, rows] = grid(size);
        let desktop = &mut self.desktop;
        let dragged = |item: &Item| desktop.dragged.contains(&item.path);
        let moved: Vec<(String, [i32; 2])> = desktop
            .items
            .iter()
            .filter(|(item, _)| dragged(item))
            .map(|(item, rect)| (item.name.clone(), cell_of(rect.loc + by, size)))
            .collect();
        let others: Vec<[i32; 2]> = desktop
            .items
            .iter()
            .filter(|(item, _)| !dragged(item))
            .map(|(_, rect)| cell_of(rect.loc, size))
            .collect();
        let refused = moved.iter().any(|(_, cell @ [column, row])| {
            !(0..columns).contains(column) || !(0..rows).contains(row) || others.contains(cell)
        });
        if refused {
            return;
        }
        // Every item is kept where it is now, so that only what was dragged moves.
        let mut places: Places = desktop
            .items
            .iter()
            .map(|(item, rect)| (item.name.clone(), cell_of(rect.loc, size)))
            .collect();
        places.extend(moved);
        write_places(&places);
        desktop.places = Some(places);
        desktop.checked = None;
    }

    /// A folder on the desktop at `pos`, not under a window or the Dock, that what is
    /// dragged could go into.
    fn desktop_folder_at(&self, pos: Point<f64, Logical>) -> Option<Item> {
        if self.space.element_under(pos).is_some() || self.dock_covers(pos) {
            return None;
        }
        self.desktop_item_at(pos)
            .filter(|item| item.is_dir && !self.desktop.dragged.contains(&item.path))
    }

    /// A key while the desktop has the keyboard: no window has it, and something on the
    /// desktop is chosen or being renamed. Returns whether it was taken.
    pub fn desktop_key(&mut self, modifiers: &ModifiersState, sym: Keysym, typed: Option<char>) -> bool {
        let plain = !(modifiers.ctrl || modifiers.alt || modifiers.logo);
        if self.desktop.renaming.is_some() {
            return plain && self.rename_key(sym, typed);
        }
        let free = self
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.current_focus().is_none());
        if !free || self.desktop.items.is_empty() {
            return false;
        }
        let chosen = self.desktop.selected.clone();
        // Ctrl too, as Linux programs copy and paste with it and no window takes it here.
        let command_key = (modifiers.logo || modifiers.ctrl) && !modifiers.alt && !modifiers.shift;
        let command = match sym {
            Keysym::n if modifiers.logo && modifiers.shift => DesktopCommand::NewFolder,
            Keysym::Escape if plain && self.desktop.looking => DesktopCommand::QuickLook,
            Keysym::v if command_key => DesktopCommand::Paste,
            Keysym::Up | Keysym::Down | Keysym::Left | Keysym::Right if plain => {
                self.move_choice(sym);
                return true;
            }
            _ if chosen.is_empty() => return false,
            // Only once something is chosen: Super A with nothing chosen shows the apps.
            Keysym::a if command_key => DesktopCommand::SelectAll,
            Keysym::space if plain => DesktopCommand::QuickLook,
            Keysym::y if command_key => DesktopCommand::QuickLook,
            Keysym::c if command_key => DesktopCommand::Copy(chosen),
            Keysym::d if command_key => DesktopCommand::Duplicate(chosen),
            // Super Shift O is 截圖's text recognition.
            Keysym::o if modifiers.logo && !modifiers.shift => DesktopCommand::Open(chosen),
            Keysym::Down if modifiers.logo => DesktopCommand::Open(chosen),
            Keysym::i if modifiers.logo => DesktopCommand::Info(chosen),
            Keysym::BackSpace if modifiers.logo => DesktopCommand::Trash(chosen),
            Keysym::Delete if plain => DesktopCommand::Trash(chosen),
            Keysym::Return | Keysym::KP_Enter | Keysym::F2 if plain && chosen.len() == 1 => {
                DesktopCommand::Rename(chosen[0].clone())
            }
            _ => match typed.filter(|character| plain && !character.is_control()) {
                Some(character) => {
                    self.choose_by_name(character);
                    return true;
                }
                None => return false,
            },
        };
        self.run_desktop_command(command);
        true
    }

    /// The arrows move the choice to the next item that way, or choose the first.
    fn move_choice(&mut self, sym: Keysym) {
        let items = &self.desktop.items;
        let current = self
            .desktop
            .selected
            .last()
            .and_then(|path| items.iter().find(|(item, _)| item.path == *path));
        let next = match current {
            None => items.first(),
            Some((_, from)) => {
                let (dx, dy) = match sym {
                    Keysym::Up => (0, -CELL_HEIGHT),
                    Keysym::Down => (0, CELL_HEIGHT),
                    Keysym::Left => (-CELL_WIDTH, 0),
                    _ => (CELL_WIDTH, 0),
                };
                let to = from.loc + Point::from((dx, dy));
                items.iter().find(|(_, rect)| rect.loc == to)
            }
        };
        if let Some((item, _)) = next {
            self.desktop.selected = vec![item.path.clone()];
        }
    }

    /// Typing a letter chooses the first item whose name starts with it.
    fn choose_by_name(&mut self, character: char) {
        let typed = character.to_lowercase().to_string();
        if let Some((item, _)) = self
            .desktop
            .items
            .iter()
            .find(|(item, _)| item.name.to_lowercase().starts_with(&typed))
        {
            self.desktop.selected = vec![item.path.clone()];
        }
    }

    /// A key while a name is typed: Return keeps it, Escape leaves the name as it was.
    pub fn rename_key(&mut self, sym: Keysym, typed: Option<char>) -> bool {
        let Some(rename) = self.desktop.renaming.as_mut() else {
            return false;
        };
        match sym {
            Keysym::Return | Keysym::KP_Enter => self.end_rename(),
            Keysym::Escape => {
                self.desktop.renaming = None;
                self.desktop.candidates = None;
            }
            _ if rename.key(sym) => {}
            _ => {
                if let Some(character) = typed.filter(|character| !character.is_control()) {
                    rename.insert(&character.to_string());
                }
            }
        }
        true
    }

    /// Text the input method put in the name being typed.
    pub fn rename_commit(&mut self, text: &str) {
        if let Some(rename) = self.desktop.renaming.as_mut() {
            rename.insert(text);
        }
    }

    /// Gives the item the name typed, unless it is empty, unchanged or taken.
    pub fn end_rename(&mut self) {
        let Some(rename) = self.desktop.renaming.take() else {
            return;
        };
        self.desktop.candidates = None;
        let name = rename.text.trim();
        let unchanged = rename.path.file_name().is_some_and(|old| old == name);
        if name.is_empty() || unchanged || name.contains('/') {
            return;
        }
        // shortcut: a taken or refused name is only logged, as the Mac's alert is not
        // drawn; add one if renaming over an existing name turns out to matter.
        match gio::File::for_path(&rename.path).set_display_name(name, gio::Cancellable::NONE) {
            Ok(renamed) => {
                self.desktop.selected = renamed.path().into_iter().collect();
                if let (Some(places), Some(old)) = (self.desktop.places.as_mut(), rename.path.file_name()) {
                    if let Some(cell) = places.remove(&*old.to_string_lossy()) {
                        places.insert(name.to_string(), cell);
                        write_places(places);
                        self.desktop.checked = None;
                    }
                }
            }
            Err(err) => tracing::warn!("{}: {err}", rename.path.display()),
        }
    }

    fn desktop_menu(&self) -> Vec<Entry> {
        let sort = self.config.desktop_sort;
        let free = self.config.desktop_free;
        let sorts = [
            (Sort::Name, "名稱"),
            (Sort::Added, "加入日期"),
            (Sort::Modified, "修改日期"),
            (Sort::Created, "建立日期"),
            (Sort::Kind, "種類"),
        ]
        .into_iter()
        .map(|(by, label)| {
            Entry::item(label, Command::Desktop(DesktopCommand::Sort(by))).checked(!free && sort == by)
        });
        let sorts =
            std::iter::once(Entry::item("無", Command::Desktop(DesktopCommand::KeepPlaces)).checked(free))
                .chain(std::iter::once(Entry::separator()))
                .chain(sorts)
                .collect();
        vec![
            Entry::item("新增檔案夾", Command::Desktop(DesktopCommand::NewFolder)).shortcut("Super Shift N"),
            Entry::separator(),
            Entry::item("貼上", Command::Desktop(DesktopCommand::Paste)).shortcut("Super V"),
            Entry::separator(),
            Entry::submenu("排序方式", sorts),
            Entry::separator(),
            Entry::item("更改桌布⋯", Command::Desktop(DesktopCommand::Wallpaper)),
        ]
    }

    /// The menu for what is chosen: one item, or several together.
    fn item_menu(&self) -> Vec<Entry> {
        let chosen = self.desktop.selected.clone();
        let command = |command: DesktopCommand| Command::Desktop(command);
        let mut entries =
            vec![Entry::item("打開", command(DesktopCommand::Open(chosen.clone()))).shortcut("Super O")];
        if let [path] = chosen.as_slice() {
            let apps = open_with(path, &chosen);
            if !apps.is_empty() {
                entries.push(Entry::submenu("打開檔案的應用程式", apps));
            }
        }
        entries.extend([
            Entry::separator(),
            Entry::item("丟到垃圾桶", command(DesktopCommand::Trash(chosen.clone()))).shortcut("Delete"),
            Entry::separator(),
            Entry::item("取得資訊", command(DesktopCommand::Info(chosen.clone()))).shortcut("Super I"),
        ]);
        if let [path] = chosen.as_slice() {
            entries.push(
                Entry::item("重新命名", command(DesktopCommand::Rename(path.clone()))).shortcut("Return"),
            );
            entries.push(Entry::item(
                "在檔案中顯示",
                command(DesktopCommand::Reveal(path.clone())),
            ));
        }
        entries.extend([
            Entry::item("製作複本", command(DesktopCommand::Duplicate(chosen.clone()))).shortcut("Super D"),
            Entry::item("快速查看", command(DesktopCommand::QuickLook)).shortcut("Space"),
            Entry::separator(),
            Entry::item("拷貝", command(DesktopCommand::Copy(chosen))).shortcut("Super C"),
        ]);
        entries
    }

    /// Files copied here, or copied or cut in a file manager, go into the Desktop folder.
    fn paste_files(&mut self) {
        let Some(folder) = desktop_folder() else {
            return;
        };
        let copied = current_data_device_selection_userdata(&self.seat).and_then(|copied| copied.clone());
        if let Some(paths) = copied {
            copy_into(&paths, &folder);
            return;
        }
        for mime_type in [COPIED_FILES, URI_LIST] {
            let mut fds = [0; 2];
            if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
                tracing::warn!("pipe: {}", std::io::Error::last_os_error());
                return;
            }
            // SAFETY: pipe2 just made these two descriptors, which nothing else owns.
            let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
            match request_data_device_client_selection(&self.seat, mime_type.into(), write) {
                Err(SelectionRequestError::InvalidMimetype) => continue,
                Err(_) => return,
                Ok(()) => {}
            }
            let folder = folder.clone();
            // The program writes it when it reads its socket, after this frame's events go out.
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Err(err) = std::fs::File::from(read).read_to_string(&mut text) {
                    tracing::warn!("reading pasted files: {err}");
                }
                let paths = uri_paths(&text);
                if text.lines().next() == Some("cut") {
                    move_into(&paths, &folder);
                } else if !paths.is_empty() {
                    copy_into(&paths, &folder);
                }
            });
            return;
        }
    }

    /// What the menu bar's Edit menu copies, pastes and chooses while no window is in
    /// front: the desktop's files, if it shows any, as the Finder's does.
    pub(crate) fn desktop_edit_targets(&self) -> Option<&[PathBuf]> {
        (!self.desktop.items.is_empty()).then_some(self.desktop.selected.as_slice())
    }

    pub fn run_desktop_command(&mut self, command: DesktopCommand) {
        match command {
            DesktopCommand::Open(paths) => paths.iter().for_each(|path| open_path(path)),
            DesktopCommand::OpenWith(app_id, paths) => {
                let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
                match gio::DesktopAppInfo::new(&app_id) {
                    Some(app) => {
                        if let Err(err) = app.launch(&files, None::<&gio::AppLaunchContext>) {
                            tracing::warn!("{app_id}: {err}");
                        }
                    }
                    None => tracing::warn!("{app_id}: no such desktop entry"),
                }
            }
            DesktopCommand::Reveal(path) => reveal_path(&path),
            DesktopCommand::Trash(paths) => {
                for path in &paths {
                    if let Err(err) = gio::File::for_path(path).trash(gio::Cancellable::NONE) {
                        tracing::warn!("{}: {err}", path.display());
                    }
                }
                self.desktop.selected.clear();
            }
            DesktopCommand::Info(paths) => ask_file_manager("ShowItemProperties", &paths),
            DesktopCommand::Rename(path) => {
                let Some((item, _)) = self.desktop.items.iter().find(|(item, _)| item.path == path) else {
                    return;
                };
                let text = item.name.clone();
                self.desktop.renaming = Some(Rename {
                    path: path.clone(),
                    caret: text.len(),
                    chosen: Some(stem_end(&text, item.is_dir)),
                    text,
                    preedit: String::new(),
                    ime_focused: false,
                });
                self.desktop.selected = vec![path];
                self.set_keyboard_focus(None);
            }
            DesktopCommand::Copy(paths) => {
                let mime_types: Vec<String> = [COPIED_FILES, URI_LIST, PLAIN_TEXT].map(String::from).into();
                if let Some(xwm) = self.xwm.as_mut() {
                    if let Err(err) = xwm.new_selection(SelectionTarget::Clipboard, Some(mime_types.clone()))
                    {
                        tracing::warn!("offering copied files to X11 programs: {err}");
                    }
                }
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, Some(paths));
            }
            DesktopCommand::Paste => self.paste_files(),
            DesktopCommand::Duplicate(paths) => {
                for path in &paths {
                    if let Some(folder) = path.parent() {
                        copy_into(std::slice::from_ref(path), folder);
                    }
                }
            }
            DesktopCommand::QuickLook => self.desktop.looking = !self.desktop.looking,
            DesktopCommand::SelectAll => {
                self.desktop.selected = self
                    .desktop
                    .items
                    .iter()
                    .map(|(item, _)| item.path.clone())
                    .collect();
            }
            DesktopCommand::NewFolder => {
                let Some(folder) = desktop_folder() else {
                    return;
                };
                let path = new_folder_path(&folder);
                match std::fs::create_dir_all(&folder).and_then(|()| std::fs::create_dir(&path)) {
                    Ok(()) => self.desktop.selected = vec![path],
                    Err(err) => tracing::warn!("{}: {err}", path.display()),
                }
            }
            DesktopCommand::Sort(sort) => {
                self.config.desktop_sort = sort;
                self.config.desktop_free = false;
                let saved = config::set(&format!("desktop_sort = {:?}", sort.name()))
                    .and_then(|()| config::unset("desktop_free"));
                if let Err(err) = saved {
                    tracing::warn!("{err}");
                }
            }
            DesktopCommand::KeepPlaces => {
                let size = self.output_size();
                let places: Places = self
                    .desktop
                    .items
                    .iter()
                    .map(|(item, rect)| (item.name.clone(), cell_of(rect.loc, size)))
                    .collect();
                write_places(&places);
                self.desktop.places = Some(places);
                self.desktop.checked = None;
                self.config.desktop_free = true;
                if let Err(err) = config::set("desktop_free = true") {
                    tracing::warn!("{err}");
                }
            }
            DesktopCommand::Wallpaper => config::spawn("NEKOLAND_SETTINGS_PAGE=桌布 nekoland-settings"),
        }
    }
}

/// The applications that open a file of this kind, the default first.
fn open_with(path: &Path, paths: &[PathBuf]) -> Vec<Entry> {
    let Some(kind) = gio::File::for_path(path)
        .query_info(
            "standard::content-type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|info| info.content_type())
    else {
        return Vec::new();
    };
    gio::AppInfo::recommended_for_type(&kind)
        .into_iter()
        .filter_map(|app| {
            let id = app.id()?.to_string();
            let command = Command::Desktop(DesktopCommand::OpenWith(id, paths.to_vec()));
            Some(Entry::item(app.display_name().to_string(), command))
        })
        .collect()
}

fn picture_element(
    renderer: &mut GlesRenderer,
    picture: &Picture,
    at: Point<i32, Logical>,
    scale: f64,
) -> Option<SceneElement> {
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at.to_f64().to_physical(scale),
        &picture.buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    )
    .ok()
    .map(SceneElement::Picture)
}

/// Draws Quick Look's panel for the item looked at once what it needs is read: a picture
/// on a thread, the panel showing when it is read, anything else at once.
fn refresh_quick_look(desktop: &mut Desktop, item: &Item, wanted: Looked) {
    let reading = desktop.reading.as_ref().filter(|(looked, _)| *looked == wanted);
    let drawn = desktop
        .quick_look
        .as_ref()
        .is_some_and(|(looked, _)| *looked == wanted);
    if drawn && reading.is_none() {
        return;
    }
    let (_, dark, size) = wanted.clone();
    if let Some((_, read)) = reading {
        let Some(pixels) = read.lock().ok().and_then(|mut read| read.take()) else {
            return;
        };
        desktop.reading = None;
        desktop.quick_look = draw_quick_look(item, pixels, size, dark).map(|picture| (wanted, picture));
        return;
    }
    desktop.reading = None;
    if !item.kind.starts_with("image/") {
        desktop.quick_look = draw_quick_look(item, None, size, dark).map(|picture| (wanted, picture));
        return;
    }
    // Its card, as for any file, stands in until the picture is read.
    desktop.quick_look = draw_quick_look(item, None, size, dark).map(|picture| (wanted.clone(), picture));
    let room = look_room(size) * text::picture_scale();
    desktop.reading = Some((wanted, stacks::read_on_a_thread(item.path.clone(), room)));
}

/// Keeps an icon for each item, its kind's at first, then its picture once that is read on
/// a thread; one read in has the items drawn again.
fn refresh_icons(desktop: &mut Desktop, items: &[Item]) {
    let shown: HashSet<&Item> = items.iter().collect();
    desktop.icons.retain(|(item, _)| shown.contains(item));
    desktop.icon_reads.retain(|(item, _)| shown.contains(item));
    let mut done = Vec::new();
    desktop.icon_reads.retain(
        |(item, read)| match read.lock().ok().and_then(|mut read| read.take()) {
            Some(pixels) => {
                done.push((item.clone(), pixels));
                false
            }
            None => true,
        },
    );
    for (item, pixels) in done {
        if let Some(icon) = pixels.and_then(stacks::pixels_surface) {
            desktop.icons.retain(|(known, _)| *known != item);
            desktop.icons.push((item, icon));
            desktop.drawn = None;
        }
    }
    let missing: Vec<&Item> = {
        let known: HashSet<&Item> = desktop
            .icons
            .iter()
            .map(|(item, _)| item)
            .chain(desktop.icon_reads.iter().map(|(item, _)| item))
            .collect();
        items.iter().filter(|item| !known.contains(item)).collect()
    };
    for item in missing {
        if let Some(icon) = stacks::kind_icon(item, ICON, (1.0, 1.0, 1.0)) {
            desktop.icons.push((item.clone(), icon));
        }
        if let Some(path) = stacks::picture_path(item) {
            let size = ICON * text::picture_scale();
            let read = stacks::read_on_a_thread(path, Size::from((size, size)));
            desktop.icon_reads.push((item.clone(), read));
        }
    }
}

/// The most of the output Quick Look's picture or text takes, under its title.
fn look_room(output: Size<i32, Logical>) -> Size<i32, Logical> {
    Size::from((output.w * 7 / 10, output.h * 7 / 10 - LOOK_TITLE))
}

/// Quick Look's panel for `item`, at most seven tenths of `output`: its name over the
/// picture it is, the start of the text it holds, or else its icon, kind, size and date.
fn draw_quick_look(
    item: &Item,
    pixels: Option<stacks::Pixels>,
    output: Size<i32, Logical>,
    dark: bool,
) -> Option<Picture> {
    let colors = menubar::colors(dark);
    let room = look_room(output);
    let scale = text::picture_scale();
    let image = pixels.and_then(stacks::pixels_surface);
    let shown = (!item.is_dir && gio::content_type_is_a(&item.kind, "text/plain"))
        .then(|| {
            let mut start = Vec::new();
            std::fs::File::open(&item.path)
                .and_then(|file| file.take(LOOK_TEXT_BYTES).read_to_end(&mut start))
                .ok()?;
            Some(
                String::from_utf8_lossy(&start)
                    .lines()
                    .take(LOOK_TEXT_LINES)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })
        .flatten();
    let body: Size<i32, Logical> = match (&image, &shown) {
        (Some(image), _) => {
            image.set_device_scale(f64::from(scale), f64::from(scale));
            let (w, h) = text::points(image);
            (w.ceil() as i32, h.ceil() as i32).into()
        }
        (None, Some(_)) => (room.w.min(720), room.h.min(540)).into(),
        (None, None) => (440, 168).into(),
    };
    let size = Size::<i32, Logical>::from((body.w.max(320), body.h + LOOK_TITLE));
    let surface = text::canvas(size.w, size.h).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let (w, h) = (f64::from(size.w), f64::from(size.h));
    text::rounded_rectangle(&cr, 0.5, 0.5, w - 1.0, h - 1.0, LOOK_RADIUS);
    let (r, g, b) = colors.sheet;
    cr.set_source_rgb(r, g, b);
    let _ = cr.fill_preserve();
    let (r, g, b, a) = colors.edge;
    cr.set_source_rgba(r, g, b, a);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    let ink = |cr: &cairo::Context, (r, g, b): (f64, f64, f64)| cr.set_source_rgb(r, g, b);

    let title = text::new_layout(&cr);
    title.set_font_description(Some(&text::font("Semibold 11")));
    title.set_text(&item.name);
    title.set_width((size.w - 32) * pango::SCALE);
    title.set_ellipsize(pango::EllipsizeMode::Middle);
    title.set_alignment(pango::Alignment::Center);
    let (_, logical) = title.pixel_extents();
    ink(&cr, colors.ink);
    cr.move_to(16.0, f64::from(LOOK_TITLE - logical.height()) / 2.0);
    pangocairo::functions::show_layout(&cr, &title);
    let (r, g, b) = colors.ink;
    cr.set_source_rgba(r, g, b, 0.12);
    cr.rectangle(0.0, f64::from(LOOK_TITLE) - 1.0, w, 1.0);
    let _ = cr.fill();

    let top = f64::from(LOOK_TITLE);
    if let Some(image) = &image {
        let _ = cr.save();
        // The picture's corners go under the panel's rounded ones.
        text::rounded_rectangle(&cr, 0.0, 0.0, w, h, LOOK_RADIUS);
        cr.clip();
        let _ = cr.set_source_surface(image, (w - f64::from(body.w)) / 2.0, top);
        let _ = cr.paint();
        let _ = cr.restore();
    } else if let Some(shown) = &shown {
        let layout = text::new_layout(&cr);
        layout.set_font_description(Some(&text::font("Monospace 10")));
        layout.set_text(shown);
        layout.set_width((body.w - 32) * pango::SCALE);
        layout.set_height((body.h - 24) * pango::SCALE);
        layout.set_wrap(pango::WrapMode::WordChar);
        layout.set_ellipsize(pango::EllipsizeMode::End);
        ink(&cr, colors.ink);
        cr.move_to(16.0, top + 12.0);
        pangocairo::functions::show_layout(&cr, &layout);
    } else {
        if let Some(icon) = stacks::item_icon(item, LOOK_ICON, colors.ink) {
            let _ = cr.save();
            cr.translate(20.0, top + 20.0);
            stacks::paint_fitted(&cr, &icon, f64::from(LOOK_ICON), 1.0);
            let _ = cr.restore();
        }
        let meta = std::fs::metadata(&item.path).ok();
        let modified = meta
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|since| gio::glib::DateTime::from_unix_local(since.as_secs() as i64).ok())
            .and_then(|time| time.format("%Y/%m/%d %H:%M").ok());
        let size_text = meta
            .filter(|meta| !meta.is_dir())
            .map(|meta| gio::glib::format_size(meta.len()));
        let lines = [
            Some(gio::content_type_get_description(&item.kind)),
            size_text,
            modified,
        ];
        let x = f64::from(20 + LOOK_ICON + 20);
        let width = (size.w - 20 - LOOK_ICON - 40) * pango::SCALE;
        let name = text::new_layout(&cr);
        name.set_font_description(Some(&text::font("Semibold 13")));
        name.set_text(&item.name);
        name.set_width(width);
        name.set_ellipsize(pango::EllipsizeMode::Middle);
        ink(&cr, colors.ink);
        cr.move_to(x, top + 40.0);
        pangocairo::functions::show_layout(&cr, &name);
        let mut y = top + 40.0 + f64::from(name.pixel_extents().1.height()) + 6.0;
        ink(&cr, colors.ink_3);
        for line in lines.into_iter().flatten() {
            let layout = text::new_layout(&cr);
            layout.set_font_description(Some(&text::font("11")));
            layout.set_text(&line);
            layout.set_width(width);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            cr.move_to(x, y);
            pangocairo::functions::show_layout(&cr, &layout);
            y += f64::from(layout.pixel_extents().1.height()) + 2.0;
        }
    }
    drop(cr);
    text::picture(surface)
}

/// Writes files copied or dragged off the desktop in the form `mime_type` asks for, on a
/// thread: the program reads it when it gets to its socket.
/// Files dragged off the desktop, as a window that they are dropped on asks for them.
struct DesktopDrag(Vec<PathBuf>);

impl IsAlive for DesktopDrag {
    fn alive(&self) -> bool {
        true
    }
}

impl Source for DesktopDrag {
    fn metadata(&self) -> Option<SourceMetadata> {
        Some(SourceMetadata {
            mime_types: vec![URI_LIST.to_string()],
            dnd_actions: [DndAction::Copy, DndAction::Move].into_iter().collect(),
        })
    }

    fn choose_action(&self, _action: DndAction) {}

    fn send(&self, mime_type: &str, fd: OwnedFd) {
        send_copied(mime_type, &self.0, fd);
    }

    fn drop_performed(&self) {}

    fn cancel(&self) {}

    fn finished(&self) {}
}

pub fn send_copied(mime_type: &str, paths: &[PathBuf], fd: OwnedFd) {
    let uris: Vec<String> = paths
        .iter()
        .map(|path| gio::File::for_path(path).uri().into())
        .collect();
    let text = match mime_type {
        COPIED_FILES => format!("copy\n{}", uris.join("\n")),
        URI_LIST => uris.iter().map(|uri| format!("{uri}\r\n")).collect(),
        _ => paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    std::thread::spawn(move || {
        if let Err(err) = std::fs::File::from(fd).write_all(text.as_bytes()) {
            tracing::warn!("sending copied files: {err}");
        }
    });
}

/// Copies `paths` into `folder` in Nekoland's file operation window, which asks about a
/// name already taken there; a copy into the folder a file is in goes beside it.
fn copy_into(paths: &[PathBuf], folder: &Path) {
    let spawned = std::process::Command::new("nekoland-fileop")
        .arg("copy")
        .arg(folder)
        .args(paths)
        .spawn();
    if let Err(err) = spawned {
        tracing::warn!("nekoland-fileop: {err}");
    }
}

/// The first of `未命名檔案夾`, `未命名檔案夾 2`, … not taken in `folder`.
fn new_folder_path(folder: &Path) -> PathBuf {
    (1..)
        .map(|number| match number {
            1 => folder.join(NEW_FOLDER),
            _ => folder.join(format!("{NEW_FOLDER} {number}")),
        })
        .find(|path| !path.exists())
        .expect("some number is free")
}

/// The box choosing items: see-through white with a lighter edge, as the Mac's.
fn draw_band(size: Size<i32, Logical>) -> Option<Picture> {
    let surface = text::canvas(size.w.max(1), size.h.max(1)).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    cr.rectangle(0.5, 0.5, f64::from(size.w) - 1.0, f64::from(size.h) - 1.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, f64::from(BAND_FILL_ALPHA));
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, f64::from(BAND_EDGE_ALPHA));
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    drop(cr);
    text::picture(surface)
}

/// `icon` dragged alone, as from a folder in the Dock, its middle `middle` from the
/// pointer, where it was taken.
pub(crate) fn dragged_alone(
    icon: &ImageSurface,
    middle: Point<i32, Logical>,
) -> Option<(Picture, Point<i32, Logical>)> {
    let offset = middle + Point::from((-ICON / 2, -ICON / 2 - DRAG_BADGE as i32 / 2));
    draw_dragged(icon, 1).map(|picture| (picture, offset))
}

/// The icon dragged, faded, with how many items go with it on a red badge when more
/// than one does.
fn draw_dragged(icon: &ImageSurface, count: usize) -> Option<Picture> {
    let badge = DRAG_BADGE;
    let surface = text::canvas(ICON + badge as i32 / 2, ICON + badge as i32 / 2).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let _ = cr.save();
    cr.translate(0.0, badge / 2.0);
    stacks::paint_fitted(&cr, icon, f64::from(ICON), 0.7);
    let _ = cr.restore();
    if count > 1 {
        let center = (f64::from(ICON) - badge / 2.0, badge / 2.0);
        cr.set_source_rgb(0.92, 0.26, 0.24);
        cr.arc(center.0, center.1 + 1.0, badge / 2.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        let layout = text::new_layout(&cr);
        layout.set_font_description(Some(&text::font("Bold 9")));
        layout.set_text(&count.to_string());
        let (width, height) = layout.pixel_size();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to(
            center.0 - f64::from(width) / 2.0,
            center.1 + 1.0 - f64::from(height) / 2.0,
        );
        pangocairo::functions::show_layout(&cr, &layout);
    }
    drop(cr);
    text::picture(surface)
}

/// The items drawn on one picture spanning their columns, the chosen ones marked as the
/// Mac does: a grey square behind the icon and the name on the accent colour. A name being
/// typed is drawn in a white field, with what is chosen of it marked and the caret.
fn draw_items(
    items: &[(Item, Rectangle<i32, Logical>)],
    icons: &[(Item, ImageSurface)],
    selected: &[PathBuf],
    renaming: Option<&Rename>,
) -> Option<(Picture, Point<i32, Logical>)> {
    let mut area = items
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|area, rect| area.merge(rect))?;
    if renaming.is_some() {
        area.size.h += RENAME_ROOM;
    }
    let surface = text::canvas(area.size.w, area.size.h).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let [r, g, b, _] = OUTLINE_COLOR.map(f64::from);
    for (item, rect) in items {
        let at = rect.loc - area.loc;
        let (x, y) = (f64::from(at.x), f64::from(at.y));
        let chosen = selected.contains(&item.path);
        let icon_x = x + f64::from(CELL_WIDTH - ICON) / 2.0;
        if chosen {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
            text::rounded_rectangle(
                &cr,
                icon_x - 4.0,
                y + f64::from(ICON_TOP) - 4.0,
                f64::from(ICON + 8),
                f64::from(ICON + 8),
                6.0,
            );
            let _ = cr.fill();
        }
        if let Some((_, icon)) = icons.iter().find(|(known, _)| known == item) {
            let _ = cr.save();
            cr.translate(icon_x, y + f64::from(ICON_TOP));
            stacks::paint_fitted(&cr, icon, f64::from(ICON), 1.0);
            let _ = cr.restore();
        }
        let label_x = x + 4.0;
        let label_y = y + f64::from(LABEL_TOP);
        if let Some(rename) = renaming.filter(|rename| rename.path == item.path) {
            draw_field(&cr, rename, label_x, label_y, (r, g, b));
            continue;
        }
        let layout = label_layout(&cr, &item.name);
        layout.set_height(-2);
        layout.set_ellipsize(pango::EllipsizeMode::Middle);
        let (ink, logical) = layout.pixel_extents();
        if chosen {
            cr.set_source_rgb(r, g, b);
            text::rounded_rectangle(
                &cr,
                label_x + f64::from(ink.x()) - 4.0,
                label_y + f64::from(logical.y()) - 1.0,
                f64::from(ink.width()) + 8.0,
                f64::from(logical.height()) + 2.0,
                4.0,
            );
            let _ = cr.fill();
        } else {
            // A shadow keeps the name readable on any wallpaper.
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
            cr.move_to(label_x, label_y + 1.0);
            pangocairo::functions::show_layout(&cr, &layout);
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to(label_x, label_y);
        pangocairo::functions::show_layout(&cr, &layout);
    }
    drop(cr);
    Some((text::picture(surface)?, area.loc))
}

fn label_layout(cr: &cairo::Context, name: &str) -> pango::Layout {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(&text::font(&LABEL_POINTS.to_string())));
    layout.set_text(name);
    layout.set_width((CELL_WIDTH - 8) * pango::SCALE);
    layout.set_wrap(pango::WrapMode::WordChar);
    layout.set_alignment(pango::Alignment::Center);
    layout
}

/// A name being typed: black on a white field edged in the accent colour, what is chosen
/// of it on a light accent, what the input method composes underlined, and the caret.
fn draw_field(cr: &cairo::Context, rename: &Rename, x: f64, y: f64, (r, g, b): (f64, f64, f64)) {
    let mut shown = rename.text.clone();
    shown.insert_str(rename.caret, &rename.preedit);
    let layout = label_layout(cr, &shown);
    let attributes = pango::AttrList::new();
    let rgb16 = |value: f64| (value * 65535.0) as u16;
    if let Some(end) = rename.chosen {
        let mut chosen = pango::AttrColor::new_background(
            rgb16(0.6 + r * 0.4),
            rgb16(0.6 + g * 0.4),
            rgb16(0.6 + b * 0.4),
        );
        chosen.set_start_index(0);
        chosen.set_end_index(end as u32);
        attributes.insert(chosen);
    }
    if !rename.preedit.is_empty() {
        let mut composing = pango::AttrInt::new_underline(pango::Underline::Single);
        composing.set_start_index(rename.caret as u32);
        composing.set_end_index((rename.caret + rename.preedit.len()) as u32);
        attributes.insert(composing);
    }
    layout.set_attributes(Some(&attributes));
    let (_, logical) = layout.pixel_extents();
    let field = (
        x + f64::from(logical.x()) - 4.0,
        y + f64::from(logical.y()) - 2.0,
        f64::from(logical.width()) + 8.0,
        f64::from(logical.height()) + 4.0,
    );
    text::rounded_rectangle(cr, field.0, field.1, field.2, field.3, 4.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(r, g, b);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, &layout);
    if rename.chosen.is_none() {
        let (caret, _) = layout.cursor_pos((rename.caret + rename.preedit.len()) as i32);
        let scale = f64::from(pango::SCALE);
        cr.rectangle(
            x + f64::from(caret.x()) / scale,
            y + f64::from(caret.y()) / scale,
            1.0,
            f64::from(caret.height()) / scale,
        );
        let _ = cr.fill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_fill_columns_from_the_top_right_corner() {
        let size = Size::from((1000, MENU_BAR_HEIGHT + MARGIN * 2 + CELL_HEIGHT * 3));
        let places = cells(4, size);
        let top = MENU_BAR_HEIGHT + MARGIN;
        let right = size.w - MARGIN - CELL_WIDTH;
        assert_eq!(places[0].loc, Point::from((right, top)));
        assert_eq!(places[2].loc, Point::from((right, top + CELL_HEIGHT * 2)));
        // The fourth starts the column to the left.
        assert_eq!(places[3].loc, Point::from((right - CELL_WIDTH, top)));
    }

    #[test]
    fn placed_items_keep_their_cells_and_the_rest_fill_the_free_ones() {
        let size = Size::from((1000, MENU_BAR_HEIGHT + MARGIN * 2 + CELL_HEIGHT * 3));
        let folder = std::env::temp_dir().join(format!("nekoland-places-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["a", "b", "c"] {
            std::fs::write(folder.join(name), "").unwrap();
        }
        let items = stacks::read_folder(&folder, Sort::Name);
        std::fs::remove_dir_all(&folder).unwrap();
        // `c` asks for `a`'s cell, which `a` came first to; it goes where `a` would have.
        let places = Places::from([("a".into(), [0, 1]), ("c".into(), [0, 1])]);
        let cells = placed_cells(&items, &places, size);
        assert_eq!(cells[0], cell_rect([0, 1], size));
        assert_eq!(cells[1], cell_rect([0, 0], size));
        assert_eq!(cells[2], cell_rect([0, 2], size));
        assert_eq!(cell_of(cells[2].loc + Point::from((30, -30)), size), [0, 2]);
    }

    #[test]
    fn new_folders_are_numbered_after_the_first() {
        let folder = std::env::temp_dir().join(format!("nekoland-desktop-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(new_folder_path(&folder), folder.join(NEW_FOLDER));
        std::fs::create_dir(folder.join(NEW_FOLDER)).unwrap();
        assert_eq!(new_folder_path(&folder), folder.join(format!("{NEW_FOLDER} 2")));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn renaming_replaces_the_name_but_not_its_extension() {
        let text = "筆記.txt".to_string();
        let mut rename = Rename {
            path: PathBuf::from("/tmp/筆記.txt"),
            caret: text.len(),
            chosen: Some(stem_end(&text, false)),
            text,
            preedit: String::new(),
            ime_focused: false,
        };
        rename.insert("日");
        rename.insert("記");
        assert_eq!(rename.text, "日記.txt");
        assert!(rename.key(Keysym::BackSpace));
        assert_eq!(rename.text, "日.txt");
        assert!(rename.key(Keysym::End));
        assert!(rename.key(Keysym::BackSpace));
        assert_eq!(rename.text, "日.tx");
        assert_eq!(stem_end("資料夾.v2", true), "資料夾.v2".len());
        assert_eq!(stem_end(".bashrc", false), ".bashrc".len());
    }
}
