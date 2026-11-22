//! The menu bar's VPN item: Plaitway's profiles, as `plaitway watch -json` tells of them,
//! each turned on and off from its menu, and Settings opened for one that asks for a password.

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

use serde::Deserialize;
use smithay::reexports::calloop::channel::Sender;

use crate::{
    dbus::Event,
    menubar::{Command as MenuCommand, Entry},
};

const CLI: &str = "plaitway";
/// How long a helper that is not running is left before it is asked again.
const RETRY: Duration = Duration::from_secs(5);
/// The page of Settings that has Plaitway's profiles.
pub const SETTINGS_PAGE: &str = "VPN";
/// Settings' windows, which ask for a profile's password themselves.
pub const SETTINGS_APP_ID: &str = "io.github.koukeneko.Nekoland.Settings";

/// The `plaitway watch` running, which `reread` ends to have it started again.
static WATCHING: Mutex<Option<Child>> = Mutex::new(None);
/// Set by `reread`: the watch ended on purpose, and starts again at once.
static REREAD: AtomicBool = AtomicBool::new(false);

/// Reads the profiles again. The helper tells watchers nothing of a new order, which Settings
/// asks for this after; a new watch starts with every profile as it is now.
pub fn reread() {
    if let Some(child) = WATCHING.lock().unwrap().as_mut() {
        REREAD.store(true, Ordering::Relaxed);
        if let Err(err) = child.kill() {
            tracing::warn!("plaitway watch: {err}");
        }
    }
}

/// A profile, as much of it as the menu shows.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Tunnel {
    pub id: String,
    pub name: String,
    pub state: String,
    pub desired_enabled: bool,
    #[serde(skip_deserializing)]
    priority: i32,
}

impl Tunnel {
    fn state(&self) -> &str {
        self.state.trim_start_matches("PROFILE_STATE_")
    }

    pub fn asks_for_password(&self) -> bool {
        self.state() == "AWAITING_CREDENTIALS"
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Profile {
    #[serde(flatten)]
    tunnel: Tunnel,
    settings: Option<ProfileSettings>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProfileSettings {
    priority: i32,
}

impl From<Profile> for Tunnel {
    fn from(profile: Profile) -> Self {
        Tunnel { priority: profile.settings.map_or(0, |settings| settings.priority), ..profile.tunnel }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WatchEvent {
    snapshot: Option<Snapshot>,
    changed: Option<Profile>,
    removed: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Snapshot {
    profiles: Vec<Profile>,
}

/// Takes one line of `plaitway watch -json` into the profiles, in their order.
fn apply(tunnels: &mut Vec<Tunnel>, line: &str) -> Result<(), serde_json::Error> {
    let event: WatchEvent = serde_json::from_str(line)?;
    if let Some(snapshot) = event.snapshot {
        *tunnels = snapshot.profiles.into_iter().map(Tunnel::from).collect();
    }
    if let Some(changed) = event.changed.map(Tunnel::from) {
        match tunnels.iter_mut().find(|tunnel| tunnel.id == changed.id) {
            Some(tunnel) => *tunnel = changed,
            None => tunnels.push(changed),
        }
    }
    if let Some(removed) = event.removed {
        tunnels.retain(|tunnel| tunnel.id != removed);
    }
    tunnels.sort_by_key(|tunnel| tunnel.priority);
    Ok(())
}

/// Follows Plaitway's helper on a thread while it runs; without Plaitway there is no item.
/// The traffic counted every few seconds changes nothing the menu shows, and is not told.
pub fn watch(events: Sender<Event>) {
    let installed = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(CLI).is_file()));
    if !installed {
        return;
    }
    let spawned = std::thread::Builder::new().name("nekoland-vpn".into()).spawn(move || loop {
        let mut told: Option<Vec<Tunnel>> = None;
        let child = Command::new(CLI)
            .args(["watch", "-json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                let lines = BufReader::new(child.stdout.take().expect("piped")).lines();
                *WATCHING.lock().unwrap() = Some(child);
                let mut tunnels = Vec::new();
                let mut gone = false;
                for line in lines {
                    let Ok(line) = line else { break };
                    if apply(&mut tunnels, &line).is_err() || told.as_ref() == Some(&tunnels) {
                        continue;
                    }
                    if events.send(Event::Vpn(Some(tunnels.clone()))).is_err() {
                        gone = true;
                        break;
                    }
                    told = Some(tunnels.clone());
                }
                if let Some(mut child) = WATCHING.lock().unwrap().take() {
                    if gone {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                }
                if gone {
                    return;
                }
            }
            Err(err) => tracing::warn!("plaitway watch: {err}"),
        }
        if REREAD.swap(false, Ordering::Relaxed) {
            continue;
        }
        // A helper that stopped leaves states that are no longer true.
        if told.is_some() && events.send(Event::Vpn(None)).is_err() {
            return;
        }
        std::thread::sleep(RETRY);
    });
    if let Err(err) = spawned {
        tracing::warn!("no VPN item: {err}");
    }
}

/// The item's icon: what most needs to be seen of how the profiles stand.
pub fn icon(tunnels: &[Tunnel]) -> &'static str {
    let any = |states: &[&str]| tunnels.iter().any(|tunnel| states.contains(&tunnel.state()));
    if any(&["AWAITING_CREDENTIALS"]) {
        "plaitway-state-credentials-symbolic"
    } else if any(&["FAILED"]) {
        "plaitway-state-attention-symbolic"
    } else if any(&["CONNECTING"]) {
        "plaitway-state-connecting-symbolic"
    } else if any(&["RECONNECTING"]) {
        "plaitway-state-reconnecting-symbolic"
    } else if any(&["CONNECTED"]) {
        "plaitway-state-connected-symbolic"
    } else if any(&["DISCONNECTING"]) {
        "plaitway-state-disconnecting-symbolic"
    } else {
        "plaitway-state-idle-symbolic"
    }
}

/// The menu: a row per profile, which does what the profile needs, as Plaitway's own does.
pub fn menu(tunnels: &[Tunnel]) -> Vec<Entry> {
    let connected = tunnels.iter().filter(|tunnel| tunnel.state() == "CONNECTED").count();
    let mut entries = vec![
        Entry::note(if connected == 0 {
            "VPN：未連線".to_string()
        } else {
            format!("VPN：已連線 {connected} 個")
        }),
        Entry::separator(),
    ];
    for tunnel in tunnels {
        let (word, command) = match tunnel.state() {
            "CONNECTED" => ("", format!("{CLI} disconnect {}", tunnel.id)),
            "CONNECTING" => ("正在連線…", format!("{CLI} disconnect {}", tunnel.id)),
            "RECONNECTING" => ("正在重新連線…", format!("{CLI} disconnect {}", tunnel.id)),
            "DISCONNECTING" => ("正在中斷連線…", String::new()),
            "AWAITING_CREDENTIALS" => ("等待登入資訊", String::new()),
            "FAILED" => ("連線失敗", format!("{CLI} connect -no-wait {}", tunnel.id)),
            _ => ("", format!("{CLI} connect -no-wait {}", tunnel.id)),
        };
        let label = if word.is_empty() { tunnel.name.clone() } else { format!("{} · {word}", tunnel.name) };
        let entry = match tunnel.state() {
            "AWAITING_CREDENTIALS" => Entry::item(label, MenuCommand::SettingsPage(SETTINGS_PAGE)),
            "DISCONNECTING" => Entry::item(label, MenuCommand::Spawn(String::new())).enabled(false),
            _ => Entry::item(label, MenuCommand::Spawn(command)),
        };
        entries.push(entry.checked(tunnel.state() == "CONNECTED"));
    }
    entries.push(Entry::separator());
    if tunnels.iter().any(|tunnel| tunnel.desired_enabled) {
        entries.push(Entry::item("全部中斷連線", MenuCommand::Spawn(format!("{CLI} disconnect -all"))));
    }
    entries.push(Entry::item("VPN 設定⋯", MenuCommand::SettingsPage(SETTINGS_PAGE)));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watch_keeps_the_profiles_in_their_order() {
        let mut tunnels = Vec::new();
        apply(&mut tunnels, r#"{"snapshot":{"profiles":[{"id":"B","name":"lab","state":"PROFILE_STATE_DISCONNECTED","settings":{"priority":2}},{"id":"A","name":"office","state":"PROFILE_STATE_CONNECTED","desiredEnabled":true,"settings":{"priority":1},"status":{"rxBytes":"12"}}]}}"#).unwrap();
        assert_eq!(tunnels.iter().map(|tunnel| tunnel.name.as_str()).collect::<Vec<_>>(), ["office", "lab"]);
        apply(&mut tunnels, r#"{"changed":{"id":"B","name":"lab","state":"PROFILE_STATE_AWAITING_CREDENTIALS","settings":null}}"#).unwrap();
        assert!(tunnels.iter().any(|tunnel| tunnel.id == "B" && tunnel.asks_for_password()));
        assert_eq!(icon(&tunnels), "plaitway-state-credentials-symbolic");
        apply(&mut tunnels, r#"{"removed":"B"}"#).unwrap();
        assert_eq!(tunnels.len(), 1);
        assert_eq!(icon(&tunnels), "plaitway-state-connected-symbolic");
    }
}
