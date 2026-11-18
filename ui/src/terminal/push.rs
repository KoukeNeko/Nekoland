//! Notifications pushed on to a phone, as Nekominal's: an ntfy topic, or a webhook of the user's own that takes JSON.
//! One thread sends them with curl, so the terminal takes no HTTP library. A push that fails is dropped: a phone told a
//! minute late after retries is worth less than a window that never waits on the network.

use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{
        mpsc::{sync_channel, SyncSender, TrySendError},
        OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use gtk::glib;
use serde::{Deserialize, Serialize};

use super::lang::tr;
use super::config::{self, Choice};

/// Pushes are sent one after another; more than this waiting are dropped, so a target that hangs cannot pile them up.
const QUEUE: usize = 8;
/// Discord takes no longer `content`.
const MOST_CONTENT: usize = 2000;
/// Seconds curl may take for one push.
const TIME_LIMIT: u32 = 5;
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];
const DISCORD_HOSTS: [&str; 4] = ["discord.com", "discordapp.com", "canary.discord.com", "ptb.discord.com"];
const SENDER_NAME: &str = "Nekoland";
pub const BAD_URL: &str = "網址要是 https://（http:// 只能連到這台電腦）";
pub const TEST_TITLE: &str = "終端機";
pub const TEST_BODY: &str = "終端機的測試通知";

/// Where a notification is pushed to. An empty `url` turns it off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Push {
    pub url: String,
    pub kind: PushKind,
    /// Sent as `Authorization: Bearer`; empty sends none.
    pub token: String,
    pub when: PushWhen,
    pub events: Vec<PushEvent>,
}

impl Default for Push {
    fn default() -> Self {
        // What needs the user or ends the work: the rest stays on the desk.
        Self {
            url: String::new(),
            kind: PushKind::Ntfy,
            token: String::new(),
            when: PushWhen::Away,
            events: vec![PushEvent::Waiting, PushEvent::Failed, PushEvent::Done],
        }
    }
}

/// What the target of the push speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushKind {
    /// The text as the body, the rest in ntfy's headers.
    #[default]
    Ntfy,
    /// A JSON object for any receiver.
    Webhook,
}

impl Choice for PushKind {
    const ALL: &'static [Self] = &[Self::Ntfy, Self::Webhook];

    fn label(self) -> &'static str {
        match self {
            Self::Ntfy => "ntfy",
            Self::Webhook => "Webhook",
        }
    }
}

/// When a notification is also pushed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushWhen {
    /// Only while no window of the terminal is in front: at the desk the desktop's notification is enough.
    #[default]
    Away,
    Always,
}

impl Choice for PushWhen {
    const ALL: &'static [Self] = &[Self::Away, Self::Always];

    fn label(self) -> &'static str {
        match self {
            Self::Away => tr("不在電腦前"),
            Self::Always => tr("一律"),
        }
    }
}

/// A kind of notification that can be pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushEvent {
    Waiting,
    Failed,
    Done,
    LongCommand,
    /// A program's own notification, or a trigger's.
    Message,
}

impl PushEvent {
    pub const ALL: [Self; 5] = [Self::Waiting, Self::Failed, Self::Done, Self::LongCommand, Self::Message];

    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => tr("等待輸入"),
            Self::Failed => tr("失敗"),
            Self::Done => tr("完成"),
            Self::LongCommand => tr("長指令結束"),
            Self::Message => tr("程式的通知"),
        }
    }
}

/// What happened, which decides the choice it is pushed under, ntfy's tags and whether it is urgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Happening {
    Waiting,
    Failed,
    Done,
    Command,
    CommandFailed,
    Message,
}

impl Happening {
    /// Its word in the webhook's JSON.
    fn word(self) -> &'static str {
        match self {
            Self::Waiting => "blocked",
            Self::Failed => "error",
            Self::Done => "done",
            Self::Command => "command",
            Self::CommandFailed => "commandFailed",
            Self::Message => "message",
        }
    }

    pub fn event(self) -> PushEvent {
        match self {
            Self::Waiting => PushEvent::Waiting,
            Self::Failed => PushEvent::Failed,
            Self::Done => PushEvent::Done,
            Self::Command | Self::CommandFailed => PushEvent::LongCommand,
            Self::Message => PushEvent::Message,
        }
    }

    fn tags(self) -> &'static str {
        match self {
            Self::Waiting => "robot,question",
            Self::Failed | Self::CommandFailed => "warning",
            Self::Message => "speech_balloon",
            Self::Command => "white_check_mark",
            Self::Done => "robot",
        }
    }

    fn is_urgent(self) -> bool {
        matches!(self, Self::Waiting | Self::Failed | Self::CommandFailed)
    }
}

/// What one push says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub title: String,
    pub body: String,
    pub happening: Happening,
    /// The id of the tab.
    pub tab: u64,
    /// When, in seconds since 1970.
    pub at: u64,
}

impl Message {
    pub fn new(title: &str, body: &str, happening: Happening, tab: u64) -> Self {
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        Self { title: title.to_owned(), body: body.to_owned(), happening, tab, at }
    }
}

/// Whether a notification goes to the phone now: a target is set, the user chose its kind, and the user is away from
/// the terminal unless the setting says always.
pub fn wanted(push: &Push, terminal_in_front: bool, happening: Happening) -> bool {
    !push.url.is_empty()
        && push.events.contains(&happening.event())
        && (push.when == PushWhen::Always || !terminal_in_front)
}

/// Whether `url` is taken as the target: the token and the text would cross the network in the clear over anything but
/// HTTPS, so plain HTTP is only for a receiver on this computer. Empty is off, and taken.
pub fn is_valid_url(url: &str) -> bool {
    if url.is_empty() {
        return true;
    }
    if url.chars().any(|letter| letter.is_whitespace() || letter.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    let host = host(url).unwrap_or_default().to_ascii_lowercase();
    (lower.starts_with("https://") && !host.is_empty()) || (lower.starts_with("http://") && LOCAL_HOSTS.contains(&host.as_str()))
}

/// The host of `url`, without user, port, path or query: what the log may say of an address.
pub fn host(url: &str) -> Option<&str> {
    let rest = &url[url.find("://")? + 3..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    Some(match host.find(']') {
        Some(end) if host.starts_with('[') => &host[..=end],
        _ => host.split(':').next().unwrap_or(host),
    })
}

/// The headers and the body of the POST for the kind of the target.
pub fn request(push: &Push, message: &Message) -> (Vec<(&'static str, String)>, String) {
    let mut headers = Vec::new();
    if !push.token.is_empty() {
        headers.push(("Authorization", format!("Bearer {}", push.token)));
    }
    let body = match push.kind {
        PushKind::Ntfy => {
            headers.push(("Content-Type", "text/plain; charset=utf-8".to_owned()));
            headers.push(("Title", header_text(&message.title)));
            headers.push(("Tags", message.happening.tags().to_owned()));
            if message.happening.is_urgent() {
                headers.push(("Priority", "high".to_owned()));
            }
            message.body.clone()
        }
        PushKind::Webhook => {
            headers.push(("Content-Type", "application/json".to_owned()));
            // Discord refuses a post without `content` and Slack reads `text`; both ignore the rest.
            let text: String = format!("**{}**\n{}", message.title, message.body).chars().take(MOST_CONTENT).collect();
            let mut json = serde_json::json!({
                "content": text,
                "text": text,
                "title": message.title,
                "body": message.body,
                "kind": message.happening.word(),
                "tab": message.tab,
                "at": message.at,
            });
            if host(&push.url).is_some_and(|host| DISCORD_HOSTS.contains(&host)) {
                json["username"] = SENDER_NAME.into();
            }
            json.to_string()
        }
    };
    (headers, body)
}

/// A header value that stays one header: on one line, and in ASCII, with anything else as an RFC 2047 word, which ntfy
/// reads.
fn header_text(text: &str) -> String {
    let line = text.replace(['\r', '\n'], " ");
    if line.is_ascii() {
        line
    } else {
        format!("=?UTF-8?B?{}?=", glib::base64_encode(line.as_bytes()))
    }
}

fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for letter in value.chars() {
        match letter {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The request as a config of curl, read from its standard input: the address, the token and the text never appear on
/// a command line another process can read.
fn curl_config(url: &str, headers: &[(&'static str, String)], body: &str) -> String {
    // No --location: a redirect could lead the token to plain HTTP or another host.
    let mut config = format!("silent\nfail\nmax-time = {TIME_LIMIT}\nproto = \"=http,https\"\nurl = {}\n", quoted(url));
    for (name, value) in headers {
        let value = value.replace(['\r', '\n'], " ");
        config.push_str(&format!("header = {}\n", quoted(&format!("{name}: {value}"))));
    }
    // data-raw: a body that starts with @ is text, not the name of a file to send.
    config.push_str(&format!("data-raw = {}\n", quoted(body)));
    config
}

/// Sends the request with curl; blocks for up to its time limit and a little more.
fn post(config: &str) -> Result<(), String> {
    let mut child = Command::new("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("curl 無法啟動：{error}"))?;
    let mut input = child.stdin.take().ok_or(tr("curl 沒有輸入"))?;
    input.write_all(config.as_bytes()).map_err(|error| format!("curl：{error}"))?;
    drop(input);
    let status = child.wait().map_err(|error| format!("curl：{error}"))?;
    // curl's exit code names the failure (6: no host, 22: an HTTP error, 28: the time ran out) without the address.
    if status.success() {
        Ok(())
    } else {
        Err(format!("curl 結束代碼 {}", status.code().unwrap_or(-1)))
    }
}

/// Sends one push now, where blocking does not matter.
fn deliver(push: &Push, message: &Message) -> Result<(), String> {
    if !is_valid_url(&push.url) || push.url.is_empty() {
        return Err(tr(BAD_URL).to_owned());
    }
    let (headers, body) = request(push, message);
    post(&curl_config(&push.url, &headers, &body))
}

/// Hands the push to the thread that sends it; returns at once.
pub fn send(push: &Push, message: Message) {
    static SENDER: OnceLock<Option<SyncSender<(Push, Message)>>> = OnceLock::new();
    let sender = SENDER.get_or_init(|| {
        let (sender, jobs) = sync_channel::<(Push, Message)>(QUEUE);
        let spawned = std::thread::Builder::new().name("push".into()).spawn(move || {
            // A failure is logged once until a push gets through again, not once for every agent that finishes.
            let mut failing = false;
            for (push, message) in jobs {
                match deliver(&push, &message) {
                    Ok(()) => failing = false,
                    Err(error) if !failing => {
                        failing = true;
                        eprintln!("nekoland-terminal: push to {}: {error}", host(&push.url).unwrap_or("?"));
                    }
                    Err(_) => {}
                }
            }
        });
        match spawned {
            Ok(_) => Some(sender),
            Err(error) => {
                eprintln!("nekoland-terminal: could not start pushing: {error}");
                None
            }
        }
    });
    let Some(sender) = sender else {
        return;
    };
    if let Err(TrySendError::Full(_)) = sender.try_send((push.clone(), message)) {
        eprintln!("nekoland-terminal: push: the queue is full, a notification was dropped");
    }
}

/// Pushes what happened in tab `tab` when the settings want it.
pub fn notify(title: &str, body: &str, happening: Happening, tab: u64, terminal_in_front: bool) {
    let push = config::current().push;
    if wanted(&push, terminal_in_front, happening) {
        send(&push, Message::new(title, body, happening, tab));
    }
}

/// Sends a test push off the main thread; `done` is told what it came to.
pub fn send_test(push: Push, done: impl FnOnce(Result<(), String>) + 'static) {
    let message = Message::new(tr(TEST_TITLE), tr(TEST_BODY), Happening::Message, 0);
    let task = gtk::gio::spawn_blocking(move || deliver(&push, &message));
    glib::MainContext::default().spawn_local(async move {
        done(task.await.unwrap_or_else(|_| Err(tr("測試沒有完成").to_owned())));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(happening: Happening) -> Message {
        Message { title: "claude · api".into(), body: "Waiting\nAllow edit?".into(), happening, tab: 7, at: 1_700_000_000 }
    }

    fn target(kind: PushKind, token: &str) -> Push {
        Push { url: "https://ntfy.sh/cat".into(), kind, token: token.into(), ..Push::default() }
    }

    fn header<'a>(headers: &'a [(&str, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(listed, _)| *listed == name).map(|(_, value)| value.as_str())
    }

    #[test]
    fn ntfy_gets_the_text_as_the_body_and_the_rest_in_its_headers() {
        let (headers, body) = request(&target(PushKind::Ntfy, ""), &message(Happening::Waiting));
        assert_eq!(body, "Waiting\nAllow edit?");
        assert_eq!(header(&headers, "Tags"), Some("robot,question"));
        assert_eq!(header(&headers, "Priority"), Some("high"));
        assert_eq!(header(&headers, "Authorization"), None);
        assert_eq!(header(&headers, "Title"), Some("=?UTF-8?B?Y2xhdWRlIMK3IGFwaQ==?="));
        let (headers, _) = request(&target(PushKind::Ntfy, "tk"), &Message { title: "a\nb".into(), ..message(Happening::Done) });
        assert_eq!(header(&headers, "Title"), Some("a b"));
        assert_eq!(header(&headers, "Priority"), None);
        assert_eq!(header(&headers, "Authorization"), Some("Bearer tk"));
    }

    #[test]
    fn a_webhook_gets_json() {
        let (headers, body) = request(&target(PushKind::Webhook, ""), &message(Happening::CommandFailed));
        assert_eq!(header(&headers, "Content-Type"), Some("application/json"));
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["kind"], "commandFailed");
        assert_eq!(json["text"], "**claude · api**\nWaiting\nAllow edit?");
        assert_eq!(json["tab"], 7);
    }

    #[test]
    fn away_pushes_only_while_the_terminal_is_behind() {
        let mut push = target(PushKind::Ntfy, "");
        assert!(wanted(&push, false, Happening::Waiting));
        assert!(!wanted(&push, true, Happening::Waiting));
        assert!(!wanted(&push, false, Happening::Command), "a long command is not chosen by default");
        push.when = PushWhen::Always;
        assert!(wanted(&push, true, Happening::Done));
        push.url.clear();
        assert!(!wanted(&push, false, Happening::Done));
    }

    #[test]
    fn only_https_or_this_computer_is_taken() {
        assert!(is_valid_url(""));
        assert!(is_valid_url("https://ntfy.sh/cat"));
        assert!(is_valid_url("http://localhost:8080/x"));
        assert!(!is_valid_url("http://ntfy.sh/cat"));
        assert!(!is_valid_url("https://ntfy.sh/a b"));
        assert_eq!(host("https://user@[::1]:80/x"), Some("[::1]"));
    }

    #[test]
    fn the_curl_config_keeps_every_value_on_its_line() {
        let config = curl_config("https://ntfy.sh/cat", &[("Title", "a \"b\" \\ c".into())], "1\n2\t@file");
        assert!(config.contains("header = \"Title: a \\\"b\\\" \\\\ c\"\n"));
        assert!(config.contains("data-raw = \"1\\n2\\t@file\"\n"));
        assert_eq!(config.lines().count(), 7);
    }
}
