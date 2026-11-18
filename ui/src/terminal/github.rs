//! The pull request of the branch a tab is on, with its checks, as GitHub's `gh` tells it in the folder of the
//! repository. `gh` runs on a thread of its own: at once for a branch not seen before, then at most once a minute while
//! the pull request is open and every ten minutes for one merged or closed, or none; it is stopped after 30 seconds.
//! Without `gh`, signed out of it, outside GitHub or without a pull request, there is none.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{channel, Receiver, Sender},
    time::{Duration, Instant},
};

/// How long what gh said of an open pull request holds: its checks change.
const FRESH: Duration = Duration::from_secs(60);
/// How long an answer that rarely changes holds: merged, closed, or none. Each question is a call GitHub counts.
const SETTLED: Duration = Duration::from_secs(10 * 60);
/// A gh that hangs (the network, a credential helper waiting for nobody) is stopped, which answers none.
const GH_TIMEOUT: Duration = Duration::from_secs(30);
const GH_WAIT_STEP: Duration = Duration::from_millis(50);
const FIELDS: &str = "number,state,isDraft,url,statusCheckRollup";
const PASSING: [&str; 3] = ["SUCCESS", "NEUTRAL", "SKIPPED"];
const FAILING: [&str; 6] = ["FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

/// The checks of the head commit, counted by how they stand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    pub pending: u32,
    pub passing: u32,
    pub failing: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub state: State,
    pub checks: Checks,
    pub url: String,
}

impl PullRequest {
    /// A short line: `#12 open`.
    pub fn summary(&self) -> String {
        let state = match self.state {
            State::Open => "open",
            State::Draft => "draft",
            State::Merged => "merged",
            State::Closed => "closed",
        };
        format!("#{} {state}", self.number)
    }

    fn settled(&self) -> bool {
        matches!(self.state, State::Merged | State::Closed)
    }
}

/// Counts `statusCheckRollup` as `gh pr checks` does: check runs have a `status` and, once completed, a `conclusion`;
/// commit statuses have a `state`. What is neither a pass nor a failure is pending.
fn count_checks(rollup: &[serde_json::Value]) -> Checks {
    let mut checks = Checks::default();
    for item in rollup {
        let text = |key: &str| item.get(key).and_then(|value| value.as_str()).unwrap_or("");
        let verdict = match item.get("status") {
            Some(_) if text("status") != "COMPLETED" => "PENDING",
            Some(_) => text("conclusion"),
            None => text("state"),
        };
        if PASSING.contains(&verdict) {
            checks.passing += 1;
        } else if FAILING.contains(&verdict) {
            checks.failing += 1;
        } else {
            checks.pending += 1;
        }
    }
    checks
}

/// What `gh pr view --json` printed; none for anything else.
pub fn parse(json: &str) -> Option<PullRequest> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let text = |key: &str| value.get(key).and_then(|field| field.as_str()).filter(|text| !text.is_empty());
    let state = match text("state")? {
        "MERGED" => State::Merged,
        "CLOSED" => State::Closed,
        _ if value.get("isDraft").and_then(serde_json::Value::as_bool) == Some(true) => State::Draft,
        _ => State::Open,
    };
    let rollup = value.get("statusCheckRollup").and_then(|rollup| rollup.as_array());
    Some(PullRequest {
        number: value.get("number")?.as_u64()?,
        state,
        checks: count_checks(rollup.map_or(&[], Vec::as_slice)),
        url: text("url")?.to_string(),
    })
}

/// Asks gh in `repository` for the pull request of the branch checked out there. Blocks: run on a thread of its own.
fn ask_gh(repository: &Path) -> Option<PullRequest> {
    let mut child = Command::new("gh")
        .args(["pr", "view", "--json", FIELDS])
        .current_dir(repository)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read while it runs: a large answer would otherwise fill the pipe and hold gh until it is stopped.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + GH_TIMEOUT;
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(GH_WAIT_STEP),
            _ => {
                if let Err(error) = child.kill() {
                    eprintln!("nekoland-terminal: gh did not stop: {error}");
                }
                let _ = child.wait();
                return None;
            }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    succeeded.then(|| parse(&String::from_utf8_lossy(&bytes))).flatten()
}

/// A repository's folder and the branch checked out there.
pub type Key = (PathBuf, String);

struct Known {
    pull_request: Option<PullRequest>,
    asked: Instant,
}

/// What gh said of each repository and branch, asked again when it grows old, never twice at a time.
pub struct PullRequests {
    known: HashMap<Key, Known>,
    asking: HashSet<Key>,
    answers: Receiver<(Key, Option<PullRequest>)>,
    sender: Sender<(Key, Option<PullRequest>)>,
}

impl Default for PullRequests {
    fn default() -> Self {
        let (sender, answers) = channel();
        Self { known: HashMap::new(), asking: HashSet::new(), answers, sender }
    }
}

impl PullRequests {
    /// Takes in the answers gh gave since.
    pub fn collect(&mut self) {
        while let Ok((key, pull_request)) = self.answers.try_recv() {
            self.asking.remove(&key);
            self.known.insert(key, Known { pull_request, asked: Instant::now() });
        }
    }

    /// The pull request of `key` as last known, asking gh again when that is old.
    pub fn get(&mut self, key: &Key) -> Option<PullRequest> {
        let known = self.known.get(key);
        let stale = known.is_none_or(|known| {
            let settled = known.pull_request.as_ref().is_none_or(PullRequest::settled);
            known.asked.elapsed() >= if settled { SETTLED } else { FRESH }
        });
        if stale && self.asking.insert(key.clone()) {
            let (key, sender) = (key.clone(), self.sender.clone());
            std::thread::spawn(move || {
                let pull_request = ask_gh(&key.0);
                // The window may have gone; nobody is left to tell.
                let _ = sender.send((key, pull_request));
            });
        }
        known.and_then(|known| known.pull_request.clone())
    }

    /// Forgets what no tab is on any more.
    pub fn retain(&mut self, keys: &[Key]) {
        self.known.retain(|key, _| keys.contains(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request(state: &str, draft: bool, rollup: &str) -> PullRequest {
        parse(&format!(
            r#"{{"number":142,"state":"{state}","isDraft":{draft},"url":"https://github.com/o/r/pull/142","statusCheckRollup":[{rollup}]}}"#
        ))
        .unwrap()
    }

    #[test]
    fn the_state_follows_merged_closed_then_draft() {
        assert_eq!(pull_request("OPEN", false, "").state, State::Open);
        assert_eq!(pull_request("OPEN", true, "").state, State::Draft);
        assert_eq!(pull_request("MERGED", true, "").state, State::Merged);
        assert_eq!(pull_request("CLOSED", false, "").state, State::Closed);
    }

    #[test]
    fn checks_are_counted_as_gh_counts_them() {
        let rollup = r#"{"status":"COMPLETED","conclusion":"SUCCESS"},{"status":"IN_PROGRESS"},{"state":"FAILURE"},{"status":"COMPLETED","conclusion":"SKIPPED"},{"state":"STALE"}"#;
        assert_eq!(pull_request("OPEN", false, rollup).checks, Checks { pending: 2, passing: 2, failing: 1 });
    }

    #[test]
    fn what_is_not_a_pull_request_is_none() {
        for text in ["", "no pull requests found", "[]", r#"{"number":-1,"state":"OPEN","url":"u"}"#, r#"{"number":1,"state":"OPEN","url":""}"#] {
            assert_eq!(parse(text), None, "{text}");
        }
    }

    #[test]
    fn a_summary_names_the_number_and_the_state() {
        assert_eq!(pull_request("MERGED", false, "").summary(), "#142 merged");
    }
}
