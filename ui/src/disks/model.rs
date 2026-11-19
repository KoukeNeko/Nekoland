//! What `lsblk` and `losetup` say of the disks, as the drives, their partitions and the free
//! space between them.

use std::{collections::HashMap, process::Command};

use serde_json::Value;

const COLUMNS: &str = "NAME,KNAME,PATH,SIZE,TYPE,FSTYPE,LABEL,UUID,MOUNTPOINTS,MODEL,VENDOR,TRAN,RM,RO,\
                       HOTPLUG,PTTYPE,PARTN,PARTLABEL,START,FSUSED,LOG-SEC";
/// Free space smaller than this is not shown: it is what alignment leaves.
const MIN_FREE: u64 = 8 << 20;
/// What a partition table keeps at the start and the end of a disk.
pub const TABLE_SPACE: u64 = 1 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Internal,
    External,
    Image,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Part {
    pub path: String,
    pub kname: String,
    /// 0 for a filesystem that fills a disk with no table.
    pub number: u32,
    pub name: String,
    pub fstype: String,
    pub uuid: String,
    /// The device, and where it is mounted, of this partition and what is in it.
    pub mounts: Vec<(String, String)>,
    pub size: u64,
    /// In bytes from the start of the disk.
    pub start: u64,
    pub used: Option<u64>,
}

impl Part {
    pub fn is_swap(&self) -> bool {
        self.fstype == "swap"
    }

    pub fn mounted(&self) -> bool {
        !self.mounts.is_empty()
    }

    /// Where it is mounted, first.
    pub fn mount_point(&self) -> Option<&str> {
        self.mounts.first().map(|(_, point)| point.as_str()).filter(|point| !point.starts_with('['))
    }

    /// Whether the system runs from it, so it is not unmounted or formatted.
    pub fn is_system(&self) -> bool {
        self.mounts.iter().any(|(_, point)| ["/", "/boot", "/boot/efi", "/usr", "/var", "/etc"].contains(&point.as_str()))
    }

    pub fn label(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if self.is_swap() {
            "swap".into()
        } else if let Some(point) = self.mount_point() {
            match point {
                "/" => "系統".into(),
                "/boot/efi" => "EFI".into(),
                "/boot" => "開機".into(),
                "/home" => "資料".into(),
                _ => point.rsplit('/').next().unwrap_or(point).to_string(),
            }
        } else if self.fstype == "vfat" && self.size < (1 << 30) {
            "EFI".into()
        } else {
            self.kname.clone()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Slot {
    Part(usize),
    /// Free space, from this byte to that.
    Free(u64, u64),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Disk {
    pub path: String,
    pub kname: String,
    pub name: String,
    pub size: u64,
    pub kind: Kind,
    pub read_only: bool,
    pub tran: String,
    pub table: Option<String>,
    pub sector: u64,
    pub backing: Option<String>,
    pub parts: Vec<Part>,
}

impl Disk {
    /// The partitions and the free space between them, in order.
    pub fn slots(&self) -> Vec<Slot> {
        let mut parts: Vec<usize> = (0..self.parts.len()).collect();
        parts.sort_by_key(|&index| self.parts[index].start);
        let mut slots = Vec::new();
        let mut at = TABLE_SPACE;
        for index in parts {
            let part = &self.parts[index];
            if self.table.is_some() && part.start > at + MIN_FREE {
                slots.push(Slot::Free(at, part.start));
            }
            slots.push(Slot::Part(index));
            at = at.max(part.start + part.size);
        }
        let end = self.size.saturating_sub(TABLE_SPACE);
        if self.table.is_some() && end > at + MIN_FREE {
            slots.push(Slot::Free(at, end));
        }
        slots
    }

    /// The free space that follows `part`, in bytes.
    pub fn free_after(&self, part: &Part) -> u64 {
        let end = part.start + part.size;
        self.slots()
            .into_iter()
            .find_map(|slot| match slot {
                Slot::Free(start, to) if start.abs_diff(end) < MIN_FREE => Some(to - start),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub fn used(&self) -> Option<u64> {
        let used: Vec<u64> = self.parts.iter().filter_map(|part| part.used).collect();
        (!used.is_empty()).then(|| used.iter().sum())
    }

    /// The share of it filled, in percent, of what is known.
    pub fn fill(&self) -> Option<f64> {
        let known: u64 = self.parts.iter().filter(|part| part.used.is_some()).map(|part| part.size).sum();
        (known > 0).then(|| self.used().unwrap_or(0) as f64 / known as f64 * 100.0)
    }

    pub fn title(&self) -> String {
        match self.kind {
            Kind::Image => self.backing.as_deref().and_then(|file| file.rsplit('/').next()).unwrap_or(&self.kname).to_string(),
            _ if !self.name.is_empty() => self.name.clone(),
            _ => match interface(&self.tran) {
                Some(interface) => format!("{interface} 磁碟"),
                None => self.kname.clone(),
            },
        }
    }
}

pub fn interface(tran: &str) -> Option<String> {
    Some(match tran {
        "" => return None,
        "usb" => "USB".into(),
        "nvme" => "NVMe".into(),
        "sata" | "ata" => "SATA".into(),
        "virtio" => "VirtIO".into(),
        other => other.to_uppercase(),
    })
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().trim().to_string()
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|number| number.as_u64().or_else(|| number.as_str()?.parse().ok()))
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(|flag| flag.as_bool().unwrap_or_else(|| matches!(flag.as_str(), Some("1" | "true"))))
}

/// Every mount in `node` and under it: the device and where it is mounted.
fn mounts_in(node: &Value, into: &mut Vec<(String, String)>) {
    let path = text(node, "path");
    for point in node.get("mountpoints").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        into.push((path.clone(), point.to_string()));
    }
    for child in node.get("children").and_then(Value::as_array).into_iter().flatten() {
        mounts_in(child, into);
    }
}

/// The first amount of use in `node` or under it.
fn used_in(node: &Value) -> Option<u64> {
    number(node, "fsused").or_else(|| node.get("children")?.as_array()?.iter().find_map(used_in))
}

fn part_of(node: &Value, sector: u64) -> Part {
    let mut mounts = Vec::new();
    mounts_in(node, &mut mounts);
    let mut fstype = text(node, "fstype");
    if fstype == "LVM2_member" {
        fstype = "LVM".into();
    }
    let name = match text(node, "label") {
        label if !label.is_empty() => label,
        _ => text(node, "partlabel"),
    };
    Part {
        path: text(node, "path"),
        kname: text(node, "kname"),
        number: number(node, "partn").unwrap_or(0) as u32,
        name,
        fstype,
        uuid: text(node, "uuid"),
        mounts,
        size: number(node, "size").unwrap_or(0),
        start: number(node, "start").unwrap_or(0) * sector,
        used: used_in(node),
    }
}

/// The disks in `lsblk`'s JSON; `backing` says which file each loop device is of.
pub fn parse(lsblk: &str, backing: &HashMap<String, String>) -> Result<Vec<Disk>, String> {
    let json: Value = serde_json::from_str(lsblk).map_err(|err| format!("lsblk: {err}"))?;
    let mut disks = Vec::new();
    for node in json.get("blockdevices").and_then(Value::as_array).into_iter().flatten() {
        let kind = text(node, "type");
        let name = text(node, "kname");
        let path = text(node, "path");
        let size = number(node, "size").unwrap_or(0);
        let fstype = text(node, "fstype");
        let file = backing.get(&path).cloned().or_else(|| Some(text(node, "back-file")).filter(|file| !file.is_empty()));
        let image = match (kind.as_str(), file) {
            ("loop", Some(file)) if !file.starts_with("/var/lib/snapd") => Some(file),
            ("loop", _) => continue,
            ("disk", _) if !name.starts_with("zram") && !name.starts_with("ram") => None,
            // A disc is shown while it holds something.
            ("rom", _) if !fstype.is_empty() => None,
            _ => continue,
        };
        let sector = number(node, "log-sec").unwrap_or(512).max(512);
        let mut parts: Vec<Part> = node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|child| text(child, "type") == "part")
            .map(|child| part_of(child, sector))
            .collect();
        // A filesystem with no table fills the disk.
        if parts.is_empty() && !fstype.is_empty() {
            parts.push(part_of(node, sector));
        }
        let tran = text(node, "tran");
        let external = flag(node, "rm") || flag(node, "hotplug") || tran == "usb";
        disks.push(Disk {
            path,
            kname: name,
            name: [text(node, "vendor"), text(node, "model")]
                .iter()
                .filter(|part| !part.is_empty() && !part.starts_with("0x"))
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
            size,
            kind: if image.is_some() {
                Kind::Image
            } else if external {
                Kind::External
            } else {
                Kind::Internal
            },
            read_only: flag(node, "ro"),
            tran,
            table: Some(text(node, "pttype")).filter(|table| !table.is_empty()),
            sector,
            backing: image,
            parts,
        });
    }
    Ok(disks)
}

/// Which file each loop device is of.
fn backing_files() -> HashMap<String, String> {
    let Ok(output) = Command::new("losetup").args(["-J", "-l", "-O", "NAME,BACK-FILE"]).output() else {
        return HashMap::new();
    };
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    json.get("loopdevices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|device| Some((text(device, "name"), text(device, "back-file"))))
        .filter(|(_, file)| !file.is_empty())
        .collect()
}

/// The disks now. `NEKOLAND_DISKS_LSBLK` names a file of `lsblk -J -b` output to show instead,
/// to try the window on a machine without such drives.
pub fn read() -> Result<Vec<Disk>, String> {
    if let Some(file) = std::env::var_os("NEKOLAND_DISKS_LSBLK") {
        let json = std::fs::read_to_string(&file).map_err(|err| format!("{}: {err}", file.to_string_lossy()))?;
        return parse(&json, &HashMap::new());
    }
    let output = Command::new("lsblk")
        .args(["-J", "-b", "-o", COLUMNS])
        .env("LC_ALL", "C")
        .output()
        .map_err(|err| format!("lsblk: {err}"))?;
    if !output.status.success() {
        return Err(format!("lsblk: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let mut disks = parse(&String::from_utf8_lossy(&output.stdout), &backing_files())?;
    disks.sort_by_key(|disk| (disk.kind as u8, disk.kname.clone()));
    Ok(disks)
}

/// An amount of bytes as GB, MB or the like, in tens, as the drives' makers count.
pub fn size(bytes: u64) -> String {
    let value = bytes as f64;
    let (amount, unit) = if value >= 1e12 {
        (value / 1e12, "TB")
    } else if value >= 1e9 {
        (value / 1e9, "GB")
    } else if value >= 1e6 {
        (value / 1e6, "MB")
    } else if value >= 1e3 {
        (value / 1e3, "KB")
    } else {
        return format!("{bytes} B");
    };
    if amount >= 100.0 || (amount - amount.round()).abs() < 0.05 {
        format!("{amount:.0} {unit}")
    } else {
        format!("{amount:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"blockdevices":[
      {"name":"vda","kname":"vda","path":"/dev/vda","size":68719476736,"type":"disk","fstype":null,"label":null,"uuid":null,"mountpoints":[],"model":"Disk","vendor":"0x1af4","tran":"virtio","rm":false,"ro":false,"hotplug":false,"pttype":"gpt","partn":null,"partlabel":null,"start":null,"fsused":null,"log-sec":512,"children":[
        {"name":"vda1","kname":"vda1","path":"/dev/vda1","size":1127219200,"type":"part","fstype":"vfat","label":null,"uuid":"974B-10D0","mountpoints":["/boot/efi"],"partn":1,"partlabel":null,"start":2048,"fsused":6840320,"log-sec":512},
        {"name":"vda3","kname":"vda3","path":"/dev/vda3","size":32000000000,"type":"part","fstype":"LVM2_member","label":null,"uuid":"x","mountpoints":[],"partn":3,"partlabel":null,"start":6397952,"fsused":null,"log-sec":512,"children":[
          {"name":"lv","kname":"dm-0","path":"/dev/mapper/lv","size":30000000000,"type":"lvm","fstype":"ext4","mountpoints":["/"],"fsused":20000000000}]}]},
      {"name":"loop0","kname":"loop0","path":"/dev/loop0","size":4096,"type":"loop","fstype":"squashfs","mountpoints":["/snap/bare/5"],"ro":true,"log-sec":512},
      {"name":"loop9","kname":"loop9","path":"/dev/loop9","size":6200000000,"type":"loop","fstype":"iso9660","label":"Ubuntu","mountpoints":["/media/iso"],"ro":true,"log-sec":512,"fsused":6200000000},
      {"name":"sr0","kname":"sr0","path":"/dev/sr0","size":0,"type":"rom","fstype":null,"mountpoints":[],"log-sec":2048}]}"#;

    fn disks() -> Vec<Disk> {
        let backing = HashMap::from([
            ("/dev/loop0".to_string(), "/var/lib/snapd/snaps/bare_5.snap".to_string()),
            ("/dev/loop9".to_string(), "/home/a/ubuntu.iso".to_string()),
        ]);
        parse(SAMPLE, &backing).unwrap()
    }

    #[test]
    fn snap_loops_and_empty_drives_are_left_out() {
        let names: Vec<_> = disks().iter().map(|disk| disk.kname.clone()).collect();
        assert_eq!(names, ["vda", "loop9"]);
    }

    #[test]
    fn a_partition_holding_a_logical_volume_takes_its_mount_and_use() {
        let disks = disks();
        let lvm = &disks[0].parts[1];
        assert_eq!(lvm.mount_point(), Some("/"));
        assert_eq!(lvm.used, Some(20_000_000_000));
        assert!(lvm.is_system());
        assert_eq!(lvm.fstype, "LVM");
    }

    #[test]
    fn free_space_is_found_between_and_after_partitions() {
        let disk = &disks()[0];
        let slots = disk.slots();
        // vda1 starts at 1 MiB; the gap after vda1 up to vda3 is the (absent) vda2.
        assert!(matches!(slots[0], Slot::Part(0)));
        assert!(matches!(slots[1], Slot::Free(..)));
        assert!(matches!(slots[2], Slot::Part(1)));
        assert!(matches!(slots.last(), Some(Slot::Free(..))));
    }

    #[test]
    fn an_image_is_named_by_its_file_and_a_bare_filesystem_is_one_partition() {
        let image = &disks()[1];
        assert_eq!(image.kind, Kind::Image);
        assert_eq!(image.title(), "ubuntu.iso");
        assert_eq!(image.parts.len(), 1);
        assert_eq!(image.parts[0].number, 0);
        assert!(image.slots().len() == 1);
    }

    #[test]
    fn sizes_are_counted_in_tens() {
        assert_eq!(size(512_000_000), "512 MB");
        assert_eq!(size(1_000_000_000_000), "1 TB");
        assert_eq!(size(6_200_000_000), "6.2 GB");
        assert_eq!(size(480_103_981_056), "480 GB");
    }
}
