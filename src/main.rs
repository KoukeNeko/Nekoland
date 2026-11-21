#![allow(irrefutable_let_patterns)]

mod appgrid;
mod arrange;
mod agenda;
mod clockbar;
mod clockmenu;
mod config;
mod control;
mod corners;
mod cursor;
mod dbus;
mod desktop;
mod dock;
mod fileinfo;
mod focus;
mod frame;
mod glass;
mod handlers;
mod hover;
mod icc;
mod ime;

mod grabs;
mod input;
mod ipc;
mod lock;
mod menubar;
mod motion;
mod nautilus;
mod fps;
mod identify;
mod night;
mod notices;
mod notification_center;
mod overview;
mod power;
mod privacy;
mod render;
mod rounded;
mod screencopy;
mod snap_layouts;
mod sprite;
mod switcher;
mod widgets;
mod stacks;
mod state;
mod system;
mod text;
mod udev;
mod windows;
mod winit;
mod workspaces;
mod x11;
mod xsettings;

use smithay::reexports::{
    calloop::EventLoop,
    wayland_server::{Display, DisplayHandle},
};
pub use state::Nekoland;

/// Where a nested Nekoland keeps the outer session's X display, which XWayland's replaces;
/// `shell/xrdp-clipboard.sh` reads it.
const HOST_DISPLAY: &str = "NEKOLAND_HOST_DISPLAY";

/// Where the kernel lists the GPUs; one that can draw has a render node, `renderD128` and on.
const DRI_DIRECTORY: &str = "/dev/dri";
const RENDER_NODE_PREFIX: &str = "renderD";
const GTK_RENDERER: &str = "GSK_RENDERER";
const GTK_RENDERER_WITHOUT_GL: &str = "cairo";
const SESSION_TYPE: &str = "XDG_SESSION_TYPE";
const SESSION_TYPE_WAYLAND: &str = "wayland";

/// Without a GPU, GTK 4 draws with OpenGL on llvmpipe, whose buffers and compiled shaders
/// cost each program of it some 150 MB more than Cairo does (243 MB against 89 MB for the
/// task manager on a Hyper-V VM). A choice already made is kept.
fn draw_gtk_without_gl_when_gpuless() {
    if std::env::var_os(GTK_RENDERER).is_none() && !has_render_node() {
        std::env::set_var(GTK_RENDERER, GTK_RENDERER_WITHOUT_GL);
    }
}

fn has_render_node() -> bool {
    std::fs::read_dir(DRI_DIRECTORY).is_ok_and(|nodes| {
        nodes
            .flatten()
            .any(|node| node.file_name().to_string_lossy().starts_with(RENDER_NODE_PREFIX))
    })
}

pub struct CalloopData {
    state: Nekoland,
    display_handle: DisplayHandle,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `nekoland msg WORD` talks to the compositor that is already running.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let [command, message] = arguments.as_slice() {
        if command == "msg" {
            return Ok(ipc::send(message)?);
        }
    }

    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    // Logging in on a seat gives the session CAP_WAKE_ALARM as an ambient capability, which
    // everything started from here would inherit; bubblewrap, which the image loaders run in,
    // will not start with any, so no icon or picture could be read.
    // SAFETY: only clears this process's ambient set.
    if unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0) } != 0 {
        tracing::warn!("could not clear the ambient capabilities: {}", std::io::Error::last_os_error());
    }

    // The Mac thickens the outlines of all its text. FreeType can do so only for CFF fonts,
    // which Noto Sans CJK is: here by 0.4px at any stem width, where its own curve leaves
    // stems as wide as those at 2x alone. Latin is set heavier instead; see
    // text::TEXT_WEIGHT. FreeType reads this as it starts, here and in what starts here.
    if std::env::var_os("FREETYPE_PROPERTIES").is_none() {
        std::env::set_var(
            "FREETYPE_PROPERTIES",
            "cff:no-stem-darkening=0 cff:darkening-parameters=500,400,1000,400,3000,400,8000,400",
        );
    }
    draw_gtk_without_gl_when_gpuless();

    let mut event_loop: EventLoop<'static, CalloopData> = EventLoop::try_new()?;

    let display: Display<Nekoland> = Display::new()?;
    let display_handle = display.handle();
    let mut state = Nekoland::new(&mut event_loop, display);
    // For the lock's preview in Settings as much as for the lock.
    state.draw_lock_shortcuts();

    let mut data = CalloopData {
        state,
        display_handle,
    };

    // The menu bar's application menus and status items come over D-Bus, on a thread.
    let (dbus_events, dbus_channel) = smithay::reexports::calloop::channel::channel();
    event_loop
        .handle()
        .insert_source(dbus_channel, |event, _, data| {
            if let smithay::reexports::calloop::channel::Event::Msg(event) = event {
                data.state.notice_event(&event);
                data.state.menu_bar_event(event);
            }
        })
        .map_err(|err| err.error)?;
    system::watch_sound(dbus_events.clone());
    system::watch_orientation(dbus_events.clone());
    privacy::watch_devices(dbus_events.clone());
    data.state.events = Some(dbus_events.clone());
    data.state.menu_bar.bus = dbus::start(dbus_events)
        .inspect_err(|err| tracing::warn!("the menu bar has no D-Bus: {err}"))
        .ok();

    // Inside another session, be a window in it; on a bare console, drive the display.
    let nested = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    // An xrdp session is Nekoland's own, though it is drawn in the X server xrdp started.
    let owns_session = !nested || data.state.remote;
    if owns_session {
        // Portals pick their backends by the desktop's name.
        std::env::set_var("XDG_CURRENT_DESKTOP", "Nekoland");
        // A session logged into on a console is "tty" to logind, and Chrome, Electron and
        // others choose X11 then, as XWayland's DISPLAY is there too: drawn through XWayland,
        // Chrome has no window buttons and its menus' shadows take glass.
        std::env::set_var(SESSION_TYPE, SESSION_TYPE_WAYLAND);
    }
    if nested {
        // XWayland takes DISPLAY; the clipboard of xrdp's client is on this one.
        if let Some(host) = std::env::var_os("DISPLAY") {
            std::env::set_var(HOST_DISPLAY, host);
        }
        crate::winit::init_winit(&mut event_loop, &mut data)?;
    } else {
        crate::udev::init_udev(&mut event_loop, &mut data)?;
    }

    let command = match arguments.as_slice() {
        [flag, command] if flag == "-c" || flag == "--command" => Some(command.clone()),
        _ => None,
    };

    if let Err(err) = ipc::listen(&event_loop.handle(), &data.state.socket_name) {
        tracing::warn!("no control socket: {err}");
    }
    lock::watch_idle(&event_loop.handle());
    text::watch_applications();
    // So that what is started from here finds `nekoland msg`.
    if let Some(directory) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(std::path::Path::parent)
    {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(
            std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&path)),
        )?;
        std::env::set_var("PATH", joined);
        // GTK's submenus open beside their menus, as the Mac's do; see menus/src/lib.rs.
        let menus = directory.join("libnekoland_menus.so");
        if menus.exists() {
            let mut preload = menus.into_os_string();
            if let Some(others) = std::env::var_os("LD_PRELOAD").filter(|others| !others.is_empty()) {
                preload.push(" ");
                preload.push(others);
            }
            std::env::set_var("LD_PRELOAD", preload);
        }
    }

    // X11 programs find XWayland through the environment, so everything else starts after it.
    x11::start(&event_loop.handle(), &mut data, move |data| {
        // Services D-Bus starts, such as the portal that tells programs whether to be dark,
        // have to find this display; a nested compositor leaves the outer session's alone.
        if owns_session {
            let renderer = std::env::var_os(GTK_RENDERER).map(|_| GTK_RENDERER);
            let updated = std::process::Command::new("dbus-update-activation-environment")
                .args([
                    "--systemd",
                    "WAYLAND_DISPLAY",
                    "DISPLAY",
                    "XDG_CURRENT_DESKTOP",
                    SESSION_TYPE,
                    "LD_PRELOAD",
                    "FREETYPE_PROPERTIES",
                ])
                .args(renderer)
                .status();
            if !updated.as_ref().is_ok_and(|status| status.success()) {
                tracing::warn!("D-Bus services will not find this display: {updated:?}");
            }
            // The menu bar's question of dark or light started the portal before it could know
            // the desktop's name, and it picks its backends once, as it starts: the file chooser
            // would be GTK's.
            config::spawn("systemctl --user try-restart xdg-desktop-portal.service");
        }
        if data.state.remote {
            // shortcut: text only, and xrdp's side is looked at twice a second; bridge the
            // selections in the compositor once other kinds of content are wanted across.
            config::spawn(include_str!("../shell/xrdp-clipboard.sh"));
        }
        // The launcher's clipboard history; it ends with the display it watches.
        config::spawn("wl-paste --type text --watch nekoland-launcher --remember-clipboard");
        // The screen reader, when Settings' 協助工具 turned it on, as GNOME starts it.
        config::spawn(
            "[ \"$(gsettings get org.gnome.desktop.a11y.applications screen-reader-enabled 2>/dev/null)\" = true ] && exec orca",
        );
        // Nekoland comes with no web browser: Market asks for one until there is one or the
        // person skips it, and otherwise ends at once.
        config::spawn("nekoland-store --choose-browser");
        let config = &data.state.config;
        config.autostart.iter().for_each(|command| config::spawn(command));
        let wallpaper = config::start_wallpaper(
            data.state.desktop_wallpaper(data.state.workspaces.active).as_deref(),
            config.wallpaper_mode,
        );
        match &command {
            Some(command) => config::spawn(command),
            // Something to start from, when nothing else was asked for.
            None if config.autostart.is_empty() => config::spawn(&config.terminal),
            None => {}
        }
        data.state.wallpaper = wallpaper;
    });

    event_loop.run(None, &mut data, move |_| {
        // Nekoland is running
    })?;

    Ok(())
}
