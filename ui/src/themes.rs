//! The icon and cursor themes installed where GTK and the compositor look for them.

// Settings uses only the names.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use gtk::glib;

pub struct Installed {
    pub name: String,
    pub path: PathBuf,
}

impl Installed {
    /// Put there by the system, not by the person, so not theirs to remove.
    pub fn system(&self) -> bool {
        !self.path.starts_with(glib::home_dir())
    }
}

/// Where a theme the person installs goes.
pub fn user_dir() -> PathBuf {
    glib::user_data_dir().join("icons")
}

/// What nekoland-conform-icons marks the icons it makes with.
pub const CONVERTED_MARK: &[u8] = b"<!-- conform-icons -->";

/// The icons made from other applications' own, in the person's half of Nekoland's theme;
/// there are some while Appearance's switch for them is on.
pub fn converted_icons() -> Vec<PathBuf> {
    use std::io::Read;
    let mut icons: Vec<PathBuf> = std::fs::read_dir(user_dir().join("Nekoland/scalable/apps"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .filter(|path| {
            let mut head = [0; 64];
            std::fs::File::open(path)
                .and_then(|mut file| file.read(&mut head))
                .is_ok_and(|read| head[..read].starts_with(CONVERTED_MARK))
        })
        .collect();
    icons.sort();
    icons
}

pub fn is_cursor_theme(path: &Path) -> bool {
    path.join("cursors").is_dir()
}

pub fn is_icon_theme(path: &Path) -> bool {
    std::fs::read_to_string(path.join("index.theme")).is_ok_and(|index| index.contains("Directories="))
}

/// Where themes are looked for, first first.
pub fn dirs() -> Vec<PathBuf> {
    let mut dirs = vec![glib::home_dir().join(".icons"), user_dir()];
    dirs.extend(glib::system_data_dirs().into_iter().map(|dir| dir.join("icons")));
    dirs
}

/// The installed themes by name, each where it is found first.
pub fn installed(cursors: bool) -> Vec<Installed> {
    let mut found: Vec<Installed> = dirs()
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|theme| {
            let path = theme.path();
            if cursors {
                is_cursor_theme(&path)
            } else {
                is_icon_theme(&path)
            }
        })
        .filter_map(|theme| {
            Some(Installed {
                name: theme.file_name().into_string().ok()?,
                path: theme.path(),
            })
        })
        // The bases every theme falls back to, not ones to pick or remove: Nekoland's own
        // is what "default" means.
        .filter(|theme| !["hicolor", "locolor", "default", "Nekoland"].contains(&theme.name.as_str()))
        .collect();
    // A stable sort keeps the first place each name was found in first.
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|later, first| later.name == first.name);
    found
}
