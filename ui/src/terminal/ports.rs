//! The TCP ports the programs of a tab listen on (a dev server): one look at the listening sockets and the processes of
//! the machine per tick, then each pane's shell and what it started, by the sockets they hold.

use std::collections::HashMap;

use super::proc;

const TCP_PREFIX: &str = "TCP ";

/// The machine at one tick: who started whom, and the port of each listening TCP socket by its inode.
pub struct Snapshot {
    children: HashMap<i32, Vec<i32>>,
    listeners: HashMap<u64, u16>,
}

/// The machine now; none when nothing listens, which spares reading every process.
pub fn snapshot() -> Option<Snapshot> {
    let listeners: HashMap<u64, u16> = proc::listening_ports()
        .into_iter()
        .filter_map(|(inode, socket)| Some((inode, socket.strip_prefix(TCP_PREFIX)?.parse().ok()?)))
        .collect();
    if listeners.is_empty() {
        return None;
    }
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for process in proc::read_procs() {
        children.entry(process.ppid).or_default().push(process.pid);
    }
    Some(Snapshot { children, listeners })
}

impl Snapshot {
    /// The ports `roots` and every process under them listen on, ascending, each once (a server on IPv4 and IPv6
    /// counts once).
    pub fn ports_of(&self, roots: &[i32]) -> Vec<u16> {
        let mut ports: Vec<u16> = self
            .descendants(roots)
            .into_iter()
            .flat_map(|pid| proc::open_files(pid).1)
            .filter_map(|inode| self.listeners.get(&inode).copied())
            .collect();
        ports.sort_unstable();
        ports.dedup();
        ports
    }

    fn descendants(&self, roots: &[i32]) -> Vec<i32> {
        let mut found = roots.to_vec();
        let mut next = 0;
        while let Some(&pid) = found.get(next) {
            found.extend(self.children.get(&pid).into_iter().flatten());
            next += 1;
        }
        found
    }
}

/// What opens a port in the browser.
pub fn url(port: u16) -> String {
    format!("http://localhost:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_this_process_listens_on_is_found() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let me = i32::try_from(std::process::id()).unwrap();
        assert!(snapshot().unwrap().ports_of(&[me]).contains(&port));
    }

    #[test]
    fn the_processes_under_a_root_are_followed_down() {
        let snapshot = Snapshot {
            children: HashMap::from([(1, vec![2, 3]), (3, vec![4]), (9, vec![10])]),
            listeners: HashMap::new(),
        };
        assert_eq!(snapshot.descendants(&[1]), [1, 2, 3, 4]);
    }
}
