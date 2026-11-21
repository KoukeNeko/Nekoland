//! What can be done to files: opening, the trash, copying through the clipboard, new
//! folders, renaming, compressing, tags, and the menus that offer all of it.

use std::{
    cell::{Cell, RefCell},
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
    rename, tags, Window,
};

/// The clipboard types Files and the desktop copy files as.
const COPIED_FILES: &str = "x-special/gnome-copied-files";
const URI_LIST: &str = "text/uri-list";
/// The name a new folder is given, as the desktop names one.
pub const NEW_FOLDER: &str = "未命名檔案夾";

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
    doing.append(Some("製作複本"), Some("win.duplicate"));
    doing.append(Some("製作連結"), Some("win.link"));
    doing.append(Some("快速查看"), Some("win.quick-look"));
    menu.append_section(None, &doing);
    menu.append_section(None, &section(&[("拷貝", "win.copy")]));

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
        ("大小", "size"),
    ] {
        menu.append(Some(label), Some(&format!("win.sort::{key}")));
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
fn target(info: &gio::FileInfo) -> Option<gio::File> {
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

/// nekoland-fileop next to this program, as an install puts them, else on PATH.
fn fileop() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("nekoland-fileop")))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("nekoland-fileop"))
}

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

fn transfers_changed() {
    WATCHERS.with(|w| w.borrow_mut().retain(|watcher| watcher()));
}

pub fn transfer(verb: &str, folder: &Path, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let spawned = std::process::Command::new(fileop())
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

/// What is dropped on a folder: moved when it is on the same drive, copied when it is not
/// or Ctrl is held, as the Finder decides.
pub fn drop_into(files: &[gio::File], folder: &gio::File, modifiers: gdk::ModifierType) {
    let Some(folder) = folder.path() else { return };
    let paths: Vec<PathBuf> = files.iter().filter_map(|f| f.path()).collect();
    let device = |path: &Path| std::fs::metadata(path).map(|m| m.dev()).ok();
    let same_drive = paths
        .iter()
        .all(|p| device(p).is_some() && device(p) == device(&folder));
    if same_drive && !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        let moving: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| *p != folder && p.parent() != Some(folder.as_path()))
            .collect();
        transfer("move", &folder, &moving);
    } else {
        transfer("copy", &folder, &paths);
    }
}

pub fn duplicate(files: &[gio::File]) {
    for file in files {
        if let (Some(path), Some(folder)) = (file.path(), file.parent().and_then(|p| p.path())) {
            transfer("copy", &folder, &[path]);
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
                    transfer(if cut { "move" } else { "copy" }, &into, &paths);
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
    for file in files {
        let win = Rc::downgrade(win);
        let name = display_name(&file);
        file.trash_async(glib::Priority::DEFAULT, gio::Cancellable::NONE, move |done| {
            if let (Err(err), Some(win)) = (done, win.upgrade()) {
                win.alert(&format!("無法把「{name}」丟到垃圾桶"), Some(err.message()));
            }
        });
    }
}

/// Puts what is in the trash back where it was.
pub fn restore(win: &Rc<Window>, chosen: &[gio::FileInfo]) {
    for info in chosen {
        let (Some(file), Some(orig)) = (info_file(info), info.attribute_byte_string("trash::orig-path"))
        else {
            continue;
        };
        let to = gio::File::for_path(orig.as_str());
        if let Some(parent) = to.parent() {
            let _ = parent.make_directory_with_parents(gio::Cancellable::NONE);
        }
        if let Err(err) = file.move_(
            &to,
            gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
            None,
        ) {
            win.alert(
                &format!("無法把「{}」放回原處", info.display_name()),
                Some(err.message()),
            );
        }
    }
}

/// Deletes for good, after asking: what is chosen in the trash.
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

/// Deletes items in the trash, folders whole, away from the window's thread. The trash
/// deletes a folder with what it holds itself, and refuses to change what is inside one.
fn delete_all(win: &Rc<Window>, files: Vec<gio::File>) {
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for file in files {
            if let Err(err) = file.delete(gio::Cancellable::NONE) {
                let _ = sender.send(format!("{}：{}", display_name(&file), err.message()));
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
    let Some(folder) = pane.here().folder().and_then(|f| f.path()) else {
        return;
    };
    let path = folder.join(NEW_FOLDER);
    let path = if path.exists() { free_name(&path) } else { path };
    let file = gio::File::for_path(&path);
    if let Err(err) = file.make_directory(gio::Cancellable::NONE) {
        win.alert("無法建立檔案夾", Some(err.message()));
        return;
    }
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
            rename::ask(&win.window, files, move |done| {
                if let (Err(err), Some(win)) = (done, weak.upgrade()) {
                    win.alert("無法重新命名", Some(err.message()));
                }
            });
        }
    }
}

/// Symbolic links to each chosen item, beside it.
pub fn make_links(win: &Rc<Window>, files: &[gio::File]) {
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
        if let Err(err) = std::os::unix::fs::symlink(&path, &link) {
            win.alert(
                &format!("無法製作「{}」的連結", display_name(file)),
                Some(&err.to_string()),
            );
        }
    }
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
                if !matches!(finished, Ok(Some(status)) if status.success()) {
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
    let command = format!("set wallpaper = {:?}", path.to_string_lossy());
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
