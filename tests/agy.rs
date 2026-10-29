//! Ticket 35: Antigravity CLI (`agy`) sessions. Its hooks call `paddock
//! hook --agent agy <event>` and get `{}` back; in paddock's terminals its
//! status reports (OSC 7501) say when it works, waits or is done, and the
//! session ends when the terminal goes back to the shell or closes.
//! Outside them, an external session lives on its hooks alone. `paddock
//! hooks install` writes paddock's entry in the sandbox's
//! `~/.gemini/config/hooks.json` when `agy` is on `PATH`, or when named.
//!
//! The payloads in `tests/fixtures/hooks/agy/` are shaped from agy's hook
//! docs, not recorded from a live run. The `agy` in the terminals is a
//! copy of paddock under that name (`agy hook` waits on its input), so
//! the backend sees `agy` run; its status reports are written to the
//! terminal as agy 1.4.3 wrote them in a real run.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks/agy");
const WAITING: &str = "repo is waiting";
const IDLE: &str = "repo is idle";

/// Run agy's hooks for fixtures `names` (`<event>[-<variant>]`), in
/// order, in `terminal`.
fn agy(sandbox: &Sandbox, terminal: &str, names: &[&str]) {
    for name in names {
        let path = format!("{FIXTURES}/{name}.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let event = name.split('-').next().unwrap();
        sandbox.agent_hook(terminal, "agy", event, &text);
    }
}

/// agy's status report `state` (and its `kind`), as agy writes it.
fn status(state: &str, kind: Option<&str>) -> Vec<u8> {
    let kind = kind.map(|k| format!(":kind={k}")).unwrap_or_default();
    format!("\x1b]7501;state={state}{kind}:app=antigravity\x07").into_bytes()
}

/// Write `bytes` to `terminal` as the program in it would: to its tty.
fn write_to(sandbox: &Sandbox, terminal: &str, bytes: &[u8]) {
    let tty = sandbox.terminal(terminal).tty.expect("the terminal's tty");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(&tty)
        .unwrap_or_else(|e| panic!("{tty}: {e}"));
    f.write_all(bytes).unwrap();
}

/// Put a stand-in `agy` in the sandbox's `bin` (first on the terminals'
/// `PATH`): a program under that name, so tmux names it `agy`. A copy of
/// paddock, as macOS kills a copy of its own `sleep`, and a link would be
/// named after its target.
fn stub_agy(sandbox: &Sandbox) {
    sandbox.copy_paddock("bin/agy");
}

/// Start the stand-in `agy` in the focused terminal, `terminal`, and wait
/// until the backend sees it run there.
fn start_agy(sandbox: &Sandbox, paddock: &mut Paddock, terminal: &str) {
    // `paddock hook` reads its input to the end first, so this waits.
    paddock.type_line("agy hook");
    paddock.wait_for("agy to run", |_| {
        sandbox.terminal(terminal).command.as_deref() == Some("agy")
    });
}

/// Interrupt the stand-in `agy` in `terminal`, as `C-c` there would: its
/// shell is back at the prompt.
fn quit_agy(sandbox: &Sandbox, terminal: &str) {
    let tty = sandbox.terminal(terminal).tty.expect("the terminal's tty");
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
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Whether some row of the whole screen holds every one of `parts`.
fn row_with(paddock: &Paddock, parts: &[&str]) -> bool {
    paddock
        .full_rows()
        .iter()
        .any(|l| parts.iter().all(|p| l.contains(p)))
}

fn wait_row(paddock: &Paddock, what: &str, parts: &[&str]) {
    paddock.wait_for(what, |_| row_with(paddock, parts));
}

#[test]
fn an_agy_session_goes_idle_working_waiting_done_then_ends_at_the_shell() {
    let sandbox = Sandbox::new();
    stub_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    // agy runs in the first tab; the second is the one looked at, so its
    // alerts notify.
    start_agy(&sandbox, &mut paddock, "space-1:0");
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);

    agy(&sandbox, "space-1:0", &["SessionStart"]);
    // The sidebar names the agent's space; its kind is the dashboard's.
    wait_row(&paddock, "idle at start", &["● repo", "idle", "space-1"]);

    agy(&sandbox, "space-1:0", &["PreInvocation"]);
    wait_row(&paddock, "working", &["● repo", "running"]);
    write_to(&sandbox, "space-1:0", &status("working", None));
    write_to(
        &sandbox,
        "space-1:0",
        &status("blocked", Some("permission")),
    );
    wait_row(&paddock, "waiting", &["● repo", "waiting"]);
    assert_eq!(sandbox.wait_for_notifications(1), [WAITING]);
    write_to(&sandbox, "space-1:0", &status("working", None));
    wait_row(&paddock, "working again", &["● repo", "running"]);
    // The turn ends twice over: agy's report, then its Stop hook.
    write_to(&sandbox, "space-1:0", &status("done", None));
    wait_row(&paddock, "done", &["● repo", "done"]);
    agy(&sandbox, "space-1:0", &["Stop"]);
    assert_eq!(
        sandbox.wait_for_notifications(2),
        [WAITING, IDLE],
        "one alert per turn"
    );
    // The reports are in the server's log, with the hooks.
    let events = sandbox.path("state/paddock/events");
    let log: String = std::fs::read_dir(&events)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().is_some_and(|n| n != "external.jsonl"))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    assert_eq!(log.matches(r#""event":"OSC7501""#).count(), 4, "{log}");
    assert_eq!(log.matches(r#""kind":"agy""#).count(), 7, "{log}");

    paddock.command(b"o");
    paddock.wait_for_text("AGENT");
    paddock.wait_for("the dashboard's row", |s| {
        s.lines()
            .any(|l| l.contains(" repo ") && l.contains(" done ") && l.trim_end().ends_with(" agy"))
    });
    paddock.send(b"\x1b");

    // agy quits: the terminal is back at the shell, and agy's last report
    // does not keep the session.
    write_to(&sandbox, "space-1:0", &status("idle", None));
    quit_agy(&sandbox, "space-1:0");
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
}

#[test]
fn an_agy_session_ends_when_its_terminal_closes() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    agy(&sandbox, "space-1:1", &["SessionStart", "PreInvocation"]);
    wait_row(&paddock, "working", &["● repo", "running"]);
    sandbox.end_terminal("space-1:1");
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
}

#[test]
fn reports_from_other_programs_or_without_a_session_change_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    stub_agy(&sandbox);
    start_agy(&sandbox, &mut paddock, "space-1:0");
    // The second tab is the one looked at, so the first one's alerts would
    // notify.
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    // An agy session that ended: agy started again reports its sign-in
    // screen before any hook, which neither brings it back nor alerts.
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    quit_agy(&sandbox, "space-1:0");
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
    write_to(&sandbox, "space-1:0", &status("blocked", Some("auth")));
    // More than the 1 s a report may come before its session starts.
    std::thread::sleep(std::time::Duration::from_millis(1_500));
    assert!(sandbox.notifications().is_empty());
    assert!(!paddock.full_screen().contains("● repo"));
    // agy started again on the same conversation (a resume), typed in the
    // first tab, then back on the second.
    paddock.command(b"1");
    paddock.wait_for_current_tab("1:home");
    start_agy(&sandbox, &mut paddock, "space-1:0");
    paddock.command(b"2");
    paddock.wait_for_current_tab("2:home");
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    // Another program's report says nothing of agy's session.
    write_to(
        &sandbox,
        "space-1:0",
        b"\x1b]7501;state=blocked:kind=auth:app=brew\x07",
    );
    agy(&sandbox, "space-1:0", &["Stop-busy"]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(row_with(&paddock, &["● repo", "idle"]), "still idle");
    assert!(sandbox.notifications().is_empty());
}

/// Every line of the sandbox's server hook logs (not the external one).
fn server_log(sandbox: &Sandbox) -> String {
    std::fs::read_dir(sandbox.path("state/paddock/events"))
        .map(|dir| {
            dir.map(|e| e.unwrap().path())
                .filter(|p| p.file_name().is_some_and(|n| n != "external.jsonl"))
                .map(|p| std::fs::read_to_string(p).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_same_report_after_a_hook_shows_again() {
    let sandbox = Sandbox::new();
    stub_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock, "space-1:0");
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    write_to(
        &sandbox,
        "space-1:0",
        &status("blocked", Some("permission")),
    );
    wait_row(&paddock, "waiting", &["● repo", "waiting"]);
    agy(&sandbox, "space-1:0", &["PreInvocation"]);
    wait_row(&paddock, "running", &["● repo", "running"]);
    write_to(
        &sandbox,
        "space-1:0",
        &status("blocked", Some("permission")),
    );
    wait_row(&paddock, "waiting again", &["● repo", "waiting"]);
}

#[test]
fn a_report_right_after_the_start_hook_waits_for_its_session() {
    let sandbox = Sandbox::new();
    stub_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock, "space-1:0");
    // The hook is in the log, but paddock has not read it yet when the
    // report comes.
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    write_to(&sandbox, "space-1:0", &status("blocked", Some("auth")));
    wait_row(&paddock, "waiting", &["● repo", "waiting"]);
    assert_eq!(server_log(&sandbox).matches("OSC7501").count(), 1);
}

/// The state on the session row of `repo`, if there is one.
fn repo_state(paddock: &Paddock) -> Option<&'static str> {
    let rows = paddock.full_rows();
    let row = rows.iter().find(|l| l.contains("● repo"))?;
    ["waiting", "done", "idle", "running"]
        .into_iter()
        .find(|s| row.contains(s))
}

/// The `ts` of the first record of `event` in the server log.
fn logged_ts(sandbox: &Sandbox, event: &str) -> u64 {
    server_log(sandbox)
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|v| v["event"] == event)
        .and_then(|v| v["ts"].as_u64())
        .unwrap_or_else(|| panic!("no {event} in {}", server_log(sandbox)))
}

/// Detach, open paddock again on the same log, and wait until the
/// replayed row of `repo` is `state`.
fn reopen_and_wait(sandbox: &Sandbox, mut paddock: Paddock, state: &str) {
    paddock.detach_keys();
    assert!(paddock.wait_exit());
    let again = ready(sandbox);
    again.wait_for(&format!("the replayed row {state}"), |_| {
        repo_state(&again) == Some(state)
    });
}

#[test]
fn a_report_just_before_its_session_starts_belongs_to_it_live_and_on_replay() {
    let sandbox = Sandbox::new();
    stub_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock, "space-1:0");
    // The report comes before agy's first hook, within the 1 s it may.
    write_to(&sandbox, "space-1:0", &status("blocked", Some("auth")));
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    paddock.wait_for("waiting", |_| repo_state(&paddock) == Some("waiting"));
    // Written no earlier than the hook folded before it, so a replay
    // folds it after the session started too.
    assert!(logged_ts(&sandbox, "OSC7501") >= logged_ts(&sandbox, "SessionStart"));
    reopen_and_wait(&sandbox, paddock, "waiting");
}

#[test]
fn a_report_older_than_a_later_hook_is_dropped() {
    let sandbox = Sandbox::new();
    stub_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock, "space-1:0");
    // agy reported idle, then started and ran a prompt before paddock read
    // the hooks: the prompt is the news, live and on replay.
    write_to(&sandbox, "space-1:0", &status("idle", None));
    agy(&sandbox, "space-1:0", &["SessionStart", "PreInvocation"]);
    paddock.wait_for("running", |_| repo_state(&paddock) == Some("running"));
    reopen_and_wait(&sandbox, paddock, "running");
}

#[test]
fn reports_from_a_terminal_without_an_agy_session_are_not_recorded() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    write_to(&sandbox, "space-1:0", &status("blocked", Some("auth")));
    // A Claude session in the terminal: agy's report is still not its.
    let start = r#"{"session_id":"c1","cwd":"/tmp/repo","source":"startup"}"#;
    sandbox.hook("space-1:0", "SessionStart", start);
    wait_row(&paddock, "Claude's row", &["● repo", "idle"]);
    write_to(&sandbox, "space-1:0", &status("working", None));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let log = server_log(&sandbox);
    assert!(log.contains("SessionStart"), "{log}");
    assert!(!log.contains("OSC7501"), "{log}");
    assert!(row_with(&paddock, &["● repo", "idle"]));
}

#[test]
fn agy_quitting_without_a_prompt_does_not_notify() {
    let sandbox = Sandbox::new();
    stub_agy(&sandbox);
    let mut paddock = ready(&sandbox);
    start_agy(&sandbox, &mut paddock, "space-1:0");
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    // What agy writes as it quits, twice over: the second is not recorded.
    write_to(&sandbox, "space-1:0", &status("idle", None));
    write_to(&sandbox, "space-1:0", &status("idle", None));
    quit_agy(&sandbox, "space-1:0");
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(sandbox.notifications().is_empty());
    assert_eq!(server_log(&sandbox).matches("OSC7501").count(), 1);
}

#[test]
fn paddocks_terminals_ask_agy_for_its_status_reports() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo agy-$AGY_CLI_OSC7501");
    paddock.wait_for_text("agy-on");
}

#[test]
fn an_external_agy_session_lands_in_the_external_group() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    for name in ["SessionStart", "PreInvocation"] {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).unwrap();
        // No tmux at all: agy running outside paddock.
        let out = sandbox.run(&["hook", "--agent", "agy", name], &[], &text);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, b"{}", "{out:?}");
        assert!(out.stderr.is_empty(), "{out:?}");
    }
    // One line under the header: dot, label and state, no space.
    paddock.wait_for("the external group", |_| {
        let lines = paddock.sidebar();
        let header = lines.iter().position(|l| l == " EXTERNAL");
        header.is_some_and(|h| lines.get(h + 1).map(String::as_str) == Some(" ● repo  running"))
    });
    // Its kind is the dashboard's.
    paddock.command(b"o");
    paddock.wait_for("the dashboard's row", |s| {
        s.lines()
            .any(|l| l.contains(" repo ") && l.trim_end().ends_with(" agy"))
    });
    let log = std::fs::read_to_string(sandbox.path("state/paddock/events/external.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 2);
    assert!(log.lines().all(|l| l.contains(r#""kind":"agy""#)), "{log}");
}

// `paddock hooks install|uninstall` and agy's hooks file.

/// A `PATH` of the sandbox's own: `/usr/bin:/bin`, after a directory with
/// a stub `agy` if `with_agy`. Never the user's, where a real `agy` is.
fn path(sandbox: &Sandbox, with_agy: bool) -> String {
    let dir = sandbox.path("agy-bin");
    if with_agy && !dir.join("agy").exists() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("agy");
        std::fs::write(&stub, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    if with_agy {
        format!("{}:/usr/bin:/bin", dir.display())
    } else {
        "/usr/bin:/bin".to_string()
    }
}

/// `paddock hooks <args>` with `PATH` as [`path`] gives it; must succeed.
fn hooks(sandbox: &Sandbox, with_agy: bool, args: &[&str]) -> String {
    let mut all = vec!["hooks"];
    all.extend_from_slice(args);
    let path = path(sandbox, with_agy);
    let out = sandbox.run(&all, &[("PATH", &path)], "");
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn agy_hooks(sandbox: &Sandbox) -> PathBuf {
    sandbox.path("home/.gemini/config/hooks.json")
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A user's `hooks.json`, with named hooks of their own.
fn users_hooks() -> Value {
    json!({
        "lint-after-command": {
            "PostToolUse": [{"matcher": "run_command", "hooks": [
                {"type": "command", "command": "./scripts/lint.sh", "timeout": 10}
            ]}]
        },
        "reminder": {
            "enabled": true,
            "PreInvocation": [{"type": "command", "command": "./scripts/reminder.sh"}]
        }
    })
}

/// paddock's entry as install writes it.
fn paddocks_entry() -> Value {
    let paddock = env!("CARGO_BIN_EXE_paddock");
    let hook = |event: &str| {
        json!([{
            "type": "command",
            "command": format!("{paddock} hook --agent agy {event}"),
            "timeout": 10
        }])
    };
    json!({
        "SessionStart": hook("SessionStart"),
        "PreInvocation": hook("PreInvocation"),
        "Stop": hook("Stop"),
    })
}

#[test]
fn hooks_install_writes_agys_entry_only_when_agy_is_on_path() {
    let sandbox = Sandbox::new();
    let out = hooks(&sandbox, false, &["install"]);
    assert!(!agy_hooks(&sandbox).exists(), "{out}");
    assert!(!out.contains("Antigravity"), "{out}");

    let out = hooks(&sandbox, true, &["install"]);
    assert!(
        out.contains("Restart running Antigravity sessions"),
        "{out}"
    );
    assert_eq!(
        read(&agy_hooks(&sandbox)),
        json!({"paddock": paddocks_entry()})
    );

    let out = hooks(&sandbox, true, &["uninstall"]);
    assert!(out.contains(".gemini/config/hooks.json"), "{out}");
    assert_eq!(read(&agy_hooks(&sandbox)), json!({}));
}

#[test]
fn hooks_install_for_agy_keeps_the_users_hooks() {
    let sandbox = Sandbox::new();
    let path = agy_hooks(&sandbox);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string_pretty(&users_hooks()).unwrap()).unwrap();
    // Named, agy needs not be on PATH, and Claude is left alone.
    let out = hooks(&sandbox, false, &["install", "--agent", "agy"]);
    assert!(!sandbox.path("home/.claude").exists(), "{out}");
    assert!(
        !sandbox.path("home/.gemini/settings.json").exists(),
        "{out}"
    );
    let installed = read(&path);
    let mut want = users_hooks();
    want["paddock"] = paddocks_entry();
    assert_eq!(installed, want);

    // Twice is once.
    hooks(&sandbox, false, &["install", "--agent", "agy"]);
    assert_eq!(read(&path), installed);

    hooks(&sandbox, false, &["uninstall", "--agent", "agy"]);
    assert_eq!(read(&path), users_hooks());
}

#[test]
fn agy_uninstall_never_creates_the_hooks_file() {
    let sandbox = Sandbox::new();
    let out = hooks(&sandbox, true, &["uninstall", "--agent", "agy"]);
    assert!(!sandbox.path("home/.gemini").exists(), "{out}");
    assert!(out.contains("does not exist"), "{out}");
}

#[test]
fn an_agy_hooks_file_paddock_cannot_read_is_left_untouched() {
    let sandbox = Sandbox::new();
    let path = agy_hooks(&sandbox);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "{\n  // mine\n}\n";
    std::fs::write(&path, text).unwrap();
    let env_path = self::path(&sandbox, true);
    let out = sandbox.run(&["hooks", "install"], &[("PATH", &env_path)], "");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(out.stdout.is_empty(), "nothing done: {out:?}");
    assert!(!sandbox.path("home/.claude/settings.json").exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}
