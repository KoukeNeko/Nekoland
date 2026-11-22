//! `nekoland-keyring-prompt`: the keyring's password prompts as Nekoland's own alert, in place
//! of gcr-prompter's GTK 3 window. gnome-keyring asks through gcr's prompter protocol on the
//! session bus, and the password goes back over its secret exchange, encrypted.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::{Rc, Weak},
};

use adw::prelude::*;
use gtk::{gio, glib};

mod alert;

use exchange::Exchange;

const NAMES: [&str; 2] = [
    "org.gnome.keyring.SystemPrompter",
    "org.gnome.keyring.PrivatePrompter",
];
const PATH: &str = "/org/gnome/keyring/Prompter";
const CALLBACK: &str = "org.gnome.keyring.internal.Prompter.Callback";
const INTERFACE: &str = r#"<node><interface name="org.gnome.keyring.internal.Prompter">
  <method name="BeginPrompting"><arg name="callback" type="o" direction="in"/></method>
  <method name="PerformPrompt">
    <arg name="callback" type="o" direction="in"/><arg name="type" type="s" direction="in"/>
    <arg name="properties" type="a{sv}" direction="in"/><arg name="exchange" type="s" direction="in"/>
  </method>
  <method name="StopPrompting"><arg name="callback" type="o" direction="in"/></method>
</interface></node>"#;
const SHAKE: &str = ".shake { animation: shake 360ms cubic-bezier(.2, .8, .2, 1); }
@keyframes shake {
  20% { transform: translateX(-8px); } 40% { transform: translateX(7px); }
  60% { transform: translateX(-4px); } 80% { transform: translateX(2px); }
}";

/// Who asks: a prompt object of gnome-keyring's on the bus.
#[derive(Clone, PartialEq, Eq)]
struct Caller {
    name: String,
    path: String,
}

/// The prompting one caller has begun, shown one at a time as gcr-prompter does.
struct Active {
    caller: Caller,
    exchange: Exchange,
    /// What the caller has set so far; each PerformPrompt sends only what changed.
    properties: HashMap<String, glib::Variant>,
    kind: String,
    /// Between PerformPrompt and the answer, while the alert waits for the user.
    asking: bool,
    prompt: Option<Prompt>,
    /// Stops watching for the caller leaving the bus. gio's watcher id has no nameable type.
    unwatch: Box<dyn FnOnce()>,
}

struct Prompter {
    bus: gio::DBusConnection,
    active: Option<Active>,
    waiting: VecDeque<Caller>,
    /// The bus names, given up before leaving so no prompt is sent to a prompter on its way out.
    owners: Vec<gio::OwnerId>,
    idle: Option<glib::SourceId>,
    running: Option<gio::ApplicationHoldGuard>,
}

type Shared = Rc<RefCell<Prompter>>;

fn main() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id("io.github.koukeneko.Nekoland.KeyringPrompt")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(|app| {
        if let Err(err) = serve(app) {
            eprintln!("nekoland-keyring-prompt: {err}");
        }
    });
    app.run_with_args::<&str>(&[]);
    glib::ExitCode::SUCCESS
}

fn serve(app: &adw::Application) -> Result<(), glib::Error> {
    let css = gtk::CssProvider::new();
    css.load_from_string(SHAKE);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let shared: Shared = Rc::new(RefCell::new(Prompter {
        bus: bus.clone(),
        active: None,
        waiting: VecDeque::new(),
        owners: Vec::new(),
        idle: None,
        running: Some(app.hold()),
    }));
    let info = gio::DBusNodeInfo::for_xml(INTERFACE)?;
    let interface = info
        .lookup_interface("org.gnome.keyring.internal.Prompter")
        .expect("declared above");
    let prompter = shared.clone();
    // Registered for as long as the program runs; the id is never needed.
    let _ = bus
        .register_object(PATH, &interface)
        .method_call(move |_, sender, _, _, method, parameters, invocation| {
            handle(
                &prompter,
                sender.unwrap_or_default(),
                method,
                &parameters,
                invocation,
            );
        })
        .build()?;
    for name in NAMES {
        let owner = gio::bus_own_name_on_connection(
            &bus,
            name,
            gio::BusNameOwnerFlags::NONE,
            |_, _| {},
            |_, name| eprintln!("nekoland-keyring-prompt: {name} is another program's"),
        );
        shared.borrow_mut().owners.push(owner);
    }
    next(&shared);
    Ok(())
}

fn handle(
    shared: &Shared,
    sender: &str,
    method: &str,
    parameters: &glib::Variant,
    invocation: gio::DBusMethodInvocation,
) {
    let caller = Caller {
        name: sender.to_owned(),
        path: parameters.child_value(0).str().unwrap_or_default().to_owned(),
    };
    match method {
        "BeginPrompting" => {
            let mut prompter = shared.borrow_mut();
            if prompter
                .active
                .as_ref()
                .is_some_and(|active| active.caller == caller)
                || prompter.waiting.contains(&caller)
            {
                drop(prompter);
                invocation.return_dbus_error(
                    "org.gnome.keyring.Prompter.InProgress",
                    "Already begun prompting for this callback",
                );
                return;
            }
            prompter.waiting.push_back(caller);
            drop(prompter);
            invocation.return_value(None);
            next(shared);
        }
        "PerformPrompt" => perform(shared, &caller, parameters, invocation),
        "StopPrompting" => {
            invocation.return_value(None);
            stop(shared, &caller, true);
            next(shared);
        }
        _ => invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method),
    }
}

/// Starts the next caller waiting, when none is being prompted: it is told the prompter is
/// ready, with this side's half of the secret exchange. With no one waiting, it leaves after
/// a while, as gcr-prompter does.
fn next(shared: &Shared) {
    let mut prompter = shared.borrow_mut();
    if prompter.active.is_some() {
        return;
    }
    let Some(caller) = prompter.waiting.pop_front() else {
        if prompter.owners.is_empty() {
            prompter.running = None;
        } else if prompter.idle.is_none() {
            let weak = Rc::downgrade(shared);
            prompter.idle = Some(glib::timeout_add_seconds_local_once(10, move || {
                if let Some(shared) = weak.upgrade() {
                    leave(&shared);
                }
            }));
        }
        return;
    };
    if let Some(idle) = prompter.idle.take() {
        idle.remove();
    }
    let weak = Rc::downgrade(shared);
    let gone = caller.clone();
    let watch = gio::bus_watch_name_on_connection(
        &prompter.bus,
        &caller.name,
        gio::BusNameWatcherFlags::NONE,
        |_, _, _| {},
        move |_, _| {
            if let Some(shared) = weak.upgrade() {
                stop(&shared, &gone, false);
                next(&shared);
            }
        },
    );
    prompter.active = Some(Active {
        caller,
        exchange: Exchange::new(),
        properties: HashMap::new(),
        kind: String::new(),
        asking: false,
        prompt: None,
        unwatch: Box::new(move || gio::bus_unwatch_name(watch)),
    });
    drop(prompter);
    ready(shared, "", None, HashMap::new());
}

/// Gives up the bus names, then leaves once what was already on its way to them is answered.
fn leave(shared: &Shared) {
    let mut prompter = shared.borrow_mut();
    prompter.idle = None;
    for owner in prompter.owners.drain(..) {
        gio::bus_unown_name(owner);
    }
    let weak = Rc::downgrade(shared);
    glib::timeout_add_seconds_local_once(2, move || {
        if let Some(shared) = weak.upgrade() {
            next(&shared);
        }
    });
}

/// Answers the caller: "" when ready for a prompt, "yes" or "no" after one.
fn ready(shared: &Shared, reply: &str, secret: Option<&str>, changed: HashMap<String, glib::Variant>) {
    let prompter = shared.borrow();
    let Some(active) = &prompter.active else {
        return;
    };
    let caller = active.caller.clone();
    let sent = active.exchange.send(secret);
    let weak = Rc::downgrade(shared);
    let gone = caller.clone();
    prompter.bus.call(
        Some(&caller.name),
        &caller.path,
        CALLBACK,
        "PromptReady",
        Some(&(reply, changed, sent).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        -1,
        gio::Cancellable::NONE,
        move |answered| {
            if let Err(err) = answered {
                eprintln!("nekoland-keyring-prompt: the keyring's prompt did not take the answer: {err}");
                if let Some(shared) = weak.upgrade() {
                    stop(&shared, &gone, false);
                    next(&shared);
                }
            }
        },
    );
}

fn perform(
    shared: &Shared,
    caller: &Caller,
    parameters: &glib::Variant,
    invocation: gio::DBusMethodInvocation,
) {
    let mut prompter = shared.borrow_mut();
    let Some(active) = prompter.active.as_mut().filter(|active| &active.caller == caller) else {
        drop(prompter);
        invocation.return_dbus_error(
            "org.freedesktop.DBus.Error.Failed",
            "Not begun prompting for this callback",
        );
        return;
    };
    if active.asking {
        drop(prompter);
        invocation.return_dbus_error(
            "org.freedesktop.DBus.Error.Failed",
            "Already performing a prompt for this callback",
        );
        return;
    }
    let kind = parameters.child_value(1).str().unwrap_or_default().to_owned();
    let received = parameters.child_value(3).str().unwrap_or_default().to_owned();
    if !matches!(kind.as_str(), "password" | "confirm") || !active.exchange.receive(&received) {
        drop(prompter);
        invocation.return_dbus_error(
            "org.freedesktop.DBus.Error.InvalidArgs",
            "Invalid prompt type or secret exchange",
        );
        return;
    }
    let properties = parameters.child_value(2);
    for entry in properties.iter() {
        if let (Some(name), Some(value)) = (entry.child_value(0).str(), entry.child_value(1).as_variant()) {
            active.properties.insert(name.to_owned(), value);
        }
    }
    active.kind = kind;
    active.asking = true;
    let shown = active.prompt.is_some();
    let prompt = active
        .prompt
        .get_or_insert_with(|| Prompt::new(Rc::downgrade(shared)));
    prompt.update(&active.kind, &active.properties);
    let alert = prompt.alert.clone();
    let field = prompt.password.clone();
    drop(prompter);
    invocation.return_value(None);
    if !shown {
        // Without a parent the alert is a window of its own.
        alert.present(None::<&gtk::Widget>);
    }
    field.grab_focus();
}

/// The user answered the alert.
fn answer(shared: &Shared, accepted: bool) {
    let mut prompter = shared.borrow_mut();
    let Some(active) = prompter.active.as_mut().filter(|active| active.asking) else {
        return;
    };
    let Some(prompt) = &active.prompt else {
        return;
    };
    let secret = (accepted && active.kind == "password").then(|| prompt.password.text().to_string());
    // Each value boxed twice, as gcr-prompter sends them: gcr takes one box off.
    let boxed = |value: glib::Variant| glib::Variant::from_variant(&value);
    let mut changed = HashMap::new();
    if let Some(secret) = &secret {
        // gnome-keyring asks before keeping a new keyring unencrypted unless this is above 0.
        changed.insert(
            "password-strength".to_owned(),
            boxed(i32::from(!secret.is_empty()).to_variant()),
        );
    }
    if prompt.choice.is_visible() {
        changed.insert(
            "choice-chosen".to_owned(),
            boxed(prompt.choice.is_active().to_variant()),
        );
    }
    // Until the keyring says whether the password was right, there is nothing to answer.
    prompt.set_sensitive(false);
    active.asking = false;
    drop(prompter);
    ready(
        shared,
        if accepted { "yes" } else { "no" },
        secret.as_deref(),
        changed,
    );
}

/// Ends the caller's prompting; `done` tells it so with PromptDone, which a caller that asked
/// for the end waits for, as with gcr-prompter.
fn stop(shared: &Shared, caller: &Caller, done: bool) {
    let mut prompter = shared.borrow_mut();
    prompter.waiting.retain(|waiting| waiting != caller);
    let active = prompter.active.take_if(|active| &active.caller == caller);
    let bus = prompter.bus.clone();
    drop(prompter);
    let Some(active) = active else {
        return;
    };
    (active.unwatch)();
    if let Some(prompt) = &active.prompt {
        prompt.alert.force_close();
    }
    if done {
        bus.call(
            Some(&caller.name),
            &caller.path,
            CALLBACK,
            "PromptDone",
            None,
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            -1,
            gio::Cancellable::NONE,
            |_| {},
        );
    }
}

/// The alert, with every field it may need; what a prompt does not use is hidden.
struct Prompt {
    alert: adw::AlertDialog,
    fields: gtk::Box,
    password: gtk::PasswordEntry,
    again: gtk::PasswordEntry,
    warning: gtk::Label,
    choice: gtk::CheckButton,
}

impl Prompt {
    fn new(shared: Weak<RefCell<Prompter>>) -> Prompt {
        let entry = |placeholder: &str| {
            gtk::PasswordEntry::builder()
                .placeholder_text(placeholder)
                .show_peek_icon(true)
                .activates_default(true)
                .width_request(alert::FIELD_WIDTH)
                .build()
        };
        let password = entry("密碼");
        let again = entry("再次輸入密碼");
        again.set_show_peek_icon(false);
        let warning = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["error"])
            .visible(false)
            .build();
        let choice = gtk::CheckButton::with_label("登入時自動解鎖");
        let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
        fields.append(&password);
        fields.append(&again);
        fields.append(&warning);
        fields.append(&choice);

        let alert = adw::AlertDialog::new(None, None);
        alert::set_icon(&alert, "system-lock-screen-symbolic");
        alert.set_extra_child(Some(&fields));
        alert.add_responses(&[("cancel", "取消"), ("continue", "解鎖")]);
        alert.set_response_appearance("continue", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("continue"));
        alert.set_close_response("cancel");
        // It stays up until the keyring ends the prompting, so a wrong password is said in place.
        alert.set_can_close(false);
        let answered = shared.clone();
        alert.connect_response(None, move |_, response| {
            if let Some(shared) = answered.upgrade() {
                answer(&shared, response == "continue");
            }
        });
        // A response asks to close first too; only a close attempt no response follows, such
        // as Escape, cancels.
        alert.connect_close_attempt(move |_| {
            let shared = shared.clone();
            glib::idle_add_local_once(move || {
                if let Some(shared) = shared.upgrade() {
                    answer(&shared, false);
                }
            });
        });

        let prompt = Prompt {
            alert,
            fields,
            password,
            again,
            warning,
            choice,
        };
        for field in [&prompt.password, &prompt.again] {
            let (alert, password, again, warning) = (
                prompt.alert.clone(),
                prompt.password.clone(),
                prompt.again.clone(),
                prompt.warning.clone(),
            );
            field.connect_changed(move |_| {
                password.remove_css_class("error");
                let differ =
                    again.is_visible() && !again.text().is_empty() && password.text() != again.text();
                again.set_css_classes(if differ { &["error"] } else { &[] });
                warning.set_label("兩次輸入的密碼不同。");
                warning.set_visible(differ);
                alert.set_response_enabled("continue", valid(&password, &again));
            });
        }
        prompt
    }

    fn update(&self, kind: &str, properties: &HashMap<String, glib::Variant>) {
        let text = |name: &str| {
            properties
                .get(name)
                .and_then(|value| value.str())
                .unwrap_or_default()
        };
        let flag = |name: &str| {
            properties
                .get(name)
                .and_then(|value| value.get::<bool>())
                .unwrap_or(false)
        };
        let copy = Copy::of(
            text("title"),
            text("message"),
            text("description"),
            text("continue-label"),
        );
        self.alert.set_heading(Some(&copy.heading));
        self.alert.set_body(&copy.body);
        self.alert.set_response_label("continue", &copy.verb);
        let password = kind == "password";
        self.password.set_visible(password);
        self.again.set_visible(password && flag("password-new"));
        self.choice
            .set_visible(password && !text("choice-label").is_empty());
        self.choice.set_active(flag("choice-chosen"));
        self.set_sensitive(true);
        self.alert
            .set_response_enabled("continue", !password || valid(&self.password, &self.again));

        let warning = warning(text("warning"));
        self.warning.set_visible(!warning.is_empty());
        if !warning.is_empty() {
            self.warning.set_label(&warning);
            self.password.add_css_class("error");
            self.password.select_region(0, -1);
            // The class is taken off first so the shake plays again on another wrong password.
            self.fields.remove_css_class("shake");
            let fields = self.fields.clone();
            glib::idle_add_local_once(move || fields.add_css_class("shake"));
        }
    }

    fn set_sensitive(&self, sensitive: bool) {
        self.fields.set_sensitive(sensitive);
        self.alert.set_response_enabled("cancel", sensitive);
        self.alert.set_response_enabled("continue", sensitive);
    }
}

/// A new password needs typing twice the same; any password, blank too, unlocks.
fn valid(password: &gtk::PasswordEntry, again: &gtk::PasswordEntry) -> bool {
    !again.is_visible() || (!password.text().is_empty() && password.text() == again.text())
}

/// The alert's words for gnome-keyring's prompts, which it sends in English when it has no
/// translation; one it does not know is shown as sent.
struct Copy {
    heading: String,
    body: String,
    verb: String,
}

impl Copy {
    fn of(title: &str, message: &str, description: &str, verb: &str) -> Copy {
        let name = keyring_name(description);
        let (heading, body, verb) = match (title, message) {
            (_, "Choose password for new keyring") => (
                "建立鑰匙圈",
                format!("應用程式要把密碼保存在新的鑰匙圈「{name}」。"),
                "建立",
            ),
            ("Unlock Keyring", _) => (
                "解鎖鑰匙圈",
                format!("應用程式要讀取鑰匙圈「{name}」中的密碼。"),
                "解鎖",
            ),
            ("Unlock Login Keyring", _) if description.contains("no longer matches") => (
                "解鎖鑰匙圈",
                "登入密碼與鑰匙圈「登入」的密碼不同。".to_owned(),
                "解鎖",
            ),
            ("Unlock Login Keyring", _) => (
                "解鎖鑰匙圈",
                "鑰匙圈「登入」沒有在登入時解鎖。".to_owned(),
                "解鎖",
            ),
            (_, "Store passwords unencrypted?") => (
                "不加密保存密碼？",
                "使用空白密碼時，保存的密碼不會加密。".to_owned(),
                "繼續",
            ),
            ("Change Keyring Password", message) if message.starts_with("Enter the old password") => (
                "更改鑰匙圈密碼",
                format!("輸入鑰匙圈「{name}」的舊密碼。"),
                "繼續",
            ),
            ("Change Keyring Password", _) => (
                "更改鑰匙圈密碼",
                format!("為鑰匙圈「{name}」設定新密碼。"),
                "繼續",
            ),
            _ => {
                let heading = if message.is_empty() { title } else { message };
                return Copy {
                    heading: heading.to_owned(),
                    body: description.to_owned(),
                    verb: if verb.is_empty() { "繼續" } else { verb }.to_owned(),
                };
            }
        };
        Copy {
            heading: heading.to_owned(),
            body,
            verb: verb.to_owned(),
        }
    }
}

/// The keyring a description names in its quotes, by the name Nekoland shows for it.
fn keyring_name(description: &str) -> String {
    let name = description
        .split_once('“')
        .and_then(|(_, rest)| rest.split_once('”'))
        .map_or("", |(name, _)| name);
    match name.to_lowercase().as_str() {
        "default" | "default keyring" => "預設".to_owned(),
        "login" => "登入".to_owned(),
        "" | "unnamed" => "未命名".to_owned(),
        _ => name.to_owned(),
    }
}

fn warning(sent: &str) -> String {
    match sent {
        "The unlock password was incorrect" => "密碼不正確。",
        "The original password was incorrect" => "舊密碼不正確。",
        sent => sent,
    }
    .to_owned()
}

/// gcr's "sx-aes-1" secret exchange: Diffie-Hellman in the 1536-bit IKE group, the shared
/// secret through HKDF-SHA256 into an AES-128-CBC key, with PKCS#7 padding.
mod exchange {
    use aes::Aes128;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use num_bigint::BigUint;

    const SECTION: &str = "[sx-aes-1]";
    /// RFC 3526's 1536-bit MODP group, whose generator is 2.
    const PRIME: &str =
        "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DD\
        EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED\
        EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F\
        83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA237327FFFFFFFFFFFFFFFF";
    const PRIME_BYTES: usize = 192;

    fn prime() -> BigUint {
        BigUint::parse_bytes(PRIME.as_bytes(), 16).expect("a hexadecimal number")
    }

    fn random<const N: usize>() -> [u8; N] {
        let mut bytes = [0; N];
        getrandom::fill(&mut bytes).expect("the system gives no random numbers");
        bytes
    }

    pub struct Exchange {
        private: BigUint,
        public: Vec<u8>,
        pub(super) key: Option<[u8; 16]>,
    }

    impl Exchange {
        pub fn new() -> Exchange {
            let mut bytes = random::<PRIME_BYTES>();
            // Below the prime, whose top bit is set.
            bytes[0] &= 0x7f;
            let private = BigUint::from_bytes_be(&bytes);
            let public = BigUint::from(2u32).modpow(&private, &prime()).to_bytes_be();
            Exchange {
                private,
                public,
                key: None,
            }
        }

        /// This side's public key, and the secret when there is one and a key to seal it with.
        pub fn send(&self, secret: Option<&str>) -> String {
            let mut text = format!("{SECTION}\npublic={}\n", STANDARD.encode(&self.public));
            if let (Some(secret), Some(key)) = (secret, self.key) {
                let iv = random::<16>();
                let sealed = cbc::Encryptor::<Aes128>::new(&key.into(), &iv.into())
                    .encrypt_padded_vec_mut::<Pkcs7>(secret.as_bytes());
                text += &format!("secret={}\niv={}\n", STANDARD.encode(sealed), STANDARD.encode(iv));
            }
            text
        }

        /// Takes the key from the other side's public key, the first time; false when what it
        /// sent has none.
        pub fn receive(&mut self, text: &str) -> bool {
            if self.key.is_some() {
                return true;
            }
            let Some(peer) = field(text, "public").and_then(|value| STANDARD.decode(value).ok()) else {
                return false;
            };
            // Unpadded, as gcr 3, which gnome-keyring links, writes it.
            // shortcut: gcr 4 pads it to the prime's length, which differs once in 256 exchanges;
            // pad when gnome-keyring moves to gcr 4.
            let secret = BigUint::from_bytes_be(&peer)
                .modpow(&self.private, &prime())
                .to_bytes_be();
            let mut key = [0; 16];
            hkdf::Hkdf::<sha2::Sha256>::new(None, &secret)
                .expand(&[], &mut key)
                .expect("16 bytes is a length HKDF gives");
            self.key = Some(key);
            true
        }
    }

    pub(super) fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
        let mut lines = text.lines().map(str::trim);
        lines.by_ref().find(|line| *line == SECTION)?;
        lines.take_while(|line| !line.starts_with('[')).find_map(|line| {
            line.split_once('=')
                .filter(|(key, _)| key.trim() == name)
                .map(|(_, value)| value.trim())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::exchange::{field, Exchange};
    use aes::Aes128;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

    #[test]
    fn both_sides_seal_and_open_the_same_secret() {
        let (mut prompter, mut keyring) = (Exchange::new(), Exchange::new());
        assert!(keyring.receive(&prompter.send(None)));
        assert!(prompter.receive(&keyring.send(None)));
        assert_eq!(prompter.key, keyring.key);

        let sent = prompter.send(Some("hunter2"));
        let sealed = STANDARD.decode(field(&sent, "secret").unwrap()).unwrap();
        let iv: [u8; 16] = STANDARD
            .decode(field(&sent, "iv").unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let opened = cbc::Decryptor::<Aes128>::new(&keyring.key.unwrap().into(), &iv.into())
            .decrypt_padded_vec_mut::<Pkcs7>(&sealed)
            .unwrap();
        assert_eq!(opened, b"hunter2");
    }

    #[test]
    fn nothing_to_derive_from_a_text_without_a_public_key() {
        assert!(!Exchange::new().receive("[sx-aes-1]\niv=AAAA\n"));
        assert!(!Exchange::new().receive("public=AAAA\n"));
    }

    #[test]
    fn names_the_keyrings_nekoland_knows() {
        let copy = super::Copy::of(
            "Unlock Keyring",
            "Authentication required",
            "An application wants access to the keyring “Login”, but it is locked",
            "Unlock",
        );
        assert_eq!(copy.body, "應用程式要讀取鑰匙圈「登入」中的密碼。");
        assert_eq!(super::keyring_name("called “Work”. Choose"), "Work");
    }
}
