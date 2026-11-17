//! What 磁碟用量 knows of a folder: every file and folder under it with the space each takes
//! on the disk, read on a thread of its own, kept, and compared with an earlier scan.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};

use serde::{Deserialize, Serialize};

/// A folder with more files than this has its smallest ones put together as one item.
const MANY_FILES: usize = 500;
/// A file under this share of its folder, in such a folder, is one of the smallest.
const TINY_SHARE: u64 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Folder,
    File,
    /// The files of a folder too small to tell apart, put together.
    Small,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub kind: Kind,
    /// The space it takes on the disk, what is in it included.
    pub size: u64,
    /// The files in it, or in Small the files it stands for.
    pub files: u64,
    /// Seconds since 1970.
    pub modified: i64,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

/// A finished scan of one folder.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tree {
    pub root: PathBuf,
    /// Seconds since 1970.
    pub scanned: i64,
    pub nodes: Vec<Node>,
    /// Folders it could not read.
    pub denied: u64,
    /// The free space of the disk the folder is on, and the disk's size.
    pub free: Option<(u64, u64)>,
}

pub const ROOT: usize = 0;

impl Tree {
    pub fn path(&self, mut id: usize) -> PathBuf {
        let mut names = Vec::new();
        while let Some(parent) = self.nodes[id].parent {
            names.push(self.nodes[id].name.as_str());
            id = parent;
        }
        let mut path = self.root.clone();
        path.extend(names.iter().rev());
        path
    }

    /// The folders from the scan's to `id`'s, both included.
    pub fn trail(&self, mut id: usize) -> Vec<usize> {
        let mut trail = vec![id];
        while let Some(parent) = self.nodes[id].parent {
            trail.push(parent);
            id = parent;
        }
        trail.reverse();
        trail
    }

    /// `id`'s children, largest first.
    pub fn children(&self, id: usize) -> Vec<usize> {
        let mut children = self.nodes[id].children.clone();
        children.sort_by(|a, b| self.nodes[*b].size.cmp(&self.nodes[*a].size));
        children
    }

    /// Every item under `id` whose name has `text` in it, ignoring case, largest first.
    pub fn find(&self, id: usize, text: &str) -> Vec<usize> {
        let text = text.to_lowercase();
        let mut found = Vec::new();
        let mut left = self.nodes[id].children.clone();
        while let Some(next) = left.pop() {
            let node = &self.nodes[next];
            if node.kind != Kind::Small && node.name.to_lowercase().contains(&text) {
                found.push(next);
            }
            left.extend(&node.children);
        }
        found.sort_by(|a, b| self.nodes[*b].size.cmp(&self.nodes[*a].size));
        found
    }

    /// The largest files under `id`, at most `count` of them.
    pub fn largest_files(&self, id: usize, count: usize) -> Vec<usize> {
        let mut files = Vec::new();
        let mut left = vec![id];
        while let Some(next) = left.pop() {
            let node = &self.nodes[next];
            if node.kind == Kind::File {
                files.push(next);
            }
            left.extend(&node.children);
        }
        files.sort_by(|a, b| self.nodes[*b].size.cmp(&self.nodes[*a].size));
        files.truncate(count);
        files
    }

    pub fn is_under(&self, id: usize, folder: usize) -> bool {
        self.trail(id).contains(&folder)
    }

    /// Takes `id` out, as its file was put in the trash: what held it gets smaller.
    pub fn remove(&mut self, id: usize) {
        let (size, files) = (self.nodes[id].size, self.nodes[id].files);
        let Some(parent) = self.nodes[id].parent else { return };
        self.nodes[parent].children.retain(|child| *child != id);
        let mut up = Some(parent);
        while let Some(folder) = up {
            let node = &mut self.nodes[folder];
            node.size = node.size.saturating_sub(size);
            node.files = node.files.saturating_sub(files);
            up = node.parent;
        }
        if let Some((free, total)) = &mut self.free {
            *free = (*free + size).min(*total);
        }
    }

    /// Every item's size by its path under the scanned folder, for comparing scans.
    fn sizes(&self) -> HashMap<PathBuf, (u64, Kind)> {
        let mut sizes = HashMap::new();
        let mut left = vec![(ROOT, PathBuf::new())];
        while let Some((id, path)) = left.pop() {
            let node = &self.nodes[id];
            if id != ROOT && node.kind != Kind::Small {
                sizes.insert(path.clone(), (node.size, node.kind));
            }
            for child in &node.children {
                left.push((*child, path.join(&self.nodes[*child].name)));
            }
        }
        sizes
    }
}

/// How far a scan has gone, read by the window as it goes.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub at: Mutex<PathBuf>,
    pub stop: AtomicBool,
}

/// Reads everything under `root`, staying on its disk and leaving out `excluded`. A file
/// with several names counts once. None when it was stopped.
pub fn scan(root: &Path, excluded: &[PathBuf], progress: &Progress) -> Option<Tree> {
    let meta = fs::symlink_metadata(root).ok()?;
    let device = meta.dev();
    let mut nodes = vec![Node {
        name: root.display().to_string(),
        kind: Kind::Folder,
        size: meta.blocks() * 512,
        files: 0,
        modified: meta.mtime(),
        parent: None,
        children: Vec::new(),
    }];
    let mut denied = 0;
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut folders = vec![(ROOT, root.to_path_buf())];
    while let Some((id, path)) = folders.pop() {
        if progress.stop.load(Ordering::Relaxed) {
            return None;
        }
        if let Ok(mut at) = progress.at.lock() {
            at.clone_from(&path);
        }
        let Ok(entries) = fs::read_dir(&path) else {
            denied += 1;
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if excluded.contains(&child) || meta.dev() != device {
                continue;
            }
            if meta.nlink() > 1 && !meta.is_dir() && !seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
            let size = meta.blocks() * 512;
            let folder = meta.is_dir();
            nodes.push(Node {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind: if folder { Kind::Folder } else { Kind::File },
                size,
                files: u64::from(!folder),
                modified: meta.mtime(),
                parent: Some(id),
                children: Vec::new(),
            });
            let new = nodes.len() - 1;
            nodes[id].children.push(new);
            if folder {
                folders.push((new, child));
            } else {
                progress.files.fetch_add(1, Ordering::Relaxed);
                progress.bytes.fetch_add(size, Ordering::Relaxed);
            }
        }
    }
    add_up(&mut nodes);
    put_small_together(&mut nodes);
    Some(Tree {
        root: root.to_path_buf(),
        scanned: now(),
        nodes,
        denied,
        free: free_space(root),
    })
}

/// Gives each folder the size and files of what is in it. Children always come after
/// their folder, so going backwards finishes each child first.
fn add_up(nodes: &mut [Node]) {
    for id in (1..nodes.len()).rev() {
        let (size, files) = (nodes[id].size, nodes[id].files);
        if let Some(parent) = nodes[id].parent {
            nodes[parent].size += size;
            nodes[parent].files += files;
        }
    }
}

/// In a folder of very many files, the smallest become one item, which is all a chart could
/// draw of them anyway. Their nodes stay in the list, no longer anyone's children.
fn put_small_together(nodes: &mut Vec<Node>) {
    for id in 0..nodes.len() {
        if nodes[id].kind != Kind::Folder || nodes[id].children.len() <= MANY_FILES {
            continue;
        }
        let limit = nodes[id].size / TINY_SHARE;
        let (small, kept): (Vec<usize>, Vec<usize>) = nodes[id]
            .children
            .iter()
            .partition(|child| nodes[**child].kind == Kind::File && nodes[**child].size < limit);
        if small.len() < 2 {
            continue;
        }
        let size = small.iter().map(|child| nodes[*child].size).sum();
        let modified = small.iter().map(|child| nodes[*child].modified).max().unwrap_or(0);
        for child in &small {
            nodes[*child].parent = None;
        }
        nodes.push(Node {
            name: format!("{} 個小檔案", small.len()),
            kind: Kind::Small,
            size,
            files: small.len() as u64,
            modified,
            parent: Some(id),
            children: Vec::new(),
        });
        let together = nodes.len() - 1;
        nodes[id].children = kept;
        nodes[id].children.push(together);
    }
}

fn free_space(path: &Path) -> Option<(u64, u64)> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: the path is a NUL-ended string and stats a statvfs to fill.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let unit = stats.f_frsize as u64;
    Some((stats.f_bavail as u64 * unit, stats.f_blocks as u64 * unit))
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// Where scans are kept, the latest of each folder's few, for the sidebar and comparing.
pub fn kept_folder() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("nekoland/disk")
}

/// A scan file's extension: JSON, gzipped.
pub const EXTENSION: &str = "nekoscan";
/// How many scans of each folder are kept.
const KEPT_PER_FOLDER: usize = 5;

pub fn save(tree: &Tree, path: &Path) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let partial = path.with_extension("partial");
    let mut out = flate2::write::GzEncoder::new(fs::File::create(&partial)?, flate2::Compression::default());
    serde_json::to_writer(&mut out, tree)?;
    out.finish()?.sync_all()?;
    fs::rename(partial, path)
}

/// A scan file read back; its tree is checked, since the file may be anyone's.
pub fn load(path: &Path) -> Result<Tree, String> {
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(fs::File::open(path).map_err(|err| err.to_string())?)
        .read_to_end(&mut text)
        .map_err(|err| err.to_string())?;
    let tree: Tree = serde_json::from_slice(&text).map_err(|err| err.to_string())?;
    let count = tree.nodes.len();
    let sound = count > 0
        && tree.nodes[ROOT].parent.is_none()
        && tree.nodes.iter().enumerate().all(|(id, node)| {
            node.parent.is_none_or(|parent| parent < id)
                && node.children.iter().all(|child| *child > id && *child < count && tree.nodes[*child].parent == Some(id))
        });
    if sound {
        Ok(tree)
    } else {
        Err("不是完整的掃描檔".into())
    }
}

/// Keeps a finished scan with the others, dropping that folder's oldest past a few.
pub fn keep(tree: &Tree) -> std::io::Result<PathBuf> {
    let folder = kept_folder();
    let path = folder.join(format!("{}.{EXTENSION}", tree.scanned));
    save(tree, &path)?;
    write_heading(tree, &path)?;
    let mut same: Vec<(i64, PathBuf)> = kept()
        .into_iter()
        .filter(|kept| kept.root == tree.root)
        .map(|kept| (kept.scanned, kept.path))
        .collect();
    same.sort_by_key(|(scanned, _)| std::cmp::Reverse(*scanned));
    for (_, old) in same.into_iter().skip(KEPT_PER_FOLDER) {
        forget(&old)?;
    }
    Ok(path)
}

/// A kept scan, as the sidebar lists it, without its tree.
#[derive(Clone, Debug)]
pub struct Kept {
    pub path: PathBuf,
    pub root: PathBuf,
    pub scanned: i64,
    pub size: u64,
}

#[derive(Deserialize)]
struct Heading {
    root: PathBuf,
    scanned: i64,
}

/// The kept scans, latest first. Each file's name is its scan's time; its folder and size
/// are read from a small file beside it.
pub fn kept() -> Vec<Kept> {
    let Ok(entries) = fs::read_dir(kept_folder()) else { return Vec::new() };
    let mut kept: Vec<Kept> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != EXTENSION {
                return None;
            }
            let about = fs::read(path.with_extension("json")).ok()?;
            let heading: Heading = serde_json::from_slice(&about).ok()?;
            let size = serde_json::from_slice::<serde_json::Value>(&about).ok()?.get("size")?.as_u64()?;
            Some(Kept { path, root: heading.root, scanned: heading.scanned, size })
        })
        .collect();
    kept.sort_by_key(|kept| std::cmp::Reverse(kept.scanned));
    kept
}

/// The small file `kept` reads, written beside each kept scan.
pub fn write_heading(tree: &Tree, path: &Path) -> std::io::Result<()> {
    let about = serde_json::json!({ "root": tree.root, "scanned": tree.scanned, "size": tree.nodes[ROOT].size });
    let mut file = fs::File::create(path.with_extension("json"))?;
    file.write_all(about.to_string().as_bytes())
}

/// Drops a kept scan and what describes it.
pub fn forget(path: &Path) -> std::io::Result<()> {
    fs::remove_file(path)?;
    match fs::remove_file(path.with_extension("json")) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Grew,
    Shrank,
    Appeared,
    Disappeared,
}

/// What changed between an earlier scan and a later one of the same folder: each item that
/// grew, shrank, appeared or disappeared, with its path under the folder and its sizes then
/// and now, those that changed most first. Of a folder that appeared or disappeared only the
/// folder is told, not what is in it.
pub fn compare(before: &Tree, after: &Tree) -> Vec<(PathBuf, Change, u64, u64)> {
    let (old, new) = (before.sizes(), after.sizes());
    let whole_folder_counted = |path: &Path, other: &HashMap<PathBuf, (u64, Kind)>| {
        path.ancestors().skip(1).any(|up| !up.as_os_str().is_empty() && !other.contains_key(up))
    };
    let mut changes = Vec::new();
    for (path, (now, _)) in &new {
        match old.get(path) {
            Some((then, _)) if then == now => {}
            Some((then, _)) => {
                let change = if now > then { Change::Grew } else { Change::Shrank };
                changes.push((path.clone(), change, *then, *now));
            }
            None if !whole_folder_counted(path, &old) => changes.push((path.clone(), Change::Appeared, 0, *now)),
            None => {}
        }
    }
    for (path, (then, _)) in &old {
        if !new.contains_key(path) && !whole_folder_counted(path, &new) {
            changes.push((path.clone(), Change::Disappeared, *then, 0));
        }
    }
    changes.sort_by(|a, b| b.3.abs_diff(b.2).cmp(&a.3.abs_diff(a.2)).then_with(|| a.0.cmp(&b.0)));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, kind: Kind, size: u64, parent: Option<usize>, children: &[usize]) -> Node {
        Node {
            name: name.into(),
            kind,
            size,
            files: u64::from(kind == Kind::File),
            modified: 0,
            parent,
            children: children.to_vec(),
        }
    }

    /// root (a (x 10, y 20), z 5), sizes added up.
    fn tree() -> Tree {
        let mut nodes = vec![
            node("/r", Kind::Folder, 0, None, &[1, 4]),
            node("a", Kind::Folder, 0, Some(0), &[2, 3]),
            node("x", Kind::File, 10, Some(1), &[]),
            node("y", Kind::File, 20, Some(1), &[]),
            node("z", Kind::File, 5, Some(0), &[]),
        ];
        add_up(&mut nodes);
        Tree { root: "/r".into(), scanned: 0, nodes, denied: 0, free: Some((100, 1000)) }
    }

    #[test]
    fn folders_hold_what_is_in_them() {
        let tree = tree();
        assert_eq!((tree.nodes[ROOT].size, tree.nodes[ROOT].files), (35, 3));
        assert_eq!(tree.nodes[1].size, 30);
        assert_eq!(tree.path(3), PathBuf::from("/r/a/y"));
        assert_eq!(tree.trail(3), [0, 1, 3]);
        assert_eq!(tree.children(1), [3, 2]);
        assert_eq!(tree.find(ROOT, "Y"), [3]);
        assert_eq!(tree.largest_files(ROOT, 2), [3, 2]);
    }

    #[test]
    fn what_is_removed_leaves_its_folders_smaller() {
        let mut tree = tree();
        tree.remove(3);
        assert_eq!(tree.nodes[1].children, [2]);
        assert_eq!((tree.nodes[1].size, tree.nodes[ROOT].size, tree.nodes[ROOT].files), (10, 15, 2));
        assert_eq!(tree.free, Some((120, 1000)));
    }

    #[test]
    fn the_smallest_of_many_files_are_one_item() {
        let mut nodes = vec![node("/r", Kind::Folder, 0, None, &[])];
        for index in 0..=MANY_FILES {
            nodes.push(node(&index.to_string(), Kind::File, 1, Some(0), &[]));
            nodes[0].children.push(index + 1);
        }
        nodes.push(node("big", Kind::File, 1_000_000, Some(0), &[]));
        let big = nodes.len() - 1;
        nodes[0].children.push(big);
        add_up(&mut nodes);
        put_small_together(&mut nodes);
        let children = &nodes[0].children;
        assert_eq!(children.len(), 2);
        let together = &nodes[*children.last().unwrap()];
        assert_eq!((together.kind, together.size, together.files), (Kind::Small, MANY_FILES as u64 + 1, MANY_FILES as u64 + 1));
    }

    #[test]
    fn scans_are_kept_and_read_back() {
        let folder = std::env::temp_dir().join(format!("nekoland-disk-test-{}", std::process::id()));
        let path = folder.join(format!("scan.{EXTENSION}"));
        let tree = tree();
        save(&tree, &path).unwrap();
        let read = load(&path).unwrap();
        assert_eq!(read.nodes, tree.nodes);
        fs::write(&path, b"not a scan").unwrap();
        assert!(load(&path).is_err());
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_real_folder_is_scanned() {
        let folder = std::env::temp_dir().join(format!("nekoland-disk-scan-{}", std::process::id()));
        fs::create_dir_all(folder.join("inner")).unwrap();
        fs::write(folder.join("inner/file"), vec![1; 10_000]).unwrap();
        fs::hard_link(folder.join("inner/file"), folder.join("again")).unwrap();
        fs::write(folder.join("left-out"), vec![1; 10_000]).unwrap();
        let tree = scan(&folder, &[folder.join("left-out")], &Progress::default()).unwrap();
        let names: Vec<&str> = tree.nodes.iter().skip(1).map(|node| node.name.as_str()).collect();
        assert_eq!(tree.nodes[ROOT].files, 1, "{names:?}");
        assert!(!names.contains(&"left-out"));
        assert!(tree.nodes[ROOT].size >= 10_000);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn changes_between_scans() {
        let before = tree();
        let mut after = tree();
        after.remove(4);
        after.nodes[2].size = 50;
        after.nodes.push(node("new", Kind::File, 7, Some(1), &[]));
        let new = after.nodes.len() - 1;
        after.nodes[1].children.push(new);
        let changes = compare(&before, &after);
        let told: Vec<(String, Change)> =
            changes.iter().map(|(path, change, ..)| (path.display().to_string(), *change)).collect();
        assert!(told.contains(&("a/x".into(), Change::Grew)));
        assert!(told.contains(&("a/new".into(), Change::Appeared)));
        assert!(told.contains(&("z".into(), Change::Disappeared)));
        assert_eq!(changes[0].0, PathBuf::from("a/x"));
    }
}
