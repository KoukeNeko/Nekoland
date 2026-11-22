//! The sidebar: recent files, the home folders and the trash, bookmarks, drives, servers and
//! tags. Each folder in it takes what is dropped on it, and springs open under a drag.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::{Rc, Weak},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{
    browse::{self, is_server, places, Location, Server, EDGE, GAP},
    hamasen, ops,
    pane::{trash, Where},
    tags, Window,
};

#[derive(Clone)]
enum Target {
    Go(Where),
    /// A drive not mounted yet: clicking it mounts it.
    Volume(gio::Volume),
    /// A Hamasen server not mounted yet: clicking mounts it.
    Hamasen(Server),
}

pub struct Sidebar {
    win: Weak<Window>,
    pub root: gtk::ScrolledWindow,
    list: gtk::ListBox,
    targets: RefCell<Vec<Target>>,
    bookmarks: RefCell<Vec<gio::File>>,
    here: RefCell<Option<Where>>,
    monitors: RefCell<Vec<gio::FileMonitor>>,
    /// Held, as GIO lets the volume monitor and its signals go with its last holder.
    volumes: gio::VolumeMonitor,
    refilling: Cell<bool>,
    rings: RefCell<Vec<Ring>>,
    /// The Hamasen server clicked while not mounted, opened once it is.
    opening: RefCell<Option<String>>,
}

/// A drive's ring, which takes its eject button's place while something is copied or moved
/// onto it.
struct Ring {
    root: PathBuf,
    eject: Option<gtk::Button>,
    area: gtk::DrawingArea,
    done: Rc<Cell<f64>>,
}

impl Sidebar {
    pub fn new(win: &Rc<Window>) -> Rc<Sidebar> {
        let list = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .margin_top(GAP)
            .margin_bottom(EDGE)
            .margin_start(GAP)
            .margin_end(GAP)
            .build();
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let sidebar = Rc::new(Sidebar {
            win: Rc::downgrade(win),
            root,
            list,
            targets: RefCell::new(Vec::new()),
            bookmarks: RefCell::new(Vec::new()),
            here: RefCell::new(None),
            monitors: RefCell::new(Vec::new()),
            volumes: gio::VolumeMonitor::get(),
            refilling: Cell::new(false),
            rings: RefCell::new(Vec::new()),
            opening: RefCell::new(None),
        });
        sidebar
            .list
            .connect_row_activated(on!(&sidebar, |s, _l, row| s.activate(row.index() as usize)));
        let monitor = &sidebar.volumes;
        monitor.connect_mount_added(on!(&sidebar, |s, _m, _x| s.fill()));
        monitor.connect_mount_removed(on!(&sidebar, |s, _m, _x| s.fill()));
        monitor.connect_mount_changed(on!(&sidebar, |s, _m, _x| s.fill()));
        monitor.connect_volume_added(on!(&sidebar, |s, _m, _x| s.fill()));
        monitor.connect_volume_removed(on!(&sidebar, |s, _m, _x| s.fill()));
        // The bookmarks file, Hamasen's servers and the trash change from elsewhere too.
        let watched = [
            gio::File::for_path(crate::browse::bookmarks_path())
                .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE),
            gio::File::for_path(crate::browse::hamasen_servers_path())
                .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE),
            trash().monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE),
        ];
        for monitor in watched.into_iter().flatten() {
            monitor.connect_changed(on!(&sidebar, |s, _m, _a, _b, _e| s.fill()));
            sidebar.monitors.borrow_mut().push(monitor);
        }
        let weak = Rc::downgrade(&sidebar);
        ops::watch_transfers(move || weak.upgrade().map(|s| s.show_transfers()).is_some());
        sidebar.fill();
        sidebar
    }

    fn show_transfers(&self) {
        for ring in self.rings.borrow().iter() {
            let running = ops::transfer_into(&ring.root);
            ring.area.set_visible(running.is_some());
            if let Some(eject) = &ring.eject {
                eject.set_visible(running.is_none());
            }
            if let Some(fraction) = running {
                ring.done.set(fraction);
                ring.area.queue_draw();
            }
        }
    }

    pub fn fill(self: &Rc<Self>) {
        // Signals arrive in bursts as a drive mounts; one refill for them.
        if self.refilling.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(sidebar) = weak.upgrade() {
                sidebar.refilling.set(false);
                sidebar.refill();
            }
        });
    }

    fn refill(self: &Rc<Self>) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        self.rings.borrow_mut().clear();
        let mut targets = Vec::new();
        let mut headers: Vec<(i32, &'static str)> = Vec::new();
        let themed = |name: &str| gio::ThemedIcon::new(name).upcast::<gio::Icon>();
        let mut bookmarks = Vec::new();
        let sections = places();
        let servers = browse::hamasen_servers();
        for (title, section) in sections {
            match title {
                None => {
                    for (index, place) in section.into_iter().enumerate() {
                        if index == 1 {
                            headers.push((targets.len() as i32, "位置"));
                        }
                        let to = match place.location {
                            Location::Recent => Where::Recent,
                            Location::Folder(file) => Where::Folder(file),
                        };
                        self.add_row(&place.icon, &place.label, None, &to);
                        targets.push(Target::Go(to));
                    }
                    let full = trash()
                        .query_info(
                            "trash::item-count",
                            gio::FileQueryInfoFlags::NONE,
                            gio::Cancellable::NONE,
                        )
                        .is_ok_and(|info| info.attribute_uint32("trash::item-count") > 0);
                    let icon = themed(if full {
                        "user-trash-full-symbolic"
                    } else {
                        "user-trash-symbolic"
                    });
                    let to = Where::Folder(trash());
                    self.add_row(&icon, "垃圾桶", None, &to);
                    targets.push(Target::Go(to));
                }
                Some("書籤") => {
                    if !section.is_empty() {
                        headers.push((targets.len() as i32, "書籤"));
                    }
                    for place in section {
                        if let Location::Folder(file) = place.location {
                            bookmarks.push(file.clone());
                            let to = Where::Folder(file);
                            self.add_row(&place.icon, &place.label, None, &to);
                            targets.push(Target::Go(to));
                        }
                    }
                }
                Some("Hamasen") => {
                    if !section.is_empty() {
                        headers.push((targets.len() as i32, "Hamasen"));
                    }
                    for place in section {
                        let (Location::Folder(file), Some(id)) = (place.location, place.hamasen) else {
                            continue;
                        };
                        let Some(server) = servers.iter().find(|s| s.id == id) else {
                            continue;
                        };
                        if place.mount.is_none() {
                            let line = self.add_plain_row(&place.icon, &place.label);
                            if hamasen::connecting(&id) {
                                line.append(&gtk::Spinner::builder().spinning(true).build());
                            }
                            self.add_server_menu(server);
                            targets.push(Target::Hamasen(server.clone()));
                            continue;
                        }
                        let to = Where::Folder(file);
                        self.add_row(&place.icon, &place.label, place.mount.as_ref(), &to);
                        self.add_server_menu(server);
                        if self.opening.borrow().as_ref() == Some(&id) {
                            self.opening.take();
                            if let Some(pane) = self.win.upgrade().and_then(|w| w.pane()) {
                                pane.go(to.clone(), true);
                            }
                        }
                        targets.push(Target::Go(to));
                    }
                }
                Some(_) => {
                    let (local, remote): (Vec<_>, Vec<_>) = section
                        .into_iter()
                        .partition(|place| place.mount.as_ref().is_some_and(|m| !is_server(m)));
                    // Drives plugged in but not mounted yet.
                    let unmounted: Vec<gio::Volume> = gio::VolumeMonitor::get()
                        .volumes()
                        .into_iter()
                        .filter(|v| v.get_mount().is_none() && v.can_mount())
                        .collect();
                    if !local.is_empty() || !unmounted.is_empty() {
                        headers.push((targets.len() as i32, "裝置"));
                    }
                    for place in local {
                        let Location::Folder(file) = place.location else {
                            continue;
                        };
                        let to = Where::Folder(file);
                        self.add_row(&place.icon, &place.label, place.mount.as_ref(), &to);
                        targets.push(Target::Go(to));
                    }
                    for volume in unmounted {
                        self.add_plain_row(&volume.symbolic_icon(), &volume.name());
                        targets.push(Target::Volume(volume));
                    }
                    headers.push((targets.len() as i32, "網路"));
                    for place in remote {
                        let Location::Folder(file) = place.location else {
                            continue;
                        };
                        let to = Where::Folder(file);
                        self.add_row(&place.icon, &place.label, place.mount.as_ref(), &to);
                        targets.push(Target::Go(to));
                    }
                    let network = Where::Folder(gio::File::for_uri("network:///"));
                    self.add_row(&themed("network-workgroup-symbolic"), "網路", None, &network);
                    targets.push(Target::Go(network));
                }
            }
        }
        headers.push((targets.len() as i32, "標籤"));
        // All seven, as the Finder's sidebar shows them by default.
        for (tag, _) in tags::TAGS {
            let line = row_line();
            // In an icon's width, so the dot and name line up with the rows above.
            let slot = gtk::CenterBox::builder().width_request(16).build();
            slot.set_center_widget(Some(&tags::dot(tag)));
            line.append(&slot);
            line.append(&row_label(tag));
            self.list.append(&line);
            targets.push(Target::Go(Where::Tag(tag.to_string())));
        }
        self.list.set_header_func(move |row, _| {
            let title = headers
                .iter()
                .find(|(index, _)| *index == row.index())
                .map(|(_, t)| *t);
            row.set_header(
                title
                    .map(|title| {
                        gtk::Label::builder()
                            .label(title)
                            .xalign(0.0)
                            .css_classes(["caption", "dim-label"])
                            .margin_top(EDGE)
                            .margin_start(GAP)
                            .margin_bottom(GAP / 2)
                            .build()
                    })
                    .as_ref(),
            );
        });
        *self.targets.borrow_mut() = targets;
        *self.bookmarks.borrow_mut() = bookmarks;
        self.show_transfers();
        let here = self.here.borrow().clone();
        if let Some(here) = here {
            self.sync(&here);
        }
    }

    /// A row that goes somewhere: taking drops when it is a folder, with its drive's eject
    /// button, and its own menu when it is a bookmark.
    fn add_row(self: &Rc<Self>, icon: &gio::Icon, label: &str, mount: Option<&gio::Mount>, to: &Where) {
        let line = row_line();
        line.append(&gtk::Image::from_gicon(icon));
        line.append(&row_label(label));
        let eject = mount.filter(|m| m.can_eject() || m.can_unmount()).map(|mount| {
            let eject = gtk::Button::builder()
                .icon_name("media-eject-symbolic")
                .tooltip_text("退出")
                .css_classes(["flat", "circular", "nf-eject"])
                .valign(gtk::Align::Center)
                .build();
            let (mount, win) = (mount.clone(), self.win.clone());
            eject.connect_clicked(move |_| eject_mount(&win, &mount));
            line.append(&eject);
            eject
        });
        if let Some(root) = mount.and_then(|m| m.root().path()) {
            let (area, done) = progress_ring();
            line.append(&area);
            self.rings.borrow_mut().push(Ring {
                root,
                eject,
                area,
                done,
            });
        }
        self.list.append(&line);
        let Some(row) = line.parent() else { return };
        if let Where::Folder(folder) = to {
            if folder.path().is_some() {
                let drop = gtk::DropTarget::new(
                    gdk::FileList::static_type(),
                    gdk::DragAction::COPY | gdk::DragAction::MOVE,
                );
                let into = folder.clone();
                drop.connect_drop(move |target, value, _, _| {
                    let Ok(list) = value.get::<gdk::FileList>() else {
                        return false;
                    };
                    ops::drop_into(&list.files(), &into, target.current_event_state());
                    true
                });
                let (to, win) = (to.clone(), self.win.clone());
                crate::pane::spring(
                    &drop,
                    &row,
                    move || Some(to.clone()),
                    move |to| {
                        if let Some(pane) = win.upgrade().and_then(|w| w.pane()) {
                            pane.go(to, true);
                        }
                    },
                );
                row.add_controller(drop);
            }
            // A bookmark's menu takes it off the sidebar.
            let folder = folder.clone();
            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let (weak, row_weak) = (Rc::downgrade(self), row.downgrade());
            click.connect_pressed(move |_, _, x, y| {
                let (Some(sidebar), Some(row)) = (weak.upgrade(), row_weak.upgrade()) else {
                    return;
                };
                if sidebar.bookmarks.borrow().iter().any(|b| b.equal(&folder)) {
                    sidebar.bookmark_menu(&row, &folder, x, y);
                }
            });
            row.add_controller(click);
        }
    }

    fn add_plain_row(&self, icon: &gio::Icon, label: &str) -> gtk::Box {
        let line = row_line();
        line.append(&gtk::Image::from_gicon(icon));
        line.append(&row_label(label));
        self.list.append(&line);
        line
    }

    /// The menu of the row just added for a server: changing it or taking it off the list.
    fn add_server_menu(&self, server: &Server) {
        let Some(row) = self.list.last_child() else { return };
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let (win, server, row_weak) = (self.win.clone(), server.clone(), row.downgrade());
        click.connect_pressed(move |_, _, x, y| {
            let Some(row) = row_weak.upgrade() else { return };
            let (edit_win, edited) = (win.clone(), server.clone());
            let (delete_win, deleted) = (win.clone(), server.clone());
            let items: Vec<(&str, Box<dyn Fn()>)> = vec![
                (
                    "編輯⋯",
                    Box::new(move || {
                        if let Some(win) = edit_win.upgrade() {
                            hamasen::edit(&win, Some(&edited));
                        }
                    }),
                ),
                (
                    "刪除",
                    Box::new(move || {
                        if let Some(win) = delete_win.upgrade() {
                            hamasen::delete(&win, deleted.clone());
                        }
                    }),
                ),
            ];
            row_menu(&row, x, y, items);
        });
        row.add_controller(click);
    }

    fn bookmark_menu(self: &Rc<Self>, row: &gtk::Widget, folder: &gio::File, x: f64, y: f64) {
        let (win, folder) = (self.win.clone(), folder.clone());
        let remove: Box<dyn Fn()> = Box::new(move || {
            if let Some(win) = win.upgrade() {
                ops::remove_bookmark(&win, &folder);
            }
        });
        row_menu(row, x, y, vec![("從側欄移除", remove)]);
    }

    fn activate(self: &Rc<Self>, index: usize) {
        let Some(target) = self.targets.borrow().get(index).cloned() else {
            return;
        };
        let Some(win) = self.win.upgrade() else { return };
        match target {
            Target::Go(to) => {
                if let Some(pane) = win.pane() {
                    pane.go(to, true);
                    pane.focus_content();
                }
            }
            Target::Hamasen(server) => {
                if hamasen::connecting(&server.id) {
                    return;
                }
                self.opening.replace(Some(server.id.clone()));
                let mounting = hamasen::mount(&server, Some(win.window.clone().upcast()));
                let weak = Rc::downgrade(self);
                glib::spawn_future_local(async move {
                    let mounted = mounting.await;
                    let Some(sidebar) = weak.upgrade() else { return };
                    if let Err(why) = mounted {
                        sidebar.opening.take();
                        if let Some(win) = sidebar.win.upgrade() {
                            win.alert(&format!("無法掛載「{}」", server.name), Some(&why));
                        }
                    }
                    sidebar.fill();
                });
                // Shows it connecting.
                self.fill();
            }
            Target::Volume(volume) => {
                let operation = gtk::MountOperation::new(Some(&win.window));
                let weak = Rc::downgrade(&win);
                let kept = volume.clone();
                volume.mount(
                    gio::MountMountFlags::NONE,
                    Some(&operation),
                    gio::Cancellable::NONE,
                    move |mounted| {
                        let Some(win) = weak.upgrade() else { return };
                        match mounted {
                            Ok(()) => {
                                if let (Some(mount), Some(pane)) = (kept.get_mount(), win.pane()) {
                                    pane.go(Where::Folder(mount.root()), true);
                                }
                            }
                            Err(err) if err.matches(gio::IOErrorEnum::FailedHandled) => {}
                            Err(err) => {
                                win.alert(&format!("無法掛載「{}」", kept.name()), Some(err.message()))
                            }
                        }
                    },
                );
            }
        }
    }

    /// Chooses the row of where the tab is, or none.
    pub fn sync(&self, here: &Where) {
        self.here.replace(Some(here.clone()));
        let index = self
            .targets
            .borrow()
            .iter()
            .position(|t| matches!(t, Target::Go(to) if to == here));
        match index.and_then(|i| self.list.row_at_index(i as i32)) {
            Some(row) => self.list.select_row(Some(&row)),
            None => self.list.unselect_all(),
        }
    }
}

/// A pie, filled in ink as far as the work is, on the line colour.
fn progress_ring() -> (gtk::DrawingArea, Rc<Cell<f64>>) {
    let done = Rc::new(Cell::new(0.0));
    let ring = gtk::DrawingArea::builder()
        .content_width(14)
        .content_height(14)
        .valign(gtk::Align::Center)
        .margin_end(GAP / 2)
        .visible(false)
        .build();
    let fraction = done.clone();
    ring.set_draw_func(move |ring, cr, width, height| {
        let ink = ring.color();
        let (x, y) = (width as f64 / 2.0, height as f64 / 2.0);
        let radius = x.min(y);
        let start = -std::f64::consts::FRAC_PI_2;
        cr.set_source_rgba(
            ink.red().into(),
            ink.green().into(),
            ink.blue().into(),
            ink.alpha() as f64 * 0.18,
        );
        cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
        // A failed fill leaves the ring blank, with nothing better to show.
        let _ = cr.fill();
        cr.set_source_rgba(
            ink.red().into(),
            ink.green().into(),
            ink.blue().into(),
            ink.alpha().into(),
        );
        cr.move_to(x, y);
        cr.arc(
            x,
            y,
            radius,
            start,
            start + std::f64::consts::TAU * fraction.get(),
        );
        cr.close_path();
        let _ = cr.fill();
    });
    (ring, done)
}

fn row_line() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, GAP)
}

fn row_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build()
}

/// A menu at a point of a row, each item a button that closes it.
fn row_menu(row: &gtk::Widget, x: f64, y: f64, items: Vec<(&str, Box<dyn Fn()>)>) {
    let popover = gtk::Popover::builder().has_arrow(false).build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for (label, run) in items {
        let button = gtk::Button::builder().label(label).css_classes(["flat"]).build();
        button.connect_clicked(glib::clone!(
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                run();
            }
        ));
        column.append(&button);
    }
    popover.set_child(Some(&column));
    popover.set_parent(row);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

fn eject_mount(win: &Weak<Window>, mount: &gio::Mount) {
    let Some(window) = win.upgrade() else { return };
    // A folder rclone serves waits for the uploads still on their way, and loses its
    // bookmark with it.
    let all = [mount.clone()];
    if let Some(server) = browse::hamasen_servers()
        .into_iter()
        .find(|s| !s.gvfs && s.mount(&all).is_some())
    {
        hamasen::unmount(&window, server);
        return;
    }
    let operation = gtk::MountOperation::new(Some(&window.window));
    let (win, name) = (win.clone(), mount.name());
    let done = move |result: Result<(), glib::Error>| {
        if let (Err(err), Some(win)) = (result, win.upgrade()) {
            if !err.matches(gio::IOErrorEnum::FailedHandled) {
                win.alert(&format!("無法退出「{name}」"), Some(err.message()));
            }
        }
    };
    if mount.can_eject() {
        mount.eject_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&operation),
            gio::Cancellable::NONE,
            done,
        );
    } else {
        mount.unmount_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&operation),
            gio::Cancellable::NONE,
            done,
        );
    }
}
