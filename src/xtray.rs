//! The X11 system tray, for programs that have no other: Wine's put the icons of Windows'
//! notification area there, and without a tray show them in a window of their own.
//!
//! Each icon is docked in a window of ours that is never drawn. Its picture goes to the menu
//! bar as a status item, and a click on that goes back to the icon, as if made on it there.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use smithay::{
    reexports::calloop::channel::Sender,
    utils::{Logical, Point, Rectangle},
};
use x11rb::{
    connection::Connection,
    protocol::{
        damage::{self, ConnectionExt as _},
        xproto::{
            AtomEnum, ButtonPressEvent, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
            ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat, KeyButMask, PropMode, VisualClass,
            WindowClass, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT,
        },
        Event,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
    CURRENT_TIME, NONE,
};

use crate::dbus::{self, StatusItem};

/// What the keys of the menu bar's items for docked icons start with.
const KEY_PREFIX: &str = "xembed:";
/// The icon's side in points, which the menu bar draws it at.
const ICON_POINTS: f64 = 18.0;
const REQUEST_DOCK: u32 = 0;
const XEMBED_EMBEDDED_NOTIFY: u32 = 0;
/// How soon after a click on an icon a menu it opens comes up.
const MENU_WITHIN: Duration = Duration::from_secs(2);

struct Tray {
    connection: RustConnection,
    root: u32,
    /// Each docked icon's window, and the window of ours it is in.
    icons: Mutex<HashMap<u32, u32>>,
    /// The side of the icons, in X's pixels.
    side: u16,
    /// When and where an icon was last clicked.
    clicked: Mutex<Option<(Instant, Point<f64, Logical>)>>,
    /// The menu that click opened, while it is shown.
    menu: Mutex<Option<u32>>,
}

static TRAY: OnceLock<Tray> = OnceLock::new();

/// Becomes the tray of X display `display`, whose programs draw at `scale` pixels a point.
pub fn start(display: u32, events: Sender<dbus::Event>, scale: f64) {
    std::thread::spawn(move || {
        if let Err(err) = serve(display, &events, scale) {
            tracing::warn!("X11 tray icons are shown in windows of their own: {err}");
        }
    });
}

/// Whether X window `window` is one of ours that an icon is docked in, which is not shown.
pub fn holds(window: u32) -> bool {
    TRAY.get().is_some_and(|tray| {
        tray.icons
            .lock()
            .unwrap()
            .values()
            .any(|&container| container == window)
    })
}

/// Whether the menu bar item `key` is a docked icon.
pub fn is_icon(key: &str) -> bool {
    key.starts_with(KEY_PREFIX)
}

/// Clicks `button` (1 for the left, 3 for the right) on the docked icon of item `key`, at `at`,
/// under it at the menu bar's bottom edge. The icon is moved there first, so that a menu it
/// opens at the pointer comes up below the menu bar.
pub fn click(key: &str, button: u8, at: Point<f64, Logical>, scale: f64) {
    let (Some(tray), Some(icon)) = (
        TRAY.get(),
        key.strip_prefix(KEY_PREFIX).and_then(|icon| icon.parse().ok()),
    ) else {
        return;
    };
    let Some(container) = tray.icons.lock().unwrap().get(&icon).copied() else {
        return;
    };
    *tray.clicked.lock().unwrap() = Some((Instant::now(), at));
    let half = (tray.side / 2) as i16;
    let (x, y) = ((at.x * scale).round() as i16, (at.y * scale).round() as i16);
    let held = match button {
        1 => KeyButMask::BUTTON1,
        3 => KeyButMask::BUTTON3,
        _ => KeyButMask::default(),
    };
    let sent = (|| -> Result<(), x11rb::errors::ConnectionError> {
        let connection = &tray.connection;
        let to = ConfigureWindowAux::new()
            .x(i32::from(x - half))
            .y(i32::from(y - half));
        connection.configure_window(container, &to)?;
        for (response_type, state, mask) in [
            (BUTTON_PRESS_EVENT, KeyButMask::default(), EventMask::BUTTON_PRESS),
            (BUTTON_RELEASE_EVENT, held, EventMask::BUTTON_RELEASE),
        ] {
            let event = ButtonPressEvent {
                response_type,
                detail: button,
                sequence: 0,
                time: CURRENT_TIME,
                root: tray.root,
                event: icon,
                child: NONE,
                root_x: x,
                root_y: y,
                event_x: half,
                event_y: half,
                state,
                same_screen: true,
            };
            connection.send_event(false, icon, mask, event)?;
        }
        connection.flush()
    })();
    if let Err(err) = sent {
        tracing::warn!("X11 tray: {err}");
    }
}

/// X window `window`, a menu or tooltip, came up at `area`. Right after a click on an icon,
/// one at the pointer is the menu that click opened; the icon's tooltip comes up elsewhere.
pub fn popup_shown(window: u32, area: Rectangle<i32, Logical>) {
    let Some(tray) = TRAY.get() else {
        return;
    };
    let mut clicked = tray.clicked.lock().unwrap();
    let Some((when, at)) = *clicked else {
        return;
    };
    // A menu may be moved a little to fit on the display.
    let near = area.to_f64();
    let near = Rectangle::new(near.loc - Point::from((2.0, 2.0)), near.size + (4.0, 4.0).into());
    if when.elapsed() < MENU_WITHIN && near.contains(at) {
        *clicked = None;
        *tray.menu.lock().unwrap() = Some(window);
    }
}

/// X window `window` went away.
pub fn popup_gone(window: u32) {
    if let Some(tray) = TRAY.get() {
        tray.menu.lock().unwrap().take_if(|menu| *menu == window);
    }
}

/// Closes the menu an icon opened, for a press at `at` that missed it. Its program sees no
/// presses outside its own windows, so the press is sent to the menu, which closes on one
/// outside it.
pub fn close_menu(at: Point<f64, Logical>, scale: f64) {
    let Some(tray) = TRAY.get() else {
        return;
    };
    let Some(menu) = tray.menu.lock().unwrap().take() else {
        return;
    };
    let (x, y) = ((at.x * scale).round() as i16, (at.y * scale).round() as i16);
    let sent = (|| -> Result<(), x11rb::errors::ConnectionError> {
        let connection = &tray.connection;
        for (response_type, state, mask) in [
            (BUTTON_PRESS_EVENT, KeyButMask::default(), EventMask::BUTTON_PRESS),
            (
                BUTTON_RELEASE_EVENT,
                KeyButMask::BUTTON1,
                EventMask::BUTTON_RELEASE,
            ),
        ] {
            let event = ButtonPressEvent {
                response_type,
                detail: 1,
                sequence: 0,
                time: CURRENT_TIME,
                root: tray.root,
                event: menu,
                child: NONE,
                root_x: x,
                root_y: y,
                event_x: -1,
                event_y: -1,
                state,
                same_screen: true,
            };
            connection.send_event(false, menu, mask, event)?;
        }
        connection.flush()
    })();
    if let Err(err) = sent {
        tracing::warn!("X11 tray: {err}");
    }
}

fn serve(display: u32, events: &Sender<dbus::Event>, scale: f64) -> Result<(), Box<dyn std::error::Error>> {
    let (connection, screen) = x11rb::connect(Some(&format!(":{display}")))?;
    let screen = connection.setup().roots[screen].clone();
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom)
    };
    let selection = atom(&format!("_NET_SYSTEM_TRAY_S{}", 0))?;
    let opcode = atom("_NET_SYSTEM_TRAY_OPCODE")?;
    let xembed = atom("_XEMBED")?;
    connection.damage_query_version(1, 1)?.reply()?;

    // Icons with a 32-bit visual can be drawn with their transparency, which the menu bar keeps.
    let argb = screen
        .allowed_depths
        .iter()
        .filter(|depth| depth.depth == 32)
        .flat_map(|depth| &depth.visuals)
        .find(|visual| visual.class == VisualClass::TRUE_COLOR)
        .map(|visual| visual.visual_id)
        .ok_or("no 32-bit visual")?;
    let colormap = connection.generate_id()?;
    connection.create_colormap(
        x11rb::protocol::xproto::ColormapAlloc::NONE,
        colormap,
        screen.root,
        argb,
    )?;

    let manager = connection.generate_id()?;
    connection.create_window(
        0,
        manager,
        screen.root,
        -1,
        -1,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )?;
    connection.change_property32(
        PropMode::REPLACE,
        manager,
        atom("_NET_SYSTEM_TRAY_VISUAL")?,
        AtomEnum::VISUALID,
        &[argb],
    )?;
    connection.change_property32(
        PropMode::REPLACE,
        manager,
        atom("_NET_SYSTEM_TRAY_ORIENTATION")?,
        AtomEnum::CARDINAL,
        &[0],
    )?;
    connection.set_selection_owner(manager, selection, CURRENT_TIME)?;
    if connection.get_selection_owner(selection)?.reply()?.owner != manager {
        return Err("another program is the tray".into());
    }
    let announce = ClientMessageEvent::new(
        32,
        screen.root,
        atom("MANAGER")?,
        [CURRENT_TIME, selection, manager, 0, 0],
    );
    connection.send_event(false, screen.root, EventMask::STRUCTURE_NOTIFY, announce)?;
    connection.flush()?;

    let tray = TRAY.get_or_init(|| Tray {
        connection,
        root: screen.root,
        icons: Mutex::new(HashMap::new()),
        side: (ICON_POINTS * scale).round() as u16,
        clicked: Mutex::new(None),
        menu: Mutex::new(None),
    });
    let connection = &tray.connection;
    let mut damages: HashMap<u32, u32> = HashMap::new();
    loop {
        match connection.wait_for_event()? {
            Event::ClientMessage(message) if message.type_ == opcode => {
                let [_, request, icon, ..] = message.data.as_data32();
                if request != REQUEST_DOCK || tray.icons.lock().unwrap().contains_key(&icon) {
                    continue;
                }
                match dock(tray, icon, argb, colormap, xembed) {
                    Ok(damage) => {
                        damages.insert(icon, damage);
                    }
                    Err(err) => tracing::warn!("X11 tray: could not dock {icon}: {err}"),
                }
            }
            Event::DamageNotify(damaged) => {
                connection.damage_subtract(damaged.damage, NONE, NONE)?;
                if let Some(item) = read_icon(tray, damaged.drawable) {
                    let _ = events.send(dbus::Event::Item(item));
                }
            }
            // Gone, or taken back by the program: its item goes too.
            Event::DestroyNotify(gone) => undock(tray, &mut damages, gone.window, events),
            Event::ReparentNotify(moved)
                if tray.icons.lock().unwrap().get(&moved.window) != Some(&moved.parent) =>
            {
                undock(tray, &mut damages, moved.window, events)
            }
            _ => {}
        }
    }
}

/// Puts `icon` in a window of ours and has its pictures reported; returns its damage.
fn dock(
    tray: &Tray,
    icon: u32,
    visual: u32,
    colormap: u32,
    xembed: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    let connection = &tray.connection;
    let container = connection.generate_id()?;
    let side = tray.side;
    connection.create_window(
        32,
        container,
        tray.root,
        0,
        0,
        side,
        side,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(colormap)
            .override_redirect(1),
    )?;
    // Known as ours before XWayland reports it mapped, so that it is never shown.
    tray.icons.lock().unwrap().insert(icon, container);
    connection.change_window_attributes(
        icon,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )?;
    connection.reparent_window(icon, container, 0, 0)?;
    connection.configure_window(
        icon,
        &ConfigureWindowAux::new()
            .width(u32::from(side))
            .height(u32::from(side)),
    )?;
    let embedded = ClientMessageEvent::new(
        32,
        icon,
        xembed,
        [CURRENT_TIME, XEMBED_EMBEDDED_NOTIFY, 0, container, 0],
    );
    connection.send_event(false, icon, EventMask::NO_EVENT, embedded)?;
    connection.map_window(icon)?;
    connection.map_window(container)?;
    let damage = connection.generate_id()?;
    connection.damage_create(damage, icon, damage::ReportLevel::NON_EMPTY)?;
    connection.flush()?;
    Ok(damage)
}

fn undock(tray: &Tray, damages: &mut HashMap<u32, u32>, icon: u32, events: &Sender<dbus::Event>) {
    let Some(container) = tray.icons.lock().unwrap().remove(&icon) else {
        return;
    };
    let connection = &tray.connection;
    if let Some(damage) = damages.remove(&icon) {
        let _ = connection.damage_destroy(damage);
    }
    // The icon goes back to the root before ours is destroyed, which would destroy it too.
    let _ = connection.reparent_window(icon, tray.root, 0, 0);
    let _ = connection.destroy_window(container);
    let _ = connection.flush();
    let _ = events.send(dbus::Event::ItemGone(format!("{KEY_PREFIX}{icon}")));
}

/// The menu bar's item for docked `icon`, with the picture it shows now.
fn read_icon(tray: &Tray, icon: u32) -> Option<StatusItem> {
    let connection = &tray.connection;
    let side = tray.side;
    let image = connection
        .get_image(ImageFormat::Z_PIXMAP, icon, 0, 0, side, side, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    // X's 32-bit pixels are BGRA in memory; status items' are ARGB. Without an alpha channel
    // the fourth byte is anything, and the picture is opaque.
    let opaque = image.depth != 32;
    let argb = image
        .data
        .chunks_exact(4)
        .flat_map(|pixel| [if opaque { 0xff } else { pixel[3] }, pixel[2], pixel[1], pixel[0]])
        .collect();
    // Wine names each of its windows after its program, as WM_CLASS.
    let class = connection
        .get_property(false, icon, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
        .ok()?
        .reply()
        .ok()?
        .value;
    let program = class.split(|&byte| byte == 0).next().unwrap_or_default();
    Some(StatusItem {
        key: format!("{KEY_PREFIX}{icon}"),
        id: String::from_utf8_lossy(program).into_owned(),
        icon_name: String::new(),
        icon_theme_path: String::new(),
        pixmap: Some((i32::from(side), i32::from(side), argb)),
        title: String::new(),
        passive: false,
        menu: Vec::new(),
    })
}
