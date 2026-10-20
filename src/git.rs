//! What paddock reads from git: the branch a directory has checked out, and a
//! branch's status (ahead/behind its upstream, uncommitted line changes).
//! paddock only reads; it never changes a repository.

use std::path::Path;
use std::process::{Command, Stdio};

/// Branches taken as the default when `origin/HEAD` names none.
const DEFAULTS: &[&str] = &["main", "master"];
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
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
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
    let origin = git(
        dir,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    );
    if let Some(b) = origin
        .as_deref()
        .and_then(|o| o.trim().strip_prefix("origin/"))
    {
        return Some(b.to_string());
    }
    DEFAULTS
        .iter()
        .find(|b| {
            git(
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{b}"),
                ],
            )
            .is_some()
        })
        .map(|b| b.to_string())
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
