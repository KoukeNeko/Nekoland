//! Quick Look: what is chosen, large, without opening it. Pictures and PDFs as pictures,
//! videos and music played, text read, anything else its thumbnail and what it is. The
//! space bar opens and closes it; with several chosen, its arrows go through them, and
//! with one, through the folder.

use std::{
    cell::{Cell, RefCell},
    io::Read,
    rc::{Rc, Weak},
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{
    browse::{info_file, is_dir, EDGE, GAP},
    pane, thumbs, Window,
};

/// How much of a text file is read to be shown.
const TEXT_LIMIT: u64 = 256 << 10;
const PICTURE_SIZE: i32 = 2048;

pub struct QuickLook {
    win: Weak<Window>,
    pub window: adw::Window,
    stack: gtk::Stack,
    picture: gtk::Picture,
    video: gtk::Video,
    text: gtk::TextView,
    icon: gtk::Image,
    about: gtk::Label,
    open: gtk::Button,
    pager: gtk::Box,
    page: gtk::Label,
    /// What it pages through when several were chosen as it opened.
    items: RefCell<Vec<gio::FileInfo>>,
    index: Cell<usize>,
    shown: RefCell<String>,
}

impl QuickLook {
    pub fn new(win: &Rc<Window>) -> Rc<QuickLook> {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .build();
        let video = gtk::Video::builder().autoplay(true).build();
        let text = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(EDGE)
            .bottom_margin(EDGE)
            .left_margin(EDGE)
            .right_margin(EDGE)
            .build();
        let icon = gtk::Image::builder().pixel_size(160).build();
        let about = gtk::Label::builder()
            .css_classes(["dim-label"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let other = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(EDGE)
            .valign(gtk::Align::Center)
            .build();
        other.append(&icon);
        other.append(&about);
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .vexpand(true)
            .build();
        let picture_frame = gtk::Box::builder()
            .margin_top(EDGE)
            .margin_bottom(EDGE)
            .margin_start(EDGE)
            .margin_end(EDGE)
            .build();
        picture.set_hexpand(true);
        picture_frame.append(&picture);
        stack.add_named(&picture_frame, Some("picture"));
        stack.add_named(&video, Some("video"));
        stack.add_named(&crate::browse::scrolled(&text), Some("text"));
        stack.add_named(&other, Some("other"));

        let open = gtk::Button::new();
        let previous = crate::browse::icon_button("go-previous-symbolic", "上一個");
        let next = crate::browse::icon_button("go-next-symbolic", "下一個");
        let page = gtk::Label::builder()
            .css_classes(["dim-label", "numeric", "nf-small"])
            .build();
        let pager = gtk::Box::new(gtk::Orientation::Horizontal, GAP / 2);
        pager.append(&previous);
        pager.append(&page);
        pager.append(&next);
        let header = adw::HeaderBar::new();
        header.pack_start(&pager);
        header.pack_end(&open);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));
        // The app's own, so the Dock and the menu bar count it as Files, and gone with the
        // window it previews for, so a hidden one does not keep Files running.
        let window = adw::Window::builder()
            .transient_for(&win.window)
            .destroy_with_parent(true)
            .default_width(760)
            .default_height(560)
            .hide_on_close(true)
            .content(&view)
            .build();
        // Added, not set: setting the classes would drop the window's own.
        window.add_css_class("nf-quick-look");
        if let Some(app) = win.window.application() {
            window.set_application(Some(&app));
        }

        let look = Rc::new(QuickLook {
            win: Rc::downgrade(win),
            window,
            stack,
            picture,
            video,
            text,
            icon,
            about,
            open,
            pager,
            page,
            items: RefCell::new(Vec::new()),
            index: Cell::new(0),
            shown: RefCell::new(String::new()),
        });
        previous.connect_clicked(on!(&look, |q, _b| q.turn(-1)));
        next.connect_clicked(on!(&look, |q, _b| q.turn(1)));
        look.open.connect_clicked(on!(&look, |q, _b| q.open_shown()));
        look.window.connect_hide(on!(&look, |q, _w| {
            q.video.set_media_stream(None::<&gtk::MediaStream>);
            q.shown.replace(String::new());
            if let Some(content) = q.window.content() {
                content.remove_css_class("nf-zoom-in");
            }
        }));
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&look);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(look) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::space | gdk::Key::Escape => look.window.close(),
                gdk::Key::Left | gdk::Key::Up => look.turn(-1),
                gdk::Key::Right | gdk::Key::Down => look.turn(1),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        look.window.add_controller(keys);
        look
    }

    /// Opens on `chosen`: paging through them when there are several.
    pub fn open_on(&self, chosen: Vec<gio::FileInfo>) {
        if chosen.is_empty() {
            return;
        }
        self.pager.set_visible(chosen.len() > 1);
        self.items.replace(chosen);
        self.index.set(0);
        self.show_index();
        // Its content grows into place, as the Finder's zooms out of the item.
        if let Some(content) = self.window.content() {
            content.add_css_class("nf-zoom-in");
        }
        self.window.present();
    }

    /// What is chosen changed while it is open: it follows, unless it pages through several.
    pub fn follow(&self, chosen: &[gio::FileInfo]) {
        if !self.window.is_visible() || self.items.borrow().len() > 1 {
            return;
        }
        if let [one] = chosen {
            self.items.replace(vec![one.clone()]);
            self.index.set(0);
            self.show_index();
        }
    }

    fn turn(&self, by: i32) {
        let n = self.items.borrow().len();
        if n > 1 {
            self.index
                .set((self.index.get() as i32 + by).rem_euclid(n as i32) as usize);
            self.show_index();
        } else if let Some(pane) = self.win.upgrade().and_then(|w| w.pane()) {
            pane.move_selection(by);
        }
    }

    fn show_index(&self) {
        let items = self.items.borrow();
        let Some(info) = items.get(self.index.get()) else {
            return;
        };
        self.page
            .set_label(&format!("{} / {}", self.index.get() + 1, items.len()));
        self.show(info);
    }

    fn show(&self, info: &gio::FileInfo) {
        let file = info_file(info);
        let uri = file.as_ref().map(|f| f.uri().to_string()).unwrap_or_default();
        if *self.shown.borrow() == uri {
            return;
        }
        self.shown.replace(uri.clone());
        self.window.set_title(Some(&info.display_name()));
        self.video.set_media_stream(None::<&gtk::MediaStream>);
        let content_type = info.content_type().map(|t| t.to_string()).unwrap_or_default();
        let app = (!is_dir(info))
            .then(|| gio::AppInfo::default_for_type(&content_type, false))
            .flatten();
        self.open.set_visible(app.is_some());
        if let Some(app) = &app {
            self.open.set_label(&format!("以「{}」打開", app.display_name()));
        }
        self.show_other(info);
        let Some(path) = file.as_ref().and_then(|f| f.path()) else {
            return;
        };
        if content_type.starts_with("image/") || content_type == "application/pdf" {
            let (picture, stack, shown) = (
                self.picture.downgrade(),
                self.stack.downgrade(),
                self.shown.clone(),
            );
            thumbs::read(&path, PICTURE_SIZE, move |texture| {
                let (Some(picture), Some(stack), Some(texture)) =
                    (picture.upgrade(), stack.upgrade(), texture)
                else {
                    return;
                };
                if *shown.borrow() == uri {
                    picture.set_paintable(Some(&texture));
                    stack.set_visible_child_name("picture");
                }
            });
        } else if content_type.starts_with("video/") || content_type.starts_with("audio/") {
            self.video.set_file(file.as_ref());
            self.stack.set_visible_child_name("video");
        } else if is_text(&content_type) {
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path).and_then(|f| f.take(TEXT_LIMIT).read_to_end(&mut bytes));
            if read.is_ok() {
                self.text.buffer().set_text(&String::from_utf8_lossy(&bytes));
                self.stack.set_visible_child_name("text");
            }
        }
    }

    /// A folder's or any other file's thumbnail and what it is.
    fn show_other(&self, info: &gio::FileInfo) {
        let picture = gtk::Picture::new();
        thumbs::show(&picture, &self.icon, info, info_file(info).as_ref());
        let kind = pane::kind_label(info);
        let about = if is_dir(info) {
            let count = info_file(info)
                .and_then(|f| {
                    f.enumerate_children(
                        "standard::name",
                        gio::FileQueryInfoFlags::NONE,
                        gio::Cancellable::NONE,
                    )
                    .ok()
                })
                .map_or(0, |children| children.count());
            format!("{kind} · {count} 個項目")
        } else {
            format!("{kind} · {}", glib::format_size(info.size() as u64))
        };
        self.about.set_label(&about);
        self.stack.set_visible_child_name("other");
    }

    fn open_shown(&self) {
        let items = self.items.borrow().clone();
        if let Some(info) = items.get(self.index.get()) {
            crate::ops::open(&self.win, std::slice::from_ref(info));
        }
        self.window.close();
    }
}

fn is_text(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || gio::content_type_is_a(content_type, "text/plain")
        || matches!(
            content_type,
            "application/json" | "application/xml" | "application/x-shellscript" | "application/toml"
        )
}
