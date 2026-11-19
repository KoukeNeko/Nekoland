//! What the disks program does to a disk: each is a job of commands, run in a thread, some
//! as root through `sudo -A` with Nekoland's askpass asking for the password.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
};

use regex::Regex;
use serde_json::Value;

use super::model::{Disk, Kind, Part};

/// The file systems a partition can be made with: the name `mkfs` knows, and the name shown.
pub const FILE_SYSTEMS: [(&str, &str); 7] = [
    ("ext4", "ext4"),
    ("btrfs", "btrfs"),
    ("xfs", "xfs"),
    ("exfat", "exFAT"),
    ("vfat", "FAT32"),
    ("ntfs", "NTFS"),
    ("swap", "swap"),
];

pub enum Event {
    /// How far it is, 0 to 1, or None when that is not known.
    Progress(Option<f64>),
    /// What it said when it worked, or why it did not.
    Done(Result<String, String>),
}

/// One command.
#[derive(Clone, Default)]
pub struct Step {
    pub argv: Vec<String>,
    pub root: bool,
    /// The bytes a `dd` writes, to show how far it is.
    pub total: Option<u64>,
    /// Run when this is not allowed to the user, as root.
    pub fallback: Option<Box<Step>>,
    /// Go on when it fails.
    pub tolerate: bool,
    /// A failing exit says something too, as smartctl's does; what it printed is taken.
    pub lenient: bool,
}

impl Step {
    pub fn user(argv: &[&str]) -> Step {
        Step { argv: argv.iter().map(|arg| arg.to_string()).collect(), ..Step::default() }
    }

    pub fn root(argv: &[&str]) -> Step {
        Step { root: true, ..Step::user(argv) }
    }

    /// Run as the user, or as root where the user may not.
    pub fn either(argv: &[&str], root: &[&str]) -> Step {
        Step { fallback: Some(Box::new(Step::root(root))), ..Step::user(argv) }
    }

    pub fn tolerated(mut self) -> Step {
        self.tolerate = true;
        self
    }

    pub fn of_bytes(mut self, total: u64) -> Step {
        self.total = Some(total);
        self
    }

    pub fn lenient(mut self) -> Step {
        self.lenient = true;
        self
    }
}

pub enum Action {
    Run(Step),
    /// Finds the partition a table just got, which is what `{new}` in the steps after stands for.
    FindNew { disk: String, before: Vec<String> },
}

pub struct Job {
    /// Says why the password is asked.
    pub reason: String,
    pub actions: Vec<Action>,
}

impl Job {
    fn new(reason: &str, steps: Vec<Step>) -> Job {
        Job { reason: reason.into(), actions: steps.into_iter().map(Action::Run).collect() }
    }
}

#[derive(Clone, Default)]
pub struct Cancel {
    asked: Arc<AtomicBool>,
    pid: Arc<AtomicI32>,
}

impl Cancel {
    pub fn cancel(&self) {
        self.asked.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 0 {
            // sudo passes the signal to what it runs.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }

    fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

/// Runs `job` in a thread, telling `events` how it goes.
pub fn spawn(job: Job) -> (async_channel::Receiver<Event>, Cancel) {
    let (sender, receiver) = async_channel::unbounded();
    let cancel = Cancel::default();
    let running = cancel.clone();
    std::thread::spawn(move || {
        let done = run(&job, &running, &sender);
        let _ = sender.send_blocking(Event::Done(done));
    });
    (receiver, cancel)
}

fn auth_failed(said: &str) -> bool {
    let said = said.to_lowercase();
    ["not authorized", "notauthorized", "polkit", "authentication", "permission denied", "must be root"]
        .iter()
        .any(|word| said.contains(word))
}

fn run(job: &Job, cancel: &Cancel, events: &async_channel::Sender<Event>) -> Result<String, String> {
    let mut said = String::new();
    let mut found = String::new();
    for action in &job.actions {
        if cancel.asked() {
            return Err("已取消".into());
        }
        let step = match action {
            Action::FindNew { disk, before } => {
                found = new_part(disk, before)?;
                continue;
            }
            Action::Run(step) => step,
        };
        let mut step = step.clone();
        for arg in &mut step.argv {
            *arg = arg.replace("{new}", &found);
        }
        let mut result = run_step(&step, &job.reason, cancel, events);
        if let (Err(why), Some(fallback)) = (&result, &step.fallback) {
            if auth_failed(why) {
                result = run_step(fallback, &job.reason, cancel, events);
            }
        }
        match result {
            Ok(out) => said.push_str(&out),
            Err(_) if step.tolerate && !cancel.asked() => {}
            Err(why) => return Err(why),
        }
    }
    Ok(said)
}

/// The partition `disk` has that it did not before.
fn new_part(disk: &str, before: &[String]) -> Result<String, String> {
    let _ = Command::new("udevadm").arg("settle").status();
    for _ in 0..20 {
        let output = Command::new("lsblk").args(["-nrpo", "NAME", disk]).output().map_err(|err| err.to_string())?;
        let now = String::from_utf8_lossy(&output.stdout).to_string();
        if let Some(new) = now.lines().find(|line| *line != disk && !before.iter().any(|old| old == line)) {
            return Ok(new.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err("找不到新的分割區".into())
}

fn run_step(step: &Step, reason: &str, cancel: &Cancel, events: &async_channel::Sender<Event>) -> Result<String, String> {
    let mut command = if step.root {
        let mut command = Command::new("sudo");
        command.args(["-A", "--"]).args(&step.argv);
        command.env("SUDO_ASKPASS", program("nekoland-askpass"));
        command.env("NEKOLAND_ASKPASS_REASON", reason);
        command
    } else {
        let mut command = Command::new(&step.argv[0]);
        command.args(&step.argv[1..]);
        command
    };
    let mut child = command
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{}: {err}", step.argv[0]))?;
    cancel.pid.store(child.id() as i32, Ordering::SeqCst);
    let mut stdout = child.stdout.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let mut stderr = child.stderr.take().expect("piped");
    let (mut said, mut line) = (String::new(), Vec::new());
    let mut chunk = [0u8; 4096];
    loop {
        let read = match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &chunk[..read] {
            if byte == b'\r' || byte == b'\n' {
                said.push_str(&String::from_utf8_lossy(&line));
                said.push('\n');
                if let (Some(total), Some(done)) = (step.total, written(&line)) {
                    let _ = events.send_blocking(Event::Progress(Some((done as f64 / total as f64).min(1.0))));
                }
                line.clear();
            } else {
                line.push(byte);
            }
        }
    }
    said.push_str(&String::from_utf8_lossy(&line));
    let status = child.wait().map_err(|err| err.to_string())?;
    cancel.pid.store(0, Ordering::SeqCst);
    let out = out.join().unwrap_or_default();
    if cancel.asked() {
        return Err("已取消".into());
    }
    if !status.success() && !(step.lenient && !out.trim().is_empty()) {
        return Err(why_failed(&said, &out, step));
    }
    Ok(format!("{out}{said}"))
}

/// The bytes a `dd` progress line says it has written.
fn written(line: &[u8]) -> Option<u64> {
    let line = String::from_utf8_lossy(line);
    let (count, rest) = line.trim().split_once(' ')?;
    rest.starts_with("bytes").then(|| count.parse().ok()).flatten()
}

fn why_failed(said: &str, out: &str, step: &Step) -> String {
    let lower = said.to_lowercase();
    if step.root && (lower.contains("no password") || lower.contains("incorrect password") || lower.contains("a password is required")) {
        return "沒有輸入管理員密碼".into();
    }
    let tail: Vec<&str> = said.lines().chain(out.lines()).map(str::trim).filter(|line| !line.is_empty()).collect();
    if tail.is_empty() {
        format!("{} 失敗", step.argv.first().map(String::as_str).unwrap_or_default())
    } else {
        tail[tail.len().saturating_sub(3)..].join("\n")
    }
}

/// A program of Nekoland's, beside this one or on the path.
fn program(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
        .or_else(|| which(name))
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin").join(name))
}

/// Where `name` is, on the path or where the system's tools are kept.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let found = path
        .split(':')
        .chain(["/usr/sbin", "/sbin", "/usr/local/sbin"])
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file());
    found
}

/// The programs using what is mounted at `point`, by name, as far as this user can see.
pub fn users_of(point: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else { return names };
    let inside = |path: &Path| path.starts_with(point);
    for entry in procs.flatten() {
        let pid = entry.path();
        let Some(Ok(_)) = pid.file_name().map(|name| name.to_string_lossy().parse::<u32>()) else { continue };
        let using = std::fs::read_link(pid.join("cwd")).is_ok_and(|path| inside(&path))
            || std::fs::read_dir(pid.join("fd")).is_ok_and(|fds| fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|path| inside(&path))));
        if using {
            if let Ok(name) = std::fs::read_to_string(pid.join("comm")) {
                let name = name.trim().to_string();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names.truncate(4);
    names
}

pub fn mkfs_argv(fstype: &str, label: &str, dev: &str) -> Vec<String> {
    let label = label.trim();
    let mut argv: Vec<String> = match fstype {
        "ext4" => vec!["mkfs.ext4".into(), "-F".into(), "-q".into()],
        "btrfs" => vec!["mkfs.btrfs".into(), "-f".into(), "-q".into()],
        "xfs" => vec!["mkfs.xfs".into(), "-f".into(), "-q".into()],
        "exfat" => vec!["mkfs.exfat".into()],
        "vfat" => vec!["mkfs.vfat".into()],
        "ntfs" => vec!["mkfs.ntfs".into(), "-F".into(), "-f".into()],
        _ => vec!["mkswap".into(), "-f".into()],
    };
    if !label.is_empty() {
        let flag = if fstype == "vfat" { "-n" } else { "-L" };
        // FAT keeps eleven capital letters.
        let label = if fstype == "vfat" { label.to_uppercase().chars().take(11).collect() } else { label.to_string() };
        argv.extend([flag.to_string(), label]);
    }
    argv.push(dev.to_string());
    argv
}

fn root_args(argv: Vec<String>) -> Step {
    Step { argv, root: true, ..Step::default() }
}

fn unmount_steps(part: &Part) -> Vec<Step> {
    part.mounts
        .iter()
        .rev()
        .map(|(dev, point)| {
            if point.starts_with('[') {
                Step::root(&["swapoff", dev])
            } else {
                Step::either(&["udisksctl", "unmount", "-b", dev], &["umount", dev])
            }
        })
        .collect()
}

fn unmount_all(disk: &Disk) -> Vec<Step> {
    disk.parts.iter().flat_map(unmount_steps).collect()
}

pub fn mount(part: &Part) -> Job {
    let name: String = part.label().chars().map(|c| if c == '/' || c == ' ' { '_' } else { c }).collect();
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let point = format!("/media/{user}/{name}");
    Job::new(
        &format!("掛載「{}」", part.label()),
        vec![Step {
            fallback: Some(Box::new(Step::root(&["sh", "-c", "mkdir -p \"$2\" && mount \"$1\" \"$2\"", "sh", &part.path, &point]))),
            ..Step::user(&["udisksctl", "mount", "-b", &part.path])
        }],
    )
}

pub fn unmount(part: &Part) -> Job {
    Job::new(&format!("卸載「{}」", part.label()), unmount_steps(part))
}

pub fn swap(part: &Part, on: bool) -> Job {
    let command = if on { "swapon" } else { "swapoff" };
    Job::new(&format!("{}「swap」", if on { "啟用" } else { "停用" }), vec![Step::root(&[command, &part.path])])
}

/// Takes an external drive out, or lets go of an image.
pub fn eject(disk: &Disk) -> Job {
    let mut steps = unmount_all(disk);
    steps.push(if disk.kind == Kind::Image {
        Step::either(&["udisksctl", "loop-delete", "-b", &disk.path], &["losetup", "-d", &disk.path])
    } else {
        Step::either(&["udisksctl", "power-off", "-b", &disk.path], &["eject", &disk.path])
    });
    Job::new(&format!("退出「{}」", disk.title()), steps)
}

pub fn format(disk: &Disk, part: &Part, fstype: &str, label: &str, zero: bool) -> Job {
    let mut steps: Vec<Step> = unmount_steps(part).into_iter().map(Step::tolerated).collect();
    if zero {
        steps.push(Step::root(&["dd", "if=/dev/zero", &format!("of={}", part.path), "bs=4M", "status=progress", "conv=fsync"]).of_bytes(part.size).tolerated());
    }
    steps.push(Step::root(&["wipefs", "-a", &part.path]));
    steps.push(root_args(mkfs_argv(fstype, label, &part.path)));
    steps.push(Step::root(&["udevadm", "settle"]).tolerated());
    let _ = disk;
    Job::new(&format!("格式化「{}」", part.label()), steps)
}

pub fn make_table(disk: &Disk, kind: &str) -> Job {
    Job::new(
        &format!("建立「{}」的分割表", disk.title()),
        vec![Step::root(&["parted", "-s", &disk.path, "mklabel", kind]), Step::root(&["udevadm", "settle"]).tolerated()],
    )
}

pub fn wipe(disk: &Disk, zero: bool, table: Option<&str>) -> Job {
    let mut steps: Vec<Step> = unmount_all(disk).into_iter().map(Step::tolerated).collect();
    if zero {
        steps.push(Step::root(&["dd", "if=/dev/zero", &format!("of={}", disk.path), "bs=4M", "status=progress", "conv=fsync"]).of_bytes(disk.size).tolerated());
    } else {
        for part in disk.parts.iter().filter(|part| part.number > 0) {
            steps.push(Step::root(&["wipefs", "-a", &part.path]).tolerated());
        }
        steps.push(Step::root(&["wipefs", "-a", "--force", &disk.path]));
    }
    if let Some(kind) = table {
        steps.push(Step::root(&["parted", "-s", &disk.path, "mklabel", kind]));
    }
    steps.push(Step::root(&["partprobe", &disk.path]).tolerated());
    steps.push(Step::root(&["udevadm", "settle"]).tolerated());
    Job::new(&format!("清除「{}」", disk.title()), steps)
}

/// How `parted` names a file system, for the partition type it writes.
fn parted_fs(fstype: &str) -> &'static str {
    match fstype {
        "btrfs" => "btrfs",
        "xfs" => "xfs",
        "vfat" => "fat32",
        "exfat" | "ntfs" => "ntfs",
        "swap" => "linux-swap",
        _ => "ext4",
    }
}

pub fn create(disk: &Disk, start: u64, size: u64, fstype: &str, label: &str) -> Job {
    let end = start + size - 1;
    let name = if disk.table.as_deref() == Some("gpt") { if label.trim().is_empty() { "partition" } else { label.trim() } } else { "primary" };
    let before = lsblk_names(&disk.path);
    Job {
        reason: format!("新增分割區於「{}」", disk.title()),
        actions: vec![
            Action::Run(Step::root(&["parted", "-s", &disk.path, "unit", "B", "mkpart", name, parted_fs(fstype), &format!("{start}B"), &format!("{end}B")])),
            Action::FindNew { disk: disk.path.clone(), before },
            Action::Run(root_args(mkfs_argv(fstype, label, "{new}"))),
            Action::Run(Step::root(&["udevadm", "settle"]).tolerated()),
        ],
    }
}

fn lsblk_names(disk: &str) -> Vec<String> {
    Command::new("lsblk")
        .args(["-nrpo", "NAME", disk])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// How a file system is resized.
#[derive(Clone, Copy, PartialEq)]
pub enum Resize {
    Ext,
    Btrfs,
    Xfs,
    Ntfs,
}

pub fn resize_kind(part: &Part) -> Option<Resize> {
    if part.number == 0 {
        return None;
    }
    match part.fstype.as_str() {
        "ext2" | "ext3" | "ext4" => Some(Resize::Ext),
        "btrfs" if part.mounted() => Some(Resize::Btrfs),
        "xfs" if part.mounted() => Some(Resize::Xfs),
        "ntfs" if which("ntfsresize").is_some() => Some(Resize::Ntfs),
        _ => None,
    }
}

/// The smallest a partition can be made.
pub fn min_size(part: &Part) -> u64 {
    match part.used {
        Some(used) => (used + used / 10 + (64 << 20)).min(part.size),
        None => part.size,
    }
}

pub fn resize(disk: &Disk, part: &Part, kind: Resize, new_size: u64) -> Job {
    let new_size = new_size / (1 << 20) * (1 << 20);
    let end = part.start + new_size - 1;
    let table = Step::root(&[
        "sh",
        "-c",
        "printf 'Yes\\n' | parted ---pretend-input-tty \"$1\" unit B resizepart \"$2\" \"$3\"",
        "sh",
        &disk.path,
        &part.number.to_string(),
        &format!("{end}B"),
    ]);
    let point = part.mount_point().unwrap_or_default().to_string();
    let mut steps = Vec::new();
    if new_size >= part.size {
        steps.push(table);
        steps.push(match kind {
            Resize::Ext => Step::root(&["resize2fs", &part.path]),
            Resize::Btrfs => Step::root(&["btrfs", "filesystem", "resize", "max", &point]),
            Resize::Xfs => Step::root(&["xfs_growfs", &point]),
            Resize::Ntfs => Step::root(&["ntfsresize", "-f", &part.path]),
        });
    } else {
        match kind {
            Resize::Ext => {
                steps.extend(unmount_steps(part));
                steps.push(Step::root(&["e2fsck", "-f", "-p", &part.path]).lenient());
                steps.push(Step::root(&["resize2fs", &part.path, &format!("{}K", new_size / 1024)]));
            }
            Resize::Btrfs => steps.push(Step::root(&["btrfs", "filesystem", "resize", &new_size.to_string(), &point])),
            Resize::Ntfs => {
                steps.extend(unmount_steps(part));
                steps.push(Step::root(&["ntfsresize", "-f", "-s", &new_size.to_string(), &part.path]));
            }
            Resize::Xfs => {}
        }
        steps.push(table);
    }
    steps.push(Step::root(&["partprobe", &disk.path]).tolerated());
    Job::new(&format!("調整「{}」大小", part.label()), steps)
}

/// The command that checks the file system of `part`, or None for one that has none.
pub fn check(part: &Part) -> Option<Job> {
    let dev = part.path.as_str();
    let mounted = part.mounted();
    let argv: Vec<&str> = match part.fstype.as_str() {
        "ext2" | "ext3" | "ext4" => vec!["e2fsck", "-f", if mounted { "-n" } else { "-p" }, dev],
        "btrfs" => vec!["btrfs", "check", "--readonly", dev],
        "xfs" if !mounted => vec!["xfs_repair", "-n", dev],
        "vfat" => vec!["fsck.vfat", "-n", dev],
        "exfat" => vec!["fsck.exfat", "-n", dev],
        "ntfs" => vec!["ntfsfix", "-n", dev],
        _ => return None,
    };
    Some(Job::new(&format!("檢查「{}」", part.label()), vec![Step::root(&argv).lenient()]))
}

pub fn image_create(disk: &Disk, file: &Path, zstd: bool) -> Job {
    let owner = format!("{}:{}", unsafe { libc::getuid() }, unsafe { libc::getgid() });
    let script = if zstd {
        "zstd -T0 -q -c -- \"$1\" > \"$2\" && chown \"$3\" \"$2\""
    } else {
        "dd if=\"$1\" of=\"$2\" bs=4M status=none conv=fsync && chown \"$3\" \"$2\""
    };
    Job::new(
        &format!("建立「{}」的映像檔", disk.title()),
        vec![Step::root(&["sh", "-c", script, "sh", &disk.path, &file.to_string_lossy(), &owner])],
    )
}

pub fn image_restore(file: &Path, disk: &Disk) -> Job {
    let script = "case \"$1\" in *.zst) zstd -dc -- \"$1\" ;; *) cat -- \"$1\" ;; esac | dd of=\"$2\" bs=4M status=none conv=fsync";
    let mut steps: Vec<Step> = unmount_all(disk).into_iter().map(Step::tolerated).collect();
    steps.push(Step::root(&["sh", "-c", script, "sh", &file.to_string_lossy(), &disk.path]));
    steps.push(Step::root(&["partprobe", &disk.path]).tolerated());
    Job::new(&format!("還原「{}」", disk.title()), steps)
}

/// Makes a disk of a file.
pub fn open_image(file: &Path) -> Job {
    let path = file.to_string_lossy();
    let read_only = path.ends_with(".iso");
    let mut user = vec!["udisksctl", "loop-setup", "-f", &path];
    let mut root = vec!["losetup", "-fP"];
    if read_only {
        user.push("-r");
        root.push("-r");
    }
    root.push(&path);
    Job::new("開啟映像檔", vec![Step::either(&user, &root)])
}

/// Reads a gigabyte off `disk`, and writes half of one in `point` when there is somewhere to.
pub fn speed(disk: &Disk, point: Option<&str>) -> Job {
    let mut steps = vec![Step::root(&["dd", &format!("if={}", disk.path), "of=/dev/null", "bs=1M", "count=1024", "iflag=direct"])];
    if let Some(point) = point {
        let file = format!("{}/.nekoland-speed", point.trim_end_matches('/'));
        steps.push(Step::user(&["dd", "if=/dev/zero", &format!("of={file}"), "bs=1M", "count=512", "oflag=direct", "conv=fsync"]).tolerated());
        steps.push(Step::user(&["rm", "-f", &file]).tolerated());
    }
    Job::new(&format!("測試「{}」的速度", disk.title()), steps)
}

/// The speeds, in bytes a second, in what `speed` printed: reading, then writing.
pub fn speeds(said: &str) -> (Option<f64>, Option<f64>) {
    let line = Regex::new(r"copied, [\d.,]+ s, ([\d.,]+) ([kMG]?B)/s").expect("a pattern");
    let mut found = said.lines().filter_map(|text| {
        let caught = line.captures(text)?;
        let amount: f64 = caught[1].replace(',', ".").parse().ok()?;
        let unit = match &caught[2] {
            "kB" => 1e3,
            "MB" => 1e6,
            "GB" => 1e9,
            _ => 1.0,
        };
        Some(amount * unit)
    });
    (found.next(), found.next())
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Smart {
    pub passed: Option<bool>,
    pub temperature: Option<i64>,
    pub rows: Vec<(String, String)>,
}

pub fn smart(disk: &Disk, root: bool) -> Job {
    let argv = ["smartctl", "-j", "-a", &disk.path];
    Job::new(&format!("讀取「{}」的 SMART 資料", disk.title()), vec![if root { Step::root(&argv) } else { Step::user(&argv) }.lenient()])
}

/// What `smartctl -j` says, as the rows to show.
pub fn parse_smart(json: &str) -> Option<Smart> {
    let json: Value = serde_json::from_str(json).ok()?;
    let mut smart = Smart { passed: json.pointer("/smart_status/passed").and_then(Value::as_bool), ..Smart::default() };
    let status = match smart.passed {
        Some(true) => "健康",
        Some(false) => "異常",
        None => "未知",
    };
    smart.rows.push(("整體評估".into(), status.into()));
    smart.temperature = json.pointer("/temperature/current").and_then(Value::as_i64);
    if let Some(degrees) = smart.temperature {
        smart.rows.push(("溫度".into(), format!("{degrees}°C")));
    }
    if let Some(hours) = json.pointer("/power_on_time/hours").and_then(Value::as_u64) {
        smart.rows.push(("通電時間".into(), format!("{} 小時", group(hours))));
    }
    if let Some(log) = json.get("nvme_smart_health_information_log") {
        if let Some(units) = log.get("data_units_written").and_then(Value::as_u64) {
            smart.rows.push(("已寫入".into(), super::model::size(units * 512_000)));
        }
        if let Some(used) = log.get("percentage_used").and_then(Value::as_i64) {
            smart.rows.push(("剩餘壽命".into(), format!("{}%", (100 - used).max(0))));
        }
        if let Some(spare) = log.get("available_spare").and_then(Value::as_i64) {
            smart.rows.push(("可用備用區".into(), format!("{spare}%")));
        }
    }
    for attribute in json.pointer("/ata_smart_attributes/table").and_then(Value::as_array).into_iter().flatten() {
        let raw = attribute.pointer("/raw/value").and_then(Value::as_u64);
        match (attribute.get("id").and_then(Value::as_u64), raw) {
            (Some(5), Some(raw)) => smart.rows.push(("重新配置磁區".into(), group(raw))),
            (Some(197), Some(raw)) => smart.rows.push(("待處理磁區".into(), group(raw))),
            (Some(241), Some(raw)) => smart.rows.push(("已寫入".into(), super::model::size(raw * 512))),
            _ => {}
        }
    }
    Some(smart)
}

fn group(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkfs_commands_carry_the_label() {
        assert_eq!(mkfs_argv("ext4", "資料", "/dev/sda1"), ["mkfs.ext4", "-F", "-q", "-L", "資料", "/dev/sda1"]);
        assert_eq!(mkfs_argv("vfat", "backup disk long", "/dev/sda1"), ["mkfs.vfat", "-n", "BACKUP DISK", "/dev/sda1"]);
        assert_eq!(mkfs_argv("swap", "", "/dev/sda2"), ["mkswap", "-f", "/dev/sda2"]);
    }

    #[test]
    fn dd_progress_is_read_from_its_lines() {
        assert_eq!(written(b"1048576 bytes (1.0 MB, 1.0 MiB) copied, 0.1 s, 10 MB/s"), Some(1_048_576));
        assert_eq!(written(b"dd: error"), None);
    }

    #[test]
    fn speeds_are_read_in_order() {
        let said = "1073741824 bytes (1.1 GB, 1.0 GiB) copied, 0.2 s, 5.3 GB/s\n536870912 bytes (537 MB, 512 MiB) copied, 0.5 s, 1.1 GB/s\n";
        assert_eq!(speeds(said), (Some(5.3e9), Some(1.1e9)));
    }

    #[test]
    fn smart_of_an_nvme_drive_shows_its_wear() {
        let json = r#"{"smart_status":{"passed":true},"temperature":{"current":38},"power_on_time":{"hours":4812},
            "nvme_smart_health_information_log":{"data_units_written":41800000,"percentage_used":4,"available_spare":100}}"#;
        let smart = parse_smart(json).unwrap();
        assert_eq!(smart.passed, Some(true));
        assert_eq!(smart.temperature, Some(38));
        assert!(smart.rows.contains(&("通電時間".to_string(), "4,812 小時".to_string())));
        assert!(smart.rows.contains(&("剩餘壽命".to_string(), "96%".to_string())));
    }

    fn kind_of(dir: &std::path::Path, name: &str, mib: u64) -> Disk {
        let path = dir.join(name);
        std::fs::File::create(&path).unwrap().set_len(mib << 20).unwrap();
        Disk {
            path: path.to_string_lossy().into(),
            kname: name.into(),
            name: name.into(),
            size: mib << 20,
            kind: crate::model::Kind::Image,
            read_only: false,
            tran: String::new(),
            table: None,
            sector: 512,
            backing: None,
            parts: Vec::new(),
        }
    }

    fn whole(disk: &Disk, fstype: &str) -> Part {
        Part {
            path: disk.path.clone(),
            kname: disk.kname.clone(),
            number: 0,
            name: "t".into(),
            fstype: fstype.into(),
            uuid: String::new(),
            mounts: Vec::new(),
            size: disk.size,
            start: 0,
            used: None,
        }
    }

    fn finish(job: Job) -> Result<String, String> {
        let (events, _cancel) = spawn(job);
        loop {
            if let Event::Done(done) = events.recv_blocking().unwrap() {
                return done;
            }
        }
    }

    fn said(argv: &[&str]) -> String {
        let out = Command::new(argv[0]).args(&argv[1..]).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    #[test]
    fn jobs_work_on_a_disk_image() {
        let dir = std::env::temp_dir().join(format!("nekoland-disks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A sudo that asks nothing, on image files the user owns, to run the root steps as they are built.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("sudo"), "#!/bin/sh\n[ \"$1\" = -A ] && [ \"$2\" = -- ] && [ -n \"$SUDO_ASKPASS\" ] || exit 99\nshift 2\nexec \"$@\"\n").unwrap();
        std::fs::set_permissions(bin.join("sudo"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
        let mut disk = kind_of(&dir, "a.img", 64);

        finish(make_table(&disk, "gpt")).unwrap();
        assert!(said(&["parted", "-s", &disk.path, "print"]).contains("gpt"));

        // The size the partition is made at, then grown the way the resize job does.
        finish(Job::new("t", vec![Step::root(&["parted", "-s", &disk.path, "unit", "B", "mkpart", "p", "ext4", "1048576B", "33554431B"])])).unwrap();
        let mut part = whole(&disk, "ext4");
        part.number = 1;
        part.start = 1 << 20;
        part.size = 32 << 20;
        part.path = dir.join("fs.img").to_string_lossy().into();
        std::fs::File::create(&part.path).unwrap().set_len(part.size).unwrap();
        disk.parts = vec![part.clone()];
        finish(resize(&disk, &part, Resize::Ext, 48 << 20)).unwrap_err(); // the file system has not been made yet

        finish(format(&disk, &part, "ext4", "資料", false)).unwrap();
        assert!(said(&["dumpe2fs", "-h", &part.path]).contains("資料"));
        finish(check(&part).unwrap()).unwrap();

        // Shrink, then grow, the file system and the table together.
        finish(resize(&disk, &part, Resize::Ext, 16 << 20)).unwrap();
        assert!(said(&["parted", "-s", &disk.path, "unit", "B", "print"]).contains("17825791B"));
        part.size = 16 << 20;
        finish(resize(&disk, &part, Resize::Ext, 32 << 20)).unwrap();
        assert!(said(&["parted", "-s", &disk.path, "unit", "B", "print"]).contains("34603007B"));

        // Back it up compressed, wipe it, put it back.
        let image = dir.join("a.img.zst");
        finish(image_create(&disk, &image, true)).unwrap();
        finish(wipe(&disk, false, None)).unwrap();
        assert!(!said(&["parted", "-s", &disk.path, "print"]).contains("gpt"));
        finish(image_restore(&image, &disk)).unwrap();
        assert!(said(&["parted", "-s", &disk.path, "print"]).contains("gpt"));

        let said = finish(speed(&disk, Some(&dir.to_string_lossy()))).unwrap();
        let (read, write) = speeds(&said);
        assert!(read.is_some() && write.is_some(), "{said}");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
