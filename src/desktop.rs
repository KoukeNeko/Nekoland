//! The files on the desktop, as on the Mac: what is in the Desktop folder, in columns from
//! the top right corner down, chosen with a click and opened with a double click, and the
//! menus a right click opens on one of them or on the desktop itself.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gio::prelude::*;
use smithay::{
    backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, Kind},
        gles::GlesRenderer,
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    config::{self, Sort},
    dock::{open_path, reveal_path},
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
/// The name a new folder is given, numbered after the first.
const NEW_FOLDER: &str = "未命名檔案夾";

#[derive(Clone, Debug, PartialEq)]
pub enum DesktopCommand {
    Open(PathBuf),
    Reveal(PathBuf),
    Trash(PathBuf),
    NewFolder,
    Sort(Sort),
    Wallpaper,
}

#[derive(Default)]
pub struct Desktop {
    /// The items, as last read, and where each is.
    items: Vec<(Item, Rectangle<i32, Logical>)>,
    selected: Option<PathBuf>,
    last_click: Option<(PathBuf, Instant)>,
    /// The items drawn, at the place of the first column's top, and what that picture
    /// showed, to know when to draw them again.
    picture: Option<(Picture, Point<i32, Logical>)>,
    drawn: Option<(Vec<Item>, Option<PathBuf>, Size<i32, Logical>)>,
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

impl Nekoland {
    /// Reads the Desktop folder again when it has changed and draws its items when they,
    /// the choice among them or the output's size have.
    pub fn refresh_desktop(&mut self) {
        let folder = desktop_folder().filter(|_| self.config.desktop_icons);
        let items = match &folder {
            Some(folder) => self.folder_items(folder, self.config.desktop_sort).to_vec(),
            None => Vec::new(),
        };
        let size = self.output_size();
        let wanted = (items, self.desktop.selected.clone(), size);
        if self.desktop.drawn.as_ref() == Some(&wanted) {
            return;
        }
        let (items, selected, _) = &wanted;
        let places = cells(items.len(), size);
        self.desktop.items = items.iter().cloned().zip(places.iter().copied()).collect();
        self.desktop.picture = draw_items(&self.desktop.items, selected.as_deref());
        self.desktop.drawn = Some(wanted);
    }

    /// The desktop's items, over the wallpaper and under every window.
    pub fn desktop_elements(
        &self,
        renderer: &mut GlesRenderer,
        output_loc: Point<i32, Logical>,
        scale: f64,
    ) -> Option<SceneElement> {
        let (picture, at) = self.desktop.picture.as_ref()?;
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (*at - output_loc).to_f64().to_physical(scale),
            &picture.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(SceneElement::Picture)
    }

    fn desktop_item_at(&self, pos: Point<f64, Logical>) -> Option<Item> {
        let pos = pos.to_i32_round();
        self.desktop
            .items
            .iter()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(item, _)| item.clone())
    }

    /// A press on the desktop, where no window is: an item is chosen, opened on the second
    /// click, or given its menu; the desktop itself gets its own menu. Returns whether the
    /// press was taken.
    pub fn desktop_press(&mut self, pos: Point<f64, Logical>, right: bool) -> bool {
        let item = self.desktop_item_at(pos);
        self.desktop.selected = item.as_ref().map(|item| item.path.clone());
        if right {
            let entries = match &item {
                Some(item) => item_menu(item),
                None => self.desktop_menu(),
            };
            self.open_menu_at(entries, pos.to_i32_round());
            return true;
        }
        let Some(item) = item else {
            self.desktop.last_click = None;
            return false;
        };
        let twice = self
            .desktop
            .last_click
            .take()
            .is_some_and(|(path, at)| path == item.path && at.elapsed() < DOUBLE_CLICK);
        if twice {
            open_path(&item.path);
        } else {
            self.desktop.last_click = Some((item.path, Instant::now()));
        }
        true
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
            Entry::item("新增檔案夾", Command::Desktop(DesktopCommand::NewFolder)),
            Entry::separator(),
            Entry::submenu("排序方式", sorts),
            Entry::separator(),
            Entry::item("更改桌布⋯", Command::Desktop(DesktopCommand::Wallpaper)),
        ]
    }

    pub fn run_desktop_command(&mut self, command: DesktopCommand) {
        match command {
            DesktopCommand::Open(path) => open_path(&path),
            DesktopCommand::Reveal(path) => reveal_path(&path),
            DesktopCommand::Trash(path) => {
                if let Err(err) = gio::File::for_path(&path).trash(gio::Cancellable::NONE) {
                    tracing::warn!("{}: {err}", path.display());
                }
                self.desktop.selected = None;
            }
            DesktopCommand::NewFolder => {
                let Some(folder) = desktop_folder() else {
                    return;
                };
                let path = new_folder_path(&folder);
                match std::fs::create_dir_all(&folder).and_then(|()| std::fs::create_dir(&path)) {
                    Ok(()) => self.desktop.selected = Some(path),
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

fn item_menu(item: &Item) -> Vec<Entry> {
    let path = || item.path.clone();
    vec![
        Entry::item("打開", Command::Desktop(DesktopCommand::Open(path()))),
        Entry::item("在檔案中顯示", Command::Desktop(DesktopCommand::Reveal(path()))),
        Entry::separator(),
        Entry::item("丟到垃圾桶", Command::Desktop(DesktopCommand::Trash(path()))),
    ]
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

/// The items drawn on one picture spanning their columns, the chosen one marked as the
/// Mac does: a grey square behind its icon and its name on the accent colour.
fn draw_items(
    items: &[(Item, Rectangle<i32, Logical>)],
    selected: Option<&Path>,
) -> Option<(Picture, Point<i32, Logical>)> {
    let area = items
        .iter()
        .map(|(_, rect)| *rect)
        .reduce(|area, rect| area.merge(rect))?;
    let surface = text::canvas(area.size.w, area.size.h).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    let [r, g, b, _] = OUTLINE_COLOR.map(f64::from);
    for (item, rect) in items {
        let at = rect.loc - area.loc;
        let (x, y) = (f64::from(at.x), f64::from(at.y));
        let chosen = selected == Some(item.path.as_path());
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
        let layout = text::new_layout(&cr);
        layout.set_font_description(Some(&text::font(&LABEL_POINTS.to_string())));
        layout.set_text(&item.name);
        layout.set_width((CELL_WIDTH - 8) * pango::SCALE);
        layout.set_height(-2);
        layout.set_wrap(pango::WrapMode::WordChar);
        layout.set_ellipsize(pango::EllipsizeMode::Middle);
        layout.set_alignment(pango::Alignment::Center);
        let (ink, logical) = layout.pixel_extents();
        let label_x = x + 4.0;
        let label_y = y + f64::from(LABEL_TOP);
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
}
