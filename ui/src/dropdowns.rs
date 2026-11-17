//! Drop-down menus without GTK's tail, opening as the Mac's pop-up buttons do. GTK draws the
//! tail at a fixed size whenever a popover has its arrow, which no stylesheet can take away,
//! and a ComboRow keeps its popover to itself.

use adw::prelude::*;
use gtk::glib::{self, gobject_ffi, translate::IntoGlib};

/// Turns the tail off for every drop-down the application shows from now on: each, as it is
/// mapped, which is before its menu can open.
pub fn drop_tails() {
    // SAFETY: "map" is GtkWidget's, registered once GTK has started, as it has by an
    // application's startup; the hook carries no data, so there is nothing to free.
    unsafe {
        let map = gobject_ffi::g_signal_lookup(c"map".as_ptr(), gtk::Widget::static_type().into_glib());
        gobject_ffi::g_signal_add_emission_hook(map, 0, Some(on_map), std::ptr::null_mut(), None);
    }
}

unsafe extern "C" fn on_map(
    _hint: *mut gobject_ffi::GSignalInvocationHint,
    _count: u32,
    params: *const gobject_ffi::GValue,
    _data: glib::ffi::gpointer,
) -> glib::ffi::gboolean {
    // SAFETY: a signal's first parameter is the instance it is emitted on, and glib::Value is
    // a GValue.
    let instance = unsafe { &*params.cast::<glib::Value>() };
    if let Ok(widget) = instance.get::<gtk::Widget>() {
        if widget.is::<adw::ComboRow>() || widget.is::<gtk::DropDown>() {
            drop_tails_within(&widget);
        }
    }
    // Kept for the next widget mapped.
    glib::ffi::GTRUE
}

fn drop_tails_within(widget: &gtk::Widget) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(popover) = current.downcast_ref::<gtk::Popover>() {
            popover.set_has_arrow(false);
        }
        drop_tails_within(&current);
        child = current.next_sibling();
    }
}
