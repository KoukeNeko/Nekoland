//! The Keyboard page, with its layouts and input methods.

use super::*;

pub(crate) const XKB_LIST: &str = "/usr/share/X11/xkb/rules/base.lst";

/// The keyboard layouts XKB knows, each followed by its variants, as
/// `(layout, variant, description)`.
pub(crate) fn xkb_layouts(list: &str) -> Vec<(String, String, String)> {
    let (mut layouts, mut variants) = (Vec::new(), Vec::new());
    let mut section = "";
    for line in list.lines() {
        if let Some(name) = line.strip_prefix("! ") {
            section = name.trim();
            continue;
        }
        let Some((name, description)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let description = description.trim();
        match section {
            "layout" => layouts.push((name, description)),
            // A variant says its layout first: "dvorak          us: English (Dvorak)".
            "variant" => {
                if let Some((layout, description)) = description.split_once(": ") {
                    variants.push((layout, name, description));
                }
            }
            _ => {}
        }
    }
    let mut all = Vec::new();
    for (layout, description) in layouts {
        all.push((layout.into(), String::new(), description.into()));
        for (_, variant, description) in variants.iter().filter(|(of, ..)| *of == layout) {
            all.push((layout.into(), variant.to_string(), description.to_string()));
        }
    }
    all
}

/// What the Caps Lock key can do, as XKB options.
pub(crate) const CAPS_OPTIONS: [&str; 4] = ["", "ctrl:nocaps", "caps:escape", "ctrl:swapcaps"];

pub(crate) fn keyboard_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let layout = group("");
    let layouts = match std::fs::read_to_string(XKB_LIST) {
        Ok(list) => xkb_layouts(&list),
        Err(err) => {
            eprintln!("{XKB_LIST}: {err}");
            Vec::new()
        }
    };
    let descriptions: Vec<&str> = layouts.iter().map(|(.., text)| text.as_str()).collect();
    let current = (config.word("keyboard_layout"), config.word("keyboard_variant"));
    let row = adw::ComboRow::builder()
        .title("鍵盤排列")
        .model(&gtk::StringList::new(&descriptions))
        .enable_search(true)
        .build();
    let chosen = layouts.iter().position(|(name, variant, _)| (&**name, &**variant) == current);
    if let Some(at) = chosen {
        row.set_selected(at as u32);
    }
    row.connect_selected_notify(move |row| {
        if let Some((name, variant, _)) = layouts.get(row.selected() as usize) {
            // One setting at a time, each a layout and variant XKB has: the variant goes
            // first so that it is never one the old layout lacks.
            set("keyboard_variant = \"\"");
            set(&format!("keyboard_layout = {name:?}"));
            set(&format!("keyboard_variant = {variant:?}"));
        }
    });
    layout.add(&row);
    layout.add(&choice_row(
        config,
        "Caps Lock 鍵",
        "keyboard_options",
        &CAPS_OPTIONS,
        &["Caps Lock", "Control", "Esc", "與 Control 對調"],
    ));
    page.add(&layout);
    page.add(&input_method_group());

    let repeat = group("");
    let repeats = switch_row(config, "按鍵重複", "key_repeat");
    repeat.add(&repeats);
    let delay = scale_row(
        "重複延遲",
        config.number("repeat_delay", 200.0),
        (150.0, 1000.0, 50.0),
        |delay| format!("{delay:.0} 毫秒"),
        |delay| set(&format!("repeat_delay = {}", delay.round() as i32)),
    );
    let rate = scale_row(
        "重複速度",
        config.number("repeat_rate", 25.0),
        (10.0, 60.0, 5.0),
        |rate| format!("{rate:.0} 次/秒"),
        |rate| set(&format!("repeat_rate = {}", rate.round() as i32)),
    );
    for row in [&delay, &rate] {
        repeats.bind_property("active", row, "visible").sync_create().build();
        repeat.add(row);
    }
    page.add(&repeat);
    let typing = group("");
    typing.add(&switch_row(config, "黏著鍵", "sticky_keys"));
    let blink = gsettings_switch("文字游標閃爍", INTERFACE, "cursor-blink");
    typing.add(&blink);
    // GNOME's time is one whole blink, on and off.
    let blink_time = scale_row(
        "閃爍間隔",
        interface_setting("cursor-blink-time").parse().unwrap_or(1200.0),
        (100.0, 2500.0, 100.0),
        |time| format!("{time:.0} 毫秒"),
        |time| set_interface("cursor-blink-time", &(time.round() as i32).to_string()),
    );
    blink.bind_property("active", &blink_time, "visible").sync_create().build();
    typing.add(&blink_time);
    let (shortcuts, scheme) = keys::shortcuts_page(config);
    typing.add(&subpage_row("鍵盤快速鍵", scheme, shortcuts));
    page.add(&typing);
    page
}

pub(crate) const FCITX: &str = "org.fcitx.Fcitx5";
pub(crate) const FCITX_PATH: &str = "/controller";
pub(crate) const FCITX_CONTROLLER: &str = "org.fcitx.Fcitx.Controller1";

/// Calls Fcitx's controller, the one fcitx5-config-qt calls; None when Fcitx is not
/// running or refuses.
pub(crate) fn fcitx(
    bus: &gio::DBusConnection,
    method: &str,
    args: Option<&glib::Variant>,
    reply: &str,
) -> Option<glib::Variant> {
    bus.call_sync(
        Some(FCITX),
        FCITX_PATH,
        FCITX_CONTROLLER,
        method,
        args,
        glib::VariantTy::new(reply).ok(),
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    )
    .inspect_err(|err| eprintln!("fcitx5 {method}: {err}"))
    .ok()
}

/// Fcitx's group of input methods in use: its layout, and each method's ID, name and
/// layout, in its order. The first is the one Fcitx turns to when it is off.
#[derive(Clone)]
pub(crate) struct InputMethods {
    pub(crate) group: String,
    pub(crate) layout: String,
    pub(crate) methods: Vec<(String, String, String)>,
}

/// An input method's name in its own language when Fcitx has one, as 酷音 for Chewing.
pub(crate) fn input_method_name(native: &str, name: &str) -> String {
    if native.is_empty() { name } else { native }.to_string()
}

pub(crate) fn input_methods(bus: &gio::DBusConnection) -> Option<InputMethods> {
    let group = fcitx(bus, "CurrentInputMethodGroup", None, "(s)")?
        .child_value(0)
        .str()?
        .to_string();
    let info = fcitx(
        bus,
        "FullInputMethodGroupInfo",
        Some(&(group.as_str(),).to_variant()),
        "(sssa{sv}a(sssssssbsa{sv}))",
    )?;
    let layout = info.child_value(2).str()?.to_string();
    let methods = info
        .child_value(4)
        .iter()
        .map(|method| {
            let text = |at| method.child_value(at).str().unwrap_or_default().to_string();
            (text(0), input_method_name(&text(2), &text(1)), text(8))
        })
        .collect();
    Some(InputMethods {
        group,
        layout,
        methods,
    })
}

/// Fcitx's input methods in its group in use, to add, remove and put in order.
pub(crate) fn input_method_group() -> adw::PreferencesGroup {
    let methods = group("輸入法");
    let add = adw::ActionRow::builder()
        .title("新增輸入法")
        .activatable(true)
        .build();
    add.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .inspect_err(|err| eprintln!("session bus: {err}"))
        .ok();
    methods.add(&add);
    let Some(bus) = bus else {
        add.set_visible(false);
        return methods;
    };
    list_input_methods(&methods, &add, &rows, &bus, input_methods(&bus));
    add.connect_activated(glib::clone!(
        #[weak]
        methods,
        #[strong]
        rows,
        #[strong]
        bus,
        move |add| {
            let Some(current) = input_methods(&bus) else {
                list_input_methods(&methods, add, &rows, &bus, None);
                return;
            };
            input_method_picker(
                add,
                &bus,
                current,
                glib::clone!(
                    #[weak]
                    methods,
                    #[weak]
                    add,
                    #[strong]
                    rows,
                    #[strong]
                    bus,
                    move |id: String, name: String| {
                        let Some(mut changed) = input_methods(&bus) else {
                            list_input_methods(&methods, &add, &rows, &bus, None);
                            return;
                        };
                        if !changed.methods.iter().any(|(added, ..)| *added == id) {
                            changed.methods.push((id, name, String::new()));
                        }
                        save_input_methods(&methods, &add, &rows, &bus, changed);
                    }
                ),
            );
        }
    ));
    methods
}

/// Gives Fcitx the group as changed, which it saves, and lists what it then has.
pub(crate) fn save_input_methods(
    group: &adw::PreferencesGroup,
    add: &adw::ActionRow,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    bus: &gio::DBusConnection,
    changed: InputMethods,
) {
    let entries: Vec<(String, String)> = changed
        .methods
        .into_iter()
        .map(|(id, _, layout)| (id, layout))
        .collect();
    fcitx(
        bus,
        "SetInputMethodGroupInfo",
        Some(&(changed.group.as_str(), changed.layout.as_str(), entries).to_variant()),
        "()",
    );
    list_input_methods(group, add, rows, bus, input_methods(bus));
}

/// Lists the input methods above the row that adds one, again after each change.
pub(crate) fn list_input_methods(
    group: &adw::PreferencesGroup,
    add: &adw::ActionRow,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    bus: &gio::DBusConnection,
    current: Option<InputMethods>,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    group.remove(add);
    let Some(current) = current else {
        let row = adw::ActionRow::builder()
            .title("Fcitx 5 未執行")
            .css_classes(["dim-label"])
            .build();
        group.add(&row);
        rows.borrow_mut().push(row);
        return;
    };
    let count = current.methods.len();
    for (index, (id, name, _)) in current.methods.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(name))
            .build();
        let button = |icon: &str, tooltip: &str, sensitive: bool| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .sensitive(sensitive)
                .build()
        };
        let up = button("go-up-symbolic", "上移", index > 0);
        let remove = button("user-trash-symbolic", "移除", count > 1);
        for (button, moved) in [(&up, true), (&remove, false)] {
            let id = id.clone();
            button.connect_clicked(glib::clone!(
                #[weak]
                group,
                #[weak]
                add,
                #[strong]
                rows,
                #[strong]
                bus,
                move |_| {
                    // Fcitx's group as it is now, as it may have changed since it was listed.
                    let Some(mut changed) = input_methods(&bus) else {
                        list_input_methods(&group, &add, &rows, &bus, None);
                        return;
                    };
                    match changed.methods.iter().position(|(listed, ..)| *listed == id) {
                        Some(at) if moved && at > 0 => changed.methods.swap(at - 1, at),
                        Some(at) if !moved && changed.methods.len() > 1 => {
                            changed.methods.remove(at);
                        }
                        _ => {}
                    }
                    save_input_methods(&group, &add, &rows, &bus, changed);
                }
            ));
        }
        row.add_suffix(&up);
        row.add_suffix(&remove);
        row.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let dragged = id.clone();
        let grabbed = Rc::new(Cell::new((0, 0)));
        drag.connect_prepare(glib::clone!(
            #[strong]
            grabbed,
            move |_, x, y| {
                grabbed.set((x as i32, y as i32));
                Some(gdk::ContentProvider::for_value(&dragged.to_value()))
            }
        ));
        // The row is drawn anew on a card of its own to follow the pointer, as GNOME's
        // Settings does: a picture of the row itself would be clear where the list is.
        let name = name.clone();
        drag.connect_drag_begin(glib::clone!(
            #[weak]
            row,
            move |_, drag| {
                let copy = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&name))
                    .build();
                copy.add_prefix(&gtk::Image::from_icon_name("list-drag-handle-symbolic"));
                let card = gtk::ListBox::builder().css_classes(["boxed-list"]).build();
                card.set_size_request(row.width(), row.height());
                card.append(&copy);
                gtk::DragIcon::for_drag(drag).set_child(Some(&card));
                let (x, y) = grabbed.get();
                drag.set_hotspot(x, y);
            }
        ));
        row.add_controller(drag);
        // A row dropped on another takes its place.
        let drop = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        let target = id.clone();
        drop.connect_drop(glib::clone!(
            #[weak]
            group,
            #[weak]
            add,
            #[strong]
            rows,
            #[strong]
            bus,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Ok(dragged) = value.get::<String>() else {
                    return false;
                };
                let Some(mut changed) = input_methods(&bus) else {
                    return false;
                };
                let at = |id: &str| changed.methods.iter().position(|(listed, ..)| listed == id);
                let (Some(from), Some(to)) = (at(&dragged), at(&target)) else {
                    return false;
                };
                let moved = changed.methods.remove(from);
                changed.methods.insert(to, moved);
                // The rows are made again once the drop is over, not while it runs on one.
                let (rows, bus) = (rows.clone(), bus.clone());
                glib::idle_add_local_once(move || save_input_methods(&group, &add, &rows, &bus, changed));
                true
            }
        ));
        row.add_controller(drop);
        group.add(&row);
        rows.borrow_mut().push(row);
    }
    group.add(add);
}

/// Fcitx's input methods not in the group, those of a language before its keyboard
/// layouts; clicking one or Enter adds it.
pub(crate) fn input_method_picker(
    parent: &adw::ActionRow,
    bus: &gio::DBusConnection,
    current: InputMethods,
    picked: impl Fn(String, String) + 'static,
) {
    let mut available: Vec<(String, String)> = fcitx(bus, "AvailableInputMethods", None, "(a(ssssssb))")
        .map(|reply| {
            reply
                .child_value(0)
                .iter()
                .map(|method| {
                    let text = |at| method.child_value(at).str().unwrap_or_default().to_string();
                    (text(0), input_method_name(&text(2), &text(1)))
                })
                .filter(|(id, _)| !current.methods.iter().any(|(added, ..)| added == id))
                .collect()
        })
        .unwrap_or_default();
    available.sort_by_cached_key(|(id, name)| (id.starts_with("keyboard-"), name.to_lowercase()));
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    for (_, name) in &available {
        list.append(
            &gtk::Label::builder()
                .label(name.as_str())
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .margin_top(4)
                .margin_bottom(4)
                .build(),
        );
    }
    list.set_placeholder(Some(
        &gtk::Label::builder()
            .label("沒有符合的輸入法")
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build(),
    ));
    let available = Rc::new(available);
    let search = gtk::SearchEntry::builder().placeholder_text("搜尋").build();
    list.set_filter_func(glib::clone!(
        #[strong]
        available,
        #[weak]
        search,
        #[upgrade_or]
        true,
        move |row| {
            let wanted = search.text().to_lowercase();
            let (id, name) = &available[row.index() as usize];
            name.to_lowercase().contains(&wanted) || id.contains(&wanted)
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[weak]
        list,
        move |_| list.invalidate_filter()
    ));
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(24)
        .build();
    body.append(&search);
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .css_classes(["card"])
            .overflow(gtk::Overflow::Hidden)
            .child(&list)
            .build(),
    );
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("新增輸入法")
        .content_width(440)
        .content_height(560)
        .child(&view)
        .build();
    list.connect_row_activated(glib::clone!(
        #[weak]
        dialog,
        move |_, row| {
            if let Some((id, name)) = available.get(row.index() as usize) {
                picked(id.clone(), name.clone());
            }
            dialog.close();
        }
    ));
    search.set_key_capture_widget(Some(&dialog));
    dialog.present(Some(parent));
}
