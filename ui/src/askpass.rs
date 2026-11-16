//! `nekoland-askpass`: asks for the administrator's password for `sudo -A`, which runs it
//! as its SUDO_ASKPASS and reads the password from what it prints. Nekoland has no polkit
//! agent, so the App Store asks this way. NEKOLAND_ASKPASS_REASON says what it is for.

use std::{cell::Cell, io::Write, rc::Rc};

use adw::prelude::*;
use gtk::glib;

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
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("需要管理員密碼")
        .default_width(360)
        .resizable(false)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(24)
        .margin_bottom(20)
        .margin_start(24)
        .margin_end(24)
        .build();
    content.append(
        &gtk::Image::builder()
            .icon_name("system-lock-screen-symbolic")
            .pixel_size(32)
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label("需要管理員密碼")
            .css_classes(["title-4"])
            .build(),
    );
    if !reason.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(&reason)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .margin_top(4)
        .build();
    content.append(&entry);
    let buttons = gtk::Box::builder()
        .spacing(8)
        .homogeneous(true)
        .margin_top(8)
        .build();
    let cancel = gtk::Button::with_label("取消");
    let ok = gtk::Button::builder()
        .label("認證")
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&cancel);
    buttons.append(&ok);
    content.append(&buttons);
    window.set_content(Some(&content));
    window.set_default_widget(Some(&ok));

    cancel.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| window.close()
    ));
    ok.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[weak]
        entry,
        move |_| {
            let mut out = std::io::stdout();
            if writeln!(out, "{}", entry.text())
                .and_then(|()| out.flush())
                .is_ok()
            {
                given.set(true);
            }
            window.close();
        }
    ));
    window.present();
    entry.grab_focus();
}
