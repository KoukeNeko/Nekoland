//! What is chosen, at the side of the window, in the columns' last column and beside the
//! gallery's picture: its picture, name, kind, tags and dates, and what can be done to it.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use adw::prelude::*;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::{gio, glib};

use crate::{
    browse::{info_file, is_dir, modified_label, EDGE, GAP},
    ops, pane, tags, thumbs, Window,
};

/// The camera's settings exiftool is asked for, with what each is called here.
const EXIF: [(&str, &str); 6] = [
    ("Model", "相機"),
    ("LensModel", "鏡頭"),
    ("FNumber", "光圈"),
    ("ExposureTime", "快門"),
    ("ISO", "ISO"),
    ("FocalLength", "焦距"),
];

pub struct Facts {
    pub root: gtk::Box,
    picture: gtk::Picture,
    image: gtk::Image,
    name: gtk::Label,
    kind: gtk::Label,
    tags: gtk::Box,
    rows: gtk::Grid,
    actions: gtk::Box,
    exif: bool,
    /// Whose facts are shown, as a camera's settings arrive after them.
    shown: Rc<RefCell<String>>,
}

pub fn scrolled_facts(facts: &Facts) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        // Its own width whatever is chosen: the rows' values expand within it, not it.
        .hexpand(false)
        .child(&facts.root)
        .css_classes(["nf-facts"])
        .build()
}

impl Facts {
    pub fn new(with_picture: bool, exif: bool) -> Facts {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(EDGE)
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .height_request(180)
            .css_classes(["nf-thumb"])
            .overflow(gtk::Overflow::Hidden)
            .build();
        let image = gtk::Image::builder().pixel_size(128).height_request(180).build();
        if with_picture {
            root.append(&picture);
            root.append(&image);
        }
        let name = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["heading"])
            .build();
        let kind = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim-label", "nf-small"])
            .build();
        let heading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(GAP / 2)
            .build();
        heading.append(&name);
        heading.append(&kind);
        root.append(&heading);
        let tags = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        root.append(&tags);
        let rows = gtk::Grid::builder()
            .column_spacing(GAP)
            .row_spacing(GAP / 2)
            .build();
        root.append(&rows);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        root.append(&actions);
        Facts {
            root,
            picture,
            image,
            name,
            kind,
            tags,
            rows,
            actions,
            exif,
            shown: Rc::default(),
        }
    }

    /// Shows `chosen`, or the folder when nothing is chosen.
    pub fn show(&self, chosen: &[gio::FileInfo], folder: Option<&gio::File>, win: &Weak<Window>) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        for container in [&self.tags, &self.actions] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }
        let folder_info = match chosen {
            [] => folder.and_then(pane::query),
            _ => None,
        };
        let one = match chosen {
            [one] => Some(one.clone()),
            [] => folder_info.clone(),
            _ => None,
        };
        // Nothing chosen where there is no folder, as in a tag's or search's results.
        if chosen.is_empty() && one.is_none() {
            self.shown.replace(String::new());
            self.picture.set_visible(false);
            self.image.set_visible(false);
            self.name.set_label("");
            self.kind.set_label("");
            return;
        }
        let Some(info) = one else {
            // Several: how many, and how large together.
            self.shown.replace(String::new());
            let size: u64 = chosen
                .iter()
                .filter(|i| !is_dir(i))
                .map(|i| i.size() as u64)
                .sum();
            self.picture.set_visible(false);
            self.image.set_visible(true);
            self.image.set_icon_name(Some("edit-copy-symbolic"));
            self.name.set_label(&format!("{} 個項目", chosen.len()));
            self.kind.set_label(&glib::format_size(size));
            self.add_actions(chosen, win);
            return;
        };
        let file = info_file(&info);
        let uri = file.as_ref().map(|f| f.uri().to_string()).unwrap_or_default();
        self.shown.replace(uri.clone());
        thumbs::show(&self.picture, &self.image, &info, file.as_ref());
        self.name.set_label(&info.display_name());
        let kind = pane::kind_label(&info);
        self.kind.set_label(&if is_dir(&info) {
            kind
        } else {
            format!("{kind} · {}", glib::format_size(info.size() as u64))
        });

        // Its tags, and adding one.
        for tag in tags::of(&info) {
            let chip = gtk::Box::builder()
                .spacing(GAP / 2)
                .css_classes(["nf-chip"])
                .build();
            chip.append(&tags::dot(&tag));
            chip.append(
                &gtk::Label::builder()
                    .label(&tag)
                    .css_classes(["nf-small"])
                    .build(),
            );
            self.tags.append(&chip);
        }
        if folder_info.is_none() {
            self.tags.append(
                &gtk::MenuButton::builder()
                    .label("加入標籤")
                    .css_classes(["flat", "nf-chip", "nf-small"])
                    .popover(&ops::tag_popover(win))
                    .build(),
            );
        }

        let local = |time: Option<glib::DateTime>| time.and_then(|t| t.to_local().ok());
        let created = pane::created(&info);
        let modified = local(info.modification_date_time());
        let accessed = info
            .attribute_uint64("time::access")
            .checked_sub(0)
            .filter(|t| *t > 0)
            .and_then(|t| glib::DateTime::from_unix_local(t as i64).ok());
        let mut row = 0;
        let mut add = |key: &str, value: String| {
            self.rows.attach(
                &gtk::Label::builder()
                    .label(key)
                    .xalign(0.0)
                    .css_classes(["dim-label", "nf-small"])
                    .build(),
                0,
                row,
                1,
                1,
            );
            self.rows.attach(
                &gtk::Label::builder()
                    .label(&value)
                    .xalign(1.0)
                    .hexpand(true)
                    .wrap(true)
                    .css_classes(["numeric", "nf-small"])
                    .build(),
                1,
                row,
                1,
                1,
            );
            row += 1;
        };
        let now = glib::DateTime::now_local().ok();
        let said = |when: &Option<glib::DateTime>| match (when, &now) {
            (Some(when), Some(now)) => Some(modified_label(when, now)),
            _ => None,
        };
        if let Some(text) = said(&created) {
            add("建立日期", text);
        }
        if let Some(text) = said(&modified) {
            add("修改日期", text);
        }
        if let Some(text) = said(&accessed) {
            add("上次打開日期", text);
        }
        if let Some(orig) = info.attribute_byte_string("trash::orig-path") {
            add("原本位置", orig.to_string());
        }
        let content_type = info.content_type().map(|t| t.to_string()).unwrap_or_default();
        let path = file.as_ref().and_then(|f| f.path());
        if let (true, Some(path)) = (content_type.starts_with("image/"), &path) {
            if let Some((_, width, height)) = Pixbuf::file_info(path) {
                add("尺寸", format!("{width} × {height}"));
            }
            if self.exif {
                self.read_exif(path.clone(), uri.clone(), row);
            }
        }
        if folder_info.is_none() {
            self.add_actions(std::slice::from_ref(&info), win);
        }
    }

    /// Rotating a picture, using it as the wallpaper, and the item's whole menu.
    fn add_actions(&self, chosen: &[gio::FileInfo], win: &Weak<Window>) {
        let pictures = !chosen.is_empty()
            && chosen
                .iter()
                .all(|i| i.content_type().is_some_and(|t| t.starts_with("image/")));
        if pictures {
            self.actions.append(&action_button(
                "object-rotate-left-symbolic",
                "向左旋轉",
                "win.rotate",
            ));
            if self.exif && chosen.len() == 1 {
                self.actions.append(&action_button(
                    "preferences-desktop-wallpaper-symbolic",
                    "設為桌布",
                    "win.wallpaper",
                ));
            }
        }
        let (Some(win), false) = (win.upgrade(), chosen.is_empty()) else {
            return;
        };
        let Some(pane) = win.pane() else { return };
        let (menu, custom) = ops::menu(&win, &pane, chosen);
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        for (id, child) in custom {
            popover.add_child(&child, &id);
        }
        self.actions.append(
            &gtk::MenuButton::builder()
                .label("更多⋯")
                .popover(&popover)
                .build(),
        );
    }

    /// A camera's settings, from exiftool, which takes a moment: on a thread of its own.
    fn read_exif(&self, path: std::path::PathBuf, uri: String, first_row: i32) {
        let (shown, rows) = (self.shown.clone(), self.rows.downgrade());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let output = std::process::Command::new("exiftool")
                .args(["-s", "-s"])
                .args(EXIF.iter().map(|(tag, _)| format!("-{tag}")))
                .arg(&path)
                .output();
            let text = output
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let _ = sender.send(text);
        });
        glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            let text = match receiver.try_recv() {
                Ok(text) => text,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(_) => return glib::ControlFlow::Break,
            };
            let Some(rows) = rows.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if *shown.borrow() != uri {
                return glib::ControlFlow::Break;
            }
            for (row, (key, value)) in (first_row..).zip(parse_exif(&text)) {
                rows.attach(
                    &gtk::Label::builder()
                        .label(key)
                        .xalign(0.0)
                        .css_classes(["dim-label", "nf-small"])
                        .build(),
                    0,
                    row,
                    1,
                    1,
                );
                rows.attach(
                    &gtk::Label::builder()
                        .label(&value)
                        .xalign(1.0)
                        .hexpand(true)
                        .wrap(true)
                        .css_classes(["numeric", "nf-small"])
                        .build(),
                    1,
                    row,
                    1,
                    1,
                );
            }
            glib::ControlFlow::Break
        });
    }
}

fn action_button(icon: &str, label: &str, action: &str) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
    gtk::Button::builder().child(&content).action_name(action).build()
}

/// exiftool's `Tag: value` lines as what each is called here, in EXIF's order.
fn parse_exif(text: &str) -> Vec<(&'static str, String)> {
    EXIF.iter()
        .filter_map(|(tag, label)| {
            text.lines()
                .find_map(|line| line.split_once(": ").filter(|(t, _)| t.trim() == *tag))
                .map(|(_, value)| {
                    let value = value.trim();
                    let value = match *tag {
                        "FNumber" => format!("ƒ/{value}"),
                        "ExposureTime" => format!("{value} 秒"),
                        _ => value.to_owned(),
                    };
                    (*label, value)
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cameras_settings_are_read_in_order() {
        let text = "ISO: 200\nModel: X100V\nFNumber: 2.0\nExposureTime: 1/250\n";
        assert_eq!(
            parse_exif(text),
            [
                ("相機", "X100V".to_string()),
                ("光圈", "ƒ/2.0".to_string()),
                ("快門", "1/250 秒".to_string()),
                ("ISO", "200".to_string()),
            ]
        );
    }
}
