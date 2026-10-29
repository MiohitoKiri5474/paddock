//! Ticket 06: Claude sessions, driven by recorded hook payloads, as the
//! sidebar's spaces and agents and the tabs holding them show them.
//!
//! Payloads come from spike 02's recorded fixtures. The waiting payloads in
//! `tests/fixtures/hooks/` were not recorded (spike 02 ran with
//! bypassPermissions, so none fired); they follow Claude Code's documented
//! hook input schema.

mod harness;

use harness::{Paddock, Sandbox, CURRENT_ITEM, PROMPT};
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

/// AGENTS' lines for these `(label, state)` Claude agents, all in
/// space-1: two lines each, labels and states in columns, then the space
/// and kind under the label.
fn agents(list: &[(&str, &str)]) -> Vec<String> {
    let label = list.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    list.iter()
        .flat_map(|(l, s)| {
            [
                format!(" ● {l:<label$}  {s}"),
                "   space-1  claude".to_string(),
            ]
        })
        .collect()
}

/// The sidebar with space-1 and one agent in it.
fn one_agent(label: &str, state: &str) -> Vec<String> {
    let mut lines = vec![space("space-1", true), at_home(), " AGENTS".into()];
    lines.extend(agents(&[(label, state)]));
    lines
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
    ];
    for (name, state) in steps {
        let event = name.split('-').next().unwrap();
        let out = sandbox.hook("space-1", event, &payload(name));
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
        wait_for_lines(&paddock, &one_agent("repo", state));
    }
    // Claude quit: its agent leaves the sidebar.
    let out = sandbox.hook("space-1", "SessionEnd", &payload("SessionEnd"));
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    wait_for_lines(&paddock, &no_agents());
}

/// What a case is called, the hook's extra environment, and its event.
type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);

/// tmux-only: the variables a hook outside paddock's terminals may see are
/// tmux's (another server's `TMUX`, or paddock's without `TMUX_PANE`).
#[test]
fn tmux_only_hooks_outside_paddock_say_nothing_and_go_to_the_external_log() {
    let sandbox = Sandbox::new();
    let _paddock = ready(&sandbox);
    let pid = sandbox.state().instance.expect("the instance id");
    let foreign = format!("/tmp/tmux-501/default,{pid},0");
    let ours = sandbox.terminal("space-1").hook_env["TMUX"].clone();
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
    // External sessions: none in the server's log, each in the external one.
    let events = sandbox.path("state/paddock/events");
    assert!(!events.join(format!("{pid}.jsonl")).exists());
    let external = std::fs::read_to_string(events.join("external.jsonl")).unwrap();
    let recorded: Vec<Value> = external
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(recorded.len(), cases.len(), "{external}");
    for r in &recorded {
        assert_eq!(r["session_id"], SESSION);
        assert!(r.get("pane").is_none(), "{r}");
    }

    // A broken payload in a paddock terminal is still silent and still exits 0.
    let out = sandbox.hook("space-1", "SessionStart", "{not json");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
}

#[test]
fn agents_of_every_space_are_listed_with_their_space() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    new_tab(&mut paddock, "2:home");
    new_tab(&mut paddock, "3:home");
    new_tab(&mut paddock, "4:home");
    paddock.command(b"C\r");
    paddock.wait_for_tabs(&["1:home"]);

    let start = |t: &str, s: &str, dir: &str, event: &str| {
        sandbox.hook(t, event, &payload_for(event, s, dir));
    };
    start("space-1:0", "s1", "/w/stopped", "SessionEnd");
    start("space-1:1", "s2", "/w/running", "UserPromptSubmit");
    start("space-1:2", "s3", "/w/idle", "Stop");
    start("space-1:3", "s4", "/w/waiting", "PermissionRequest");

    // In tab order, never by state; the turn that ended out of sight is
    // done; the session that ended is not listed.
    let mut want = vec![
        space("space-1", true),
        at_home(),
        space("space-2", false),
        at_home(),
        " AGENTS".into(),
    ];
    want.extend(agents(&[
        ("running", "running"),
        ("idle", "done"),
        ("waiting", "waiting"),
    ]));
    wait_for_lines(&paddock, &want);
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
    wait_for_lines(&again, &one_agent("repo", "waiting"));
}

#[test]
fn claude_agents_corrects_missed_events() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    sandbox.hook("space-1", "UserPromptSubmit", &payload("UserPromptSubmit"));
    wait_for_lines(&paddock, &one_agent("repo", "running"));

    // Esc fires no Stop; claude still reports the session as idle.
    sandbox.set_agents(
        &json!([{"sessionId": SESSION, "name": "fix-login", "status": "idle", "pid": 1}])
            .to_string(),
    );

    wait_for_lines(&paddock, &one_agent("fix-login", "idle"));
}

#[test]
fn claude_agents_finds_sessions_no_hook_reported() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    let shell = sandbox.terminal("space-1").pid.expect("the shell's pid");

    sandbox.set_agents(
        &json!([{"sessionId": "found", "cwd": "/w/older", "status": "waiting", "pid": shell}])
            .to_string(),
    );

    wait_for_lines(&paddock, &one_agent("older", "waiting"));
}

#[test]
fn c_a_w_selects_the_current_space_and_esc_leaves_without_switching() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
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
    paddock.command(b"C\r");
    paddock.wait_for_tabs(&["1:home"]);
    paddock.wait_for_text(PROMPT);
    sandbox.hook("space-1:1", "Stop", &payload("Stop"));
    paddock.wait_for("done", |_| paddock.full_screen().contains("done"));
    paddock
}

/// Wait until the terminal shown prints `text`, and check it is space-1's
/// tab `tab` (from 0), current there as the backend has it.
fn wait_in_tab(paddock: &Paddock, sandbox: &Sandbox, tab: usize, text: &str) {
    paddock.wait_for_text(&format!("{text}\n"));
    assert_eq!(sandbox.space("space-1").current, tab);
}

#[test]
fn enter_on_an_agent_jumps_to_its_space_tab_and_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    // C-a s starts on the current space (no agent is focused); one down
    // is the agent.
    paddock.command(b"s");
    paddock.send(b"j\r");

    paddock.wait_for_tabs(&["1:home", "2:● claude"]);
    paddock.wait_for_current_tab("2:● claude");
    // Looked at now: the done turn is idle.
    paddock.wait_for("idle", |_| paddock.full_screen().contains("idle"));
    paddock.type_line("echo typed-after-the-jump");
    wait_in_tab(&paddock, &sandbox, 1, "typed-after-the-jump");
}

#[test]
fn c_a_s_starts_on_the_current_space_when_no_agent_is_focused() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    paddock.command(b"s");

    // Rows: 1-2 space-1, 3-4 space-2 (current), 7 the agent.
    paddock.wait_for("space-2 selected", |_| paddock.reversed(3, 5));
    assert!(!paddock.reversed(1, 5), "not the top item");
    assert!(!paddock.reversed(7, 5), "not the agent");
}

/// One space with two agents: one waiting in tab 1, and one idle in tab 2,
/// the current tab, so the agent focused is not the first in AGENTS.
fn focused_agent_second(sandbox: &Sandbox) -> Paddock {
    let mut paddock = ready(sandbox);
    new_tab(&mut paddock, "2:home");
    sandbox.hook(
        "space-1:0",
        "PermissionRequest",
        &payload_for("PermissionRequest", "s1", "/w/first"),
    );
    sandbox.hook("space-1:1", "Stop", &payload_for("Stop", "s2", "/w/here"));
    let mut want = vec![space("space-1", true), at_home(), " AGENTS".into()];
    want.extend(agents(&[("first", "waiting"), ("here", "idle")]));
    wait_for_lines(&paddock, &want);
    paddock
}

#[test]
fn the_focused_agent_has_the_current_background() {
    let sandbox = Sandbox::new();
    let paddock = focused_agent_second(&sandbox);

    // Rows: 1-2 space-1, 4 AGENTS, 5-6 first, 7-8 here.
    paddock.wait_for("here highlighted", |_| {
        paddock.colors(7, 5).1 == CURRENT_ITEM
    });
    assert_eq!(paddock.colors(7, 20).1, CURRENT_ITEM, "across its line");
    assert_eq!(paddock.colors(8, 20).1, CURRENT_ITEM, "and its second");
    assert_eq!(
        paddock.colors(9, 5).1,
        vt100::Color::Default,
        "not the line below"
    );
    assert_eq!(paddock.colors(6, 5).1, vt100::Color::Default, "not first");
}

#[test]
fn c_a_s_starts_on_the_focused_agent_even_with_the_sidebar_hidden() {
    let sandbox = Sandbox::new();
    let mut paddock = focused_agent_second(&sandbox);

    paddock.command(b"s");
    paddock.wait_for("here selected", |_| {
        paddock.reversed(7, 5) && paddock.reversed(8, 5)
    });
    assert!(!paddock.reversed(5, 5), "not the first agent");
    assert!(!paddock.reversed(1, 5), "not the top item");

    // Prefix commands work in the sidebar: hiding it gives the keyboard back.
    paddock.command(b"b");
    paddock.wait_for("sidebar hidden", |_| paddock.sidebar().is_empty());
    paddock.command(b"s");
    paddock.wait_for("shown, here selected", |_| {
        !paddock.sidebar().is_empty() && paddock.reversed(7, 5)
    });
    assert!(!paddock.reversed(5, 5) && !paddock.reversed(1, 5));
}

#[test]
fn clicking_an_agent_row_jumps_to_its_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    // Rows: 0 SPACES, 1-4 two spaces, 5 blank, 6 AGENTS, 7-8 the agent:
    // a click on its second line picks it too.
    paddock.click_screen(5, 8);

    paddock.wait_for_current_tab("2:● claude");
    paddock.type_line("echo typed-after-the-click");
    wait_in_tab(&paddock, &sandbox, 1, "typed-after-the-click");
}

#[test]
fn x_on_an_agent_row_closes_its_pane() {
    let sandbox = Sandbox::new();
    let mut paddock = agent_in_another_space(&sandbox);

    paddock.command(b"s");
    paddock.send(b"jx");

    paddock.wait_for("its tab closed", |_| {
        sandbox.space("space-1").tabs.len() == 1
    });
    assert_eq!(sandbox.space_names(), ["space-1", "space-2"]);
}

/// Wait until space-1's terminal is `cols` wide in the backend.
fn wait_for_width(sandbox: &Sandbox, cols: u16) {
    let start = std::time::Instant::now();
    let width = || sandbox.terminal("space-1").width;
    while start.elapsed() < harness::TIMEOUT {
        if width() == cols {
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
    let log = sandbox.state().event_log.expect("the backend's event log");
    let log = std::fs::read_to_string(log).expect("event log");
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

#[test]
fn braces_resize_the_sidebar_and_the_width_is_kept_on_reopen() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    let cols = |sidebar: u16| harness::COLS - sidebar;
    wait_for_width(&sandbox, cols(32));

    paddock.command(b"}");
    wait_for_width(&sandbox, cols(34));
    paddock.command(b"{");
    paddock.command(b"{");
    wait_for_width(&sandbox, cols(30));
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let _again = ready(&sandbox);
    wait_for_width(&sandbox, cols(30));
}

#[test]
fn the_sidebar_never_gets_narrower_than_sixteen_or_wider_than_half() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    for _ in 0..12 {
        paddock.command(b"{");
    }
    wait_for_width(&sandbox, harness::COLS - 16);
    for _ in 0..30 {
        paddock.command(b"}");
    }
    wait_for_width(&sandbox, harness::COLS / 2);
}

#[test]
fn dragging_the_border_resizes_the_sidebar() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r"); // a second space, so a stray click would switch
    paddock.wait_for_text(PROMPT);
    wait_for_width(&sandbox, harness::COLS - 32);

    // The border is column 31; on row 3 the sidebar shows space-2.
    paddock.drag_screen(3, 31, 39);

    wait_for_width(&sandbox, harness::COLS - 40);
    paddock.type_line("echo still-in-two");
    paddock.wait_for_text("still-in-two\n");
    assert_eq!(sandbox.terminal("space-2").width, 60);
}

#[test]
fn the_config_sets_the_starting_width() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[sidebar]\nwidth = 40\n");
    let _paddock = ready(&sandbox);

    wait_for_width(&sandbox, harness::COLS - 40);
}

#[test]
fn a_click_on_the_border_without_a_drag_is_a_click_on_the_row() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo in-two");
    paddock.wait_for_text("in-two\n");

    // The border is column 31; row 1 is space-1's line.
    paddock.click_screen(31, 1);

    paddock.wait_for("space-1 shown", |s| !s.contains("in-two"));
    wait_for_width(&sandbox, harness::COLS - 32);
    let layout =
        std::fs::read_to_string(sandbox.path("state/paddock/layout.toml")).unwrap_or_default();
    assert!(!layout.contains("sidebar"), "nothing resized: {layout}");
}

#[test]
fn a_wide_sidebar_is_remembered_as_asked_even_on_a_narrow_screen() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    for _ in 0..4 {
        paddock.command(b"}");
    }
    wait_for_width(&sandbox, harness::COLS - 40);
    paddock.resize(60, 20);
    wait_for_width(&sandbox, 30);
    paddock.command(b"?"); // any command saves the state
    paddock.wait_for("help", |_| paddock.full_screen().contains("paddock keys"));

    let layout = std::fs::read_to_string(sandbox.path("state/paddock/layout.toml")).unwrap();
    assert!(layout.contains("sidebar = 40"), "{layout}");
}

#[test]
fn a_drag_on_the_border_that_does_not_move_it_saves_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    wait_for_width(&sandbox, harness::COLS - 32);

    // Press, drag and release on the border column itself (31).
    paddock.drag_screen(10, 31, 31);
    paddock.command(b"?"); // a command saves the state
    paddock.wait_for("help", |_| paddock.full_screen().contains("paddock keys"));

    let layout = std::fs::read_to_string(sandbox.path("state/paddock/layout.toml")).unwrap();
    assert!(!layout.contains("sidebar"), "{layout}");
}
