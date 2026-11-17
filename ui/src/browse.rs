//! What the windows that browse files share: the file chooser, the file manager and the
//! window that copies and moves. One padding for all of them, the sidebar's places, and
//! how a date or a free name is said.

// Each of those programs uses part of this.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gio, glib};

/// One padding for every window that browses files: their edges and around each bar, and
/// one gap between the controls in them.
pub const EDGE: i32 = 12;
pub const GAP: i32 = 8;

/// Set for nekoland-fileop, it writes `<done> <total>` bytes on stdout as it works, for the
/// sidebar of Files that started it.
pub const REPORT_PROGRESS: &str = "NEKOLAND_FILEOP_REPORT";

pub fn home() -> PathBuf {
    glib::home_dir()
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
}

impl PartialEq for Location {
    fn eq(&self, other: &Location) -> bool {
        match (self, other) {
            (Location::Recent, Location::Recent) => true,
            (Location::Folder(a), Location::Folder(b)) => a.equal(b),
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
}

/// The places the sidebar lists, in sections: recent and the home folders, the GTK
/// bookmarks Files keeps too, and mounted drives.
pub fn places() -> Vec<(Option<&'static str>, Vec<Place>)> {
    let themed = |name: &str| gio::ThemedIcon::new(name).upcast::<gio::Icon>();
    let folder = |icon: &str, label: &str, path: PathBuf| Place {
        icon: themed(icon),
        label: label.to_owned(),
        location: Location::Folder(gio::File::for_path(path)),
        mount: None,
    };
    let mut first = vec![
        Place {
            icon: themed("document-open-recent-symbolic"),
            label: "最近使用".into(),
            location: Location::Recent,
            mount: None,
        },
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
    let bookmarks = std::fs::read_to_string(bookmarks_path())
        .map(|text| parse_bookmarks(&text))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(uri, label)| {
            let file = gio::File::for_uri(&uri);
            let label = label.or_else(|| file.basename().map(|b| b.to_string_lossy().into_owned()))?;
            file.path().is_some_and(|p| p.is_dir()).then(|| Place {
                icon: themed("folder-symbolic"),
                label,
                location: Location::Folder(file),
                mount: None,
            })
        })
        .collect();
    let mounts = gio::VolumeMonitor::get()
        .mounts()
        .into_iter()
        .filter(|mount| !mount.is_shadowed())
        .map(|mount| Place {
            icon: mount.symbolic_icon(),
            label: mount.name().into(),
            location: Location::Folder(mount.root()),
            mount: Some(mount),
        })
        .collect();
    vec![(None, first), (Some("書籤"), bookmarks), (Some("裝置"), mounts)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
