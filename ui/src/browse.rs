//! What the windows that browse files share: the file chooser, the file manager and the
//! window that copies and moves. One padding for all of them, the sidebar's places, and
//! how a date or a free name is said.

// Each of those programs uses part of this.
#![allow(dead_code)]

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use gtk::prelude::*;
use gtk::{gio, glib};

/// One padding for every window that browses files: their edges and around each bar, and
/// one gap between the controls in them.
pub const EDGE: i32 = 12;
pub const GAP: i32 = 8;

/// Set for nekoland-fileop, it writes `<done> <total>` bytes on stdout as it works, for the
/// sidebar of Files that started it.
pub const REPORT_PROGRESS: &str = "NEKOLAND_FILEOP_REPORT";

/// How many folders stay read and watched: every folder row a list shows is read ahead.
const KEPT_FOLDERS: usize = 32;
/// How long the folder before stays in view while the next one is read.
pub const READ_WAIT: Duration = Duration::from_millis(100);

pub fn home() -> PathBuf {
    glib::home_dir()
}

/// Bytes in a file, or in everything in a folder; links are not followed.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| size_of(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

pub fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
}

pub fn info_file(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file").and_downcast()
}

/// The lines of GTK's bookmarks file as (URI, label), for local folders only.
pub fn parse_bookmarks(text: &str) -> Vec<(String, Option<String>)> {
    text.lines()
        .filter(|line| line.starts_with("file://"))
        .map(|line| match line.split_once(' ') {
            Some((uri, label)) => (
                uri.to_owned(),
                Some(label.trim().to_owned()).filter(|l| !l.is_empty()),
            ),
            None => (line.trim().to_owned(), None),
        })
        .collect()
}

pub fn bookmarks_path() -> PathBuf {
    glib::user_config_dir().join("gtk-3.0/bookmarks")
}

/// Where the servers GVfs has no backend for are mounted, each a FUSE folder rclone serves.
pub fn hamasen_root() -> PathBuf {
    home().join("Hamasen")
}

/// A mount of a server rather than of a drive: one by GVfs, or a folder rclone mounted.
pub fn is_server(mount: &gio::Mount) -> bool {
    let root = mount.root();
    !root.is_native() || root.path().is_some_and(|path| path.starts_with(hamasen_root()))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Mounted through GIO.
    Gvfs,
    /// An rclone remote signed in to in the browser.
    Oauth,
    /// An rclone remote the server's record describes in full.
    S3,
}

pub struct Protocol {
    pub label: &'static str,
    pub scheme: &'static str,
    pub port: u64,
    pub kind: Kind,
    pub rclone_type: &'static str,
}

/// The kinds of server Hamasen and Files set up, in the order they are offered.
pub const PROTOCOLS: [Protocol; 10] = [
    Protocol { label: "SFTP", scheme: "sftp", port: 22, kind: Kind::Gvfs, rclone_type: "" },
    Protocol { label: "FTP", scheme: "ftp", port: 21, kind: Kind::Gvfs, rclone_type: "" },
    Protocol { label: "SMB", scheme: "smb", port: 445, kind: Kind::Gvfs, rclone_type: "" },
    Protocol { label: "WebDAV", scheme: "dav", port: 80, kind: Kind::Gvfs, rclone_type: "" },
    Protocol { label: "WebDAV (HTTPS)", scheme: "davs", port: 443, kind: Kind::Gvfs, rclone_type: "" },
    Protocol { label: "NFS", scheme: "nfs", port: 2049, kind: Kind::Gvfs, rclone_type: "" },
    Protocol { label: "Google Drive", scheme: "gdrive", port: 0, kind: Kind::Oauth, rclone_type: "drive" },
    Protocol { label: "OneDrive", scheme: "onedrive", port: 0, kind: Kind::Oauth, rclone_type: "onedrive" },
    Protocol { label: "Dropbox", scheme: "dropbox", port: 0, kind: Kind::Oauth, rclone_type: "dropbox" },
    Protocol { label: "Amazon S3", scheme: "s3", port: 0, kind: Kind::S3, rclone_type: "s3" },
];

pub fn protocol(scheme: &str) -> Option<&'static Protocol> {
    PROTOCOLS.iter().find(|p| p.scheme == scheme)
}

/// A server set up in the list Files carries on from Hamasen.
#[derive(Clone)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// The folder it opens on, once mounted.
    pub location: gio::File,
    /// GVfs mounts it, rather than rclone in a folder of its own.
    pub gvfs: bool,
    /// Everything Hamasen keeps of it, the fields neither program reads included.
    pub record: serde_json::Map<String, serde_json::Value>,
}

impl Server {
    pub fn mount(&self, mounts: &[gio::Mount]) -> Option<gio::Mount> {
        mounts
            .iter()
            .find(|mount| {
                let root = mount.root();
                if self.gvfs {
                    // GVfs mounts a host once, whichever folder on it a server opens on.
                    !root.is_native() && self.location.uri().starts_with(root.uri().as_str())
                } else {
                    root.equal(&self.location)
                }
            })
            .cloned()
    }

    pub fn field(&self, key: &str) -> &str {
        self.record.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }

    pub fn protocol(&self) -> Option<&'static Protocol> {
        protocol(self.field("scheme"))
    }
}

/// Files keeps the list itself: in Hamasen's folder it went to the trash with Hamasen.
pub fn hamasen_servers_path() -> PathBuf {
    glib::user_config_dir().join("nekoland/servers.json")
}

/// The list as it is kept, Hamasen's own until Files first saves one; empty when there is
/// none yet.
pub fn hamasen_records() -> Vec<serde_json::Map<String, serde_json::Value>> {
    let path = Some(hamasen_servers_path())
        .filter(|path| path.exists())
        .unwrap_or_else(|| glib::user_config_dir().join("hamasen/servers.json"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        eprintln!("nekoland: cannot read {}: {err}", path.display());
        Vec::new()
    })
}

/// The servers set up in Hamasen's list, in its order.
pub fn hamasen_servers() -> Vec<Server> {
    hamasen_records().into_iter().filter_map(hamasen_server).collect()
}

/// A server as Hamasen keeps it: where it mounts it is its `server_uri` and `mount_path`.
pub fn hamasen_server(record: serde_json::Map<String, serde_json::Value>) -> Option<Server> {
    let field = |key: &str| record.get(key)?.as_str().filter(|value| !value.is_empty());
    let (id, name, scheme) = (
        field("id")?.to_owned(),
        field("name")?.to_owned(),
        field("scheme")?,
    );
    let Some(protocol) = protocol(scheme).filter(|p| p.kind == Kind::Gvfs) else {
        // The rest rclone mounts on a folder named after the server.
        let location = gio::File::for_path(hamasen_root().join(name.replace('/', "_")));
        return Some(Server { id, name, location, gvfs: false, record });
    };
    let user = field("user")
        .map(|user| format!("{}@", glib::Uri::escape_string(user, None, false)))
        .unwrap_or_default();
    let port = record
        .get("port")
        .and_then(|port| port.as_u64())
        .filter(|port| *port != 0 && *port != protocol.port)
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = glib::Uri::escape_string(
        field("path").unwrap_or("").trim_start_matches('/'),
        Some("/"),
        false,
    );
    let host = field("host")?;
    let location = gio::File::for_uri(&format!("{scheme}://{user}{host}{port}/{path}"));
    Some(Server { id, name, location, gvfs: true, record })
}

/// When a file was changed, as short as it can be said: the time today or yesterday, the
/// day this year, the date before.
pub fn modified_label(when: &glib::DateTime, now: &glib::DateTime) -> String {
    let (year, month, day) = when.ymd();
    let time = format!("{:02}:{:02}", when.hour(), when.minute());
    if (year, month, day) == now.ymd() {
        format!("今天 {time}")
    } else if now
        .add_days(-1)
        .is_ok_and(|yesterday| yesterday.ymd() == (year, month, day))
    {
        format!("昨天 {time}")
    } else if year == now.year() {
        format!("{month}月{day}日")
    } else {
        format!("{year}年{month}月{day}日")
    }
}

/// A name in `path`'s folder no file has yet: `path` with a number before its extension,
/// counting up from 2, as Files and the Finder name a second copy.
pub fn free_name(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy()));
    (2..)
        .map(|n| path.with_file_name(format!("{stem} {n}{}", extension.as_deref().unwrap_or(""))))
        .find(|candidate| std::fs::symlink_metadata(candidate).is_err())
        .expect("some number is free")
}

#[derive(Clone, Debug)]
pub enum Location {
    Recent,
    Folder(gio::File),
    /// The files given a tag.
    Tag(String),
}

impl PartialEq for Location {
    fn eq(&self, other: &Location) -> bool {
        match (self, other) {
            (Location::Recent, Location::Recent) => true,
            (Location::Folder(a), Location::Folder(b)) => a.equal(b),
            (Location::Tag(a), Location::Tag(b)) => a == b,
            _ => false,
        }
    }
}

/// A sidebar place.
pub struct Place {
    pub icon: gio::Icon,
    pub label: String,
    pub location: Location,
    /// The drive's mount, for a place that is one.
    pub mount: Option<gio::Mount>,
    /// The id of the Hamasen server it is, mounted or not.
    pub hamasen: Option<String>,
    /// The drive plugged in but not mounted yet, which choosing the place mounts; it has no
    /// location until then.
    pub volume: Option<gio::Volume>,
}

pub fn trash() -> gio::File {
    gio::File::for_uri("trash:///")
}

/// The places the sidebar lists, in sections: recent, the home folders and the trash, the
/// GTK bookmarks Files keeps too, drives, network shares, and the servers set up in Hamasen.
pub fn places() -> Vec<(Option<&'static str>, Vec<Place>)> {
    let themed = |name: &str| gio::ThemedIcon::new(name).upcast::<gio::Icon>();
    let place = |icon: gio::Icon, label: &str, location: Location| Place {
        icon,
        label: label.to_owned(),
        location,
        mount: None,
        hamasen: None,
        volume: None,
    };
    let folder =
        |icon: &str, label: &str, path: PathBuf| place(themed(icon), label, Location::Folder(gio::File::for_path(path)));
    let mut first = vec![
        place(themed("document-open-recent-symbolic"), "最近使用", Location::Recent),
        folder("user-home-symbolic", &glib::user_name().to_string_lossy(), home()),
    ];
    for (dir, icon, label) in [
        (glib::UserDirectory::Desktop, "user-desktop-symbolic", "桌面"),
        (
            glib::UserDirectory::Documents,
            "folder-documents-symbolic",
            "文件",
        ),
        (glib::UserDirectory::Downloads, "folder-download-symbolic", "下載"),
        (glib::UserDirectory::Pictures, "folder-pictures-symbolic", "圖片"),
        (glib::UserDirectory::Music, "folder-music-symbolic", "音樂"),
        (glib::UserDirectory::Videos, "folder-videos-symbolic", "影片"),
    ] {
        // xdg-user-dirs points one it has no folder for at home.
        if let Some(path) = glib::user_special_dir(dir).filter(|p| p.is_dir() && *p != home()) {
            first.push(folder(icon, label, path));
        }
    }
    let full = trash()
        .query_info("trash::item-count", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .is_ok_and(|info| info.attribute_uint32("trash::item-count") > 0);
    let trash_icon = if full { "user-trash-full-symbolic" } else { "user-trash-symbolic" };
    first.push(place(themed(trash_icon), "垃圾桶", Location::Folder(trash())));
    let all: Vec<gio::Mount> = gio::VolumeMonitor::get()
        .mounts()
        .into_iter()
        .filter(|mount| !mount.is_shadowed())
        .collect();
    let mut claimed = Vec::new();
    let servers: Vec<Place> = hamasen_servers()
        .into_iter()
        .map(|server| {
            let mount = server.mount(&all);
            claimed.extend(mount.clone());
            Place {
                icon: themed("network-server-symbolic"),
                label: server.name,
                location: Location::Folder(server.location),
                mount,
                hamasen: Some(server.id),
                volume: None,
            }
        })
        .collect();
    let (mut drives, mut network): (Vec<Place>, Vec<Place>) = all
        .iter()
        .filter(|mount| !claimed.contains(mount))
        .map(|mount| Place {
            icon: if mount.root().is_native() && is_server(mount) {
                themed("network-server-symbolic")
            } else {
                mount.symbolic_icon()
            },
            label: mount.name().into(),
            location: Location::Folder(mount.root()),
            mount: Some(mount.clone()),
            hamasen: None,
            volume: None,
        })
        .partition(|place| place.mount.as_ref().is_some_and(|m| !is_server(m)));
    drives.extend(
        gio::VolumeMonitor::get()
            .volumes()
            .into_iter()
            .filter(|volume| volume.get_mount().is_none() && volume.can_mount())
            .map(|volume| Place {
                volume: Some(volume.clone()),
                ..place(volume.symbolic_icon(), &volume.name(), Location::Recent)
            }),
    );
    network.push(place(
        themed("network-workgroup-symbolic"),
        "網路",
        Location::Folder(gio::File::for_uri("network:///")),
    ));
    let bookmarks = std::fs::read_to_string(bookmarks_path())
        .map(|text| parse_bookmarks(&text))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(uri, label)| {
            let file = gio::File::for_uri(&uri);
            let label = label.or_else(|| file.basename().map(|b| b.to_string_lossy().into_owned()))?;
            // Hamasen bookmarks the folders it mounts, which its section lists already.
            let mounted = all.iter().any(|mount| mount.root().equal(&file));
            (!mounted && file.path().is_some_and(|p| p.is_dir())).then(|| Place {
                icon: themed("folder-symbolic"),
                label,
                location: Location::Folder(file),
                mount: None,
                hamasen: None,
                volume: None,
            })
        })
        .collect();
    vec![
        (None, first),
        (Some("書籤"), bookmarks),
        (Some("裝置"), drives),
        (Some("網路"), network),
        (Some("伺服器"), servers),
    ]
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .css_classes(["flat"])
        .build()
}

pub fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(child)
        .build()
}

thread_local! {
    /// Folders read lately, still watched, oldest first, each with when it last changed.
    static KEPT: RefCell<VecDeque<(gtk::DirectoryList, Option<SystemTime>)>> =
        const { RefCell::new(VecDeque::new()) };
}

/// A folder's files, read with the program's `attributes`, as kept from a moment ago when
/// the folder has not changed since, so that opening it again, going back to it or
/// unfolding it shows them at once. Folders elsewhere than this computer are read anew
/// each time.
pub fn listing(folder: &gio::File, attributes: &str) -> gtk::DirectoryList {
    // A watch misses what another computer changes on a shared drive; its time does not.
    let changed = folder
        .path()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok());
    let kept = KEPT.with_borrow_mut(|kept| {
        let at = kept
            .iter()
            .position(|(list, _)| list.file().is_some_and(|f| f.equal(folder)))?;
        let (list, when) = kept.remove(at)?;
        (when == changed && list.error().is_none()).then(|| {
            kept.push_back((list.clone(), when));
            list
        })
    });
    if let Some(list) = kept {
        return list;
    }
    let list = gtk::DirectoryList::new(Some(attributes), Some(folder));
    list.set_monitored(true);
    if folder.is_native() {
        KEPT.with_borrow_mut(|kept| {
            kept.push_back((list.clone(), changed));
            if kept.len() > KEPT_FOLDERS {
                kept.pop_front();
            }
        });
    }
    list
}

/// Reads a folder anew the next time it is listed.
pub fn forget(folder: &gio::File) {
    KEPT.with_borrow_mut(|kept| kept.retain(|(list, _)| !list.file().is_some_and(|f| f.equal(folder))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hamasen_servers_mount_where_hamasen_does() {
        let sftp = serde_json::json!({"id": "a", "name": "NAS", "scheme": "sftp", "host": "nas",
            "port": 2222, "user": "ko ke", "path": "/srv/共用"});
        let record = |v: serde_json::Value| v.as_object().unwrap().clone();
        let server = hamasen_server(record(sftp)).unwrap();
        assert!(server.gvfs);
        assert_eq!(
            server.location.uri(),
            "sftp://ko%20ke@nas:2222/srv/%E5%85%B1%E7%94%A8"
        );
        let smb = serde_json::json!({"id": "b", "name": "S", "scheme": "smb", "host": "h", "port": 445});
        assert_eq!(hamasen_server(record(smb)).unwrap().location.uri(), "smb://h/");
        let drive = serde_json::json!({"id": "c", "name": "a/b", "scheme": "gdrive", "port": 0});
        let drive = hamasen_server(record(drive)).unwrap();
        assert!(!drive.gvfs);
        assert_eq!(drive.location.path().unwrap(), home().join("Hamasen/a_b"));
    }

    #[test]
    fn bookmarks_keep_their_labels() {
        let text = "file:///home/neko/Nekoland\nfile:///home/neko/Documents/%E5%A0%B1%E5%91%8A 報告\nsftp://host/x\n";
        assert_eq!(
            parse_bookmarks(text),
            vec![
                ("file:///home/neko/Nekoland".into(), None),
                (
                    "file:///home/neko/Documents/%E5%A0%B1%E5%91%8A".into(),
                    Some("報告".into())
                ),
            ]
        );
    }

    #[test]
    fn dates_are_said_as_short_as_they_can_be() {
        let tz = glib::TimeZone::utc();
        let at = |y, mo, d, h, mi| glib::DateTime::new(&tz, y, mo, d, h, mi, 0.0).unwrap();
        let now = at(2026, 11, 17, 14, 30);
        assert_eq!(modified_label(&at(2026, 11, 17, 9, 5), &now), "今天 09:05");
        assert_eq!(modified_label(&at(2026, 11, 16, 22, 40), &now), "昨天 22:40");
        assert_eq!(modified_label(&at(2026, 3, 18, 8, 0), &now), "3月18日");
        assert_eq!(modified_label(&at(2024, 1, 5, 8, 0), &now), "2024年1月5日");
    }
}
