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
    paddock.wait_for("help", |_| paddock.full_screen().contains("paddock keys"));
    paddock.send(b"\x1b");
    paddock.wait_for("help closed", |_| {
        !paddock.full_screen().contains("paddock keys")
    });
}

#[test]
fn waiting_and_finished_turns_notify_once_each() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:home"); // look away from the first tab

    hooks(
        &sandbox,
        "space-1:0",
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
        "space-1:0",
        &["UserPromptSubmit", "PermissionRequest", "Stop"],
    );
    paddock.wait_for("idle", |_| paddock.full_screen().contains("idle"));

    // With the outer window in the background, even this terminal notifies.
    paddock.send(b"\x1b[O");
    settle(&mut paddock);
    hooks(
        &sandbox,
        "space-1:0",
        &["UserPromptSubmit", "PermissionRequest"],
    );
    assert_eq!(sandbox.wait_for_notifications(1), [WAITING]);

    paddock.send(b"\x1b[I");
    settle(&mut paddock);
    hooks(&sandbox, "space-1:0", &["Stop"]);
    paddock.wait_for("idle", |_| paddock.full_screen().contains("idle"));

    // The sentinel, once the first tab is out of sight.
    new_tab(&mut paddock, "2:home");
    hooks(
        &sandbox,
        "space-1:0",
        &["UserPromptSubmit", "PermissionRequest"],
    );
    assert_eq!(sandbox.wait_for_notifications(2), [WAITING, WAITING]);
}

#[test]
fn events_while_paddock_is_closed_never_notify() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:home"); // reopens on the second tab
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    hooks(
        &sandbox,
        "space-1:0",
        &["UserPromptSubmit", "PermissionRequest"],
    );
    let again = ready(&sandbox);
    again.wait_for("waiting", |_| again.full_screen().contains("waiting"));
    hooks(
        &sandbox,
        "space-1:0",
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

    new_tab(&mut paddock, "2:home");
    paddock.command(b"C"); // a second space, current
    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_text(PROMPT);
    let hook = |t: &str, s: &str, dir: &str, event: &str| {
        sandbox.hook(t, event, &payload_for(event, s, dir));
    };
    hook("space-1:0", "s1", "/w/idle", "Stop");
    hook("space-1:1", "s2", "/w/running", "UserPromptSubmit");
    hook("space-2:0", "s3", "/w/waiting", "PermissionRequest");
    // The last hook: all three applied.
    paddock.wait_for_tabs(&["1:claude ●"]);

    paddock.command(b"a");
    paddock.wait_for_tabs(&["1:claude ●", "2:claude"]);
    paddock.wait_for_current_tab("1:claude ●");
    paddock.command(b"a");
    paddock.wait_for_tabs(&["1:claude ●"]);
    paddock.command(b"w");
    paddock.send(b"k\r");
    paddock.command(b"2");
    paddock.wait_for_current_tab("2:claude");
    paddock.command(b"a");
    paddock.wait_for_tabs(&["1:claude ●"]);

    // Highlights, in the terminal's own colours: the waiting agent's state
    // and the tab holding the idle session.
    let (black, yellow, green) = (
        vt100::Color::Idx(0),
        vt100::Color::Idx(3),
        vt100::Color::Idx(2),
    );
    // Rows: 0 SPACES, 1-4 two spaces, 5 blank, 6 AGENTS, 7 the waiting agent.
    assert_eq!(paddock.sidebar()[7], " ● waiting     waiting");
    assert_eq!(paddock.colors(7, 15), (black, yellow));
    paddock.command(b"w");
    paddock.send(b"k\r");
    paddock.wait_for_tabs(&["1:claude ●", "2:claude"]);
    let left = harness::SIDEBAR;
    assert_eq!(paddock.colors(0, left + 1).0, green);
}

#[test]
fn c_a_a_focuses_the_split_running_the_session() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"|");
    paddock.wait_for("two prompts", |s| s.matches(PROMPT).count() == 2);
    sandbox.hook("space-1:0.1", "Stop", &payload("Stop"));
    paddock.wait_for_tabs(&["1:claude ●"]); // the focused pane runs Claude
    paddock.command(b"h"); // focus the left pane

    paddock.command(b"a");
    paddock.type_line("echo typed-into-the-split");

    let capture = || sandbox.tmux(&["capture-pane", "-p", "-t", "space-1:0.1"]);
    let start = std::time::Instant::now();
    while !capture().contains("typed-into-the-split\n") {
        assert!(
            start.elapsed() < harness::TIMEOUT,
            "split got:\n{}",
            capture()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn a_turn_finished_out_of_sight_is_done_until_you_look_at_it() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:home");

    hooks(&sandbox, "space-1:0", &["UserPromptSubmit", "Stop"]);
    paddock.wait_for("done", |_| paddock.full_screen().contains("done"));

    // Looking at it, with the outer window focused, makes it idle.
    paddock.command(b"1");
    paddock.wait_for("idle", |_| {
        let s = paddock.full_screen();
        s.contains("idle") && !s.contains("done")
    });
}

#[test]
fn a_turn_finished_in_view_is_idle_at_once() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);

    hooks(&sandbox, "space-1:0", &["UserPromptSubmit", "Stop"]);

    paddock.wait_for("idle", |_| paddock.full_screen().contains("idle"));
    assert!(!paddock.full_screen().contains("done"));
}
