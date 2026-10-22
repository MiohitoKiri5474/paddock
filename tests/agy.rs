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
//! tmux sees `agy` run; its status reports are written to the terminal as
//! agy 1.4.3 wrote them in a real run.

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
    let tty = sandbox.display(terminal, "#{pane_tty}");
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
    std::fs::copy(env!("CARGO_BIN_EXE_paddock"), sandbox.path("bin/agy")).unwrap();
}

/// Start the stand-in `agy` in `terminal` and wait until tmux sees it.
fn start_agy(sandbox: &Sandbox, paddock: &Paddock, terminal: &str) {
    // `paddock hook` reads its input to the end first, so this waits.
    sandbox.tmux(&["send-keys", "-t", terminal, "agy hook", "Enter"]);
    paddock.wait_for("agy to run", |_| {
        sandbox.display(terminal, "#{pane_current_command}") == "agy"
    });
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
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    start_agy(&sandbox, &paddock, "space-1:0");

    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle at start", &["● repo", "idle"]);
    wait_row(&paddock, "the sidebar's tag", &["space-1 · agy"]);

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
    sandbox.tmux(&["send-keys", "-t", "space-1:0", "C-c"]);
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
    sandbox.tmux(&["kill-pane", "-t", "space-1:1"]);
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
}

#[test]
fn reports_from_other_programs_or_without_a_session_change_nothing() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    // The second tab is the one looked at, so the first one's alerts would
    // notify.
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    // An agy session that ended: agy started again reports its sign-in
    // screen before any hook, which neither brings it back nor alerts.
    stub_agy(&sandbox);
    start_agy(&sandbox, &paddock, "space-1:0");
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    sandbox.tmux(&["send-keys", "-t", "space-1:0", "C-c"]);
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
    write_to(&sandbox, "space-1:0", &status("blocked", Some("auth")));
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(sandbox.notifications().is_empty());
    assert!(!paddock.full_screen().contains("● repo"));
    // agy started again on the same conversation (a resume).
    start_agy(&sandbox, &paddock, "space-1:0");
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
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);
    start_agy(&sandbox, &paddock, "space-1:0");
    agy(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    // What agy writes as it quits, twice over: the second is not recorded.
    write_to(&sandbox, "space-1:0", &status("idle", None));
    write_to(&sandbox, "space-1:0", &status("idle", None));
    sandbox.tmux(&["send-keys", "-t", "space-1:0", "C-c"]);
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
    let paddock = ready(&sandbox);
    sandbox.tmux(&[
        "send-keys",
        "-t",
        "space-1:0",
        "echo agy-$AGY_CLI_OSC7501",
        "Enter",
    ]);
    paddock.wait_for_text("agy-on");
}

#[test]
fn an_external_agy_session_lands_in_the_external_group() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    for name in ["SessionStart", "PreInvocation"] {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}.json")).unwrap();
        // No tmux at all: agy running outside paddock.
        let out = sandbox.run(&["hook", "--agent", "agy", name], &[], &text);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, b"{}", "{out:?}");
        assert!(out.stderr.is_empty(), "{out:?}");
    }
    paddock.wait_for("the external group", |_| {
        let rows = paddock.full_rows();
        let header = rows.iter().position(|l| l.contains(" EXTERNAL"));
        header.is_some_and(|h| {
            rows[h + 1].contains("● repo")
                && rows[h + 1].contains("running")
                && rows[h + 2].contains("agy")
        })
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
