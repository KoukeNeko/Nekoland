//! What the menu bar learns over D-Bus, on a thread of its own so that a slow program never
//! holds up a frame: which actions an application offers (About, Settings, Quit...), the
//! menus it exports (com.canonical.dbusmenu from Qt, org.gtk.Menus from GTK), the status
//! items programs put in the menu bar (StatusNotifierItem) and their menus, and whether
//! the desktop is dark.

use std::{
    collections::{BTreeSet, HashMap},
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
const REGISTRAR_NAME: &str = "com.canonical.AppMenu.Registrar";
const NOTIFICATIONS_NAME: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const REGISTRAR_PATH: &str = "/com/canonical/AppMenu/Registrar";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DBUSMENU: &str = "com.canonical.dbusmenu";
/// What applications say of their Dock icon: a count, how far something has come, and
/// that they ask for attention, as Ubuntu's dock and KDE's task manager take it.
const LAUNCHER_ENTRY: &str = "com.canonical.Unity.LauncherEntry";
const GTK_MENUS: &str = "org.gtk.Menus";
const GTK_ACTIONS: &str = "org.gtk.Actions";

/// Where a window's menus are, as its program said over Wayland.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AppMenuAddress {
    Dbusmenu {
        bus: String,
        path: String,
    },
    Gtk {
        bus: String,
        menubar: String,
        /// Where the `app.` and `win.` actions of the menu items are.
        app: String,
        window: String,
    },
}

/// What a GTK action is given when it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    None,
    Text(String),
    Number(i32),
}

/// How choosing a menu entry reaches its program.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Dbusmenu {
        bus: String,
        path: String,
        id: i32,
    },
    Gtk {
        bus: String,
        path: String,
        name: String,
        target: Target,
    },
}

/// What the compositor asks for.
pub enum Request {
    /// The actions an application exports; answered with `Event::Actions`.
    Actions(String),
    /// A notification went away: 1 expired, 2 dismissed, 3 closed by its program.
    NotificationClosed {
        id: u32,
        reason: u32,
    },
    /// One of a notification's actions was chosen; "default" for a click on it.
    NotificationAction {
        id: u32,
        key: String,
    },
    Activate {
        app_id: String,
        action: String,
    },
    /// The menus at an address; answered with `Event::AppMenus`.
    AppMenus(AppMenuAddress),
    /// A menu entry was chosen.
    Invoke(MenuAction),
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
    Notify(Notification),
    CloseNotification(u32),
    Actions {
        app_id: String,
        actions: Vec<String>,
    },
    AppMenus {
        address: AppMenuAddress,
        menus: Vec<MenuEntry>,
    },
    /// A program said its menus changed.
    MenusChanged(String),
    Item(StatusItem),
    ItemGone(String),
    Dark(bool),
    /// An X11 window said where its menus are, through the AppMenu registrar.
    X11Menu {
        window: u32,
        address: Option<AppMenuAddress>,
    },
    /// The default output's volume, or None when there is no sound server to ask.
    Sound(Option<crate::system::Volume>),
    /// Whether a microphone or a camera is open; the screen's sharing is the compositor's own.
    InUse(crate::privacy::InUse),
    Ime(crate::ime::ImeEvent),
    /// An application said what its Dock icon is to show, from `sender`.
    Launcher {
        sender: String,
        app_id: String,
        update: LauncherUpdate,
    },
    /// A bus name went away, and with it what its application said of its icon.
    LauncherGone(String),
}

/// What an application changed of its Dock icon; what it left out stays as it was.
#[derive(Clone, Debug, Default)]
pub struct LauncherUpdate {
    pub count: Option<i64>,
    pub count_visible: Option<bool>,
    pub progress: Option<f64>,
    pub progress_visible: Option<bool>,
    pub urgent: Option<bool>,
}

/// A notification as a program sent it.
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// An icon name or file, from the program or its desktop entry.
    pub icon: String,
    /// A picture sent along: width, height and RGBA rows.
    pub image: Option<(i32, i32, Vec<u8>)>,
    pub summary: String,
    pub body: String,
    /// Action keys and their labels, in pairs.
    pub actions: Vec<String>,
    pub timeout: Expiry,
    /// Whether the program asked for it to be shown and not kept, with the `transient` hint.
    pub transient: bool,
}

/// How long a notification is shown.
#[derive(Clone, Copy, Debug)]
pub enum Expiry {
    UntilDismissed,
    After(std::time::Duration),
    /// The program leaves it to the server, which leaves it to the settings.
    AsSet,
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
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    /// Some(state) for an entry with a check mark or a radio button.
    pub checked: Option<bool>,
    /// Its keyboard shortcut, as the menu shows it.
    pub shortcut: String,
    pub action: Option<MenuAction>,
    pub children: Vec<MenuEntry>,
}

impl MenuEntry {
    fn separator() -> Self {
        Self {
            label: String::new(),
            enabled: false,
            separator: true,
            checked: None,
            shortcut: String::new(),
            action: None,
            children: Vec::new(),
        }
    }
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
    // Qt only exports its menus when there is a registrar, on Wayland too; X11 programs
    // say where their menus are through it.
    // Notifications are shown by Nekoland; another daemon running keeps the name.
    let notifications = Notifications {
        next_id: std::sync::atomic::AtomicU32::new(1),
        events: events.clone(),
    };
    connection.object_server().at(NOTIFICATIONS_PATH, notifications)?;
    if let Err(err) = connection.request_name(NOTIFICATIONS_NAME) {
        tracing::warn!("notifications are shown elsewhere: {err}");
    }

    let registrar = Registrar {
        windows: Arc::new(Mutex::new(HashMap::new())),
        events: events.clone(),
    };
    connection.object_server().at(REGISTRAR_PATH, registrar)?;
    if let Err(err) = connection.request_name(REGISTRAR_NAME) {
        tracing::warn!("application menus are shown elsewhere: {err}");
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

struct Notifications {
    next_id: std::sync::atomic::AtomicU32,
    events: Sender<Event>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    fn get_capabilities(&self) -> Vec<&'static str> {
        // Persistence: what is not transient stays in the Notification Center after its banner.
        vec!["body", "actions", "icon-static", "persistence"]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        let text = |name: &str| {
            hints
                .get(name)
                .and_then(|value| String::try_from(value.clone()).ok())
                .unwrap_or_default()
        };
        // The icon: what the program named, else a picture file, else its desktop entry's.
        let icon = [app_icon, text("image-path"), text("image_path")]
            .into_iter()
            .find(|icon| !icon.is_empty())
            .unwrap_or_else(|| text("desktop-entry"));
        let image = ["image-data", "image_data", "icon_data"]
            .iter()
            .find_map(|name| hints.get(*name))
            .and_then(image_data);
        let timeout = match expire_timeout {
            0 => Expiry::UntilDismissed,
            milliseconds if milliseconds > 0 => {
                Expiry::After(std::time::Duration::from_millis(milliseconds as u64))
            }
            _ => Expiry::AsSet,
        };
        let transient = hints
            .get("transient")
            .and_then(|value| bool::try_from(value.clone()).ok())
            .unwrap_or(false);
        let _ = self.events.send(Event::Notify(Notification {
            id,
            app_name,
            icon,
            image,
            summary,
            body: plain_text(&body),
            actions,
            timeout,
            transient,
        }));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.events.send(Event::CloseNotification(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Nekoland", "Nekoland", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

/// The picture of an image-data hint: width, height, row stride, alpha, bits per sample,
/// channels and bytes, turned into rows of RGBA.
fn image_data(value: &OwnedValue) -> Option<(i32, i32, Vec<u8>)> {
    let Value::Structure(image) = &**value else {
        return None;
    };
    let [Value::I32(width), Value::I32(height), Value::I32(stride), Value::Bool(alpha), Value::I32(_bits), Value::I32(channels), Value::Array(bytes)] =
        image.fields()
    else {
        return None;
    };
    let bytes: Vec<u8> = bytes
        .iter()
        .filter_map(|byte| match byte {
            Value::U8(byte) => Some(*byte),
            _ => None,
        })
        .collect();
    // Any program may send this: one that claims more than its bytes hold, or a picture far
    // bigger than a notification shows, is left out rather than read past its end.
    let (Ok(w), Ok(h), Ok(stride), Ok(channels)) = (
        usize::try_from(*width),
        usize::try_from(*height),
        usize::try_from(*stride),
        usize::try_from(*channels),
    ) else {
        return None;
    };
    let sides = 1..=MAX_IMAGE_SIDE;
    if !sides.contains(&w)
        || !sides.contains(&h)
        || channels != if *alpha { 4 } else { 3 }
        || stride < w * channels
        || bytes.len() < stride.checked_mul(h - 1)?.checked_add(w * channels)?
    {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let pixel = &bytes[y * stride + x * channels..];
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], if *alpha { pixel[3] } else { 255 }]);
        }
    }
    Some((*width, *height, rgba))
}

/// The widest and tallest notification picture taken.
const MAX_IMAGE_SIDE: usize = 1024;

/// Notification bodies may carry a little markup; it is shown as plain text.
fn plain_text(markup: &str) -> String {
    let mut text = String::with_capacity(markup.len());
    let mut in_tag = false;
    for c in markup.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

struct Registrar {
    windows: Arc<Mutex<HashMap<u32, (String, String)>>>,
    events: Sender<Event>,
}

#[interface(name = "com.canonical.AppMenu.Registrar")]
impl Registrar {
    async fn register_window(
        &self,
        window_id: u32,
        menu_object_path: zbus::zvariant::ObjectPath<'_>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let bus = header
            .sender()
            .map(|sender| sender.to_string())
            .unwrap_or_default();
        let path = menu_object_path.to_string();
        self.windows
            .lock()
            .unwrap()
            .insert(window_id, (bus.clone(), path.clone()));
        let _ = Self::window_registered(&emitter, window_id, &bus, menu_object_path).await;
        let _ = self.events.send(Event::X11Menu {
            window: window_id,
            address: Some(AppMenuAddress::Dbusmenu { bus, path }),
        });
    }

    async fn unregister_window(&self, window_id: u32, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        self.windows.lock().unwrap().remove(&window_id);
        let _ = Self::window_unregistered(&emitter, window_id).await;
        let _ = self.events.send(Event::X11Menu {
            window: window_id,
            address: None,
        });
    }

    fn get_menu_for_window(&self, window_id: u32) -> (String, zbus::zvariant::OwnedObjectPath) {
        let (bus, path) = self
            .windows
            .lock()
            .unwrap()
            .get(&window_id)
            .cloned()
            .unwrap_or_default();
        let path =
            zbus::zvariant::OwnedObjectPath::try_from(if path.is_empty() { "/".to_string() } else { path })
                .unwrap_or_default();
        (bus, path)
    }

    #[zbus(signal)]
    async fn window_registered(
        emitter: &SignalEmitter<'_>,
        window_id: u32,
        service: &str,
        path: zbus::zvariant::ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn window_unregistered(emitter: &SignalEmitter<'_>, window_id: u32) -> zbus::Result<()>;
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
            .interface(DBUSMENU)?
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
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(LAUNCHER_ENTRY)?
            .member("Update")?
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
                                    let _ = events.send(Event::LauncherGone(name.clone()));
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
                        "Update" if header.interface().is_some_and(|name| name.as_str() == LAUNCHER_ENTRY) => {
                            if let Ok((uri, properties)) =
                                message.body().deserialize::<(String, HashMap<String, OwnedValue>)>()
                            {
                                let app_id = uri.trim_start_matches("application://").trim_end_matches(".desktop");
                                let read = |key: &str| properties.get(key).and_then(|value| value.try_clone().ok());
                                let update = LauncherUpdate {
                                    count: read("count").and_then(|value| i64::try_from(value).ok()),
                                    count_visible: read("count-visible").and_then(|value| bool::try_from(value).ok()),
                                    progress: read("progress").and_then(|value| f64::try_from(value).ok()),
                                    progress_visible: read("progress-visible")
                                        .and_then(|value| bool::try_from(value).ok()),
                                    urgent: read("urgent").and_then(|value| bool::try_from(value).ok()),
                                };
                                let _ = events.send(Event::Launcher {
                                    sender: sender.clone(),
                                    app_id: app_id.to_string(),
                                    update,
                                });
                            }
                        }
                        // A menu changing says so on the menu's path, an item on its own.
                        "LayoutUpdated" | "ItemsPropertiesUpdated" => {
                            let _ = events.send(Event::MenusChanged(sender.clone()));
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
    // Status items by key, with the bus name and path to reach them.
    let mut items: HashMap<String, (String, String)> = HashMap::new();

    for request in requests {
        match request {
            Request::Actions(app_id) => {
                let path = application_path(&app_id);
                let actions = connection
                    .call_method(
                        Some(app_id.as_str()),
                        path.as_str(),
                        Some(GTK_ACTIONS),
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
                activate_gtk(&connection, &app_id, &path, &action, &Target::None);
            }
            Request::AppMenus(address) => {
                let menus = match &address {
                    AppMenuAddress::Dbusmenu { bus, path } => read_dbusmenu(&connection, bus, path),
                    AppMenuAddress::Gtk {
                        bus,
                        menubar,
                        app,
                        window,
                    } => read_gtk_menus(&connection, bus, menubar, app, window),
                };
                // A menu that could not be read keeps what was shown before.
                if let Some(menus) = menus {
                    let _ = events.send(Event::AppMenus { address, menus });
                }
            }
            Request::Invoke(MenuAction::Dbusmenu { bus, path, id }) => {
                let data = Value::from(0i32);
                if let Err(err) = connection.call_method(
                    Some(bus.as_str()),
                    path.as_str(),
                    Some(DBUSMENU),
                    "Event",
                    &(id, "clicked", data, 0u32),
                ) {
                    tracing::warn!("{bus}{path}: menu entry {id}: {err}");
                }
            }
            Request::Invoke(MenuAction::Gtk {
                bus,
                path,
                name,
                target,
            }) => activate_gtk(&connection, &bus, &path, &name, &target),
            Request::ItemActivate { item, x, y } => {
                if let Some((bus, path)) = items.get(&item) {
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
                // A signal from a menu, or from a path that is not a known item: read all of
                // the program's items again, and the new one if it just registered.
                let key = format!("{bus}{path}");
                let mut keys: Vec<(String, String)> = if path.is_empty() || !items.contains_key(&key) {
                    items
                        .values()
                        .filter(|(item_bus, _)| *item_bus == bus)
                        .cloned()
                        .collect()
                } else {
                    vec![(bus.clone(), path.clone())]
                };
                if !path.is_empty() && !items.contains_key(&key) && registered.lock().unwrap().contains(&key)
                {
                    keys.push((bus.clone(), path.clone()));
                }
                for (bus, path) in keys {
                    match read_item(&connection, &bus, &path) {
                        Some(item) => {
                            items.insert(item.key.clone(), (bus, path));
                            let _ = events.send(Event::Item(item));
                        }
                        None => tracing::debug!("status item {bus}{path} could not be read"),
                    }
                }
            }
            Request::NotificationClosed { id, reason } => {
                let _ = connection.emit_signal(
                    None::<()>,
                    NOTIFICATIONS_PATH,
                    NOTIFICATIONS_NAME,
                    "NotificationClosed",
                    &(id, reason),
                );
            }
            Request::NotificationAction { id, key } => {
                let _ = connection.emit_signal(
                    None::<()>,
                    NOTIFICATIONS_PATH,
                    NOTIFICATIONS_NAME,
                    "ActionInvoked",
                    &(id, key.as_str()),
                );
            }
            Request::Gone(name) => {
                let gone: Vec<String> = items
                    .iter()
                    .filter(|(_, (bus, _))| *bus == name)
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

fn activate_gtk(connection: &Connection, bus: &str, path: &str, name: &str, target: &Target) {
    let parameter: Vec<Value> = match target {
        Target::None => Vec::new(),
        Target::Text(text) => vec![Value::from(text.as_str())],
        Target::Number(number) => vec![Value::from(*number)],
    };
    let platform: HashMap<&str, Value> = HashMap::new();
    if let Err(err) = connection.call_method(
        Some(bus),
        path,
        Some(GTK_ACTIONS),
        "Activate",
        &(name, parameter, platform),
    ) {
        tracing::warn!("{bus}{path}: {name}: {err}");
    }
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

fn read_item(connection: &Connection, bus: &str, path: &str) -> Option<StatusItem> {
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
    let pixmap = properties.get("IconPixmap").and_then(largest_pixmap);
    let entries = menu
        .as_deref()
        .and_then(|menu| read_dbusmenu(connection, bus, menu))
        .unwrap_or_default();

    Some(StatusItem {
        key: format!("{bus}{path}"),
        icon_name: text("IconName"),
        icon_theme_path: text("IconThemePath"),
        pixmap,
        title: text("Title"),
        passive: text("Status") == "Passive",
        menu: entries,
    })
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

/// The top of a dbusmenu layout: its ID, its properties and its children, each a variant
/// holding a node like it. Only the children are entries.
#[derive(serde::Deserialize, zbus::zvariant::Type)]
struct Layout {
    _id: i32,
    _properties: HashMap<String, OwnedValue>,
    children: Vec<OwnedValue>,
}

/// A dbusmenu: the entries under its root, each with its own submenu.
// AboutToShow is not called: Qt answers it by saying its layout changed, which would
// have the menu read again, and again.
fn read_dbusmenu(connection: &Connection, bus: &str, path: &str) -> Option<Vec<MenuEntry>> {
    let properties: Vec<&str> = Vec::new();
    let reply = connection
        .call_method(
            Some(bus),
            path,
            Some(DBUSMENU),
            "GetLayout",
            &(0i32, -1i32, properties),
        )
        .ok()?;
    let (_revision, root) = reply.body().deserialize::<(u32, Layout)>().ok()?;
    Some(tidy(
        root.children
            .iter()
            .filter_map(|child| dbusmenu_entry(child, bus, path))
            .collect(),
    ))
}

/// Drops entries with nothing to show, and separators at the ends or next to each other.
fn tidy(entries: Vec<MenuEntry>) -> Vec<MenuEntry> {
    let mut tidied: Vec<MenuEntry> = Vec::with_capacity(entries.len());
    for mut entry in entries {
        if entry.separator {
            if tidied.last().is_some_and(|last| !last.separator) {
                tidied.push(entry);
            }
            continue;
        }
        if entry.label.trim().is_empty() && entry.children.is_empty() {
            continue;
        }
        entry.children = tidy(std::mem::take(&mut entry.children));
        tidied.push(entry);
    }
    while tidied.last().is_some_and(|last| last.separator) {
        tidied.pop();
    }
    tidied
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

fn dbusmenu_entry(node: &Value<'_>, bus: &str, path: &str) -> Option<MenuEntry> {
    let (id, properties, children) = layout_node(node)?;
    let text = |name: &str| match properties.get(name) {
        Some(Value::Str(text)) => Some(text.to_string()),
        _ => None,
    };
    let flag = |name: &str| match properties.get(name) {
        Some(Value::Bool(flag)) => Some(*flag),
        _ => None,
    };
    if flag("visible") == Some(false) {
        return None;
    }
    if text("type").as_deref() == Some("separator") {
        return Some(MenuEntry::separator());
    }
    let checked = match text("toggle-type").as_deref() {
        Some("checkmark" | "radio") => Some(matches!(properties.get("toggle-state"), Some(Value::I32(1)))),
        _ => None,
    };
    // Shortcuts come as lists of keys, e.g. [["Control", "S"]]; the first is shown.
    let shortcut = match properties.get("shortcut") {
        Some(Value::Array(shortcuts)) => shortcuts
            .iter()
            .next()
            .and_then(|keys| match keys {
                Value::Array(keys) => Some(
                    keys.iter()
                        .filter_map(|key| match key {
                            Value::Str(key) => Some(key.to_string()),
                            _ => None,
                        })
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .map(|keys| shortcut_label(&keys))
            .unwrap_or_default(),
        _ => String::new(),
    };
    Some(MenuEntry {
        label: strip_mnemonic(&text("label").unwrap_or_default()),
        enabled: flag("enabled").unwrap_or(true),
        separator: false,
        checked,
        shortcut,
        action: Some(MenuAction::Dbusmenu {
            bus: bus.to_string(),
            path: path.to_string(),
            id,
        }),
        children: children
            .into_iter()
            .filter_map(|child| dbusmenu_entry(child, bus, path))
            .collect(),
    })
}

/// A GTK menubar: its menus, read group by group, with what the actions say about whether
/// they can run and what state they are in.
fn read_gtk_menus(
    connection: &Connection,
    bus: &str,
    menubar: &str,
    app: &str,
    window: &str,
) -> Option<Vec<MenuEntry>> {
    type Items = Vec<HashMap<String, OwnedValue>>;
    let mut menus: HashMap<(u32, u32), Items> = HashMap::new();
    let mut wanted: BTreeSet<u32> = BTreeSet::from([0]);
    let mut loaded: BTreeSet<u32> = BTreeSet::new();
    while !wanted.is_subset(&loaded) {
        let groups: Vec<u32> = wanted.difference(&loaded).copied().collect();
        loaded.extend(groups.iter().copied());
        let reply = connection
            .call_method(Some(bus), menubar, Some(GTK_MENUS), "Start", &(groups.clone(),))
            .ok()?;
        let (contents,) = reply.body().deserialize::<(Vec<(u32, u32, Items)>,)>().ok()?;
        // Starting subscribes to changes; this reads once, so it lets go again.
        let _ = connection.call_method(Some(bus), menubar, Some(GTK_MENUS), "End", &(groups,));
        for (group, menu, items) in contents {
            for item in &items {
                for link in [":section", ":submenu"] {
                    if let Some((group, _)) = item
                        .get(link)
                        .and_then(|value| <(u32, u32)>::try_from(value.clone()).ok())
                    {
                        wanted.insert(group);
                    }
                }
            }
            menus.insert((group, menu), items);
        }
    }

    let describe = |path: &str| -> HashMap<String, (bool, Option<OwnedValue>)> {
        if path.is_empty() {
            return HashMap::new();
        }
        connection
            .call_method(Some(bus), path, Some(GTK_ACTIONS), "DescribeAll", &())
            .and_then(|reply| {
                reply
                    .body()
                    .deserialize::<(HashMap<String, (bool, zbus::zvariant::Signature, Vec<OwnedValue>)>,)>()
            })
            .map(|(actions,)| {
                actions
                    .into_iter()
                    .map(|(name, (enabled, _, state))| (name, (enabled, state.into_iter().next())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let actions = GtkActions {
        bus: bus.to_string(),
        app: app.to_string(),
        window: window.to_string(),
        app_actions: describe(app),
        window_actions: describe(window),
    };
    Some(tidy(gtk_menu(&menus, (0, 0), &actions, 0)))
}

struct GtkActions {
    bus: String,
    app: String,
    window: String,
    app_actions: HashMap<String, (bool, Option<OwnedValue>)>,
    window_actions: HashMap<String, (bool, Option<OwnedValue>)>,
}

const MAX_MENU_DEPTH: usize = 8;

fn gtk_menu(
    menus: &HashMap<(u32, u32), Vec<HashMap<String, OwnedValue>>>,
    at: (u32, u32),
    actions: &GtkActions,
    depth: usize,
) -> Vec<MenuEntry> {
    let Some(items) = menus.get(&at) else {
        return Vec::new();
    };
    if depth > MAX_MENU_DEPTH {
        return Vec::new();
    }
    let link = |item: &HashMap<String, OwnedValue>, name: &str| {
        item.get(name)
            .and_then(|value| <(u32, u32)>::try_from(value.clone()).ok())
    };
    let mut entries = Vec::new();
    for item in items {
        // A section is its items inline, set apart by separators.
        if let Some(section) = link(item, ":section") {
            let inner = gtk_menu(menus, section, actions, depth + 1);
            if inner.is_empty() {
                continue;
            }
            if !entries.is_empty() {
                entries.push(MenuEntry::separator());
            }
            entries.extend(inner);
            continue;
        }
        let text = |name: &str| {
            item.get(name)
                .and_then(|value| String::try_from(value.clone()).ok())
        };
        let label = strip_mnemonic(&text("label").unwrap_or_default());
        let children = link(item, ":submenu")
            .map(|submenu| gtk_menu(menus, submenu, actions, depth + 1))
            .unwrap_or_default();
        let target = match item.get("target").map(|value| &**value) {
            Some(Value::Str(text)) => Target::Text(text.to_string()),
            Some(Value::I32(number)) => Target::Number(*number),
            _ => Target::None,
        };
        let (action, enabled, checked) = match text("action") {
            Some(full) => {
                let (scope, name) = full.split_once('.').unwrap_or(("app", full.as_str()));
                let (path, described) = match scope {
                    "win" => (&actions.window, actions.window_actions.get(name)),
                    _ => (&actions.app, actions.app_actions.get(name)),
                };
                let enabled = described.is_none_or(|(enabled, _)| *enabled);
                // A boolean state is a check mark; a state equal to the target, a radio button.
                let checked = described.and_then(|(_, state)| match (state.as_deref(), &target) {
                    (Some(Value::Bool(on)), Target::None) => Some(*on),
                    (Some(Value::Str(state)), Target::Text(target)) => Some(state.as_str() == target),
                    (Some(Value::I32(state)), Target::Number(target)) => Some(state == target),
                    _ => None,
                });
                (
                    Some(MenuAction::Gtk {
                        bus: actions.bus.clone(),
                        path: path.clone(),
                        name: name.to_string(),
                        target: target.clone(),
                    }),
                    enabled,
                    checked,
                )
            }
            None => (None, !children.is_empty(), None),
        };
        entries.push(MenuEntry {
            label,
            enabled,
            separator: false,
            checked,
            shortcut: text("accel")
                .map(|accel| accelerator_label(&accel))
                .unwrap_or_default(),
            action,
            children,
        });
    }
    entries
}

/// dbusmenu and GTK mark the access key with an underscore; a doubled one is a real underscore.
fn strip_mnemonic(label: &str) -> String {
    label
        .replace("__", "\u{0}")
        .replace('_', "")
        .replace('\u{0}', "_")
}

/// A dbusmenu shortcut, such as ["Control", "Shift", "s"], as "Shift Ctrl S".
fn shortcut_label(keys: &[String]) -> String {
    let mut modifiers = Vec::new();
    let mut key = String::new();
    for part in keys {
        match part.as_str() {
            "Control" => modifiers.push("Ctrl"),
            "Shift" => modifiers.push("Shift"),
            "Alt" => modifiers.push("Alt"),
            "Super" => modifiers.push("Super"),
            other => key = other.to_uppercase(),
        }
    }
    modifiers.sort_by_key(|modifier| MODIFIER_ORDER.iter().position(|known| known == modifier));
    modifiers.push(&key);
    modifiers.join(" ")
}

/// A GTK accelerator, such as "<Primary><Shift>s", as "Shift Ctrl S".
fn accelerator_label(accel: &str) -> String {
    let mut modifiers = Vec::new();
    let mut rest = accel;
    while let Some(start) = rest.strip_prefix('<') {
        let Some((modifier, after)) = start.split_once('>') else {
            break;
        };
        modifiers.push(match modifier.to_lowercase().as_str() {
            "primary" | "control" | "ctrl" => "Ctrl",
            "shift" => "Shift",
            "alt" | "mod1" => "Alt",
            "super" | "meta" => "Super",
            _ => continue,
        });
        rest = after;
    }
    modifiers.sort_by_key(|modifier| MODIFIER_ORDER.iter().position(|known| known == modifier));
    let key = rest.to_uppercase();
    modifiers.push(&key);
    modifiers.join(" ")
}

// The order the Mac shows modifiers in, with Ctrl where it shows Command.
const MODIFIER_ORDER: [&str; 4] = ["Super", "Alt", "Shift", "Ctrl"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonics_are_dropped_and_doubled_underscores_kept() {
        assert_eq!(strip_mnemonic("_Restart"), "Restart");
        assert_eq!(strip_mnemonic("snake__case"), "snake_case");
    }

    #[test]
    fn notification_pictures_are_read_only_as_far_as_their_bytes_go() {
        let image = |stride: i32, alpha: bool, channels: i32, bytes: Vec<u8>| {
            OwnedValue::try_from(Value::from((1i32, 1i32, stride, alpha, 8i32, channels, bytes)))
                .map(|value| image_data(&value))
                .unwrap()
        };
        assert_eq!(
            image(3, false, 3, vec![1, 2, 3]),
            Some((1, 1, vec![1, 2, 3, 255]))
        );
        assert_eq!(
            image(4, true, 4, vec![1, 2, 3, 4]),
            Some((1, 1, vec![1, 2, 3, 4]))
        );
        // Alpha claimed over three channels would read a fourth byte that is not there.
        assert_eq!(image(3, true, 3, vec![1, 2, 3]), None);
        assert_eq!(image(-4, true, 4, vec![1, 2, 3, 4]), None);
        let huge = OwnedValue::try_from(Value::from((
            20000i32,
            20000i32,
            0i32,
            true,
            8i32,
            4i32,
            vec![0u8; 80_000],
        )))
        .unwrap();
        assert_eq!(image_data(&huge), None);
    }

    #[test]
    fn application_paths_follow_gtk() {
        assert_eq!(application_path("org.gnome.TextEditor"), "/org/gnome/TextEditor");
        assert_eq!(application_path("io.github.some-app"), "/io/github/some_app");
    }

    #[test]
    fn tidying_drops_blank_entries_and_stray_separators() {
        let item = |label: &str| MenuEntry {
            label: label.into(),
            ..MenuEntry::separator()
        };
        let entry = |label: &str| MenuEntry {
            separator: false,
            enabled: true,
            ..item(label)
        };
        let tidied = tidy(vec![
            MenuEntry::separator(),
            entry("Print"),
            MenuEntry::separator(),
            entry(""),
            MenuEntry::separator(),
            entry("Close"),
            MenuEntry::separator(),
        ]);
        let labels: Vec<_> = tidied
            .iter()
            .map(|entry| (entry.label.as_str(), entry.separator))
            .collect();
        assert_eq!(labels, [("Print", false), ("", true), ("Close", false)]);
    }

    #[test]
    fn notification_markup_becomes_plain_text() {
        assert_eq!(plain_text("<b>Done</b> &amp; saved"), "Done & saved");
        assert_eq!(plain_text("a &lt; b"), "a < b");
    }

    #[test]
    fn shortcuts_read_as_the_menu_shows_them() {
        assert_eq!(accelerator_label("<Primary><Shift>s"), "Shift Ctrl S");
        assert_eq!(accelerator_label("<Control>q"), "Ctrl Q");
        assert_eq!(accelerator_label("F11"), "F11");
        let keys = ["Control", "Shift", "n"].map(String::from);
        assert_eq!(shortcut_label(&keys), "Shift Ctrl N");
    }
}
