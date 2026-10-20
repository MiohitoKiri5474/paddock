//! Ticket 06: Claude sessions, driven by recorded hook payloads, as the
//! sidebar's spaces and agents and the tabs holding them show them.
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

/// Sidebar lines below its SPACES header, blank ones dropped.
fn sidebar(paddock: &Paddock) -> Vec<String> {
    paddock
        .sidebar()
        .into_iter()
        .skip(1)
        .filter(|l| !l.is_empty())
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
        paddock.full_screen()
    );
}

/// A space's line: a dot if agents run in it, then its name.
fn space(name: &str, agents: bool) -> String {
    let dot = if agents { "●" } else { " " };
    format!(" {dot} {name}")
}

/// A space's second line, for one in the test shells' home (no repository).
fn at_home() -> String {
    "   ~".into()
}

/// An agent's first line: dot, label, and its state at the right.
fn agent(label: &str, state: &str) -> String {
    let width = 22 - 3 - state.len();
    format!(" ● {label:<width$}{state}")
}

/// An agent's second line: its space and tab.
fn place(space: &str, tab: &str) -> String {
    format!("   {space} · {tab}")
}

/// The sidebar with space-1 and one agent in its first tab.
fn one_agent(label: &str, state: &str, tab: &str) -> Vec<String> {
    vec![
        space("space-1", true),
        at_home(),
        " AGENTS".into(),
        agent(label, state),
        place("space-1", tab),
    ]
}

/// The sidebar with only space-1 and no agents.
fn no_agents() -> Vec<String> {
    vec![space("space-1", false), at_home(), " AGENTS".into()]
}

fn wait_for_lines(paddock: &Paddock, want: &[String]) {
    let want: Vec<&str> = want.iter().map(String::as_str).collect();
    wait_for_sidebar(paddock, &want);
}

fn new_tab(paddock: &mut Paddock, label: &str) {
    paddock.command(b"c");
    paddock.wait_for_current_tab(label);
    paddock.wait_for_text(PROMPT);
}

#[test]
fn hook_events_drive_the_agent_row() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    wait_for_lines(&paddock, &no_agents());

    // The tab is looked at, so a finished turn is idle, not done.
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
        let out = sandbox.hook("space-1", event, &payload(name));
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
        let tab = if state == "stopped" { "home" } else { "claude" };
        wait_for_lines(&paddock, &one_agent("repo", state, tab));
    }
}

/// What a case is called, the hook's extra environment, and its event.
type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

#[test]
fn hooks_outside_paddock_record_nothing_and_say_nothing() {
    let sandbox = Sandbox::new();
    let _paddock = ready(&sandbox);
    let pid = sandbox.display("space-1", "#{pid}");
    let foreign = format!("/tmp/tmux-501/default,{pid},0");
    let ours = sandbox.display("space-1", "#{socket_path},#{pid},0");
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
    let out = sandbox.hook("space-1", "SessionStart", "{not json");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
}

#[test]
fn agents_of_every_space_are_listed_with_their_space_and_tab() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:home");
    new_tab(&mut paddock, "3:home");
    new_tab(&mut paddock, "4:home");
    paddock.command(b"C");
    paddock.wait_for_tabs(&["1:home"]);

    let start = |t: &str, s: &str, dir: &str, event: &str| {
        sandbox.hook(t, event, &payload_for(event, s, dir));
    };
    start("space-1:0", "s1", "/w/stopped", "SessionEnd");
    start("space-1:1", "s2", "/w/running", "UserPromptSubmit");
    start("space-1:2", "s3", "/w/idle", "Stop");
    start("space-1:3", "s4", "/w/waiting", "PermissionRequest");

    // Sorted by state; the turn that ended out of sight is done.
    wait_for_lines(
        &paddock,
        &[
            space("space-1", true),
            at_home(),
            space("space-2", false),
            at_home(),
            " AGENTS".into(),
            agent("waiting", "waiting"),
            place("space-1", "claude"),
            agent("idle", "done"),
            place("space-1", "claude"),
            agent("running", "running"),
            place("space-1", "claude"),
            agent("stopped", "stopped"),
            place("space-1", "home"),
        ],
    );
    // The space's tabs show theirs; tabs running Claude are called so.
    paddock.command(b"w");
    paddock.send(b"k\r");
    paddock.wait_for_tabs(&["1:home", "2:claude", "3:● claude", "4:● claude"]);
}

#[test]
fn events_recorded_while_paddock_is_closed_appear_on_reopen() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    sandbox.hook("space-1", "UserPromptSubmit", &payload("UserPromptSubmit"));
    sandbox.hook(
        "space-1",
        "PermissionRequest",
        &payload("PermissionRequest"),
    );

    let again = ready(&sandbox);
    wait_for_lines(&again, &one_agent("repo", "waiting", "claude"));
}

#[test]
fn claude_agents_corrects_missed_events() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    sandbox.hook("space-1", "UserPromptSubmit", &payload("UserPromptSubmit"));
    wait_for_lines(&paddock, &one_agent("repo", "running", "claude"));

    // Esc fires no Stop; claude still reports the session as idle.
    sandbox.set_agents(
        &json!([{"sessionId": SESSION, "name": "fix-login", "status": "idle", "pid": 1}])
            .to_string(),
    );

    wait_for_lines(&paddock, &one_agent("fix-login", "idle", "claude"));
}

#[test]
fn claude_agents_finds_sessions_no_hook_reported() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    let shell: u32 = sandbox
        .display("space-1", "#{pane_pid}")
        .parse()
        .expect("pane pid");

    sandbox.set_agents(
        &json!([{"sessionId": "found", "cwd": "/w/older", "status": "waiting", "pid": shell}])
            .to_string(),
    );

    wait_for_lines(&paddock, &one_agent("older", "waiting", "claude"));
}

#[test]
fn c_a_w_selects_the_current_space_and_esc_leaves_without_switching() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C");
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo in-two");
    paddock.wait_for_text("in-two\n");

    paddock.command(b"w");
    paddock.wait_for("space-2 selected", |_| paddock.reversed(3, 5));
    paddock.send(b"k");
    // space-1 (screen rows 1-2) is selected.
    paddock.wait_for("space-1 selected", |_| paddock.reversed(1, 5));
    paddock.send(b"\x1b");
    std::thread::sleep(std::time::Duration::from_millis(200));
    paddock.type_line("echo still-on-two");
    paddock.wait_for_text("still-on-two\n");
    assert!(paddock.screen().contains("in-two"));
}

#[test]
fn b_hides_and_shows_the_sidebar() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let next_to_sidebar = harness::COLS - harness::SIDEBAR;
    wait_for_width(&sandbox, next_to_sidebar);

    paddock.command(b"b");
    paddock.wait_for("sidebar hidden", |_| paddock.sidebar().is_empty());
    paddock.wait_for("tab bar at the left edge", |_| {
        paddock.full_screen().starts_with("  home")
    });
    wait_for_width(&sandbox, harness::COLS);

    paddock.command(b"b");
    wait_for_lines(&paddock, &no_agents());
    wait_for_width(&sandbox, next_to_sidebar);
}

/// Two spaces, the second current, and an agent done in space-1's second
/// tab: what the sidebar's agent rows are tested with.
fn agent_in_another_space(sandbox: &Sandbox) -> Paddock {
    let mut paddock = ready(sandbox);
    new_tab(&mut paddock, "2:home");
    paddock.command(b"C");
    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_text(PROMPT);
    sandbox.hook("space-1:1", "Stop", &payload("Stop"));
    paddock.wait_for("done", |_| paddock.full_screen().contains("done"));
    paddock
}

/// Wait until pane `target` shows `text`, as tmux has it.
fn wait_in_pane(sandbox: &Sandbox, target: &str, text: &str) {
    let capture = || sandbox.tmux(&["capture-pane", "-p", "-t", target]);
    let start = std::time::Instant::now();
    while !capture().contains(text) {
        assert!(start.elapsed() < harness::TIMEOUT, "pane:\n{}", capture());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn enter_on_an_agent_jumps_to_its_space_tab_and_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    // C-a s starts on the first agent.
    paddock.command(b"s");
    paddock.send(b"\r");

    paddock.wait_for_tabs(&["1:home", "2:● claude"]);
    paddock.wait_for_current_tab("2:● claude");
    // Looked at now: the done turn is idle.
    paddock.wait_for("idle", |_| paddock.full_screen().contains("idle"));
    paddock.type_line("echo typed-after-the-jump");
    wait_in_pane(&sandbox, "space-1:1", "typed-after-the-jump");
}

#[test]
fn clicking_an_agent_row_jumps_to_its_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    // Rows: 0 SPACES, 1-4 two spaces, 5 blank, 6 AGENTS, 7-8 the agent.
    paddock.click_screen(5, 8);

    paddock.wait_for_current_tab("2:● claude");
    paddock.type_line("echo typed-after-the-click");
    wait_in_pane(&sandbox, "space-1:1", "typed-after-the-click");
}

#[test]
fn x_on_an_agent_row_closes_its_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    paddock.command(b"s");
    paddock.send(b"x");

    paddock.wait_for("its tab closed", |_| {
        sandbox
            .tmux(&["list-windows", "-t", "space-1"])
            .lines()
            .count()
            == 1
    });
    assert_eq!(sandbox.session_names(), ["space-1", "space-2"]);
}

/// Wait until space-1's pane is `cols` wide on the hidden server.
fn wait_for_width(sandbox: &Sandbox, cols: u16) {
    let start = std::time::Instant::now();
    let width = || sandbox.display("space-1", "#{pane_width}");
    while start.elapsed() < harness::TIMEOUT {
        if width() == cols.to_string() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("pane is {} wide, not {cols}", width());
}

#[test]
fn parallel_hooks_never_interleave_records() {
    let sandbox = Sandbox::new();
    let _paddock = ready(&sandbox);
    let big = payload("Stop").replace("Both subagents", &"x".repeat(20_000));
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| sandbox.hook("space-1", "Stop", &big));
        }
    });
    let pid = sandbox.display("space-1", "#{pid}");
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
