//! Settings' Bluetooth page, through BlueZ's `bluetoothctl`: the adapter's power, the devices
//! paired, those nearby, and connecting, pairing and forgetting them.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{alert, group, output};

/// How long the page looks for devices nearby each time.
const SCAN_SECONDS: &str = "8";
/// How long a question to bluetoothctl may hold up the window: without bluetoothd it waits
/// for the daemon forever.
const QUERY_SECONDS: &str = "2";
/// Where the kernel lists Bluetooth adapters.
const ADAPTERS_DIR: &str = "/sys/class/bluetooth";

/// A device BlueZ knows, as `bluetoothctl devices` lists it.
#[derive(Clone, Debug, PartialEq)]
struct Device {
    address: String,
    name: String,
}

#[derive(Default)]
struct BluetoothPage {
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    /// The device being connected or paired, whose row says so until it is done.
    busy: RefCell<Option<String>>,
}

/// Whether there is a Bluetooth adapter, without which there is no Bluetooth page.
pub fn has_bluetooth() -> bool {
    has_adapter() && query(&["list"]).is_some_and(|text| text.lines().any(|line| line.starts_with("Controller ")))
}

/// Whether the kernel knows an adapter, asked first so a computer without one starts no
/// bluetoothctl at all.
fn has_adapter() -> bool {
    std::fs::read_dir(ADAPTERS_DIR).is_ok_and(|mut adapters| adapters.next().is_some())
}

/// What bluetoothctl answers, given up on after `QUERY_SECONDS`.
fn query(args: &[&str]) -> Option<String> {
    let bounded: Vec<&str> = ["--timeout", QUERY_SECONDS].into_iter().chain(args.iter().copied()).collect();
    output("bluetoothctl", &bounded)
}

/// "Device AA:BB:CC:DD:EE:FF Name" lines; a device that gave no name has its address for one.
fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Device ")?;
            let (address, name) = rest.split_once(' ').unwrap_or((rest, ""));
            // Unnamed, bluetoothctl gives the address with dashes for the name.
            let name = if name.is_empty() || name.replace('-', ":") == address { address } else { name };
            Some(Device { address: address.to_string(), name: name.to_string() })
        })
        .collect()
}

fn devices(filter: Option<&str>) -> Vec<Device> {
    let mut args = vec!["devices"];
    args.extend(filter);
    parse_devices(&query(&args).unwrap_or_default())
}

pub(crate) fn powered() -> Option<bool> {
    let shown = query(&["show"])?;
    Some(shown.lines().any(|line| line.trim() == "Powered: yes"))
}

/// The icon BlueZ gives the kind of device, or a generic one.
fn device_icon(address: &str) -> String {
    query(&["info", address])
        .and_then(|info| {
            info.lines()
                .find_map(|line| line.trim().strip_prefix("Icon: ").map(|icon| format!("{icon}-symbolic")))
        })
        .unwrap_or_else(|| "bluetooth-symbolic".into())
}

/// Runs bluetoothctl without holding up the window; `then` gets whether it worked.
pub(crate) fn bluetoothctl(args: &[&str], then: impl FnOnce(bool) + 'static) {
    let argv: Vec<&std::ffi::OsStr> = std::iter::once("bluetoothctl")
        .chain(args.iter().copied())
        .map(std::ffi::OsStr::new)
        .collect();
    let command = args.join(" ");
    let child = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_SILENCE) {
        Ok(child) => child,
        Err(err) => {
            eprintln!("bluetoothctl {command}: {err}");
            return then(false);
        }
    };
    child.wait_check_async(gio::Cancellable::NONE, move |result| {
        if let Err(err) = &result {
            eprintln!("bluetoothctl {command}: {err}");
        }
        then(result.is_ok());
    });
}

pub fn bluetooth_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let shown = Rc::new(BluetoothPage::default());
    fill(&page, &shown);
    scan(&page, &shown);
    page
}

/// What puts the page there again, after a change it made.
fn refill(page: &adw::PreferencesPage, shown: &Rc<BluetoothPage>) -> Rc<dyn Fn()> {
    let (weak, shown) = (page.downgrade(), shown.clone());
    Rc::new(move || {
        shown.busy.take();
        if let Some(page) = weak.upgrade() {
            fill(&page, &shown);
        }
    })
}

/// Looks for devices nearby for a while, then shows what was found.
fn scan(page: &adw::PreferencesPage, shown: &Rc<BluetoothPage>) {
    if powered() != Some(true) {
        return;
    }
    let done = refill(page, shown);
    bluetoothctl(&["--timeout", SCAN_SECONDS, "scan", "on"], move |_| done());
}

fn fill(page: &adw::PreferencesPage, shown: &Rc<BluetoothPage>) {
    for group in shown.groups.take() {
        page.remove(&group);
    }
    let mut groups = Vec::new();
    let done = refill(page, shown);
    let on = powered();

    let power = group("");
    let switch = adw::SwitchRow::builder()
        .title("藍牙")
        .active(on == Some(true))
        .sensitive(on.is_some())
        .build();
    let (weak, again) = (page.downgrade(), shown.clone());
    switch.connect_active_notify(move |row| {
        let value = if row.is_active() { "on" } else { "off" };
        let (weak, again) = (weak.clone(), again.clone());
        bluetoothctl(&["power", value], move |_| {
            if let Some(page) = weak.upgrade() {
                fill(&page, &again);
                scan(&page, &again);
            }
        });
    });
    power.add(&switch);
    groups.push(power);

    if on == Some(true) {
        let paired = devices(Some("Paired"));
        let connected = devices(Some("Connected"));
        let busy = shown.busy.borrow().clone();

        if !paired.is_empty() {
            let mine = group("我的裝置");
            for device in &paired {
                let is_connected = connected.contains(device);
                let row = adw::ActionRow::builder().title(&device.name).use_markup(false).activatable(true).build();
                row.add_prefix(&gtk::Image::from_icon_name(&device_icon(&device.address)));
                row.set_subtitle(match (busy.as_deref() == Some(&device.address), is_connected) {
                    (true, _) => "正在連線…",
                    (false, true) => "已連線",
                    (false, false) => "未連線",
                });
                let verb = if is_connected { "disconnect" } else { "connect" };
                connect_row(&row, shown, &done, device, &[verb]);
                row.add_suffix(&forget_button(device, done.clone()));
                mine.add(&row);
            }
            groups.push(mine);
        }

        let nearby = group("附近的裝置");
        let rescan = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("重新掃描")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let (weak, again) = (page.downgrade(), shown.clone());
        rescan.connect_clicked(move |button| {
            button.set_sensitive(false);
            if let Some(page) = weak.upgrade() {
                scan(&page, &again);
            }
        });
        nearby.set_header_suffix(Some(&rescan));
        for device in devices(None).iter().filter(|device| !paired.contains(device)) {
            let row = adw::ActionRow::builder().title(&device.name).use_markup(false).activatable(true).build();
            row.add_prefix(&gtk::Image::from_icon_name(&device_icon(&device.address)));
            if busy.as_deref() == Some(&device.address) {
                row.set_subtitle("正在配對…");
            }
            connect_pair(&row, shown, &done, device);
            nearby.add(&row);
        }
        groups.push(nearby);
    }

    for group in &groups {
        page.add(group);
    }
    shown.groups.replace(groups);
}

/// Runs each of `verbs` on the device in turn as its row is clicked, stopping at the first
/// that fails, which is told.
fn connect_row(
    row: &adw::ActionRow,
    shown: &Rc<BluetoothPage>,
    done: &Rc<dyn Fn()>,
    device: &Device,
    verbs: &[&'static str],
) {
    let (shown, done, device, verbs) = (shown.clone(), done.clone(), device.clone(), verbs.to_vec());
    row.connect_activated(move |row| {
        if shown.busy.borrow().is_some() {
            return;
        }
        shown.busy.replace(Some(device.address.clone()));
        row.set_subtitle(if verbs[0] == "pair" { "正在配對…" } else { "正在處理…" });
        run_verbs(row.clone().upcast(), device.clone(), verbs.clone(), done.clone());
    });
}

fn run_verbs(row: gtk::Widget, device: Device, mut verbs: Vec<&'static str>, done: Rc<dyn Fn()>) {
    if verbs.is_empty() {
        return done();
    }
    let verb = verbs.remove(0);
    let address = device.address.clone();
    bluetoothctl(&[verb, &address], move |worked| {
        if worked {
            run_verbs(row, device, verbs, done);
            return;
        }
        let title = match verb {
            "pair" | "trust" => format!("無法與「{}」配對", device.name),
            "disconnect" => format!("無法中斷「{}」的連線", device.name),
            _ => format!("無法連線到「{}」", device.name),
        };
        alert::notice(&row, &title, Some("請確認裝置已開啟並在附近。"));
        done();
    });
}

/// Pairs with the device as its row is clicked, then trusts and connects it.
fn connect_pair(row: &adw::ActionRow, shown: &Rc<BluetoothPage>, done: &Rc<dyn Fn()>, device: &Device) {
    let (shown, done, device) = (shown.clone(), done.clone(), device.clone());
    row.connect_activated(move |row| {
        if shown.busy.borrow().is_some() {
            return;
        }
        shown.busy.replace(Some(device.address.clone()));
        row.set_subtitle("正在配對…");
        pair(row.clone().upcast(), device.clone(), done.clone());
    });
}

/// Whether bluez itself says the device is paired, past what its name could say: the name
/// comes before bluez's own "Paired:" line.
fn is_paired(address: &str) -> bool {
    let info = query(&["info", address]).unwrap_or_default();
    info.lines().rev().find_map(|line| line.trim().strip_prefix("Paired:")).is_some_and(|paired| paired.trim() == "yes")
}

/// What bluetoothctl's agent asks or tells while pairing.
#[derive(Debug, PartialEq)]
enum AgentEvent {
    /// A PIN or passkey to type in, for a device with nowhere to show one.
    AskCode,
    /// A passkey both show, to compare.
    Confirm(String),
    /// A code to type on the device, a keyboard.
    Show(String),
    /// A service, or a pairing, some device wants; bluetoothctl does not say which device.
    Authorize,
    Paired,
    Failed(String),
}

/// bluetoothctl's words for each, without its colours.
fn agent_event(line: &str) -> Option<AgentEvent> {
    let line = strip_colours(line);
    // A device's name comes only after "Device", as in "[NEW] Device", and may hold a carriage
    // return and anything after it: such a line is never an answer.
    if line.contains("Device ") {
        return None;
    }
    // What follows a carriage return is drawn over what came before it, such as the prompt.
    let line = line.rsplit('\r').next().unwrap_or_default().trim();
    if line.starts_with("Pairing successful") {
        return Some(AgentEvent::Paired);
    }
    if let Some(why) = line.strip_prefix("Failed to pair:") {
        return Some(AgentEvent::Failed(why.trim().to_string()));
    }
    let line = line.strip_prefix("[agent]")?;
    let number = |text: &str| text.split(|c: char| !c.is_ascii_digit()).find(|word| !word.is_empty()).map(str::to_string);
    if let Some(rest) = line.split_once("Confirm passkey").map(|(_, rest)| rest) {
        return number(rest).map(AgentEvent::Confirm);
    }
    if line.contains("Enter PIN code") || line.contains("Enter passkey") {
        return Some(AgentEvent::AskCode);
    }
    for shown in ["Passkey:", "PinCode:"] {
        if let Some((_, rest)) = line.split_once(shown) {
            return number(rest).map(AgentEvent::Show);
        }
    }
    if line.contains("Authorize service") || line.contains("Accept pairing") {
        return Some(AgentEvent::Authorize);
    }
    None
}

fn strip_colours(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            // An escape runs to its first letter.
            for inner in chars.by_ref() {
                if inner.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if character != '\u{1}' && character != '\u{2}' {
            plain.push(character);
        }
    }
    plain
}

/// How long pairing may take, the person's answers included, before it is given up.
const PAIRING_TIME: std::time::Duration = std::time::Duration::from_secs(90);

/// Pairs through an interactive bluetoothctl that is the agent, asking what the device asks
/// for, then trusts and connects it.
fn pair(row: gtk::Widget, device: Device, done: Rc<dyn Fn()>) {
    let argv = ["bluetoothctl"].map(std::ffi::OsStr::new);
    let flags = gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE;
    let (child, input, output) = match gio::Subprocess::newv(&argv, flags) {
        Ok(child) => match (child.stdin_pipe(), child.stdout_pipe()) {
            (Some(input), Some(output)) => (child, input, output),
            _ => return failed_to_pair(&row, &device, None, &done),
        },
        Err(err) => {
            eprintln!("bluetoothctl: {err}");
            return failed_to_pair(&row, &device, None, &done);
        }
    };
    let say = {
        let input = input.clone();
        Rc::new(move |line: &str| {
            if let Err(err) = input.write_all(format!("{line}\n").as_bytes(), gio::Cancellable::NONE) {
                eprintln!("bluetoothctl: {err}");
            }
        })
    };
    say("agent KeyboardDisplay");
    say("default-agent");
    say(&format!("pair {}", device.address));
    // Ended once, whichever way it ends; the dialog showing a code goes with it.
    let ended = Rc::new(Cell::new(false));
    let showing: Rc<RefCell<Option<adw::AlertDialog>>> = Rc::default();
    let end = {
        let (child, ended, showing, say) = (child.clone(), ended.clone(), showing.clone(), say.clone());
        Rc::new(move || {
            if ended.replace(true) {
                return false;
            }
            if let Some(dialog) = showing.take() {
                dialog.close();
            }
            say("quit");
            let child = child.clone();
            glib::timeout_add_seconds_local_once(2, move || child.force_exit());
            true
        })
    };
    {
        let (end, row, device, done) = (end.clone(), row.clone(), device.clone(), done.clone());
        glib::timeout_add_local_once(PAIRING_TIME, move || {
            if end() {
                failed_to_pair(&row, &device, Some("配對逾時。"), &done);
            }
        });
    }
    let handle: Rc<dyn Fn(AgentEvent)> = Rc::new(move |event| match event {
        // Pairing a device Nekoland asked for is not authorized this way, and one that does
        // ask could be any device in range; a paired device is trusted after, which bluez
        // authorizes on its own.
        AgentEvent::Authorize => say("no"),
        // shortcut: bluetoothctl's prompts do not say which device asks, so another device's
        // request during this pairing is shown under this one's name; the person still
        // compares the code with the device in hand. Use bluez's Agent1 over D-Bus, which
        // names the device, to refuse the rest.
        AgentEvent::Confirm(passkey) => {
            let alert = adw::AlertDialog::new(
                Some(&format!("與「{}」配對", device.name)),
                Some(&format!("確認「{}」顯示的配對碼是 {passkey}。", device.name)),
            );
            alert.add_responses(&[("no", "取消"), ("yes", "配對")]);
            alert.set_response_appearance("yes", adw::ResponseAppearance::Suggested);
            alert.set_close_response("no");
            let say = say.clone();
            alert.choose(&row, gio::Cancellable::NONE, move |response| say(&response));
        }
        AgentEvent::AskCode => {
            let entry = gtk::Entry::builder().input_purpose(gtk::InputPurpose::Digits).activates_default(true).width_request(alert::FIELD_WIDTH).build();
            let alert = adw::AlertDialog::new(Some(&format!("輸入「{}」的配對碼", device.name)), None);
            alert.set_extra_child(Some(&entry));
            alert.add_responses(&[("cancel", "取消"), ("do", "配對")]);
            alert.set_response_appearance("do", adw::ResponseAppearance::Suggested);
            alert.set_default_response(Some("do"));
            alert.set_close_response("cancel");
            let (say, end, done) = (say.clone(), end.clone(), done.clone());
            alert.choose(&row, gio::Cancellable::NONE, move |response| {
                let code = entry.text().trim().to_string();
                if response == "do" && !code.is_empty() {
                    say(&code);
                } else if end() {
                    done();
                }
            });
        }
        AgentEvent::Show(code) => {
            let alert = adw::AlertDialog::new(
                Some(&format!("在「{}」上輸入 {code}", device.name)),
                Some("輸入後按 Enter 鍵。"),
            );
            alert.add_response("cancel", "取消");
            let (end, done) = (end.clone(), done.clone());
            alert.connect_response(None, move |_, _| {
                if end() {
                    done();
                }
            });
            alert.present(Some(&row));
            if let Some(old) = showing.replace(Some(alert)) {
                old.close();
            }
        }
        AgentEvent::Paired => {
            if end() {
                if is_paired(&device.address) {
                    run_verbs(row.clone(), device.clone(), vec!["trust", "connect"], done.clone());
                } else {
                    failed_to_pair(&row, &device, None, &done);
                }
            }
        }
        AgentEvent::Failed(why) => {
            if end() {
                failed_to_pair(&row, &device, Some(&why), &done);
            }
        }
    });
    read_agent(output, String::new(), handle, ended);
}

/// Reads what bluetoothctl says as it comes, handing on each event. Its questions end
/// without a newline, so what is left of a chunk is looked at too.
fn read_agent(output: gio::InputStream, mut pending: String, handle: Rc<dyn Fn(AgentEvent)>, ended: Rc<Cell<bool>>) {
    let stream = output.clone();
    output.read_bytes_async(4096, glib::Priority::DEFAULT, gio::Cancellable::NONE, move |read| {
        let Ok(bytes) = read else { return };
        if bytes.is_empty() || ended.get() {
            return;
        }
        pending.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(newline) = pending.find('\n') {
            let line: String = pending.drain(..=newline).collect();
            if let Some(event) = agent_event(&line) {
                handle(event);
            }
        }
        if let Some(event) = agent_event(&pending) {
            pending.clear();
            handle(event);
        }
        read_agent(stream, pending, handle, ended);
    });
}

fn failed_to_pair(row: &gtk::Widget, device: &Device, why: Option<&str>, done: &Rc<dyn Fn()>) {
    alert::notice(row, &format!("無法與「{}」配對", device.name), Some(why.unwrap_or("請確認裝置已開啟並在附近。")));
    done();
}

fn forget_button(device: &Device, done: Rc<dyn Fn()>) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("忘記此裝置")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let device = device.clone();
    button.connect_clicked(move |button| {
        let (address, done) = (device.address.clone(), done.clone());
        let title = format!("忘記「{}」？", device.name);
        let body = "這部電腦不會再自動連線到這個裝置，再次使用時需要重新配對。";
        alert::confirm(button, &title, Some(body), "忘記", move || {
            bluetoothctl(&["remove", &address], move |_| done());
        });
    });
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_is_understood() {
        let event = |line: &str| agent_event(line);
        assert_eq!(event("\u{1b}[0;94m[agent]\u{1b}[0m Confirm passkey 123456 (yes/no): "), Some(AgentEvent::Confirm("123456".into())));
        assert_eq!(event("[agent] Enter PIN code: "), Some(AgentEvent::AskCode));
        assert_eq!(event("[agent] Passkey: 004521"), Some(AgentEvent::Show("004521".into())));
        assert_eq!(event("[agent] PinCode: 0000"), Some(AgentEvent::Show("0000".into())));
        assert_eq!(event("[agent] Authorize service 0000110d-0000 (yes/no): "), Some(AgentEvent::Authorize));
        assert_eq!(event("Pairing successful"), Some(AgentEvent::Paired));
        assert_eq!(
            event("Failed to pair: org.bluez.Error.AuthenticationFailed"),
            Some(AgentEvent::Failed("org.bluez.Error.AuthenticationFailed".into()))
        );
        assert_eq!(event("[CHG] Device 11:22:33:44:55:66 RSSI: -60"), None);
        // A name made to look like an answer is still a name.
        assert_eq!(event("[NEW] Device 11:22:33:44:55:66 Pairing successful"), None);
        assert_eq!(event("[NEW] Device 11:22:33:44:55:66 [agent] Confirm passkey 1 (yes/no)"), None);
        assert_eq!(event("\r[bluetooth]# \r[agent] Enter passkey (number in 0-999999): "), Some(AgentEvent::AskCode));
        assert_eq!(event("[NEW] Device 11:22:33:44:55:66 x\r[agent] Confirm passkey 1 (yes/no)"), None);
    }

    #[test]
    fn devices_are_read_with_their_names() {
        let listed = "Device 11:22:33:44:55:66 AirPods Pro\nDevice AA:BB:CC:DD:EE:FF AA-BB-CC-DD-EE-FF\nDevice 01:02:03:04:05:06\n[CHG] Controller 00:00:00:00:00:00 Discovering: yes\n";
        let device = |address: &str, name: &str| Device { address: address.into(), name: name.into() };
        assert_eq!(
            parse_devices(listed),
            [
                device("11:22:33:44:55:66", "AirPods Pro"),
                device("AA:BB:CC:DD:EE:FF", "AA:BB:CC:DD:EE:FF"),
                device("01:02:03:04:05:06", "01:02:03:04:05:06"),
            ]
        );
    }
}
