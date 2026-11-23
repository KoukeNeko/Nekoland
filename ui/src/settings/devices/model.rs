//! The devices of this computer, from udev's database, sorted into the kinds Windows' Device
//! Manager shows them under, with what a device's 內容 shows: its driver, its resources, its
//! power settings and what the kernel said of it.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
};

/// Where a device disabled here is kept disabled across restarts.
pub const RULES: &str = "/etc/udev/rules.d/61-nekoland-disabled.rules";
const SYS: &str = "/sys";
/// The vendor of the root hubs the kernel makes for each USB controller.
const LINUX_FOUNDATION: &str = "1d6b";
/// A USB device of this class is a hub, as `TYPE` writes it, in decimal.
const USB_HUB_CLASS: &str = "9";
/// How many lines of the kernel's log a device's 事件 shows.
const EVENTS: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Audio,
    Battery,
    Bluetooth,
    Camera,
    Computer,
    Disk,
    Display,
    Optical,
    Hid,
    Keyboard,
    Mouse,
    Monitor,
    Network,
    Other,
    Processor,
    Security,
    Sensor,
    Storage,
    System,
    UsbController,
    Usb,
}

/// In the order Windows lists them.
pub const KINDS: [Kind; 21] = [
    Kind::Audio,
    Kind::Battery,
    Kind::Bluetooth,
    Kind::Camera,
    Kind::Computer,
    Kind::Disk,
    Kind::Display,
    Kind::Optical,
    Kind::Hid,
    Kind::Keyboard,
    Kind::Mouse,
    Kind::Monitor,
    Kind::Network,
    Kind::Other,
    Kind::Processor,
    Kind::Security,
    Kind::Sensor,
    Kind::Storage,
    Kind::System,
    Kind::UsbController,
    Kind::Usb,
];

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Audio => "音效、視訊及遊戲控制器",
            Kind::Battery => "電池",
            Kind::Bluetooth => "藍牙",
            Kind::Camera => "相機",
            Kind::Computer => "電腦",
            Kind::Disk => "磁碟機",
            Kind::Display => "顯示卡",
            Kind::Optical => "DVD/CD-ROM 光碟機",
            Kind::Hid => "人性化介面裝置",
            Kind::Keyboard => "鍵盤",
            Kind::Mouse => "滑鼠及其他指標裝置",
            Kind::Monitor => "監視器",
            Kind::Network => "網路介面卡",
            Kind::Other => "其他裝置",
            Kind::Processor => "處理器",
            Kind::Security => "安全性裝置",
            Kind::Sensor => "感應器",
            Kind::Storage => "存放控制器",
            Kind::System => "系統裝置",
            Kind::UsbController => "通用序列匯流排控制器",
            Kind::Usb => "通用序列匯流排裝置",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Audio => "audio-card-symbolic",
            Kind::Battery => "battery-symbolic",
            Kind::Bluetooth => "bluetooth-symbolic",
            Kind::Camera => "camera-web-symbolic",
            Kind::Computer => "computer-symbolic",
            Kind::Disk => "drive-harddisk-symbolic",
            Kind::Display => "video-display-symbolic",
            Kind::Optical => "drive-optical-symbolic",
            Kind::Hid => "input-gaming-symbolic",
            Kind::Keyboard => "input-keyboard-symbolic",
            Kind::Mouse => "input-mouse-symbolic",
            Kind::Monitor => "video-single-display-symbolic",
            Kind::Network => "network-wired-symbolic",
            Kind::Other => "dialog-question-symbolic",
            Kind::Processor => "emblem-system-symbolic",
            Kind::Security => "auth-smartcard-symbolic",
            Kind::Sensor => "preferences-system-details-symbolic",
            Kind::Storage => "drive-multidisk-symbolic",
            Kind::System => "preferences-system-devices-symbolic",
            Kind::UsbController => "media-removable-symbolic",
            Kind::Usb => "drive-removable-media-symbolic",
        }
    }

    /// A device of this kind without a driver does not work; a bridge or a processor needs
    /// none.
    fn needs_driver(self) -> bool {
        !matches!(
            self,
            Kind::Battery
                | Kind::Computer
                | Kind::Monitor
                | Kind::Processor
                | Kind::System
                | Kind::Security
                | Kind::Sensor
        )
    }

    /// Whether a device of this kind can be disabled or uninstalled here: not what the
    /// computer itself is made of.
    pub fn can_change(self) -> bool {
        !matches!(
            self,
            Kind::Battery | Kind::Computer | Kind::Monitor | Kind::Processor | Kind::System
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Working,
    Disabled,
    NoDriver,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Working => "運作正常",
            State::Disabled => "已停用",
            State::NoDriver => "沒有驅動程式",
        }
    }
}

/// One device, as `udevadm info --export-db` tells of it.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Record {
    /// Its path under /sys, as `/devices/pci0000:00/0000:00:14.0`.
    pub path: String,
    pub props: Vec<(String, String)>,
    /// The other names of its device node.
    pub links: Vec<String>,
}

impl Record {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn subsystem(&self) -> &str {
        self.get("SUBSYSTEM").unwrap_or_default()
    }

    fn devtype(&self) -> &str {
        self.get("DEVTYPE").unwrap_or_default()
    }

    fn sysname(&self) -> &str {
        sysname(&self.path)
    }
}

pub fn sysname(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> Option<&str> {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .filter(|parent| parent.matches('/').count() >= 2)
}

/// Each device of the database, in its order.
pub fn parse(db: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut record = Record::default();
    for line in db.lines().chain([""]) {
        if line.is_empty() {
            if !record.path.is_empty() {
                records.push(std::mem::take(&mut record));
            }
            record = Record::default();
            continue;
        }
        let Some((tag, value)) = line.split_once(": ") else {
            continue;
        };
        match tag {
            "P" => record.path = value.to_string(),
            "S" => record.links.push(format!("/dev/{value}")),
            "E" => {
                if let Some((key, value)) = value.split_once('=') {
                    record.props.push((key.to_string(), value.to_string()));
                }
            }
            _ => {}
        }
    }
    records
}

/// What udev's `_ENC` values escape, as `\x20` for a space, written out.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\' && bytes.get(at + 1) == Some(&b'x') {
            if let Some(byte) = text
                .get(at + 2..at + 4)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(byte);
                at += 4;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A device, as the device manager shows it.
#[derive(Clone, PartialEq, Debug)]
pub struct Device {
    pub path: String,
    pub kind: Kind,
    pub name: String,
    pub vendor: Option<String>,
    /// The device on a bus that is disabled, uninstalled and has the driver: the device itself,
    /// or for what a driver makes, as a network interface, the one it is made of. Empty for
    /// what is made of nothing, as a virtual interface.
    pub node: String,
    pub bus: String,
    /// A USB device, which is turned off rather than let go of by its driver.
    pub usb_device: bool,
    pub driver: Option<String>,
    pub state: State,
    pub hidden: bool,
    pub props: Vec<(String, String)>,
    pub links: Vec<String>,
    pub children: Vec<String>,
    /// The ACPI device the firmware describes it, or what it is on, as, which places a PCI
    /// bus or a processor in the tree as Windows places them.
    pub firmware: Option<String>,
}

impl Device {
    pub fn node_name(&self) -> &str {
        sysname(&self.node)
    }

    pub fn prop(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn attr(&self, name: &str) -> Option<String> {
        read_attr(&self.node, name)
    }
}

fn read_attr(path: &str, name: &str) -> Option<String> {
    std::fs::read_to_string(format!("{SYS}{path}/{name}"))
        .ok()
        .map(|text| text.trim().to_string())
}

/// What is read from sysfs and elsewhere besides the database, so that sorting can be tested.
pub struct Machine {
    pub buses: HashSet<String>,
    pub attr: Box<dyn Fn(&str, &str) -> Option<String>>,
    /// The name of a connected display, from the folder of its connector.
    pub monitor: Box<dyn Fn(&str) -> String>,
    pub cpu: String,
    /// Where a device's `firmware_node` leads.
    pub firmware: Box<dyn Fn(&str) -> Option<String>>,
    /// What disabling here has kept disabled, by bus and name.
    pub disabled: HashSet<(String, String)>,
}

impl Machine {
    fn read() -> Machine {
        let buses = std::fs::read_dir(format!("{SYS}/bus"))
            .map(|dir| {
                dir.flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|info| {
                info.lines().find_map(|line| {
                    line.strip_prefix("model name")
                        .and_then(|rest| rest.split_once(':'))
                        .map(|(_, name)| name.trim().to_string())
                })
            })
            .unwrap_or_else(|| "處理器".into());
        Machine {
            buses,
            attr: Box::new(read_attr),
            monitor: Box::new(|path| {
                crate::edid::identity(&Path::new(SYS).join(path.trim_start_matches('/'))).0
            }),
            cpu,
            firmware: Box::new(|path| {
                let target = std::fs::canonicalize(format!("{SYS}{path}/firmware_node")).ok()?;
                target.to_str()?.strip_prefix(SYS).map(str::to_string)
            }),
            disabled: disabled(&std::fs::read_to_string(RULES).unwrap_or_default()),
        }
    }
}

/// The devices of this computer.
pub fn read() -> Result<Vec<Device>, String> {
    let out = Command::new("udevadm")
        .args(["info", "--export-db"])
        .output()
        .map_err(|err| format!("udevadm: {err}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(devices(
        &parse(&String::from_utf8_lossy(&out.stdout)),
        &Machine::read(),
    ))
}

/// What the rules written here keep disabled.
pub fn disabled(rules: &str) -> HashSet<(String, String)> {
    let field = |line: &str, key: &str| {
        let start = line.find(&format!("{key}==\""))? + key.len() + 3;
        line[start..].split('"').next().map(str::to_string)
    };
    rules
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| Some((field(line, "SUBSYSTEM")?, field(line, "KERNEL")?)))
        .collect()
}

/// The start of a rule of `RULES`, the same for both kinds of rule, by which it is found again.
pub fn rule_key(bus: &str, name: &str) -> String {
    format!("SUBSYSTEM==\"{bus}\", KERNEL==\"{name}\",")
}

/// The rule that keeps the device disabled: a USB device not let in, anything else let go of
/// by its driver whenever one takes it.
pub fn rule(bus: &str, name: &str, usb_device: bool) -> String {
    let key = rule_key(bus, name);
    if usb_device {
        format!("{key} ACTION==\"add\", ATTR{{authorized}}=\"0\"")
    } else {
        format!("{key} ACTION==\"bind\", RUN+=\"/bin/sh -c 'echo $kernel > /sys$devpath/driver/unbind'\"")
    }
}

/// `PCI_CLASS`, as `30000`, into the kind of device.
fn pci_kind(class: u32) -> Kind {
    match (class >> 16, (class >> 8) & 0xff) {
        (0x00, _) | (0xff, _) => Kind::Other,
        (0x01, _) => Kind::Storage,
        (0x02, _) => Kind::Network,
        (0x03, _) => Kind::Display,
        (0x04, _) => Kind::Audio,
        (0x09, _) => Kind::Hid,
        (0x0c, 0x03) => Kind::UsbController,
        (0x0d, 0x11) => Kind::Bluetooth,
        (0x0d, _) => Kind::Network,
        (0x10, _) => Kind::Security,
        _ => Kind::System,
    }
}

/// The ACPI devices Windows lists among its system devices, by their hardware ID.
fn acpi_name(id: &str) -> Option<&'static str> {
    Some(match id {
        "PNP0000" => "可程式化中斷控制器",
        "PNP0100" => "系統計時器",
        "PNP0103" => "高精確度事件計時器",
        "PNP0200" => "直接記憶體存取控制器",
        "PNP0800" => "系統喇叭",
        "PNP0A03" => "PCI 匯流排",
        "PNP0A08" => "PCI Express 根複合體",
        "PNP0B00" => "系統 CMOS/即時時鐘",
        "PNP0C01" => "系統板",
        "PNP0C02" => "主機板資源",
        "PNP0C04" => "數值資料處理器",
        "PNP0C09" => "嵌入式控制器",
        "PNP0C0C" => "電源按鈕",
        "PNP0C0D" => "蓋子開關",
        "PNP0C0E" => "睡眠按鈕",
        "PNP0C14" => "WMI 介面",
        "ACPI000E" => "喚醒計時器",
        "INT33A1" => "電源引擎外掛程式",
        "LNXVIDEO" => "ACPI 視訊匯流排",
        "LNXSYSTM" => "ACPI 系統",
        _ => return None,
    })
}

/// What a USB device is called and who made it.
fn usb_name(record: &Record) -> (String, Option<String>) {
    let name = record
        .get("ID_MODEL_FROM_DATABASE")
        .map(str::to_string)
        .or_else(|| record.get("ID_MODEL_ENC").map(decode))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("USB 裝置 {}", record.sysname()));
    let vendor = record
        .get("ID_VENDOR_FROM_DATABASE")
        .map(str::to_string)
        .or_else(|| record.get("ID_VENDOR_ENC").map(decode));
    (name, vendor)
}

/// A device's kind, whether it is hidden until asked for, its name and its maker; None for
/// what is not shown at all.
fn classify(record: &Record, machine: &Machine) -> Option<(Kind, bool, String, Option<String>)> {
    let get = |key: &str| record.get(key).map(str::to_string);
    let attr = |name: &str| (machine.attr)(&record.path, name);
    let from_database = || (get("ID_MODEL_FROM_DATABASE"), get("ID_VENDOR_FROM_DATABASE"));
    let is_virtual = record.path.starts_with("/devices/virtual/");
    Some(match (record.subsystem(), record.devtype()) {
        ("pci", _) => {
            let class = u32::from_str_radix(record.get("PCI_CLASS")?, 16).ok()?;
            let name = get("ID_MODEL_FROM_DATABASE")
                .or_else(|| get("ID_PCI_SUBCLASS_FROM_DATABASE"))
                .unwrap_or_else(|| format!("PCI 裝置 {}", record.get("PCI_ID").unwrap_or_default()));
            (pci_kind(class), false, name, get("ID_VENDOR_FROM_DATABASE"))
        }
        ("usb", "usb_device") => {
            let hub = record
                .get("TYPE")
                .is_some_and(|kind| kind.split('/').next() == Some(USB_HUB_CLASS));
            if hub && record.get("ID_VENDOR_ID") == Some(LINUX_FOUNDATION) {
                let version = attr("version")
                    .and_then(|version| version.trim().parse::<f64>().ok())
                    .map(|version| format!("{version:.1}"))
                    .unwrap_or_default();
                (
                    Kind::UsbController,
                    false,
                    format!("USB {version} 根集線器"),
                    None,
                )
            } else if hub {
                let (name, vendor) = usb_name(record);
                (
                    Kind::UsbController,
                    false,
                    if record.get("ID_MODEL_FROM_DATABASE").is_some() {
                        name
                    } else {
                        "USB 集線器".into()
                    },
                    vendor,
                )
            } else {
                let (name, vendor) = usb_name(record);
                (Kind::Usb, false, name, vendor)
            }
        }
        ("input", _) if record.sysname().starts_with("input") => {
            let name = get("NAME")?.trim_matches('"').to_string();
            let on = |key: &str| record.get(key) == Some("1");
            let vendor = get("ID_VENDOR_ENC").map(|vendor| decode(&vendor));
            // What a program makes through uinput is software, hidden as Windows hides it.
            if on("ID_INPUT_KEYBOARD") {
                (Kind::Keyboard, is_virtual, name, vendor)
            } else if [
                "ID_INPUT_MOUSE",
                "ID_INPUT_TOUCHPAD",
                "ID_INPUT_POINTINGSTICK",
                "ID_INPUT_TABLET",
                "ID_INPUT_TOUCHSCREEN",
            ]
            .iter()
            .any(|key| on(key))
            {
                (Kind::Mouse, is_virtual, name, vendor)
            } else {
                (Kind::Hid, !on("ID_INPUT_JOYSTICK"), name, vendor)
            }
        }
        ("hid", _) => (
            Kind::Hid,
            false,
            get("HID_NAME")
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "HID 裝置".into()),
            None,
        ),
        ("block", "disk") => {
            if is_virtual && attr("size").is_none_or(|size| size == "0") {
                return None;
            }
            let name = get("ID_MODEL_ENC")
                .map(|model| decode(&model))
                .or_else(|| get("ID_MODEL").map(|model| model.replace('_', " ")))
                .or_else(|| (machine.attr)(&record.path, "device/model"))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| record.sysname().to_string());
            let kind = if record.get("ID_CDROM") == Some("1") {
                Kind::Optical
            } else {
                Kind::Disk
            };
            (
                kind,
                is_virtual,
                name,
                get("ID_VENDOR_ENC")
                    .map(|vendor| decode(&vendor))
                    .filter(|vendor| !vendor.is_empty()),
            )
        }
        ("net", _) => {
            let (model, vendor) = from_database();
            (
                Kind::Network,
                is_virtual,
                model.unwrap_or_else(|| record.get("INTERFACE").unwrap_or(record.sysname()).to_string()),
                vendor,
            )
        }
        ("bluetooth", "host") => (
            Kind::Bluetooth,
            false,
            format!("藍牙介面卡 {}", record.sysname()),
            None,
        ),
        ("video4linux", _) => {
            if !record
                .get("ID_V4L_CAPABILITIES")
                .is_some_and(|caps| caps.contains(":capture:"))
                || attr("index").as_deref() != Some("0")
            {
                return None;
            }
            (
                Kind::Camera,
                false,
                get("ID_V4L_PRODUCT")
                    .or_else(|| attr("name"))
                    .unwrap_or_else(|| record.sysname().into()),
                get("ID_VENDOR_FROM_DATABASE"),
            )
        }
        ("sound", _) if record.sysname().starts_with("card") => {
            let (model, vendor) = from_database();
            (
                Kind::Audio,
                false,
                model
                    .or_else(|| attr("id"))
                    .unwrap_or_else(|| record.sysname().into()),
                vendor,
            )
        }
        ("drm", "drm_connector") => {
            if attr("status").as_deref() != Some("connected") {
                return None;
            }
            (Kind::Monitor, false, (machine.monitor)(&record.path), None)
        }
        ("cpu", _) => (Kind::Processor, false, machine.cpu.clone(), None),
        ("power_supply", _) => match record.get("POWER_SUPPLY_TYPE") {
            Some("Battery") if record.get("POWER_SUPPLY_SCOPE") != Some("Device") => (
                Kind::Battery,
                false,
                get("POWER_SUPPLY_MODEL_NAME")
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "電池".into()),
                get("POWER_SUPPLY_MANUFACTURER"),
            ),
            Some("Mains") => (Kind::Battery, false, "AC 變壓器".into(), None),
            _ => (
                Kind::Battery,
                true,
                get("POWER_SUPPLY_MODEL_NAME").unwrap_or_else(|| record.sysname().into()),
                get("POWER_SUPPLY_MANUFACTURER"),
            ),
        },
        ("tpm", _) => {
            let version = attr("tpm_version_major")
                .map(|major| format!(" {major}.0"))
                .unwrap_or_default();
            (Kind::Security, false, format!("信賴平台模組{version}"), None)
        }
        ("iio", _) => (
            Kind::Sensor,
            false,
            attr("name").unwrap_or_else(|| record.sysname().into()),
            None,
        ),
        ("dmi", _) => {
            let product = attr("product_name").filter(|name| {
                !name.is_empty() && !name.contains("System Product Name") && !name.contains("To Be Filled")
            });
            let name = product
                .or_else(|| attr("board_name"))
                .unwrap_or_else(|| "電腦".into());
            (
                Kind::Computer,
                false,
                name,
                attr("sys_vendor").or_else(|| attr("board_vendor")),
            )
        }
        ("acpi", _) => {
            if attr("status").as_deref() == Some("0") {
                return None;
            }
            let id = record.sysname().split(':').next().unwrap_or_default();
            match acpi_name(id) {
                Some(name) => (Kind::System, false, name.into(), get("ID_VENDOR_FROM_DATABASE")),
                None => (
                    Kind::System,
                    true,
                    record.sysname().into(),
                    get("ID_VENDOR_FROM_DATABASE"),
                ),
            }
        }
        ("platform", _) => (Kind::System, true, record.sysname().into(), None),
        _ => return None,
    })
}

/// Each device to show, sorted by kind and name.
pub fn devices(records: &[Record], machine: &Machine) -> Vec<Device> {
    let by_path: HashMap<&str, &Record> = records
        .iter()
        .map(|record| (record.path.as_str(), record))
        .collect();
    let mut children: HashMap<&str, Vec<&Record>> = HashMap::new();
    for record in records {
        if let Some(parent) = parent(&record.path) {
            children.entry(parent).or_default().push(record);
        }
    }
    // The device on a bus nearest to, or at, a path.
    let node_of = |path: &str| -> Option<&Record> {
        let mut at = Some(path);
        while let Some(path) = at {
            if let Some(record) = by_path.get(path) {
                if machine.buses.contains(record.subsystem()) {
                    return Some(record);
                }
            }
            at = parent(path);
        }
        None
    };
    let mut shown: Vec<Device> = Vec::new();
    for record in records {
        let Some((kind, hidden, name, vendor)) = classify(record, machine) else {
            continue;
        };
        let node = node_of(&record.path);
        let bus = node.map(|node| node.subsystem().to_string()).unwrap_or_default();
        let node_path = node.map(|node| node.path.clone()).unwrap_or_default();
        let usb_device = node.is_some_and(|node| node.subsystem() == "usb" && node.devtype() == "usb_device");
        let interfaces: Vec<&Record> = if usb_device {
            children
                .get(node_path.as_str())
                .map(|list| {
                    list.iter()
                        .copied()
                        .filter(|child| child.devtype() == "usb_interface")
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let driver = if usb_device {
            let mut drivers: Vec<&str> = Vec::new();
            for driver in interfaces.iter().filter_map(|interface| interface.get("DRIVER")) {
                if !drivers.contains(&driver) {
                    drivers.push(driver);
                }
            }
            (!drivers.is_empty()).then(|| drivers.join("、"))
        } else {
            node.and_then(|node| node.get("DRIVER")).map(str::to_string)
        };
        let off = usb_device && (machine.attr)(&node_path, "authorized").as_deref() == Some("0");
        let state = if off
            || machine
                .disabled
                .contains(&(bus.clone(), sysname(&node_path).to_string()))
        {
            State::Disabled
        // A USB device a program talks to itself, as a phone or a radio, needs no driver.
        } else if node.is_some() && !usb_device && driver.is_none() && kind.needs_driver() {
            State::NoDriver
        } else {
            State::Working
        };
        shown.push(Device {
            path: record.path.clone(),
            kind,
            name,
            vendor: vendor.filter(|vendor| !vendor.is_empty()),
            node: node_path,
            bus,
            usb_device,
            driver,
            state,
            hidden,
            props: record.props.clone(),
            links: record.links.clone(),
            children: children
                .get(record.path.as_str())
                .map(|list| list.iter().map(|child| child.path.clone()).collect())
                .unwrap_or_default(),
            firmware: std::iter::successors(Some(record.path.as_str()), |path| parent(path))
                .find_map(|path| (machine.firmware)(path)),
        });
    }
    // What a driver makes of a device listed under the same kind is that device again, as
    // a PCI network card's interface.
    let listed: HashSet<(String, Kind)> = shown
        .iter()
        .map(|device| (device.path.clone(), device.kind))
        .collect();
    shown
        .retain(|device| device.node == device.path || !listed.contains(&(device.node.clone(), device.kind)));
    let order = |kind: Kind| {
        KINDS
            .iter()
            .position(|other| *other == kind)
            .unwrap_or(KINDS.len())
    };
    // The shorter path first among the same name puts cpu2 before cpu10.
    shown.sort_by(|a, b| {
        (order(a.kind), &a.name, a.path.len(), &a.path).cmp(&(order(b.kind), &b.name, b.path.len(), &b.path))
    });
    shown
}

/// The devices as what each is connected to: each one's place in `devices`, how deep it is and
/// whether anything is under it, in order, but for what is under one not `expanded`. What is
/// connected to nothing shown is under the ACPI device the firmware places it on, else under
/// the computer, as in Windows.
/// What each device is connected to, by its index: the nearest device above it in sysfs, else
/// the one its firmware node is under, else the computer, which is under none.
pub fn parents(devices: &[&Device]) -> Vec<Option<usize>> {
    let at: HashMap<&str, usize> = devices
        .iter()
        .enumerate()
        .map(|(index, device)| (device.path.as_str(), index))
        .collect();
    let computer = devices.iter().position(|device| device.kind == Kind::Computer);
    let nearest = |from: Option<&str>| {
        std::iter::successors(from, |path| parent(path)).find_map(|path| at.get(path).copied())
    };
    devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            let firmware = || {
                nearest(device.firmware.as_deref())
                    .filter(|&above| !devices[above].path.starts_with(&device.path))
            };
            if Some(index) == computer {
                None
            } else {
                nearest(parent(&device.path)).or_else(firmware).or(computer)
            }
        })
        .collect()
}

pub fn tree(devices: &[&Device], expanded: &HashSet<String>) -> Vec<(usize, usize, bool)> {
    let mut under: Vec<Vec<usize>> = vec![Vec::new(); devices.len()];
    let mut tops = Vec::new();
    for (index, above) in parents(devices).into_iter().enumerate() {
        match above {
            Some(parent) => under[parent].push(index),
            None => tops.push(index),
        }
    }
    let mut rows = Vec::new();
    let mut stack: Vec<(usize, usize)> = tops.into_iter().rev().map(|top| (top, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        rows.push((index, depth, !under[index].is_empty()));
        if expanded.contains(&devices[index].path) {
            stack.extend(under[index].iter().rev().map(|&child| (child, depth + 1)));
        }
    }
    rows
}

/// The paths of the drive the system runs from and of what it is made of, which are not to be
/// disabled from under it.
pub fn system_paths() -> Vec<String> {
    let run = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let Some(source) = run("findmnt", &["-nvo", "SOURCE", "/"]) else {
        return Vec::new();
    };
    let Some(names) = run("lsblk", &["-nsro", "KNAME", source.trim()]) else {
        return Vec::new();
    };
    names
        .lines()
        .filter_map(|name| std::fs::canonicalize(format!("{SYS}/class/block/{}", name.trim())).ok())
        .filter_map(|path| {
            path.to_str()
                .and_then(|path| path.strip_prefix(SYS))
                .map(str::to_string)
        })
        .collect()
}

/// Whether disabling or removing `device` would take away what the system runs from.
pub fn holds_system(device: &Device, system: &[String]) -> bool {
    !device.node.is_empty()
        && system
            .iter()
            .any(|path| path == &device.node || path.starts_with(&format!("{}/", device.node)))
}

/// Where a device is, as Windows' 一般 says it.
pub fn location(device: &Device) -> String {
    let name = device.node_name();
    match device.bus.as_str() {
        "pci" => {
            // domain:bus:slot.function, in hexadecimal; Windows says them in decimal.
            let hex = |text: &str| u32::from_str_radix(text, 16).ok();
            let mut fields = name.split(':').skip(1);
            let (bus, place) = (
                fields.next().and_then(hex),
                fields.next().and_then(|place| place.split_once('.')),
            );
            match (
                bus,
                place.and_then(|(slot, function)| Some((hex(slot)?, hex(function)?))),
            ) {
                (Some(bus), Some((slot, function))) => {
                    format!("PCI 匯流排 {bus}，裝置 {slot}，功能 {function}")
                }
                _ => name.to_string(),
            }
        }
        "usb" => format!("USB 連接埠 {}", name.split(':').next().unwrap_or(name)),
        "" => "虛擬".into(),
        bus => format!("{bus} {name}"),
    }
}

/// What a device's 一般 states besides its maker, its place and how it is.
pub fn facts(device: &Device) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    let own = |name: &str| read_attr(&device.path, name);
    if device.usb_device {
        if let Some(speed) = device.attr("speed") {
            facts.push(("速度", format!("{speed} Mb/s")));
        }
        if let Some(power) = device.attr("bMaxPower").filter(|power| power != "0mA") {
            facts.push(("最大電流", power.replace("mA", " mA")));
        }
    }
    if let Some(interface) = device.prop("INTERFACE") {
        facts.push(("介面", interface.to_string()));
        if let Some(address) = own("address") {
            facts.push(("實體位址", address));
        }
    }
    if device.kind == Kind::Disk || device.kind == Kind::Optical {
        if let Some(sectors) = own("size")
            .and_then(|size| size.parse::<u64>().ok())
            .filter(|size| *size > 0)
        {
            facts.push(("容量", gtk::glib::format_size(sectors * 512).to_string()));
        }
    }
    if device.kind == Kind::Monitor {
        facts.push((
            "連接埠",
            sysname(&device.path)
                .split_once('-')
                .map_or("", |(_, port)| port)
                .to_string(),
        ));
    }
    if let Some(node) = device.prop("DEVNAME") {
        facts.push(("裝置節點", node.to_string()));
    }
    facts
}

/// What a device's 詳細資料 lists: each property and its values.
pub fn details(device: &Device) -> Vec<(String, Vec<String>)> {
    let mut list: Vec<(String, Vec<String>)> = Vec::new();
    let mut add = |name: &str, values: Vec<String>| {
        let values: Vec<String> = values.into_iter().filter(|value| !value.is_empty()).collect();
        if !values.is_empty() {
            list.push((name.to_string(), values));
        }
    };
    add("裝置描述", vec![device.name.clone()]);
    add("類別", vec![device.kind.name().into()]);
    add("製造商", device.vendor.iter().cloned().collect());
    let ids = match (
        device.prop("PCI_ID"),
        device.prop("ID_VENDOR_ID"),
        device.prop("ID_MODEL_ID"),
    ) {
        (Some(id), _, _) => vec![id.to_lowercase()],
        (None, Some(vendor), Some(model)) => vec![format!("{vendor}:{model}")],
        _ => Vec::new(),
    };
    add("硬體識別碼", ids);
    add(
        "子系統識別碼",
        device
            .prop("PCI_SUBSYS_ID")
            .map(str::to_lowercase)
            .into_iter()
            .collect(),
    );
    add(
        "修訂",
        device
            .prop("ID_REVISION")
            .map(str::to_string)
            .into_iter()
            .collect(),
    );
    add(
        "相容識別碼",
        device.prop("MODALIAS").map(str::to_string).into_iter().collect(),
    );
    add("裝置執行個體路徑", vec![device.path.clone()]);
    if device.node != device.path {
        add("所屬裝置", vec![device.node.clone()]);
    }
    add("匯流排", vec![device.bus.clone()]);
    add(
        "位置路徑",
        device.prop("ID_PATH").map(str::to_string).into_iter().collect(),
    );
    add(
        "父系",
        parent(&device.path).map(str::to_string).into_iter().collect(),
    );
    add("子系", device.children.clone());
    add("驅動程式", device.driver.iter().cloned().collect());
    let mut nodes: Vec<String> = device.prop("DEVNAME").map(str::to_string).into_iter().collect();
    nodes.extend(device.links.iter().cloned());
    add("裝置節點", nodes);
    for (key, value) in &device.props {
        add(key, vec![value.clone()]);
    }
    list
}

/// What `modinfo` tells of a device's driver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriverInfo {
    pub module: Option<String>,
    pub fields: Vec<(&'static str, String)>,
}

/// The driver of the device at `node`, from its module.
pub fn driver_info(node: &str, driver: &str) -> DriverInfo {
    let module = std::fs::read_link(format!("{SYS}{node}/driver/module"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()));
    let asked = module.clone().unwrap_or_else(|| driver.replace('-', "_"));
    let said = Command::new("modinfo")
        .arg(&asked)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let mut info = DriverInfo {
        module,
        fields: Vec::new(),
    };
    let first = |key: &str| {
        said.lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .map(|value| value.trim().to_string())
            })
            .filter(|value| !value.is_empty())
    };
    let kernel = first("vermagic")
        .and_then(|magic| magic.split_whitespace().next().map(str::to_string))
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|release| release.trim().to_string())
        });
    for (name, value) in [
        ("描述", first("description")),
        ("版本", first("version").or(kernel)),
        ("提供者", first("author")),
        ("授權", first("license")),
        ("檔案", first("filename").filter(|file| file != "(builtin)")),
    ] {
        if let Some(value) = value {
            info.fields.push((name, value));
        }
    }
    info
}

/// The interrupts and the ranges of memory and I/O ports a PCI device uses.
pub fn resources(node: &str) -> Vec<(&'static str, String)> {
    let mut list = Vec::new();
    let mut irqs: Vec<u32> = std::fs::read_dir(format!("{SYS}{node}/msi_irqs"))
        .map(|dir| {
            dir.flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    if irqs.is_empty() {
        irqs.extend(
            read_attr(node, "irq")
                .and_then(|irq| irq.parse::<u32>().ok())
                .filter(|irq| *irq != 0),
        );
    }
    irqs.sort_unstable();
    list.extend(irqs.into_iter().map(|irq| ("IRQ", irq.to_string())));
    list.extend(ranges(&read_attr(node, "resource").unwrap_or_default()));
    list
}

/// The ranges of a PCI device's `resource`: a start, an end and flags to a line.
fn ranges(resource: &str) -> Vec<(&'static str, String)> {
    const IO: u64 = 0x100;
    const MEMORY: u64 = 0x200;
    resource
        .lines()
        .filter_map(|line| {
            let mut fields = line
                .split_whitespace()
                .map(|field| u64::from_str_radix(field.trim_start_matches("0x"), 16).ok());
            let (start, end, flags) = (fields.next()??, fields.next()??, fields.next()??);
            if start == 0 && end == 0 {
                return None;
            }
            let kind = if flags & IO != 0 {
                "I/O 範圍"
            } else if flags & MEMORY != 0 {
                "記憶體範圍"
            } else {
                return None;
            };
            Some((kind, format!("{start:#X}–{end:#X}")))
        })
        .collect()
}

/// A device's power settings: whether the computer may turn it off to save power, and whether
/// it may wake the computer, each where it has the setting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Power {
    pub suspend: Option<bool>,
    pub wakeup: Option<bool>,
}

pub fn power(node: &str) -> Power {
    Power {
        suspend: read_attr(node, "power/control").map(|control| control == "auto"),
        wakeup: read_attr(node, "power/wakeup")
            .filter(|wakeup| !wakeup.is_empty())
            .map(|wakeup| wakeup == "enabled"),
    }
}

pub fn power_file(node: &str, name: &str) -> PathBuf {
    PathBuf::from(format!("{SYS}{node}/power/{name}"))
}

/// What the kernel said of a device since the computer started, the newest first.
pub fn events(device: &Device) -> Result<Vec<(String, String)>, String> {
    let out = Command::new("journalctl")
        .args(["-k", "-b", "-q", "--no-pager", "-o", "short-iso"])
        .env("LC_ALL", "C")
        .output()
        .map_err(|err| format!("journalctl: {err}"))?;
    let log = String::from_utf8_lossy(&out.stdout);
    let said = String::from_utf8_lossy(&out.stderr);
    // Without the right to read the journal it says so and prints nothing.
    if log.trim().is_empty() && !said.trim().is_empty() {
        return Err(said.trim().to_string());
    }
    let mut names = vec![device.node_name().to_string()];
    for name in [
        sysname(&device.path).to_string(),
        device.prop("INTERFACE").unwrap_or_default().to_string(),
    ] {
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names.retain(|name| !name.is_empty());
    Ok(log
        .lines()
        .filter_map(|line| {
            let (time, rest) = line.split_once(' ')?;
            let message = rest.split_once(" kernel: ").map_or(rest, |(_, message)| message);
            names.iter().any(|name| mentions(message, name)).then(|| {
                (
                    time.replacen('T', " ", 1).chars().take(19).collect(),
                    message.to_string(),
                )
            })
        })
        .rev()
        .take(EVENTS)
        .collect())
}

/// Whether `text` names `name` as a word, so that `1-8` is not found in `1-8.3`.
fn mentions(text: &str, name: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || matches!(c, '.' | '-' | '_');
    text.match_indices(name).any(|(at, _)| {
        text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !part(c) && c != ':')
            && text[at + name.len()..].chars().next().is_none_or(|c| !part(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "\
P: /devices/pci0000:00/0000:00:14.0
E: SUBSYSTEM=pci
E: DRIVER=xhci_hcd
E: PCI_CLASS=C0330
E: PCI_ID=8086:7AE0
E: ID_MODEL_FROM_DATABASE=Alder Lake-S PCH USB 3.2 Gen 2x2 XHCI Controller
E: ID_VENDOR_FROM_DATABASE=Intel Corporation

P: /devices/pci0000:00/0000:00:14.0/usb1
E: SUBSYSTEM=usb
E: DEVTYPE=usb_device
E: DRIVER=usb
E: TYPE=9/0/1
E: ID_VENDOR_ID=1d6b

P: /devices/pci0000:00/0000:00:14.0/usb1/1-8
E: SUBSYSTEM=usb
E: DEVTYPE=usb_device
E: DRIVER=usb
E: TYPE=0/0/0
E: ID_VENDOR_ID=046d
E: ID_MODEL_ENC=USB\\x20Receiver

P: /devices/pci0000:00/0000:00:14.0/usb1/1-8/1-8:1.0
E: SUBSYSTEM=usb
E: DEVTYPE=usb_interface
E: DRIVER=usbhid

P: /devices/pci0000:00/0000:00:14.0/usb1/1-9
E: SUBSYSTEM=usb
E: DEVTYPE=usb_device
E: DRIVER=usb
E: TYPE=255/0/0
E: ID_MODEL_ENC=Gadget

P: /devices/pci0000:00/0000:00:14.0/usb1/1-9/1-9:1.0
E: SUBSYSTEM=usb
E: DEVTYPE=usb_interface

P: /devices/pci0000:00/0000:00:14.0/usb1/1-8/1-8:1.0/0003:046D:C548.0001/input/input3
E: SUBSYSTEM=input
E: NAME=\"Logitech USB Receiver\"
E: ID_INPUT_KEYBOARD=1

P: /devices/pci0000:00/0000:00:14.0/usb1/1-8/1-8:1.0/0003:046D:C548.0001
E: SUBSYSTEM=hid
E: DRIVER=hid-generic
E: HID_NAME=Logitech USB Receiver

P: /devices/pci0000:00/0000:00:1f.6
E: SUBSYSTEM=pci
E: PCI_CLASS=20000
E: ID_MODEL_FROM_DATABASE=Ethernet Connection

P: /devices/pci0000:00/0000:00:1f.6/net/eno1
E: SUBSYSTEM=net
E: INTERFACE=eno1
E: ID_MODEL_FROM_DATABASE=Ethernet Connection

P: /devices/LNXSYSTM:00/LNXSYBUS:00/PNP0A08:00
E: SUBSYSTEM=acpi

P: /devices/virtual/net/lo
E: SUBSYSTEM=net
E: INTERFACE=lo
S: never/shown
";

    fn machine(disabled: &[(&str, &str)]) -> Machine {
        Machine {
            buses: ["pci", "usb", "hid"].iter().map(|bus| bus.to_string()).collect(),
            attr: Box::new(|path, name| match name {
                "version" if path.ends_with("usb1") => Some(" 2.00".into()),
                "authorized" if path.ends_with("1-9") => Some("1".into()),
                _ => None,
            }),
            monitor: Box::new(|_| "顯示器".into()),
            cpu: "CPU".into(),
            firmware: Box::new(|path| {
                (path == "/devices/pci0000:00").then(|| "/devices/LNXSYSTM:00/LNXSYBUS:00/PNP0A08:00".into())
            }),
            disabled: disabled
                .iter()
                .map(|(bus, name)| (bus.to_string(), name.to_string()))
                .collect(),
        }
    }

    fn find<'a>(devices: &'a [Device], end: &str) -> &'a Device {
        devices
            .iter()
            .find(|device| device.path.ends_with(end))
            .unwrap_or_else(|| panic!("{end} not listed"))
    }

    #[test]
    fn sorts_devices_into_windows_kinds() {
        let records = parse(DB);
        assert_eq!(records.len(), 12);
        let devices = devices(&records, &machine(&[]));
        assert_eq!(find(&devices, "0000:00:14.0").kind, Kind::UsbController);
        assert_eq!(find(&devices, "usb1").name, "USB 2.0 根集線器");
        let receiver = find(&devices, "1-8");
        assert_eq!(
            (receiver.kind, receiver.name.as_str(), receiver.driver.as_deref()),
            (Kind::Usb, "USB Receiver", Some("usbhid"))
        );
        let keyboard = find(&devices, "input3");
        assert_eq!(
            (keyboard.kind, keyboard.name.as_str(), keyboard.bus.as_str()),
            (Kind::Keyboard, "Logitech USB Receiver", "hid")
        );
        assert_eq!(keyboard.node_name(), "0003:046D:C548.0001");
        assert_eq!(find(&devices, "0000:00:1f.6").state, State::NoDriver);
        assert!(
            devices.iter().all(|device| !device.path.ends_with("eno1")),
            "the card's own interface is the card"
        );
        let lo = find(&devices, "/lo");
        assert!(lo.hidden && lo.state == State::Working && lo.node.is_empty());
        assert_eq!(find(&devices, "1-9").state, State::Working);
        assert_eq!(devices.first().map(|device| device.kind), Some(Kind::Hid));
    }

    #[test]
    fn shows_what_each_device_is_connected_to() {
        let mut list = devices(&parse(DB), &machine(&[]));
        let mut computer = list[0].clone();
        computer.path = "/devices/virtual/dmi/id".into();
        computer.kind = Kind::Computer;
        list.push(computer);
        let shown: Vec<&Device> = list.iter().filter(|device| !device.hidden).collect();
        let name = |index: usize| sysname(&shown[index].path).to_string();
        let rows = |expanded: &HashSet<String>| {
            tree(&shown, expanded)
                .iter()
                .map(|&(index, depth, _)| (name(index), depth))
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&HashSet::new()), [("id".to_string(), 0)]);
        let computer: HashSet<String> = ["/devices/virtual/dmi/id".to_string()].into();
        assert_eq!(
            rows(&computer),
            [("id".to_string(), 0), ("PNP0A08:00".to_string(), 1)],
            "a PCI bus is on the ACPI device the firmware says"
        );
        let mut open: HashSet<String> = shown.iter().map(|device| device.path.clone()).collect();
        open.insert("/devices/virtual/dmi/id".into());
        let rows: Vec<(String, usize, bool)> = tree(&shown, &open)
            .into_iter()
            .map(|(index, depth, under)| (name(index), depth, under))
            .collect();
        let place = |end: &str| {
            rows.iter()
                .find(|(name, ..)| name == end)
                .map(|(_, depth, under)| (*depth, *under))
        };
        assert_eq!(place("id"), Some((0, true)));
        assert_eq!(place("0000:00:14.0"), Some((2, true)));
        assert_eq!(place("usb1"), Some((3, true)));
        assert_eq!(place("1-8"), Some((4, true)));
        assert_eq!(place("0003:046D:C548.0001"), Some((5, true)));
        assert_eq!(place("input3"), Some((6, false)));
        let usb1 = rows.iter().position(|(name, ..)| name == "usb1").unwrap();
        assert_eq!(
            (rows[usb1 + 1].0.as_str(), rows[usb1 + 1].1),
            ("1-9", 4),
            "a device's own come right after it, by name"
        );
    }

    #[test]
    fn what_is_disabled_here_stays_disabled() {
        let rules = format!(
            "# Nekoland\n{}\n{}\n",
            rule("pci", "0000:00:14.0", false),
            rule("usb", "1-8", true)
        );
        let disabled = disabled(&rules);
        assert!(disabled.contains(&("pci".into(), "0000:00:14.0".into())));
        assert!(disabled.contains(&("usb".into(), "1-8".into())));
        assert_eq!(disabled.len(), 2);
        let devices = devices(&parse(DB), &machine(&[("usb", "1-8")]));
        assert_eq!(find(&devices, "1-8").state, State::Disabled);
        assert!(rule("usb", "1-8", true).starts_with(&rule_key("usb", "1-8")));
        assert!(!rule("usb", "1-8.3", true).starts_with(&rule_key("usb", "1-8")));
    }

    #[test]
    fn reads_what_is_said_of_a_device() {
        assert_eq!(decode("USB\\x20Receiver\\x20\\x20"), "USB Receiver");
        assert!(mentions("usb 1-8: new full-speed USB device", "1-8"));
        assert!(!mentions("usb 1-8.3: new full-speed USB device", "1-8"));
        assert!(mentions("nvme 0000:01:00.0: enabling device", "0000:01:00.0"));
        assert!(mentions("e1000e: eno1 NIC Link is Up", "eno1"));
        assert_eq!(
            ranges("0x00000000a0000000 0x00000000a0ffffff 0x0000000000040200\n0x0000000000000000 0x0000000000000000 0x0000000000000000\n0x0000000000004000 0x000000000000407f 0x0000000000040101\n"),
            vec![("記憶體範圍", "0xA0000000–0xA0FFFFFF".to_string()), ("I/O 範圍", "0x4000–0x407F".to_string())]
        );
        let device = find(&devices(&parse(DB), &machine(&[])), "0000:00:14.0").clone();
        assert_eq!(location(&device), "PCI 匯流排 0，裝置 20，功能 0");
        assert!(holds_system(
            &device,
            &["/devices/pci0000:00/0000:00:14.0/usb1/1-8/host0/block/sda".into()]
        ));
        assert!(!holds_system(
            &device,
            &["/devices/pci0000:00/0000:00:14.1/nvme".into()]
        ));
    }
}
