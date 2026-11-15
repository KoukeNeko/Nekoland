#![allow(irrefutable_let_patterns)]

mod handlers;

mod grabs;
mod input;
mod state;
mod udev;
mod winit;

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
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    let mut event_loop: EventLoop<CalloopData> = EventLoop::try_new()?;

    let display: Display<Nekoland> = Display::new()?;
    let display_handle = display.handle();
    let state = Nekoland::new(&mut event_loop, display);

    let mut data = CalloopData {
        state,
        display_handle,
    };

    // Inside another session, be a window in it; on a bare console, drive the display.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some() {
        crate::winit::init_winit(&mut event_loop, &mut data)?;
    } else {
        crate::udev::init_udev(&mut event_loop, &mut data)?;
    }

    let mut args = std::env::args().skip(1);
    let flag = args.next();
    let arg = args.next();

    match (flag.as_deref(), arg) {
        (Some("-c") | Some("--command"), Some(command)) => {
            std::process::Command::new(command).spawn().ok();
        }
        _ => {
            std::process::Command::new("foot").spawn().ok();
        }
    }

    event_loop.run(None, &mut data, move |_| {
        // Nekoland is running
    })?;

    Ok(())
}
