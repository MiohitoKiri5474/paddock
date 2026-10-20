//! What paddock reads from git: the branch a directory has checked out, and a
//! branch's status (ahead/behind its upstream, uncommitted line changes).
//! paddock only reads; it never changes a branch, the index or a working
//! tree. Telling a squash merge writes one unreferenced commit object.

use std::path::Path;
use std::process::{Command, Stdio};

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

/// A task branch's git fields, as the sidebar shows them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    /// Commits ahead of and behind the upstream; `None` without one.
    pub ahead_behind: Option<(u32, u32)>,
    /// Uncommitted lines added and removed (untracked files count as added);
    /// `None` unless the branch is checked out in the directory asked about.
    pub changes: Option<(u32, u32)>,
}

/// `git -C dir args…`'s output, or `None` if it failed. Never prints: hooks
/// must stay silent.
fn git(dir: &str, args: &[&str]) -> Option<String> {
    git_with(dir, args, &[])
}

/// [`git`] with environment variables `env` set.
fn git_with(dir: &str, args: &[&str], env: &[(&str, &str)]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        // Polling must never take the index lock from Claude's own git.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The branch checked out in `dir`; `None` if detached or not a repository.
pub fn current_branch(dir: &str) -> Option<String> {
    let out = git(dir, &["branch", "--show-current"])?;
    Some(out.trim().to_string()).filter(|b| !b.is_empty())
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
        std::fs::write(dir.join("f.txt"), "1\nchanged\n3\nnew\n").unwrap();
        commit(&dir, "a");
        // Uncommitted work is not part of the base diff.
        std::fs::write(dir.join("loose.txt"), "x\n").unwrap();
        let d = details(s(&dir), "feat/x");
        // f.txt +2 -1, a +1
        assert_eq!(d.base_diff, Some(diff(3, 1, 2)));
        assert!(d.last_commit.is_some_and(|t| t > 0));
        assert!(!d.merged);
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
