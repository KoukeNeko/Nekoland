//! `nekoland-security`: what Ctrl+Alt+Delete brings up on Windows, over the whole screen:
//! locking, logging out, the password and the task manager, and the power button.

use adw::prelude::*;
use gtk::{gdk, glib};

mod ipc;

const APP_ID: &str = "io.github.koukeneko.Nekoland.Security";

const CSS: &str = "
window.security { background: alpha(#101012, .94); color: white; }
.security .choice {
  font-size: 20px;
  font-weight: 500;
  padding: 10px 24px;
  border-radius: 8px;
  color: white;
  background: none;
}
.security .choice:hover { background: alpha(white, .12); }
.security .choice:focus { background: white; color: #101012; }

.security .power { color: white; background: none; min-width: 40px; min-height: 40px; border-radius: 999px; }
.security .power:hover { background: alpha(white, .12); }
";

/// Starts `command` through the shell, as the compositor's own commands are.
fn run(command: &str) {
    if let Err(err) = std::process::Command::new("sh").arg("-c").arg(command).spawn() {
        eprintln!("nekoland-security: {command}: {err}");
    }
}

fn open(app: &adw::Application) {
    let window = gtk::ApplicationWindow::builder().application(app).title("安全選項").decorated(false).build();
    window.add_css_class("security");

    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_halign(gtk::Align::Center);
    column.set_valign(gtk::Align::Center);
    let choice = |label: &str, then: Box<dyn Fn()>| {
        let button = gtk::Button::builder().label(label).css_classes(["choice"]).build();
        let window = window.clone();
        button.connect_clicked(move |_| {
            then();
            window.close();
        });
        column.append(&button);
        button
    };
    let lock = choice("鎖定", Box::new(|| drop(ipc::msg("lock"))));
    choice("登出", Box::new(|| run("nekoland-confirm logout")));
    choice("變更密碼", Box::new(|| run("NEKOLAND_SETTINGS_PAGE=使用者 nekoland-settings")));
    choice("工作管理員", Box::new(|| run("nekoland-tasks")));

    let cancel = gtk::Button::builder().label("取消").css_classes(["choice"]).halign(gtk::Align::Center).build();
    cancel.set_margin_top(24);
    cancel.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| window.close()
    ));
    column.append(&cancel);

    // The power button, at the bottom right as Windows has it.
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let power = gtk::MenuButton::builder()
        .icon_name("system-shutdown-symbolic")
        .tooltip_text("電源")
        .css_classes(["power"])
        .direction(gtk::ArrowType::Up)
        .halign(gtk::Align::End)
        .valign(gtk::Align::End)
        .build();
    power.set_margin_end(24);
    power.set_margin_bottom(24);
    let popover = gtk::Popover::builder().child(&menu).build();
    for (label, then) in [
        ("睡眠", Box::new(|| drop(ipc::msg("sleep"))) as Box<dyn Fn()>),
        ("關機", Box::new(|| run("nekoland-confirm shutdown"))),
        ("重新開機", Box::new(|| run("nekoland-confirm restart"))),
    ] {
        let item = gtk::Button::builder().label(label).css_classes(["flat"]).build();
        let window = window.clone();
        item.connect_clicked(move |_| {
            then();
            window.close();
        });
        menu.append(&item);
    }
    power.set_popover(Some(&popover));

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&column));
    overlay.add_overlay(&power);
    window.set_child(Some(&overlay));

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        window,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, _| {
            if key == gdk::Key::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    ));
    window.add_controller(keys);
    window.fullscreen();
    window.present();
    lock.grab_focus();
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        // Always dark, over whatever the desktop is.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.present(),
        None => open(app),
    });
    app.run()
}
