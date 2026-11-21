//! The Windows page and the hover preview it opens.

use super::*;

pub(crate) fn windows_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let titlebar = group("標題列");
    for (title, key) in [
        ("按兩下", "action-double-click-titlebar"),
        ("按中鍵", "action-middle-click-titlebar"),
        ("按右鍵", "action-right-click-titlebar"),
    ] {
        let current = output("gsettings", &["get", WM_PREFERENCES, key]).unwrap_or_default();
        let row = adw::ComboRow::builder()
            .title(title)
            .model(&gtk::StringList::new(&["縮放", "最小化", "無"]))
            // What Nekoland cannot do, GNOME's window menu or lowering, does nothing here.
            .selected(TITLEBAR_ACTIONS.iter().position(|action| current.contains(action)).unwrap_or(2) as u32)
            .build();
        row.connect_selected_notify(move |row| {
            if let Some(action) = TITLEBAR_ACTIONS.get(row.selected() as usize) {
                run("gsettings", &["set", WM_PREFERENCES, key, action]);
            }
        });
        titlebar.add(&row);
    }
    page.add(&titlebar);

    let tiling = group("並排");
    tiling.add(&switch_row(config, "拖到螢幕邊緣時並排", "edge_tiling"));
    tiling.add(&scale_row(
        "間距",
        config.number("window_gap", 0.0),
        (0.0, 32.0, 2.0),
        |gap| format!("{gap:.0} px"),
        |gap| set(&format!("window_gap = {gap:.0}")),
    ));
    page.add(&tiling);

    let focus = group("焦點");
    focus.add(&switch_row(config, "焦點跟隨游標", "focus_follows_pointer"));
    page.add(&focus);
    page
}

/// The hover panel: when it shows, where, how it looks and which fields, a group each, as
/// FinderHover's settings have them.
pub(crate) fn hover_place_summary() -> String {
    let config = Config::load();
    if config.flag("hover_follow") {
        return "跟隨指標".into();
    }
    let offset = |key: &str| config.number(key, 15.0);
    format!("偏移 {:.0}・{:.0} px", offset("hover_offset_x"), offset("hover_offset_y"))
}

pub(crate) fn hover_page(config: &Config) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let general = group("");
    let shown = switch_row(config, "顯示", "hover_info");
    general.add(&shown);
    let mut rows: Vec<gtk::Widget> = vec![
        scale_row(
            "延遲",
            config.number("hover_delay", 0.1),
            (0.1, 2.0, 0.1),
            |seconds| format!("{seconds:.1} 秒"),
            |seconds| set(&format!("hover_delay = {seconds:.1}")),
        )
        .upcast(),
        switch_row(config, "自動隱藏", "hover_auto_hide").upcast(),
        switch_row(config, "大型檔案保護", "hover_large_file_protection").upcast(),
    ];
    let hidden: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(
        config
            .0
            .get("hover_hide")
            .and_then(toml::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_else(|| HOVER_HIDDEN.map(String::from).to_vec()),
    ));
    let fields = adw::ActionRow::builder().title("欄位").activatable(true).build();
    fields.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    fields.connect_activated(move |row| hover_fields_dialog(row, hidden.clone()));
    rows.push(fields.upcast());
    for row in &rows {
        general.add(row);
    }
    page.add(&general);

    let place = group("");
    place.add(&switch_row(config, "跟隨指標", "hover_follow"));
    for (title, key) in [("水平偏移", "hover_offset_x"), ("垂直偏移", "hover_offset_y")] {
        place.add(&scale_row(
            title,
            config.number(key, 15.0),
            (0.0, 50.0, 5.0),
            |offset| format!("{offset:.0} px"),
            move |offset| set(&format!("{key} = {offset:.0}")),
        ));
    }

    let look = group("外觀");
    look.add(&choice_row(
        config,
        "樣式",
        "hover_style",
        &HOVER_STYLES,
        &["詳細", "工具提示"],
    ));
    let glass = switch_row(config, "毛玻璃", "hover_glass");
    let opacity = scale_row(
        "不透明度",
        config.number("hover_opacity", 0.98),
        (0.7, 1.0, 0.05),
        percent,
        |opacity| set(&format!("hover_opacity = {opacity:.2}")),
    );
    // As in FinderHover, opacity is for the plain panel; glass has its own.
    glass
        .bind_property("active", &opacity, "sensitive")
        .invert_boolean()
        .sync_create()
        .build();
    look.add(&glass);
    look.add(&opacity);
    look.add(&switch_row(config, "緊湊模式", "hover_compact"));
    look.add(&scale_row(
        "寬度",
        config.number("hover_width", 300.0),
        (300.0, 600.0, 20.0),
        |width| format!("{width:.0} px"),
        |width| set(&format!("hover_width = {width:.0}")),
    ));
    look.add(&scale_row(
        "文字大小",
        config.number("hover_font_size", 11.0),
        (9.0, 14.0, 1.0),
        |size| format!("{size:.0} 點"),
        |size| set(&format!("hover_font_size = {size:.1}")),
    ));
    page.add(&look);
    let more = group("");
    more.add(&subpage_row("位置", hover_place_summary, page_of_groups(&[place])));
    page.add(&more);

    // With the panel off, only the switch that brings it back is left.
    rows.extend([more.upcast(), look.upcast()]);
    for widget in rows {
        shown
            .bind_property("active", &widget, "visible")
            .sync_create()
            .build();
    }
    page
}

/// The hover panel's parts and every kind's fields, each to show or leave out, beside the
/// panel as the compositor draws it, as FinderHover's display settings.
pub(crate) fn hover_fields_dialog(parent: &adw::ActionRow, hidden: Rc<RefCell<Vec<String>>>) {
    const BASIC: &str = "基本資訊";
    // A line a part: its name, then its fields' labels, by tabs.
    let catalogue: Vec<(String, Vec<String>)> = output("nekoland", &["msg", "hover fields"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut words = line.split('\t');
            Some((words.next()?.to_string(), words.map(String::from).collect()))
        })
        .collect();
    let labels_of = |name: &str| {
        catalogue
            .iter()
            .find(|(part, _)| part == name)
            .map(|(_, labels)| labels.clone())
            .unwrap_or_default()
    };
    let basic = [
        ("dates", "日期", labels_of("dates")),
        ("file", "檔案資訊", labels_of("file")),
    ];
    let kinds: Vec<(String, Vec<String>)> = catalogue
        .iter()
        .filter(|(name, _)| name != "dates" && name != "file")
        .cloned()
        .collect();
    let is_hidden = {
        let hidden = hidden.clone();
        move |name: &str| hidden.borrow().iter().any(|hidden| hidden == name)
    };

    let picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();
    let selected = Rc::new(RefCell::new(BASIC.to_string()));
    let show_preview: Rc<dyn Fn()> = Rc::new({
        let (picture, selected) = (picture.clone(), selected.clone());
        move || {
            let command = format!("hover preview {}", selected.borrow());
            let Some(reply) = output("nekoland", &["msg", &command]) else {
                return;
            };
            let words: Vec<&str> = reply.trim().split('\t').collect();
            let [path, width, height] = words[..] else {
                return;
            };
            match gdk::Texture::from_filename(path) {
                Ok(texture) => {
                    picture.set_paintable(Some(&texture));
                    // In points: the PNG is drawn at the screen's scale.
                    picture.set_size_request(width.parse().unwrap_or(-1), height.parse().unwrap_or(-1));
                }
                Err(err) => eprintln!("{path}: {err}"),
            }
        }
    });

    // Each kind in the list says how many of its fields show, or that it is off.
    let summaries: Rc<Vec<(String, Vec<String>, gtk::Label)>> = Rc::new(
        kinds
            .iter()
            .map(|(name, labels)| {
                let label = gtk::Label::builder()
                    .css_classes(["dim-label", "numeric"])
                    .build();
                (name.clone(), labels.clone(), label)
            })
            .collect(),
    );
    let summarize: Rc<dyn Fn()> = Rc::new({
        let (summaries, is_hidden) = (summaries.clone(), is_hidden.clone());
        move || {
            for (name, labels, label) in summaries.iter() {
                if is_hidden(name) || is_hidden("type") {
                    label.set_label("關閉");
                } else {
                    let shown = labels
                        .iter()
                        .filter(|field| !is_hidden(&format!("{name}/{field}")))
                        .count();
                    label.set_label(&format!("{shown}/{}", labels.len()));
                }
            }
        }
    });
    let toggle: Rc<dyn Fn(&str, bool)> = Rc::new({
        let (hidden, summarize, show_preview) = (hidden.clone(), summarize.clone(), show_preview.clone());
        move |name, shown| {
            {
                let mut hidden = hidden.borrow_mut();
                hidden.retain(|hidden| hidden != name);
                if !shown {
                    hidden.push(name.to_string());
                }
                if *hidden == HOVER_HIDDEN {
                    unset("hover_hide");
                } else {
                    set(&format!("hover_hide = {hidden:?}"));
                }
            }
            summarize();
            show_preview();
        }
    });
    let switch = {
        let (toggle, is_hidden) = (toggle.clone(), is_hidden.clone());
        move |title: &str, name: String| {
            let row = adw::SwitchRow::builder()
                .title(title)
                .active(!is_hidden(&name))
                .build();
            let toggle = toggle.clone();
            row.connect_active_notify(move |row| toggle(&name, row.is_active()));
            row
        }
    };

    let inspector = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    let fill_inspector = {
        let (inspector, toggle, is_hidden) = (inspector.clone(), toggle.clone(), is_hidden.clone());
        let kinds = kinds.clone();
        move |name: &str| {
            while let Some(child) = inspector.first_child() {
                inspector.remove(&child);
            }
            if name == BASIC {
                let sections = group(BASIC);
                for (title, section) in HOVER_SECTIONS {
                    sections.add(&switch(title, section.to_string()));
                }
                inspector.append(&sections);
                for (part, title, labels) in &basic {
                    let fields = group(title);
                    for label in labels {
                        fields.add(&switch(label, format!("{part}/{label}")));
                    }
                    inspector.append(&fields);
                }
                return;
            }
            let Some((_, labels)) = kinds.iter().find(|(kind, _)| kind == name) else {
                return;
            };
            let fields = group(name);
            let master = gtk::Switch::builder()
                .active(!is_hidden(name))
                .valign(gtk::Align::Center)
                .build();
            master.update_property(&[gtk::accessible::Property::Label(name)]);
            fields.set_header_suffix(Some(&master));
            for label in labels {
                let row = switch(label, format!("{name}/{label}"));
                master
                    .bind_property("active", &row, "sensitive")
                    .sync_create()
                    .build();
                fields.add(&row);
            }
            let (toggle, kind) = (toggle.clone(), name.to_string());
            master.connect_active_notify(move |master| toggle(&kind, master.is_active()));
            inspector.append(&fields);
        }
    };

    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let names: Vec<String> = std::iter::once(BASIC.to_string())
        .chain(kinds.iter().map(|(name, _)| name.clone()))
        .collect();
    for name in &names {
        let title = gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let line = gtk::Box::builder().spacing(8).build();
        line.append(&title);
        if let Some((.., summary)) = summaries.iter().find(|(kind, ..)| kind == name) {
            line.append(summary);
        }
        list.append(&line);
    }
    summarize();
    list.connect_row_selected(move |_, row| {
        let Some(name) = row.and_then(|row| names.get(row.index() as usize)) else {
            return;
        };
        selected.replace(name.clone());
        fill_inspector(name);
        show_preview();
    });

    let body = gtk::Box::builder().build();
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(false)
            .width_request(160)
            .child(&list)
            .build(),
    );
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(
        &gtk::ScrolledWindow::builder()
            .hexpand(true)
            .css_classes(["view"])
            .child(&picture)
            .build(),
    );
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(false)
            .width_request(260)
            .child(&inspector)
            .build(),
    );
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("欄位")
        .content_width(1040)
        .content_height(720)
        .child(&view)
        .build();
    dialog.present(Some(parent));
    list.select_row(list.row_at_index(0).as_ref());
}
