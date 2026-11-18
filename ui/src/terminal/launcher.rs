//! The terminal's Dock icon asks for attention, as Nekominal's taskbar button flashes: through the LauncherEntry signal
//! that Nekoland's dock reads, as Ubuntu's and KDE's do. The dock lets it go once the terminal is brought forward.

use std::{cell::Cell, collections::HashMap, time::{Duration, Instant}};

use gtk::{gio, prelude::*};

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
const SIGNAL: &str = "Update";
const PATH: &str = "/io/github/koukeneko/Nekoland/Terminal/launcher";
const URGENT: &str = "urgent";
/// At most one ask this often, as Nekominal's: a program that prints a binary file would otherwise ring all through it.
const ALERT_INTERVAL: Duration = Duration::from_secs(1);

thread_local! {
    static LAST_ASKED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Asks for attention on the Dock, unless it was asked within the last second.
pub fn ask_attention(application: &gio::Application) {
    let recent = LAST_ASKED.with(Cell::get).is_some_and(|at| at.elapsed() < ALERT_INTERVAL);
    if recent {
        return;
    }
    let Some(connection) = application.dbus_connection() else {
        return;
    };
    LAST_ASKED.with(|last| last.set(Some(Instant::now())));
    let properties = HashMap::from([(URGENT.to_owned(), true.to_variant())]);
    let body = (app_uri(application), properties).to_variant();
    if let Err(error) = connection.emit_signal(None, PATH, INTERFACE, SIGNAL, Some(&body)) {
        eprintln!("nekoland-terminal: could not ask the Dock for attention: {error}");
    }
}

/// The application as the dock names it: its desktop file.
fn app_uri(application: &gio::Application) -> String {
    let id = application.application_id().map(String::from).unwrap_or_default();
    format!("application://{id}.desktop")
}
