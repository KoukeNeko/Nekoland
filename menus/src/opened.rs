//! A button with its menu open looks chosen, as GTK's own menu button does: solid ink.
//!
//! GTK's menu button is a toggle, checked while its menu shows, and the stylesheet draws
//! that. A program that opens a popover from a plain button of its own, as a browser's
//! site menu, leaves the button as it was, and a stylesheet cannot tell that a popover is
//! showing. So each popover says so itself: shown, it marks the button it belongs to with
//! a style class, and takes the mark off when it goes.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::sync::OnceLock;

use crate::{find, GType};

/// The class a button has while a popover of its own is showing.
const OPENED: &CStr = c"nekoland-opened";
const MAP: &CStr = c"map";
const UNMAP: &CStr = c"unmap";

/// What a popover calls back with when it is shown or goes: itself.
type Shown = unsafe extern "C" fn(*mut c_void, *mut c_void);

struct Gtk {
    button_type: GType,
    is_a: unsafe extern "C" fn(*mut c_void, GType) -> c_int,
    parent: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    add_class: unsafe extern "C" fn(*mut c_void, *const c_char),
    remove_class: unsafe extern "C" fn(*mut c_void, *const c_char),
    connect: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        Shown,
        *mut c_void,
        *mut c_void,
        c_uint,
    ) -> std::ffi::c_ulong,
}

static GTK: OnceLock<Option<Gtk>> = OnceLock::new();

unsafe fn function<T>(name: &CStr) -> Option<T> {
    // A pointer to a function is as large as any other.
    find(name).map(|symbol| std::mem::transmute_copy(&symbol))
}

unsafe fn load() -> Option<Gtk> {
    let button_type = function::<unsafe extern "C" fn() -> GType>(c"gtk_button_get_type")?;
    Some(Gtk {
        button_type: button_type(),
        is_a: function(c"g_type_check_instance_is_a")?,
        parent: function(c"gtk_widget_get_parent")?,
        add_class: function(c"gtk_widget_add_css_class")?,
        remove_class: function(c"gtk_widget_remove_css_class")?,
        connect: function(c"g_signal_connect_data")?,
    })
}

/// The button a popover belongs to, if it is a button's own.
unsafe fn button_of(gtk: &Gtk, popover: *mut c_void) -> Option<*mut c_void> {
    let parent = (gtk.parent)(popover);
    (!parent.is_null() && (gtk.is_a)(parent, gtk.button_type) != 0).then_some(parent)
}

unsafe extern "C" fn shown(popover: *mut c_void, _data: *mut c_void) {
    if let Some(gtk) = GTK.get().and_then(Option::as_ref) {
        if let Some(button) = button_of(gtk, popover) {
            (gtk.add_class)(button, OPENED.as_ptr());
        }
    }
}

unsafe extern "C" fn gone(popover: *mut c_void, _data: *mut c_void) {
    if let Some(gtk) = GTK.get().and_then(Option::as_ref) {
        if let Some(button) = button_of(gtk, popover) {
            (gtk.remove_class)(button, OPENED.as_ptr());
        }
    }
}

/// Has a popover just made mark the button it belongs to while it shows.
pub unsafe fn watch(popover: *mut c_void) {
    // The buttons' class is made the first time this is asked, well after the popovers'.
    let Some(gtk) = GTK.get_or_init(|| load()) else {
        return;
    };
    let nothing = std::ptr::null_mut();
    (gtk.connect)(popover, MAP.as_ptr(), shown, nothing, nothing, 0);
    (gtk.connect)(popover, UNMAP.as_ptr(), gone, nothing, nothing, 0);
}
