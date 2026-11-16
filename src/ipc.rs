//! A socket other programs use to tell the running compositor what to do, such as
//! the panel's button that opens the overview: `nekoland msg overview`.

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

use smithay::reexports::calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction};

use crate::CalloopData;

const SOCKET_ENV: &str = "NEKOLAND_SOCKET";
// Longer than any command; a client that sends more is cut off.
const MAX_MESSAGE: u64 = 256;

fn socket_path(display: &std::ffi::OsStr) -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    Some(runtime.join(format!("nekoland-{}.sock", display.to_string_lossy())))
}

/// Starts listening, and tells the programs started from here where.
pub fn listen(handle: &LoopHandle<'static, CalloopData>, display: &std::ffi::OsStr) -> std::io::Result<()> {
    let path = socket_path(display).ok_or(std::io::ErrorKind::NotFound)?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    std::env::set_var(SOCKET_ENV, &path);

    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, data| {
                while let Ok((stream, _)) = listener.accept() {
                    let mut message = String::new();
                    // A local client that stalls must not stall the compositor.
                    stream.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
                    if stream.take(MAX_MESSAGE).read_to_string(&mut message).is_ok() {
                        run(&mut data.state, message.trim());
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(())
}

fn run(state: &mut crate::Nekoland, message: &str) {
    match message {
        "overview" => state.toggle_overview(),
        "workspace-add" => {
            state.add_workspace();
            state.rebuild_overview();
        }
        "workspace-next" => state.switch_workspace_by(1),
        "workspace-previous" => state.switch_workspace_by(-1),
        "quit" => state.loop_signal.stop(),
        other => tracing::warn!("unknown message: {other}"),
    }
}

/// The client side: sends one message to the compositor this program was started from.
pub fn send(message: &str) -> std::io::Result<()> {
    let path = std::env::var_os(SOCKET_ENV).ok_or(std::io::ErrorKind::NotFound)?;
    UnixStream::connect(path)?.write_all(message.as_bytes())
}
