//! The triggers, as Nekominal's: each line a pane writes is matched against the rules of the settings, past the escape
//! sequences in it. A highlight is drawn by giving the matched text a background on its way to the screen; the other
//! actions are handed back for the pane to carry out.

use regex::{Captures, Regex};

use super::config::{Trigger, TriggerAction, DEFAULT_HIGHLIGHT, HIGHLIGHTS};

const ESCAPE: u8 = 0x1b;
const BELL: u8 = 0x07;
/// The longest line kept for matching; a longer one is matched by its start.
const LINE_LIMIT: usize = 4096;

/// What a matching line asks of the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fired {
    Notify(String),
    Badge,
    Send(String),
    Bell,
}

struct Rule {
    trigger: Trigger,
    regex: Regex,
    /// The background of a highlight, as an SGR sequence, and its end.
    highlight: Option<(Vec<u8>, &'static [u8])>,
}

/// Where the reading of the output is, between two chunks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Text,
    Escape,
    Csi,
    /// An OSC, DCS or other string, until BEL or ST.
    String,
    StringEscape,
}

#[derive(Default)]
pub struct Matcher {
    rules: Vec<Rule>,
    mode: Mode,
    line: String,
    /// The rules that matched the line being written, so an instant one fires once a line.
    fired_on_line: Vec<usize>,
}

/// Whether `pattern` is a regular expression the triggers can use.
pub fn is_valid(pattern: &str) -> bool {
    !pattern.is_empty() && Regex::new(pattern).is_ok()
}

fn sgr_background(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    let (red, green, blue) = (channel(0)?, channel(2)?, channel(4)?);
    Some(format!("\x1b[48;2;{red};{green};{blue}m").into_bytes())
}

/// `$0` to `$9` in `text` replaced by the match and its groups.
fn expand(text: &str, captures: &Captures) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match (ch, chars.peek().and_then(|next| next.to_digit(10))) {
            ('$', Some(group)) => {
                chars.next();
                out.push_str(captures.get(group as usize).map_or("", |found| found.as_str()));
            }
            _ => out.push(ch),
        }
    }
    out
}

/// A program's name as a trigger names it: without its folder, and the same in any case.
fn same_program(running: Option<&str>, wanted: &str) -> bool {
    if wanted.trim().is_empty() {
        return true;
    }
    let name = |program: &str| {
        let base = program.rsplit('/').next().unwrap_or(program).to_lowercase();
        base.strip_suffix(".exe").map(str::to_string).unwrap_or(base)
    };
    running.is_some_and(|running| name(running) == name(wanted.trim()))
}

impl Matcher {
    pub fn new(triggers: &[Trigger]) -> Self {
        let rules = triggers
            .iter()
            .filter(|trigger| trigger.enabled && !trigger.pattern.is_empty())
            .filter_map(|trigger| {
                let regex = Regex::new(&trigger.pattern).ok()?;
                let highlight = (trigger.action == TriggerAction::Highlight)
                    .then(|| sgr_background(&trigger.color))
                    .flatten()
                    .map(|start| (start, b"\x1b[49m".as_slice()));
                Some(Rule { trigger: trigger.clone(), regex, highlight })
            })
            .collect();
        Self { rules, ..Self::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Reads a chunk of output: what goes to the screen (with the highlights in it), and what the lines that ended in
    /// it, or the line being written for an instant rule, ask for.
    pub fn process(&mut self, bytes: &[u8], program: Option<&str>) -> (Vec<u8>, Vec<Fired>) {
        let mut screen = Vec::with_capacity(bytes.len());
        let mut fired = Vec::new();
        let mut run_start = None;
        for (at, &byte) in bytes.iter().enumerate() {
            let in_text = self.mode == Mode::Text && byte != ESCAPE && byte >= 0x20 || self.mode == Mode::Text && byte == b'\t';
            if in_text {
                run_start.get_or_insert(at);
                continue;
            }
            if let Some(start) = run_start.take() {
                self.take_text(&bytes[start..at], &mut screen, program);
            }
            screen.push(byte);
            self.step(byte, program, &mut fired);
        }
        if let Some(start) = run_start {
            self.take_text(&bytes[start..], &mut screen, program);
        }
        self.match_instant(program, &mut fired);
        (screen, fired)
    }

    /// Plain text: kept for the line, and written to the screen with its highlights.
    fn take_text(&mut self, text: &[u8], screen: &mut Vec<u8>, program: Option<&str>) {
        let decoded = String::from_utf8_lossy(text);
        if self.line.len() < LINE_LIMIT {
            self.line.push_str(&decoded);
        }
        let mut marks: Vec<(usize, usize, &[u8], &[u8])> = Vec::new();
        // Highlights only where the text is whole: a sequence cut in two by the read is left as it is.
        if !matches!(decoded, std::borrow::Cow::Owned(_)) {
            for rule in &self.rules {
                let Some((start, end)) = &rule.highlight else { continue };
                if !same_program(program, &rule.trigger.program) {
                    continue;
                }
                for found in rule.regex.find_iter(&decoded) {
                    let overlaps = marks.iter().any(|(from, to, _, _)| found.start() < *to && *from < found.end());
                    if !found.is_empty() && !overlaps {
                        marks.push((found.start(), found.end(), start, end));
                    }
                }
            }
        }
        marks.sort_by_key(|(from, _, _, _)| *from);
        let mut at = 0;
        for (from, to, start, end) in marks {
            screen.extend_from_slice(&text[at..from]);
            screen.extend_from_slice(start);
            screen.extend_from_slice(&text[from..to]);
            screen.extend_from_slice(end);
            at = to;
        }
        screen.extend_from_slice(&text[at..]);
    }

    /// A byte that is not plain text: an escape sequence goes by, a line break ends the line.
    fn step(&mut self, byte: u8, program: Option<&str>, fired: &mut Vec<Fired>) {
        self.mode = match (self.mode, byte) {
            (Mode::Text, ESCAPE) => Mode::Escape,
            (Mode::Text, b'\n') => {
                self.end_line(program, fired);
                Mode::Text
            }
            (Mode::Text, _) => Mode::Text,
            (Mode::Escape, b'[') => Mode::Csi,
            (Mode::Escape, b']' | b'P' | b'_' | b'^' | b'X') => Mode::String,
            (Mode::Escape, _) => Mode::Text,
            (Mode::Csi, 0x40..=0x7e) => Mode::Text,
            (Mode::Csi, _) => Mode::Csi,
            (Mode::String, BELL) => Mode::Text,
            (Mode::String, ESCAPE) => Mode::StringEscape,
            (Mode::String, _) => Mode::String,
            (Mode::StringEscape, b'\\') => Mode::Text,
            (Mode::StringEscape, _) => Mode::String,
        };
    }

    fn end_line(&mut self, program: Option<&str>, fired: &mut Vec<Fired>) {
        let line = std::mem::take(&mut self.line);
        let line = line.trim_end_matches('\r');
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.trigger.instant && self.fired_on_line.contains(&index) {
                continue;
            }
            fire(rule, line, program, fired);
        }
        self.fired_on_line.clear();
    }

    /// The instant rules on the line not yet ended: a prompt waits without a line break.
    fn match_instant(&mut self, program: Option<&str>, fired: &mut Vec<Fired>) {
        for (index, rule) in self.rules.iter().enumerate() {
            if !rule.trigger.instant || self.fired_on_line.contains(&index) {
                continue;
            }
            let before = fired.len();
            fire(rule, &self.line, program, fired);
            if fired.len() > before {
                self.fired_on_line.push(index);
            }
        }
    }
}

/// What `rule` asks for when it matches `line`; a highlight was drawn already.
fn fire(rule: &Rule, line: &str, program: Option<&str>, fired: &mut Vec<Fired>) {
    if !same_program(program, &rule.trigger.program) {
        return;
    }
    let Some(captures) = rule.regex.captures(line) else {
        return;
    };
    match rule.trigger.action {
        TriggerAction::Highlight => {}
        TriggerAction::Notify => fired.push(Fired::Notify(expand(&rule.trigger.text, &captures))),
        TriggerAction::Badge => fired.push(Fired::Badge),
        TriggerAction::Send if rule.trigger.allow_send => {
            fired.push(Fired::Send(expand(&rule.trigger.text, &captures)));
        }
        TriggerAction::Send => {}
        TriggerAction::Bell => fired.push(Fired::Bell),
    }
}

/// Nekominal's ready-made rules: their names, and the rules.
pub fn presets() -> [Trigger; 4] {
    let rule = |name: &str, pattern: &str, action: TriggerAction, instant: bool| Trigger {
        name: name.to_string(),
        pattern: pattern.to_string(),
        action,
        color: if action == TriggerAction::Highlight { HIGHLIGHTS[1].to_string() } else { DEFAULT_HIGHLIGHT.to_string() },
        instant,
        ..Trigger::default()
    };
    [
        rule("錯誤標紅", r"error(\[E\d+\])?:", TriggerAction::Highlight, false),
        rule("測試失敗加徽章", "test result: FAILED", TriggerAction::Badge, false),
        rule("部署完成通知", r"(?i)deploy(ment)? (complete|finished|succeeded)", TriggerAction::Notify, false),
        rule("密碼提示時響鈴", r"(?i)password( for [^:]+)?:\s*$", TriggerAction::Bell, true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(pattern: &str, action: TriggerAction) -> Trigger {
        Trigger { pattern: pattern.to_string(), action, ..Trigger::default() }
    }

    #[test]
    fn a_highlight_wraps_the_match_in_a_background() {
        let mut matcher = Matcher::new(&[trigger("error:", TriggerAction::Highlight)]);
        let (screen, fired) = matcher.process(b"x error: y\n", None);
        assert_eq!(screen, b"x \x1b[48;2;122;92;0merror:\x1b[49m y\n");
        assert!(fired.is_empty());
    }

    #[test]
    fn a_line_is_matched_past_the_colours_in_it() {
        let mut matcher = Matcher::new(&[Trigger { text: "found $1".to_string(), ..trigger("deploy (done)", TriggerAction::Notify) }]);
        let (screen, fired) = matcher.process(b"\x1b[32mdeploy \x1b[0mdo", None);
        assert_eq!(screen, b"\x1b[32mdeploy \x1b[0mdo");
        assert!(fired.is_empty());
        let (_, fired) = matcher.process(b"ne\r\n", None);
        assert_eq!(fired, vec![Fired::Notify("found done".to_string())]);
    }

    #[test]
    fn an_instant_rule_fires_once_before_the_line_ends() {
        let mut matcher = Matcher::new(&[Trigger { instant: true, ..trigger("password:", TriggerAction::Bell) }]);
        assert_eq!(matcher.process(b"password:", None).1, vec![Fired::Bell]);
        assert!(matcher.process(b" ", None).1.is_empty());
        assert!(matcher.process(b"\n", None).1.is_empty());
    }

    #[test]
    fn text_is_sent_only_when_allowed_and_rules_keep_to_their_program() {
        let send = Trigger { text: "y\r".to_string(), program: "claude".to_string(), ..trigger("continue\\?", TriggerAction::Send) };
        let mut matcher = Matcher::new(std::slice::from_ref(&send));
        assert!(matcher.process(b"continue?\n", Some("claude")).1.is_empty());
        let mut matcher = Matcher::new(&[Trigger { allow_send: true, ..send }]);
        assert!(matcher.process(b"continue?\n", Some("bash")).1.is_empty());
        assert_eq!(matcher.process(b"continue?\n", Some("/usr/bin/claude")).1, vec![Fired::Send("y\r".to_string())]);
    }

    #[test]
    fn every_preset_is_a_valid_rule() {
        for preset in presets() {
            assert!(is_valid(&preset.pattern), "{}", preset.name);
        }
    }
}
