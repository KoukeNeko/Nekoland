//! What the task manager reads: every process in /proc, and the machine's CPU, memory,
//! disks and network, as counters that two readings a second apart turn into rates.

use std::{collections::HashMap, path::Path};

/// One process, as /proc has it at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub name: String,
    /// CPU time used so far, in clock ticks.
    pub ticks: u64,
    /// When it started, in clock ticks after boot.
    pub started: u64,
    pub threads: u32,
    /// Memory in use, in bytes; None for a kernel thread.
    pub memory: Option<u64>,
    /// Bytes read from and written to disk so far; None when it is not the user's to read.
    pub io: Option<u64>,
}

/// The machine at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    /// CPU time, all cores together, in clock ticks: in use, and in all.
    pub busy: u64,
    pub total: u64,
    pub cores: usize,
    pub memory_total: u64,
    pub memory_available: u64,
    /// Bytes read and written by the disks, and received and sent by the network, so far.
    pub disk: u64,
    pub network: u64,
    /// When the machine started, in seconds since 1970.
    pub boot: u64,
}

pub fn read_procs() -> Vec<Proc> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter_map(read_proc)
        .collect()
}

/// One process; None when it ended while being read.
fn read_proc(pid: i32) -> Option<Proc> {
    let dir = Path::new("/proc").join(pid.to_string());
    let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
    let (comm, fields) = parse_stat(&stat)?;
    let status = std::fs::read_to_string(dir.join("status")).ok()?;
    let field = |key: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
    };
    let number = |index: usize| fields.get(index).and_then(|field| field.parse::<u64>().ok());
    let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
    Some(Proc {
        pid,
        ppid: number(1)? as i32,
        uid: field("Uid:")?.split_whitespace().next()?.parse().ok()?,
        name: full_name(&comm, &cmdline),
        ticks: number(11)? + number(12)?,
        started: number(19)?,
        threads: field("Threads:")
            .and_then(|threads| threads.parse().ok())
            .unwrap_or(1),
        memory: field("VmRSS:")
            .and_then(|rss| rss.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kb| kb * 1024),
        io: std::fs::read_to_string(dir.join("io"))
            .ok()
            .map(|io| parse_io(&io)),
    })
}

/// `pid (comm) state ppid …`: the name, and the fields after it, `state` the first. The
/// name is taken to the last parenthesis, as it may hold one itself.
pub fn parse_stat(stat: &str) -> Option<(String, Vec<&str>)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    Some((
        stat[open + 1..close].to_string(),
        stat[close + 1..].split_whitespace().collect(),
    ))
}

fn parse_io(io: &str) -> u64 {
    io.lines()
        .filter_map(|line| {
            line.strip_prefix("read_bytes:")
                .or_else(|| line.strip_prefix("write_bytes:"))
        })
        .filter_map(|bytes| bytes.trim().parse::<u64>().ok())
        .sum()
}

/// The kernel keeps 15 characters of a name; the program's own name, when it starts the
/// same, is the whole of it.
pub fn full_name(comm: &str, cmdline: &[u8]) -> String {
    let program = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
    let program = String::from_utf8_lossy(program);
    let base = program.rsplit('/').next().unwrap_or_default();
    if comm.len() >= 15 && base.starts_with(comm) {
        base.to_string()
    } else {
        comm.to_string()
    }
}

pub fn read_machine() -> Machine {
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
    let stat = read("/proc/stat");
    let mut machine = parse_cpu(&stat);
    machine.boot = stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|boot| boot.trim().parse().ok())
        .unwrap_or_default();
    let meminfo = read("/proc/meminfo");
    let kb = |key: &str| {
        meminfo
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or_default()
            * 1024
    };
    machine.memory_total = kb("MemTotal:");
    machine.memory_available = kb("MemAvailable:");
    machine.disk = parse_diskstats(&read("/proc/diskstats"), |name| {
        // Disks themselves, not their partitions, loop devices or what is mapped over them.
        Path::new("/sys/block").join(name).join("device").exists()
    });
    machine.network = parse_net_dev(&read("/proc/net/dev"));
    machine
}

fn parse_cpu(stat: &str) -> Machine {
    let mut machine = Machine::default();
    for line in stat.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("cpu") => {
                // user, nice, system, idle, iowait, irq, softirq, steal
                let times: Vec<u64> = fields.take(8).filter_map(|time| time.parse().ok()).collect();
                machine.total = times.iter().sum();
                machine.busy = machine.total - times.get(3).unwrap_or(&0) - times.get(4).unwrap_or(&0);
            }
            Some(cpu) if cpu.starts_with("cpu") => machine.cores += 1,
            _ => {}
        }
    }
    machine
}

/// Bytes read and written by the disks `counts` says to count, from their sectors of 512.
fn parse_diskstats(diskstats: &str, counts: impl Fn(&str) -> bool) -> u64 {
    diskstats
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let sectors = |index: usize| fields.get(index)?.parse::<u64>().ok();
            counts(fields.get(2)?).then(|| sectors(5).unwrap_or(0) + sectors(9).unwrap_or(0))
        })
        .sum::<u64>()
        * 512
}

/// Bytes received and sent by every interface but the loopback.
fn parse_net_dev(dev: &str) -> u64 {
    dev.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim() != "lo")
        .map(|(_, counters)| {
            let fields: Vec<u64> = counters
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect();
            fields.first().unwrap_or(&0) + fields.get(8).unwrap_or(&0)
        })
        .sum()
}

/// The application each process belongs to: the one whose window it, or a process it was
/// started from, has.
pub fn owners<'a>(procs: &[Proc], windows: &'a HashMap<i32, String>) -> HashMap<i32, &'a str> {
    let parents: HashMap<i32, i32> = procs.iter().map(|proc| (proc.pid, proc.ppid)).collect();
    procs
        .iter()
        .filter_map(|proc| {
            let mut pid = proc.pid;
            // As deep as process trees go, and never round a loop.
            for _ in 0..64 {
                if let Some(app) = windows.get(&pid) {
                    return Some((proc.pid, app.as_str()));
                }
                pid = *parents.get(&pid).filter(|parent| **parent > 1)?;
            }
            None
        })
        .collect()
}

/// A user's login name, from /etc/passwd.
pub fn user_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        (fields.nth(1)?.parse() == Ok(uid)).then(|| name.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_are_read_past_a_name_with_parentheses() {
        let stat = "4242 (Web Content (1)) S 4100 4242 4100 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 31 0 9000 1000 2000";
        let (name, fields) = parse_stat(stat).unwrap();
        assert_eq!(name, "Web Content (1)");
        assert_eq!(fields[1], "4100");
        assert_eq!(
            (fields[11], fields[12], fields[17], fields[19]),
            ("250", "50", "31", "9000")
        );
    }

    #[test]
    fn a_cut_name_is_made_whole_from_the_command_line() {
        assert_eq!(
            full_name("tracker-miner-f", b"/usr/libexec/tracker-miner-fs-3\0--x"),
            "tracker-miner-fs-3"
        );
        assert_eq!(full_name("bash", b"/usr/bin/bash\0"), "bash");
        assert_eq!(
            full_name("Isolated Web Co", b"/usr/lib/firefox/firefox\0-contentproc"),
            "Isolated Web Co"
        );
    }

    #[test]
    fn counters_are_summed() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nbtime 1\n";
        let machine = parse_cpu(stat);
        assert_eq!((machine.total, machine.busy, machine.cores), (1000, 150, 2));
        let dev = "Inter-|   Receive\n face |bytes\n    lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n  eth0: 100 1 0 0 0 0 0 0 40 1 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(dev), 140);
        let disks = "   8       0 sda 10 0 4 0 5 0 6 0 0 0 0\n   8       1 sda1 10 0 4 0 5 0 6 0 0 0 0\n";
        assert_eq!(parse_diskstats(disks, |name| name == "sda"), 10 * 512);
        assert_eq!(parse_io("rchar: 9\nread_bytes: 4096\nwrite_bytes: 8192\n"), 12288);
    }

    #[test]
    fn processes_belong_to_the_application_they_were_started_from() {
        let proc = |pid, ppid| Proc {
            pid,
            ppid,
            uid: 1000,
            name: String::new(),
            ticks: 0,
            started: 0,
            threads: 1,
            memory: Some(1),
            io: None,
        };
        let procs = [proc(10, 1), proc(20, 10), proc(30, 20), proc(40, 1), proc(50, 10)];
        let windows = HashMap::from([(20, "foot".to_string()), (40, "firefox".to_string())]);
        let owners = owners(&procs, &windows);
        assert_eq!(owners.get(&30), Some(&"foot"));
        assert_eq!(owners.get(&20), Some(&"foot"));
        assert_eq!(owners.get(&40), Some(&"firefox"));
        assert_eq!(owners.get(&10), None);
        assert_eq!(owners.get(&50), None);
    }
}
