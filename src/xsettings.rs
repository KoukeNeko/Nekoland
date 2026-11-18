//! XSettings for X11 programs: the theme, icons, cursor, font and dark preference that GTK
//! programs on XWayland read from the X server, given by an `xsettingsd` Nekoland starts.
//! They are GNOME's settings, which Settings writes and Wayland programs read, so they are
//! what is read here; Settings and the dark switches say when they changed.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicI32, Ordering},
        Mutex,
    },
};

const INTERFACE: &str = "org.gnome.desktop.interface";
const WM_PREFERENCES: &str = "org.gnome.desktop.wm.preferences";
const PREFER_DARK: &str = "prefer-dark";
const BASE_DPI: f64 = 96.0;
/// Xft/DPI is in 1024ths of a dot per inch.
const DPI_UNIT: f64 = 1024.0;

/// xsettingsd's process, once it runs, to tell it that its file changed.
static DAEMON: AtomicI32 = AtomicI32::new(0);
/// Gathering and writing the file is one at a time, so that the last to ask has the last say.
static WRITING: Mutex<()> = Mutex::new(());

#[derive(Default, PartialEq, Debug)]
struct Values {
    theme: String,
    icons: String,
    cursor: String,
    cursor_size: i64,
    font: String,
    buttons: String,
    dark: bool,
    text_scale: f64,
}

fn gsetting(schema: &str, key: &str) -> String {
    Command::new("gsettings")
        .args(["get", schema, key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().trim_matches('\'').to_string())
        .unwrap_or_default()
}

fn read() -> Values {
    Values {
        theme: gsetting(INTERFACE, "gtk-theme"),
        icons: gsetting(INTERFACE, "icon-theme"),
        cursor: gsetting(INTERFACE, "cursor-theme"),
        cursor_size: gsetting(INTERFACE, "cursor-size").parse().unwrap_or_default(),
        font: gsetting(INTERFACE, "font-name"),
        buttons: gsetting(WM_PREFERENCES, "button-layout"),
        dark: gsetting(INTERFACE, "color-scheme") == PREFER_DARK,
        text_scale: gsetting(INTERFACE, "text-scaling-factor").parse().unwrap_or(1.0),
    }
}

/// xsettingsd's file for `values`; what is not set is left out, for the program's own.
fn render(values: &Values) -> String {
    let mut lines = Vec::new();
    for (name, value) in [
        ("Net/ThemeName", &values.theme),
        ("Net/IconThemeName", &values.icons),
        ("Gtk/CursorThemeName", &values.cursor),
        ("Gtk/FontName", &values.font),
        ("Gtk/DecorationLayout", &values.buttons),
    ] {
        if !value.is_empty() {
            lines.push(format!("{name} \"{}\"", value.replace(['"', '\\'], "")));
        }
    }
    if values.cursor_size > 0 {
        lines.push(format!("Gtk/CursorThemeSize {}", values.cursor_size));
    }
    lines.push(format!("Gtk/ApplicationPreferDarkTheme {}", i32::from(values.dark)));
    lines.push("Xft/Antialias 1".into());
    lines.push("Xft/Hinting 1".into());
    lines.push("Xft/HintStyle \"hintslight\"".into());
    lines.push("Xft/RGBA \"none\"".into());
    lines.push(format!("Xft/DPI {}", (BASE_DPI * DPI_UNIT * values.text_scale).round() as i64));
    lines.join("\n") + "\n"
}

/// Nekoland's own file, in the runtime folder: the person's xsettingsd, if they have one,
/// is not touched.
fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("nekoland-xsettings.conf"))
}

fn write() -> Option<PathBuf> {
    let path = path()?;
    if let Err(err) = std::fs::write(&path, render(&read())) {
        tracing::warn!("{}: {err}", path.display());
        return None;
    }
    Some(path)
}

/// Starts xsettingsd on the X display, which has to be in the environment by now. It is a
/// program of its own, so a computer without it runs X11 programs as they are.
pub fn start() {
    std::thread::spawn(|| {
        let _writing = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(path) = write() else {
            return;
        };
        match Command::new("xsettingsd")
            .arg("-c")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => DAEMON.store(i32::try_from(child.id()).unwrap_or_default(), Ordering::Relaxed),
            Err(err) => tracing::warn!("xsettingsd: X11 programs keep their own settings: {err}"),
        }
    });
}

/// Writes the settings again and tells xsettingsd, which publishes what changed.
pub fn sync() {
    if DAEMON.load(Ordering::Relaxed) == 0 {
        return;
    }
    std::thread::spawn(|| {
        let _writing = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if write().is_none() {
            return;
        }
        let daemon = DAEMON.load(Ordering::Relaxed);
        // SAFETY: a signal to the process this one started; if it is gone, the call fails.
        if daemon != 0 && unsafe { libc::kill(daemon, libc::SIGHUP) } != 0 {
            tracing::warn!("xsettingsd: {}", std::io::Error::last_os_error());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_names_what_is_set_and_leaves_out_the_rest() {
        let values = Values {
            theme: "Nekoland".into(),
            font: "Ubuntu Sans 11".into(),
            cursor_size: 24,
            dark: true,
            text_scale: 1.25,
            ..Default::default()
        };
        let text = render(&values);
        assert!(text.contains("Net/ThemeName \"Nekoland\"\n"));
        assert!(text.contains("Gtk/FontName \"Ubuntu Sans 11\"\n"));
        assert!(text.contains("Gtk/CursorThemeSize 24\n"));
        assert!(text.contains("Gtk/ApplicationPreferDarkTheme 1\n"));
        assert!(text.contains(&format!("Xft/DPI {}\n", 120 * 1024)));
        assert!(!text.contains("Net/IconThemeName") && !text.contains("Gtk/CursorThemeName"));
    }

    #[test]
    fn a_quote_in_a_name_cannot_end_its_string() {
        let values = Values { theme: "a\"b\\c".into(), text_scale: 1.0, ..Default::default() };
        assert!(render(&values).contains("Net/ThemeName \"abc\"\n"));
    }
}
