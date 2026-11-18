//! What Claude Code says of its session in a pane, learnt as Nekominal learns it. This program run as Claude Code's
//! status line (`--statusline`) keeps the JSON it is handed in a file of the pane; run as its hook (`--claude-hook`) it
//! notes where the session's transcript is, whose last answer fills in the context when the status line has not run.
//! The `claude` function of the shell integration hands Claude Code the settings that wire both; the pane is named by
//! [`SESSION_VAR`].

use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

use gtk::glib;
use serde_json::Value;

/// The variable that names the pane a program runs in, for the commands Claude Code runs.
pub const SESSION_VAR: &str = "NEKOLAND_TERMINAL_SESSION";
/// The variable that names the settings file the `claude` function hands to Claude Code.
pub const CLAUDE_SETTINGS_VAR: &str = "NEKOLAND_TERMINAL_CLAUDE_SETTINGS";
const STATUSLINE_FLAG: &str = "--statusline";
const HOOK_FLAG: &str = "--claude-hook";
const LINE_EXTENSION: &str = "json";
const HOOK_EXTENSION: &str = "claude";
/// The events whose hooks note the session: it starts (or is resumed, cleared, compacted), and its turns start and end.
/// Not `PostToolUse`, which also runs for every tool of every subagent.
const HOOK_EVENTS: [&str; 3] = ["SessionStart", "UserPromptSubmit", "Stop"];
/// How much of the end of a transcript is read for its last answer: a few answers' worth.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;
const DATA_FOLDER: &str = "nekoland-terminal";
const AGENTS_FOLDER: &str = "agents";
const WINDOWS_FILE: &str = "context-windows.json";
const SETTINGS_FILE: &str = "claude-settings.json";
const HOOKS_FILE: &str = "claude-hooks.json";
/// How long the files of a pane are kept unwritten: a pane's files go with it, but a terminal that crashed leaves them.
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// The context window of a model whose window the status line never gave, and the larger one past it.
const USUAL_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;
/// A part of a model's id this long in digits is its date, not its version.
const DATE_DIGITS: usize = 8;
const MILLIS_PER_SECOND: u64 = 1000;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3600;
const FULL: f64 = 100.0;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A name for a new pane, unique on this machine while the process lives.
pub fn new_session() -> String {
    format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Keeps the names handed out next apart from `name`, a pane's that the copy of this program before carried over.
pub fn continue_after(name: &str) {
    let number = name.rsplit_once('-').and_then(|(_, number)| number.parse::<u64>().ok());
    if let Some(number) = number {
        NEXT.fetch_max(number + 1, Ordering::Relaxed);
    }
}

fn data_dir() -> PathBuf {
    glib::user_data_dir().join(DATA_FOLDER)
}

/// A file of a pane with this extension; None for a name this program did not hand out, so that no name leads
/// elsewhere.
fn session_file(session: &str, extension: &str) -> Option<PathBuf> {
    let handed_out = !session.is_empty() && session.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-');
    handed_out.then(|| data_dir().join(AGENTS_FOLDER).join(format!("{session}.{extension}")))
}

/// Writes beside and moves over, flushed first, so that the window never reads half a file. The part is named by this
/// process, so that a status line and a hook that run at once do not write into one another's.
fn write_whole(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = file.with_extension(format!("{}.part", std::process::id()));
    let written = std::fs::File::create(&part)
        .and_then(|mut out| out.write_all(text.as_bytes()).and_then(|()| out.sync_all()))
        .and_then(|()| std::fs::rename(&part, file));
    if written.is_err() {
        // The part is of no use to anyone; the write's own error is the one worth telling.
        let _ = std::fs::remove_file(&part);
    }
    written
}

fn write_if_changed(file: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(file).ok().as_deref() == Some(text) {
        return Ok(());
    }
    write_whole(file, text)
}

/// Runs as one of Claude Code's commands when the program was started for one; whether it was. It prints nothing:
/// Claude Code would show a status line's output and add a hook's to the conversation.
pub fn run_as_command() -> bool {
    let keep: fn(Value) = match std::env::args().nth(1).as_deref() {
        Some(STATUSLINE_FLAG) => keep_status_line,
        Some(HOOK_FLAG) => keep_hook,
        _ => return false,
    };
    if let Some(input) = read_input() {
        keep(input);
    }
    true
}

/// One JSON value, read up to its end and not to the end of the stream, so that nothing hangs on a caller that keeps
/// stdin open.
fn read_input() -> Option<Value> {
    let mut values = serde_json::Deserializer::from_reader(std::io::stdin().lock()).into_iter::<Value>();
    values.next()?.ok()
}

fn keep_status_line(value: Value) {
    let file = std::env::var(SESSION_VAR).ok().and_then(|session| session_file(&session, LINE_EXTENSION));
    if let Some(file) = file {
        report_failure(&file, write_whole(&file, &value.to_string()));
    }
    let model = value.pointer("/model/id").and_then(Value::as_str);
    let size = value.pointer("/context_window/context_window_size").and_then(Value::as_u64);
    if let (Some(model), Some(size)) = (model, size) {
        remember_window(model, size);
    }
}

fn keep_hook(input: Value) {
    let Some(file) = std::env::var(SESSION_VAR).ok().and_then(|session| session_file(&session, HOOK_EXTENSION)) else {
        return;
    };
    let old = std::fs::read_to_string(&file).ok();
    if let Some(record) = hook_record(&input, old.as_deref()) {
        report_failure(&file, write_if_changed(&file, &record));
    }
}

fn report_failure(file: &Path, written: std::io::Result<()>) {
    if let Err(error) = written {
        eprintln!("nekoland-terminal: could not write {}: {error}", file.display());
    }
}

fn text_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// What the file of a pane keeps of a hook's input: the session, its transcript and folder, and the model when the
/// event names one; a model said before is kept while the session is the same one.
fn hook_record(input: &Value, old: Option<&str>) -> Option<String> {
    let session = text_at(input, "session_id");
    let transcript = text_at(input, "transcript_path")?;
    // A model is a string in some versions and `{id, display_name}` in others.
    let model = input.get("model").and_then(|model| {
        model.as_str().map(str::to_owned).or_else(|| text_at(model, "display_name")).or_else(|| text_at(model, "id"))
    });
    let old: Option<Value> = old.and_then(|old| serde_json::from_str(old).ok());
    let model = model.or_else(|| {
        old.as_ref()
            .filter(|old| text_at(old, "session_id") == session)
            .and_then(|old| text_at(old, "model"))
    });
    let record = serde_json::json!({
        "session_id": session,
        "transcript_path": transcript,
        "cwd": text_at(input, "cwd"),
        "model": model,
    });
    Some(record.to_string())
}

/// What the agent of a pane last said about its session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub model: Option<String>,
    /// The share of the context window in use, 0 to 100.
    pub context: Option<f64>,
    pub cost: Option<f64>,
    pub duration_ms: Option<u64>,
    pub api_duration_ms: Option<u64>,
    /// The tokens of the last request, whose sum of the first three is the context in use, and of its answer.
    pub input_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// The size of the context window the share is of.
    pub context_window: Option<u64>,
    /// The whole session's tokens and lines.
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    /// The session's transcript, beside which Claude Code keeps its subagents.
    pub transcript: Option<PathBuf>,
}

impl Usage {
    /// The tokens of the context in use.
    pub fn used_tokens(&self) -> Option<u64> {
        let parts = [self.input_tokens, self.cache_creation_tokens, self.cache_read_tokens];
        parts.iter().any(Option::is_some).then(|| parts.iter().flatten().sum())
    }
}

/// The JSON Claude Code hands its `statusLine` command, read as text.
#[cfg(test)]
fn parse_usage(json: &str) -> Option<Usage> {
    let value: Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    Some(usage_of(&value))
}

fn usage_of(value: &Value) -> Usage {
    let text = |pointer: &str| {
        value.pointer(pointer).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
    };
    // The context is null until the first answer.
    let number = |pointer: &str| {
        value.pointer(pointer).and_then(Value::as_f64).filter(|number| number.is_finite() && *number >= 0.0)
    };
    let count = |pointer: &str| number(pointer).map(|number| number as u64);
    Usage {
        model: text("/model/display_name"),
        context: number("/context_window/used_percentage").map(|share| share.min(FULL)),
        cost: number("/cost/total_cost_usd"),
        duration_ms: count("/cost/total_duration_ms"),
        api_duration_ms: count("/cost/total_api_duration_ms"),
        input_tokens: count("/context_window/current_usage/input_tokens"),
        cache_creation_tokens: count("/context_window/current_usage/cache_creation_input_tokens"),
        cache_read_tokens: count("/context_window/current_usage/cache_read_input_tokens"),
        output_tokens: count("/context_window/current_usage/output_tokens"),
        context_window: count("/context_window/context_window_size").filter(|&size| size > 0),
        total_input_tokens: count("/context_window/total_input_tokens"),
        total_output_tokens: count("/context_window/total_output_tokens"),
        lines_added: count("/cost/total_lines_added"),
        lines_removed: count("/cost/total_lines_removed"),
        transcript: text("/transcript_path").map(PathBuf::from),
    }
}

/// When the files a pane's usage comes from were last written: the status line's, the hooks', and the transcript the
/// hooks named. Read every second, so a pane whose stamp did not move is not read again.
pub type Stamp = [Option<SystemTime>; 3];

fn modified(file: &Path) -> Option<SystemTime> {
    std::fs::metadata(file).ok()?.modified().ok()
}

fn read_json(file: &Path) -> Option<Value> {
    serde_json::from_str(std::fs::read_to_string(file).ok()?.trim_start_matches('\u{feff}')).ok()
}

fn hook_transcript(record: &Value) -> Option<PathBuf> {
    text_at(record, "transcript_path").map(PathBuf::from)
}

/// The stamp of a pane's files; None while Claude Code has said nothing in it.
pub fn stamp(session: &str) -> Option<Stamp> {
    let line = modified(&session_file(session, LINE_EXTENSION)?);
    let hook_file = session_file(session, HOOK_EXTENSION)?;
    let hook = modified(&hook_file);
    let transcript = hook
        .and_then(|_| read_json(&hook_file))
        .and_then(|record| hook_transcript(&record))
        .and_then(|transcript| modified(&transcript));
    (line.is_some() || hook.is_some()).then_some([line, hook, transcript])
}

/// What the status line last said of a pane's session, with the transcript's last answer filling in a context it has
/// not said or said before that answer.
pub fn read_usage(session: &str) -> Option<Usage> {
    let line_file = session_file(session, LINE_EXTENSION)?;
    let line_value = read_json(&line_file);
    let hook = read_json(&session_file(session, HOOK_EXTENSION)?);
    if line_value.is_none() && hook.is_none() {
        return None;
    }
    let line = line_value.as_ref().map(|value| Line {
        usage: usage_of(value),
        session: text_at(value, "session_id"),
        at: modified(&line_file),
    });
    let transcript = hook.as_ref().and_then(hook_transcript);
    let turn = transcript.as_deref().and_then(read_tail).and_then(|(tail, cut)| last_turn(&tail, cut));
    let answered = turn.map(|turn| (turn, transcript.as_deref().and_then(modified)));
    let mut usage = combine(line, hook.as_ref().and_then(|hook| text_at(hook, "session_id")), answered);
    usage.transcript = transcript.or(usage.transcript);
    usage.model = usage.model.or_else(|| hook.as_ref().and_then(|hook| text_at(hook, "model")).map(|id| model_name(&id)));
    Some(usage)
}

/// The status line's last run: what it said, of which session, and when.
struct Line {
    usage: Usage,
    session: Option<String>,
    at: Option<SystemTime>,
}

/// The context comes from the newer of the status line and the transcript; what only the status line knows (cost,
/// time, lines, totals) is kept from its last run. A status line of another session than the hooks follow is the
/// pane's history, and left out.
fn combine(line: Option<Line>, session: Option<String>, turn: Option<(Turn, Option<SystemTime>)>) -> Usage {
    let line = line.filter(|line| session.is_none() || line.session.is_none() || line.session == session);
    let line_at = line.as_ref().and_then(|line| line.at);
    let mut usage = line.map(|line| line.usage).unwrap_or_default();
    if let Some((turn, at)) = turn {
        let newer = usage.context.is_none()
            || match (at, line_at) {
                (Some(at), Some(line_at)) => at > line_at,
                _ => true,
            };
        if newer {
            let window = usage
                .context_window
                .unwrap_or_else(|| window_size(known_window(&turn.model), turn.context()));
            fill_from_turn(&mut usage, &turn, window);
        }
    }
    usage
}

/// The last answer of a session's main conversation, as its transcript has it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Turn {
    model: String,
    input: u64,
    cache_creation: u64,
    cache_read: u64,
    output: u64,
}

impl Turn {
    /// The context the request of the answer took: what it sent, wrote to the cache, and read from it.
    fn context(&self) -> u64 {
        self.input + self.cache_creation + self.cache_read
    }
}

/// The end of a transcript, and whether it was cut (its first line then is most likely a part of one).
pub fn read_tail(file: &Path) -> Option<(String, bool)> {
    let mut input = std::fs::File::open(file).ok()?;
    let length = input.metadata().ok()?.len();
    let start = length.saturating_sub(TRANSCRIPT_TAIL);
    input.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).ok()?;
    Some((String::from_utf8_lossy(&bytes).into_owned(), start > 0))
}

/// The last answer with tokens: not a subagent's (`isSidechain`), and not one Claude Code made up itself (model
/// `<synthetic>`).
fn last_turn(tail: &str, cut: bool) -> Option<Turn> {
    let whole = if cut { tail.split_once('\n').map_or("", |(_, rest)| rest) } else { tail };
    whole.lines().rev().find_map(|line| {
        let entry: Value = serde_json::from_str(line).ok()?;
        let sidechain = entry.get("isSidechain").and_then(Value::as_bool) == Some(true);
        if entry.get("type")?.as_str()? != "assistant" || sidechain {
            return None;
        }
        let message = entry.get("message")?;
        let model = message.get("model")?.as_str().filter(|model| !model.is_empty() && *model != "<synthetic>")?;
        let usage = message.get("usage")?;
        let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let turn = Turn {
            model: model.to_owned(),
            input: count("input_tokens"),
            cache_creation: count("cache_creation_input_tokens"),
            cache_read: count("cache_read_input_tokens"),
            output: count("output_tokens"),
        };
        (turn.context() > 0).then_some(turn)
    })
}

fn fill_from_turn(usage: &mut Usage, turn: &Turn, window: u64) {
    // The newer answer's model: the session may have changed it since the status line last ran.
    usage.model = Some(model_name(&turn.model));
    usage.context = Some((turn.context() as f64 * FULL / window as f64).min(FULL));
    usage.context_window = Some(window);
    usage.input_tokens = Some(turn.input);
    usage.cache_creation_tokens = Some(turn.cache_creation);
    usage.cache_read_tokens = Some(turn.cache_read);
    usage.output_tokens = Some(turn.output);
}

/// The window a model's context is measured against: the one the status line once gave for it, else the usual one,
/// unless more than that is in use already (a larger window under the same id).
fn window_size(known: Option<u64>, used: u64) -> u64 {
    known
        .filter(|&size| size >= used)
        .unwrap_or(if used > USUAL_WINDOW { LARGE_WINDOW } else { USUAL_WINDOW })
}

/// The name a model goes by, from its id: `claude-opus-4-1-20250805` is "Opus 4.1"; another id stays as it is.
pub fn model_name(id: &str) -> String {
    let Some(rest) = id.strip_prefix("claude-") else {
        return id.to_owned();
    };
    let is_number = |part: &&str| part.bytes().all(|byte| byte.is_ascii_digit());
    let parts: Vec<&str> = rest.split('-').filter(|part| !part.is_empty()).collect();
    let names: Vec<String> = parts
        .iter()
        .filter(|part| !is_number(part))
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        })
        .collect();
    let version: Vec<&str> = parts.iter().copied().filter(|part| is_number(part) && part.len() < DATE_DIGITS).collect();
    if names.is_empty() {
        return id.to_owned();
    }
    format!("{} {}", names.join(" "), version.join(".")).trim_end().to_owned()
}

fn windows_file() -> PathBuf {
    data_dir().join(WINDOWS_FILE)
}

fn known_window(model: &str) -> Option<u64> {
    read_json(&windows_file())?.get(model)?.as_u64().filter(|&size| size > 0)
}

/// Keeps the window the status line gave for a model, for the transcript's answers of later sessions.
fn remember_window(model: &str, size: u64) {
    if size == 0 || known_window(model) == Some(size) {
        return;
    }
    let file = windows_file();
    let mut known = match read_json(&file) {
        Some(Value::Object(known)) => known,
        _ => serde_json::Map::new(),
    };
    known.insert(model.to_owned(), size.into());
    report_failure(&file, write_whole(&file, &Value::Object(known).to_string()));
}

/// Takes the files of a pane that is gone.
pub fn forget(session: &str) {
    for extension in [LINE_EXTENSION, HOOK_EXTENSION] {
        let Some(file) = session_file(session, extension) else {
            continue;
        };
        if let Err(error) = std::fs::remove_file(&file) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("nekoland-terminal: could not remove {}: {error}", file.display());
            }
        }
    }
}

/// Deletes the files of panes, and the parts of their writes, that no one has written for [`STALE_AFTER`].
pub fn clean_stale() {
    clean_stale_in(&data_dir().join(AGENTS_FOLDER), SystemTime::now());
}

fn clean_stale_in(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_pane_file = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.split('.').next().and_then(|session| session_file(session, "")).is_some());
        let stale = modified(&path).and_then(|at| now.duration_since(at).ok()).is_some_and(|age| age > STALE_AFTER);
        if is_pane_file && stale {
            if let Err(error) = std::fs::remove_file(&path) {
                eprintln!("nekoland-terminal: could not remove {}: {error}", path.display());
            }
        }
    }
}

/// Whether Claude Code's own settings set a status line, which the window then leaves in place.
fn has_own_status_line() -> bool {
    let file = glib::home_dir().join(".claude").join("settings.json");
    read_json(&file).and_then(|settings| settings.get("statusLine").cloned()).is_some_and(|line| !line.is_null())
}

/// This program run with `flag`, quoted for the shell Claude Code runs it in.
fn command(exe: &Path, flag: &str) -> String {
    format!("{} {flag}", glib::shell_quote(exe).to_string_lossy())
}

/// The settings Claude Code takes with `--settings` in a pane: this program as the hooks that note the session, and as
/// the status line when `status_line`.
fn settings_json(exe: &Path, status_line: bool) -> String {
    let hook = serde_json::json!([{ "hooks": [{ "type": "command", "command": command(exe, HOOK_FLAG) }] }]);
    let hooks: serde_json::Map<String, Value> =
        HOOK_EVENTS.iter().map(|event| ((*event).to_owned(), hook.clone())).collect();
    let mut settings = serde_json::json!({ "hooks": hooks });
    if status_line {
        settings["statusLine"] = serde_json::json!({ "type": "command", "command": command(exe, STATUSLINE_FLAG) });
    }
    settings.to_string()
}

/// Writes the two settings files `claude` can be handed, rewritten when this program moved: the status line and the
/// hooks, and the hooks alone for a status line of the user's own (the `claude` function also picks it for a project
/// that sets one). Returns the one for a pane.
pub fn claude_settings_file() -> Option<PathBuf> {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("nekoland-terminal: could not find its own program for Claude Code: {error}");
            return None;
        }
    };
    let dir = data_dir();
    for (name, status_line) in [(SETTINGS_FILE, true), (HOOKS_FILE, false)] {
        let file = dir.join(name);
        if let Err(error) = write_if_changed(&file, &settings_json(&exe, status_line)) {
            eprintln!("nekoland-terminal: could not write {}: {error}", file.display());
            return None;
        }
    }
    Some(dir.join(if has_own_status_line() { HOOKS_FILE } else { SETTINGS_FILE }))
}

/// A length of time as a clock: `4:05`, or `1:42:10` past an hour.
pub fn clock(ms: u64) -> String {
    let seconds = ms / MILLIS_PER_SECOND;
    let minutes = seconds / SECONDS_PER_MINUTE % SECONDS_PER_MINUTE;
    let rest = seconds % SECONDS_PER_MINUTE;
    if seconds >= SECONDS_PER_HOUR {
        format!("{}:{minutes:02}:{rest:02}", seconds / SECONDS_PER_HOUR)
    } else {
        format!("{}:{rest:02}", seconds / SECONDS_PER_MINUTE)
    }
}

pub fn cost_text(cost: f64) -> String {
    format!("${cost:.2}")
}

/// A count of tokens as the bar says it: `2.3k`, `12.4M`.
pub fn tokens_text(tokens: u64) -> String {
    const THOUSAND: f64 = 1_000.0;
    const MILLION: f64 = 1_000_000.0;
    let tokens = tokens as f64;
    if tokens >= MILLION {
        format!("{:.1}M", tokens / MILLION)
    } else if tokens >= THOUSAND {
        format!("{:.1}k", tokens / THOUSAND)
    } else {
        format!("{tokens}")
    }
}

/// The size of a context window, a round number as it is: `1M`, `200k`.
pub fn window_text(tokens: u64) -> String {
    tokens_text(tokens).replace(".0", "")
}

/// The share of the context and the cost, as a row's line of the context says them.
pub fn context_text(share: f64, cost: Option<f64>) -> String {
    match cost {
        Some(cost) => format!("{share:.0}% · {}", cost_text(cost)),
        None => format!("{share:.0}%"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_json_gives_the_numbers_of_the_bar() {
        let json = r#"{"session_id":"abc","model":{"id":"claude-opus","display_name":"Opus"},
            "cost":{"total_cost_usd":0.8412,"total_duration_ms":134000},"context_window":{"context_window_size":200000,"used_percentage":42}}"#;
        let usage = parse_usage(json).unwrap();
        assert_eq!(usage.model.as_deref(), Some("Opus"));
        assert_eq!(usage.context, Some(42.0));
        assert_eq!(usage.cost, Some(0.8412));
        assert_eq!(usage.duration_ms, Some(134_000));
        assert_eq!(parse_usage(r#"{"model":{}}"#), Some(Usage::default()));
        assert_eq!(parse_usage("not json"), None);
        assert_eq!(parse_usage(r#"{"context_window":{"used_percentage":250}}"#).unwrap().context, Some(FULL));
    }

    #[test]
    fn the_last_main_answer_of_a_transcript_is_the_context() {
        let tail = concat!(
            "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-opus-4-1\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":90}}}\n",
            "{\"type\":\"assistant\",\"isSidechain\":true,\"message\":{\"model\":\"claude-haiku-4\",\"usage\":{\"input_tokens\":5}}}\n",
            "{\"type\":\"assistant\",\"message\":{\"model\":\"<synthetic>\",\"usage\":{\"input_tokens\":0}}}\n",
        );
        let turn = last_turn(tail, false).unwrap();
        assert_eq!(turn.model, "claude-opus-4-1");
        assert_eq!(turn.context(), 100);
        assert!(last_turn(tail.lines().next().unwrap(), true).is_none(), "a cut first line is a part of one");
    }

    #[test]
    fn a_newer_answer_than_the_status_line_gives_the_context() {
        let then = SystemTime::UNIX_EPOCH;
        let later = then + Duration::from_secs(1);
        let line = || Line {
            usage: Usage { context: Some(10.0), cost: Some(1.0), context_window: Some(1_000), ..Usage::default() },
            session: Some("a".into()),
            at: Some(then),
        };
        let turn = Turn { model: "claude-opus-4-1".into(), input: 500, ..Turn::default() };
        let usage = combine(Some(line()), Some("a".into()), Some((turn.clone(), Some(later))));
        assert_eq!((usage.context, usage.cost), (Some(50.0), Some(1.0)));
        let usage = combine(Some(line()), Some("b".into()), None);
        assert_eq!(usage, Usage::default(), "another session's status line is history");
    }

    #[test]
    fn models_go_by_their_names() {
        assert_eq!(model_name("claude-opus-4-1-20250805"), "Opus 4.1");
        assert_eq!(model_name("claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(model_name("gpt-5"), "gpt-5");
    }

    #[test]
    fn windows_grow_past_what_is_in_use() {
        assert_eq!(window_size(None, 10), USUAL_WINDOW);
        assert_eq!(window_size(None, USUAL_WINDOW + 1), LARGE_WINDOW);
        assert_eq!(window_size(Some(500), 600), USUAL_WINDOW);
    }

    #[test]
    fn a_pane_name_is_only_ever_one_this_program_hands_out() {
        assert!(new_session().bytes().all(|byte| byte.is_ascii_digit() || byte == b'-'));
        assert_ne!(new_session(), new_session());
        assert!(session_file("../../evil", LINE_EXTENSION).is_none());
        assert!(session_file("", LINE_EXTENSION).is_none());
    }

    #[test]
    fn the_files_of_panes_unwritten_for_a_week_go_and_nothing_else_does() {
        let dir = std::env::temp_dir().join(format!("nekoland-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["123-1.json", "123-1.claude", "123-1.json.456.part", "notes.txt"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        clean_stale_in(&dir, SystemTime::now());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4);
        clean_stale_in(&dir, SystemTime::now() + STALE_AFTER + Duration::from_secs(60));
        let left: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["notes.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn times_read_as_clocks_and_tokens_as_counts() {
        assert_eq!(clock(245_000), "4:05");
        assert_eq!(clock(6_130_000), "1:42:10");
        assert_eq!(tokens_text(2_300), "2.3k");
        assert_eq!(tokens_text(12_400_000), "12.4M");
        assert_eq!(window_text(1_000_000), "1M");
        assert_eq!(window_text(200_000), "200k");
        assert_eq!(context_text(58.4, Some(4.123)), "58% · $4.12");
    }
}
