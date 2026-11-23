//! Who asks for a password: the program by its name and icon, shown at the top of the
//! keyring's and polkit's prompts, and the shake a wrong password gets there.

use std::collections::HashMap;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{apps, ipc, proc};

pub const CSS: &str = ".shake { animation: shake 360ms cubic-bezier(.2, .8, .2, 1); }
@keyframes shake {
  20% { transform: translateX(-8px); } 40% { transform: translateX(7px); }
  60% { transform: translateX(-4px); } 80% { transform: translateX(2px); }
}
/* Clear of the lock's overhang, 12px from the question below. */
.asker { margin-top: 3px; margin-bottom: 12px; }
.asker-badge {
  min-width: 24px; min-height: 24px; border-radius: 12px; margin: 0 -6px -6px 0;
  background-color: var(--dialog-fg-color); color: var(--dialog-bg-color);
  box-shadow: 0 0 0 3px var(--dialog-bg-color);
}";

pub fn load_css() {
    let css = gtk::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

/// The program process `pid` belongs to: the application, by the name and icon of its
/// .desktop file, or else the command, by its name.
pub fn of(pid: i32) -> Option<(String, gio::Icon)> {
    // Each process belongs to the application whose window it, or a process it was started
    // from, has, as the task manager counts them.
    let windows: HashMap<i32, String> = ipc::msg("tasks")?
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some((fields.next()?.parse().ok()?, fields.next()?.to_owned()))
        })
        .collect();
    let procs = proc::read_procs();
    let app = proc::owners(&procs, &windows)
        .get(&pid)
        .and_then(|app| gio::DesktopAppInfo::new(&format!("{app}.desktop")));
    // The icon the Dock shows: from the icon theme chosen in Appearance, else Nekoland's.
    let look = apps::Look::load();
    let (dirs, themes) = (apps::data_dirs(), look.icon_themes());
    let icon = |name: &str| {
        let path = apps::find_icon(&dirs, &themes, name)
            .or_else(|| apps::find_icon(&dirs, &themes, apps::NO_ICON))?;
        Some(gio::FileIcon::new(&gio::File::for_path(path)).upcast::<gio::Icon>())
    };
    if let Some(app) = app {
        let name = app.string("Icon").unwrap_or_default();
        return Some((app.name().to_string(), icon(&name)?));
    }
    let command = procs.iter().find(|proc| proc.pid == pid)?.name.clone();
    Some((command, icon("utilities-terminal")?))
}

/// The asking program's icon, with a lock at its corner for what it asks.
pub fn picture(icon: &gio::Icon) -> gtk::Overlay {
    let picture = gtk::Overlay::builder()
        .halign(gtk::Align::Center)
        .css_classes(["asker"])
        .child(&gtk::Image::builder().gicon(icon).pixel_size(56).build())
        .build();
    let badge = gtk::Image::builder()
        .icon_name("system-lock-screen-symbolic")
        .pixel_size(13)
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .css_classes(["asker-badge"])
        .build();
    picture.add_overlay(&badge);
    picture
}

/// Shakes `fields`, as after a wrong password; the class is taken off first so it plays
/// again on the next.
pub fn shake(fields: &impl IsA<gtk::Widget>) {
    fields.remove_css_class("shake");
    let fields = fields.clone().upcast::<gtk::Widget>();
    glib::idle_add_local_once(move || fields.add_css_class("shake"));
}
