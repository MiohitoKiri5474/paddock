//! Ticket 13: closing a pane or item (asking first while Claude is busy) and
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
const ASK_ITEM: &str = "close workspace? y/n";

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

/// Report `event` for a Claude session in the pane tmux target `terminal` names.
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
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Window count of workspace-1.
fn tabs(sandbox: &Sandbox) -> usize {
    sandbox
        .tmux(&["list-windows", "-t", "workspace-1"])
        .lines()
        .count()
}

fn wait_for_tab_count(paddock: &Paddock, sandbox: &Sandbox, want: usize) {
    paddock.wait_for(&format!("{want} tabs"), |_| tabs(sandbox) == want);
}

fn wait_for_items(paddock: &Paddock, sandbox: &Sandbox, want: &[&str]) {
    paddock.wait_for(&format!("items {want:?}"), |_| {
        sandbox.session_names() == want
    });
}

#[test]
fn x_closes_an_idle_session_without_asking() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "workspace-1:1", "Stop", "s2");
    paddock.wait_for_tabs(&["1:home", "2:claude ●"]);

    paddock.command(b"x");

    wait_for_tab_count(&paddock, &sandbox, 1);
    paddock.wait_for_tabs(&["1:home"]);
    assert!(!paddock.screen().contains(ASK));
}

#[test]
fn x_asks_while_claude_is_running() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    claude(&sandbox, "workspace-1:1", "UserPromptSubmit", "s2");
    paddock.wait_for_tabs(&["1:home", "2:claude"]);

    paddock.command(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"n");
    paddock.wait_for("the question gone", |s| !s.contains(ASK));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(tabs(&sandbox), 2);

    paddock.command(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"y");
    wait_for_tab_count(&paddock, &sandbox, 1);
}

#[test]
fn the_question_shows_even_with_many_tabs() {
    let sandbox = Sandbox::new();
    let mut paddock = two_tabs(&sandbox);
    for n in 3..=5 {
        paddock.command(b"c");
        paddock.wait_for_current_tab(&format!("{n}:home"));
    }
    claude(&sandbox, "workspace-1:4", "UserPromptSubmit", "s5");
    paddock.wait_for_text("5:claude");

    paddock.command(b"x");
    paddock.wait_for_text(ASK);
    paddock.send(b"\x1b");
    paddock.wait_for_text("5:claude");
    assert_eq!(tabs(&sandbox), 5);
}

#[test]
fn x_on_a_sidebar_item_with_a_waiting_claude_asks_then_closes_the_item() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.command(b"C");
    paddock.wait_for_tabs(&["1:home"]);
    claude(&sandbox, "workspace-1:0", "PermissionRequest", "s1");
    paddock.wait_for("waiting", |_| paddock.full_screen().contains("waiting"));

    paddock.command(b"s");
    paddock.send(b"k");
    paddock.send(b"x");
    paddock.wait_for_text(ASK_ITEM);
    paddock.send(b"y");

    wait_for_items(&paddock, &sandbox, &["workspace-2"]);
    paddock.wait_for_tabs(&["1:home"]);
}

#[test]
fn x_on_a_quiet_sidebar_item_closes_it_without_asking() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C");
    paddock.wait_for_text(PROMPT);

    paddock.command(b"s");
    paddock.send(b"x");

    wait_for_items(&paddock, &sandbox, &["workspace-1"]);
    paddock.wait_for("one item left", |_| {
        !paddock.full_screen().contains("workspace-2")
    });
    paddock.send(b"\x1b");
    // Esc right before a key would read as Alt with that key.
    std::thread::sleep(Duration::from_millis(200));
    paddock.type_line("echo back-in-one");
    paddock.wait_for_text("back-in-one\n");
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
        "workspace-1",
        "Stop",
        &payload("Stop", "0000aaaa-0000-4000-8000-00000000000a", &dir),
    );
    paddock.wait_for_tabs(&["1:claude ●"]);

    paddock.command(b"f");

    paddock.wait_for_current_tab("2:repo");
    assert_eq!(
        sandbox.claude_calls(),
        [format!(
            "{canonical} --resume 0000aaaa-0000-4000-8000-00000000000a --fork-session"
        )]
    );
}

#[test]
fn f_on_a_sidebar_item_forks_its_session_into_a_new_tab_of_that_item() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C");
    paddock.wait_for_text(PROMPT);
    let (dir, canonical) = repo(&sandbox);
    sandbox.hook(
        "workspace-1",
        "Stop",
        &payload("Stop", "0000bbbb-0000-4000-8000-00000000000b", &dir),
    );
    paddock.wait_for("done", |_| paddock.full_screen().contains("done"));

    paddock.command(b"s");
    paddock.send(b"k");
    paddock.send(b"f");

    paddock.wait_for_tabs(&["1:claude ●", "2:repo"]);
    paddock.wait_for_current_tab("2:repo");
    assert_eq!(
        sandbox.claude_calls(),
        [format!(
            "{canonical} --resume 0000bbbb-0000-4000-8000-00000000000b --fork-session"
        )]
    );
    assert_eq!(sandbox.session_names(), ["workspace-1", "workspace-2"]);
    // The keyboard is back in the new pane.
    paddock.type_line("echo in-the-fork");
    paddock.wait_for_text("in-the-fork\n");
}

#[test]
fn f_in_a_plain_shell_says_so_and_opens_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"f");

    paddock.wait_for_text("no Claude session here");
    assert_eq!(tabs(&sandbox), 1);
}
