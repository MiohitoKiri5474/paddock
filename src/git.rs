//! What paddock reads from git: the branch a directory has checked out, a
//! branch's status (ahead/behind its upstream, uncommitted line changes),
//! and the repository's worktrees; and the one thing it changes: it adds
//! and removes worktrees with their branches (ADR 0004, as amended).
//! Nothing else touches a branch, the index or a working tree. Telling a
//! squash merge writes one unreferenced commit object.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// Branches taken as the default when `origin/HEAD` names none.
const DEFAULTS: &[&str] = &["main", "master"];
/// Who makes the commit that tells a squash merge, and when: fixed, so
/// it needs no configured identity and is the same object every time.
const SQUASH_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "paddock"),
    ("GIT_AUTHOR_EMAIL", "paddock@localhost"),
    ("GIT_AUTHOR_DATE", "@0 +0000"),
    ("GIT_COMMITTER_NAME", "paddock"),
    ("GIT_COMMITTER_EMAIL", "paddock@localhost"),
    ("GIT_COMMITTER_DATE", "@0 +0000"),
];
/// Untracked files larger than this (bytes) are not counted in line changes.
const MAX_UNTRACKED: u64 = 1 << 20;
/// How long a git read may take before it counts as failed: reads run on
/// the key path too, and the screen waits for them.
// ponytail: the deadlines are per git call, not per key: `x` makes several
// reads, `C-a O` some per space, a delete several mutations, all on the UI
// thread. The upgrade is to run deletes and mutations off it.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long adding or removing a worktree may take (a checkout of a big
/// repository) before it counts as failed.
// ponytail: a `worktree remove` killed at this deadline can leave a
// half-deleted tree; git's own `worktree prune` cleans up after it.
const MUTATION_TIMEOUT: Duration = Duration::from_secs(60);
/// What follows a program's name in its error when it missed its
/// deadline: `git did not finish in 5s` (see [`late_reason`],
/// [`is_late`]).
const LATE: &str = " did not finish";
/// How often to look whether git finished.
const POLL: Duration = Duration::from_millis(5);

/// A task branch's git fields, as the sidebar shows them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    /// Commits ahead of and behind the upstream; `None` without one.
    pub ahead_behind: Option<(u32, u32)>,
    /// Uncommitted lines added and removed (untracked files count as added);
    /// `None` unless the branch is checked out in the directory asked about.
    pub changes: Option<(u32, u32)>,
}

/// `git -C dir args…`'s output, or `None` if it failed or took longer than
/// [`READ_TIMEOUT`].
fn git(dir: &str, args: &[&str]) -> Option<String> {
    git_with(dir, args, &[])
}

/// [`git`] with environment variables `env` set.
fn git_with(dir: &str, args: &[&str], env: &[(&str, &str)]) -> Option<String> {
    run(dir, args, env, READ_TIMEOUT).ok()
}

/// Run `git -C dir args…` for a change (adding or removing a worktree,
/// deleting a branch), waiting at most [`MUTATION_TIMEOUT`]; fails with
/// git's reason and the command.
fn run_mutation(dir: &str, args: &[&str]) -> Result<(), String> {
    let mut command = git_command(dir, args, &[]);
    // Nothing reads it: a chatty hook must not fill a pipe.
    command.stdout(Stdio::null());
    wait(command, MUTATION_TIMEOUT)
        .map(drop)
        .map_err(|e| format!("{e} (git {})", args.join(" ")))
}

/// A git read: `git -C dir args…` with environment variables `env` set,
/// waiting at most `timeout`. Returns its stdout, or why it failed.
fn run(
    dir: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = git_command(dir, args, env);
    command.stdout(Stdio::piped());
    wait(command, timeout)
}

/// `git -C dir args…` with environment variables `env` set, its stderr
/// piped for [`reason`]; the caller says where stdout goes.
fn git_command(dir: &str, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        // Polling must never take the index lock from Claude's own git.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// Every git paddock runs ends here (and `gh`, see [`crate::github`]):
/// start `command` and wait at most
/// `timeout` for it to exit and for its output to end, then give up (and
/// kill git if it still runs). A process git leaves behind holding its
/// pipes (a hook's `cmd &`) counts as not finished too. Returns stdout,
/// or why it failed (see [`reason`]). Never prints: hooks must stay
/// silent.
pub fn wait(mut command: Command, timeout: Duration) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let late = || late_reason(dir_name(&program), timeout);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    // Read both pipes while waiting, so a long output never stalls git.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => {
                // Best effort: it may have ended just now. The readers are
                // left to end with the pipes.
                let _ = child.kill();
                let _ = child.wait();
                return Err(late());
            }
            Err(e) => {
                // Cannot tell whether it runs: never leave it behind.
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
    };
    let left = || deadline.saturating_duration_since(Instant::now());
    let out = stdout.recv_timeout(left()).map_err(|_| late())?;
    if status.success() {
        return Ok(out);
    }
    // Without its words in time, the exit status says why.
    let said = stderr.recv_timeout(left()).unwrap_or_default();
    Err(reason(&said).unwrap_or_else(|| status.to_string()))
}

/// Why `program` (its name: `git`, `gh`) failed when it did not finish
/// within `timeout`.
pub fn late_reason(program: &str, timeout: Duration) -> String {
    format!("{program}{LATE} in {timeout:?}")
}

/// Whether `reason`, a failure from git, is git missing its deadline (see
/// [`late_reason`]), not git refusing. A change's reason ends with its
/// command, so only the start counts.
pub fn is_late(reason: &str) -> bool {
    reason
        .strip_prefix("git")
        .is_some_and(|rest| rest.starts_with(LATE))
}

/// Read all of `pipe` (none: nothing) on a thread of its own; the text
/// comes on the channel once the pipe ends.
fn drain(pipe: Option<impl Read + Send + 'static>) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    rx
}

/// The branch checked out in `dir`; `None` if detached or not a repository.
pub fn current_branch(dir: &str) -> Option<String> {
    let out = git(dir, &["branch", "--show-current"])?;
    Some(out.trim().to_string()).filter(|b| !b.is_empty())
}

/// What is checked out in `dir`: its branch, else (detached) the short
/// commit id; `None` outside a repository.
pub fn head(dir: &str) -> Option<String> {
    current_branch(dir).or_else(|| {
        let sha = git(dir, &["rev-parse", "--short", "HEAD"])?;
        Some(sha.trim().to_string()).filter(|s| !s.is_empty())
    })
}

/// The repository's default branch: what `origin/HEAD` points at, else the
/// first of `main` and `master` that exists.
pub fn default_branch(dir: &str) -> Option<String> {
    let base = base_ref(dir)?;
    let name = base
        .strip_prefix("refs/remotes/origin/")
        .or_else(|| base.strip_prefix("refs/heads/"))
        .unwrap_or(&base);
    Some(name.to_string())
}

/// The default branch as a full ref, which tasks are compared with: the
/// remote branch `origin/HEAD` points at, else the first of `main` and
/// `master` that exists.
fn base_ref(dir: &str) -> Option<String> {
    let origin = git(
        dir,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    );
    if let Some(r) = origin
        .map(|o| o.trim().to_string())
        .filter(|r| !r.is_empty())
    {
        return Some(r);
    }
    DEFAULTS
        .iter()
        .map(|b| format!("refs/heads/{b}"))
        .find(|r| git(dir, &["rev-parse", "--verify", "--quiet", r]).is_some())
}

/// The branch checked out in `dir` if it is a task's: not the default branch.
pub fn task_branch(dir: &str) -> Option<String> {
    let branch = current_branch(dir)?;
    (default_branch(dir).as_deref() != Some(branch.as_str())).then_some(branch)
}

/// One entry of `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The working tree's directory, with links resolved (as tmux reports
    /// a pane's directory), so it compares with a space's.
    pub path: String,
    /// The branch checked out there; `None` if detached.
    pub branch: Option<String>,
}

/// Every working tree of the repository `dir` is in, the main checkout
/// first; empty outside a repository.
pub fn worktrees(dir: &str) -> Vec<Worktree> {
    git(dir, &["worktree", "list", "--porcelain"])
        .map(|out| parse_worktrees(&out))
        .unwrap_or_default()
        .into_iter()
        .map(|w| Worktree {
            path: resolved(&w.path),
            ..w
        })
        .collect()
}

/// `worktree list --porcelain` output: a block per worktree, `worktree
/// <path>` first, then `HEAD`, `branch refs/heads/<name>` or `detached`,
/// and more attributes (`bare`, `locked`, `prunable`).
fn parse_worktrees(out: &str) -> Vec<Worktree> {
    out.split("\n\n")
        .filter_map(|block| {
            let path = block.lines().find_map(|l| l.strip_prefix("worktree "))?;
            let branch = block
                .lines()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
                .map(str::to_string);
            Some(Worktree {
                path: path.to_string(),
                branch,
            })
        })
        .collect()
}

/// Which working tree a directory is in: a repository's main checkout or
/// one of its linked worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingTree {
    /// The working tree's top directory, links resolved.
    pub top: String,
    /// The repository's main checkout (the first worktree listed), links
    /// resolved; the same as `top` unless `top` is a linked worktree.
    pub main: String,
}

impl WorkingTree {
    /// Whether this is a linked worktree, not the main checkout.
    pub fn is_linked(&self) -> bool {
        self.top != self.main
    }

    /// The repository's name: its main checkout's directory name.
    pub fn repo(&self) -> &str {
        dir_name(&self.main)
    }
}

/// The last part of `path`, or all of it if it has none.
pub fn dir_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// The working tree `dir` is in and its repository's main checkout;
/// `None` outside a repository, or if git did not answer in time.
pub fn working_tree(dir: &str) -> Option<WorkingTree> {
    working_tree_in_time(dir).ok().flatten()
}

/// [`working_tree`], telling git that did not answer in time (`Err`)
/// apart from a directory outside any repository (`Ok(None)`).
pub fn working_tree_in_time(dir: &str) -> Result<Option<WorkingTree>, String> {
    // A failure other than the deadline means: not a repository.
    let read = |args: &[&str]| match run(dir, args, &[], READ_TIMEOUT) {
        Ok(out) => Ok(Some(out)),
        Err(e) if is_late(&e) => Err(e),
        Err(_) => Ok(None),
    };
    let Some(top) = read(&["rev-parse", "--show-toplevel"])? else {
        return Ok(None);
    };
    let top = top.trim();
    let Some(list) = read(&["worktree", "list", "--porcelain"])? else {
        return Ok(None);
    };
    let Some(main) = parse_worktrees(&list).into_iter().next() else {
        return Ok(None);
    };
    Ok((!top.is_empty()).then(|| WorkingTree {
        top: resolved(top),
        main: resolved(&main.path),
    }))
}

/// `path` with links resolved (`/tmp` is `/private/tmp` on macOS), as tmux
/// reports a pane's directory; as given if that fails.
pub fn resolved(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

/// Where a worktree of repository `repo` for `branch` goes under
/// `directory`: `<directory>/<repo>/<branch>`, slashes in the branch
/// written `-`.
pub fn worktree_path(directory: &Path, repo: &str, branch: &str) -> PathBuf {
    directory.join(repo).join(branch.replace('/', "-"))
}

/// Whether `name` may name a branch (`git check-ref-format --branch`, run
/// in `dir`). It takes no `--`, so a name starting with `-` is refused
/// here, as is one with `@{` (git expands it, so it is not literal).
pub fn valid_branch(dir: &str, name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        // `@{-1}` and the like pass, but git expands them: not literal.
        && !name.contains("@{")
        && git(dir, &["check-ref-format", "--branch", name]).is_some()
}

/// Whether branch `branch` exists in the repository `dir` is in.
pub fn has_branch(dir: &str, branch: &str) -> bool {
    let tip = format!("refs/heads/{branch}");
    git(dir, &["rev-parse", "--verify", "--quiet", &tip]).is_some()
}

/// Check `branch` out in a new worktree at `path`, from the repository
/// `repo` is in, creating the branch (from `HEAD`) if there is none.
/// Returns whether the branch was created; fails with git's reason.
pub fn add_worktree(repo: &str, path: &Path, branch: &str) -> Result<bool, String> {
    let path = path.to_string_lossy();
    let created = !has_branch(repo, branch);
    let args: &[&str] = if created {
        &["worktree", "add", "-b", branch, "--", &path]
    } else {
        &["worktree", "add", "--", &path, branch]
    };
    run_mutation(repo, args)?;
    Ok(created)
}

/// Remove the worktree at `path` of the repository `repo` is in, with its
/// ignored files; `force` discards uncommitted changes too. Fails with
/// git's reason.
pub fn remove_worktree(repo: &str, path: &str, force: bool) -> Result<(), String> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", path]);
    run_mutation(repo, &args)
}

/// Delete `branch` of the repository `repo` is in if git finds it merged
/// (`branch -d`, never `-D`). Fails with git's reason.
pub fn delete_branch(repo: &str, branch: &str) -> Result<(), String> {
    run_mutation(repo, &["branch", "-d", "--", branch])
}

/// Whether the working tree `dir` has uncommitted changes (untracked
/// files count). If git cannot tell, it counts as dirty: asking is safer.
pub fn is_dirty(dir: &str) -> bool {
    git(dir, &["status", "--porcelain"]).is_none_or(|out| !out.trim().is_empty())
}

/// Why git failed, from what it said on stderr, on one line: its `fatal:`
/// and `error:` lines (progress such as `Preparing worktree` and `hint:`
/// lines left out), else whatever else it said.
fn reason(said: &str) -> Option<String> {
    let lines: Vec<&str> = said
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let errors: Vec<&str> = lines
        .iter()
        .filter_map(|l| {
            l.strip_prefix("fatal: ")
                .or_else(|| l.strip_prefix("error: "))
        })
        .collect();
    let kept = if errors.is_empty() {
        lines
            .into_iter()
            .filter(|l| !l.starts_with("hint:"))
            .collect()
    } else {
        errors
    };
    Some(kept.join("; ")).filter(|r| !r.is_empty())
}

/// `branch`'s status, read in `dir`.
pub fn status(dir: &str, branch: &str) -> Status {
    let range = format!("refs/heads/{branch}...{branch}@{{upstream}}");
    let ahead_behind =
        git(dir, &["rev-list", "--left-right", "--count", &range]).and_then(|o| parse_counts(&o));
    let changes = (current_branch(dir).as_deref() == Some(branch))
        .then(|| changes(dir))
        .flatten();
    Status {
        ahead_behind,
        changes,
    }
}

/// What a branch changed since it left its base.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BaseDiff {
    /// Lines added and removed.
    pub added: u32,
    pub removed: u32,
    /// Files changed.
    pub files: u32,
}

/// What the dashboard shows of a task branch beyond its [`Status`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Details {
    /// What the branch changed since it left its base; `None` without a base.
    pub base_diff: Option<BaseDiff>,
    /// When the branch's last commit was made (seconds since the epoch).
    pub last_commit: Option<u64>,
    /// The branch's work is in its base (merged, rebased or squashed), and
    /// it is not just the base's tip (as a branch with no commits of its
    /// own is).
    pub merged: bool,
}

/// `branch`'s details, read in `dir`.
pub fn details(dir: &str, branch: &str) -> Details {
    let tip = format!("refs/heads/{branch}");
    let Some(rev) = git(dir, &["rev-parse", "--verify", "--quiet", &tip]) else {
        return Details::default();
    };
    let last_commit =
        git(dir, &["log", "-1", "--format=%ct", &tip]).and_then(|o| o.trim().parse().ok());
    let Some(base) = base_ref(dir) else {
        return Details {
            last_commit,
            ..Details::default()
        };
    };
    let range = format!("{base}...{tip}");
    let base_diff =
        git(dir, &["diff", "--no-color", "--numstat", &range]).map(|o| parse_base_diff(&o));
    // A branch at the base's tip may be merged, or new with no commits of
    // its own: there is no telling which, so it counts as not merged.
    let at_base = git(dir, &["rev-parse", "--verify", "--quiet", &base])
        .is_some_and(|b| b.trim() == rev.trim());
    Details {
        base_diff,
        last_commit,
        merged: !at_base && merged(dir, &base, &tip),
    }
}

/// Whether the work of `rev` (a branch's `refs/heads/…`, or `HEAD`, even
/// detached) is in its base: merged, rebased or squashed as [`details`]
/// tells, or at the base's tip with nothing of its own. Losing `rev`
/// loses nothing then. Without a base, it is not.
pub fn in_base(dir: &str, rev: &str) -> bool {
    base_ref(dir).is_some_and(|base| merged(dir, &base, rev))
}

/// Whether branch `tip`'s work is in `base`: merged (`tip` is an ancestor),
/// rebased (each of its commits has an equivalent in `base`), or squashed
/// (its whole change, as one commit, has).
fn merged(dir: &str, base: &str, tip: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", tip, base]).is_some()
        || all_in(dir, base, tip)
        || squashed(dir, base, tip).is_some_and(|squash| all_in(dir, base, &squash))
}

/// Whether every commit of `head` not in `base` has a patch-equivalent
/// commit in `base` (`git cherry` marks those `-`).
fn all_in(dir: &str, base: &str, head: &str) -> bool {
    git(dir, &["cherry", base, head]).is_some_and(|out| {
        let mut lines = out.lines().filter(|l| !l.is_empty()).peekable();
        lines.peek().is_some() && lines.all(|l| l.starts_with('-'))
    })
}

/// A commit holding all of `tip`'s change since it left `base`, as a
/// squash merge would. Writes one unreferenced commit object; its fixed
/// author, date and message make it the same object on every poll, and it
/// is never signed (no prompt on a poll).
fn squashed(dir: &str, base: &str, tip: &str) -> Option<String> {
    let fork = git(dir, &["merge-base", base, tip])?;
    let tree = format!("{tip}^{{tree}}");
    let args = [
        "commit-tree",
        "--no-gpg-sign",
        &tree,
        "-p",
        fork.trim(),
        "-m",
        "paddock: squash check",
    ];
    git_with(dir, &args, SQUASH_IDENTITY).map(|c| c.trim().to_string())
}

/// `diff --numstat` output as lines added, removed, and files changed.
fn parse_base_diff(out: &str) -> BaseDiff {
    let (added, removed) = parse_numstat(out);
    let files = out.lines().filter(|l| !l.is_empty()).count();
    BaseDiff {
        added,
        removed,
        files: u32::try_from(files).unwrap_or(u32::MAX),
    }
}

/// Seconds in a minute, an hour and a day, for ages.
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// A short age: `now`, `5m`, `2h`, `3d`.
pub fn age(secs: u64) -> String {
    match secs {
        0..MINUTE => "now".to_string(),
        MINUTE..HOUR => format!("{}m", secs / MINUTE),
        HOUR..DAY => format!("{}h", secs / HOUR),
        _ => format!("{}d", secs / DAY),
    }
}

/// Uncommitted lines added and removed in `dir`'s working tree.
fn changes(dir: &str) -> Option<(u32, u32)> {
    let (mut added, removed) =
        parse_numstat(&git(dir, &["diff", "--no-color", "--numstat", "HEAD"])?);
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    let untracked = git(
        dir,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--full-name",
            ":/",
        ],
    )?;
    for file in untracked.split('\0').filter(|f| !f.is_empty()) {
        let path = Path::new(top.trim()).join(file);
        // Status is polled: never read a big untracked file (data, logs), nor
        // follow a link (to a FIFO it would block, to /dev/zero never end).
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_UNTRACKED)
        {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            added += count_lines(&bytes);
        }
    }
    Some((added, removed))
}

/// Lines in a new file; a binary file (holding a NUL byte) has none.
fn count_lines(bytes: &[u8]) -> u32 {
    if bytes.contains(&0) {
        return 0;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    let last = usize::from(bytes.last().is_some_and(|&b| b != b'\n'));
    u32::try_from(newlines + last).unwrap_or(u32::MAX)
}

/// `rev-list --left-right --count A...B` output: `left\tright`.
fn parse_counts(out: &str) -> Option<(u32, u32)> {
    let mut w = out.split_whitespace();
    Some((w.next()?.parse().ok()?, w.next()?.parse().ok()?))
}

/// Total added and removed lines of `diff --numstat`; binary files (`-`) count as none.
fn parse_numstat(out: &str) -> (u32, u32) {
    out.lines().fold((0, 0), |(a, r), l| {
        let mut w = l.split('\t');
        let mut n = || w.next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0);
        let (da, dr) = (n(), n());
        (a.saturating_add(da), r.saturating_add(dr))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Run git for a test, ignoring the user's own git configuration.
    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A repository on `main` with one commit of a three-line file.
    fn repo(root: &Path) -> PathBuf {
        let dir = root.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("f.txt"), "1\n2\n3\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "one"]);
        dir
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), "x\n").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-q", "-m", file]);
    }

    fn s(p: &Path) -> &str {
        p.to_str().unwrap()
    }

    fn diff(added: u32, removed: u32, files: u32) -> BaseDiff {
        BaseDiff {
            added,
            removed,
            files,
        }
    }

    #[test]
    fn parses_counts_and_numstat() {
        assert_eq!(parse_counts("3\t1\n"), Some((3, 1)));
        assert_eq!(parse_counts("oops"), None);
        assert_eq!(
            parse_numstat("2\t1\ta.txt\n-\t-\tlogo.png\n10\t0\tb.rs\n"),
            (12, 1)
        );
        assert_eq!(parse_numstat(""), (0, 0));
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"\0bin\n"), 0);
    }

    #[test]
    fn the_default_branch_is_no_task() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        assert_eq!(current_branch(s(&dir)).as_deref(), Some("main"));
        assert_eq!(task_branch(s(&dir)), None);
        run(&dir, &["switch", "-q", "-c", "feat/x"]);
        assert_eq!(task_branch(s(&dir)).as_deref(), Some("feat/x"));
        assert_eq!(task_branch(s(&tmp.path().join("nope"))), None);
        assert_eq!(task_branch(s(tmp.path())), None, "not a repository");
    }

    #[test]
    fn head_names_the_branch_or_the_detached_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        assert_eq!(head(s(&dir)).as_deref(), Some("main"));
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(head(s(&sub)).as_deref(), Some("main"), "inside the repo");
        run(&dir, &["switch", "-q", "--detach"]);
        let sha = git(s(&dir), &["rev-parse", "--short", "HEAD"]).unwrap();
        assert_eq!(head(s(&dir)), Some(sha.trim().to_string()));
        assert_eq!(head(s(tmp.path())), None, "not a repository");
        assert_eq!(head(s(&tmp.path().join("nope"))), None);
    }

    #[test]
    fn origin_head_names_the_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = repo(tmp.path());
        run(&origin, &["branch", "-q", "trunk"]);
        run(&origin, &["switch", "-q", "trunk"]);
        let clone = tmp.path().join("clone");
        run(tmp.path(), &["clone", "-q", s(&origin), s(&clone)]);
        assert_eq!(default_branch(s(&clone)).as_deref(), Some("trunk"));
        assert_eq!(task_branch(s(&clone)), None);
        run(&clone, &["switch", "-q", "main"]);
        assert_eq!(task_branch(s(&clone)).as_deref(), Some("main"));
    }

    #[test]
    fn status_counts_ahead_behind_and_uncommitted_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        run(&dir, &["switch", "-q", "-c", "feat/x", "--track", "main"]);
        commit(&dir, "a");
        commit(&dir, "b");
        run(&dir, &["switch", "-q", "main"]);
        commit(&dir, "c");
        run(&dir, &["switch", "-q", "feat/x"]);
        std::fs::write(dir.join("f.txt"), "1\nchanged\n3\nnew\n").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/new.txt"), "x\ny\nz").unwrap();
        std::fs::write(dir.join("logo.png"), b"\0\x01").unwrap();
        std::fs::write(dir.join("big.log"), "x\n".repeat(600_000)).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored\n").unwrap();
        std::fs::write(dir.join("ignored"), "lots\nof\nlines\n").unwrap();
        let want = Status {
            ahead_behind: Some((2, 1)),
            // f.txt +2 -1, sub/new.txt +3, .gitignore +1
            changes: Some((6, 1)),
        };
        assert_eq!(status(s(&dir), "feat/x"), want);
        assert_eq!(
            status(s(&dir.join("sub")), "feat/x"),
            want,
            "from a subdirectory"
        );
        // A branch that is not checked out here has no line changes.
        assert_eq!(status(s(&dir), "main"), Status::default());
    }

    #[test]
    fn parses_base_numstat_and_ages() {
        assert_eq!(
            parse_base_diff("2\t1\ta.txt\n-\t-\tlogo.png\n10\t0\tb.rs\n"),
            diff(12, 1, 3)
        );
        assert_eq!(parse_base_diff(""), diff(0, 0, 0));
        assert_eq!(age(0), "now");
        assert_eq!(age(59), "now");
        assert_eq!(age(60), "1m");
        assert_eq!(age(3599), "59m");
        assert_eq!(age(3600), "1h");
        assert_eq!(age(86_399), "23h");
        assert_eq!(age(86_400 * 3), "3d");
    }

    #[test]
    fn details_compare_a_branch_with_main() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        run(&dir, &["switch", "-q", "-c", "feat/x"]);
        let fresh = details(s(&dir), "feat/x");
        assert_eq!(fresh.base_diff, Some(diff(0, 0, 0)));
        assert!(!fresh.merged, "a branch with no commits of its own");
        assert!(in_base(s(&dir), "refs/heads/feat/x"), "nothing of its own");
        assert!(in_base(s(&dir), "HEAD"));
        assert!(!in_base(s(&dir), "refs/heads/nope"));
        std::fs::write(dir.join("f.txt"), "1\nchanged\n3\nnew\n").unwrap();
        commit(&dir, "a");
        // Uncommitted work is not part of the base diff.
        std::fs::write(dir.join("loose.txt"), "x\n").unwrap();
        let d = details(s(&dir), "feat/x");
        // f.txt +2 -1, a +1
        assert_eq!(d.base_diff, Some(diff(3, 1, 2)));
        assert!(d.last_commit.is_some_and(|t| t > 0));
        assert!(!d.merged);
        assert!(!in_base(s(&dir), "refs/heads/feat/x"));
        // A detached HEAD's commits count the same.
        run(&dir, &["switch", "-q", "--detach"]);
        assert!(!in_base(s(&dir), "HEAD"));
        assert_eq!(details(s(&dir), "nope"), Details::default());
    }

    #[test]
    fn a_branch_merged_into_its_base_is_merged() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        run(&dir, &["switch", "-q", "-c", "feat/x"]);
        commit(&dir, "a");
        run(&dir, &["switch", "-q", "main"]);
        commit(&dir, "b");
        run(&dir, &["merge", "-q", "--no-edit", "feat/x"]);
        let d = details(s(&dir), "feat/x");
        assert!(d.merged);
        assert_eq!(d.base_diff, Some(diff(0, 0, 0)));
    }

    /// A repository whose `feat/x` has two commits of its own, and whose
    /// `main` has moved on since; `main` is checked out.
    fn diverged(root: &Path) -> PathBuf {
        let dir = repo(root);
        run(&dir, &["switch", "-q", "-c", "feat/x"]);
        commit(&dir, "a");
        commit(&dir, "b");
        run(&dir, &["switch", "-q", "main"]);
        commit(&dir, "c");
        dir
    }

    #[test]
    fn a_branch_rebase_merged_into_its_base_is_merged() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = diverged(tmp.path());
        run(&dir, &["cherry-pick", "main..feat/x~1"]);
        assert!(!details(s(&dir), "feat/x").merged, "one commit of two");
        run(&dir, &["cherry-pick", "feat/x"]);
        assert!(details(s(&dir), "feat/x").merged);
    }

    #[test]
    fn a_branch_squash_merged_into_its_base_is_merged() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = diverged(tmp.path());
        run(&dir, &["merge", "-q", "--squash", "feat/x"]);
        run(&dir, &["commit", "-q", "-m", "squashed"]);
        assert!(details(s(&dir), "feat/x").merged);
        // More work on the branch is not in its base.
        run(&dir, &["switch", "-q", "feat/x"]);
        commit(&dir, "d");
        assert!(!details(s(&dir), "feat/x").merged);
    }

    #[test]
    fn the_base_is_what_origin_head_names_on_the_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = repo(tmp.path());
        run(&origin, &["switch", "-q", "-c", "trunk"]);
        let clone = tmp.path().join("clone");
        run(tmp.path(), &["clone", "-q", s(&origin), s(&clone)]);
        // The local trunk moves on; the remote's does not.
        commit(&clone, "local");
        run(&clone, &["switch", "-q", "-c", "feat/x"]);
        commit(&clone, "a");
        let d = details(s(&clone), "feat/x");
        assert_eq!(d.base_diff, Some(diff(2, 0, 2)), "vs origin/trunk");
    }

    #[test]
    fn a_read_that_takes_too_long_is_killed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = s(tmp.path());
        let start = Instant::now();
        // An alias that sleeps: git runs it through a shell, as the user's
        // own aliases and hooks would.
        let err = super::run(
            dir,
            &["-c", "alias.nap=!sleep 5", "nap"],
            &[],
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(err.contains("did not finish"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(3));
        // git exits at once, but leaves a process holding its pipes: the
        // deadline still holds.
        let start = Instant::now();
        let err = super::run(
            dir,
            &["-c", "alias.bg=!(sleep 3 &)", "bg"],
            &[],
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(err.contains("did not finish in 200ms"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(2));
        // A mutation reads no stdout, and a successful one no stderr: it
        // is done as soon as git is.
        let start = Instant::now();
        assert_eq!(
            run_mutation(dir, &["-c", "alias.bg=!(sleep 3 &)", "bg"]),
            Ok(())
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        // One in time gives its output.
        let big = super::run(dir, &["version"], &[], READ_TIMEOUT).unwrap();
        assert!(big.starts_with("git version"));
    }

    #[test]
    fn a_late_reason_is_told_apart_by_its_start() {
        let d = Duration::from_secs(5);
        assert!(is_late(&late_reason("git", d)));
        assert_eq!(late_reason("gh", d), "gh did not finish in 5s");
        assert!(!is_late(&late_reason("gh", d)), "git's only");
        assert!(!is_late("fatal: git did not finish"));
    }

    #[test]
    fn git_s_reason_is_its_error_lines_on_one_line() {
        let said =
            "Preparing worktree (new branch 'x')\nfatal: a branch named 'x' already exists\n";
        assert_eq!(
            reason(said).as_deref(),
            Some("a branch named 'x' already exists")
        );
        let said = "error: The branch 'y' is not fully merged.\nhint: use -D\n";
        assert_eq!(
            reason(said).as_deref(),
            Some("The branch 'y' is not fully merged.")
        );
        assert_eq!(reason("odd\nhint: x\n").as_deref(), Some("odd"));
        assert_eq!(reason("\n"), None);
    }

    #[test]
    fn parses_worktree_list() {
        let out = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\n\
                   worktree /w/feat-x\nHEAD def\nbranch refs/heads/feat/x\nlocked\n\n\
                   worktree /w/loose\nHEAD 123\ndetached\n\n";
        let wt = |path: &str, branch: Option<&str>| Worktree {
            path: path.into(),
            branch: branch.map(str::to_string),
        };
        assert_eq!(
            parse_worktrees(out),
            [
                wt("/r", Some("main")),
                wt("/w/feat-x", Some("feat/x")),
                wt("/w/loose", None)
            ]
        );
        assert_eq!(parse_worktrees(""), []);
    }

    #[test]
    fn worktree_paths_go_under_the_repo_with_slashes_as_dashes() {
        assert_eq!(
            worktree_path(Path::new("/w"), "api", "feat/x/y"),
            PathBuf::from("/w/api/feat-x-y")
        );
        let tmp = tempfile::tempdir().unwrap();
        let dir = s(tmp.path());
        assert!(valid_branch(dir, "feat/x"));
        for bad in [
            "", "-x", "a..b", "a b", "x/", "HEAD", "a\x7fb", "@{-1}", "x@{u}",
        ] {
            assert!(!valid_branch(dir, bad), "{bad:?}");
        }
        assert_eq!(dir_name("/w/api"), "api");
        assert_eq!(dir_name("/"), "/");
    }

    #[test]
    fn worktrees_are_added_listed_told_apart_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        let main = resolved(s(&dir));
        assert_eq!(worktrees(s(&dir)).len(), 1);
        let sub = dir.join("sub").join("deeper");
        std::fs::create_dir_all(&sub).unwrap();
        let home = working_tree(s(&sub)).unwrap();
        assert_eq!(
            working_tree(s(&dir)),
            Some(WorkingTree {
                top: main.clone(),
                main: main.clone()
            })
        );
        assert!(!home.is_linked() && home.repo() == "repo");
        assert_eq!(working_tree(s(tmp.path())), None, "not a repository");

        let new = tmp.path().join("wt").join("repo").join("feat-x");
        assert_eq!(add_worktree(s(&dir), &new, "feat/x"), Ok(true), "created");
        assert_eq!(current_branch(s(&new)).as_deref(), Some("feat/x"));
        let linked = working_tree(s(&new)).unwrap();
        assert!(linked.is_linked());
        assert_eq!(linked.top, resolved(s(&new)));
        assert_eq!(linked.main, main);
        let listed = worktrees(s(&new));
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].path, main);
        assert_eq!(listed[1].branch.as_deref(), Some("feat/x"));

        // An existing branch is checked out as it is.
        run(&dir, &["branch", "-q", "feat/y"]);
        let other = tmp.path().join("wt").join("repo").join("feat-y");
        assert_eq!(add_worktree(s(&dir), &other, "feat/y"), Ok(false));
        // A branch checked out elsewhere, or a taken path: git's reason.
        let err = add_worktree(s(&dir), &tmp.path().join("again"), "feat/x").unwrap_err();
        assert!(
            err.contains("feat/x") && err.contains("(git worktree add"),
            "{err}"
        );
        let err = add_worktree(s(&dir), &other, "feat/z").unwrap_err();
        assert!(err.contains("already exists"), "{err}");

        assert!(!is_dirty(s(&new)));
        assert!(
            is_dirty(s(&tmp.path().join("gone"))),
            "unknown counts as dirty"
        );
        std::fs::write(new.join("loose.txt"), "x\n").unwrap();
        assert!(is_dirty(s(&new)));
        let err = remove_worktree(s(&dir), s(&new), false).unwrap_err();
        assert!(err.contains("(git worktree remove"), "{err}");
        assert!(new.is_dir(), "kept");
        assert_eq!(remove_worktree(s(&dir), s(&new), true), Ok(()));
        assert!(!new.exists());
        assert_eq!(delete_branch(s(&dir), "feat/x"), Ok(()));
        assert!(!has_branch(s(&dir), "feat/x"));
        // An unmerged branch stays (`-d`, not `-D`).
        commit(&other, "a");
        assert_eq!(remove_worktree(s(&dir), s(&other), false), Ok(()));
        let err = delete_branch(s(&dir), "feat/y").unwrap_err();
        assert!(err.contains("(git branch -d"), "{err}");
        assert!(!other.exists() && has_branch(s(&dir), "feat/y"));
        assert_eq!(worktrees(s(&dir)).len(), 1);
    }

    #[test]
    fn untracked_links_to_special_files_are_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(tmp.path());
        run(&dir, &["switch", "-q", "-c", "feat/x"]);
        let fifo = tmp.path().join("fifo");
        let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(made.success());
        std::os::unix::fs::symlink(&fifo, dir.join("pipe")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", dir.join("zero")).unwrap();
        let dir = s(&dir).to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(status(&dir, "feat/x")));
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reading git status hung on a special file");
        assert_eq!(got.changes, Some((0, 0)));
    }
}
