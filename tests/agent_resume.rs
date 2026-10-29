//! Ticket 49: a terminal that ran a Gemini CLI or Antigravity CLI session
//! when the native server ended comes back running that session's resume
//! command in its directory, as Claude's do (ticket 48,
//! `tests/claude_resume.rs`): `gemini --resume <id>` and `agy
//! --conversation <id>`. The stub `gemini` and the `agy` put in its place
//! after the server ends only record how they were called. agy reports
//! no end: its session is over once its terminal is back at the shell.
//! Every test here picks the native backend in its config, so it runs on
//! either run of the suite.

mod harness;

use harness::backend::{stop_server, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks");
/// The session id in the Gemini fixtures.
const GEMINI_SESSION: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
/// The conversation id in the agy fixtures.
const AGY_SESSION: &str = "dddddddd-eeee-4fff-8aaa-bbbbbbbbbbbb";
/// How long before the server's close a session may end and still be
/// taken for the same shutdown (`seen::CLOSE_WINDOW`), with some margin.
const CLOSE_WINDOW: Duration = Duration::from_millis(2_500);

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

/// Report `kind`'s hook `event` (fixture `<event>.json`) in `terminal`.
fn hook(sandbox: &Sandbox, terminal: &str, kind: &str, event: &str) {
    let path = format!("{FIXTURES}/{kind}/{event}.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    sandbox.agent_hook(terminal, kind, event, &text);
}

/// Put a stub `name` first on the terminals' `PATH`: each call appends
/// `<directory> <arguments>` to `<name>.log` and exits.
fn recording_stub(sandbox: &Sandbox, name: &str) {
    let log = sandbox.path(&format!("{name}.log"));
    let script = format!("#!/bin/sh\necho \"$(pwd -P) $*\" >> '{}'\n", log.display());
    let path = sandbox.path(&format!("bin/{name}"));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// What the stub `name` was called with so far, one line per call.
fn calls(sandbox: &Sandbox, name: &str) -> Vec<String> {
    let log = std::fs::read_to_string(sandbox.path(&format!("{name}.log"))).unwrap_or_default();
    log.lines().map(str::to_string).collect()
}

/// Wait until the stub `name` was called at least once.
fn wait_for_calls(sandbox: &Sandbox, name: &str) -> Vec<String> {
    let start = Instant::now();
    loop {
        let calls = calls(sandbox, name);
        if !calls.is_empty() {
            return calls;
        }
        assert!(start.elapsed() < harness::TIMEOUT, "{name} never called");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The saved layout's only terminal's agent, once `ready` holds for it.
fn wait_for_agent(
    sandbox: &Sandbox,
    what: &str,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let path = sandbox.state_dir().join("layout.json");
    let start = Instant::now();
    loop {
        let saved = std::fs::read_to_string(&path).unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&saved).unwrap_or_default();
        let agent = &json["spaces"][0]["tabs"][0]["splits"]["terminal"]["agent"];
        if ready(agent) {
            return agent.clone();
        }
        assert!(start.elapsed() < harness::TIMEOUT, "{what}: {saved}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether `agent` is session `session` of `kind`, over or not.
fn is(agent: &serde_json::Value, kind: &str, session: &str, over: bool) -> bool {
    agent["kind"] == kind && agent["session"] == session && (agent["over"] == true) == over
}

/// Run a line in the focused terminal and wait for its output: whatever
/// its shell read before it has run.
fn mark(paddock: &mut Paddock) {
    paddock.type_line("echo MARK''-DONE");
    paddock.wait_for_text("MARK-DONE");
}

/// End the server cleanly.
fn stop(sandbox: &Sandbox, mut paddock: Paddock) {
    stop_server(&sandbox.state_dir());
    assert!(paddock.wait_exit(), "a clean end");
}

/// Put a stand-in `agy` first on the terminals' `PATH`: a copy of
/// paddock, so its process is named `agy` (`agy hook` waits on its
/// input), as in `tests/agy.rs`.
fn stand_in_agy(sandbox: &Sandbox) {
    std::fs::copy(env!("CARGO_BIN_EXE_paddock"), sandbox.path("bin/agy")).unwrap();
}

/// Start the stand-in `agy` in the only terminal and report its session.
fn start_agy(sandbox: &Sandbox, paddock: &mut Paddock) {
    paddock.type_line("agy hook");
    paddock.wait_for("agy to run", |_| {
        sandbox.terminal("space-1").command.as_deref() == Some("agy")
    });
    hook(sandbox, "space-1", "agy", "SessionStart");
}

/// Interrupt the stand-in `agy` in the only terminal: its shell is back
/// at the prompt.
fn quit_agy(sandbox: &Sandbox, paddock: &Paddock) {
    let tty = sandbox.terminal("space-1").tty.expect("the terminal's tty");
    let ps = std::process::Command::new("ps")
        .args(["-t", tty.trim_start_matches("/dev/"), "-o", "pid=,comm="])
        .output()
        .expect("run ps");
    let ps = String::from_utf8_lossy(&ps.stdout);
    let pid = ps
        .lines()
        .find_map(|l| l.trim().strip_suffix(" agy"))
        .unwrap_or_else(|| panic!("no agy on {tty}: {ps}"));
    let status = std::process::Command::new("kill")
        .args(["-INT", pid.trim()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -INT {pid}");
    paddock.wait_for("the shell back", |_| {
        sandbox.terminal("space-1").command.as_deref() != Some("agy")
    });
}

#[test]
fn a_restarted_server_resumes_a_live_gemini_session_in_its_directory() {
    let sandbox = native();
    recording_stub(&sandbox, "gemini");
    let home = real(&sandbox.path("home"));
    let paddock = ready(&sandbox);
    hook(&sandbox, "space-1", "gemini", "SessionStart");
    wait_for_agent(&sandbox, "the session saved", |a| {
        is(a, "gemini", GEMINI_SESSION, false)
    });

    stop(&sandbox, paddock);
    let again = sandbox.launch();

    again.wait_for_text("restored 1 space, resumed 1 session");
    assert_eq!(
        wait_for_calls(&sandbox, "gemini"),
        [format!("{home} --resume {GEMINI_SESSION}")]
    );
    // The stub quit at once: the shell takes the next line.
    again.wait_for_text(PROMPT);
}

#[test]
fn a_restarted_server_resumes_a_running_agy_conversation_in_its_directory() {
    let sandbox = native();
    stand_in_agy(&sandbox);
    let home = real(&sandbox.path("home"));
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock);
    wait_for_agent(&sandbox, "the conversation saved", |a| {
        is(a, "agy", AGY_SESSION, false)
    });

    // agy still runs as the server ends: its conversation is not over.
    stop(&sandbox, paddock);
    let saved = wait_for_agent(&sandbox, "the last save", |a| a.is_object());
    assert!(is(&saved, "agy", AGY_SESSION, false), "{saved}");
    recording_stub(&sandbox, "agy");
    let again = sandbox.launch();

    again.wait_for_text("restored 1 space, resumed 1 session");
    assert_eq!(
        wait_for_calls(&sandbox, "agy"),
        [format!("{home} --conversation {AGY_SESSION}")]
    );
    again.wait_for_text(PROMPT);
}

/// agy reports no end: once its terminal is back at the shell its
/// conversation is over, and the next server gives it a plain shell.
#[test]
fn an_agy_conversation_quit_to_the_shell_is_not_resumed() {
    let sandbox = native();
    stand_in_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock);
    wait_for_agent(&sandbox, "the conversation saved", |a| {
        is(a, "agy", AGY_SESSION, false)
    });

    quit_agy(&sandbox, &paddock);
    wait_for_agent(&sandbox, "the conversation over", |a| {
        is(a, "agy", AGY_SESSION, true)
    });
    // An agy back at the shell just before the server ends may have been
    // stopped by the same reboot: this one quit well before.
    std::thread::sleep(CLOSE_WINDOW);
    stop(&sandbox, paddock);
    recording_stub(&sandbox, "agy");
    let mut again = sandbox.launch();

    again.wait_for_text("restored 1 space");
    again.wait_for_text(PROMPT);
    mark(&mut again);
    assert!(!again.full_screen().contains("resumed"));
    assert_eq!(
        calls(&sandbox, "agy"),
        Vec::<String>::new(),
        "agy never run"
    );
}

/// A Gemini session started under an id of the user's own
/// (`gemini --session-id <id>`), not a UUID, is not typed into a shell:
/// its terminal gets a plain shell, and the notice names it.
#[test]
fn a_live_session_whose_id_cannot_be_typed_gets_a_shell_and_is_named() {
    let sandbox = native();
    recording_stub(&sandbox, "gemini");
    let paddock = ready(&sandbox);
    let path = format!("{FIXTURES}/gemini/SessionStart.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let text = text.replace(GEMINI_SESSION, "my-tag; echo oops");
    sandbox.agent_hook("space-1", "gemini", "SessionStart", &text);
    wait_for_agent(&sandbox, "the session saved", |a| {
        is(a, "gemini", "my-tag; echo oops", false)
    });

    stop(&sandbox, paddock);
    let mut again = sandbox.launch();

    again.wait_for_text("restored 1 space, could not resume 1 gemini session");
    again.wait_for_text(PROMPT);
    mark(&mut again);
    assert!(!again.full_screen().contains("oops"));
    assert_eq!(calls(&sandbox, "gemini"), Vec::<String>::new(), "never run");
}
