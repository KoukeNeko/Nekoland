//! Finding files by name or by tag: a walk through folders on a thread of its own, which
//! hands what it finds to the window a batch at a time.
// shortcut: walks the folders at every search; an index (localsearch) would answer at once, but the desktopless install has none.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::tags;

/// More than this many found and the walk stops.
const LIMIT: usize = 2000;
const BATCH_EVERY: Duration = Duration::from_millis(120);

#[derive(Clone, Debug, PartialEq)]
pub enum Find {
    /// In the name, any case.
    Name(String),
    Tag(String),
}

impl Find {
    fn matches(&self, path: &Path) -> bool {
        match self {
            Find::Name(query) => path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().to_lowercase().contains(query.as_str())),
            Find::Tag(tag) => gio::File::for_path(path)
                .query_info(
                    tags::ATTRIBUTE,
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    gio::Cancellable::NONE,
                )
                .is_ok_and(|info| tags::of(&info).iter().any(|t| t == tag)),
        }
    }
}

type Found = Box<dyn Fn(Vec<PathBuf>, bool)>;

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(0) };
    static WAITING: RefCell<HashMap<u64, Found>> = RefCell::new(HashMap::new());
}

/// A walk going on; dropping it stops it.
pub struct Walk(Arc<AtomicBool>, u64);

impl Drop for Walk {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        WAITING.with_borrow_mut(|waiting| waiting.remove(&self.1));
    }
}

/// Walks `roots` for what `find` asks; `found` gets each batch, and true with the last.
pub fn walk(
    roots: Vec<PathBuf>,
    find: Find,
    hidden: bool,
    found: impl Fn(Vec<PathBuf>, bool) + 'static,
) -> Walk {
    let id = NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    WAITING.with_borrow_mut(|waiting| waiting.insert(id, Box::new(found)));
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let find = match find {
        Find::Name(query) => Find::Name(query.to_lowercase()),
        tag => tag,
    };
    std::thread::spawn(move || {
        let send = |batch: Vec<PathBuf>, last: bool| {
            glib::MainContext::default().invoke(move || {
                WAITING.with_borrow(|waiting| {
                    if let Some(found) = waiting.get(&id) {
                        found(batch, last);
                    }
                });
            });
        };
        let mut batch = Vec::new();
        let mut count = 0;
        let mut sent = Instant::now();
        let mut folders: Vec<PathBuf> = roots;
        while let Some(folder) = folders.pop() {
            if stopped.load(Ordering::Relaxed) || count >= LIMIT {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_hidden = entry.file_name().to_string_lossy().starts_with('.');
                if is_hidden && !hidden {
                    continue;
                }
                if find.matches(&path) {
                    batch.push(path.clone());
                    count += 1;
                }
                // Symbolic links are not followed: one can lead back up.
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    folders.push(path);
                }
            }
            if !batch.is_empty() && sent.elapsed() >= BATCH_EVERY {
                send(std::mem::take(&mut batch), false);
                sent = Instant::now();
            }
        }
        send(batch, true);
    });
    Walk(stop, id)
}

/// The kinds a search can be narrowed to, by what each file's content type starts with.
pub const KINDS: [(&str, &[&str]); 7] = [
    ("檔案夾", &["inode/directory"]),
    (
        "文件",
        &[
            "text/",
            "application/pdf",
            "application/vnd.oasis",
            "application/vnd.openxmlformats",
            "application/msword",
        ],
    ),
    ("PDF 文件", &["application/pdf"]),
    ("圖片", &["image/"]),
    ("影片", &["video/"]),
    ("音樂", &["audio/"]),
    (
        "封存",
        &[
            "application/zip",
            "application/x-tar",
            "application/gzip",
            "application/x-7z",
            "application/x-xz",
            "application/x-bzip",
            "application/vnd.rar",
            "application/x-compressed-tar",
        ],
    ),
];

/// How far back a search can be narrowed to, in days.
pub const SINCE: [(&str, i64); 4] = [("今天", 0), ("過去 7 天", 7), ("過去 30 天", 30), ("今年", -1)];

pub fn is_kind(content_type: &str, kind: usize) -> bool {
    KINDS
        .get(kind)
        .is_some_and(|(_, starts)| starts.iter().any(|start| content_type.starts_with(start)))
}

/// Whether `when` is within the span `since` names, seen from `now`.
pub fn is_since(when: &glib::DateTime, now: &glib::DateTime, since: usize) -> bool {
    let Some((_, days)) = SINCE.get(since) else {
        return true;
    };
    match days {
        -1 => when.year() == now.year(),
        0 => when.ymd() == now.ymd(),
        days => now.difference(when).as_seconds() <= days * 24 * 3600,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_by_content_type() {
        assert!(is_kind("application/pdf", 1));
        assert!(is_kind("application/pdf", 2));
        assert!(is_kind("image/png", 3));
        assert!(!is_kind("image/png", 4));
        assert!(is_kind("application/zip", 6));
    }

    #[test]
    fn spans_are_seen_from_now() {
        let tz = glib::TimeZone::utc();
        let at = |y, mo, d| glib::DateTime::new(&tz, y, mo, d, 12, 0, 0.0).unwrap();
        let now = at(2026, 11, 17);
        assert!(is_since(&at(2026, 11, 17), &now, 0));
        assert!(!is_since(&at(2026, 11, 16), &now, 0));
        assert!(is_since(&at(2026, 11, 12), &now, 1));
        assert!(!is_since(&at(2026, 11, 1), &now, 1));
        assert!(is_since(&at(2026, 1, 1), &now, 3));
        assert!(!is_since(&at(2025, 12, 31), &now, 3));
    }

    #[test]
    fn a_walk_finds_names_below_and_stops_at_hidden_folders() {
        let dir = std::env::temp_dir().join(format!("nekoland-search-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("a/b/季報.pdf"), "").unwrap();
        std::fs::write(dir.join(".hidden/季報 2.pdf"), "").unwrap();
        let found = Rc::new(RefCell::new(Vec::new()));
        let done = Rc::new(Cell::new(false));
        let _walk = {
            let (found, done) = (found.clone(), done.clone());
            walk(
                vec![dir.clone()],
                Find::Name("季報".into()),
                false,
                move |batch, last| {
                    found.borrow_mut().extend(batch);
                    done.set(last);
                },
            )
        };
        let context = glib::MainContext::default();
        while !done.get() {
            context.iteration(true);
        }
        assert_eq!(*found.borrow(), [dir.join("a/b/季報.pdf")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use std::rc::Rc;
}
