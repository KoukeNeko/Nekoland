//! Restarting with the sessions carried over, as Nekominal's "Update and restart": the program `exec`s the copy now
//! installed in its place, which takes over every pane. The ptys stay open through the `exec` and their shells are still
//! its children, so what runs in them keeps running; the windows, tabs, splits and the text of each pane are written to
//! a file for the new copy, named to it in `NEKOLAND_TERMINAL_CARRIED`.

use std::{
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

use gtk::{gio, glib, prelude::*};
use serde::{Deserialize, Serialize};

/// The variable that names the file of what was carried over to the new copy.
const CARRIED_VAR: &str = "NEKOLAND_TERMINAL_CARRIED";
const FOLDER: &str = "nekoland-terminal";
/// What the kernel adds to the program's path once the file was replaced, as an update does.
const REPLACED_SUFFIX: &str = " (deleted)";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedPane {
    /// The pty's descriptor, open through the `exec`, and the shell on it.
    pub master: i32,
    pub shell_pid: i32,
    /// Its id, which its programs know it by (`NEKOLAND_TERMINAL_PANE`).
    pub id: String,
    pub profile: String,
    pub directory: Option<PathBuf>,
    /// What it showed, its scrollback with it, the cursor's line last.
    pub text: String,
    /// A program other than the shell ran in front, to be told to draw itself again.
    pub busy: bool,
    /// The title its program gave it, which it would not give again until its next prompt.
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    Pane(CarriedPane),
    /// Two parts side by side (`vertical` false) or one above the other, the divider `position` points from the start.
    Split { vertical: bool, position: i32, start: Box<Layout>, end: Box<Layout> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedTab {
    pub title: Option<String>,
    pub colour: Option<String>,
    pub selected: bool,
    pub layout: Layout,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedWindow {
    pub tabs: Vec<CarriedTab>,
    pub drop_down: bool,
    pub visible: bool,
}

fn carried_file() -> PathBuf {
    glib::user_runtime_dir().join(FOLDER).join(format!("carried-{}.json", std::process::id()))
}

/// The program as installed now: the path it was started from, which an update put a new copy at.
fn installed_program() -> io::Result<PathBuf> {
    let running = std::env::current_exe()?;
    let text = running.to_string_lossy();
    Ok(text.strip_suffix(REPLACED_SUFFIX).map_or(running.clone(), PathBuf::from))
}

/// Whether a newer copy was installed over the running one.
pub fn is_replaced() -> bool {
    std::env::current_exe().is_ok_and(|running| running.to_string_lossy().ends_with(REPLACED_SUFFIX))
}

/// Writes what is carried and becomes the installed copy; returns only when that failed.
pub fn restart(windows: &[CarriedWindow]) -> io::Error {
    let file = carried_file();
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| serde_json::to_vec(windows).map_err(io::Error::other))
        .and_then(|text| std::fs::write(&file, text));
    if let Err(error) = written {
        return error;
    }
    let error = match installed_program() {
        Ok(program) => Command::new(program).env(CARRIED_VAR, &file).exec(),
        Err(error) => error,
    };
    remove(&file);
    error
}

fn remove(file: &Path) {
    if let Err(error) = std::fs::remove_file(file) {
        eprintln!("nekoland-terminal: could not remove {}: {error}", file.display());
    }
}

/// What the copy before this one carried over, once: None when it was started anew.
pub fn take() -> Option<Vec<CarriedWindow>> {
    let file = PathBuf::from(std::env::var_os(CARRIED_VAR)?);
    // The shells started from here are not to see it.
    std::env::remove_var(CARRIED_VAR);
    let carried = std::fs::read(&file).map_err(|error| error.to_string()).and_then(|text| {
        serde_json::from_slice::<Vec<CarriedWindow>>(&text).map_err(|error| error.to_string())
    });
    remove(&file);
    carried.inspect_err(|error| eprintln!("nekoland-terminal: the carried sessions could not be read: {error}")).ok()
}

/// Whether this copy was started to take over the sessions of the one before.
pub fn is_carrying() -> bool {
    std::env::var_os(CARRIED_VAR).is_some()
}

/// Waits a little for the copy before to have let go of the app's name on the bus, which goes as its connection closes
/// in the `exec`: holding it, this copy would hand its start to that one, which is no more, and end with every pane.
pub fn wait_until_released(app_id: &str) {
    const TRIES: u32 = 100;
    const BETWEEN: std::time::Duration = std::time::Duration::from_millis(20);
    let bus = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(bus) => bus,
        Err(error) => {
            eprintln!("nekoland-terminal: the session bus could not be reached: {error}");
            return;
        }
    };
    for _ in 0..TRIES {
        let owned = bus
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                Some(&(app_id,).to_variant()),
                Some(glib::VariantTy::new("(b)").expect("a valid type")),
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
            )
            .map(|reply| reply.get::<(bool,)>().is_some_and(|(owned,)| owned));
        match owned {
            Ok(false) => return,
            Ok(true) => std::thread::sleep(BETWEEN),
            Err(error) => {
                eprintln!("nekoland-terminal: could not ask the bus about {app_id}: {error}");
                return;
            }
        }
    }
}

/// What a pane is shown again with: the title its program gave it, then its lines in the screen's line breaks, the
/// cursor's line not ended.
pub fn screen_text(pane: &CarriedPane) -> String {
    let title = pane.title.as_deref().map(|title| format!("\x1b]2;{title}\x07")).unwrap_or_default();
    title + &pane.text.replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> CarriedPane {
        CarriedPane {
            master: 3,
            shell_pid: 10,
            id: id.into(),
            profile: "bash".into(),
            directory: None,
            text: "$ ls\na\n$ ".into(),
            busy: false,
            title: None,
        }
    }

    #[test]
    fn a_split_window_reads_back_as_written() {
        let layout = Layout::Split {
            vertical: true,
            position: 200,
            start: Box::new(Layout::Pane(pane("1-1"))),
            end: Box::new(Layout::Split {
                vertical: false,
                position: 90,
                start: Box::new(Layout::Pane(pane("1-2"))),
                end: Box::new(Layout::Pane(pane("1-3"))),
            }),
        };
        let window = CarriedWindow {
            tabs: vec![CarriedTab { title: Some("API".into()), colour: None, selected: true, layout }],
            drop_down: false,
            visible: true,
        };
        let text = serde_json::to_string(std::slice::from_ref(&window)).unwrap();
        assert_eq!(serde_json::from_str::<Vec<CarriedWindow>>(&text).unwrap(), [window]);
    }

    #[test]
    fn the_text_goes_back_with_the_screens_line_breaks_after_its_title() {
        assert_eq!(screen_text(&pane("1-1")), "$ ls\r\na\r\n$ ");
        let titled = CarriedPane { title: Some("vim".into()), ..pane("1-1") };
        assert_eq!(screen_text(&titled), "\x1b]2;vim\x07$ ls\r\na\r\n$ ");
    }
}
