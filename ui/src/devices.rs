//! `nekoland-devices`, 裝置管理員: the devices of this computer as Windows' Device Manager shows
//! them, by kind or by what each is connected to, marked when one has no driver or is
//! disabled. A device's 內容 tells of it, its driver, the details udev keeps, the interrupts and
//! ranges it uses, its power settings and what the kernel said of it. A device can be
//! disabled, which lasts across restarts, enabled again, and uninstalled until hardware is
//! scanned for again. What is done to a device is done as root, after asking for the
//! administrator's password, as 磁碟 does.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod alert;
#[allow(dead_code)]
mod edid;
#[path = "devices/model.rs"]
mod model;
#[path = "devices/ops.rs"]
mod ops;
mod program;

use model::{Device, Kind, State};

const APP_ID: &str = "io.github.koukeneko.Nekoland.Devices";
/// How often the devices are read again, to show one plugged in or taken out.
const REFRESH: Duration = Duration::from_secs(3);
/// How wide the driver's and the state's columns are.
const COLUMN_WIDTHS: [i32; 2] = [170, 110];
/// How far a device stands in from what it is connected to.
const INDENT: i32 = 20;
/// How many of the other devices a disabling takes along are named.
const NAMED: usize = 3;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Tree,
    Kind(Kind),
}

struct Window {
    window: adw::ApplicationWindow,
    devices: RefCell<Vec<Device>>,
    view: Cell<View>,
    /// The chosen device's path, kept across reading the devices again.
    chosen: RefCell<Option<String>>,
    expanded: RefCell<HashSet<String>>,
    hidden: Cell<bool>,
    /// What the system runs from, which is not disabled from under it.
    system: Vec<String>,
    busy: Cell<bool>,

    sidebar: gtk::ListBox,
    views: RefCell<Vec<View>>,
    groups: Rc<RefCell<Vec<&'static str>>>,
    quiet: Cell<bool>,
    stack: gtk::Stack,
    title: gtk::Label,
    chip: gtk::Label,
    table: gtk::ListBox,
    table_scroll: gtk::ScrolledWindow,
    /// The path of each row of the table.
    rows: RefCell<Vec<String>>,
    columns: Vec<gtk::SizeGroup>,
    actions: gtk::Box,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

/// A row stating a fact, which can be selected to copy.
fn fact(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).use_markup(false).build();
    let value = gtk::Label::builder()
        .label(value)
        .css_classes(["dim-label"])
        .selectable(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .tooltip_text(value)
        .build();
    row.add_suffix(&value);
    row
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

/// What a disabling takes along: the other devices on the same one, or under it.
fn taken_along(device: &Device, devices: &[Device]) -> Vec<String> {
    let inside = format!("{}/", device.node);
    devices
        .iter()
        .filter(|other| {
            !other.hidden
                && other.path != device.path
                && (other.node == device.node || other.path.starts_with(&inside))
        })
        .fold(Vec::new(), |mut names, other| {
            if !names.contains(&other.name) {
                names.push(other.name.clone());
            }
            names
        })
}

/// What disabling a device of `kind` does besides, as Windows warns of it.
fn warning(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Display => Some("使用這張顯示卡的螢幕會停止顯示。"),
        Kind::Keyboard | Kind::Mouse | Kind::UsbController => Some("可能無法再操作這部電腦。"),
        Kind::Network => Some("透過這個裝置的網路連線會中斷。"),
        Kind::Storage | Kind::Disk => Some("上面的磁碟會無法使用。"),
        _ => None,
    }
}

impl Window {
    fn open(app: &adw::Application) -> Rc<Window> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("裝置管理員")
            .default_width(1100)
            .default_height(720)
            .build();

        let sidebar = gtk::ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();
        let groups: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let known_groups = groups.clone();
        sidebar.set_header_func(move |row, before| {
            let groups = known_groups.borrow();
            let here = groups.get(row.index() as usize).copied();
            let before = before.and_then(|row| groups.get(row.index() as usize).copied());
            match (here, before) {
                (Some(here), before) if before != Some(here) => {
                    row.set_header(Some(&label(here, &["dm-group"])))
                }
                _ => row.set_header(None::<&gtk::Widget>),
            }
        });
        let scan = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("掃描硬體變更")
            .action_name("win.scan")
            .build();
        let sidebar_bar = adw::HeaderBar::new();
        sidebar_bar.pack_end(&scan);
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_bar);
        sidebar_view.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .child(&sidebar)
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        ));
        let sidebar_page = adw::NavigationPage::builder()
            .title("裝置管理員")
            .child(&sidebar_view)
            .build();

        let title = label("", &["title-4"]);
        let chip = label("", &["dm-chip", "warn"]);
        chip.set_visible(false);
        let heading = gtk::Box::builder().spacing(10).valign(gtk::Align::Center).build();
        heading.append(&title);
        heading.append(&chip);
        let more_menu = gio::Menu::new();
        more_menu.append(Some("顯示隱藏的裝置"), Some("win.show-hidden"));
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&more_menu)
            .tooltip_text("更多")
            .build();
        let content_bar = adw::HeaderBar::new();
        content_bar.set_title_widget(Some(&heading));
        content_bar.pack_end(&more);

        let columns: Vec<gtk::SizeGroup> = COLUMN_WIDTHS
            .iter()
            .map(|_| gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal))
            .collect();
        // A double click opens a device's 內容, as in Windows.
        let table = gtk::ListBox::builder()
            .css_classes(["dm-list"])
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .build();
        let table_scroll = gtk::ScrolledWindow::builder()
            .child(&table)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .build();
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["card"])
            .overflow(gtk::Overflow::Hidden)
            .valign(gtk::Align::Start)
            .build();
        card.append(&Window::header_row(&columns));
        card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        card.append(&table_scroll);

        let actions = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .css_classes(["card", "dm-actions"])
            .build();

        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(24)
            .margin_end(24)
            .build();
        page.append(&card);
        page.append(&actions);

        let empty = adw::StatusPage::builder()
            .icon_name("computer-symbolic")
            .title("沒有裝置")
            .build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .build();
        stack.add_named(&page, Some("devices"));
        stack.add_named(&empty, Some("empty"));
        stack.set_visible_child_name("empty");
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_bar);
        content_view.set_content(Some(&stack));
        let content = adw::NavigationPage::builder()
            .title("裝置管理員")
            .child(&content_view)
            .build();

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content)
            .min_sidebar_width(240.0)
            .max_sidebar_width(300.0)
            .build();
        window.set_content(Some(&split));

        let win = Rc::new(Window {
            window,
            devices: RefCell::default(),
            view: Cell::new(View::Tree),
            chosen: RefCell::default(),
            expanded: RefCell::default(),
            hidden: Cell::new(false),
            system: model::system_paths(),
            busy: Cell::new(false),
            sidebar,
            views: RefCell::default(),
            groups,
            quiet: Cell::new(false),
            stack,
            title,
            chip,
            table,
            table_scroll,
            rows: RefCell::default(),
            columns,
            actions,
        });
        win.wire();
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
        let row = gtk::Box::builder()
            .spacing(12)
            .css_classes(["dm-row", "dm-head"])
            .build();
        let name = label("名稱", &["dm-head"]);
        name.set_hexpand(true);
        row.append(&name);
        for (index, title) in ["驅動程式", "狀態"].iter().enumerate() {
            let cell = label(title, &["dm-head"]);
            cell.set_width_request(COLUMN_WIDTHS[index]);
            columns[index].add_widget(&cell);
            row.append(&cell);
        }
        row
    }

    fn wire(self: &Rc<Self>) {
        let this = self.clone();
        self.sidebar.connect_row_selected(move |_, row| {
            if this.quiet.get() {
                return;
            }
            let Some(row) = row else { return };
            let view = this.views.borrow().get(row.index() as usize).copied();
            if let Some(view) = view {
                if this.view.get() != view {
                    this.view.set(view);
                    this.chosen.replace(None);
                    this.fill();
                    this.table_scroll.vadjustment().set_value(0.0);
                }
            }
        });
        let this = self.clone();
        self.table.connect_row_selected(move |_, row| {
            if this.quiet.get() {
                return;
            }
            let path = row.and_then(|row| this.rows.borrow().get(row.index() as usize).cloned());
            if path.is_some() && *this.chosen.borrow() != path {
                this.chosen.replace(path);
                this.fill_actions();
            }
        });
        let this = self.clone();
        self.table.connect_row_activated(move |_, _| {
            if let Some(device) = this.current() {
                this.properties(device);
            }
        });

        // A right click chooses the device under it and opens what can be done to it.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let this = self.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(row) = this.table.row_at_y(y as i32) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            this.table.select_row(Some(&row));
            this.popup_menu(x, y);
        });
        self.table.add_controller(click);
        // Right and Left open and close a device in the tree, as in Windows.
        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let open = match key {
                gdk::Key::Right => true,
                gdk::Key::Left => false,
                gdk::Key::Menu => {
                    if let Some(row) = this.table.selected_row() {
                        let bounds = row.compute_bounds(&this.table);
                        let (x, y) = bounds.map_or((0.0, 0.0), |bounds| {
                            (bounds.x() as f64 + 40.0, (bounds.y() + bounds.height()) as f64)
                        });
                        this.popup_menu(x, y);
                    }
                    return glib::Propagation::Stop;
                }
                _ => return glib::Propagation::Proceed,
            };
            if this.view.get() != View::Tree {
                return glib::Propagation::Proceed;
            }
            let Some(path) = this.chosen.borrow().clone() else {
                return glib::Propagation::Proceed;
            };
            let changed = if open {
                this.expanded.borrow_mut().insert(path)
            } else {
                this.expanded.borrow_mut().remove(&path)
            };
            if changed {
                this.fill_table();
            }
            glib::Propagation::Stop
        });
        self.table.add_controller(keys);

        let add = |name: &str, run: fn(&Rc<Window>, Device)| {
            let action = gio::SimpleAction::new(name, None);
            let this = self.clone();
            action.connect_activate(move |_, _| {
                if let Some(device) = this.current() {
                    run(&this, device);
                }
            });
            self.window.add_action(&action);
        };
        add("properties", |this, device| this.properties(device));
        add("disable", Window::disable);
        add("enable", |this, device| {
            let name = device.name.clone();
            this.run_root(format!("無法啟用「{name}」"), move || ops::enable(&device));
        });
        add("uninstall", Window::uninstall);
        let scan = gio::SimpleAction::new("scan", None);
        let this = self.clone();
        scan.connect_activate(move |_, _| this.run_root("無法掃描硬體變更".into(), ops::scan));
        self.window.add_action(&scan);
        let refresh = gio::SimpleAction::new("refresh", None);
        let this = self.clone();
        refresh.connect_activate(move |_, _| this.reload());
        self.window.add_action(&refresh);
        let hidden = gio::SimpleAction::new_stateful("show-hidden", None, &false.to_variant());
        let this = self.clone();
        hidden.connect_change_state(move |action, state| {
            let Some(on) = state.and_then(|state| state.get::<bool>()) else {
                return;
            };
            action.set_state(&on.to_variant());
            this.hidden.set(on);
            this.fill_sidebar();
            this.fill();
            this.table_scroll.vadjustment().set_value(0.0);
        });
        self.window.add_action(&hidden);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.refresh", &["F5", "<Control>r"]);
            app.set_accels_for_action("win.properties", &["<Alt>Return"]);
        }
    }

    fn popup_menu(&self, x: f64, y: f64) {
        let Some(device) = self.current() else { return };
        let menu = gio::Menu::new();
        let first = gio::Menu::new();
        first.append(Some("內容"), Some("win.properties"));
        menu.append_section(None, &first);
        let change = gio::Menu::new();
        if device.state == State::Disabled {
            change.append(Some("啟用裝置"), Some("win.enable"));
        } else {
            change.append(Some("停用裝置"), Some("win.disable"));
        }
        change.append(Some("解除安裝裝置"), Some("win.uninstall"));
        menu.append_section(None, &change);
        let scan = gio::Menu::new();
        scan.append(Some("掃描硬體變更"), Some("win.scan"));
        menu.append_section(None, &scan);
        // A new one each time, with the chosen device's own items.
        let popover = gtk::PopoverMenu::builder()
            .menu_model(&menu)
            .has_arrow(false)
            .halign(gtk::Align::Start)
            .build();
        // On the card rather than the list: a list box does not lay out a popover of its own.
        let Some(card) = self.table_scroll.parent() else {
            return;
        };
        let at = self
            .table
            .compute_point(&card, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or(gtk::graphene::Point::new(x as f32, y as f32));
        popover.set_parent(&card);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(at.x() as i32, at.y() as i32, 1, 1)));
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            // Unparented after it has finished closing, which the activated item still needs.
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }

    fn shown(&self) -> Vec<Device> {
        self.devices
            .borrow()
            .iter()
            .filter(|device| self.hidden.get() || !device.hidden)
            .cloned()
            .collect()
    }

    fn current(&self) -> Option<Device> {
        let path = self.chosen.borrow().clone()?;
        self.devices
            .borrow()
            .iter()
            .find(|device| device.path == path)
            .cloned()
    }

    fn reload(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(model::read).await {
                Ok(Ok(devices)) => this.apply(devices),
                Ok(Err(why)) => eprintln!("nekoland-devices: {why}"),
                Err(_) => eprintln!("nekoland-devices: reading the devices stopped"),
            }
        });
    }

    fn apply(self: &Rc<Self>, devices: Vec<Device>) {
        if *self.devices.borrow() == devices {
            return;
        }
        if self.devices.borrow().is_empty() {
            // The tree starts with the computer open, as in Windows.
            let computer = devices
                .iter()
                .find(|device| device.kind == Kind::Computer)
                .map(|device| device.path.clone());
            self.expanded.borrow_mut().extend(computer);
        }
        self.devices.replace(devices);
        self.fill_sidebar();
        self.fill();
    }

    fn fill_sidebar(self: &Rc<Self>) {
        self.quiet.set(true);
        while let Some(row) = self.sidebar.row_at_index(0) {
            self.sidebar.remove(&row);
        }
        let shown = self.shown();
        let mut views = vec![View::Tree];
        let mut groups = vec!["依連線"];
        self.sidebar.append(&Self::sidebar_row(
            "computer-symbolic",
            &glib::host_name(),
            None,
            false,
        ));
        for kind in model::KINDS {
            let of_kind: Vec<&Device> = shown.iter().filter(|device| device.kind == kind).collect();
            if of_kind.is_empty() {
                continue;
            }
            let problem = of_kind.iter().any(|device| device.state == State::NoDriver);
            self.sidebar.append(&Self::sidebar_row(
                kind.icon(),
                kind.name(),
                Some(of_kind.len()),
                problem,
            ));
            views.push(View::Kind(kind));
            groups.push("依類型");
        }
        // A kind no longer shown leaves the tree shown instead.
        if !views.contains(&self.view.get()) {
            self.view.set(View::Tree);
        }
        let at = views
            .iter()
            .position(|view| *view == self.view.get())
            .unwrap_or(0);
        self.views.replace(views);
        self.groups.replace(groups);
        self.sidebar.invalidate_headers();
        self.sidebar
            .select_row(self.sidebar.row_at_index(at as i32).as_ref());
        self.quiet.set(false);
    }

    fn sidebar_row(icon: &str, name: &str, count: Option<usize>, problem: bool) -> gtk::ListBoxRow {
        let row = gtk::Box::builder().spacing(8).build();
        row.append(&gtk::Image::from_icon_name(icon));
        let name = label(name, &[]);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&name);
        if problem {
            let warn = gtk::Image::builder()
                .icon_name("dialog-warning-symbolic")
                .css_classes(["warning"])
                .tooltip_text(State::NoDriver.name())
                .build();
            row.append(&warn);
        }
        if let Some(count) = count {
            row.append(&label(&count.to_string(), &["numeric", "dim-label", "caption"]));
        }
        gtk::ListBoxRow::builder().child(&row).build()
    }

    /// Shows the chosen kind, or the tree.
    fn fill(self: &Rc<Self>) {
        let shown = self.shown();
        let in_view: Vec<&Device> = match self.view.get() {
            View::Tree => shown.iter().collect(),
            View::Kind(kind) => shown.iter().filter(|device| device.kind == kind).collect(),
        };
        self.title.set_label(&match self.view.get() {
            View::Tree => glib::host_name().to_string(),
            View::Kind(kind) => kind.name().to_string(),
        });
        let problems = in_view
            .iter()
            .filter(|device| device.state == State::NoDriver)
            .count();
        self.chip.set_visible(problems > 0);
        self.chip.set_label(&format!("{problems} 個問題"));
        if in_view.is_empty() {
            self.stack.set_visible_child_name("empty");
            self.chosen.replace(None);
            return;
        }
        self.fill_table();
        self.stack.set_visible_child_name("devices");
    }

    fn fill_table(self: &Rc<Self>) {
        let shown = self.shown();
        let tree = self.view.get() == View::Tree;
        let listed: Vec<&Device> = match self.view.get() {
            View::Tree => shown.iter().collect(),
            View::Kind(kind) => shown.iter().filter(|device| device.kind == kind).collect(),
        };
        let rows: Vec<(usize, usize, bool)> = if tree {
            model::tree(&listed, &self.expanded.borrow())
        } else {
            (0..listed.len()).map(|index| (index, 0, false)).collect()
        };
        // What was chosen stays chosen; under a closed device, that device is.
        let chosen = self.chosen.borrow().clone();
        let at = chosen
            .as_ref()
            .and_then(|path| {
                rows.iter()
                    .position(|&(index, ..)| &listed[index].path == path)
                    .or_else(|| {
                        rows.iter()
                            .enumerate()
                            .filter(|(_, &(index, ..))| path.starts_with(&format!("{}/", listed[index].path)))
                            .map(|(at, _)| at)
                            .next_back()
                    })
            })
            .unwrap_or(0);
        self.quiet.set(true);
        while let Some(row) = self.table.row_at_index(0) {
            self.table.remove(&row);
        }
        let mut paths = Vec::new();
        for (position, &(index, depth, under)) in rows.iter().enumerate() {
            let device = listed[index];
            let row = gtk::Box::builder().spacing(12).css_classes(["dm-row"]).build();
            let first = gtk::Box::builder().spacing(9).hexpand(true).build();
            if tree {
                first.set_margin_start(depth as i32 * INDENT);
                first.append(&self.expander(device, under));
            }
            first.append(&gtk::Image::from_icon_name(device.kind.icon()));
            let name = label(&device.name, &["dm-name"]);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_tooltip_text(Some(&device.name));
            first.append(&name);
            row.append(&first);
            let driver = label(device.driver.as_deref().unwrap_or("—"), &["dim-label"]);
            driver.set_ellipsize(gtk::pango::EllipsizeMode::End);
            driver.set_width_request(COLUMN_WIDTHS[0]);
            self.columns[0].add_widget(&driver);
            row.append(&driver);
            let state = gtk::Box::builder()
                .width_request(COLUMN_WIDTHS[1])
                .valign(gtk::Align::Center)
                .build();
            state.append(&match device.state {
                State::Working => label(State::Working.name(), &["dim-label"]),
                State::Disabled => label(State::Disabled.name(), &["dm-chip", "neutral"]),
                State::NoDriver => label(State::NoDriver.name(), &["dm-chip", "warn"]),
            });
            self.columns[1].add_widget(&state);
            row.append(&state);
            let list_row = gtk::ListBoxRow::builder().child(&row).build();
            self.table.append(&list_row);
            if position == at {
                self.table.select_row(Some(&list_row));
            }
            paths.push(device.path.clone());
        }
        self.chosen.replace(paths.get(at).cloned());
        self.rows.replace(paths);
        self.quiet.set(false);
        self.fill_actions();
    }

    /// The arrow that opens or closes a device in the tree, or room for one.
    fn expander(self: &Rc<Self>, device: &Device, under: bool) -> gtk::Widget {
        if !under {
            return gtk::Box::builder().css_classes(["dm-expander"]).build().upcast();
        }
        let open = self.expanded.borrow().contains(&device.path);
        let button = gtk::Button::builder()
            .icon_name(if open {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            })
            .css_classes(["flat", "dm-expander"])
            .valign(gtk::Align::Center)
            .tooltip_text(if open { "收合" } else { "展開" })
            .build();
        let this = self.clone();
        let path = device.path.clone();
        button.connect_clicked(move |_| {
            {
                let mut expanded = this.expanded.borrow_mut();
                if !expanded.remove(&path) {
                    expanded.insert(path.clone());
                }
            }
            this.fill_table();
        });
        button.upcast()
    }

    fn set_enabled(&self, action: &str, on: bool) {
        if let Some(action) = self
            .window
            .lookup_action(action)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(on);
        }
    }

    fn button(&self, text: &str, class: Option<&str>, action: &str) {
        let button = gtk::Button::builder()
            .label(text)
            .action_name(format!("win.{action}"))
            .build();
        if let Some(class) = class {
            button.add_css_class(class);
        }
        self.actions.append(&button);
    }

    fn fill_actions(&self) {
        while let Some(child) = self.actions.first_child() {
            self.actions.remove(&child);
        }
        let device = self.current();
        let can_change = device
            .as_ref()
            .is_some_and(|device| device.kind.can_change() && !device.node.is_empty())
            && !self.busy.get();
        let disabled = device
            .as_ref()
            .is_some_and(|device| device.state == State::Disabled);
        self.set_enabled("properties", device.is_some());
        self.set_enabled("disable", can_change && !disabled);
        self.set_enabled("enable", can_change && disabled);
        self.set_enabled(
            "uninstall",
            can_change && device.as_ref().is_some_and(ops::can_uninstall),
        );
        self.set_enabled("scan", !self.busy.get());
        let Some(device) = device else { return };
        let who = gtk::Box::builder()
            .spacing(9)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        who.append(&gtk::Image::from_icon_name(device.kind.icon()));
        let name = label(&device.name, &["dm-who"]);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        who.append(&name);
        if !device.node.is_empty() {
            who.append(&label(device.node_name(), &["dm-dev"]));
        }
        self.actions.append(&who);
        self.button("內容", None, "properties");
        if disabled {
            self.button("啟用裝置", Some("suggested-action"), "enable");
        } else {
            self.button("停用裝置", None, "disable");
        }
        self.button("解除安裝裝置", Some("destructive-action"), "uninstall");
    }

    // --- what is done ---------------------------------------------------------------------

    /// Runs `job` as root in a thread, then reads the devices again.
    fn run_root(self: &Rc<Self>, failure: String, job: impl FnOnce() -> Result<(), String> + Send + 'static) {
        if self.busy.replace(true) {
            return;
        }
        self.fill_actions();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(job)
                .await
                .unwrap_or_else(|_| Err("已停止".into()));
            this.busy.set(false);
            if let Err(why) = result {
                alert::notice(&this.window, &failure, Some(&why));
            }
            this.fill_actions();
            this.reload();
        });
    }

    fn refuse_system(&self, verb: &str, device: &Device) -> bool {
        let holds = model::holds_system(device, &self.system);
        if holds {
            alert::notice(
                &self.window,
                &format!("無法{verb}「{}」", device.name),
                Some("系統磁碟連接在這個裝置上。"),
            );
        }
        holds
    }

    fn disable(self: &Rc<Self>, device: Device) {
        if self.refuse_system("停用", &device) {
            return;
        }
        let along = taken_along(&device, &self.devices.borrow());
        let mut body: Vec<String> = warning(device.kind).into_iter().map(str::to_string).collect();
        if !along.is_empty() {
            let mut names: Vec<String> = along
                .iter()
                .take(NAMED)
                .map(|name| format!("「{name}」"))
                .collect();
            if along.len() > NAMED {
                names.push(format!("其他 {} 個裝置", along.len() - NAMED));
            }
            body.push(format!("{}也會一併停用。", names.join("、")));
        }
        let body = (!body.is_empty()).then(|| body.join("\n"));
        let this = self.clone();
        alert::confirm(
            &self.window,
            &format!("停用「{}」？", device.name),
            body.as_deref(),
            "停用",
            move || {
                let name = device.name.clone();
                this.run_root(format!("無法停用「{name}」"), move || ops::disable(&device));
            },
        );
    }

    fn uninstall(self: &Rc<Self>, device: Device) {
        if self.refuse_system("解除安裝", &device) {
            return;
        }
        let body = if device.bus == "usb" {
            "要再使用，請拔除後重新插上。"
        } else {
            "掃描硬體變更後會再找到它。"
        };
        let this = self.clone();
        alert::confirm(
            &self.window,
            &format!("解除安裝「{}」？", device.name),
            Some(body),
            "解除安裝",
            move || {
                let name = device.name.clone();
                this.run_root(format!("無法解除安裝「{name}」"), move || {
                    ops::uninstall(&device)
                });
            },
        );
    }

    // --- 內容 -----------------------------------------------------------------------------

    fn properties(self: &Rc<Self>, device: Device) {
        let mut pages: Vec<(&str, gtk::Widget)> = vec![
            ("一般", self.general_page(&device).upcast()),
            ("驅動程式", driver_page(&device).upcast()),
            ("詳細資料", details_page(&device).upcast()),
        ];
        if !device.node.is_empty() {
            let resources = model::resources(&device.node);
            if !resources.is_empty() {
                pages.push(("資源", resources_page(&resources).upcast()));
            }
            let power = model::power(&device.node);
            if power != model::Power::default() && device.kind.can_change() {
                pages.push(("電源管理", power_page(&device, power).upcast()));
            }
        }
        pages.push(("事件", events_page(&device).upcast()));
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .build();
        for (index, (_, page)) in pages.iter().enumerate() {
            stack.add_named(page, Some(&index.to_string()));
        }
        let labels: Vec<&str> = pages.iter().map(|(name, _)| *name).collect();
        let switcher = segmented(&labels);
        let shown = stack.clone();
        switcher.connect_notify_local(Some("active"), move |switcher, _| {
            shown.set_visible_child_name(&switcher.property::<u32>("active").to_string())
        });
        let bar = adw::HeaderBar::new();
        bar.set_title_widget(Some(&switcher));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&bar);
        view.set_content(Some(&stack));
        let dialog = adw::Dialog::builder()
            .title(format!("{} 內容", device.name))
            .content_width(600)
            .content_height(640)
            .child(&view)
            .build();
        dialog.present(Some(&self.window));
    }

    fn general_page(&self, device: &Device) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();
        let who = group("");
        let head = adw::ActionRow::builder()
            .title(&device.name)
            .subtitle(device.kind.name())
            .use_markup(false)
            .build();
        head.add_prefix(
            &gtk::Image::builder()
                .icon_name(device.kind.icon())
                .icon_size(gtk::IconSize::Large)
                .build(),
        );
        who.add(&head);
        page.add(&who);
        let facts = group("");
        if let Some(vendor) = &device.vendor {
            facts.add(&fact("製造商", vendor));
        }
        facts.add(&fact("位置", &model::location(device)));
        for (name, value) in model::facts(device) {
            facts.add(&fact(name, &value));
        }
        page.add(&facts);
        let state = group("裝置狀態");
        let row = adw::ActionRow::builder()
            .title(device.state.name())
            .use_markup(false)
            .build();
        let icon = match device.state {
            State::Working => gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .css_classes(["success"])
                .build(),
            State::Disabled => gtk::Image::builder()
                .icon_name("action-unavailable-symbolic")
                .css_classes(["dim-label"])
                .build(),
            State::NoDriver => gtk::Image::builder()
                .icon_name("dialog-warning-symbolic")
                .css_classes(["warning"])
                .build(),
        };
        row.add_prefix(&icon);
        state.add(&row);
        page.add(&state);
        page
    }
}

/// Windows' tabs, as Files chooses its views: libadwaita's toggle group, which came with 1.7,
/// made from its name as settings/design.rs makes it, its `active` the index of the tab.
fn segmented(labels: &[&str]) -> gtk::Widget {
    let toggles: String = labels
        .iter()
        .map(|label| {
            format!(
                "<child><object class=\"AdwToggle\"><property name=\"label\">{}</property></object></child>",
                glib::markup_escape_text(label)
            )
        })
        .collect();
    gtk::Builder::from_string(&format!(
        "<interface><object class=\"AdwToggleGroup\" id=\"group\">{toggles}</object></interface>"
    ))
    .object("group")
    .expect("the toggle group just described")
}

fn driver_page(device: &Device) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let facts = group("");
    page.add(&facts);
    let Some(driver) = device.driver.clone() else {
        facts.add(&fact("驅動程式", State::NoDriver.name()));
        return page;
    };
    facts.add(&fact("驅動程式", &driver));
    // A USB device has a driver for each of its interfaces; the one of its own is the kernel's.
    if device.usb_device {
        return page;
    }
    let (node, facts) = (device.node.clone(), facts.clone());
    glib::spawn_future_local(async move {
        let Ok(info) = gio::spawn_blocking(move || model::driver_info(&node, &driver)).await else {
            return;
        };
        facts.add(&fact("模組", info.module.as_deref().unwrap_or("內建於核心")));
        for (name, value) in &info.fields {
            facts.add(&fact(name, value));
        }
    });
    page
}

fn details_page(device: &Device) -> adw::PreferencesPage {
    let details = model::details(device);
    let page = adw::PreferencesPage::new();
    let choose = group("");
    let names: Vec<&str> = details.iter().map(|(name, _)| name.as_str()).collect();
    let property = adw::ComboRow::builder()
        .title("屬性")
        .model(&gtk::StringList::new(&names))
        .enable_search(true)
        .build();
    property.set_expression(Some(gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    )));
    choose.add(&property);
    page.add(&choose);
    let values = group("值");
    page.add(&values);
    let shown: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let show = move |at: usize| {
        for row in shown.borrow_mut().drain(..) {
            values.remove(&row);
        }
        let Some((_, list)) = details.get(at) else { return };
        for value in list {
            let row = adw::ActionRow::builder()
                .title(value)
                .use_markup(false)
                .title_selectable(true)
                .build();
            row.add_css_class("monospace");
            let copy = gtk::Button::builder()
                .icon_name("edit-copy-symbolic")
                .tooltip_text("複製")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            let text = value.clone();
            copy.connect_clicked(move |button| button.clipboard().set_text(&text));
            row.add_suffix(&copy);
            values.add(&row);
            shown.borrow_mut().push(row);
        }
    };
    show(0);
    property.connect_selected_notify(move |row| show(row.selected() as usize));
    page
}

fn resources_page(resources: &[(&'static str, String)]) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let list = group("");
    for (kind, value) in resources {
        let row = fact(kind, value);
        row.add_css_class("numeric");
        list.add(&row);
    }
    page.add(&list);
    page
}

fn power_page(device: &Device, power: model::Power) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let list = group("");
    let switch = |title: &str, on: bool, set: fn(&Device, bool) -> Result<(), String>| {
        let row = adw::SwitchRow::builder().title(title).active(on).build();
        // Set back when it could not be changed, which turns it again.
        let reverting = Rc::new(Cell::new(false));
        let device = device.clone();
        let failure = format!("無法變更「{title}」");
        row.connect_active_notify(move |row| {
            if reverting.replace(false) {
                return;
            }
            let (row, device, failure, reverting) =
                (row.clone(), device.clone(), failure.clone(), reverting.clone());
            let on = row.is_active();
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || set(&device, on))
                    .await
                    .unwrap_or_else(|_| Err("已停止".into()));
                if let Err(why) = result {
                    reverting.set(true);
                    row.set_active(!on);
                    alert::notice(&row, &failure, Some(&why));
                }
            });
        });
        list.add(&row);
    };
    if let Some(on) = power.suspend {
        switch("允許電腦關閉這個裝置以節省電源", on, ops::set_suspend);
    }
    if let Some(on) = power.wakeup {
        switch("允許這個裝置喚醒電腦", on, ops::set_wakeup);
    }
    page.add(&list);
    page
}

fn events_page(device: &Device) -> gtk::Stack {
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(120)
        .build();
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    stack.add_named(&spinner, Some("reading"));
    let device = device.clone();
    let shown = stack.clone();
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || model::events(&device))
            .await
            .unwrap_or_else(|_| Err("已停止".into()));
        let page: gtk::Widget = match result {
            Ok(events) if events.is_empty() => adw::StatusPage::builder().title("沒有事件").build().upcast(),
            Ok(events) => {
                let page = adw::PreferencesPage::new();
                let list = group("");
                for (time, message) in events {
                    let row = adw::ActionRow::builder()
                        .title(&message)
                        .subtitle(&time)
                        .use_markup(false)
                        .title_selectable(true)
                        .build();
                    list.add(&row);
                }
                page.add(&list);
                page.upcast()
            }
            Err(why) => adw::StatusPage::builder()
                .title("無法讀取核心紀錄")
                .description(glib::markup_escape_text(&why).as_str())
                .build()
                .upcast(),
        };
        shown.add_named(&page, Some("events"));
        shown.set_visible_child_name("events");
    });
    stack
}

fn css() -> &'static str {
    "
.dm-group { font-size: 12px; font-weight: 600; opacity: .7; padding: 12px 12px 4px; }
.dm-chip { font-size: 12px; font-weight: 600; padding: 2px 9px; border-radius: 99px; }
.dm-chip.neutral { background: alpha(currentColor, .1); color: inherit; opacity: .8; }
.dm-chip.warn { background: alpha(@warning_color, .16); color: @warning_color; }
.dm-list { background: none; }
.dm-row { padding: 12px 14px; }
.dm-head { font-size: 12px; font-weight: 600; opacity: .7; }
.dm-row.dm-head { padding-top: 10px; padding-bottom: 10px; }
.dm-name { font-weight: 500; }
.dm-actions { padding: 12px 14px; min-height: 40px; }
.dm-who { font-size: 15px; font-weight: 600; }
.dm-dev { font-size: 12.5px; opacity: .65; font-family: monospace; }
.dm-expander { min-width: 22px; min-height: 22px; padding: 0; margin: -4px 0; }
"
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css());
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    let window: Rc<RefCell<Option<Rc<Window>>>> = Rc::default();
    app.connect_activate(move |app| match app.active_window() {
        Some(window) => window.present(),
        None => {
            window.replace(Some(Window::open(app)));
        }
    });
    app.run()
}
