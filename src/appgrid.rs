//! The overview's applications, as GNOME shows them: the desktops' bar stays open at the
//! top, a search field under it, and every application below, a page at a time. Typing
//! narrows them down; a click starts one on the desktop shown, and one dragged onto a
//! desktop in the bar starts there, onto the add button on a new desktop, onto the Dock
//! is kept in it.
//!
//! As on the Mac's Launchpad, the grid is arranged by dragging: an application dropped
//! between others goes there, onto another makes a folder of the two, onto a folder goes
//! into it. A folder opens over the grid; its applications are reordered there or dragged
//! out of it, and its name is changed by clicking it. The arrangement is kept in
//! `app-grid.toml` beside the configuration.
//!
//! An icon held down, or 編輯 in the menu a right click opens, starts arranging: the icons
//! shake, and a click no longer starts anything until Escape or a click beside them ends
//! it. The same menu puts the arrangement back as it was at first.

use std::time::Instant;

use cairo::{Context, Format, ImageSurface};
use gio::prelude::*;
use serde::{Deserialize, Serialize};
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point, Rectangle, Size},
};

mod list;

use crate::{
    config::{self, AppsView},
    dock::paint_pane,
    glass::Tier,
    menubar::{colors, draw_field, Overlay, FIELD_HEIGHT},
    overview::{expanded_height, BarAction},
    text::{self, Picture},
    Nekoland,
};

const CELL_WIDTH: i32 = 132;
const CELL_HEIGHT: i32 = 128;
const ICON: i32 = 66;
/// How far under the top of its place an icon is.
const ICON_TOP: i32 = 12;
/// How much the icon under the pointer grows, as on the iPad, its name staying as it is.
const HOVER_LIFT: f64 = 1.08;
/// How many seconds an icon takes to grow most of the way under the pointer, or to settle
/// back: each moment it goes a share of what is left, so it starts quick and lands softly.
const LIFT_TIME: f64 = 0.1;
/// Nearer its own size than this, an icon the pointer left is drawn at it.
const LIFT_AT_REST: f64 = 0.002;
/// How finely a lift is told apart from the one drawn before it.
const LIFT_STEPS: f64 = 1000.0;
const SEARCH_WIDTH: i32 = 360;
/// Room under the grid for the dots that tell the pages.
const DOTS: i32 = 40;
/// How long the view and a folder take to fade in, and a page to slide to the next.
const FADE_TIME: f64 = 0.2;
/// Nearer its size than this, the view is drawn at it.
const ZOOM_AT_REST: f64 = 0.0005;
const PAGE_TIME: f64 = 0.25;
/// How much the wheel has to turn, in its own units, to go a page on.
const PAGE_SCROLL: f64 = 10.0;
const DRAG_THRESHOLD: f64 = 6.0;
const HELD_SIZE: i32 = 72;
/// How long an icon is held down before the grid is being arranged.
const HOLD_TIME: f64 = 0.5;
/// How far an icon being arranged turns either way, in radians, and how often a second.
const SHAKE: f64 = 0.03;
const SHAKE_RATE: f64 = 4.0;
/// How long a dragged icon rests at the grid's edge before the page turns.
const EDGE_TIME: f64 = 0.6;
const EDGE: f64 = 28.0;
/// Within this of an icon's middle, a dragged application goes onto it, not beside it.
const ONTO: f64 = 30.0;
const FOLDER_PADDING: i32 = 20;
const FOLDER_TITLE: i32 = 48;
const FOLDER_COLUMNS: usize = 5;
const FOLDER_ROWS: usize = 4;
const FOLDER_RADIUS: i32 = 24;
const ARRANGEMENT: &str = "app-grid.toml";

struct GridApp {
    /// The desktop entry's name without `.desktop`, as the Dock keeps it.
    id: String,
    name: String,
    /// Everything it can be found by, in lower case.
    words: String,
    icons: Vec<String>,
    /// What a folder made with it is called.
    category: &'static str,
    info: gio::DesktopAppInfo,
}

/// A place on the grid: an application, or a folder of them.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Entry {
    App(usize),
    Folder(usize),
}

/// What is drawn in a place on the page: an entry, or the room kept for a dragged one.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Cell {
    Entry(Entry),
    Gap,
}

struct Folder {
    id: String,
    name: String,
    apps: Vec<usize>,
}

/// The arrangement as kept between sessions, by the applications' IDs.
#[derive(Serialize, Deserialize, Default)]
struct Arrangement {
    /// Applications and `folder:ID`s, in the grid's order.
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    folders: Vec<SavedFolder>,
    /// The applications last started from here, the latest first.
    #[serde(default)]
    recent: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct SavedFolder {
    id: String,
    name: String,
    apps: Vec<String>,
}

/// Where the parts of the view are on the output.
#[derive(Clone, Copy, PartialEq)]
struct Layout {
    search: Rectangle<i32, Logical>,
    /// The pages' grid; for the long page, the part of it in view.
    grid: Rectangle<i32, Logical>,
    columns: usize,
    rows: usize,
    /// The list of kinds beside the icons, in the sidebar style.
    sidebar: Option<Rectangle<i32, Logical>>,
}

impl Layout {
    fn per_page(&self) -> usize {
        self.columns * self.rows
    }

    /// Which place on the page a point is on, and how far from that place's icon middle.
    fn cell_at(&self, pos: Point<f64, Logical>) -> Option<(usize, f64)> {
        cell_at(self.grid, self.columns, self.rows, pos)
    }
}

/// Which place of a grid of `columns` by `rows` at `grid` a point is on, and how far from
/// that place's icon middle.
fn cell_at(
    grid: Rectangle<i32, Logical>,
    columns: usize,
    rows: usize,
    pos: Point<f64, Logical>,
) -> Option<(usize, f64)> {
    let x = pos.x - f64::from(grid.loc.x);
    let y = pos.y - f64::from(grid.loc.y);
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let (column, row) = (
        (x / f64::from(CELL_WIDTH)) as usize,
        (y / f64::from(CELL_HEIGHT)) as usize,
    );
    if column >= columns || row >= rows {
        return None;
    }
    let middle = (
        (column as f64 + 0.5) * f64::from(CELL_WIDTH),
        row as f64 * f64::from(CELL_HEIGHT) + f64::from(ICON_TOP) + f64::from(ICON) / 2.0,
    );
    Some((row * columns + column, (x - middle.0).hypot(y - middle.1)))
}

/// Where the icon of a grid's place is on screen.
fn icon_rect(grid: Rectangle<i32, Logical>, columns: usize, cell: usize) -> Rectangle<i32, Logical> {
    let (column, row) = ((cell % columns.max(1)) as i32, (cell / columns.max(1)) as i32);
    Rectangle::new(
        grid.loc + Point::from((column * CELL_WIDTH + (CELL_WIDTH - ICON) / 2, row * CELL_HEIGHT + ICON_TOP)),
        Size::from((ICON, ICON)),
    )
}

/// An icon's worth of the screen around a point, for what was started by a click there.
fn icon_around(pos: Point<f64, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(pos.to_i32_round() - Point::from((ICON / 2, ICON / 2)), Size::from((ICON, ICON)))
}

/// Where an icon being dragged came from.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Grid,
    Folder(usize),
}

struct Held {
    entry: Entry,
    from: Source,
    grab: Point<f64, Logical>,
}

/// Where a dragged icon would go if let go now.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Target {
    Nowhere,
    /// Between the grid's entries, before the one at this place among those left.
    Between(usize),
    /// Onto the entry at this place among those left.
    Onto(usize),
    /// Among the open folder's applications.
    InFolder(usize),
}

/// How far each icon lately under the pointer has grown, from 0 to 1, by its place.
#[derive(Clone, Debug, Default, PartialEq)]
struct Lifts(Vec<(usize, f64)>);

impl Lifts {
    /// Moves every icon on, `seconds` later: the one at `hovered` toward its larger size,
    /// the others back, all at once where nothing is `animated`.
    fn step(&mut self, hovered: Option<usize>, seconds: f64, animated: bool) {
        if let Some(index) = hovered.filter(|index| self.0.iter().all(|(at, _)| at != index)) {
            self.0.push((index, 0.0));
        }
        let share = if animated { 1.0 - (-seconds / LIFT_TIME).exp() } else { 1.0 };
        for (index, lifted) in &mut self.0 {
            let goal = if Some(*index) == hovered { 1.0 } else { 0.0 };
            *lifted += (goal - *lifted) * share;
        }
        self.0
            .retain(|(index, lifted)| Some(*index) == hovered || *lifted > LIFT_AT_REST);
    }

    /// How many times its size the icon at a place is drawn.
    fn of(&self, index: usize) -> f64 {
        let lifted = self.0.iter().find(|(at, _)| *at == index).map_or(0.0, |(_, lifted)| *lifted);
        1.0 + (HOVER_LIFT - 1.0) * lifted
    }

    /// What tells one drawing of the lifts from the next.
    fn drawn(&self) -> Vec<(usize, u16)> {
        self.0
            .iter()
            .map(|(index, lifted)| (*index, (lifted * LIFT_STEPS).round() as u16))
            .collect()
    }
}

/// What a press went down on.
#[derive(Clone, Copy, PartialEq)]
enum Pressed {
    Cell(usize),
    FolderCell(usize),
    /// An application on the long page.
    App(usize),
}

/// What a folder was drawn with: its name, its applications, where room was kept,
/// whether it was being renamed, and how far the icons pointed at had grown.
type FolderDrawn = (String, Vec<usize>, Option<usize>, bool, Vec<(usize, u16)>, Option<u128>);
/// What the grid was drawn with: what was typed, how far the pages had turned, how far the
/// icons pointed at had grown, the places, where a dragged icon was aimed, whether a folder
/// was open, and how far the icons had shaken.
type GridDrawn = (String, i64, Vec<(usize, u16)>, Vec<Cell>, Target, bool, Option<u128>);

/// Where an open folder is: its pane, its grid, and the grid's columns and rows.
struct FolderPlace {
    panel: Rectangle<i32, Logical>,
    grid: Rectangle<i32, Logical>,
    columns: usize,
    rows: usize,
}

struct OpenFolder {
    folder: usize,
    renaming: bool,
    opened: Instant,
    hovered: Option<usize>,
    lifts: Lifts,
    picture: Option<Picture>,
    drawn: Option<FolderDrawn>,
}

pub struct AppGrid {
    style: AppsView,
    apps: Vec<GridApp>,
    /// The latest started, by ID.
    recent: Vec<String>,
    /// How far down the long page is scrolled, and the kind chosen in the sidebar.
    scroll: f64,
    category: usize,
    /// The application the arrow keys have come to on the long page.
    focus: Option<usize>,
    list_picture: Option<Picture>,
    list_drawn: Option<list::ListDrawn>,
    sidebar_picture: Option<(list::SidebarDrawn, Picture)>,
    entries: Vec<Entry>,
    folders: Vec<Folder>,
    query: String,
    /// What the input method is composing, shown after the query until it is committed;
    /// and what it offers to choose from.
    pub preedit: String,
    pub candidates: Option<crate::ime::Candidates>,
    candidates_picture: Option<(crate::ime::Candidates, Picture)>,
    /// Whether the input method has been told that this view takes keys.
    pub ime_focused: bool,
    page: usize,
    /// The page shown, on its way to `page`.
    shown: f64,
    hovered: Option<usize>,
    lifts: Lifts,
    press: Option<(Pressed, Point<f64, Logical>)>,
    pressed: Instant,
    /// Since when the grid is being arranged.
    editing: Option<Instant>,
    held: Option<Held>,
    target: Target,
    /// Where room is kept for a dragged icon among the grid's entries: first where it was,
    /// then where it was last aimed between others.
    gap: Option<usize>,
    /// Since when a dragged icon has rested at an edge of the grid, and which.
    edge: Option<(bool, Instant)>,
    open: Option<OpenFolder>,
    opened: Instant,
    last_tick: Instant,
    scrolled: f64,
    layout: Option<Layout>,
    picture: Option<Picture>,
    drawn: Option<GridDrawn>,
    search: Option<((String, String), Picture)>,
    held_picture: Option<Picture>,
    backdrop: Option<(Size<i32, Logical>, Picture)>,
    ids: [Id; 7],
}

/// What the menu a right click on the grid opens offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppsCommand {
    Edit,
    Reset,
}

/// The kind of application a desktop entry says it is, as a folder of it is named.
fn category(info: &gio::DesktopAppInfo) -> &'static str {
    let categories = info.categories().map(|text| text.to_string()).unwrap_or_default();
    let has = |names: &[&str]| categories.split(';').any(|category| names.contains(&category));
    if has(&["Game"]) {
        "遊戲"
    } else if has(&["WebBrowser", "InstantMessaging", "Chat", "Email", "Network"]) {
        "網際網路"
    } else if has(&["Graphics", "Audio", "Video", "AudioVideo"]) {
        "圖片與影音"
    } else if has(&["Office"]) {
        "辦公"
    } else if has(&["Development"]) {
        "開發"
    } else if has(&[
        "System",
        "Settings",
        "HardwareSettings",
        "PackageManager",
        "Security",
    ]) {
        "系統"
    } else if has(&["Utility"]) {
        "工具程式"
    } else {
        "資料夾"
    }
}

/// Every application the desktop shows, A to Z by name.
fn installed() -> Vec<GridApp> {
    let mut apps: Vec<GridApp> = gio::AppInfo::all()
        .into_iter()
        .filter(|app| app.should_show())
        .filter_map(|app| app.downcast::<gio::DesktopAppInfo>().ok())
        .filter_map(|info| {
            let id = info.id()?.trim_end_matches(".desktop").to_string();
            let name = info.display_name().to_string();
            let words = [
                name.clone(),
                info.name().to_string(),
                info.generic_name()
                    .map(|name| name.to_string())
                    .unwrap_or_default(),
                info.keywords()
                    .iter()
                    .map(|word| word.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                info.executable().display().to_string(),
                id.clone(),
            ]
            .join(" ")
            .to_lowercase();
            let icons = info
                .icon()
                .and_then(|icon| icon.downcast::<gio::ThemedIcon>().ok())
                .map(|icon| icon.names().iter().map(|name| name.to_string()).collect())
                .unwrap_or_default();
            Some(GridApp {
                id,
                name,
                words,
                icons,
                category: category(&info),
                info,
            })
        })
        .collect();
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

/// The applications for what was typed: names starting with it first, then the rest
/// that mention it anywhere, as indexes into `words`, which are in lower case.
fn matching(words: &[(&str, &str)], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<usize> = (0..words.len())
        .filter(|&i| query.is_empty() || words[i].1.contains(&query))
        .collect();
    if !query.is_empty() {
        found.sort_by_key(|&i| !words[i].0.to_lowercase().starts_with(&query));
    }
    found
}

/// The grid's entries and folders from what was kept: kept places first, then the
/// applications installed since, in the order they come.
fn arrange(ids: &[String], kept: &Arrangement) -> (Vec<Entry>, Vec<Folder>) {
    let index = |id: &str| ids.iter().position(|known| known == id);
    let folders: Vec<Folder> = kept
        .folders
        .iter()
        .map(|folder| Folder {
            id: folder.id.clone(),
            name: folder.name.clone(),
            apps: folder.apps.iter().filter_map(|id| index(id)).collect(),
        })
        .collect();
    let in_folder = |app: usize| folders.iter().any(|folder| folder.apps.contains(&app));
    let mut entries = Vec::new();
    for id in &kept.order {
        let entry = match id.strip_prefix("folder:") {
            Some(id) => folders
                .iter()
                .position(|folder| folder.id == id)
                .map(Entry::Folder),
            None => index(id).filter(|&app| !in_folder(app)).map(Entry::App),
        };
        if let Some(entry) = entry.filter(|entry| !entries.contains(entry)) {
            entries.push(entry);
        }
    }
    for folder in 0..folders.len() {
        if !entries.contains(&Entry::Folder(folder)) {
            entries.push(Entry::Folder(folder));
        }
    }
    for app in 0..ids.len() {
        if !in_folder(app) && !entries.contains(&Entry::App(app)) {
            entries.push(Entry::App(app));
        }
    }
    entries.retain(|entry| !matches!(entry, Entry::Folder(folder) if folders[*folder].apps.is_empty()));
    (entries, folders)
}

impl AppGrid {
    pub fn style(&self) -> AppsView {
        self.style
    }

    fn new(query: &str, style: AppsView) -> Self {
        let apps = installed();
        let kept: Arrangement = config::config_file(ARRANGEMENT)
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| {
                toml::from_str(&text)
                    .inspect_err(|err| tracing::warn!("{ARRANGEMENT}: {err}"))
                    .ok()
            })
            .unwrap_or_default();
        let ids: Vec<String> = apps.iter().map(|app| app.id.clone()).collect();
        let (entries, folders) = arrange(&ids, &kept);
        Self {
            style,
            recent: kept.recent.clone(),
            scroll: 0.0,
            category: 0,
            focus: None,
            list_picture: None,
            list_drawn: None,
            sidebar_picture: None,
            apps,
            entries,
            folders,
            query: query.to_string(),
            preedit: String::new(),
            candidates: None,
            candidates_picture: None,
            ime_focused: false,
            page: 0,
            shown: 0.0,
            hovered: None,
            lifts: Lifts::default(),
            press: None,
            pressed: Instant::now(),
            editing: None,
            held: None,
            target: Target::Nowhere,
            gap: None,
            edge: None,
            open: None,
            opened: Instant::now(),
            last_tick: Instant::now(),
            scrolled: 0.0,
            layout: None,
            picture: None,
            drawn: None,
            search: None,
            held_picture: None,
            backdrop: None,
            ids: std::array::from_fn(|_| Id::new()),
        }
    }

    /// Writes the arrangement where it is kept.
    fn save(&self) {
        let arrangement = Arrangement {
            order: self
                .entries
                .iter()
                .map(|entry| match entry {
                    Entry::App(app) => self.apps[*app].id.clone(),
                    Entry::Folder(folder) => format!("folder:{}", self.folders[*folder].id),
                })
                .collect(),
            folders: self
                .folders
                .iter()
                .filter(|folder| !folder.apps.is_empty())
                .map(|folder| SavedFolder {
                    id: folder.id.clone(),
                    name: folder.name.clone(),
                    apps: folder.apps.iter().map(|&app| self.apps[app].id.clone()).collect(),
                })
                .collect(),
            recent: self.recent.clone(),
        };
        let Some(path) = config::config_file(ARRANGEMENT) else {
            return;
        };
        let written = toml::to_string(&arrangement)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
                std::fs::write(&path, text)
            });
        if let Err(err) = written {
            tracing::warn!("{}: {err}", path.display());
        }
    }

    fn matches(&self) -> Vec<usize> {
        let words: Vec<(&str, &str)> = self
            .apps
            .iter()
            .map(|app| (app.name.as_str(), app.words.as_str()))
            .collect();
        matching(&words, &self.query)
    }

    /// The entries as they stand, the one being dragged out of the grid left out.
    fn remaining(&self) -> Vec<Entry> {
        match &self.held {
            Some(held) => self.without(held.entry, held.from),
            None => self.entries.clone(),
        }
    }

    /// The grid's entries without one being dragged out of it.
    fn without(&self, entry: Entry, from: Source) -> Vec<Entry> {
        self.entries
            .iter()
            .copied()
            .filter(|other| !(from == Source::Grid && *other == entry))
            .collect()
    }

    /// What the pages show: what matches what was typed, or the arrangement with room
    /// kept where a dragged icon would go.
    fn cells(&self) -> Vec<Cell> {
        if !self.query.is_empty() {
            return self
                .matches()
                .into_iter()
                .map(|app| Cell::Entry(Entry::App(app)))
                .collect();
        }
        let mut cells: Vec<Cell> = self.remaining().into_iter().map(Cell::Entry).collect();
        if let (Some(_), Some(at)) = (&self.held, self.gap) {
            cells.insert(at.min(cells.len()), Cell::Gap);
        }
        cells
    }

    fn pages(&self, layout: &Layout) -> usize {
        self.cells().len().div_ceil(layout.per_page()).max(1)
    }

    fn fade(&self) -> f32 {
        fade_since(self.opened)
    }

    /// How far the icons have shaken, a step for each frame, while the grid is arranged.
    fn shake(&self) -> Option<u128> {
        self.editing
            .filter(|_| self.query.is_empty())
            .map(|since| since.elapsed().as_millis() / 8)
    }

    /// Puts every application back where it is with nothing arranged, out of its folder.
    fn reset(&mut self) {
        let ids: Vec<String> = self.apps.iter().map(|app| app.id.clone()).collect();
        (self.entries, self.folders) = arrange(&ids, &Arrangement::default());
        self.open = None;
        self.page = 0;
        self.save();
    }

    /// Where the open folder is, and its grid of applications.
    fn folder_layout(&self, layout: &Layout) -> Option<FolderPlace> {
        let open = self.open.as_ref()?;
        let count = self.folders.get(open.folder)?.apps.len().max(1);
        let columns = count.clamp(3, FOLDER_COLUMNS);
        let rows = count.div_ceil(columns).clamp(1, FOLDER_ROWS);
        let size = Size::from((
            columns as i32 * CELL_WIDTH + FOLDER_PADDING * 2,
            rows as i32 * CELL_HEIGHT + FOLDER_PADDING * 2 + FOLDER_TITLE,
        ));
        let center = layout.grid.loc + Point::from((layout.grid.size.w / 2, layout.grid.size.h / 2));
        let panel = Rectangle::new(center - Point::from((size.w / 2, size.h / 2)), size);
        let grid = Rectangle::new(
            panel.loc + Point::from((FOLDER_PADDING, FOLDER_PADDING + FOLDER_TITLE)),
            Size::from((columns as i32 * CELL_WIDTH, rows as i32 * CELL_HEIGHT)),
        );
        Some(FolderPlace {
            panel,
            grid,
            columns,
            rows,
        })
    }

    /// The glass under it all, a darker tint over the blurred desktop, and the search field.
    fn refresh_chrome(&mut self, layout: &Layout, output_size: Option<Size<i32, Logical>>) {
        if self.backdrop.as_ref().map(|(size, _)| Some(*size)) != Some(output_size) {
            self.backdrop = output_size.and_then(|size| {
                let surface = text::canvas(size.w, size.h).ok()?;
                {
                    let cr = Context::new(&surface).ok()?;
                    cr.set_source_rgba(0.08, 0.08, 0.1, 0.42);
                    cr.paint().ok()?;
                }
                Some((size, text::picture(surface)?))
            });
        }
        let typed = (self.query.clone(), self.preedit.clone());
        let stale = self
            .search
            .as_ref()
            .is_none_or(|(drawn, picture)| *drawn != typed || picture.size != layout.search.size);
        if stale {
            self.search = draw_search(&typed.0, &typed.1, layout.search.size).map(|picture| (typed, picture));
        }
        if self.candidates.as_ref() != self.candidates_picture.as_ref().map(|(drawn, _)| drawn) {
            self.candidates_picture = self
                .candidates
                .clone()
                .and_then(|candidates| Some((candidates.clone(), draw_candidates(&candidates)?)));
        }
    }

    /// Lets a dragged icon go where it is aimed, and keeps the new arrangement.
    fn drop_held(&mut self, held: Held) {
        let Held { entry, from, .. } = held;
        if self.target == Target::Nowhere {
            return;
        }
        let Entry::App(app) = entry else {
            // A folder only moves among the others.
            if let Target::Between(at) = self.target {
                let mut entries = self.without(entry, from);
                entries.insert(at.min(entries.len()), entry);
                self.entries = entries;
                self.save();
            }
            return;
        };
        if let (Source::Folder(folder), Target::InFolder(at)) = (from, self.target) {
            let apps = &mut self.folders[folder].apps;
            apps.retain(|&other| other != app);
            apps.insert(at.min(apps.len()), app);
            self.save();
            return;
        }
        let mut entries = self.without(entry, from);
        if let Source::Folder(folder) = from {
            self.folders[folder].apps.retain(|&other| other != app);
        }
        match self.target {
            Target::Between(at) => entries.insert(at.min(entries.len()), entry),
            Target::Onto(at) => match entries.get(at).copied() {
                Some(Entry::Folder(folder)) => self.folders[folder].apps.push(app),
                Some(Entry::App(other)) => {
                    let id = (0..)
                        .map(|n: usize| format!("folder-{n}"))
                        .find(|id| self.folders.iter().all(|folder| folder.id != *id))
                        .unwrap_or_default();
                    self.folders.push(Folder {
                        id,
                        name: self.apps[other].category.to_string(),
                        apps: vec![other, app],
                    });
                    entries[at] = Entry::Folder(self.folders.len() - 1);
                }
                None => entries.push(entry),
            },
            Target::InFolder(_) | Target::Nowhere => {}
        }
        // A folder its last application left goes too.
        entries
            .retain(|entry| !matches!(entry, Entry::Folder(folder) if self.folders[*folder].apps.is_empty()));
        self.entries = entries;
        self.save();
    }
}

fn fade_since(at: Instant) -> f32 {
    let t = (at.elapsed().as_secs_f64() / FADE_TIME).clamp(0.0, 1.0);
    (1.0 - (1.0 - t).powi(3)) as f32
}

fn text_layout(cr: &Context, text: &str, points: f64) -> pango::Layout {
    let layout = text::new_layout(cr);
    layout.set_font_description(Some(&text::font(&points.to_string())));
    layout.set_text(text);
    layout
}

/// What a page's place shows, to be drawn: an application's icon and name, or a folder's
/// first applications in a square, with its name.
struct Tile {
    name: String,
    icon: Option<ImageSurface>,
    minis: Vec<Option<ImageSurface>>,
}

impl Nekoland {
    /// Opens the overview on its applications, with `query` typed already; a second time,
    /// with nothing typed, closes it again.
    pub fn show_apps(&mut self, query: &str) {
        if let Some(overview) = &self.overview {
            if overview.apps.is_some() && query.is_empty() {
                self.close_overview(None);
                return;
            }
        } else {
            self.toggle_overview();
        }
        if let Some(overview) = self.overview.as_mut() {
            match overview.apps.as_mut() {
                Some(apps) => {
                    apps.query.push_str(query);
                    apps.page = 0;
                    apps.open = None;
                }
                None => overview.apps = Some(AppGrid::new(query, self.config.apps_view)),
            }
        }
    }

    /// How the applications are shown, while they are.
    pub fn apps_style(&self) -> Option<AppsView> {
        Some(self.overview.as_ref()?.apps.as_ref()?.style)
    }

    pub fn apps_shown(&self) -> bool {
        self.overview
            .as_ref()
            .is_some_and(|overview| overview.apps.is_some())
    }

    fn apps_layout(&self) -> Option<Layout> {
        let output = self.space.outputs().next()?;
        let area = self.space.output_geometry(output)?;
        let style = self.overview.as_ref()?.apps.as_ref()?.style;
        if style.is_list() {
            return Some(list::layout(
                style,
                area,
                crate::dock::dock_space(self.config.dock_size),
            ));
        }
        // Launchpad has the whole output; in the overview, the desktops' bar is above.
        let top = match style {
            AppsView::Launchpad => area.loc.y + 56,
            _ => area.loc.y + expanded_height(area) + 24,
        };
        let search = Rectangle::new(
            (area.loc.x + (area.size.w - SEARCH_WIDTH) / 2, top).into(),
            (SEARCH_WIDTH, FIELD_HEIGHT).into(),
        );
        let grid_top = top + FIELD_HEIGHT + 32;
        let bottom = area.loc.y + area.size.h - crate::dock::dock_space(self.config.dock_size) - DOTS - 8;
        let columns = ((area.size.w - 160) / CELL_WIDTH).clamp(4, 8) as usize;
        let rows = ((bottom - grid_top) / CELL_HEIGHT).clamp(2, 5) as usize;
        let size = Size::from((columns as i32 * CELL_WIDTH, rows as i32 * CELL_HEIGHT));
        let grid = Rectangle::new((area.loc.x + (area.size.w - size.w) / 2, grid_top).into(), size);
        Some(Layout {
            search,
            grid,
            columns,
            rows,
            sidebar: None,
        })
    }

    /// An application's icon at a size, or a folder's square of its first ones.
    fn grid_tile(&mut self, entry: Entry, size: i32) -> Option<Tile> {
        let apps = self.overview.as_ref()?.apps.as_ref()?;
        let (name, icons, minis): (String, Vec<String>, Vec<Vec<String>>) = match entry {
            Entry::App(app) => (
                apps.apps[app].name.clone(),
                apps.apps[app].icons.clone(),
                Vec::new(),
            ),
            Entry::Folder(folder) => {
                let folder = &apps.folders[folder];
                (
                    folder.name.clone(),
                    Vec::new(),
                    folder
                        .apps
                        .iter()
                        .take(9)
                        .map(|&app| apps.apps[app].icons.clone())
                        .collect(),
                )
            }
        };
        let mini_size = (size - 14) / 3 - 2;
        Some(Tile {
            name,
            icon: icons.iter().find_map(|name| self.dock_icon(name, size, true)),
            minis: minis
                .iter()
                .map(|names| {
                    names
                        .iter()
                        .find_map(|name| self.dock_icon(name, mini_size, true))
                })
                .collect(),
        })
    }

    /// Brings the view up to date before a frame: where a dragged icon is aimed, the page
    /// sliding into place, and what is drawn again when it changed.
    pub fn refresh_apps(&mut self) {
        let layout = self.apps_layout();
        let animated = self.config.animations;
        let pointer = self.seat.get_pointer().map(|pointer| pointer.current_location());
        let output_size = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|area| area.size);
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        let Some(layout) = layout else {
            return;
        };
        apps.layout = Some(layout);
        let now = Instant::now();
        let dt = now.duration_since(apps.last_tick).as_secs_f64().min(0.1);
        apps.last_tick = now;
        apps.refresh_chrome(&layout, output_size);
        if apps.style.is_list() {
            self.refresh_list(&layout, pointer);
            return;
        }

        // A dragged icon resting at the grid's side turns the page that way.
        let mut pages = apps.pages(&layout);
        if let (Some(_), Some(pos), None) = (&apps.held, pointer, &apps.open) {
            let grid = layout.grid.to_f64();
            let side = if pos.x < grid.loc.x + EDGE {
                Some(false)
            } else if pos.x > grid.loc.x + grid.size.w - EDGE {
                Some(true)
            } else {
                None
            };
            apps.edge = match (side, apps.edge) {
                (Some(side), Some((was, since))) if side == was => {
                    if since.elapsed().as_secs_f64() >= EDGE_TIME {
                        apps.page = if side {
                            (apps.page + 1).min(pages)
                        } else {
                            apps.page.saturating_sub(1)
                        };
                        Some((side, now))
                    } else {
                        Some((was, since))
                    }
                }
                (Some(side), _) => Some((side, now)),
                (None, _) => None,
            };
        }
        pages = pages.max(apps.page + 1);
        apps.page = apps.page.min(pages - 1);
        let target = apps.page as f64;
        apps.shown += (target - apps.shown) * (1.0 - (-dt / (PAGE_TIME / 3.0)).exp());
        if (apps.shown - target).abs() < 0.002 {
            apps.shown = target;
        }
        if apps.press.is_some() && apps.pressed.elapsed().as_secs_f64() >= HOLD_TIME && apps.query.is_empty() {
            apps.editing.get_or_insert(now);
        }
        let cells = apps.cells();
        let first = apps.page * layout.per_page();
        apps.hovered = pointer
            .filter(|_| apps.held.is_none() && apps.open.is_none() && apps.shown == target)
            .and_then(|pos| layout.cell_at(pos))
            .map(|(cell, _)| first + cell)
            .filter(|&index| index < cells.len());
        apps.lifts.step(apps.hovered, dt, animated);

        let dimmed = apps.open.is_some();
        let key = (
            apps.query.clone(),
            (apps.shown * 1000.0).round() as i64,
            apps.lifts.drawn(),
            cells.clone(),
            apps.target,
            dimmed,
            apps.shake(),
        );
        let held = apps.held.as_ref().map(|held| held.entry);
        let open_key = self.refresh_folder(&layout, pointer, dt);
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        let grid_stale = apps.drawn.as_ref() != Some(&key) || apps.picture.is_none();
        let held_stale = held.is_some() != apps.held_picture.is_some();
        if grid_stale {
            apps.drawn = Some(key);
            let shown = apps.shown;
            let lifts = apps.lifts.clone();
            let aimed = match apps.target {
                // Among the places shown, the room kept before it counts too.
                Target::Onto(at) => Some(match apps.gap {
                    Some(gap) if at >= gap => at + 1,
                    _ => at,
                }),
                _ => None,
            };
            let (low, high) = (shown.floor() as usize, shown.ceil() as usize);
            let per_page = layout.per_page();
            let mut places: Vec<(usize, usize, Cell)> = Vec::new();
            for page in [low, high].into_iter().collect::<std::collections::BTreeSet<_>>() {
                for (slot, cell) in cells.iter().skip(page * per_page).take(per_page).enumerate() {
                    places.push((page, slot, *cell));
                }
            }
            let tiles: Vec<Option<Tile>> = places
                .iter()
                .map(|(.., cell)| match cell {
                    Cell::Entry(entry) => self.grid_tile(*entry, ICON),
                    Cell::Gap => None,
                })
                .collect();
            let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
                return;
            };
            let pages = pages.max(cells.len().div_ceil(per_page));
            let shake = apps.shake();
            apps.picture = draw_grid(&layout, shown, &places, &tiles, &lifts, aimed, pages, shake);
        }
        if held_stale {
            let tile = held.and_then(|entry| self.grid_tile(entry, HELD_SIZE));
            let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
                return;
            };
            apps.held_picture = tile.and_then(|tile| draw_held(&tile));
        }
        let _ = open_key;
    }

    /// Draws the open folder again when what it shows changed.
    fn refresh_folder(&mut self, layout: &Layout, pointer: Option<Point<f64, Logical>>, dt: f64) -> Option<()> {
        let animated = self.config.animations;
        let apps = self.overview.as_ref()?.apps.as_ref()?;
        let FolderPlace {
            panel,
            grid,
            columns,
            rows,
        } = apps.folder_layout(layout)?;
        let open = apps.open.as_ref()?;
        let folder = &apps.folders[open.folder];
        // While an application of it is dragged, the others make room where it would go.
        let mut shown: Vec<usize> = folder.apps.clone();
        let mut gap = None;
        if let Some(held) = &apps.held {
            if let (Entry::App(app), Source::Folder(_)) = (held.entry, held.from) {
                shown.retain(|&other| other != app);
                if let Target::InFolder(at) = apps.target {
                    gap = Some(at.min(shown.len()));
                }
            }
        }
        let hovered = pointer
            .filter(|_| apps.held.is_none())
            .and_then(|pos| cell_at(grid, columns, rows, pos))
            .map(|(cell, _)| cell)
            .filter(|&cell| cell < shown.len());
        let shake = apps.shake();
        let name = folder.name.clone();
        let renaming = open.renaming;
        let open = self.overview.as_mut()?.apps.as_mut()?.open.as_mut()?;
        open.hovered = hovered;
        open.lifts.step(hovered, dt, animated);
        let lifts = open.lifts.clone();
        let key = (name.clone(), shown.clone(), gap, renaming, lifts.drawn(), shake);
        if open.drawn.as_ref() == Some(&key) && open.picture.is_some() {
            return Some(());
        }
        let tiles: Vec<Option<Tile>> = shown
            .iter()
            .map(|&app| self.grid_tile(Entry::App(app), ICON))
            .collect();
        let open = self.overview.as_mut()?.apps.as_mut()?.open.as_mut()?;
        open.drawn = Some(key);
        open.picture = draw_folder(
            panel.size,
            grid.loc - panel.loc,
            columns,
            &name,
            renaming,
            &tiles,
            gap,
            &lifts,
            shake,
        );
        Some(())
    }

    /// What the view puts on screen in front of the desktops' bar, front first, and the
    /// glass that goes behind everything.
    pub fn apps_overlays(&self) -> (Vec<Overlay<'_>>, Option<Overlay<'_>>) {
        let Some(apps) = self.overview.as_ref().and_then(|overview| overview.apps.as_ref()) else {
            return (Vec::new(), None);
        };
        let Some(layout) = apps.layout else {
            return (Vec::new(), None);
        };
        // Out as it came in: with the overview it is part of, as that closes.
        let leaving = (self.overview.as_ref())
            .filter(|overview| overview.closing)
            .map_or(1.0, |overview| overview.progress() as f32);
        let fade = apps.fade().min(leaving);
        let mut front = Vec::new();
        let pointer = self.seat.get_pointer().map(|pointer| pointer.current_location());
        if let (Some(held), Some(picture), Some(pos)) = (&apps.held, &apps.held_picture, pointer) {
            front.push(Overlay {
                fade: 1.0,
                id: &apps.ids[0],
                rect: Rectangle::new((pos - held.grab).to_i32_round(), picture.size),
                glass: None,
                alpha: 1.0,
                picture,
            });
        }
        if let (true, Some(picture)) = (apps.style.is_list(), &apps.list_picture) {
            front.push(Overlay {
                fade,
                id: &apps.ids[1],
                rect: Rectangle::new(layout.grid.loc, picture.size),
                glass: None,
                alpha: 1.0,
                picture,
            });
        }
        if let (Some(open), Some(FolderPlace { panel, .. })) = (&apps.open, apps.folder_layout(&layout)) {
            if let Some(picture) = &open.picture {
                front.push(Overlay {
                    fade: fade_since(open.opened).min(leaving),
                    id: &apps.ids[4],
                    rect: panel,
                    glass: Some((Tier::Menu, FOLDER_RADIUS)),
                    alpha: 1.0,
                    picture,
                });
            }
        }
        if let (false, Some(picture)) = (apps.style.is_list(), &apps.picture) {
            front.push(Overlay {
                fade,
                id: &apps.ids[1],
                rect: Rectangle::new(layout.grid.loc, picture.size),
                glass: None,
                alpha: if apps.open.is_some() { 0.3 } else { 1.0 },
                picture,
            });
        }
        if let Some((_, picture)) = &apps.search {
            front.push(Overlay {
                fade,
                id: &apps.ids[2],
                rect: layout.search,
                glass: None,
                alpha: 1.0,
                picture,
            });
        }
        // shortcut: under the search field even while a folder is renamed, near enough there.
        if let Some((_, picture)) = &apps.candidates_picture {
            front.push(Overlay {
                fade: leaving,
                id: &apps.ids[6],
                rect: Rectangle::new(
                    (
                        layout.search.loc.x,
                        layout.search.loc.y + layout.search.size.h + 8,
                    )
                        .into(),
                    picture.size,
                ),
                glass: Some((Tier::Menu, CANDIDATES_RADIUS)),
                alpha: 1.0,
                picture,
            });
        }
        // Behind its search field.
        if let (Some(sidebar), Some((_, picture))) = (layout.sidebar, &apps.sidebar_picture) {
            front.push(Overlay {
                fade,
                id: &apps.ids[5],
                rect: sidebar,
                glass: Some((Tier::Menu, list::SIDEBAR_RADIUS)),
                alpha: 1.0,
                picture,
            });
        }
        let backdrop = apps.backdrop.as_ref().and_then(|(_, picture)| {
            let output = self.space.outputs().next()?;
            let area = self.space.output_geometry(output)?;
            Some(Overlay {
                fade,
                id: &apps.ids[3],
                rect: area,
                glass: Some((Tier::Menu, 0)),
                alpha: 1.0,
                picture,
            })
        });
        (front, backdrop)
    }

    /// A press in the view. In an open folder: an application, or its name to change it;
    /// outside it, the folder closes. On the grid: an entry, which may be started, opened
    /// or dragged; the search field, which opens the launcher, which takes an input
    /// method; the pages' dots; anywhere else leaves.
    pub fn apps_press(&mut self, pos: Point<f64, Logical>) {
        if self.apps_style().is_some_and(AppsView::is_list) {
            return self.list_press(pos);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        let Some(layout) = apps.layout else {
            return;
        };
        if let Some(FolderPlace {
            panel,
            grid,
            columns,
            rows,
        }) = apps.folder_layout(&layout)
        {
            let title = Rectangle::new(
                panel.loc,
                Size::from((panel.size.w, FOLDER_PADDING + FOLDER_TITLE)),
            );
            let count = apps
                .open
                .as_ref()
                .map_or(0, |open| apps.folders[open.folder].apps.len());
            if let Some(open) = apps.open.as_mut() {
                if title.to_f64().contains(pos) {
                    open.renaming = true;
                } else if let Some((cell, _)) =
                    cell_at(grid, columns, rows, pos).filter(|(cell, _)| *cell < count)
                {
                    open.renaming = false;
                    apps.press = Some((Pressed::FolderCell(cell), pos));
                    apps.pressed = Instant::now();
                } else if !panel.to_f64().contains(pos) {
                    apps.open = None;
                    apps.save();
                }
            }
            return;
        }
        if layout.search.to_f64().contains(pos) {
            let query = apps.query.clone();
            config::spawn(&format!(
                "{} {}{}",
                self.config.launcher,
                self.config.launcher_search,
                config::quote(&query)
            ));
            self.close_overview(None);
            return;
        }
        let cells = apps.cells().len();
        let index = layout
            .cell_at(pos)
            .map(|(cell, _)| apps.page * layout.per_page() + cell)
            .filter(|&index| index < cells);
        match index {
            Some(index) => {
                apps.press = Some((Pressed::Cell(index), pos));
                apps.pressed = Instant::now();
            }
            // Between the pages' dots: to that page.
            None if pos.y >= f64::from(layout.grid.loc.y + layout.grid.size.h) => {
                let pages = apps.pages(&layout);
                let center = f64::from(layout.grid.loc.x + layout.grid.size.w / 2);
                let first = center - f64::from(pages as i32 - 1) * 9.0;
                let page = ((pos.x - first) / 18.0).round();
                if page >= 0.0 && (page as usize) < pages && (pos.x - (first + page * 18.0)).abs() < 9.0 {
                    apps.page = page as usize;
                } else {
                    self.leave_apps();
                }
            }
            None => self.leave_apps(),
        }
    }

    /// A click beside the icons: arranging ends, or with nothing arranged the view closes.
    fn leave_apps(&mut self) {
        let editing = self
            .overview
            .as_mut()
            .and_then(|overview| overview.apps.as_mut())
            .and_then(|apps| apps.editing.take());
        if editing.is_none() {
            self.close_overview(None);
        }
    }

    /// A right click on the grid opens its menu. Returns whether it did.
    pub fn apps_menu(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(apps) = self.overview.as_ref().and_then(|overview| overview.apps.as_ref()) else {
            return false;
        };
        // Under the desktops' bar only, and not among what was typed for.
        let below_bar = apps.layout.is_some_and(|layout| pos.y >= f64::from(layout.search.loc.y));
        if apps.style.is_list() || !apps.query.is_empty() || !below_bar {
            return false;
        }
        let edit = if apps.editing.is_some() { "完成" } else { "編輯" };
        let command = |command| crate::menubar::Command::Apps(command);
        self.open_menu_at(
            vec![
                crate::menubar::Entry::item(edit, command(AppsCommand::Edit)),
                crate::menubar::Entry::item("還原預設", command(AppsCommand::Reset)),
            ],
            pos.to_i32_round(),
        );
        true
    }

    pub fn run_apps_command(&mut self, command: AppsCommand) {
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        match command {
            AppsCommand::Edit => {
                apps.editing = match apps.editing {
                    Some(_) => None,
                    None => Some(Instant::now()),
                }
            }
            AppsCommand::Reset => apps.reset(),
        }
    }

    /// While an application is dragged, the Dock makes room for it as for its own icons.
    fn offer_to_dock(&mut self, id: String) {
        self.dock.incoming = Some((id, self.dock.incoming.take().and_then(|(_, slot)| slot)));
    }

    /// An application let go over the Dock is kept there, where the Dock made room for it;
    /// one kept already moves there. Returns whether it was over the Dock.
    fn drop_on_dock(&mut self, id: String, pos: Point<f64, Logical>) -> bool {
        let slot = self.dock.incoming.take().and_then(|(_, slot)| slot);
        if !self.dock_covers(pos) {
            return false;
        }
        self.config.dock.retain(|kept| !kept.eq_ignore_ascii_case(&id));
        let at = slot.unwrap_or(self.config.dock.len()).min(self.config.dock.len());
        self.config.dock.insert(at, id);
        config::save_dock(&self.config.dock);
        true
    }

    /// The pointer moved: a press far enough becomes a drag, and a dragged icon is aimed
    /// where the pointer is.
    pub fn apps_motion(&mut self, pos: Point<f64, Logical>) {
        if self.apps_style().is_some_and(AppsView::is_list) {
            return self.list_motion(pos);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        let Some(layout) = apps.layout else {
            return;
        };
        if let Some((pressed, start)) = apps.press {
            if (pos.x - start.x).hypot(pos.y - start.y) > DRAG_THRESHOLD {
                let held = match pressed {
                    // Dragging rearranges the whole grid only; among matches it starts.
                    Pressed::Cell(index) => match apps.cells().get(index) {
                        Some(Cell::Entry(entry)) => Some((*entry, Source::Grid)),
                        _ => None,
                    },
                    Pressed::FolderCell(index) => apps.open.as_ref().and_then(|open| {
                        let app = *apps.folders[open.folder].apps.get(index)?;
                        Some((Entry::App(app), Source::Folder(open.folder)))
                    }),
                    // The long page's are handled by its own motion.
                    Pressed::App(_) => None,
                };
                if let Some((entry, from)) = held {
                    apps.gap = match from {
                        Source::Grid => apps.entries.iter().position(|other| *other == entry),
                        Source::Folder(_) => None,
                    };
                    let half = f64::from(HELD_SIZE) / 2.0;
                    apps.held = Some(Held {
                        entry,
                        from,
                        grab: Point::from((half, half)),
                    });
                }
                apps.press = None;
            }
        }
        let Some(held) = &apps.held else {
            return;
        };
        let (entry, from) = (held.entry, held.from);
        if let Entry::App(app) = entry {
            let id = apps.apps[app].id.clone();
            self.offer_to_dock(id);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        // Inside the open folder, it moves among the folder's; out of it, the folder closes
        // and it goes back to the grid.
        if let Some(FolderPlace {
            panel,
            grid,
            columns,
            rows,
        }) = apps.folder_layout(&layout)
        {
            if panel.to_f64().contains(pos) && matches!(from, Source::Folder(_)) {
                let count = apps
                    .open
                    .as_ref()
                    .map_or(0, |open| apps.folders[open.folder].apps.len());
                apps.target = cell_at(grid, columns, rows, pos).map_or(Target::Nowhere, |(cell, _)| {
                    Target::InFolder(cell.min(count.saturating_sub(1)))
                });
                return;
            }
            apps.open = None;
        }
        if !apps.query.is_empty() {
            apps.target = Target::Nowhere;
            return;
        }
        let remaining = apps.remaining().len();
        let Some((cell, distance)) = layout.cell_at(pos) else {
            apps.target = Target::Nowhere;
            return;
        };
        let shown = apps.page * layout.per_page() + cell;
        // Past the room kept, the places shown are one further than among the entries.
        let at = match apps.gap {
            Some(gap) if shown > gap => shown - 1,
            _ => shown,
        };
        apps.target =
            if apps.gap != Some(shown) && distance < ONTO && at < remaining && matches!(entry, Entry::App(_))
            {
                // The room stays where it was, so nothing moves under the pointer.
                Target::Onto(at)
            } else {
                let at = at.min(remaining);
                apps.gap = Some(at);
                Target::Between(at)
            };
    }

    /// A button came up: a press let go where it went down starts the application or
    /// opens the folder; a dragged one goes where it is aimed, or starts on a desktop it
    /// was dropped on, or is kept in the Dock.
    pub fn apps_release(&mut self, pos: Point<f64, Logical>) {
        if self.apps_style().is_some_and(AppsView::is_list) {
            return self.list_release(pos);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        if let Some((pressed, _)) = apps.press.take() {
            let editing = apps.editing.is_some();
            match pressed {
                Pressed::Cell(index) => {
                    let here = apps.layout.and_then(|layout| {
                        layout
                            .cell_at(pos)
                            .map(|(cell, _)| apps.page * layout.per_page() + cell)
                    });
                    match apps.cells().get(index).copied().filter(|_| here == Some(index)) {
                        Some(Cell::Entry(Entry::App(_))) if editing => {}
                        Some(Cell::Entry(Entry::App(app))) => {
                            let icon = apps.layout.map(|layout| {
                                icon_rect(layout.grid, layout.columns, index % layout.per_page().max(1))
                            });
                            self.start_grid_app(app, None, icon);
                        }
                        Some(Cell::Entry(Entry::Folder(folder))) => {
                            apps.open = Some(OpenFolder {
                                folder,
                                renaming: false,
                                opened: Instant::now(),
                                hovered: None,
                                lifts: Lifts::default(),
                                picture: None,
                                drawn: None,
                            });
                        }
                        _ => {}
                    }
                }
                Pressed::FolderCell(index) => {
                    let app = apps
                        .open
                        .as_ref()
                        .and_then(|open| apps.folders[open.folder].apps.get(index).copied());
                    let icon = apps
                        .layout
                        .and_then(|layout| apps.folder_layout(&layout))
                        .map(|place| icon_rect(place.grid, place.columns, index));
                    if let Some(app) = app.filter(|_| !editing) {
                        self.start_grid_app(app, None, icon);
                    }
                }
                Pressed::App(app) => self.start_grid_app(app, None, Some(icon_around(pos))),
            }
            return;
        }
        let Some(held) = apps.held.take() else {
            return;
        };
        apps.edge = None;
        apps.gap = None;
        if let Entry::App(app) = held.entry {
            let id = apps.apps[app].id.clone();
            if self.drop_on_dock(id, pos) {
                return;
            }
            match self.overview_bar_action(pos) {
                Some(BarAction::Switch(desktop)) => return self.start_grid_app(app, Some(desktop), None),
                Some(BarAction::Add) => {
                    let desktop = self.workspaces.count();
                    self.add_workspace();
                    return self.start_grid_app(app, Some(desktop), None);
                }
                None => {}
            }
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return;
        };
        apps.drop_held(held);
        apps.target = Target::Nowhere;
        apps.gap = None;
    }

    /// Starts an application of the grid, on `desktop` if one is given, and leaves. Its
    /// window opens out of `icon`, where it was started by a click on one.
    fn start_grid_app(&mut self, index: usize, desktop: Option<usize>, icon: Option<Rectangle<i32, Logical>>) {
        let Some(apps) = self.overview.as_ref().and_then(|overview| overview.apps.as_ref()) else {
            return;
        };
        let Some(app) = apps.apps.get(index) else {
            return;
        };
        let (info, id) = (app.info.clone(), app.id.clone());
        if let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) {
            apps.recent.retain(|recent| *recent != id);
            apps.recent.insert(0, id.clone());
            apps.recent.truncate(list::RECENT);
            apps.save();
        }
        if let Some(desktop) = desktop {
            self.switch_workspace(desktop);
        }
        if let Err(err) = info.launch(&[], None::<&gio::AppLaunchContext>) {
            tracing::warn!("{id}: {err}");
        }
        if let Some(icon) = icon {
            let class = info.startup_wm_class().map(|class| class.to_string());
            self.motion_launching(std::iter::once(id.clone()).chain(class).collect(), icon);
        }
        self.dock_launching(&id);
        self.close_overview(None);
    }

    /// How many times its size the overlay named `id` is drawn, if it is the applications
    /// on their way in or out: they come in from a little larger and leave the same way.
    pub fn apps_zoom_of(&self, id: &Id) -> Option<f64> {
        let overview = self.overview.as_ref().filter(|_| self.config.animations)?;
        let apps = overview.apps.as_ref().filter(|apps| &apps.ids[1] == id)?;
        let leaving = overview.closing.then(|| overview.progress());
        Some(crate::motion::apps_zoom(apps.opened, leaving)).filter(|zoom| (zoom - 1.0).abs() > ZOOM_AT_REST)
    }

    /// A key in the view: typing narrows it down, or names the folder being renamed;
    /// Return starts the first match; the arrows turn the pages; Escape stops renaming,
    /// closes the folder, clears what was typed, or leaves. Returns whether it was taken.
    pub fn apps_key(&mut self, sym: smithay::input::keyboard::Keysym, typed: Option<char>) -> bool {
        use smithay::input::keyboard::Keysym;
        if self.apps_style().is_some_and(AppsView::is_list) {
            return self.list_key(sym, typed);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return false;
        };
        if let Some(open) = apps.open.as_mut() {
            let folder = open.folder;
            if open.renaming {
                let name = &mut apps.folders[folder].name;
                match (sym, typed) {
                    (Keysym::Return | Keysym::KP_Enter | Keysym::Escape, _) => {
                        open.renaming = false;
                        apps.save();
                    }
                    (Keysym::BackSpace, _) => {
                        name.pop();
                    }
                    (_, Some(character)) if !character.is_control() => name.push(character),
                    _ => return false,
                }
                return true;
            }
            if sym == Keysym::Escape {
                apps.open = None;
                return true;
            }
            apps.open = None;
        }
        let pages = apps.layout.map_or(1, |layout| apps.pages(&layout));
        match sym {
            Keysym::Escape if apps.editing.is_some() => apps.editing = None,
            Keysym::Escape if !apps.query.is_empty() => {
                apps.query.clear();
                apps.page = 0;
            }
            Keysym::Escape => self.close_overview(None),
            Keysym::BackSpace => {
                apps.query.pop();
                apps.page = 0;
            }
            Keysym::Return | Keysym::KP_Enter => {
                let cells = apps.cells();
                let first = apps.hovered.unwrap_or(0);
                match cells.get(first) {
                    Some(Cell::Entry(Entry::App(app))) => {
                        let app = *app;
                        self.start_grid_app(app, None, None);
                    }
                    _ => return false,
                }
            }
            Keysym::Left | Keysym::Page_Up => apps.page = apps.page.saturating_sub(1),
            Keysym::Right | Keysym::Page_Down => apps.page = (apps.page + 1).min(pages - 1),
            _ => match typed {
                Some(character) if !character.is_control() => {
                    apps.query.push(character);
                    apps.page = 0;
                }
                _ => return false,
            },
        }
        true
    }

    /// The wheel turns the pages. Returns whether it was taken.
    pub fn apps_scroll(&mut self, amount: f64) -> bool {
        if self.apps_style().is_some_and(AppsView::is_list) {
            return self.list_scroll(amount);
        }
        let Some(apps) = self.overview.as_mut().and_then(|overview| overview.apps.as_mut()) else {
            return false;
        };
        if apps.open.is_some() {
            return true;
        }
        let pages = apps.layout.map_or(1, |layout| apps.pages(&layout));
        apps.scrolled += amount;
        if apps.scrolled.abs() >= PAGE_SCROLL {
            apps.page = if apps.scrolled > 0.0 {
                (apps.page + 1).min(pages - 1)
            } else {
                apps.page.saturating_sub(1)
            };
            apps.scrolled = 0.0;
        }
        true
    }
}

/// The search field, the design language's, with what was typed and what is being
/// composed, which is underlined, and the caret after them; or what the field is for. The
/// overview is dark, and so is its field.
fn draw_search(query: &str, preedit: &str, size: Size<i32, Logical>) -> Option<Picture> {
    let surface = text::canvas(size.w, size.h).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        let colors = colors(true);
        draw_field(&cr, (0.0, 0.0, f64::from(size.w), f64::from(size.h)), &colors);
        cr.set_source_rgb(colors.ink_3.0, colors.ink_3.1, colors.ink_3.2);
        cr.set_line_width(1.6);
        let (cx, cy) = (22.0, f64::from(size.h) / 2.0 - 1.0);
        cr.arc(cx, cy, 6.0, 0.0, std::f64::consts::TAU);
        cr.stroke().ok()?;
        cr.move_to(cx + 4.5, cy + 4.5);
        cr.line_to(cx + 9.0, cy + 9.0);
        cr.stroke().ok()?;
        let empty = query.is_empty() && preedit.is_empty();
        let (text, color) = if empty {
            ("搜尋應用程式".to_string(), colors.ink_3)
        } else {
            (format!("{query}{preedit}"), colors.ink)
        };
        let line = text_layout(&cr, &text, 11.0);
        let (width, height) = line.pixel_size();
        let top = f64::from(size.h - height) / 2.0;
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.move_to(40.0, top);
        pangocairo::functions::show_layout(&cr, &line);
        if !preedit.is_empty() {
            let start = f64::from(text_layout(&cr, query, 11.0).pixel_size().0);
            cr.rectangle(
                40.0 + start,
                top + f64::from(height),
                f64::from(width) - start,
                1.0,
            );
            cr.fill().ok()?;
        }
        if !empty {
            cr.rectangle(41.0 + f64::from(width), f64::from(size.h) / 2.0 - 9.0, 1.5, 18.0);
            cr.fill().ok()?;
        }
    }
    text::picture(surface)
}

const CANDIDATES_RADIUS: i32 = 10;

/// The input method's candidates in a row, each after the number that picks it, the lit
/// one on solid ink; then its note, such as the page.
pub(crate) fn draw_candidates((candidates, lit, note): &crate::ime::Candidates) -> Option<Picture> {
    const PAD: f64 = 6.0;
    const HEIGHT: i32 = 36;
    let measure = ImageSurface::create(Format::ARgb32, 1, 1).ok()?;
    let labels: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(index, text)| format!("{} {text}", (index + 1) % 10))
        .collect();
    let widths: Vec<f64> = {
        let cr = Context::new(&measure).ok()?;
        labels
            .iter()
            .map(|label| f64::from(text_layout(&cr, label, 11.0).pixel_size().0) + 20.0)
            .chain(std::iter::once(if note.is_empty() {
                0.0
            } else {
                f64::from(text_layout(&cr, note, 9.5).pixel_size().0) + 20.0
            }))
            .collect()
    };
    let width = (widths.iter().sum::<f64>() + PAD * 2.0).ceil() as i32;
    let surface = text::canvas(width, HEIGHT).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        paint_pane(
            &cr,
            f64::from(width),
            f64::from(HEIGHT),
            f64::from(CANDIDATES_RADIUS),
            true,
            Tier::Menu,
        );
        let mut x = PAD;
        for (index, label) in labels.iter().enumerate() {
            let cell = widths[index];
            let on = *lit == Some(index);
            if on {
                text::rounded_rectangle(&cr, x, PAD, cell, f64::from(HEIGHT) - PAD * 2.0, 6.0);
                cr.set_source_rgba(0.945, 0.937, 0.918, 1.0);
                cr.fill().ok()?;
            }
            let line = text_layout(&cr, label, 11.0);
            let (_, height) = line.pixel_size();
            if on {
                cr.set_source_rgba(0.09, 0.09, 0.1, 1.0);
            } else {
                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            }
            cr.move_to(x + 10.0, f64::from(HEIGHT - height) / 2.0);
            pangocairo::functions::show_layout(&cr, &line);
            x += cell;
        }
        if !note.is_empty() {
            let line = text_layout(&cr, note, 9.5);
            let (_, height) = line.pixel_size();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.6);
            cr.move_to(x + 10.0, f64::from(HEIGHT - height) / 2.0);
            pangocairo::functions::show_layout(&cr, &line);
        }
    }
    text::picture(surface)
}

/// A folder's square: translucent, its first applications in three rows of three.
fn paint_folder(cr: &Context, x: f64, y: f64, size: f64, minis: &[Option<ImageSurface>]) {
    text::rounded_rectangle(cr, x, y, size, size, size * 0.24);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.2);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.3);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
    let inset = 7.0;
    let step = (size - inset * 2.0) / 3.0;
    for (index, mini) in minis.iter().enumerate().take(9) {
        let Some(mini) = mini else {
            continue;
        };
        let (row, column) = ((index / 3) as f64, (index % 3) as f64);
        let _ = cr.save();
        cr.translate(x + inset + column * step + 1.0, y + inset + row * step + 1.0);
        crate::stacks::paint_fitted(cr, mini, step - 2.0, 1.0);
        let _ = cr.restore();
    }
}

/// An application's icon or a folder's square at the top of a place, its name under it; the
/// icon `lift` times its size, a little bigger while the pointer is on it.
fn paint_tile(cr: &Context, x: f64, y: f64, tile: &Tile, lift: f64) -> Option<()> {
    paint_tile_in(cr, x, y, tile, CELL_WIDTH, ICON, lift)
}

/// The same, turned about its icon's middle as far as the place at `index` has shaken:
/// each place a little out of step with the next, so that they do not sway as one.
fn paint_shaken(cr: &Context, x: f64, y: f64, tile: &Tile, index: usize, lift: f64, shake: Option<u128>) -> Option<()> {
    let Some(step) = shake else {
        return paint_tile(cr, x, y, tile, lift);
    };
    let seconds = step as f64 * 0.008;
    let angle = SHAKE * (seconds * SHAKE_RATE * std::f64::consts::TAU + index as f64 * 2.4).sin();
    let (middle_x, middle_y) = (x + f64::from(CELL_WIDTH) / 2.0, y + 12.0 + f64::from(ICON) / 2.0);
    cr.save().ok()?;
    cr.translate(middle_x, middle_y);
    cr.rotate(angle);
    cr.translate(-middle_x, -middle_y);
    let painted = paint_tile(cr, x, y, tile, lift);
    cr.restore().ok()?;
    painted
}

/// The same in a place `width` wide, with icons of `icon` grown `lift` times about their
/// middle.
fn paint_tile_in(
    cr: &Context,
    x: f64,
    y: f64,
    tile: &Tile,
    width: i32,
    icon_size: i32,
    lift: f64,
) -> Option<()> {
    let size = f64::from(icon_size) * lift;
    let grown = (size - f64::from(icon_size)) / 2.0;
    let left = x + f64::from(width - icon_size) / 2.0 - grown;
    let top = y + 12.0 - grown;
    match &tile.icon {
        Some(icon) => {
            cr.save().ok()?;
            cr.translate(left, top);
            crate::stacks::paint_fitted(cr, icon, size, 1.0);
            cr.restore().ok()?;
        }
        None if !tile.minis.is_empty() => paint_folder(cr, left, top, size, &tile.minis),
        None => {}
    }
    let label = text_layout(cr, &tile.name, if icon_size < ICON { 9.0 } else { 10.0 });
    label.set_width((width - 14) * pango::SCALE);
    label.set_ellipsize(pango::EllipsizeMode::End);
    let (text_width, _) = label.pixel_size();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(
        x + f64::from(width - text_width.min(width - 14)) / 2.0,
        y + f64::from(icon_size) + 20.0,
    );
    pangocairo::functions::show_layout(cr, &label);
    Some(())
}

/// The grid: each page's places, the pointed-at one's icon grown and the one a dragged icon is
/// aimed onto shaded, sliding as the pages turn; and the dots under it.
#[allow(clippy::too_many_arguments)]
fn draw_grid(
    layout: &Layout,
    shown: f64,
    places: &[(usize, usize, Cell)],
    tiles: &[Option<Tile>],
    lifts: &Lifts,
    aimed: Option<usize>,
    pages: usize,
    shake: Option<u128>,
) -> Option<Picture> {
    let (width, height) = (layout.grid.size.w, layout.grid.size.h + DOTS);
    let per_page = layout.per_page();
    let surface = text::canvas(width, height).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(layout.grid.size.h));
        cr.clip();
        for ((page, slot, _), tile) in places.iter().zip(tiles) {
            let (row, column) = (slot / layout.columns, slot % layout.columns);
            let x = (*page as f64 - shown) * f64::from(width) + f64::from(column as i32 * CELL_WIDTH);
            let y = f64::from(row as i32 * CELL_HEIGHT);
            let index = page * per_page + slot;
            if aimed == Some(index) {
                text::rounded_rectangle(
                    &cr,
                    x + 4.0,
                    y + 2.0,
                    f64::from(CELL_WIDTH) - 8.0,
                    f64::from(CELL_HEIGHT) - 6.0,
                    14.0,
                );
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
                cr.fill().ok()?;
            }
            if let Some(tile) = tile {
                paint_shaken(&cr, x, y, tile, index, lifts.of(index), shake)?;
            }
        }
        cr.reset_clip();
        if pages > 1 {
            let center = f64::from(width) / 2.0;
            let first = center - f64::from(pages as i32 - 1) * 9.0;
            for page in 0..pages {
                let lit = (page as f64 - shown).abs() < 0.5;
                cr.arc(
                    first + page as f64 * 18.0,
                    f64::from(layout.grid.size.h) + 20.0,
                    3.5,
                    0.0,
                    std::f64::consts::TAU,
                );
                cr.set_source_rgba(1.0, 1.0, 1.0, if lit { 1.0 } else { 0.35 });
                cr.fill().ok()?;
            }
        }
    }
    text::picture(surface)
}

/// The open folder on its pane: its name, a field while it is changed, then its
/// applications, room kept where a dragged one would go.
#[allow(clippy::too_many_arguments)]
fn draw_folder(
    size: Size<i32, Logical>,
    grid: Point<i32, Logical>,
    columns: usize,
    name: &str,
    renaming: bool,
    tiles: &[Option<Tile>],
    gap: Option<usize>,
    lifts: &Lifts,
    shake: Option<u128>,
) -> Option<Picture> {
    let surface = text::canvas(size.w, size.h).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        paint_pane(
            &cr,
            f64::from(size.w),
            f64::from(size.h),
            f64::from(FOLDER_RADIUS),
            true,
            Tier::Menu,
        );
        let title = text_layout(&cr, name, 14.0);
        title.set_font_description(Some(&text::font("Bold 14")));
        let (width, height) = title.pixel_size();
        let (x, y) = (
            f64::from(size.w - width) / 2.0,
            f64::from(FOLDER_PADDING) + f64::from(FOLDER_TITLE - height) / 2.0 - 6.0,
        );
        if renaming {
            // The design language's field around the name, dark as the overview is.
            let colors = colors(true);
            let field = f64::from(FIELD_HEIGHT);
            let top = y + f64::from(height) / 2.0 - field / 2.0;
            draw_field(&cr, (x - 12.0, top, f64::from(width) + 24.0, field), &colors);
            cr.set_source_rgb(colors.ink.0, colors.ink.1, colors.ink.2);
            cr.rectangle(x + f64::from(width) + 2.0, y, 1.5, f64::from(height));
            cr.fill().ok()?;
        }
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to(x, y);
        pangocairo::functions::show_layout(&cr, &title);
        let mut place = 0;
        for (index, tile) in tiles.iter().enumerate() {
            if gap == Some(index) {
                place += 1;
            }
            let (row, column) = (place / columns, place % columns);
            let (x, y) = (
                f64::from(grid.x + column as i32 * CELL_WIDTH),
                f64::from(grid.y + row as i32 * CELL_HEIGHT),
            );
            if let Some(tile) = tile {
                paint_shaken(&cr, x, y, tile, index, lifts.of(index), shake)?;
            }
            place += 1;
        }
    }
    text::picture(surface)
}

/// The dragged entry's icon, a little bigger, as it follows the pointer.
fn draw_held(tile: &Tile) -> Option<Picture> {
    let surface = text::canvas(HELD_SIZE, HELD_SIZE).ok()?;
    {
        let cr = Context::new(&surface).ok()?;
        match &tile.icon {
            Some(icon) => crate::stacks::paint_fitted(&cr, icon, f64::from(HELD_SIZE), 0.92),
            None => paint_folder(&cr, 0.0, 0.0, f64::from(HELD_SIZE), &tile.minis),
        }
    }
    text::picture(surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_grows_under_the_pointer_and_settles_back_when_it_leaves() {
        let frame = 1.0 / 60.0;
        let mut lifts = Lifts::default();
        lifts.step(Some(3), frame, true);
        let first = lifts.of(3);
        assert!(first > 1.0 && first < HOVER_LIFT, "one frame is part of the way: {first}");
        for _ in 0..60 {
            lifts.step(Some(3), frame, true);
        }
        assert!((lifts.of(3) - HOVER_LIFT).abs() < 0.001);
        // On to the next: the one left comes down as the other goes up.
        lifts.step(Some(4), frame, true);
        assert!(lifts.of(3) < HOVER_LIFT && lifts.of(3) > 1.0);
        assert!(lifts.of(4) > 1.0);
        for _ in 0..120 {
            lifts.step(None, frame, true);
        }
        assert_eq!(lifts, Lifts::default());
        assert_eq!(lifts.of(3), 1.0);
    }

    #[test]
    fn with_nothing_animated_an_icon_is_at_its_size_at_once() {
        let mut lifts = Lifts::default();
        lifts.step(Some(1), 1.0 / 60.0, false);
        assert_eq!(lifts.of(1), HOVER_LIFT);
        lifts.step(None, 1.0 / 60.0, false);
        assert_eq!(lifts, Lifts::default());
    }

    #[test]
    fn a_grid_finds_the_cell_and_how_near_its_icon() {
        let grid = Rectangle::new((100, 200).into(), (CELL_WIDTH * 4, CELL_HEIGHT * 2).into());
        let (cell, _) = cell_at(grid, 4, 2, Point::from((101.0, 201.0))).unwrap();
        assert_eq!(cell, 0);
        let middle = Point::from((
            100.0 + f64::from(CELL_WIDTH) * 2.5,
            200.0 + f64::from(CELL_HEIGHT) + 12.0 + f64::from(ICON) / 2.0,
        ));
        assert_eq!(cell_at(grid, 4, 2, middle), Some((6, 0.0)));
        assert_eq!(cell_at(grid, 4, 2, Point::from((99.0, 201.0))), None);
        assert_eq!(
            cell_at(
                grid,
                4,
                2,
                Point::from((101.0, 200.0 + f64::from(CELL_HEIGHT) * 2.0 + 1.0))
            ),
            None
        );
    }

    #[test]
    fn typing_finds_names_that_start_with_it_first() {
        let words = [
            ("Text Editor", "text editor gedit"),
            ("Terminal", "terminal foot"),
            ("Files", "files terminal"),
        ];
        assert_eq!(matching(&words, "ter"), [1, 2]);
        assert_eq!(matching(&words, ""), [0, 1, 2]);
    }

    #[test]
    fn an_arrangement_keeps_its_order_and_puts_new_applications_last() {
        let kept: Arrangement = toml::from_str(
            r#"
            order = ["b", "folder:f", "gone"]
            [[folders]]
            id = "f"
            name = "工具程式"
            apps = ["c", "a"]
            "#,
        )
        .unwrap();
        let ids: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let (entries, folders) = arrange(&ids, &kept);
        // "gone" is no longer installed; "d" was installed since and comes last.
        assert_eq!(entries, [Entry::App(1), Entry::Folder(0), Entry::App(3)]);
        assert_eq!(folders[0].apps, [2, 0]);
        assert_eq!(folders[0].name, "工具程式");
    }
}
