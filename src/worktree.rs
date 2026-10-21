//! Worktrees as spaces (ADR 0004, as amended), the part the app's keys
//! (`C-a W`, `C-a O`, sidebar `x`) and the `paddock worktree` command
//! share: the layout and ownership rules, which spaces a worktree holds,
//! what a delete would lose, and making and removing a worktree with
//! nothing left half-done. Each failure says what kind it is, so the
//! command can give it an exit code; the app shows it as a notice.

use crate::git::{self, WorkingTree, Worktree};
use crate::tmux::structure::FLOAT_OPTION;
use crate::tmux::Server;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// How long the whole check of what a delete would lose may take.
pub const RISK_DEADLINE: Duration = Duration::from_secs(5);
/// What a base whose name is not known is called.
pub const BASE: &str = "its base";

/// Why making, opening or removing a worktree failed.
#[derive(Debug)]
pub enum Failure {
    /// The request itself is wrong: a bad branch name, a path that exists.
    Invalid(String),
    /// git refused, with its reason.
    Git(String),
    /// git did not answer in time.
    Late(String),
    /// What would be lost, or who would be hit, was not agreed to.
    Refused(String),
    /// tmux refused (the space could not be made or closed).
    Tmux(String),
}

impl Failure {
    /// A git failure, told apart by whether git ran out of time.
    pub fn git(reason: String) -> Self {
        if git::is_late(&reason) {
            Self::Late(reason)
        } else {
            Self::Git(reason)
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(m) | Self::Git(m) | Self::Late(m) | Self::Refused(m) | Self::Tmux(m) => {
                f.write_str(m)
            }
        }
    }
}

impl std::error::Error for Failure {}

/// A worktree space, as deleting it needs: its tmux session, every space
/// in that worktree (it first, read again before deleting), its
/// worktree's directory and branch (`None` if detached), and the
/// repository's main checkout, where git runs once the worktree is gone.
// ponytail: paths are `String` here and in `git` (as tmux and git print
// them) but `PathBuf` in config and `worktree_path`; one type would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSpace {
    pub space: String,
    pub spaces: Vec<String>,
    pub path: String,
    pub branch: Option<String>,
    pub main: String,
}

impl WorktreeSpace {
    /// What the questions call it: its branch, else (detached) its directory's name.
    pub fn label(&self) -> &str {
        self.branch
            .as_deref()
            .unwrap_or_else(|| git::dir_name(&self.path))
    }
}

/// What deleting a worktree would lose, beyond its ignored files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Risks {
    /// Uncommitted changes (untracked files count).
    pub dirty: bool,
    /// The base's name, if commits checked out there are not in it.
    pub unmerged: Option<String>,
    /// The check did not finish in time: nothing is known, so both of
    /// the above are assumed.
    pub late: bool,
}

impl Risks {
    /// What a check that did not finish in time assumes: every risk.
    pub fn unknown() -> Self {
        Self {
            dirty: true,
            unmerged: Some(BASE.into()),
            late: true,
        }
    }
}

/// What deleting the worktree at `path` would lose, read in at most
/// `limit` all told (its several git reads share it); `None` if nothing.
/// Not known in time, it counts as every risk ([`Risks::unknown`]).
pub fn risks_within(path: &str, limit: Duration) -> Option<Risks> {
    let (tx, rx) = mpsc::channel();
    let path = path.to_string();
    // Left to finish on its own if late: each read has its own timeout.
    std::thread::spawn(move || {
        let dirty = git::is_dirty(&path);
        let unmerged = (!git::in_base(&path, "HEAD"))
            .then(|| git::default_branch(&path).unwrap_or_else(|| BASE.into()));
        let risky = dirty || unmerged.is_some();
        let _ = tx.send(risky.then_some(Risks {
            dirty,
            unmerged,
            late: false,
        }));
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| Some(Risks::unknown()))
}

/// Whether the worktree at `top` is one paddock made, and so may delete:
/// `<directory>/<repo>/<name>`, as `C-a W` lays them out, exactly. A
/// worktree deeper down (a repository's `.claude/worktrees/agent-*` when
/// repositories live under `directory`) is not, nor one right inside its
/// own main checkout (a repository at `<directory>/<repo>` holding one).
pub fn is_ours(directory: &Path, tree: &WorkingTree) -> bool {
    let directory = git::resolved(&directory.to_string_lossy());
    let parent = Path::new(&tree.top).parent();
    parent.and_then(Path::parent) == Some(Path::new(&directory))
        && parent != Some(Path::new(&tree.main))
}

/// The name a space in worktree `w` of repository `repo` gets:
/// `<repo>/<branch>`, or `<repo>` for the main checkout, or the
/// directory's name after the repository for a detached one.
pub fn space_name(repo: &str, main: &str, w: &Worktree) -> String {
    if w.path == main {
        return repo.to_string();
    }
    let name = w
        .branch
        .as_deref()
        .unwrap_or_else(|| git::dir_name(&w.path));
    format!("{repo}/{name}")
}

/// Check `branch` out in a new worktree of `tree`'s repository at
/// `<directory>/<repo>/<branch>` (creating the branch at `tree`'s HEAD if
/// there is none), then `make_space` there, named `<repo>/<branch>`.
/// Nothing is left half-made: if the space fails, the worktree goes
/// again, and the branch too if it was made for it. Returns the path and
/// what `make_space` gave.
pub fn create<T>(
    directory: &Path,
    tree: &WorkingTree,
    branch: &str,
    make_space: impl FnOnce(&str, PathBuf) -> anyhow::Result<T>,
) -> Result<(PathBuf, T), Failure> {
    let branch = branch.trim();
    if !git::valid_branch(&tree.top, branch) {
        return Err(Failure::Invalid(format!(
            "{branch:?} is not a valid branch name"
        )));
    }
    let repo = tree.repo();
    let path = git::worktree_path(directory, repo, branch);
    if path.exists() {
        return Err(Failure::Invalid(format!("{} exists", path.display())));
    }
    // From the given working tree: a new branch starts at its HEAD.
    let created = git::add_worktree(&tree.top, &path, branch).map_err(Failure::git)?;
    let name = format!("{repo}/{branch}");
    match make_space(&name, path.clone()) {
        Ok(made) => Ok((path, made)),
        Err(e) => {
            // Best effort: it was made a moment ago, with nothing in it.
            let _ = git::remove_worktree(&tree.main, &path.to_string_lossy(), true);
            if created {
                let _ = git::delete_branch(&tree.main, branch);
            }
            Err(Failure::Tmux(format!("{e:#}")))
        }
    }
}

/// Space `id`'s directory: that of its first tab's first pane, else the
/// session's start directory.
pub fn space_dir(server: &Server, id: &str) -> Option<String> {
    let format = format!("#{{{FLOAT_OPTION}}}\x1f#{{pane_current_path}}");
    let pane = server
        .run(&["list-panes", "-s", "-t", id, "-F", &format])
        .ok()
        .and_then(|out| first_tab_dir(&out));
    pane.or_else(|| server.display(id, "#{session_path}").ok())
        .filter(|p| !p.is_empty())
}

/// The first directory in `list-panes -s` output of float marker and
/// directory, skipping floats' panes: a float is never the first tab,
/// even when tmux lists it first.
fn first_tab_dir(out: &str) -> Option<String> {
    out.lines()
        .filter_map(|l| l.split_once('\x1f'))
        .find(|(float, _)| float.is_empty())
        .map(|(_, dir)| dir.to_string())
        .filter(|dir| !dir.is_empty())
}

/// Which of spaces `ids` are in the working tree at `top`, read now: a
/// space belongs to the worktree of its first tab's first pane. If git
/// does not answer in time for any space, it fails: a space is never
/// skipped unread.
// ponytail: a space whose other panes are in the worktree is missed.
pub fn spaces_in(server: &Server, ids: &[String], top: &str) -> Result<Vec<String>, Failure> {
    let mut found = Vec::new();
    for id in ids {
        let Some(dir) = space_dir(server, id) else {
            continue;
        };
        let tree = git::working_tree_in_time(&dir).map_err(|_| {
            Failure::Late("couldn't read every space in time: nothing deleted".into())
        })?;
        if tree.is_some_and(|t| t.top == top) {
            found.push(id.clone());
        }
    }
    Ok(found)
}

/// Close every space in `target`'s worktree, remove the worktree and
/// delete its branch. Uncommitted changes go only if `confirmed` says so
/// (`--force`); changes or commits not confirmed, found now, leave
/// everything as it is. Returns a notice if the branch was kept (`branch
/// -d` found it unmerged); that is not a failure.
pub fn remove(
    server: &Server,
    target: &WorktreeSpace,
    confirmed: Option<&Risks>,
) -> Result<Option<String>, Failure> {
    let path = &target.path;
    let label = target.label();
    let force = confirmed.is_some_and(|r| r.dirty);
    if !force && git::is_dirty(path) {
        return Err(Failure::Refused(format!(
            "{label} has uncommitted changes now: nothing deleted"
        )));
    }
    let unmerged_ok = confirmed.is_some_and(|r| r.unmerged.is_some());
    if !unmerged_ok && !git::in_base(path, "HEAD") {
        return Err(Failure::Refused(format!(
            "{label} has commits not in its base now: nothing deleted"
        )));
    }
    let alive: Vec<String> = server.sessions().into_iter().map(|s| s.id).collect();
    for space in target.spaces.iter().filter(|s| alive.contains(s)) {
        server
            .kill_session(space)
            .map_err(|e| Failure::Tmux(format!("{e:#}")))?;
    }
    // The path first: a notice cut to the tab bar still names it.
    git::remove_worktree(&target.main, path, force).map_err(|e| {
        let late = git::is_late(&e);
        let left = crate::app::HomePaths::new().show(path);
        let message = format!("space closed; {left} left: {e}");
        if late {
            Failure::Late(message)
        } else {
            Failure::Git(message)
        }
    })?;
    Ok(target.branch.as_ref().and_then(|branch| {
        git::delete_branch(&target.main, branch)
            .err()
            .map(|e| format!("worktree removed; branch {branch} kept: {e}"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(top: &str, main: &str) -> WorkingTree {
        WorkingTree {
            top: top.into(),
            main: main.into(),
        }
    }

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: path.into(),
            branch: branch.map(str::to_string),
        }
    }

    #[test]
    fn a_worktree_space_is_named_after_its_repository_and_branch() {
        assert_eq!(
            space_name("api", "/r/api", &wt("/r/api", Some("main"))),
            "api"
        );
        assert_eq!(
            space_name("api", "/r/api", &wt("/w/api/feat-x", Some("feat/x"))),
            "api/feat/x"
        );
        assert_eq!(
            space_name("api", "/r/api", &wt("/w/api/loose", None)),
            "api/loose"
        );
    }

    #[test]
    fn only_worktrees_laid_out_as_paddock_makes_them_are_its_own() {
        let dir = Path::new("/nonexistent/code");
        let elsewhere = "/r/repo";
        let ours = |top: &str, main: &str| is_ours(dir, &tree(top, main));
        assert!(ours("/nonexistent/code/repo/feat-x", elsewhere));
        assert!(!ours(
            "/nonexistent/code/repo/.claude/worktrees/agent-x",
            "/nonexistent/code/repo"
        ));
        assert!(!ours("/elsewhere/repo/feat-x", elsewhere));
        // A repository at `<directory>/<repo>` with a worktree right in it.
        assert!(!ours(
            "/nonexistent/code/repo/feat-x",
            "/nonexistent/code/repo"
        ));
    }

    #[test]
    fn a_risk_check_past_its_deadline_counts_as_every_risk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        let late = risks_within(&dir, Duration::ZERO);
        assert_eq!(late, Some(Risks::unknown()));
    }

    #[test]
    fn a_space_s_directory_is_its_first_tab_s_never_a_float_s() {
        let out = "g\x1f/w/float\n\x1f/w/tab\n\x1f/w/second\n";
        assert_eq!(first_tab_dir(out).as_deref(), Some("/w/tab"));
        assert_eq!(
            first_tab_dir("\x1f/w/tab\ng\x1f/w/float\n").as_deref(),
            Some("/w/tab")
        );
        assert_eq!(first_tab_dir("g\x1f/w/float\n"), None);
        assert_eq!(first_tab_dir("\x1f\n"), None);
    }

    #[test]
    fn a_git_failure_says_whether_git_ran_out_of_time() {
        let late = Failure::git(git::late_reason(Duration::from_secs(60)));
        assert!(matches!(late, Failure::Late(_)), "{late:?}");
        let refused = Failure::git("'main' is already used by worktree".into());
        assert!(matches!(refused, Failure::Git(_)), "{refused:?}");
    }
}
