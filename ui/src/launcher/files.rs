//! The user's files by name: the home folder walked once as the launcher opens, on a thread
//! of its own, so typing is never held up by the disk.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Deep enough for a project's sources; the walk stops there.
const DEEPEST: usize = 8;
/// More than any home folder worth searching this way holds; the walk stops there.
const MOST: usize = 200_000;
/// Folders that hold what programs make, not what the user looks for.
const SKIPPED: [&str; 6] = ["node_modules", "target", "__pycache__", "snap", "go", "venv"];

pub struct File {
    pub path: PathBuf,
    /// Its name in lower case, which is what is searched.
    pub name: String,
    pub folder: bool,
}

/// The files found, empty until the walk ends.
pub type Index = Arc<Mutex<Vec<File>>>;

/// Starts walking the home folder; `done` is called on the walk's thread when it ends.
pub fn index(done: impl FnOnce() + Send + 'static) -> Index {
    let index: Index = Arc::default();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return index;
    };
    let filling = index.clone();
    std::thread::spawn(move || {
        let mut found = Vec::new();
        walk(&home, 0, &mut found);
        *filling.lock().unwrap() = found;
        done();
    });
    index
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<File>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if found.len() >= MOST {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        found.push(File { name: name.to_lowercase(), folder: kind.is_dir(), path: path.clone() });
        if kind.is_dir() && depth < DEEPEST && !SKIPPED.contains(&name.as_str()) {
            walk(&path, depth + 1, found);
        }
    }
}

/// The files whose names hold what was typed: those that start with it first, then the
/// shallowest.
pub fn matching(index: &Index, query: &str, most: usize) -> Vec<(PathBuf, bool)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let files = index.lock().unwrap();
    let mut found: Vec<&File> = files.iter().filter(|file| file.name.contains(&query)).collect();
    found.sort_by_key(|file| (!file.name.starts_with(&query), file.path.components().count()));
    found.into_iter().take(most).map(|file| (file.path.clone(), file.folder)).collect()
}
