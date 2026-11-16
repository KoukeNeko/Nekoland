#![allow(irrefutable_let_patterns)]

mod config;
mod cursor;
mod focus;
mod handlers;

mod grabs;
mod input;
mod ipc;
mod overview;
mod render;
mod state;
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

    let mut event_loop: EventLoop<'static, CalloopData> = EventLoop::try_new()?;

    let display: Display<Nekoland> = Display::new()?;
    let display_handle = display.handle();
    let state = Nekoland::new(&mut event_loop, display);

    let mut data = CalloopData {
        state,
        display_handle,
    };

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
    }

    // X11 programs find XWayland through the environment, so everything else starts after it.
    x11::start(&event_loop.handle(), &mut data, move |data| {
        // Services D-Bus starts, such as the portal that tells programs whether to be dark,
        // have to find this display; a nested compositor leaves the outer session's alone.
        if !nested {
            let updated = std::process::Command::new("dbus-update-activation-environment")
                .args(["--systemd", "WAYLAND_DISPLAY", "DISPLAY", "XDG_CURRENT_DESKTOP"])
                .status();
            if !updated.as_ref().is_ok_and(|status| status.success()) {
                tracing::warn!("D-Bus services will not find this display: {updated:?}");
            }
        }
        let config = &data.state.config;
        config.autostart.iter().for_each(|command| config::spawn(command));
        match &command {
            Some(command) => config::spawn(command),
            // Something to start from, when nothing else was asked for.
            None if config.autostart.is_empty() => config::spawn(&config.terminal),
            None => {}
        }
    });

    event_loop.run(None, &mut data, move |_| {
        // Nekoland is running
    })?;

    Ok(())
}
