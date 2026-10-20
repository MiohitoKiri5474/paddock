//! Ticket 13: closing a terminal (asking first while Claude is busy) and
//! forking a session into a new tab. The stub `claude` only records how it
//! was run.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::time::Duration;

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
const ASK: &str = "close? y/n";

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

/// Report `event` for a Claude session in `terminal`.
fn claude(sandbox: &Sandbox, terminal: &str, event: &str, session: &str) {
    sandbox.hook(terminal, event, &payload(event, session, "/w/repo"));
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Two tabs, the second current. Closing the last tab would quit paddock.
fn two_tabs(sandbox: &Sandbox) -> Paddock {
    let mut paddock = ready(sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:terminal-2");
    paddock.wait_for_text(PROMPT);
    paddock
}

fn wait_for_terminals(paddock: &Paddock, sandbox: &Sandbox, want: &[&str]) {
    paddock.wait_for(&format!("terminals {want:?}"), |_| {
        sandbox.session_names() == want
    });
}

#[test]
fn x_closes_an_idle_session_without_asking() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-2", "Stop", "s2");
    paddock.wait_for_text("idle");

    paddock.command(b"x");

    wait_for_terminals(&paddock, &sandbox, &["terminal-1"]);
    paddock.wait_for_tabs(&["1:terminal-1"]);
    assert!(!paddock.screen().contains(ASK));
}

#[test]
fn x_asks_while_claude_is_running() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-2", "UserPromptSubmit", "s2");
    paddock.wait_for_text("running");

    paddock.command(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"n");
    paddock.wait_for("the question gone", |s| !s.contains(ASK));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sandbox.session_names(), ["terminal-1", "terminal-2"]);

    paddock.command(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"y");
    wait_for_terminals(&paddock, &sandbox, &["terminal-1"]);
}

#[test]
fn the_question_shows_even_with_many_tabs() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    for n in 3..=5 {
        paddock.command(b"c");
        paddock.wait_for_current_tab(&format!("{n}:terminal-{n}"));
    }
    claude(&sandbox, "terminal-5", "UserPromptSubmit", "s5");
    paddock.wait_for_text("running");

    paddock.command(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"\x1b");
    paddock.wait_for_text("5:terminal-5");
    assert_eq!(sandbox.session_names().len(), 5);
}

#[test]
fn x_on_a_waiting_sidebar_row_asks_then_closes_that_terminal() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "terminal-1", "PermissionRequest", "s1");
    paddock.wait_for_text("waiting");

    paddock.command(b"s");
    paddock.send(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"y");

    wait_for_terminals(&paddock, &sandbox, &["terminal-2"]);
    paddock.wait_for_tabs(&["1:terminal-2"]);
}

/// A real directory for the session, and how the stub sees it.
fn repo(sandbox: &Sandbox) -> (String, String) {
    let dir = sandbox.path("home/repo");
    std::fs::create_dir_all(&dir).expect("mkdir repo");
    let canonical = dir.canonicalize().expect("canonical repo");
    (
        dir.to_string_lossy().into_owned(),
        canonical.to_string_lossy().into_owned(),
    )
}

#[test]
fn f_forks_the_session_into_a_new_tab_in_its_directory() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let (dir, canonical) = repo(&sandbox);
    sandbox.hook(
        "terminal-1",
        "Stop",
        &payload("Stop", "0000aaaa-0000-4000-8000-00000000000a", &dir),
    );
    paddock.wait_for_text("idle");

    paddock.command(b"f");

    paddock.wait_for_current_tab("2:terminal-2");
    assert_eq!(
        sandbox.claude_calls(),
        [format!(
            "{canonical} --resume 0000aaaa-0000-4000-8000-00000000000a --fork-session"
        )]
    );
}

#[test]
fn f_on_a_sidebar_row_forks_that_session() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    let (dir, canonical) = repo(&sandbox);
    sandbox.hook(
        "terminal-1",
        "Stop",
        &payload("Stop", "0000bbbb-0000-4000-8000-00000000000b", &dir),
    );
    paddock.wait_for_text("idle");

    paddock.command(b"s");
    paddock.send(b"f");

    paddock.wait_for_current_tab("3:terminal-3");
    assert_eq!(
        sandbox.claude_calls(),
        [format!(
            "{canonical} --resume 0000bbbb-0000-4000-8000-00000000000b --fork-session"
        )]
    );
    // The keyboard is back in the new terminal.
    paddock.type_line("echo in-the-fork");
    paddock.wait_for_text("in-the-fork\n");
}

#[test]
fn f_in_a_plain_shell_says_so_and_opens_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"f");

    paddock.wait_for_text("no Claude session here");
    assert_eq!(sandbox.session_names(), ["terminal-1"]);
}
