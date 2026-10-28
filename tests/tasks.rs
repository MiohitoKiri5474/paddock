//! Tickets 08-10: session and task rows (main agents and subagents) with
//! their git fields, against real temporary git repositories. The sidebar
//! shows workspaces (ADR 0006), so these rows are read from the dashboard's
//! list (`C-a o`).
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
/// The start of the dashboard's key line, as drawn while it is open.
const OPEN: &str = "j down  k up";
/// The dashboard's title, on its first line, right under the tab bar.
const TITLE: &str = "dashboard";
/// The selected session block's background: the default
/// `[colors] current_space`.
const SELECTED: vt100::Color = vt100::Color::Idx(236);

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

/// The dashboard's list lines below its column header, spaces collapsed;
/// empty while it is closed.
fn list(paddock: &Paddock) -> Vec<String> {
    let rows = paddock.rows();
    if rows.get(1).is_none_or(|r| r.trim() != TITLE) {
        return Vec::new();
    }
    rows.iter()
        // The tab bar, the title, the column header and a blank line.
        .skip(4)
        // Up to the preview's border.
        .take_while(|r| !r.starts_with('╭'))
        // Blank lines part the sessions, and fill the list's minimum height.
        .filter(|r| !r.is_empty() && !r.contains("no agent sessions yet"))
        .map(|r| r.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// Whether list line `line` matches `want`: `prefix`, or `prefix|state`
/// with `state` in its own column.
fn matches(line: &str, want: &str) -> bool {
    let (prefix, state) = want.split_once('|').unwrap_or((want, ""));
    line.starts_with(prefix) && (state.is_empty() || line.split(' ').any(|w| w == state))
}

/// Wait until the dashboard lists lines matching `want` (see [`matches`]),
/// opening it first if needed.
fn wait_for_list(paddock: &mut Paddock, want: &[&str]) {
    let start = Instant::now();
    let mut asked = start.checked_sub(Duration::from_secs(1)).unwrap_or(start);
    while start.elapsed() < harness::TIMEOUT {
        let got = list(paddock);
        if got.len() == want.len() && got.iter().zip(want).all(|(l, w)| matches(l, w)) {
            return;
        }
        // Open it, once in a while: a key may land before it is drawn.
        if !paddock.screen().contains(OPEN) && asked.elapsed() > Duration::from_millis(300) {
            paddock.command(b"o");
            asked = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "list is {:?}, not {want:?}; screen:\n{}",
        list(paddock),
        paddock.full_screen()
    );
}

/// Close the dashboard, so keys reach the terminal again.
fn close_list(paddock: &mut Paddock) {
    if paddock.screen().contains(OPEN) {
        paddock.command(b"o");
        paddock.wait_for("the dashboard closed", |s| !s.contains(OPEN));
    }
}

/// The session row of the test repository.
fn session(state: &str) -> String {
    format!("repo|{state}")
}

/// A main-agent task row: its branch, then its git fields.
fn task(branch: &str, fields: &str) -> String {
    format!("{branch} {fields}")
}

/// Tab count of space-1.
fn tabs(sandbox: &Sandbox) -> usize {
    sandbox.space("space-1").tabs.len()
}

#[test]
fn switching_branches_adds_a_task_row_for_each_branch() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    let bash = |sandbox: &Sandbox| {
        sandbox.hook(
            "space-1",
            "PostToolUse",
            &payload("PostToolUse", &dir, Some("Bash")),
        );
    };

    sandbox.hook(
        "space-1",
        "SessionStart",
        &payload("SessionStart", &dir, None),
    );
    wait_for_list(&mut paddock, &[&session("idle")]);

    git(&dir, &["switch", "-q", "-c", "feat/one"]);
    bash(&sandbox);
    wait_for_list(&mut paddock, &[&session("running"), "feat/one"]);

    git(&dir, &["switch", "-q", "-c", "feat/two"]);
    bash(&sandbox);
    wait_for_list(&mut paddock, &[&session("running"), "feat/one", "feat/two"]);

    // Only a shell command can switch branch; other tools do not re-read it.
    git(&dir, &["switch", "-q", "-c", "feat/three"]);
    sandbox.hook(
        "space-1",
        "PostToolUse",
        &payload("PostToolUse", &dir, Some("Edit")),
    );
    sandbox.hook("space-1", "Stop", &payload("Stop", &dir, None));
    wait_for_list(&mut paddock, &[&session("idle"), "feat/one", "feat/two"]);
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
    let mut paddock = ready(&sandbox);

    sandbox.hook(
        "space-1",
        "UserPromptSubmit",
        &payload("UserPromptSubmit", &dir, None),
    );
    wait_for_list(
        &mut paddock,
        &[&session("running"), &task("feat/x", "↑2↓1 +2 −1")],
    );

    // No hook: the periodic refresh of rows on screen picks this up.
    std::fs::write(dir.join("new.txt"), "a\nb\nc\n").unwrap();
    wait_for_list(
        &mut paddock,
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
    paddock.type_line(&format!(
        "cd {} && echo moved-$((1+1))",
        shell_dir.display()
    ));
    // The new tab starts where the shell is, once it got there.
    paddock.wait_for_text("moved-2\n");
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:shell");

    for name in ["SessionStart", "UserPromptSubmit"] {
        sandbox.hook("space-1:1", name, &payload(name, &dir, None));
    }
    sandbox.hook(
        "space-1:1",
        "PostToolUse",
        &payload("PostToolUse", &dir, Some("Bash")),
    );
    wait_for_list(&mut paddock, &[&session("running")]);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(list(&paddock).len(), 1, "{:?}", list(&paddock));
}

// Ticket 09: subagent task rows, from spike 02's recorded subagent payloads.

const AGENT_A: &str = "a02c2bd27020e23b4";
const AGENT_B: &str = "a9700dfeed5ef352c";
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
/// Another session's id, for the recorded payloads' [`SESSION`].
const OTHER_SESSION: &str = "22222222-2222-4333-8444-555555555555";
/// A third session's id: a new Claude after one quit.
const NEW_CLAUDE: &str = "33333333-2222-4333-8444-555555555555";
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

/// A subagent's task row: its description (else its branch), then its
/// state.
fn sub(label: &str, state: &str) -> String {
    format!("{label}|{state}")
}

#[test]
fn parallel_subagents_show_as_task_rows_from_running_to_done() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    write_meta(&sandbox);
    let (wt_a, wt_b) = (worktree(&dir, AGENT_A), worktree(&dir, AGENT_B));
    let mut paddock = ready(&sandbox);
    let hook = |name: &str, event: &str, agent: &str| {
        sandbox.hook("space-1", event, &recorded(&sandbox, name, &dir, agent));
    };

    hook("SessionStart", "SessionStart", AGENT_A);
    hook("UserPromptSubmit", "UserPromptSubmit", AGENT_A);
    hook("SubagentStart-subagent", "SubagentStart", AGENT_A);
    hook("SubagentStart-subagent", "SubagentStart", AGENT_B);
    // Its branch is longer than the name column: only its start shows.
    let first_b = format!("worktree-agent-{}", &AGENT_B[..8]);
    wait_for_list(
        &mut paddock,
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
    wait_for_list(
        &mut paddock,
        &[
            &session("idle"),
            &sub(FIX_A, "running"),
            &sub("fix/b", "running"),
        ],
    );

    hook("SubagentStop-subagent", "SubagentStop", AGENT_A);
    hook("SubagentStop-subagent", "SubagentStop", AGENT_B);
    wait_for_list(
        &mut paddock,
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
        sandbox.hook("space-1", event, &recorded(&sandbox, name, &dir, agent));
    };
    hook("UserPromptSubmit", "UserPromptSubmit", AGENT_A);
    for (agent, wt, branch) in [(AGENT_A, &wt_a, "fix/a"), (AGENT_B, &wt_b, "fix/b")] {
        hook("SubagentStart-subagent", "SubagentStart", agent);
        git(wt, &["switch", "-q", "-c", branch]);
        hook("PostToolUse-subagent", "PostToolUse", agent);
    }
    hook("SubagentStop-subagent", "SubagentStop", AGENT_A);

    // In tree order, not by state: the finished one stays first.
    let mut again = ready(&sandbox);
    wait_for_list(
        &mut again,
        &[
            &session("running"),
            &sub(FIX_A, "done"),
            &sub("fix/b", "running"),
        ],
    );
}

#[test]
fn a_session_that_ends_while_paddock_is_closed_is_gone_with_its_tasks_on_reopen() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    write_meta(&sandbox);
    let (wt_a, wt_b) = (worktree(&dir, AGENT_A), worktree(&dir, AGENT_B));
    let mut paddock = ready(&sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let hook = |name: &str, event: &str, agent: &str| {
        sandbox.hook("space-1:0", event, &recorded(&sandbox, name, &dir, agent));
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
    witness(&sandbox, "space-1:1", &dir);

    let mut again = ready(&sandbox);
    wait_for_list(&mut again, &[&session("idle")]);
}

/// Another session, in the terminal `terminal` names, that
/// starts and goes idle in `dir`, which must be on its default branch, so
/// without tasks. Hooked last while paddock is closed, its row on reopen
/// shows the whole event log has been read.
fn witness(sandbox: &Sandbox, terminal: &str, dir: &Path) {
    for event in ["SessionStart", "Stop"] {
        let text = payload(event, dir, None).replace(SESSION, OTHER_SESSION);
        sandbox.hook(terminal, event, &text);
    }
}

// Ticket 10: rows stay, across new conversations and reopening, until
// dismissed with `D` or until their session ends (ticket 20).

/// Main-agent events in the terminal `terminal` names: it starts in `dir`, then a
/// shell command leaves it on each of `branches` in turn, then it stops.
fn work_on(sandbox: &Sandbox, terminal: &str, dir: &Path, branches: &[&str]) {
    sandbox.hook(
        terminal,
        "SessionStart",
        &payload("SessionStart", dir, None),
    );
    for b in branches {
        git(dir, &["switch", "-q", "-c", b]);
        sandbox.hook(
            terminal,
            "PostToolUse",
            &payload("PostToolUse", dir, Some("Bash")),
        );
    }
    sandbox.hook(terminal, "Stop", &payload("Stop", dir, None));
}

/// Detach, then open paddock again.
fn reopen(sandbox: &Sandbox, mut paddock: Paddock) -> Paddock {
    paddock.detach_keys();
    assert!(paddock.wait_exit());
    ready(sandbox)
}

fn has_branch(dir: &Path, branch: &str) -> bool {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["branch", "--list", branch])
        .output()
        .expect("run git");
    !String::from_utf8_lossy(&out.stdout).trim().is_empty()
}

#[test]
fn j_skips_branch_rows_and_d_dismisses_them_with_the_session() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    work_on(&sandbox, "space-1", &dir, &["feat/one", "feat/two"]);
    wait_for_list(&mut paddock, &[&session("idle"), "feat/one", "feat/two"]);
    // The selection covers the session's whole block, as a background.
    let (col, row) = paddock.find("feat/two").expect("feat/two drawn");
    let x = paddock.left() + col;
    assert_eq!(paddock.colors(row, x).1, SELECTED);

    // Task rows cannot be selected: `j` stays on the session.
    paddock.send(b"jD");
    wait_for_list(&mut paddock, &[]);
    assert!(has_branch(&dir, "feat/one"), "git is untouched");

    let mut again = reopen(&sandbox, paddock);
    // Claude is prompted again on feat/two: only that branch comes back.
    sandbox.hook(
        "space-1",
        "UserPromptSubmit",
        &payload("UserPromptSubmit", &dir, None),
    );
    wait_for_list(&mut again, &[&session("running"), "feat/two"]);
}

#[test]
fn d_on_a_session_row_hides_it_with_its_tasks_until_claude_reports_again() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    work_on(&sandbox, "space-1", &dir, &["feat/one"]);
    wait_for_list(&mut paddock, &[&session("idle"), "feat/one"]);

    paddock.send(b"D");
    wait_for_list(&mut paddock, &[]);
    assert_eq!(tabs(&sandbox), 1, "terminal kept");
    assert!(has_branch(&dir, "feat/one"), "git is untouched");

    let mut again = reopen(&sandbox, paddock);
    // Claude is prompted again, back on the default branch.
    git(&dir, &["switch", "-q", "main"]);
    sandbox.hook(
        "space-1",
        "UserPromptSubmit",
        &payload("UserPromptSubmit", &dir, None),
    );
    // Back, but the tasks dismissed with it stay gone.
    wait_for_list(&mut again, &[&session("running")]);
}

#[test]
fn a_closed_terminals_rows_leave_at_once_and_stay_gone_after_reopening() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    work_on(&sandbox, "space-1:1", &dir, &["feat/one"]);
    wait_for_list(&mut paddock, &[&session("idle"), "feat/one"]);

    close_list(&mut paddock);
    paddock.type_line("exit");
    wait_for_list(&mut paddock, &[]);
    assert_eq!(tabs(&sandbox), 1);

    // Replaying the event log does not bring them back.
    paddock.detach_keys();
    assert!(paddock.wait_exit());
    git(&dir, &["switch", "-q", "main"]);
    witness(&sandbox, "space-1:0", &dir);
    let mut again = ready(&sandbox);
    wait_for_list(&mut again, &[&session("idle")]);
}

#[test]
fn a_session_whose_claude_quit_leaves_with_its_task_rows() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    work_on(&sandbox, "space-1:0", &dir, &["feat/one"]);
    // Its turn ended in a tab you are not looking at: done.
    wait_for_list(&mut paddock, &[&session("done"), "feat/one"]);

    sandbox.hook(
        "space-1:0",
        "SessionEnd",
        &payload("SessionEnd", &dir, None),
    );
    wait_for_list(&mut paddock, &[]);
    assert_eq!(tabs(&sandbox), 2, "terminal kept");

    paddock.detach_keys();
    assert!(paddock.wait_exit());
    git(&dir, &["switch", "-q", "main"]);
    witness(&sandbox, "space-1:1", &dir);
    let mut again = ready(&sandbox);
    wait_for_list(&mut again, &[&session("idle")]);

    // A new Claude in the same terminal starts without the ended
    // session's task rows.
    let next = payload("SessionStart", &dir, None).replace(SESSION, NEW_CLAUDE);
    sandbox.hook("space-1:0", "SessionStart", &next);
    wait_for_list(&mut again, &[&session("idle"), &session("idle")]);
}

#[test]
fn a_new_conversation_in_the_same_terminal_keeps_its_task_rows() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    let mut paddock = ready(&sandbox);
    work_on(&sandbox, "space-1", &dir, &["feat/one"]);
    wait_for_list(&mut paddock, &[&session("idle"), "feat/one"]);

    let next = payload("UserPromptSubmit", &dir, None).replace(SESSION, OTHER_SESSION);
    sandbox.hook("space-1", "UserPromptSubmit", &next);
    wait_for_list(&mut paddock, &[&session("running"), "feat/one"]);
}

#[test]
fn j_skips_subagent_rows_and_d_on_the_session_takes_them_along() {
    let sandbox = Sandbox::new();
    let dir = repo(&sandbox);
    write_meta(&sandbox);
    let wt_a = worktree(&dir, AGENT_A);
    let mut paddock = ready(&sandbox);
    let hook = |name: &str, event: &str| {
        sandbox.hook("space-1", event, &recorded(&sandbox, name, &dir, AGENT_A));
    };
    hook("UserPromptSubmit", "UserPromptSubmit");
    hook("SubagentStart-subagent", "SubagentStart");
    git(&wt_a, &["switch", "-q", "-c", "fix/a"]);
    hook("PostToolUse-subagent", "PostToolUse");
    hook("SubagentStop-subagent", "SubagentStop");
    wait_for_list(&mut paddock, &[&session("running"), &sub(FIX_A, "done")]);

    // The subagent's row cannot be selected: `j` stays on the session,
    // and `D` dismisses the session with its subagent.
    paddock.send(b"jD");
    wait_for_list(&mut paddock, &[]);

    let mut again = reopen(&sandbox, paddock);
    hook("Stop", "Stop");
    // Back, but its subagent dismissed with it stays gone.
    wait_for_list(&mut again, &[&session("idle")]);
}
