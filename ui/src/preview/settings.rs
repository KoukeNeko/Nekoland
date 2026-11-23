//! 設定, as the Mac's Preview has it on 一般, 影像, PDF and 簽名, and 鍵盤快速鍵, both kept on the
//! application so every window shares them.

use super::*;

/// The window that 設定 and 鍵盤快速鍵 belong to: the one in front, else any.
fn front(app: &adw::Application) -> Option<Rc<Win>> {
    let active = app.active_window();
    WINDOWS.with(|windows| {
        let windows = windows.borrow();
        windows.iter().find(|win| active.as_ref() == Some(win.window.upcast_ref())).or(windows.first()).cloned()
    })
}

/// Saves `change` to 設定 and shows it in every window.
fn change(change: impl FnOnce(&mut Prefs)) {
    let prefs = PREFS.with(Rc::clone);
    change(&mut prefs.borrow_mut());
    prefs.borrow().save();
    let windows = WINDOWS.with(|windows| windows.borrow().clone());
    for win in windows {
        win.apply_background();
        win.apply_zoom();
        if let Some(item) = win.item() {
            win.update_title(&item);
        }
    }
}

fn switch(title: &str, active: bool, set: impl Fn(&mut Prefs, bool) + 'static) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(active).build();
    row.connect_active_notify(move |row| {
        let on = row.is_active();
        change(|prefs| set(prefs, on));
    });
    row
}

/// The colours the window's background can be, as CSS writes them; the first keeps the theme's.
const BACKGROUNDS: [(&str, Option<&str>); 5] = [("預設", None), ("白色", Some("#ffffff")), ("淺灰色", Some("#d0d0d0")), ("深灰色", Some("#3a3a3a")), ("黑色", Some("#000000"))];

fn preferences(app: &adw::Application) {
    let prefs = PREFS.with(|prefs| prefs.borrow().clone());

    let general = adw::PreferencesPage::builder().title("一般").icon_name("preferences-system-symbolic").build();
    let window = adw::PreferencesGroup::new();
    let background = adw::ComboRow::builder()
        .title("視窗背景")
        .model(&gtk::StringList::new(&BACKGROUNDS.map(|(name, _)| name)))
        .selected(BACKGROUNDS.iter().position(|(_, colour)| colour.map(str::to_owned) == prefs.background).unwrap_or(0) as u32)
        .build();
    background.connect_selected_notify(|row| {
        let colour = BACKGROUNDS.get(row.selected() as usize).and_then(|(_, colour)| colour.map(str::to_owned));
        change(|prefs| prefs.background = colour);
    });
    window.add(&background);
    window.add(&switch("同時打開的檔案放在同一個視窗", prefs.group_files, |prefs, on| prefs.group_files = on));
    general.add(&window);

    let images = adw::PreferencesPage::builder().title("影像").icon_name("image-x-generic-symbolic").build();
    let zoom = adw::PreferencesGroup::builder().title("縮放 100% 時").build();
    let image_size = adw::ComboRow::builder().title("影像大小").model(&gtk::StringList::new(&["一個像素等於一個螢幕像素", "等於列印大小"])).selected(u32::from(prefs.image_print_size)).build();
    image_size.connect_selected_notify(|row| {
        let print = row.selected() == 1;
        change(|prefs| prefs.image_print_size = print);
    });
    zoom.add(&image_size);
    images.add(&zoom);
    let editing = adw::PreferencesGroup::new();
    editing.add(&switch("拷貝主體", prefs.copy_subject, |prefs, on| prefs.copy_subject = on));
    images.add(&editing);

    let pdf = adw::PreferencesPage::builder().title("PDF").icon_name("x-office-document-symbolic").build();
    let opening = adw::PreferencesGroup::new();
    opening.add(&switch("從上次閱讀的頁面繼續", prefs.resume, |prefs, on| prefs.resume = on));
    const LAYOUTS: [(&str, Layout); 3] = [("連續捲動", Layout::Continuous), ("單頁", Layout::Single), ("雙頁", Layout::Two)];
    let layout = adw::ComboRow::builder()
        .title("首次打開時")
        .model(&gtk::StringList::new(&LAYOUTS.map(|(name, _)| name)))
        .selected(LAYOUTS.iter().position(|(_, layout)| *layout == prefs.first_layout).unwrap_or(0) as u32)
        .build();
    layout.connect_selected_notify(|row| {
        let layout = LAYOUTS.get(row.selected() as usize).map_or(Layout::Continuous, |(_, layout)| *layout);
        change(|prefs| prefs.first_layout = layout);
    });
    opening.add(&layout);
    opening.add(&switch("使用邏輯頁碼", prefs.logical_numbers, |prefs, on| prefs.logical_numbers = on));
    let pdf_size = adw::ComboRow::builder().title("縮放 100% 時").model(&gtk::StringList::new(&["一點等於一個螢幕像素", "等於列印大小"])).selected(u32::from(prefs.pdf_print_size)).build();
    pdf_size.connect_selected_notify(|row| {
        let print = row.selected() == 1;
        change(|prefs| prefs.pdf_print_size = print);
    });
    opening.add(&pdf_size);
    pdf.add(&opening);
    let annotations = adw::PreferencesGroup::builder().title("註解").build();
    annotations.add(&switch("加上名稱", prefs.sign_annotations, |prefs, on| prefs.sign_annotations = on));
    let author = adw::EntryRow::builder().title("名稱").text(&prefs.author).show_apply_button(true).build();
    author.connect_apply(|row| {
        let name = row.text().trim().to_owned();
        change(|prefs| prefs.author = name);
    });
    annotations.add(&author);
    pdf.add(&annotations);

    let signatures = adw::PreferencesPage::builder().title("簽名").icon_name("document-edit-symbolic").build();
    let made = adw::PreferencesGroup::new();
    for (_, name) in &prefs.signatures {
        made.add(&adw::ActionRow::builder().title(glib::markup_escape_text(name)).build());
    }
    if prefs.signatures.is_empty() {
        made.add(&adw::ActionRow::builder().title("沒有簽名").build());
    }
    signatures.add(&made);
    let manage = adw::PreferencesGroup::new();
    let button = adw::ActionRow::builder().activatable(true).title("製作簽名⋯").build();
    let dialog = adw::PreferencesDialog::new();
    {
        let (app, dialog) = (app.clone(), dialog.downgrade());
        button.connect_activated(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            if let Some(win) = front(&app) {
                win.make_signature();
            }
        });
    }
    manage.add(&button);
    signatures.add(&manage);

    for page in [&general, &images, &pdf, &signatures] {
        dialog.add(page);
    }
    dialog.set_title("設定");
    if let Some(win) = front(app) {
        dialog.present(Some(&win.window));
    }
}

fn shortcuts(app: &adw::Application) {
    let page = adw::PreferencesPage::new();
    for (title, entries) in menus::with_keys() {
        let group = adw::PreferencesGroup::builder().title(title).build();
        for entry in entries {
            let row = adw::ActionRow::builder().title(entry.label.trim_end_matches('⋯')).build();
            for key in entry.keys.iter().take(1) {
                row.add_suffix(&gtk::ShortcutLabel::builder().accelerator(*key).valign(gtk::Align::Center).build());
            }
            group.add(&row);
        }
        page.add(&group);
    }
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder().title("鍵盤快速鍵").content_width(480).content_height(600).child(&view).build();
    if let Some(win) = front(app) {
        dialog.present(Some(&win.window));
    }
}

/// 結束: every window closed as 關閉視窗 closes it, saving what it changed first.
fn quit() {
    let windows = WINDOWS.with(|windows| windows.borrow().clone());
    for win in windows {
        win.close_window();
    }
}

pub fn add_app_actions(app: &adw::Application) {
    let actions: [(&str, fn(&adw::Application)); 3] = [("preferences", preferences), ("shortcuts", shortcuts), ("quit", |_| quit())];
    for (name, run) in actions {
        let action = gio::SimpleAction::new(name, None);
        let app_ref = app.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(app) = app_ref.upgrade() {
                run(&app);
            }
        });
        app.add_action(&action);
    }
}
