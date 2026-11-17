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
        // The base every theme falls back to, not one of its own.
        .filter(|theme| !["hicolor", "locolor", "default"].contains(&theme.name.as_str()))
        .collect();
    // A stable sort keeps the first place each name was found in first.
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|later, first| later.name == first.name);
    found
}
