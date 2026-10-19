//! Ticket 08: main-agent task rows, against real temporary git repositories.
//!
//! Hook payloads come from spike 02's recorded fixtures, pointed at the test
//! repository; the Bash `PostToolUse` is the recorded one with its tool
//! renamed (the spike's main agent only ran the Agent tool).

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
/// Columns of the sidebar before its border.
const SIDEBAR: usize = 23;

/// A recorded main-agent payload for `cwd`; `tool` renames its tool.
fn payload(name: &str, cwd: &Path, tool: Option<&str>) -> String {
    let path = format!("{SPIKE}/{name}.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let text = text.replace("\"/tmp/repo\"", &format!("\"{}\"", cwd.display()));
    match tool {
        Some(t) => text.replace(
            "\"tool_name\": \"Agent\"",
            &format!("\"tool_name\": \"{t}\""),
        ),
        None => text,
    }
}

/// Run git in `dir`, ignoring the user's own git configuration.
fn git(dir: &Path, args: &[&str]) {
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
fn repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("repo");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "1\n2\n3\n").unwrap();
    commit(&dir, "one");
    dir
}

fn commit(dir: &Path, message: &str) {
    std::fs::write(dir.join(message), "x\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Sidebar lines (below its title), trimmed.
fn sidebar(paddock: &Paddock) -> Vec<String> {
    paddock
        .rows()
        .into_iter()
        .skip(1)
        .map(|r| {
            r.chars()
                .take(SIDEBAR)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|r| !r.is_empty())
        .collect()
}

fn wait_for_sidebar(paddock: &Paddock, want: &[&str]) {
    let start = Instant::now();
    while start.elapsed() < harness::TIMEOUT {
        if sidebar(paddock) == want {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "sidebar is {:?}, not {want:?}; screen:\n{}",
        sidebar(paddock),
        paddock.screen()
    );
}

/// A sidebar line as drawn and trimmed: `left`, then `right` at the end.
fn line(indent: usize, left: &str, right: &str) -> String {
    let gap = SIDEBAR - indent - 1 - left.chars().count() - right.chars().count();
    format!("{left}{}{right}", " ".repeat(gap))
}

fn session(state: &str) -> String {
    line(1, "repo", state)
}

fn task(branch: &str, fields: &str) -> String {
    line(2, branch, fields)
}

#[test]
fn switching_branches_adds_a_task_row_for_each_branch() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let paddock = ready(&sandbox);
    let bash = |sandbox: &Sandbox| {
        sandbox.hook(
            "terminal-1",
            "PostToolUse",
            &payload("PostToolUse", &dir, Some("Bash")),
        );
    };

    sandbox.hook(
        "terminal-1",
        "SessionStart",
        &payload("SessionStart", &dir, None),
    );
    wait_for_sidebar(&paddock, &[&session("idle")]);

    git(&dir, &["switch", "-q", "-c", "feat/one"]);
    bash(&sandbox);
    wait_for_sidebar(&paddock, &[&session("running"), "feat/one"]);

    git(&dir, &["switch", "-q", "-c", "feat/two"]);
    bash(&sandbox);
    wait_for_sidebar(&paddock, &[&session("running"), "feat/one", "feat/two"]);

    // Only a shell command can switch branch; other tools do not re-read it.
    git(&dir, &["switch", "-q", "-c", "feat/three"]);
    sandbox.hook(
        "terminal-1",
        "PostToolUse",
        &payload("PostToolUse", &dir, Some("Edit")),
    );
    sandbox.hook("terminal-1", "Stop", &payload("Stop", &dir, None));
    wait_for_sidebar(&paddock, &[&session("idle"), "feat/one", "feat/two"]);
}

#[test]
fn task_rows_show_ahead_behind_and_line_changes() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    git(&dir, &["switch", "-q", "-c", "feat/x", "--track", "main"]);
    commit(&dir, "a");
    commit(&dir, "b");
    git(&dir, &["switch", "-q", "main"]);
    commit(&dir, "c");
    git(&dir, &["switch", "-q", "feat/x"]);
    std::fs::write(dir.join("f.txt"), "1\nchanged\n3\nnew\n").unwrap();
    let paddock = ready(&sandbox);

    sandbox.hook(
        "terminal-1",
        "UserPromptSubmit",
        &payload("UserPromptSubmit", &dir, None),
    );
    wait_for_sidebar(
        &paddock,
        &[&session("running"), &task("feat/x", "↑2↓1 +2 −1")],
    );

    // No hook: the periodic refresh of rows on screen picks this up.
    std::fs::write(dir.join("new.txt"), "a\nb\nc\n").unwrap();
    wait_for_sidebar(
        &paddock,
        &[&session("running"), &task("feat/x", "↑2↓1 +5 −1")],
    );
}

#[test]
fn no_rows_for_the_default_branch_or_shells_without_claude() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    // A plain shell, in the repository on a task branch, never gets a row.
    let shell_dir = sandbox.path("shell");
    git(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/shell",
            shell_dir.to_str().unwrap(),
        ],
    );
    paddock.type_line(&format!("cd {}", shell_dir.display()));
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:terminal-2");

    for name in ["SessionStart", "UserPromptSubmit"] {
        sandbox.hook("terminal-2", name, &payload(name, &dir, None));
    }
    sandbox.hook(
        "terminal-2",
        "PostToolUse",
        &payload("PostToolUse", &dir, Some("Bash")),
    );
    wait_for_sidebar(&paddock, &[&session("running")]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(sidebar(&paddock), [session("running")]);
}
