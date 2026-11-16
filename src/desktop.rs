//! The files on the desktop, as on the Mac: what is in the Desktop folder, in columns from
//! the top right corner down. A click chooses one, Shift or Ctrl adds to the choice, and
//! dragging over the desktop draws a box choosing what it touches; a double click opens.
//! What is chosen can be dragged into a folder, onto the Dock or into a window, renamed in
//! place, and opened, put in the Trash or shown in the file manager from the keyboard or
//! the menus a right click opens.

use std::{
    io::Write,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gio::prelude::*;
use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, Kind},
        gles::GlesRenderer,
    },
    input::{keyboard::Keysym, keyboard::ModifiersState, pointer::GrabStartData},
    reexports::wayland_server::protocol::wl_data_device_manager::DndAction,
    utils::{Logical, Point, Rectangle, Serial, Size},
    wayland::selection::data_device::{start_dnd, SourceMetadata},
};

use crate::{
    appgrid::draw_candidates,
    config::{self, Sort},
    dock::{ask_file_manager, move_into, open_path, reveal_path},
    ime::Candidates,
    menubar::{Command, Entry, MENU_BAR_HEIGHT},
    render::{SceneElement, OUTLINE_COLOR},
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
    NewFolder,
    Sort(Sort),
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
    /// The left button held down on the desktop.
    press: Option<Press>,
    /// The box being dragged out to choose items, and its picture.
    band: Option<(Rectangle<i32, Logical>, Picture)>,
    /// What was last dragged off the desktop, for whoever it is dropped on to ask for.
    dragged: Vec<PathBuf>,
    /// The icon following the pointer while dragging, and where it is from the pointer.
    drag_picture: Option<(Picture, Point<i32, Logical>)>,
    /// The folder on the desktop what is dragged would go into, marked as chosen.
    drop_target: Option<PathBuf>,
    pointer: Point<f64, Logical>,
    pub renaming: Option<Rename>,
    pub candidates: Option<Candidates>,
    candidates_picture: Option<(Candidates, Picture)>,
}

type Drawn = (Vec<Item>, Vec<PathBuf>, Option<Rename>, Size<i32, Logical>);

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
    let top = MENU_BAR_HEIGHT + MARGIN;
    let rows = ((size.h - top - MARGIN) / CELL_HEIGHT).max(1) as usize;
    (0..count)
        .map(|index| {
            let (column, row) = ((index / rows) as i32, (index % rows) as i32);
            let x = size.w - MARGIN - (column + 1) * CELL_WIDTH;
            Rectangle::new(
                Point::from((x, top + row * CELL_HEIGHT)),
                Size::from((CELL_WIDTH, CELL_HEIGHT)),
            )
        })
        .collect()
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
        let items = match &folder {
            Some(folder) => self.folder_items(folder, self.config.desktop_sort).to_vec(),
            None => Vec::new(),
        };
        let size = self.output_size();
        let desktop = &mut self.desktop;
        // What went away is no longer chosen.
        desktop
            .selected
            .retain(|path| items.iter().any(|item| item.path == *path));
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
        let wanted = (items, marked, desktop.renaming.clone(), size);
        if desktop.drawn.as_ref() == Some(&wanted) {
            return;
        }
        let (items, selected, renaming, _) = &wanted;
        let places = cells(items.len(), size);
        desktop.items = items.iter().cloned().zip(places.iter().copied()).collect();
        desktop.picture = draw_items(&desktop.items, selected, renaming.as_ref());
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

    /// The icon of what is being dragged off the desktop, over everything.
    pub fn desktop_drag_element(
        &self,
        renderer: &mut GlesRenderer,
        output_loc: Point<i32, Logical>,
        scale: f64,
    ) -> Option<SceneElement> {
        let (picture, offset) = self.desktop.drag_picture.as_ref()?;
        let at = self.desktop.pointer.to_i32_round() + *offset - output_loc;
        picture_element(renderer, picture, at, scale)
    }

    fn desktop_item_at(&self, pos: Point<f64, Logical>) -> Option<Item> {
        let pos = pos.to_i32_round();
        self.desktop
            .items
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(item, _)| item.clone())
    }

    fn modifiers(&self) -> ModifiersState {
        self.seat
            .get_keyboard()
            .map(|keyboard| keyboard.modifier_state())
            .unwrap_or_default()
    }

    /// A press on the desktop, where no window is: an item is chosen, opened on the second
    /// click, or given its menu; the desktop itself gets its own menu, or starts a box.
    /// Returns whether the press was on an item or opened a menu.
    pub fn desktop_press(&mut self, pos: Point<f64, Logical>, right: bool, serial: Serial) -> bool {
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
            return false;
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
        if self.desktop.drag_picture.is_some() {
            self.desktop.drop_target = self.desktop_folder_at(pos).map(|folder| folder.path);
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
        self.desktop.dragged = self.desktop.selected.clone();
        let icon_at = cell.loc + Point::from(((CELL_WIDTH - ICON) / 2, ICON_TOP));
        self.desktop.drag_picture = draw_dragged(item, self.desktop.dragged.len())
            .map(|picture| (picture, icon_at - press.from.to_i32_round()));
        let start = GrabStartData {
            focus: None,
            button: BUTTON_LEFT,
            location: press.from,
        };
        let metadata = SourceMetadata {
            mime_types: vec![URI_LIST.to_string()],
            dnd_action: DndAction::Copy | DndAction::Move,
        };
        let (display, seat) = (self.display_handle.clone(), self.seat.clone());
        start_dnd(&display, &seat, self, press.serial, Some(start), None, metadata);
    }

    /// A window asked for what is dragged off the desktop.
    pub fn desktop_drag_send(&mut self, fd: OwnedFd) {
        let list: String = self
            .desktop
            .dragged
            .iter()
            .map(|path| format!("{}\r\n", gio::File::for_path(path).uri()))
            .collect();
        // On a thread: the program reads it when it gets to its socket.
        std::thread::spawn(move || {
            if let Err(err) = std::fs::File::from(fd).write_all(list.as_bytes()) {
                tracing::warn!("sending dragged files: {err}");
            }
        });
    }

    /// The drag off the desktop ended, taken by a window or not.
    pub fn desktop_drag_ended(&mut self) {
        self.desktop.drag_picture = None;
        self.desktop.drop_target = None;
    }

    /// No window took what was dragged off the desktop: on the Dock, it goes where the
    /// Dock puts files; on a folder on the desktop, into it.
    pub fn desktop_dropped_here(&mut self) {
        let pos = self.desktop.pointer;
        let paths = self.desktop.dragged.clone();
        if self.drop_files_on_dock(pos, &paths) {
            return;
        }
        if let Some(folder) = self.desktop_folder_at(pos) {
            move_into(&paths, &folder.path);
        }
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
        let command = match sym {
            Keysym::n if modifiers.logo && modifiers.shift => DesktopCommand::NewFolder,
            Keysym::Up | Keysym::Down | Keysym::Left | Keysym::Right if plain => {
                self.move_choice(sym);
                return true;
            }
            _ if chosen.is_empty() => return false,
            Keysym::o | Keysym::Down if modifiers.logo => DesktopCommand::Open(chosen),
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
            Ok(renamed) => self.desktop.selected = renamed.path().into_iter().collect(),
            Err(err) => tracing::warn!("{}: {err}", rename.path.display()),
        }
    }

    fn desktop_menu(&self) -> Vec<Entry> {
        let sort = self.config.desktop_sort;
        let sorts = [
            (Sort::Name, "名稱"),
            (Sort::Added, "加入日期"),
            (Sort::Modified, "修改日期"),
            (Sort::Created, "建立日期"),
            (Sort::Kind, "種類"),
        ]
        .into_iter()
        .map(|(by, label)| Entry::item(label, Command::Desktop(DesktopCommand::Sort(by))).checked(sort == by))
        .collect();
        vec![
            Entry::item("新增檔案夾", Command::Desktop(DesktopCommand::NewFolder)).shortcut("Super Shift N"),
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
        entries
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
                if let Err(err) = config::set(&format!("desktop_sort = {:?}", sort.name())) {
                    tracing::warn!("{err}");
                }
            }
            DesktopCommand::Wallpaper => config::spawn("NEKOLAND_SETTINGS_PAGE=桌面 nekoland-settings"),
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
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.15);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.6);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    drop(cr);
    text::picture(surface)
}

/// The icon dragged, faded, with how many items go with it on a red badge when more
/// than one does.
fn draw_dragged(item: &Item, count: usize) -> Option<Picture> {
    let icon = stacks::item_icon(item, ICON, (1.0, 1.0, 1.0))?;
    let badge = 22.0;
    let surface = text::canvas(ICON + badge as i32 / 2, ICON + badge as i32 / 2).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let _ = cr.save();
    cr.translate(0.0, badge / 2.0);
    stacks::paint_fitted(&cr, &icon, f64::from(ICON), 0.7);
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
        if let Some(icon) = stacks::item_icon(item, ICON, (1.0, 1.0, 1.0)) {
            let _ = cr.save();
            cr.translate(icon_x, y + f64::from(ICON_TOP));
            stacks::paint_fitted(&cr, &icon, f64::from(ICON), 1.0);
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
