//! XSettings for X11 programs: the theme, icons, cursor, font and dark preference that GTK
//! programs on XWayland read from the X server, given by an `xsettingsd` Nekoland starts.
//! They are GNOME's settings, which Settings writes and Wayland programs read, so they are
//! what is read here; Settings and the dark switches say when they changed.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering},
        Mutex,
    },
};

const INTERFACE: &str = "org.gnome.desktop.interface";
const WM_PREFERENCES: &str = "org.gnome.desktop.wm.preferences";
const PREFER_DARK: &str = "prefer-dark";
const BASE_DPI: f64 = 96.0;
/// Xft/DPI is in 1024ths of a dot per inch.
const DPI_UNIT: f64 = 1024.0;

/// How many dots X11 programs draw a point with, as the bits of an f64: the display's scale,
/// which XWayland is told, so that they draw sharp rather than being enlarged.
static SCALE: AtomicU64 = AtomicU64::new(0x3ff0_0000_0000_0000);

/// The DPI last written, to tell Windows' programs when it changed.
static LAST_DPI: AtomicI64 = AtomicI64::new(0);

/// xsettingsd's process, once it runs, to tell it that its file changed.
static DAEMON: AtomicI32 = AtomicI32::new(0);
/// Gathering and writing the file is one at a time, so that the last to ask has the last say.
static WRITING: Mutex<()> = Mutex::new(());
/// Whether GTK's programs are in Nekoland's look, as of the last time the settings were read.
/// The GTK 3 theme Settings chooses while Nekoland's look is on.
const NEKOLAND_THEME: &str = "Nekoland";
static NEKOLAND_LOOK: AtomicBool = AtomicBool::new(true);
/// Whether Nekoland chooses Qt's style in this session: Kvantum, drawing the look's theme,
/// while the look is on.
static QT_STYLE: AtomicBool = AtomicBool::new(false);
const QT_STYLE_OVERRIDE: &str = "QT_STYLE_OVERRIDE";
const KVANTUM: &str = "kvantum";
/// Kvantum's theme of the look, installed with Nekoland; its dark one is this with "Dark".
const KVANTUM_THEME: &str = "Nekoland";

/// Whether the look Settings gives GTK's programs is on, for what the compositor draws as
/// they would.
pub fn is_nekoland_look() -> bool {
    NEKOLAND_LOOK.load(Ordering::Relaxed)
}

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
    /// The display's scale, which X11 programs draw at; 0 is taken for 1.
    scale: f64,
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
        scale: f64::from_bits(SCALE.load(Ordering::Relaxed)),
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
    let scale = values.scale.max(1.0);
    if values.cursor_size > 0 {
        lines.push(format!("Gtk/CursorThemeSize {}", (values.cursor_size as f64 * scale).round() as i64));
    }
    lines.push(format!("Gtk/ApplicationPreferDarkTheme {}", i32::from(values.dark)));
    lines.push("Xft/Antialias 1".into());
    lines.push("Xft/Hinting 1".into());
    lines.push("Xft/HintStyle \"hintslight\"".into());
    lines.push("Xft/RGBA \"none\"".into());
    let dpi = BASE_DPI * DPI_UNIT * values.text_scale * scale;
    lines.push(format!("Xft/DPI {}", dpi.round() as i64));
    // GTK 3 enlarges its widgets by whole times only, and its text by the rest.
    let times = scale.floor();
    lines.push(format!("Gdk/WindowScalingFactor {times}"));
    lines.push(format!("Gdk/UnscaledDPI {}", (dpi / times).round() as i64));
    lines.join("\n") + "\n"
}

/// The X resources of the cursor, for X11 programs that take its size from them rather than
/// from XSettings, as Chromium's and Electron's do.
fn cursor_resources(values: &Values) -> Option<String> {
    if values.cursor_size <= 0 {
        return None;
    }
    let size = (values.cursor_size as f64 * values.scale.max(1.0)).round() as i64;
    let mut text = format!("Xcursor.size: {size}\n");
    if !values.cursor.is_empty() {
        text += &format!("Xcursor.theme: {}\n", values.cursor.replace(['\n', '\\'], ""));
    }
    Some(text)
}

/// Merges `resources` into the X server's, for programs started from now on.
fn merge_resources(resources: &str) {
    use std::io::Write;
    let merged = Command::new("xrdb")
        .args(["-nocpp", "-merge"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().expect("piped").write_all(resources.as_bytes())?;
            child.wait()
        });
    match merged {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!("xrdb: {status}"),
        Err(err) => tracing::warn!("xrdb: X11 programs keep their own cursor size: {err}"),
    }
}

/// Nekoland's own file, in the runtime folder: the person's xsettingsd, if they have one,
/// is not touched.
fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("nekoland-xsettings.conf"))
}

/// Has Qt's programs drawn in the look while it is on, in a session that is Nekoland's and
/// whose person chose no Qt style of their own. Programs started from now on take it.
pub fn choose_qt_style() {
    if std::env::var_os(QT_STYLE_OVERRIDE).is_some() {
        return;
    }
    QT_STYLE.store(true, Ordering::Relaxed);
    if gsetting(INTERFACE, "gtk-theme") == NEKOLAND_THEME {
        std::env::set_var(QT_STYLE_OVERRIDE, KVANTUM);
    }
}

/// Gives a program started from Nekoland the Qt style of the look as it is now, which may
/// have been switched since the session started.
pub fn set_qt_style(command: &mut Command) {
    if !QT_STYLE.load(Ordering::Relaxed) {
        return;
    }
    if is_nekoland_look() {
        command.env(QT_STYLE_OVERRIDE, KVANTUM);
    } else {
        command.env_remove(QT_STYLE_OVERRIDE);
    }
}

/// Points Kvantum at the look's theme, light or dark, unless the person chose a theme of
/// their own there.
fn choose_kvantum_theme(dark: bool) {
    let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))
    else {
        return;
    };
    let path = config.join("Kvantum/kvantum.kvconfig");
    let theme = if dark { format!("{KVANTUM_THEME}Dark") } else { KVANTUM_THEME.to_string() };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => match kvantum_config(&text, &theme) {
            Some(text) => text,
            None => return,
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => format!("[General]\ntheme={theme}\n"),
        Err(err) => {
            tracing::warn!("{}: {err}", path.display());
            return;
        }
    };
    let written = std::fs::create_dir_all(config.join("Kvantum")).and_then(|()| std::fs::write(&path, text));
    if let Err(err) = written {
        tracing::warn!("{}: {err}", path.display());
    }
}

/// Kvantum's file `text` with the look's `theme` in place of the look's other one; nothing
/// when it already names `theme`, or names one that is not the look's.
fn kvantum_config(text: &str, theme: &str) -> Option<String> {
    let current = text.lines().find_map(|line| line.trim().strip_prefix("theme="))?.trim();
    let ours = [KVANTUM_THEME.to_string(), format!("{KVANTUM_THEME}Dark")];
    if current == theme || !ours.iter().any(|name| name == current) {
        return None;
    }
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            if line.trim().starts_with("theme=") {
                format!("theme={theme}")
            } else {
                line.to_string()
            }
        })
        .collect();
    Some(lines.join("\n") + "\n")
}

fn write() -> Option<PathBuf> {
    let path = path()?;
    let values = read();
    let look = values.theme == NEKOLAND_THEME;
    NEKOLAND_LOOK.store(look, Ordering::Relaxed);
    if look && QT_STYLE.load(Ordering::Relaxed) {
        choose_kvantum_theme(values.dark);
    }
    let text = render(&values);
    if let Some(resources) = cursor_resources(&values) {
        merge_resources(&resources);
    }
    if let Err(err) = std::fs::write(&path, &text) {
        tracing::warn!("{}: {err}", path.display());
        return None;
    }
    // Wine's programs keep their DPI in each bottle's registry, which Settings sets from
    // this file.
    let dpi = text.lines().find_map(|line| line.strip_prefix("Xft/DPI ")).and_then(|dpi| dpi.parse().ok());
    if let Some(dpi) = dpi.filter(|dpi| LAST_DPI.swap(*dpi, Ordering::Relaxed) != *dpi) {
        tracing::info!("X11 programs draw at {} DPI", dpi / DPI_UNIT as i64);
        crate::config::spawn("nekoland-settings --wine-dpi");
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

/// How many dots X11 programs draw a point with.
pub fn scale() -> f64 {
    f64::from_bits(SCALE.load(Ordering::Relaxed)).max(1.0)
}

/// Has X11 programs draw at `scale`, the display's, which XWayland was told.
pub fn set_scale(scale: f64) {
    SCALE.store(scale.to_bits(), Ordering::Relaxed);
    sync();
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
    fn kvantum_takes_the_looks_theme_only_in_place_of_the_look() {
        assert_eq!(
            kvantum_config("[General]\ntheme=Nekoland\n\n[Applications]\nKvFlat=vlc\n", "NekolandDark")
                .as_deref(),
            Some("[General]\ntheme=NekolandDark\n\n[Applications]\nKvFlat=vlc\n")
        );
        assert_eq!(
            kvantum_config("[General]\ntheme=NekolandDark\n", "Nekoland").as_deref(),
            Some("[General]\ntheme=Nekoland\n")
        );
        assert_eq!(kvantum_config("[General]\ntheme=Nekoland\n", "Nekoland"), None);
        assert_eq!(kvantum_config("[General]\ntheme=KvFlat\n", "Nekoland"), None);
        assert_eq!(kvantum_config("[General]\ntheme=NekolandFoo\n", "Nekoland"), None);
        assert_eq!(kvantum_config("[Applications]\nKvFlat=vlc\n", "Nekoland"), None);
    }

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
    fn the_cursor_resources_are_as_large_as_its_xsettings() {
        let values = Values {
            cursor: "WhiteSur-cursors".into(),
            cursor_size: 24,
            scale: 1.5,
            ..Default::default()
        };
        assert_eq!(
            cursor_resources(&values).as_deref(),
            Some("Xcursor.size: 36\nXcursor.theme: WhiteSur-cursors\n")
        );
        assert_eq!(cursor_resources(&Values::default()), None);
    }

    #[test]
    fn at_twice_the_density_x11_programs_draw_twice_as_large() {
        let values = Values { cursor_size: 24, text_scale: 1.25, scale: 2.0, ..Default::default() };
        let text = render(&values);
        assert!(text.contains("Gtk/CursorThemeSize 48\n"));
        assert!(text.contains(&format!("Xft/DPI {}\n", 240 * 1024)));
        assert!(text.contains("Gdk/WindowScalingFactor 2\n"));
        assert!(text.contains(&format!("Gdk/UnscaledDPI {}\n", 120 * 1024)));
    }

    #[test]
    fn a_quote_in_a_name_cannot_end_its_string() {
        let values = Values { theme: "a\"b\\c".into(), text_scale: 1.0, ..Default::default() };
        assert!(render(&values).contains("Net/ThemeName \"abc\"\n"));
    }
}
