//! The drop-down window, as Nekominal's on Win+`: Super+` in Nekoland runs `nekoland-terminal drop-down`, which shows a
//! window of its own across the top of the screen, or hides it again. Its tabs stay while it is hidden, and it is not
//! kept for the next start. Where it goes is the compositor's to say: it is put there each time it comes up.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::glib;

use super::{super::ipc, all_windows, Window};

/// What the compositor is asked to do with the window once it has the keyboard.
const PLACE_MESSAGE: &str = "window drop-down";
/// How long a window just shown waits for the compositor to have mapped and focused it, as Nekoland's launcher does.
const UNTIL_FOCUSED: std::time::Duration = std::time::Duration::from_millis(150);

/// The drop-down window, when one was made.
fn drop_down_window() -> Option<Rc<Window>> {
    all_windows().into_iter().find(|window| window.drop_down.get())
}

/// Shows the drop-down window, making it the first time, or hides it when it is the one in front.
pub fn toggle_drop_down(app: &adw::Application) {
    match drop_down_window() {
        Some(window) if window.window.is_visible() && window.window.is_active() => window.window.set_visible(false),
        Some(window) => show(&window),
        None => {
            let window = Window::new(app);
            window.drop_down.set(true);
            window.open_tab(None, None, None);
            show(&window);
        }
    }
}

pub(super) fn show(window: &Rc<Window>) {
    place_once_mapped(&window.window);
    window.window.present();
}

/// Asks the compositor to put the window across the top once it is on screen and focused, which is the window its
/// command acts on. GTK still says a window hidden while active is active, so its being shown is what is waited for.
fn place_once_mapped(window: &adw::ApplicationWindow) {
    let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
    let kept = handler.clone();
    let id = window.connect_map(move |window| {
        if let Some(id) = kept.take() {
            window.disconnect(id);
        }
        glib::timeout_add_local_once(UNTIL_FOCUSED, || {
            if ipc::msg(PLACE_MESSAGE).is_none() {
                eprintln!("nekoland-terminal: Nekoland could not be asked to place the drop-down window");
            }
        });
    });
    handler.replace(Some(id));
}
