//! `nekoland-disks`, 磁碟: the drives of this computer and what is on them. A drive's partitions
//! and free space are drawn side by side as wide as they are large; one is chosen to mount,
//! unmount, resize, check or format it, or to put a partition in the free space. A drive can be
//! taken out, read for its health, timed, copied to an image and written back from one, and an
//! image opened as a drive. What is done to a drive is done as root, after asking for the
//! administrator's password, as Market does; what is asked of udisks is not.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod alert;
#[path = "disks/map.rs"]
mod map;
#[path = "disks/model.rs"]
mod model;
#[path = "disks/ops.rs"]
mod ops;

use map::{PartMap, Seg};
use model::{Disk, Kind, Part, Slot};
use ops::{Event, Job};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Disks";
/// How often the drives are read again, to show a drive plugged in or a mount made elsewhere.
const REFRESH: Duration = Duration::from_secs(3);
const MIB: u64 = 1 << 20;
/// Why a change asked for is not made: the drive is not the one it was asked for.
const CHANGED: &str = "磁碟已拔除或已變更";

/// The drive as it is now, if it is still the one asked about: one plugged in meanwhile can
/// take an unplugged one's device name. What is mounted is as it is now.
fn disk_now(disk: &Disk) -> Option<Disk> {
    let layout = |disk: &Disk| -> Vec<(String, String, u64)> {
        disk.parts.iter().map(|part| (part.kname.clone(), part.uuid.clone(), part.size)).collect()
    };
    let now = model::read().ok()?.into_iter().find(|now| now.kname == disk.kname)?;
    (now.size == disk.size && now.name == disk.name && layout(&now) == layout(disk)).then_some(now)
}

/// The partition as it is now, if it is still the one asked about.
fn part_now(part: &Part) -> Option<Part> {
    model::read()
        .ok()?
        .into_iter()
        .flat_map(|disk| disk.parts)
        .find(|now| now.kname == part.kname && now.uuid == part.uuid && now.size == part.size)
}
/// How wide the columns of amounts are.
const COLUMN_WIDTHS: [i32; 4] = [110, 170, 90, 100];

/// Which of what a drive holds is chosen, kept across reading the drives again.
#[derive(Clone, PartialEq)]
enum Chosen {
    Part(String),
    Free(u64),
}

struct Window {
    window: adw::ApplicationWindow,
    drives: RefCell<Vec<Disk>>,
    disk: RefCell<Option<String>>,
    chosen: RefCell<Option<Chosen>>,
    /// What each drive's SMART data says, once read; None for a drive that does not tell.
    health: RefCell<HashMap<String, Option<ops::Smart>>>,

    list: gtk::ListBox,
    groups: Rc<RefCell<Vec<&'static str>>>,
    quiet: Cell<bool>,
    stack: gtk::Stack,
    title: gtk::Label,
    chip: gtk::Label,
    meta: gtk::Box,
    map: Rc<PartMap>,
    blank_map: Rc<PartMap>,
    blank_meta: gtk::Box,
    table: gtk::ListBox,
    columns: Vec<gtk::SizeGroup>,
    action_stack: gtk::Stack,
    actions: gtk::Box,
    busy_who: gtk::Label,
    busy_bar: gtk::ProgressBar,
    busy_text: gtk::Label,
    busy_cancel: gtk::Button,
    more: gtk::MenuButton,
    busy: Cell<bool>,
    known: Cell<bool>,
    cancel: RefCell<Option<ops::Cancel>>,
    animate: Cell<bool>,
}

fn dark() -> bool {
    adw::StyleManager::default().is_dark()
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(classes.to_vec()).build()
}

/// A square in the color of a partition, or hatched for free space.
fn dot(color: Option<usize>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().content_width(9).content_height(9).valign(gtk::Align::Center).build();
    area.set_draw_func(move |_, cr, width, height| {
        let (w, h) = (width as f64, height as f64);
        cr.rectangle(0.0, 0.0, w, h);
        match color {
            Some(color) => {
                let c = map::palette(dark(), color);
                cr.set_source_rgb(c.0, c.1, c.2);
            }
            None => cr.set_source_rgba(0.5, 0.5, 0.5, 0.6),
        }
        let _ = cr.fill();
        if color.is_none() {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.7);
            cr.set_line_width(1.0);
            cr.move_to(0.0, h);
            cr.line_to(w, 0.0);
            let _ = cr.stroke();
        }
    });
    area
}

/// A group of rows in a card, as a form.
fn form(rows: &[&gtk::Widget]) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).width_request(alert::FIELD_WIDTH).build();
    for row in rows {
        list.append(*row);
    }
    list
}

fn combo(title: &str, items: &[&str], selected: u32) -> adw::ComboRow {
    adw::ComboRow::builder().title(title).model(&gtk::StringList::new(items)).selected(selected).build()
}

fn entry(title: &str, text: &str) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title(title).build();
    row.set_text(text);
    row
}

fn fact(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&label(value, &["numeric", "dim-label"]));
    row
}

fn table_name(table: &str) -> &str {
    match table {
        "dos" => "MBR",
        other => other,
    }
}

/// What a partition of `disk` is drawn in.
fn color_of(disk: &Disk, index: usize) -> usize {
    let part = &disk.parts[index];
    if part.is_swap() {
        return 2;
    }
    if part.mount_point() == Some("/") {
        return 1;
    }
    if part.mount_point() == Some("/boot/efi") || (part.fstype == "vfat" && part.size < (1 << 30)) || disk.kind == Kind::Image {
        return 0;
    }
    let order = disk.parts[..index].iter().filter(|other| !other.is_swap() && other.mount_point() != Some("/") && other.fstype != "vfat").count();
    [3, 4, 1][order % 3]
}

impl Window {
    fn open(app: &adw::Application) -> Rc<Window> {
        let window = adw::ApplicationWindow::builder().application(app).title("磁碟").default_width(1100).default_height(720).build();

        let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let groups: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let known_groups = groups.clone();
        list.set_header_func(move |row, before| {
            let groups = known_groups.borrow();
            let here = groups.get(row.index() as usize).copied();
            let before = before.and_then(|row| groups.get(row.index() as usize).copied());
            match (here, before) {
                (Some(here), before) if before != Some(here) => row.set_header(Some(&label(here, &["dm-group"]))),
                _ => row.set_header(None::<&gtk::Widget>),
            }
        });
        let open_image = gtk::Button::builder().icon_name("document-open-symbolic").tooltip_text("開啟映像檔").build();
        let sidebar_bar = adw::HeaderBar::new();
        sidebar_bar.pack_end(&open_image);
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_bar);
        sidebar_view.set_content(Some(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build()));
        let sidebar = adw::NavigationPage::builder().title("磁碟").child(&sidebar_view).build();

        let title = label("", &["title-4"]);
        let chip = label("", &["dm-chip"]);
        chip.set_visible(false);
        let heading = gtk::Box::builder().spacing(10).valign(gtk::Align::Center).build();
        heading.append(&title);
        heading.append(&chip);
        let menu = gio::Menu::new();
        let reading = gio::Menu::new();
        reading.append(Some("SMART 資料"), Some("win.smart"));
        reading.append(Some("測試速度"), Some("win.speed"));
        menu.append_section(None, &reading);
        let images = gio::Menu::new();
        images.append(Some("建立磁碟映像檔…"), Some("win.image-create"));
        images.append(Some("還原磁碟映像檔…"), Some("win.image-restore"));
        menu.append_section(None, &images);
        let wipe = gio::Menu::new();
        wipe.append(Some("清除磁碟…"), Some("win.wipe"));
        menu.append_section(None, &wipe);
        let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).tooltip_text("更多").build();
        let content_bar = adw::HeaderBar::new();
        content_bar.set_title_widget(Some(&heading));
        content_bar.pack_end(&more);

        let meta = gtk::Box::builder().spacing(28).build();
        let map = PartMap::new();
        let columns: Vec<gtk::SizeGroup> = COLUMN_WIDTHS.iter().map(|_| gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal)).collect();
        let table = gtk::ListBox::builder().css_classes(["dm-list"]).selection_mode(gtk::SelectionMode::Single).activate_on_single_click(true).build();
        let table_scroll = gtk::ScrolledWindow::builder().child(&table).hscrollbar_policy(gtk::PolicyType::Never).propagate_natural_height(true).build();
        let card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["card"]).overflow(gtk::Overflow::Hidden).valign(gtk::Align::Start).build();
        card.append(&Window::header_row(&columns));
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        card.append(&table_scroll);

        let actions = gtk::Box::builder().spacing(8).valign(gtk::Align::Center).build();
        let busy_who = label("", &["dm-who"]);
        let busy_bar = gtk::ProgressBar::builder().hexpand(true).valign(gtk::Align::Center).build();
        let busy_text = label("", &["numeric", "dim-label"]);
        let busy_cancel = gtk::Button::builder().label("取消").build();
        let busy = gtk::Box::builder().spacing(14).build();
        busy.append(&busy_who);
        busy.append(&busy_bar);
        busy.append(&busy_text);
        busy.append(&busy_cancel);
        let action_stack = gtk::Stack::builder().css_classes(["card", "dm-actions"]).transition_type(gtk::StackTransitionType::Crossfade).transition_duration(150).build();
        action_stack.add_named(&actions, Some("buttons"));
        action_stack.add_named(&busy, Some("busy"));

        let disk_page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(20).margin_bottom(20).margin_start(24).margin_end(24).build();
        disk_page.append(&meta);
        disk_page.append(&map.area);
        disk_page.append(&card);
        disk_page.append(&action_stack);

        let blank_meta = gtk::Box::builder().spacing(28).build();
        let blank_map = PartMap::new();
        let make_table = gtk::Button::builder().label("建立分割表").action_name("win.new-table").css_classes(["suggested-action", "pill"]).halign(gtk::Align::Center).build();
        let blank_empty = adw::StatusPage::builder().title("沒有分割表").child(&make_table).vexpand(true).build();
        let blank_page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(20).margin_bottom(20).margin_start(24).margin_end(24).build();
        blank_page.append(&blank_meta);
        blank_page.append(&blank_map.area);
        blank_page.append(&blank_empty);

        let empty = adw::StatusPage::builder().icon_name("drive-harddisk-symbolic").title("沒有磁碟").build();
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).transition_duration(120).build();
        stack.add_named(&disk_page, Some("disk"));
        stack.add_named(&blank_page, Some("blank"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("empty");
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_bar);
        content_view.set_content(Some(&stack));
        let content = adw::NavigationPage::builder().title("磁碟").child(&content_view).build();

        let split = adw::NavigationSplitView::builder().sidebar(&sidebar).content(&content).min_sidebar_width(240.0).max_sidebar_width(300.0).build();
        window.set_content(Some(&split));

        let win = Rc::new(Window {
            window,
            drives: RefCell::default(),
            disk: RefCell::default(),
            chosen: RefCell::default(),
            health: RefCell::default(),
            list,
            groups,
            quiet: Cell::new(false),
            stack,
            title,
            chip,
            meta,
            map,
            blank_map,
            blank_meta,
            table,
            columns,
            action_stack,
            actions,
            busy_who,
            busy_bar,
            busy_text,
            busy_cancel,
            more,
            busy: Cell::new(false),
            known: Cell::new(false),
            cancel: RefCell::default(),
            animate: Cell::new(true),
        });
        win.wire(&open_image);
        win.reload();
        let this = win.clone();
        glib::timeout_add_local(REFRESH, move || {
            if !this.busy.get() {
                this.reload();
            }
            glib::ControlFlow::Continue
        });
        win.window.present();
        win
    }

    fn header_row(columns: &[gtk::SizeGroup]) -> gtk::Box {
        let row = gtk::Box::builder().spacing(12).css_classes(["dm-row", "dm-head"]).build();
        let name = label("分割區", &["dm-head"]);
        name.set_hexpand(true);
        row.append(&name);
        for (index, title) in ["檔案系統", "掛載點", "大小", "已用"].iter().enumerate() {
            let cell = label(title, &["dm-head"]);
            if index >= 2 {
                cell.set_xalign(1.0);
            }
            cell.set_width_request(COLUMN_WIDTHS[index]);
            columns[index].add_widget(&cell);
            row.append(&cell);
        }
        row
    }

    fn wire(self: &Rc<Self>, open_image: &gtk::Button) {
        let this = self.clone();
        self.list.connect_row_selected(move |_, row| {
            if this.quiet.get() {
                return;
            }
            let Some(row) = row else { return };
            let kname = this.drives.borrow().get(row.index() as usize).map(|disk| disk.kname.clone());
            if let Some(kname) = kname {
                this.select_disk(&kname);
            }
        });
        let this = self.clone();
        self.table.connect_row_selected(move |_, row| {
            if this.quiet.get() {
                return;
            }
            if let Some(row) = row {
                this.select_slot(row.index() as usize);
            }
        });
        for map in [&self.map, &self.blank_map] {
            let this = self.clone();
            map.connect_choose(move |index| this.select_slot(index));
        }
        let this = self.clone();
        self.busy_cancel.connect_clicked(move |_| {
            if let Some(cancel) = this.cancel.borrow().as_ref() {
                cancel.cancel();
            }
        });
        let this = self.clone();
        open_image.connect_clicked(move |_| this.choose_image());
        let new_table = gio::SimpleAction::new("new-table", None);
        let this = self.clone();
        new_table.connect_activate(move |_, _| this.new_table());
        self.window.add_action(&new_table);

        let add = |name: &str, run: fn(&Rc<Window>, Disk)| {
            let action = gio::SimpleAction::new(name, None);
            let this = self.clone();
            action.connect_activate(move |_, _| {
                if let Some(disk) = this.current_disk() {
                    run(&this, disk);
                }
            });
            self.window.add_action(&action);
        };
        add("smart", Window::show_smart);
        add("speed", Window::show_speed);
        add("image-create", Window::image_create);
        add("image-restore", Window::image_restore);
        add("wipe", Window::wipe);
        let on_part = |name: &str, run: fn(&Rc<Window>, Disk, Part)| {
            let action = gio::SimpleAction::new(name, None);
            let this = self.clone();
            action.connect_activate(move |_, _| {
                if let Some((disk, Slot::Part(index))) = this.selection() {
                    let part = disk.parts[index].clone();
                    run(&this, disk, part);
                }
            });
            self.window.add_action(&action);
        };
        on_part("mount", |this, _, part| this.run_job(&format!("掛載「{}」", part.label()), format!("無法掛載「{}」", part.label()), ops::mount(&part)));
        on_part("unmount", |this, _, part| this.unmount(part));
        on_part("swap", |this, _, part| this.run_job("處理 swap", "無法處理 swap".into(), ops::swap(&part, !part.mounted())));
        on_part("resize", Window::resize);
        on_part("check", |this, _, part| this.check(part));
        on_part("format", |this, _, part| this.format(part));
        let new_partition = gio::SimpleAction::new("new-partition", None);
        let this = self.clone();
        new_partition.connect_activate(move |_, _| {
            if let Some((disk, Slot::Free(start, end))) = this.selection() {
                this.new_partition(disk, start, end);
            }
        });
        self.window.add_action(&new_partition);
        let eject = gio::SimpleAction::new("eject-disk", None);
        let this = self.clone();
        eject.connect_activate(move |_, _| {
            if let Some(disk) = this.current_disk() {
                this.eject(&disk);
            }
        });
        self.window.add_action(&eject);
        let refresh = gio::SimpleAction::new("refresh", None);
        let this = self.clone();
        refresh.connect_activate(move |_, _| this.reload());
        self.window.add_action(&refresh);
        let open = gio::SimpleAction::new("open-image", None);
        let this = self.clone();
        open.connect_activate(move |_, _| this.choose_image());
        self.window.add_action(&open);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.refresh", &["F5", "<Control>r"]);
            app.set_accels_for_action("win.open-image", &["<Control>o"]);
        }
        let this = self.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| this.fill());
    }

    fn current_disk(&self) -> Option<Disk> {
        let kname = self.disk.borrow().clone()?;
        self.drives.borrow().iter().find(|disk| disk.kname == kname).cloned()
    }

    fn slot_index(&self, disk: &Disk, slots: &[Slot]) -> Option<usize> {
        let at = match self.chosen.borrow().as_ref() {
            Some(Chosen::Part(path)) => slots.iter().position(|slot| matches!(slot, Slot::Part(index) if &disk.parts[*index].path == path)),
            Some(Chosen::Free(start)) => slots.iter().position(|slot| matches!(slot, Slot::Free(from, _) if from.abs_diff(*start) < 64 * MIB)),
            None => None,
        };
        // The first of a drive is chosen when there is nothing else to be.
        at.or_else(|| (!slots.is_empty()).then(|| slots.iter().position(|slot| matches!(slot, Slot::Part(_))).unwrap_or(0)))
    }

    fn reload(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(model::read).await {
                Ok(Ok(disks)) => this.apply(disks),
                Ok(Err(why)) => eprintln!("nekoland-disks: {why}"),
                Err(_) => eprintln!("nekoland-disks: reading the drives stopped"),
            }
        });
    }

    fn apply(self: &Rc<Self>, disks: Vec<Disk>) {
        if *self.drives.borrow() == disks {
            return;
        }
        let first = self.drives.borrow().is_empty();
        self.drives.replace(disks);
        let kept = self.current_disk().map(|disk| disk.kname);
        if kept.is_none() {
            let first = self.drives.borrow().first().map(|disk| disk.kname.clone());
            self.disk.replace(first);
            self.chosen.replace(None);
        }
        self.animate.set(first || kept.is_none());
        self.fill_sidebar();
        self.fill();
    }

    fn select_disk(self: &Rc<Self>, kname: &str) {
        if self.disk.borrow().as_deref() == Some(kname) {
            return;
        }
        self.disk.replace(Some(kname.to_string()));
        self.chosen.replace(None);
        self.animate.set(true);
        self.fill();
    }

    fn select_slot(self: &Rc<Self>, index: usize) {
        let Some(disk) = self.current_disk() else { return };
        let slots = disk.slots();
        let Some(slot) = slots.get(index) else { return };
        let chosen = match slot {
            Slot::Part(part) => Chosen::Part(disk.parts[*part].path.clone()),
            Slot::Free(start, _) => Chosen::Free(*start),
        };
        if self.chosen.borrow().as_ref() == Some(&chosen) {
            return;
        }
        self.chosen.replace(Some(chosen));
        self.animate.set(false);
        self.fill();
    }

    fn fill_sidebar(self: &Rc<Self>) {
        self.quiet.set(true);
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let mut groups = Vec::new();
        let selected = self.disk.borrow().clone();
        for disk in self.drives.borrow().iter() {
            groups.push(match disk.kind {
                Kind::Internal => "內建",
                Kind::External => "外接",
                Kind::Image => "映像檔",
            });
            let row = self.drive_row(disk);
            self.list.append(&row);
            if selected.as_deref() == Some(disk.kname.as_str()) {
                self.list.select_row(Some(&row));
            }
        }
        self.groups.replace(groups);
        // The headers are chosen by what each row is, so they are asked again.
        self.list.invalidate_headers();
        self.quiet.set(false);
    }

    fn drive_row(self: &Rc<Self>, disk: &Disk) -> gtk::ListBoxRow {
        let icon = match disk.kind {
            Kind::Internal => "drive-harddisk-symbolic",
            Kind::External => "drive-removable-media-symbolic",
            Kind::Image => "media-optical-symbolic",
        };
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).build();
        text.append(&label(&disk.title(), &["heading"]));
        let detail = format!("{} · {}", model::size(disk.size), disk.kname);
        text.append(&label(&detail, &["caption", "dim-label"]));
        if let Some(fill) = disk.fill() {
            let bar = gtk::ProgressBar::builder().fraction(fill / 100.0).css_classes(["dm-mini"]).build();
            text.append(&bar);
        }
        let row = gtk::Box::builder().spacing(10).margin_top(4).margin_bottom(4).build();
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&text);
        if disk.kind != Kind::Internal {
            let eject = gtk::Button::builder().icon_name("media-eject-symbolic").css_classes(["flat", "circular"]).valign(gtk::Align::Center).tooltip_text("退出").build();
            let this = self.clone();
            let disk = disk.clone();
            eject.connect_clicked(move |_| this.eject(&disk));
            row.append(&eject);
        }
        gtk::ListBoxRow::builder().child(&row).build()
    }

    /// Shows what the chosen drive and partition are.
    fn fill(self: &Rc<Self>) {
        let Some(disk) = self.current_disk() else {
            self.stack.set_visible_child_name("empty");
            self.title.set_label("磁碟");
            self.chip.set_visible(false);
            self.more.set_sensitive(false);
            return;
        };
        self.title.set_label(&disk.title());
        self.fill_chip(&disk);
        self.read_health(&disk);
        self.update_menu(&disk);
        let blank = disk.table.is_none() && disk.parts.is_empty();
        let slots = disk.slots();
        let chosen = self.slot_index(&disk, &slots);
        let segs: Vec<Seg> = if blank {
            vec![Seg { name: "未配置".into(), size: model::size(disk.size), weight: disk.size as f64, used: None, color: None, tip: format!("未配置 · {}", model::size(disk.size)) }]
        } else {
            slots
                .iter()
                .map(|slot| match slot {
                    Slot::Part(index) => {
                        let part = &disk.parts[*index];
                        Seg {
                            name: part.label(),
                            size: model::size(part.size),
                            weight: part.size as f64,
                            used: part.used.map(|used| used as f64 / part.size.max(1) as f64),
                            color: Some(color_of(&disk, *index)),
                            tip: format!("{} · {} · {}", part.label(), part.fstype, model::size(part.size)),
                        }
                    }
                    Slot::Free(start, end) => Seg {
                        name: "未配置".into(),
                        size: model::size(end - start),
                        weight: (end - start) as f64,
                        used: None,
                        color: None,
                        tip: format!("未配置 · {}", model::size(end - start)),
                    },
                })
                .collect()
        };
        let animate = self.animate.replace(false);
        self.fill_meta(&disk, if blank { &self.blank_meta } else { &self.meta });
        if blank {
            self.blank_map.set(segs, Some(0), animate);
            self.stack.set_visible_child_name("blank");
            return;
        }
        self.map.set(segs, chosen, animate);
        self.fill_table(&disk, &slots, chosen);
        self.fill_actions(&disk, chosen.map(|index| slots[index]));
        self.stack.set_visible_child_name("disk");
    }

    fn fill_chip(&self, disk: &Disk) {
        let health = self.health.borrow().get(&disk.kname).cloned().flatten();
        let (text, warn) = if disk.table.is_none() && disk.parts.is_empty() {
            (Some("未初始化"), false)
        } else if disk.read_only {
            (Some("唯讀"), false)
        } else {
            match health.as_ref().and_then(|smart| smart.passed) {
                Some(true) => (Some("健康"), false),
                Some(false) => (Some("異常"), true),
                None => (None, false),
            }
        };
        self.chip.set_visible(text.is_some());
        self.chip.set_label(text.unwrap_or_default());
        self.chip.remove_css_class("neutral");
        self.chip.remove_css_class("warn");
        if warn {
            self.chip.add_css_class("warn");
        } else if text != Some("健康") {
            self.chip.add_css_class("neutral");
        }
    }

    fn fill_meta(&self, disk: &Disk, into: &gtk::Box) {
        while let Some(child) = into.first_child() {
            into.remove(&child);
        }
        let mut facts: Vec<(String, &str)> = vec![(model::size(disk.size), "容量")];
        if disk.kind == Kind::Image {
            facts.push((disk.parts.first().map(|part| part.fstype.clone()).unwrap_or_else(|| "—".into()), "檔案系統"));
            facts.push((disk.kname.clone(), "裝置"));
        } else {
            facts.push((disk.table.as_deref().map(|table| table_name(table).to_string()).unwrap_or_else(|| "—".into()), "分割表"));
            if let Some(interface) = model::interface(&disk.tran) {
                facts.push((interface, "介面"));
            }
            if let Some(degrees) = self.health.borrow().get(&disk.kname).cloned().flatten().and_then(|smart| smart.temperature) {
                facts.push((format!("{degrees}°C"), "溫度"));
            }
        }
        for (value, name) in facts {
            let cell = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            cell.append(&label(&value, &["dm-value"]));
            cell.append(&label(name, &["caption", "dim-label"]));
            into.append(&cell);
        }
    }

    fn fill_table(&self, disk: &Disk, slots: &[Slot], chosen: Option<usize>) {
        self.quiet.set(true);
        while let Some(row) = self.table.row_at_index(0) {
            self.table.remove(&row);
        }
        for (position, slot) in slots.iter().enumerate() {
            let (color, name, fs, point, size, used) = match slot {
                Slot::Part(index) => {
                    let part = &disk.parts[*index];
                    (
                        Some(color_of(disk, *index)),
                        part.label(),
                        part.fstype.clone(),
                        part.mount_point().map(str::to_string).unwrap_or_else(|| "—".into()),
                        model::size(part.size),
                        part.used.map(model::size).unwrap_or_else(|| "—".into()),
                    )
                }
                Slot::Free(start, end) => (None, "未配置".into(), "—".into(), "—".into(), model::size(end - start), "—".into()),
            };
            let row = gtk::Box::builder().spacing(12).css_classes(["dm-row"]).build();
            let first = gtk::Box::builder().spacing(9).hexpand(true).build();
            first.append(&dot(color));
            first.append(&label(&name, &["dm-name"]));
            row.append(&first);
            for (index, text) in [fs, point, size, used].iter().enumerate() {
                let cell = label(text, if index >= 2 { &["numeric"][..] } else { &[][..] });
                cell.set_ellipsize(gtk::pango::EllipsizeMode::End);
                if index >= 2 {
                    cell.set_xalign(1.0);
                } else if index == 1 {
                    cell.add_css_class("dim-label");
                }
                cell.set_width_request(COLUMN_WIDTHS[index]);
                self.columns[index].add_widget(&cell);
                row.append(&cell);
            }
            let list_row = gtk::ListBoxRow::builder().child(&row).build();
            self.table.append(&list_row);
            if Some(position) == chosen {
                self.table.select_row(Some(&list_row));
            }
        }
        self.quiet.set(false);
    }

    /// A button that does `action`, on what is chosen.
    fn button(&self, text: &str, class: Option<&str>, action: &str, enabled: bool) {
        let button = gtk::Button::builder().label(text).action_name(format!("win.{action}")).build();
        if let Some(class) = class {
            button.add_css_class(class);
        }
        self.set_enabled(action, enabled && !self.busy.get());
        self.actions.append(&button);
    }

    fn set_enabled(&self, action: &str, on: bool) {
        if let Some(action) = self.window.lookup_action(action).and_downcast::<gio::SimpleAction>() {
            action.set_enabled(on);
        }
    }

    fn who(&self, color: Option<usize>, name: &str, device: &str) {
        let who = gtk::Box::builder().spacing(9).hexpand(true).valign(gtk::Align::Center).build();
        who.append(&dot(color));
        who.append(&label(name, &["dm-who"]));
        who.append(&label(device, &["dm-dev"]));
        self.actions.append(&who);
    }

    fn fill_actions(self: &Rc<Self>, disk: &Disk, slot: Option<Slot>) {
        while let Some(child) = self.actions.first_child() {
            self.actions.remove(&child);
        }
        for action in ["mount", "unmount", "swap", "resize", "check", "format", "new-partition", "eject-disk"] {
            self.set_enabled(action, false);
        }
        let Some(slot) = slot else { return };
        match slot {
            Slot::Free(start, end) => {
                self.who(None, "未配置", &model::size(end - start));
                self.button("新增分割區", Some("suggested-action"), "new-partition", !disk.read_only);
            }
            Slot::Part(index) => {
                let part = &disk.parts[index];
                self.who(Some(color_of(disk, index)), &part.label(), part.path.trim_start_matches("/dev/"));
                self.part_buttons(disk, part);
            }
        }
    }

    fn part_buttons(&self, disk: &Disk, part: &Part) {
        if disk.kind == Kind::External {
            self.button("退出", Some("suggested-action"), "eject-disk", true);
        }
        let mountable = !part.fstype.is_empty() && part.fstype != "LVM" && !part.is_swap();
        if part.is_swap() {
            self.button(if part.mounted() { "停用" } else { "啟用" }, None, "swap", !part.is_system());
        } else if part.mounted() {
            self.button("卸載", None, "unmount", !part.is_system());
        } else if mountable {
            self.button("掛載", None, "mount", true);
        }
        if disk.read_only {
            return;
        }
        self.button("調整大小", None, "resize", ops::resize_kind(part).is_some());
        self.button("檢查檔案系統", None, "check", ops::check(part).is_some());
        self.actions.append(&gtk::Box::builder().width_request(24).build());
        self.button("格式化", Some("destructive-action"), "format", !part.is_system() && part.number > 0);
    }

    /// The drive and partition or free space chosen.
    fn selection(&self) -> Option<(Disk, Slot)> {
        let disk = self.current_disk()?;
        let slots = disk.slots();
        let index = self.slot_index(&disk, &slots)?;
        let slot = slots[index];
        Some((disk, slot))
    }

    fn update_menu(&self, disk: &Disk) {
        let enable = |name: &str, on: bool| {
            if let Some(action) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(on && !self.busy.get());
            }
        };
        let whole = disk.kind != Kind::Image;
        enable("smart", whole && ops::which("smartctl").is_some());
        enable("speed", true);
        enable("image-create", true);
        enable("image-restore", whole && !disk.read_only && !disk.parts.iter().any(|part| part.is_system()));
        enable("wipe", whole && !disk.read_only && !disk.parts.iter().any(|part| part.is_system()));
        self.more.set_sensitive(!self.busy.get());
    }

    /// Reads a drive's health without asking for the password, as far as that tells.
    fn read_health(self: &Rc<Self>, disk: &Disk) {
        if disk.kind == Kind::Image || self.health.borrow().contains_key(&disk.kname) || ops::which("smartctl").is_none() {
            return;
        }
        self.health.borrow_mut().insert(disk.kname.clone(), None);
        let (events, _) = ops::spawn(ops::smart(disk, false));
        let (this, kname) = (self.clone(), disk.kname.clone());
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                if let Event::Done(Ok(json)) = event {
                    if let Some(smart) = ops::parse_smart(&json) {
                        this.health.borrow_mut().insert(kname.clone(), Some(smart));
                        this.fill();
                    }
                    return;
                }
                if let Event::Done(Err(_)) = event {
                    return;
                }
            }
        });
    }

    // --- running -------------------------------------------------------------------------

    fn set_busy(self: &Rc<Self>, status: Option<&str>) {
        self.busy.set(status.is_some());
        match status {
            Some(status) => {
                self.busy_who.set_label(status);
                self.busy_text.set_label("");
                self.busy_bar.set_fraction(0.0);
                self.known.set(false);
                self.action_stack.set_visible_child_name("busy");
                let this = self.clone();
                glib::timeout_add_local(Duration::from_millis(120), move || {
                    if !this.busy.get() {
                        return glib::ControlFlow::Break;
                    }
                    if !this.known.get() {
                        this.busy_bar.pulse();
                    }
                    glib::ControlFlow::Continue
                });
            }
            None => self.action_stack.set_visible_child_name("buttons"),
        }
        if let Some(disk) = self.current_disk() {
            self.update_menu(&disk);
        }
        self.fill_sidebar_sensitivity();
    }

    fn fill_sidebar_sensitivity(&self) {
        self.table.set_sensitive(!self.busy.get());
    }

    /// Runs `job`, showing how far it is; `then` learns how it went.
    fn run(self: &Rc<Self>, status: &str, job: Job, then: impl FnOnce(&Rc<Window>, Result<String, String>) + 'static) {
        if self.busy.get() {
            return;
        }
        self.set_busy(Some(status));
        let (events, cancel) = ops::spawn(job);
        self.cancel.replace(Some(cancel));
        let this = self.clone();
        let mut then = Some(then);
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                match event {
                    Event::Progress(done) => {
                        this.known.set(true);
                        this.busy_bar.set_fraction(done);
                        this.busy_text.set_label(&format!("{:.0}%", done * 100.0));
                    }
                    Event::Done(result) => {
                        this.cancel.replace(None);
                        this.set_busy(None);
                        if let Some(then) = then.take() {
                            then(&this, result);
                        }
                        this.reload();
                        return;
                    }
                }
            }
        });
    }

    /// Runs `job`, telling why when it did not work.
    fn run_job(self: &Rc<Self>, status: &str, error: String, job: Job) {
        self.run(status, job, move |this, result| {
            if let Err(why) = result {
                this.failed(&error, &why);
            }
        });
    }

    fn failed(&self, title: &str, why: &str) {
        if why != "已取消" {
            alert::notice(&self.window, title, Some(why));
        }
    }

    // --- what is done ---------------------------------------------------------------------

    fn unmount(self: &Rc<Self>, part: Part) {
        let name = part.label();
        let job = ops::unmount(&part);
        self.run(&format!("卸載「{name}」"), job, move |this, result| {
            let Err(why) = result else { return };
            if !why.to_lowercase().contains("busy") {
                return this.failed(&format!("無法卸載「{name}」"), &why);
            }
            let users = part.mount_point().map(ops::users_of).unwrap_or_default();
            let body = if users.is_empty() { "有程式正在使用".to_string() } else { format!("{} 正在使用 {}", users.join("、"), part.mount_point().unwrap_or_default()) };
            let alert = adw::AlertDialog::new(Some(&format!("無法卸載「{name}」")), Some(&body));
            alert.add_responses(&[("cancel", "取消"), ("retry", "重試")]);
            alert.set_response_appearance("retry", adw::ResponseAppearance::Suggested);
            alert.set_default_response(Some("retry"));
            alert.set_close_response("cancel");
            let this = this.clone();
            let window = this.window.clone();
            alert.choose(&window, gio::Cancellable::NONE, move |response| {
                if response == "retry" {
                    this.unmount(part);
                }
            });
        });
    }

    fn eject(self: &Rc<Self>, disk: &Disk) {
        let name = disk.title();
        self.run_job(&format!("退出「{name}」"), format!("無法退出「{name}」"), ops::eject(disk));
    }

    fn format(self: &Rc<Self>, part: Part) {
        let known: Vec<&str> = ops::FILE_SYSTEMS.iter().map(|(_, name)| *name).collect();
        let current = ops::FILE_SYSTEMS.iter().position(|(id, _)| *id == part.fstype).unwrap_or(0) as u32;
        let fs = combo("檔案系統", &known, current);
        let name = entry("名稱", &part.name);
        let wipe = combo("清除方式", &["快速", "覆寫零"], 0);
        let name_text = part.label();
        let extra = form(&[fs.upcast_ref(), name.upcast_ref(), wipe.upcast_ref()]);
        let this = self.clone();
        self.ask(
            &format!("格式化「{name_text}」？"),
            Some("分割區內的所有檔案會被清除，無法復原。"),
            &extra,
            "格式化",
            true,
            move || {
                let Some(part) = part_now(&part) else {
                    this.failed(&format!("無法格式化「{name_text}」"), CHANGED);
                    return;
                };
                let fstype = ops::FILE_SYSTEMS[fs.selected() as usize].0;
                let job = ops::format(&part, fstype, name.text().as_str(), wipe.selected() == 1);
                this.run_job(&format!("格式化「{name_text}」"), format!("無法格式化「{name_text}」"), job);
            },
        );
    }

    fn check(self: &Rc<Self>, part: Part) {
        let Some(job) = ops::check(&part) else { return };
        let name = part.label();
        self.run(&format!("檢查「{name}」"), job, move |this, result| match result {
            Err(why) => this.failed(&format!("無法檢查「{name}」"), &why),
            Ok(said) => {
                let text = gtk::Label::builder().label(said.trim()).selectable(true).xalign(0.0).wrap(true).css_classes(["monospace", "caption"]).build();
                let scroll = gtk::ScrolledWindow::builder().child(&text).min_content_height(120).max_content_height(260).width_request(alert::FIELD_WIDTH + 60).propagate_natural_height(true).build();
                let alert = adw::AlertDialog::new(Some(&format!("檢查「{name}」")), None);
                alert.set_extra_child(Some(&scroll));
                alert.add_response("close", "關閉");
                alert.present(Some(&this.window));
            }
        });
    }

    fn new_table(self: &Rc<Self>) {
        let Some(disk) = self.current_disk() else { return };
        let kind = combo("類型", &["GPT", "MBR"], 0);
        let extra = form(&[kind.upcast_ref()]);
        let this = self.clone();
        self.ask("建立分割表", Some("磁碟上的所有資料會被清除，無法復原。"), &extra, "建立", true, move || {
            let Some(disk) = disk_now(&disk) else {
                this.failed("無法建立分割表", CHANGED);
                return;
            };
            let kind = if kind.selected() == 0 { "gpt" } else { "msdos" };
            this.run_job("建立分割表", "無法建立分割表".into(), ops::make_table(&disk, kind));
        });
    }

    fn wipe(self: &Rc<Self>, disk: Disk) {
        let how = combo("清除方式", &["快速", "覆寫零"], 0);
        let table = combo("分割表", &["GPT", "MBR", "無"], 0);
        let extra = form(&[how.upcast_ref(), table.upcast_ref()]);
        let this = self.clone();
        let title = disk.title();
        self.ask(&format!("清除「{title}」？"), Some("磁碟上的所有分割區與資料會被清除，無法復原。"), &extra, "清除", true, move || {
            let Some(disk) = disk_now(&disk) else {
                this.failed(&format!("無法清除「{title}」"), CHANGED);
                return;
            };
            let table = [Some("gpt"), Some("msdos"), None][table.selected() as usize];
            let job = ops::wipe(&disk, how.selected() == 1, table);
            this.run_job(&format!("清除「{title}」"), format!("無法清除「{title}」"), job);
        });
    }

    fn new_partition(self: &Rc<Self>, disk: Disk, start: u64, end: u64) {
        let start = start.div_ceil(MIB) * MIB;
        let free = (end.saturating_sub(start)) / MIB * MIB;
        if free < 8 * MIB {
            return;
        }
        let top = free as f64 / 1e9;
        let adjustment = gtk::Adjustment::new(top, 0.1_f64.min(top), top, 1.0, 10.0, 0.0);
        let size = adw::SpinRow::new(Some(&adjustment), 1.0, 1);
        size.set_title("大小 (GB)");
        let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&adjustment));
        scale.set_draw_value(false);
        let known: Vec<&str> = ops::FILE_SYSTEMS.iter().map(|(_, name)| *name).collect();
        let fs = combo("檔案系統", &known, 0);
        let name = entry("名稱", "");
        let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).build();
        extra.append(&form(&[size.upcast_ref(), fs.upcast_ref(), name.upcast_ref()]));
        extra.append(&scale);
        let this = self.clone();
        self.ask("新增分割區", None, &extra, "新增", false, move || {
            let Some(disk) = disk_now(&disk) else {
                this.failed("無法新增分割區", CHANGED);
                return;
            };
            let wanted = adjustment.value();
            let bytes = if wanted >= top - 0.05 { free } else { ((wanted * 1e9) as u64 / MIB * MIB).max(MIB) };
            let fstype = ops::FILE_SYSTEMS[fs.selected() as usize].0;
            let job = ops::create(&disk, start, bytes, fstype, name.text().as_str());
            this.run_job("新增分割區", "無法新增分割區".into(), job);
        });
    }

    fn resize(self: &Rc<Self>, disk: Disk, part: Part) {
        let Some(kind) = ops::resize_kind(&part) else { return };
        let (low, high) = (ops::min_size(&part), part.size + disk.free_after(&part));
        let (low_gb, high_gb) = (low as f64 / 1e9, (high as f64 / 1e9).max(low as f64 / 1e9 + 0.1));
        let adjustment = gtk::Adjustment::new(part.size as f64 / 1e9, low_gb, high_gb, 1.0, 10.0, 0.0);
        let size = adw::SpinRow::new(Some(&adjustment), 1.0, 1);
        size.set_title("新大小 (GB)");
        let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&adjustment));
        scale.set_draw_value(false);
        let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).build();
        extra.append(&form(&[fact("目前", &model::size(part.size)).upcast_ref(), fact("最小", &model::size(low)).upcast_ref(), size.upcast_ref()]));
        extra.append(&scale);
        let this = self.clone();
        let name = part.label();
        self.ask(&format!("調整「{name}」大小"), None, &extra, "調整大小", false, move || {
            let (Some(disk), Some(part)) = (disk_now(&disk), part_now(&part)) else {
                this.failed(&format!("無法調整「{name}」大小"), CHANGED);
                return;
            };
            let wanted = adjustment.value();
            let bytes = if (wanted - high_gb).abs() < 0.05 { high } else { ((wanted * 1e9) as u64).clamp(low, high) };
            let job = ops::resize(&disk, &part, kind, bytes);
            this.run_job(&format!("調整「{name}」大小"), format!("無法調整「{name}」大小"), job);
        });
    }

    fn image_create(self: &Rc<Self>, disk: Disk) {
        let has_zstd = ops::which("zstd").is_some();
        let compress = combo("壓縮", if has_zstd { &["zstd", "無"] } else { &["無"] }, 0);
        let extra = form(&[compress.upcast_ref()]);
        let this = self.clone();
        let title = disk.title();
        self.ask(&format!("建立「{title}」的映像檔"), None, &extra, "建立", false, move || {
            let zstd = has_zstd && compress.selected() == 0;
            let dialog = gtk::FileDialog::builder()
                .title("儲存映像檔")
                .initial_name(format!("{}.img{}", disk.kname, if zstd { ".zst" } else { "" }))
                .modal(true)
                .build();
            let this = this.clone();
            let window = this.window.clone();
            let disk = disk.clone();
            let title = title.clone();
            dialog.save(Some(&window), gio::Cancellable::NONE, move |chosen| {
                if let Some(path) = chosen.ok().and_then(|file| file.path()) {
                    let job = ops::image_create(&disk, &path, zstd);
                    this.run_job(&format!("建立「{title}」的映像檔"), "無法建立映像檔".into(), job);
                }
            });
        });
    }

    fn image_restore(self: &Rc<Self>, disk: Disk) {
        let dialog = gtk::FileDialog::builder().title("選擇映像檔").modal(true).build();
        let this = self.clone();
        let window = self.window.clone();
        dialog.open(Some(&window), gio::Cancellable::NONE, move |chosen| {
            let Some(path) = chosen.ok().and_then(|file| file.path()) else { return };
            let title = disk.title();
            let file = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let extra = form(&[fact("映像檔", &file).upcast_ref(), fact("目標", &title).upcast_ref()]);
            let this = this.clone();
            let disk = disk.clone();
            let again = this.clone();
            again.ask("還原磁碟映像檔", Some("目標磁碟上的所有資料會被覆寫，無法復原。"), &extra, "還原", true, move || {
                let Some(disk) = disk_now(&disk) else {
                    this.failed(&format!("無法還原「{title}」"), CHANGED);
                    return;
                };
                let job = ops::image_restore(&path, &disk);
                this.run_job(&format!("還原「{title}」"), format!("無法還原「{title}」"), job);
            });
        });
    }

    fn choose_image(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("磁碟映像檔"));
        for suffix in ["iso", "img", "raw", "dd"] {
            filter.add_suffix(suffix);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("開啟映像檔").filters(&filters).modal(true).build();
        let this = self.clone();
        let window = self.window.clone();
        dialog.open(Some(&window), gio::Cancellable::NONE, move |chosen| {
            if let Some(path) = chosen.ok().and_then(|file| file.path()) {
                this.open_image(&path);
            }
        });
    }

    fn open_image(self: &Rc<Self>, path: &Path) {
        self.run_job("開啟映像檔", "無法開啟映像檔".into(), ops::open_image(path));
    }

    fn show_smart(self: &Rc<Self>, disk: Disk) {
        let title = disk.title();
        self.run(&format!("讀取「{title}」的 SMART 資料"), ops::smart(&disk, true), move |this, result| {
            let smart = match result {
                Ok(json) => ops::parse_smart(&json),
                Err(why) => return this.failed("無法讀取 SMART 資料", &why),
            };
            let Some(smart) = smart else { return alert::notice(&this.window, "沒有 SMART 資料", None) };
            this.health.borrow_mut().insert(disk.kname.clone(), Some(ops::Smart { passed: smart.passed, temperature: smart.temperature, rows: smart.rows.clone() }));
            let rows = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).width_request(alert::FIELD_WIDTH + 80).build();
            for (name, value) in &smart.rows {
                rows.append(&fact(name, value));
            }
            let alert = adw::AlertDialog::new(Some("SMART 資料"), None);
            alert.set_extra_child(Some(&rows));
            alert.add_response("close", "關閉");
            alert.present(Some(&this.window));
        });
    }

    fn show_speed(self: &Rc<Self>, disk: Disk) {
        let home = std::env::var("HOME").unwrap_or_default();
        let point = disk
            .parts
            .iter()
            .filter_map(|part| part.mount_point())
            .find(|point| ["/media/", "/mnt/", "/run/media/", "/home"].iter().any(|start| point.starts_with(start)))
            .map(str::to_string)
            .or_else(|| disk.parts.iter().any(|part| part.mount_point() == Some("/")).then_some(home).filter(|home| !home.is_empty()));
        let title = disk.title();
        let again = disk.clone();
        self.run(&format!("測試「{title}」的速度"), ops::speed(&disk, point.as_deref()), move |this, result| {
            let said = match result {
                Ok(said) => said,
                Err(why) => return this.failed("無法測試速度", &why),
            };
            let (read, write) = ops::speeds(&said);
            let speed = |bytes: Option<f64>| bytes.map(|bytes| if bytes >= 1e9 { format!("{:.1} GB/s", bytes / 1e9) } else { format!("{:.0} MB/s", bytes / 1e6) }).unwrap_or_else(|| "—".into());
            let rows = form(&[fact("讀取", &speed(read)).upcast_ref(), fact("寫入", &speed(write)).upcast_ref()]);
            let alert = adw::AlertDialog::new(Some(&format!("測試「{title}」的速度")), None);
            alert.set_extra_child(Some(&rows));
            alert.add_responses(&[("close", "關閉"), ("again", "再測一次")]);
            alert.set_response_appearance("again", adw::ResponseAppearance::Suggested);
            alert.set_close_response("close");
            let this = this.clone();
            let window = this.window.clone();
            alert.choose(&window, gio::Cancellable::NONE, move |response| {
                if response == "again" {
                    this.show_speed(again);
                }
            });
        });
    }

    /// Asks with `extra` under the question; `then` does it.
    fn ask(&self, title: &str, body: Option<&str>, extra: &impl IsA<gtk::Widget>, verb: &str, destructive: bool, then: impl FnOnce() + 'static) {
        let alert = adw::AlertDialog::new(Some(title), body);
        alert.set_extra_child(Some(extra));
        alert.add_responses(&[("cancel", "取消"), ("do", verb)]);
        alert.set_response_appearance("do", if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested });
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            if response == "do" {
                then();
            }
        });
    }
}

fn css() -> &'static str {
    "
.dm-group { font-size: 12px; font-weight: 600; opacity: .7; padding: 12px 12px 4px; }
.dm-mini { min-height: 4px; margin-top: 6px; }
.dm-mini trough, .dm-mini progress, .dm-mini trough > progress { min-height: 4px; border-radius: 2px; }
.dm-value { font-size: 15px; font-weight: 600; }
.dm-chip { font-size: 12px; font-weight: 600; padding: 2px 9px; border-radius: 99px; background: alpha(@success_color, .16); color: @success_color; }
.dm-chip.neutral { background: alpha(currentColor, .1); color: inherit; opacity: .8; }
.dm-chip.warn { background: alpha(@error_color, .16); color: @error_color; }
.dm-list { background: none; }
.dm-row { padding: 12px 14px; }
.dm-head { font-size: 12px; font-weight: 600; opacity: .7; }
.dm-row.dm-head { padding-top: 10px; padding-bottom: 10px; }
.dm-name { font-weight: 500; }
.dm-actions { padding: 12px 14px; min-height: 40px; }
.dm-who { font-size: 15px; font-weight: 600; }
.dm-dev { font-size: 12.5px; opacity: .65; font-family: monospace; }
"
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css());
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    let window: Rc<RefCell<Option<Rc<Window>>>> = Rc::default();
    let held = window.clone();
    app.connect_activate(move |app| match app.active_window() {
        Some(window) => window.present(),
        None => {
            held.replace(Some(Window::open(app)));
        }
    });
    // An image opened with it is a drive.
    let held = window.clone();
    app.connect_open(move |app, files, _| {
        if held.borrow().is_none() {
            held.replace(Some(Window::open(app)));
        }
        let win = held.borrow().clone();
        for path in files.iter().filter_map(|file| file.path()).collect::<Vec<PathBuf>>() {
            if let Some(win) = &win {
                win.open_image(&path);
            }
        }
    });
    app.run()
}
