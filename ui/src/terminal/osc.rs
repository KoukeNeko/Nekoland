//! Reads, from what a shell and its programs write, what the screen does not show: the bell,
//! the agents' OSC 7501 reports and their question whether the terminal reads them, the
//! shell's OSC 133 command marks, OSC 9 and 777 notifications, OSC 9;4 progress and OSC 52's
//! clipboard, which the screen leaves alone. The
//! bytes themselves go on to the screen as they are; the scanner only watches them pass,
//! and keeps its place between reads, as a sequence can be split across two.

use base64::{engine::general_purpose::STANDARD, Engine};

use super::{agent::Report, kitty_keyboard};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
/// CAN and SUB end a sequence where they stand, as in any terminal.
const CANCEL: u8 = 0x18;
const SUBSTITUTE: u8 = 0x1a;
/// The longest parameters of a CSI the scanner keeps: a keyboard request is a few digits.
const CSI_LIMIT: usize = 32;
/// The final byte of the kitty keyboard requests.
const KEYBOARD_FINAL: u8 = b'u';
/// The final bytes of a private mode set and reset, and the modes that swap in the alternate screen.
const MODE_SET: u8 = b'h';
const MODE_RESET: u8 = b'l';
const ALTERNATE_SCREEN_MODES: [&str; 3] = ["1049", "1047", "47"];
/// The longest command the scanner keeps; OSC 7501 allows no more.
const OSC_LIMIT: usize = 4096;
/// What a copy through OSC 52 may hold, in its base64.
const CLIPBOARD_LIMIT: usize = 1024 * 1024;
const CLIPBOARD_CODE: &str = "52";
const CLIPBOARD_PREFIX: &[u8] = b"52;";

const AGENT_CODE: &str = "7501";
const MARK_CODE: &str = "133";
const PROMPT_MARK: &[u8] = b"\x1b]133;A";
const PROMPT_END_MARK: &[u8] = b"\x1b]133;B";
const NOTIFY_CODE: &str = "9";
const NOTIFY_TITLED_CODE: &str = "777";
const PROGRESS_PREFIX: &str = "4;";
const QUERY: &str = "?";
const PROGRESS_CLEAR: &str = "0";
const PROGRESS_MAX: u8 = 100;

/// How a command was ended; an answer is ended the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminator {
    Bell,
    StringTerminator,
}

impl Terminator {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Terminator::Bell => b"\x07",
            Terminator::StringTerminator => b"\x1b\\",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    Bell,
    /// An agent asks whether its reports are read; the terminal says so before anything
    /// else it answers.
    AgentQuery(Terminator),
    AgentReport(Report),
    PromptStart,
    CommandStart,
    /// The command's exit status, when the shell told it.
    CommandEnd(Option<i32>),
    Notification {
        title: Option<String>,
        body: String,
    },
    /// Percent done; None when the program cleared it.
    Progress(Option<u8>),
    /// A program puts text on the clipboard.
    ClipboardWrite(String),
    /// A program asks what the clipboard holds; the answer ends as the question did.
    ClipboardRead(Terminator),
    /// A program asks for the kitty keyboard protocol, or what of it is on.
    Keyboard(kitty_keyboard::Request),
    /// The alternate screen comes (true) or goes.
    AlternateScreen(bool),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Ground,
    Escape,
    Command,
    CommandEscape,
    /// In a command too long to keep, until it ends.
    Overflow,
    OverflowEscape,
    /// In a CSI, its parameters kept for the keyboard requests.
    Csi,
}

#[derive(Default)]
pub struct Scanner {
    mode: Mode,
    command: Vec<u8>,
    csi: Vec<u8>,
}

impl Scanner {
    /// What `bytes` tell, in order, added to `signals`.
    pub fn scan(&mut self, bytes: &[u8], signals: &mut Vec<Signal>) {
        for &byte in bytes {
            self.mode = self.step(byte, signals);
        }
    }

    fn step(&mut self, byte: u8, signals: &mut Vec<Signal>) -> Mode {
        match (self.mode, byte) {
            (Mode::Ground | Mode::Csi, ESC) => Mode::Escape,
            (Mode::Ground, BEL) => {
                signals.push(Signal::Bell);
                Mode::Ground
            }
            (Mode::Ground, _) => Mode::Ground,
            (Mode::Csi, CANCEL | SUBSTITUTE) => Mode::Ground,
            (Mode::Csi, 0x40..=0x7e) => {
                self.finish_csi(byte, signals);
                Mode::Ground
            }
            (Mode::Csi, _) => {
                if self.csi.len() < CSI_LIMIT {
                    self.csi.push(byte);
                }
                Mode::Csi
            }
            (Mode::CommandEscape, b'\\') => {
                self.finish(Terminator::StringTerminator, signals);
                Mode::Ground
            }
            (Mode::Escape | Mode::CommandEscape | Mode::OverflowEscape, b']') => {
                self.command.clear();
                Mode::Command
            }
            (Mode::Escape | Mode::CommandEscape | Mode::OverflowEscape, b'[') => {
                self.csi.clear();
                Mode::Csi
            }
            (Mode::Escape | Mode::CommandEscape | Mode::OverflowEscape, ESC) => Mode::Escape,
            (Mode::Escape | Mode::CommandEscape | Mode::OverflowEscape, _) => Mode::Ground,
            (Mode::Command, BEL) => {
                self.finish(Terminator::Bell, signals);
                Mode::Ground
            }
            (Mode::Command, ESC) => Mode::CommandEscape,
            (Mode::Command | Mode::Overflow, CANCEL | SUBSTITUTE) => Mode::Ground,
            (Mode::Command, _) if self.command.len() >= self.limit() => Mode::Overflow,
            (Mode::Command, _) => {
                self.command.push(byte);
                Mode::Command
            }
            (Mode::Overflow, BEL) => Mode::Ground,
            (Mode::Overflow, ESC) => Mode::OverflowEscape,
            (Mode::Overflow, _) => Mode::Overflow,
        }
    }

    /// A keyboard request, or a switch of screens; the screen takes every CSI as well.
    fn finish_csi(&mut self, final_byte: u8, signals: &mut Vec<Signal>) {
        let parameters = String::from_utf8_lossy(&self.csi);
        match final_byte {
            KEYBOARD_FINAL => signals.extend(kitty_keyboard::parse_request(&parameters).map(Signal::Keyboard)),
            MODE_SET | MODE_RESET if swaps_screen(&parameters) => {
                signals.push(Signal::AlternateScreen(final_byte == MODE_SET));
            }
            _ => {}
        }
    }

    /// A copy is longer than any other command.
    fn limit(&self) -> usize {
        if self.command.starts_with(CLIPBOARD_PREFIX) {
            CLIPBOARD_LIMIT
        } else {
            OSC_LIMIT
        }
    }

    fn finish(&mut self, terminator: Terminator, signals: &mut Vec<Signal>) {
        let command = String::from_utf8_lossy(&self.command);
        signals.extend(parse_command(&command, terminator));
        self.command.clear();
    }
}

/// Whether private modes `?…` name one that swaps in the alternate screen.
fn swaps_screen(parameters: &str) -> bool {
    parameters
        .strip_prefix('?')
        .is_some_and(|modes| modes.split(';').any(|mode| ALTERNATE_SCREEN_MODES.contains(&mode)))
}

/// One OSC command, its code and what follows it.
fn parse_command(command: &str, terminator: Terminator) -> Option<Signal> {
    let (code, rest) = command.split_once(';').unwrap_or((command, ""));
    match code {
        AGENT_CODE if rest == QUERY => Some(Signal::AgentQuery(terminator)),
        AGENT_CODE => Report::parse(rest).map(Signal::AgentReport),
        MARK_CODE => parse_mark(rest),
        NOTIFY_CODE => parse_notify(rest),
        NOTIFY_TITLED_CODE => parse_titled_notify(rest),
        CLIPBOARD_CODE => parse_clipboard(rest, terminator),
        _ => None,
    }
}

/// `52;selection;data`: the data in base64, or `?` to ask what the clipboard holds. Every selection is the clipboard.
fn parse_clipboard(rest: &str, terminator: Terminator) -> Option<Signal> {
    let (_, data) = rest.split_once(';')?;
    if data == QUERY {
        return Some(Signal::ClipboardRead(terminator));
    }
    let text = String::from_utf8(STANDARD.decode(data).ok()?).ok()?;
    Some(Signal::ClipboardWrite(text))
}

/// The answer to a question what the clipboard holds.
pub fn clipboard_answer(text: &str, terminator: Terminator) -> Vec<u8> {
    let head = format!("\x1b]{CLIPBOARD_CODE};c;{}", STANDARD.encode(text));
    [head.as_bytes(), terminator.bytes()].concat()
}

/// What the last prompt in `bytes` shows, from its OSC 133 `A` mark to its `B`, its escape sequences left out: its last
/// line, the one the command is typed on. None when `bytes` start no prompt, or it shows nothing; a prompt split across
/// two reads is cut where the read ends.
pub fn prompt_text(bytes: &[u8]) -> Option<String> {
    let start = bytes.windows(PROMPT_MARK.len()).rposition(|window| window == PROMPT_MARK)? + PROMPT_MARK.len();
    let rest = &bytes[start..];
    let end = rest.windows(PROMPT_END_MARK.len()).position(|window| window == PROMPT_END_MARK).unwrap_or(rest.len());
    let shown = without_sequences(&String::from_utf8_lossy(&rest[..end]));
    let line = shown.rsplit(['\n', '\r']).next()?.trim_end();
    (!line.is_empty()).then(|| line.to_owned())
}

/// `text` without its escape sequences and control characters but line ends: what of it is drawn.
fn without_sequences(text: &str) -> String {
    let mut shown = String::new();
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        match char {
            '\u{1b}' => skip_sequence(&mut chars),
            '\n' | '\r' => shown.push(char),
            _ if char.is_control() => {}
            _ => shown.push(char),
        }
    }
    shown
}

/// Skips what follows an ESC: a CSI to its final byte, an OSC to its terminator, anything else one character.
fn skip_sequence(chars: &mut std::iter::Peekable<std::str::Chars>) {
    match chars.next() {
        Some('[') => {
            for char in chars.by_ref() {
                if ('@'..='~').contains(&char) {
                    return;
                }
            }
        }
        Some(']') => {
            while let Some(char) = chars.next() {
                if char == '\u{7}' || (char == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                    return;
                }
            }
        }
        _ => {}
    }
}

/// `A` where the prompt starts, `C` where a command's output does, `D;status` where it
/// ended; `B`, where what is typed starts, tells nothing more.
fn parse_mark(mark: &str) -> Option<Signal> {
    let mut fields = mark.split(';');
    match fields.next()? {
        "A" => Some(Signal::PromptStart),
        "C" => Some(Signal::CommandStart),
        "D" => Some(Signal::CommandEnd(
            fields.next().and_then(|status| status.parse().ok()),
        )),
        _ => None,
    }
}

/// `9;text` is a notification; `9;4;state;percent` is progress. ConEmu's other numbered
/// commands under 9, such as `9;9;path`, are not notifications.
fn parse_notify(rest: &str) -> Option<Signal> {
    if let Some(progress) = rest.strip_prefix(PROGRESS_PREFIX) {
        return Some(parse_progress(progress));
    }
    let first = rest.split(';').next().unwrap_or_default();
    let numbered = !first.is_empty() && first.bytes().all(|byte| byte.is_ascii_digit());
    (!numbered && !rest.is_empty()).then(|| Signal::Notification {
        title: None,
        body: rest.to_string(),
    })
}

fn parse_progress(progress: &str) -> Signal {
    let mut fields = progress.split(';');
    let state = fields.next().unwrap_or(PROGRESS_CLEAR);
    if state == PROGRESS_CLEAR {
        return Signal::Progress(None);
    }
    let percent = fields
        .next()
        .and_then(|percent| percent.parse::<u8>().ok())
        .unwrap_or(0)
        .min(PROGRESS_MAX);
    Signal::Progress(Some(percent))
}

/// `777;notify;title;body`.
fn parse_titled_notify(rest: &str) -> Option<Signal> {
    let mut fields = rest.splitn(3, ';');
    if fields.next()? != "notify" {
        return None;
    }
    let title = fields.next().unwrap_or_default();
    Some(Signal::Notification {
        title: (!title.is_empty()).then(|| title.to_string()),
        body: fields.next().unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::agent::AgentState;
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Signal> {
        let mut scanner = Scanner::default();
        let mut signals = Vec::new();
        for chunk in chunks {
            scanner.scan(chunk, &mut signals);
        }
        signals
    }

    #[test]
    fn keyboard_requests_are_read_and_other_csis_are_not() {
        use kitty_keyboard::Request;
        assert_eq!(scan(&[b"\x1b[>1u\x1b[?u"]), [Signal::Keyboard(Request::Push(1)), Signal::Keyboard(Request::Query)]);
        assert_eq!(scan(&[b"\x1b[<", b"u"]), [Signal::Keyboard(Request::Pop(1))]);
        assert_eq!(scan(&[b"\x1b[u\x1b[2J\x1b[31m"]), []);
        assert_eq!(scan(&[b"\x1b[1m\x1b]133;A\x07"]), [Signal::PromptStart]);
        assert_eq!(scan(&[b"\x1b[?1049h\x1b[?25l\x1b[?1049l"]), [Signal::AlternateScreen(true), Signal::AlternateScreen(false)]);
    }

    #[test]
    fn a_bell_alone_rings_but_one_that_ends_a_command_does_not() {
        assert_eq!(scan(&[b"done\x07"]), [Signal::Bell]);
        assert_eq!(scan(&[b"\x1b]0;title\x07"]), []);
    }

    #[test]
    fn the_agent_query_is_answered_with_its_own_terminator() {
        assert_eq!(
            scan(&[b"\x1b]7501;?\x1b\\"]),
            [Signal::AgentQuery(Terminator::StringTerminator)]
        );
        assert_eq!(
            scan(&[b"\x1b]7501;?\x07"]),
            [Signal::AgentQuery(Terminator::Bell)]
        );
    }

    #[test]
    fn a_report_split_across_reads_is_read_whole() {
        let signals = scan(&[b"\x1b]7501;state=bl", b"ocked:id=a:kind=permission\x07"]);
        let [Signal::AgentReport(report)] = signals.as_slice() else {
            panic!("expected one report, got {signals:?}");
        };
        assert_eq!(report.state, Some(AgentState::Blocked));
        assert_eq!(report.id, "a");
    }

    #[test]
    fn a_prompt_is_read_without_its_colours() {
        let bytes = b"out\x1b]133;D;0\x07\x1b]133;A\x07\x1b[32mme@host\x1b[0m:\x1b]0;title\x07~$ \x1b]133;B\x07ls";
        assert_eq!(prompt_text(bytes).as_deref(), Some("me@host:~$"));
    }

    #[test]
    fn a_two_line_prompt_gives_its_last_line() {
        assert_eq!(prompt_text(b"\x1b]133;A\x1b\\(main) ~/src\r\n> \x1b]133;B\x1b\\").as_deref(), Some(">"));
        assert_eq!(prompt_text(b"no mark here"), None);
    }

    #[test]
    fn command_marks_carry_the_exit_status() {
        assert_eq!(
            scan(&[b"\x1b]133;A\x07\x1b]133;C\x07\x1b]133;D;2\x07"]),
            [
                Signal::PromptStart,
                Signal::CommandStart,
                Signal::CommandEnd(Some(2))
            ]
        );
    }

    #[test]
    fn notifications_and_progress_are_told_apart() {
        assert_eq!(
            scan(&[b"\x1b]9;Build finished\x07"]),
            [Signal::Notification {
                title: None,
                body: "Build finished".into()
            }]
        );
        assert_eq!(scan(&[b"\x1b]9;4;1;42\x07"]), [Signal::Progress(Some(42))]);
        assert_eq!(scan(&[b"\x1b]9;4;0\x07"]), [Signal::Progress(None)]);
        assert_eq!(scan(&[b"\x1b]9;9;/home\x07"]), []);
        assert_eq!(
            scan(&[b"\x1b]777;notify;cargo;done\x07"]),
            [Signal::Notification {
                title: Some("cargo".into()),
                body: "done".into()
            }]
        );
    }

    #[test]
    fn a_command_past_the_limit_is_dropped_and_what_follows_is_read() {
        let mut long = b"\x1b]9;".to_vec();
        long.extend(std::iter::repeat_n(b'x', OSC_LIMIT + 10));
        long.extend(b"\x07\x07");
        assert_eq!(scan(&[&long]), [Signal::Bell]);
    }

    #[test]
    fn a_copy_is_read_long_and_a_question_answered_in_kind() {
        let mut copy = b"\x1b]52;c;".to_vec();
        copy.extend(STANDARD.encode("x".repeat(OSC_LIMIT * 2)).bytes());
        copy.push(BEL);
        assert_eq!(scan(&[&copy]), [Signal::ClipboardWrite("x".repeat(OSC_LIMIT * 2))]);
        assert_eq!(scan(&[b"\x1b]52;c;?\x1b\\"]), [Signal::ClipboardRead(Terminator::StringTerminator)]);
        assert_eq!(scan(&[b"\x1b]52;c;not base64!\x07"]), []);
        assert_eq!(clipboard_answer("hi", Terminator::Bell), b"\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn cancel_ends_a_command_without_a_signal() {
        assert_eq!(scan(&[b"\x1b]9;hi\x18\x07"]), [Signal::Bell]);
    }
}
