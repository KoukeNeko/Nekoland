//! The subagents of a Claude Code session, as Nekominal reads them from what Claude Code itself keeps beside the
//! session's transcript (`<session>.jsonl` and the folder `<session>/`). No hook is needed: every subagent gets
//! `subagents/agent-<id>.meta.json` when it starts (its type and title, and `"finished": true` once it ends), and every
//! workflow appends to `subagents/workflows/<run>/journal.jsonl` as its agents start and end, writes its script to
//! `workflows/scripts/` when it is launched and its outcome to `workflows/<run>.json` when it ends.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::Value;

use super::usage;

const SUBAGENTS: &str = "subagents";
const WORKFLOWS: &str = "workflows";
const SCRIPTS: &str = "scripts";
const JOURNAL: &str = "journal.jsonl";
const AGENT_PREFIX: &str = "agent-";
const META_SUFFIX: &str = ".meta.json";
const WORKFLOW_AGENT: &str = "workflow-subagent";
/// What a workflow puts before the task it hands an agent.
const TASK_FRAME: &str = "The computed task text follows:";
/// The fields of a tool's input that say what it was called on, the likeliest first.
const MAIN_ARGS: [&str; 8] = ["file_path", "path", "pattern", "command", "url", "query", "description", "prompt"];
/// How many of the last tool calls the drawer lists.
const LAST_CALLS: usize = 5;

/// How a subagent, or an agent of a workflow, stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
}

impl State {
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// A subagent the session started with the Agent tool.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub id: String,
    /// Its kind (`Explore`, `general-purpose`, a plugin's `name:agent`).
    pub kind: String,
    /// The title the session gave it (the Agent tool's `description`).
    pub title: String,
    pub started: SystemTime,
    /// When it ended, as far as the files tell: when its meta file was written for the last time.
    pub finished: Option<SystemTime>,
    /// The last thing it said, once it ended.
    pub reply: Option<String>,
}

/// An agent of a workflow, as its journal tells it.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub id: String,
    pub label: String,
    pub state: State,
    /// The first line of what it handed back.
    pub result: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phase {
    pub title: String,
    /// In the order they started.
    pub steps: Vec<Step>,
}

impl Phase {
    pub fn count(&self, state: State) -> usize {
        self.steps.iter().filter(|step| step.state == state).count()
    }

    pub fn finished(&self) -> usize {
        self.steps.len() - self.count(State::Running)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workflow {
    pub run: String,
    pub name: String,
    pub summary: Option<String>,
    pub started: SystemTime,
    /// When it ended: its outcome was written.
    pub finished: Option<SystemTime>,
    pub phases: Vec<Phase>,
}

impl Workflow {
    fn count(&self, state: State) -> usize {
        self.phases.iter().map(|phase| phase.count(state)).sum()
    }

    pub fn is_running(&self) -> bool {
        self.count(State::Running) > 0
    }
}

/// The subagents of the session that are running, with those that ran alongside them: what the bar and its card show.
/// Empty when nothing runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subagents {
    /// Running first, then by when they started.
    pub agents: Vec<Agent>,
    pub workflows: Vec<Workflow>,
    /// Every workflow of the session as last read, by its run, with when its journal and its outcome were last written:
    /// one whose files did not change is not read again.
    seen: HashMap<String, (Stamps, Workflow)>,
}

/// When the journal and the outcome of a workflow were last written.
type Stamps = (Option<SystemTime>, Option<SystemTime>);

impl Subagents {
    pub fn running(&self) -> usize {
        self.agents.iter().filter(|agent| agent.finished.is_none()).count()
            + self.workflows.iter().map(|workflow| workflow.count(State::Running)).sum::<usize>()
    }

    pub fn done(&self) -> usize {
        self.agents.iter().filter(|agent| agent.finished.is_some()).count()
            + self
                .workflows
                .iter()
                .map(|workflow| workflow.count(State::Done) + workflow.count(State::Failed))
                .sum::<usize>()
    }
}

/// How long something ran, or has run so far, as the bar writes times.
pub fn duration_text(started: SystemTime, finished: Option<SystemTime>) -> String {
    let end = finished.unwrap_or_else(SystemTime::now);
    let elapsed = end.duration_since(started).unwrap_or_default();
    usage::clock(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

/// The folder Claude Code keeps a session's subagents in, beside its transcript.
pub fn folder(transcript: &Path) -> PathBuf {
    transcript.with_extension("")
}

fn modified(file: &Path) -> Option<SystemTime> {
    std::fs::metadata(file).ok()?.modified().ok()
}

fn read_json(file: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// When the folder of the subagents was last changed: a new subagent or workflow adds files to it.
pub fn stamp(folder: &Path) -> Option<SystemTime> {
    modified(&folder.join(SUBAGENTS))
}

/// Reads the subagents of the session whose folder is `folder`. `old` is the last read, whose replies are kept instead of
/// read again from the transcripts.
pub fn read(folder: &Path, old: &Subagents) -> Subagents {
    let directory = folder.join(SUBAGENTS);
    let agents = read_agents(&directory, old);
    let mut seen = HashMap::new();
    for entry in std::fs::read_dir(directory.join(WORKFLOWS)).into_iter().flatten().flatten() {
        let run = entry.file_name().to_string_lossy().into_owned();
        let journal = entry.path().join(JOURNAL);
        let outcome_file = folder.join(WORKFLOWS).join(format!("{run}.json"));
        let stamps = (modified(&journal), modified(&outcome_file));
        if stamps.0.is_none() {
            continue;
        }
        if let Some((before, workflow)) = old.seen.get(&run).filter(|(before, _)| *before == stamps) {
            seen.insert(run, (*before, workflow.clone()));
            continue;
        }
        if let Some(workflow) = read_workflow(folder, &run, &journal, &outcome_file) {
            seen.insert(run, (stamps, workflow));
        }
    }
    let workflows = seen.values().map(|(_, workflow)| workflow.clone()).collect();
    Subagents { seen, ..batch(agents, workflows) }
}

fn read_agents(directory: &Path, old: &Subagents) -> Vec<Agent> {
    let replies: HashMap<&str, &String> =
        old.agents.iter().filter_map(|agent| Some((agent.id.as_str(), agent.reply.as_ref()?))).collect();
    let mut agents = Vec::new();
    for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_prefix(AGENT_PREFIX).and_then(|rest| rest.strip_suffix(META_SUFFIX)) else {
            continue;
        };
        let (Ok(meta), Some(value)) = (entry.metadata(), read_json(&entry.path())) else {
            continue;
        };
        let created = meta.created().unwrap_or(SystemTime::UNIX_EPOCH);
        let Some(mut agent) = parse_meta(id, &value, created, meta.modified().ok()) else {
            continue;
        };
        if agent.finished.is_some() {
            agent.reply = replies
                .get(id)
                .map(|reply| (*reply).clone())
                .or_else(|| last_reply(&directory.join(format!("{AGENT_PREFIX}{id}.jsonl"))));
        }
        agents.push(agent);
    }
    agents
}

/// A workflow from its journal, its outcome when it ended and the script it was launched with.
fn read_workflow(folder: &Path, run: &str, journal: &Path, outcome_file: &Path) -> Option<Workflow> {
    let text = std::fs::read_to_string(journal).ok()?;
    let started = std::fs::metadata(journal).and_then(|meta| meta.created()).unwrap_or(SystemTime::UNIX_EPOCH);
    let outcome = read_json(outcome_file);
    let finished = outcome.as_ref().and_then(|_| modified(outcome_file));
    let script = script_of(&folder.join(WORKFLOWS).join(SCRIPTS), run);
    let name = outcome
        .as_ref()
        .and_then(|outcome| text_of(outcome, "workflowName"))
        .or_else(|| script.as_ref().map(|(name, _)| name.clone()))
        .unwrap_or_else(|| run.to_owned());
    let summary = outcome
        .as_ref()
        .and_then(|outcome| text_of(outcome, "summary"))
        .or_else(|| script.and_then(|(_, text)| script_description(&text)));
    Some(Workflow { phases: parse_journal(&text, outcome.as_ref()), run: run.to_owned(), name, summary, started, finished })
}

/// A subagent from its meta file; None for an agent of a workflow, which its journal tells.
fn parse_meta(id: &str, meta: &Value, created: SystemTime, modified: Option<SystemTime>) -> Option<Agent> {
    let kind = text_of(meta, "agentType").unwrap_or_default();
    if kind == WORKFLOW_AGENT || meta.get("workflowPhase").is_some() {
        return None;
    }
    // Ended: it finished, the user stopped it, or (an observer) it was stopped; a stopped agent never gets `finished`.
    let flag = |name: &str| meta.get(name).and_then(Value::as_bool) == Some(true);
    let finished = flag("finished") || flag("stoppedByUser") || flag("observerStopped");
    Some(Agent {
        id: id.to_owned(),
        kind,
        title: text_of(meta, "description").unwrap_or_default(),
        started: created,
        finished: finished.then(|| modified.unwrap_or(created)),
        reply: None,
    })
}

/// The phases of a workflow from its journal: an agent runs from `started` until its `result`. `outcome`, written when
/// the workflow ended, has the state of every agent, failures among them.
fn parse_journal(text: &str, outcome: Option<&Value>) -> Vec<Phase> {
    let mut phases: Vec<Phase> = Vec::new();
    let mut at: HashMap<String, (usize, usize)> = HashMap::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(kind), Some(id)) = (text_of(&event, "type"), text_of(&event, "agentId")) else {
            continue;
        };
        if kind == "started" {
            let title = text_of(&event, "phase").unwrap_or_default();
            let index = phases.iter().position(|phase| phase.title == title).unwrap_or_else(|| {
                phases.push(Phase { title, steps: Vec::new() });
                phases.len() - 1
            });
            at.insert(id.clone(), (index, phases[index].steps.len()));
            let label = text_of(&event, "label").unwrap_or_else(|| id.clone());
            phases[index].steps.push(Step { label, id, state: State::Running, result: None });
            continue;
        }
        let Some(&(phase, step)) = at.get(&id) else {
            continue;
        };
        let step = &mut phases[phase].steps[step];
        let result = event.get("result");
        let failed = kind != "result" || result.is_some_and(|result| result.get("error").is_some());
        step.state = if failed { State::Failed } else { State::Done };
        step.result = result
            .map(|result| result.as_str().map_or_else(|| result.to_string(), str::to_owned))
            .map(|result| first_line(&result));
    }
    let states = outcome
        .and_then(|outcome| outcome.get("workflowProgress")?.as_array())
        .into_iter()
        .flatten()
        .filter_map(|progress| Some((progress.get("agentId")?.as_str()?, progress.get("state")?.as_str()?)));
    for (id, state) in states {
        if let Some(&(phase, step)) = at.get(id) {
            phases[phase].steps[step].state = match state {
                "done" => State::Done,
                "running" | "queued" => State::Running,
                _ => State::Failed,
            };
        }
    }
    phases
}

/// The name of a workflow and its script, from the file `<name>-<run>.js` written when it was launched.
fn script_of(scripts: &Path, run: &str) -> Option<(String, String)> {
    let suffix = format!("-{run}.js");
    std::fs::read_dir(scripts).ok()?.flatten().find_map(|entry| {
        let file = entry.file_name().to_string_lossy().into_owned();
        let name = file.strip_suffix(&suffix)?.to_owned();
        Some((name, std::fs::read_to_string(entry.path()).unwrap_or_default()))
    })
}

/// The `description` of a workflow script's `meta`: the first string after `description:`.
fn script_description(script: &str) -> Option<String> {
    let rest = script.split_once("description:")?.1.trim_start();
    let quote = rest.chars().next().filter(|letter| matches!(letter, '\'' | '"' | '`'))?;
    let text = rest[1..].split(quote).next()?;
    (!text.is_empty()).then(|| text.to_owned())
}

/// The last text a subagent said, from the end of its transcript.
fn last_reply(transcript: &Path) -> Option<String> {
    let (tail, cut) = usage::read_tail(transcript)?;
    let whole = if cut { tail.split_once('\n').map_or("", |(_, rest)| rest) } else { &tail };
    whole.lines().rev().find_map(|line| {
        let entry: Value = serde_json::from_str(line).ok()?;
        if entry.get("type")?.as_str()? != "assistant" {
            return None;
        }
        let text = first_line(&texts(entry.pointer("/message/content")?));
        (!text.is_empty()).then_some(text)
    })
}

/// A tool a subagent called: its name and what it was called on, and whether its answer is still to come.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub tool: String,
    pub arg: String,
    pub open: bool,
}

/// What the drawer shows of one subagent, from its transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub prompt: Option<String>,
    /// The last few tool calls, oldest first, and how many it made in all.
    pub calls: Vec<Call>,
    pub total_calls: usize,
    pub reply: Option<String>,
}

/// The transcript of a subagent, or of an agent of workflow `run`.
pub fn transcript_of(folder: &Path, run: Option<&str>, id: &str) -> PathBuf {
    let directory = folder.join(SUBAGENTS);
    let file = format!("{AGENT_PREFIX}{id}.jsonl");
    match run {
        Some(run) => directory.join(WORKFLOWS).join(run).join(file),
        None => directory.join(file),
    }
}

/// The journal of workflow `run`.
pub fn journal_of(folder: &Path, run: &str) -> PathBuf {
    folder.join(SUBAGENTS).join(WORKFLOWS).join(run).join(JOURNAL)
}

/// Reads what a subagent was asked, the tools it called and what it said last, from its transcript.
pub fn detail(transcript: &Path) -> Detail {
    let Ok(text) = std::fs::read_to_string(transcript) else {
        return Detail::default();
    };
    let mut detail = Detail::default();
    let mut calls: Vec<(String, Call)> = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let content = entry.pointer("/message/content");
        match entry.get("type").and_then(Value::as_str) {
            Some("user") => {
                if detail.prompt.is_none() {
                    let asked = content.map(|content| content.as_str().map_or_else(|| texts(content), str::to_owned));
                    detail.prompt = asked.map(|asked| unframe(&asked)).filter(|asked| !asked.is_empty());
                }
                let answered = content
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("tool_use_id")?.as_str());
                for answered in answered {
                    if let Some((_, call)) = calls.iter_mut().rev().find(|(id, _)| id == answered) {
                        call.open = false;
                    }
                }
            }
            Some("assistant") => {
                for part in content.and_then(Value::as_array).into_iter().flatten() {
                    if part.get("type").and_then(Value::as_str) == Some("tool_use") {
                        let field = |key: &str| text_of(part, key).unwrap_or_default();
                        calls.push((field("id"), Call { tool: field("name"), arg: main_arg(part.get("input")), open: true }));
                    }
                }
                let said = content.map(|content| first_line(&texts(content))).filter(|said| !said.is_empty());
                detail.reply = said.or(detail.reply.take());
            }
            _ => {}
        }
    }
    detail.total_calls = calls.len();
    let skipped = calls.len().saturating_sub(LAST_CALLS);
    detail.calls = calls.into_iter().skip(skipped).map(|(_, call)| call).collect();
    detail
}

/// The text parts of a message's content.
fn texts(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text")?.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The task of a workflow agent without the frame the workflow puts around it (its lines are indented after a header).
fn unframe(asked: &str) -> String {
    match asked.split_once(TASK_FRAME) {
        Some((_, task)) => task.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n"),
        None => asked.trim().to_owned(),
    }
}

/// What a tool was called on: the path, pattern, command, address or question of its input.
fn main_arg(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    MAIN_ARGS.iter().find_map(|key| input.get(*key)?.as_str()).map(first_line).unwrap_or_default()
}

pub fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned()
}

/// What ran alongside what is running now: the running subagents and workflows, and every one that ended after the
/// earliest of them started, and so on back. Nothing when nothing runs.
fn batch(mut agents: Vec<Agent>, mut workflows: Vec<Workflow>) -> Subagents {
    let live_workflow = |workflow: &Workflow| workflow.finished.is_none() && (workflow.is_running() || workflow.phases.is_empty());
    let Some(mut since) = agents
        .iter()
        .filter(|agent| agent.finished.is_none())
        .map(|agent| agent.started)
        .chain(workflows.iter().filter(|workflow| live_workflow(workflow)).map(|workflow| workflow.started))
        .min()
    else {
        return Subagents::default();
    };
    // An interval that ended after `since` overlaps the batch and may start it earlier.
    loop {
        let earlier = agents
            .iter()
            .filter_map(|agent| Some((agent.started, agent.finished?)))
            .chain(workflows.iter().filter_map(|workflow| Some((workflow.started, workflow.finished?))))
            .filter(|&(start, end)| end >= since && start < since)
            .map(|(start, _)| start)
            .min();
        match earlier {
            Some(start) => since = start,
            None => break,
        }
    }
    agents.retain(|agent| agent.finished.is_none_or(|end| end >= since));
    workflows.retain(|workflow| workflow.finished.is_none_or(|end| end >= since));
    agents.sort_by_key(|agent| (agent.finished.is_some(), agent.started));
    workflows.sort_by_key(|workflow| workflow.started);
    Subagents { agents, workflows, ..Subagents::default() }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn agent(id: &str, start: u64, end: Option<u64>) -> Agent {
        Agent { id: id.into(), kind: "Explore".into(), title: id.into(), started: at(start), finished: end.map(at), reply: None }
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nekoland-subagents-{name}-{}", std::process::id()))
    }

    #[test]
    fn the_batch_is_what_runs_and_what_ran_alongside_it() {
        let ids = |subagents: &Subagents| subagents.agents.iter().map(|agent| agent.id.clone()).collect::<Vec<_>>();
        let got = batch(
            vec![agent("old", 0, Some(10)), agent("early", 20, Some(50)), agent("mid", 15, Some(25)), agent("run", 40, None)],
            vec![],
        );
        assert_eq!(ids(&got), ["run", "mid", "early"]);
        assert_eq!((got.running(), got.done()), (1, 2));
        assert_eq!(batch(vec![agent("a", 0, Some(10))], vec![]), Subagents::default());
    }

    #[test]
    fn a_meta_file_gives_a_subagent_and_leaves_workflow_agents_to_their_journal() {
        let plain = serde_json::json!({"agentType":"general-purpose","description":"say hi","finished":true});
        let got = parse_meta("a1", &plain, at(5), Some(at(9))).unwrap();
        assert_eq!((got.kind.as_str(), got.title.as_str(), got.finished), ("general-purpose", "say hi", Some(at(9))));
        let running = serde_json::json!({"agentType":"Explore","description":"look"});
        assert_eq!(parse_meta("a2", &running, at(5), Some(at(6))).unwrap().finished, None);
        let stopped = serde_json::json!({"agentType":"Explore","stoppedByUser":true});
        assert_eq!(parse_meta("a4", &stopped, at(5), Some(at(7))).unwrap().finished, Some(at(7)));
        let step = serde_json::json!({"agentType":"workflow-subagent","description":"beta","workflowPhase":"Beta"});
        assert_eq!(parse_meta("a3", &step, at(5), None), None);
    }

    #[test]
    fn the_journal_gives_the_phases_and_the_outcome_their_failures() {
        let journal = [
            r#"{"type":"launched"}"#,
            r#"{"type":"started","agentId":"a","label":"scope","phase":"Scope"}"#,
            r#"{"type":"result","agentId":"a","result":{"angles":[1]}}"#,
            r#"{"type":"started","agentId":"b","label":"x","phase":"Search"}"#,
            r#"{"type":"started","agentId":"c","label":"y","phase":"Search"}"#,
            r#"{"type":"result","agentId":"b","result":"ok\nmore"}"#,
        ]
        .join("\n");
        let phases = parse_journal(&journal, None);
        let sizes: Vec<(&str, usize)> = phases.iter().map(|phase| (phase.title.as_str(), phase.steps.len())).collect();
        assert_eq!(sizes, [("Scope", 1), ("Search", 2)]);
        assert_eq!(phases[1].steps.iter().map(|step| step.state).collect::<Vec<_>>(), [State::Done, State::Running]);
        assert_eq!(phases[1].steps[0].result.as_deref(), Some("ok"));
        let outcome = serde_json::json!({"workflowProgress":[{"agentId":"c","state":"failed"}]});
        assert_eq!(parse_journal(&journal, Some(&outcome))[1].steps[1].state, State::Failed);
    }

    #[test]
    fn a_transcript_gives_the_task_the_last_calls_and_the_last_words() {
        let file = scratch("transcript").with_extension("jsonl");
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"[Workflow harness] blah. The computed task text follows:\n  Verify the claim\n  about timers"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"x"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Grep","input":{"pattern":"SetTimer"}}]}}"#,
        ];
        std::fs::write(&file, lines.join("\n")).unwrap();
        let got = detail(&file);
        assert_eq!(got.prompt.as_deref(), Some("Verify the claim\nabout timers"));
        assert_eq!(got.total_calls, 2);
        assert_eq!(
            got.calls,
            [
                Call { tool: "Read".into(), arg: "/a.rs".into(), open: false },
                Call { tool: "Grep".into(), arg: "SetTimer".into(), open: true }
            ]
        );
        assert_eq!(got.reply.as_deref(), Some("Looking."));
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn a_workflow_script_gives_its_description() {
        let script = "export const meta = {\n  name: 'alpha-beta-ok',\n  description: 'Two phases, one agent each',\n}";
        assert_eq!(script_description(script).as_deref(), Some("Two phases, one agent each"));
        assert_eq!(script_description("export const meta = { name: 'x' }"), None);
    }

    #[test]
    fn the_files_claude_code_writes_are_read_into_a_batch() {
        let root = scratch("files");
        let directory = root.join(SUBAGENTS);
        let run = directory.join(WORKFLOWS).join("wf_1");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::create_dir_all(root.join(WORKFLOWS).join(SCRIPTS)).unwrap();
        std::fs::write(directory.join("agent-p1.meta.json"), r#"{"agentType":"Explore","description":"Trace the pipe"}"#).unwrap();
        std::fs::write(directory.join("agent-p2.meta.json"), r#"{"agentType":"Explore","description":"Read the test","finished":true}"#)
            .unwrap();
        std::fs::write(
            directory.join("agent-p2.jsonl"),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"It races on a pipe name.\nDetails"}]}}"#,
        )
        .unwrap();
        std::fs::write(run.join(JOURNAL), "{\"type\":\"started\",\"agentId\":\"w1\",\"label\":\"a\",\"phase\":\"Write\"}\n").unwrap();
        std::fs::write(root.join(WORKFLOWS).join(SCRIPTS).join("cats-wf_1.js"), "meta = { description: 'Three stories' }").unwrap();
        let got = read(&root, &Subagents::default());
        let titles: Vec<(&str, bool)> = got.agents.iter().map(|agent| (agent.title.as_str(), agent.finished.is_some())).collect();
        assert_eq!(titles, [("Trace the pipe", false), ("Read the test", true)]);
        assert_eq!(got.agents[1].reply.as_deref(), Some("It races on a pipe name."));
        assert_eq!((got.workflows[0].name.as_str(), got.workflows[0].summary.as_deref()), ("cats", Some("Three stories")));
        assert_eq!((got.running(), got.done()), (2, 1));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
