//! `nekoland-askpass`: asks for the administrator's password for `sudo -A`, which runs it
//! as its SUDO_ASKPASS and reads the password from what it prints. Nekoland has no polkit
//! agent, so the App Store asks this way. NEKOLAND_ASKPASS_REASON says what it is for.

use std::{
    cell::{Cell, RefCell},
    io::Write,
    rc::Rc,
};

use adw::prelude::*;
use gtk::glib;

mod alert;

fn main() -> glib::ExitCode {
    let given = Rc::new(Cell::new(false));
    // Not unique: each sudo starts its own, and none should wait on another.
    let app = adw::Application::builder()
        .application_id("io.github.koukeneko.Nekoland.Askpass")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(glib::clone!(
        #[strong]
        given,
        move |app| ask(app, given.clone())
    ));
    // sudo passes its prompt as an argument, which GTK must not take for a file to open.
    app.run_with_args::<&str>(&[]);
    if given.get() {
        glib::ExitCode::SUCCESS
    } else {
        glib::ExitCode::FAILURE
    }
}

fn ask(app: &adw::Application, given: Rc<Cell<bool>>) {
    let reason = std::env::var("NEKOLAND_ASKPASS_REASON").unwrap_or_default();
    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .width_request(alert::FIELD_WIDTH)
        .build();
    let alert = adw::AlertDialog::new(
        Some("需要管理員密碼"),
        Some(reason.as_str()).filter(|reason| !reason.is_empty()),
    );
    alert::set_icon(&alert, "system-lock-screen-symbolic");
    alert.set_extra_child(Some(&entry));
    alert.add_responses(&[("cancel", "取消"), ("auth", "認證")]);
    alert.set_response_appearance("auth", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("auth"));
    alert.set_close_response("cancel");
    let typed = entry.clone();
    alert.connect_response(None, move |_, response| {
        if response != "auth" {
            return;
        }
        let mut out = std::io::stdout();
        if writeln!(out, "{}", typed.text())
            .and_then(|()| out.flush())
            .is_ok()
        {
            given.set(true);
        }
    });
    // Shown without a parent, the alert is a window of its own, which the application does not
    // count; it is kept running until the alert closes.
    let running = RefCell::new(Some(app.hold()));
    alert.connect_closed(move |_| drop(running.take()));
    alert.present(None::<&gtk::Widget>);
    entry.grab_focus();
}
