//! Ticket 07: sessions that need you stand out, notify, and are one key away.
//!
//! Notifications go to the sandbox's stub `osascript`, which logs each
//! message. To show that something never notifies, a test makes it happen,
//! then makes one thing that should notify, and checks that only that one did.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
const WAITING: &str = "repo is waiting";
const IDLE: &str = "repo is idle";

fn payload(name: &str) -> String {
    let path = if name.starts_with("PermissionRequest") || name.starts_with("Notification") {
        format!("{OURS}/{name}.json")
    } else {
        format!("{SPIKE}/{name}.json")
    };
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn payload_for(name: &str, session: &str, cwd: &str) -> String {
    payload(name)
        .replace(SESSION, session)
        .replace("\"/tmp/repo\"", &format!("\"{cwd}\""))
}

/// Run hooks in order in terminal `terminal`; `-` separates an event from
/// its payload variant (`Notification-permission_prompt`).
fn hooks(sandbox: &Sandbox, terminal: &str, names: &[&str]) {
    for name in names {
        let event = name.split('-').next().unwrap();
        sandbox.hook(terminal, event, &payload(name));
    }
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

fn new_tab(paddock: &mut Paddock, label: &str) {
    paddock.command(b"c");
    paddock.wait_for_current_tab(label);
    paddock.wait_for_text(PROMPT);
}

/// Wait until paddock has handled every key sent so far: keys are handled in
/// order, so once the help it opens is up, the earlier ones are done.
fn settle(paddock: &mut Paddock) {
    paddock.command(b"?");
    paddock.wait_for_text("paddock keys");
    paddock.send(b"\x1b");
    paddock.wait_for("help closed", |s| !s.contains("paddock keys"));
}

#[test]
fn waiting_and_finished_turns_notify_once_each() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:terminal-2"); // look away from terminal-1

    hooks(
        &sandbox,
        "terminal-1",
        &[
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            // Claude Code reports the same prompt again about 6s later.
            "Notification-permission_prompt",
            "PostToolUse",
            "Stop",
            // The sentinel: a new wait.
            "UserPromptSubmit",
            "PermissionRequest",
        ],
    );

    // Hooks handled in one batch may notify in any order.
    let mut shown = sandbox.wait_for_notifications(3);
    shown.sort();
    assert_eq!(shown, [IDLE, WAITING, WAITING]);
}

#[test]
fn the_terminal_you_look_at_never_notifies() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    hooks(
        &sandbox,
        "terminal-1",
        &["UserPromptSubmit", "PermissionRequest", "Stop"],
    );
    paddock.wait_for_text("idle");

    // With the outer window in the background, even this terminal notifies.
    paddock.send(b"\x1b[O");
    settle(&mut paddock);
    hooks(
        &sandbox,
        "terminal-1",
        &["UserPromptSubmit", "PermissionRequest"],
    );
    assert_eq!(sandbox.wait_for_notifications(1), [WAITING]);

    paddock.send(b"\x1b[I");
    settle(&mut paddock);
    hooks(&sandbox, "terminal-1", &["Stop"]);
    paddock.wait_for_text("idle");

    // The sentinel, once terminal-1 is out of sight.
    new_tab(&mut paddock, "2:terminal-2");
    hooks(
        &sandbox,
        "terminal-1",
        &["UserPromptSubmit", "PermissionRequest"],
    );
    assert_eq!(sandbox.wait_for_notifications(2), [WAITING, WAITING]);
}

#[test]
fn events_while_paddock_is_closed_never_notify() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:terminal-2"); // reopens on terminal-2
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    hooks(
        &sandbox,
        "terminal-1",
        &["UserPromptSubmit", "PermissionRequest"],
    );
    let again = ready(&sandbox);
    again.wait_for_text("waiting");
    hooks(
        &sandbox,
        "terminal-1",
        &[
            // The reminder of the wait that began while closed.
            "Notification-permission_prompt",
            "PostToolUse",
            "Stop",
        ],
    );

    assert_eq!(sandbox.wait_for_notifications(1), [IDLE]);
}

#[test]
fn c_a_a_cycles_through_sessions_that_need_you() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"a");
    paddock.wait_for_text("no session needs you");

    new_tab(&mut paddock, "2:terminal-2");
    new_tab(&mut paddock, "3:terminal-3");
    let hook = |t: &str, s: &str, dir: &str, event: &str| {
        sandbox.hook(t, event, &payload_for(event, s, dir));
    };
    hook("terminal-1", "s1", "/w/idle", "Stop");
    hook("terminal-2", "s2", "/w/running", "UserPromptSubmit");
    hook("terminal-3", "s3", "/w/waiting", "PermissionRequest");
    paddock.wait_for_text(" waiting"); // the last hook: all three applied

    paddock.command(b"a");
    paddock.wait_for_current_tab("1:terminal-1");
    paddock.command(b"a");
    paddock.wait_for_current_tab("3:terminal-3");
    paddock.command(b"2");
    paddock.wait_for_current_tab("2:terminal-2");
    paddock.command(b"a");
    paddock.wait_for_current_tab("3:terminal-3");

    // Highlights, in the terminal's own colours: the waiting row and the
    // tab holding the idle session.
    let (black, yellow, green) = (
        vt100::Color::Idx(0),
        vt100::Color::Idx(3),
        vt100::Color::Idx(2),
    );
    assert!(paddock.rows()[1].starts_with(" waiting"));
    assert_eq!(paddock.colors(1, 1), (black, yellow));
    let bar = paddock.tab_bar();
    let at = bar.find("1:terminal-1").expect("tab 1");
    let tab1 = bar[..at].chars().count() as u16;
    assert_eq!(paddock.colors(0, tab1).0, green);
}
