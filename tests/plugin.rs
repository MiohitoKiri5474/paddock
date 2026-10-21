//! Ticket 25: the repository is a Claude Code plugin marketplace. A cheap
//! check of what `claude plugin validate --strict` checks in full: both
//! manifests parse, names match, sources exist, and every skill has
//! frontmatter with a name and a description. Ticket 27: the plugin's hooks
//! run silently, and with `paddock hooks install`'s record each event once.

use serde_json::Value;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn text(v: &Value, key: &str, of: &str) -> String {
    match v[key].as_str() {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => panic!("{of}: no {key}"),
    }
}

/// The `key: value` lines between a SKILL.md's opening and closing `---`.
fn frontmatter(path: &Path) -> Vec<(String, String)> {
    let body = std::fs::read_to_string(path).unwrap();
    let mut lines = body.lines();
    assert_eq!(
        lines.next(),
        Some("---"),
        "{}: no frontmatter",
        path.display()
    );
    let fields: Vec<&str> = lines.by_ref().take_while(|l| *l != "---").collect();
    assert!(
        lines.next().is_some(),
        "{}: frontmatter not closed, or nothing after it",
        path.display()
    );
    fields
        .iter()
        .map(|l| {
            let (k, v) = l
                .split_once(':')
                .unwrap_or_else(|| panic!("{}: bad line {l:?}", path.display()));
            (k.trim().to_string(), v.trim().to_string())
        })
        .collect()
}

#[test]
fn the_marketplace_lists_the_paddock_plugin_and_every_skill_has_frontmatter() {
    let root = Path::new(ROOT);
    let market = read_json(&root.join(".claude-plugin/marketplace.json"));
    assert_eq!(text(&market, "name", "marketplace"), "paddock");
    text(&market["owner"], "name", "marketplace owner");
    let plugins = market["plugins"].as_array().expect("plugins");
    assert_eq!(plugins.len(), 1);
    for entry in plugins {
        let name = text(entry, "name", "plugin entry");
        let source = text(entry, "source", &name);
        assert!(
            source.starts_with("./") && !source.contains(".."),
            "{source}"
        );
        let dir: PathBuf = root.join(&source);
        let manifest = read_json(&dir.join(".claude-plugin/plugin.json"));
        assert_eq!(
            text(&manifest, "name", "plugin.json"),
            name,
            "entry and manifest names"
        );
        text(&manifest, "description", "plugin.json");
        text(&manifest, "version", "plugin.json");

        let mut skills: Vec<String> = std::fs::read_dir(dir.join("skills"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        skills.sort();
        assert_eq!(
            skills,
            ["paddock-worktrees", "worktree-list", "worktree-new"]
        );
        for skill in &skills {
            let path = dir.join("skills").join(skill).join("SKILL.md");
            let fields = frontmatter(&path);
            let get = |k: &str| {
                fields
                    .iter()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.as_str())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| panic!("{}: no {k}", path.display()))
            };
            assert_eq!(get("name"), skill, "name matches its directory");
            get("description");
            // `rm` is never pre-approved: the user confirms each delete.
            let allowed = fields
                .iter()
                .find(|(k, _)| k == "allowed-tools")
                .map_or("", |(_, v)| v.as_str());
            assert!(!allowed.contains("rm"), "{skill}: {allowed}");
            assert!(!allowed.contains("worktree *"), "{skill}: {allowed}");
        }
    }
}

// Ticket 27: the plugin carries paddock's hooks. A unit test in
// `src/hooks.rs` checks the file against `hooks::EVENTS` and the groups
// `paddock hooks install` writes; these run its commands as Claude would.

const PADDOCK: &str = env!("CARGO_BIN_EXE_paddock");
const SOCKET: &str = "paddock-plugin-test";
const SERVER_PID: &str = "4242";
const PAYLOAD: &str = r#"{"session_id":"s1"}"#;

fn plugin_hooks() -> Value {
    read_json(&Path::new(ROOT).join("plugins/paddock/hooks/hooks.json"))
}

/// `(event, command)` for every hook in a `{"hooks": {event: [group]}}` file.
fn hook_commands(file: &Value) -> Vec<(String, String)> {
    let events = file["hooks"]
        .as_object()
        .expect("a top-level \"hooks\" map");
    events
        .iter()
        .flat_map(|(event, groups)| {
            groups
                .as_array()
                .expect("a list of groups")
                .iter()
                .flat_map(|g| g["hooks"].as_array().expect("hooks").clone())
                .map(move |h| {
                    assert_eq!(h["type"], "command", "{event}");
                    (event.clone(), text(&h, "command", event))
                })
        })
        .collect()
}

fn events_of(commands: &[(String, String)]) -> Vec<String> {
    commands.iter().map(|(e, _)| e.clone()).collect()
}

/// HOME, state and PATH of their own, never the user's.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for d in ["home", "state", "bin", "work"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn settings(&self) -> PathBuf {
        self.path("home/.claude/settings.json")
    }

    /// Put `paddock` on the sandbox's PATH.
    fn add_paddock(&self) {
        std::os::unix::fs::symlink(PADDOCK, self.path("bin/paddock")).unwrap();
    }

    /// `program args`, with `stdin`, as if in a terminal of paddock's server.
    fn run(&self, program: &str, args: &[&str], stdin: &str) -> std::process::Output {
        use std::io::Write;
        let path = format!("{}:/usr/bin:/bin", self.path("bin").display());
        let mut child = std::process::Command::new(program)
            .args(args)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("PATH", path)
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_CONFIG_HOME", self.path("home/.config"))
            .env("PADDOCK_TMUX_SOCKET", SOCKET)
            .env("TMUX", format!("/nowhere/{SOCKET},{SERVER_PID},0"))
            .env("TMUX_PANE", "%7")
            .current_dir(self.path("work"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(stdin.as_bytes()).unwrap();
        drop(input);
        child.wait_with_output().unwrap()
    }

    /// Run a hook command as Claude Code does: `sh -c`, the payload on
    /// stdin. It must exit 0 and print nothing: stdout of `SessionStart` and
    /// `UserPromptSubmit` reaches Claude's context.
    fn fire(&self, command: &str) {
        let out = self.run("/bin/sh", &["-c", command], PAYLOAD);
        assert!(out.status.success(), "{command}: {out:?}");
        assert!(out.stdout.is_empty(), "{command}: {out:?}");
        assert!(out.stderr.is_empty(), "{command}: {out:?}");
    }

    /// The events recorded, in order.
    fn events(&self) -> Vec<String> {
        let log = self
            .path("state/paddock/events")
            .join(format!("{SERVER_PID}.jsonl"));
        let Ok(body) = std::fs::read_to_string(&log) else {
            return Vec::new();
        };
        body.lines()
            .map(|l| text(&serde_json::from_str(l).unwrap(), "event", "record"))
            .collect()
    }
}

#[test]
fn the_plugin_hooks_run_paddock_hook_for_their_event() {
    let commands = hook_commands(&plugin_hooks());
    assert_eq!(commands.len(), 10, "one hook per event");
    for (event, command) in &commands {
        assert!(
            command.contains(&format!("exec paddock hook {event} >/dev/null 2>&1")),
            "{command}"
        );
    }
}

#[test]
fn without_paddock_on_path_the_plugin_hooks_exit_0_silently() {
    let sandbox = Sandbox::new();
    for (_, command) in hook_commands(&plugin_hooks()) {
        sandbox.fire(&command);
    }
    assert!(sandbox.events().is_empty());
}

#[test]
fn the_plugin_hooks_alone_record_every_event_once() {
    let sandbox = Sandbox::new();
    sandbox.add_paddock();
    let commands = hook_commands(&plugin_hooks());
    for (_, command) in &commands {
        sandbox.fire(command);
    }
    assert_eq!(sandbox.events(), events_of(&commands));
}

#[test]
fn the_plugin_and_settings_hooks_together_record_every_event_once() {
    let sandbox = Sandbox::new();
    sandbox.add_paddock();
    let out = sandbox.run(PADDOCK, &["hooks", "install"], "");
    assert!(out.status.success(), "{out:?}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("plugin"));

    let from_settings = hook_commands(&read_json(&sandbox.settings()));
    let from_plugin = hook_commands(&plugin_hooks());
    assert_eq!(events_of(&from_settings), events_of(&from_plugin));
    // The plugin's copies defer to the settings' ones.
    for (_, p) in &from_plugin {
        sandbox.fire(p);
    }
    assert!(sandbox.events().is_empty(), "{:?}", sandbox.events());
    // Claude runs both copies of an event, in either order.
    for (i, ((_, s), (_, p))) in from_settings.iter().zip(&from_plugin).enumerate() {
        let (first, second) = if i % 2 == 0 { (s, p) } else { (p, s) };
        sandbox.fire(first);
        sandbox.fire(second);
    }
    assert_eq!(sandbox.events(), events_of(&from_plugin));
}

#[test]
fn hooks_install_notes_an_enabled_plugin() {
    let sandbox = Sandbox::new();
    std::fs::create_dir_all(sandbox.settings().parent().unwrap()).unwrap();
    std::fs::write(
        sandbox.settings(),
        r#"{"enabledPlugins": {"paddock@paddock": true}}"#,
    )
    .unwrap();
    let out = sandbox.run(PADDOCK, &["hooks", "install"], "");
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("the paddock plugin is enabled"), "{stdout}");
    assert!(stdout.contains("paddock hooks uninstall"), "{stdout}");
    let settings = read_json(&sandbox.settings());
    assert_eq!(settings["enabledPlugins"]["paddock@paddock"], true);
    assert!(settings["hooks"]["Stop"].is_array());
}
