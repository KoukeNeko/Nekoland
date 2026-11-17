//! The file pointed at in a Nautilus window, for the hover panel, found as FinderHover finds
//! Finder's: through the accessibility tree, here AT-SPI's, where Nautilus names each file's
//! cell. Which folder the window shows comes from Nautilus's FileManager1 interface. Both
//! take round trips over D-Bus, so a thread of its own asks. Nekoland's own Files says
//! which file is at a point itself.

use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
};

use zbus::{blocking::Connection, zvariant::OwnedObjectPath, zvariant::OwnedValue};

use gio::prelude::*;

use crate::{
    config::Sort,
    stacks::{self, Item},
};

pub const APP_ID: &str = "org.gnome.Nautilus";
pub const FILES_APP_ID: &str = "io.github.koukeneko.Nekoland.Files";
const REGISTRY: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
// AT-SPI's roles and coordinate types, as numbers on the bus.
const ROLE_LIST: u32 = 31;
const ROLE_TABLE: u32 = 55;
const ROLE_TREE_TABLE: u32 = 66;
const COORDS_WINDOW: u32 = 1;
/// How far into a window's tree its file view is looked for, at most.
const MOST_NODES: usize = 4000;

/// A point in a Nautilus or Files window: its program, its process, its title and GTK's
/// name for it on the bus, and the point from the window's top left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub app_id: String,
    pub pid: u32,
    pub title: String,
    pub window: Option<String>,
    pub point: (i32, i32),
}

/// Where questions go; each is answered on the channel it comes with.
pub type Asker = Sender<(Question, Sender<Option<Answer>>)>;

/// The file pointed at, and its cell's left, top, width and height in the window.
pub type Answer = (Item, (i32, i32, i32, i32));

/// Starts the thread that answers.
pub fn start() -> Asker {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || serve(receiver));
    sender
}

/// An accessible object: the bus name of its program and its path.
type Node = (String, OwnedObjectPath);

fn serve(questions: Receiver<(Question, Sender<Option<Answer>>)>) {
    let mut asker = Asking::default();
    while let Ok(first) = questions.recv() {
        // Only the newest point matters: the pointer has left the others.
        let (question, answer) = questions.try_iter().last().unwrap_or(first);
        let _ = answer.send(asker.item(&question));
    }
}

#[derive(Default)]
struct Asking {
    session: Option<Connection>,
    a11y: Option<Connection>,
    /// The file view found in each window, not to look for it again.
    views: HashMap<Node, Node>,
}

impl Asking {
    fn item(&mut self, question: &Question) -> Option<Answer> {
        if self.session.is_none() {
            self.session = Connection::session()
                .inspect_err(|err| tracing::warn!("session bus: {err}"))
                .ok();
        }
        if question.app_id == FILES_APP_ID {
            return files_item(self.session.as_ref()?, question);
        }
        if self.a11y.is_none() {
            self.a11y = self.session.as_ref().and_then(a11y_bus);
        }
        let cell = self.cell(question);
        if cell.is_none() {
            // A connection that broke is made again for the next question.
            self.a11y = None;
        }
        let (name, extents) = cell?;
        let folder = folder(self.session.as_ref()?, question.window.as_deref())?;
        Some((file_named(&folder, &name)?, extents))
    }

    /// The name of the file's cell under the point, as `notes.md. File`, and where the cell is.
    fn cell(&mut self, question: &Question) -> Option<(String, (i32, i32, i32, i32))> {
        let bus = self.a11y.as_ref()?;
        let window = window_node(bus, question)?;
        let cached = self.views.get(&window).cloned();
        if let Some(found) = cached.and_then(|view| under(bus, &view, question.point)) {
            return Some(found);
        }
        // The view was switched, between grid and list, or the folder changed.
        let view = find_view(bus, &window)?;
        self.views.insert(window, view.clone());
        under(bus, &view, question.point)
    }
}

/// The file under a point in a view, by the name of its cell or row, and where that is: the
/// grid's cells are the view's, the list's rows are in a list beside its header.
fn under(bus: &Connection, view: &Node, (x, y): (i32, i32)) -> Option<(String, (i32, i32, i32, i32))> {
    let mut nodes = children(bus, view)?;
    if let Some(list) = nodes.iter().find(|node| role(bus, node) == Some(ROLE_LIST)) {
        nodes = children(bus, list)?;
    }
    let (node, area) = nodes.into_iter().find_map(|node| {
        let area = extents(bus, &node)?;
        let (left, top, width, height) = area;
        (x >= left && x < left + width && y >= top && y < top + height).then_some((node, area))
    })?;
    Some((name(bus, &node)?, area))
}

/// A connection to the bus accessibility goes over, which the session bus says where it is.
fn a11y_bus(session: &Connection) -> Option<Connection> {
    let reply = session
        .call_method(
            Some("org.a11y.Bus"),
            "/org/a11y/bus",
            Some("org.a11y.Bus"),
            "GetAddress",
            &(),
        )
        .inspect_err(|err| tracing::warn!("accessibility bus: {err}"))
        .ok()?;
    let (address,) = reply.body().deserialize::<(String,)>().ok()?;
    zbus::blocking::connection::Builder::address(address.as_str())
        .ok()?
        .build()
        .inspect_err(|err| tracing::warn!("accessibility bus at {address}: {err}"))
        .ok()
}

/// The window's object among its program's: by its title, else the program's only one.
fn window_node(bus: &Connection, question: &Question) -> Option<Node> {
    let root: Node = (REGISTRY.to_string(), OwnedObjectPath::try_from(ROOT).ok()?);
    let app = children(bus, &root)?.into_iter().find(|(name, _)| {
        bus.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetConnectionUnixProcessID",
            &(name.as_str(),),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<(u32,)>().ok())
            == Some((question.pid,))
    })?;
    let windows = children(bus, &app)?;
    match windows
        .iter()
        .find(|window| name(bus, window).as_deref() == Some(&question.title))
    {
        Some(window) => Some(window.clone()),
        None => (windows.len() == 1).then(|| windows[0].clone()),
    }
}

/// The view of files in a window, a table in the grid and a tree table in the list,
/// looked for breadth first.
fn find_view(bus: &Connection, window: &Node) -> Option<Node> {
    let mut queue = std::collections::VecDeque::from([window.clone()]);
    let mut seen = 0;
    while let Some(node) = queue.pop_front() {
        seen += 1;
        if seen > MOST_NODES {
            return None;
        }
        if matches!(role(bus, &node), Some(ROLE_TABLE | ROLE_TREE_TABLE)) {
            return Some(node);
        }
        queue.extend(children(bus, &node).unwrap_or_default());
    }
    None
}

fn children(bus: &Connection, (name, path): &Node) -> Option<Vec<Node>> {
    let reply = bus
        .call_method(Some(name.as_str()), path, Some(ACCESSIBLE), "GetChildren", &())
        .ok()?;
    reply.body().deserialize::<Vec<Node>>().ok()
}

fn role(bus: &Connection, (name, path): &Node) -> Option<u32> {
    let reply = bus
        .call_method(Some(name.as_str()), path, Some(ACCESSIBLE), "GetRole", &())
        .ok()?;
    reply.body().deserialize::<u32>().ok()
}

fn name(bus: &Connection, (name, path): &Node) -> Option<String> {
    let reply = bus
        .call_method(
            Some(name.as_str()),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(ACCESSIBLE, "Name"),
        )
        .ok()?;
    let (value,) = reply.body().deserialize::<(OwnedValue,)>().ok()?;
    String::try_from(value).ok().filter(|name| !name.is_empty())
}

/// Left, top, width and height, from the window's top left corner.
fn extents(bus: &Connection, (name, path): &Node) -> Option<(i32, i32, i32, i32)> {
    let reply = bus
        .call_method(
            Some(name.as_str()),
            path,
            Some(COMPONENT),
            "GetExtents",
            &(COORDS_WINDOW,),
        )
        .ok()?;
    reply.body().deserialize::<(i32, i32, i32, i32)>().ok()
}

/// The folder a Nautilus window shows: the one GTK named on the bus, else the only window's.
fn folder(session: &Connection, window: Option<&str>) -> Option<std::path::PathBuf> {
    let reply = session
        .call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.FileManager1", "OpenWindowsWithLocations"),
        )
        .ok()?;
    let (value,) = reply.body().deserialize::<(OwnedValue,)>().ok()?;
    let windows = HashMap::<String, Vec<String>>::try_from(value).ok()?;
    let locations = match window.and_then(|window| windows.get(window)) {
        Some(locations) => locations,
        None if windows.len() == 1 => windows.values().next()?,
        None => return None,
    };
    gio::File::for_uri(locations.first()?).path()
}

/// The file Files shows at the point, and its row or tile.
fn files_item(session: &Connection, question: &Question) -> Option<Answer> {
    let reply = session
        .call_method(
            Some(FILES_APP_ID),
            "/org/freedesktop/FileManager1",
            Some("io.github.koukeneko.Nekoland.Files.Hover"),
            "ItemAt",
            &(
                question.window.as_deref().unwrap_or(""),
                question.point.0,
                question.point.1,
            ),
        )
        .ok()?;
    let (path, left, top, width, height) = reply.body().deserialize::<(String, i32, i32, i32, i32)>().ok()?;
    let path = Path::new(&path);
    let item = file_named(path.parent()?, path.file_name()?.to_str()?)?;
    Some((item, (left, top, width, height)))
}

/// The file in `folder` a cell is for.
fn file_named(folder: &Path, cell: &str) -> Option<Item> {
    let items = stacks::read_folder(folder, Sort::Name);
    let name = longest_named(cell, items.iter().map(|item| item.name.as_str()))?.to_string();
    items.into_iter().find(|item| item.name == name)
}

/// The longest of `names` a cell's name starts with: Nautilus names a cell for its file,
/// then a dot and the file's kind, in the language of the desktop.
fn longest_named<'a>(cell: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    names
        .filter(|name| {
            cell.strip_prefix(name)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
        .max_by_key(|name| name.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_is_for_the_longest_file_name_it_starts_with() {
        let names = ["notes", "notes.md", "notes.md.bak", "logo.pn", "Music"];
        let found = |cell: &str| longest_named(cell, names.into_iter());
        assert_eq!(found("notes.md. File"), Some("notes.md"));
        assert_eq!(found("notes.md. 檔案"), Some("notes.md"));
        assert_eq!(found("Music"), Some("Music"));
        assert_eq!(found("logo.png. File"), None);
    }
}
