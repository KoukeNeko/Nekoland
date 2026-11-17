//! Settings' Wi-Fi page, through NetworkManager's `nmcli`: the radio, the network joined,
//! the networks kept and those heard, and joining one, with what it asks for.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{alert, group, network, output};

/// How long the copy button shows that the password was copied.
const COPIED_SHOWN: std::time::Duration = std::time::Duration::from_secs(2);
const COPY_ICON: &str = "edit-copy-symbolic";
const COPIED_ICON: &str = "object-select-symbolic";
/// A connection's STATE once it is up, as `nmcli connection show` prints it.
const ACTIVATED: &str = "activated";

/// The network Settings is opened to join, by the control center when joining it took more
/// than NetworkManager kept.
const JOIN_ENV: &str = "NEKOLAND_WIFI_JOIN";

thread_local! {
    /// The `nmcli monitor`s the network pages follow, stopped as Settings ends.
    pub static NETWORK_MONITORS: RefCell<Vec<gio::Subprocess>> = const { RefCell::new(Vec::new()) };
}

/// How a network is locked, which says what joining it asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lock {
    Open,
    Wep,
    /// WPA and WPA2 personal, and WPA3 networks that take WPA2 too.
    Psk,
    /// WPA3 personal alone.
    Sae,
    /// WPA enterprise, 802.1X, as eduroam: an account and its password.
    Eap,
}

impl Lock {
    /// From the SECURITY `nmcli device wifi list` says, such as "WPA2 802.1X".
    fn heard(security: &str) -> Lock {
        let has = |word: &str| security.split_whitespace().any(|part| part == word);
        if has("802.1X") {
            Lock::Eap
        } else if has("WPA1") || has("WPA2") {
            Lock::Psk
        } else if has("WPA3") {
            Lock::Sae
        } else if has("WEP") {
            Lock::Wep
        } else {
            // Nothing, or OWE, which encrypts without a password.
            Lock::Open
        }
    }

    /// From a kept network's 802-11-wireless-security.key-mgmt.
    fn kept(key_mgmt: &str) -> Lock {
        match key_mgmt {
            "wpa-psk" => Lock::Psk,
            "sae" => Lock::Sae,
            "wpa-eap" | "wpa-eap-suite-b-192" => Lock::Eap,
            "none" | "ieee8021x" => Lock::Wep,
            _ => Lock::Open,
        }
    }

    fn key_mgmt(self) -> Option<&'static str> {
        match self {
            Lock::Open => None,
            Lock::Wep => Some("none"),
            Lock::Psk => Some("wpa-psk"),
            Lock::Sae => Some("sae"),
            Lock::Eap => Some("wpa-eap"),
        }
    }

    /// The setting the password is, as `nmcli connection up passwd-file` names it.
    fn secret(self) -> Option<&'static str> {
        match self {
            Lock::Open => None,
            Lock::Wep => Some("802-11-wireless-security.wep-key0"),
            Lock::Psk | Lock::Sae => Some("802-11-wireless-security.psk"),
            Lock::Eap => Some("802-1x.password"),
        }
    }
}

/// A network to join: by its name, and by the network NetworkManager keeps for it, if any.
#[derive(Clone, Debug)]
struct Target {
    ssid: String,
    lock: Lock,
    kept: Option<String>,
    hidden: bool,
}

/// A network `nmcli device wifi list` heard, by its strongest access point.
#[derive(Clone, Debug, PartialEq)]
struct Heard {
    ssid: String,
    signal: u32,
    lock: Lock,
    active: bool,
}

/// A Wi-Fi network NetworkManager keeps.
#[derive(Clone, Debug)]
struct Kept {
    ssid: String,
    uuid: String,
    /// The device it is up on, when it is.
    device: Option<String>,
}

/// The fields of one of nmcli's terse lines, which escapes the colons and backslashes in
/// them.
pub(crate) fn nmcli_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => fields.last_mut().unwrap().extend(chars.next()),
            ':' => fields.push(String::new()),
            c => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

/// What `nmcli -t -f IN-USE,SIGNAL,SECURITY,SSID device wifi list` printed, a network
/// once, the strongest first; nameless, hidden networks are joined by "其他…".
fn parse_heard(text: &str) -> Vec<Heard> {
    let mut heard: Vec<Heard> = Vec::new();
    for line in text.lines() {
        let [in_use, signal, security, ssid] = &nmcli_fields(line)[..] else {
            continue;
        };
        let Ok(signal) = signal.parse() else { continue };
        if ssid.is_empty() {
            continue;
        }
        let network = Heard {
            ssid: ssid.clone(),
            signal,
            lock: Lock::heard(security),
            active: in_use == "*",
        };
        match heard.iter_mut().find(|known| known.ssid == network.ssid) {
            Some(known) => {
                known.active |= network.active;
                known.signal = known.signal.max(network.signal);
            }
            None => heard.push(network),
        }
    }
    heard.sort_by_key(|network| std::cmp::Reverse(network.signal));
    heard
}

fn heard_networks() -> Vec<Heard> {
    let list = output(
        "nmcli",
        &["-t", "-f", "IN-USE,SIGNAL,SECURITY,SSID", "device", "wifi", "list", "--rescan", "no"],
    );
    parse_heard(&list.unwrap_or_default())
}

fn kept_networks() -> Vec<Kept> {
    let list = output(
        "nmcli",
        &["-t", "-f", "UUID,TYPE,DEVICE,STATE", "connection", "show"],
    )
    .unwrap_or_default();
    list.lines()
        .filter_map(|line| {
            let [uuid, kind, device, state] = &nmcli_fields(line)[..] else {
                return None;
            };
            if kind != network::WIFI_TYPE {
                return None;
            }
            // Its name may not be the network's: the network's is asked for.
            let ssid = setting(uuid, "802-11-wireless.ssid")?;
            Some(Kept {
                ssid,
                uuid: uuid.clone(),
                // Still joining, as while the password is tried, it is not up yet.
                device: (state == ACTIVATED).then(|| device.clone()),
            })
        })
        .collect()
}

/// One setting of a kept network, or one of what it has while it is up, as IP4.ADDRESS.
pub(crate) fn setting(uuid: &str, name: &str) -> Option<String> {
    let text = output("nmcli", &["-t", "-g", name, "connection", "show", "uuid", uuid])?;
    // Terse output escapes colons, as in a network's name.
    Some(nmcli_fields(text.trim_end_matches('\n')).join(":"))
}

/// Whether NetworkManager has a Wi-Fi device, without which there is no Wi-Fi page.
pub fn has_wifi() -> bool {
    output("nmcli", &["-t", "-f", "TYPE", "device"]).is_some_and(|text| text.lines().any(|kind| kind == "wifi"))
}

fn radio_on() -> Option<bool> {
    Some(output("nmcli", &["-t", "-f", "WIFI", "radio"])?.trim() == "enabled")
}

fn signal_icon(signal: u32) -> &'static str {
    match signal {
        0..=24 => "network-wireless-signal-weak-symbolic",
        25..=49 => "network-wireless-signal-ok-symbolic",
        50..=74 => "network-wireless-signal-good-symbolic",
        _ => "network-wireless-signal-excellent-symbolic",
    }
}

/// Runs nmcli without holding up the window; `then` gets what it printed, or what it said
/// went wrong.
pub(crate) fn nmcli(args: &[&str], then: impl FnOnce(Result<String, String>) + 'static) {
    nmcli_with(args, None, then);
}

/// As `nmcli`, with `input` written to its standard input.
fn nmcli_with(args: &[&str], input: Option<&str>, then: impl FnOnce(Result<String, String>) + 'static) {
    let argv: Vec<&std::ffi::OsStr> = std::iter::once("nmcli")
        .chain(args.iter().copied())
        .map(std::ffi::OsStr::new)
        .collect();
    let command = args.join(" ");
    let mut flags = gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE;
    if input.is_some() {
        flags |= gio::SubprocessFlags::STDIN_PIPE;
    }
    let child = match gio::Subprocess::newv(&argv, flags) {
        Ok(child) => child,
        Err(err) => return then(Err(err.to_string())),
    };
    let waited = child.clone();
    child.communicate_utf8_async(input.map(String::from), gio::Cancellable::NONE, move |result| match result {
        Ok((out, _)) if waited.is_successful() => then(Ok(out.map(Into::into).unwrap_or_default())),
        Ok((_, err)) => {
            let err = err.map(String::from).unwrap_or_default();
            // nmcli says "Error: " before what went wrong.
            let err = err.trim().trim_start_matches("Error: ").to_string();
            // Not the password, which goes through standard input, not the command.
            eprintln!("nmcli {command}: {err}");
            then(Err(err))
        }
        Err(err) => then(Err(err.to_string())),
    });
}

/// Brings a kept network up with `secret` as its `setting`, given to nmcli through a pipe, so
/// that the password is never in a command line others can see, nor in a file.
pub(crate) fn up_with(uuid: &str, setting: &str, secret: &str, then: impl FnOnce(Result<String, String>) + 'static) {
    let line = format!("{setting}:{}\n", passwd_file_value(secret));
    nmcli_with(&["connection", "up", "uuid", uuid, "passwd-file", "/dev/stdin"], Some(&line), then);
}

/// A secret as a `passwd-file` line holds it: nmcli takes a backslash there as an escape and
/// trims the spaces at the value's ends, which a password may have.
fn passwd_file_value(secret: &str) -> String {
    secret.chars().fold(String::with_capacity(secret.len()), |mut escaped, character| {
        if character == '\\' || character.is_whitespace() {
            escaped.push('\\');
        }
        escaped.push(character);
        escaped
    })
}

fn failed(parent: &gtk::Widget, ssid: &str, why: &str) {
    alert::notice(parent, &format!("無法加入「{ssid}」"), Some(why));
}

/// Joins `target`: with what is kept for it first, unless `ask` or there is nothing kept,
/// then with a password asked for. `done` follows, whatever came of it.
fn join(parent: &gtk::Widget, target: Target, ask: bool, done: Rc<dyn Fn()>) {
    let ask = ask || (target.kept.is_none() && target.lock != Lock::Open);
    if !ask || target.lock == Lock::Open {
        let args: Vec<String> = match &target.kept {
            Some(uuid) => vec!["connection".into(), "up".into(), "uuid".into(), uuid.clone()],
            None => vec!["device".into(), "wifi".into(), "connect".into(), target.ssid.clone()],
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let parent = parent.clone();
        let locked = target.lock != Lock::Open;
        nmcli(&args, move |result| match result {
            Ok(_) => done(),
            // What was kept was not enough, as when the password changed.
            Err(_) if locked => join(&parent, target, true, done),
            Err(err) => {
                failed(&parent, &target.ssid, &err);
                done();
            }
        });
        return;
    }
    if target.lock == Lock::Wep {
        failed(parent, &target.ssid, "不支援以 WEP 加密的網路。");
        return done();
    }
    let weak = parent.downgrade();
    ask_secret(parent, &target.clone(), done.clone(), move |identity, password| {
        let Some(parent) = weak.upgrade() else { return };
        join_with(&parent, target, identity, password, done);
    });
}

/// Joins with a password typed, and for an enterprise network the account it is for.
fn join_with(parent: &gtk::Widget, target: Target, identity: Option<String>, password: String, done: Rc<dyn Fn()>) {
    let secret = target.lock.secret().expect("a locked network");
    let parent = parent.clone();
    if let Some(uuid) = target.kept.clone() {
        let kept = uuid.clone();
        let up = move || {
            up_with(&uuid, secret, &password, move |result| {
                if let Err(err) = result {
                    failed(&parent, &target.ssid, &err);
                }
                done();
            })
        };
        match identity {
            Some(identity) => {
                nmcli(&["connection", "modify", "uuid", &kept, "802-1x.identity", &identity], move |_| up());
            }
            None => up(),
        }
        return;
    }
    // A network kept from now on, not joined on its own until it has been, once, with the
    // password: NetworkManager keeps that as it comes up.
    let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map(|uuid| uuid.trim().to_string())
        .unwrap_or_default();
    let ssid = target.ssid.clone();
    let mut args = vec![
        "connection", "add", "type", "wifi", "con-name", &ssid, "ssid", &ssid,
        "connection.uuid", &uuid, "connection.autoconnect", "no",
    ];
    if target.hidden {
        args.extend(["802-11-wireless.hidden", "yes"]);
    }
    if let Some(key_mgmt) = target.lock.key_mgmt() {
        args.extend(["wifi-sec.key-mgmt", key_mgmt]);
    }
    let identity = identity.unwrap_or_default();
    let realm = identity.rsplit_once('@').map(|(_, realm)| realm.to_string()).unwrap_or_default();
    if target.lock == Lock::Eap && realm.is_empty() {
        failed(&parent, &target.ssid, "請以「帳號@網域」的格式輸入帳號。");
        return done();
    }
    if target.lock == Lock::Eap {
        // The server must hold a certificate from a CA the system trusts, for the realm the
        // account is in, or a look-alike network could take the password.
        // shortcut: PEAP with MSCHAPv2, which eduroam and most campus networks take; one
        // needing TTLS or a CA of its own fails to join and wants its own settings.
        args.extend([
            "802-1x.eap", "peap", "802-1x.phase2-auth", "mschapv2", "802-1x.identity", &identity,
            "802-1x.system-ca-certs", "yes", "802-1x.domain-suffix-match", &realm,
        ]);
    }
    let added = uuid.clone();
    nmcli(&args, move |result| {
        if let Err(err) = result {
            failed(&parent, &target.ssid, &err);
            return done();
        }
        let uuid = added;
        up_with(&uuid.clone(), secret, &password, move |result| match result {
            Ok(_) => nmcli(&["connection", "modify", "uuid", &uuid, "connection.autoconnect", "yes"], move |_| done()),
            Err(err) => {
                // Not kept with a password that did not work.
                nmcli(&["connection", "delete", "uuid", &uuid], |_| {});
                failed(&parent, &target.ssid, &err);
                done();
            }
        });
    });
}

/// Asks for a network's password, and for an enterprise network's account.
fn ask_secret(
    parent: &gtk::Widget,
    target: &Target,
    cancelled: Rc<dyn Fn()>,
    then: impl FnOnce(Option<String>, String) + 'static,
) {
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let account = (target.lock == Lock::Eap).then(|| {
        let entry = gtk::Entry::builder()
            .placeholder_text("帳號@網域")
            .width_request(alert::FIELD_WIDTH)
            .activates_default(true)
            .build();
        fields.append(&entry);
        entry
    });
    let password = gtk::PasswordEntry::builder()
        .placeholder_text("密碼")
        .show_peek_icon(true)
        .width_request(alert::FIELD_WIDTH)
        .activates_default(true)
        .build();
    fields.append(&password);
    let title = format!("輸入「{}」的密碼", target.ssid);
    let dialog = adw::AlertDialog::new(Some(&title), None);
    dialog.set_extra_child(Some(&fields));
    dialog.add_responses(&[("cancel", "取消"), ("join", "加入")]);
    dialog.set_response_appearance("join", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("join"));
    dialog.set_close_response("cancel");
    let first: gtk::Widget = account.clone().map_or_else(|| password.clone().upcast(), |entry| entry.upcast());
    first.connect_map(|field| {
        let field = field.clone();
        glib::idle_add_local_once(move || {
            field.grab_focus();
        });
    });
    dialog.choose(parent, gio::Cancellable::NONE, move |response| {
        let identity = account.map(|entry| entry.text().trim().to_string());
        let typed = password.text().to_string();
        if response == "join" && !typed.is_empty() && identity.as_ref().is_none_or(|id| !id.is_empty()) {
            then(identity, typed);
        } else {
            cancelled();
        }
    });
}

/// Joins a network not heard by its name: one that keeps it to itself.
fn join_hidden(parent: &gtk::Widget, done: Rc<dyn Fn()>) {
    const LOCKS: [(&str, Lock); 4] = [
        ("無", Lock::Open),
        ("WPA/WPA2 個人", Lock::Psk),
        ("WPA3 個人", Lock::Sae),
        ("WPA/WPA2 企業", Lock::Eap),
    ];
    let name = gtk::Entry::builder()
        .placeholder_text("網路名稱")
        .width_request(alert::FIELD_WIDTH)
        .activates_default(true)
        .build();
    let security = gtk::DropDown::from_strings(&LOCKS.map(|(label, _)| label));
    security.set_selected(1);
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fields.append(&name);
    fields.append(&security);
    let dialog = adw::AlertDialog::new(Some("加入其他網路"), None);
    dialog.set_extra_child(Some(&fields));
    dialog.add_responses(&[("cancel", "取消"), ("join", "加入")]);
    dialog.set_response_appearance("join", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("join"));
    dialog.set_close_response("cancel");
    name.connect_map(|field| {
        let field = field.clone();
        glib::idle_add_local_once(move || {
            field.grab_focus();
        });
    });
    let weak = parent.downgrade();
    dialog.choose(parent, gio::Cancellable::NONE, move |response| {
        let ssid = name.text().trim().to_string();
        let Some(parent) = weak.upgrade().filter(|_| response == "join" && !ssid.is_empty()) else {
            return;
        };
        let lock = LOCKS[security.selected() as usize].1;
        let target = Target { ssid, lock, kept: None, hidden: true };
        if lock == Lock::Open {
            let parent = parent.clone();
            let ssid = target.ssid.clone();
            nmcli(&["device", "wifi", "connect", &target.ssid, "hidden", "yes"], move |result| {
                if let Err(err) = result {
                    failed(&parent, &ssid, &err);
                }
                done();
            });
        } else {
            join(&parent, target, true, done);
        }
    });
}

/// A kept network's addresses while it is up, whether it is joined on its own, and
/// forgetting it.
fn details_dialog(parent: &gtk::Widget, kept: &Kept, done: Rc<dyn Fn()>) {
    let page = adw::PreferencesPage::new();
    if let Some(device) = &kept.device {
        page.add(&network::device_facts(device));
    }
    let joining = group("");
    joining.add(&network::autoconnect_row(&kept.uuid, "自動加入"));
    joining.add(&network::low_data_row(&kept.uuid));
    if let Some(password) = password_row(&kept.uuid) {
        joining.add(&password);
    }
    page.add(&joining);
    network::address_settings(&page, &kept.uuid);

    let forget = gtk::Button::builder()
        .label("忘記此網路")
        .halign(gtk::Align::Center)
        .css_classes(["destructive-action", "pill"])
        .build();
    let end = group("");
    end.add(&forget);
    page.add(&end);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(&kept.ssid)
        .content_width(440)
        .child(&view)
        .build();
    let (uuid, ssid) = (kept.uuid.clone(), kept.ssid.clone());
    forget.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |button| {
            let (uuid, done) = (uuid.clone(), done.clone());
            let title = format!("忘記「{ssid}」？");
            let body = "這部電腦不會再自動加入這個網路，再次加入時需要重新輸入密碼。";
            alert::confirm(button, &title, Some(body), "忘記", move || {
                nmcli(&["connection", "delete", "uuid", &uuid], move |_| done());
                dialog.close();
            });
        }
    ));
    dialog.present(Some(parent));
}

/// The Mac's 拷貝密碼, for a kept network locked with a password.
fn password_row(uuid: &str) -> Option<adw::ActionRow> {
    let key_mgmt = setting(uuid, "802-11-wireless-security.key-mgmt").unwrap_or_default();
    let secret = Lock::kept(&key_mgmt).secret()?;
    let row = adw::ActionRow::builder().title("密碼").build();
    let copy = gtk::Button::builder()
        .icon_name(COPY_ICON)
        .tooltip_text("拷貝密碼")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let uuid = uuid.to_string();
    copy.connect_clicked(move |button| copy_password(button, &uuid, secret));
    row.add_suffix(&copy);
    Some(row)
}

/// Asks NetworkManager for a kept network's password and puts it on the clipboard; the button
/// says it did, as nothing else shows it.
fn copy_password(button: &gtk::Button, uuid: &str, secret: &str) {
    let button = button.clone();
    let args = ["--show-secrets", "-t", "-g", secret, "connection", "show", "uuid", uuid];
    nmcli(&args, move |result| {
        // Terse output escapes colons, as in a password.
        let password = result.map(|text| nmcli_fields(text.trim_end_matches('\n')).join(":"));
        match password {
            Ok(password) if !password.is_empty() => {
                button.clipboard().set_text(&password);
                button.set_icon_name(COPIED_ICON);
                glib::timeout_add_local_once(COPIED_SHOWN, move || button.set_icon_name(COPY_ICON));
            }
            Ok(_) => alert::notice(&button, "無法拷貝密碼", Some("這個網路沒有儲存密碼。")),
            Err(err) => alert::notice(&button, "無法拷貝密碼", Some(&err)),
        }
    });
}

/// What the page shows, which is put there again as NetworkManager changes.
#[derive(Default)]
struct WifiPage {
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    /// The network being joined, said on its row while the page is put there again.
    joining: RefCell<Option<String>>,
}

pub fn wifi_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let shown = Rc::new(WifiPage::default());
    fill_wifi_page(&page, &shown);
    let followed = shown.clone();
    follow_network(&page, move |page| fill_wifi_page(page, &followed));
    // What was heard may be old: heard again, and shown once it has been.
    let weak = page.downgrade();
    let again = shown.clone();
    nmcli(&["device", "wifi", "rescan"], move |_| {
        if let Some(page) = weak.upgrade() {
            glib::timeout_add_seconds_local_once(3, move || fill_wifi_page(&page, &again));
        }
    });
    if let Some(ssid) = std::env::var(JOIN_ENV).ok().filter(|ssid| !ssid.is_empty()) {
        std::env::remove_var(JOIN_ENV);
        // Once the page is in the window the question goes over.
        let asked = Cell::new(false);
        page.connect_map(move |page| {
            if asked.replace(true) {
                return;
            }
            let ssid = ssid.clone();
            let kept = kept_networks().into_iter().find(|kept| kept.ssid == ssid);
            let heard = heard_networks().into_iter().find(|heard| heard.ssid == ssid);
            let lock = match (&kept, &heard) {
                (Some(kept), _) => Lock::kept(&setting(&kept.uuid, "802-11-wireless-security.key-mgmt").unwrap_or_default()),
                (None, Some(heard)) => heard.lock,
                (None, None) => Lock::Psk,
            };
            let target = Target { ssid, lock, kept: kept.map(|kept| kept.uuid), hidden: false };
            join(page.upcast_ref(), target, true, refill(page, &shown));
        });
    }
    page
}

/// What puts the page there again, after a change it made.
fn refill(page: &adw::PreferencesPage, shown: &Rc<WifiPage>) -> Rc<dyn Fn()> {
    let (weak, shown) = (page.downgrade(), shown.clone());
    Rc::new(move || {
        shown.joining.take();
        if let Some(page) = weak.upgrade() {
            fill_wifi_page(&page, &shown);
        }
    })
}

/// Fills `page` again with `fill` as `nmcli monitor` says NetworkManager changed, once it
/// has been quiet for half a second.
pub(crate) fn follow_network(page: &adw::PreferencesPage, fill: impl Fn(&adw::PreferencesPage) + 'static) {
    let argv = ["nmcli", "monitor"].map(std::ffi::OsStr::new);
    let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("nmcli monitor: {err}");
            return;
        }
    };
    let Some(changes) = child.stdout_pipe() else { return };
    NETWORK_MONITORS.with_borrow_mut(|monitors| monitors.push(child.clone()));
    let fill = Rc::new(fill);
    let weak = page.downgrade();
    let quiet: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    glib::spawn_future_local(async move {
        while let Ok(bytes) = changes.read_bytes_future(4096, glib::Priority::DEFAULT).await {
            let Some(page) = weak.upgrade().filter(|_| !bytes.is_empty()) else { break };
            if let Some(waiting) = quiet.take() {
                waiting.remove();
            }
            let (fill, quiet_then) = (fill.clone(), quiet.clone());
            let waiting = glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                quiet_then.take();
                fill(&page);
            });
            *quiet.borrow_mut() = Some(waiting);
        }
        child.force_exit();
    });
}

fn fill_wifi_page(page: &adw::PreferencesPage, shown: &Rc<WifiPage>) {
    for group in shown.groups.take() {
        page.remove(&group);
    }
    let mut groups = Vec::new();
    let done = refill(page, shown);
    let on = radio_on();

    let radio = group("");
    let switch = adw::SwitchRow::builder()
        .title("Wi-Fi")
        .active(on == Some(true))
        .sensitive(on.is_some())
        .build();
    switch.connect_active_notify(|row| {
        let value = if row.is_active() { "on" } else { "off" };
        nmcli(&["radio", "wifi", value], |_| {});
    });
    radio.add(&switch);
    groups.push(radio);

    if on == Some(true) {
        let kept = kept_networks();
        let heard = heard_networks();
        let joining = shown.joining.borrow().clone();

        // The network joined, as the radio's group has it on the Mac.
        if let Some(active) = kept.iter().find(|kept| kept.device.is_some()) {
            let row = adw::ActionRow::builder().title(&active.ssid).subtitle("已連線").build();
            row.add_suffix(&details_button(active, done.clone()));
            groups[0].add(&row);
        }

        let known = group("已知網路");
        let mut any_known = false;
        for network in kept.iter().filter(|kept| kept.device.is_none()) {
            let in_range = heard.iter().find(|heard| heard.ssid == network.ssid);
            let row = adw::ActionRow::builder()
                .title(&network.ssid)
                .activatable(in_range.is_some())
                .build();
            if joining.as_deref() == Some(&network.ssid) {
                row.set_subtitle("正在加入…");
            }
            if let Some(heard) = in_range {
                add_network_icons(&row, heard);
                let target = Target {
                    ssid: network.ssid.clone(),
                    lock: heard.lock,
                    kept: Some(network.uuid.clone()),
                    hidden: false,
                };
                connect_join(&row, target, shown, &done);
            }
            row.add_suffix(&details_button(network, done.clone()));
            known.add(&row);
            any_known = true;
        }
        if any_known {
            groups.push(known);
        }

        let others = group("其他網路");
        let rescan = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("重新掃描")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let (weak, again) = (page.downgrade(), shown.clone());
        rescan.connect_clicked(move |button| {
            button.set_sensitive(false);
            let (weak, again) = (weak.clone(), again.clone());
            nmcli(&["device", "wifi", "list", "--rescan", "yes"], move |_| {
                if let Some(page) = weak.upgrade() {
                    fill_wifi_page(&page, &again);
                }
            });
        });
        others.set_header_suffix(Some(&rescan));
        for network in heard
            .iter()
            .filter(|heard| !heard.active && !kept.iter().any(|kept| kept.ssid == heard.ssid))
        {
            let row = adw::ActionRow::builder()
                .title(&network.ssid)
                .activatable(true)
                .build();
            if joining.as_deref() == Some(&network.ssid) {
                row.set_subtitle("正在加入…");
            }
            add_network_icons(&row, network);
            let target = Target {
                ssid: network.ssid.clone(),
                lock: network.lock,
                kept: None,
                hidden: false,
            };
            connect_join(&row, target, shown, &done);
            others.add(&row);
        }
        let hidden = adw::ActionRow::builder()
            .title("加入其他網路…")
            .activatable(true)
            .build();
        let again = done.clone();
        hidden.connect_activated(move |row| join_hidden(row.upcast_ref(), again.clone()));
        others.add(&hidden);
        groups.push(others);
    }

    for group in &groups {
        page.add(group);
    }
    shown.groups.replace(groups);
}

fn add_network_icons(row: &adw::ActionRow, network: &Heard) {
    if network.lock != Lock::Open {
        row.add_suffix(&gtk::Image::from_icon_name("changes-prevent-symbolic"));
    }
    row.add_suffix(&gtk::Image::from_icon_name(signal_icon(network.signal)));
}

fn details_button(kept: &Kept, done: Rc<dyn Fn()>) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("詳細資訊")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let kept = kept.clone();
    button.connect_clicked(move |button| details_dialog(button.upcast_ref(), &kept, done.clone()));
    button
}

fn connect_join(row: &adw::ActionRow, target: Target, shown: &Rc<WifiPage>, done: &Rc<dyn Fn()>) {
    let (shown, done) = (shown.clone(), done.clone());
    row.connect_activated(move |row| {
        if shown.joining.borrow().is_some() {
            return;
        }
        shown.joining.replace(Some(target.ssid.clone()));
        row.set_subtitle("正在加入…");
        join(row.upcast_ref(), target.clone(), false, done.clone());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nmcli_fields_keep_escaped_colons() {
        assert_eq!(nmcli_fields(r"*:72:WPA2:Caf\:e\\"), ["*", "72", "WPA2", r"Caf:e\"]);
    }

    #[test]
    fn heard_networks_are_listed_once_by_their_strongest() {
        let list = "*:40:WPA2:Home\n:72:WPA2:Home\n:80::Cafe\n:20:WPA2:\n:55:WPA2 802.1X:eduroam\n:30:WPA3:Lab\n";
        assert_eq!(
            parse_heard(list),
            vec![
                Heard { ssid: "Cafe".into(), signal: 80, lock: Lock::Open, active: false },
                Heard { ssid: "Home".into(), signal: 72, lock: Lock::Psk, active: true },
                Heard { ssid: "eduroam".into(), signal: 55, lock: Lock::Eap, active: false },
                Heard { ssid: "Lab".into(), signal: 30, lock: Lock::Sae, active: false },
            ]
        );
    }

    #[test]
    fn kept_locks_follow_key_management() {
        assert_eq!(Lock::kept("wpa-psk"), Lock::Psk);
        assert_eq!(Lock::kept("wpa-eap"), Lock::Eap);
        assert_eq!(Lock::kept(""), Lock::Open);
        assert_eq!(Lock::heard("WPA1 WPA2"), Lock::Psk);
        assert_eq!(Lock::heard("OWE"), Lock::Open);
    }

    #[test]
    fn passwd_file_values_keep_backslashes_and_spaces() {
        assert_eq!(passwd_file_value(r"neko:pa\ss1"), r"neko:pa\\ss1");
        assert_eq!(passwd_file_value(" a b "), r"\ a\ b\ ");
    }
}
