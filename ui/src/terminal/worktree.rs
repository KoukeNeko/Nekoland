//! A git worktree for an agent, as Nekominal makes one: a second checkout of the repository on a branch of its own, so
//! that two agents (or an agent and the user) work on one repository without touching each other's files.
//!
//! It goes beside the main checkout, `<parent>/<repo>-<branch>`: `~/src/app` and the branch `fix/login` make
//! `~/src/app-fix-login`, the layout of `git worktree`'s own examples. A new worktree gets the ignored files
//! `.worktreeinclude` names (`.env`…), and its tab starts with the repository's setup script typed, not run.
//!
//! A worktree is removed (`git worktree remove`, then its branch with `git branch -d`) or archived (the folder goes, the
//! branch stays): never the main checkout, and never with changes not committed. git runs with its arguments, never
//! through a shell.

use std::{
    ffi::{OsStr, OsString},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const INCLUDE_FILE: &str = ".worktreeinclude";
const SETUP_SCRIPT: &str = ".nekominal/setup.sh";
/// What a folder name cannot hold of a branch's: the `/` of `fix/login`, and what shells and other systems trip on.
const FOLDER_UNSAFE: &str = "/<>|\"";
const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];
/// `git check-ignore`'s status when none of the paths is ignored.
const NONE_IGNORED: i32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The folder is not in a git repository (or git is not installed).
    NotRepo,
    /// What git said when it refused.
    Git(String),
    /// The rules below say no before git is run.
    Refused(Refusal),
}

/// Why a worktree is not removed or archived.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The main checkout holds the repository itself.
    Main,
    /// Changes not committed and files not tracked, which removing the folder would lose.
    Dirty { changed: usize, untracked: usize },
    /// On no branch: nothing would keep its commits once the folder goes.
    Detached,
}

/// What a worktree holds, as git sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub path: PathBuf,
    /// None when HEAD is on no branch.
    pub branch: Option<String>,
    /// The main checkout of the repository, not a worktree added to it.
    pub main: bool,
    /// Changed tracked files (staged or not), and files not tracked.
    pub changed: usize,
    pub untracked: usize,
    /// Files and folders git ignores (`.env`, build output): the removal of the folder deletes them too.
    pub ignored: usize,
    /// The changed files with the letter of their state (`M`, `A`, `R`, `?`…), as git lists them.
    pub files: Vec<(char, String)>,
    /// The remote the branch pushes to (its upstream's, else the first one); None when there is none.
    pub remote: Option<String>,
    /// Commits ahead of the upstream, or, with no upstream, on no remote branch; None when git could not tell.
    pub unpushed: Option<usize>,
    /// The default branch (`origin/HEAD`, else `main` or `master`) and whether HEAD is merged into it.
    pub default_branch: Option<String>,
    pub merged: Option<bool>,
}

impl Status {
    pub fn dirty(&self) -> bool {
        self.changed + self.untracked > 0
    }
}

/// Whether git takes `name` as a new branch's: the rules of `git check-ref-format --branch`, checked as it is typed.
pub fn valid_branch(name: &str) -> bool {
    let bad_char = |ch: char| ch.is_ascii_control() || " ~^:?*[\\".contains(ch);
    !name.is_empty()
        && name != "HEAD"
        && name != "@"
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.chars().any(bad_char)
        && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// The folder of the worktree for `branch` beside the repository at `root`: `<root>-<branch>`, `-2`, `-3`… while taken.
pub fn worktree_path(root: &Path, branch: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let repository = root.file_name().map_or_else(|| "repo".to_string(), |name| name.to_string_lossy().into_owned());
    let folder: String = branch.chars().map(|ch| if FOLDER_UNSAFE.contains(ch) { '-' } else { ch }).collect();
    let base = format!("{repository}-{folder}");
    let parent = root.parent().unwrap_or(root);
    (1..)
        .map(|number| parent.join(if number == 1 { base.clone() } else { format!("{base}-{number}") }))
        .find(|path| !exists(path))
        .expect("an unused name is found")
}

fn git_not_run(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "git was not found on the PATH".to_string()
    } else {
        format!("git: {error}")
    }
}

/// The last line git complained with: the `fatal:` line, not the progress before it.
fn last_said(stderr: &[u8], fallback: &str) -> String {
    let said = String::from_utf8_lossy(stderr);
    said.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or(fallback).trim().to_string()
}

/// Runs git in `folder`: what it wrote, or the last line of what it complained of.
fn git<I, S>(folder: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git").arg("-C").arg(folder).args(args).output().map_err(git_not_run)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Err(last_said(&output.stderr, "git failed"))
}

/// Which of `paths` git ignores in `root`, separated by NUL; the paths go on its standard input, as many as there are.
fn check_ignore(root: &Path, paths: &[&str]) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(git_not_run)?;
    let input = paths.join("\0").into_bytes();
    let mut stdin = child.stdin.take().ok_or("git check-ignore has no input")?;
    // Written from a thread of its own: git answers while it reads, and a full pipe both ways would hold both.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output().map_err(|error| format!("git: {error}"))?;
    match writer.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("git check-ignore: {error}")),
        Err(_) => return Err("git check-ignore: the writer stopped".to_string()),
    }
    match output.status.code() {
        Some(0) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Some(NONE_IGNORED) => Ok(String::new()),
        _ => Err(last_said(&output.stderr, "git check-ignore failed")),
    }
}

/// The main checkout of the repository `folder` is in, also from one of its worktrees, so they all sit beside it.
pub fn repository_root(folder: &Path) -> Result<PathBuf, Error> {
    let common = git(folder, ["rev-parse", "--path-format=absolute", "--git-common-dir"]).map_err(|_| Error::NotRepo)?;
    let common = PathBuf::from(common);
    match common.parent() {
        Some(root) if common.file_name().is_some_and(|name| name == ".git") => Ok(root.to_path_buf()),
        // A bare repository or a separate git folder has no checkout to sit beside.
        _ => Err(Error::NotRepo),
    }
}

/// Makes a worktree on the new branch `branch` for the repository `folder` is in: its folder, and why the files
/// `.worktreeinclude` names were not copied, when they were not.
pub fn create(folder: &Path, branch: &str) -> Result<(PathBuf, Option<String>), Error> {
    let root = repository_root(folder)?;
    git(&root, ["check-ref-format", "--branch", branch]).map_err(Error::Git)?;
    let path = worktree_path(&root, branch, Path::exists);
    let args: [&OsStr; 6] =
        ["worktree".as_ref(), "add".as_ref(), "-b".as_ref(), branch.as_ref(), "--".as_ref(), path.as_os_str()];
    git(&root, args).map_err(Error::Git)?;
    let copy_failed = copy_included(&root, &path).err();
    Ok((path, copy_failed))
}

/// Copies into the new worktree `destination` the files of `root` that `.worktreeinclude` names and git ignores; a file
/// already there is left alone. The files copied, as git names them. The patterns are `.gitignore`'s, which git matches.
pub fn copy_included(root: &Path, destination: &Path) -> Result<Vec<String>, String> {
    let include = root.join(INCLUDE_FILE);
    if !include.is_file() {
        return Ok(Vec::new());
    }
    let mut exclude_from = OsString::from("--exclude-from=");
    exclude_from.push(&include);
    let listed = git(root, [OsString::from("ls-files"), "-z".into(), "--others".into(), "--ignored".into(), exclude_from])?;
    let candidates: Vec<&str> = listed.split('\0').filter(|path| !path.is_empty()).collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let ignored = check_ignore(root, &candidates)?;
    let mut copied = Vec::new();
    for file in ignored.split('\0').filter(|path| !path.is_empty()) {
        let target = destination.join(file);
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{file}: {error}"))?;
        }
        std::fs::copy(root.join(file), &target).map_err(|error| format!("{file}: {error}"))?;
        copied.push(file.to_string());
    }
    Ok(copied)
}

/// What to type in a new worktree's tab when the repository has a setup script: typed, not run, as it comes with the
/// repository and the user should see it first.
pub fn setup_command(worktree: &Path) -> Option<String> {
    worktree.join(SETUP_SCRIPT).is_file().then(|| format!("sh {SETUP_SCRIPT}"))
}

/// A line of `git status --porcelain --ignored` for what git ignores.
fn ignored_line(line: &str) -> bool {
    line.trim_start().starts_with("!!")
}

/// Changed and untracked files in `git status --porcelain`; the ignored ones are counted apart.
pub fn parse_porcelain(text: &str) -> (usize, usize) {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !ignored_line(line))
        .fold((0, 0), |(changed, untracked), line| {
            if line.starts_with("??") {
                (changed, untracked + 1)
            } else {
                (changed + 1, untracked)
            }
        })
}

/// The files of `git status --porcelain` with the letter of their state, `?` for one not tracked; git's quotes around a
/// path with a space are taken off.
pub fn parse_files(text: &str) -> Vec<(char, String)> {
    let unquote = |path: &str| path.strip_prefix('"').and_then(|path| path.strip_suffix('"')).unwrap_or(path).to_string();
    text.lines()
        .filter(|line| !ignored_line(line))
        .filter_map(|line| {
            let (code, path) = line.trim_start().split_once(' ')?;
            let path = path.trim_start().split(" -> ").map(unquote).collect::<Vec<_>>().join(" -> ");
            Some((code.chars().next()?, path))
        })
        .collect()
}

/// (behind, ahead) in `git rev-list --left-right --count @{upstream}...HEAD`.
pub fn parse_ahead_behind(text: &str) -> Option<(usize, usize)> {
    let mut counts = text.split_whitespace().map(str::parse::<usize>);
    match (counts.next(), counts.next(), counts.next()) {
        (Some(Ok(behind)), Some(Ok(ahead)), None) => Some((behind, ahead)),
        _ => None,
    }
}

fn remote_of(path: &Path, branch: Option<&str>) -> Option<String> {
    branch
        .and_then(|branch| git(path, ["config", "--get", &format!("branch.{branch}.remote")]).ok())
        .or_else(|| git(path, ["remote"]).ok().and_then(|names| names.lines().next().map(str::to_string)))
        .filter(|name| !name.is_empty())
}

fn unpushed_of(path: &Path) -> Option<usize> {
    match git(path, ["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]) {
        Ok(counts) => parse_ahead_behind(&counts).map(|(_, ahead)| ahead),
        Err(_) => git(path, ["rev-list", "--count", "HEAD", "--not", "--remotes"]).ok()?.parse().ok(),
    }
}

fn default_branch_of(path: &Path) -> Option<String> {
    git(path, ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).ok().or_else(|| {
        DEFAULT_BRANCHES
            .into_iter()
            .find(|branch| git(path, ["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok())
            .map(str::to_string)
    })
}

/// Inspects the worktree `folder` is in.
pub fn inspect(folder: &Path) -> Result<Status, Error> {
    let folders = git(folder, ["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"])
        .map_err(|_| Error::NotRepo)?;
    let [top, git_dir, common] = folders.lines().collect::<Vec<_>>()[..] else {
        return Err(Error::NotRepo);
    };
    let path = PathBuf::from(top);
    let branch = git(&path, ["branch", "--show-current"]).ok().filter(|branch| !branch.is_empty());
    let porcelain = git(&path, ["-c", "core.quotePath=false", "status", "--porcelain", "--ignored"]).map_err(Error::Git)?;
    let (changed, untracked) = parse_porcelain(&porcelain);
    let default_branch = default_branch_of(&path);
    let merged = default_branch.as_ref().map(|default| git(&path, ["merge-base", "--is-ancestor", "HEAD", default]).is_ok());
    Ok(Status {
        remote: remote_of(&path, branch.as_deref()),
        unpushed: unpushed_of(&path),
        main: git_dir.trim_end_matches('/') == common.trim_end_matches('/'),
        ignored: porcelain.lines().filter(|line| ignored_line(line)).count(),
        files: parse_files(&porcelain),
        path,
        branch,
        changed,
        untracked,
        default_branch,
        merged,
    })
}

/// Whether the folder `folder` (a pane's) is `root` or inside it.
pub fn contains(root: &Path, folder: &Path) -> bool {
    folder.starts_with(root)
}

/// Whether the worktree may be removed: never the main checkout, and not with changes.
pub fn check_remove(status: &Status) -> Result<(), Refusal> {
    if status.main {
        return Err(Refusal::Main);
    }
    if status.dirty() {
        return Err(Refusal::Dirty { changed: status.changed, untracked: status.untracked });
    }
    Ok(())
}

/// Removes the worktree, then its branch when `delete_branch` (`git branch -d`, which keeps a branch not merged). What
/// git said when it kept the branch.
pub fn remove(status: &Status, delete_branch: bool) -> Result<Option<String>, Error> {
    check_remove(status).map_err(Error::Refused)?;
    if status.branch.is_none() {
        return Err(Error::Refused(Refusal::Detached));
    }
    // From the main checkout: the folder being removed is no place to run git in.
    let root = repository_root(&status.path)?;
    let args: [&OsStr; 3] = ["worktree".as_ref(), "remove".as_ref(), status.path.as_os_str()];
    git(&root, args).map_err(Error::Git)?;
    match &status.branch {
        // Without the advice git writes after "not fully merged", which would be the last line, the one told.
        Some(branch) if delete_branch => {
            Ok(git(&root, ["-c", "advice.forceDeleteBranch=false", "branch", "-d", branch]).err())
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_follow_the_rules_of_git_check_ref_format() {
        for good in ["fix", "fix/login", "feature/新功能", "v1.2", "a@b"] {
            assert!(valid_branch(good), "{good}");
        }
        for bad in ["", "HEAD", "@", "-x", "a..b", "a b", "a~", "a^", "a:", "a?", "a*", "a[", "a\\b", "a\u{7}", "a//b", "a/.b", "a.lock", "a.", "a@{b"] {
            assert!(!valid_branch(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_worktree_goes_beside_the_repository_named_after_it_and_the_branch() {
        let root = Path::new("/src/app");
        assert_eq!(worktree_path(root, "fix/login", |_| false), Path::new("/src/app-fix-login"));
        assert_eq!(worktree_path(root, "a<b>|\"c", |_| false), Path::new("/src/app-a-b---c"));
        let taken = |path: &Path| path == Path::new("/src/app-x") || path == Path::new("/src/app-x-2");
        assert_eq!(worktree_path(root, "x", taken), Path::new("/src/app-x-3"));
    }

    #[test]
    fn porcelain_counts_changed_untracked_and_names_files() {
        let text = "M  a.rs\n?? new dir/\nR  \"old name.txt\" -> \"new name.txt\"\n!! target/\n";
        assert_eq!(parse_porcelain(text), (2, 1));
        assert_eq!(
            parse_files(text),
            [('M', "a.rs".into()), ('?', "new dir/".into()), ('R', "old name.txt -> new name.txt".into())]
        );
        assert_eq!(parse_ahead_behind("2\t5"), Some((2, 5)));
        assert_eq!(parse_ahead_behind("x"), None);
    }

    #[test]
    fn a_folder_is_in_a_worktree_only_below_it() {
        let root = Path::new("/src/app-fix");
        assert!(contains(root, Path::new("/src/app-fix")));
        assert!(contains(root, Path::new("/src/app-fix/sub")));
        assert!(!contains(root, Path::new("/src/app-fix-2")));
        assert!(!contains(root, Path::new("/src")));
    }

    fn git_in(folder: &Path, args: &[&str]) {
        let status = Command::new("git").arg("-C").arg(folder).args(args).output().unwrap().status;
        assert!(status.success(), "git {args:?}");
    }

    /// A repository with a commit in a fresh temporary folder; none when git is not installed.
    fn repository(name: &str) -> Option<PathBuf> {
        Command::new("git").arg("--version").output().ok()?;
        let base = std::env::temp_dir().join(format!("nekoland-worktree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("app");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
        ] {
            git_in(&root, args);
        }
        std::fs::write(root.join("README"), "hi").unwrap();
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        std::fs::write(root.join(INCLUDE_FILE), "# copied\n.env\n").unwrap();
        git_in(&root, &["add", "README", ".gitignore", INCLUDE_FILE]);
        git_in(&root, &["commit", "-q", "-m", "first"]);
        Some(root)
    }

    #[test]
    fn a_real_worktree_is_made_beside_the_repository_and_can_be_removed() {
        let Some(root) = repository("real") else { return };
        let (path, copy_failed) = create(&root.join("."), "fix/login").unwrap();
        assert_eq!(path, root.parent().unwrap().join("app-fix-login"));
        assert_eq!(copy_failed, None);
        assert_eq!(std::fs::read_to_string(path.join(".env")).unwrap(), "SECRET=1");
        assert!(matches!(create(&path, "fix/login"), Err(Error::Git(_))), "the branch exists");

        let main = inspect(&root).unwrap();
        assert!(main.main);
        assert_eq!(check_remove(&main), Err(Refusal::Main));

        std::fs::write(path.join("work.txt"), "x").unwrap();
        let dirty = inspect(&path).unwrap();
        assert_eq!((dirty.main, dirty.untracked, dirty.branch.as_deref()), (false, 1, Some("fix/login")));
        assert_eq!(remove(&dirty, true), Err(Error::Refused(Refusal::Dirty { changed: 0, untracked: 1 })));

        std::fs::remove_file(path.join("work.txt")).unwrap();
        let clean = inspect(&path).unwrap();
        assert_eq!((clean.remote.as_deref(), clean.merged, clean.ignored), (None, Some(true), 1));
        assert_eq!(remove(&clean, true).unwrap(), None);
        assert!(!path.exists());
        assert!(git(&root, ["rev-parse", "--verify", "--quiet", "refs/heads/fix/login"]).is_err(), "the branch went");
        assert_eq!(repository_root(&std::env::temp_dir()), Err(Error::NotRepo));
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
