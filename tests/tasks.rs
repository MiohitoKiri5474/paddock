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

/// A sidebar line as drawn and trimmed: `left`, cut to fit, then `right` at
/// the end.
fn line(indent: usize, left: &str, right: &str) -> String {
    let room = SIDEBAR - indent - 1 - right.chars().count();
    let left: String = left.chars().take(room - 1).collect();
    let gap = room - left.chars().count();
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

// Ticket 09: subagent task rows, from spike 02's recorded subagent payloads.

const AGENT_A: &str = "a02c2bd27020e23b4";
const AGENT_B: &str = "a9700dfeed5ef352c";
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
const FIX_A: &str = "Create fix/a branch and a.txt";
const FIX_B: &str = "Create fix/b branch and b.txt";

/// A recorded payload for the test repository `repo` and the sandbox's home.
/// The recorded subagent is agent A; for agent B the two agents' ids and
/// descriptions swap places.
fn recorded(sandbox: &Sandbox, name: &str, repo: &Path, agent: &str) -> String {
    let path = format!("{SPIKE}/{name}.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let swap =
        |text: String, a: &str, b: &str| text.replace(a, "\u{0}").replace(b, a).replace('\u{0}', b);
    let text = if agent == AGENT_B {
        swap(swap(text, AGENT_A, AGENT_B), FIX_A, FIX_B)
    } else {
        text
    };
    let home = sandbox.path("home");
    text.replace("\"/tmp/repo", &format!("\"{}", repo.display()))
        .replace("\"/home/user", &format!("\"{}", home.display()))
}

/// The worktree Claude makes for a subagent, on its first branch.
fn worktree(repo: &Path, agent: &str) -> PathBuf {
    let dir = repo.join(format!(".claude/worktrees/agent-{agent}"));
    let branch = format!("worktree-agent-{agent}");
    git(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            dir.to_str().unwrap(),
        ],
    );
    dir
}

/// Agent A's `.meta.json`, as Claude writes it next to the transcript;
/// agent B has none, so its description only arrives with its SubagentStop.
fn write_meta(sandbox: &Sandbox) {
    let dir = sandbox.path(&format!(
        "home/.claude/projects/-tmp-repo/{SESSION}/subagents"
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("agent-{AGENT_A}.meta.json")),
        format!(r#"{{"agentType":"general-purpose","description":"{FIX_A}","finished":false}}"#),
    )
    .unwrap();
}

fn sub(label: &str, state: &str) -> String {
    line(2, label, state)
}

#[test]
fn parallel_subagents_show_as_task_rows_from_running_to_done() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    write_meta(&sandbox);
    let (wt_a, wt_b) = (worktree(&dir, AGENT_A), worktree(&dir, AGENT_B));
    let paddock = ready(&sandbox);
    let hook = |name: &str, event: &str, agent: &str| {
        sandbox.hook("terminal-1", event, &recorded(&sandbox, name, &dir, agent));
    };

    hook("SessionStart", "SessionStart", AGENT_A);
    hook("UserPromptSubmit", "UserPromptSubmit", AGENT_A);
    hook("SubagentStart-subagent", "SubagentStart", AGENT_A);
    hook("SubagentStart-subagent", "SubagentStart", AGENT_B);
    let first_b = format!("worktree-agent-{AGENT_B}");
    wait_for_sidebar(
        &paddock,
        &[
            &session("running"),
            &sub(FIX_A, "running"),
            &sub(&first_b, "running"),
        ],
    );

    git(&wt_a, &["switch", "-q", "-c", "fix/a"]);
    hook("PostToolUse-subagent", "PostToolUse", AGENT_A);
    git(&wt_b, &["switch", "-q", "-c", "fix/b"]);
    hook("PostToolUse-subagent", "PostToolUse", AGENT_B);
    // The main agent stops while its subagents still run.
    hook("Stop", "Stop", AGENT_A);
    wait_for_sidebar(
        &paddock,
        &[
            &session("idle"),
            &sub(FIX_A, "running"),
            &sub("fix/b", "running"),
        ],
    );

    hook("SubagentStop-subagent", "SubagentStop", AGENT_A);
    hook("SubagentStop-subagent", "SubagentStop", AGENT_B);
    wait_for_sidebar(
        &paddock,
        &[&session("idle"), &sub(FIX_A, "done"), &sub(FIX_B, "done")],
    );
}

#[test]
fn subagent_work_done_while_paddock_is_closed_appears_on_reopen() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    write_meta(&sandbox);
    let (wt_a, wt_b) = (worktree(&dir, AGENT_A), worktree(&dir, AGENT_B));
    let mut paddock = ready(&sandbox);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let hook = |name: &str, event: &str, agent: &str| {
        sandbox.hook("terminal-1", event, &recorded(&sandbox, name, &dir, agent));
    };
    hook("UserPromptSubmit", "UserPromptSubmit", AGENT_A);
    for (agent, wt, branch) in [(AGENT_A, &wt_a, "fix/a"), (AGENT_B, &wt_b, "fix/b")] {
        hook("SubagentStart-subagent", "SubagentStart", agent);
        git(wt, &["switch", "-q", "-c", branch]);
        hook("PostToolUse-subagent", "PostToolUse", agent);
    }
    hook("SubagentStop-subagent", "SubagentStop", AGENT_A);
    // The session ends while agent B still runs.
    hook("SessionEnd", "SessionEnd", AGENT_A);

    let again = ready(&sandbox);
    wait_for_sidebar(
        &again,
        &[
            &session("stopped"),
            &sub(FIX_A, "done"),
            &sub("fix/b", "stopped"),
        ],
    );
}
