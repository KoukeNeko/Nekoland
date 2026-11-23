//! Undoing and redoing what Files did to files: renaming, moving, making and putting in the
//! trash. Shared by every window, as the files are.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use adw::prelude::*;
use gtk::gio;

use crate::{ops, rename, Window};

/// Something done to files, as it can be undone.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Given new names in their folders, each from its old path to its new one.
    Renamed(Vec<(PathBuf, PathBuf)>),
    /// Moved to another folder, each from where it was to where it is.
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Made: new folders and files, copies, links, archives. Undone into the trash.
    Made(Vec<PathBuf>),
    /// Put in the trash from these places. Undone back out of it.
    Trashed(Vec<PathBuf>),
}

impl Change {
    fn is_empty(&self) -> bool {
        match self {
            Change::Renamed(pairs) | Change::Moved(pairs) => pairs.is_empty(),
            Change::Made(paths) | Change::Trashed(paths) => paths.is_empty(),
        }
    }
}

thread_local! {
    static UNDO: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
    static REDO: RefCell<Vec<Change>> = const { RefCell::new(Vec::new()) };
}

/// Remembers what was just done, to undo; what was undone before can no longer be redone.
pub fn record(change: Change) {
    if change.is_empty() {
        return;
    }
    UNDO.with_borrow_mut(|undo| undo.push(change));
    REDO.with_borrow_mut(Vec::clear);
    crate::history_changed();
}

pub fn can_undo() -> bool {
    UNDO.with_borrow(|undo| !undo.is_empty())
}

pub fn can_redo() -> bool {
    REDO.with_borrow(|redo| !redo.is_empty())
}

pub fn undo(win: &Rc<Window>) {
    let Some(change) = UNDO.with_borrow_mut(Vec::pop) else {
        return;
    };
    let (reversed, failed) = reverse(change);
    if !reversed.is_empty() {
        REDO.with_borrow_mut(|redo| redo.push(reversed));
    }
    crate::history_changed();
    if let Some(failed) = failed {
        win.alert("無法還原", Some(&failed));
    }
}

pub fn redo(win: &Rc<Window>) {
    let Some(change) = REDO.with_borrow_mut(Vec::pop) else {
        return;
    };
    let (reversed, failed) = reverse(change);
    if !reversed.is_empty() {
        UNDO.with_borrow_mut(|undo| undo.push(reversed));
    }
    crate::history_changed();
    if let Some(failed) = failed {
        win.alert("無法重做", Some(&failed));
    }
}

/// Does the opposite of `change`: what undoes that in turn, of what could be done, and why
/// the rest could not.
fn reverse(change: Change) -> (Change, Option<String>) {
    let mut failed = Vec::new();
    let reversed = match change {
        Change::Renamed(pairs) => {
            let now: Vec<gio::File> = pairs
                .iter()
                .map(|(_, after)| gio::File::for_path(after))
                .collect();
            let names: Vec<String> = pairs.iter().map(|(before, _)| file_name(before)).collect();
            match rename::rename_all(&now, &names) {
                Ok(_) => Change::Renamed(pairs.into_iter().map(|(before, after)| (after, before)).collect()),
                Err(err) => {
                    failed.push(err.message().to_owned());
                    Change::Renamed(Vec::new())
                }
            }
        }
        Change::Moved(pairs) => {
            let mut back = Vec::new();
            for (before, after) in pairs {
                if std::fs::symlink_metadata(&before).is_ok() {
                    failed.push(format!("「{}」已經存在", file_name(&before)));
                    continue;
                }
                match std::fs::rename(&after, &before) {
                    Ok(()) => back.push((after, before)),
                    // shortcut: across drives it goes back by its own name, which is the one it
                    // had unless the move gave it another; carry the old name along if needed.
                    Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
                        if let Some(folder) = before.parent() {
                            ops::transfer("move", folder, std::slice::from_ref(&after), false);
                            back.push((after, before));
                        }
                    }
                    Err(err) => failed.push(format!("{}：{err}", file_name(&after))),
                }
            }
            Change::Moved(back)
        }
        Change::Made(paths) => {
            let mut trashed = Vec::new();
            for path in paths {
                match gio::File::for_path(&path).trash(gio::Cancellable::NONE) {
                    Ok(()) => trashed.push(path),
                    Err(err) => failed.push(format!("{}：{}", file_name(&path), err.message())),
                }
            }
            Change::Trashed(trashed)
        }
        Change::Trashed(paths) => {
            let mut back = Vec::new();
            for (item, orig) in in_trash(&paths) {
                match ops::put_back(&item, &orig) {
                    Ok(()) => back.push(orig),
                    Err(err) => failed.push(format!("{}：{}", file_name(&orig), err.message())),
                }
            }
            if back.len() < paths.len() && failed.is_empty() {
                failed.push("垃圾桶裡已經沒有這些項目".to_owned());
            }
            Change::Made(back)
        }
    };
    (reversed, (!failed.is_empty()).then(|| failed.join("\n")))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The items in the trash that came from these places, the last put there of each, with
/// where each came from.
fn in_trash(paths: &[PathBuf]) -> Vec<(gio::File, PathBuf)> {
    let trash = crate::pane::trash();
    let Ok(children) = trash.enumerate_children(
        "standard::name,trash::orig-path,trash::deletion-date",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, String, gio::File)> = Vec::new();
    for info in children.filter_map(Result::ok) {
        let Some(orig) = info.attribute_byte_string("trash::orig-path") else {
            continue;
        };
        let orig = PathBuf::from(orig.as_str());
        if !paths.contains(&orig) {
            continue;
        }
        // ISO 8601 times, which sort as they read.
        let when = info
            .attribute_string("trash::deletion-date")
            .map(|when| when.to_string())
            .unwrap_or_default();
        let item = trash.child(info.name());
        match found.iter_mut().find(|(path, ..)| *path == orig) {
            Some(entry) if entry.1 < when => *entry = (orig, when, item),
            Some(_) => {}
            None => found.push((orig, when, item)),
        }
    }
    found.into_iter().map(|(orig, _, item)| (item, orig)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nekoland-undo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_rename_undone_and_redone() {
        let dir = folder("rename");
        std::fs::write(dir.join("a"), "A").unwrap();
        std::fs::rename(dir.join("a"), dir.join("b")).unwrap();
        let change = Change::Renamed(vec![(dir.join("a"), dir.join("b"))]);
        let (undone, failed) = reverse(change.clone());
        assert_eq!(failed, None);
        assert!(dir.join("a").exists() && !dir.join("b").exists());
        let (redone, failed) = reverse(undone);
        assert_eq!(failed, None);
        assert_eq!(redone, change);
        assert!(dir.join("b").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_move_goes_back_unless_its_place_is_taken() {
        let dir = folder("move");
        std::fs::create_dir(dir.join("in")).unwrap();
        std::fs::write(dir.join("in/a"), "A").unwrap();
        std::fs::write(dir.join("in/b"), "B").unwrap();
        std::fs::write(dir.join("b"), "taken").unwrap();
        let (undone, failed) = reverse(Change::Moved(vec![
            (dir.join("a"), dir.join("in/a")),
            (dir.join("b"), dir.join("in/b")),
        ]));
        assert_eq!(std::fs::read_to_string(dir.join("a")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dir.join("b")).unwrap(), "taken");
        assert!(failed.is_some());
        assert_eq!(undone, Change::Moved(vec![(dir.join("in/a"), dir.join("a"))]));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
