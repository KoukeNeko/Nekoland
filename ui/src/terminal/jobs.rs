//! Which program runs in a pane's foreground, and whether it is a coding agent. An agent
//! started through an interpreter, such as `node …/claude`, is known by its script's name.

use std::path::{Path, PathBuf};

use super::agent::{agent_named, Job};
use super::proc;

/// How many of the command's first words are looked at for an agent's name: the program,
/// and what an interpreter runs.
const WORDS_FOR_AGENT: usize = 2;

/// The foreground job of a pane whose shell is `shell`, when it is not the shell itself.
pub fn foreground_job(group: i32, shell: i32) -> Option<Job> {
    if group == shell {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{group}/stat")).ok()?;
    let (comm, _) = proc::parse_stat(&stat)?;
    let cmdline = std::fs::read(format!("/proc/{group}/cmdline")).unwrap_or_default();
    let program = proc::full_name(&comm, &cmdline);
    let agent = agent_named(&program).or_else(|| agent_in_command(&cmdline));
    Some(Job { program, agent })
}

fn agent_in_command(cmdline: &[u8]) -> Option<&'static str> {
    cmdline
        .split(|byte| *byte == 0)
        .take(WORDS_FOR_AGENT)
        .filter_map(|word| std::str::from_utf8(word).ok())
        .filter_map(|word| Path::new(word).file_name()?.to_str())
        .find_map(agent_named)
}

const GIT: &str = ".git";
const GITDIR_PREFIX: &str = "gitdir:";
const BRANCH_PREFIX: &str = "ref: refs/heads/";
/// How much of a commit's hash names a detached HEAD.
const SHORT_HASH: usize = 7;

/// The repository `directory` is in: its folder's name and the branch checked out there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    /// The folder with the `.git`: the checkout, or the worktree.
    pub root: PathBuf,
    pub name: String,
    pub branch: Option<String>,
}

pub fn repository_of(directory: &Path) -> Option<Repository> {
    let root = directory
        .ancestors()
        .find(|folder| folder.join(GIT).exists())?;
    Some(Repository {
        root: root.to_path_buf(),
        name: root.file_name()?.to_str()?.to_string(),
        branch: branch_of(&root.join(GIT)),
    })
}

/// The branch in `.git/HEAD`; a worktree's `.git` is a file naming the folder that holds its HEAD.
fn branch_of(git: &Path) -> Option<String> {
    let folder = if git.is_dir() {
        git.to_path_buf()
    } else {
        let pointer = std::fs::read_to_string(git).ok()?;
        let target = pointer.trim().strip_prefix(GITDIR_PREFIX)?.trim();
        git.parent()?.join(target)
    };
    let head = std::fs::read_to_string(folder.join("HEAD")).ok()?;
    Some(parse_head(&head))
}

fn parse_head(head: &str) -> String {
    let head = head.trim();
    match head.strip_prefix(BRANCH_PREFIX) {
        Some(branch) => branch.to_string(),
        None => head.chars().take(SHORT_HASH).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_run_by_an_interpreter_is_known_by_its_script() {
        assert_eq!(
            agent_in_command(b"node\0/usr/lib/node_modules/.bin/claude\0--resume\0"),
            Some("Claude Code")
        );
        assert_eq!(agent_in_command(b"node\0server.js\0"), None);
    }

    #[test]
    fn head_names_the_branch_or_the_commit() {
        assert_eq!(parse_head("ref: refs/heads/feature/x\n"), "feature/x");
        assert_eq!(parse_head("9c0d3f6a1b2c\n"), "9c0d3f6");
    }
}
