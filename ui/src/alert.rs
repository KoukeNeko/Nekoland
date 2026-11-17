//! What the alerts of Nekoland's own programs add to libadwaita's.

use adw::prelude::*;

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
