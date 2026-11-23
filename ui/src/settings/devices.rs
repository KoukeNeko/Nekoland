//! 裝置管理員: the devices of this computer as Windows' Device Manager shows them, by kind or
//! by what each is connected to, marked when one has no driver or is disabled. A device's page
//! tells of it, its driver, the details udev keeps, the interrupts and ranges it uses, its
//! power settings and what the kernel said of it. A device can be disabled, which lasts across
//! restarts, enabled again, and uninstalled until hardware is scanned for again, each as root
//! after asking for the administrator's password, as 磁碟 does.

use super::*;
use std::time::Duration;

#[path = "devices/model.rs"]
mod model;
#[path = "devices/ops.rs"]
mod ops;

use model::{Device, Kind, State};

/// How often the devices are read again while the page is shown, to show one plugged in or
/// taken out.
const REFRESH: Duration = Duration::from_secs(3);
/// How far a device stands in from what it is connected to, and how wide the button that
/// opens it is.
const INDENT: i32 = 20;
const OPENER_WIDTH: i32 = 34;
/// The distances libadwaita's preferences pages keep, which the cards, the tiles, the bars
/// and the tabs here keep too: between groups, under a group's heading, and inside a row.
const GROUP_GAP: i32 = 24;
const HEADING_GAP: i32 = 12;
const CARD_PADDING: i32 = 12;
const TILE_GAP: i32 = 12;
/// How many of the other devices a disabling takes along are named.
const NAMED: usize = 3;

struct Shown {
    devices: RefCell<Vec<Device>>,
    by_connection: Cell<bool>,
    hidden: Cell<bool>,
    /// The devices opened in the tree, by path, kept as the devices are read again.
    open: RefCell<HashSet<String>>,
    /// What the system runs from, which is not disabled from under it.
    system: Vec<String>,
    busy: Cell<bool>,
    summary: adw::PreferencesGroup,
    attention: adw::PreferencesGroup,
    list: adw::PreferencesGroup,
    /// What `fill` put into the three groups above, taken out as it fills them again.
    filled: RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>,
}

pub(crate) fn devices_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let summary = group("");
    let attention = group("需要注意");
    attention.set_visible(false);
    let view = segmented(&["依類型", "依連線"], 0);
    view.set_valign(gtk::Align::Center);
    let scan = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("掃描硬體變更")
        .action_name("devices.scan")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let menu = gio::Menu::new();
    menu.append(Some("顯示隱藏的裝置"), Some("devices.show-hidden"));
    let more = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&menu)
        .tooltip_text("更多")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let gap = gtk::Box::builder().hexpand(true).build();
    let list = group("");
    // Over what it changes, as the first of the group's widgets: what `fill` adds goes under it.
    let bar = line(
        4,
        &[
            view.upcast_ref(),
            gap.upcast_ref(),
            scan.upcast_ref(),
            more.upcast_ref(),
        ],
    );
    bar.set_margin_bottom(HEADING_GAP);
    list.add(&bar);
    page.add(&summary);
    page.add(&attention);
    page.add(&list);
    let shown = Rc::new(Shown {
        devices: RefCell::default(),
        by_connection: Cell::new(false),
        hidden: Cell::new(false),
        open: RefCell::default(),
        system: model::system_paths(),
        busy: Cell::new(false),
        summary,
        attention,
        list,
        filled: RefCell::default(),
    });
    view.connect_notify_local(
        Some("active"),
        glib::clone!(
            #[strong]
            shown,
            move |view, _| {
                shown.by_connection.set(view.property::<u32>("active") == 1);
                fill(&shown);
            }
        ),
    );

    let actions = gio::SimpleActionGroup::new();
    let hidden = gio::SimpleAction::new_stateful("show-hidden", None, &false.to_variant());
    hidden.connect_change_state(glib::clone!(
        #[strong]
        shown,
        move |action, state| {
            let Some(on) = state.and_then(|state| state.get::<bool>()) else {
                return;
            };
            action.set_state(&on.to_variant());
            shown.hidden.set(on);
            fill(&shown);
        }
    ));
    actions.add_action(&hidden);
    let scan = gio::SimpleAction::new("scan", None);
    scan.connect_activate(glib::clone!(
        #[strong]
        shown,
        #[weak]
        page,
        move |_, _| run_root(&shown, page.upcast_ref(), "無法掃描硬體變更".into(), ops::scan)
    ));
    actions.add_action(&scan);
    page.insert_action_group("devices", Some(&actions));

    page.connect_map(glib::clone!(
        #[strong]
        shown,
        move |_| reload(&shown)
    ));
    let weak = page.downgrade();
    glib::timeout_add_local(REFRESH, move || {
        let Some(page) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if page.is_mapped() && !shown.busy.get() {
            reload(&shown);
        }
        glib::ControlFlow::Continue
    });
    page
}

fn reload(shown: &Rc<Shown>) {
    let shown = shown.clone();
    glib::spawn_future_local(async move {
        let devices = match gio::spawn_blocking(model::read).await {
            Ok(Ok(devices)) => devices,
            Ok(Err(why)) => return eprintln!("裝置管理員: {why}"),
            Err(_) => return eprintln!("裝置管理員: reading the devices stopped"),
        };
        if *shown.devices.borrow() == devices {
            return;
        }
        if shown.devices.borrow().is_empty() {
            // The tree starts with the computer open, as in Windows.
            let computer = devices.iter().find(|device| device.kind == Kind::Computer);
            shown
                .open
                .borrow_mut()
                .extend(computer.map(|device| device.path.clone()));
        }
        shown.devices.replace(devices);
        fill(&shown);
    });
}

/// The devices shown: those hidden only when asked for.
fn listed(shown: &Shown) -> Vec<Device> {
    shown
        .devices
        .borrow()
        .iter()
        .filter(|device| shown.hidden.get() || !device.hidden)
        .cloned()
        .collect()
}

fn fill(shown: &Rc<Shown>) {
    for (group, widget) in shown.filled.take() {
        group.remove(&widget);
    }
    let devices = listed(shown);
    let mut filled: Vec<(adw::PreferencesGroup, gtk::Widget)> = Vec::new();

    let computer = devices.iter().find(|device| device.kind == Kind::Computer);
    let troubled: Vec<&Device> = devices
        .iter()
        .filter(|device| device.state != State::Working)
        .collect();
    let state = if troubled.is_empty() {
        (State::Working, "全部運作正常".to_string())
    } else {
        (State::NoDriver, format!("{} 個裝置需要注意", troubled.len()))
    };
    let about = computer
        .map(|computer| format!("{} · {} 個裝置", computer.name, devices.len()))
        .unwrap_or_else(|| format!("{} 個裝置", devices.len()));
    let card = card(
        "computer-symbolic",
        &glib::host_name(),
        &about,
        (state.0, &state.1),
        None,
    );
    filled.push((shown.summary.clone(), card.upcast()));

    for device in &troubled {
        let row = device_row(shown, device, None);
        row.set_subtitle(&format!("{} · {}", device.kind.name(), device.state.name()));
        filled.push((shown.attention.clone(), row.upcast()));
    }
    shown.attention.set_visible(!troubled.is_empty());

    if devices.is_empty() {
        let none = adw::StatusPage::builder()
            .icon_name("computer-symbolic")
            .title("沒有裝置")
            .build();
        filled.push((shown.list.clone(), none.upcast()));
    } else if shown.by_connection.get() {
        // A list of its own, as the group's own would be put over the bar.
        let tree = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        for row in connection_rows(shown, &devices) {
            tree.append(&row);
        }
        filled.push((shown.list.clone(), tree.upcast()));
    } else {
        filled.push((shown.list.clone(), kind_tiles(shown, &devices).upcast()));
    }
    for (group, widget) in &filled {
        group.add(widget);
    }
    shown.filled.replace(filled);
}

/// A card telling what is shown, under its icon: the computer, or a device, with how it is
/// and, for a device, what can be done to it.
fn card(
    icon: &str,
    title: &str,
    about: &str,
    state: (State, &str),
    action: Option<&gtk::Widget>,
) -> gtk::Box {
    let card = gtk::Box::builder().spacing(12).css_classes(["card"]).build();
    let image = gtk::Image::builder()
        .icon_name(icon)
        .pixel_size(48)
        .margin_start(CARD_PADDING)
        .margin_top(CARD_PADDING)
        .margin_bottom(CARD_PADDING)
        .valign(gtk::Align::Center)
        .build();
    card.append(&image);
    let words = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .margin_top(CARD_PADDING)
        .margin_bottom(CARD_PADDING)
        .build();
    let label = |text: &str, classes: &[&str]| {
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(classes.to_vec())
            .build()
    };
    let name = label(title, &["title-3"]);
    name.set_tooltip_text(Some(title));
    words.append(&name);
    words.append(&label(about, &["dim-label"]));
    let now = gtk::Box::builder().spacing(6).margin_top(4).build();
    let mark = state_icon(state.0);
    mark.set_pixel_size(14);
    now.append(&mark);
    now.append(&label(state.1, &[]));
    words.append(&now);
    card.append(&words);
    let end = action.map_or(CARD_PADDING, |_| 0);
    words.set_margin_end(end);
    if let Some(action) = action {
        action.set_valign(gtk::Align::Center);
        action.set_margin_end(CARD_PADDING);
        card.append(action);
    }
    card
}

/// The kinds as tiles, three to a line, each opening the devices of its kind.
fn kind_tiles(shown: &Rc<Shown>, devices: &[Device]) -> gtk::Grid {
    // A grid rather than a flow box, whose children keep padding of their own and would stand
    // the tiles in from the cards over them.
    let tiles = gtk::Grid::builder()
        .column_homogeneous(true)
        .row_homogeneous(true)
        .column_spacing(TILE_GAP)
        .row_spacing(TILE_GAP)
        .build();
    let mut at = 0;
    for kind in model::KINDS {
        let of_kind = devices.iter().filter(|device| device.kind == kind).count();
        if of_kind == 0 {
            continue;
        }
        let inside = gtk::Box::builder()
            .spacing(12)
            .margin_top(CARD_PADDING)
            .margin_bottom(CARD_PADDING)
            .margin_start(CARD_PADDING)
            .margin_end(CARD_PADDING)
            .build();
        inside.append(&gtk::Image::from_icon_name(kind.icon()));
        let name = gtk::Label::builder()
            .label(kind.name())
            .xalign(0.0)
            .hexpand(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .tooltip_text(kind.name())
            .build();
        inside.append(&name);
        if devices
            .iter()
            .any(|device| device.kind == kind && device.state == State::NoDriver)
        {
            inside.append(&state_icon(State::NoDriver));
        }
        inside.append(
            &gtk::Label::builder()
                .label(of_kind.to_string())
                .css_classes(["numeric", "dim-label"])
                .build(),
        );
        let tile = gtk::Button::builder()
            .child(&inside)
            .css_classes(["card"])
            .build();
        let shown = shown.clone();
        tile.connect_clicked(move |tile| open_kind(&shown, tile, kind));
        tiles.attach(&tile, at % 3, at / 3, 1, 1);
        at += 1;
    }
    tiles
}

/// The devices of a kind one level down, those alike, as the processor's threads are, as one.
fn open_kind(shown: &Rc<Shown>, from: &gtk::Button, kind: Kind) {
    let devices: Vec<Device> = listed(shown)
        .into_iter()
        .filter(|device| device.kind == kind)
        .collect();
    let mut alike: Vec<Vec<&Device>> = Vec::new();
    for device in &devices {
        let same = |first: &&Device| {
            first.name == device.name && first.driver == device.driver && first.state == device.state
        };
        match alike.iter_mut().find(|them| same(&them[0])) {
            Some(them) => them.push(device),
            None => alike.push(vec![device]),
        }
    }
    let list = group("");
    for them in alike {
        if let [device] = them[..] {
            list.add(&device_row(shown, device, None));
            continue;
        }
        let first = them[0];
        let row = adw::ActionRow::builder()
            .title(&first.name)
            .subtitle(summary(first))
            .use_markup(false)
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(kind.icon()));
        if first.state != State::Working {
            row.add_suffix(&state_icon(first.state));
        }
        row.add_suffix(
            &gtk::Label::builder()
                .label(format!("× {}", them.len()))
                .css_classes(["numeric", "dim-label"])
                .build(),
        );
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let alike: Vec<Device> = them.iter().map(|device| (*device).clone()).collect();
        let shown = shown.clone();
        row.connect_activated(move |row| open_alike(&shown, row, &alike));
        list.add(&row);
    }
    vpn::push(from, kind.name(), None, &page_of_groups(&[list]));
}

/// Devices alike one level down, each told apart by where it is.
fn open_alike(shown: &Rc<Shown>, from: &adw::ActionRow, alike: &[Device]) {
    let list = group("");
    for device in alike {
        let one = adw::ActionRow::builder()
            .title(model::location(device))
            .use_markup(false)
            .activatable(true)
            .build();
        one.add_prefix(&gtk::Image::from_icon_name(device.kind.icon()));
        one.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (shown, device) = (shown.clone(), device.clone());
        one.connect_activated(move |one| open_device(&shown, one, &device));
        list.add(&one);
    }
    vpn::push(from, &alike[0].name, None, &page_of_groups(&[list]));
}

/// The tree of what each device is connected to, from the computer down, each row stood in
/// by its depth: libadwaita's expander rows, nested, neither stand in nor show which is open.
fn connection_rows(shown: &Rc<Shown>, devices: &[Device]) -> Vec<gtk::Widget> {
    let listed: Vec<&Device> = devices.iter().collect();
    let rows = model::tree(&listed, &shown.open.borrow());
    rows.into_iter()
        .map(|(index, depth, under)| {
            let device = listed[index];
            let opener: gtk::Widget = if under {
                let open = shown.open.borrow().contains(&device.path);
                let button = gtk::Button::builder()
                    .icon_name(if open {
                        "pan-down-symbolic"
                    } else {
                        "pan-end-symbolic"
                    })
                    .tooltip_text(if open { "收合" } else { "展開" })
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "circular"])
                    .build();
                let (shown, path) = (shown.clone(), device.path.clone());
                button.connect_clicked(move |_| {
                    {
                        let mut open = shown.open.borrow_mut();
                        if !open.remove(&path) {
                            open.insert(path.clone());
                        }
                    }
                    fill(&shown);
                });
                button.upcast()
            } else {
                gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast()
            };
            opener.set_width_request(OPENER_WIDTH);
            opener.set_margin_start(depth as i32 * INDENT);
            device_row(shown, device, Some(&opener)).upcast()
        })
        .collect()
}

/// A device's row, opening its page; `lead` stands before its icon.
fn device_row(shown: &Rc<Shown>, device: &Device, lead: Option<&gtk::Widget>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&device.name)
        .subtitle(summary(device))
        .use_markup(false)
        .activatable(true)
        .build();
    // Each prefix goes before those already there.
    row.add_prefix(&gtk::Image::from_icon_name(device.kind.icon()));
    if let Some(lead) = lead {
        row.add_prefix(lead);
    }
    if device.state != State::Working {
        row.add_suffix(&state_icon(device.state));
    }
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let (shown, device) = (shown.clone(), device.clone());
    row.connect_activated(move |row| open_device(&shown, row, &device));
    row
}

/// A device's driver, or what is wrong with it.
fn summary(device: &Device) -> String {
    match device.state {
        State::Working => device.driver.clone().unwrap_or_default(),
        state => state.name().to_string(),
    }
}

fn state_icon(state: State) -> gtk::Image {
    let (icon, class) = match state {
        State::Working => ("object-select-symbolic", "success"),
        State::Disabled => ("action-unavailable-symbolic", "dim-label"),
        State::NoDriver => ("dialog-warning-symbolic", "warning"),
    };
    gtk::Image::builder()
        .icon_name(icon)
        .css_classes([class])
        .tooltip_text(state.name())
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

// --- what is done ---------------------------------------------------------------------------

/// Runs `job` as root in a thread, then reads the devices again; done, the device's page,
/// which told of it as it was, is left.
fn run_root(
    shown: &Rc<Shown>,
    from: &gtk::Widget,
    failure: String,
    job: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    if shown.busy.replace(true) {
        return;
    }
    from.set_sensitive(false);
    let (shown, from) = (shown.clone(), from.clone());
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(job)
            .await
            .unwrap_or_else(|_| Err("已停止".into()));
        shown.busy.set(false);
        from.set_sensitive(true);
        match result {
            Ok(()) => {
                if let Some(navigation) = from
                    .ancestor(adw::NavigationView::static_type())
                    .and_downcast::<adw::NavigationView>()
                {
                    navigation.pop_to_tag(SHOWN_PAGE);
                }
            }
            Err(why) => alert::notice(&from, &failure, Some(&why)),
        }
        reload(&shown);
    });
}

fn refuse_system(shown: &Shown, from: &gtk::Widget, verb: &str, device: &Device) -> bool {
    let holds = model::holds_system(device, &shown.system);
    if holds {
        alert::notice(
            from,
            &format!("無法{verb}「{}」", device.name),
            Some("系統磁碟連接在這個裝置上。"),
        );
    }
    holds
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

fn disable(shown: &Rc<Shown>, from: &gtk::Widget, device: Device) {
    if refuse_system(shown, from, "停用", &device) {
        return;
    }
    let along = taken_along(&device, &shown.devices.borrow());
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
    let (shown, button) = (shown.clone(), from.clone());
    alert::confirm(
        from,
        &format!("停用「{}」？", device.name),
        body.as_deref(),
        "停用",
        move || {
            let name = device.name.clone();
            run_root(&shown, &button, format!("無法停用「{name}」"), move || {
                ops::disable(&device)
            });
        },
    );
}

fn enable(shown: &Rc<Shown>, from: &gtk::Widget, device: Device) {
    let name = device.name.clone();
    run_root(shown, from, format!("無法啟用「{name}」"), move || {
        ops::enable(&device)
    });
}

fn uninstall(shown: &Rc<Shown>, from: &gtk::Widget, device: Device) {
    if refuse_system(shown, from, "解除安裝", &device) {
        return;
    }
    let body = if device.bus == "usb" {
        "要再使用，請拔除後重新插上。"
    } else {
        "掃描硬體變更後會再找到它。"
    };
    let (shown, button) = (shown.clone(), from.clone());
    alert::confirm(
        from,
        &format!("解除安裝「{}」？", device.name),
        Some(body),
        "解除安裝",
        move || {
            let name = device.name.clone();
            run_root(
                &shown,
                &button,
                format!("無法解除安裝「{name}」"),
                move || ops::uninstall(&device),
            );
        },
    );
}

// --- a device's page ------------------------------------------------------------------------

/// Shows the device one level down: its card, with what can be done to it, over Windows' tabs
/// of its 內容.
fn open_device(shown: &Rc<Shown>, from: &impl IsA<gtk::Widget>, device: &Device) {
    let mut tabs: Vec<(&str, gtk::Widget)> = vec![
        ("一般", general_page(shown, device).upcast()),
        ("驅動程式", driver_page(shown, device).upcast()),
        ("詳細資料", details_page(device).upcast()),
    ];
    if !device.node.is_empty() {
        let resources = model::resources(&device.node);
        if !resources.is_empty() {
            tabs.push(("資源", resources_page(&resources).upcast()));
        }
        let power = model::power(&device.node);
        if power != model::Power::default() && device.kind.can_change() {
            tabs.push(("電源管理", power_page(device, power).upcast()));
        }
    }
    tabs.push(("事件", events_page(device).upcast()));
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(120)
        .vhomogeneous(false)
        .build();
    for (index, (_, tab)) in tabs.iter().enumerate() {
        stack.add_named(tab, Some(&index.to_string()));
    }
    let labels: Vec<&str> = tabs.iter().map(|(name, _)| *name).collect();
    let switcher = segmented(&labels, 0);
    let shown_tab = stack.clone();
    switcher.connect_notify_local(Some("active"), move |switcher, _| {
        shown_tab.set_visible_child_name(&switcher.property::<u32>("active").to_string())
    });

    let action = can_change(device).then(|| {
        let disabled = device.state == State::Disabled;
        let button = gtk::Button::builder()
            .label(if disabled { "啟用裝置" } else { "停用裝置" })
            .build();
        let (shown, device) = (shown.clone(), device.clone());
        button.connect_clicked(move |button| {
            if disabled {
                enable(&shown, button.upcast_ref(), device.clone())
            } else {
                disable(&shown, button.upcast_ref(), device.clone())
            }
        });
        button
    });
    let about = match &device.vendor {
        Some(vendor) => format!("{} · {vendor}", device.kind.name()),
        None => device.kind.name().to_string(),
    };
    let card = card(
        device.kind.icon(),
        &device.name,
        &about,
        (device.state, device.state.name()),
        action.as_ref().map(|button| button.upcast_ref()),
    );
    // Laid out as the page of devices is, its card and its bar where that page has them.
    let page = adw::PreferencesPage::new();
    let card_group = group("");
    card_group.add(&card);
    page.add(&card_group);
    let tabs = group("");
    switcher.set_valign(gtk::Align::Center);
    let bar = line(4, &[&switcher]);
    bar.set_height_request(bar_height());
    bar.set_margin_bottom(HEADING_GAP);
    tabs.add(&bar);
    tabs.add(&stack);
    page.add(&tabs);
    vpn::push(from, &device.name, None, &page);
}

/// Whether the device can be disabled and uninstalled: one on a bus, not the computer itself
/// nor what the firmware or the processor is.
fn can_change(device: &Device) -> bool {
    device.kind.can_change() && !device.node.is_empty()
}

fn general_page(shown: &Rc<Shown>, device: &Device) -> gtk::Box {
    let page = tab_page();
    let facts = group("");
    facts.add(&fact("位置", &model::location(device)));
    facts.add(&fact(
        "驅動程式",
        device.driver.as_deref().unwrap_or(State::NoDriver.name()),
    ));
    for (name, value) in model::facts(device) {
        facts.add(&fact(name, &value));
    }
    if let Some(above) = connected_to(shown, device) {
        let row = adw::ActionRow::builder()
            .title("連接到")
            .activatable(true)
            .build();
        let name = gtk::Label::builder()
            .label(&above.name)
            .css_classes(["dim-label"])
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .tooltip_text(&above.name)
            .build();
        row.add_suffix(&name);
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let shown = shown.clone();
        row.connect_activated(move |row| open_device(&shown, row, &above));
        facts.add(&row);
    }
    page.append(&facts);
    page
}

/// The device this one is connected to, as the tree has it.
fn connected_to(shown: &Shown, device: &Device) -> Option<Device> {
    let devices = shown.devices.borrow();
    let all: Vec<&Device> = devices.iter().collect();
    let at = all.iter().position(|other| other.path == device.path)?;
    model::parents(&all)[at].map(|above| all[above].clone())
}

fn driver_page(shown: &Rc<Shown>, device: &Device) -> gtk::Box {
    let page = tab_page();
    let facts = group("");
    page.append(&facts);
    // Last on the page, as Windows has it.
    if can_change(device) && ops::can_uninstall(device) {
        let remove = group("");
        let row = button_row("解除安裝裝置");
        let (shown, device) = (shown.clone(), device.clone());
        row.connect_closure(
            "activated",
            false,
            glib::closure_local!(move |row: gtk::Widget| uninstall(&shown, &row, device.clone())),
        );
        remove.add(&row);
        page.append(&remove);
    }
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

/// libadwaita's row that is a destructive button, which came with 1.6, made from its name as
/// `segmented` makes the toggle group.
fn button_row(title: &str) -> gtk::Widget {
    gtk::Builder::from_string(&format!(
        "<interface><object class=\"AdwButtonRow\" id=\"row\"><property name=\"title\">{}</property>\
         <style><class name=\"destructive-action\"/></style></object></interface>",
        glib::markup_escape_text(title)
    ))
    .object("row")
    .expect("the button row just described")
}

/// A tab's page: its groups, a group's distance apart.
fn tab_page() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(GROUP_GAP)
        .build()
}

/// How high the bar over the devices is: as its buttons are, in the look shown.
fn bar_height() -> i32 {
    let button = gtk::Button::builder()
        .icon_name("view-more-symbolic")
        .css_classes(["flat"])
        .build();
    button.measure(gtk::Orientation::Vertical, -1).1
}

fn details_page(device: &Device) -> gtk::Box {
    let details = model::details(device);
    let page = tab_page();
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
    page.append(&choose);
    let values = group("值");
    page.append(&values);
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

fn resources_page(resources: &[(&'static str, String)]) -> gtk::Box {
    let page = tab_page();
    let list = group("");
    for (kind, value) in resources {
        let row = fact(kind, value);
        row.add_css_class("numeric");
        list.add(&row);
    }
    page.append(&list);
    page
}

fn power_page(device: &Device, power: model::Power) -> gtk::Box {
    let page = tab_page();
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
    page.append(&list);
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
                let page = tab_page();
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
                page.append(&list);
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
