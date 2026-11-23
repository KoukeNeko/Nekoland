//! How the window shows what it has: the sidebar's lists, a PDF a page or two at a time, going
//! to a page and back, the contact sheet, the slideshow, the magnifier, and the colours behind.

use super::*;

impl Win {
    pub fn toggle_sidebar(self: &Rc<Self>) {
        if self.sidebar_mode.get() == Sidebar::ContactSheet {
            return self.set_sidebar(Sidebar::Thumbnails);
        }
        let show = !self.split.shows_sidebar();
        if show && self.sidebar_mode.get() == Sidebar::Hidden {
            return self.set_sidebar(Sidebar::Thumbnails);
        }
        self.split.set_show_sidebar(show);
    }

    pub fn set_sidebar(self: &Rc<Self>, mode: Sidebar) {
        let leaving_sheet = self.sidebar_mode.replace(mode) == Sidebar::ContactSheet;
        self.set_action_state("sidebar", mode.key().to_variant());
        match mode {
            Sidebar::Hidden => self.split.set_show_sidebar(false),
            Sidebar::ContactSheet => {
                self.split.set_show_sidebar(false);
                self.fill_sheet();
                self.stack.set_visible_child_name("sheet");
            }
            _ => {
                self.sidebar_stack.set_visible_child_name(mode.key());
                self.split.set_show_sidebar(true);
            }
        }
        if leaving_sheet && mode != Sidebar::ContactSheet {
            self.select(self.current.get());
        }
    }

    /// Fills the lists the sidebar shows besides the thumbnails, for the PDF shown.
    pub fn fill_sidebar(self: &Rc<Self>) {
        self.fill_contents();
        self.fill_notes();
        self.fill_bookmarks();
    }

    fn fill_contents(self: &Rc<Self>) {
        while let Some(row) = self.contents.row_at_index(0) {
            self.contents.remove(&row);
        }
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        fn add(win: &Rc<Win>, entries: &[pdfdoc::Entry], depth: i32) {
            for entry in entries {
                let label = gtk::Label::builder()
                    .label(&entry.title)
                    .xalign(0.0)
                    .ellipsize(pango::EllipsizeMode::End)
                    .margin_start(GAP * 2 * depth)
                    .hexpand(true)
                    .build();
                let page = gtk::Label::builder().css_classes(["dim-label", "numeric"]).build();
                if let Some(index) = entry.page {
                    if let Some(pdf) = win.pdf.borrow().as_ref() {
                        page.set_label(&pdfdoc::page_label(&pdf.document(), index, win.prefs.borrow().logical_numbers));
                    }
                }
                let line = gtk::Box::builder().spacing(GAP).build();
                line.append(&label);
                line.append(&page);
                let row = gtk::ListBoxRow::builder().child(&line).build();
                let (target, top) = (entry.page, entry.top);
                let weak = Rc::downgrade(win);
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| {
                    if let (Some(win), Some(index)) = (weak.upgrade(), target) {
                        win.remember_place();
                        win.go_page(index, top);
                    }
                });
                row.add_controller(click);
                win.contents.append(&row);
                add(win, &entry.children, depth + 1);
            }
        }
        add(self, &pdfdoc::outline(&pdf.document()), 0);
    }

    /// Every highlight and note in the PDF, page by page.
    pub fn fill_notes(self: &Rc<Self>) {
        while let Some(row) = self.notes.row_at_index(0) {
            self.notes.remove(&row);
        }
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let document = pdf.document();
        for index in 0..pdf.sizes.len() {
            let Some(page) = document.page(index as i32) else { continue };
            for annotation in annots::on_page(&page) {
                let text = if annotation.kind.marks_text() {
                    let mut area = rectangle(annotation.place.0, annotation.place.1, annotation.place.2, annotation.place.3);
                    page.selected_text(poppler::SelectionStyle::Glyph, &mut area).map(|text| text.to_string()).unwrap_or_default()
                } else {
                    annotation.contents.clone()
                };
                let title = gtk::Label::builder()
                    .label(format!("{} · 第 {} 頁", annotation.kind.label(), pdfdoc::page_label(&document, index, self.prefs.borrow().logical_numbers)))
                    .xalign(0.0)
                    .css_classes(["caption", "dim-label"])
                    .build();
                let body = gtk::Label::builder()
                    .label(text.trim())
                    .xalign(0.0)
                    .wrap(true)
                    .lines(3)
                    .ellipsize(pango::EllipsizeMode::End)
                    .build();
                let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_top(GAP / 2).margin_bottom(GAP / 2).build();
                if let Some((red, green, blue)) = annotation.color {
                    let swatch = gtk::DrawingArea::builder().content_height(3).build();
                    swatch.set_draw_func(move |_, cr, width, height| {
                        cr.set_source_rgb(red, green, blue);
                        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
                        let _ = cr.fill();
                    });
                    column.append(&swatch);
                }
                column.append(&title);
                if !text.trim().is_empty() {
                    column.append(&body);
                }
                let row = gtk::ListBoxRow::builder().child(&column).build();
                let weak = Rc::downgrade(self);
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| {
                    let Some(win) = weak.upgrade() else { return };
                    win.remember_place();
                    let top = annotation.place.1;
                    win.go_page(index, Some(top));
                    *win.chosen.borrow_mut() = Some((index, annotation.clone()));
                    win.redraw_pages();
                    win.update_actions();
                });
                row.add_controller(click);
                self.notes.append(&row);
            }
        }
    }

    fn fill_bookmarks(self: &Rc<Self>) {
        while let Some(row) = self.bookmarks.row_at_index(0) {
            self.bookmarks.remove(&row);
        }
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let marks = self.prefs.borrow().bookmarks.get(&pdf.path).cloned().unwrap_or_default();
        for (page, name) in marks {
            let label = gtk::Label::builder().label(&name).xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).build();
            let remove = gtk::Button::builder().icon_name("list-remove-symbolic").tooltip_text("移除書籤").css_classes(["flat"]).build();
            let line = gtk::Box::builder().spacing(GAP).build();
            line.append(&label);
            line.append(&remove);
            let row = gtk::ListBoxRow::builder().child(&line).build();
            let weak = Rc::downgrade(self);
            let click = gtk::GestureClick::new();
            click.connect_released(move |_, _, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.remember_place();
                    win.go_page(page, None);
                }
            });
            label.add_controller(click);
            let (weak, path) = (Rc::downgrade(self), pdf.path.clone());
            remove.connect_clicked(move |_| {
                let Some(win) = weak.upgrade() else { return };
                {
                    let mut prefs = win.prefs.borrow_mut();
                    if let Some(marks) = prefs.bookmarks.get_mut(&path) {
                        marks.retain(|(kept, _)| *kept != page);
                    }
                    prefs.save();
                }
                win.fill_bookmarks();
            });
            self.bookmarks.append(&row);
        }
    }

    /// 加入書籤: the page shown, named as it is numbered.
    pub fn add_bookmark(self: &Rc<Self>) {
        let (Some(pdf), Some(page)) = (self.pdf.borrow().clone(), self.item().and_then(|item| item.page)) else { return };
        let index = page as usize - 1;
        let name = format!("第 {} 頁", pdfdoc::page_label(&pdf.document(), index, self.prefs.borrow().logical_numbers));
        let weak = Rc::downgrade(self);
        alert::prompt(&self.window, "加入書籤", &name, -1, "加入", move |name| {
            let Some(win) = weak.upgrade() else { return };
            {
                let mut prefs = win.prefs.borrow_mut();
                let marks = prefs.bookmarks.entry(pdf.path.clone()).or_default();
                marks.retain(|(kept, _)| *kept != index);
                marks.push((index, name));
                marks.sort_by_key(|(page, _)| *page);
                prefs.save();
            }
            win.fill_bookmarks();
            win.set_sidebar(Sidebar::Bookmarks);
        });
    }

    /// The contact sheet: every item, larger, in a grid instead of the item shown.
    fn fill_sheet(self: &Rc<Self>) {
        while let Some(child) = self.sheet.child_at_index(0) {
            self.sheet.remove(&child);
        }
        for (index, thumbnail) in self.thumbnails.borrow().iter().enumerate() {
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .width_request(THUMB_SIZE)
                .height_request(THUMB_SIZE)
                .build();
            picture.set_paintable(thumbnail.paintable().as_ref());
            let name = self.items.borrow().get(index).map(|item| match item.page {
                Some(page) => page.to_string(),
                None => item.name(),
            });
            let label = gtk::Label::builder().label(name.unwrap_or_default()).ellipsize(pango::EllipsizeMode::Middle).css_classes(["caption"]).build();
            let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(GAP).build();
            column.append(&picture);
            column.append(&label);
            self.sheet.insert(&column, -1);
        }
        if let Some(child) = self.sheet.child_at_index(self.current.get() as i32) {
            self.sheet.select_child(&child);
        }
    }

    /// Lays out the PDF's pages in rows, as its layout has them.
    pub fn lay_out_pages(&self, pdf: &Pdf) {
        while let Some(row) = self.pdf_column.first_child() {
            if let Some(row) = row.downcast_ref::<gtk::Box>() {
                while let Some(page) = row.first_child() {
                    row.remove(&page);
                }
            }
            self.pdf_column.remove(&row);
        }
        for row in pdf.rows() {
            let line = gtk::Box::builder().spacing(EDGE).halign(gtk::Align::Center).build();
            for index in row {
                line.append(&pdf.pages[index]);
            }
            self.pdf_column.append(&line);
        }
        pdf.undrawn.borrow_mut().clear();
        pdf.pages.iter().for_each(|page| page.queue_draw());
    }

    pub fn set_layout(self: &Rc<Self>, layout: Layout) {
        self.layout.set(layout);
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let page = self.item().and_then(|item| item.page).map_or(0, |page| page as usize - 1);
        pdf.layout.set(layout);
        pdf.spread.set(pdf.spread_of(page));
        self.lay_out_pages(&pdf);
        pdf.scale.set(0.0);
        self.apply_zoom();
        self.go_page(page, None);
    }

    /// A page or two at a time, shows the spread page `index` is in.
    pub fn show_spread(&self, pdf: &Pdf, index: usize) {
        if pdf.layout.get() == Layout::Continuous || pdf.spread.get() == pdf.spread_of(index) {
            return;
        }
        pdf.spread.set(pdf.spread_of(index));
        self.lay_out_pages(pdf);
        pdf.scale.set(0.0);
        self.apply_zoom();
    }

    /// Shows page `index` of the PDF shown, `top` points down it when given or just above it.
    pub fn go_page(&self, index: usize, top: Option<f64>) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        if index >= pdf.sizes.len() {
            return;
        }
        self.show_spread(&pdf, index);
        let place = match top {
            Some(top) => (index, top / pdf.sizes[index].1, -f64::from(EDGE)),
            None => (index, 0.0, -f64::from(EDGE)),
        };
        self.pending.set(Some(place));
        self.apply_pending();
        self.list_page(&pdf, index);
        if let Some(row) = self.item_of_page(&pdf, index).and_then(|row| self.list.row_at_index(row as i32)) {
            // The row is scrolled to once the list is laid out.
            let row = row.clone();
            glib::idle_add_local_once(move || {
                row.grab_focus();
            });
        }
        let mut prefs = self.prefs.borrow_mut();
        if prefs.last_page.get(&pdf.path) != Some(&index) {
            prefs.last_page.insert(pdf.path.clone(), index);
            prefs.save();
        }
    }

    /// Keeps the place shown for 上一步, before going somewhere else.
    pub fn remember_place(&self) {
        let down = self.pdf.borrow().as_ref().map_or(0.0, |pdf| self.scrolled_to(pdf).1);
        self.back.borrow_mut().push((self.current.get(), down));
        self.forward.borrow_mut().clear();
        self.update_actions();
    }

    fn go_to_place(self: &Rc<Self>, (index, down): (usize, f64)) {
        self.select(index);
        if let (Some(pdf), Some(page)) = (self.pdf.borrow().clone(), self.item().and_then(|item| item.page)) {
            let page = page as usize - 1;
            self.pending.set(Some((page, down, 0.0)));
            self.apply_pending();
            let _ = pdf;
        }
        self.update_actions();
    }

    pub fn go_back(self: &Rc<Self>) {
        let Some(place) = self.back.borrow_mut().pop() else { return };
        let down = self.pdf.borrow().as_ref().map_or(0.0, |pdf| self.scrolled_to(pdf).1);
        self.forward.borrow_mut().push((self.current.get(), down));
        self.go_to_place(place);
    }

    pub fn go_forward(self: &Rc<Self>) {
        let Some(place) = self.forward.borrow_mut().pop() else { return };
        let down = self.pdf.borrow().as_ref().map_or(0.0, |pdf| self.scrolled_to(pdf).1);
        self.back.borrow_mut().push((self.current.get(), down));
        self.go_to_place(place);
    }

    /// The first or last page of the PDF shown.
    pub fn go_to_end(&self, last: bool) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        self.remember_place();
        self.go_page(if last { pdf.sizes.len() - 1 } else { 0 }, None);
    }

    /// 前往頁面: a page by its number, or its label when 設定 says so.
    pub fn ask_page(self: &Rc<Self>) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let current = self.item().and_then(|item| item.page).map_or(0, |page| page as usize - 1);
        let logical = self.prefs.borrow().logical_numbers;
        let label = pdfdoc::page_label(&pdf.document(), current, logical);
        let weak = Rc::downgrade(self);
        let title = format!("前往頁面（共 {} 頁）", pdf.sizes.len());
        alert::prompt(&self.window, &title, &label, -1, "前往", move |wanted| {
            let Some(win) = weak.upgrade() else { return };
            match pdfdoc::page_named(&pdf.document(), &wanted, logical) {
                Some(index) => {
                    win.remember_place();
                    win.go_page(index, None);
                }
                None => alert::notice(&win.window, "找不到這一頁", Some(&format!("沒有「{wanted}」這一頁。"))),
            }
        });
    }

    /// The first item of the next or previous file.
    pub fn step_document(self: &Rc<Self>, by: i32) {
        let items = self.items.borrow().clone();
        let Some(here) = items.get(self.current.get()).map(|item| item.path.clone()) else { return };
        let mut starts: Vec<(usize, PathBuf)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if starts.last().is_none_or(|(_, path)| *path != item.path) {
                starts.push((index, item.path.clone()));
            }
        }
        let Some(at) = starts.iter().position(|(_, path)| *path == here) else { return };
        let next = (at as i32 + by).rem_euclid(starts.len() as i32) as usize;
        self.remember_place();
        self.select(starts[next].0);
    }

    /// A page or two at a time, the wheel turned past the end of the view goes on to the next.
    pub fn scrolled_past(self: &Rc<Self>, dy: f64) -> glib::Propagation {
        let Some(pdf) = self.pdf.borrow().clone().filter(|pdf| pdf.layout.get() != Layout::Continuous) else {
            return glib::Propagation::Proceed;
        };
        if self.stack.visible_child_name().as_deref() != Some("pdf") {
            return glib::Propagation::Proceed;
        }
        let adjustment = self.pdf_scroll.vadjustment();
        let at_end = adjustment.value() + adjustment.page_size() >= adjustment.upper() - 1.0;
        let at_start = adjustment.value() <= adjustment.lower() + 1.0;
        if (dy > 0.0 && at_end) || (dy < 0.0 && at_start) {
            let step = if pdf.layout.get() == Layout::Two { 2 } else { 1 };
            let next = pdf.spread.get() as i64 + if dy > 0.0 { step } else { -step };
            if (0..pdf.sizes.len() as i64).contains(&next) {
                self.go_page(next as usize, None);
                if dy < 0.0 {
                    // Back a page, its foot is shown.
                    let adjustment = adjustment.clone();
                    glib::idle_add_local_once(move || adjustment.set_value(adjustment.upper()));
                }
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    /// 縮放至選取範圍: the part chosen fills the view.
    pub fn zoom_selection(&self) {
        if let Some(pdf) = self.pdf.borrow().clone().filter(|_| self.item().is_some_and(|item| item.is_page())) {
            let place = self.area.get().or_else(|| {
                let selection = self.selection.get()?;
                Some((selection.from.0, (0.0, selection.from.2.min(selection.to.2), pdf.sizes[selection.from.0].0, selection.from.2.max(selection.to.2) + 12.0)))
            });
            let Some((index, (x1, y1, x2, y2))) = place else { return };
            let across = self.pdf_scroll.hadjustment().page_size() - f64::from(2 * EDGE);
            let down = self.pdf_scroll.vadjustment().page_size() - f64::from(2 * EDGE);
            let scale = (across / (x2 - x1).max(1.0)).min(down / (y2 - y1).max(1.0));
            self.set_zoom(Zoom::Scale(scale / self.item().map_or(1.0, |item| self.actual_size(&item))));
            self.pending.set(Some((index, y1 / pdf.sizes[index].1, 0.0)));
            self.apply_pending();
            let scale = pdf.scale.get();
            let hadjustment = self.pdf_scroll.hadjustment();
            glib::idle_add_local_once(move || hadjustment.set_value(x1 * scale));
            return;
        }
        let Some((x1, y1, x2, y2)) = self.image_mask.borrow().as_ref().and_then(imaging::Mask::bounds) else { return };
        let across = self.scroll.hadjustment().page_size() - f64::from(2 * EDGE);
        let down = self.scroll.vadjustment().page_size() - f64::from(2 * EDGE);
        let scale = (across / (x2 - x1).max(1) as f64).min(down / (y2 - y1).max(1) as f64);
        self.set_zoom(Zoom::Scale(scale));
        let (hadjustment, vadjustment) = (self.scroll.hadjustment(), self.scroll.vadjustment());
        glib::idle_add_local_once(move || {
            hadjustment.set_value(x1 as f64 * scale);
            vadjustment.set_value(y1 as f64 * scale);
        });
    }

    /// The colour behind the pages and pictures, as 設定 has it.
    pub fn apply_background(&self) {
        let css = match &self.prefs.borrow().background {
            Some(colour) => format!(".pv-canvas {{ background-color: {colour}; }}"),
            None => String::new(),
        };
        self.background_css.load_from_string(&css);
    }

    pub fn set_dark_pdf(&self, on: bool) {
        self.dark_pdf.set(on);
        self.redraw_pages();
    }

    pub fn set_checker(&self, on: bool) {
        self.checker.set(on);
        if on {
            self.picture.add_css_class("pv-checker");
        } else {
            self.picture.remove_css_class("pv-checker");
        }
    }

    pub fn set_loupe(&self, on: bool) {
        self.loupe.set(on);
        self.loupe_layer.queue_draw();
    }

    /// Draws the magnifier: what is under the pointer at three times its size, in a circle.
    fn draw_loupe(&self, cr: &gtk::cairo::Context) {
        const RADIUS: f64 = 90.0;
        const POWER: f64 = 3.0;
        let (Some((x, y)), true) = (self.loupe_at.get(), self.loupe.get()) else { return };
        cr.arc(x, y, RADIUS, 0.0, std::f64::consts::TAU);
        let _ = cr.save();
        cr.clip_preserve();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.fill_preserve();
        let stack = self.stack.upcast_ref::<gtk::Widget>();
        if self.stack.visible_child_name().as_deref() == Some("pdf") {
            let point = stack.compute_point(&self.pdf_column, &gtk::graphene::Point::new(x as f32, y as f32));
            let hit = point.and_then(|point| self.hit_loose(f64::from(point.x()), f64::from(point.y())));
            if let (Some((index, px, py)), Some(pdf)) = (hit, self.pdf.borrow().clone()) {
                if let Some(page) = pdf.document().page(index as i32) {
                    let scale = pdf.scale.get() * POWER;
                    cr.translate(x - px * scale, y - py * scale);
                    cr.scale(scale, scale);
                    let (width, height) = pdf.sizes[index];
                    cr.rectangle(0.0, 0.0, width, height);
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.fill();
                    page.render(cr);
                }
            }
        } else if let Some(pixbuf) = self.shown.borrow().clone() {
            if let Some(point) = stack.compute_point(&self.picture, &gtk::graphene::Point::new(x as f32, y as f32)) {
                let scale = self.scale() * POWER;
                let (px, py) = (f64::from(point.x()) / self.scale(), f64::from(point.y()) / self.scale());
                cr.translate(x - px * scale, y - py * scale);
                cr.scale(scale, scale);
                cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
                cr.source().set_filter(gtk::cairo::Filter::Nearest);
                let _ = cr.paint();
            }
        }
        let _ = cr.restore();
        cr.arc(x, y, RADIUS, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
    }

    /// The slideshow: each item filling a screen of its own, going on with the arrows or by
    /// itself.
    pub fn slideshow(self: &Rc<Self>) {
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).hexpand(true).vexpand(true).build();
        let button = |icon: &str, tip: &str| gtk::Button::builder().icon_name(icon).tooltip_text(tip).css_classes(["circular", "osd"]).build();
        let previous = button("media-skip-backward-symbolic", "上一項");
        let play = button("media-playback-start-symbolic", "播放");
        let next = button("media-skip-forward-symbolic", "下一項");
        let close = button("window-close-symbolic", "結束幻燈片秀");
        let controls = gtk::Box::builder().spacing(GAP).halign(gtk::Align::Center).valign(gtk::Align::End).margin_bottom(EDGE * 2).build();
        for button in [&previous, &play, &next, &close] {
            controls.append(button);
        }
        let revealer = gtk::Revealer::builder().child(&controls).transition_type(gtk::RevealerTransitionType::Crossfade).reveal_child(true).valign(gtk::Align::End).build();
        let overlay = gtk::Overlay::builder().child(&picture).build();
        overlay.add_overlay(&revealer);
        let window = gtk::Window::builder().child(&overlay).css_classes(["pv-slideshow"]).transient_for(&self.window).build();
        let at = Rc::new(Cell::new(self.current.get()));
        let density = f64::from(self.window.scale_factor());
        let show = {
            let (weak, picture, at) = (Rc::downgrade(self), picture.clone(), at.clone());
            move || {
                let Some(win) = weak.upgrade() else { return };
                let Some(item) = win.items.borrow().get(at.get()).cloned() else { return };
                let texture = match item.page {
                    Some(page) => {
                        let document = win.pdf.borrow().as_ref().filter(|pdf| pdf.path == item.path).map(|pdf| pdf.document());
                        let document = document.or_else(|| open_pdf(&item.path, win.passwords.borrow().get(&item.path).map(String::as_str)).ok());
                        document.and_then(|document| document.page(page as i32 - 1)).and_then(|page| {
                            let (width, height) = page.size();
                            let screen = picture.height().max(720) as f64 * density;
                            page_texture(&page, screen / height.max(width * 9.0 / 16.0))
                        })
                    }
                    None => match item.edited.borrow().clone() {
                        Some(pixbuf) => Some(gdk::Texture::for_pixbuf(&pixbuf)),
                        None => {
                            let (picture, at, index, path) = (picture.clone(), at.clone(), at.get(), item.path.clone());
                            glib::spawn_future_local(async move {
                                let loaded = read_off_thread(move || load_picture(&path)).await;
                                // The slideshow has gone on to another picture meanwhile.
                                if at.get() == index {
                                    picture.set_paintable(loaded.ok().map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf)).as_ref());
                                }
                            });
                            return;
                        }
                    },
                };
                picture.set_paintable(texture.as_ref());
            }
        };
        let show = Rc::new(show);
        let step = {
            let (weak, at, show) = (Rc::downgrade(self), at.clone(), show.clone());
            Rc::new(move |by: i64| {
                let Some(win) = weak.upgrade() else { return };
                let count = win.items.borrow().len() as i64;
                at.set((at.get() as i64 + by).rem_euclid(count.max(1)) as usize);
                show();
            })
        };
        let toggle_play = {
            let (weak, step, play) = (Rc::downgrade(self), step.clone(), play.clone());
            Rc::new(move || {
                let Some(win) = weak.upgrade() else { return };
                if let Some(timer) = win.playing.borrow_mut().take() {
                    timer.remove();
                    play.set_icon_name("media-playback-start-symbolic");
                    play.set_tooltip_text(Some("播放"));
                    return;
                }
                let step = step.clone();
                let timer = glib::timeout_add_seconds_local(4, move || {
                    step(1);
                    glib::ControlFlow::Continue
                });
                *win.playing.borrow_mut() = Some(timer);
                play.set_icon_name("media-playback-pause-symbolic");
                play.set_tooltip_text(Some("暫停"));
            })
        };
        previous.connect_clicked({
            let step = step.clone();
            move |_| step(-1)
        });
        next.connect_clicked({
            let step = step.clone();
            move |_| step(1)
        });
        play.connect_clicked({
            let toggle_play = toggle_play.clone();
            move |_| toggle_play()
        });
        close.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.end_slideshow();
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let (weak, step, toggle_play) = (Rc::downgrade(self), step.clone(), toggle_play.clone());
            move |_, key, _, _| {
                let Some(win) = weak.upgrade() else { return glib::Propagation::Proceed };
                match key {
                    gdk::Key::Right | gdk::Key::Down | gdk::Key::Page_Down => step(1),
                    gdk::Key::Left | gdk::Key::Up | gdk::Key::Page_Up => step(-1),
                    gdk::Key::space => toggle_play(),
                    gdk::Key::Escape => win.end_slideshow(),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        window.add_controller(keys);
        // The controls show while the pointer moves, and go after it stops.
        let motion = gtk::EventControllerMotion::new();
        let hide: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        motion.connect_motion(move |_, _, _| {
            revealer.set_reveal_child(true);
            if let Some(timer) = hide.borrow_mut().take() {
                timer.remove();
            }
            let (revealer, hide_again) = (revealer.clone(), hide.clone());
            *hide.borrow_mut() = Some(glib::timeout_add_seconds_local_once(2, move || {
                revealer.set_reveal_child(false);
                hide_again.borrow_mut().take();
            }));
        });
        overlay.add_controller(motion);
        window.connect_close_request({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(win) = weak.upgrade() {
                    if let Some(timer) = win.playing.borrow_mut().take() {
                        timer.remove();
                    }
                    win.slideshow.borrow_mut().take();
                }
                glib::Propagation::Proceed
            }
        });
        window.fullscreen();
        window.present();
        *self.slideshow.borrow_mut() = Some(window);
        show();
    }

    pub fn end_slideshow(&self) {
        if let Some(window) = self.slideshow.borrow_mut().take() {
            window.close();
        }
        if let Some(timer) = self.playing.borrow_mut().take() {
            timer.remove();
        }
    }

    /// The sidebar's rows are dragged to put a PDF's pages in another order.
    pub fn drag_rows(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        source.connect_prepare({
            let row = row.downgrade();
            move |_, _, _| {
                let row = row.upgrade()?;
                Some(gdk::ContentProvider::for_value(&format!("pv-row:{}", row.index()).to_value()))
            }
        });
        row.add_controller(source);
    }

    /// The magnifier, the contact sheet and the sidebar's rows dropped on one another.
    pub fn connect_view(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.loupe_layer.set_draw_func({
            let weak = weak.clone();
            move |_, cr, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.draw_loupe(cr);
                }
            }
        });
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let weak = weak.clone();
            move |_, x, y| {
                let Some(win) = weak.upgrade() else { return };
                win.loupe_at.set(Some((x, y)));
                if win.loupe.get() {
                    win.loupe_layer.queue_draw();
                }
            }
        });
        motion.connect_leave({
            let weak = weak.clone();
            move |_| {
                if let Some(win) = weak.upgrade() {
                    win.loupe_at.set(None);
                    win.loupe_layer.queue_draw();
                }
            }
        });
        self.stack.add_controller(motion);

        self.sheet.connect_child_activated({
            let weak = weak.clone();
            move |_, child| {
                if let Some(win) = weak.upgrade() {
                    win.current.set(child.index() as usize);
                    win.set_sidebar(Sidebar::Thumbnails);
                }
            }
        });

        let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        drop.connect_drop({
            let weak = weak.clone();
            move |_, value, _, y| {
                let (Some(win), Ok(text)) = (weak.upgrade(), value.get::<String>()) else { return false };
                let Some(from) = text.strip_prefix("pv-row:").and_then(|index| index.parse::<usize>().ok()) else { return false };
                let to = win.list.row_at_y(y as i32).map_or(win.items.borrow().len().saturating_sub(1), |row| row.index() as usize);
                win.move_page_to(from, to);
                true
            }
        });
        self.list.add_controller(drop);
        // Files dropped on the sidebar of a PDF are put in it as pages where they are let go.
        let files = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        files.connect_drop({
            let weak = weak.clone();
            move |_, value, _, y| {
                let (Some(win), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) else { return false };
                let paths: Vec<PathBuf> = files.files().iter().filter_map(|file| file.path()).collect();
                let at = win.list.row_at_y(y as i32).map(|row| row.index() as usize);
                match at {
                    Some(at) if win.items.borrow().get(at).is_some_and(|item| item.is_page() && is_pdf(&item.path)) => win.insert_files(paths, at),
                    _ => win.add_files(paths),
                }
                true
            }
        });
        self.list.add_controller(files);
        let focus = gtk::EventControllerFocus::new();
        for enter in [true, false] {
            let weak = weak.clone();
            let then = move |_: &gtk::EventControllerFocus| {
                if let Some(win) = weak.upgrade() {
                    win.update_actions();
                }
            };
            if enter {
                focus.connect_enter(then);
            } else {
                focus.connect_leave(then);
            }
        }
        self.list.add_controller(focus);
    }
}
