//! A socket other programs use to tell the running compositor what to do, such as
//! the panel's button that opens the overview: `nekoland msg overview`.

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

use smithay::reexports::calloop::{
    generic::Generic,
    timer::{TimeoutAction, Timer},
    Interest, LoopHandle, Mode, PostAction,
};

use crate::{
    windows::{is_managed, window_names, LAUNCHER_APP_ID},
    CalloopData,
};

const SOCKET_ENV: &str = "NEKOLAND_SOCKET";
// Longer than any command, a list of what the hover panel leaves out among them; a client
// that sends more is dropped.
const MAX_MESSAGE: u64 = 16 << 10;
/// How long what the launcher asks of the window under it waits for the launcher to have
/// gone and that window to have the keyboard again.
const AFTER_LAUNCHER: std::time::Duration = std::time::Duration::from_millis(150);

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

    let clients = handle.clone();
    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, _| {
                while let Ok((stream, _)) = listener.accept() {
                    if let Err(err) = read_client(&clients, stream) {
                        tracing::warn!("ipc client: {err}");
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(())
}

/// Reads a client's message as it arrives, so one that is slow holds up nothing else, and
/// does what it asks once the client has sent it all.
fn read_client(handle: &LoopHandle<'static, CalloopData>, stream: UnixStream) -> std::io::Result<()> {
    stream.set_nonblocking(true)?;
    let mut message = Vec::new();
    let later = handle.clone();
    handle
        .insert_source(Generic::new(stream, Interest::READ, Mode::Level), move |_, stream, data| {
            let mut chunk = [0; MAX_MESSAGE as usize];
            loop {
                match (&**stream).read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) if message.len() + read <= MAX_MESSAGE as usize => {
                        message.extend_from_slice(&chunk[..read]);
                    }
                    Ok(_) => return Ok(PostAction::Remove),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Ok(PostAction::Remove),
                }
            }
            let message = String::from_utf8_lossy(&message);
            if let Some(command) = message.trim().strip_prefix("after-launcher ") {
                after_launcher(&later, &mut data.state, command.to_string());
                return Ok(PostAction::Remove);
            }
            if let Some(reply) = run(&mut data.state, message.trim()) {
                // A blocking write: replies are far smaller than the socket's buffer.
                let _ = stream.set_nonblocking(false);
                if let Err(err) = (&**stream).write_all(reply.as_bytes()) {
                    tracing::warn!("answering {message:?}: {err}");
                }
            }
            Ok(PostAction::Remove)
        })
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(())
}

/// `after-launcher COMMAND`: what the launcher asks of the window it was opened over, done
/// once the launcher has closed and that window has the keyboard back.
fn after_launcher(handle: &LoopHandle<'static, CalloopData>, state: &mut crate::Nekoland, command: String) {
    state.close_launcher_unless(None);
    let inserted = handle.insert_source(Timer::from_duration(AFTER_LAUNCHER), move |_, _, data| {
        let state = &mut data.state;
        match command.split_once(' ').unwrap_or((&command, "")) {
            ("paste", _) => state.paste_into_active(),
            ("menu-search", query) => state.search_menus(query),
            ("window", action) => {
                if !state.window_command(action) {
                    tracing::warn!("unknown window command: {action}");
                }
            }
            // `focus INDEX APP_ID`: one of the windows `windows` listed, if it is still there.
            ("focus", which) => {
                let (index, app_id) = which.split_once(' ').unwrap_or((which, ""));
                let window = index
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| launcher_windows(state).into_iter().nth(index))
                    .filter(|window| window_names(window).1 == app_id);
                if let Some(window) = window {
                    state.unminimize(&window);
                    state.focus_window(&window);
                }
            }
            _ => tracing::warn!("unknown message: after-launcher {command}"),
        }
        TimeoutAction::Drop
    });
    if let Err(err) = inserted {
        tracing::warn!("after-launcher: {err}");
    }
}

/// The windows the launcher can switch to, the topmost first and the minimized after.
fn launcher_windows(state: &crate::Nekoland) -> Vec<smithay::desktop::Window> {
    state
        .space
        .elements()
        .rev()
        .chain(state.minimized.iter().map(|(window, _)| window))
        .filter(|window| is_managed(window) && window_names(window).1 != LAUNCHER_APP_ID)
        .cloned()
        .collect()
}

/// Does what a message asks; a question gets its answer back.
fn run(state: &mut crate::Nekoland, message: &str) -> Option<String> {
    match message {
        // The configuration changed: the settings application wrote it.
        "reload" => state.reload_config(),
        // The output's mode, then every mode it has, one a line, as WIDTHxHEIGHT@HZ.
        "modes" => {
            let output = state.space.outputs().next()?;
            let name = |mode: smithay::output::Mode| {
                format!("{}x{}@{}", mode.size.w, mode.size.h, (mode.refresh + 500) / 1000)
            };
            let current = output.current_mode().map(name).unwrap_or_default();
            let mut lines = vec![current];
            for mode in output.modes() {
                let line = name(mode);
                if !lines[1..].contains(&line) {
                    lines.push(line);
                }
            }
            return Some(lines.join("\n") + "\n");
        }
        // The settings in effect, as TOML, with what is not in the file at its default.
        "config" => {
            return Some(toml::to_string(&state.config).unwrap_or_else(|err| format!("# {err}\n")));
        }
        // `set KEY = VALUE`: one setting changed, kept in the file, and put in effect.
        set if set.starts_with("set ") => {
            if let Err(err) = crate::config::set(&set["set ".len()..]) {
                return Some(format!("{err}\n"));
            }
            state.reload_config();
        }
        // `unset KEY`: one setting back at its default.
        unset if unset.starts_with("unset ") => {
            if let Err(err) = crate::config::unset(&unset["unset ".len()..]) {
                return Some(format!("{err}\n"));
            }
            state.reload_config();
        }
        // What the hover panel can leave out, and the panel for a kind's sample as a PNG.
        "hover fields" => return Some(crate::fileinfo::catalogue()),
        preview if preview.starts_with("hover preview ") => {
            return state.hover_preview(&preview["hover preview ".len()..]);
        }
        // The windows `after-launcher focus` can switch to, one a line: application ID,
        // a tab, and title.
        "windows" => {
            return Some(
                launcher_windows(state)
                    .iter()
                    .map(|window| {
                        let (title, app_id) = window_names(window);
                        format!("{app_id}\t{}\n", title.replace(['\t', '\n'], " "))
                    })
                    .collect(),
            );
        }
        "overview" => state.toggle_overview(),
        "lock" => state.lock_screen(),
        "sleep" => state.sleep(),
        "workspace-add" => {
            state.add_workspace();
            state.rebuild_overview();
        }
        "workspace-next" => state.switch_workspace_by(1),
        "workspace-previous" => state.switch_workspace_by(-1),
        "quit" => state.loop_signal.stop(),
        other => tracing::warn!("unknown message: {other}"),
    }
    None
}

/// The client side: sends one message to the compositor this program was started from,
/// and prints what it answers.
pub fn send(message: &str) -> std::io::Result<()> {
    let path = std::env::var_os(SOCKET_ENV).ok_or(std::io::ErrorKind::NotFound)?;
    let mut stream = UnixStream::connect(path)?;
    stream.write_all(message.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    print!("{reply}");
    Ok(())
}
