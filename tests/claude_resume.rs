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
/// The fixtures' directory.
const FIXTURE_CWD: &str = "\"/tmp/repo\"";
const LIVE: &str = "0000aaaa-0000-4000-8000-00000000000a";
const ENDED: &str = "0000bbbb-0000-4000-8000-00000000000b";
const OTHER: &str = "0000cccc-0000-4000-8000-00000000000c";

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

/// Report that Claude session `session` in the terminal `terminal` names
/// ended for `reason`: `prompt_input_exit` (its user quit it) or `other`
/// (the fixture's, as a signal gives).
fn end(sandbox: &Sandbox, terminal: &str, session: &str, reason: &str) {
    let ended = payload("SessionEnd", session).replace("\"other\"", &format!("\"{reason}\""));
    sandbox.hook(terminal, "SessionEnd", &ended);
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

/// Whether `saved` holds session `session`, over or not.
fn saved_as(saved: &str, session: &str, over: bool) -> bool {
    saved_agents(saved)
        .iter()
        .any(|a| a["session"] == session && (a["over"] == true) == over)
}

/// Run a line in the focused terminal and wait for its output: whatever
/// its shell read before it has run.
fn mark(paddock: &mut Paddock) {
    paddock.type_line("echo MARK''-DONE");
    paddock.wait_for_text("MARK-DONE");
}

/// What the stub `claude` was called with so far.
fn claude_log(sandbox: &Sandbox) -> String {
    std::fs::read_to_string(sandbox.path("claude.log")).unwrap_or_default()
}

/// How many agents the sidebar lists (each with its kind beside its
/// place).
fn agents_shown(paddock: &Paddock) -> usize {
    let sidebar = paddock.sidebar();
    sidebar.iter().filter(|l| l.ends_with("claude")).count()
}

/// The server ends cleanly and a new paddock starts the next one.
fn restart(sandbox: &Sandbox, mut paddock: Paddock) -> Paddock {
    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    sandbox.launch()
}

/// One tab split in two: the second split in `dir` runs session [`LIVE`]
/// (reporting `dir` as its directory), the first ran [`ENDED`], which
/// its user quit. Returns once both are saved.
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
    let cwd = format!("\"{real}\"");
    for event in ["SessionStart", "UserPromptSubmit"] {
        let p = payload(event, LIVE).replace(FIXTURE_CWD, &cwd);
        sandbox.hook("space-1:0.1", event, &p);
    }
    claude(sandbox, "space-1:0.0", "SessionStart", ENDED);
    end(sandbox, "space-1:0.0", ENDED, "prompt_input_exit");
    wait_for_saved(sandbox, "both sessions saved", |s| {
        saved_as(s, LIVE, false) && saved_as(s, ENDED, true)
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
    assert_eq!(agents[0]["over"], true, "{saved}");
    assert_eq!(agents[1]["session"], LIVE, "{saved}");
    assert_eq!(agents[1]["kind"], "claude", "{saved}");
    assert_ne!(agents[1]["over"], true, "{saved}");

    let mut again = restart(&sandbox, paddock);

    again.wait_for_text("restored 1 space, resumed 1 session");
    sandbox.claude_calls();
    // Every resume is typed by now: a line typed into the ended
    // session's terminal shows it ran none there.
    again.command(b"h");
    again.wait_for("the focus on the first split", |_| {
        sandbox.terminal("space-1:0.0").focused
    });
    mark(&mut again);
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

    let mut again = restart(&sandbox, paddock);
    again.wait_for_text("resumed 1 session");
    sandbox.claude_calls();
    // The resumed Claude starts in its new terminal with the same id.
    let cwd = format!("\"{}\"", real(&dir));
    let resume = payload("SessionStart", LIVE)
        .replace("startup", "resume")
        .replace(FIXTURE_CWD, &cwd);
    sandbox.hook("space-1:0.1", "SessionStart", &resume);
    let prompt = payload("UserPromptSubmit", LIVE).replace(FIXTURE_CWD, &cwd);
    sandbox.hook("space-1:0.1", "UserPromptSubmit", &prompt);

    again.wait_for("the resumed agent", |_| agents_shown(&again) > 0);
    // Another session's event, after the resumed one's: once it shows,
    // a second row for the resumed session would have too.
    let other = payload("UserPromptSubmit", OTHER).replace(FIXTURE_CWD, "\"/w/elsewhere\"");
    sandbox.hook("space-1:0.0", "UserPromptSubmit", &other);
    again.wait_for("the other agent", |_| agents_shown(&again) > 1);
    assert_eq!(agents_shown(&again), 2, "{}", again.full_screen());
    again.command(b"o");
    again.wait_for_text("j down  k up");
    again.wait_for_text("AGENT");
    again.wait_for("both rows", |s| {
        s.lines()
            .filter(|l| l.trim_end().ends_with(" claude"))
            .count()
            == 2
    });
    // The resumed session's row is the one in its directory.
    let screen = again.screen();
    let rows: Vec<&str> = screen
        .lines()
        .filter(|l| l.trim_end().ends_with(" claude"))
        .collect();
    let resumed = rows.iter().filter(|l| l.contains(" proj ")).count();
    let other = rows.iter().filter(|l| l.contains(" elsewhere ")).count();
    assert_eq!((resumed, other), (1, 1), "one row each: {screen}");
}

/// A terminal whose session its user quit gets a fresh shell, and the
/// notice names no resumed session.
#[test]
fn a_restart_with_no_live_session_resumes_nothing() {
    let sandbox = native();
    let paddock = ready(&sandbox);
    claude(&sandbox, "space-1:0.0", "SessionStart", ENDED);
    end(&sandbox, "space-1:0.0", ENDED, "prompt_input_exit");
    wait_for_saved(&sandbox, "the ended session", |s| saved_as(s, ENDED, true));

    let mut again = restart(&sandbox, paddock);

    again.wait_for_text("restored 1 space");
    again.wait_for_text(PROMPT);
    assert!(!again.full_screen().contains("resumed"));
    mark(&mut again);
    assert_eq!(claude_log(&sandbox), "", "claude never run");
}

/// A reboot signals Claude as it does the server: an end from outside
/// (reason `other`) logged just before the server stops, and already
/// saved as over, does not stop the session from being resumed.
#[test]
fn a_session_ended_from_outside_as_the_server_stops_is_resumed() {
    let sandbox = native();
    let paddock = ready(&sandbox);
    let dir = real(&sandbox.path("home"));
    claude(&sandbox, "space-1:0.0", "SessionStart", LIVE);
    wait_for_saved(&sandbox, "the session", |s| saved_as(s, LIVE, false));
    end(&sandbox, "space-1:0.0", LIVE, "other");
    wait_for_saved(&sandbox, "the end saved", |s| saved_as(s, LIVE, true));

    let again = restart(&sandbox, paddock);

    again.wait_for_text("restored 1 space, resumed 1 session");
    assert_eq!(sandbox.claude_calls(), [format!("{dir} --resume {LIVE}")]);
    again.wait_for_text(PROMPT);
}

/// A terminal whose directory is gone starts in `$HOME`, where Claude
/// would not find the session: it is not resumed.
#[test]
fn a_session_whose_directory_is_gone_is_not_resumed() {
    let sandbox = native();
    let dir = sandbox.path("home/gone");
    std::fs::create_dir_all(&dir).unwrap();
    let mut paddock = ready(&sandbox);
    paddock.type_line(&format!("cd '{}'", dir.display()));
    let real_dir = real(&dir);
    paddock.wait_for("the cd", |_| {
        sandbox.terminal("space-1").dir.as_deref() == Some(real_dir.as_str())
    });
    claude(&sandbox, "space-1:0.0", "SessionStart", LIVE);
    wait_for_saved(&sandbox, "the session", |s| {
        saved_as(s, LIVE, false) && s.contains("/gone\"")
    });

    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
    std::fs::remove_dir(&dir).unwrap();
    let mut again = sandbox.launch();

    again.wait_for_text("restored 1 space");
    again.wait_for_text(PROMPT);
    assert!(!again.full_screen().contains("resumed"));
    mark(&mut again);
    assert_eq!(claude_log(&sandbox), "", "claude never run");
}
