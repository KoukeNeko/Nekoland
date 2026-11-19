//! 標記: the markup bar and what its tools do. On a picture they choose a part of it or paint on
//! it; on a PDF they choose text or a part of a page, black it out, or put annotations on it,
//! which are chosen, moved, recoloured and taken off again. Every change can be taken back.

use super::*;
use poppler::prelude::*;

/// How far from an annotation's bottom right corner, in screen pixels, its handle is held.
const HANDLE: f64 = 8.0;
/// How small, in points, an annotation is resized to at the least.
const MIN_SIDE: f64 = 8.0;

/// The markup bar and the controls the window keeps of it.
pub struct Bar {
    pub bar: gtk::Box,
    pub picture_tools: Vec<gtk::Widget>,
    pub pdf_tools: Vec<gtk::Widget>,
    pub color: gtk::ColorDialogButton,
    pub fill: gtk::ColorDialogButton,
    pub font: gtk::FontDialogButton,
    pub signature_menu: gio::Menu,
}

fn tool_button(icon: &str, tip: &str, key: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder().icon_name(icon).tooltip_text(tip).action_name("win.tool").build();
    button.set_action_target_value(Some(&key.to_variant()));
    button
}

fn menu_button(icon: &str, tip: &str, menu: &gio::Menu) -> gtk::MenuButton {
    gtk::MenuButton::builder().icon_name(icon).tooltip_text(tip).menu_model(menu).build()
}

pub fn build_bar() -> Bar {
    let bar = gtk::Box::builder().spacing(GAP).css_classes(["pv-markup"]).build();
    let selection = gio::Menu::new();
    for (label, key) in [("矩形選取", "rectangle"), ("橢圓形選取", "ellipse"), ("套索選取", "lasso"), ("智慧型套索", "smart-lasso")] {
        selection.append(Some(label), Some(&format!("win.tool::{key}")));
    }
    let shapes_menu = gio::Menu::new();
    for (shape, name) in shapes::SHAPES {
        shapes_menu.append(Some(name), Some(&format!("win.annotate::{}", shape.key())));
    }
    let signature_menu = gio::Menu::new();
    let style = gio::Menu::new();
    let widths = gio::Menu::new();
    for width in shapes::WIDTHS {
        widths.append(Some(&format!("{width} 點")), Some(&format!("win.line-width::{width}")));
    }
    style.append_section(None, &widths);
    style.append(Some("填滿"), Some("win.fill-shapes"));

    let text = tool_button("edit-select-symbolic", "文字選取", "text");
    let area = tool_button("selection-mode-symbolic", "矩形選取", "rectangle");
    let redact = tool_button("edit-clear-all-symbolic", "塗黑", "redact");
    let choose = menu_button("selection-mode-symbolic", "選取工具", &selection);
    let alpha = tool_button("color-select-symbolic", "即時 Alpha", "instant-alpha");
    let sketch = tool_button("input-tablet-symbolic", "草圖", "sketch");
    let shapes_button = menu_button("star-new-symbolic", "形狀", &shapes_menu);
    let text_box = tool_button("insert-text-symbolic", "文字", "textbox");
    let signature = gtk::MenuButton::builder().label("簽名").tooltip_text("簽名").menu_model(&signature_menu).build();
    let note = tool_button("chat-message-new-symbolic", "附註", "note");
    let form = tool_button("format-text-plaintext-symbolic", "表單填寫", "form");
    let adjust = gtk::Button::builder().icon_name("applications-graphics-symbolic").tooltip_text("調整顏色").action_name("win.adjust-color").build();
    let resize = gtk::Button::builder().icon_name("zoom-fit-best-symbolic").tooltip_text("調整大小").action_name("win.resize").build();
    let width = menu_button("format-justify-fill-symbolic", "形狀樣式", &style);
    let color = gtk::ColorDialogButton::builder()
        .dialog(&gtk::ColorDialog::builder().with_alpha(true).build())
        .rgba(&gdk::RGBA::new(0.85, 0.15, 0.12, 1.0))
        .tooltip_text("邊框顏色")
        .build();
    let fill = gtk::ColorDialogButton::builder()
        .dialog(&gtk::ColorDialog::builder().with_alpha(true).build())
        .rgba(&gdk::RGBA::new(1.0, 1.0, 1.0, 1.0))
        .tooltip_text("填充顏色")
        .build();
    let font = gtk::FontDialogButton::builder()
        .dialog(&gtk::FontDialog::new())
        .font_desc(&pango::FontDescription::from_string("Sans 14"))
        .tooltip_text("文字樣式")
        .build();
    let separator = || gtk::Separator::new(gtk::Orientation::Vertical);
    let (before_adjust, after_adjust) = (separator(), separator());
    let widgets: [&gtk::Widget; 18] = [
        text.upcast_ref(),
        area.upcast_ref(),
        choose.upcast_ref(),
        alpha.upcast_ref(),
        redact.upcast_ref(),
        sketch.upcast_ref(),
        shapes_button.upcast_ref(),
        text_box.upcast_ref(),
        signature.upcast_ref(),
        note.upcast_ref(),
        form.upcast_ref(),
        before_adjust.upcast_ref(),
        adjust.upcast_ref(),
        resize.upcast_ref(),
        after_adjust.upcast_ref(),
        width.upcast_ref(),
        color.upcast_ref(),
        fill.upcast_ref(),
    ];
    for widget in widgets {
        bar.append(widget);
    }
    bar.append(&font);
    Bar {
        bar,
        picture_tools: vec![choose.upcast(), alpha.upcast(), before_adjust.upcast(), adjust.upcast(), resize.upcast()],
        pdf_tools: vec![text.upcast(), area.upcast(), redact.upcast(), note.upcast(), form.upcast()],
        color,
        fill,
        font,
        signature_menu,
    }
}

/// Where a mark made by dragging from `from` to `to` goes: the box between them, or one of
/// `size` at `from` when it was a click.
fn placed(from: (f64, f64), to: (f64, f64), size: (f64, f64)) -> Place {
    if (to.0 - from.0).abs() < 4.0 && (to.1 - from.1).abs() < 4.0 {
        return (from.0, from.1, from.0 + size.0, from.1 + size.1);
    }
    (from.0.min(to.0), from.1.min(to.1), from.0.max(to.0), from.1.max(to.1))
}

fn inside((x1, y1, x2, y2): Place, (x, y): (f64, f64)) -> bool {
    x >= x1.min(x2) && x <= x1.max(x2) && y >= y1.min(y2) && y <= y1.max(y2)
}

/// A picture as a Cairo surface, which a stamp annotation takes.
fn surface_of(pixbuf: &Pixbuf) -> Option<gtk::cairo::ImageSurface> {
    let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, pixbuf.width(), pixbuf.height()).ok()?;
    {
        let cr = gtk::cairo::Context::new(&surface).ok()?;
        cr.set_source_pixbuf(pixbuf, 0.0, 0.0);
        cr.paint().ok()?;
    }
    surface.flush();
    Some(surface)
}

impl Win {
    pub fn connect_markup(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.mark_layer.set_draw_func({
            let weak = weak.clone();
            move |_, cr, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.draw_picture_marks(cr);
                }
            }
        });
        // A colour chosen while an annotation is chosen recolours it.
        self.color.connect_rgba_notify({
            let weak = weak.clone();
            move |button| {
                if let Some(win) = weak.upgrade() {
                    win.recolour_chosen(shapes::rgb(&button.rgba()));
                }
            }
        });

        let paint = gtk::GestureDrag::new();
        paint.connect_drag_begin({
            let weak = weak.clone();
            move |_, x, y| {
                if let Some(win) = weak.upgrade().filter(|win| win.tool.get().is_some()) {
                    let scale = win.scale();
                    *win.stroke.borrow_mut() = vec![(x / scale, y / scale)];
                    win.mark_layer.queue_draw();
                }
            }
        });
        paint.connect_drag_update({
            let weak = weak.clone();
            move |drag, dx, dy| {
                let (Some(win), Some((x, y))) = (weak.upgrade().filter(|win| win.tool.get().is_some()), drag.start_point()) else { return };
                let scale = win.scale();
                win.stroke.borrow_mut().push(((x + dx) / scale, (y + dy) / scale));
                win.mark_layer.queue_draw();
            }
        });
        paint.connect_drag_end({
            let weak = weak.clone();
            move |_, dx, dy| {
                if let Some(win) = weak.upgrade() {
                    win.finish_picture_mark(dx.hypot(dy));
                }
            }
        });
        self.picture.add_controller(paint);

        // Over a PDF's pages: text chosen by dragging, a double click a word and a triple a
        // line; or what the tool chosen makes.
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin({
            let weak = weak.clone();
            move |_, x, y| {
                if let Some(win) = weak.upgrade() {
                    win.pdf_column.grab_focus();
                    win.page_drag_begin(x, y);
                }
            }
        });
        drag.connect_drag_update({
            let weak = weak.clone();
            move |drag, dx, dy| {
                let (Some(win), Some((x, y))) = (weak.upgrade(), drag.start_point()) else { return };
                win.page_drag_update(x, y, dx, dy);
            }
        });
        drag.connect_drag_end({
            let weak = weak.clone();
            move |drag, dx, dy| {
                let (Some(win), Some((x, y))) = (weak.upgrade(), drag.start_point()) else { return };
                win.page_drag_end(x, y, dx, dy);
            }
        });
        self.pdf_column.add_controller(drag);
        let clicks = gtk::GestureClick::new();
        clicks.connect_pressed({
            let weak = weak.clone();
            move |_, presses, x, y| {
                let Some(win) = weak.upgrade() else { return };
                if !matches!(win.tool.get(), None | Some(Tool::Text)) || presses < 2 {
                    return;
                }
                let Some(at) = win.hit(x, y) else { return };
                // A note or a text box clicked twice is written in again.
                if presses == 2 && win.annotation_at(at).is_some() {
                    return win.edit_contents(at);
                }
                let style = if presses == 2 { poppler::SelectionStyle::Word } else { poppler::SelectionStyle::Line };
                win.set_selection(Some(Selection { from: at, to: at, style }));
            }
        });
        self.pdf_column.add_controller(clicks);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let weak = weak.clone();
            move |_, x, y| {
                if let Some(win) = weak.upgrade() {
                    win.point_at(x, y);
                }
            }
        });
        self.pdf_column.add_controller(motion);
    }

    /// The markup bar shown or hidden; hidden, the tool is put down.
    pub fn set_markup(self: &Rc<Self>, on: bool) {
        self.markup_bar.set_visible(on);
        self.set_action_state("markup", on.to_variant());
        if !on {
            self.set_tool(None);
        } else if self.tool.get().is_none() && self.item().is_some_and(|item| item.is_page()) {
            self.set_tool(Some(Tool::Text));
        }
    }

    pub fn set_tool(self: &Rc<Self>, tool: Option<Tool>) {
        self.tool.set(tool);
        self.set_action_state("tool", tool.map_or("", Tool::key).to_variant());
        if tool.is_some() && !self.markup_bar.is_visible() {
            self.markup_bar.set_visible(true);
            self.set_action_state("markup", true.to_variant());
        }
        self.stroke.take();
        let crosshair = tool.is_some_and(|tool| tool != Tool::Text);
        let cursor = if crosshair { Some("crosshair") } else { None };
        self.picture.set_cursor_from_name(cursor);
        self.pdf_column.set_cursor_from_name(Some(cursor.unwrap_or("text")));
        self.mark_layer.queue_draw();
        self.update_actions();
    }

    pub fn set_width(self: &Rc<Self>, width: f64) {
        self.width.set(width);
        if let Some((index, annotation)) = self.chosen.borrow().clone() {
            if matches!(annotation.kind, annots::Kind::Square | annots::Kind::Circle) {
                self.change_pdf(index, |_, _| {
                    annots::set_border_width(&annotation.annot, width);
                    Some(annotation.annot.clone())
                });
            }
        }
    }

    /// A shape or text from the menus: the markup bar shown with it chosen.
    pub fn annotate(self: &Rc<Self>, key: &str) {
        match key {
            "text" => self.set_tool(Some(Tool::TextBox)),
            "note" => self.set_tool(Some(Tool::Note)),
            _ => {
                if let Some(shape) = Shape::from_key(key) {
                    self.shape.set(shape);
                    self.set_tool(Some(Tool::Shape));
                }
            }
        }
    }

    pub fn style(&self) -> Style {
        Style {
            stroke: self.color.rgba(),
            fill: self.filled.get().then(|| self.fill.rgba()),
            width: self.width.get(),
            font: self.font.font_desc().unwrap_or_else(|| pango::FontDescription::from_string("Sans 14")),
        }
    }

    /// The text chosen in a PDF is marked as `kind` says, and so is what is chosen next.
    pub fn mark(self: &Rc<Self>, kind: &str) {
        let kind = match kind {
            "underline" => annots::Kind::Underline,
            "strikeout" => annots::Kind::StrikeOut,
            _ => annots::Kind::Highlight,
        };
        let marking = Marking { kind, colour: self.last_marking.get().colour };
        self.last_marking.set(marking);
        self.start_marking(marking);
    }

    pub fn mark_colour(self: &Rc<Self>, index: usize) {
        let colour = MARK_COLOURS.get(index).map_or(MARK_COLOURS[0].0, |(colour, _)| *colour);
        let marking = Marking { kind: self.last_marking.get().kind, colour };
        self.last_marking.set(marking);
        // A highlight chosen takes the colour.
        if self.chosen.borrow().as_ref().is_some_and(|(_, annotation)| annotation.kind.marks_text()) {
            return self.recolour_chosen(colour);
        }
        self.start_marking(marking);
    }

    /// The 標示重點 button: marking on, as last, or off.
    pub fn toggle_highlight(self: &Rc<Self>) {
        if self.marking.get().is_some() {
            self.marking.set(None);
            self.highlight_button.remove_css_class("suggested-action");
            return;
        }
        self.start_marking(self.last_marking.get());
    }

    fn start_marking(self: &Rc<Self>, marking: Marking) {
        self.marking.set(Some(marking));
        self.highlight_button.add_css_class("suggested-action");
        if !matches!(self.tool.get(), None | Some(Tool::Text)) {
            self.set_tool(Some(Tool::Text));
        }
        self.mark_selection();
    }

    /// Marks the text chosen, as 標示重點 is set to.
    fn mark_selection(self: &Rc<Self>) {
        let (Some(marking), Some(selection), Some(pdf)) = (self.marking.get(), self.selection.get(), self.pdf.borrow().clone()) else { return };
        self.snapshot(&pdf);
        let document = pdf.document();
        for index in selection.pages() {
            let Some(page) = document.page(index as i32) else { continue };
            let Some(area) = selection.on(index, pdf.sizes[index]) else { continue };
            let lines = annots::text_lines(&page, &area, selection.style);
            if let Some(annot) = annots::mark_text(&document, &page, marking.kind, &lines, marking.colour) {
                self.sign(&annot);
            }
        }
        self.set_selection(None);
        self.changed(&pdf, selection.pages().collect());
    }

    /// Puts the author's name on an annotation, as 設定 says.
    fn sign(&self, annot: &poppler::Annot) {
        let prefs = self.prefs.borrow();
        if prefs.sign_annotations && !prefs.author.is_empty() {
            annots::set_author(annot, &prefs.author);
        }
    }

    /// Keeps the PDF as it is, for 還原 to go back to.
    pub fn snapshot(&self, pdf: &Pdf) {
        if let Some(path) = save_snapshot(pdf) {
            pdf.undo.borrow_mut().push(path);
            pdf.redo.borrow_mut().clear();
        }
    }

    /// The PDF was changed on `pages`: they are drawn again, with what lists them.
    fn changed(self: &Rc<Self>, pdf: &Rc<Pdf>, pages: Vec<usize>) {
        pdf.dirty.set(true);
        for index in &pages {
            if let Some(page) = pdf.pages.get(*index) {
                page.queue_draw();
            }
        }
        self.refresh_thumbnails(pdf, &pages);
        self.fill_notes();
        if let Some(item) = self.item() {
            self.update_title(&item);
        }
        self.update_actions();
    }

    fn refresh_thumbnails(&self, pdf: &Pdf, pages: &[usize]) {
        let density = f64::from(self.window.scale_factor());
        let document = pdf.document();
        for &index in pages {
            let (Some(row), Some(page)) = (self.item_of_page(pdf, index), document.page(index as i32)) else { continue };
            let Some(picture) = self.thumbnails.borrow().get(row).cloned() else { continue };
            let (width, height) = page.size();
            if let Some(texture) = page_texture(&page, f64::from(THUMB_SIZE) / width.max(height) * density) {
                picture.set_paintable(Some(&texture));
            }
        }
    }

    /// Changes page `index` of the PDF shown with `change`, kept first for 還原; the
    /// annotation it hands back is signed.
    pub fn change_pdf(self: &Rc<Self>, index: usize, change: impl FnOnce(&poppler::Document, &poppler::Page) -> Option<poppler::Annot>) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let document = pdf.document();
        let Some(page) = document.page(index as i32) else { return };
        self.snapshot(&pdf);
        if let Some(annot) = change(&document, &page) {
            self.sign(&annot);
        }
        self.changed(&pdf, vec![index]);
    }

    pub fn undo(self: &Rc<Self>) {
        if let Some(pdf) = self.pdf.borrow().clone().filter(|_| self.item().is_some_and(|item| item.is_page())) {
            let Some(before) = pdf.undo.borrow_mut().pop() else { return };
            if let Some(now) = save_snapshot(&pdf) {
                pdf.redo.borrow_mut().push(now);
            }
            return self.restore(&pdf, &before, !pdf.undo.borrow().is_empty());
        }
        let Some(item) = self.item() else { return };
        let Some(before) = item.undo.borrow_mut().pop() else { return };
        let now = item.edited.replace(before.clone());
        item.redo.borrow_mut().push(now);
        self.set_mask(None, Vec::new());
        match before {
            Some(pixbuf) => self.show(&item, pixbuf),
            None => self.select(self.current.get()),
        }
    }

    pub fn redo(self: &Rc<Self>) {
        if let Some(pdf) = self.pdf.borrow().clone().filter(|_| self.item().is_some_and(|item| item.is_page())) {
            let Some(after) = pdf.redo.borrow_mut().pop() else { return };
            if let Some(now) = save_snapshot(&pdf) {
                pdf.undo.borrow_mut().push(now);
            }
            return self.restore(&pdf, &after, true);
        }
        let Some(item) = self.item() else { return };
        let Some(after) = item.redo.borrow_mut().pop() else { return };
        let now = item.edited.replace(after.clone());
        item.undo.borrow_mut().push(now);
        match after {
            Some(pixbuf) => self.show(&item, pixbuf),
            None => self.select(self.current.get()),
        }
    }

    /// Reads the PDF again as it was kept at `path`.
    fn restore(self: &Rc<Self>, pdf: &Rc<Pdf>, path: &Path, dirty: bool) {
        match open_pdf(path, pdf.password.as_deref()) {
            Ok(document) => {
                *pdf.document.borrow_mut() = document;
                self.chosen.take();
                self.changed(pdf, (0..pdf.sizes.len()).collect());
                pdf.dirty.set(dirty);
                if let Some(item) = self.item() {
                    self.update_title(&item);
                }
            }
            Err(err) => alert::notice(&self.window, "無法還原", Some(&err.to_string())),
        }
    }

    /// 刪除: the part of a picture chosen made clear, the annotation chosen taken off, or the
    /// pages chosen in the sidebar taken out.
    pub fn delete(self: &Rc<Self>) {
        if let Some((index, annotation)) = self.chosen.take() {
            return self.change_pdf(index, |_, page| {
                annots::remove(page, &annotation.annot);
                None
            });
        }
        if let Some(mask) = self.image_mask.borrow().clone().filter(|mask| !mask.is_empty()) {
            self.keep_clear_pixels();
            self.edit(|pixbuf| Some(imaging::clear(&imaging::Rgba::from_pixbuf(pixbuf), &mask).to_pixbuf()));
            return;
        }
        if self.item().is_some_and(|item| item.is_page()) {
            self.delete_pages();
        }
    }

    /// A picture in a format with no clear pixels is saved as a PNG beside it once some are made.
    pub fn keep_clear_pixels(&self) {
        let Some(item) = self.item() else { return };
        let target = item.target();
        let keeps_alpha = target.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(e.to_lowercase().as_str(), "png" | "tif" | "tiff" | "heic" | "heif" | "webp" | "gif" | "exr" | "jp2" | "avif")
        });
        if !keeps_alpha {
            *item.save_to.borrow_mut() = Some(target.with_extension("png"));
        }
    }

    fn recolour_chosen(self: &Rc<Self>, colour: (f64, f64, f64)) {
        let Some((index, annotation)) = self.chosen.borrow().clone() else { return };
        self.change_pdf(index, |_, _| {
            annots::set_colour(&annotation.annot, colour);
            None
        });
        if let Some((_, chosen)) = self.chosen.borrow_mut().as_mut() {
            chosen.color = Some(colour);
        }
    }

    /// The annotation at a point on a page, the last drawn first.
    fn annotation_at(&self, (index, x, y): (usize, f64, f64)) -> Option<annots::Annotation> {
        let pdf = self.pdf.borrow().clone()?;
        let page = pdf.document().page(index as i32)?;
        annots::on_page(&page).into_iter().rev().find(|annotation| inside(annotation.place, (x, y)))
    }

    /// A note or text box written in again.
    fn edit_contents(self: &Rc<Self>, at: (usize, f64, f64)) {
        let Some(annotation) = self.annotation_at(at) else { return };
        let weak = Rc::downgrade(self);
        let title = if annotation.kind == annots::Kind::Note { "附註" } else { "文字" };
        alert::prompt(&self.window, title, &annotation.contents, -1, "完成", move |text| {
            if let Some(win) = weak.upgrade() {
                win.change_pdf(at.0, |_, _| {
                    annotation.annot.set_contents(&text);
                    None
                });
            }
        });
    }

    /// Whether `at`, in points, is on the handle at the bottom right of `annotation`, which
    /// resizes it.
    fn on_handle(&self, annotation: &annots::Annotation, at: (f64, f64)) -> bool {
        let scale = self.pdf.borrow().as_ref().map_or(1.0, |pdf| pdf.scale.get());
        let (_, _, x2, y2) = annotation.place;
        annotation.kind.resizable() && (at.0 - x2).abs() * scale <= HANDLE && (at.1 - y2).abs() * scale <= HANDLE
    }

    /// The pointer over the pages: a hand over a link or a field.
    fn point_at(&self, x: f64, y: f64) {
        if !matches!(self.tool.get(), None | Some(Tool::Text)) {
            return;
        }
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let Some((index, px, py)) = self.hit(x, y) else { return };
        let Some(page) = pdf.document().page(index as i32) else { return };
        let over_link = pdfdoc::links(&pdf.document(), &page).iter().any(|(place, _)| inside(*place, (px, py)))
            || pdfdoc::fields(&page).iter().any(|(place, _)| inside(*place, (px, py)));
        self.pdf_column.set_cursor_from_name(Some(if over_link { "pointer" } else { "text" }));
    }

    fn page_drag_begin(self: &Rc<Self>, x: f64, y: f64) {
        let Some(at) = self.hit(x, y) else { return };
        match self.tool.get() {
            None | Some(Tool::Text) => {
                // An annotation pressed on is chosen, to be dragged; anywhere else starts text.
                // Its handle, which may be just outside it, keeps it chosen.
                let held = self.chosen.borrow().clone().filter(|(on, chosen)| *on == at.0 && self.on_handle(chosen, (at.1, at.2)));
                let found = held.map(|(_, chosen)| chosen).or_else(|| self.annotation_at(at));
                let changed = found.as_ref().map(|annotation| annotation.annot.clone()) != self.chosen.borrow().as_ref().map(|(_, chosen)| chosen.annot.clone());
                *self.chosen.borrow_mut() = found.map(|annotation| (at.0, annotation));
                if changed {
                    self.redraw_pages();
                    self.update_actions();
                }
                if self.chosen.borrow().is_some() {
                    self.stroke_page.set(Some(at.0));
                    *self.stroke.borrow_mut() = vec![(at.1, at.2)];
                }
            }
            Some(_) => {
                self.stroke_page.set(Some(at.0));
                *self.stroke.borrow_mut() = vec![(at.1, at.2)];
            }
        }
    }

    fn page_drag_update(&self, x: f64, y: f64, dx: f64, dy: f64) {
        // Not the jitter of a click.
        if dx.hypot(dy) < 3.0 {
            return;
        }
        match (self.tool.get(), self.chosen.borrow().is_some()) {
            (None | Some(Tool::Text), false) => {
                if let (Some(from), Some(to)) = (self.hit(x, y), self.hit(x + dx, y + dy)) {
                    self.set_selection(Some(Selection { from, to, style: poppler::SelectionStyle::Glyph }));
                }
            }
            _ => {
                let Some(index) = self.stroke_page.get() else { return };
                let Some((_, px, py)) = self.hit_on(index, x + dx, y + dy) else { return };
                self.stroke.borrow_mut().push((px, py));
                if let Some(pdf) = self.pdf.borrow().as_ref() {
                    pdf.pages[index].queue_draw();
                }
            }
        }
    }

    /// Where `(x, y)` over the column is on page `index`, even off it.
    fn hit_on(&self, index: usize, x: f64, y: f64) -> Option<(usize, f64, f64)> {
        let pdf = self.pdf.borrow().clone()?;
        let scale = pdf.scale.get();
        let point = self.pdf_column.compute_point(&pdf.pages[index], &gtk::graphene::Point::new(x as f32, y as f32))?;
        let (width, height) = pdf.sizes[index];
        Some((index, (f64::from(point.x()) / scale).clamp(0.0, width), (f64::from(point.y()) / scale).clamp(0.0, height)))
    }

    fn page_drag_end(self: &Rc<Self>, x: f64, y: f64, dx: f64, dy: f64) {
        let points = self.stroke.take();
        let index = self.stroke_page.take();
        let clicked = dx.hypot(dy) < 3.0;
        let tool = self.tool.get();
        if let (None | Some(Tool::Text), Some((chosen_on, chosen))) = (tool, self.chosen.borrow().clone()) {
            if let (false, Some(&from), Some(&to)) = (clicked, points.first(), points.last()) {
                if self.on_handle(&chosen, from) {
                    let (x1, y1, _, _) = chosen.place;
                    let place = (x1, y1, to.0.max(x1 + MIN_SIDE), to.1.max(y1 + MIN_SIDE));
                    self.change_pdf(chosen_on, |_, page| {
                        annots::set_place(page, &chosen.annot, place);
                        Some(chosen.annot.clone())
                    });
                    let resized = self.annotation_at((chosen_on, x1 + 1.0, y1 + 1.0));
                    *self.chosen.borrow_mut() = resized.map(|annotation| (chosen_on, annotation));
                    self.redraw_pages();
                    return;
                }
                let (shift_x, shift_y) = (to.0 - from.0, to.1 - from.1);
                self.change_pdf(chosen_on, |document, page| annots::shift(document, page, &chosen, shift_x, shift_y));
                let moved = self.annotation_at((chosen_on, chosen.place.0 + shift_x + 1.0, chosen.place.1 + shift_y + 1.0));
                *self.chosen.borrow_mut() = moved.map(|annotation| (chosen_on, annotation));
                self.redraw_pages();
            }
            return;
        }
        if matches!(tool, None | Some(Tool::Text)) {
            if clicked {
                if let Some(at) = self.hit(x, y) {
                    self.follow(at);
                }
                return;
            }
            if self.marking.get().is_some() {
                self.mark_selection();
            }
            return;
        }
        let (Some(index), Some(&from), Some(&to)) = (index, points.first(), points.last()) else { return };
        let pdf = match self.pdf.borrow().clone() {
            Some(pdf) => pdf,
            None => return,
        };
        let style = self.style();
        let (stroke, fill) = (shapes::rgb(&style.stroke), style.fill.as_ref().map(shapes::rgb));
        let width = style.width;
        match tool {
            Some(Tool::Rectangle) => {
                self.area.set((!clicked).then(|| (index, placed(from, to, (0.0, 0.0)))));
                self.redraw_pages();
                self.update_actions();
            }
            Some(Tool::Redact) if !clicked => {
                self.redactions.borrow_mut().push((index, placed(from, to, (0.0, 0.0))));
                pdf.dirty.set(true);
                pdf.pages[index].queue_draw();
                if let Some(item) = self.item() {
                    self.update_title(&item);
                }
                self.update_actions();
            }
            Some(Tool::Sketch) if points.len() > 1 => {
                self.change_pdf(index, |document, page| annots::ink(document, page, std::slice::from_ref(&points), stroke, width));
            }
            Some(Tool::Shape) if !clicked => self.put_shape(index, from, to, &style, stroke, fill),
            Some(Tool::TextBox | Tool::FormText) => {
                let font = style.font.clone();
                let size = f64::from(font.size()) / f64::from(pango::SCALE);
                let size = if tool == Some(Tool::FormText) { 12.0 } else { size.max(6.0) };
                let colour = if tool == Some(Tool::FormText) { (0.0, 0.0, 0.0) } else { stroke };
                let family = font.family().map(|family| family.to_string()).unwrap_or_else(|| String::from("Sans"));
                let weak = Rc::downgrade(self);
                let box_place = placed(from, to, (size * 12.0, size * 1.6));
                alert::prompt(&self.window, "文字", "", -1, "加入", move |text| {
                    let Some(win) = weak.upgrade() else { return };
                    let lines = text.lines().count().max(1) as f64;
                    let (x1, y1, x2, y2) = box_place;
                    let place = (x1, y1, x2.max(x1 + size * 0.6 * text.chars().count() as f64), y2.max(y1 + size * 1.4 * lines));
                    win.change_pdf(index, |document, page| annots::free_text(document, page, place, &text, &family, size, colour));
                });
            }
            Some(Tool::Note) => {
                let weak = Rc::downgrade(self);
                alert::prompt(&self.window, "附註", "", -1, "加入", move |text| {
                    if let Some(win) = weak.upgrade() {
                        win.change_pdf(index, |document, page| annots::note(document, page, from, &text, MARK_COLOURS[0].0));
                    }
                });
            }
            Some(Tool::Signature) => self.put_signature_on_page(index, from, to),
            _ => {}
        }
        pdf.pages[index].queue_draw();
    }

    /// A shape put on page `index`: a rectangle or an ellipse as one, a line as one, and the
    /// others as the lines they are drawn with.
    fn put_shape(self: &Rc<Self>, index: usize, from: (f64, f64), to: (f64, f64), style: &Style, stroke: (f64, f64, f64), fill: Option<(f64, f64, f64)>) {
        let width = style.width;
        let place = placed(from, to, (0.0, 0.0));
        match self.shape.get() {
            Shape::Rectangle => self.change_pdf(index, |document, page| annots::shape(document, page, false, place, stroke, fill, width)),
            Shape::Ellipse => self.change_pdf(index, |document, page| annots::shape(document, page, true, place, stroke, fill, width)),
            Shape::Highlight => self.change_pdf(index, |document, page| annots::shape(document, page, false, place, (1.0, 0.85, 0.0), Some((1.0, 0.85, 0.0)), 0.0)),
            Shape::Mask => self.change_pdf(index, |document, page| annots::shape(document, page, false, place, (0.0, 0.0, 0.0), Some((0.0, 0.0, 0.0)), 0.0)),
            Shape::Line => self.change_pdf(index, |document, page| annots::line(document, page, from, to, stroke, width)),
            Shape::Loupe => {
                // What is under it, at twice its size, put on as a picture.
                let Some(pdf) = self.pdf.borrow().clone() else { return };
                let Some(page) = pdf.document().page(index as i32) else { return };
                let (x1, y1, x2, y2) = place;
                let side = |length: f64| ((length * 4.0).round() as i32).max(1);
                let Ok(surface) = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, side(x2 - x1), side(y2 - y1)) else { return };
                if let Ok(cr) = gtk::cairo::Context::new(&surface) {
                    cr.scale(4.0, 4.0);
                    let lines = shapes::outline(Shape::Ellipse, (0.0, 0.0), (x2 - x1, y2 - y1), width);
                    for line in &lines {
                        if let Some(&(x, y)) = line.first() {
                            cr.move_to(x, y);
                        }
                        line.iter().skip(1).for_each(|&(x, y)| cr.line_to(x, y));
                    }
                    cr.clip();
                    let (w, h) = (x2 - x1, y2 - y1);
                    cr.translate(w / 2.0, h / 2.0);
                    cr.scale(2.0, 2.0);
                    cr.translate(-(x1 + w / 2.0), -(y1 + h / 2.0));
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.paint();
                    page.render(&cr);
                    cr.reset_clip();
                    cr.identity_matrix();
                    cr.scale(4.0, 4.0);
                    for line in &lines {
                        if let Some(&(x, y)) = line.first() {
                            cr.move_to(x, y);
                        }
                        line.iter().skip(1).for_each(|&(x, y)| cr.line_to(x, y));
                    }
                    cr.set_source_rgb(stroke.0, stroke.1, stroke.2);
                    cr.set_line_width(width);
                    let _ = cr.stroke();
                }
                surface.flush();
                self.change_pdf(index, |document, page| annots::stamp(document, page, place, &surface));
            }
            shape => {
                let lines = shapes::outline(shape, from, to, width);
                self.change_pdf(index, |document, page| annots::ink(document, page, &lines, stroke, width));
            }
        }
    }

    /// A click with no tool: a link is followed, a field filled in, or what was chosen let go.
    fn follow(self: &Rc<Self>, (index, x, y): (usize, f64, f64)) {
        let Some(pdf) = self.pdf.borrow().clone() else { return };
        let document = pdf.document();
        let Some(page) = document.page(index as i32) else { return };
        if let Some((_, field)) = pdfdoc::fields(&page).into_iter().find(|(place, _)| inside(*place, (x, y))) {
            return self.fill_field(index, field);
        }
        if let Some((_, target)) = pdfdoc::links(&document, &page).into_iter().find(|(place, _)| inside(*place, (x, y))) {
            let last = pdf.sizes.len() - 1;
            self.remember_place();
            match target {
                pdfdoc::Target::Page(page, top) => self.go_page(page, top),
                pdfdoc::Target::Next => self.go_page((index + 1).min(last), None),
                pdfdoc::Target::Previous => self.go_page(index.saturating_sub(1), None),
                pdfdoc::Target::First => self.go_page(0, None),
                pdfdoc::Target::Last => self.go_page(last, None),
                // A PDF is untrusted: its links may open web pages and mail, not files or other apps.
                pdfdoc::Target::Uri(uri) => {
                    if glib::Uri::peek_scheme(&uri).is_some_and(|scheme| matches!(scheme.as_str(), "http" | "https" | "mailto")) {
                        gtk::UriLauncher::new(&uri).launch(Some(&self.window), gio::Cancellable::NONE, |_| {});
                    }
                }
            }
            return;
        }
        self.set_selection(None);
        if self.chosen.take().is_some() {
            self.redraw_pages();
        }
    }

    /// A form's field filled in: its text typed, its box ticked, or one of its choices chosen.
    fn fill_field(self: &Rc<Self>, index: usize, field: poppler::FormField) {
        if field.is_read_only() {
            return;
        }
        let weak = Rc::downgrade(self);
        match field.field_type() {
            poppler::FormFieldType::Text => {
                let now = field.text_get_text().map(|text| text.to_string()).unwrap_or_default();
                let password = field.text_is_password();
                let entry = alert::prompt(&self.window, &field.name().map(|name| name.to_string()).unwrap_or_else(|| String::from("文字")), &now, -1, "完成", move |text| {
                    if let Some(win) = weak.upgrade() {
                        win.change_pdf(index, |_, _| {
                            field.text_set_text(&text);
                            None
                        });
                    }
                });
                if password {
                    entry.set_visibility(false);
                }
            }
            poppler::FormFieldType::Button => {
                let on = field.button_get_state();
                self.change_pdf(index, |_, _| {
                    field.button_set_state(!on);
                    None
                });
            }
            poppler::FormFieldType::Choice => {
                let names: Vec<String> = (0..field.choice_get_n_items()).filter_map(|item| field.choice_get_item(item).map(|name| name.to_string())).collect();
                let list = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
                let dropdown = gtk::DropDown::builder().model(&list).build();
                if let Some(chosen) = (0..names.len() as i32).find(|&item| field.choice_is_item_selected(item)) {
                    dropdown.set_selected(chosen as u32);
                }
                let alert = adw::AlertDialog::new(field.name().as_deref(), None);
                alert.set_extra_child(Some(&dropdown));
                alert.add_responses(&[("cancel", "取消"), ("do", "完成")]);
                alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
                alert.set_default_response(Some("do"));
                alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
                    let Some(win) = weak.upgrade().filter(|_| response == "do") else { return };
                    let chosen = dropdown.selected() as i32;
                    win.change_pdf(index, |_, _| {
                        field.choice_unselect_all();
                        field.choice_select_item(chosen);
                        None
                    });
                });
            }
            _ => {}
        }
    }

    /// Draws on page `index` what the markup tools have on it, over the page drawn at points.
    pub fn draw_page_marks(&self, index: usize, page: &poppler::Page, cr: &gtk::cairo::Context) {
        cr.set_source_rgb(0.0, 0.0, 0.0);
        for (_, (x1, y1, x2, y2)) in self.redactions.borrow().iter().filter(|(on, _)| *on == index) {
            cr.rectangle(*x1, *y1, x2 - x1, y2 - y1);
            let _ = cr.fill();
        }
        let accent = (0.21, 0.52, 0.89);
        let dashed = |cr: &gtk::cairo::Context, (x1, y1, x2, y2): Place| {
            cr.set_source_rgb(accent.0, accent.1, accent.2);
            cr.set_line_width(1.0);
            cr.set_dash(&[4.0, 3.0], 0.0);
            cr.rectangle(x1, y1, x2 - x1, y2 - y1);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        };
        if let Some((_, place)) = self.area.get().filter(|(on, _)| *on == index) {
            dashed(cr, place);
        }
        if let Some((_, annotation)) = self.chosen.borrow().as_ref().filter(|(on, _)| *on == index) {
            let (x1, y1, x2, y2) = annotation.place;
            dashed(cr, (x1 - 2.0, y1 - 2.0, x2 + 2.0, y2 + 2.0));
            if annotation.kind.resizable() {
                let side = HANDLE / self.pdf.borrow().as_ref().map_or(1.0, |pdf| pdf.scale.get());
                cr.set_source_rgb(0.21, 0.52, 0.89);
                cr.rectangle(x2 - side / 2.0, y2 - side / 2.0, side, side);
                let _ = cr.fill();
            }
        }
        if self.stroke_page.get() != Some(index) {
            return;
        }
        let stroke = self.stroke.borrow();
        let (Some(&from), Some(&to)) = (stroke.first(), stroke.last()) else { return };
        let style = self.style();
        match self.tool.get() {
            Some(Tool::Sketch) => shapes::draw_sketch(cr, &stroke, &style, style.width),
            Some(Tool::Shape) => {
                let under = |cr: &gtk::cairo::Context| page.render(cr);
                shapes::draw_shape(cr, self.shape.get(), from, to, &style, style.width, &under);
            }
            Some(Tool::Redact) => {
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
                let (x1, y1, x2, y2) = placed(from, to, (0.0, 0.0));
                cr.rectangle(x1, y1, x2 - x1, y2 - y1);
                let _ = cr.fill();
            }
            Some(Tool::Rectangle | Tool::TextBox | Tool::Signature) => dashed(cr, placed(from, to, (0.0, 0.0))),
            None | Some(Tool::Text) if self.chosen.borrow().is_some() => {
                if let Some((_, annotation)) = self.chosen.borrow().as_ref() {
                    let (x1, y1, x2, y2) = annotation.place;
                    if self.on_handle(annotation, from) {
                        dashed(cr, (x1, y1, to.0.max(x1 + MIN_SIDE), to.1.max(y1 + MIN_SIDE)));
                    } else {
                        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                        dashed(cr, (x1 + dx, y1 + dy, x2 + dx, y2 + dy));
                    }
                }
            }
            _ => {}
        }
    }

    /// Draws over the picture what is chosen of it and what is being drawn.
    fn draw_picture_marks(&self, cr: &gtk::cairo::Context) {
        let scale = self.scale();
        cr.scale(scale, scale);
        // What is not chosen is dimmed.
        if let Some(overlay) = self.mask_overlay.borrow().as_ref() {
            cr.set_source_pixbuf(overlay, 0.0, 0.0);
            let _ = cr.paint();
        }
        let outline = self.image_outline.borrow();
        if outline.len() > 1 {
            cr.set_line_width(1.0 / scale);
            for (colour, dash) in [((1.0, 1.0, 1.0), 0.0), ((0.0, 0.0, 0.0), 4.0)] {
                cr.set_source_rgb(colour.0, colour.1, colour.2);
                cr.set_dash(&[4.0 / scale, 4.0 / scale], dash / scale);
                cr.move_to(outline[0].0, outline[0].1);
                outline.iter().skip(1).for_each(|&(x, y)| cr.line_to(x, y));
                let _ = cr.stroke();
            }
            cr.set_dash(&[], 0.0);
        }
        let stroke = self.stroke.borrow();
        let (Some(&from), Some(&to), Some(tool), Some(pixbuf)) = (stroke.first(), stroke.last(), self.tool.get(), self.shown.borrow().clone()) else { return };
        let style = self.style();
        let line = shapes::picture_width(&pixbuf, &style);
        let ants = |cr: &gtk::cairo::Context, lines: &[Vec<(f64, f64)>]| {
            cr.set_line_width(1.0 / scale);
            cr.set_source_rgb(0.21, 0.52, 0.89);
            cr.set_dash(&[4.0 / scale, 3.0 / scale], 0.0);
            for points in lines {
                if let Some(&(x, y)) = points.first() {
                    cr.move_to(x, y);
                }
                points.iter().skip(1).for_each(|&(x, y)| cr.line_to(x, y));
            }
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        };
        match tool {
            Tool::Sketch => shapes::draw_sketch(cr, &stroke, &style, line),
            Tool::Shape => {
                let under = |cr: &gtk::cairo::Context| {
                    cr.set_source_pixbuf(&pixbuf, 0.0, 0.0);
                    let _ = cr.paint();
                };
                shapes::draw_shape(cr, self.shape.get(), from, to, &style, line, &under);
            }
            Tool::Rectangle | Tool::TextBox | Tool::Signature => ants(cr, &shapes::outline(Shape::Rectangle, from, to, 1.0)),
            Tool::Ellipse => ants(cr, &shapes::outline(Shape::Ellipse, from, to, 1.0)),
            Tool::Lasso | Tool::SmartLasso => ants(cr, std::slice::from_ref(&stroke)),
            Tool::InstantAlpha => {
                let reach = (to.0 - from.0).hypot(to.1 - from.1);
                cr.arc(from.0, from.1, reach, 0.0, std::f64::consts::TAU);
                cr.set_source_rgba(0.21, 0.52, 0.89, 0.3);
                let _ = cr.fill();
            }
            _ => {}
        }
    }

    /// The pointer let go over the picture: what the tool makes of the stroke, a part of the
    /// picture chosen or a mark painted on, a text or signature asked for first.
    fn finish_picture_mark(self: &Rc<Self>, dragged: f64) {
        let Some(tool) = self.tool.get() else { return };
        let points = self.stroke.take();
        self.mark_layer.queue_draw();
        let (Some(&from), Some(&to), Some(pixbuf)) = (points.first(), points.last(), self.shown.borrow().clone()) else { return };
        let (width, height) = (pixbuf.width() as usize, pixbuf.height() as usize);
        let clamp = |(x, y): (f64, f64)| (x.clamp(0.0, width as f64), y.clamp(0.0, height as f64));
        let (from, to) = (clamp(from), clamp(to));
        let corners = (from.0, from.1, to.0, to.1);
        let choose = |mask: imaging::Mask, outline: Vec<(f64, f64)>| self.set_mask(Some(mask), outline);
        match tool {
            Tool::Rectangle => choose(imaging::Mask::rectangle(width, height, corners), shapes::outline(Shape::Rectangle, from, to, 1.0).remove(0)),
            Tool::Ellipse => choose(imaging::Mask::ellipse(width, height, corners), shapes::outline(Shape::Ellipse, from, to, 1.0).remove(0)),
            Tool::Lasso => {
                let mut outline: Vec<(f64, f64)> = points.iter().map(|&point| clamp(point)).collect();
                outline.push(outline[0]);
                choose(imaging::Mask::lasso(width, height, &outline), outline);
            }
            Tool::SmartLasso => {
                // The lasso, less what is like the colours it was drawn over: the background
                // around what it was drawn about.
                let mut outline: Vec<(f64, f64)> = points.iter().map(|&point| clamp(point)).collect();
                outline.push(outline[0]);
                let picture = imaging::Rgba::from_pixbuf(&pixbuf);
                let mut mask = imaging::Mask::lasso(width, height, &outline);
                let seeds: Vec<(usize, usize)> = outline.iter().map(|&(x, y)| ((x as usize).min(width - 1), (y as usize).min(height - 1))).collect();
                let around = imaging::flood(&picture, &seeds, 0.1);
                for (inside, background) in mask.inside.iter_mut().zip(&around.inside) {
                    *inside &= !background;
                }
                choose(mask, Vec::new());
            }
            Tool::InstantAlpha => {
                let at = ((from.0 as usize).min(width - 1), (from.1 as usize).min(height - 1));
                let mask = imaging::instant_alpha(&imaging::Rgba::from_pixbuf(&pixbuf), at, dragged);
                choose(mask, Vec::new());
            }
            Tool::Sketch | Tool::Shape => {
                let (style, shape) = (self.style(), self.shape.get());
                if dragged >= 3.0 {
                    self.edit(|pixbuf| shapes::paint(pixbuf, tool, shape, &style, &points, ""));
                }
            }
            Tool::TextBox => {
                let style = self.style();
                let weak = Rc::downgrade(self);
                alert::prompt(&self.window, "文字", "", -1, "加入", move |text| {
                    if let Some(win) = weak.upgrade() {
                        win.edit(|pixbuf| shapes::paint(pixbuf, Tool::TextBox, Shape::Rectangle, &style, &[from], &text));
                    }
                });
            }
            Tool::Signature => {
                let Some(stamp) = self.signature_picture() else { return self.make_signature() };
                let to = if dragged < 3.0 { (from.0 + (width as f64 / 4.0), from.1) } else { to };
                self.edit(|pixbuf| shapes::paint_picture(pixbuf, &stamp, from, to));
            }
            _ => {}
        }
    }

    /// The part of the picture chosen, and the outline drawn around it; none, or an empty
    /// one, lets go of it.
    pub fn set_mask(&self, mask: Option<imaging::Mask>, outline: Vec<(f64, f64)>) {
        let mask = mask.filter(|mask| !mask.is_empty());
        *self.mask_overlay.borrow_mut() = mask.as_ref().map(mask_overlay);
        *self.image_outline.borrow_mut() = if mask.is_some() { outline } else { Vec::new() };
        *self.image_mask.borrow_mut() = mask;
        self.mark_layer.queue_draw();
        self.update_actions();
    }

    /// The signature chosen, as a picture.
    fn signature_picture(&self) -> Option<Pixbuf> {
        let index = self.signature.get()?;
        let (file, _) = self.prefs.borrow().signatures.get(index)?.clone();
        Pixbuf::from_file(prefs::signatures_dir().join(file)).ok()
    }

    fn put_signature_on_page(self: &Rc<Self>, index: usize, from: (f64, f64), to: (f64, f64)) {
        let Some(stamp) = self.signature_picture() else { return self.make_signature() };
        let ratio = f64::from(stamp.height()) / f64::from(stamp.width());
        let (x1, y1, x2, _) = placed(from, to, (160.0, 160.0 * ratio));
        let place = (x1, y1, x2, y1 + (x2 - x1) * ratio);
        let Some(surface) = surface_of(&stamp) else { return };
        self.change_pdf(index, |document, page| annots::stamp(document, page, place, &surface));
    }

    /// The signatures made, in the markup bar's 簽名 menu.
    pub fn fill_signatures(&self) {
        self.signature_menu.remove_all();
        let made = gio::Menu::new();
        for (index, (_, name)) in self.prefs.borrow().signatures.iter().enumerate() {
            made.append(Some(name), Some(&format!("win.signature::{index}")));
        }
        self.signature_menu.append_section(None, &made);
        self.signature_menu.append(Some("製作簽名⋯"), Some("win.manage-signatures"));
    }

    pub fn choose_signature(self: &Rc<Self>, index: Option<usize>) {
        self.signature.set(index);
        self.set_tool(Some(Tool::Signature));
    }

    /// 製作簽名: drawn on the trackpad or with the mouse, or taken from a picture of one signed
    /// on paper, as the Mac takes it from its camera.
    pub fn make_signature(self: &Rc<Self>) {
        let strokes: Rc<RefCell<Vec<Vec<(f64, f64)>>>> = Rc::default();
        let pad = gtk::DrawingArea::builder().content_width(420).content_height(160).css_classes(["pv-sheet"]).build();
        pad.set_draw_func({
            let strokes = strokes.clone();
            move |_, cr, _, _| {
                cr.set_source_rgb(1.0, 1.0, 1.0);
                let _ = cr.paint();
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.set_line_width(2.5);
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                cr.set_line_join(gtk::cairo::LineJoin::Round);
                for stroke in strokes.borrow().iter() {
                    if let Some(&(x, y)) = stroke.first() {
                        cr.move_to(x, y);
                    }
                    stroke.iter().skip(1).for_each(|&(x, y)| cr.line_to(x, y));
                }
                let _ = cr.stroke();
            }
        });
        let draw = gtk::GestureDrag::new();
        draw.connect_drag_begin({
            let (strokes, pad) = (strokes.clone(), pad.clone());
            move |drag, x, y| {
                // The dialog moves the window when dragged; a stroke on the pad is not that.
                drag.set_state(gtk::EventSequenceState::Claimed);
                strokes.borrow_mut().push(vec![(x, y)]);
                pad.queue_draw();
            }
        });
        draw.connect_drag_update({
            let (strokes, pad) = (strokes.clone(), pad.clone());
            move |drag, dx, dy| {
                if let (Some((x, y)), Some(stroke)) = (drag.start_point(), strokes.borrow_mut().last_mut()) {
                    stroke.push((x + dx, y + dy));
                }
                pad.queue_draw();
            }
        });
        pad.add_controller(draw);
        let clear = gtk::Button::builder().label("清除").build();
        clear.connect_clicked({
            let (strokes, pad) = (strokes.clone(), pad.clone());
            move |_| {
                strokes.borrow_mut().clear();
                pad.queue_draw();
            }
        });
        let from_picture = gtk::Button::builder().label("從影像⋯").build();
        let name = gtk::Entry::builder().placeholder_text("名稱").text(format!("簽名 {}", self.prefs.borrow().signatures.len() + 1)).build();
        let buttons = gtk::Box::builder().spacing(GAP).build();
        buttons.append(&clear);
        buttons.append(&from_picture);
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(GAP).build();
        column.append(&pad);
        column.append(&buttons);
        column.append(&name);
        let made: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(self.prefs.borrow().signatures.clone()));
        let list = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        let fill_list: Rc<dyn Fn()> = Rc::new({
            let (list, made, weak) = (list.clone(), made.clone(), Rc::downgrade(self));
            move || {
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
                for (index, (file, label)) in made.borrow().iter().enumerate() {
                    let row = adw::ActionRow::builder().title(glib::markup_escape_text(label)).build();
                    if let Ok(texture) = gdk::Texture::from_filename(prefs::signatures_dir().join(file)) {
                        let picture = gtk::Picture::builder().paintable(&texture).height_request(32).width_request(96).content_fit(gtk::ContentFit::Contain).build();
                        row.add_prefix(&picture);
                    }
                    let remove = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("刪除").css_classes(["flat"]).valign(gtk::Align::Center).build();
                    let (weak, made, file) = (weak.clone(), made.clone(), file.clone());
                    remove.connect_clicked(move |button| {
                        made.borrow_mut().retain(|(kept, _)| *kept != file);
                        let _ = std::fs::remove_file(prefs::signatures_dir().join(&file));
                        if let Some(win) = weak.upgrade() {
                            win.prefs.borrow_mut().signatures = made.borrow().clone();
                            win.prefs.borrow().save();
                            win.fill_signatures();
                        }
                        if let Some(row) = button.ancestor(adw::ActionRow::static_type()) {
                            row.set_visible(false);
                        }
                        let _ = index;
                    });
                    row.add_suffix(&remove);
                    list.append(&row);
                }
            }
        });
        fill_list();
        column.append(&list);
        let alert = adw::AlertDialog::new(Some("簽名"), None);
        alert.set_extra_child(Some(&column));
        alert.add_responses(&[("cancel", "取消"), ("do", "完成")]);
        alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
        alert.set_close_response("cancel");
        from_picture.connect_clicked({
            let (weak, name, made, fill_list) = (Rc::downgrade(self), name.clone(), made.clone(), fill_list.clone());
            move |_| {
                let Some(win) = weak.upgrade() else { return };
                let dialog = gtk::FileDialog::builder().title("從影像").build();
                let (weak, name, made, fill_list) = (weak.clone(), name.clone(), made.clone(), fill_list.clone());
                dialog.open(Some(&win.window), gio::Cancellable::NONE, move |chosen| {
                    let (Some(win), Ok(file)) = (weak.upgrade(), chosen) else { return };
                    let Some(pixbuf) = file.path().and_then(|path| load_picture(&path).ok()) else {
                        return alert::notice(&win.window, "無法打開", None);
                    };
                    // Ink on paper: the paper made clear, the ink kept black.
                    let mut picture = imaging::Rgba::from_pixbuf(&pixbuf);
                    for pixel in picture.bytes.chunks_exact_mut(4) {
                        let light = (u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3;
                        pixel[3] = if light > 150 { 0 } else { (255 - light * 255 / 150).min(255) as u8 };
                        pixel[..3].fill(0);
                    }
                    let cut = imaging::cut(&picture, &imaging::Mask { width: picture.width, height: picture.height, inside: picture.bytes.chunks_exact(4).map(|pixel| pixel[3] > 0).collect() });
                    if let Some(signature) = cut {
                        win.keep_signature(&signature.to_pixbuf(), &name.text(), &made);
                        fill_list();
                    }
                });
            }
        });
        let weak = Rc::downgrade(self);
        alert.choose(&self.window, gio::Cancellable::NONE, move |response| {
            let Some(win) = weak.upgrade() else { return };
            if response != "do" || strokes.borrow().is_empty() {
                win.fill_signatures();
                return;
            }
            // The strokes drawn, cut to what they cover, at twice the pad's size.
            let all: Vec<(f64, f64)> = strokes.borrow().iter().flatten().copied().collect();
            let (x1, y1, x2, y2) = all.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(a, b, c, d), &(x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)));
            let pad = 6.0;
            let (width, height) = (((x2 - x1 + pad * 2.0) * 2.0) as i32, ((y2 - y1 + pad * 2.0) * 2.0) as i32);
            let Ok(surface) = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width.max(1), height.max(1)) else { return };
            if let Ok(cr) = gtk::cairo::Context::new(&surface) {
                cr.scale(2.0, 2.0);
                cr.translate(pad - x1, pad - y1);
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.set_line_width(2.5);
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                cr.set_line_join(gtk::cairo::LineJoin::Round);
                for stroke in strokes.borrow().iter() {
                    if let Some(&(x, y)) = stroke.first() {
                        cr.move_to(x, y);
                    }
                    stroke.iter().skip(1).for_each(|&(x, y)| cr.line_to(x, y));
                }
                let _ = cr.stroke();
            }
            surface.flush();
            #[allow(deprecated)]
            let pixbuf = gdk::pixbuf_get_from_surface(&surface, 0, 0, width.max(1), height.max(1));
            if let Some(pixbuf) = pixbuf {
                win.keep_signature(&pixbuf, &name.text(), &made);
            }
        });
    }

    /// Keeps a signature made, and chooses it.
    fn keep_signature(self: &Rc<Self>, pixbuf: &Pixbuf, name: &str, made: &RefCell<Vec<(String, String)>>) {
        let dir = prefs::signatures_dir();
        let file = format!("{}.png", glib::real_time());
        let written = std::fs::create_dir_all(&dir).map_err(|err| err.to_string()).and_then(|()| pixbuf.savev(dir.join(&file), "png", &[]).map_err(|err| err.to_string()));
        if let Err(err) = written {
            return alert::notice(&self.window, "無法儲存", Some(&err));
        }
        let name = if name.trim().is_empty() { String::from("簽名") } else { name.trim().to_owned() };
        made.borrow_mut().push((file, name));
        self.prefs.borrow_mut().signatures = made.borrow().clone();
        self.prefs.borrow().save();
        self.fill_signatures();
        self.choose_signature(Some(made.borrow().len() - 1));
    }
}

/// What a selection leaves out, dimmed, as a picture to draw over the one chosen from.
fn mask_overlay(mask: &imaging::Mask) -> Pixbuf {
    let mut bytes = Vec::with_capacity(mask.inside.len() * 4);
    for inside in &mask.inside {
        bytes.extend_from_slice(if *inside { &[0, 0, 0, 0] } else { &[0, 0, 0, 110] });
    }
    imaging::Rgba { width: mask.width, height: mask.height, bytes }.to_pixbuf()
}

/// The PDF as it is now, written to the scratch folder for 還原.
fn save_snapshot(pdf: &Pdf) -> Option<PathBuf> {
    let dir = scratch_dir().join("undo");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.pdf", glib::monotonic_time()));
    match pdf.document().save(&gio::File::for_path(&path).uri()) {
        Ok(()) => Some(path),
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            None
        }
    }
}
