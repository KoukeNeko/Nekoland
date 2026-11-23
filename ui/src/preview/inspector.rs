//! 顯示檢閱器: what the file is, its keywords, a PDF's permissions and annotations, and the size
//! of the part chosen, each on a tab of its own as the Mac's Preview has them.

use super::*;

/// A row of a name and its value, the value selectable.
fn property(title: &str, value: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(glib::markup_escape_text(value))
        .subtitle_selectable(true)
        .css_classes(["property"])
        .build()
}

fn group_of(rows: Vec<(String, String)>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    for (title, value) in rows.into_iter().filter(|(_, value)| !value.is_empty()) {
        group.add(&property(&title, &value));
    }
    group
}

fn date_label(when: Option<glib::DateTime>) -> String {
    when.and_then(|when| Some(modified_label(&when.to_local().ok()?, &glib::DateTime::now_local().ok()?))).unwrap_or_default()
}

/// A PDF's date, which poppler gives as seconds since 1970, or -1 when it has none.
fn unix_date(seconds: std::ffi::c_long) -> Option<glib::DateTime> {
    (seconds > 0).then(|| glib::DateTime::from_unix_local(i64::from(seconds)).ok()).flatten()
}

/// What exiftool reads of a picture: its rows to show, where it was taken, signed, and its
/// keywords.
#[derive(Default)]
struct Exif {
    rows: Vec<(String, String)>,
    gps: Option<(f64, f64)>,
    keywords: Vec<String>,
}

/// Runs exiftool once for all the inspector shows; nothing when it is not there.
fn read_exif(path: &Path) -> Exif {
    let mut exif = Exif::default();
    // `#` reads those two as signed numbers, the rest as exiftool words them.
    let args = ["-j", "-G1", "-q", "-EXIF:all", "-XMP:all", "-IPTC:all", "-Composite:GPSPosition", "-Composite:GPSLatitude#", "-Composite:GPSLongitude#"];
    let Ok(output) = Command::new("exiftool").args(args).arg(path).output() else {
        return exif;
    };
    let (mut latitude, mut longitude) = (None, None);
    // A tag a line: read one at a time, since serde_json's maps here sort what the file orders.
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim().trim_end_matches(',');
        let Ok(entry) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&format!("{{{line}}}")) else { continue };
        for (key, value) in entry {
            let words: Vec<String> = match value {
                serde_json::Value::Array(items) => items.iter().map(json_word).collect(),
                value => vec![json_word(&value)],
            };
            match key.as_str() {
                "SourceFile" => {}
                "Composite:GPSLatitude" => latitude = words.first().and_then(|word| word.parse::<f64>().ok()),
                "Composite:GPSLongitude" => longitude = words.first().and_then(|word| word.parse::<f64>().ok()),
                _ => {
                    if key == "XMP-dc:Subject" {
                        exif.keywords = words.iter().map(|word| word.trim()).filter(|word| !word.is_empty()).map(str::to_owned).collect();
                    }
                    let name = key.split_once(':').map_or(key.as_str(), |(_, name)| name);
                    let value = words.join(", ");
                    if !value.trim().is_empty() {
                        exif.rows.push((name.to_owned(), value.trim().to_owned()));
                    }
                }
            }
        }
    }
    exif.gps = latitude.zip(longitude);
    exif
}

fn json_word(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

impl Win {
    pub fn show_info(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        if item.is_page() || item.untitled {
            return self.present_info(&item, Exif::default());
        }
        // exiftool takes a while on a large file; the window's thread is not held up for it.
        let (weak, path) = (Rc::downgrade(self), item.path.clone());
        glib::spawn_future_local(async move {
            let exif = gio::spawn_blocking(move || read_exif(&path)).await.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            let Some(win) = weak.upgrade() else { return };
            if win.item().is_some_and(|shown| Rc::ptr_eq(&shown, &item)) {
                win.present_info(&item, exif);
            }
        });
    }

    fn present_info(self: &Rc<Self>, item: &Rc<Item>, exif: Exif) {
        let pdf = self.pdf.borrow().clone().filter(|_| item.is_page());
        let stack = adw::ViewStack::new();

        // 一般資訊
        let file = gio::File::for_path(&item.path);
        let info = file.query_info("standard::*,time::modified", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
        let kind = info.as_ref().and_then(|info| info.content_type()).map(|content_type| gio::content_type_get_description(&content_type).to_string()).unwrap_or_default();
        let mut rows = vec![("名稱".to_owned(), item.name()), ("種類".to_owned(), kind)];
        match (&pdf, self.shown.borrow().as_ref()) {
            (Some(pdf), _) => rows.push(("頁數".to_owned(), pdf.sizes.len().to_string())),
            (None, Some(pixbuf)) => {
                rows.push(("尺寸".to_owned(), format!("{} × {} 像素", pixbuf.width(), pixbuf.height())));
                if let Some(dpi) = item.dpi.get() {
                    rows.push(("解析度".to_owned(), format!("{} 像素/英寸", dpi.round())));
                }
            }
            (None, None) => {}
        }
        if !item.untitled {
            rows.push(("大小".to_owned(), info.as_ref().map(|info| glib::format_size(info.size() as u64).to_string()).unwrap_or_default()));
            rows.push(("修改日期".to_owned(), date_label(info.as_ref().and_then(|info| info.modification_date_time()))));
            rows.push(("位置".to_owned(), item.path.parent().map(|dir| dir.display().to_string()).unwrap_or_default()));
        }
        let general = adw::PreferencesPage::new();
        general.add(&group_of(rows));
        stack.add_titled_with_icon(&general, Some("general"), "一般資訊", "document-properties-symbolic");

        // 更多資訊
        let more = adw::PreferencesPage::new();
        let more_rows: Vec<(String, String)> = match &pdf {
            Some(pdf) => {
                let document = pdf.document();
                let text = |value: Option<glib::GString>| value.map(|value| value.to_string()).unwrap_or_default();
                vec![
                    ("標題".to_owned(), text(document.title())),
                    ("作者".to_owned(), text(document.author())),
                    ("主題".to_owned(), text(document.subject())),
                    ("製作程式".to_owned(), text(document.creator())),
                    ("PDF 製作程式".to_owned(), text(document.producer())),
                    ("版本".to_owned(), text(document.pdf_version_string())),
                    ("製作日期".to_owned(), date_label(unix_date(document.creation_date()))),
                    ("修改日期".to_owned(), date_label(unix_date(document.modification_date()))),
                    ("已加密".to_owned(), if pdf.password.is_some() { "是".to_owned() } else { "否".to_owned() }),
                    ("快速網頁檢視".to_owned(), if document.is_linearized() { "是".to_owned() } else { "否".to_owned() }),
                ]
            }
            None => exif.rows,
        };
        if more_rows.is_empty() {
            more.add(&adw::PreferencesGroup::builder().title("沒有其他資訊").build());
        } else {
            more.add(&group_of(more_rows));
        }
        if let Some((latitude, longitude)) = exif.gps {
            let place = adw::PreferencesGroup::builder().title("位置").build();
            let row = adw::ActionRow::builder().title(format!("{latitude:.5}, {longitude:.5}")).activatable(true).build();
            row.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
            let window = self.window.clone();
            row.connect_activated(move |_| {
                let url = format!("https://www.openstreetmap.org/?mlat={latitude}&mlon={longitude}#map=15/{latitude}/{longitude}");
                gtk::UriLauncher::new(&url).launch(Some(&window), gio::Cancellable::NONE, |_| {});
            });
            place.add(&row);
            more.add(&place);
        }
        stack.add_titled_with_icon(&more, Some("more"), "更多資訊", "dialog-information-symbolic");

        // 關鍵字
        if !item.untitled {
            stack.add_titled_with_icon(&self.keywords_page(item, pdf.clone(), exif.keywords), Some("keywords"), "關鍵字", "tag-symbolic");
        }

        // 加密
        if let Some(pdf) = &pdf {
            let allowed = pdfdoc::permissions(&pdf.document());
            let yes_no = |allowed: bool| if allowed { "允許" } else { "不允許" }.to_owned();
            let page = adw::PreferencesPage::new();
            page.add(&group_of(vec![
                ("需要密碼才能打開".to_owned(), if pdf.password.is_some() { "是".to_owned() } else { "否".to_owned() }),
                ("列印".to_owned(), yes_no(allowed.print)),
                ("拷貝文字和影像".to_owned(), yes_no(allowed.copy)),
                ("插入、刪除和旋轉頁面".to_owned(), yes_no(allowed.assemble)),
                ("加入註解和簽名".to_owned(), yes_no(allowed.annotate)),
                ("填寫現有的表單欄位".to_owned(), yes_no(allowed.forms)),
            ]));
            let change = adw::PreferencesGroup::new();
            let button = adw::ActionRow::builder().activatable(true).title("編輯權限⋯").build();
            let weak = Rc::downgrade(self);
            button.connect_activated(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.edit_permissions();
                }
            });
            change.add(&button);
            page.add(&change);
            stack.add_titled_with_icon(&page, Some("encryption"), "加密", "channel-secure-symbolic");
        }

        // 裁切
        let crop = adw::PreferencesPage::new();
        let chosen = match &pdf {
            Some(_) => self.area.get().map(|(_, (x1, y1, x2, y2))| format!("{:.0} × {:.0} 點（{:.2} × {:.2} 公分）", x2 - x1, y2 - y1, (x2 - x1) / 72.0 * 2.54, (y2 - y1) / 72.0 * 2.54)),
            None => self.image_mask.borrow().as_ref().and_then(imaging::Mask::bounds).map(|(x1, y1, x2, y2)| format!("{} × {} 像素", x2 - x1, y2 - y1)),
        };
        crop.add(&group_of(vec![("選取範圍".to_owned(), chosen.unwrap_or_else(|| "無".to_owned()))]));
        stack.add_titled_with_icon(&crop, Some("crop"), "裁切", "edit-cut-symbolic");

        // 註解
        if let Some(pdf) = &pdf {
            let page = adw::PreferencesPage::new();
            let document = pdf.document();
            let mut any = false;
            for index in 0..pdf.sizes.len() {
                let Some(on) = document.page(index as i32) else { continue };
                let found = annots::on_page(&on);
                if found.is_empty() {
                    continue;
                }
                any = true;
                let group = adw::PreferencesGroup::builder().title(format!("第 {} 頁", pdfdoc::page_label(&document, index, self.prefs.borrow().logical_numbers))).build();
                for annotation in found {
                    let row = adw::ActionRow::builder().title(annotation.kind.label()).subtitle(glib::markup_escape_text(&annotation.contents)).activatable(true).build();
                    let weak = Rc::downgrade(self);
                    let top = annotation.place.1;
                    row.connect_activated(move |_| {
                        if let Some(win) = weak.upgrade() {
                            win.go_page(index, Some(top));
                        }
                    });
                    group.add(&row);
                }
                page.add(&group);
            }
            if !any {
                page.add(&adw::PreferencesGroup::builder().title("沒有註解").build());
            }
            stack.add_titled_with_icon(&page, Some("annotations"), "註解", "chat-message-new-symbolic");
        }

        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build()));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));
        let dialog = adw::Dialog::builder().title("檢閱器").content_width(720).content_height(520).child(&view).build();
        dialog.present(Some(&self.window));
    }

    /// 關鍵字: a PDF's kept in it, to be saved with it; a picture's written at once.
    fn keywords_page(self: &Rc<Self>, item: &Rc<Item>, pdf: Option<Rc<Pdf>>, picture_keywords: Vec<String>) -> adw::PreferencesPage {
        let words: Vec<String> = match &pdf {
            Some(pdf) => pdf.document().keywords().map(|words| words.split([',', ';']).map(str::trim).filter(|word| !word.is_empty()).map(str::to_owned).collect()).unwrap_or_default(),
            None => picture_keywords,
        };
        let words = Rc::new(RefCell::new(words));
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::new();
        let entry = adw::EntryRow::builder().title("新增關鍵字").show_apply_button(true).build();
        group.add(&entry);
        page.add(&group);
        let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        let list_group = adw::PreferencesGroup::new();
        list_group.add(&list);
        page.add(&list_group);

        let write: Rc<dyn Fn(&[String])> = {
            let (weak, item) = (Rc::downgrade(self), item.clone());
            Rc::new(move |words: &[String]| {
                let Some(win) = weak.upgrade() else { return };
                match &pdf {
                    Some(_) => {
                        let words = words.join(", ");
                        win.change_pdf(0, move |document, _| {
                            document.set_keywords(&words);
                            None
                        });
                    }
                    None => {
                        win.keep_version(&item.path);
                        let mut command = Command::new("exiftool");
                        command.args(["-overwrite_original", "-q", "-XMP-dc:Subject="]);
                        for word in words {
                            command.arg(format!("-XMP-dc:Subject+={word}"));
                        }
                        match command.arg(&item.path).output() {
                            Ok(output) if output.status.success() => {}
                            Ok(output) => alert::notice(&win.window, "無法儲存", Some(String::from_utf8_lossy(&output.stderr).trim())),
                            Err(err) => alert::notice(&win.window, "無法儲存", Some(&format!("exiftool: {err}"))),
                        }
                    }
                }
            })
        };
        let fill: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
        let fill_with: Rc<dyn Fn()> = {
            let (list, words, write, fill) = (list.clone(), words.clone(), write.clone(), fill.clone());
            Rc::new(move || {
                list.remove_all();
                list.set_visible(!words.borrow().is_empty());
                for (index, word) in words.borrow().iter().enumerate() {
                    let row = adw::ActionRow::builder().title(glib::markup_escape_text(word)).build();
                    let remove = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("刪除").valign(gtk::Align::Center).css_classes(["flat"]).build();
                    let (words, write, fill) = (words.clone(), write.clone(), fill.clone());
                    remove.connect_clicked(move |_| {
                        words.borrow_mut().remove(index);
                        write(&words.borrow());
                        if let Some(fill) = fill.borrow().clone() {
                            fill();
                        }
                    });
                    row.add_suffix(&remove);
                    list.append(&row);
                }
            })
        };
        *fill.borrow_mut() = Some(fill_with.clone());
        fill_with();
        entry.connect_apply(move |entry| {
            let word = entry.text().trim().to_owned();
            if word.is_empty() || words.borrow().contains(&word) {
                return;
            }
            words.borrow_mut().push(word);
            write(&words.borrow());
            entry.set_text("");
            fill_with();
        });
        page
    }
}
