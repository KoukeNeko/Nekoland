//! 取得資訊: a window for one item, with its tags, what it is, its name, the program that
//! opens its kind, its picture, and who may read and change it.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{
    browse::{info_file, is_dir, modified_label, GAP},
    pane, tags, thumbs, Window,
};

/// What the owner, the group and everyone else may do, as unix::mode's bits for each.
const ACCESS: [(&str, u32); 3] = [("讀寫", 6), ("唯讀", 4), ("無法存取", 0)];

pub fn show(win: &Rc<Window>, info: &gio::FileInfo) {
    let Some(file) = info_file(info) else { return };
    let name = info.display_name().to_string();
    let page = adw::PreferencesPage::new();

    // What it looks like, once: its name is under 名稱與副檔名, its kind and size under 一般.
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .height_request(200)
        .css_classes(["nf-thumb"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    let image = gtk::Image::builder().pixel_size(128).build();
    thumbs::show(&picture, &image, info, Some(&file));
    let top = gtk::Box::new(gtk::Orientation::Vertical, 0);
    top.append(&picture);
    top.append(&image);
    let top_group = adw::PreferencesGroup::new();
    top_group.add(&top);
    page.add(&top_group);
    let kind = pane::kind_label(info);

    // Tags.
    let tag_group = adw::PreferencesGroup::builder().title("標籤").build();
    let tag_row = gtk::Box::builder().spacing(GAP).build();
    let current = std::rc::Rc::new(std::cell::RefCell::new(tags::of(info)));
    for (tag, color) in tags::TAGS {
        let button = gtk::ToggleButton::builder()
            .tooltip_text(tag)
            .css_classes(["nf-tag-button", color])
            .active(current.borrow().iter().any(|t| t == tag))
            .build();
        let (file, current, weak) = (file.clone(), current.clone(), Rc::downgrade(win));
        button.connect_toggled(move |button| {
            let changed = tags::toggled(&current.borrow(), tag, button.is_active());
            match tags::write(&file, &changed) {
                Ok(()) => *current.borrow_mut() = changed,
                Err(err) => {
                    if let Some(win) = weak.upgrade() {
                        win.alert("無法更改標籤", Some(err.message()));
                    }
                }
            }
        });
        tag_row.append(&button);
    }
    tag_group.add(&tag_row);
    page.add(&tag_group);

    // What it is.
    let general = adw::PreferencesGroup::builder().title("一般").build();
    let fact = |title: &str, value: &str| {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(value)
            .subtitle_selectable(true)
            .build();
        row.add_css_class("property");
        row
    };
    general.add(&fact("種類", &kind));
    let size_row = fact(
        "大小",
        &if is_dir(info) {
            "—".to_string()
        } else {
            size_words(info.size() as u64)
        },
    );
    general.add(&size_row);
    if is_dir(info) {
        count_folder(&file, &size_row);
    }
    if let Some(folder) = file.parent() {
        let place = folder
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| folder.uri().to_string());
        general.add(&fact("位置", &place));
    }
    let now = glib::DateTime::now_local().ok();
    let said = |when: Option<glib::DateTime>| match (when.and_then(|t| t.to_local().ok()), &now) {
        (Some(when), Some(now)) => Some(modified_label(&when, now)),
        _ => None,
    };
    if let Some(text) = said(pane::created(info)) {
        general.add(&fact("建立日期", &text));
    }
    if let Some(text) = said(info.modification_date_time()) {
        general.add(&fact("修改日期", &text));
    }
    if let Some(orig) = info.attribute_byte_string("trash::orig-path") {
        general.add(&fact("原本位置", &orig));
    }
    page.add(&general);

    // Its name.
    let naming = adw::PreferencesGroup::builder().title("名稱與副檔名").build();
    let name_row = adw::EntryRow::builder()
        .title("名稱")
        .text(&name)
        .show_apply_button(true)
        .sensitive(info.boolean("access::can-rename"))
        .build();
    {
        let (file, weak) = (file.clone(), Rc::downgrade(win));
        name_row.connect_apply(move |row| {
            let new = row.text().trim().to_owned();
            let Some(win) = weak.upgrade() else { return };
            if new.is_empty() || new.contains('/') {
                win.alert("名稱不能是空的或含有「/」", None);
                return;
            }
            if let Err(err) = file.set_display_name(&new, gio::Cancellable::NONE) {
                win.alert("無法重新命名", Some(err.message()));
            }
        });
    }
    naming.add(&name_row);
    page.add(&naming);

    // The program that opens its kind.
    let content_type = info.content_type().map(|t| t.to_string()).unwrap_or_default();
    if !is_dir(info) && !content_type.is_empty() {
        let opening = adw::PreferencesGroup::builder()
            .title("打開檔案的應用程式")
            .build();
        let default = gio::AppInfo::default_for_type(&content_type, false);
        let mut apps: Vec<gio::AppInfo> = default.iter().cloned().collect();
        for app in gio::AppInfo::all_for_type(&content_type) {
            if !apps.iter().any(|a| a.equal(&app)) {
                apps.push(app);
            }
        }
        if !apps.is_empty() {
            let names: Vec<String> = apps.iter().map(|a| a.display_name().to_string()).collect();
            let pick = adw::ComboRow::builder()
                .title("應用程式")
                .model(&gtk::StringList::new(
                    &names.iter().map(String::as_str).collect::<Vec<_>>(),
                ))
                .build();
            let all = gtk::Button::builder()
                .label("全部改用這個應用程式")
                .valign(gtk::Align::Center)
                .sensitive(false)
                .build();
            pick.connect_selected_notify(glib::clone!(
                #[weak]
                all,
                move |pick| all.set_sensitive(pick.selected() != 0)
            ));
            let (weak, picked) = (Rc::downgrade(win), pick.clone());
            all.connect_clicked(move |button| {
                let (Some(win), Some(app)) = (weak.upgrade(), apps.get(picked.selected() as usize)) else {
                    return;
                };
                match app.set_as_default_for_type(&content_type) {
                    Ok(()) => button.set_sensitive(false),
                    Err(err) => win.alert("無法更改打開檔案的應用程式", Some(err.message())),
                }
            });
            let row = adw::ActionRow::new();
            row.add_suffix(&all);
            opening.add(&pick);
            opening.add(&row);
            page.add(&opening);
        }
    }

    // Who may read and change it.
    if info.has_attribute("unix::mode") {
        let mode = info.attribute_uint32("unix::mode");
        let mine = info
            .attribute_string("owner::user")
            .is_some_and(|owner| owner == glib::user_name().to_string_lossy().as_ref());
        let sharing = adw::PreferencesGroup::builder().title("共享與權限").build();
        let owner = info
            .attribute_string("owner::user")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let group = info
            .attribute_string("owner::group")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let mode = std::rc::Rc::new(std::cell::Cell::new(mode));
        for (index, who) in [owner, group, "所有人".to_string()].into_iter().enumerate() {
            let shift = 6 - 3 * index as u32;
            let bits = (mode.get() >> shift) & 6;
            let pick = adw::ComboRow::builder()
                .title(who)
                .model(&gtk::StringList::new(&ACCESS.map(|(label, _)| label)))
                .selected(ACCESS.iter().position(|(_, b)| *b == bits).unwrap_or(2) as u32)
                .sensitive(mine)
                .build();
            let (file, weak, mode) = (file.clone(), Rc::downgrade(win), mode.clone());
            pick.connect_selected_notify(move |pick| {
                let wanted = ACCESS[pick.selected() as usize].1;
                // A folder's x goes with its r, or what is in it cannot be reached.
                let x = if is_dir_file(&file) && wanted & 4 != 0 {
                    1
                } else {
                    (mode.get() >> shift) & 1
                };
                let new = (mode.get() & !(7 << shift)) | ((wanted | x) << shift);
                match file.set_attribute_uint32(
                    "unix::mode",
                    new,
                    gio::FileQueryInfoFlags::NONE,
                    gio::Cancellable::NONE,
                ) {
                    Ok(()) => mode.set(new),
                    Err(err) => {
                        if let Some(win) = weak.upgrade() {
                            win.alert("無法更改權限", Some(err.message()));
                        }
                    }
                }
            });
            sharing.add(&pick);
        }
        page.add(&sharing);
    }

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let window = adw::Window::builder()
        .title(format!("{name} 資訊"))
        .default_width(360)
        .default_height(680)
        .content(&view)
        .build();
    if let Some(app) = win.window.application() {
        window.set_application(Some(&app));
    }
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
            if key == gtk::gdk::Key::Escape
                || (key == gtk::gdk::Key::w && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK))
            {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    ));
    window.add_controller(keys);
    window.present();
}

fn is_dir_file(file: &gio::File) -> bool {
    file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) == gio::FileType::Directory
}

fn size_words(bytes: u64) -> String {
    format!("{}（{} 位元組）", glib::format_size(bytes), group_digits(bytes))
}

fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A folder's size, counted on a thread of its own: everything in it, all the way down.
fn count_folder(folder: &gio::File, row: &adw::ActionRow) {
    let Some(path) = folder.path() else { return };
    let (sender, receiver) = std::sync::mpsc::channel::<(u64, u64)>();
    std::thread::spawn(move || {
        let mut stack = vec![path];
        let (mut items, mut bytes) = (0u64, 0u64);
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                items += 1;
                match entry.metadata() {
                    Ok(meta) if meta.is_dir() && !entry.file_type().is_ok_and(|t| t.is_symlink()) => {
                        stack.push(entry.path())
                    }
                    Ok(meta) => bytes += meta.len(),
                    Err(_) => {}
                }
            }
        }
        let _ = sender.send((items, bytes));
    });
    let row = row.downgrade();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok((items, bytes)) => {
                if let Some(row) = row.upgrade() {
                    row.set_subtitle(&format!("{}，{items} 個項目", size_words(bytes)));
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => glib::ControlFlow::Break,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_grouped_in_threes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }
}
