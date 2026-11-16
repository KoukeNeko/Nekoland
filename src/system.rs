//! The system's own items in the menu bar, as on the Mac: the sound, and the network; and
//! what the control center reads besides, the sound outputs and the display's brightness.

use std::{net::Ipv4Addr, path::Path, process::Command, time::Duration};

use smithay::reexports::calloop::channel::Sender;

use crate::dbus::Event;

/// How often the sound is looked at; wpctl has no way to say when it changes.
const SOUND_POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub percent: u32,
    pub muted: bool,
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
                let now = read_volume();
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

fn read_volume() -> Option<Volume> {
    let output = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
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

/// A device PipeWire plays sound on.
#[derive(Clone, Debug, PartialEq)]
pub struct Sink {
    pub id: u32,
    pub description: String,
    pub default: bool,
}

/// The sound outputs, the default one marked; none without PipeWire.
pub fn read_sinks() -> Vec<Sink> {
    let Ok(output) = Command::new("pw-dump").output() else {
        return Vec::new();
    };
    parse_sinks(&String::from_utf8_lossy(&output.stdout))
}

fn parse_sinks(dump: &str) -> Vec<Sink> {
    use serde_json::Value;
    let objects: Vec<Value> = match serde_json::from_str(dump) {
        Ok(objects) => objects,
        Err(err) => {
            tracing::warn!("pw-dump: {err}");
            return Vec::new();
        }
    };
    // PipeWire keeps the default by node name, in its "default" metadata.
    let default = objects
        .iter()
        .filter(|object| object["props"]["metadata.name"] == "default")
        .flat_map(|object| object["metadata"].as_array().into_iter().flatten())
        .find(|entry| entry["key"] == "default.audio.sink")
        .and_then(|entry| entry["value"]["name"].as_str());
    objects
        .iter()
        .filter_map(|object| {
            let props = &object["info"]["props"];
            if props["media.class"] != "Audio/Sink" {
                return None;
            }
            let name = props["node.name"].as_str()?;
            Some(Sink {
                id: u32::try_from(object["id"].as_u64()?).ok()?,
                description: props["node.description"].as_str().unwrap_or(name).to_string(),
                default: Some(name) == default,
            })
        })
        .collect()
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
    fn sinks_are_read_from_pw_dump() {
        let dump = r#"[
            {"id": 30, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
             "metadata": [{"key": "default.audio.sink", "value": {"name": "alsa_output.usb"}}]},
            {"id": 41, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.pci", "node.description": "Built-in Audio"}}},
            {"id": 42, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.usb"}}},
            {"id": 43, "info": {"props": {"media.class": "Audio/Source", "node.name": "alsa_input.pci"}}}
        ]"#;
        assert_eq!(
            parse_sinks(dump),
            vec![
                Sink {
                    id: 41,
                    description: "Built-in Audio".into(),
                    default: false
                },
                Sink {
                    id: 42,
                    description: "alsa_output.usb".into(),
                    default: true
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
