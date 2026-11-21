//! Nekoland's own programs, which an install puts side by side.

use std::path::PathBuf;

/// The absolute path of one of Nekoland's programs: beside this one, else on PATH, else where
/// install.sh puts them. sudo takes only an absolute path.
pub fn path(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
        .or_else(|| gtk::glib::find_program_in_path(name))
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin").join(name))
}

