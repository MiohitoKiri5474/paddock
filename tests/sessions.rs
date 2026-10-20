//! Ticket 06: Claude sessions in the sidebar, driven by recorded hook payloads.
//!
//! Payloads come from spike 02's recorded fixtures. The waiting payloads in
//! `tests/fixtures/hooks/` were not recorded (spike 02 ran with
//! bypassPermissions, so none fired); they follow Claude Code's documented
//! hook input schema.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use serde_json::{json, Value};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const OURS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
/// Columns of the sidebar before its border.
const SIDEBAR: usize = 23;

/// A recorded (or schema-built) payload, optionally for another session and directory.
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

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Sidebar rows (below its title), trimmed.
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
    let start = std::time::Instant::now();
    while start.elapsed() < harness::TIMEOUT {
        if sidebar(paddock) == want {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!(
        "sidebar is {:?}, not {want:?}; screen:\n{}",
        sidebar(paddock),
        paddock.screen()
    );
}

/// A sidebar row as drawn: label left, state right.
fn row(label: &str, state: &str) -> String {
    let gap = SIDEBAR - 2 - label.len() - state.len();
    format!("{label}{}{state}", " ".repeat(gap))
}

fn new_tab(paddock: &mut Paddock, label: &str) {
    paddock.command(b"c");
    paddock.wait_for_current_tab(label);
    paddock.wait_for_text(PROMPT);
}

#[test]
fn hook_events_drive_the_session_row() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:terminal-1"]); // no sidebar without sessions

    let steps = [
        ("SessionStart", "idle"),
        ("UserPromptSubmit", "running"),
        ("PermissionRequest", "waiting"),
        ("PostToolUse", "running"),
        ("Notification-permission_prompt", "waiting"),
        ("Stop", "idle"),
        ("SessionEnd", "stopped"),
    ];
    for (name, state) in steps {
        let event = name.split('-').next().unwrap();
        let out = sandbox.hook("terminal-1", event, &payload(name));
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
        wait_for_sidebar(&paddock, &[&row("repo", state)]);
    }
    paddock.wait_for_text("sessions");
}

/// What a case is called, the hook's extra environment, and its event.
type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

#[test]
fn hooks_outside_paddock_record_nothing_and_say_nothing() {
    let sandbox = Sandbox::new();
    let _paddock = ready(&sandbox);
    let pid = sandbox.display("terminal-1", "#{pid}");
    let foreign = format!("/tmp/tmux-501/default,{pid},0");
    let ours = sandbox.display("terminal-1", "#{socket_path},#{pid},0");
    let foreign_env = [("TMUX", foreign.as_str()), ("TMUX_PANE", "%0")];
    let no_pane = [("TMUX", ours.as_str())];
    let cases: [Case; 4] = [
        ("foreign tmux", &foreign_env, "Stop"),
        ("no tmux", &[], "Stop"),
        ("no pane", &no_pane, "Stop"),
        ("no pane, prompt", &no_pane, "UserPromptSubmit"),
    ];
    for (what, env, event) in cases {
        let out = sandbox.run(&["hook", event], env, &payload(event));
        assert!(out.status.success(), "{what}: {out:?}");
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "{what}: {out:?}"
        );
    }
    assert!(
        !sandbox.path("state/paddock/events").exists(),
        "nothing recorded"
    );

    // A broken payload in a paddock terminal is still silent and still exits 0.
    let out = sandbox.hook("terminal-1", "SessionStart", "{not json");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
}

#[test]
fn events_map_to_their_own_terminal_and_rows_sort_by_state() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:terminal-2");
    new_tab(&mut paddock, "3:terminal-3");
    new_tab(&mut paddock, "4:terminal-4");

    let start = |t: &str, s: &str, dir: &str, event: &str| {
        sandbox.hook(t, event, &payload_for(event, s, dir));
    };
    start("terminal-1", "s1", "/w/stopped", "SessionEnd");
    start("terminal-2", "s2", "/w/running", "UserPromptSubmit");
    start("terminal-3", "s3", "/w/idle", "Stop");
    start("terminal-4", "s4", "/w/waiting", "PermissionRequest");

    wait_for_sidebar(
        &paddock,
        &[
            &row("waiting", "waiting"),
            &row("idle", "idle"),
            &row("running", "running"),
            &row("stopped", "stopped"),
        ],
    );
}

#[test]
fn events_recorded_while_paddock_is_closed_appear_on_reopen() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    sandbox.hook(
        "terminal-1",
        "UserPromptSubmit",
        &payload("UserPromptSubmit"),
    );
    sandbox.hook(
        "terminal-1",
        "PermissionRequest",
        &payload("PermissionRequest"),
    );

    let again = ready(&sandbox);
    wait_for_sidebar(&again, &[&row("repo", "waiting")]);
}

#[test]
fn a_renamed_tab_labels_its_session_row() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    sandbox.hook("terminal-1", "Stop", &payload("Stop"));
    wait_for_sidebar(&paddock, &[&row("repo", "idle")]);

    paddock.command(b",");
    paddock.wait_for_text("rename tab:");
    paddock.send(b"\x15api\r");

    wait_for_sidebar(&paddock, &[&row("api", "idle")]);
}

#[test]
fn claude_agents_names_sessions_and_corrects_missed_events() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    sandbox.hook(
        "terminal-1",
        "UserPromptSubmit",
        &payload("UserPromptSubmit"),
    );
    wait_for_sidebar(&paddock, &[&row("repo", "running")]);

    // Esc fires no Stop; claude still reports the session as idle.
    sandbox.set_agents(
        &json!([{"sessionId": SESSION, "name": "fix-login", "status": "idle", "pid": 1}])
            .to_string(),
    );

    wait_for_sidebar(&paddock, &[&row("fix-login", "idle")]);
}

#[test]
fn claude_agents_finds_sessions_no_hook_reported() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    let shell: u32 = sandbox
        .display("terminal-1", "#{pane_pid}")
        .parse()
        .expect("pane pid");

    sandbox.set_agents(
        &json!([{"sessionId": "found", "cwd": "/w/older", "status": "waiting", "pid": shell}])
            .to_string(),
    );

    wait_for_sidebar(&paddock, &[&row("older", "waiting")]);
}

#[test]
fn s_focuses_the_sidebar_and_enter_jumps_to_the_terminal() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:terminal-2");
    new_tab(&mut paddock, "3:terminal-3");
    sandbox.hook("terminal-1", "Stop", &payload_for("Stop", "s1", "/w/one"));
    sandbox.hook(
        "terminal-2",
        "PermissionRequest",
        &payload_for("PermissionRequest", "s2", "/w/two"),
    );
    wait_for_sidebar(&paddock, &[&row("two", "waiting"), &row("one", "idle")]);

    paddock.command(b"s");
    paddock.send(b"j"); // down to "one"
    paddock.send(b"\r");
    paddock.wait_for_current_tab("1:terminal-1");

    paddock.command(b"s");
    paddock.send(b"k\r"); // back up to "two"
    paddock.wait_for_current_tab("2:terminal-2");

    // Keys typed after Enter reach the terminal again.
    paddock.type_line("echo back-in-the-shell");
    paddock.wait_for_text("back-in-the-shell\n");
}

#[test]
fn enter_focuses_the_split_running_the_session() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"|");
    paddock.wait_for("two terminals", |_| {
        sandbox.session_names() == ["terminal-1", "terminal-2"]
    });
    sandbox.hook("terminal-2", "Stop", &payload("Stop"));
    wait_for_sidebar(&paddock, &[&row("repo", "idle")]);
    paddock.command(b"h"); // focus the left terminal

    paddock.command(b"s");
    paddock.send(b"\r");
    paddock.type_line("echo typed-into-the-split");

    let capture = || sandbox.tmux(&["capture-pane", "-p", "-t", "terminal-2"]);
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
fn esc_leaves_the_sidebar_without_jumping() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:terminal-2");
    sandbox.hook("terminal-1", "Stop", &payload("Stop"));
    wait_for_sidebar(&paddock, &[&row("repo", "idle")]);

    paddock.command(b"s");
    paddock.send(b"\x1b");
    std::thread::sleep(std::time::Duration::from_millis(200));
    paddock.type_line("echo still-on-two");
    paddock.wait_for_text("still-on-two\n");
    assert_eq!(paddock.current_tab(), "2:terminal-2");
}

#[test]
fn b_hides_and_shows_the_sidebar() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    sandbox.hook("terminal-1", "Stop", &payload("Stop"));
    wait_for_sidebar(&paddock, &[&row("repo", "idle")]);
    let next_to_sidebar = harness::COLS - SIDEBAR as u16 - 1;
    wait_for_width(&sandbox, next_to_sidebar);

    paddock.command(b"b");
    paddock.wait_for("sidebar hidden", |s| !s.contains("sessions"));
    paddock.wait_for("tab bar at the left edge", |s| {
        s.starts_with(" 1:terminal-1")
    });
    wait_for_width(&sandbox, harness::COLS);

    paddock.command(b"b");
    wait_for_sidebar(&paddock, &[&row("repo", "idle")]);
    wait_for_width(&sandbox, next_to_sidebar);
}

/// Wait until terminal-1 is `cols` wide on the hidden server.
fn wait_for_width(sandbox: &Sandbox, cols: u16) {
    let start = std::time::Instant::now();
    let width = || sandbox.display("terminal-1", "#{pane_width}");
    while start.elapsed() < harness::TIMEOUT {
        if width() == cols.to_string() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("terminal is {} wide, not {cols}", width());
}

#[test]
fn parallel_hooks_never_interleave_records() {
    let sandbox = Sandbox::new();
    let _paddock = ready(&sandbox);
    let big = payload("Stop").replace("Both subagents", &"x".repeat(20_000));
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| sandbox.hook("terminal-1", "Stop", &big));
        }
    });
    let pid = sandbox.display("terminal-1", "#{pid}");
    let log = std::fs::read_to_string(sandbox.path(&format!("state/paddock/events/{pid}.jsonl")))
        .expect("event log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 16);
    for l in lines {
        let v: Value = serde_json::from_str(l).expect("whole record");
        assert_eq!(v["event"], "Stop");
    }
}

#[test]
fn hooks_install_keeps_other_hooks_and_uninstall_removes_only_paddocks() {
    let sandbox = Sandbox::new();
    let settings = sandbox.path("home/.claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let theirs = json!({
        "model": "opus",
        "hooks": {"Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "supacode # supacode-managed-hook"}]}]}
    });
    let original = serde_json::to_string_pretty(&theirs).unwrap() + "\n";
    std::fs::write(&settings, &original).unwrap();

    let out = sandbox.run(&["hooks", "install"], &[], "");
    assert!(out.status.success(), "{out:?}");
    let once = std::fs::read_to_string(&settings).unwrap();
    assert!(sandbox.run(&["hooks", "install"], &[], "").status.success());
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        once,
        "idempotent"
    );

    let v: Value = serde_json::from_str(&once).unwrap();
    let exe = env!("CARGO_BIN_EXE_paddock");
    let stop: Vec<&str> = v["hooks"]["Stop"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["hooks"][0]["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        stop,
        [
            "supacode # supacode-managed-hook",
            &format!("{exe} hook Stop")
        ]
    );
    assert!(v["hooks"]["PermissionRequest"].is_array());

    assert!(sandbox
        .run(&["hooks", "uninstall"], &[], "")
        .status
        .success());
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), original);
}
