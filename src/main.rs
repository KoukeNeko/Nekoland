#![allow(irrefutable_let_patterns)]

mod appgrid;
mod config;
mod cursor;
mod dbus;
mod dock;
mod focus;
mod glass;
mod handlers;
mod ime;

mod grabs;
mod input;
mod ipc;
mod lock;
mod menubar;
mod notices;
mod overview;
mod render;
mod screencopy;
mod stacks;
mod state;
mod system;
mod text;
mod udev;
mod windows;
mod winit;
mod workspaces;
mod x11;

use smithay::reexports::{
    calloop::EventLoop,
    wayland_server::{Display, DisplayHandle},
};
pub use state::Nekoland;

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

    let mut event_loop: EventLoop<'static, CalloopData> = EventLoop::try_new()?;

    let display: Display<Nekoland> = Display::new()?;
    let display_handle = display.handle();
    let state = Nekoland::new(&mut event_loop, display);

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
    data.state.events = Some(dbus_events.clone());
    data.state.menu_bar.bus = dbus::start(dbus_events)
        .inspect_err(|err| tracing::warn!("the menu bar has no D-Bus: {err}"))
        .ok();

    // Inside another session, be a window in it; on a bare console, drive the display.
    let nested = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if nested {
        crate::winit::init_winit(&mut event_loop, &mut data)?;
    } else {
        // Portals pick their backends by the desktop's name.
        std::env::set_var("XDG_CURRENT_DESKTOP", "Nekoland");
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
        if !nested {
            let updated = std::process::Command::new("dbus-update-activation-environment")
                .args([
                    "--systemd",
                    "WAYLAND_DISPLAY",
                    "DISPLAY",
                    "XDG_CURRENT_DESKTOP",
                    "LD_PRELOAD",
                    "FREETYPE_PROPERTIES",
                ])
                .status();
            if !updated.as_ref().is_ok_and(|status| status.success()) {
                tracing::warn!("D-Bus services will not find this display: {updated:?}");
            }
        }
        let config = &data.state.config;
        config.autostart.iter().for_each(|command| config::spawn(command));
        let wallpaper = config::start_wallpaper(config);
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
