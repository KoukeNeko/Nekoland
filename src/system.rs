//! The system's own items in the menu bar, as on the Mac: the sound, and the network; and
//! what the control center reads besides: the sound outputs and inputs, the microphone's volume, the display's
//! brightness, the Wi-Fi networks, whether there is Bluetooth, and what plays.

use std::{collections::HashMap, net::Ipv4Addr, path::Path, process::Command, time::Duration};

use smithay::reexports::calloop::channel::Sender;

use crate::dbus::Event;

/// How often the sound is looked at; wpctl has no way to say when it changes.
const SOUND_POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub percent: u32,
    pub muted: bool,
}

/// The computer's own battery, as the kernel tells it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    /// Plugged in and full, or held below full to spare it.
    pub plugged_full: bool,
}

/// A battery from its `type`, `scope`, `capacity` and `status` files; a mouse's or a
/// keyboard's, whose scope is "Device", is not the computer's.
fn battery_from(kind: &str, scope: &str, capacity: &str, status: &str) -> Option<Battery> {
    if kind.trim() != "Battery" || scope.trim() == "Device" {
        return None;
    }
    let status = status.trim();
    Some(Battery {
        percent: capacity.trim().parse::<u8>().ok()?.min(100),
        charging: status == "Charging",
        plugged_full: status == "Full" || status == "Not charging",
    })
}

/// The first of the computer's batteries.
// shortcut: a second battery, as some ThinkPads have, is left out; weigh them by their energy
// once one is seen.
pub fn read_battery() -> Option<Battery> {
    let read = |path: &std::path::Path, name: &str| std::fs::read_to_string(path.join(name)).unwrap_or_default();
    let mut supplies: Vec<std::path::PathBuf> =
        std::fs::read_dir("/sys/class/power_supply").ok()?.flatten().map(|entry| entry.path()).collect();
    supplies.sort();
    supplies.iter().find_map(|path| {
        battery_from(&read(path, "type"), &read(path, "scope"), &read(path, "capacity"), &read(path, "status"))
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub wireless: bool,
    pub interface: String,
    pub connected: bool,
    pub address: Option<Ipv4Addr>,
}

/// Watches the default output's volume on a thread, and says when it changes. Without
/// PipeWire there is nothing to watch, and the menu bar shows no sound item.
pub fn watch_sound(events: Sender<Event>) {
    let spawned = std::thread::Builder::new()
        .name("nekoland-sound".into())
        .spawn(move || {
            let mut last = None;
            loop {
                let now = read_volume(DEFAULT_SINK);
                if now != last {
                    if events.send(Event::Sound(now)).is_err() {
                        return;
                    }
                    last = now;
                }
                std::thread::sleep(SOUND_POLL);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("no sound item: {err}");
    }
}

/// Reads which way up the computer is from iio-sensor-proxy, where there is an
/// accelerometer, and says so whenever it turns.
pub fn watch_orientation(events: Sender<Event>) {
    let spawned = std::thread::Builder::new().name("nekoland-orientation".into()).spawn(move || {
        let read = || -> zbus::Result<()> {
            let bus = zbus::blocking::Connection::system()?;
            let proxy = zbus::blocking::Proxy::new(&bus, SENSOR_PROXY, "/net/hadess/SensorProxy", SENSOR_PROXY)?;
            if !proxy.get_property::<bool>("HasAccelerometer")? {
                return Ok(());
            }
            proxy.call_method("ClaimAccelerometer", &())?;
            let mut last = String::new();
            loop {
                let now: String = proxy.get_property("AccelerometerOrientation")?;
                if now != last && now != "undefined" {
                    if events.send(Event::Orientation(now.clone())).is_err() {
                        return Ok(());
                    }
                    last = now;
                }
                std::thread::sleep(ORIENTATION_POLL);
            }
        };
        // Without iio-sensor-proxy, or an accelerometer, there is nothing to turn.
        if let Err(err) = read() {
            tracing::debug!("no orientation: {err}");
        }
    });
    if let Err(err) = spawned {
        tracing::warn!("no orientation: {err}");
    }
}

const SENSOR_PROXY: &str = "net.hadess.SensorProxy";
const ORIENTATION_POLL: std::time::Duration = std::time::Duration::from_millis(500);

pub const DEFAULT_SINK: &str = "@DEFAULT_AUDIO_SINK@";
pub const DEFAULT_SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

/// The volume of `target`, the default output or input as wpctl names them.
pub fn read_volume(target: &str) -> Option<Volume> {
    let output = Command::new("wpctl").args(["get-volume", target]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_volume(&String::from_utf8_lossy(&output.stdout))
}

/// wpctl says "Volume: 0.40", followed by " [MUTED]" when muted.
fn parse_volume(text: &str) -> Option<Volume> {
    let level: f64 = text
        .trim()
        .strip_prefix("Volume:")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(Volume {
        percent: (level * 100.0).round() as u32,
        muted: text.contains("[MUTED]"),
    })
}

/// A device PipeWire plays sound on, or records it from.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioDevice {
    pub id: u32,
    pub description: String,
    pub default: bool,
}

/// The sound outputs and inputs, the default of each marked; none without PipeWire.
pub fn read_audio_devices() -> (Vec<AudioDevice>, Vec<AudioDevice>) {
    let Ok(output) = Command::new("pw-dump").output() else {
        return Default::default();
    };
    parse_audio_devices(&String::from_utf8_lossy(&output.stdout))
}

fn parse_audio_devices(dump: &str) -> (Vec<AudioDevice>, Vec<AudioDevice>) {
    use serde_json::Value;
    let objects: Vec<Value> = match serde_json::from_str(dump) {
        Ok(objects) => objects,
        Err(err) => {
            tracing::warn!("pw-dump: {err}");
            return Default::default();
        }
    };
    let devices = |class: &str, default_key: &str| {
        // PipeWire keeps the default by node name, in its "default" metadata.
        let default = objects
            .iter()
            .filter(|object| object["props"]["metadata.name"] == "default")
            .flat_map(|object| object["metadata"].as_array().into_iter().flatten())
            .find(|entry| entry["key"] == default_key)
            .and_then(|entry| entry["value"]["name"].as_str());
        objects
            .iter()
            .filter_map(|object| {
                let props = &object["info"]["props"];
                if props["media.class"] != class {
                    return None;
                }
                let name = props["node.name"].as_str()?;
                Some(AudioDevice {
                    id: u32::try_from(object["id"].as_u64()?).ok()?,
                    description: props["node.description"].as_str().unwrap_or(name).to_string(),
                    default: Some(name) == default,
                })
            })
            .collect()
    };
    (
        devices("Audio/Sink", "default.audio.sink"),
        devices("Audio/Source", "default.audio.source"),
    )
}

/// The display's brightness in percent, from its backlight; none without one.
pub fn read_brightness() -> Option<u32> {
    let output = Command::new("brightnessctl")
        .args(["--class=backlight", "--machine-readable", "info"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_brightness(&String::from_utf8_lossy(&output.stdout))
}

/// brightnessctl says "device,class,current,percent%,max".
fn parse_brightness(text: &str) -> Option<u32> {
    text.lines()
        .next()?
        .split(',')
        .nth(3)?
        .strip_suffix('%')?
        .parse()
        .ok()
}

/// Runs a program on a thread of its own, which waits for it so that it leaves nothing
/// behind; what it is given goes to it as it is, not through a shell.
pub fn run(program: &str, args: &[&str]) {
    let mut command = Command::new(program);
    command.args(args);
    let program = program.to_string();
    std::thread::spawn(move || match command.status() {
        Ok(status) if !status.success() => tracing::warn!("{program}: {status}"),
        Ok(_) => {}
        Err(err) => tracing::warn!("{program}: {err}"),
    });
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub on: bool,
    pub networks: Vec<WifiNetwork>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u32,
    pub active: bool,
    /// Whether joining it takes a password.
    pub secure: bool,
}

/// How many networks the control center lists, the strongest.
const WIFI_SHOWN: usize = 6;

/// Wi-Fi as NetworkManager sees it; none without NetworkManager or a wireless device.
pub fn read_wifi() -> Option<Wifi> {
    let nmcli = |args: &[&str]| {
        let output = Command::new("nmcli").args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let on = nmcli(&["-t", "-f", "WIFI", "radio"])?.trim() == "enabled";
    let list = nmcli(&[
        "-t",
        "-f",
        "IN-USE,SIGNAL,SECURITY,SSID",
        "device",
        "wifi",
        "list",
        "--rescan",
        "no",
    ])?;
    Some(Wifi {
        on,
        networks: parse_wifi(&list),
    })
}

/// nmcli's terse lines, "*:72:WPA2:Home", with colons in the name escaped; a network heard
/// through several access points is listed once, by its strongest.
fn parse_wifi(text: &str) -> Vec<WifiNetwork> {
    let mut networks: Vec<WifiNetwork> = Vec::new();
    for line in text.lines() {
        let mut fields = line.splitn(4, ':');
        let (Some(in_use), Some(signal), Some(security), Some(ssid)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let ssid = ssid.replace("\\:", ":").replace("\\\\", "\\");
        let Ok(signal) = signal.parse() else {
            continue;
        };
        if ssid.is_empty() {
            continue;
        }
        let network = WifiNetwork {
            ssid,
            signal,
            active: in_use == "*",
            // OWE encrypts without a password.
            secure: !matches!(security, "" | "--" | "OWE"),
        };
        match networks.iter_mut().find(|known| known.ssid == network.ssid) {
            Some(known) => {
                known.active |= network.active;
                known.signal = known.signal.max(network.signal);
            }
            None => networks.push(network),
        }
    }
    networks.sort_by_key(|network| (!network.active, std::cmp::Reverse(network.signal)));
    networks.truncate(WIFI_SHOWN);
    networks
}

/// Joins a network with what NetworkManager has kept for it; when that is not enough, as
/// for a network with a password not joined before, Settings asks for what it needs.
pub fn join_wifi(ssid: &str) {
    let ssid = ssid.to_string();
    std::thread::spawn(move || {
        let joined = Command::new("nmcli")
            .args(["device", "wifi", "connect", &ssid])
            .output();
        match joined {
            Ok(output) if output.status.success() => return,
            Ok(output) => tracing::warn!("nmcli: {}", String::from_utf8_lossy(&output.stderr).trim()),
            Err(err) => {
                tracing::warn!("nmcli: {err}");
                return;
            }
        }
        if let Err(err) = Command::new("nekoland-settings")
            .env("NEKOLAND_SETTINGS_PAGE", "Wi-Fi")
            .env("NEKOLAND_WIFI_JOIN", &ssid)
            .status()
        {
            tracing::warn!("nekoland-settings: {err}");
        }
    });
}

/// Whether the computer has a Bluetooth adapter.
pub fn has_bluetooth() -> bool {
    std::fs::read_dir("/sys/class/bluetooth").is_ok_and(|mut adapters| adapters.next().is_some())
}

/// A program playing, or able to play, through MPRIS.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub bus: String,
    pub app: String,
    pub title: String,
    pub artist: String,
    /// A local picture of what plays, as its cover.
    pub art: Option<String>,
    pub playing: bool,
}

const MPRIS: &str = "org.mpris.MediaPlayer2";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";

/// The player to show: one that plays, or else the first with something to play.
pub fn read_player() -> Option<Player> {
    use zbus::{blocking::Connection, zvariant::OwnedValue};
    let connection = Connection::session().ok()?;
    let reply = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .ok()?;
    let (names,) = reply.body().deserialize::<(Vec<String>,)>().ok()?;
    let all = |bus: &str, interface: &str| -> Option<HashMap<String, OwnedValue>> {
        let reply = connection
            .call_method(
                Some(bus),
                MPRIS_PATH,
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &(interface,),
            )
            .ok()?;
        Some(
            reply
                .body()
                .deserialize::<(HashMap<String, OwnedValue>,)>()
                .ok()?
                .0,
        )
    };
    let mut players: Vec<Player> = names
        .iter()
        .filter(|name| name.starts_with(&format!("{MPRIS}.")))
        .filter_map(|bus| {
            let player = all(bus, &format!("{MPRIS}.Player"))?;
            let text = |value: &OwnedValue| String::try_from(value.clone()).ok();
            let metadata: HashMap<String, OwnedValue> = player
                .get("Metadata")
                .and_then(|value| HashMap::try_from(value.clone()).ok())
                .unwrap_or_default();
            let title = metadata.get("xesam:title").and_then(text).unwrap_or_default();
            if title.is_empty() {
                return None;
            }
            let artist = metadata
                .get("xesam:artist")
                .and_then(|value| Vec::<String>::try_from(value.clone()).ok())
                .map(|artists| artists.join("、"))
                .unwrap_or_default();
            let art = metadata
                .get("mpris:artUrl")
                .and_then(text)
                .and_then(|url| url.strip_prefix("file://").map(String::from));
            let app = all(bus, MPRIS)
                .and_then(|root| root.get("Identity").and_then(text))
                .unwrap_or_default();
            Some(Player {
                bus: bus.clone(),
                app,
                title,
                artist,
                art,
                playing: player.get("PlaybackStatus").and_then(text).as_deref() == Some("Playing"),
            })
        })
        .collect();
    players.sort_by_key(|player| !player.playing);
    players.into_iter().next()
}

/// Asks a player to `method`, as PlayPause or Next.
pub fn player_command(bus: &str, method: &str) {
    let connection = match zbus::blocking::Connection::session() {
        Ok(connection) => connection,
        Err(err) => return tracing::warn!("{bus}: {err}"),
    };
    if let Err(err) = connection.call_method(
        Some(bus),
        MPRIS_PATH,
        Some(&*format!("{MPRIS}.Player")),
        method,
        &(),
    ) {
        tracing::warn!("{bus}: {method}: {err}");
    }
}

/// The interface the computer is most likely online through: the first one that is up,
/// or else the first one there is.
pub fn read_network() -> Option<Network> {
    let mut interfaces: Vec<Network> = std::fs::read_dir("/sys/class/net")
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let interface = entry.file_name().to_string_lossy().to_string();
            let dir = entry.path();
            // Loopback, bridges and the like have no device of their own.
            if !dir.join("device").exists() {
                return None;
            }
            let state = std::fs::read_to_string(dir.join("operstate")).unwrap_or_default();
            Some(Network {
                wireless: Path::new(&dir).join("wireless").exists(),
                connected: state.trim() == "up",
                address: ipv4_address(&interface),
                interface,
            })
        })
        .collect();
    interfaces.sort_by_key(|network| (!network.connected, network.interface.clone()));
    interfaces.into_iter().next()
}

fn ipv4_address(interface: &str) -> Option<Ipv4Addr> {
    let mut found = None;
    // SAFETY: getifaddrs hands over a list that stays valid until freeifaddrs, and the
    // address of an AF_INET entry is a sockaddr_in.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return None;
        }
        let mut entry = list;
        while !entry.is_null() {
            let address = (*entry).ifa_addr;
            let name = std::ffi::CStr::from_ptr((*entry).ifa_name).to_string_lossy();
            if !address.is_null() && i32::from((*address).sa_family) == libc::AF_INET && name == interface {
                let address = &*(address as *const libc::sockaddr_in);
                found = Some(Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)));
                break;
            }
            entry = (*entry).ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computers_battery_is_read() {
        assert_eq!(
            battery_from("Battery\n", "", "57\n", "Charging\n"),
            Some(Battery { percent: 57, charging: true, plugged_full: false })
        );
        assert_eq!(
            battery_from("Battery", "System", "100", "Full"),
            Some(Battery { percent: 100, charging: false, plugged_full: true })
        );
        assert_eq!(battery_from("Battery", "Device", "80", "Discharging"), None);
        assert_eq!(battery_from("Mains", "", "", ""), None);
    }

    #[test]
    fn audio_devices_are_read_from_pw_dump() {
        let dump = r#"[
            {"id": 30, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
             "metadata": [{"key": "default.audio.sink", "value": {"name": "alsa_output.usb"}}]},
            {"id": 41, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.pci", "node.description": "Built-in Audio"}}},
            {"id": 42, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.usb"}}},
            {"id": 43, "info": {"props": {"media.class": "Audio/Source", "node.name": "alsa_input.pci"}}}
        ]"#;
        assert_eq!(
            parse_audio_devices(dump),
            (
                vec![
                    AudioDevice {
                        id: 41,
                        description: "Built-in Audio".into(),
                        default: false
                    },
                    AudioDevice {
                        id: 42,
                        description: "alsa_output.usb".into(),
                        default: true
                    },
                ],
                vec![AudioDevice {
                    id: 43,
                    description: "alsa_input.pci".into(),
                    default: false
                },]
            )
        );
    }

    #[test]
    fn wifi_networks_are_read_from_nmcli() {
        let list = "*:72:WPA2:Home\n:40:WPA2:Home\n:80:--:Caf\\:e\n:20::\n:55:WPA2 802.1X:Office\n";
        assert_eq!(
            parse_wifi(list),
            vec![
                WifiNetwork {
                    ssid: "Home".into(),
                    signal: 72,
                    active: true,
                    secure: true
                },
                WifiNetwork {
                    ssid: "Caf:e".into(),
                    signal: 80,
                    active: false,
                    secure: false
                },
                WifiNetwork {
                    ssid: "Office".into(),
                    signal: 55,
                    active: false,
                    secure: true
                },
            ]
        );
    }

    #[test]
    fn brightness_is_read_from_brightnessctl() {
        assert_eq!(
            parse_brightness("intel_backlight,backlight,4800,50%,9600\n"),
            Some(50)
        );
        assert_eq!(parse_brightness(""), None);
    }

    #[test]
    fn volume_is_read_from_wpctl() {
        assert_eq!(
            parse_volume("Volume: 0.40\n"),
            Some(Volume {
                percent: 40,
                muted: false
            })
        );
        assert_eq!(
            parse_volume("Volume: 1.00 [MUTED]\n"),
            Some(Volume {
                percent: 100,
                muted: true
            })
        );
        assert_eq!(parse_volume("not a volume"), None);
    }
}
