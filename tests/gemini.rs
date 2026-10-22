//! Ticket 34: Gemini CLI sessions. Their hooks call `paddock hook --agent
//! gemini <event>`; a session shows, alerts and ends like Claude's, in
//! paddock's terminals and as an external session. `paddock hooks install`
//! writes them to the sandbox's `~/.gemini/settings.json` when `gemini` is
//! on `PATH`, or when named. The payloads in `tests/fixtures/hooks/gemini/`
//! are shaped as Gemini CLI 0.64 builds them (`createBaseInput` and each
//! event's fields), not recorded from a live run.

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hooks/gemini");
const WAITING: &str = "repo is waiting";
const IDLE: &str = "repo is idle";

/// The fixture `name` (`<event>[-<variant>]`) and its event.
fn payload(name: &str) -> (String, String) {
    let path = format!("{FIXTURES}/{name}.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = serde_json::from_str(&text).unwrap();
    (v["hook_event_name"].as_str().unwrap().to_string(), text)
}

/// Run Gemini's hooks for fixtures `names`, in order, in `terminal`.
fn gemini(sandbox: &Sandbox, terminal: &str, names: &[&str]) {
    for name in names {
        let (event, text) = payload(name);
        sandbox.agent_hook(terminal, "gemini", &event, &text);
    }
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
fn a_gemini_session_goes_working_waiting_idle_then_ends() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    // Gemini runs in the first tab; the second is the one looked at, so
    // its alerts notify.
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(PROMPT);

    gemini(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle at start", &["● repo", "idle"]);
    // The tab named after the session, and the sidebar, take Gemini's tag.
    wait_row(
        &paddock,
        "the sidebar's tag",
        &["space-1 · gemini", "gemini │"],
    );

    gemini(&sandbox, "space-1:0", &["BeforeAgent"]);
    wait_row(&paddock, "working", &["● repo", "running"]);
    gemini(&sandbox, "space-1:0", &["Notification-ToolPermission"]);
    wait_row(&paddock, "waiting", &["● repo", "waiting"]);
    assert_eq!(sandbox.wait_for_notifications(1), [WAITING]);
    // The permission granted, the tool's `AfterTool` ends the wait.
    gemini(&sandbox, "space-1:0", &["AfterTool"]);
    wait_row(&paddock, "working again", &["● repo", "running"]);
    gemini(&sandbox, "space-1:0", &["AfterAgent"]);
    wait_row(&paddock, "done", &["● repo", "done"]);
    assert_eq!(sandbox.wait_for_notifications(2), [WAITING, IDLE]);

    paddock.command(b"o");
    paddock.wait_for_text("AGENT");
    paddock.wait_for("the dashboard's row", |s| {
        s.lines().any(|l| {
            l.contains(" repo ") && l.contains(" done ") && l.trim_end().ends_with(" gemini")
        })
    });
    paddock.send(b"\x1b");

    gemini(&sandbox, "space-1:0", &["SessionEnd"]);
    paddock.wait_for("the session left", |_| {
        !paddock.full_screen().contains("● repo")
    });
}

#[test]
fn fork_refuses_a_gemini_session() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    gemini(&sandbox, "space-1:0", &["SessionStart"]);
    wait_row(&paddock, "idle", &["● repo", "idle"]);
    paddock.command(b"o");
    paddock.wait_for_text("AGENT");
    paddock.send(b"f");
    paddock.wait_for_text("fork needs a Claude session");
    assert!(!sandbox.path("claude.log").exists(), "claude was never run");
}

#[test]
fn gemini_clear_keeps_the_terminals_session() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    gemini(&sandbox, "space-1:0", &["BeforeAgent"]);
    wait_row(&paddock, "working", &["● repo", "running"]);
    let (event, clear) = payload("SessionStart-clear");
    gemini(&sandbox, "space-1:0", &["SessionEnd-clear"]);
    // `/clear` starts a conversation with a new id at once.
    let clear = clear.replace("aaaaaaaa-", "cccccccc-");
    sandbox.agent_hook("space-1:0", "gemini", &event, &clear);
    wait_row(&paddock, "idle after /clear", &["● repo", "idle"]);
}

#[test]
fn an_external_gemini_session_lands_in_the_external_group() {
    let sandbox = Sandbox::new();
    let paddock = ready(&sandbox);
    for name in ["BeforeAgent", "Notification-ToolPermission"] {
        let (event, text) = payload(name);
        // No tmux at all: Gemini running outside paddock.
        let out = sandbox.run(&["hook", "--agent", "gemini", &event], &[], &text);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    }
    paddock.wait_for("the external group", |_| {
        let rows = paddock.full_rows();
        let header = rows.iter().position(|l| l.contains(" EXTERNAL"));
        header.is_some_and(|h| {
            rows[h + 1].contains("● repo")
                && rows[h + 1].contains("waiting")
                && rows[h + 2].contains("gemini")
        })
    });
    let state = sandbox.path("state/paddock/events/external.jsonl");
    let log = std::fs::read_to_string(state).unwrap();
    assert_eq!(log.lines().count(), 2);
    assert!(
        log.lines().all(|l| l.contains(r#""kind":"gemini""#)),
        "{log}"
    );
}

// `paddock hooks install|uninstall` and Gemini's settings.

/// A `PATH` of the sandbox's own: `/usr/bin:/bin`, after a directory with
/// a stub `gemini` if `with_gemini`. Never the user's, where a real
/// `gemini` may be.
fn path(sandbox: &Sandbox, with_gemini: bool) -> String {
    let dir = sandbox.path("gemini-bin");
    if with_gemini && !dir.join("gemini").exists() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("gemini");
        std::fs::write(&stub, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    if with_gemini {
        format!("{}:/usr/bin:/bin", dir.display())
    } else {
        "/usr/bin:/bin".to_string()
    }
}

/// `paddock hooks <args>` with `PATH` as [`path`] gives it; must succeed.
fn hooks(sandbox: &Sandbox, with_gemini: bool, args: &[&str]) -> String {
    let mut all = vec!["hooks"];
    all.extend_from_slice(args);
    let path = path(sandbox, with_gemini);
    let out = sandbox.run(&all, &[("PATH", &path)], "");
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn gemini_settings(sandbox: &Sandbox) -> PathBuf {
    sandbox.path("home/.gemini/settings.json")
}

fn claude_settings(sandbox: &Sandbox) -> PathBuf {
    sandbox.path("home/.claude/settings.json")
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A user's Gemini settings, with hooks and settings of their own.
fn users_settings() -> Value {
    json!({
        "general": {"vimMode": true},
        "security": {"auth": {"selectedType": "oauth-personal"}},
        "hooksConfig": {"notifications": false},
        "hooks": {
            "disabled": [],
            "AfterAgent": [{"matcher": "*", "hooks": [
                {"name": "ding", "type": "command", "command": "afplay ding.aiff", "timeout": 2000}
            ]}],
            "BeforeTool": [{"matcher": "write_file", "hooks": [
                {"type": "command", "command": "./check.sh"}
            ]}]
        }
    })
}

fn write_users_settings(sandbox: &Sandbox) {
    let path = gemini_settings(sandbox);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&users_settings()).unwrap(),
    )
    .unwrap();
}

/// `(event, command, timeout)` of each of paddock's hooks in `settings`.
fn ours(settings: &Value) -> Vec<(String, String, Value)> {
    let mut found = Vec::new();
    for (event, groups) in settings["hooks"].as_object().into_iter().flatten() {
        for g in groups.as_array().into_iter().flatten() {
            for h in g["hooks"].as_array().into_iter().flatten() {
                let command = h["command"].as_str().unwrap_or_default();
                if command.contains(" hook --agent gemini ") {
                    found.push((event.clone(), command.to_string(), h["timeout"].clone()));
                }
            }
        }
    }
    found
}

#[test]
fn hooks_install_writes_gemini_hooks_only_when_gemini_is_on_path() {
    let sandbox = Sandbox::new();
    let out = hooks(&sandbox, false, &["install"]);
    assert!(claude_settings(&sandbox).exists(), "Claude, as always");
    assert!(!gemini_settings(&sandbox).exists(), "{out}");
    assert!(!out.contains("Gemini"), "{out}");

    let out = hooks(&sandbox, true, &["install"]);
    assert!(out.contains("Restart running Gemini sessions"), "{out}");
    let paddock = env!("CARGO_BIN_EXE_paddock");
    let mut found = ours(&read(&gemini_settings(&sandbox)));
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut want: Vec<(String, String, Value)> = [
        "AfterAgent",
        "AfterTool",
        "BeforeAgent",
        "Notification",
        "SessionEnd",
        "SessionStart",
    ]
    .iter()
    .map(|e| {
        let command = format!("{paddock} hook --agent gemini {e}");
        (e.to_string(), command, json!(10_000))
    })
    .collect();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(found, want);

    let out = hooks(&sandbox, true, &["uninstall"]);
    assert!(out.contains(".gemini/settings.json"), "{out}");
    assert!(ours(&read(&gemini_settings(&sandbox))).is_empty());
    assert_eq!(read(&claude_settings(&sandbox)), json!({}));
}

#[test]
fn hooks_install_for_gemini_keeps_the_users_hooks_and_settings() {
    let sandbox = Sandbox::new();
    write_users_settings(&sandbox);
    // Named, Gemini needs not be on PATH, and Claude is left alone.
    let out = hooks(&sandbox, false, &["install", "--agent", "gemini"]);
    assert!(!claude_settings(&sandbox).exists(), "{out}");
    assert!(!out.contains("Claude"), "{out}");
    let installed = read(&gemini_settings(&sandbox));
    assert_eq!(ours(&installed).len(), 6);
    for key in ["general", "security", "hooksConfig"] {
        assert_eq!(installed[key], users_settings()[key], "{key}");
    }
    assert_eq!(installed["hooks"]["disabled"], json!([]));
    assert_eq!(
        installed["hooks"]["BeforeTool"],
        users_settings()["hooks"]["BeforeTool"]
    );
    assert_eq!(
        installed["hooks"]["AfterAgent"][0],
        users_settings()["hooks"]["AfterAgent"][0],
        "the user's hook first"
    );

    // Twice is once.
    hooks(&sandbox, false, &["install", "--agent", "gemini"]);
    assert_eq!(read(&gemini_settings(&sandbox)), installed);

    hooks(&sandbox, false, &["uninstall", "--agent", "gemini"]);
    assert_eq!(read(&gemini_settings(&sandbox)), users_settings());
    assert!(!claude_settings(&sandbox).exists());
}

#[test]
fn gemini_hooks_follow_gemini_cli_home() {
    let sandbox = Sandbox::new();
    let home = sandbox.path("gemini-home");
    let path = path(&sandbox, false);
    let out = sandbox.run(
        &["hooks", "install", "--agent", "gemini"],
        &[("PATH", &path), ("GEMINI_CLI_HOME", home.to_str().unwrap())],
        "",
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(ours(&read(&home.join(".gemini/settings.json"))).len(), 6);
    assert!(!gemini_settings(&sandbox).exists());
}

#[test]
fn a_gemini_settings_file_paddock_cannot_read_is_left_untouched() {
    let sandbox = Sandbox::new();
    let path = gemini_settings(&sandbox);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Gemini reads comments in its settings; paddock does not edit them.
    let text = "{\n  // mine\n  \"general\": {}\n}\n";
    std::fs::write(&path, text).unwrap();
    let env_path = self::path(&sandbox, false);
    let out = sandbox.run(
        &["hooks", "install", "--agent", "gemini"],
        &[("PATH", &env_path)],
        "",
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("invalid JSON"),
        "{out:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}
