//! What can be done to files: opening, the trash, copying through the clipboard, new
//! folders, renaming, compressing, tags, and the menus that offer all of it.

use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    io::BufRead,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::mpsc,
    time::Duration,
};

use adw::prelude::*;
use gtk::gdk_pixbuf::{Pixbuf, PixbufRotation};
use gtk::{gdk, gio, glib};

use crate::{
    browse::{bookmarks_path, free_name, info_file, is_dir, parse_bookmarks, size_of},
    pane::{Pane, Where},
    rename, tags,
    undo::{self, Change},
    Window,
};

/// The clipboard types Files and the desktop copy files as.
const COPIED_FILES: &str = "x-special/gnome-copied-files";
const URI_LIST: &str = "text/uri-list";
/// The name a new folder is given, as the desktop names one.
pub const NEW_FOLDER: &str = "未命名檔案夾";
/// The name a new text file is given.
const NEW_TEXT_FILE: &str = "未命名文件.txt";
/// The kinds of archive bsdtar takes apart.
const ARCHIVES: &[&str] = &[
    "application/zip",
    "application/x-tar",
    "application/x-compressed-tar",
    "application/x-bzip-compressed-tar",
    "application/x-xz-compressed-tar",
    "application/x-zstd-compressed-tar",
    "application/x-7z-compressed",
    "application/vnd.rar",
    "application/x-rar",
    "application/x-cpio",
];
/// What nekoland-settings is asked with for what Windows' programs add to a file's menu.
const WINDOWS_MENU_FLAG: &str = "--windows-menu";

// The menus.

/// The menu for what is chosen, or for the folder when nothing is; with the widgets its
/// custom items are, by id.
pub fn menu(
    win: &Rc<Window>,
    pane: &Rc<Pane>,
    chosen: &[gio::FileInfo],
) -> (gio::Menu, Vec<(String, gtk::Widget)>) {
    let menu = gio::Menu::new();
    let section = |items: &[(&str, &str)]| {
        let section = gio::Menu::new();
        for (label, action) in items {
            section.append(Some(label), Some(action));
        }
        section
    };
    let here = pane.here();
    if chosen.is_empty() {
        let first = gio::Menu::new();
        if here.is_trash() {
            first.append(Some("清空垃圾桶"), Some("win.empty-trash"));
        } else if here.folder().is_some() {
            first.append(Some("新增檔案夾"), Some("win.new-folder"));
            first.append(Some("新增文字文件"), Some("win.new-text-file"));
        }
        menu.append_section(None, &first);
        menu.append_section(None, &section(&[("取得資訊", "win.info")]));
        if !here.is_trash() {
            menu.append_section(None, &section(&[("貼上", "win.paste")]));
        }
        let view = gio::Menu::new();
        view.append_submenu(Some("排序方式"), &sort_menu());
        view.append_submenu(Some("檢視內容為"), &view_menu());
        view.append(Some("顯示預覽"), Some("win.preview"));
        menu.append_section(None, &view);
        return (menu, Vec::new());
    }

    if here.is_trash() {
        menu.append_section(None, &section(&[("放回原處", "win.restore")]));
        menu.append_section(
            None,
            &section(&[("快速查看", "win.quick-look"), ("取得資訊", "win.info")]),
        );
        menu.append_section(None, &section(&[("立即刪除⋯", "win.delete-now")]));
        return (menu, Vec::new());
    }

    let first = gio::Menu::new();
    first.append(Some("打開"), Some("win.open"));
    if let [info] = chosen {
        if is_dir(info) {
            first.append(Some("在新分頁打開"), Some("win.open-tab"));
        } else {
            let apps = open_with_menu(info);
            if apps.n_items() > 0 {
                first.append_submenu(Some("打開檔案的應用程式"), &apps);
            }
        }
        if let Some(path) = info_file(info).and_then(|file| file.path()) {
            add_windows_menu(&first, &path);
        }
    }
    menu.append_section(None, &first);
    menu.append_section(None, &section(&[("丟到垃圾桶", "win.trash")]));

    let rename_label = match chosen.len() {
        1 => "重新命名".to_string(),
        n => format!("重新命名 {n} 個項目⋯"),
    };
    let compress_label = match chosen {
        // Doubled, as a menu reads one underscore as the next letter's mnemonic.
        [one] => format!("壓縮「{}」", one.display_name().replace('_', "__")),
        _ => format!("壓縮 {} 個項目", chosen.len()),
    };
    let doing = gio::Menu::new();
    doing.append(Some("取得資訊"), Some("win.info"));
    doing.append(Some(&rename_label), Some("win.rename"));
    doing.append(Some(&compress_label), Some("win.compress"));
    if let [one] = chosen {
        if is_archive(one) {
            doing.append(Some("解壓縮"), Some("win.extract"));
        }
    }
    doing.append(Some("製作複本"), Some("win.duplicate"));
    doing.append(Some("製作連結"), Some("win.link"));
    doing.append(Some("快速查看"), Some("win.quick-look"));
    menu.append_section(None, &doing);
    menu.append_section(
        None,
        &section(&[("拷貝", "win.copy"), ("複製路徑", "win.copy-path")]),
    );

    // The tags, as a row of dots to click.
    let tag_section = gio::Menu::new();
    let item = gio::MenuItem::new(None, None);
    item.set_attribute_value("custom", Some(&"tags".to_variant()));
    tag_section.append_item(&item);
    tag_section.append(Some("標籤⋯"), Some("win.all-tags"));
    menu.append_section(None, &tag_section);

    let last = gio::Menu::new();
    if pane.is_searching() || matches!(here, Where::Recent | Where::Tag(_)) {
        last.append(Some("顯示所在檔案夾"), Some("win.reveal"));
    }
    if let [info] = chosen {
        if is_dir(info) && info_file(info).is_some_and(|f| f.path().is_some()) {
            last.append(Some("加入側欄"), Some("win.bookmark"));
        }
    }
    if chosen
        .iter()
        .all(|i| i.content_type().is_some_and(|t| t.starts_with("image/")))
    {
        if chosen.len() == 1 {
            last.append(Some("設為桌布"), Some("win.wallpaper"));
        }
        last.append(Some("向左旋轉"), Some("win.rotate"));
    }
    if last.n_items() > 0 {
        menu.append_section(None, &last);
    }
    (menu, vec![("tags".to_string(), tag_dots(win, chosen).upcast())])
}

/// One button a tag, in its colour, ringed when every chosen item has it.
fn tag_dots(win: &Rc<Window>, chosen: &[gio::FileInfo]) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(crate::browse::GAP)
        .css_classes(["nf-tag-row"])
        .build();
    for (index, (tag, color)) in tags::TAGS.iter().enumerate() {
        let all = !chosen.is_empty() && chosen.iter().all(|i| tags::of(i).iter().any(|t| t == tag));
        let button = gtk::Button::builder()
            .tooltip_text(*tag)
            .css_classes(["nf-tag-button", color])
            .build();
        if all {
            button.add_css_class("on");
        }
        let win = Rc::downgrade(win);
        button.connect_clicked(move |button| {
            let Some(win) = win.upgrade() else { return };
            win.toggle_tag(index);
            if let Some(popover) = button
                .ancestor(gtk::Popover::static_type())
                .and_downcast::<gtk::Popover>()
            {
                popover.popdown();
            }
        });
        row.append(&button);
    }
    row
}

/// The tag menu, as the Finder's: a field for a new tag, then each tag in its colour,
/// checked when every chosen item has it and dashed when some do, then 所有標籤⋯. Filled
/// as it opens, from what is chosen then.
pub fn tag_popover(win: &Weak<Window>) -> gtk::Popover {
    let popover = gtk::Popover::builder()
        .has_arrow(false)
        .css_classes(["nf-tag-menu"])
        .build();
    let win = win.clone();
    popover.connect_show(move |popover| fill_tag_popover(popover, &win));
    popover
}

fn fill_tag_popover(popover: &gtk::Popover, win: &Weak<Window>) {
    let Some(pane) = win.upgrade().and_then(|w| w.pane()) else {
        return;
    };
    let chosen = pane.selected();
    let entry = gtk::Entry::builder().placeholder_text("加入標籤").build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    for (tag, _) in tags::TAGS {
        let carried = chosen
            .iter()
            .filter(|i| tags::of(i).iter().any(|t| t == tag))
            .count();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, crate::browse::GAP);
        let slot = gtk::CenterBox::builder().width_request(16).build();
        slot.set_center_widget(Some(&tags::dot(tag)));
        line.append(&slot);
        line.append(&gtk::Label::builder().label(tag).xalign(0.0).hexpand(true).build());
        let state = match carried {
            0 => None,
            n if n == chosen.len() => Some("object-select-symbolic"),
            _ => Some("list-remove-symbolic"),
        };
        let mark = gtk::Image::builder().pixel_size(16).build();
        mark.set_icon_name(state);
        line.append(&mark);
        list.append(&line);
    }
    let all = gtk::Box::new(gtk::Orientation::Horizontal, crate::browse::GAP);
    all.append(&gtk::Box::builder().width_request(16).build());
    all.append(&gtk::Label::new(Some("所有標籤⋯")));
    let all_row = gtk::ListBoxRow::builder().child(&all).build();
    list.append(&all_row);
    list.set_header_func(|row, _| {
        if row.index() == tags::TAGS.len() as i32 {
            row.set_header(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
        }
    });

    let weak = win.clone();
    list.connect_row_activated(glib::clone!(
        #[weak]
        popover,
        move |_, row| {
            popover.popdown();
            let Some(win) = weak.upgrade() else { return };
            match usize::try_from(row.index()) {
                Ok(index) if index < tags::TAGS.len() => win.toggle_tag(index),
                _ => {
                    if let Some(pane) = win.pane() {
                        edit_tags(&win, pane.selected());
                    }
                }
            }
        }
    ));
    let weak = win.clone();
    entry.connect_activate(glib::clone!(
        #[weak]
        popover,
        move |entry| {
            popover.popdown();
            let Some(win) = weak.upgrade() else { return };
            let Some(pane) = win.pane() else { return };
            for name in tags::parse(&entry.text()) {
                set_tag(&win, &pane.selected(), &name, true);
            }
            pane.refilter();
            win.selection_changed(&pane);
        }
    ));

    let content = gtk::Box::new(gtk::Orientation::Vertical, crate::browse::GAP / 2);
    content.append(&entry);
    content.append(&list);
    popover.set_child(Some(&content));
}

pub fn sort_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, key) in [
        ("名稱", "name"),
        ("種類", "kind"),
        ("修改日期", "modified"),
        ("建立日期", "created"),
        ("大小", "size"),
    ] {
        menu.append(Some(label), Some(&format!("win.sort::{key}")));
    }
    menu
}

pub fn group_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, key) in [
        ("無", "none"),
        ("種類", "kind"),
        ("修改日期", "modified"),
        ("大小", "size"),
    ] {
        menu.append(Some(label), Some(&format!("win.group::{key}")));
    }
    menu
}

/// Which columns the list shows, for a right click on its header.
pub fn columns_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (key, label) in crate::pane::OPTIONAL_COLUMNS {
        menu.append(Some(label), Some(&format!("win.column-{key}")));
    }
    menu
}

pub fn view_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, key) in [
        ("圖示", "icons"),
        ("列表", "list"),
        ("欄位", "columns"),
        ("圖庫", "gallery"),
    ] {
        menu.append(Some(label), Some(&format!("win.view::{key}")));
    }
    menu
}

/// The applications that open a file of this kind, the default first.
fn open_with_menu(info: &gio::FileInfo) -> gio::Menu {
    let menu = gio::Menu::new();
    let Some(content_type) = info.content_type() else {
        return menu;
    };
    let default = gio::AppInfo::default_for_type(&content_type, false);
    let mut apps: Vec<gio::AppInfo> = default.iter().cloned().collect();
    for app in gio::AppInfo::recommended_for_type(&content_type) {
        if !apps.iter().any(|a| a.equal(&app)) {
            apps.push(app);
        }
    }
    for app in apps {
        if let Some(id) = app.id() {
            let item = gio::MenuItem::new(Some(&app.display_name().replace('_', "__")), None);
            item.set_action_and_target_value(Some("win.open-with"), Some(&id.to_variant()));
            if let Some(icon) = app.icon() {
                item.set_icon(&icon);
            }
            menu.append_item(&item);
        }
    }
    menu
}

/// Adds to `section`, once nekoland-settings says them, what Windows' programs add to a file's
/// right-click menu there; the menu shows meanwhile, as asking may start Wine and take seconds.
fn add_windows_menu(section: &gio::Menu, file: &Path) {
    let asked = gio::Subprocess::newv(
        &[OsStr::new("nekoland-settings"), OsStr::new(WINDOWS_MENU_FLAG), file.as_os_str()],
        gio::SubprocessFlags::STDOUT_PIPE,
    );
    let process = match asked {
        Ok(process) => process,
        Err(err) => return eprintln!("nekoland-files: {WINDOWS_MENU_FLAG}: {err}"),
    };
    let section = section.downgrade();
    glib::spawn_future_local(async move {
        let said = match process.communicate_utf8_future(None).await {
            Ok((said, _)) => said.unwrap_or_default(),
            Err(err) => return eprintln!("nekoland-files: {WINDOWS_MENU_FLAG}: {err}"),
        };
        let windows = windows_menu(&said);
        if let Some(section) = section.upgrade().filter(|_| windows.n_items() > 0) {
            section.append_submenu(Some("Windows"), &windows);
        }
    });
}

/// What Windows' programs add to a file's right-click menu there, as nekoland-settings says
/// them: a line an item, its submenus' names and its own, tabs between.
fn windows_menu(said: &str) -> gio::Menu {
    let menu = gio::Menu::new();
    let mut submenus: Vec<(String, gio::Menu)> = Vec::new();
    for line in said.lines() {
        let mut into = menu.clone();
        let (mut above, names) = (String::new(), line.split('\t').collect::<Vec<_>>());
        let Some((label, parents)) = names.split_last() else {
            continue;
        };
        for parent in parents {
            above = format!("{above}\t{parent}");
            into = match submenus.iter().find(|(path, _)| *path == above) {
                Some((_, submenu)) => submenu.clone(),
                None => {
                    let submenu = gio::Menu::new();
                    into.append_submenu(Some(&parent.replace('_', "__")), &submenu);
                    submenus.push((above.clone(), submenu.clone()));
                    submenu
                }
            };
        }
        let item = gio::MenuItem::new(Some(&label.replace('_', "__")), None);
        item.set_action_and_target_value(Some("win.windows-menu"), Some(&line.to_variant()));
        into.append_item(&item);
    }
    menu
}

/// Has Windows do one of the items it has for a file, named as [`windows_menu`] read it.
pub fn windows_item(win: &Rc<Window>, item: &str, file: &Path) {
    let asked = std::process::Command::new("nekoland-settings")
        .arg(WINDOWS_MENU_FLAG)
        .arg(file)
        .args(item.split('\t'))
        .spawn();
    if let Err(err) = asked {
        win.alert("無法執行 Windows 的項目", Some(&err.to_string()));
    }
}

// Opening.

/// Opens what is chosen: a folder in this tab, or in new tabs when there are several;
/// each file in the program that opens its kind.
pub fn open(win: &Weak<Window>, chosen: &[gio::FileInfo]) {
    let Some(win) = win.upgrade() else { return };
    let folders: Vec<gio::File> = chosen.iter().filter(|i| is_dir(i)).filter_map(target).collect();
    match folders.as_slice() {
        [] => {}
        [one] if chosen.len() == 1 => {
            if let Some(pane) = win.pane() {
                pane.go(Where::Folder(one.clone()), true);
            }
        }
        several => {
            for folder in several {
                win.new_tab(Where::Folder(folder.clone()));
            }
        }
    }
    let context = gtk::prelude::WidgetExt::display(&win.window).app_launch_context();
    for info in chosen.iter().filter(|i| !is_dir(i)) {
        let Some(file) = info_file(info) else { continue };
        gtk::RecentManager::default().add_item(&file.uri());
        if let Err(err) = gio::AppInfo::launch_default_for_uri(&file.uri(), Some(&context)) {
            win.alert(
                &format!("無法打開「{}」", info.display_name()),
                Some(err.message()),
            );
        }
    }
}

/// The folder a folder item stands for: a mount's or a link's target, else itself.
pub fn target(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_string("standard::target-uri")
        .map(|uri| gio::File::for_uri(&uri))
        .or_else(|| info_file(info))
}

pub fn open_with(win: &Rc<Window>, app_id: &str, files: &[gio::File]) {
    let Some(app) = gio::DesktopAppInfo::new(app_id) else {
        return;
    };
    let context = gtk::prelude::WidgetExt::display(&win.window).app_launch_context();
    if let Err(err) = app.launch(files, Some(&context)) {
        win.alert(
            &format!("無法以「{}」打開", app.display_name()),
            Some(err.message()),
        );
    }
}

// Copying and moving, by nekoland-fileop.

thread_local! {
    /// The transfers running, by the folder each goes into, with how far each is, 0 to 1.
    static TRANSFERS: RefCell<Vec<(PathBuf, Rc<Cell<f64>>)>> = const { RefCell::new(Vec::new()) };
    /// Told when one starts, moves on or ends; each says whether to go on telling it.
    static WATCHERS: RefCell<Vec<Box<dyn Fn() -> bool>>> = const { RefCell::new(Vec::new()) };
}

/// How far a transfer into `root` or a folder in it is, while one runs.
pub fn transfer_into(root: &Path) -> Option<f64> {
    TRANSFERS.with(|t| {
        t.borrow()
            .iter()
            .find(|(folder, _)| folder.starts_with(root))
            .map(|(_, done)| done.get())
    })
}

pub fn watch_transfers(watcher: impl Fn() -> bool + 'static) {
    WATCHERS.with(|w| w.borrow_mut().push(Box::new(watcher)));
}

pub fn transfers_changed() {
    WATCHERS.with(|w| w.borrow_mut().retain(|watcher| watcher()));
}

/// Copies or moves `paths` into `folder` by nekoland-fileop; `remember` keeps what it did
/// to undo.
pub fn transfer(verb: &str, folder: &Path, paths: &[PathBuf], remember: bool) {
    if paths.is_empty() {
        return;
    }
    // What the folder held, to tell the copies, which may have been given other names.
    let names_before = names_in(folder);
    let (moving, into) = (verb == "move", folder.to_owned());
    let sources = paths.to_vec();
    let spawned = std::process::Command::new(crate::program::path("nekoland-fileop"))
        .arg(verb)
        .arg(folder)
        .args(paths)
        .env(crate::browse::REPORT_PROGRESS, "1")
        .stdout(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            eprintln!("nekoland-fileop: {err}");
            return;
        }
    };
    let Some(stdout) = child.stdout.take() else { return };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            let mut numbers = line.split(' ').filter_map(|n| n.parse::<u64>().ok());
            if let (Some(done), Some(total)) = (numbers.next(), numbers.next()) {
                let fraction = if total > 0 {
                    done as f64 / total as f64
                } else {
                    0.0
                };
                if sender.send(fraction).is_err() {
                    break;
                }
            }
        }
        if let Err(err) = child.wait() {
            eprintln!("nekoland-fileop: {err}");
        }
    });
    let done = Rc::new(Cell::new(0.0));
    TRANSFERS.with(|t| t.borrow_mut().push((folder.to_owned(), done.clone())));
    transfers_changed();
    glib::timeout_add_local(Duration::from_millis(100), move || {
        let mut moved = false;
        loop {
            match receiver.try_recv() {
                Ok(fraction) => {
                    done.set(fraction);
                    moved = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    TRANSFERS.with(|t| t.borrow_mut().retain(|(_, d)| !Rc::ptr_eq(d, &done)));
                    transfers_changed();
                    if remember {
                        undo::record(transferred(moving, &into, &sources, &names_before));
                    }
                    return glib::ControlFlow::Break;
                }
            }
        }
        if moved {
            transfers_changed();
        }
        glib::ControlFlow::Continue
    });
}

fn names_in(folder: &Path) -> std::collections::HashSet<std::ffi::OsString> {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect()
        })
        .unwrap_or_default()
}

/// What a finished transfer did: each moved item that left its place for one in `folder`,
/// or what is in `folder` that was not before the copying.
fn transferred(
    moving: bool,
    folder: &Path,
    sources: &[PathBuf],
    names_before: &std::collections::HashSet<std::ffi::OsString>,
) -> Change {
    if moving {
        Change::Moved(
            sources
                .iter()
                .filter(|source| std::fs::symlink_metadata(source).is_err())
                .filter_map(|source| Some((source.clone(), folder.join(source.file_name()?))))
                .filter(|(_, moved)| std::fs::symlink_metadata(moved).is_ok())
                .collect(),
        )
    } else {
        let mut made: Vec<PathBuf> = names_in(folder)
            .difference(names_before)
            .map(|name| folder.join(name))
            .collect();
        made.sort();
        Change::Made(made)
    }
}

/// What is dropped on a folder: moved when it is on the same drive, copied when it is not
/// or Ctrl is held, as the Finder decides.
/// Has a drop target ask for a move of what the source lets go of, and a copy only of what
/// it does not: GTK would ask for a copy first, and on Wayland a drop says then only the
/// action asked for, so `drop_into` would copy everything.
pub fn prefer_move(target: &gtk::DropTarget) {
    target.connect_motion(|target, _, _| drop_action(target));
}

/// What a sidebar bookmark's drag says before where it goes: only where it goes, so that
/// dropped on a folder, the folder itself is not moved there.
pub const BOOKMARK_PREFIX: &str = "nekoland-bookmark:";

/// Whether a drop is a sidebar bookmark being moved, which only the sidebar takes: a drag of
/// Files' own, read as it comes. GTK would read its text as files for any other folder.
pub fn is_bookmark_drag(drop: &gdk::Drop) -> bool {
    drop.drag()
        .map(|drag| drag.content())
        .filter(|content| content.formats().contains_type(String::static_type()))
        .and_then(|content| content.value(String::static_type()).ok())
        .and_then(|value| value.get::<String>().ok())
        .is_some_and(|text| text.starts_with(BOOKMARK_PREFIX))
}

/// The action `prefer_move` asks for; none for a bookmark being moved.
pub fn drop_action(target: &gtk::DropTarget) -> gdk::DragAction {
    if target.current_drop().is_some_and(|drop| is_bookmark_drag(&drop)) {
        return gdk::DragAction::empty();
    }
    let offered = target.current_drop().map_or(gdk::DragAction::empty(), |drop| drop.actions()) & target.actions();
    [gdk::DragAction::MOVE, gdk::DragAction::COPY]
        .into_iter()
        .find(|action| offered.contains(*action))
        .unwrap_or(gdk::DragAction::empty())
}

pub fn drop_into(files: &[gio::File], folder: &gio::File, target: &gtk::DropTarget) {
    let Some(folder) = folder.path() else { return };
    let modifiers = target.current_event_state();
    // A source that only lets go of copies, as Preview its open file, is copied.
    let movable = target
        .current_drop()
        .is_none_or(|drop| drop.actions().contains(gdk::DragAction::MOVE));
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| f.path()).collect();
    let device = |path: &Path| std::fs::metadata(path).map(|m| m.dev()).ok();
    let same_drive = paths
        .iter()
        .all(|p| device(p).is_some() && device(p) == device(&folder));
    if same_drive && movable && !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        let moving: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| *p != folder && p.parent() != Some(folder.as_path()))
            .collect();
        transfer("move", &folder, &moving, true);
    } else {
        transfer("copy", &folder, &paths, true);
    }
}

/// A picture dropped with no file to it, as from a web page: kept as a PNG in `folder`.
pub fn drop_picture(win: &Window, texture: &gdk::Texture, folder: &gio::File) {
    let Some(folder) = folder.path() else { return };
    let path = free_name(&folder.join("圖片.png"));
    if let Err(err) = texture.save_to_png(&path) {
        win.alert("無法儲存圖片", Some(&err.message));
        return;
    }
    undo::record(Change::Made(vec![path]));
}

/// The file whose bytes a drag carries with no file to it, as Chromium drags a picture
/// out of a web page: `application/octet-stream;name="cat.png"`, and its name.
pub fn dropped_file(formats: &gdk::ContentFormats) -> Option<(glib::GString, String)> {
    formats.mime_types().into_iter().find_map(|mime| {
        let name = mime.strip_prefix("application/octet-stream;name=")?.trim_matches('"');
        // Only a name: the program says it, not where it goes.
        let name = Path::new(name).file_name()?.to_string_lossy().into_owned();
        Some((mime, name))
    })
}

/// Writes the bytes of `dropped_file` into `folder`, under its name or the next free one.
pub fn drop_file_bytes(win: Weak<Window>, drop: gdk::Drop, mime: glib::GString, name: String, folder: PathBuf) {
    glib::spawn_future_local(async move {
        let path = folder.join(&name);
        let path = if path.exists() { free_name(&path) } else { path };
        let written = async {
            let (bytes, _) = drop.read_future(&[mime.as_str()], glib::Priority::DEFAULT).await?;
            let file = gio::File::for_path(&path);
            let out = file
                .create_future(gio::FileCreateFlags::NONE, glib::Priority::DEFAULT)
                .await?;
            let flags = gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET;
            out.splice_future(&bytes, flags, glib::Priority::DEFAULT).await
        }
        .await;
        match written {
            Ok(_) => {
                undo::record(Change::Made(vec![path]));
                drop.finish(gdk::DragAction::COPY);
            }
            Err(err) => {
                drop.finish(gdk::DragAction::empty());
                let _ = std::fs::remove_file(&path);
                if let Some(win) = win.upgrade() {
                    win.alert(&format!("無法儲存「{name}」"), Some(err.message()));
                }
            }
        }
    });
}

pub fn duplicate(files: &[gio::File]) {
    for file in files {
        if let (Some(path), Some(folder)) = (file.path(), file.parent().and_then(|p| p.path())) {
            transfer("copy", &folder, &[path], true);
        }
    }
}

// The clipboard, as Files and the desktop share it.

pub fn copy_to_clipboard(win: &Window, files: &[gio::File], cut: bool) {
    if files.is_empty() {
        return;
    }
    let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
    let copied = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
    let list = uris.iter().map(|u| format!("{u}\r\n")).collect::<String>();
    let provider = gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_bytes(COPIED_FILES, &glib::Bytes::from_owned(copied.into_bytes())),
        gdk::ContentProvider::for_bytes(URI_LIST, &glib::Bytes::from_owned(list.into_bytes())),
        gdk::ContentProvider::for_value(&gdk::FileList::from_array(files).to_value()),
    ]);
    if let Err(err) = win.window.clipboard().set_content(Some(&provider)) {
        eprintln!("nekoland-files: cannot copy: {err}");
    }
}

/// The paths of `files`, a line each, as text.
pub fn copy_paths(win: &Window, files: &[gio::File]) {
    let paths: Vec<String> = files
        .iter()
        .map(|f| {
            f.path()
                .map_or_else(|| f.uri().to_string(), |p| p.display().to_string())
        })
        .collect();
    if !paths.is_empty() {
        win.window.clipboard().set_text(&paths.join("\n"));
    }
}

/// Files copied or cut anywhere go into `folder`: copied, or moved when they were cut.
pub fn paste(win: &Rc<Window>, folder: gio::File) {
    let Some(into) = folder.path() else { return };
    let clipboard = win.window.clipboard();
    clipboard.read_async(
        &[COPIED_FILES, URI_LIST],
        glib::Priority::DEFAULT,
        gio::Cancellable::NONE,
        move |read| {
            let Ok((stream, mime_type)) = read else { return };
            let bytes = gio::MemoryOutputStream::new_resizable();
            let flags =
                gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET;
            let kept = bytes.clone();
            bytes.splice_async(
                &stream,
                flags,
                glib::Priority::DEFAULT,
                gio::Cancellable::NONE,
                move |done| {
                    if done.is_err() {
                        return;
                    }
                    let text = String::from_utf8_lossy(&kept.steal_as_bytes()).into_owned();
                    let (cut, uris) = parse_clipboard(&text, &mime_type);
                    let paths: Vec<PathBuf> =
                        uris.iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
                    transfer(if cut { "move" } else { "copy" }, &into, &paths, true);
                },
            );
        },
    );
}

/// Whether the files were cut, and their URIs, from what the clipboard holds.
fn parse_clipboard(text: &str, mime_type: &str) -> (bool, Vec<String>) {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let cut = if mime_type == COPIED_FILES {
        lines.next() == Some("cut")
    } else {
        false
    };
    (cut, lines.map(str::to_owned).collect())
}

// The trash.

pub fn trash(win: &Rc<Window>, files: Vec<gio::File>) {
    // How many are still on their way, and the places of those in the trash, to undo at once.
    let trashed = Rc::new(RefCell::new((files.len(), Vec::new())));
    for file in files {
        let win = Rc::downgrade(win);
        let name = display_name(&file);
        let trashed = trashed.clone();
        file.clone()
            .trash_async(glib::Priority::DEFAULT, gio::Cancellable::NONE, move |done| {
                let mut trashed = trashed.borrow_mut();
                trashed.0 -= 1;
                match done {
                    Ok(()) => trashed.1.extend(file.path()),
                    Err(err) => {
                        if let Some(win) = win.upgrade() {
                            win.alert(&format!("無法把「{name}」丟到垃圾桶"), Some(err.message()));
                        }
                    }
                }
                if trashed.0 == 0 {
                    undo::record(Change::Trashed(std::mem::take(&mut trashed.1)));
                }
            });
    }
}

/// Puts what is in the trash back where it was.
pub fn restore(win: &Rc<Window>, chosen: &[gio::FileInfo]) {
    let mut restored = Vec::new();
    for info in chosen {
        let (Some(file), Some(orig)) = (info_file(info), info.attribute_byte_string("trash::orig-path"))
        else {
            continue;
        };
        let orig = PathBuf::from(orig.as_str());
        match put_back(&file, &orig) {
            Ok(()) => restored.push(orig),
            Err(err) => win.alert(
                &format!("無法把「{}」放回原處", info.display_name()),
                Some(err.message()),
            ),
        }
    }
    undo::record(Change::Made(restored));
}

/// Moves an item in the trash back to where it came from.
pub fn put_back(item: &gio::File, orig: &Path) -> Result<(), glib::Error> {
    let to = gio::File::for_path(orig);
    if let Some(parent) = to.parent() {
        let _ = parent.make_directory_with_parents(gio::Cancellable::NONE);
    }
    item.move_(
        &to,
        gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
        gio::Cancellable::NONE,
        None,
    )
}

/// Deletes for good, after asking: what is chosen, in the trash or anywhere.
pub fn delete_now(win: &Rc<Window>, files: Vec<gio::File>) {
    if files.is_empty() {
        return;
    }
    let title = match files.as_slice() {
        [one] => format!("永久刪除「{}」？", display_name(one)),
        several => format!("永久刪除 {} 個項目？", several.len()),
    };
    win.confirm(&title, "刪除後無法復原。", "刪除", move |win| {
        delete_all(win, files)
    });
}

pub fn empty_trash(win: &Rc<Window>) {
    let trash = crate::pane::trash();
    let children: Vec<gio::File> = trash
        .enumerate_children(
            "standard::name",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map(|children| {
            children
                .filter_map(Result::ok)
                .map(|info| trash.child(info.name()))
                .collect()
        })
        .unwrap_or_default();
    if children.is_empty() {
        return;
    }
    let body = format!("{} 個項目會永久刪除，無法復原。", children.len());
    win.confirm("清空垃圾桶？", &body, "清空垃圾桶", move |win| {
        delete_all(win, children)
    });
}

/// Deletes items, folders whole, away from the window's thread. The trash deletes a folder
/// with what it holds itself, and refuses to change what is inside one; a folder on a disk
/// is emptied first.
fn delete_all(win: &Rc<Window>, files: Vec<gio::File>) {
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for file in files {
            let deleted = match file.path() {
                Some(path) if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) => {
                    std::fs::remove_dir_all(&path).map_err(|err| err.to_string())
                }
                _ => file
                    .delete(gio::Cancellable::NONE)
                    .map_err(|err| err.message().to_owned()),
            };
            if let Err(err) = deleted {
                let _ = sender.send(format!("{}：{err}", display_name(&file)));
            }
        }
    });
    let win = Rc::downgrade(win);
    glib::timeout_add_local(Duration::from_millis(100), move || match receiver.try_recv() {
        Ok(failed) => {
            if let Some(win) = win.upgrade() {
                win.alert("無法刪除", Some(&failed));
            }
            glib::ControlFlow::Continue
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => glib::ControlFlow::Break,
    });
}

// Making things.

pub fn new_folder(win: &Rc<Window>, pane: &Rc<Pane>) {
    make_new(
        win,
        pane,
        NEW_FOLDER,
        |file| file.make_directory(gio::Cancellable::NONE),
        "無法建立檔案夾",
    );
}

pub fn new_text_file(win: &Rc<Window>, pane: &Rc<Pane>) {
    make_new(
        win,
        pane,
        NEW_TEXT_FILE,
        // Closed at once: the folder's watch tells of a new file once it is written.
        |file| {
            file.create(gio::FileCreateFlags::NONE, gio::Cancellable::NONE)?
                .close(gio::Cancellable::NONE)
        },
        "無法建立文件",
    );
}

/// Makes a new item of this name, or the next free one, where the tab is, chosen with its
/// name to type over.
fn make_new(
    win: &Rc<Window>,
    pane: &Rc<Pane>,
    name: &str,
    make: impl FnOnce(&gio::File) -> Result<(), glib::Error>,
    failed: &str,
) {
    let Some(folder) = pane.here().folder().and_then(|f| f.path()) else {
        return;
    };
    let path = folder.join(name);
    let path = if path.exists() { free_name(&path) } else { path };
    let file = gio::File::for_path(&path);
    if let Err(err) = make(&file) {
        win.alert(failed, Some(err.message()));
        return;
    }
    undo::record(Change::Made(vec![path]));
    let win = Rc::downgrade(win);
    pane.select_when_listed(file.clone(), move |_| {
        if let Some(win) = win.upgrade() {
            rename_one(&win, file);
        }
    });
}

/// A name for one item, with its name before the extension chosen to type over.
pub fn rename_one(win: &Rc<Window>, file: gio::File) {
    let name = display_name(&file);
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].chars().count() as i32,
        _ => -1,
    };
    let weak = Rc::downgrade(win);
    let shown = name.clone();
    crate::alert::prompt(
        &win.window,
        "重新命名",
        &shown,
        stem,
        "重新命名",
        move |new| {
            let Some(win) = weak.upgrade() else { return };
            if new == name {
                return;
            }
            if new.contains('/') {
                win.alert("名稱不能含有「/」", None);
                return;
            }
            match file.set_display_name(&new, gio::Cancellable::NONE) {
                Ok(renamed) => {
                    if let (Some(before), Some(after)) = (file.path(), renamed.path()) {
                        undo::record(Change::Renamed(vec![(before, after)]));
                    }
                    if let Some(pane) = win.pane() {
                        pane.select_when_listed(renamed, |_| {});
                    }
                }
                Err(err) => win.alert(&format!("無法重新命名「{name}」"), Some(err.message())),
            }
        },
    );
}

pub fn rename(win: &Rc<Window>, files: Vec<gio::File>) {
    match files.len() {
        0 => {}
        1 => rename_one(win, files.into_iter().next().expect("one")),
        _ => {
            let weak = Rc::downgrade(win);
            let before: Vec<Option<PathBuf>> = files.iter().map(|f| f.path()).collect();
            rename::ask(&win.window, files, move |done| match done {
                Ok(renamed) => undo::record(Change::Renamed(
                    before
                        .iter()
                        .zip(&renamed)
                        .filter_map(|(before, after)| Some((before.clone()?, after.path()?)))
                        .filter(|(before, after)| before != after)
                        .collect(),
                )),
                Err(err) => {
                    if let Some(win) = weak.upgrade() {
                        win.alert("無法重新命名", Some(err.message()));
                    }
                }
            });
        }
    }
}

/// Symbolic links to each chosen item, beside it.
pub fn make_links(win: &Rc<Window>, files: &[gio::File]) {
    let mut made = Vec::new();
    for file in files {
        let (Some(path), Some(folder)) = (file.path(), file.parent().and_then(|p| p.path())) else {
            continue;
        };
        let link = folder.join(format!("{} 的連結", display_name(file)));
        let link = if std::fs::symlink_metadata(&link).is_ok() {
            free_name(&link)
        } else {
            link
        };
        match std::os::unix::fs::symlink(&path, &link) {
            Ok(()) => made.push(link),
            Err(err) => win.alert(
                &format!("無法製作「{}」的連結", display_name(file)),
                Some(&err.to_string()),
            ),
        }
    }
    undo::record(Change::Made(made));
}

/// A ZIP archive of what is chosen, beside it, by bsdtar or zip; its row shows how far
/// along it is.
pub fn compress(win: &Rc<Window>, pane: &Rc<Pane>, files: &[gio::File]) {
    let Some(folder) = files.first().and_then(|f| f.parent()).and_then(|p| p.path()) else {
        return;
    };
    let names: Vec<String> = files
        .iter()
        .filter_map(|f| f.basename())
        .map(|b| b.to_string_lossy().into_owned())
        .collect();
    let name = match names.as_slice() {
        [one] => format!("{one}.zip"),
        _ => "封存.zip".to_string(),
    };
    let target = folder.join(name);
    let target = if target.exists() {
        free_name(&target)
    } else {
        target
    };
    // Weighed away from the window's thread: a large folder takes a while to walk.
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| f.path()).collect();
    let (sender, sized) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(paths.iter().map(|p| size_of(p)).sum::<u64>().max(1));
    });
    let mut total = None;
    let have = |program: &str| glib::find_program_in_path(program).is_some();
    let mut command = if have("bsdtar") {
        let mut command = std::process::Command::new("bsdtar");
        command
            .args(["-a", "-cf"])
            .arg(&target)
            .arg("-C")
            .arg(&folder)
            .arg("--")
            .args(&names);
        command
    } else {
        let mut command = std::process::Command::new("zip");
        command
            .args(["-r", "-q", "-y"])
            .arg(&target)
            .arg("--")
            .args(&names)
            .current_dir(&folder);
        command
    };
    let child = match command
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            win.alert("無法壓縮", Some(&err.to_string()));
            return;
        }
    };
    let uri = gio::File::for_path(&target).uri().to_string();
    pane.set_job(&uri, Some(0.0));
    let (pane, win) = (Rc::downgrade(pane), Rc::downgrade(win));
    let child = std::cell::RefCell::new(child);
    glib::timeout_add_local(Duration::from_millis(150), move || {
        let done = child.borrow_mut().try_wait();
        let pane = pane.upgrade();
        match done {
            Ok(None) => {
                total = total.or_else(|| sized.try_recv().ok());
                // How much has been written of what goes in, which compression shrinks: never full until done.
                let written = std::fs::metadata(&target).map_or(0, |m| m.len());
                if let (Some(pane), Some(total)) = (&pane, total) {
                    pane.set_job(&uri, Some((written as f64 / total as f64).min(0.95)));
                }
                glib::ControlFlow::Continue
            }
            finished => {
                if let Some(pane) = &pane {
                    pane.set_job(&uri, None);
                }
                if matches!(finished, Ok(Some(status)) if status.success()) {
                    undo::record(Change::Made(vec![target.clone()]));
                } else {
                    let _ = std::fs::remove_file(&target);
                    if let Some(win) = win.upgrade() {
                        win.alert("無法壓縮", None);
                    }
                }
                glib::ControlFlow::Break
            }
        }
    });
}

pub fn is_archive(info: &gio::FileInfo) -> bool {
    info.content_type().is_some_and(|kind| {
        ARCHIVES
            .iter()
            .any(|archive| gio::content_type_is_a(&kind, archive))
    })
}

/// An archive's name without what says it is one: `photos` of `photos.tar.gz`.
fn archive_stem(name: &str) -> &str {
    let lower = name.to_lowercase();
    for suffix in [
        ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tgz", ".tbz2", ".txz",
    ] {
        if lower.ends_with(suffix) && lower.len() > suffix.len() {
            return &name[..name.len() - suffix.len()];
        }
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// Takes an archive apart beside it, by bsdtar: what it holds in a folder named after it,
/// or its one item alone, as the Finder does; that is chosen once there.
pub fn extract(win: &Rc<Window>, pane: &Rc<Pane>, archive: &gio::File) {
    let (Some(path), Some(folder)) = (archive.path(), archive.parent().and_then(|p| p.path())) else {
        return;
    };
    let into = folder.join(archive_stem(&display_name(archive)));
    let into = if std::fs::symlink_metadata(&into).is_ok() {
        free_name(&into)
    } else {
        into
    };
    if let Err(err) = std::fs::create_dir(&into) {
        win.alert("無法解壓縮", Some(&err.to_string()));
        return;
    }
    let (sender, receiver) = mpsc::channel();
    let taking_apart = into.clone();
    std::thread::spawn(move || {
        let output = std::process::Command::new("bsdtar")
            .arg("-xf")
            .arg(&path)
            .arg("-C")
            .arg(&taking_apart)
            .stdin(std::process::Stdio::null())
            .output();
        let _ = sender.send(output);
    });
    let (pane, win) = (Rc::downgrade(pane), Rc::downgrade(win));
    glib::timeout_add_local(Duration::from_millis(150), move || {
        let finished = match receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
            Ok(finished) => finished,
        };
        let failed = match &finished {
            Ok(output) if output.status.success() => None,
            Ok(output) => Some(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
            Err(err) => Some(err.to_string()),
        };
        if let Some(failed) = failed {
            let _ = std::fs::remove_dir_all(&into);
            if let Some(win) = win.upgrade() {
                win.alert(
                    "無法解壓縮",
                    Some(&failed).filter(|f| !f.is_empty()).map(String::as_str),
                );
            }
            return glib::ControlFlow::Break;
        }
        let made = hoist_only_item(&into);
        undo::record(Change::Made(vec![made.clone()]));
        if let Some(pane) = pane.upgrade() {
            pane.select_when_listed(gio::File::for_path(&made), |_| {});
        }
        glib::ControlFlow::Break
    });
}

/// The one item a folder holds put in its place, or the folder when it holds more.
fn hoist_only_item(folder: &Path) -> PathBuf {
    let entries: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    let ([only], Some(parent)) = (entries.as_slice(), folder.parent()) else {
        return folder.to_owned();
    };
    let Some(name) = only.file_name() else {
        return folder.to_owned();
    };
    let to = parent.join(name);
    // Its own name is the folder's own until the folder is gone.
    let to = if to != folder && std::fs::symlink_metadata(&to).is_ok() {
        free_name(&to)
    } else {
        to
    };
    let through = parent.join(format!(".nekoland-extract-{}", std::process::id()));
    let hoisted = std::fs::rename(only, &through)
        .and_then(|()| std::fs::remove_dir(folder))
        .and_then(|()| std::fs::rename(&through, &to));
    match hoisted {
        Ok(()) => to,
        Err(err) => {
            eprintln!(
                "nekoland-files: cannot take the one item out of {}: {err}",
                folder.display()
            );
            if through.exists() && !folder.exists() {
                let _ =
                    std::fs::create_dir(folder).and_then(|()| std::fs::rename(&through, folder.join(name)));
            }
            folder.to_owned()
        }
    }
}

// Tags, pictures and the sidebar.

pub fn set_tag(win: &Rc<Window>, chosen: &[gio::FileInfo], tag: &str, on: bool) {
    for info in chosen {
        let Some(file) = info_file(info) else { continue };
        let tags = tags::toggled(&tags::of(info), tag, on);
        match tags::write(&file, &tags) {
            // The listing reads the attribute again when told the file changed.
            Ok(()) => info.set_attribute_string(tags::ATTRIBUTE, &tags.join(",")),
            Err(err) => {
                win.alert(
                    &format!("無法替「{}」加上標籤", info.display_name()),
                    Some(err.message()),
                );
                return;
            }
        }
    }
}

/// Every tag, the seven and those the chosen items carry, to check or clear on all of them,
/// and a field for new ones, a comma between two.
pub fn edit_tags(win: &Rc<Window>, chosen: Vec<gio::FileInfo>) {
    let mut names: Vec<String> = tags::TAGS.iter().map(|(tag, _)| tag.to_string()).collect();
    for info in &chosen {
        for tag in tags::of(info) {
            if !names.contains(&tag) {
                names.push(tag);
            }
        }
    }
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let mut checks = Vec::new();
    for name in names {
        let carried = chosen.iter().filter(|i| tags::of(i).contains(&name)).count();
        let all = carried > 0 && carried == chosen.len();
        let check = gtk::CheckButton::builder()
            .active(all)
            .inconsistent(carried > 0 && !all)
            .valign(gtk::Align::Center)
            .build();
        check.connect_toggled(|check| check.set_inconsistent(false));
        let row = adw::ActionRow::builder()
            .title(&name)
            .use_markup(false)
            .activatable_widget(&check)
            .build();
        row.add_prefix(&tags::dot(&name));
        row.add_suffix(&check);
        list.append(&row);
        checks.push((name, check, all));
    }
    let entry = gtk::Entry::builder()
        .placeholder_text("加入標籤")
        .activates_default(true)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(crate::browse::GAP)
        .width_request(crate::alert::FIELD_WIDTH)
        .build();
    content.append(&entry);
    content.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(400)
            .child(&list)
            .build(),
    );
    let alert = adw::AlertDialog::new(Some("所有標籤"), None);
    alert.set_extra_child(Some(&content));
    alert.add_responses(&[("cancel", "取消"), ("apply", "套用")]);
    alert.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("apply"));
    alert.set_close_response("cancel");
    let weak = Rc::downgrade(win);
    alert.choose(&win.window, gio::Cancellable::NONE, move |response| {
        let Some(win) = weak.upgrade() else { return };
        if response != "apply" {
            return;
        }
        for (name, check, was) in &checks {
            if !check.is_inconsistent() && check.is_active() != *was {
                set_tag(&win, &chosen, name, check.is_active());
            }
        }
        for name in tags::parse(&entry.text()) {
            set_tag(&win, &chosen, &name, true);
        }
        if let Some(pane) = win.pane() {
            pane.refilter();
            win.selection_changed(&pane);
        }
    });
}

/// Uses a picture as the desktop's, as Settings does.
pub fn set_wallpaper(win: &Rc<Window>, file: &gio::File) {
    let Some(path) = file.path() else { return };
    let command = format!("set wallpaper = {}", toml::Value::from(path.to_string_lossy().as_ref()).to_string());
    match std::process::Command::new("nekoland")
        .args(["msg", &command])
        .output()
    {
        Ok(output) if output.status.success() && output.stdout.iter().all(u8::is_ascii_whitespace) => {}
        Ok(output) => win.alert(
            "無法設為桌布",
            Some(String::from_utf8_lossy(&output.stdout).trim()),
        ),
        Err(err) => win.alert("無法設為桌布", Some(&err.to_string())),
    }
}

/// Turns pictures a quarter to the left and saves them so, away from the window's thread.
pub fn rotate_left(win: &Rc<Window>, files: Vec<gio::File>) {
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for path in files.iter().filter_map(|f| f.path()) {
            if let Err(err) = rotate(&path) {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let _ = sender.send(format!("{name}：{err}"));
            }
        }
    });
    let win = Rc::downgrade(win);
    glib::timeout_add_local(Duration::from_millis(100), move || match receiver.try_recv() {
        Ok(failed) => {
            if let Some(win) = win.upgrade() {
                win.alert("無法旋轉", Some(&failed));
            }
            glib::ControlFlow::Continue
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => glib::ControlFlow::Break,
    });
}

fn rotate(path: &Path) -> Result<(), glib::Error> {
    let failed = |err: std::io::Error| glib::Error::new(gio::IOErrorEnum::Failed, &err.to_string());
    // A link stays a link, to the picture turned.
    let path = &std::fs::canonicalize(path).map_err(failed)?;
    let (format, _, _) = Pixbuf::file_info(path)
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::NotSupported, "不是能讀取的圖片"))?;
    let kind = format.name().map(|n| n.to_string()).unwrap_or_default();
    let pixbuf = Pixbuf::from_file(path)?;
    let upright = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let turned = upright
        .rotate_simple(PixbufRotation::Counterclockwise)
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "無法旋轉"))?;
    let options: &[(&str, &str)] = if kind == "jpeg" { &[("quality", "95")] } else { &[] };
    // Written beside it and moved over it, so a failure leaves the picture as it was.
    let temporary = path.with_file_name(format!(
        ".{}.rotating",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    // shortcut: gdk-pixbuf writes no EXIF, so the date taken and the like are not kept; a
    // lossless JPEG rotation (jpegtran) if they are missed.
    let written = turned.savev(&temporary, &kind, options).and_then(|()| {
        // Only those who could read the picture can read it turned.
        let permissions = std::fs::metadata(path).map_err(failed)?.permissions();
        std::fs::set_permissions(&temporary, permissions).map_err(failed)?;
        std::fs::rename(&temporary, path).map_err(failed)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

pub fn add_bookmark(win: &Rc<Window>, folder: &gio::File) {
    let path = bookmarks_path();
    let Some(mut text) = read_bookmarks(win, &path) else { return };
    let uri = folder.uri();
    if parse_bookmarks(&text).iter().any(|(u, _)| *u == uri.as_str()) {
        return;
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("{uri}\n"));
    write_bookmarks(win, &path, &text);
}

pub fn remove_bookmark(win: &Rc<Window>, folder: &gio::File) {
    let path = bookmarks_path();
    let Some(text) = read_bookmarks(win, &path) else { return };
    let uri = folder.uri();
    let kept: String = text
        .lines()
        .filter(|line| line.split(' ').next() != Some(uri.as_str()))
        .map(|line| format!("{line}\n"))
        .collect();
    write_bookmarks(win, &path, &kept);
}

/// Puts `folder` among the bookmarks just before `before`, or last without it: added, or
/// moved there with its name when it is one already.
pub fn place_bookmark(win: &Rc<Window>, folder: &gio::File, before: Option<&gio::File>) {
    let path = bookmarks_path();
    let Some(text) = read_bookmarks(win, &path) else { return };
    let before = before.map(|b| b.uri());
    write_bookmarks(win, &path, &bookmarks_placed(&text, &folder.uri(), before.as_deref()));
}

/// The bookmarks file's `text` with `uri`'s line, or a new one, just before `before`'s.
fn bookmarks_placed(text: &str, uri: &str, before: Option<&str>) -> String {
    let is_of = |line: &str, uri: &str| line.split(' ').next() == Some(uri);
    if before == Some(uri) {
        return text.to_owned();
    }
    let line = text.lines().find(|line| is_of(line, uri)).unwrap_or(uri);
    let mut lines: Vec<&str> = text.lines().filter(|l| !is_of(l, uri)).collect();
    let at = before
        .and_then(|before| lines.iter().position(|l| is_of(l, before)))
        .unwrap_or(lines.len());
    lines.insert(at, line);
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// The bookmarks file's text, empty when there is none yet. One that cannot be read is not
/// written over either.
fn read_bookmarks(win: &Rc<Window>, path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some(String::new()),
        Err(err) => {
            win.alert("無法更改側欄", Some(&err.to_string()));
            None
        }
    }
}

fn write_bookmarks(win: &Rc<Window>, path: &Path, text: &str) {
    // Written beside it and moved over it, so a failure leaves the bookmarks as they were;
    // over the file a link there points to, as one from a dotfiles folder.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let temporary = target.with_file_name(".bookmarks.partial");
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&temporary, text))
        .and_then(|()| std::fs::rename(&temporary, &target));
    match written {
        Ok(()) => win.places_changed(),
        Err(err) => win.alert("無法更改側欄", Some(&err.to_string())),
    }
}

pub fn display_name(file: &gio::File) -> String {
    file.query_info(
        "standard::display-name",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    )
    .map(|info| info.display_name().to_string())
    .ok()
    .or_else(|| file.basename().map(|b| b.to_string_lossy().into_owned()))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_are_placed_before_another_with_their_names() {
        let text = "file:///a\nfile:///b Bee\nsftp://host/c\n";
        assert_eq!(bookmarks_placed(text, "file:///b", Some("file:///a")), "file:///b Bee\nfile:///a\nsftp://host/c\n");
        assert_eq!(bookmarks_placed(text, "file:///a", None), "file:///b Bee\nsftp://host/c\nfile:///a\n");
        assert_eq!(bookmarks_placed(text, "file:///d", Some("sftp://host/c")), "file:///a\nfile:///b Bee\nfile:///d\nsftp://host/c\n");
        assert_eq!(bookmarks_placed(text, "file:///a", Some("file:///a")), text);
    }

    #[test]
    fn cut_files_are_told_from_copied_ones() {
        assert_eq!(
            parse_clipboard("cut\nfile:///a\nfile:///b", COPIED_FILES),
            (true, vec!["file:///a".to_string(), "file:///b".to_string()])
        );
        assert_eq!(
            parse_clipboard("copy\nfile:///a\n", COPIED_FILES),
            (false, vec!["file:///a".to_string()])
        );
        assert_eq!(
            parse_clipboard("# a comment\r\nfile:///a\r\n", URI_LIST),
            (false, vec!["file:///a".to_string()])
        );
    }

    #[test]
    fn an_archive_is_named_without_its_kind() {
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("Photos.TGZ"), "Photos");
        assert_eq!(archive_stem("報告.zip"), "報告");
        assert_eq!(archive_stem("v1.2.7z"), "v1.2");
        assert_eq!(archive_stem(".tar.gz"), ".tar");
        assert_eq!(archive_stem("README"), "README");
    }

    #[test]
    fn the_one_item_of_an_archive_takes_its_folders_place() {
        let dir = std::env::temp_dir().join(format!("nekoland-hoist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Named as the folder: in its place.
        std::fs::create_dir_all(dir.join("project/project")).unwrap();
        assert_eq!(hoist_only_item(&dir.join("project")), dir.join("project"));
        assert!(!dir.join("project/project").exists());
        // Named as another item beside it: the next free name.
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/b.txt"), "B").unwrap();
        std::fs::write(dir.join("b.txt"), "taken").unwrap();
        let hoisted = hoist_only_item(&dir.join("a"));
        assert_eq!(std::fs::read_to_string(&hoisted).unwrap(), "B");
        assert!(!dir.join("a").exists());
        // Several: the folder stays.
        std::fs::create_dir_all(dir.join("c")).unwrap();
        std::fs::write(dir.join("c/1"), "").unwrap();
        std::fs::write(dir.join("c/2"), "").unwrap();
        assert_eq!(hoist_only_item(&dir.join("c")), dir.join("c"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picture_turns_left_in_place() {
        let path = std::env::temp_dir().join(format!("nekoland-rotate-{}.png", std::process::id()));
        let pixbuf = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 4, 2).unwrap();
        pixbuf.savev(&path, "png", &[]).unwrap();
        rotate(&path).unwrap();
        let (_, width, height) = Pixbuf::file_info(&path).unwrap();
        assert_eq!((width, height), (2, 4));
        std::fs::remove_file(&path).unwrap();
    }
}
