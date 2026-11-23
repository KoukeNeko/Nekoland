//! Changes to what is shown: a picture turned, flipped, cut, its background taken out, its
//! colour adjusted and its size changed; a PDF's pages turned, cropped, put in another order,
//! taken out and put in from files, blank or from a scanner.

use super::*;

impl Win {
    /// Changes the picture shown with `change`, kept first for 還原.
    pub fn edit(self: &Rc<Self>, change: impl FnOnce(&Pixbuf) -> Option<Pixbuf>) {
        let (Some(item), Some(pixbuf)) = (self.item(), self.shown.borrow().clone()) else {
            return;
        };
        let Some(changed) = change(&pixbuf) else {
            return alert::notice(&self.window, "無法編輯", Some("記憶體不足。"));
        };
        let before = item.edited.borrow_mut().replace(changed.clone());
        item.undo.borrow_mut().push(before);
        item.redo.borrow_mut().clear();
        self.show(&item, changed);
    }

    /// 向左旋轉 or 向右旋轉: the picture, or the PDF's pages chosen.
    pub fn turn(self: &Rc<Self>, clockwise: bool) {
        if self.item().is_some_and(|item| item.is_page()) {
            let pages = self.chosen_pages();
            let degrees = if clockwise { 90 } else { -90 };
            return self.change_pages(move |path, password| pdfdoc::rotate(path, password.as_deref(), &pages, degrees), None);
        }
        let rotation = if clockwise { PixbufRotation::Clockwise } else { PixbufRotation::Counterclockwise };
        self.set_mask(None, Vec::new());
        for_each_picture(self, |pixbuf| pixbuf.rotate_simple(rotation));
    }

    pub fn flip(self: &Rc<Self>, horizontal: bool) {
        self.set_mask(None, Vec::new());
        for_each_picture(self, |pixbuf| pixbuf.flip(horizontal));
    }

    /// 裁切: the picture cut to the part chosen, or the PDF's pages chosen to the part of a
    /// page chosen.
    pub fn crop(self: &Rc<Self>) {
        if self.item().is_some_and(|item| item.is_page()) {
            let Some((_, place)) = self.area.take() else { return };
            let pages = self.chosen_pages();
            self.redraw_pages();
            return self.change_pages(move |path, password| pdfdoc::crop(path, password.as_deref(), &pages, place), None);
        }
        let Some(mask) = self.image_mask.borrow().clone() else { return };
        self.set_mask(None, Vec::new());
        let whole_box = mask.inside.iter().all(|inside| *inside) || is_box(&mask);
        if !whole_box {
            self.keep_clear_pixels();
        }
        self.edit(|pixbuf| imaging::cut(&imaging::Rgba::from_pixbuf(pixbuf), &mask).map(|part| part.to_pixbuf()));
    }

    /// 剪下: what is chosen copied, then made clear.
    pub fn cut(self: &Rc<Self>) {
        self.copy();
        self.delete();
    }

    /// 貼上: a picture on the clipboard put on the picture shown, or text into 搜尋, or a
    /// picture as a new window's when nothing that takes it is shown.
    pub fn paste(self: &Rc<Self>) {
        let clipboard = self.window.clipboard();
        let weak = Rc::downgrade(self);
        clipboard.read_texture_async(gio::Cancellable::NONE, move |texture| {
            let Some(win) = weak.upgrade() else { return };
            let Ok(Some(texture)) = texture else { return };
            let Some(stamp) = texture_pixbuf(&texture) else { return };
            if win.shown.borrow().is_some() && win.item().is_some_and(|item| !item.is_page()) {
                // In the middle of the picture, as large as it is.
                let Some((width, height)) = win.shown.borrow().as_ref().map(|pixbuf| (f64::from(pixbuf.width()), f64::from(pixbuf.height()))) else { return };
                let (w, h) = (f64::from(stamp.width()).min(width), f64::from(stamp.height()).min(height));
                let from = ((width - w) / 2.0, (height - h) / 2.0);
                win.edit(|pixbuf| shapes::paint_picture(pixbuf, &stamp, from, (from.0 + w, from.1 + h)));
            }
        });
    }

    /// 反向選取.
    pub fn invert_selection(&self) {
        let Some(mask) = self.image_mask.borrow().clone() else { return };
        self.set_mask(Some(mask.invert()), Vec::new());
    }

    /// 移除背景: what reaches the edges in colours like theirs made clear.
    pub fn remove_background(self: &Rc<Self>) {
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        let picture = imaging::Rgba::from_pixbuf(&pixbuf);
        let mask = imaging::background(&picture);
        if mask.is_empty() || mask.inside.iter().all(|inside| *inside) {
            return alert::notice(&self.window, "無法移除背景", Some("找不到與主體分開的背景。"));
        }
        self.keep_clear_pixels();
        self.set_mask(None, Vec::new());
        self.edit(|_| Some(imaging::clear(&picture, &mask).to_pixbuf()));
    }

    /// 拷貝主體: the picture's subject, its background taken out, put on the clipboard; the
    /// model that finds it is downloaded first, once the person agrees.
    pub fn copy_subject(self: &Rc<Self>) {
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        if !subject::model_path().exists() {
            return self.download_subject_model();
        }
        let picture = imaging::Rgba::from_pixbuf(&pixbuf);
        let win = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || subject::subject(&picture)).await;
            let Some(win) = win.upgrade() else { return };
            let failure = match found {
                Ok(Ok(Some(subject))) => {
                    return win.window.clipboard().set_texture(&gdk::Texture::for_pixbuf(&subject.to_pixbuf()));
                }
                Ok(Ok(None)) => "找不到主體。".to_string(),
                Ok(Err(err)) => err,
                Err(_) => "分析影像時發生錯誤。".to_string(),
            };
            alert::notice(&win.window, "無法拷貝主體", Some(&failure));
        });
    }

    /// Asks to download the model 拷貝主體 needs, then copies the subject with it.
    fn download_subject_model(self: &Rc<Self>) {
        let alert = adw::AlertDialog::new(
            Some("要下載拷貝主體的模型嗎？"),
            Some(&format!("模型大小為 {}，只需下載一次。可在「設定」關閉拷貝主體。", subject::MODEL_SIZE)),
        );
        alert.add_responses(&[("cancel", "取消"), ("download", "下載")]);
        alert.set_response_appearance("download", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("download"));
        alert.set_close_response("cancel");
        let win = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            if response != "download" {
                return;
            }
            let path = subject::model_path();
            let partial = path.with_extension("part");
            // Written beside it and moved in whole, so a download cut off is not taken for the model.
            let command = "curl -fsSL --create-dirs -o \"$1\" \"$3\" && mv \"$1\" \"$2\"";
            let started = gio::Subprocess::newv(
                &["sh".as_ref(), "-c".as_ref(), command.as_ref(), "sh".as_ref(), partial.as_os_str(), path.as_os_str(), subject::MODEL_URL.as_ref()],
                gio::SubprocessFlags::STDERR_PIPE,
            );
            let process = match started {
                Ok(process) => process,
                Err(err) => {
                    if let Some(win) = win.upgrade() {
                        alert::notice(&win.window, "無法下載模型", Some(err.message()));
                    }
                    return;
                }
            };
            glib::spawn_future_local(async move {
                let result = process.communicate_utf8_future(None).await;
                let Some(win) = win.upgrade() else { return };
                match result {
                    Ok(_) if process.is_successful() => win.copy_subject(),
                    Ok((_, error)) => {
                        let error = error.map(|error| error.trim().to_string()).filter(|error| !error.is_empty());
                        alert::notice(&win.window, "無法下載模型", Some(error.as_deref().unwrap_or("請檢查網路連線後再試一次。")));
                    }
                    Err(err) => alert::notice(&win.window, "無法下載模型", Some(err.message())),
                }
            });
        });
    }

    /// 調整顏色: the picture's exposure, contrast, colour and levels, shown as they are moved
    /// and kept when the panel closes.
    pub fn adjust_color(self: &Rc<Self>) {
        let Some(original) = self.shown.borrow().clone() else { return };
        *self.adjusting.borrow_mut() = Some(original.clone());
        // Moved on a small copy, so the sliders keep up; the whole picture is done at the end.
        let preview = {
            let scale = (1200.0 / f64::from(original.width().max(original.height()))).min(1.0);
            original
                .scale_simple(((f64::from(original.width()) * scale) as i32).max(1), ((f64::from(original.height()) * scale) as i32).max(1), gtk::gdk_pixbuf::InterpType::Bilinear)
                .unwrap_or_else(|| original.clone())
        };
        let small = imaging::Rgba::from_pixbuf(&preview);
        let settings = Rc::new(Cell::new(imaging::Adjust::default()));
        let histogram = gtk::DrawingArea::builder().content_height(80).build();
        let counts = Rc::new(RefCell::new(imaging::histogram(&small)));
        histogram.set_draw_func({
            let counts = counts.clone();
            move |_, cr, width, height| {
                let counts = counts.borrow();
                let most = counts.iter().flat_map(|channel| channel.iter()).copied().max().unwrap_or(1).max(1) as f64;
                let (width, height) = (f64::from(width), f64::from(height));
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.08);
                cr.rectangle(0.0, 0.0, width, height);
                let _ = cr.fill();
                for (channel, (red, green, blue)) in counts.iter().zip([(1.0, 0.2, 0.2), (0.2, 0.8, 0.2), (0.2, 0.4, 1.0)]) {
                    cr.set_source_rgba(red, green, blue, 0.5);
                    cr.move_to(0.0, height);
                    for (value, count) in channel.iter().enumerate() {
                        cr.line_to(value as f64 / 255.0 * width, height - (*count as f64 / most).sqrt() * height);
                    }
                    cr.line_to(width, height);
                    let _ = cr.fill();
                }
            }
        });
        let page = adw::PreferencesPage::new();
        let levels_group = adw::PreferencesGroup::builder().title("色階").build();
        levels_group.add(&histogram);
        let group = adw::PreferencesGroup::new();
        type Field = fn(&mut imaging::Adjust) -> &mut f64;
        let sliders: Vec<(&str, f64, f64, f64, Field)> = vec![
            ("黑點", 0.0, 1.0, 0.0, |a| &mut a.black),
            ("中間調", 0.0, 1.0, 0.5, |a| &mut a.middle),
            ("白點", 0.0, 1.0, 1.0, |a| &mut a.white),
            ("曝光度", -1.0, 1.0, 0.0, |a| &mut a.exposure),
            ("對比", -1.0, 1.0, 0.0, |a| &mut a.contrast),
            ("亮部", -1.0, 1.0, 0.0, |a| &mut a.highlights),
            ("陰影", -1.0, 1.0, 0.0, |a| &mut a.shadows),
            ("飽和度", -1.0, 1.0, 0.0, |a| &mut a.saturation),
            ("色溫", -1.0, 1.0, 0.0, |a| &mut a.temperature),
            ("色調", -1.0, 1.0, 0.0, |a| &mut a.tint),
            ("復古色調", 0.0, 1.0, 0.0, |a| &mut a.sepia),
            ("銳利度", -1.0, 1.0, 0.0, |a| &mut a.sharpness),
        ];
        let scales: Rc<RefCell<Vec<(gtk::Scale, Field)>>> = Rc::default();
        let busy = Rc::new(Cell::new(false));
        let redraw = {
            let (weak, settings, busy, counts, histogram) = (Rc::downgrade(self), settings.clone(), busy.clone(), counts.clone(), histogram.clone());
            Rc::new(move || {
                let Some(win) = weak.upgrade() else { return };
                if busy.get() {
                    return;
                }
                let adjusted = imaging::adjust(&small, &settings.get());
                *counts.borrow_mut() = imaging::histogram(&adjusted);
                histogram.queue_draw();
                win.picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&adjusted.to_pixbuf())));
            })
        };
        for (index, (label, low, high, start, field)) in sliders.into_iter().enumerate() {
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, low, high, 0.01);
            scale.set_value(start);
            scale.set_hexpand(true);
            scale.set_width_request(220);
            let row = adw::ActionRow::builder().title(label).build();
            row.add_suffix(&scale);
            if index < 3 {
                levels_group.add(&row);
            } else {
                group.add(&row);
            }
            let (settings, redraw) = (settings.clone(), redraw.clone());
            scale.connect_value_changed(move |scale| {
                let mut now = settings.get();
                *field(&mut now) = scale.value();
                settings.set(now);
                redraw();
            });
            scales.borrow_mut().push((scale, field));
        }
        let set_all = {
            let (scales, busy, redraw, settings) = (scales.clone(), busy.clone(), redraw.clone(), settings.clone());
            Rc::new(move |to: imaging::Adjust| {
                busy.set(true);
                let mut to_mut = to;
                for (scale, field) in scales.borrow().iter() {
                    scale.set_value(*field(&mut to_mut));
                }
                settings.set(to);
                busy.set(false);
                redraw();
            })
        };
        let auto = gtk::Button::builder().label("自動調整色階").build();
        auto.connect_clicked({
            let (set_all, small, settings) = (set_all.clone(), imaging::Rgba::from_pixbuf(&preview), settings.clone());
            move |_| {
                let (black, white) = imaging::auto_levels(&small);
                set_all(imaging::Adjust { black, white, ..settings.get() });
            }
        });
        let reset = gtk::Button::builder().label("全部重置").build();
        reset.connect_clicked({
            let set_all = set_all.clone();
            move |_| set_all(imaging::Adjust::default())
        });
        let buttons = gtk::Box::builder().spacing(GAP).halign(gtk::Align::End).margin_top(GAP).build();
        buttons.append(&auto);
        buttons.append(&reset);
        group.add(&buttons);
        page.add(&levels_group);
        page.add(&group);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("調整顏色").content_width(420).content_height(640).child(&view).build();
        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |_| {
            let Some(win) = weak.upgrade() else { return };
            let Some(original) = win.adjusting.take() else { return };
            let now = settings.get();
            if now == imaging::Adjust::default() {
                win.picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&original)));
                return;
            }
            win.edit(|_| Some(imaging::adjust(&imaging::Rgba::from_pixbuf(&original), &now).to_pixbuf()));
        });
        dialog.present(Some(&self.window));
    }

    /// 調整大小: the picture's size by pixels, a share of it or a printed size, resampled or
    /// only printed at another resolution; with several chosen, each of them.
    pub fn resize(self: &Rc<Self>) {
        let Some(pixbuf) = self.shown.borrow().clone() else { return };
        let Some(item) = self.item() else { return };
        let (width, height) = (f64::from(pixbuf.width()), f64::from(pixbuf.height()));
        let dpi = item.dpi.get().unwrap_or(72.0);
        const FITS: [(&str, Option<(f64, f64)>); 8] = [
            ("自訂", None),
            ("320 × 240", Some((320.0, 240.0))),
            ("640 × 480", Some((640.0, 480.0))),
            ("800 × 600", Some((800.0, 600.0))),
            ("1024 × 768", Some((1024.0, 768.0))),
            ("1280 × 1024", Some((1280.0, 1024.0))),
            ("1920 × 1080", Some((1920.0, 1080.0))),
            ("3840 × 2160", Some((3840.0, 2160.0))),
        ];
        let fit = adw::ComboRow::builder().title("符合於").model(&gtk::StringList::new(&FITS.map(|(name, _)| name))).build();
        let spin = |value: f64| gtk::SpinButton::builder().adjustment(&gtk::Adjustment::new(value, 0.01, 100000.0, 1.0, 10.0, 0.0)).digits(2).valign(gtk::Align::Center).build();
        let (across, down) = (spin(width), spin(height));
        let across_row = adw::ActionRow::builder().title("寬度").build();
        across_row.add_suffix(&across);
        let down_row = adw::ActionRow::builder().title("高度").build();
        down_row.add_suffix(&down);
        let unit = adw::ComboRow::builder().title("單位").model(&gtk::StringList::new(&imaging::UNITS.map(|(_, name)| name))).build();
        let resolution = gtk::SpinButton::builder().adjustment(&gtk::Adjustment::new(dpi, 1.0, 10000.0, 1.0, 10.0, 0.0)).digits(0).valign(gtk::Align::Center).build();
        let resolution_row = adw::ActionRow::builder().title("解析度").subtitle("像素/英寸").build();
        resolution_row.add_suffix(&resolution);
        let keep = adw::SwitchRow::builder().title("依比例縮放").active(true).build();
        let resample = adw::SwitchRow::builder().title("重新取樣影像").active(true).build();
        let result = adw::ActionRow::builder().title("結果大小").css_classes(["property"]).build();
        let group = adw::PreferencesGroup::new();
        for row in [fit.upcast_ref::<gtk::Widget>(), across_row.upcast_ref(), down_row.upcast_ref(), unit.upcast_ref(), resolution_row.upcast_ref(), keep.upcast_ref(), resample.upcast_ref(), result.upcast_ref()] {
            group.add(row);
        }
        // The size wanted in pixels and the resolution, the fields showing them in the unit chosen.
        let pixels = Rc::new(Cell::new((width, height)));
        let settling = Rc::new(Cell::new(false));
        let unit_of = {
            let unit = unit.clone();
            move || imaging::UNITS[unit.selected() as usize].0
        };
        let show = {
            let (across, down, pixels, settling, unit_of, resolution, result, resample) =
                (across.clone(), down.clone(), pixels.clone(), settling.clone(), unit_of.clone(), resolution.clone(), result.clone(), resample.clone());
            Rc::new(move || {
                settling.set(true);
                let (w, h) = pixels.get();
                let dpi = resolution.value();
                across.set_value(imaging::from_pixels(w, unit_of(), dpi, width));
                down.set_value(imaging::from_pixels(h, unit_of(), dpi, height));
                settling.set(false);
                let (w, h) = if resample.is_active() { (w, h) } else { (width, height) };
                let bytes = (w.round() * h.round() * 4.0) as u64;
                result.set_subtitle(&format!("{} × {} 像素，{}", w.round(), h.round(), glib::format_size(bytes)));
            })
        };
        show();
        for (this, is_width) in [(&across, true), (&down, false)] {
            let (pixels, settling, unit_of, resolution, keep, show, fit) = (pixels.clone(), settling.clone(), unit_of.clone(), resolution.clone(), keep.clone(), show.clone(), fit.clone());
            this.connect_value_changed(move |this| {
                if settling.get() {
                    return;
                }
                let original = if is_width { width } else { height };
                let value = imaging::to_pixels(this.value(), unit_of(), resolution.value(), original);
                let (mut w, mut h) = pixels.get();
                if is_width {
                    w = value;
                    if keep.is_active() {
                        h = value / width * height;
                    }
                } else {
                    h = value;
                    if keep.is_active() {
                        w = value / height * width;
                    }
                }
                pixels.set((w.max(1.0), h.max(1.0)));
                fit.set_selected(0);
                show();
            });
        }
        unit.connect_selected_notify({
            let show = show.clone();
            move |_| show()
        });
        resolution.connect_value_changed({
            let (show, resample, pixels) = (show.clone(), resample.clone(), pixels.clone());
            move |resolution| {
                // Not resampled, the pixels stay and the printed size follows the resolution.
                if !resample.is_active() {
                    pixels.set((width, height));
                }
                let _ = resolution;
                show();
            }
        });
        resample.connect_active_notify({
            let (show, pixels, across, down) = (show.clone(), pixels.clone(), across.clone(), down.clone());
            move |resample| {
                if !resample.is_active() {
                    pixels.set((width, height));
                }
                for field in [&across, &down] {
                    field.set_sensitive(resample.is_active() || true);
                }
                show();
            }
        });
        fit.connect_selected_notify({
            let (pixels, show) = (pixels.clone(), show.clone());
            move |fit| {
                if let Some((w, h)) = FITS[fit.selected() as usize].1 {
                    let scale = (w / width).min(h / height);
                    pixels.set(((width * scale).round().max(1.0), (height * scale).round().max(1.0)));
                    show();
                }
            }
        });
        let alert = adw::AlertDialog::new(Some("調整大小"), None);
        alert.set_extra_child(Some(&group));
        alert.add_responses(&[("cancel", "取消"), ("do", "好")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("do"));
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
            let (w, h) = pixels.get();
            let dpi = resolution.value();
            let resampling = resample.is_active();
            for index in win.chosen_items() {
                if let Some(item) = win.items.borrow().get(index) {
                    item.dpi.set(Some(dpi));
                }
            }
            if !resampling {
                if let Some(item) = win.item() {
                    // Only the resolution it is saved with changes.
                    let before = item.edited.borrow().clone();
                    let now = before.clone().or_else(|| win.shown.borrow().clone());
                    item.undo.borrow_mut().push(before);
                    *item.edited.borrow_mut() = now;
                    win.update_title(&item);
                    win.update_actions();
                }
                return;
            }
            let (scale_x, scale_y) = (w / width, h / height);
            for_each_picture(&win, |pixbuf| {
                let (pw, ph) = ((f64::from(pixbuf.width()) * scale_x).round() as i32, (f64::from(pixbuf.height()) * scale_y).round() as i32);
                pixbuf.scale_simple(pw.max(1), ph.max(1), gtk::gdk_pixbuf::InterpType::Hyper)
            });
        });
    }

    /// 指定描述檔: the colour profile the picture is saved with, from those installed.
    pub fn assign_profile(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let mut profiles: Vec<PathBuf> = ["/usr/share/color/icc", "/usr/local/share/color/icc"]
            .iter()
            .map(PathBuf::from)
            .chain(std::iter::once(glib::user_data_dir().join("icc")))
            .flat_map(|dir| walk_profiles(&dir))
            .collect();
        profiles.sort();
        if profiles.is_empty() {
            return alert::notice(&self.window, "沒有色彩描述檔", Some("/usr/share/color/icc 中沒有 ICC 描述檔。"));
        }
        let names: Vec<String> = profiles.iter().map(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()).collect();
        let dropdown = gtk::DropDown::builder().model(&gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>())).enable_search(true).build();
        if let Some(at) = item.profile.borrow().as_ref().and_then(|now| profiles.iter().position(|path| path == now)) {
            dropdown.set_selected(at as u32);
        }
        let alert = adw::AlertDialog::new(Some("指定描述檔"), None);
        alert.set_extra_child(Some(&dropdown));
        alert.add_responses(&[("cancel", "取消"), ("do", "指定")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
            *item.profile.borrow_mut() = profiles.get(dropdown.selected() as usize).cloned();
            // Saved with it, the picture counts as changed.
            if item.edited.borrow().is_none() {
                let shown = win.shown.borrow().clone();
                item.undo.borrow_mut().push(None);
                *item.edited.borrow_mut() = shown;
            }
            win.update_title(&item);
            win.update_actions();
        });
    }

    /// 影像描述: the words a screen reader says for the picture, kept in the file.
    pub fn describe(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let now = Command::new("exiftool")
            .args(["-s3", "-XMP-dc:Description"])
            .arg(&item.path)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        let weak = Rc::downgrade(self);
        alert::prompt(&self.window, "影像描述", &now, -1, "儲存", move |text| {
            let Some(win) = weak.upgrade() else { return };
            win.keep_version(&item.path);
            let written = Command::new("exiftool")
                .args(["-overwrite_original", "-q"])
                .arg(format!("-XMP-dc:Description={text}"))
                .arg(format!("-EXIF:ImageDescription={text}"))
                .arg(&item.path)
                .output();
            match written {
                Ok(output) if output.status.success() => {}
                Ok(output) => alert::notice(&win.window, "無法儲存", Some(String::from_utf8_lossy(&output.stderr).trim())),
                Err(err) => alert::notice(&win.window, "無法儲存", Some(&format!("exiftool: {err}"))),
            }
        });
    }

    /// The PDF's pages chosen in the sidebar, counting from 1.
    pub fn chosen_pages(&self) -> Vec<u32> {
        let items = self.items.borrow();
        let path = items.get(self.current.get()).map(|item| item.path.clone());
        self.chosen_items().iter().filter_map(|&index| items.get(index)).filter(|item| Some(&item.path) == path.as_ref()).filter_map(|item| item.page).collect()
    }

    /// Writes the PDF shown with `change`, its annotations saved into it first, then reads it
    /// again at page `then`, counting from 1, or where it was.
    pub fn change_pages(self: &Rc<Self>, change: impl FnOnce(&Path, Option<String>) -> Result<(), String> + Send + 'static, then: Option<u32>) {
        let Some(item) = self.item() else { return };
        if !pdfdoc::has_qpdf() {
            return alert::notice(&self.window, "需要 qpdf", Some("變更 PDF 的頁面需要 qpdf。"));
        }
        if let Err(err) = self.flush_pdf() {
            return alert::notice(&self.window, "無法儲存", Some(&err));
        }
        self.keep_version(&item.path);
        let path = item.path.clone();
        let password = self.passwords.borrow().get(&path).cloned();
        let page = then.or(item.page).unwrap_or(1);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking({
                let path = path.clone();
                move || change(&path, password)
            })
            .await
            .unwrap_or_else(|_| Err(String::new()));
            let Some(win) = weak.upgrade() else { return };
            match result {
                Ok(()) => win.reload(&path, page),
                Err(err) => alert::notice(&win.window, "無法寫入", Some(&err)),
            }
        });
    }

    /// Reads the file at `path` again, keeping the rest, and shows its page `page`.
    pub fn reload(self: &Rc<Self>, path: &Path, page: u32) {
        *self.pdf.borrow_mut() = None;
        let mut failed = Vec::new();
        let fresh = match self.items_of(&[path.to_owned()], &mut failed) {
            Ok(fresh) => fresh,
            Err(_) => return,
        };
        let mut items: Vec<Rc<Item>> = Vec::new();
        let mut put = false;
        for item in self.items.borrow().iter() {
            if item.path == path {
                if !put {
                    items.extend(fresh.iter().cloned());
                    put = true;
                }
            } else {
                items.push(item.clone());
            }
        }
        let first = items.iter().position(|item| item.path == path).unwrap_or(0);
        let count = fresh.len().max(1);
        self.set_items(items, first + (page as usize).clamp(1, count) - 1);
    }

    /// 刪除此頁: the pages chosen taken out of the PDF.
    pub fn delete_pages(self: &Rc<Self>) {
        let Some(item) = self.item() else { return };
        let pages = self.chosen_pages();
        let count = self.pages_in(&item.path);
        if pages.len() >= count {
            return;
        }
        let title = match pages.as_slice() {
            [page] => format!("刪除第 {page} 頁？"),
            _ => format!("刪除 {} 頁？", pages.len()),
        };
        let weak = Rc::downgrade(self);
        alert::confirm(&self.window, &title, None, "刪除", move || {
            let Some(win) = weak.upgrade() else { return };
            let plan: Vec<u32> = (1..=count as u32).filter(|page| !pages.contains(page)).collect();
            let then = pages.first().copied().unwrap_or(1).min(plan.len() as u32);
            win.rewrite(plan.into_iter().map(|page| (None, page)).collect(), then);
        });
    }

    /// Writes the PDF shown again with the pages `plan` names: its own, or a page of another file.
    fn rewrite(self: &Rc<Self>, plan: Vec<(Option<PathBuf>, u32)>, then: u32) {
        let Some(item) = self.item() else { return };
        let passwords = self.passwords.borrow().clone();
        let path = item.path.clone();
        let plan: Vec<(PathBuf, Option<String>, u32)> = plan
            .into_iter()
            .map(|(file, page)| {
                let file = file.unwrap_or_else(|| path.clone());
                let password = passwords.get(&file).cloned();
                (file, password, page)
            })
            .collect();
        // Without qpdf, pages are still put in order by poppler's own tools, for PDFs with no password.
        if !pdfdoc::has_qpdf() && plan.iter().all(|(_, password, _)| password.is_none()) {
            if let Err(err) = self.flush_pdf() {
                return alert::notice(&self.window, "無法儲存", Some(&err));
            }
            self.keep_version(&path);
            let simple: Vec<(PathBuf, u32)> = plan.into_iter().map(|(file, _, page)| (file, page)).collect();
            return match rebuild_pdf(&path, &simple) {
                Ok(()) => self.reload(&path, then),
                Err(err) => alert::notice(&self.window, "無法寫入", Some(&err)),
            };
        }
        self.change_pages(move |path, _| pdfdoc::rebuild(path, &plan), Some(then));
    }

    /// Moves the page shown `by` places.
    pub fn move_page(self: &Rc<Self>, by: i32) {
        let Some(page) = self.item().and_then(|item| item.page) else { return };
        let to = page as i64 + i64::from(by);
        let Some(first) = self.items.borrow().iter().position(|other| self.item().is_some_and(|item| item.path == other.path)) else { return };
        if to >= 1 {
            self.move_page_to(first + page as usize - 1, first + to as usize - 1);
        }
    }

    /// Moves the sidebar's row `from` to where row `to` is, in the PDF itself.
    pub fn move_page_to(self: &Rc<Self>, from: usize, to: usize) {
        let items = self.items.borrow().clone();
        let (Some(moved), Some(target)) = (items.get(from), items.get(to)) else { return };
        let (Some(from_page), Some(to_page)) = (moved.page, target.page) else { return };
        if moved.path != target.path || from == to || !is_pdf(&moved.path) {
            return;
        }
        self.select(from);
        let count = self.pages_in(&moved.path) as u32;
        let mut plan: Vec<u32> = (1..=count).collect();
        let page = plan.remove(from_page as usize - 1);
        plan.insert(to_page as usize - 1, page);
        self.rewrite(plan.into_iter().map(|page| (None, page)).collect(), to_page);
    }

    /// 插入檔案中的頁面: PDFs' pages, and pictures as pages, after the page shown.
    pub fn insert_from_file(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("圖片與 PDF"));
        for mime in MIME_TYPES {
            filter.add_mime_type(mime);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("插入檔案中的頁面").filters(&filters).build();
        let weak = Rc::downgrade(self);
        dialog.open_multiple(Some(&self.window), gio::Cancellable::NONE, move |chosen| {
            let (Some(win), Ok(files)) = (weak.upgrade(), chosen) else { return };
            let paths: Vec<PathBuf> = files.iter::<gio::File>().filter_map(Result::ok).filter_map(|file| file.path()).collect();
            let at = win.current.get();
            win.insert_files(paths, at);
        });
    }

    /// Puts the pages of `paths` after the sidebar's row `at`, pictures made pages first.
    pub fn insert_files(self: &Rc<Self>, paths: Vec<PathBuf>, at: usize) {
        let Some(after) = self.items.borrow().get(at).and_then(|item| item.page) else { return };
        let Some(path) = self.item().map(|item| item.path.clone()) else { return };
        let count = self.pages_in(&path) as u32;
        let mut plan: Vec<(Option<PathBuf>, u32)> = (1..=count).map(|page| (None, page)).collect();
        let mut inserted = Vec::new();
        for file in paths {
            let file = if opens_as_pdf(&file) {
                file
            } else {
                // A picture becomes a page of its own, at 72 pixels to the inch.
                let made = scratch_dir().join(format!("insert-{}.pdf", glib::monotonic_time()));
                let _ = std::fs::create_dir_all(scratch_dir());
                match load_picture(&file).and_then(|pixbuf| export_pdf(&pixbuf, &made)) {
                    Ok(()) => made,
                    Err(err) => return alert::notice(&self.window, "無法插入", Some(&err)),
                }
            };
            match page_count(&file) {
                Ok(pages) => inserted.extend((1..=pages).map(|page| (Some(file.clone()), page))),
                Err(err) => return alert::notice(&self.window, "無法插入", Some(&err)),
            }
        }
        let place = after as usize;
        plan.splice(place..place, inserted);
        self.rewrite(plan, after + 1);
    }

    /// 插入空白頁面: a page the size of the one shown, after it.
    pub fn insert_blank(self: &Rc<Self>) {
        let (Some(pdf), Some(page)) = (self.pdf.borrow().clone(), self.item().and_then(|item| item.page)) else { return };
        let made = scratch_dir().join(format!("blank-{}.pdf", glib::monotonic_time()));
        let _ = std::fs::create_dir_all(scratch_dir());
        if let Err(err) = pdfdoc::blank_page(&made, pdf.sizes[page as usize - 1]) {
            return alert::notice(&self.window, "無法插入", Some(&err));
        }
        self.insert_files(vec![made], self.current.get());
    }

    /// 插入掃描器中的頁面.
    pub fn insert_scanner(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.scan(move |path| {
            if let Some(win) = weak.upgrade() {
                let at = win.current.get();
                win.insert_files(vec![path], at);
            }
        });
    }
}

/// Changes each picture chosen in the sidebar with `change`, the one shown last so it is shown
/// changed.
fn for_each_picture(win: &Rc<Win>, change: impl Fn(&Pixbuf) -> Option<Pixbuf>) {
    let current = win.current.get();
    for index in win.chosen_items().into_iter().filter(|&index| index != current) {
        let Some(item) = win.items.borrow().get(index).cloned().filter(|item| !item.is_page()) else { continue };
        let Some(pixbuf) = item.edited.borrow().clone().or_else(|| load_picture(&item.path).ok()) else { continue };
        if let Some(changed) = change(&pixbuf) {
            let before = item.edited.borrow_mut().replace(changed.clone());
            item.undo.borrow_mut().push(before);
            if let (Some(picture), Some(small)) = (win.thumbnails.borrow().get(index), thumbnail_of(&changed)) {
                picture.set_paintable(Some(&gdk::Texture::for_pixbuf(&small)));
            }
        }
    }
    win.edit(change);
}

/// Whether a mask is a box, which cutting to needs no clear pixels.
fn is_box(mask: &imaging::Mask) -> bool {
    let Some((left, top, width, height)) = mask.bounds() else { return false };
    (top..top + height).all(|y| (left..left + width).all(|x| mask.inside[y * mask.width + x]))
}

pub fn texture_pixbuf(texture: &gdk::Texture) -> Option<Pixbuf> {
    let bytes = texture.save_to_png_bytes();
    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    loader.write(&bytes).ok()?;
    loader.close().ok()?;
    loader.pixbuf()
}

fn walk_profiles(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk_profiles(&path)
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("icc") || e.eq_ignore_ascii_case("icm")) {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}
