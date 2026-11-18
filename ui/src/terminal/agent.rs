//! What a pane is doing, from what its agent reported and what its shell marked: the state
//! the sidebar groups it under, the tab shows, and a notification tells.


use super::lang::{tr, trf};
use base64::{engine::general_purpose::STANDARD, Engine};

/// No pane keeps more records than this; an agent that sends more loses its oldest.
const RECORD_LIMIT: usize = 256;
const CLEAR: &str = "clear";

/// The coding agents told by their program's name, and what they are called.
const KNOWN_AGENTS: [(&str, &str); 5] = [
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("aider", "Aider"),
    ("opencode", "OpenCode"),
];

/// What an agent says it is doing, least urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentState {
    Idle,
    Done,
    Working,
    Error,
    Blocked,
}

impl AgentState {
    fn parse(state: &str) -> Option<Self> {
        match state {
            "idle" => Some(Self::Idle),
            "done" => Some(Self::Done),
            "working" => Some(Self::Working),
            "error" => Some(Self::Error),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }

    /// Working and blocked last only as long as the program that said so.
    fn is_transient(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }
}

/// What a blocked agent waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Permission,
    Question,
    Auth,
}

impl Ask {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "permission" => Some(Self::Permission),
            "question" => Some(Self::Question),
            "auth" => Some(Self::Auth),
            _ => None,
        }
    }
}

/// One OSC 7501 report: `state=…:id=…:kind=…:progress=…:app=…:title=…:msg=…`, the title
/// and message in base64. A report replaces the whole record of its id.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub id: String,
    /// None clears the record.
    pub state: Option<AgentState>,
    pub ask: Option<Ask>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
}

impl Report {
    pub fn parse(fields: &str) -> Option<Self> {
        let mut report = Report {
            id: String::new(),
            state: None,
            ask: None,
            progress: None,
            app: None,
            title: None,
            message: None,
        };
        let mut has_state = false;
        for field in fields.split(':') {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "state" if value == CLEAR => has_state = true,
                "state" => {
                    report.state = Some(AgentState::parse(value)?);
                    has_state = true;
                }
                "id" => report.id = value.to_string(),
                "kind" => report.ask = Ask::parse(value),
                "progress" => report.progress = value.parse().ok(),
                "app" => report.app = Some(value.to_string()),
                "title" => report.title = decode_text(value),
                "msg" => report.message = decode_text(value),
                _ => {}
            }
        }
        has_state.then_some(report)
    }
}

fn decode_text(encoded: &str) -> Option<String> {
    let bytes = STANDARD.decode(encoded).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (!text.is_empty()).then_some(text)
}

/// The name an agent goes by, from its program's name.
pub fn agent_named(program: &str) -> Option<&'static str> {
    let program = program.to_lowercase();
    KNOWN_AGENTS
        .iter()
        .find(|(prefix, _)| program.starts_with(prefix))
        .map(|(_, name)| *name)
}

/// The state a pane is grouped under, in the order the sidebar lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Waiting,
    Working,
    Done,
    Failed,
    Idle,
}

pub const STATUS_ORDER: [Status; 5] = [
    Status::Waiting,
    Status::Working,
    Status::Done,
    Status::Failed,
    Status::Idle,
];

/// Nekominal's groups of the sidebar: what needs the user, what works, what finished, and the rest.
pub const GROUP_HEADINGS: [&str; 4] = ["等待回應", "執行中", "已完成", "其他"];

impl Status {
    /// Its group in the sidebar grouped by state, an index into [`GROUP_HEADINGS`].
    pub fn group(self) -> usize {
        match self {
            Self::Waiting | Self::Failed => 0,
            Self::Working => 1,
            Self::Done => 2,
            Self::Idle => 3,
        }
    }

    /// It needs the user: its name and its line take its colour.
    pub fn is_urgent(self) -> bool {
        matches!(self, Self::Waiting | Self::Failed)
    }

    /// The word beside a tab in its state; an idle shell has none.
    pub fn word(self) -> Option<&'static str> {
        match self {
            Self::Waiting => Some(tr("等待輸入")),
            Self::Working => Some(tr("執行中")),
            Self::Done => Some(tr("已完成")),
            Self::Failed => Some(tr("錯誤")),
            Self::Idle => None,
        }
    }

    /// libadwaita's style class for its colour, so that it follows the desktop's palette.
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Waiting => "warning",
            Self::Working => "accent",
            Self::Done => "success",
            Self::Failed => "error",
            Self::Idle => "dim-label",
        }
    }

    /// Which of a tab's panes speaks for it: the one that needs the user most.
    pub fn urgency(self) -> u8 {
        match self {
            Self::Waiting => 4,
            Self::Failed => 3,
            Self::Working => 2,
            Self::Done => 1,
            Self::Idle => 0,
        }
    }

    /// Its colour: the scheme's yellow, blue, green and red, as Nekominal takes them.
    pub fn colour(self) -> &'static str {
        let palette = super::theme::current_scheme().palette;
        match self {
            Self::Waiting => palette[3],
            Self::Working => palette[4],
            Self::Done => palette[2],
            Self::Failed => palette[1],
            Self::Idle => palette[8],
        }
    }

    /// Worth a notification when it comes while no one is looking.
    pub fn is_news(self) -> bool {
        matches!(self, Self::Waiting | Self::Done | Self::Failed)
    }
}

/// The program in the foreground, when it is not the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub program: String,
    pub agent: Option<&'static str>,
}

/// Everything a pane has been told.
#[derive(Debug, Default)]
pub struct PaneState {
    records: Vec<Report>,
    running_command: bool,
    /// Whether a command ran since the last exit status, so that a prompt redrawn on an
    /// empty line does not repeat the last failure.
    command_started: bool,
    failed: Option<i32>,
    /// The shell marked a command's end since its last prompt.
    command_ended: bool,
    progress: Option<u8>,
    /// A bell or notification no one has seen yet.
    attention: Option<String>,
    pub job: Option<Job>,
}

impl PaneState {
    pub fn apply(&mut self, signal: &super::osc::Signal) {
        use super::osc::Signal;
        match signal {
            Signal::Bell => {
                self.attention.get_or_insert_with(String::new);
            }
            Signal::Notification { title, body } => {
                self.attention = Some(title.clone().map_or(body.clone(), |title| {
                    format!("{title}：{body}")
                }));
            }
            Signal::AgentReport(report) => self.apply_report(report.clone()),
            // Only a prompt after a command ended means the agent is gone: readline draws
            // the prompt again when the screen is resized, as it is when the band opens.
            Signal::PromptStart => {
                if std::mem::take(&mut self.command_ended) {
                    self.drop_transient();
                }
                self.running_command = false;
            }
            Signal::CommandStart => {
                self.running_command = true;
                self.command_started = true;
                self.failed = None;
                self.progress = None;
            }
            Signal::CommandEnd(status) => {
                self.command_ended = true;
                if self.command_started {
                    self.failed = status.filter(|status| *status != 0);
                }
                self.command_started = false;
                self.running_command = false;
            }
            Signal::Progress(percent) => self.progress = *percent,
            Signal::AgentQuery(_) | Signal::ClipboardWrite(_) | Signal::ClipboardRead(_) | Signal::Keyboard(_) | Signal::AlternateScreen(_) => {}
        }
    }

    fn apply_report(&mut self, report: Report) {
        let existing = self.records.iter().position(|record| record.id == report.id);
        match (existing, report.state) {
            (Some(index), None) => {
                self.records.remove(index);
            }
            (Some(index), Some(_)) => self.records[index] = report,
            (None, None) => {}
            (None, Some(_)) => {
                if self.records.len() == RECORD_LIMIT {
                    self.records.remove(0);
                }
                self.records.push(report);
            }
        }
    }

    fn drop_transient(&mut self) {
        self.records
            .retain(|record| !record.state.is_some_and(AgentState::is_transient));
    }

    /// The shell exited: what it ran said nothing more.
    pub fn end(&mut self) {
        self.drop_transient();
        self.running_command = false;
    }

    /// Someone looked at the pane.
    pub fn acknowledge(&mut self) {
        self.attention = None;
    }

    fn most_urgent(&self) -> Option<&Report> {
        self.records
            .iter()
            .rev()
            .max_by_key(|record| record.state)
    }

    pub fn status(&self) -> Status {
        match self.most_urgent().and_then(|record| record.state) {
            Some(AgentState::Blocked) => return Status::Waiting,
            Some(AgentState::Error) => return Status::Failed,
            Some(AgentState::Working) => return Status::Working,
            Some(AgentState::Done) => return Status::Done,
            Some(AgentState::Idle) | None => {}
        }
        let running_program = self.job.as_ref().is_some_and(|job| job.agent.is_none());
        if self.attention.is_some() {
            Status::Waiting
        } else if self.failed.is_some() {
            Status::Failed
        } else if self.running_command || self.progress.is_some() || running_program {
            Status::Working
        } else {
            Status::Idle
        }
    }

    /// What the agent calls its session, which names the tab as Nekominal's does.
    pub fn reported_title(&self) -> Option<String> {
        self.most_urgent()
            .and_then(|record| record.title.clone())
            .filter(|title| !title.is_empty())
    }

    /// The agent's name: from its reports, else from its program.
    pub fn agent_name(&self) -> Option<String> {
        let reported = self
            .most_urgent()
            .and_then(|record| record.app.as_deref())
            .map(|app| agent_named(app).map_or_else(|| app.to_string(), str::to_string));
        reported.or_else(|| {
            self.job
                .as_ref()
                .and_then(|job| job.agent)
                .map(str::to_string)
        })
    }

    /// The line that says what is going on: the agent's message, the notification, or why
    /// the command failed.
    pub fn message(&self) -> Option<String> {
        let reported = self
            .most_urgent()
            .and_then(|record| record.message.clone().or_else(|| record.title.clone()));
        reported
            .or_else(|| self.attention.clone().filter(|text| !text.is_empty()))
            .or_else(|| self.failed.map(|status| trf("結束碼 {0}", &[&status])))
    }
}

#[cfg(test)]
mod tests {
    use super::super::osc::Signal;
    use super::*;

    fn report(id: &str, state: &str) -> Signal {
        Signal::AgentReport(Report::parse(&format!("state={state}:id={id}")).unwrap())
    }

    #[test]
    fn a_report_reads_its_fields_and_decodes_its_message() {
        let report = Report::parse("state=blocked:id=7:kind=permission:app=claude:msg=5YWB6Kix")
            .unwrap();
        assert_eq!(report.state, Some(AgentState::Blocked));
        assert_eq!(report.ask, Some(Ask::Permission));
        assert_eq!(report.app.as_deref(), Some("claude"));
        assert_eq!(report.message.as_deref(), Some("允許"));
    }

    #[test]
    fn a_report_needs_a_state_it_knows() {
        assert_eq!(Report::parse("id=1"), None);
        assert_eq!(Report::parse("state=sleeping"), None);
        assert_eq!(Report::parse("state=clear:id=1").unwrap().state, None);
    }

    #[test]
    fn the_most_urgent_record_decides() {
        let mut state = PaneState::default();
        state.apply(&report("a", "working"));
        state.apply(&report("b", "blocked"));
        assert_eq!(state.status(), Status::Waiting);
        state.apply(&report("b", "clear"));
        assert_eq!(state.status(), Status::Working);
    }

    #[test]
    fn a_new_prompt_drops_working_and_blocked_but_keeps_done() {
        let mut state = PaneState::default();
        state.apply(&report("a", "blocked"));
        state.apply(&report("b", "done"));
        state.apply(&Signal::CommandEnd(Some(0)));
        state.apply(&Signal::PromptStart);
        assert_eq!(state.status(), Status::Done);
    }

    #[test]
    fn a_prompt_redrawn_on_resize_keeps_the_agent_waiting() {
        let mut state = PaneState::default();
        state.apply(&report("a", "blocked"));
        state.apply(&Signal::PromptStart);
        assert_eq!(state.status(), Status::Waiting);
    }

    #[test]
    fn a_failed_command_fails_until_the_next_one_starts() {
        let mut state = PaneState::default();
        state.apply(&Signal::CommandStart);
        assert_eq!(state.status(), Status::Working);
        state.apply(&Signal::CommandEnd(Some(1)));
        assert_eq!(state.status(), Status::Failed);
        state.apply(&Signal::CommandEnd(Some(0)));
        assert_eq!(state.status(), Status::Failed);
        state.apply(&Signal::CommandStart);
        state.apply(&Signal::CommandEnd(Some(0)));
        assert_eq!(state.status(), Status::Idle);
    }

    #[test]
    fn a_bell_waits_until_seen() {
        let mut state = PaneState::default();
        state.apply(&Signal::Bell);
        assert_eq!(state.status(), Status::Waiting);
        state.acknowledge();
        assert_eq!(state.status(), Status::Idle);
    }

    #[test]
    fn an_agent_sitting_at_its_prompt_is_not_working() {
        let mut state = PaneState {
            job: Some(Job {
                program: "claude".into(),
                agent: agent_named("claude"),
            }),
            ..PaneState::default()
        };
        assert_eq!(state.status(), Status::Idle);
        assert_eq!(state.agent_name().as_deref(), Some("Claude Code"));
        state.job = Some(Job {
            program: "cargo".into(),
            agent: None,
        });
        assert_eq!(state.status(), Status::Working);
    }

    #[test]
    fn agents_are_known_by_the_start_of_their_name() {
        assert_eq!(agent_named("Codex-cli"), Some("Codex"));
        assert_eq!(agent_named("bash"), None);
    }
}
