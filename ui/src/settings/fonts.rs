//! Choosing fonts, and the Fonts page.

use super::*;

/// A row naming its font in that font, which opens the font picker; `mono` picks a face and
/// size among the monospaced families, otherwise a family alone.
pub(crate) fn font_row(
    title: &str,
    mono: bool,
    font: &str,
    changed: impl Fn(&gtk::pango::FontDescription) + 'static,
) -> adw::ActionRow {
    let font = Rc::new(RefCell::new(gtk::pango::FontDescription::from_string(font)));
    let current = gtk::Label::builder().css_classes(["dim-label"]).build();
    let row = adw::ActionRow::builder().title(title).activatable(true).build();
    row.add_suffix(&current);
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let show = move |font: &gtk::pango::FontDescription| {
        current.set_label(&font.to_string());
        write_in(&current, font);
    };
    show(&font.borrow());
    let apply: Rc<dyn Fn(&gtk::pango::FontDescription)> = Rc::new({
        let font = font.clone();
        move |chosen| {
            show(chosen);
            font.replace(chosen.clone());
            changed(chosen);
        }
    });
    let title = format!("{title}字型");
    row.connect_activated(move |row| {
        let applied = font.borrow().clone();
        font_picker(row, &title, mono, applied, apply.clone());
    });
    row
}

/// The font picker: each family named in itself, and a page of Nekoland's own text in the
/// one shown. The arrow keys only show a family; clicking it or Enter sets it.
pub(crate) fn font_picker(
    parent: &adw::ActionRow,
    title: &str,
    mono: bool,
    applied: gtk::pango::FontDescription,
    apply: Rc<dyn Fn(&gtk::pango::FontDescription)>,
) {
    use gtk::pango::{self, prelude::*};
    let applied = Rc::new(RefCell::new(applied));
    let size = Rc::new(Cell::new(applied.borrow().size()));
    let list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let context = list.pango_context();
    let mut families: Vec<pango::FontFamily> = context
        .list_families()
        .into_iter()
        .filter(|family| !mono || family.is_monospace())
        .collect();
    families.sort_by_key(|family| family.name().to_lowercase());
    // A family as the picker sets it: the interface's alone, the monospace one's
    // default face at the size chosen.
    let font_for = {
        let size = size.clone();
        move |family: &pango::FontFamily| {
            let mut font = pango::FontDescription::new();
            if mono {
                if let Some(face) = family.face(None) {
                    font = face.describe();
                }
                font.set_size(size.get());
            }
            font.set_family(&family.name());
            font
        }
    };

    let preview = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(120)
        .css_classes(["card"])
        .build();
    let pages = [preview_page(mono), preview_page(mono)];
    for (page, labels) in &pages {
        preview.add_child(page);
        labels.iter().for_each(|label| write_in(label, &applied.borrow()));
    }
    // The page out of sight takes the font, then fades in over the other.
    let show = Rc::new(glib::clone!(
        #[weak]
        preview,
        move |font: &pango::FontDescription| {
            let shown = preview.visible_child();
            let (page, labels) = if shown.as_ref() == Some(pages[0].0.upcast_ref()) {
                &pages[1]
            } else {
                &pages[0]
            };
            labels.iter().for_each(|label| write_in(label, font));
            preview.set_visible_child(page);
        }
    ));

    let mut rows = Vec::new();
    for family in &families {
        let name = family.name();
        let mut own = pango::FontDescription::new();
        own.set_family(&name);
        own.set_size(12 * pango::SCALE);
        let label = gtk::Label::builder()
            .label(name.as_str())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        let Some(font) = context.load_font(&own) else {
            continue;
        };
        // Icon and symbol fonts, and those of other scripts alone, cannot write a name.
        if !font.has_char('a') {
            continue;
        }
        write_in(&label, &own);
        let chinese = font.has_char('體');
        let tag = gtk::Label::builder()
            .label("中文")
            .css_classes(["dim-label", "caption"])
            .visible(chinese)
            .build();
        let check = gtk::Image::builder()
            .icon_name("object-select-symbolic")
            .visible(applied.borrow().family().as_deref() == Some(name.as_str()))
            .build();
        let line = gtk::Box::builder()
            .spacing(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        line.append(&label);
        line.append(&tag);
        line.append(&check);
        list.append(&line);
        rows.push((family.clone(), chinese, check));
    }
    let rows = Rc::new(rows);
    list.set_placeholder(Some(
        &gtk::Label::builder()
            .label("沒有符合的字型")
            .css_classes(["dim-label"])
            .margin_top(24)
            .margin_bottom(24)
            .build(),
    ));

    let search = gtk::SearchEntry::builder()
        .placeholder_text("搜尋")
        .hexpand(true)
        .build();
    let chinese_only = Rc::new(Cell::new(false));
    list.set_filter_func(glib::clone!(
        #[strong]
        rows,
        #[strong]
        chinese_only,
        #[weak]
        search,
        #[upgrade_or]
        true,
        move |row| {
            let (family, chinese, _) = &rows[row.index() as usize];
            (*chinese || !chinese_only.get())
                && family
                    .name()
                    .to_lowercase()
                    .contains(&search.text().to_lowercase())
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[weak]
        list,
        move |_| list.invalidate_filter()
    ));
    let tools = gtk::Box::builder().spacing(8).build();
    tools.append(&search);
    if !mono {
        let only = segmented(&["全部", "含中文"], 0);
        only.connect_notify_local(
            Some("active"),
            glib::clone!(
                #[weak]
                list,
                move |only, _| {
                    chinese_only.set(only.property::<u32>("active") == 1);
                    list.invalidate_filter();
                }
            ),
        );
        tools.append(&only);
    }

    // The monospace font's face, among its family's, and its size.
    let faces = Rc::new(RefCell::new(Vec::<pango::FontFace>::new()));
    let face_names = gtk::StringList::new(&[]);
    let face = gtk::DropDown::builder().model(&face_names).build();
    let filling = Rc::new(Cell::new(false));
    let fill_faces = Rc::new(glib::clone!(
        #[strong]
        faces,
        #[strong]
        applied,
        #[strong]
        filling,
        #[strong]
        rows,
        #[weak]
        face,
        move || {
            let applied = applied.borrow();
            let family = rows
                .iter()
                .find(|(family, ..)| applied.family().as_deref() == Some(family.name().as_str()));
            let list: Vec<pango::FontFace> =
                family.map(|(family, ..)| family.list_faces()).unwrap_or_default();
            let mut bare = applied.clone();
            bare.unset_fields(pango::FontMask::SIZE);
            filling.set(true);
            face_names.splice(0, face_names.n_items(), &[]);
            for one in &list {
                face_names.append(&one.face_name());
            }
            face.set_selected(list.iter().position(|one| one.describe() == bare).unwrap_or(0) as u32);
            face.set_visible(list.len() > 1);
            filling.set(false);
            faces.replace(list);
        }
    ));
    let points = gtk::SpinButton::with_range(6.0, 72.0, 1.0);
    points.set_value(f64::from(size.get() / pango::SCALE));
    let set_font = Rc::new(glib::clone!(
        #[strong]
        applied,
        #[strong]
        apply,
        #[strong]
        show,
        move |font: pango::FontDescription| {
            apply(&font);
            show(&font);
            applied.replace(font);
        }
    ));
    face.connect_selected_notify(glib::clone!(
        #[strong]
        faces,
        #[strong]
        filling,
        #[strong]
        size,
        #[strong]
        set_font,
        move |face| {
            let Some(chosen) = faces
                .borrow()
                .get(face.selected() as usize)
                .map(|one| one.describe())
            else {
                return;
            };
            if !filling.get() {
                let mut font = chosen;
                font.set_size(size.get());
                set_font(font);
            }
        }
    ));
    points.connect_value_changed(glib::clone!(
        #[strong]
        size,
        #[strong]
        applied,
        #[strong]
        set_font,
        move |points| {
            size.set(points.value() as i32 * pango::SCALE);
            let mut font = applied.borrow().clone();
            font.set_size(size.get());
            set_font(font);
        }
    ));
    let face_line = gtk::Box::builder().spacing(8).visible(mono).build();
    face_line.append(&face);
    face_line.append(&gtk::Box::builder().hexpand(true).build());
    face_line.append(&points);
    if mono {
        fill_faces();
    }

    list.connect_row_selected(glib::clone!(
        #[strong]
        rows,
        #[strong]
        show,
        #[strong]
        font_for,
        move |_, row| {
            if let Some((family, ..)) = row.and_then(|row| rows.get(row.index() as usize)) {
                show(&font_for(family));
            }
        }
    ));
    list.connect_row_activated(glib::clone!(
        #[strong]
        rows,
        move |_, row| {
            let Some((family, ..)) = rows.get(row.index() as usize) else {
                return;
            };
            set_font(font_for(family));
            for (other, _, check) in rows.iter() {
                check.set_visible(other.name() == family.name());
            }
            if mono {
                fill_faces();
            }
        }
    ));

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(24)
        .build();
    body.append(&preview);
    body.append(&face_line);
    body.append(&tools);
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
        .title(title)
        .content_width(520)
        .content_height(640)
        .child(&view)
        .build();
    search.set_key_capture_widget(Some(&dialog));
    dialog.present(Some(parent));
    // Focus, which scrolls to the row, takes once the dialog is shown.
    let applied_name = applied.borrow().family().map(|family| family.to_string());
    let index = rows
        .iter()
        .position(|(family, ..)| Some(family.name().to_string()) == applied_name);
    glib::idle_add_local_once(move || {
        if let Some(row) = index.and_then(|index| list.row_at_index(index as i32)) {
            list.select_row(Some(&row));
            row.grab_focus();
        }
    });
}

/// A page of Nekoland's own text, for the font picker to show a font in, and its labels.
pub(crate) fn preview_page(mono: bool) -> (gtk::Box, Vec<gtk::Label>) {
    let mut labels = Vec::new();
    let mut text = |text: &str, classes: &[&str]| {
        let label = gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .css_classes(classes.to_vec())
            .build();
        labels.push(label.clone());
        label
    };
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    if mono {
        let code = text(
            "~/Nekoland ❯ cargo check\n\
             fn font(description: &str) -> FontDescription {\n    \
             // 沒有字族時用介面字型\n    \
             let mut font = from_string(description);\n    \
             font.set_size(11 * SCALE); // 0O1lI {}[]\n}",
            &[],
        );
        code.set_margin_top(14);
        code.set_margin_bottom(14);
        code.set_margin_start(16);
        code.set_margin_end(16);
        page.append(&code);
        return (page, labels);
    }
    let now = glib::DateTime::now_local().expect("local time");
    let weekday = ["一", "二", "三", "四", "五", "六", "日"][now.day_of_week() as usize - 1];
    let bar = gtk::Box::builder()
        .spacing(16)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(14)
        .margin_end(14)
        .build();
    bar.append(&text("文件檢視器", &["heading"]));
    for menu in ["檔案", "編輯", "檢視"] {
        bar.append(&text(menu, &[]));
    }
    let clock = text(
        &format!(
            "{}月{}日 週{weekday} {:02}:{:02}",
            now.month(),
            now.day_of_month(),
            now.hour(),
            now.minute()
        ),
        &["numeric"],
    );
    clock.set_hexpand(true);
    clock.set_xalign(1.0);
    bar.append(&clock);
    page.append(&bar);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let note = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(14)
        .margin_start(14)
        .margin_end(14)
        .css_classes(["card"])
        .build();
    let from = gtk::Box::builder()
        .margin_top(10)
        .margin_start(14)
        .margin_end(14)
        .build();
    from.append(&text("行事曆", &["caption", "dim-label"]));
    let when = text("現在", &["caption", "dim-label"]);
    when.set_hexpand(true);
    when.set_xalign(1.0);
    from.append(&when);
    note.append(&from);
    let title = text("設計評審 · 14:30", &["heading"]);
    let body = text("3A 會議室，帶 Q3 Roadmap 的 PDF 與 Dock 新圖示", &["dim-label"]);
    for line in [&title, &body] {
        line.set_margin_start(14);
        line.set_margin_end(14);
    }
    body.set_margin_bottom(10);
    note.append(&title);
    note.append(&body);
    page.append(&note);

    let window = text("季報-Q3.pdf — 文件檢視器", &["heading", "dim-label"]);
    window.set_xalign(0.5);
    window.set_margin_top(12);
    window.set_margin_bottom(14);
    page.append(&window);
    (page, labels)
}

/// The typefaces, for Nekoland and the applications, and how applications draw text.
/// Nekoland's own text keeps its size and the Mac's grey, unhinted edges.
pub(crate) fn fonts_page(look: &gtk::CssProvider) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let faces = group("字型");
    let family = UI_FONT.with_borrow(Clone::clone);
    let look = look.clone();
    faces.add(&font_row("介面", false, &family, move |font| {
        let Some(family) = font.family() else { return };
        set(&format!("font = {}", toml_string(family.as_str())));
        // GNOME's setting keeps its size; GTK takes the family from it without the look.
        let mut name = gtk::pango::FontDescription::from_string(&interface_setting("font-name"));
        name.set_family(&family);
        if name.size() == 0 {
            name.set_size(11 * gtk::pango::SCALE);
        }
        set_interface("font-name", &name.to_string());
        UI_FONT.set(family.to_string());
        if gtk_look_on() {
            if let Err(err) = set_gtk_look(true) {
                eprintln!("GTK look: {err}");
            }
            look.load_from_string(&look_for(adw::StyleManager::default().is_dark()));
        }
    }));
    faces.add(&font_row(
        "等寬",
        true,
        &interface_setting("monospace-font-name"),
        |font| set_interface("monospace-font-name", &font.to_string()),
    ));
    page.add(&faces);

    let text = group("文字");
    let scale: f64 = interface_setting("text-scaling-factor").parse().unwrap_or(1.0);
    let size = adw::ComboRow::builder()
        .title("文字大小")
        .model(&gtk::StringList::new(&["100%", "110%", "125%", "150%", "175%", "200%"]))
        .selected(TEXT_SCALES.iter().position(|known| (known - scale).abs() < 0.01).unwrap_or(0) as u32)
        .build();
    size.connect_selected_notify(|row| {
        if let Some(scale) = TEXT_SCALES.get(row.selected() as usize) {
            set_interface("text-scaling-factor", &scale.to_string());
        }
    });
    text.add(&size);

    let automatic = interface_setting("font-rendering") == "automatic";
    let antialiasing = adw::ComboRow::builder()
        .title("反鋸齒")
        .model(&gtk::StringList::new(&["自動", "灰階", "次像素", "無"]))
        .selected(if automatic {
            0
        } else {
            let current = interface_setting("font-antialiasing");
            ANTIALIASING.iter().position(|known| *known == current).unwrap_or(0) as u32
        })
        .build();
    let hinting = adw::ComboRow::builder()
        .title("微調")
        .model(&gtk::StringList::new(&["無", "輕微", "中等", "完整"]))
        .selected({
            let current = interface_setting("font-hinting");
            HINTING.iter().position(|known| *known == current).unwrap_or(0) as u32
        })
        .visible(!automatic)
        .build();
    antialiasing.connect_selected_notify(glib::clone!(
        #[weak]
        hinting,
        move |row| {
            let Some(chosen) = ANTIALIASING.get(row.selected() as usize) else { return };
            let automatic = *chosen == "automatic";
            set_interface("font-rendering", if automatic { "automatic" } else { "manual" });
            if !automatic {
                set_interface("font-antialiasing", chosen);
            }
            hinting.set_visible(!automatic);
        }
    ));
    hinting.connect_selected_notify(|row| {
        if let Some(chosen) = HINTING.get(row.selected() as usize) {
            set_interface("font-hinting", chosen);
        }
    });
    text.add(&antialiasing);
    text.add(&hinting);
    page.add(&text);
    page
}
