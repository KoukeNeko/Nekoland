//! The automation API, as Nekominal's: a socket a script or an agent drives the terminal through, as `wezterm cli` or
//! `kitten @` do, and `nekoland-terminal cli`, which speaks it. One socket for the process (all its windows),
//! `$XDG_RUNTIME_DIR/nekoland-terminal/api-<pid>.sock`, in a folder only the user can open. Every pane gets its path as
//! `NEKOLAND_TERMINAL_SOCKET` and its own id as `NEKOLAND_TERMINAL_PANE`, so a program talks to its own instance.
//!
//! The protocol is a line of JSON per request (`{"id": 1, "cmd": "list"}`) and a line per answer (`{"id": 1, "ok": true,
//! "result": …}` or `{"id": 1, "ok": false, "error": "…"}`). `subscribe` answers once and then streams a line per event
//! (`{"event": "agent-state", "pane": "…", …}`) until the client goes.
//!
//! `send-text` is a paste, as Nekominal's: bracketed when the program asked for it, and through the same check as a
//! paste of the clipboard, which asks the user first (the answer then says `asked`). A line break at its end is pressed
//! as Enter after the paste, so that what a script sends runs.
//!
//! A thread accepts clients and a thread serves each; a request goes to the main thread, which answers it from the
//! windows (`window_api.rs`) and hands the answer back.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::{UnixListener, UnixStream}},
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
};

use gtk::glib;
use serde_json::{json, Value};

pub const SOCKET_VAR: &str = "NEKOLAND_TERMINAL_SOCKET";
pub const PANE_VAR: &str = "NEKOLAND_TERMINAL_PANE";
const FOLDER: &str = "nekoland-terminal";
const SOCKET_PREFIX: &str = "api-";
const SOCKET_SUFFIX: &str = ".sock";
const PRIVATE_FOLDER: u32 = 0o700;
/// The events that wait for a subscriber that does not read them; past that it is dropped.
const BACKLOG: usize = 1024;
const COMMANDS: &str = "list, send-text, get-text, new-tab, split, focus, close, set-title, notify, subscribe";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Right,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    /// An agent in a pane starts working, waits, ends or fails.
    AgentState,
    /// A command of the shell ends, with its exit status.
    CommandFinished,
    /// A pane printed something.
    Output,
}

impl Topic {
    pub fn name(self) -> &'static str {
        match self {
            Topic::AgentState => "agent-state",
            Topic::CommandFinished => "command-finished",
            Topic::Output => "output",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [Topic::AgentState, Topic::CommandFinished, Topic::Output].into_iter().find(|topic| topic.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    List,
    SendText { pane: String, text: String },
    GetText { pane: String, lines: Option<usize> },
    NewTab { profile: Option<String>, cwd: Option<String>, command: Vec<String> },
    Split { pane: String, direction: Direction, cwd: Option<String>, command: Vec<String> },
    Focus { pane: String },
    Close { pane: String },
    SetTitle { pane: String, title: String },
    Notify { title: String, body: String, pane: Option<String> },
    Subscribe { topics: Vec<Topic> },
}

/// A string member: missing (or null) is none, anything but a string an error.
fn text(request: &Value, key: &str) -> Result<Option<String>, String> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn required(request: &Value, key: &str) -> Result<String, String> {
    text(request, key)?.ok_or_else(|| format!("\"{key}\" is missing"))
}

/// A pane id, as `list` gives it and `NEKOLAND_TERMINAL_PANE` holds; never empty.
fn pane(request: &Value) -> Result<String, String> {
    Some(required(request, "pane")?).filter(|pane| !pane.is_empty()).ok_or_else(|| "\"pane\" is empty".to_string())
}

/// The program and its arguments; none runs the profile's shell.
fn command(request: &Value) -> Result<Vec<String>, String> {
    const NOT_WORDS: &str = "\"command\" must be an array of strings";
    match request.get("command") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(words)) => {
            words.iter().map(|word| word.as_str().map(str::to_string).ok_or_else(|| NOT_WORDS.to_string())).collect()
        }
        Some(_) => Err(NOT_WORDS.to_string()),
    }
}

fn lines(request: &Value) -> Result<Option<usize>, String> {
    match request.get("lines") {
        None | Some(Value::Null) => Ok(None),
        Some(lines) => lines
            .as_u64()
            .filter(|count| *count > 0)
            .and_then(|count| usize::try_from(count).ok())
            .map(Some)
            .ok_or_else(|| "\"lines\" must be a whole number above 0".to_string()),
    }
}

fn direction(request: &Value) -> Result<Direction, String> {
    match text(request, "direction")?.as_deref() {
        None | Some("right") => Ok(Direction::Right),
        Some("down") => Ok(Direction::Down),
        Some(other) => Err(format!("\"direction\" is right or down, not {other}")),
    }
}

fn topics(request: &Value) -> Result<Vec<Topic>, String> {
    match request.get("events") {
        None | Some(Value::Null) => Ok(vec![Topic::AgentState, Topic::CommandFinished]),
        Some(Value::Array(names)) => names
            .iter()
            .map(|name| {
                name.as_str().and_then(Topic::parse).ok_or_else(|| {
                    format!("unknown event {name}; the events are agent-state, command-finished and output")
                })
            })
            .collect(),
        Some(_) => Err("\"events\" must be an array of names".to_string()),
    }
}

fn request_of(request: &Value) -> Result<Request, String> {
    let cmd = required(request, "cmd")?;
    Ok(match cmd.as_str() {
        "list" => Request::List,
        "send-text" => {
            let text = Some(required(request, "text")?).filter(|text| !text.is_empty()).ok_or("\"text\" is empty")?;
            Request::SendText { pane: pane(request)?, text }
        }
        "get-text" => Request::GetText { pane: pane(request)?, lines: lines(request)? },
        "new-tab" => Request::NewTab { profile: text(request, "profile")?, cwd: text(request, "cwd")?, command: command(request)? },
        "split" => Request::Split {
            pane: pane(request)?,
            direction: direction(request)?,
            cwd: text(request, "cwd")?,
            command: command(request)?,
        },
        "focus" => Request::Focus { pane: pane(request)? },
        "close" => Request::Close { pane: pane(request)? },
        "set-title" => Request::SetTitle { pane: pane(request)?, title: required(request, "title")? },
        "notify" => Request::Notify {
            title: required(request, "title")?,
            body: text(request, "body")?.unwrap_or_default(),
            pane: text(request, "pane")?,
        },
        "subscribe" => Request::Subscribe { topics: topics(request)? },
        other => return Err(format!("unknown command {other}")),
    })
}

/// One line of a client: the id to answer with (null when there is none) and what it asks, or why it is no request.
pub fn parse(line: &str) -> (Value, Result<Request, String>) {
    let request: Value = match serde_json::from_str(line) {
        Ok(request @ Value::Object(_)) => request,
        Ok(_) => return (Value::Null, Err("a request is a JSON object".to_string())),
        Err(error) => return (Value::Null, Err(format!("not JSON: {error}"))),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    (id, request_of(&request))
}

/// The line that answers the request `id`.
pub fn reply(id: &Value, answer: &Result<Value, String>) -> String {
    match answer {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    }
    .to_string()
}

// --- events ---------------------------------------------------------------------------------------------------------

type Subscriber = (Vec<Topic>, mpsc::SyncSender<String>);
static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());

fn subscribers() -> std::sync::MutexGuard<'static, Vec<Subscriber>> {
    SUBSCRIBERS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Someone follows `topic`: the window works out the event only then.
pub fn wants(topic: Topic) -> bool {
    subscribers().iter().any(|(topics, _)| topics.contains(&topic))
}

/// Sends an event to everyone who follows its topic, without waiting: a subscriber that went or stopped reading is
/// dropped, which ends its connection.
pub fn publish(topic: Topic, pane: &str, more: Value) {
    let mut line = json!({ "event": topic.name(), "pane": pane });
    if let (Some(line), Value::Object(more)) = (line.as_object_mut(), more) {
        line.extend(more);
    }
    let line = line.to_string();
    subscribers().retain(|(topics, sender)| !topics.contains(&topic) || sender.try_send(line.clone()).is_ok());
}

// --- the server ---------------------------------------------------------------------------------------------------

/// What the main thread answers a request with.
pub type Answer = Result<Value, String>;
type Job = (Request, mpsc::Sender<Answer>);

fn socket_folder() -> PathBuf {
    glib::user_runtime_dir().join(FOLDER)
}

/// This process's socket.
pub fn socket_path() -> PathBuf {
    socket_folder().join(format!("{SOCKET_PREFIX}{}{SOCKET_SUFFIX}", std::process::id()))
}

fn private_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(PRIVATE_FOLDER))
}

/// The process a socket's name says it is of.
fn socket_pid(name: &str) -> Option<u32> {
    name.strip_prefix(SOCKET_PREFIX)?.strip_suffix(SOCKET_SUFFIX)?.parse().ok()
}

/// Takes away the sockets of terminals that ended without taking theirs, as a killed one does.
fn remove_stale_sockets(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(socket_pid) else {
            continue;
        };
        if Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            eprintln!("nekoland-terminal: could not remove {}: {error}", entry.path().display());
        }
    }
}

/// Opens the socket and answers what comes in with `answer`, on the main thread.
pub fn start(answer: fn(Request) -> Answer) {
    let path = socket_path();
    let folder = socket_folder();
    let listener = match private_folder(&folder).and_then(|()| {
        remove_stale_sockets(&folder);
        // Left by the copy of this program before an update and restart, which had this id.
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        UnixListener::bind(&path)
    }) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("nekoland-terminal: the automation socket could not open: {error}");
            return;
        }
    };
    let (jobs, queue) = async_channel::unbounded::<Job>();
    glib::spawn_future_local(async move {
        while let Ok((request, back)) = queue.recv().await {
            // The client may have gone while it waited.
            let _ = back.send(answer(request));
        }
    });
    std::thread::spawn(move || {
        for client in listener.incoming() {
            match client {
                Ok(client) => {
                    let jobs = jobs.clone();
                    std::thread::spawn(move || serve(client, &jobs));
                }
                Err(error) => eprintln!("nekoland-terminal: a client of the automation socket failed: {error}"),
            }
        }
    });
}

/// Takes the socket away as the program ends.
pub fn stop() {
    if let Err(error) = std::fs::remove_file(socket_path()) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("nekoland-terminal: the automation socket was left: {error}");
        }
    }
}

/// Answers a client's lines until it goes; a subscription then streams its events.
fn serve(client: UnixStream, jobs: &async_channel::Sender<Job>) {
    let Ok(mut writer) = client.try_clone() else { return };
    for line in BufReader::new(client).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let (id, request) = parse(&line);
        if let Ok(Request::Subscribe { topics }) = request {
            stream_events(&mut writer, &id, topics);
            return;
        }
        let answer = request.and_then(|request| ask_main_thread(jobs, request));
        if writeln!(writer, "{}", reply(&id, &answer)).is_err() {
            return;
        }
    }
}

fn ask_main_thread(jobs: &async_channel::Sender<Job>, request: Request) -> Answer {
    let (back, answer) = mpsc::channel();
    jobs.send_blocking((request, back)).map_err(|_| "the terminal is closing".to_string())?;
    answer.recv().map_err(|_| "the terminal is closing".to_string())?
}

fn stream_events(writer: &mut UnixStream, id: &Value, topics: Vec<Topic>) {
    let names: Vec<&str> = topics.iter().map(|topic| topic.name()).collect();
    if writeln!(writer, "{}", reply(id, &Ok(json!({ "events": names })))).is_err() {
        return;
    }
    let (sender, events) = mpsc::sync_channel(BACKLOG);
    subscribers().push((topics, sender));
    for event in events {
        if writeln!(writer, "{event}").is_err() {
            return;
        }
    }
}

// --- the client: `nekoland-terminal cli` ---------------------------------------------------------------------------

/// A flag's value, or what it needs.
fn value_of<'a>(words: &mut impl Iterator<Item = &'a str>, flag: &str, what: &str) -> Result<String, String> {
    words.next().map(str::to_string).ok_or_else(|| format!("{flag} needs a value ({what})"))
}

/// What `cli <command> [options] [words]` asks for, and the socket `--socket` names. The pane is `--pane`, else
/// `pane` (the one it runs in). `send-text` sends its words joined by spaces, or `stdin` when it has none; `set-title`
/// and `notify` take their words as the title (the body after the first word of `notify`); `subscribe` the names of
/// its events. A program for `new-tab` and `split` comes after `--`.
pub fn cli_request(args: &[String], pane: Option<String>, stdin: impl FnOnce() -> String) -> Result<(Option<String>, Value), String> {
    let mut words = args.iter().map(String::as_str);
    let (mut socket, mut options, mut rest, mut program) = (None, serde_json::Map::new(), Vec::new(), None);
    let mut cmd: Option<String> = None;
    while let Some(word) = words.next() {
        match word {
            "--socket" => socket = Some(value_of(&mut words, word, "a socket")?),
            "--pane" | "-p" => {
                options.insert("pane".into(), value_of(&mut words, word, "a pane id")?.into());
            }
            "--lines" | "-n" => {
                let lines = value_of(&mut words, word, "a number")?;
                let count = lines.parse::<u64>().map_err(|_| format!("--lines takes a number, not {lines}"))?;
                options.insert("lines".into(), count.into());
            }
            "--profile" => {
                options.insert("profile".into(), value_of(&mut words, word, "a profile")?.into());
            }
            "--cwd" | "-d" => {
                options.insert("cwd".into(), value_of(&mut words, word, "a folder")?.into());
            }
            "--direction" => {
                options.insert("direction".into(), value_of(&mut words, word, "right or down")?.into());
            }
            "--" => program = Some(words.by_ref().map(str::to_string).collect::<Vec<_>>()),
            _ if cmd.is_none() => cmd = Some(word.to_string()),
            _ => rest.push(word.to_string()),
        }
    }
    let cmd = cmd.ok_or(format!("a command is missing: {COMMANDS}"))?;
    let mut request = json!({ "cmd": cmd });
    let object = request.as_object_mut().ok_or("a request is an object")?;
    let takes_pane = !matches!(cmd.as_str(), "list" | "new-tab" | "subscribe");
    // Refused, not dropped: `new-tab -p bash` means a profile, which is `--profile`.
    if !takes_pane && options.contains_key("pane") {
        return Err(format!("{cmd} takes no pane (-p, --pane); a profile is --profile"));
    }
    if takes_pane && !options.contains_key("pane") {
        if let Some(pane) = pane.filter(|pane| !pane.is_empty()) {
            options.insert("pane".into(), pane.into());
        }
    }
    object.extend(options);
    if let Some(program) = program {
        object.insert("command".into(), program.into());
    }
    match cmd.as_str() {
        "send-text" => {
            let text = if rest.is_empty() { stdin() } else { rest.join(" ") };
            object.insert("text".into(), text.into());
        }
        "set-title" => {
            object.insert("title".into(), rest.join(" ").into());
        }
        "notify" => {
            let (title, body) = rest.split_first().ok_or("notify needs a title")?;
            object.insert("title".into(), title.clone().into());
            object.insert("body".into(), body.join(" ").into());
        }
        "subscribe" if !rest.is_empty() => {
            object.insert("events".into(), rest.into());
        }
        _ if !rest.is_empty() => return Err(format!("{cmd} does not take {}", rest.join(" "))),
        _ => {}
    }
    // What the terminal would refuse is told without asking it.
    parse(&request.to_string()).1?;
    Ok((socket, request))
}

/// The socket of the newest terminal running: the one whose socket was made last and still answers.
fn newest_socket() -> Option<PathBuf> {
    let mut sockets: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(socket_folder())
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_str().and_then(socket_pid).is_some())
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    sockets.sort();
    sockets.into_iter().rev().map(|(_, path)| path).find(|path| UnixStream::connect(path).is_ok())
}

fn read_stdin() -> String {
    let mut text = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut text) {
        eprintln!("nekoland-terminal cli: could not read the text to send: {error}");
    }
    text
}

/// What the terminal answered: its result, pretty, or its error.
fn print_answer(line: &str) -> Result<(), String> {
    let answer: Value = serde_json::from_str(line).map_err(|error| format!("not an answer: {error}"))?;
    if answer.get("ok") == Some(&Value::Bool(true)) {
        let result = answer.get("result").cloned().unwrap_or(Value::Null);
        println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
        return Ok(());
    }
    Err(answer.get("error").and_then(Value::as_str).unwrap_or("the terminal refused").to_string())
}

fn talk(args: &[String]) -> Result<(), String> {
    let (socket, request) = cli_request(args, std::env::var(PANE_VAR).ok(), read_stdin)?;
    let socket = socket
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(SOCKET_VAR).map(PathBuf::from))
        .or_else(newest_socket)
        .ok_or("no Nekoland terminal is running")?;
    let mut stream = UnixStream::connect(&socket).map_err(|error| format!("{}: {error}", socket.display()))?;
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let streams = request.get("cmd").and_then(Value::as_str) == Some("subscribe");
    let mut answers = BufReader::new(stream).lines();
    let first = answers.next().ok_or("the terminal did not answer")?.map_err(|error| error.to_string())?;
    print_answer(&first)?;
    if streams {
        for event in answers {
            println!("{}", event.map_err(|error| error.to_string())?);
        }
    }
    Ok(())
}

/// Runs `nekoland-terminal cli …`; true when the terminal did what was asked.
pub fn run_cli(args: &[String]) -> bool {
    match talk(args) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("nekoland-terminal cli: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn requests_are_read_with_their_members() {
        assert_eq!(parse(r#"{"id":1,"cmd":"list"}"#), (json!(1), Ok(Request::List)));
        let (_, split) = parse(r#"{"cmd":"split","pane":"a","direction":"down","command":["htop"]}"#);
        assert_eq!(split, Ok(Request::Split { pane: "a".into(), direction: Direction::Down, cwd: None, command: vec!["htop".into()] }));
        for (line, error) in [
            ("[]", "a request is a JSON object"),
            (r#"{"cmd":"send-text","pane":"a","text":""}"#, "\"text\" is empty"),
            (r#"{"cmd":"get-text","pane":"a","lines":0}"#, "\"lines\" must be a whole number above 0"),
            (r#"{"cmd":"focus","pane":""}"#, "\"pane\" is empty"),
            (r#"{"cmd":"split","pane":"a","direction":"up"}"#, "\"direction\" is right or down, not up"),
            (r#"{"cmd":"jump"}"#, "unknown command jump"),
        ] {
            assert_eq!(parse(line).1, Err(error.to_string()), "{line}");
        }
    }

    #[test]
    fn the_cli_builds_the_request_with_the_pane_it_runs_in() {
        let here = || Some("p1".to_string());
        let (_, request) = cli_request(&words("get-text -n 5"), here(), String::new).unwrap();
        assert_eq!(request, json!({ "cmd": "get-text", "pane": "p1", "lines": 5 }));
        let (_, request) = cli_request(&words("split --direction down -- htop -d 5"), here(), String::new).unwrap();
        assert_eq!(request, json!({ "cmd": "split", "pane": "p1", "direction": "down", "command": ["htop", "-d", "5"] }));
        let (_, request) = cli_request(&words("send-text"), here(), || "piped".to_string()).unwrap();
        assert_eq!(request["text"], "piped");
        let (socket, request) = cli_request(&words("--socket /s notify Done all good"), None, String::new).unwrap();
        assert_eq!((socket.as_deref(), request["title"].as_str(), request["body"].as_str()), (Some("/s"), Some("Done"), Some("all good")));
        assert!(cli_request(&words("new-tab -p bash"), None, String::new).is_err(), "a profile is --profile");
        assert!(cli_request(&words("list extra"), None, String::new).is_err());
        assert!(cli_request(&[], None, String::new).is_err());
    }

    #[test]
    fn an_event_reaches_only_who_follows_it() {
        let (sender, events) = mpsc::sync_channel(BACKLOG);
        subscribers().push((vec![Topic::CommandFinished], sender));
        assert!(wants(Topic::CommandFinished));
        publish(Topic::Output, "p", json!({}));
        publish(Topic::CommandFinished, "p", json!({ "exit": 0 }));
        assert_eq!(events.try_recv().unwrap(), r#"{"event":"command-finished","exit":0,"pane":"p"}"#);
        assert!(events.try_recv().is_err());
        drop(events);
        publish(Topic::CommandFinished, "p", json!({}));
        assert!(!wants(Topic::CommandFinished), "a subscriber that went is dropped");
    }
}
