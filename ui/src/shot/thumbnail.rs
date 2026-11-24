//! The thumbnail of a picture just taken, in the bottom right corner as the Mac's, which
//! the compositor keeps in front without the keyboard. It goes by itself after a few
//! seconds, unless the pointer rests on it, and at once when swiped right; dragged, it is the
//! picture, dropped into a window; clicked, it opens in 截圖's window to mark up.
//!
//! It is a program of its own, under an application ID the compositor knows.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::Duration,
};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::img::Img;

pub const APP_ID: &str = "io.github.koukeneko.Nekoland.ShotThumbnail";
/// As long as the Mac's stays.
const SHOWN: Duration = Duration::from_secs(5);
/// The longest side, in points.
const SIDE: f64 = 200.0;
/// How far a swipe goes before the thumbnail goes, in a touchpad's points.
const SWIPE: f64 = 40.0;

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| "nekoland-shot".into())
}

/// Shows the thumbnail of a history entry, in a program of its own; `saved` is where the
/// picture was saved, if it was.
pub fn open(entry: &Path, saved: Option<&Path>) {
    let mut command = Command::new(exe());
    command.arg("thumbnail").arg(entry);
    if let Some(saved) = saved {
        command.arg(saved);
    }
    if let Err(err) = command.spawn() {
        eprintln!("nekoland-shot thumbnail: {err}");
    }
}

pub fn show(app: &adw::Application, entry: PathBuf, saved: Option<PathBuf>) {
    let img = match Img::load(&entry.join("base.png")) {
        Ok(img) => img,
        Err(err) => return eprintln!("nekoland-shot thumbnail: {err}"),
    };
    let k = SIDE / img.w.max(img.h).max(1) as f64;
    let (w, h) = ((img.w as f64 * k).round(), (img.h as f64 * k).round());
    // Drawn at twice its size in points, for a dense display.
    let small = img.scaled((w * 2.0) as i32, (h * 2.0) as i32).texture();
    let picture = gtk::Picture::builder()
        .paintable(&small)
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Fill)
        .width_request(w as i32)
        .height_request(h as i32)
        .overflow(gtk::Overflow::Hidden)
        .build();
    picture.add_css_class("shot-thumbnail-picture");
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("截圖")
        .decorated(false)
        // Its size in points, not its picture's in pixels, which a fixed size would take.
        .default_width(w as i32)
        .default_height(h as i32)
        .child(&picture)
        .build();
    window.add_css_class("shot-thumbnail");

    // The time it has left, restarted when the pointer leaves it.
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let start = {
        let timer = timer.clone();
        let window = window.clone();
        move || {
            let window = window.clone();
            let id = glib::timeout_add_local_once(SHOWN, move || window.close());
            if let Some(old) = timer.replace(Some(id)) {
                old.remove();
            }
        }
    };
    start();
    let hover = gtk::EventControllerMotion::new();
    {
        let timer = timer.clone();
        hover.connect_enter(move |_, _, _| {
            if let Some(id) = timer.take() {
                id.remove();
            }
        });
    }
    {
        let start = start.clone();
        hover.connect_leave(move |_| start());
    }
    window.add_controller(hover);

    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    {
        let window = window.clone();
        let entry = entry.clone();
        click.connect_released(move |_, _, _, _| {
            if let Err(err) = Command::new(exe()).arg("edit").arg(&entry).spawn() {
                eprintln!("nekoland-shot edit: {err}");
            }
            window.close();
        });
    }
    picture.add_controller(click);

    let swipe = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::HORIZONTAL);
    {
        let window = window.clone();
        let travelled = RefCell::new(0.0);
        swipe.connect_scroll(move |controller, dx, _| {
            // A wheel's notch is a step, a touchpad's travel points.
            let unit = if controller.unit() == gdk::ScrollUnit::Wheel {
                SWIPE
            } else {
                1.0
            };
            let mut travelled = travelled.borrow_mut();
            *travelled = (*travelled + dx * unit).max(0.0);
            if *travelled >= SWIPE {
                window.close();
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(swipe);

    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::COPY);
    // Where the pointer took it, which it is held by as it goes.
    let held = Rc::new(std::cell::Cell::new((0.0, 0.0)));
    {
        let img = Rc::new(img);
        let held = held.clone();
        drag.connect_prepare(move |_, x, y| {
            held.set((x, y));
            // The file where it was saved, for file managers and uploads, and the picture
            // itself for whatever takes pictures.
            let picture = gdk::ContentProvider::for_value(&img.texture().to_value());
            Some(match &saved {
                Some(saved) => gdk::ContentProvider::new_union(&[
                    gdk::ContentProvider::for_value(&gio::File::for_path(saved).to_value()),
                    picture,
                ]),
                None => picture,
            })
        });
    }
    {
        // The thumbnail as it shows, at its size in points; the texture alone would be
        // drawn at its pixels, twice that.
        let shown = gtk::WidgetPaintable::new(Some(&picture));
        drag.connect_drag_begin(move |source, _| {
            let (x, y) = held.get();
            source.set_icon(Some(&shown), x as i32, y as i32);
        });
    }
    {
        let window = window.clone();
        drag.connect_drag_end(move |_, _, _| window.close());
    }
    picture.add_controller(drag);

    window.present();
}
