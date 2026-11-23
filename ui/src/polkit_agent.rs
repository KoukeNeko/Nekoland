//! `nekoland-polkit-agent`: polkit's authentication agent for the session, as Nekoland's own
//! alert. polkitd asks it over the system bus when a program, such as Bitwarden unlocking with
//! the system's authentication, needs the password; the password goes to polkit's helper,
//! which checks it with PAM and tells polkitd, never back to the program that asked.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};

mod alert;
#[allow(dead_code)]
#[path = "launcher/apps.rs"]
mod apps;
mod asker;
mod ipc;
#[allow(dead_code)]
#[path = "tasks/proc.rs"]
mod proc;

const PATH: &str = "/io/github/koukeneko/Nekoland/PolkitAgent";
/// polkit's helper, started by systemd for each connection, as libpolkit-agent reaches it.
const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";
const CANCELLED: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";
const INTERFACE: &str = r#"<node><interface name="org.freedesktop.PolicyKit1.AuthenticationAgent">
  <method name="BeginAuthentication">
    <arg name="action_id" type="s" direction="in"/><arg name="message" type="s" direction="in"/>
    <arg name="icon_name" type="s" direction="in"/><arg name="details" type="a{ss}" direction="in"/>
    <arg name="cookie" type="s" direction="in"/><arg name="identities" type="a(sa{sv})" direction="in"/>
  </method>
  <method name="CancelAuthentication"><arg name="cookie" type="s" direction="in"/></method>
</interface></node>"#;

/// An authentication polkitd waits on, by its cookie.
struct Pending {
    alert: adw::AlertDialog,
    /// polkitd's call, answered once: when the helper says the password was right, or as
    /// cancelled.
    invocation: RefCell<Option<gio::DBusMethodInvocation>>,
    /// While the helper checks a password.
    checking: Cell<bool>,
}

type Pendings = Rc<RefCell<HashMap<String, Rc<Pending>>>>;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("io.github.koukeneko.Nekoland.PolkitAgent")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| match serve() {
        // The agent stays for the session.
        Ok(()) => std::mem::forget(app.hold()),
        Err(err) => eprintln!("nekoland-polkit-agent: {err}"),
    });
    app.run_with_args::<&str>(&[]);
    glib::ExitCode::SUCCESS
}

fn serve() -> Result<(), glib::Error> {
    asker::load_css();
    let bus = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)?;
    let info = gio::DBusNodeInfo::for_xml(INTERFACE)?;
    let interface = info
        .lookup_interface("org.freedesktop.PolicyKit1.AuthenticationAgent")
        .expect("declared above");
    let pendings: Pendings = Rc::default();
    // Registered for as long as the program runs; the id is never needed.
    let _ = bus
        .register_object(PATH, &interface)
        .method_call(move |_, _, _, _, method, parameters, invocation| match method {
            "BeginAuthentication" => begin(&pendings, &parameters, invocation),
            "CancelAuthentication" => {
                let cookie = parameters.child_value(0).str().unwrap_or_default().to_owned();
                invocation.return_value(None);
                if let Some(pending) = pendings.borrow_mut().remove(&cookie) {
                    cancel(&pending);
                }
            }
            _ => invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method),
        })
        .build()?;
    let session_id = std::env::var("XDG_SESSION_ID").unwrap_or_default();
    let subject = (
        "unix-session",
        HashMap::from([("session-id".to_owned(), session_id.to_variant())]),
    );
    // Nekoland speaks Traditional Chinese, so polkit's messages come in it where they have it.
    bus.call_sync(
        Some("org.freedesktop.PolicyKit1"),
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
        "RegisterAuthenticationAgent",
        Some(&(subject, "zh_TW.UTF-8", PATH).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
    )?;
    // gio's shared connection closes with its last reference, and polkitd then forgets the
    // agent; it is kept for as long as the program runs.
    std::mem::forget(bus);
    Ok(())
}

fn begin(pendings: &Pendings, parameters: &glib::Variant, invocation: gio::DBusMethodInvocation) {
    let text = |index| parameters.child_value(index).str().unwrap_or_default().to_owned();
    let (message, cookie) = (text(1), text(4));
    let details: HashMap<String, String> = parameters
        .child_value(3)
        .iter()
        .filter_map(|entry| {
            Some((
                entry.child_value(0).str()?.to_owned(),
                entry.child_value(1).str()?.to_owned(),
            ))
        })
        .collect();
    let uids: Vec<u32> = parameters
        .child_value(5)
        .iter()
        .filter(|identity| identity.child_value(0).str() == Some("unix-user"))
        .filter_map(|identity| {
            glib::VariantDict::new(Some(&identity.child_value(1)))
                .lookup::<u32>("uid")
                .ok()
                .flatten()
        })
        .collect();
    // The person logged in, when they may answer; else the first who may, such as an
    // administrator.
    let me = unsafe { libc::getuid() };
    let Some(user) = uids
        .iter()
        .find(|uid| **uid == me)
        .or(uids.first())
        .and_then(|uid| user_name(*uid))
    else {
        invocation.return_dbus_error(
            "org.freedesktop.PolicyKit1.Error.Failed",
            "No user to authenticate as",
        );
        return;
    };

    let password = gtk::PasswordEntry::builder()
        .placeholder_text("密碼")
        .show_peek_icon(true)
        .activates_default(true)
        .width_request(alert::FIELD_WIDTH)
        .build();
    let warning = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["error"])
        .visible(false)
        .build();
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fields.append(&password);
    fields.append(&warning);

    let mut body = glib::markup_escape_text(&message).to_string();
    if user_name(me).as_deref() != Some(user.as_str()) {
        body.push_str(&format!(
            "\n輸入 <b>{}</b> 的密碼。",
            glib::markup_escape_text(&user)
        ));
    }
    let alert = adw::AlertDialog::new(Some("需要認證"), Some(&body));
    alert.set_body_use_markup(true);
    let asking = details
        .get("polkit.subject-pid")
        .and_then(|pid| pid.parse().ok())
        .and_then(asker::of);
    match &asking {
        Some((_, icon)) => alert::set_picture(&alert, &asker::picture(icon)),
        None => alert::set_icon(&alert, "system-lock-screen-symbolic"),
    }
    alert.set_extra_child(Some(&fields));
    alert.add_responses(&[("cancel", "取消"), ("auth", "認證")]);
    alert.set_response_appearance("auth", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("auth"));
    alert.set_close_response("cancel");
    // It stays up until the password is right, so a wrong one is said in place.
    alert.set_can_close(false);

    let pending = Rc::new(Pending {
        alert: alert.clone(),
        invocation: RefCell::new(Some(invocation)),
        checking: Cell::new(false),
    });
    pendings.borrow_mut().insert(cookie.clone(), pending.clone());

    let (answered, pendings_then) = (pending.clone(), pendings.clone());
    alert.connect_response(None, move |alert, response| {
        if response != "auth" {
            return;
        }
        answered.checking.set(true);
        fields.set_sensitive(false);
        alert.set_response_enabled("auth", false);
        let (user, cookie, typed) = (user.clone(), cookie.clone(), password.text().to_string());
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(converse(&user, &cookie, &typed));
        });
        let (pending, pendings, alert, fields, password, warning) = (
            answered.clone(),
            pendings_then.clone(),
            alert.clone(),
            fields.clone(),
            password.clone(),
            warning.clone(),
        );
        glib::spawn_future_local(async move {
            let checked = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err(std::io::ErrorKind::BrokenPipe.into()));
            pending.checking.set(false);
            // Cancelled by polkitd while the helper checked.
            if pending.invocation.borrow().is_none() {
                return;
            }
            if let Ok(true) = checked {
                pendings
                    .borrow_mut()
                    .retain(|_, kept| !Rc::ptr_eq(kept, &pending));
                if let Some(invocation) = pending.invocation.take() {
                    invocation.return_value(None);
                }
                alert.force_close();
                return;
            }
            warning.set_label(&match checked {
                Err(err) => format!("無法認證：{err}"),
                _ => "密碼不正確。".to_owned(),
            });
            warning.set_visible(true);
            fields.set_sensitive(true);
            alert.set_response_enabled("auth", true);
            password.add_css_class("error");
            password.grab_focus();
            password.select_region(0, -1);
            asker::shake(&fields);
        });
    });
    // A response asks to close first too; only a close attempt no response follows, such as
    // Escape or 取消, cancels.
    let pendings = pendings.clone();
    alert.connect_close_attempt(move |_| {
        let (pending, pendings) = (pending.clone(), pendings.clone());
        glib::idle_add_local_once(move || {
            if !pending.checking.get() {
                pendings
                    .borrow_mut()
                    .retain(|_, kept| !Rc::ptr_eq(kept, &pending));
                cancel(&pending);
            }
        });
    });
    // Without a parent the alert is a window of its own.
    alert.present(None::<&gtk::Widget>);
}

/// Tells polkitd the authentication was cancelled, and closes its alert.
fn cancel(pending: &Pending) {
    if let Some(invocation) = pending.invocation.take() {
        invocation.return_dbus_error(CANCELLED, "Authentication was cancelled");
    }
    pending.alert.force_close();
}

/// Has polkit's helper check `password` for `user`; it tells polkitd itself when it is
/// right. The helper reads the user and the cookie, then answers each of PAM's questions.
/// shortcut: PAM's messages, such as a fingerprint reader's, are not shown; show them if a
/// fingerprint ever unlocks polkit.
fn converse(user: &str, cookie: &str, password: &str) -> std::io::Result<bool> {
    let stream = UnixStream::connect(HELPER_SOCKET)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(format!("{user}\n{cookie}\n").as_bytes())?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.starts_with("PAM_PROMPT_ECHO_OFF") || line.starts_with("PAM_PROMPT_ECHO_ON") {
            writer.write_all(format!("{password}\n").as_bytes())?;
        } else if line == "SUCCESS" {
            return Ok(true);
        } else if line == "FAILURE" {
            return Ok(false);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "polkit 的認證程式沒有回應",
    ))
}

fn user_name(uid: u32) -> Option<String> {
    // Read on the main thread only, where nothing else asks getpwuid.
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).pw_name) };
    Some(name.to_string_lossy().into_owned())
}
