//! Loaded into each program Nekoland starts, through `LD_PRELOAD`, it makes GTK 4's menus
//! as the Mac's are: a submenu opens beside its menu, on the right or on the left where the
//! right has no room, instead of sliding in over it, and no popover has an arrow.
//!
//! GTK has no setting for either: each popover takes them from the program's own code, and
//! GTK's calls into itself cannot be caught from outside. So `GtkPopover`'s class is changed
//! before any popover is made, and the classes made from it, the menus' among them, take
//! the change from it: `constructed` takes a new popover's arrow away, and `notify` takes
//! it away again when the program puts it back, and sets a menu to open its submenus
//! beside it when it gets its menu. A class `notify` also makes GLib announce changes no
//! program is listening for, which it otherwise leaves out.
//!
//! Nothing is linked: GTK's functions are looked up in the program, and a program without
//! GTK 4.14 or later is left as it is.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::sync::OnceLock;

type GType = usize;
type Notify = unsafe extern "C" fn(*mut c_void, *mut GParamSpec);
type Constructed = unsafe extern "C" fn(*mut c_void);

/// `GTK_POPOVER_MENU_NESTED`: submenus open as menus of their own, beside the one they are in.
const NESTED: c_uint = 1;

/// The start of GLib's `GParamSpec`, as far as its name.
#[allow(dead_code)]
#[repr(C)]
struct GParamSpec {
    g_type_instance: *mut c_void,
    name: *const c_char,
}

/// The start of GLib's `GObjectClass`, as far as `constructed`.
#[allow(dead_code)]
#[repr(C)]
struct GObjectClass {
    g_type: GType,
    construct_properties: *mut c_void,
    constructor: *mut c_void,
    set_property: *mut c_void,
    get_property: *mut c_void,
    dispose: *mut c_void,
    finalize: *mut c_void,
    dispatch_properties_changed: *mut c_void,
    notify: Option<Notify>,
    constructed: Option<Constructed>,
}

struct Gtk {
    menu_type: GType,
    is_a: unsafe extern "C" fn(*mut c_void, GType) -> c_int,
    get_flags: unsafe extern "C" fn(*mut c_void) -> c_uint,
    set_flags: unsafe extern "C" fn(*mut c_void, c_uint),
    set_has_arrow: unsafe extern "C" fn(*mut c_void, c_int),
    get_has_arrow: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// The class's own `notify` and `constructed`, if it had them, still called.
    notify: Option<Notify>,
    constructed: Option<Constructed>,
}

static GTK: OnceLock<Gtk> = OnceLock::new();

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `RTLD_DEFAULT`: every object the program has loaded.
const EVERYWHERE: *mut c_void = std::ptr::null_mut();

unsafe fn find(name: &CStr) -> Option<*mut c_void> {
    let symbol = dlsym(EVERYWHERE, name.as_ptr());
    (!symbol.is_null()).then_some(symbol)
}

unsafe extern "C" fn constructed(popover: *mut c_void) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    if let Some(constructed) = gtk.constructed {
        constructed(popover);
    }
    (gtk.set_has_arrow)(popover, 0);
}

unsafe extern "C" fn notify(popover: *mut c_void, pspec: *mut GParamSpec) {
    let Some(gtk) = GTK.get() else {
        return;
    };
    let name = CStr::from_ptr((*pspec).name).to_bytes();
    if name == b"has-arrow" && (gtk.get_has_arrow)(popover) != 0 {
        (gtk.set_has_arrow)(popover, 0);
    }
    // A menu is given its model after its flags, and the flags can be set again later.
    if (name == b"menu-model" || name == b"flags") && (gtk.is_a)(popover, gtk.menu_type) != 0 {
        let flags = (gtk.get_flags)(popover);
        if flags & NESTED == 0 {
            (gtk.set_flags)(popover, flags | NESTED);
        }
    }
    if let Some(notify) = gtk.notify {
        notify(popover, pspec);
    }
}

unsafe extern "C" fn init() {
    let (
        Some(popover_type),
        Some(menu_type),
        Some(class_ref),
        Some(is_a),
        Some(get_flags),
        Some(set_flags),
        Some(get_has_arrow),
        Some(set_has_arrow),
    ) = (
        find(c"gtk_popover_get_type"),
        find(c"gtk_popover_menu_get_type"),
        find(c"g_type_class_ref"),
        find(c"g_type_check_instance_is_a"),
        find(c"gtk_popover_menu_get_flags"),
        find(c"gtk_popover_menu_set_flags"),
        find(c"gtk_popover_get_has_arrow"),
        find(c"gtk_popover_set_has_arrow"),
    )
    else {
        return;
    };
    let popover_type: unsafe extern "C" fn() -> GType = std::mem::transmute(popover_type);
    let menu_type: unsafe extern "C" fn() -> GType = std::mem::transmute(menu_type);
    let class_ref: unsafe extern "C" fn(GType) -> *mut GObjectClass = std::mem::transmute(class_ref);
    // Only GtkPopover's class is made here: the menus' is made later, from it as it is then.
    // Kept for as long as the program runs, so the change stays.
    let class = class_ref(popover_type());
    let _ = GTK.set(Gtk {
        menu_type: menu_type(),
        is_a: std::mem::transmute(is_a),
        get_flags: std::mem::transmute(get_flags),
        set_flags: std::mem::transmute(set_flags),
        get_has_arrow: std::mem::transmute(get_has_arrow),
        set_has_arrow: std::mem::transmute(set_has_arrow),
        notify: (*class).notify,
        constructed: (*class).constructed,
    });
    (*class).notify = Some(notify);
    (*class).constructed = Some(constructed);
}

/// Runs when the library is loaded, after the libraries the program was linked with.
#[used]
#[link_section = ".init_array"]
static INIT: unsafe extern "C" fn() = init;
