//! A socket other programs use to tell the running compositor what to do, such as
//! the panel's button that opens the overview: `nekoland msg overview`.

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

use smithay::output::Output;
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

fn task_windows(state: &crate::Nekoland) -> String {
    let mut lines: Vec<String> = Vec::new();
    for window in state.all_windows().filter(|window| is_managed(window)) {
        let Some(pid) = state.window_pid(window) else {
            continue;
        };
        let answers = window
            .toplevel()
            .is_none_or(|toplevel| crate::handlers::xdg_shell::answers_pings(&toplevel.client()));
        let line = format!("{pid}\t{}\t{}\n", window_names(window).1, u8::from(answers));
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines.concat()
}

/// What follows `command` in `message`, empty when it is the command alone.
fn argument<'a>(message: &'a str, command: &str) -> Option<&'a str> {
    let rest = message.strip_prefix(command)?;
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix(' ')
    }
}

/// The display of that name, shown or not, or the main one for no name.
fn display_named(state: &crate::Nekoland, name: &str) -> Option<Output> {
    if name.is_empty() {
        return state.space.outputs().next().cloned();
    }
    state.connected_outputs.iter().chain(state.space.outputs()).find(|output| output.name() == name).cloned()
}

/// Does what a message asks; a question gets its answer back.
fn run(state: &mut crate::Nekoland, message: &str) -> Option<String> {
    if let Some(redraw) = &state.redraw {
        redraw.ping();
    }
    match message {
        // The configuration changed: the settings application wrote it.
        "reload" => state.reload_config(),
        // `action ID`: one of the shortcuts' actions, as its keys would do it: the same
        // actions, whatever program asks for them.
        action if argument(action, "action").is_some() => {
            let id = argument(action, "action")?;
            match shortcuts::action(id) {
                Some(action) => state.run_action(action, false),
                None => return Some(format!("no action: {id}\n")),
            }
        }
        // `key COMBO`: keys pressed for the window in front, as the on-screen keyboard sends.
        key if argument(key, "key").is_some() => {
            let text = argument(key, "key")?;
            match shortcuts::Combo::parse(text) {
                Some(combo) => state.send_combo(&combo),
                None => return Some(format!("no key: {text}\n")),
            }
        }
        // `calendar DATE`: the clock's panel opened on a day, given as 2026-12-25 or in words.
        day if day.starts_with("calendar ") => {
            let text = day["calendar ".len()..].trim();
            let today = calendar_data::Date::today();
            let (_, second_of_day) = calendar_data::date::local_parts(calendar_data::date::now());
            let hour_minute = (second_of_day / 3600, second_of_day / 60 % 60);
            // The first word of a sentence is its title, so a dummy one goes before the date.
            let found = calendar_data::Date::parse_key(text)
                .or_else(|| calendar_data::natural::parse(&format!("日 {text}"), today, &[], hour_minute).date);
            match found {
                Some(day) => state.show_clock_panel_on(day),
                None => return Some(format!("not a date: {text}\n")),
            }
        }
        // GNOME's settings changed, which X11 programs are given.
        "xsettings" => crate::xsettings::sync(),
        // The output's mode, then every mode it has, one a line, as WIDTHxHEIGHT@HZ; the main
        // display's, or with a name that display's.
        modes if argument(modes, "modes").is_some() => {
            let output = display_named(state, argument(modes, "modes")?)?;
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
        // Each size the output has a mode of, one a line, and the scale it is drawn at when
        // the settings name none: `WIDTHxHEIGHT SCALE`.
        scales if argument(scales, "scales").is_some() => {
            let output = display_named(state, argument(scales, "scales")?)?;
            let size_mm = output.physical_properties().size;
            let mut lines: Vec<String> = Vec::new();
            for mode in output.modes() {
                let scale = crate::udev::density_scale(
                    mode.size.w,
                    mode.size.h,
                    size_mm.w.max(0) as u32,
                    size_mm.h.max(0) as u32,
                );
                let line = format!("{}x{} {scale}\n", mode.size.w, mode.size.h);
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
            return Some(lines.concat());
        }
        // Every display, one a line, the main one first: its name, its size in points, where
        // its left and top edges are, and whether it is `on` or left dark by the projection.
        // Each display's number on it for a while, as Settings numbers them.
        "identify" => state.identify_displays(),
        "displays" => {
            let outputs: Vec<Output> = if state.connected_outputs.is_empty() {
                state.space.outputs().cloned().collect()
            } else {
                state.connected_outputs.clone()
            };
            return Some(
                outputs
                    .iter()
                    .map(|output| {
                        let size = crate::udev::logical_size(output);
                        let (place, on) = match state.space.output_geometry(output) {
                            Some(area) => (area.loc, "on"),
                            None => (output.current_location(), "off"),
                        };
                        format!("{}\t{}x{}\t{}\t{}\t{on}\n", output.name(), size.w, size.h, place.x, place.y)
                    })
                    .collect(),
            );
        }
        // The output as Settings shows it: `current` and `preferred` modes as WIDTHxHEIGHT@HZ,
        // the `scale` drawn with and the `density_scale` used when the settings name none.
        display if argument(display, "display").is_some() => {
            let output = display_named(state, argument(display, "display")?)?;
            let name = |mode: smithay::output::Mode| {
                format!("{}x{}@{}", mode.size.w, mode.size.h, (mode.refresh + 500) / 1000)
            };
            let preferred = output.preferred_mode().or_else(|| output.current_mode())?;
            let size_mm = output.physical_properties().size;
            let density = crate::udev::density_scale(
                preferred.size.w,
                preferred.size.h,
                size_mm.w.max(0) as u32,
                size_mm.h.max(0) as u32,
            );
            return Some(format!(
                "current {}\npreferred {}\nscale {}\ndensity_scale {density}\n",
                output.current_mode().map(name).unwrap_or_default(),
                name(preferred),
                output.current_scale().fractional_scale(),
            ));
        }
        // Today's sunset and sunrise where the time zone is, for Night Shift's schedule.
        "sun" => return Some(crate::night::sun_text() + "\n"),
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
        // The windows a screen share can show, the topmost first: the name
        // ext-foreign-toplevel-list gives it, its application ID and its title, tab between.
        "capturable-windows" => {
            return Some(
                launcher_windows(state)
                    .iter()
                    .filter_map(|window| {
                        let identifier = state.foreign_toplevels.identifier(window)?;
                        let (title, app_id) = window_names(window);
                        Some(format!("{identifier}\t{app_id}\t{}\n", title.replace(['\t', '\n'], " ")))
                    })
                    .collect(),
            );
        }
        // The windows on screen, the topmost first: `X Y WIDTH HEIGHT` in points, a tab and
        // the title. 截圖 takes a window by these.
        "window-areas" => {
            return Some(
                state
                    .space
                    .elements()
                    .rev()
                    .filter(|window| is_managed(window))
                    .filter_map(|window| {
                        let area = state.space.element_geometry(window)?;
                        let title = window_names(window).0.replace(['\t', '\n'], " ");
                        Some(format!("{} {} {} {}\t{title}\n", area.loc.x, area.loc.y, area.size.w, area.size.h))
                    })
                    .collect(),
            );
        }
        // The applications' windows, for the task manager, one a line: the process behind
        // it, its application ID, and `1` or `0` for whether it answers pings.
        "tasks" => return Some(task_windows(state)),
        quit if quit.starts_with("quit-app ") => state.quit_app(&quit["quit-app ".len()..]),
        force if force.starts_with("force-quit-app ") => {
            state.force_quit_app(&force["force-quit-app ".len()..]);
        }
        // For keys bound elsewhere, as Rectangle's are from a script: `window left`.
        action if action.starts_with("window ") => {
            let command = &action["window ".len()..];
            if !state.window_command(command) {
                return Some(format!("unknown window command: {command}\n"));
            }
        }
        "overview" => state.toggle_overview(),
        // As Super+A: the applications, or away with them.
        "apps" => state.show_apps(""),
        // As a click on the menu bar's clock: opens the Notification Center, or closes it.
        "notification-center" => state.toggle_notification_center(),
        "lock" => state.lock_screen(),
        open if open.starts_with("lock-open ") => state.open_from_lock(&open["lock-open ".len()..]),
        "sleep" => state.sleep(),
        "workspace-add" => {
            state.add_workspace();
            state.rebuild_overview();
        }
        "workspace-next" => state.switch_workspace_by(1),
        "workspace-previous" => state.switch_workspace_by(-1),
        "quit" => {
            // Said, so that a session that ended without it was ended from outside.
            tracing::info!("quitting: asked by `nekoland msg quit`");
            state.loop_signal.stop();
        }
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
