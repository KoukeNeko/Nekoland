//! What the menu bar learns over D-Bus, on a thread of its own so that a slow program never
//! holds up a frame: which actions an application offers (About, Settings, Quit...), the
//! status items programs put in the menu bar (StatusNotifierItem) and their menus
//! (com.canonical.dbusmenu), and whether the desktop is dark.

use std::{
    collections::HashMap,
    sync::{mpsc, Arc, Mutex},
};

use smithay::reexports::calloop::channel::Sender;
use zbus::{
    blocking::{Connection, MessageIterator},
    interface,
    message::Header,
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
    MatchRule,
};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

/// What the compositor asks for.
pub enum Request {
    /// The actions an application exports; answered with `Event::Actions`.
    Actions(String),
    Activate {
        app_id: String,
        action: String,
    },
    /// A status item's menu entry was chosen.
    MenuEvent {
        item: String,
        id: i32,
    },
    /// A status item without a menu was clicked.
    ItemActivate {
        item: String,
        x: i32,
        y: i32,
    },
    /// Read a status item again: it is new or said that it changed.
    Refresh {
        bus: String,
        path: String,
    },
    /// A program left the bus; forget its items.
    Gone(String),
}

/// What the compositor is told.
pub enum Event {
    Actions { app_id: String, actions: Vec<String> },
    Item(StatusItem),
    ItemGone(String),
    Dark(bool),
}

/// A status item as the menu bar shows it.
#[derive(Clone)]
pub struct StatusItem {
    /// Bus name and object path, which together name it.
    pub key: String,
    pub icon_name: String,
    pub icon_theme_path: String,
    /// The largest of the icons it drew itself: width, height and ARGB in network byte order.
    pub pixmap: Option<(i32, i32, Vec<u8>)>,
    pub title: String,
    pub passive: bool,
    pub menu: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    /// Some(state) for an entry with a check mark or a radio button.
    pub checked: Option<bool>,
    /// How deep in submenus it is; submenus are shown inline, indented.
    pub depth: usize,
}

/// The way to the D-Bus thread.
pub struct Bus {
    requests: mpsc::Sender<Request>,
}

impl Bus {
    pub fn send(&self, request: Request) {
        // The thread only stops when the session bus is gone, and then there is nobody to ask.
        let _ = self.requests.send(request);
    }
}

/// Connects to the session bus and starts serving and asking on a thread.
pub fn start(events: Sender<Event>) -> zbus::Result<Bus> {
    let connection = Connection::session()?;
    let (requests, receiver) = mpsc::channel();

    let registered = Arc::new(Mutex::new(Vec::new()));
    let watcher = Watcher {
        registered: registered.clone(),
        requests: requests.clone(),
    };
    connection.object_server().at(WATCHER_PATH, watcher)?;
    // Another panel may already be the watcher; then its items are not shown here.
    if let Err(err) = connection.request_name(WATCHER_NAME) {
        tracing::warn!("status items are shown elsewhere: {err}");
    }

    listen(&connection, requests.clone(), events.clone())?;

    let thread_connection = connection.clone();
    std::thread::Builder::new()
        .name("nekoland-dbus".into())
        .spawn(move || serve(thread_connection, receiver, registered, events))
        .map_err(|err| zbus::Error::Failure(err.to_string()))?;
    Ok(Bus { requests })
}

struct Watcher {
    registered: Arc<Mutex<Vec<String>>>,
    requests: mpsc::Sender<Request>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let sender = header
            .sender()
            .map(|sender| sender.to_string())
            .unwrap_or_default();
        // Programs give either their bus name or, from their unique name, an object path.
        let (bus, path) = match service.strip_prefix('/') {
            Some(_) => (sender, service.to_string()),
            None => (service.to_string(), "/StatusNotifierItem".to_string()),
        };
        let key = format!("{bus}{path}");
        let added = {
            let mut registered = self.registered.lock().unwrap();
            let added = !registered.contains(&key);
            if added {
                registered.push(key.clone());
            }
            added
        };
        if added {
            let _ = self.registered_status_notifier_items_changed(&emitter).await;
            let _ = Self::status_notifier_item_registered(&emitter, &key).await;
        }
        let _ = self.requests.send(Request::Refresh { bus, path });
    }

    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.registered.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Turns the signals the menu bar cares about into requests and events.
fn listen(
    connection: &Connection,
    requests: mpsc::Sender<Request>,
    events: Sender<Event>,
) -> zbus::Result<()> {
    let rules = [
        // An item's icon, title, status or menu changed.
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(ITEM_INTERFACE)?
            .build(),
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(MENU_INTERFACE)?
            .build(),
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .build(),
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Settings")?
            .member("SettingChanged")?
            .build(),
    ];
    for rule in rules {
        let messages = MessageIterator::for_match_rule(rule, connection, Some(64))?;
        let requests = requests.clone();
        let events = events.clone();
        std::thread::Builder::new()
            .name("nekoland-dbus-signals".into())
            .spawn(move || {
                for message in messages.flatten() {
                    let header = message.header();
                    let member = header
                        .member()
                        .map(|member| member.to_string())
                        .unwrap_or_default();
                    let sender = header
                        .sender()
                        .map(|sender| sender.to_string())
                        .unwrap_or_default();
                    let path = header.path().map(|path| path.to_string()).unwrap_or_default();
                    match member.as_str() {
                        "NameOwnerChanged" => {
                            if let Ok((name, _old, new)) =
                                message.body().deserialize::<(String, String, String)>()
                            {
                                if new.is_empty() {
                                    let _ = requests.send(Request::Gone(name));
                                }
                            }
                        }
                        "SettingChanged" => {
                            if let Ok((namespace, key, value)) =
                                message.body().deserialize::<(String, String, OwnedValue)>()
                            {
                                if namespace == "org.freedesktop.appearance" && key == "color-scheme" {
                                    let _ = events.send(Event::Dark(u32::try_from(value).ok() == Some(1)));
                                }
                            }
                        }
                        // A menu changing says so on the menu's path, an item on its own.
                        "LayoutUpdated" | "ItemsPropertiesUpdated" => {
                            let _ = requests.send(Request::Refresh {
                                bus: sender,
                                path: String::new(),
                            });
                        }
                        _ => {
                            let _ = requests.send(Request::Refresh { bus: sender, path });
                        }
                    }
                }
            })
            .map_err(|err| zbus::Error::Failure(err.to_string()))?;
    }
    Ok(())
}

fn serve(
    connection: Connection,
    requests: mpsc::Receiver<Request>,
    registered: Arc<Mutex<Vec<String>>>,
    events: Sender<Event>,
) {
    if let Some(dark) = read_dark(&connection) {
        let _ = events.send(Event::Dark(dark));
    }
    // Status items by key, with the bus name and path to reach them and their menu.
    let mut items: HashMap<String, (String, String, Option<String>)> = HashMap::new();

    for request in requests {
        match request {
            Request::Actions(app_id) => {
                let path = application_path(&app_id);
                let actions = connection
                    .call_method(
                        Some(app_id.as_str()),
                        path.as_str(),
                        Some("org.gtk.Actions"),
                        "List",
                        &(),
                    )
                    .and_then(|reply| reply.body().deserialize::<(Vec<String>,)>())
                    .map(|(actions,)| actions)
                    .unwrap_or_default();
                let _ = events.send(Event::Actions { app_id, actions });
            }
            Request::Activate { app_id, action } => {
                let path = application_path(&app_id);
                let none: Vec<Value> = Vec::new();
                let platform: HashMap<&str, Value> = HashMap::new();
                if let Err(err) = connection.call_method(
                    Some(app_id.as_str()),
                    path.as_str(),
                    Some("org.gtk.Actions"),
                    "Activate",
                    &(action.as_str(), none, platform),
                ) {
                    tracing::warn!("{app_id}: {action}: {err}");
                }
            }
            Request::MenuEvent { item, id } => {
                if let Some((bus, _, Some(menu))) = items.get(&item) {
                    let data = Value::from(0i32);
                    let _ = connection.call_method(
                        Some(bus.as_str()),
                        menu.as_str(),
                        Some(MENU_INTERFACE),
                        "Event",
                        &(id, "clicked", data, 0u32),
                    );
                }
            }
            Request::ItemActivate { item, x, y } => {
                if let Some((bus, path, _)) = items.get(&item) {
                    let _ = connection.call_method(
                        Some(bus.as_str()),
                        path.as_str(),
                        Some(ITEM_INTERFACE),
                        "Activate",
                        &(x, y),
                    );
                }
            }
            Request::Refresh { bus, path } => {
                // A signal from a menu, or from an item path that is not one: read all of the
                // program's items again.
                let keys: Vec<(String, String)> =
                    if path.is_empty() || !items.contains_key(&format!("{bus}{path}")) {
                        let mut keys: Vec<_> = items
                            .values()
                            .filter(|(item_bus, _, _)| *item_bus == bus)
                            .map(|(bus, path, _)| (bus.clone(), path.clone()))
                            .collect();
                        if !path.is_empty() && registered.lock().unwrap().contains(&format!("{bus}{path}")) {
                            keys.push((bus.clone(), path.clone()));
                        }
                        keys
                    } else {
                        vec![(bus.clone(), path.clone())]
                    };
                for (bus, path) in keys {
                    match read_item(&connection, &bus, &path) {
                        Some((item, menu)) => {
                            items.insert(item.key.clone(), (bus, path, menu));
                            let _ = events.send(Event::Item(item));
                        }
                        None => tracing::debug!("status item {bus}{path} could not be read"),
                    }
                }
            }
            Request::Gone(name) => {
                let gone: Vec<String> = items
                    .iter()
                    .filter(|(_, (bus, _, _))| *bus == name)
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in gone {
                    items.remove(&key);
                    registered.lock().unwrap().retain(|known| *known != key);
                    let _ = events.send(Event::ItemGone(key));
                }
            }
        }
    }
}

/// Where a GtkApplication exports its actions: its ID as a path.
fn application_path(app_id: &str) -> String {
    format!("/{}", app_id.replace('.', "/").replace('-', "_"))
}

fn read_dark(connection: &Connection) -> Option<bool> {
    let reply = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Settings"),
            "ReadOne",
            &("org.freedesktop.appearance", "color-scheme"),
        )
        .ok()?;
    let (value,) = reply.body().deserialize::<(OwnedValue,)>().ok()?;
    Some(u32::try_from(value).ok()? == 1)
}

fn read_item(connection: &Connection, bus: &str, path: &str) -> Option<(StatusItem, Option<String>)> {
    let reply = connection
        .call_method(
            Some(bus),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &(ITEM_INTERFACE,),
        )
        .ok()?;
    let (properties,) = reply
        .body()
        .deserialize::<(HashMap<String, OwnedValue>,)>()
        .ok()?;
    let text = |name: &str| {
        properties
            .get(name)
            .and_then(|value| String::try_from(value.clone()).ok())
            .unwrap_or_default()
    };
    let menu = properties
        .get("Menu")
        .and_then(|value| zbus::zvariant::OwnedObjectPath::try_from(value.clone()).ok())
        .map(|path| path.to_string())
        .filter(|path| path != "/");
    let pixmap = properties
        .get("IconPixmap")
        .and_then(largest_pixmap);
    let entries = menu
        .as_deref()
        .map(|menu| read_menu(connection, bus, menu))
        .unwrap_or_default();

    let item = StatusItem {
        key: format!("{bus}{path}"),
        icon_name: text("IconName"),
        icon_theme_path: text("IconThemePath"),
        pixmap,
        title: text("Title"),
        passive: text("Status") == "Passive",
        menu: entries,
    };
    Some((item, menu))
}

fn largest_pixmap(value: &OwnedValue) -> Option<(i32, i32, Vec<u8>)> {
    let Value::Array(pixmaps) = &**value else {
        return None;
    };
    pixmaps
        .iter()
        .filter_map(|pixmap| {
            let Value::Structure(pixmap) = pixmap else {
                return None;
            };
            match pixmap.fields() {
                [Value::I32(width), Value::I32(height), Value::Array(bytes)] => {
                    let bytes: Vec<u8> = bytes
                        .iter()
                        .filter_map(|byte| match byte {
                            Value::U8(byte) => Some(*byte),
                            _ => None,
                        })
                        .collect();
                    (bytes.len() == (*width * *height * 4) as usize).then_some((*width, *height, bytes))
                }
                _ => None,
            }
        })
        .max_by_key(|(width, _, _)| *width)
}

fn read_menu(connection: &Connection, bus: &str, menu: &str) -> Vec<MenuEntry> {
    let _ = connection.call_method(Some(bus), menu, Some(MENU_INTERFACE), "AboutToShow", &(0i32,));
    let properties: Vec<&str> = Vec::new();
    let Ok(reply) = connection.call_method(
        Some(bus),
        menu,
        Some(MENU_INTERFACE),
        "GetLayout",
        &(0i32, -1i32, properties),
    ) else {
        return Vec::new();
    };
    let Ok((_revision, root)) = reply.body().deserialize::<(u32, Layout)>() else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for child in &root.children {
        flatten(child, 0, &mut entries);
    }
    entries
}

/// The top of a dbusmenu layout: its ID, its properties and its children, each a variant
/// holding a node like it. Only the children are entries.
#[derive(serde::Deserialize, zbus::zvariant::Type)]
struct Layout {
    _id: i32,
    _properties: HashMap<String, OwnedValue>,
    children: Vec<OwnedValue>,
}

/// A dbusmenu layout node: its ID, its properties and its children.
fn layout_node<'a>(
    value: &'a Value<'a>,
) -> Option<(i32, HashMap<String, &'a Value<'a>>, Vec<&'a Value<'a>>)> {
    let value = match value {
        Value::Value(inner) => inner.as_ref(),
        other => other,
    };
    let Value::Structure(node) = value else {
        return None;
    };
    let [Value::I32(id), Value::Dict(properties), Value::Array(children)] = node.fields() else {
        return None;
    };
    let properties = properties
        .iter()
        .filter_map(|(key, value)| {
            let Value::Str(key) = key else {
                return None;
            };
            let value = match value {
                Value::Value(inner) => inner.as_ref(),
                other => other,
            };
            Some((key.to_string(), value))
        })
        .collect();
    Some((*id, properties, children.iter().collect()))
}

fn flatten(node: &Value<'_>, depth: usize, entries: &mut Vec<MenuEntry>) {
    let Some((id, properties, children)) = layout_node(node) else {
        return;
    };
    let text = |name: &str| match properties.get(name) {
        Some(Value::Str(text)) => Some(text.to_string()),
        _ => None,
    };
    let flag = |name: &str| match properties.get(name) {
        Some(Value::Bool(flag)) => Some(*flag),
        _ => None,
    };
    if flag("visible") == Some(false) {
        return;
    }
    let checked = match text("toggle-type").as_deref() {
        Some("checkmark" | "radio") => Some(matches!(properties.get("toggle-state"), Some(Value::I32(1)))),
        _ => None,
    };
    entries.push(MenuEntry {
        id,
        label: strip_mnemonic(&text("label").unwrap_or_default()),
        enabled: flag("enabled").unwrap_or(true),
        separator: text("type").as_deref() == Some("separator"),
        checked,
        depth,
    });
    for child in children {
        flatten(child, depth + 1, entries);
    }
}

/// dbusmenu marks the access key with an underscore; a doubled one is a real underscore.
fn strip_mnemonic(label: &str) -> String {
    label
        .replace("__", "\u{0}")
        .replace('_', "")
        .replace('\u{0}', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_are_dropped_and_doubled_underscores_kept() {
        assert_eq!(strip_mnemonic("_Restart"), "Restart");
        assert_eq!(strip_mnemonic("snake__case"), "snake_case");
    }

    #[test]
    fn application_paths_follow_gtk() {
        assert_eq!(application_path("org.gnome.TextEditor"), "/org/gnome/TextEditor");
        assert_eq!(application_path("io.github.some-app"), "/io/github/some_app");
    }
}
