//! The system's readings in the menu bar, after Stats on the Mac: the CPU, the GPU, the
//! memory, the disk, the network's speed and the sensors' temperatures, each as a number, a
//! line of its last readings or a bar, and each opening a panel in the control center's
//! glass with its history, what makes it up and the processes that use the most of it.
//!
//! A thread reads /proc and sysfs every `stats_interval` seconds while any is shown, so that
//! nothing is read on the compositor's thread. A new reading slides into the lines, the bars
//! grow to it, and a panel fades in, out, and into another's as the pointer moves along.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

use cairo::{Context, LinearGradient};
use smithay::{
    backend::renderer::element::Id,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    clockmenu::show_right,
    config::{self, Config, StatsStyle},
    control::{draw_chevron, draw_list_row, draw_panel, draw_slider, draw_well, ease_out, rect, show_line, to_f64, OPEN_TIME},
    fileinfo::{RED, YELLOW},
    glass::Tier,
    menubar::{colors, font, show_text, text_size, Overlay, GAP_BELOW_BAR, MENU_BAR_HEIGHT, MENU_RADIUS},
    text::{self, Picture},
    Nekoland,
};

#[allow(dead_code)]
#[path = "../ui/src/tasks/proc.rs"]
mod proc;

type Rgb = (f64, f64, f64);

/// The readings a panel's line holds, a minute of them at the fastest.
const HISTORY: usize = 60;
/// The readings the bar's line holds.
const BAR_HISTORY: usize = 24;
/// How long a new reading takes to slide in and the bars to grow to it.
const SETTLE: f64 = 0.4;
const MAX_PROCESSES: usize = 10;
/// Where a value turns yellow, and red, as in Stats.
const WARN: f64 = 0.8;
const ALERT: f64 = 0.9;
/// The least a line of speeds is scaled to, so that a quiet network is not drawn as a busy one.
const LEAST_SPEED: f64 = 64.0 * 1024.0;

const WIDTH: i32 = 320;
const PADDING: i32 = 12;
const GAP: i32 = 8;
const INSET: i32 = 4;
const TITLE_ROW: i32 = 26;
const DETAIL_ROW: i32 = 26;
const LIST_ROW: i32 = 30;
const HERO_CHART: i32 = 52;
const CORES_HEIGHT: i32 = 40;
/// The bar's line and bars, and the gap after a name stood on end beside them.
const BAR_CHART: (f64, f64) = (32.0, 18.0);
const BAR_LABEL_GAP: f64 = 3.0;
const STANDING_LINE: f64 = 7.0;
/// The most bars the CPU has in the bar, its cores grouped to fit.
const MOST_BARS: usize = 16;
const SETTINGS_PAGE: &str = "系統監控";
/// The room between the readings when they are one item, and a row of the panel listing them.
const COMBINED_GAP: i32 = 6;
const SUMMARY_ROW: i32 = 44;
const SUMMARY_CHART: (i32, i32) = (96, 24);
/// The ring's and the gauge's size in the bar, and the width of their arcs.
const RING: f64 = 18.0;
const GAUGE: (f64, f64) = (26.0, 15.0);
const ARC_WIDTH: f64 = 3.0;
/// Readings past a threshold in a row before it is told, as Stats waits for two.
const ALERT_AFTER: u32 = 2;
const ALERT_APP: &str = "系統監控";
const ALERT_ICON: &str = "utilities-system-monitor";
const TASKS_COMMAND: &str = "nekoland-tasks";

/// One of the readings the bar can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    Cpu,
    Gpu,
    Ram,
    Disk,
    Network,
    Sensors,
}

impl Module {
    /// Left to right, as Stats puts them.
    pub const ALL: [Module; 6] = [Module::Cpu, Module::Gpu, Module::Ram, Module::Disk, Module::Network, Module::Sensors];

    fn bit(self) -> u32 {
        1 << self as u32
    }

    pub fn enabled(self, config: &Config) -> bool {
        match self {
            Module::Cpu => config.stats_cpu,
            Module::Gpu => config.stats_gpu,
            Module::Ram => config.stats_ram,
            Module::Disk => config.stats_disk,
            Module::Network => config.stats_network,
            Module::Sensors => config.stats_sensors,
        }
    }

    /// What the order of the bar's icons calls it.
    pub fn order_name(self) -> &'static str {
        match self {
            Module::Cpu => "stats:cpu",
            Module::Gpu => "stats:gpu",
            Module::Ram => "stats:ram",
            Module::Disk => "stats:disk",
            Module::Network => "stats:network",
            Module::Sensors => "stats:sensors",
        }
    }

    /// Its name in the bar, as Stats writes it there.
    fn label(self) -> &'static str {
        match self {
            Module::Cpu => "CPU",
            Module::Gpu => "GPU",
            Module::Ram => "RAM",
            Module::Disk => "SSD",
            Module::Network => "NET",
            Module::Sensors => "TMP",
        }
    }

    /// What a notification calls its use.
    fn usage_name(self) -> &'static str {
        match self {
            Module::Cpu => "CPU 使用率",
            Module::Gpu => "GPU 使用率",
            Module::Ram => "記憶體使用量",
            Module::Disk => "磁碟使用量",
            Module::Network | Module::Sensors => "",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Module::Cpu => "CPU",
            Module::Gpu => "GPU",
            Module::Ram => "記憶體",
            Module::Disk => "磁碟",
            Module::Network => "網路",
            Module::Sensors => "感應器",
        }
    }
}

// What the reading thread is asked for, set from the compositor's settings.
static WANTED: AtomicU32 = AtomicU32::new(0);
static INTERVAL: AtomicU32 = AtomicU32::new(2);
static PROCESSES: AtomicU32 = AtomicU32::new(5);
static PUBLIC_IP: AtomicBool = AtomicBool::new(false);
/// How long the public address is kept before it is asked for again, while the interface stays.
const HWMON: &str = "/sys/class/hwmon";
/// The helper that sets a fan as root, through pkexec, and how slow the panel lets a fan be
/// set: never stopped, as Stats asks before it stops one.
const FAN_HELPER: &str = "/usr/local/libexec/nekoland-fan";
const LEAST_FAN: f64 = 0.2;
/// The slider's knob's radius, as the control center's.
const FAN_KNOB: i32 = 7;
/// The `pwm_enable` the chip drives a fan itself with, unless it had another.
const FAN_AUTOMATIC: u8 = 2;
const PUBLIC_IP_EVERY: Duration = Duration::from_secs(600);
const PUBLIC_IP_SERVICE: &str = "https://ip.mac-stats.com";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cpu {
    /// In use, by programs and by the kernel, from 0 to 1; each core's, and the load averages.
    pub usage: f64,
    pub user: f64,
    pub system: f64,
    pub cores: Vec<f64>,
    pub load: [f64; 3],
    pub uptime: u64,
    pub mhz: Option<f64>,
    pub temperature: Option<f64>,
}

/// A fan's speed in RPM, and, when it can be set, its `pwm` file, how fast it is driven from
/// 0 to 1, and its `pwm_enable`: 1 when set by hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fan {
    pub name: String,
    pub rpm: f64,
    pub pwm: Option<(PathBuf, f64, u8)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub usage: f64,
    /// Its memory in use and in all, in bytes; its temperature, and its power in watts.
    pub memory: Option<(f64, f64)>,
    pub temperature: Option<f64>,
    pub power: Option<f64>,
}

/// In bytes, and the pressure: how much of the last ten seconds something waited for memory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ram {
    pub total: f64,
    pub used: f64,
    pub cached: f64,
    pub swap: (f64, f64),
    pub pressure: Option<f64>,
}

/// The root filesystem's room in bytes, and the disks' reading and writing in bytes a second.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Disk {
    pub total: f64,
    pub free: f64,
    pub read: f64,
    pub write: f64,
}

/// The interface of the default route, in bytes a second and in all since it came up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Net {
    pub interface: String,
    pub received: f64,
    pub sent: f64,
    pub received_total: f64,
    pub sent_total: f64,
}

/// What one reading found; a module not asked for, or with nothing to read, is None.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub cpu: Option<Cpu>,
    pub gpu: Option<Gpu>,
    pub ram: Option<Ram>,
    pub disk: Option<Disk>,
    pub network: Option<Net>,
    /// Each chip's temperature in °C, and each fan's speed in RPM.
    pub temperatures: Vec<(String, f64)>,
    pub fans: Vec<Fan>,
    /// The busiest processes: by CPU, in percent of one core, and by memory, in bytes; by
    /// disk, read and written, and by network, in and out, in bytes a second.
    pub top_cpu: Vec<(String, f64)>,
    pub top_ram: Vec<(String, f64)>,
    pub top_disk: Vec<(String, f64)>,
    pub top_network: Vec<(String, f64, f64)>,
    /// The GPUs besides the one shown, as Intel's beside a discrete one.
    pub other_gpus: Vec<Gpu>,
    /// The address the internet sees, with its country's code, when asked for.
    pub public_ip: Option<(String, String)>,
}

impl Sample {
    /// The module's value from 0 to 1, as its bar and its number show it.
    fn fraction(&self, module: Module) -> Option<f64> {
        match module {
            Module::Cpu => self.cpu.as_ref().map(|cpu| cpu.usage),
            Module::Gpu => self.gpu.as_ref().map(|gpu| gpu.usage),
            Module::Ram => self.ram.as_ref().map(|ram| ram.used / ram.total.max(1.0)),
            Module::Disk => self.disk.as_ref().map(|disk| 1.0 - disk.free / disk.total.max(1.0)),
            Module::Sensors => self.hottest().map(|(_, celsius)| celsius / 100.0),
            Module::Network => None,
        }
        .map(|value| value.clamp(0.0, 1.0))
    }

    /// The two speeds a module has, in bytes a second: the network's in and out, the disk's
    /// reading and writing.
    fn speeds(&self, module: Module) -> Option<(f64, f64)> {
        match module {
            Module::Network => self.network.as_ref().map(|net| (net.received, net.sent)),
            Module::Disk => self.disk.as_ref().map(|disk| (disk.read, disk.write)),
            _ => None,
        }
    }

    fn hottest(&self) -> Option<&(String, f64)> {
        self.temperatures.iter().max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Whether the bar has anything to show for it.
    fn has(&self, module: Module) -> bool {
        match module {
            Module::Network => self.network.is_some(),
            _ => self.fraction(module).is_some(),
        }
    }
}

// Reading, on the thread.

/// Counters that two readings turn into rates.
#[derive(Default)]
struct Before {
    /// Each `cpu` line of /proc/stat: in use by programs, by the kernel, idle, and in all.
    cpu: Vec<[u64; 4]>,
    disk: Option<(u64, u64)>,
    net: Option<(String, u64, u64)>,
    ticks: HashMap<i32, u64>,
    io: HashMap<i32, u64>,
    /// Each socket's bytes sent and received, by its addresses.
    sockets: HashMap<String, (u64, u64)>,
    /// Each Intel GPU client's busy time on each engine, in nanoseconds.
    engines: HashMap<(String, String), u64>,
    public_ip: Option<(Instant, String, Option<(String, String)>)>,
    at: Option<Instant>,
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn read_number(path: impl AsRef<Path>) -> Option<f64> {
    read(path).trim().parse().ok()
}

/// Each `cpu` line's times: user and nice, system, irq and softirq, idle and iowait, all.
fn parse_cpu_times(stat: &str) -> Vec<[u64; 4]> {
    stat.lines()
        .take_while(|line| line.starts_with("cpu"))
        .map(|line| {
            let times: Vec<u64> = line.split_whitespace().skip(1).take(8).filter_map(|time| time.parse().ok()).collect();
            let at = |index: usize| times.get(index).copied().unwrap_or(0);
            [at(0) + at(1), at(2) + at(5) + at(6), at(3) + at(4), times.iter().sum()]
        })
        .collect()
}

/// The share of the time between two readings spent in use, by programs, and by the kernel.
fn cpu_share(before: &[u64; 4], now: &[u64; 4]) -> (f64, f64, f64) {
    let total = now[3].saturating_sub(before[3]) as f64;
    if total <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let part = |index: usize| now[index].saturating_sub(before[index]) as f64 / total;
    ((1.0 - part(2)).clamp(0.0, 1.0), part(0), part(1))
}

fn read_cpu(before: &mut Before, temperatures: &[(String, f64)]) -> Option<Cpu> {
    let times = parse_cpu_times(&read("/proc/stat"));
    let previous = std::mem::replace(&mut before.cpu, times.clone());
    if previous.len() != times.len() || times.is_empty() {
        return None;
    }
    let shares: Vec<_> = previous.iter().zip(&times).map(|(before, now)| cpu_share(before, now)).collect();
    let (usage, user, system) = shares[0];
    let load: Vec<f64> = read("/proc/loadavg").split_whitespace().take(3).filter_map(|load| load.parse().ok()).collect();
    let frequencies: Vec<f64> = std::fs::read_dir("/sys/devices/system/cpu/cpufreq")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|policy| read_number(policy.path().join("scaling_cur_freq")))
        .collect();
    Some(Cpu {
        usage,
        user,
        system,
        cores: shares[1..].iter().map(|share| share.0).collect(),
        load: [0, 1, 2].map(|index| load.get(index).copied().unwrap_or(0.0)),
        uptime: read("/proc/uptime").split('.').next().and_then(|seconds| seconds.parse().ok()).unwrap_or(0),
        mhz: (!frequencies.is_empty()).then(|| frequencies.iter().sum::<f64>() / frequencies.len() as f64 / 1000.0),
        temperature: temperatures.iter().find(|(name, _)| name == "CPU").map(|(_, celsius)| *celsius),
    })
}

/// A value of /proc/meminfo, in bytes.
fn meminfo_bytes(meminfo: &str, key: &str) -> f64 {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
        .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0)
        * 1024.0
}

fn read_ram() -> Option<Ram> {
    let meminfo = read("/proc/meminfo");
    let bytes = |key| meminfo_bytes(&meminfo, key);
    let total = bytes("MemTotal");
    if total <= 0.0 {
        return None;
    }
    let pressure = read("/proc/pressure/memory")
        .lines()
        .find_map(|line| line.strip_prefix("some "))
        .and_then(|line| line.split_whitespace().find_map(|field| field.strip_prefix("avg10=")))
        .and_then(|avg| avg.parse::<f64>().ok())
        .map(|percent| percent / 100.0);
    Some(Ram {
        total,
        used: total - bytes("MemAvailable"),
        cached: bytes("Cached") + bytes("Buffers") + bytes("SReclaimable"),
        swap: (bytes("SwapTotal") - bytes("SwapFree"), bytes("SwapTotal")),
        pressure,
    })
}

/// Sectors read and written by the disks themselves, not their partitions, loop devices or
/// what is mapped over them.
fn parse_disk_sectors(diskstats: &str, is_disk: impl Fn(&str) -> bool) -> (u64, u64) {
    diskstats
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let sectors = |index: usize| fields.get(index).and_then(|field| field.parse::<u64>().ok()).unwrap_or(0);
            is_disk(fields.get(2)?).then(|| (sectors(5), sectors(9)))
        })
        .fold((0, 0), |(read, written), (r, w)| (read + r, written + w))
}

fn read_disk(before: &mut Before, elapsed: Option<f64>) -> Option<Disk> {
    // SAFETY: statvfs fills the struct it is given, which lives through the call.
    let (total, free) = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c"/".as_ptr(), &mut stat) != 0 {
            return None;
        }
        let block = stat.f_frsize as f64;
        (stat.f_blocks as f64 * block, stat.f_bavail as f64 * block)
    };
    let sectors = parse_disk_sectors(&read("/proc/diskstats"), |name| {
        Path::new("/sys/block").join(name).join("device").exists()
    });
    let rate = |now: u64, then: u64| match elapsed {
        Some(seconds) if seconds > 0.0 => now.saturating_sub(then) as f64 * 512.0 / seconds,
        _ => 0.0,
    };
    let (read_rate, write_rate) = before
        .disk
        .replace(sectors)
        .map_or((0.0, 0.0), |(read, written)| (rate(sectors.0, read), rate(sectors.1, written)));
    Some(Disk { total, free, read: read_rate, write: write_rate })
}

/// The interface the default route goes out of, from /proc/net/route.
fn default_interface(route: &str) -> Option<String> {
    route.lines().skip(1).find_map(|line| {
        let mut fields = line.split_whitespace();
        let interface = fields.next()?;
        (fields.next()? == "00000000").then(|| interface.to_string())
    })
}

fn read_network(before: &mut Before, elapsed: Option<f64>) -> Option<Net> {
    let interface = default_interface(&read("/proc/net/route"))?;
    let statistics = PathBuf::from("/sys/class/net").join(&interface).join("statistics");
    let received = read_number(statistics.join("rx_bytes"))? as u64;
    let sent = read_number(statistics.join("tx_bytes"))? as u64;
    let rate = |now: u64, then: u64| match elapsed {
        Some(seconds) if seconds > 0.0 => now.saturating_sub(then) as f64 / seconds,
        _ => 0.0,
    };
    // Another interface starts again from nothing.
    let (received_rate, sent_rate) = match before.net.replace((interface.clone(), received, sent)) {
        Some((was, then_received, then_sent)) if was == interface => (rate(received, then_received), rate(sent, then_sent)),
        _ => (0.0, 0.0),
    };
    Some(Net {
        interface,
        received: received_rate,
        sent: sent_rate,
        received_total: received as f64,
        sent_total: sent as f64,
    })
}

/// `nvidia-smi`'s line for the name, use, memory in MiB, temperature and power.
fn parse_nvidia(line: &str) -> Option<Gpu> {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let number = |index: usize| fields.get(index).and_then(|field| field.parse::<f64>().ok());
    const MIB: f64 = 1024.0 * 1024.0;
    Some(Gpu {
        name: fields.first().filter(|name| !name.is_empty())?.to_string(),
        usage: number(1)? / 100.0,
        memory: number(2).zip(number(3)).map(|(used, total)| (used * MIB, total * MIB)),
        temperature: number(4),
        power: number(5),
    })
}

/// An i915 client's fdinfo: its client ID and each engine's busy time in nanoseconds, over
/// how many of that engine there are.
fn parse_fdinfo(fdinfo: &str) -> Option<(String, Vec<(String, u64, u64)>)> {
    let field = |key: &str| fdinfo.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(':')).map(str::trim);
    if field("drm-driver")? != "i915" {
        return None;
    }
    let engines = fdinfo
        .lines()
        .filter_map(|line| {
            let (engine, value) = line.strip_prefix("drm-engine-")?.split_once(':')?;
            if engine.starts_with("capacity-") {
                return None;
            }
            let nanoseconds = value.trim().strip_suffix(" ns")?.parse().ok()?;
            let capacity = field(&format!("drm-engine-capacity-{engine}")).and_then(|count| count.parse().ok()).unwrap_or(1);
            Some((engine.to_string(), nanoseconds, capacity))
        })
        .collect();
    Some((field("drm-client-id")?.to_string(), engines))
}

/// How busy Intel's GPU is: its busiest engine's share of the time between two readings, its
/// clients' times added up, each client once however many files it has open.
// shortcut: only i915 has busy times in nanoseconds; xe counts cycles, read once one is seen.
fn read_intel(before: &mut Before, elapsed: Option<f64>) -> Option<Gpu> {
    let mut clients: HashMap<String, Vec<(String, u64, u64)>> = HashMap::new();
    for process in std::fs::read_dir("/proc").ok()?.flatten() {
        for fd in std::fs::read_dir(process.path().join("fd")).into_iter().flatten().flatten() {
            let is_gpu = std::fs::read_link(fd.path()).is_ok_and(|target| target.starts_with("/dev/dri"));
            if !is_gpu {
                continue;
            }
            if let Some((client, engines)) = parse_fdinfo(&read(process.path().join("fdinfo").join(fd.file_name()))) {
                clients.insert(client, engines);
            }
        }
    }
    if clients.is_empty() {
        return None;
    }
    let now: HashMap<(String, String), u64> = clients
        .iter()
        .flat_map(|(client, engines)| engines.iter().map(move |(engine, busy, _)| ((client.clone(), engine.clone()), *busy)))
        .collect();
    let capacity: HashMap<&str, u64> = clients.values().flatten().map(|(engine, _, count)| (engine.as_str(), *count)).collect();
    let previous = std::mem::replace(&mut before.engines, now.clone());
    let mut busy: HashMap<&str, u64> = HashMap::new();
    for ((client, engine), nanoseconds) in &now {
        if let Some(then) = previous.get(&(client.clone(), engine.clone())) {
            *busy.entry(engine.as_str()).or_default() += nanoseconds.saturating_sub(*then);
        }
    }
    let seconds = elapsed.filter(|seconds| *seconds > 0.0)?;
    let usage = busy
        .iter()
        .map(|(engine, nanoseconds)| *nanoseconds as f64 / 1e9 / seconds / capacity.get(engine).copied().unwrap_or(1).max(1) as f64)
        .fold(0.0, f64::max);
    Some(Gpu { name: "Intel Graphics".into(), usage: usage.clamp(0.0, 1.0), ..Default::default() })
}

/// Every GPU that has a busy figure: AMD's, NVIDIA's, then Intel's, the discrete ones first.
fn read_gpus(before: &mut Before, elapsed: Option<f64>) -> Vec<Gpu> {
    let mut gpus: Vec<Gpu> = read_discrete().into_iter().collect();
    gpus.extend(read_intel(before, elapsed));
    gpus
}

fn read_discrete() -> Option<Gpu> {
    let mut cards: Vec<PathBuf> = std::fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("card") && !name.contains('-')))
        .collect();
    cards.sort();
    let amd = cards.iter().find_map(|card| {
        let device = card.join("device");
        let usage = read_number(device.join("gpu_busy_percent"))? / 100.0;
        let memory = read_number(device.join("mem_info_vram_used")).zip(read_number(device.join("mem_info_vram_total")));
        let temperature = std::fs::read_dir(device.join("hwmon"))
            .into_iter()
            .flatten()
            .flatten()
            .find_map(|hwmon| read_number(hwmon.path().join("temp1_input")))
            .map(|milli| milli / 1000.0);
        Some(Gpu { name: "AMD Radeon".into(), usage, memory, temperature, power: None })
    });
    amd.or_else(|| {
        // shortcut: one nvidia-smi a reading costs it a start each time; stream it with
        // `-lms` if that shows in the CPU's own use.
        let output = Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        parse_nvidia(String::from_utf8_lossy(&output.stdout).lines().next()?)
    })
}

/// What a chip is called among the temperatures, by its hwmon name.
fn chip_name(name: &str) -> String {
    match name {
        "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" => "CPU".into(),
        "nvme" => "SSD".into(),
        "amdgpu" | "nouveau" | "radeon" => "GPU".into(),
        "acpitz" => "ACPI".into(),
        name if name.starts_with("iwlwifi") => "Wi-Fi".into(),
        name => name.split('_').next().unwrap_or(name).to_string(),
    }
}

/// Each chip's first temperature, which is its package's or composite's, and each fan, of the
/// chips in `hwmon`.
fn read_sensors(hwmon: &Path) -> (Vec<(String, f64)>, Vec<Fan>) {
    let mut chips: Vec<PathBuf> = std::fs::read_dir(hwmon)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    chips.sort_by_key(|path| {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
        (name.len(), name)
    });
    let (mut temperatures, mut fans) = (Vec::new(), Vec::new());
    for chip in chips {
        let name = chip_name(read(chip.join("name")).trim());
        let mut inputs: Vec<(u32, String)> = std::fs::read_dir(&chip)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|file| {
                let stem = file.strip_suffix("_input")?;
                let index = stem.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()?;
                Some((index, stem.to_string()))
            })
            .collect();
        inputs.sort();
        if let Some((_, stem)) = inputs.iter().find(|(_, stem)| stem.starts_with("temp")) {
            if let Some(milli) = read_number(chip.join(format!("{stem}_input"))).filter(|milli| *milli > 0.0) {
                let same = temperatures.iter().filter(|(found, _): &&(String, f64)| found.starts_with(&name)).count();
                let shown = if same == 0 { name.clone() } else { format!("{name} {}", same + 1) };
                temperatures.push((shown, milli / 1000.0));
            }
        }
        for (index, stem) in inputs.iter().filter(|(_, stem)| stem.starts_with("fan")) {
            if let Some(rpm) = read_number(chip.join(format!("{stem}_input"))) {
                let label = read(chip.join(format!("{stem}_label"))).trim().to_string();
                // shortcut: fanN is taken to be driven by pwmN, as most chips have it; a chip
                // that maps them otherwise needs its pwmN_auto_channels read.
                let pwm = chip.join(format!("pwm{index}"));
                let control = read_number(&pwm).zip(read_number(chip.join(format!("pwm{index}_enable"))));
                fans.push(Fan {
                    name: if label.is_empty() { format!("風扇 {index}") } else { label },
                    rpm,
                    pwm: control.map(|(duty, enable)| (pwm, duty / 255.0, enable as u8)),
                });
            }
        }
    }
    (temperatures, fans)
}

/// The `count` busiest, the processes of one name, as a program's many are, taken together.
fn busiest(amounts: Vec<(String, f64)>, count: usize) -> Vec<(String, f64)> {
    let mut by_name: HashMap<String, f64> = HashMap::new();
    for (name, amount) in amounts {
        *by_name.entry(name).or_default() += amount;
    }
    let mut amounts: Vec<(String, f64)> = by_name.into_iter().collect();
    amounts.sort_by(|a, b| b.1.total_cmp(&a.1));
    amounts.truncate(count);
    amounts
}

/// The busiest processes by CPU, by memory and by disk, `count` of each. Only the user's own
/// processes say what they read and write.
fn read_processes(before: &mut Before, elapsed: Option<f64>, count: usize) -> [Vec<(String, f64)>; 3] {
    let procs = proc::read_procs();
    let io: HashMap<i32, u64> = procs.iter().filter_map(|proc| Some((proc.pid, proc.io?))).collect();
    let previous_io = std::mem::replace(&mut before.io, io);
    let by_disk: Vec<(String, f64)> = match elapsed {
        Some(seconds) if seconds > 0.0 => procs
            .iter()
            .filter_map(|proc| Some((proc.name.clone(), proc.io?.saturating_sub(*previous_io.get(&proc.pid)?) as f64 / seconds)))
            .filter(|(_, rate)| *rate > 0.0)
            .collect(),
        _ => Vec::new(),
    };
    // SAFETY: sysconf only reads a value of the system's.
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
    let ticks: HashMap<i32, u64> = procs.iter().map(|proc| (proc.pid, proc.ticks)).collect();
    let previous = std::mem::replace(&mut before.ticks, ticks);
    let by_cpu: Vec<(String, f64)> = match elapsed {
        Some(seconds) if seconds > 0.0 => procs
            .iter()
            .filter_map(|proc| {
                let used = proc.ticks.saturating_sub(*previous.get(&proc.pid)?) as f64;
                Some((proc.name.clone(), used / ticks_per_second / seconds * 100.0))
            })
            .collect(),
        _ => Vec::new(),
    };
    let by_memory = procs.iter().filter_map(|proc| Some((proc.name.clone(), proc.memory? as f64))).collect();
    [busiest(by_cpu, count), busiest(by_memory, count), busiest(by_disk, count)]
}

/// Each TCP socket of `ss -tinpH` with its process's name: its addresses, and its bytes sent
/// and received. A socket whose process is not the user's has no name and is left out.
fn parse_ss(ss: &str) -> Vec<(String, String, u64, u64)> {
    let mut sockets = Vec::new();
    let mut lines = ss.lines().peekable();
    while let Some(line) = lines.next() {
        let details = match lines.peek() {
            Some(next) if next.starts_with(char::is_whitespace) => lines.next().unwrap_or_default(),
            _ => "",
        };
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = line.split("((\"").nth(1).and_then(|rest| rest.split('"').next()) else {
            continue;
        };
        let counter = |key: &str| {
            details.split_whitespace().find_map(|field| field.strip_prefix(key)?.parse::<u64>().ok()).unwrap_or(0)
        };
        let addresses = format!("{} {}", fields.get(3).unwrap_or(&""), fields.get(4).unwrap_or(&""));
        sockets.push((name.to_string(), addresses, counter("bytes_sent:"), counter("bytes_received:")));
    }
    sockets
}

/// The processes that send and receive the most over TCP, in and out in bytes a second.
fn read_network_processes(before: &mut Before, elapsed: Option<f64>, count: usize) -> Vec<(String, f64, f64)> {
    let Ok(output) = Command::new("ss").args(["-tinpH"]).output() else {
        return Vec::new();
    };
    let sockets = parse_ss(&String::from_utf8_lossy(&output.stdout));
    let previous = std::mem::replace(
        &mut before.sockets,
        sockets.iter().map(|(_, addresses, sent, received)| (addresses.clone(), (*sent, *received))).collect(),
    );
    let Some(seconds) = elapsed.filter(|seconds| *seconds > 0.0) else {
        return Vec::new();
    };
    let mut by_name: HashMap<String, (f64, f64)> = HashMap::new();
    for (name, addresses, sent, received) in &sockets {
        if let Some((then_sent, then_received)) = previous.get(addresses) {
            let rates = by_name.entry(name.clone()).or_default();
            rates.0 += received.saturating_sub(*then_received) as f64 / seconds;
            rates.1 += sent.saturating_sub(*then_sent) as f64 / seconds;
        }
    }
    let mut busiest: Vec<(String, f64, f64)> =
        by_name.into_iter().filter(|(_, (down, up))| down + up > 0.0).map(|(name, (down, up))| (name, down, up)).collect();
    busiest.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)));
    busiest.truncate(count);
    busiest
}

/// The address and country Stats' own service sees, as `{"ipv4": …, "country": …}`.
fn parse_public_ip(json: &str) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let address = value.get("ipv4").or_else(|| value.get("ipv6"))?.as_str()?.to_string();
    let country = value.get("country").and_then(|country| country.as_str()).unwrap_or_default().to_string();
    Some((address, country))
}

fn fetch_public_ip() -> Option<(String, String)> {
    ["-4", "-6"].into_iter().find_map(|family| {
        let output = Command::new("curl").args(["-s", "--max-time", "5", family, PUBLIC_IP_SERVICE]).output().ok()?;
        parse_public_ip(&String::from_utf8_lossy(&output.stdout))
    })
}

/// The public address, asked for again every `PUBLIC_IP_EVERY` and when the interface changes.
fn read_public_ip(before: &mut Before, interface: &str) -> Option<(String, String)> {
    let fresh = before
        .public_ip
        .as_ref()
        .is_some_and(|(at, was, _)| was == interface && at.elapsed() < PUBLIC_IP_EVERY);
    if !fresh {
        before.public_ip = Some((Instant::now(), interface.to_string(), fetch_public_ip()));
    }
    before.public_ip.as_ref().and_then(|(_, _, found)| found.clone())
}

fn read_sample(before: &mut Before, wanted: u32) -> Sample {
    let now = Instant::now();
    let elapsed = before.at.replace(now).map(|at| now.duration_since(at).as_secs_f64());
    let wants = |module: Module| wanted & module.bit() != 0;
    let mut sample = Sample::default();
    if wants(Module::Sensors) || wants(Module::Cpu) {
        (sample.temperatures, sample.fans) = read_sensors(Path::new(HWMON));
    }
    if wants(Module::Cpu) {
        sample.cpu = read_cpu(before, &sample.temperatures);
    }
    if wants(Module::Gpu) {
        let mut gpus = read_gpus(before, elapsed).into_iter();
        sample.gpu = gpus.next();
        sample.other_gpus = gpus.collect();
    }
    if wants(Module::Ram) {
        sample.ram = read_ram();
    }
    if wants(Module::Disk) {
        sample.disk = read_disk(before, elapsed);
    }
    if wants(Module::Network) {
        sample.network = read_network(before, elapsed);
        if let Some(net) = sample.network.as_ref().filter(|_| PUBLIC_IP.load(Ordering::Relaxed)) {
            sample.public_ip = read_public_ip(before, &net.interface);
        }
    }
    let count = (PROCESSES.load(Ordering::Relaxed) as usize).min(MAX_PROCESSES);
    if count > 0 && (wants(Module::Cpu) || wants(Module::Ram) || wants(Module::Disk)) {
        [sample.top_cpu, sample.top_ram, sample.top_disk] = read_processes(before, elapsed, count);
    }
    if count > 0 && wants(Module::Network) {
        sample.top_network = read_network_processes(before, elapsed, count);
    }
    if !wants(Module::Sensors) {
        sample.temperatures.clear();
        sample.fans.clear();
    }
    sample
}

/// Reads every `INTERVAL` seconds what `WANTED` asks for, until nobody takes the readings.
fn spawn_reader() -> Option<Receiver<Sample>> {
    let (send, found) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("nekoland-stats".into()).spawn(move || {
        let mut before = Before::default();
        let mut wanted = 0;
        loop {
            let now = WANTED.load(Ordering::Relaxed);
            if now == 0 {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            // What was just asked for has nothing to count from; it is read again soon.
            let fresh = now & !wanted != 0;
            wanted = now;
            if send.send(read_sample(&mut before, wanted)).is_err() {
                return;
            }
            let seconds = INTERVAL.load(Ordering::Relaxed).clamp(1, 10);
            std::thread::sleep(if fresh { Duration::from_millis(500) } else { Duration::from_secs(seconds.into()) });
        }
    });
    match spawned {
        Ok(_) => Some(found),
        Err(err) => {
            tracing::warn!("no system readings: {err}");
            None
        }
    }
}

// Words.

fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

/// Bytes in the binary units, with a decimal under ten.
fn size_text(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn speed_text(bytes: f64) -> String {
    format!("{}/s", size_text(bytes))
}

/// What runs the helper as root to set the fan at `pwm`: `mode`, its `pwm_enable`, or
/// `speed`, how fast from 0 to 255.
fn fan_command(command: &str, pwm: &str, value: u32) -> String {
    format!("pkexec {FAN_HELPER} {command} {pwm} {value}")
}

/// `template` with each reading's name, `$cpu` and the like, replaced by its value, or by – for
/// one not read.
fn fill_text(template: &str, sample: &Sample, fahrenheit: bool) -> String {
    let usage = |module| sample.fraction(module).map(percent);
    let net = sample.network.as_ref();
    [
        ("$cpu", usage(Module::Cpu)),
        ("$gpu", usage(Module::Gpu)),
        ("$ram", usage(Module::Ram)),
        ("$disk", usage(Module::Disk)),
        ("$temp", sample.hottest().map(|(_, celsius)| temperature_text(*celsius, fahrenheit, false))),
        ("$down", net.map(|net| speed_text(net.received))),
        ("$up", net.map(|net| speed_text(net.sent))),
    ]
    .into_iter()
    .fold(template.to_string(), |text, (name, value)| text.replace(name, value.as_deref().unwrap_or("–")))
}

fn temperature_text(celsius: f64, fahrenheit: bool, unit: bool) -> String {
    let (value, symbol) = if fahrenheit { (celsius * 9.0 / 5.0 + 32.0, "°F") } else { (celsius, "°C") };
    if unit {
        format!("{value:.0} {symbol}")
    } else {
        format!("{value:.0}°")
    }
}

fn uptime_text(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days} 天 {hours} 小時")
    } else if hours > 0 {
        format!("{hours} 小時 {minutes} 分")
    } else {
        format!("{minutes} 分")
    }
}

/// Ink, or yellow and red where a value needs attention.
fn level_color(value: f64, ink: Rgb) -> Rgb {
    if value >= ALERT {
        RED
    } else if value >= WARN {
        YELLOW
    } else {
        ink
    }
}

fn lerp(from: f64, to: f64, progress: f64) -> f64 {
    from + (to - from) * progress
}

// Drawing, shared by the bar and the panels.

/// A line of `values` from 0 to 1, oldest first, across `capacity` places of `area`, the
/// newest `slide` of the way in from the right, over a fill that fades down from it.
fn draw_line(cr: &Context, (x, y, w, h): (f64, f64, f64, f64), values: &[f64], capacity: usize, slide: f64, color: Rgb, width: f64, fill: bool) {
    if values.len() < 2 {
        return;
    }
    let step = w / (capacity.max(2) - 1) as f64;
    let last = values.len() - 1;
    let point = |index: usize, value: f64| {
        (
            x + w - (last - index) as f64 * step + (1.0 - slide) * step,
            y + h - value.clamp(0.0, 1.0) * (h - width) - width / 2.0,
        )
    };
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();
    let first = values.len().saturating_sub(capacity + 1);
    let trace = |cr: &Context| {
        for (index, value) in values.iter().enumerate().skip(first) {
            let (px, py) = point(index, *value);
            if index == first {
                cr.move_to(px, py);
            } else {
                cr.line_to(px, py);
            }
        }
    };
    if fill {
        trace(cr);
        cr.line_to(point(last, 0.0).0, y + h);
        cr.line_to(point(first, 0.0).0, y + h);
        cr.close_path();
        let gradient = LinearGradient::new(0.0, y, 0.0, y + h);
        gradient.add_color_stop_rgba(0.0, color.0, color.1, color.2, 0.3);
        gradient.add_color_stop_rgba(1.0, color.0, color.1, color.2, 0.02);
        let _ = cr.set_source(&gradient);
        let _ = cr.fill();
    }
    trace(cr);
    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(width);
    cr.set_line_join(cairo::LineJoin::Round);
    let _ = cr.stroke();
    cr.restore().ok();
}

/// Bars from 0 to 1 side by side across `area`, each on a track, and with `alert` yellow or red
/// when high.
fn draw_columns(cr: &Context, (x, y, w, h): (f64, f64, f64, f64), values: &[f64], gap: f64, ink: Rgb, alert: bool) {
    if values.is_empty() {
        return;
    }
    let count = values.len() as f64;
    let width = ((w - gap * (count - 1.0)) / count).max(1.0);
    let radius = (width / 2.0).min(2.0);
    for (index, value) in values.iter().enumerate() {
        let left = x + index as f64 * (width + gap);
        text::rounded_rectangle(cr, left, y, width, h, radius);
        cr.set_source_rgba(ink.0, ink.1, ink.2, 0.15);
        let _ = cr.fill();
        let filled = (value.clamp(0.0, 1.0) * h).max(if *value > 0.0 { 1.0 } else { 0.0 });
        if filled > 0.0 {
            text::rounded_rectangle(cr, left, y + h - filled, width, filled, radius.min(filled / 2.0));
            let (r, g, b) = if alert { level_color(*value, ink) } else { ink };
            cr.set_source_rgb(r, g, b);
            let _ = cr.fill();
        }
    }
}

/// So many cores that their bars would crowd the bar are shown as pairs, each the two's mean.
fn grouped_cores(cores: &[f64], most: usize) -> Vec<f64> {
    let size = cores.len().div_ceil(most.max(1)).max(1);
    cores.chunks(size).map(|chunk| chunk.iter().sum::<f64>() / chunk.len() as f64).collect()
}

/// Speeds as fractions of the most of them, or of `LEAST_SPEED`.
fn scaled(pairs: &[(f64, f64)]) -> (Vec<f64>, Vec<f64>) {
    let most = pairs.iter().fold(LEAST_SPEED, |most, (a, b)| most.max(*a).max(*b));
    (pairs.iter().map(|(a, _)| a / most).collect(), pairs.iter().map(|(_, b)| b / most).collect())
}

// The compositor's side.

/// What the panel shows, which it is drawn again when any of changes.
#[derive(Clone, PartialEq)]
struct Shown {
    count: u64,
    slide: u32,
    dark: bool,
    hovered: Option<Part>,
    fahrenheit: bool,
    /// The fan being set by its slider, and how fast, in thousandths.
    fan_drag: Option<(usize, u32)>,
    fan_reveal: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    /// A reading in the panel of them all, which opens its own.
    Row(Module),
    /// A fan that can be set, which turns between the chip setting it and set by hand, and
    /// its slider while set by hand.
    FanMode(usize),
    FanSpeed(usize),
    Tasks,
    Settings,
}

/// What a panel shows: one reading, or all of them, as the one item they make opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Module(Module),
    Summary,
}

impl View {
    /// Whether the item for this is lit and keeps the pointer while `open` shows: the one item
    /// of them all stays so as a reading's panel takes the place of theirs.
    pub(crate) fn owns(self, open: View) -> bool {
        self == open || self == View::Summary
    }

    pub(crate) fn order_name(self) -> &'static str {
        match self {
            View::Module(module) => module.order_name(),
            View::Summary => "stats:combined",
        }
    }
}

/// Tells of a reading once when it has stayed past its threshold `ALERT_AFTER` times, and
/// again only after it has come back.
#[derive(Default)]
struct Alerts(HashMap<Module, (u32, bool)>);

impl Alerts {
    /// Whether to tell of `module` now, it being `past` its threshold, or None without one.
    fn check(&mut self, module: Module, past: Option<bool>) -> bool {
        if past != Some(true) {
            self.0.remove(&module);
            return false;
        }
        let (count, told) = self.0.entry(module).or_default();
        *count += 1;
        if *count >= ALERT_AFTER && !*told {
            *told = true;
            return true;
        }
        false
    }
}

pub struct Panel {
    view: View,
    /// Its left edge, from the output's.
    x: i32,
    opened: Instant,
    /// When it began to fade out, once it was closed.
    closed: Option<Instant>,
    hovered: Option<Part>,
    pressed: Option<Part>,
    parts: Vec<(Part, Rectangle<i32, Logical>)>,
    shown: Option<Shown>,
    picture: Option<Picture>,
    id: Id,
}

impl Panel {
    fn fade(&self) -> f64 {
        let opening = ease_out(self.opened.elapsed().as_secs_f64() / OPEN_TIME);
        match self.closed {
            Some(closed) => opening.min(1.0 - ease_out(closed.elapsed().as_secs_f64() / OPEN_TIME)),
            None => opening,
        }
    }

    /// How fast the fan's slider asks at `at`, kept from stopping it.
    fn fan_level(&self, index: usize, at: Point<i32, Logical>) -> f64 {
        let Some((_, area)) = self.parts.iter().find(|(part, _)| *part == Part::FanSpeed(index)) else {
            return LEAST_FAN;
        };
        let left = f64::from(area.loc.x + PADDING - INSET + FAN_KNOB);
        let right = f64::from(area.loc.x + area.size.w - PADDING + INSET);
        let track = right - f64::from(text_size("100%", &font("", 10.0)).0) - 12.0 - f64::from(FAN_KNOB) - left;
        ((f64::from(at.x) - left) / track.max(1.0)).clamp(LEAST_FAN, 1.0)
    }

    fn part_at(&self, at: Point<i32, Logical>) -> Option<Part> {
        self.parts.iter().find(|(_, area)| area.contains(at)).map(|(part, _)| *part)
    }
}

#[derive(Default)]
pub struct Stats {
    found: Option<Receiver<Sample>>,
    sample: Option<Sample>,
    /// The reading before, which the bars grow from.
    previous: Option<Sample>,
    /// Each module's readings, oldest first: its fraction, or its two speeds.
    history: HashMap<Module, VecDeque<(f64, f64)>>,
    arrived: Option<Instant>,
    /// How many readings came, which the bar and the panels are drawn again for.
    pub(crate) count: u64,
    wanted: u32,
    pub(crate) panel: Option<Panel>,
    /// The panel fading out as another opens or it closes.
    closing: Option<Panel>,
    alerts: Alerts,
    fan_control: bool,
    animations: bool,
    fan_drag: Option<(usize, f64)>,
    /// Each fan's slider shown or hidden last, and when, which it grows open or closed from.
    fan_reveal: HashMap<usize, (bool, Instant)>,
    /// Each fan's `pwm_enable` before it was set by hand, to give it back.
    automatic: HashMap<PathBuf, u8>,
}

/// Where the parts of a panel are, and what it says, measured before it is drawn.
struct Content {
    value: String,
    second: Option<String>,
    meta: String,
    /// The line or the two lines of its history.
    chart: (Vec<f64>, Option<Vec<f64>>),
    rows: Vec<(String, String)>,
    cores: Option<Vec<f64>>,
    processes: Option<(&'static str, Vec<(String, String)>)>,
    /// The fans that can be set, by their place among the sample's: each one's name, its
    /// speed, whether it is set by hand, and how fast it is driven.
    fans: Vec<(usize, String, String, bool, f64, f64)>,
}

impl Stats {
    pub(crate) fn panel_view(&self) -> Option<View> {
        self.panel.as_ref().map(|panel| panel.view)
    }

    /// What to tell of the newest reading: each title and body past its threshold.
    fn alerts(&mut self, config: &Config) -> Vec<(String, String)> {
        let Some(sample) = self.sample.clone() else {
            return Vec::new();
        };
        let mut told = Vec::new();
        for module in Module::ALL {
            let enabled = module.enabled(config);
            if module == Module::Sensors {
                let limit = f64::from(config.stats_alert_temperature);
                let hottest = sample.hottest().filter(|_| enabled && limit > 0.0);
                if self.alerts.check(module, hottest.map(|(_, celsius)| *celsius >= limit)) {
                    if let Some((name, celsius)) = hottest {
                        let shown = |celsius| temperature_text(celsius, config.stats_fahrenheit, true);
                        told.push((format!("{name} 溫度 {}", shown(*celsius)), format!("超過 {}", shown(limit))));
                    }
                }
                continue;
            }
            let limit = f64::from(config.stats_alert) / 100.0;
            let value = sample.fraction(module).filter(|_| enabled && limit > 0.0 && module != Module::Network);
            if self.alerts.check(module, value.map(|value| value >= limit)) {
                if let Some(value) = value {
                    told.push((format!("{} {}", module.usage_name(), percent(value)), format!("超過 {}", percent(limit))));
                }
            }
        }
        told
    }

    /// How a module is drawn: as the settings ask, but the network's speeds always as numbers
    /// where its two would not fit one ring or gauge.
    fn style(module: Module, config: &Config) -> StatsStyle {
        match config.stats_style {
            StatsStyle::Ring | StatsStyle::Gauge | StatsStyle::Pie if module == Module::Network => StatsStyle::Value,
            style => style,
        }
    }

    /// How wide all the shown readings are as one item.
    pub(crate) fn combined_width(&self, config: &Config) -> i32 {
        if let Some(text) = self.text(config) {
            return text_size(&text, &font("Medium", 10.0)).0;
        }
        let modules = self.shown(config);
        let gaps = COMBINED_GAP * (modules.len() as i32 - 1).max(0);
        modules.iter().map(|module| self.item_width(*module, config)).sum::<i32>() + gaps
    }

    /// Draws all the shown readings side by side in `area`, as one item.
    pub(crate) fn draw_combined(&self, cr: &Context, area: Rectangle<i32, Logical>, ink: Rgb, config: &Config) {
        let mut x = area.loc.x + (area.size.w - self.combined_width(config)) / 2;
        if let Some(text) = self.text(config) {
            show_text(cr, &text, &font("Medium", 10.0), f64::from(x), f64::from(MENU_BAR_HEIGHT) / 2.0, ink);
            return;
        }
        for module in self.shown(config) {
            let width = self.item_width(module, config);
            self.draw_item(cr, module, rect(x, area.loc.y, width, area.size.h), ink, config);
            x += width + COMBINED_GAP;
        }
    }

    /// How far the fan's slider is shown, from 0 to 1, as it grows open or closed.
    fn fan_reveal(&self, index: usize, manual: bool) -> f64 {
        match self.fan_reveal.get(&index) {
            Some((shown, at)) if self.animations => {
                let progress = ease_out(at.elapsed().as_secs_f64() / OPEN_TIME);
                if *shown { progress } else { 1.0 - progress }
            }
            _ => f64::from(u8::from(manual)),
        }
    }

    /// The fans' sliders' showing, in thousandths, which the panel is drawn again for.
    fn fan_reveals(&self) -> u32 {
        self.fan_reveal.keys().map(|index| {
            let manual = self.fan_reveal[index].0;
            (self.fan_reveal(*index, manual) * 1000.0) as u32
        }).sum()
    }

    /// The settings' text with the newest readings in it, or None without one.
    fn text(&self, config: &Config) -> Option<String> {
        let sample = self.sample.as_ref().filter(|_| !config.stats_text.is_empty())?;
        Some(fill_text(&config.stats_text, sample, config.stats_fahrenheit))
    }

    /// The parts of the module's use, eased in as `fraction` is: those the pie shows.
    fn parts(&self, module: Module, progress: f64) -> Vec<f64> {
        let parts = |sample: &Sample| -> Option<Vec<f64>> {
            match module {
                Module::Cpu => sample.cpu.as_ref().map(|cpu| vec![cpu.system, cpu.user]),
                Module::Ram => sample.ram.as_ref().filter(|ram| ram.total > 0.0).map(|ram| vec![ram.used / ram.total, ram.cached / ram.total]),
                _ => sample.fraction(module).map(|value| vec![value]),
            }
        };
        let now = self.sample.as_ref().and_then(parts).unwrap_or_default();
        match self.previous.as_ref().and_then(parts).filter(|before| before.len() == now.len()) {
            Some(before) => before.iter().zip(&now).map(|(from, to)| lerp(*from, *to, progress)).collect(),
            None => now,
        }
    }

    /// How far the newest reading has come in, from 0 to 1.
    fn progress(&self, animations: bool) -> f64 {
        match self.arrived {
            Some(arrived) if animations => ease_out(arrived.elapsed().as_secs_f64() / SETTLE),
            _ => 1.0,
        }
    }

    /// Whether a new reading is still sliding in, and how far, in thousandths.
    pub(crate) fn settling(&self, animations: bool) -> u32 {
        (self.progress(animations) * 1000.0).round() as u32
    }

    /// The modules the bar shows, left to right as their readings have them.
    pub(crate) fn shown(&self, config: &Config) -> Vec<Module> {
        let Some(sample) = &self.sample else {
            return Vec::new();
        };
        Module::ALL.into_iter().filter(|module| module.enabled(config) && sample.has(*module)).collect()
    }

    fn fraction(&self, module: Module, progress: f64) -> f64 {
        let now = self.sample.as_ref().and_then(|sample| sample.fraction(module)).unwrap_or(0.0);
        let before = self.previous.as_ref().and_then(|sample| sample.fraction(module)).unwrap_or(now);
        lerp(before, now, progress)
    }

    fn cores(&self, progress: f64) -> Vec<f64> {
        let now = self.sample.as_ref().and_then(|sample| sample.cpu.as_ref()).map(|cpu| cpu.cores.clone()).unwrap_or_default();
        let before = self.previous.as_ref().and_then(|sample| sample.cpu.as_ref()).map(|cpu| cpu.cores.clone());
        match before.filter(|before| before.len() == now.len()) {
            Some(before) => before.iter().zip(&now).map(|(from, to)| lerp(*from, *to, progress)).collect(),
            None => now,
        }
    }

    fn history(&self, module: Module) -> Vec<(f64, f64)> {
        self.history.get(&module).map(|history| history.iter().copied().collect()).unwrap_or_default()
    }

    fn take(&mut self, sample: Sample) {
        for module in Module::ALL {
            let point = match module {
                Module::Network | Module::Disk => sample.speeds(module),
                _ => sample.fraction(module).map(|value| (value, 0.0)),
            };
            let history = self.history.entry(module).or_default();
            match point {
                Some(point) => {
                    history.push_back(point);
                    while history.len() > HISTORY + 1 {
                        history.pop_front();
                    }
                }
                None => history.clear(),
            }
        }
        self.previous = self.sample.replace(sample);
        self.arrived = Some(Instant::now());
        self.count += 1;
    }

    /// How wide the module is in the bar.
    pub(crate) fn item_width(&self, module: Module, config: &Config) -> i32 {
        let value_font = font("Medium", if config.stats_labels { 9.0 } else { 10.0 });
        let labelled = config.stats_labels && module != Module::Network;
        let label_column = if config.stats_labels { text_size("W", &standing_font()).0 as f64 + BAR_LABEL_GAP } else { 0.0 };
        match Self::style(module, config) {
            StatsStyle::Ring | StatsStyle::Pie => (label_column + RING) as i32,
            StatsStyle::Gauge => (label_column + GAUGE.0) as i32,
            StatsStyle::Value if module == Module::Network => text_size("↓ 999 KB/s", &font("Medium", 8.0)).0,
            StatsStyle::Value => {
                let widest = if module == Module::Sensors { "100°" } else { "100%" };
                let value = text_size(widest, &value_font).0;
                if labelled { value.max(text_size(module.label(), &label_font()).0) } else { value }
            }
            StatsStyle::Chart => (label_column + BAR_CHART.0) as i32,
            StatsStyle::Bars => {
                let bars = match module {
                    Module::Cpu => {
                        let count = grouped_cores(&self.cores(1.0), MOST_BARS).len() as f64;
                        count * 3.0 + (count - 1.0).max(0.0)
                    }
                    Module::Network => 13.0,
                    _ => 6.0,
                };
                (label_column + bars) as i32
            }
        }
    }

    /// Draws the module in the bar, in `area`, its text in `ink`.
    pub(crate) fn draw_item(&self, cr: &Context, module: Module, area: Rectangle<i32, Logical>, ink: Rgb, config: &Config) {
        let progress = self.progress(config.animations);
        let width = f64::from(self.item_width(module, config));
        let (x, center) = (f64::from(area.loc.x) + (f64::from(area.size.w) - width) / 2.0, f64::from(MENU_BAR_HEIGHT) / 2.0);
        let Some(sample) = &self.sample else {
            return;
        };
        let fraction = self.fraction(module, progress);
        let value_text = match module {
            Module::Sensors => sample.hottest().map(|(_, celsius)| temperature_text(*celsius, config.stats_fahrenheit, false)).unwrap_or_default(),
            _ => percent(fraction),
        };
        let style = Self::style(module, config);
        if style == StatsStyle::Value {
            if let Some(net) = sample.network.as_ref().filter(|_| module == Module::Network) {
                let small = font("Medium", 8.0);
                show_text(cr, &format!("↑ {}", speed_text(net.sent)), &small, x, center - 5.5, ink);
                show_text(cr, &format!("↓ {}", speed_text(net.received)), &small, x, center + 5.5, ink);
                return;
            }
            let color = if module == Module::Sensors { ink } else { level_color(fraction, ink) };
            if config.stats_labels {
                let label = label_font();
                show_text(cr, module.label(), &label, x, center - 6.0, ink);
                show_text(cr, &value_text, &font("Medium", 9.0), x, center + 4.5, color);
            } else {
                show_text(cr, &value_text, &font("Medium", 10.0), x, center, color);
            }
            return;
        }
        let mut left = x;
        if config.stats_labels {
            let label = standing_font();
            let letters: Vec<String> = module.label().chars().map(String::from).collect();
            let top = center - STANDING_LINE * (letters.len() as f64 - 1.0) / 2.0;
            let column = f64::from(text_size("W", &label).0);
            for (index, letter) in letters.iter().enumerate() {
                let letter_width = f64::from(text_size(letter, &label).0);
                show_text(cr, letter, &label, x + (column - letter_width) / 2.0, top + STANDING_LINE * index as f64, ink);
            }
            left += column + BAR_LABEL_GAP;
        }
        let (w, h) = (x + width - left, BAR_CHART.1);
        let top = center - h / 2.0;
        let arc = |cr: &Context, (cx, cy, radius): (f64, f64, f64), (from, to): (f64, f64), color: Rgb, alpha: f64| {
            cr.new_sub_path();
            cr.arc(cx, cy, radius, from, to);
            cr.set_source_rgba(color.0, color.1, color.2, alpha);
            cr.set_line_width(ARC_WIDTH);
            cr.set_line_cap(cairo::LineCap::Round);
            let _ = cr.stroke();
        };
        use std::f64::consts::{FRAC_PI_2, PI, TAU};
        match style {
            StatsStyle::Pie => {
                // A thick ring, its parts one after another from the top, the first colored by
                // the whole's level and the rest fainter.
                let thickness = RING * 0.3;
                let (cx, radius) = (left + RING / 2.0, (RING - thickness) / 2.0);
                cr.set_line_width(thickness);
                cr.set_line_cap(cairo::LineCap::Butt);
                cr.new_sub_path();
                cr.arc(cx, center, radius, 0.0, TAU);
                cr.set_source_rgba(ink.0, ink.1, ink.2, 0.15);
                let _ = cr.stroke();
                let mut angle = -FRAC_PI_2;
                for (index, part) in self.parts(module, progress).into_iter().enumerate() {
                    let sweep = TAU * part.clamp(0.0, 1.0 - (angle + FRAC_PI_2) / TAU);
                    if sweep <= 0.0 {
                        continue;
                    }
                    let (color, alpha) = if index == 0 { (level_color(fraction, ink), 1.0) } else { (ink, 0.55) };
                    cr.new_sub_path();
                    cr.arc(cx, center, radius, angle, angle + sweep);
                    cr.set_source_rgba(color.0, color.1, color.2, alpha);
                    let _ = cr.stroke();
                    angle += sweep;
                }
            }
            StatsStyle::Ring => {
                let circle = (left + RING / 2.0, center, (RING - ARC_WIDTH) / 2.0);
                arc(cr, circle, (0.0, TAU), ink, 0.15);
                if fraction > 0.0 {
                    arc(cr, circle, (-FRAC_PI_2, -FRAC_PI_2 + TAU * fraction), level_color(fraction, ink), 1.0);
                }
            }
            StatsStyle::Gauge => {
                let radius = GAUGE.0 / 2.0 - ARC_WIDTH / 2.0;
                let (cx, cy) = (left + GAUGE.0 / 2.0, center + GAUGE.1 / 2.0 - ARC_WIDTH / 2.0);
                arc(cr, (cx, cy, radius), (PI, TAU), ink, 0.15);
                let color = level_color(fraction, ink);
                if fraction > 0.0 {
                    arc(cr, (cx, cy, radius), (PI, PI + PI * fraction), color, 1.0);
                }
                // The needle points at the value.
                let angle = PI + PI * fraction;
                cr.move_to(cx, cy);
                cr.line_to(cx + angle.cos() * (radius - 3.0), cy + angle.sin() * (radius - 3.0));
                cr.set_source_rgb(ink.0, ink.1, ink.2);
                cr.set_line_width(1.5);
                let _ = cr.stroke();
                cr.arc(cx, cy, 1.75, 0.0, TAU);
                let _ = cr.fill();
            }
            StatsStyle::Chart => {
                text::rounded_rectangle(cr, left, top, w, h, 3.0);
                cr.set_source_rgba(ink.0, ink.1, ink.2, 0.12);
                let _ = cr.fill();
                let history = self.history(module);
                let inner = (left + 1.0, top + 1.0, w - 2.0, h - 2.0);
                match module {
                    Module::Network | Module::Disk => {
                        let (first, second) = scaled(&history[history.len().saturating_sub(BAR_HISTORY + 1)..]);
                        // The second speed is the fainter line.
                        cr.push_group();
                        draw_line(cr, inner, &second, BAR_HISTORY, progress, ink, 1.0, false);
                        let _ = cr.pop_group_to_source();
                        let _ = cr.paint_with_alpha(0.5);
                        draw_line(cr, inner, &first, BAR_HISTORY, progress, ink, 1.0, true);
                    }
                    _ => {
                        let values: Vec<f64> = history.iter().map(|(value, _)| *value).collect();
                        draw_line(cr, inner, &values, BAR_HISTORY, progress, ink, 1.0, true);
                    }
                }
            }
            _ => {
                let area = (left, top, w, h);
                match module {
                    Module::Cpu => draw_columns(cr, area, &grouped_cores(&self.cores(progress), MOST_BARS), 1.0, ink, true),
                    Module::Network => {
                        let now = sample.speeds(module).unwrap_or_default();
                        let before = self.previous.as_ref().and_then(|sample| sample.speeds(module)).unwrap_or(now);
                        let most = self.history(module).iter().fold(LEAST_SPEED, |most, (a, b)| most.max(*a).max(*b));
                        let pair = [lerp(before.0, now.0, progress) / most, lerp(before.1, now.1, progress) / most];
                        draw_columns(cr, area, &pair, 1.0, ink, false);
                    }
                    _ => draw_columns(cr, area, &[fraction], 0.0, ink, true),
                }
            }
        }
    }

    /// What the module's panel says.
    fn content(&self, module: Module, fahrenheit: bool, progress: f64, address: Option<String>) -> Option<Content> {
        let sample = self.sample.as_ref()?;
        let temperature = |celsius: f64| temperature_text(celsius, fahrenheit, true);
        let fractions = || self.history(module).iter().map(|(value, _)| *value).collect::<Vec<_>>();
        let speeds = || {
            let (first, second) = scaled(&self.history(module));
            (first, Some(second))
        };
        let processes = |heading: &'static str, list: &[(String, f64)], shown: fn(f64) -> String| {
            (!list.is_empty()).then(|| (heading, list.iter().map(|(name, amount)| (name.clone(), shown(*amount))).collect()))
        };
        Some(match module {
            Module::Cpu => {
                let cpu = sample.cpu.as_ref()?;
                let meta = [cpu.mhz.map(|mhz| format!("{:.1} GHz", mhz / 1000.0)), cpu.temperature.map(temperature)]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                Content {
                    fans: Vec::new(),
                    value: percent(self.fraction(module, progress)),
                    second: None,
                    meta,
                    chart: (fractions(), None),
                    rows: named(vec![
                        ("使用者", percent(cpu.user)),
                        ("系統", percent(cpu.system)),
                        ("閒置", percent(1.0 - cpu.usage)),
                        ("平均負載", cpu.load.map(|load| format!("{load:.2}")).join(" · ")),
                        ("運作時間", uptime_text(cpu.uptime)),
                    ]),
                    cores: Some(self.cores(progress)),
                    processes: processes("CPU", &sample.top_cpu, |amount| format!("{amount:.1}%")),
                }
            }
            Module::Gpu => {
                let gpu = sample.gpu.as_ref()?;
                let mut rows = vec![("使用率", percent(gpu.usage))];
                if let Some((used, total)) = gpu.memory {
                    rows.push(("顯示記憶體", format!("{} / {}", size_text(used), size_text(total))));
                }
                if let Some(celsius) = gpu.temperature {
                    rows.push(("溫度", temperature(celsius)));
                }
                if let Some(watts) = gpu.power {
                    rows.push(("功耗", format!("{watts:.0} W")));
                }
                let mut rows = named(rows);
                rows.extend(sample.other_gpus.iter().map(|other| (other.name.clone(), percent(other.usage))));
                Content {
                    fans: Vec::new(),
                    value: percent(self.fraction(module, progress)),
                    second: None,
                    meta: gpu.name.clone(),
                    chart: (fractions(), None),
                    rows,
                    cores: None,
                    processes: None,
                }
            }
            Module::Ram => {
                let ram = sample.ram.as_ref()?;
                let mut rows = vec![
                    ("已使用", size_text(ram.used)),
                    ("快取", size_text(ram.cached)),
                    ("可用", size_text(ram.total - ram.used)),
                ];
                if ram.swap.1 > 0.0 {
                    rows.push(("置換空間", format!("{} / {}", size_text(ram.swap.0), size_text(ram.swap.1))));
                }
                if let Some(pressure) = ram.pressure {
                    rows.push(("記憶體壓力", percent(pressure)));
                }
                Content {
                    fans: Vec::new(),
                    value: percent(self.fraction(module, progress)),
                    second: None,
                    meta: format!("{} / {}", size_text(ram.used), size_text(ram.total)),
                    chart: (fractions(), None),
                    rows: named(rows),
                    cores: None,
                    processes: processes("記憶體", &sample.top_ram, size_text),
                }
            }
            Module::Disk => {
                let disk = sample.disk.as_ref()?;
                Content {
                    fans: Vec::new(),
                    value: percent(self.fraction(module, progress)),
                    second: None,
                    meta: format!("{} / {}", size_text(disk.total - disk.free), size_text(disk.total)),
                    chart: speeds(),
                    rows: named(vec![
                        ("可用", size_text(disk.free)),
                        ("讀取", speed_text(disk.read)),
                        ("寫入", speed_text(disk.write)),
                    ]),
                    cores: None,
                    processes: processes("讀寫", &sample.top_disk, speed_text),
                }
            }
            Module::Network => {
                let net = sample.network.as_ref()?;
                Content {
                    fans: Vec::new(),
                    value: format!("↓ {}", speed_text(net.received)),
                    second: Some(format!("↑ {}", speed_text(net.sent))),
                    meta: net.interface.clone(),
                    chart: speeds(),
                    rows: named(
                        [
                            address.map(|address| ("本機 IP", address)),
                            sample.public_ip.as_ref().map(|(address, country)| {
                                ("公用 IP", if country.is_empty() { address.clone() } else { format!("{address} · {country}") })
                            }),
                            Some(("已下載", size_text(net.received_total))),
                            Some(("已上傳", size_text(net.sent_total))),
                        ]
                        .into_iter()
                        .flatten()
                        .collect(),
                    ),
                    cores: None,
                    processes: (!sample.top_network.is_empty()).then(|| {
                        let shown = sample
                            .top_network
                            .iter()
                            .map(|(name, down, up)| (name.clone(), format!("↓ {} ↑ {}", speed_text(*down), speed_text(*up))))
                            .collect();
                        ("流量", shown)
                    }),
                }
            }
            Module::Sensors => {
                let (name, hottest) = sample.hottest()?;
                let mut rows: Vec<(String, String)> =
                    sample.temperatures.iter().map(|(name, celsius)| (name.clone(), temperature(*celsius))).collect();
                let settable = |fan: &Fan| self.fan_control && fan.pwm.is_some();
                rows.extend(sample.fans.iter().filter(|fan| !settable(fan)).map(|fan| (fan.name.clone(), format!("{:.0} RPM", fan.rpm))));
                let fans = sample
                    .fans
                    .iter()
                    .enumerate()
                    .filter_map(|(index, fan)| {
                        let (_, duty, enable) = fan.pwm.as_ref().filter(|_| settable(fan))?;
                        let duty = self.fan_drag.filter(|(dragged, _)| *dragged == index).map_or(*duty, |(_, level)| level);
                        let manual = *enable == 1;
                        Some((index, fan.name.clone(), format!("{:.0} RPM", fan.rpm), manual, duty, self.fan_reveal(index, manual)))
                    })
                    .collect();
                Content {
                    fans,
                    value: temperature(*hottest),
                    second: None,
                    meta: name.clone(),
                    chart: (fractions(), None),
                    rows,
                    cores: None,
                    processes: None,
                }
            }
        })
    }
}

fn named(rows: Vec<(&str, String)>) -> Vec<(String, String)> {
    rows.into_iter().map(|(name, value)| (name.to_string(), value)).collect()
}

fn label_font() -> pango::FontDescription {
    font("Bold", 6.5)
}

/// A name stood on end beside a line or bars, a letter a line.
fn standing_font() -> pango::FontDescription {
    font("Bold", 6.0)
}

impl Panel {
    /// Under `anchor`, kept on the output.
    fn under(anchor: Rectangle<i32, Logical>, output_width: i32) -> i32 {
        (anchor.loc.x + anchor.size.w / 2 - WIDTH / 2).min(output_width - WIDTH - 6).max(6)
    }

    fn new(view: View, x: i32) -> Self {
        Self {
            view,
            x,
            opened: Instant::now(),
            closed: None,
            hovered: None,
            pressed: None,
            parts: Vec::new(),
            shown: None,
            picture: None,
            id: Id::new(),
        }
    }

    fn draw(&mut self, module: Module, content: &Content, shown: &Shown, slide: f64) {
        let colors = colors(shown.dark);
        let inner = WIDTH - PADDING * 2;
        let small = font("", 9.0);
        let label = font("", 10.0);
        let big = font("600", 20.0);

        // Measured first: each well's place.
        let mut y = PADDING;
        let hero = rect(PADDING, y, inner, TITLE_ROW + 32 + HERO_CHART + PADDING);
        y += hero.size.h + GAP;
        let details = (!content.rows.is_empty()).then(|| {
            let well = rect(PADDING, y, inner, INSET * 2 + DETAIL_ROW * content.rows.len() as i32);
            y += well.size.h + GAP;
            well
        });
        let cores = content.cores.as_ref().map(|_| {
            let well = rect(PADDING, y, inner, TITLE_ROW + CORES_HEIGHT + PADDING);
            y += well.size.h + GAP;
            well
        });
        let slider_height = |reveal: f64| (f64::from(LIST_ROW) * reveal).round() as i32;
        let fan_rows: i32 = content.fans.iter().map(|fan| LIST_ROW + slider_height(fan.5)).sum();
        let fans = (!content.fans.is_empty()).then(|| {
            let well = rect(PADDING, y, inner, TITLE_ROW + fan_rows + INSET);
            y += well.size.h + GAP;
            well
        });
        self.parts.retain(|(part, _)| !matches!(part, Part::FanMode(_) | Part::FanSpeed(_)));
        if let Some(well) = fans {
            let mut row_y = well.loc.y + TITLE_ROW;
            for (index, .., reveal) in &content.fans {
                let row = |at: i32, height: i32| rect(well.loc.x + INSET, at, inner - INSET * 2, height);
                self.parts.push((Part::FanMode(*index), row(row_y, LIST_ROW)));
                row_y += LIST_ROW;
                let height = slider_height(*reveal);
                if height > 0 {
                    self.parts.push((Part::FanSpeed(*index), row(row_y, height)));
                    row_y += height;
                }
            }
        }
        let processes = content.processes.as_ref().map(|(_, rows)| {
            let well = rect(PADDING, y, inner, TITLE_ROW + DETAIL_ROW * rows.len() as i32 + INSET);
            y += well.size.h + GAP;
            well
        });
        let (footer, height) = self.footer_at(y);
        let Some((surface, cr)) = draw_panel(WIDTH, height, shown.dark, &colors) else {
            return;
        };
        let text_left = |well: Rectangle<i32, Logical>| f64::from(well.loc.x + PADDING);
        let text_right = |well: Rectangle<i32, Logical>| f64::from(well.loc.x + well.size.w - PADDING);
        let title = |well: Rectangle<i32, Logical>, left: &str, right: &str| {
            let center = f64::from(well.loc.y + 18);
            show_text(&cr, left, &small, text_left(well), center, colors.ink_3);
            show_right(&cr, right, &small, (text_right(well), center), colors.ink_3);
        };

        draw_well(&cr, &colors, hero, 8.0);
        title(hero, module.title(), &content.meta);
        let value_center = f64::from(hero.loc.y + TITLE_ROW + 14);
        show_text(&cr, &content.value, &big, text_left(hero), value_center, colors.ink);
        if let Some(second) = &content.second {
            let at = text_left(hero) + f64::from(text_size(&content.value, &big).0) + 12.0;
            show_text(&cr, second, &label, at, value_center + 3.0, colors.ink_3);
        }
        let chart = rect(hero.loc.x + PADDING, hero.loc.y + TITLE_ROW + 32, inner - PADDING * 2, HERO_CHART);
        let (x, cy, w, h) = to_f64(chart);
        text::rounded_rectangle(&cr, x, cy, w, h, 6.0);
        cr.set_source_rgba(colors.ink.0, colors.ink.1, colors.ink.2, 0.05);
        let _ = cr.fill();
        let area = (x, cy + 2.0, w, h - 2.0);
        if let Some(second) = &content.chart.1 {
            draw_line(&cr, area, second, HISTORY, slide, colors.ink_3, 1.5, false);
        }
        draw_line(&cr, area, &content.chart.0, HISTORY, slide, colors.ink, 1.5, true);

        let detail = |well: Rectangle<i32, Logical>, top: i32, rows: &mut dyn Iterator<Item = (&str, &str)>| {
            for (index, (name, value)) in rows.enumerate() {
                let center = f64::from(well.loc.y + top + DETAIL_ROW * index as i32 + DETAIL_ROW / 2);
                let value_width = f64::from(text_size(value, &label).0);
                show_line(&cr, name, &label, (text_left(well), center, f64::from(inner - PADDING * 2) - value_width - 8.0), colors.ink);
                show_right(&cr, value, &label, (text_right(well), center), colors.ink_3);
            }
        };
        if let Some(well) = details {
            draw_well(&cr, &colors, well, 8.0);
            detail(well, INSET, &mut content.rows.iter().map(|(name, value)| (name.as_str(), value.as_str())));
        }
        if let (Some(well), Some(values)) = (cores, &content.cores) {
            draw_well(&cr, &colors, well, 8.0);
            title(well, "核心", &values.len().to_string());
            let area = rect(well.loc.x + PADDING, well.loc.y + TITLE_ROW, inner - PADDING * 2, CORES_HEIGHT);
            draw_columns(&cr, to_f64(area), values, 2.0, colors.ink, true);
        }
        if let Some(well) = fans {
            draw_well(&cr, &colors, well, 8.0);
            title(well, "風扇", "");
            for (part, area) in &self.parts {
                let (index, center) = (match part {
                    Part::FanMode(index) | Part::FanSpeed(index) => *index,
                    _ => continue,
                }, f64::from(area.loc.y + area.size.h / 2));
                let Some((_, name, speed, manual, duty, reveal)) = content.fans.iter().find(|fan| fan.0 == index) else {
                    continue;
                };
                let left = f64::from(area.loc.x + PADDING - INSET);
                let right = f64::from(area.loc.x + area.size.w - PADDING + INSET);
                if let Part::FanSpeed(_) = part {
                    // Grown open from under its fan's row, clipped to how far it has come and
                    // fading in with it.
                    let (x, top, w, h) = to_f64(*area);
                    let center = top + f64::from(LIST_ROW) / 2.0 - f64::from(LIST_ROW) * (1.0 - reveal) / 2.0;
                    cr.save().ok();
                    cr.rectangle(x, top, w, h);
                    cr.clip();
                    cr.push_group();
                    let value = percent(*duty);
                    let value_width = f64::from(text_size("100%", &label).0);
                    show_right(&cr, &value, &label, (right, center), colors.ink_3);
                    let track = right - value_width - 12.0 - f64::from(FAN_KNOB) - left - f64::from(FAN_KNOB);
                    draw_slider(&cr, &colors, shown.dark, (left + f64::from(FAN_KNOB), center, track), *duty);
                    let _ = cr.pop_group_to_source();
                    let _ = cr.paint_with_alpha(*reveal);
                    cr.restore().ok();
                    continue;
                }
                let color = draw_list_row(&cr, &colors, *area, shown.hovered == Some(*part));
                show_text(&cr, name, &label, left, center, color);
                let mode = if *manual { "手動" } else { "自動" };
                show_right(&cr, &format!("{speed} · {mode}"), &label, (right, center), color);
            }
        }
        if let (Some(well), Some((heading, rows))) = (processes, &content.processes) {
            draw_well(&cr, &colors, well, 8.0);
            title(well, "程序", heading);
            detail(well, TITLE_ROW, &mut rows.iter().map(|(name, value)| (name.as_str(), value.as_str())));
        }

        self.draw_footer(&cr, footer, shown);
        drop(cr);
        self.picture = text::picture(surface);
    }

    /// The footer's well at `y`, its rows put among the parts, and the panel's height with it.
    fn footer_at(&mut self, y: i32) -> (Rectangle<i32, Logical>, i32) {
        let inner = WIDTH - PADDING * 2;
        let footer = rect(PADDING, y, inner, INSET * 2 + LIST_ROW * 2);
        let row = |index: i32| rect(PADDING + INSET, y + INSET + LIST_ROW * index, inner - INSET * 2, LIST_ROW);
        self.parts.retain(|(part, _)| matches!(part, Part::Row(_) | Part::FanMode(_) | Part::FanSpeed(_)));
        self.parts.extend([(Part::Tasks, row(0)), (Part::Settings, row(1))]);
        (footer, y + footer.size.h + PADDING)
    }

    fn draw_footer(&self, cr: &Context, footer: Rectangle<i32, Logical>, shown: &Shown) {
        let colors = colors(shown.dark);
        let label = font("", 10.0);
        draw_well(cr, &colors, footer, 8.0);
        for (part, area) in &self.parts {
            let text = match part {
                Part::Tasks => "開啟工作管理員",
                Part::Settings => "系統監控設定⋯",
                Part::Row(_) | Part::FanMode(_) | Part::FanSpeed(_) => continue,
            };
            let color = draw_list_row(cr, &colors, *area, shown.hovered == Some(*part));
            let center = f64::from(area.loc.y + area.size.h / 2);
            show_text(cr, text, &label, f64::from(area.loc.x + PADDING - INSET), center, color);
        }
    }

    /// The panel of all the readings: a row each, with its value and its last minute, which
    /// opens its own panel.
    fn draw_summary(&mut self, stats: &Stats, modules: &[Module], shown: &Shown, (progress, fahrenheit): (f64, bool)) {
        let colors = colors(shown.dark);
        let inner = WIDTH - PADDING * 2;
        let label = font("", 10.0);
        let small = font("", 9.0);
        let well = rect(PADDING, PADDING, inner, INSET * 2 + SUMMARY_ROW * modules.len() as i32);
        self.parts = modules
            .iter()
            .enumerate()
            .map(|(index, module)| {
                (Part::Row(*module), rect(PADDING + INSET, PADDING + INSET + SUMMARY_ROW * index as i32, inner - INSET * 2, SUMMARY_ROW))
            })
            .collect();
        let (footer, height) = self.footer_at(well.loc.y + well.size.h + GAP);
        let Some((surface, cr)) = draw_panel(WIDTH, height, shown.dark, &colors) else {
            return;
        };
        draw_well(&cr, &colors, well, 8.0);
        for (part, area) in &self.parts {
            let Part::Row(module) = *part else {
                continue;
            };
            let color = draw_list_row(&cr, &colors, *area, shown.hovered == Some(*part));
            let (x, y, w, h) = to_f64(*area);
            let center = y + h / 2.0;
            let text_x = x + f64::from(PADDING - INSET);
            show_text(&cr, module.title(), &label, text_x, center - 8.0, color);
            let value = match (module, stats.sample.as_ref()) {
                (Module::Network, Some(sample)) => sample
                    .network
                    .as_ref()
                    .map(|net| format!("↓ {} ↑ {}", speed_text(net.received), speed_text(net.sent)))
                    .unwrap_or_default(),
                (Module::Sensors, Some(sample)) => {
                    sample.hottest().map(|(_, celsius)| temperature_text(*celsius, fahrenheit, true)).unwrap_or_default()
                }
                _ => percent(stats.fraction(module, progress)),
            };
            cr.push_group();
            show_text(&cr, &value, &small, text_x, center + 9.0, color);
            let _ = cr.pop_group_to_source();
            let _ = cr.paint_with_alpha(0.7);
            let chevron = x + w - 14.0;
            draw_chevron(&cr, chevron, center, 0.0, color);
            let (chart_w, chart_h) = (f64::from(SUMMARY_CHART.0), f64::from(SUMMARY_CHART.1));
            let chart = (chevron - 10.0 - chart_w, center - chart_h / 2.0, chart_w, chart_h);
            let history = stats.history(module);
            match module {
                Module::Network | Module::Disk => {
                    let (first, second) = scaled(&history);
                    cr.push_group();
                    draw_line(&cr, chart, &second, HISTORY, progress, color, 1.0, false);
                    let _ = cr.pop_group_to_source();
                    let _ = cr.paint_with_alpha(0.5);
                    draw_line(&cr, chart, &first, HISTORY, progress, color, 1.0, true);
                }
                _ => {
                    let values: Vec<f64> = history.iter().map(|(value, _)| *value).collect();
                    draw_line(&cr, chart, &values, HISTORY, progress, color, 1.0, true);
                }
            }
        }
        self.draw_footer(&cr, footer, shown);
        drop(cr);
        self.picture = text::picture(surface);
    }
}

impl Nekoland {
    /// Passes the settings to the reading thread, takes in what it read, and draws the open
    /// panel again when anything it shows changed.
    pub(crate) fn refresh_stats(&mut self) {
        let config = &self.config;
        let wanted = Module::ALL.into_iter().filter(|module| module.enabled(config)).fold(0, |bits, module| bits | module.bit());
        WANTED.store(wanted, Ordering::Relaxed);
        INTERVAL.store(config.stats_interval, Ordering::Relaxed);
        PROCESSES.store(config.stats_processes, Ordering::Relaxed);
        PUBLIC_IP.store(config.stats_public_ip, Ordering::Relaxed);
        let combined = config.stats_combined || !config.stats_text.is_empty();
        let animations = config.animations;
        let fahrenheit = config.stats_fahrenheit;
        let fan_control = config.stats_fan_control;
        let stats = &mut self.menu_bar.stats;
        stats.fan_control = fan_control;
        stats.animations = animations;
        if wanted != 0 && stats.found.is_none() {
            stats.found = spawn_reader();
        }
        if wanted != stats.wanted {
            // What is turned off starts again from nothing when it is turned back on.
            stats.history.retain(|module, _| wanted & module.bit() != 0);
            stats.wanted = wanted;
        }
        let found: Vec<Sample> = stats.found.as_ref().map(|found| found.try_iter().collect()).unwrap_or_default();
        let mut told = Vec::new();
        for sample in found {
            stats.take(sample);
            told.extend(stats.alerts(&self.config));
        }
        for (title, body) in told {
            crate::clockbar::notify(ALERT_APP, ALERT_ICON, &title, &body);
        }
        let stats = &mut self.menu_bar.stats;
        if stats.closing.as_ref().is_some_and(|panel| panel.fade() <= 0.0) {
            stats.closing = None;
        }
        let gone = stats.panel.as_ref().is_some_and(|panel| match panel.view {
            View::Module(module) => wanted & module.bit() == 0,
            View::Summary => !combined || wanted == 0,
        });
        if gone {
            self.dismiss_stats_panel();
        }
        let modules = self.menu_bar.stats.shown(&self.config);
        let address = self.menu_bar.network.as_ref().and_then(|network| network.address).map(|address| address.to_string());
        let dark = self.menu_bar.dark;
        let stats = &mut self.menu_bar.stats;
        let progress = stats.progress(animations);
        let Some(panel) = stats.panel.as_ref() else {
            return;
        };
        let shown = Shown {
            count: stats.count,
            slide: stats.settling(animations),
            dark,
            hovered: panel.hovered,
            fahrenheit,
            fan_drag: stats.fan_drag.map(|(index, level)| (index, (level * 1000.0) as u32)),
            fan_reveal: stats.fan_reveals(),
        };
        if panel.shown.as_ref() == Some(&shown) && panel.picture.is_some() {
            return;
        }
        let Some(mut panel) = stats.panel.take() else {
            return;
        };
        match panel.view {
            View::Module(module) => {
                if let Some(content) = stats.content(module, fahrenheit, progress, address) {
                    panel.draw(module, &content, &shown, progress);
                }
            }
            View::Summary => panel.draw_summary(stats, &modules, &shown, (progress, fahrenheit)),
        }
        panel.shown = Some(shown);
        stats.panel = Some(panel);
    }

    /// Opens a panel under its place in the bar, or closes it if it is open; the one open
    /// fades out as the other fades in.
    pub(crate) fn toggle_stats_panel(&mut self, view: View, anchor: Rectangle<i32, Logical>) {
        let same = self.menu_bar.stats.panel_view() == Some(view);
        self.dismiss_stats_panel();
        if same {
            return;
        }
        let width = self.bar_area().map_or(0, |area| area.size.w);
        self.menu_bar.stats.panel = Some(Panel::new(view, Panel::under(anchor, width)));
        self.refresh_stats();
    }

    /// Fades the open panel out.
    pub(crate) fn dismiss_stats_panel(&mut self) {
        if let Some(mut panel) = self.menu_bar.stats.panel.take() {
            panel.closed = Some(Instant::now());
            panel.hovered = None;
            self.menu_bar.stats.closing = Some(panel);
        }
    }

    /// What the open panel shows.
    pub(crate) fn stats_panel_view(&self) -> Option<View> {
        self.menu_bar.stats.panel_view()
    }

    /// The panels on screen, the one open over the one fading out.
    pub(crate) fn stats_overlays(&self) -> Vec<Overlay<'_>> {
        let Some(output) = self.bar_area() else {
            return Vec::new();
        };
        let stats = &self.menu_bar.stats;
        [stats.panel.as_ref(), stats.closing.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|panel| {
                let picture = panel.picture.as_ref()?;
                Some(Overlay {
                    id: &panel.id,
                    rect: Rectangle::new(output.loc + Point::from((panel.x, MENU_BAR_HEIGHT + GAP_BELOW_BAR)), picture.size),
                    glass: Some((Tier::Menu, MENU_RADIUS)),
                    alpha: 1.0,
                    fade: panel.fade() as f32,
                    picture,
                })
            })
            .collect()
    }

    /// Where a point of the output is on the open panel, if it is on it.
    fn stats_local(&self, local: Point<i32, Logical>) -> Option<Point<i32, Logical>> {
        let panel = self.menu_bar.stats.panel.as_ref()?;
        let at = local - Point::from((panel.x, MENU_BAR_HEIGHT + GAP_BELOW_BAR));
        Rectangle::new((0, 0).into(), panel.picture.as_ref()?.size).contains(at).then_some(at)
    }

    /// A press on the output, at `local`. Returns whether it was on the panel.
    pub(crate) fn stats_press(&mut self, local: Point<i32, Logical>) -> bool {
        let Some(at) = self.stats_local(local) else {
            return false;
        };
        let stats = &mut self.menu_bar.stats;
        if let Some(panel) = stats.panel.as_mut() {
            panel.pressed = panel.part_at(at);
            if let Some(Part::FanSpeed(index)) = panel.pressed {
                stats.fan_drag = Some((index, panel.fan_level(index, at)));
            }
        }
        true
    }

    pub(crate) fn stats_motion(&mut self, local: Point<i32, Logical>) {
        let at = self.stats_local(local);
        let stats = &mut self.menu_bar.stats;
        if let Some(panel) = stats.panel.as_mut() {
            panel.hovered = at.and_then(|at| panel.part_at(at));
            if let (Some(Part::FanSpeed(index)), Some(at)) = (panel.pressed, at) {
                stats.fan_drag = Some((index, panel.fan_level(index, at)));
            }
        }
    }

    /// Sets a fan as the panel asks: driven at `level`, or given back to its chip or set by
    /// hand at the speed it runs at.
    fn set_fan(&mut self, index: usize, level: Option<f64>) {
        let stats = &mut self.menu_bar.stats;
        let Some(fan) = stats.sample.as_mut().and_then(|sample| sample.fans.get_mut(index)) else {
            return;
        };
        let Some((pwm, duty, enable)) = fan.pwm.as_mut() else {
            return;
        };
        let path = pwm.display().to_string();
        let command = match level {
            Some(level) => {
                *duty = level;
                *enable = 1;
                fan_command("speed", &path, (level * 255.0).round() as u32)
            }
            None if *enable == 1 => {
                *enable = stats.automatic.get(pwm).copied().unwrap_or(FAN_AUTOMATIC);
                fan_command("mode", &path, u32::from(*enable))
            }
            None => {
                stats.automatic.insert(pwm.clone(), *enable);
                *enable = 1;
                *duty = duty.max(LEAST_FAN);
                fan_command("speed", &path, (*duty * 255.0).round() as u32)
            }
        };
        let manual = *enable == 1;
        if stats.fan_reveal.get(&index).is_none_or(|(shown, _)| *shown != manual) && fan.pwm.is_some() {
            stats.fan_reveal.insert(index, (manual, Instant::now()));
        }
        config::spawn(&command);
        // Drawn again at once as asked, the next reading saying whether it took.
        if let Some(panel) = stats.panel.as_mut() {
            panel.shown = None;
        }
    }

    pub(crate) fn stats_release(&mut self, local: Point<i32, Logical>) {
        let under = self.stats_local(local);
        let Some(panel) = self.menu_bar.stats.panel.as_mut() else {
            return;
        };
        let pressed = panel.pressed.take();
        if let Some(Part::FanSpeed(index)) = pressed {
            let level = self.menu_bar.stats.fan_drag.take().map(|(_, level)| level);
            self.set_fan(index, level);
            return;
        }
        let part = pressed.filter(|part| under.and_then(|at| panel.part_at(at)) == Some(*part));
        match part {
            Some(Part::FanMode(index)) => {
                self.set_fan(index, None);
                return;
            }
            Some(Part::FanSpeed(_)) => return,
            // The reading's own panel takes the place of them all's, fading in over it.
            Some(Part::Row(module)) => {
                let x = panel.x;
                self.dismiss_stats_panel();
                self.menu_bar.stats.panel = Some(Panel::new(View::Module(module), x));
                self.refresh_stats();
                return;
            }
            Some(Part::Tasks) => config::spawn(TASKS_COMMAND),
            Some(Part::Settings) => config::spawn(&format!("NEKOLAND_SETTINGS_PAGE={SETTINGS_PAGE} nekoland-settings")),
            None => return,
        }
        self.dismiss_stats_panel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_share_counts_what_is_not_idle() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0 0 0\nintr 1";
        let before = parse_cpu_times(stat);
        assert_eq!(before, vec![[100, 50, 850, 1000]; 2]);
        let now = parse_cpu_times("cpu  160 0 70 900 70 0 0 0 0 0\n");
        let (usage, user, system) = cpu_share(&before[0], &now[0]);
        assert!((usage - 0.4).abs() < 1e-9);
        assert!((user - 0.3).abs() < 1e-9);
        assert!((system - 0.1).abs() < 1e-9);
        assert_eq!(cpu_share(&now[0], &now[0]), (0.0, 0.0, 0.0));
    }

    #[test]
    fn disks_are_counted_without_their_partitions() {
        let diskstats = "259 0 nvme0n1 10 0 200 0 5 0 100 0\n259 1 nvme0n1p1 10 0 200 0 5 0 100 0\n7 0 loop0 1 0 8 0 0 0 0 0";
        assert_eq!(parse_disk_sectors(diskstats, |name| name == "nvme0n1"), (200, 100));
    }

    #[test]
    fn default_route_names_its_interface() {
        let route = "Iface\tDestination\tGateway\nwg0\t0000080A\t00000000\nenp5s0\t00000000\t0101A8C0\n";
        assert_eq!(default_interface(route).as_deref(), Some("enp5s0"));
        assert_eq!(default_interface("Iface\tDestination\n"), None);
    }

    #[test]
    fn nvidia_smi_line_is_read() {
        let gpu = parse_nvidia("NVIDIA GeForce RTX 3060, 37, 1024, 12288, 51, 35.20").unwrap();
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 3060");
        assert!((gpu.usage - 0.37).abs() < 1e-9);
        assert_eq!(gpu.memory, Some((1024.0 * 1048576.0, 12288.0 * 1048576.0)));
        assert_eq!((gpu.temperature, gpu.power), (Some(51.0), Some(35.2)));
        // Without a power reading, as on some cards.
        assert_eq!(parse_nvidia("Tesla, 5, 1, 2, 40, [N/A]").unwrap().power, None);
        assert!(parse_nvidia("").is_none());
    }

    #[test]
    fn sizes_read_as_people_write_them() {
        assert_eq!(size_text(512.0), "512 B");
        assert_eq!(size_text(1536.0), "1.5 KB");
        assert_eq!(size_text(999.0 * 1024.0), "999 KB");
        assert_eq!(size_text(3.0 * 1024.0 * 1024.0 * 1024.0), "3.0 GB");
        assert_eq!(temperature_text(100.0, true, true), "212 °F");
        assert_eq!(uptime_text(90_061), "1 天 1 小時");
    }

    #[test]
    fn an_alert_is_told_once_after_two_readings_past_it() {
        let mut alerts = Alerts::default();
        assert!(!alerts.check(Module::Cpu, Some(true)));
        assert!(alerts.check(Module::Cpu, Some(true)));
        assert!(!alerts.check(Module::Cpu, Some(true)));
        // Back under, then past again: told again.
        assert!(!alerts.check(Module::Cpu, Some(false)));
        assert!(!alerts.check(Module::Cpu, Some(true)));
        assert!(alerts.check(Module::Cpu, Some(true)));
        // Without a threshold, never.
        assert!(!alerts.check(Module::Ram, None));
        assert!(!alerts.check(Module::Ram, None));
    }

    #[test]
    fn intel_clients_are_read_from_fdinfo() {
        let fdinfo = "drm-driver:\ti915\ndrm-client-id:\t1149\ndrm-engine-render:\t2000 ns\ndrm-engine-video:\t500 ns\ndrm-engine-capacity-video:\t2\n";
        let (client, engines) = parse_fdinfo(fdinfo).unwrap();
        assert_eq!(client, "1149");
        assert_eq!(engines, vec![("render".to_string(), 2000, 1), ("video".to_string(), 500, 2)]);
        assert!(parse_fdinfo("drm-driver:\tamdgpu\ndrm-client-id:\t1\n").is_none());
    }

    #[test]
    fn sockets_are_read_with_their_processes() {
        let ss = "ESTAB 0 0 10.0.0.2:49178 10.1.0.1:445 users:((\"gvfsd-smb\",pid=1,fd=9))\n\t cubic bytes_sent:932242 bytes_acked:932243 bytes_received:2362735 segs_out:7\nESTAB 0 0 172.16.0.2:44214 91.189.95.56:443\n\t cubic bytes_sent:66569 bytes_received:5972\n";
        assert_eq!(
            parse_ss(ss),
            vec![("gvfsd-smb".to_string(), "10.0.0.2:49178 10.1.0.1:445".to_string(), 932242, 2362735)]
        );
    }

    #[test]
    fn public_address_is_read_with_its_country() {
        assert_eq!(parse_public_ip(r#"{"ipv4":"1.2.3.4","country":"TW"}"#), Some(("1.2.3.4".into(), "TW".into())));
        assert_eq!(parse_public_ip(r#"{"ipv6":"::1"}"#), Some(("::1".into(), String::new())));
        assert_eq!(parse_public_ip("<html>"), None);
    }

    #[test]
    fn fans_are_read_with_what_sets_them() {
        let hwmon = std::env::temp_dir().join(format!("nekoland-hwmon-{}", std::process::id()));
        let chip = hwmon.join("hwmon3");
        std::fs::create_dir_all(&chip).unwrap();
        for (file, value) in [("name", "nct6798"), ("fan1_input", "1200"), ("pwm1", "128"), ("pwm1_enable", "5"), ("fan2_input", "800")] {
            std::fs::write(chip.join(file), format!("{value}\n")).unwrap();
        }
        let (_, fans) = read_sensors(&hwmon);
        std::fs::remove_dir_all(&hwmon).unwrap();
        assert_eq!(fans.len(), 2);
        assert_eq!((fans[0].name.as_str(), fans[0].rpm), ("風扇 1", 1200.0));
        assert_eq!(fans[0].pwm, Some((chip.join("pwm1"), 128.0 / 255.0, 5)));
        assert_eq!(fans[1].pwm, None);
        assert_eq!(fan_command("speed", "/sys/class/hwmon/hwmon3/pwm1", 128), format!("pkexec {FAN_HELPER} speed /sys/class/hwmon/hwmon3/pwm1 128"));
    }

    #[test]
    fn text_has_the_readings_in_it() {
        let sample = Sample {
            cpu: Some(Cpu { usage: 0.42, ..Default::default() }),
            temperatures: vec![("CPU".into(), 61.0)],
            ..Default::default()
        };
        assert_eq!(fill_text("C $cpu G $gpu $temp", &sample, false), "C 42% G – 61°");
    }

    #[test]
    fn processes_of_one_name_are_taken_together() {
        let amounts = vec![("Discord".into(), 1.0), ("bun".into(), 2.5), ("Discord".into(), 2.0), ("sh".into(), 0.5)];
        assert_eq!(busiest(amounts, 2), vec![("Discord".to_string(), 3.0), ("bun".to_string(), 2.5)]);
    }

    #[test]
    fn many_cores_are_shown_in_groups() {
        assert_eq!(grouped_cores(&[0.0, 1.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(grouped_cores(&[0.2; 3], 16).len(), 3);
    }

    #[test]
    fn a_new_reading_joins_the_history() {
        let mut stats = Stats::default();
        let sample = |usage| Sample { cpu: Some(Cpu { usage, ..Default::default() }), ..Default::default() };
        stats.take(sample(0.2));
        stats.take(sample(0.6));
        assert_eq!(stats.history(Module::Cpu), vec![(0.2, 0.0), (0.6, 0.0)]);
        assert!((stats.fraction(Module::Cpu, 0.5) - 0.4).abs() < 1e-9);
        assert!(stats.history(Module::Gpu).is_empty());
        for _ in 0..HISTORY * 2 {
            stats.take(sample(0.1));
        }
        assert_eq!(stats.history(Module::Cpu).len(), HISTORY + 1);
    }
}
