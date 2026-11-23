//! What is done to a device, each as root through `sudo -A` with Nekoland's askpass asking for
//! the password, as 磁碟 does: what is written to sysfs, and the rules that keep a device
//! disabled.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use super::model::{self, Device};

/// Runs `script` as root, with `args` as its `$1`, `$2`…, so that nothing in them is read as
/// the shell's.
fn root(script: &str, args: &[&str], reason: &str) -> Result<(), String> {
    let out = Command::new("sudo")
        .args(["-A", "--", "sh", "-c", script, "sh"])
        .args(args)
        .env("SUDO_ASKPASS", crate::program::path("nekoland-askpass"))
        .env("NEKOLAND_ASKPASS_REASON", reason)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("sudo: {err}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let lower = said.to_lowercase();
    if lower.contains("no password")
        || lower.contains("incorrect password")
        || lower.contains("a password is required")
    {
        return Err("沒有輸入管理員密碼".into());
    }
    let said = said
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_start_matches("sh: 1: "))
        .collect::<Vec<_>>()
        .join("\n");
    Err(if said.is_empty() {
        "sudo 失敗".into()
    } else {
        said
    })
}

fn sys(path: &str, name: &str) -> String {
    format!("/sys{path}/{name}")
}

/// Adds the rule `$1` to the rules `$2` unless it is there, then writes `$3` to `$4`, if given.
const DISABLE: &str = r#"grep -qxF -- "$1" "$2" 2>/dev/null || printf '%s\n' "$1" >> "$2" || exit 1
[ -z "$4" ] || printf '%s' "$3" > "$4""#;

/// Takes the rules starting `$1` out of `$2`, and the file when none is left, then writes `$3`
/// to `$4`.
const ENABLE: &str = r#"if [ -f "$2" ]; then grep -vF -- "$1" "$2" > "$2.new"; mv "$2.new" "$2" || exit 1; [ -s "$2" ] || rm -f "$2"; fi
printf '%s' "$3" > "$4""#;

/// Disables the device, and keeps it so: a USB device is let off the bus, anything else let go
/// of by its driver.
pub fn disable(device: &Device) -> Result<(), String> {
    let (bus, name) = (device.bus.as_str(), device.node_name());
    let rule = model::rule(bus, name, device.usb_device);
    let (value, target) = if device.usb_device {
        ("0".to_string(), sys(&device.node, "authorized"))
    } else if Path::new(&sys(&device.node, "driver")).exists() {
        (name.to_string(), sys(&device.node, "driver/unbind"))
    } else {
        (String::new(), String::new())
    };
    root(
        DISABLE,
        &[&rule, model::RULES, &value, &target],
        &format!("停用「{}」", device.name),
    )
}

/// Enables a device disabled here: its rule is taken away and its driver asked to take it.
pub fn enable(device: &Device) -> Result<(), String> {
    let (bus, name) = (device.bus.as_str(), device.node_name());
    let (value, target) = if device.usb_device {
        ("1".to_string(), sys(&device.node, "authorized"))
    } else {
        (name.to_string(), format!("/sys/bus/{bus}/drivers_probe"))
    };
    root(
        ENABLE,
        &[&model::rule_key(bus, name), model::RULES, &value, &target],
        &format!("啟用「{}」", device.name),
    )
}

pub fn can_uninstall(device: &Device) -> bool {
    !device.node.is_empty() && Path::new(&sys(&device.node, "remove")).exists()
}

/// Takes the device off its bus until hardware is scanned for again, or a USB device plugged in
/// again.
pub fn uninstall(device: &Device) -> Result<(), String> {
    root(
        r#"printf 1 > "$1""#,
        &[&sys(&device.node, "remove")],
        &format!("解除安裝「{}」", device.name),
    )
}

const PCI_RESCAN: &str = "/sys/bus/pci/rescan";

/// Looks for devices on the PCI bus again, as what was uninstalled.
pub fn scan() -> Result<(), String> {
    if !Path::new(PCI_RESCAN).exists() {
        return Ok(());
    }
    root(r#"printf 1 > "$1""#, &[PCI_RESCAN], "掃描硬體變更")
}

/// Whether the computer may turn the device off to save power.
pub fn set_suspend(device: &Device, on: bool) -> Result<(), String> {
    write_power(device, "control", if on { "auto" } else { "on" })
}

/// Whether the device may wake the computer.
pub fn set_wakeup(device: &Device, on: bool) -> Result<(), String> {
    write_power(device, "wakeup", if on { "enabled" } else { "disabled" })
}

fn write_power(device: &Device, name: &str, value: &str) -> Result<(), String> {
    let file = model::power_file(&device.node, name);
    root(
        r#"printf '%s' "$1" > "$2""#,
        &[value, &file.to_string_lossy()],
        &format!("變更「{}」的電源管理", device.name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, args: &[&str]) {
        let status = Command::new("sh")
            .args(["-c", script, "sh"])
            .args(args)
            .status()
            .expect("sh");
        assert!(status.success());
    }

    #[test]
    fn rules_are_added_once_and_taken_away_alone() {
        let dir = std::env::temp_dir().join(format!("nekoland-devices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (rules, target) = (dir.join("rules"), dir.join("unbind"));
        let (rules_path, target_path) = (rules.to_str().unwrap(), target.to_str().unwrap());
        let (first, second) = (model::rule("usb", "1-8", true), model::rule("usb", "1-8.3", true));
        sh(DISABLE, &[&first, rules_path, "0", target_path]);
        sh(DISABLE, &[&first, rules_path, "0", target_path]);
        sh(DISABLE, &[&second, rules_path, "", ""]);
        assert_eq!(
            std::fs::read_to_string(&rules).unwrap(),
            format!("{first}\n{second}\n")
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "0");
        sh(
            ENABLE,
            &[&model::rule_key("usb", "1-8"), rules_path, "1", target_path],
        );
        assert_eq!(std::fs::read_to_string(&rules).unwrap(), format!("{second}\n"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "1");
        sh(
            ENABLE,
            &[&model::rule_key("usb", "1-8.3"), rules_path, "1", target_path],
        );
        assert!(!rules.exists(), "an empty rules file is taken away");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
