//! Ticket 48: a terminal that ran a Claude session when the native server
//! ended comes back running `claude --resume <id>` in its directory, and
//! the dashboard shows it as the same session. The stub `claude` only
//! records how it was called. Every test here picks the native backend in
//! its config, so it runs on either run of the suite.

mod harness;

use harness::backend::{stop_server, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::path::Path;
use std::time::{Duration, Instant};

const SPIKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/spikes/02-subagent-worktree/fixtures"
);
const SESSION: &str = "11111111-2222-4333-8444-555555555555";
const LIVE: &str = "0000aaaa-0000-4000-8000-00000000000a";
const ENDED: &str = "0000bbbb-0000-4000-8000-00000000000b";

fn native() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);
    sandbox
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// `path` as the server reads a directory: symlinks resolved.
fn real(path: &Path) -> String {
    let real = std::fs::canonicalize(path).unwrap();
    real.to_string_lossy().into_owned()
}

/// The recorded payload of `event` for Claude session `session`.
fn payload(event: &str, session: &str) -> String {
    let path = format!("{SPIKE}/{event}.json");
    let payload = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    payload.replace(SESSION, session)
}

/// Report `event` for Claude session `session` in the terminal `terminal`
/// names.
fn claude(sandbox: &Sandbox, terminal: &str, event: &str, session: &str) {
    sandbox.hook(terminal, event, &payload(event, session));
}

/// The saved layout, once `ready` holds for its text.
fn wait_for_saved(sandbox: &Sandbox, what: &str, ready: impl Fn(&str) -> bool) -> String {
    let path = sandbox.state_dir().join("layout.json");
    let start = Instant::now();
    loop {
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        if ready(&saved) {
            return saved;
        }
        assert!(start.elapsed() < harness::TIMEOUT, "{what}: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The agent saved for each terminal, in the layout's order.
fn saved_agents(saved: &str) -> Vec<serde_json::Value> {
    fn walk(node: &serde_json::Value, out: &mut Vec<serde_json::Value>) {
        if let Some(t) = node.get("terminal") {
            return out.push(t["agent"].clone());
        }
        let cells = node["split"]["cells"].as_array().into_iter().flatten();
        cells.for_each(|c| walk(&c["node"], out));
    }
    let json: serde_json::Value = serde_json::from_str(saved).unwrap_or_default();
    let mut out = Vec::new();
    for space in json["spaces"].as_array().into_iter().flatten() {
        for tab in space["tabs"].as_array().into_iter().flatten() {
            walk(&tab["splits"], &mut out);
        }
    }
    out
}

/// How many agents the sidebar lists (each with its kind beside its
/// place).
fn agents_shown(paddock: &Paddock) -> usize {
    let sidebar = paddock.sidebar();
    sidebar.iter().filter(|l| l.ends_with("claude")).count()
}

/// One tab split in two: the second split in `dir` runs session [`LIVE`],
/// the first ran [`ENDED`], which has quit. Returns once both are saved.
fn two_sessions(paddock: &mut Paddock, sandbox: &Sandbox, dir: &Path) {
    paddock.command(b"|");
    paddock.wait_for("the split", |_| {
        sandbox.state().spaces[0].tabs[0].splits.len() == 2
    });
    paddock.type_line(&format!("cd '{}'", dir.display()));
    let real = real(dir);
    paddock.wait_for("the cd", |_| {
        sandbox.terminal("space-1:0.1").dir.as_deref() == Some(real.as_str())
    });
    claude(sandbox, "space-1:0.1", "SessionStart", LIVE);
    claude(sandbox, "space-1:0.1", "UserPromptSubmit", LIVE);
    claude(sandbox, "space-1:0.0", "SessionStart", ENDED);
    claude(sandbox, "space-1:0.0", "SessionEnd", ENDED);
    wait_for_saved(sandbox, "both sessions saved", |s| {
        s.contains(LIVE) && s.contains(ENDED)
    });
}

#[test]
fn a_restarted_server_resumes_a_live_claude_session_in_its_directory_and_not_an_ended_one() {
    let sandbox = native();
    let dir = sandbox.path("home/proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut paddock = ready(&sandbox);
    two_sessions(&mut paddock, &sandbox, &dir);
    let saved = std::fs::read_to_string(sandbox.state_dir().join("layout.json")).unwrap();
    let agents = saved_agents(&saved);
    assert_eq!(agents.len(), 2, "{saved}");
    assert_eq!(agents[0]["session"], ENDED, "{saved}");
    assert_eq!(agents[0]["ended"], true, "{saved}");
    assert_eq!(agents[1]["session"], LIVE, "{saved}");
    assert_eq!(agents[1]["kind"], "claude", "{saved}");
    assert_ne!(agents[1]["ended"], true, "{saved}");

    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    let again = sandbox.launch();

    again.wait_for_text("restored 1 space, resumed 1 session");
    let calls = sandbox.claude_calls();
    assert_eq!(calls, [format!("{} --resume {LIVE}", real(&dir))]);
    // Before the resumed Claude reports anything, a later save still
    // names its session.
    wait_for_saved(&sandbox, "the session kept", |s| {
        let agents = saved_agents(s);
        agents.len() == 2 && agents[1]["session"] == LIVE
    });
    // The stub quit at once: the shell takes the next line.
    again.wait_for_text(PROMPT);
}

#[test]
fn the_resumed_session_is_listed_once_as_the_same_session() {
    let sandbox = native();
    let dir = sandbox.path("home/proj");
    std::fs::create_dir_all(&dir).unwrap();
    let mut paddock = ready(&sandbox);
    two_sessions(&mut paddock, &sandbox, &dir);
    paddock.wait_for("the agent", |_| agents_shown(&paddock) == 1);

    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    let mut again = sandbox.launch();
    again.wait_for_text("resumed 1 session");
    sandbox.claude_calls();
    // The resumed Claude starts in its new terminal with the same id.
    let resume = payload("SessionStart", LIVE).replace("startup", "resume");
    sandbox.hook("space-1:0.1", "SessionStart", &resume);
    claude(&sandbox, "space-1:0.1", "UserPromptSubmit", LIVE);

    again.wait_for("the resumed agent", |_| agents_shown(&again) > 0);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(agents_shown(&again), 1, "{}", again.full_screen());
    again.command(b"o");
    again.wait_for_text("j down  k up");
    again.wait_for_text("AGENT");
    let screen = again.screen();
    let rows = screen.lines().filter(|l| l.trim_end().ends_with(" claude"));
    assert_eq!(rows.count(), 1, "one row: {screen}");
}

/// A terminal whose session ended gets a fresh shell, and the notice
/// names no resumed session.
#[test]
fn a_restart_with_no_live_session_resumes_nothing() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    claude(&sandbox, "space-1:0.0", "SessionStart", ENDED);
    claude(&sandbox, "space-1:0.0", "SessionEnd", ENDED);
    wait_for_saved(&sandbox, "the ended session", |s| s.contains(ENDED));

    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    let again = sandbox.launch();

    again.wait_for_text("restored 1 space");
    again.wait_for_text(PROMPT);
    assert!(!again.full_screen().contains("resumed"));
    std::thread::sleep(Duration::from_millis(300));
    let log = std::fs::read_to_string(sandbox.path("claude.log")).unwrap_or_default();
    assert_eq!(log, "", "claude never run");
}
