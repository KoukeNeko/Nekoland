//! The web browser links open in, for Nekoland's programs that show or set it.

// Each program uses part of this.
#![allow(dead_code)]

use std::time::Duration;

use gtk::{gio, glib, prelude::*};

/// What a web browser is the default for.
pub const WEB_TYPES: [&str; 3] = ["x-scheme-handler/http", "x-scheme-handler/https", "text/html"];

/// How long a newly installed program's desktop entry is waited for, in tries a quarter of a
/// second apart: GIO notices new entries through a file monitor, not at once.
const LISTING_TRIES: u32 = 20;
const LISTING_INTERVAL: Duration = Duration::from_millis(250);

/// The programs that can open web links.
pub fn installed() -> Vec<gio::AppInfo> {
    gio::AppInfo::all_for_type(WEB_TYPES[0])
}

pub fn default() -> Option<gio::AppInfo> {
    gio::AppInfo::default_for_type(WEB_TYPES[0], false)
}

pub fn make_default(app: &gio::AppInfo) {
    for kind in WEB_TYPES {
        if let Err(err) = app.set_as_default_for_type(kind) {
            eprintln!("{kind}: {err}");
        }
    }
}

/// Calls `then` with what `find` finds once GIO lists it, or with None if it never does.
pub fn when_listed(
    find: impl Fn() -> Option<gio::AppInfo> + 'static,
    then: impl FnOnce(Option<gio::AppInfo>) + 'static,
) {
    let mut then = Some(then);
    let mut tries = 0;
    glib::timeout_add_local(LISTING_INTERVAL, move || {
        tries += 1;
        let found = find();
        if found.is_none() && tries < LISTING_TRIES {
            return glib::ControlFlow::Continue;
        }
        if let Some(then) = then.take() {
            then(found);
        }
        glib::ControlFlow::Break
    });
}
