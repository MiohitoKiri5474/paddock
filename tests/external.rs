//! Ticket 32: external sessions, Claude running outside paddock's terminals.
//! Their hooks fire with no paddock `$TMUX_PANE` (here: no tmux at all), or
//! `claude agents --json` lists them with no pane; they show dim, last, in
//! the dashboard and the sidebar's AGENTS, and nothing acts on a pane for
//! them.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::path::{Path, PathBuf};
use std::process::Command;

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
/// The start of the dashboard's key line, as drawn while it is open.
const OPEN: &str = "j down  k up";
const NO_SCREEN: &str = "external session: no screen";

/// A recorded payload for session `session` in directory `cwd`.
fn payload(name: &str, session: &str, cwd: &str) -> String {
    let path = if name.starts_with("PermissionRequest") {
        format!("{OURS}/{name}.json")
    } else {
        format!("{SPIKE}/{name}.json")
    };
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}"))
        .replace(SESSION, session)
        .replace("\"/tmp/repo\"", &format!("\"{cwd}\""))
}

/// Report `event` for a Claude session outside paddock: no tmux at all.
/// The hook stays silent and exits 0.
fn outside(sandbox: &Sandbox, event: &str, session: &str, cwd: &str) {
    let out = sandbox.run(&["hook", event], &[], &payload(event, session, cwd));
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
}

/// Report `event` for a Claude session in space-1's first terminal.
fn inside(sandbox: &Sandbox, event: &str, session: &str, cwd: &str) {
    sandbox.hook("space-1", event, &payload(event, session, cwd));
}

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

/// A repository `web` with `feat/login` checked out.
fn web_repo(sandbox: &Sandbox) -> PathBuf {
    let dir = sandbox.path("web");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "x\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "one"]);
    git(&dir, &["checkout", "-q", "-b", "feat/login"]);
    dir
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

fn wait_anywhere(paddock: &Paddock, text: &str) {
    paddock.wait_for(text, |_| paddock.full_screen().contains(text));
}

/// The sidebar's lines, blank ones dropped.
fn sidebar(paddock: &Paddock) -> Vec<String> {
    paddock
        .sidebar()
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect()
}

fn wait_for_sidebar(paddock: &Paddock, what: &str, pred: impl Fn(&[String]) -> bool) {
    paddock.wait_for(what, |_| pred(&sidebar(paddock)));
}

/// An agent's first line in the sidebar: dot, label, and its state at the right.
fn agent(label: &str, state: &str) -> String {
    let width = 30 - 3 - state.len();
    format!(" ● {label:<width$}{state}")
}

/// The screen row (0-based) of the first full row starting with `text`.
fn row_of(paddock: &Paddock, text: &str) -> u16 {
    let rows = paddock.full_rows();
    let y = rows.iter().position(|r| r.starts_with(text));
    let y = y.unwrap_or_else(|| panic!("no row starts with {text:?}:\n{rows:#?}"));
    u16::try_from(y).unwrap()
}

fn open_dashboard(paddock: &mut Paddock) {
    paddock.command(b"o");
    paddock.wait_for_text(OPEN);
}

#[test]
fn an_external_session_shows_dim_and_last_with_its_state_and_branch() {
    let sandbox = Sandbox::new();
    let web = web_repo(&sandbox);
    let web = web.to_str().unwrap();
    let mut paddock = ready(&sandbox);
    inside(&sandbox, "UserPromptSubmit", "p1", "/w/mine");
    outside(&sandbox, "UserPromptSubmit", "e1", web);
    outside(&sandbox, "PermissionRequest", "e1", web);

    // Last in AGENTS, under its own header, with its branch; even waiting,
    // it comes after the running agent in paddock.
    let want = [
        agent("mine", "running"),
        "   space-1 · claude".to_string(),
        " EXTERNAL".to_string(),
        agent("web", "waiting"),
        "   feat/login".to_string(),
    ];
    wait_for_sidebar(&paddock, "the external agent", |lines| {
        lines.ends_with(&want)
    });
    let y = row_of(&paddock, &agent("web", "waiting"));
    assert!(paddock.dim(y, 3), "the external label is dim");
    let mine = row_of(&paddock, &agent("mine", "running"));
    assert!(!paddock.dim(mine, 3));
    // It counts for notifications.
    let said = sandbox.wait_for_notifications(1);
    assert_eq!(said, ["web is waiting"]);

    // The dashboard lists it last, dim, under `external`, with its branch;
    // its preview has no screen.
    open_dashboard(&mut paddock);
    paddock.wait_for_text(" external");
    let rows = paddock.rows();
    let at = |s: &str| rows.iter().position(|r| r.starts_with(s));
    let (mine, group, web) = (at(" mine"), at(" external"), at(" web"));
    assert!(mine < group && group.map(|g| g + 1) == web, "{rows:#?}");
    assert!(rows[web.unwrap() + 1].contains("feat/login"), "{rows:#?}");
    paddock.send(b"j");
    paddock.wait_for_text(NO_SCREEN);
}

#[test]
fn keys_on_an_external_row_are_refused_and_d_dismisses_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    outside(&sandbox, "PermissionRequest", "e1", "/w/web");
    wait_anywhere(&paddock, "EXTERNAL");

    // `C-a a` skips it: there is nothing to jump to.
    paddock.command(b"a");
    wait_anywhere(&paddock, "no session needs you");

    open_dashboard(&mut paddock);
    paddock.wait_for_text(NO_SCREEN);
    for (key, notice) in [
        (&b"\r"[..], "external: open it in its own terminal"),
        (b"x", "external: paddock cannot close it"),
        (b"f", "external: paddock cannot fork it"),
        (b"I", "external: no screen to type into"),
    ] {
        paddock.send(key);
        wait_anywhere(&paddock, notice);
        // The dashboard stays, not typing into anything.
        paddock.wait_for_text(OPEN);
        assert!(!paddock.full_screen().contains("typing into"));
    }
    assert_eq!(
        sandbox.tmux(&["list-windows", "-a"]).lines().count(),
        1,
        "nothing closed or forked"
    );

    paddock.send(b"D");
    paddock.wait_for_text("no Claude sessions yet");
    paddock.send(b"\x1b");
    paddock.wait_for("the external group gone", |_| {
        !paddock.full_screen().contains("EXTERNAL")
    });
}

#[test]
fn the_sidebar_refuses_to_open_an_external_agent() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    outside(&sandbox, "Stop", "e1", "/w/web");
    wait_anywhere(&paddock, "EXTERNAL");
    // `C-a s` on space-1; down to the external agent, then Enter, x and f.
    paddock.command(b"s");
    paddock.send(b"j");
    for (key, notice) in [
        (&b"\r"[..], "external: open it in its own terminal"),
        (b"x", "external: paddock cannot close it"),
        (b"f", "external: paddock cannot fork it"),
    ] {
        paddock.send(key);
        wait_anywhere(&paddock, notice);
    }
    // A click on it says the same, and nothing was closed.
    // Selected in the sidebar, its finished turn was seen.
    let y = row_of(&paddock, &agent("web", "idle"));
    paddock.click_screen(5, y);
    wait_anywhere(&paddock, "external: open it in its own terminal");
    assert_eq!(sandbox.tmux(&["list-windows", "-a"]).lines().count(), 1);
}

#[test]
fn an_external_session_leaves_when_it_ends() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    outside(&sandbox, "SessionStart", "e1", "/w/web");
    wait_for_sidebar(&paddock, "the external agent", |l| {
        l.contains(&agent("web", "idle"))
    });
    outside(&sandbox, "SessionEnd", "e1", "/w/web");
    wait_for_sidebar(&paddock, "no external group", |l| {
        !l.iter().any(|l| l.contains("EXTERNAL"))
    });
}

#[test]
fn claude_agents_finds_sessions_outside_paddock() {
    let sandbox = Sandbox::new();
    sandbox.set_agents(
        r#"[{"sessionId":"a1","name":"fix-login","status":"busy","cwd":"/w/api","pid":1}]"#,
    );
    let paddock = ready(&sandbox);
    wait_for_sidebar(&paddock, "the external agent", |l| {
        l.contains(&agent("fix-login", "running")) && l.contains(&" EXTERNAL".to_string())
    });
}

/// Milliseconds since the epoch.
fn now_ms() -> u64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    u64::try_from(d.as_millis()).unwrap()
}

#[test]
fn external_events_from_before_are_replayed_but_quiet_ones_stay_gone() {
    let sandbox = Sandbox::new();
    let events = sandbox.path("state/paddock/events");
    std::fs::create_dir_all(&events).unwrap();
    // As `paddock hook` writes them, without a pane: one an hour old (its
    // Claude was killed without a SessionEnd), one a second old.
    let line = |session: &str, cwd: &str, ts: u64| {
        format!(r#"{{"event":"Stop","session_id":"{session}","cwd":"{cwd}","ts":{ts}}}"#)
    };
    let now = now_ms();
    let log = [
        line("old", "/w/old", now - 3_600_000),
        line("new", "/w/new", now - 1_000),
    ];
    std::fs::write(events.join("external.jsonl"), log.join("\n") + "\n").unwrap();

    let paddock = ready(&sandbox);
    wait_for_sidebar(&paddock, "the recent external agent", |l| {
        l.contains(&agent("new", "done"))
    });
    assert!(!sidebar(&paddock).iter().any(|l| l.contains("old")));
}

#[test]
fn external_false_hides_the_group_and_its_notifications() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[dashboard]\nexternal = false\n");
    let paddock = ready(&sandbox);
    outside(&sandbox, "PermissionRequest", "e1", "/w/web");
    // Something later to wait for: a session in paddock's own terminal.
    inside(&sandbox, "UserPromptSubmit", "p1", "/w/mine");
    wait_for_sidebar(&paddock, "the agent in paddock", |l| {
        l.contains(&agent("mine", "running"))
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!paddock.full_screen().contains("EXTERNAL"));
    assert!(!paddock.full_screen().contains("web"));
    assert!(sandbox.notifications().is_empty());
}

/// Whether the sidebar still shows `line` after a moment.
fn still_shows(paddock: &Paddock, line: &str) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(400));
    sidebar(paddock).iter().any(|l| l == line)
}

#[test]
fn a_done_external_turn_turns_idle_once_its_dashboard_row_is_selected() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    outside(&sandbox, "Stop", "e1", "/w/web");
    let done = agent("web", "done");
    wait_for_sidebar(&paddock, "done", |l| l.contains(&done));
    // It has no tab: looking at the terminal does not see it.
    assert!(still_shows(&paddock, &done));
    open_dashboard(&mut paddock);
    let idle = agent("web", "idle");
    wait_for_sidebar(&paddock, "seen in the dashboard", |l| l.contains(&idle));
}

#[test]
fn a_done_external_turn_turns_idle_once_selected_in_the_sidebar() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    outside(&sandbox, "Stop", "e1", "/w/web");
    let done = agent("web", "done");
    wait_for_sidebar(&paddock, "done", |l| l.contains(&done));
    // The sidebar on space-1: not on the agent yet.
    paddock.command(b"s");
    assert!(still_shows(&paddock, &done));
    paddock.send(b"j");
    let idle = agent("web", "idle");
    wait_for_sidebar(&paddock, "seen in the sidebar", |l| l.contains(&idle));
}
