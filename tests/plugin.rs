//! Ticket 25: the repository is a Claude Code plugin marketplace. A cheap
//! check of what `claude plugin validate --strict` checks in full: both
//! manifests parse, names match, sources exist, and every skill has
//! frontmatter with a name and a description. Ticket 27: the plugin's hooks
//! run silently, and with `paddock hooks install`'s record each event once.

mod harness;

use serde_json::Value;
use std::cell::OnceCell;
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

/// HOME, state and PATH of their own, never the user's, in a harness
/// sandbox, whose paddock and backend start the first time a command runs
/// inside one of its terminals.
struct Sandbox {
    base: harness::Sandbox,
    /// paddock's first terminal, once a command ran inside it.
    terminal: OnceCell<Terminal>,
}

/// A terminal of paddock's, as a hook in it sees it.
struct Terminal {
    /// Its hook variables, as the backend gives them.
    hook_env: Vec<(String, String)>,
    /// The backend's instance id, which names its event log.
    instance: u32,
}

impl Sandbox {
    fn new() -> Self {
        let base = harness::Sandbox::new();
        // `bin` holds the harness's stubs: the hooks' PATH is `plugin-bin`.
        for d in ["plugin-bin", "work"] {
            std::fs::create_dir(base.path(d)).unwrap();
        }
        Self {
            base,
            terminal: OnceCell::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.base.path(name)
    }

    fn settings(&self) -> PathBuf {
        self.path("home/.claude/settings.json")
    }

    /// Put `paddock` on the sandbox's PATH.
    fn add_paddock(&self) {
        std::os::unix::fs::symlink(PADDOCK, self.path("plugin-bin/paddock")).unwrap();
    }

    /// paddock's first terminal; the first call runs paddock to start it,
    /// then detaches.
    fn terminal(&self) -> &Terminal {
        self.terminal.get_or_init(|| {
            let mut paddock = self.base.launch();
            paddock.wait_for_text(harness::PROMPT);
            let state = self.base.state();
            let instance = state.instance.expect("the backend's instance id");
            let first = state.terminal("space-1").expect("paddock's terminal");
            let hook_env = first.hook_env.clone().into_iter().collect();
            paddock.detach_keys();
            assert!(paddock.wait_exit());
            Terminal { hook_env, instance }
        })
    }

    /// `program args`, with `stdin`, outside paddock's terminals (as from
    /// the user's own shell): paddock is not started for it.
    fn run(&self, program: &str, args: &[&str], stdin: &str) -> std::process::Output {
        self.run_where(false, program, args, stdin)
    }

    /// [`Self::run`], in a terminal of paddock's server if `inside`, else
    /// outside paddock's terminals (an external session).
    fn run_where(
        &self,
        inside: bool,
        program: &str,
        args: &[&str],
        stdin: &str,
    ) -> std::process::Output {
        use std::io::Write;
        let path = format!("{}:/usr/bin:/bin", self.path("plugin-bin").display());
        let mut cmd = std::process::Command::new(program);
        cmd.args(args).env_clear().envs(self.base.backend_env());
        if inside {
            cmd.envs(self.terminal().hook_env.iter().cloned());
        }
        let mut child = cmd
            .env("HOME", self.path("home"))
            .env("PATH", path)
            // The config of the paddock the hooks talk to.
            .env("XDG_CONFIG_HOME", self.path("config"))
            .current_dir(self.path("work"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        // A program that exits without reading closes the pipe: fine.
        let _ = input.write_all(stdin.as_bytes());
        drop(input);
        child.wait_with_output().unwrap()
    }

    /// Run a hook command as Claude Code does: `sh -c`, the payload on
    /// stdin. It must exit 0 and print nothing: stdout of `SessionStart` and
    /// `UserPromptSubmit` reaches Claude's context.
    fn fire(&self, command: &str) {
        self.fire_where(true, command);
    }

    /// [`Self::fire`], in paddock's terminal if `inside`, else outside it.
    fn fire_where(&self, inside: bool, command: &str) {
        let out = self.run_where(inside, "/bin/sh", &["-c", command], PAYLOAD);
        assert!(out.status.success(), "{command}: {out:?}");
        assert!(out.stdout.is_empty(), "{command}: {out:?}");
        assert!(out.stderr.is_empty(), "{command}: {out:?}");
    }

    /// The events recorded, in order.
    /// None while no command ran inside paddock's terminal: paddock never
    /// started.
    fn events(&self) -> Vec<String> {
        match self.terminal.get() {
            Some(t) => self.events_in(&format!("{}.jsonl", t.instance)),
            None => Vec::new(),
        }
    }

    /// The events recorded in the log named `name`, in order.
    fn events_in(&self, name: &str) -> Vec<String> {
        let log = self.path("state/paddock/events").join(name);
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
            command.ends_with(&format!("; paddock hook {event} >/dev/null 2>&1 || true")),
            "{command}"
        );
    }
}

#[test]
fn a_failing_paddock_on_path_never_reaches_claude() {
    let sandbox = Sandbox::new();
    // Some other `paddock`, or a broken one: prints, and exits 2, which
    // would block `Stop`.
    let fake = sandbox.path("bin/paddock");
    std::fs::write(&fake, "#!/bin/sh\necho out\necho err >&2\nexit 2\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (_, command) in hook_commands(&plugin_hooks()) {
        sandbox.fire(&command);
    }
    assert!(sandbox.events().is_empty());
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
fn external_events_are_recorded_once_with_both_sources_installed() {
    let sandbox = Sandbox::new();
    sandbox.add_paddock();
    let out = sandbox.run(PADDOCK, &["hooks", "install"], "");
    assert!(out.status.success(), "{out:?}");
    let from_settings = hook_commands(&read_json(&sandbox.settings()));
    let from_plugin = hook_commands(&plugin_hooks());
    for ((_, s), (_, p)) in from_settings.iter().zip(&from_plugin) {
        sandbox.fire_where(false, p);
        sandbox.fire_where(false, s);
    }
    assert_eq!(sandbox.events_in("external.jsonl"), events_of(&from_plugin));
    assert!(sandbox.events().is_empty(), "not the server's log");
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
    assert!(
        stdout.contains("Restart running Claude sessions"),
        "{stdout}"
    );
    let settings = read_json(&sandbox.settings());
    assert_eq!(settings["enabledPlugins"]["paddock@paddock"], true);
    assert!(settings["hooks"]["Stop"].is_array());

    let out = sandbox.run(PADDOCK, &["hooks", "uninstall"], "");
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Restart running Claude sessions"),
        "{stdout}"
    );
}

// Ticket 33: agent kinds. Claude is the only kind; `--agent claude` names
// it, and without the flag `hooks install|uninstall` act on it as before.

#[test]
fn hooks_install_for_claude_by_name_writes_what_the_default_writes() {
    let named = Sandbox::new();
    let out = named.run(PADDOCK, &["hooks", "install", "--agent", "claude"], "");
    assert!(out.status.success(), "{out:?}");
    let default = Sandbox::new();
    let out = default.run(PADDOCK, &["hooks", "install"], "");
    assert!(out.status.success(), "{out:?}");
    let read = |s: &Sandbox| std::fs::read_to_string(s.settings()).unwrap();
    assert_eq!(read(&named), read(&default), "byte for byte");
    let commands = hook_commands(&read_json(&named.settings()));
    assert_eq!(commands.len(), 10, "every event");
    for (event, command) in &commands {
        assert_eq!(
            command,
            &format!("{PADDOCK} hook {event}"),
            "as before kinds"
        );
    }

    let out = named.run(PADDOCK, &["hooks", "uninstall", "--agent", "claude"], "");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(read(&named), "{}\n");
}

#[test]
fn hooks_install_refuses_an_unknown_agent_and_touches_nothing() {
    let sandbox = Sandbox::new();
    std::fs::create_dir_all(sandbox.settings().parent().unwrap()).unwrap();
    std::fs::write(sandbox.settings(), r#"{"model": "opus"}"#).unwrap();
    for args in [
        &["hooks", "install", "--agent", "nope"][..],
        &["hooks", "install", "--agent", "claude", "--agent", "nope"],
        &["hooks", "uninstall", "--agent"],
        &["hooks", "install", "claude"],
    ] {
        let out = sandbox.run(PADDOCK, args, "");
        assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
        assert_eq!(
            std::fs::read_to_string(sandbox.settings()).unwrap(),
            r#"{"model": "opus"}"#,
            "{args:?}"
        );
    }
}

#[test]
fn a_hook_naming_claude_records_as_claudes_and_an_unknown_one_stays_silent() {
    let sandbox = Sandbox::new();
    let run = |args: &[&str]| {
        let out = sandbox.run_where(true, PADDOCK, args, PAYLOAD);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    };
    run(&["hook", "--agent", "claude", "Stop"]);
    run(&["hook", "SessionEnd"]);
    assert_eq!(sandbox.events(), ["Stop", "SessionEnd"]);
    let log = format!("state/paddock/events/{}.jsonl", sandbox.terminal().instance);
    let log = sandbox.path(&log);
    let log = std::fs::read_to_string(log).unwrap();
    assert!(!log.contains("kind"), "Claude's lines as before: {log}");

    run(&["hook", "--agent", "nope", "Stop"]);
    run(&["hook"]);
    assert_eq!(sandbox.events(), ["Stop", "SessionEnd"], "nothing recorded");
    let errors = std::fs::read_to_string(sandbox.path("state/paddock/hook-errors.log"));
    assert!(errors.unwrap().contains("unknown agent"), "reported");
}
