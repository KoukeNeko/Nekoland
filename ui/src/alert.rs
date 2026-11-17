//! What the alerts of Nekoland's own programs add to libadwaita's, and the alerts they all
//! ask the same way: a notice, a confirmation before what cannot be undone, and a prompt
//! for one line of text.

// Each program uses part of this.
#![allow(dead_code)]

use adw::prelude::*;
use gtk::{gio, glib};

/// How wide a field in an alert is: the design language's dialog, less its padding.
pub const FIELD_WIDTH: i32 = 320;

/// Tells what happened, with nothing to decide.
pub fn notice(parent: &impl IsA<gtk::Widget>, title: &str, body: Option<&str>) {
    let alert = adw::AlertDialog::new(Some(title), body);
    alert.add_response("close", "關閉");
    alert.present(Some(parent));
}

/// Asks before something that cannot be undone; `then` does it.
pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    body: Option<&str>,
    verb: &str,
    then: impl FnOnce() + 'static,
) {
    let alert = adw::AlertDialog::new(Some(title), body);
    alert.add_responses(&[("cancel", "取消"), ("do", verb)]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Destructive);
    alert.set_default_response(Some("cancel"));
    alert.set_close_response("cancel");
    alert.choose(parent, gio::Cancellable::NONE, move |response| {
        if response == "do" {
            then();
        }
    });
}

/// Asks for one line of text, starting from `text` with its first `chosen` characters chosen
/// (-1 for all); `then` gets what was typed, trimmed, unless it is empty. The field is
/// returned for what else it needs, such as a placeholder.
pub fn prompt(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    text: &str,
    chosen: i32,
    verb: &str,
    then: impl FnOnce(String) + 'static,
) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .text(text)
        .width_request(FIELD_WIDTH)
        .activates_default(true)
        .build();
    let alert = adw::AlertDialog::new(Some(title), None);
    alert.set_extra_child(Some(&entry));
    alert.add_responses(&[("cancel", "取消"), ("do", verb)]);
    alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("do"));
    alert.set_close_response("cancel");
    // Focusing a field chooses all of it, so the part to type over is chosen once it is shown.
    entry.connect_map(move |entry| {
        let entry = entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
            entry.select_region(0, chosen);
        });
    });
    let typed = entry.clone();
    alert.choose(parent, gio::Cancellable::NONE, move |response| {
        let text = typed.text().trim().to_owned();
        if response == "do" && !text.is_empty() {
            then(text);
        }
    });
    entry
}

/// Shows `icon` above the alert's question. libadwaita's alert has no place for one, so it goes
/// first in the box that holds the question and everything under it.
pub fn set_icon(alert: &adw::AlertDialog, icon: &str) {
    let Some(area) = alert.child().as_ref().and_then(message_area) else {
        eprintln!("alert: no message area to put the {icon} icon in");
        return;
    };
    area.prepend(
        &gtk::Image::builder()
            .icon_name(icon)
            .pixel_size(32)
            .css_classes(["alert-icon"])
            .build(),
    );
}

fn message_area(widget: &gtk::Widget) -> Option<gtk::Box> {
    if widget.has_css_class("message-area") {
        return widget.clone().downcast().ok();
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(area) = message_area(&widget) {
            return Some(area);
        }
        child = widget.next_sibling();
    }
    None
}
