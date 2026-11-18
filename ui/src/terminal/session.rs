//! A shell on a pty of its own. What it writes is read on a thread, watched by the scanner
//! on the way, and handed to the screen; what is typed goes back on another thread, so that
//! a program that stops reading never holds up the window. VTE is given only the bytes, as
//! it would not let the OSC commands it does not know be seen.

use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
};

use super::osc::{Scanner, Signal};

/// How much is read from the pty at once.
const READ_CHUNK: usize = 64 * 1024;
/// How many reads may wait for the window before the reader stops reading, and the program
/// writing waits in turn.
const EVENT_BACKLOG: usize = 16;
const TERM: &str = "xterm-256color";

pub enum Event {
    Output { bytes: Vec<u8>, signals: Vec<Signal> },
    Exited,
}

pub struct Session {
    master: OwnedFd,
    input: mpsc::Sender<Vec<u8>>,
    pub shell_pid: i32,
}

/// What to run, and where.
pub struct Launch<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    /// The variables it gets on top of the terminal's, or instead of them when clean.
    pub environment: &'a [(String, String)],
    pub clean: bool,
    pub directory: Option<&'a Path>,
    /// The file that marks prompts and commands for bash, when there is one.
    pub bash_integration: Option<&'a Path>,
    pub rows: u16,
    pub columns: u16,
}

impl Session {
    pub fn spawn(launch: &Launch) -> io::Result<(Self, async_channel::Receiver<Event>)> {
        let (master, slave) = open_pty(launch.rows, launch.columns)?;
        let child = shell_command(launch, &slave)?.spawn()?;
        drop(slave);
        let shell_pid = i32::try_from(child.id()).map_err(io::Error::other)?;
        Self::start(master, shell_pid)
    }

    /// Takes over the pty a copy of this program handed over through `exec` (see `carry.rs`): its shell is still this
    /// process's child, and still runs.
    pub fn adopt(master: i32, shell_pid: i32) -> io::Result<(Self, async_channel::Receiver<Event>)> {
        // SAFETY: F_GETFD only reads the flags of a descriptor number, open or not.
        if unsafe { libc::fcntl(master, libc::F_GETFD) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor is open, and was handed over to be owned here alone.
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        keep_from_children(&master)?;
        Self::start(master, shell_pid)
    }

    /// Lets the pty outlive an `exec` of this program; the number it will have there.
    pub fn hand_over(&self) -> io::Result<i32> {
        // SAFETY: the descriptor is open for as long as `self`.
        if unsafe { libc::fcntl(self.master.as_raw_fd(), libc::F_SETFD, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(self.master.as_raw_fd())
    }

    /// Takes back a hand-over that did not happen, so that the shells started next do not inherit the pty.
    pub fn keep(&self) -> io::Result<()> {
        keep_from_children(&self.master)
    }

    fn start(master: OwnedFd, shell_pid: i32) -> io::Result<(Self, async_channel::Receiver<Event>)> {
        let (events, receiver) = async_channel::bounded(EVENT_BACKLOG);
        let reader = File::from(master.try_clone()?);
        let replies = File::from(master.try_clone()?);
        thread::Builder::new()
            .name("terminal-reader".into())
            .spawn(move || read_output(reader, replies, shell_pid, events))?;

        let (input, typed) = mpsc::channel();
        let writer = File::from(master.try_clone()?);
        thread::Builder::new()
            .name("terminal-writer".into())
            .spawn(move || write_input(writer, typed))?;

        Ok((
            Self {
                master,
                input,
                shell_pid,
            },
            receiver,
        ))
    }

    pub fn write(&self, bytes: &[u8]) {
        if self.input.send(bytes.to_vec()).is_err() {
            eprintln!("nekoland-terminal: the shell is no longer reading");
        }
    }

    /// The pty's rows and columns, and its width and height in pixels.
    pub fn resize(&self, rows: u16, columns: u16, pixels: (u16, u16)) {
        let size = window_size(rows, columns, pixels);
        // SAFETY: the descriptor is open for as long as `self`, and `size` outlives the call.
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
            eprintln!(
                "nekoland-terminal: could not resize the pty: {}",
                io::Error::last_os_error()
            );
        }
    }

    /// The process group in the foreground of the pty.
    pub fn foreground_group(&self) -> Option<i32> {
        // SAFETY: the descriptor is open for as long as `self`.
        let group = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        (group > 0).then_some(group)
    }

    /// Where the shell is.
    pub fn directory(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.shell_pid)).ok()
    }

    /// Hangs up on the shell and everything it started, as closing a terminal does.
    pub fn hang_up(&self) {
        // SAFETY: a negative pid names the shell's process group; no memory is involved.
        if unsafe { libc::kill(-self.shell_pid, libc::SIGHUP) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                eprintln!("nekoland-terminal: could not hang up on the shell: {error}");
            }
        }
    }
}

impl Drop for Session {
    /// A closed pane hangs up on its shell.
    fn drop(&mut self) {
        self.hang_up();
    }
}

fn window_size(rows: u16, columns: u16, pixels: (u16, u16)) -> libc::winsize {
    libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: pixels.0, ws_ypixel: pixels.1 }
}

fn open_pty(rows: u16, columns: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    // The pixels are told once the screen is laid out.
    let size = window_size(rows, columns, (0, 0));
    // SAFETY: both out-pointers are valid, the name and terminal settings are left out, and
    // `size` outlives the call.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    if opened < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both are open descriptors that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    keep_from_children(&master)?;
    Ok((master, slave))
}

/// The shells must not inherit the master side of a pty, or it would never close.
fn keep_from_children(master: &OwnedFd) -> io::Result<()> {
    // SAFETY: the descriptor is open for as long as `master`.
    if unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Waits for the shell to end, so that it leaves no zombie.
fn wait_for(shell_pid: i32) {
    let mut status = 0;
    loop {
        // SAFETY: `status` is a valid out-pointer; the pid is this process's child.
        if unsafe { libc::waitpid(shell_pid, &mut status, 0) } >= 0 {
            return;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            eprintln!("nekoland-terminal: could not wait for the shell: {error}");
            return;
        }
    }
}

fn shell_command(launch: &Launch, slave: &OwnedFd) -> io::Result<Command> {
    let mut command = Command::new(launch.program);
    let is_bash = Path::new(launch.program).file_name().is_some_and(|name| name == "bash");
    match (is_bash, launch.bash_integration) {
        // The integration reads the profile itself, as a login shell would.
        (true, Some(integration)) => {
            command.arg("--rcfile").arg(integration);
            command.args(launch.args.iter().filter(|arg| *arg != "--login" && *arg != "-l"));
        }
        _ => {
            command.args(launch.args);
        }
    }
    if launch.clean {
        command.env_clear();
    }
    command.envs(launch.environment.iter().map(|(name, value)| (name, value)));
    if let Some(directory) = launch.directory {
        command.current_dir(directory);
    }
    command
        .env("TERM", TERM)
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "nekoland-terminal")
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave.try_clone()?));
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

fn read_output(
    mut reader: File,
    mut replies: File,
    shell_pid: i32,
    events: async_channel::Sender<Event>,
) {
    let mut scanner = Scanner::default();
    let mut buffer = vec![0; READ_CHUNK];
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // EIO: everything holding the shell's side of the pty has closed it.
            Err(_) => break,
        };
        let mut signals = Vec::new();
        scanner.scan(&buffer[..count], &mut signals);
        answer_queries(&signals, &mut replies);
        let output = Event::Output {
            bytes: buffer[..count].to_vec(),
            signals,
        };
        if events.send_blocking(output).is_err() {
            break;
        }
    }
    wait_for(shell_pid);
    // The pane may be gone already, and with it the reason to tell.
    let _ = events.send_blocking(Event::Exited);
}

/// Answers an agent asking whether its reports are read, here, before the screen sees the
/// rest of what it wrote, so that the answer comes before any the screen gives.
fn answer_queries(signals: &[Signal], replies: &mut File) {
    for signal in signals {
        if let Signal::AgentQuery(terminator) = signal {
            let answer = [b"\x1b]7501;?".as_slice(), terminator.bytes()].concat();
            if let Err(error) = replies.write_all(&answer) {
                eprintln!("nekoland-terminal: could not answer the agent: {error}");
            }
        }
    }
}

fn write_input(mut writer: File, typed: mpsc::Receiver<Vec<u8>>) {
    for bytes in typed {
        if let Err(error) = writer.write_all(&bytes) {
            eprintln!("nekoland-terminal: could not write to the shell: {error}");
            return;
        }
    }
}
