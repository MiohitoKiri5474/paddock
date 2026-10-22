//! Agent hooks. For Claude Code, the paddock plugin carries them
//! (`plugins/paddock/hooks/hooks.json`); `paddock hooks install|uninstall`
//! edits each agent's settings file (Claude's for users without the plugin,
//! Gemini CLI's always); `paddock hook
//! [--agent <kind>] <event>` records one event (no `--agent`: Claude's, as
//! the hooks installed before kinds existed call it).

use crate::git;
use crate::paths::Paths;
use crate::sessions::{self, log, AgentKind};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The Claude Code events paddock listens to.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PreCompact",
    "PermissionRequest",
    "Notification",
    "Stop",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
];

/// The Gemini CLI events paddock listens to. Not `BeforeTool`: it runs
/// before the permission prompt, so it says nothing `AfterTool` does not,
/// and every hook delays the tool. Not `BeforeModel`, `AfterModel` (every
/// streamed chunk), `BeforeToolSelection` or `PreCompress`: they change no
/// state.
pub const GEMINI_EVENTS: &[&str] = &[
    "SessionStart",
    "BeforeAgent",
    "AfterTool",
    "Notification",
    "AfterAgent",
    "SessionEnd",
];

/// The `timeout` of paddock's Gemini hooks, in milliseconds: a hung
/// `paddock` stalls Gemini 10 s at most (Gemini's default is 60 s), as the
/// plugin's hooks bound Claude.
const GEMINI_TIMEOUT_MS: u64 = 10_000;

/// Hooks of the tmux plugin paddock replaces; install removes them.
const LEGACY: &str = "tmux-claude-session-manager";

/// The paddock plugin's key in Claude's `enabledPlugins` (`<plugin>@<marketplace>`).
const PLUGIN_KEY: &str = "paddock@paddock";

/// Set to [`PLUGIN_SOURCE`] by the plugin's hook commands. A plugin hook
/// defers to paddock's hooks in `settings.json`, so an event is recorded once
/// when both are installed.
const SOURCE_VAR: &str = "PADDOCK_HOOK_SOURCE";
const PLUGIN_SOURCE: &str = "plugin";

/// Restarting running sessions makes them run the hooks as they are now: a
/// session may not reload its settings' hooks at once.
fn restart(kind: AgentKind) -> String {
    format!(
        "Restart running {} sessions so they use the hooks as they are now.",
        match kind {
            AgentKind::Claude => "Claude",
            AgentKind::Gemini => "Gemini",
        }
    )
}

/// The events of `kind` paddock listens to.
fn events(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => EVENTS,
        AgentKind::Gemini => GEMINI_EVENTS,
    }
}

/// Where `kind` reads its user hooks. Claude:
/// `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
/// Gemini: `$GEMINI_CLI_HOME/.gemini/settings.json`, else
/// `~/.gemini/settings.json`.
pub fn settings_path(kind: AgentKind, var: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    let non_empty = |k: &str| var(k).filter(|v| !v.is_empty());
    match kind {
        AgentKind::Claude => {
            let dir = match non_empty("CLAUDE_CONFIG_DIR") {
                Some(d) => PathBuf::from(d),
                None => {
                    PathBuf::from(non_empty("HOME").context("HOME is not set")?).join(".claude")
                }
            };
            Ok(dir.join("settings.json"))
        }
        AgentKind::Gemini => {
            let home = non_empty("GEMINI_CLI_HOME")
                .or_else(|| non_empty("HOME"))
                .context("HOME is not set")?;
            Ok(PathBuf::from(home).join(".gemini").join("settings.json"))
        }
    }
}

/// The command a hook of `kind` runs for `event`: `<exe> hook <event>` for
/// Claude (as before kinds existed, and as the plugin calls it), else
/// `<exe> hook --agent <kind> <event>`.
fn hook_command(exe: &str, kind: AgentKind, event: &str) -> String {
    match kind {
        AgentKind::Claude => format!("{} hook {event}", quote(exe)),
        AgentKind::Gemini => format!("{} hook --agent {} {event}", quote(exe), kind.tag()),
    }
}

/// The group `install` writes for `event` of `kind`: one command hook.
/// Claude's has no timeout (as before kinds existed); Gemini's has
/// [`GEMINI_TIMEOUT_MS`].
fn hook_group(exe: &str, kind: AgentKind, event: &str) -> Value {
    let command = hook_command(exe, kind, event);
    match kind {
        AgentKind::Claude => json!({"hooks": [{"type": "command", "command": command}]}),
        AgentKind::Gemini => json!({"hooks": [
            {"type": "command", "command": command, "timeout": GEMINI_TIMEOUT_MS}
        ]}),
    }
}

fn quote(s: &str) -> String {
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Whether a hook command runs `paddock hook`, whatever paddock's path and
/// whatever agent it names.
fn is_ours(command: &str) -> bool {
    let (program, rest) = match command.strip_prefix('\'') {
        Some(q) => match q.split_once('\'') {
            Some((p, r)) => (p, r),
            None => return false,
        },
        None => command.split_once(' ').unwrap_or((command, "")),
    };
    Path::new(program)
        .file_name()
        .is_some_and(|n| n == "paddock")
        && rest.trim_start().starts_with("hook ")
}

/// Drop every hook whose command matches, and any group or event this left
/// empty. Other entries stay, even empty lists (Gemini keeps `disabled`, a
/// list of hook names, next to its events).
fn remove_hooks(settings: &mut Value, matches: impl Fn(&str) -> bool) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let mut emptied = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let before = groups.len();
        groups.retain_mut(|g| {
            let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = list.len();
            list.retain(|h| {
                !h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(&matches)
            });
            !(list.is_empty() && before > 0)
        });
        if groups.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    hooks.retain(|event, _| !emptied.contains(event));
    if hooks.is_empty() {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("hooks");
        }
    }
}

/// `kind`'s settings with paddock's hooks (calling `exe`) and without old
/// ones.
pub fn install(mut settings: Value, exe: &str, kind: AgentKind) -> Result<Value> {
    remove_hooks(&mut settings, |c| is_ours(c) || c.contains(LEGACY));
    let Some(obj) = settings.as_object_mut() else {
        bail!("settings are not a JSON object");
    };
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        bail!("\"hooks\" in settings is not an object");
    };
    for event in events(kind) {
        let group = hook_group(exe, kind, event);
        match hooks.entry(*event).or_insert_with(|| json!([])) {
            Value::Array(groups) => groups.push(group),
            _ => bail!("hooks for {event} in settings are not a list"),
        }
    }
    Ok(settings)
}

/// Settings without paddock's hooks; everything else stays.
pub fn uninstall(mut settings: Value) -> Value {
    remove_hooks(&mut settings, is_ours);
    settings
}

/// Whether the settings hold one of paddock's hooks for `event`.
fn has_hook(settings: &Value, event: &str) -> bool {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| g["hooks"].as_array())
        .flatten()
        .filter_map(|h| h["command"].as_str())
        .any(is_ours)
}

/// Whether the settings enable the paddock plugin.
fn plugin_enabled(settings: &Value) -> bool {
    settings["enabledPlugins"][PLUGIN_KEY].as_bool() == Some(true)
}

/// Whether a hook from `source` (`$PADDOCK_HOOK_SOURCE`) records `event`:
/// the plugin's hook is skipped when the settings (read by `settings`, and
/// only for the plugin's hook) hold paddock's own hook for that event, since
/// that one records it. Without readable settings (`None`) Claude runs no
/// settings hooks, so the plugin's hook records.
fn should_record(source: &str, settings: impl FnOnce() -> Option<Value>, event: &str) -> bool {
    source != PLUGIN_SOURCE || !settings().is_some_and(|s| has_hook(&s, event))
}

fn read_settings(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Edit a settings file in place; a missing file counts as `{}`. The file is
/// replaced atomically.
pub fn edit_file(path: &Path, f: impl FnOnce(Value) -> Result<Value>) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let settings: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;
    let mut out = serde_json::to_string_pretty(&f(settings)?)?;
    out.push('\n');
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Replace what a symlink points to (a dotfiles repo), not the link.
    let path = &std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = path.with_extension("json.paddock-tmp");
    // The settings may hold tokens: the new file is private until it gets
    // the old one's permissions. A leftover from a crash is replaced.
    let _ = std::fs::remove_file(&tmp);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(out.as_bytes()))
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, meta.permissions())
            .with_context(|| format!("cannot set permissions on {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// Whether `paddock hooks install|uninstall` without `--agent` acts on
/// `kind`, with environment `var`. Claude always, as before kinds existed,
/// whether or not `claude` is on `PATH`; Gemini when `gemini` is on `PATH`.
fn by_default(kind: AgentKind, var: impl Fn(&str) -> Option<String>) -> bool {
    match kind {
        AgentKind::Claude => true,
        AgentKind::Gemini => on_path("gemini", var("PATH").as_deref().unwrap_or_default()),
    }
}

/// Whether an executable file named `program` is in a directory of `path`
/// (a `PATH` value).
fn on_path(program: &str, path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter_map(|dir| std::fs::metadata(dir.join(program)).ok())
        .any(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The kinds `paddock hooks` acts on, from its arguments after the action:
/// each `--agent <kind>` once (repeatable), else every kind [`by_default`]
/// with environment `var`.
fn kinds(args: &[&str], var: impl Fn(&str) -> Option<String>) -> Result<Vec<AgentKind>> {
    let mut kinds = Vec::new();
    let mut rest = args.iter();
    while let Some(&arg) = rest.next() {
        if arg != "--agent" {
            bail!("unexpected argument {arg:?}: use --agent <kind>");
        }
        let Some(&tag) = rest.next() else {
            bail!("--agent needs a kind: {}", tags());
        };
        let kind = AgentKind::parse(tag)
            .with_context(|| format!("unknown agent {tag:?}: use {}", tags()))?;
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        kinds = AgentKind::ALL
            .into_iter()
            .filter(|&k| by_default(k, &var))
            .collect();
    }
    Ok(kinds)
}

/// Every kind's tag, for messages.
fn tags() -> String {
    let all: Vec<&str> = AgentKind::ALL.iter().map(|k| k.tag()).collect();
    all.join(", ")
}

/// `paddock hooks install|uninstall [--agent <kind>]...`.
pub fn command(args: &[&str]) -> Result<()> {
    let Some((&action, rest)) = args.split_first() else {
        bail!("hooks needs an action: use install or uninstall");
    };
    if !matches!(action, "install" | "uninstall") {
        bail!("unknown hooks action {action:?}: use install or uninstall");
    }
    // Every argument is checked before any file is touched.
    let var = |k: &str| std::env::var(k).ok();
    let kinds = kinds(rest, var)?;
    let paths: Vec<(AgentKind, PathBuf)> = kinds
        .into_iter()
        .map(|k| Ok((k, settings_path(k, var)?)))
        .collect::<Result<_>>()?;
    for (kind, path) in paths {
        if action == "install" {
            install_in(kind, &path)?;
        } else {
            edit_file(&path, |s| Ok(uninstall(s)))?;
            println!("paddock hooks removed from {}", path.display());
        }
        println!("{}", restart(kind));
    }
    Ok(())
}

/// `paddock hooks install` for `kind`, whose settings are at `path`.
fn install_in(kind: AgentKind, path: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find paddock's own path")?;
    let exe = exe.to_string_lossy().into_owned();
    let plugin =
        kind == AgentKind::Claude && read_settings(path).is_some_and(|s| plugin_enabled(&s));
    edit_file(path, |s| install(s, &exe, kind))?;
    println!("paddock hooks installed in {}", path.display());
    if plugin {
        println!(
            "note: the paddock plugin is enabled and carries these hooks too; \
             paddock records each event once, from these. To keep only the \
             plugin's, run `paddock hooks uninstall --agent claude`."
        );
    }
    Ok(())
}

/// The agent and event of `paddock hook`'s arguments: `<event>` is
/// Claude's, `--agent <kind> <event>` names the kind.
pub fn hook_args<'a>(args: &[&'a str]) -> Result<(AgentKind, &'a str)> {
    match args {
        [event] => Ok((AgentKind::Claude, event)),
        ["--agent", tag, event] => {
            let kind = AgentKind::parse(tag).with_context(|| format!("unknown agent {tag:?}"))?;
            Ok((kind, event))
        }
        _ => bail!("usage: paddock hook [--agent <kind>] <event>"),
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `paddock hook [--agent <kind>] <event>` (`args` after `hook`): record
/// the event, in its server's log if it comes from one of paddock's
/// terminals, else in the external log (it runs for every agent on the
/// machine, so that stays one append). Never prints, never fails, even on
/// arguments it does not understand: the agent would show the output to the
/// model, or block on a failing hook.
pub fn hook(args: &[&str]) -> ! {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| {
        if let Err(e) = record(args) {
            log_error(&e);
        }
    });
    std::process::exit(0)
}

fn record(args: &[&str]) -> Result<()> {
    // Always drain stdin, so the agent never writes into a closed pipe.
    let mut payload = String::new();
    std::io::stdin().read_to_string(&mut payload)?;
    let (kind, event) = hook_args(args)?;
    let var = |k: &str| std::env::var(k).ok();
    let text = |k: &str| var(k).unwrap_or_default();
    // Before anything else, for paddock's terminals and external sessions
    // alike. Only the plugin's hook reads the settings, to defer to them.
    let settings = || {
        settings_path(kind, var)
            .ok()
            .and_then(|p| read_settings(&p))
    };
    if !should_record(&text(SOURCE_VAR), settings, event) {
        return Ok(()); // settings.json's paddock hook records this one
    }
    let paths = Paths::resolve(var)?;
    // Outside paddock's terminals, an external session: kept by session
    // id, in a log of its own.
    let (log_path, pane) = match log::terminal(&text("TMUX"), &text("TMUX_PANE"), &paths.socket) {
        Some((pid, pane)) => (log::path(&paths.state_dir, pid), pane),
        None => (log::external_path(&paths.state_dir), String::new()),
    };
    let mut r = log::record(kind, event, &pane, &payload, now_ms());
    if pane.is_empty() && r.session_id.is_none() {
        return Ok(()); // an external event that cannot be kept apart
    }
    if sessions::needs_branch(&r) {
        r.branch = r.cwd.as_deref().and_then(git::task_branch);
        if let (Some(id), None) = (&r.agent_id, &r.description) {
            let v: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
            r.description = v
                .get("transcript_path")
                .and_then(Value::as_str)
                .and_then(|t| log::meta_description(t, id));
        }
    }
    log::append(&log_path, &r)
}

/// Best effort: hooks have nowhere else to report.
fn log_error(e: &anyhow::Error) {
    let Ok(paths) = Paths::from_env() else {
        return;
    };
    let line = format!("{} paddock hook: {e:#}\n", now_ms());
    let _ = std::fs::create_dir_all(&paths.state_dir);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.state_dir.join("hook-errors.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/bin/paddock";
    const CLAUDE: AgentKind = AgentKind::Claude;
    const GEMINI: AgentKind = AgentKind::Gemini;

    fn supacode() -> Value {
        json!({
            "model": "opus",
            "hooks": {
                "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "supacode-notify # supacode-managed-hook"}]}],
                "Notification": [{"matcher": "permission_prompt", "hooks": [{"type": "command",
                    "command": "$HOME/.config/tmux/plugins/tmux-claude-session-manager/scripts/state.sh waiting"}]}]
            }
        })
    }

    fn commands(s: &Value, event: &str) -> Vec<String> {
        s["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
            .map(|h| h["command"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn install_adds_every_event_and_keeps_other_hooks() {
        let s = install(supacode(), EXE, CLAUDE).unwrap();
        for event in EVENTS {
            assert!(
                commands(&s, event).contains(&format!("{EXE} hook {event}")),
                "{event}"
            );
        }
        assert_eq!(
            commands(&s, "Stop")[0],
            "supacode-notify # supacode-managed-hook"
        );
        assert_eq!(s["model"], "opus");
    }

    #[test]
    fn install_removes_the_old_tmux_plugin_hooks() {
        let s = install(supacode(), EXE, CLAUDE).unwrap();
        assert_eq!(
            commands(&s, "Notification"),
            [format!("{EXE} hook Notification")]
        );
    }

    #[test]
    fn install_is_idempotent_even_after_paddock_moved() {
        let once = install(supacode(), "/old/paddock", CLAUDE).unwrap();
        let twice = install(install(once, EXE, CLAUDE).unwrap(), EXE, CLAUDE).unwrap();
        assert_eq!(twice, install(supacode(), EXE, CLAUDE).unwrap());
        assert_eq!(commands(&twice, "Stop").len(), 2);
    }

    #[test]
    fn uninstall_removes_only_paddocks_hooks() {
        let s = uninstall(install(json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}}), EXE, CLAUDE).unwrap());
        assert_eq!(
            s,
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}})
        );
        assert_eq!(
            uninstall(install(json!({}), EXE, CLAUDE).unwrap()),
            json!({})
        );
    }

    #[test]
    fn odd_paths_are_quoted_and_still_recognised() {
        let s = install(json!({}), "/Users/me/my apps/paddock", CLAUDE).unwrap();
        let c = &commands(&s, "Stop")[0];
        assert_eq!(c, "'/Users/me/my apps/paddock' hook Stop");
        assert!(is_ours(c));
        assert!(!is_ours("paddock-other hook Stop"));
        assert!(!is_ours("echo paddock hook Stop"));
        assert!(is_ours("paddock hook Stop"));
        assert!(is_ours("/opt/bin/paddock hook --agent claude Stop"));
        assert!(is_ours("'/my apps/paddock' hook --agent claude Stop"));
    }

    #[test]
    fn a_hook_names_its_agent_or_is_claudes() {
        assert_eq!(hook_args(&["Stop"]).unwrap(), (CLAUDE, "Stop"));
        assert_eq!(
            hook_args(&["--agent", "claude", "Stop"]).unwrap(),
            (CLAUDE, "Stop")
        );
        assert!(hook_args(&["--agent", "nope", "Stop"]).is_err());
        assert!(hook_args(&[]).is_err());
        assert!(hook_args(&["--agent", "claude"]).is_err());
        assert!(hook_args(&["Stop", "extra"]).is_err());
    }

    #[test]
    fn claudes_hooks_are_written_as_before_kinds() {
        assert_eq!(
            hook_command(EXE, CLAUDE, "Stop"),
            format!("{EXE} hook Stop")
        );
        let (kind, event) = hook_args(&["Stop"]).unwrap();
        assert_eq!(hook_command("paddock", kind, event), "paddock hook Stop");
    }

    /// An environment with only `PATH` set to `path`.
    fn path_env(path: &str) -> impl Fn(&str) -> Option<String> + '_ {
        move |k| (k == "PATH").then(|| path.to_string())
    }

    #[test]
    fn hooks_act_on_the_agents_named_or_by_default_on_claude() {
        let none = path_env("");
        assert_eq!(kinds(&[], &none).unwrap(), [CLAUDE]);
        assert_eq!(kinds(&["--agent", "claude"], &none).unwrap(), [CLAUDE]);
        assert_eq!(
            kinds(&["--agent", "claude", "--agent", "claude"], &none).unwrap(),
            [CLAUDE],
            "named twice, acted on once"
        );
        assert_eq!(kinds(&["--agent", "gemini"], &none).unwrap(), [GEMINI]);
        assert_eq!(
            kinds(&["--agent", "gemini", "--agent", "claude"], &none).unwrap(),
            [GEMINI, CLAUDE]
        );
        assert!(kinds(&["--agent", "nope"], &none).is_err());
        assert!(kinds(&["--agent"], &none).is_err());
        assert!(kinds(&["claude"], &none).is_err());
    }

    /// A directory holding a file `name` with mode `mode`.
    fn bin_with(name: &str, mode: u32) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(name);
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        dir
    }

    #[test]
    fn hooks_act_on_gemini_by_default_only_when_it_is_on_path() {
        let bin = bin_with("gemini", 0o755);
        let dir = bin.path().to_str().unwrap();
        let path = format!("/nowhere:{dir}");
        assert_eq!(kinds(&[], path_env(&path)).unwrap(), [CLAUDE, GEMINI]);
        assert_eq!(
            kinds(&["--agent", "claude"], path_env(&path)).unwrap(),
            [CLAUDE],
            "named: only those"
        );
        assert_eq!(kinds(&[], path_env("/nowhere")).unwrap(), [CLAUDE]);
        assert_eq!(kinds(&[], |_| None).unwrap(), [CLAUDE], "no PATH");
        let plain = bin_with("gemini", 0o644);
        let plain = plain.path().to_str().unwrap();
        assert_eq!(
            kinds(&[], path_env(plain)).unwrap(),
            [CLAUDE],
            "not executable"
        );
        let other = bin_with("gemini-other", 0o755);
        let other = other.path().to_str().unwrap();
        assert_eq!(kinds(&[], path_env(other)).unwrap(), [CLAUDE]);
        std::fs::create_dir(bin.path().join("sub")).unwrap();
        let dir_only = format!("{dir}/sub");
        std::fs::create_dir(bin.path().join("sub/gemini")).unwrap();
        assert_eq!(
            kinds(&[], path_env(&dir_only)).unwrap(),
            [CLAUDE],
            "a directory named gemini"
        );
    }

    fn gemini_settings() -> Value {
        json!({
            "general": {"vimMode": true},
            "hooksConfig": {"enabled": true, "notifications": false},
            "hooks": {
                "disabled": [],
                "BeforeTool": [{"matcher": "write_file|replace", "hooks": [
                    {"name": "security-check", "type": "command", "command": "$GEMINI_PROJECT_DIR/.gemini/hooks/security.sh", "timeout": 5000}
                ]}],
                "AfterAgent": [{"hooks": [{"type": "command", "command": "notify-done"}]}]
            }
        })
    }

    #[test]
    fn gemini_hooks_name_their_agent_and_have_a_timeout() {
        let s = install(json!({}), EXE, GEMINI).unwrap();
        let mut events: Vec<&String> = s["hooks"].as_object().unwrap().keys().collect();
        events.sort();
        let mut expected = GEMINI_EVENTS.to_vec();
        expected.sort();
        assert_eq!(events, expected);
        for event in GEMINI_EVENTS {
            assert_eq!(
                s["hooks"][event],
                json!([{"hooks": [{
                    "type": "command",
                    "command": format!("{EXE} hook --agent gemini {event}"),
                    "timeout": 10_000
                }]}]),
                "{event}"
            );
            assert!(is_ours(&hook_command(EXE, GEMINI, event)));
            let (kind, e) = hook_args(&["--agent", "gemini", event]).unwrap();
            assert_eq!((kind, e), (GEMINI, *event));
        }
    }

    #[test]
    fn gemini_install_and_uninstall_keep_the_users_hooks_and_settings() {
        let s = install(gemini_settings(), EXE, GEMINI).unwrap();
        assert_eq!(s["general"], json!({"vimMode": true}));
        assert_eq!(
            s["hooksConfig"],
            json!({"enabled": true, "notifications": false})
        );
        assert_eq!(s["hooks"]["disabled"], json!([]));
        assert_eq!(
            s["hooks"]["BeforeTool"],
            gemini_settings()["hooks"]["BeforeTool"]
        );
        assert_eq!(
            commands(&s, "AfterAgent"),
            [
                "notify-done".to_string(),
                format!("{EXE} hook --agent gemini AfterAgent")
            ]
        );
        assert_eq!(
            install(s.clone(), "/moved/paddock", GEMINI).unwrap(),
            install(gemini_settings(), "/moved/paddock", GEMINI).unwrap(),
            "idempotent, even after paddock moved"
        );
        assert_eq!(uninstall(s), gemini_settings(), "exactly as before");
    }

    #[test]
    fn uninstall_keeps_entries_it_did_not_empty() {
        let s = json!({"hooks": {"disabled": [], "Stop": []}});
        assert_eq!(uninstall(s.clone()), s);
        let theirs = json!({"hooks": {"Stop": [{"hooks": []}]}});
        assert_eq!(uninstall(theirs.clone()), theirs);
    }

    /// The plugin hooks' `timeout`, in seconds.
    const PLUGIN_TIMEOUT_S: u64 = 10;

    /// The plugin's command for `event`: quiet and exit 0 when `paddock` is
    /// not on PATH (stdin drained), nothing printed when it is.
    fn plugin_command(event: &str) -> String {
        format!(
            "command -v paddock >/dev/null 2>&1 || {{ cat >/dev/null; exit 0; }}; \
             export {SOURCE_VAR}={PLUGIN_SOURCE}; paddock hook {event} >/dev/null 2>&1 || true"
        )
    }

    /// `plugins/paddock/hooks/hooks.json` has exactly `EVENTS`, with the
    /// groups (matchers) `install` writes and the plugin's command: the two
    /// sources cannot drift apart.
    #[test]
    fn the_plugin_hooks_file_matches_events_and_install() {
        let file: Value =
            serde_json::from_str(include_str!("../plugins/paddock/hooks/hooks.json")).unwrap();
        let settings = install(json!({}), "paddock", CLAUDE).unwrap();
        let mut expected = Map::new();
        for event in EVENTS {
            let mut groups = settings["hooks"][event].clone();
            for g in groups.as_array_mut().unwrap() {
                g["hooks"][0]["command"] = Value::String(plugin_command(event));
                // A hung `paddock` stalls Claude 10 s at most (default 600).
                // SessionEnd keeps Claude's 1.5 s budget, which a longer
                // timeout would raise.
                if *event != "SessionEnd" {
                    g["hooks"][0]["timeout"] = json!(PLUGIN_TIMEOUT_S);
                }
            }
            expected.insert((*event).to_string(), groups);
        }
        assert_eq!(file["hooks"], Value::Object(expected));
        let mut keys: Vec<&String> = file.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["description", "hooks"]);
    }

    /// [`should_record`] with `settings` already read.
    fn records(source: &str, settings: Option<&Value>, event: &str) -> bool {
        should_record(source, || settings.cloned(), event)
    }

    #[test]
    fn only_the_plugins_hook_reads_the_settings() {
        let unread = || -> Option<Value> { panic!("settings read") };
        assert!(should_record("", unread, "Stop"));
        assert!(should_record("other", unread, "Stop"));
    }

    #[test]
    fn a_plugin_hook_defers_to_paddocks_hook_in_settings() {
        let both = install(json!({}), EXE, CLAUDE).unwrap();
        for event in EVENTS {
            assert!(records("", Some(&both), event), "settings' own hook");
            assert!(!records(PLUGIN_SOURCE, Some(&both), event), "{event}");
        }
        // Settings with no paddock hook for this event, or none at all.
        assert!(records(PLUGIN_SOURCE, Some(&supacode()), "Stop"));
        assert!(records(PLUGIN_SOURCE, Some(&json!({})), "Stop"));
        assert!(records(PLUGIN_SOURCE, None, "Stop"));
        let only_stop = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "paddock hook Stop"}]}]}});
        assert!(!records(PLUGIN_SOURCE, Some(&only_stop), "Stop"));
        assert!(records(PLUGIN_SOURCE, Some(&only_stop), "SessionStart"));
        // Hooks paddock cannot read count as none.
        assert!(records(
            PLUGIN_SOURCE,
            Some(&json!({"hooks": {"Stop": "oops"}})),
            "Stop"
        ));
    }

    #[test]
    fn the_plugin_counts_as_enabled_only_when_set_to_true() {
        assert!(plugin_enabled(
            &json!({"enabledPlugins": {"paddock@paddock": true}})
        ));
        assert!(!plugin_enabled(
            &json!({"enabledPlugins": {"paddock@paddock": false}})
        ));
        assert!(!plugin_enabled(
            &json!({"enabledPlugins": {"paddock@other": true}})
        ));
        assert!(!plugin_enabled(&supacode()));
    }

    #[test]
    fn install_refuses_settings_it_does_not_understand() {
        assert!(install(json!([]), EXE, CLAUDE).is_err());
        assert!(install(json!({"hooks": {"Stop": 1}}), EXE, CLAUDE).is_err());
    }

    #[test]
    fn settings_follow_claude_config_dir_or_home() {
        let p = settings_path(CLAUDE, |k| (k == "HOME").then(|| "/h".to_string())).unwrap();
        assert_eq!(p, PathBuf::from("/h/.claude/settings.json"));
        let p = settings_path(CLAUDE, |k| {
            (k == "CLAUDE_CONFIG_DIR").then(|| "/c".to_string())
        })
        .unwrap();
        assert_eq!(p, PathBuf::from("/c/settings.json"));
    }

    #[test]
    fn geminis_settings_follow_gemini_cli_home_or_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let p = settings_path(GEMINI, env(&[("HOME", "/h")])).unwrap();
        assert_eq!(p, PathBuf::from("/h/.gemini/settings.json"));
        let p = settings_path(GEMINI, env(&[("HOME", "/h"), ("GEMINI_CLI_HOME", "/g")])).unwrap();
        assert_eq!(p, PathBuf::from("/g/.gemini/settings.json"));
        let p = settings_path(GEMINI, env(&[("HOME", "/h"), ("GEMINI_CLI_HOME", "")])).unwrap();
        assert_eq!(p, PathBuf::from("/h/.gemini/settings.json"), "empty: unset");
        let p = settings_path(GEMINI, env(&[("HOME", "/h"), ("CLAUDE_CONFIG_DIR", "/c")]));
        assert_eq!(p.unwrap(), PathBuf::from("/h/.gemini/settings.json"));
        assert!(settings_path(GEMINI, env(&[])).is_err());
    }

    #[test]
    fn edit_file_creates_and_rewrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude/settings.json");
        edit_file(&path, |s| install(s, EXE, CLAUDE)).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        edit_file(&path, |s| install(s, EXE, CLAUDE)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            first,
            "byte-identical"
        );
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        edit_file(&link, |s| Ok(uninstall(s))).unwrap();
        assert!(link.is_symlink(), "the link stays a link");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
        std::fs::write(&path, "{oops").unwrap();
        assert!(edit_file(&path, |s| install(s, EXE, CLAUDE)).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{oops",
            "left untouched"
        );
    }

    #[test]
    fn edit_file_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        edit_file(&path, |s| install(s, EXE, CLAUDE)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "settings may hold tokens");
    }
}
