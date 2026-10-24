//! Agent hooks. For Claude Code, the paddock plugin carries them
//! (`plugins/paddock/hooks/hooks.json`); `paddock hooks install|uninstall`
//! edits each agent's settings file (Claude's for users without the plugin,
//! Gemini CLI's and Antigravity CLI's always); `paddock hook
//! [--agent <kind>] <event>` records one event (no `--agent`: Claude's, as
//! the hooks installed before kinds existed call it).

use crate::backend;
use crate::git;
use crate::paths::Paths;
use crate::sessions::{self, log, AgentKind};
use crate::shell;
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

/// The Antigravity CLI events paddock listens to. Not `PreToolUse`: agy
/// needs a decision from it. Not `PostToolUse` or `PostInvocation`: they
/// change no state paddock shows (agy's status reports do, in paddock's
/// terminals).
pub const AGY_EVENTS: &[&str] = &["SessionStart", "PreInvocation", "Stop"];

/// The `timeout` of paddock's Antigravity hooks, in seconds (agy's unit;
/// its default is 30 s), as for Gemini.
const AGY_TIMEOUT_S: u64 = 10;

/// The name of paddock's entry in Antigravity's `hooks.json`, which is
/// keyed by hook name: `{"paddock": {"<event>": [...]}}`.
pub const AGY_HOOK_NAME: &str = "paddock";

/// What `paddock hook` prints for an event of `kind`. Claude and Gemini
/// show a hook's output to the model, so it prints nothing; Antigravity
/// reads a JSON reply, and `{}` asks nothing of it for any event.
fn reply(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude | AgentKind::Gemini => "",
        AgentKind::Agy => "{}",
    }
}

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
            AgentKind::Agy => "Antigravity",
        }
    )
}

/// The events of `kind` paddock listens to.
fn events(kind: AgentKind) -> &'static [&'static str] {
    match kind {
        AgentKind::Claude => EVENTS,
        AgentKind::Gemini => GEMINI_EVENTS,
        AgentKind::Agy => AGY_EVENTS,
    }
}

/// Where `kind` reads its user hooks. Claude:
/// `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
/// Gemini: `$GEMINI_CLI_HOME/.gemini/settings.json`, else
/// `~/.gemini/settings.json`. Antigravity: `~/.gemini/config/hooks.json`
/// (its docs name no variable that moves it).
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
        AgentKind::Agy => {
            let home = non_empty("HOME").context("HOME is not set")?;
            Ok(PathBuf::from(home).join(".gemini/config/hooks.json"))
        }
    }
}

/// The command a hook of `kind` runs for `event`: `<exe> hook <event>` for
/// Claude (as before kinds existed, and as the plugin calls it), else
/// `<exe> hook --agent <kind> <event>`.
fn hook_command(exe: &str, kind: AgentKind, event: &str) -> String {
    match kind {
        AgentKind::Claude => format!("{} hook {event}", shell::word(exe)),
        AgentKind::Gemini | AgentKind::Agy => {
            format!("{} hook --agent {} {event}", shell::word(exe), kind.tag())
        }
    }
}

/// The group `install` writes for `event` of `kind`: one command hook.
/// Claude's has no timeout (as before kinds existed); Gemini's has
/// [`GEMINI_TIMEOUT_MS`]. Antigravity's events paddock listens to take
/// their command hooks directly, not in groups: [`AGY_TIMEOUT_S`].
fn hook_group(exe: &str, kind: AgentKind, event: &str) -> Value {
    let command = hook_command(exe, kind, event);
    match kind {
        AgentKind::Claude => json!({"hooks": [{"type": "command", "command": command}]}),
        AgentKind::Gemini => json!({"hooks": [
            {"type": "command", "command": command, "timeout": GEMINI_TIMEOUT_MS}
        ]}),
        AgentKind::Agy => json!({"type": "command", "command": command, "timeout": AGY_TIMEOUT_S}),
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
        let groups_before = groups.len();
        groups.retain_mut(|g| {
            let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let hooks_before = list.len();
            list.retain(|h| {
                !h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(&matches)
            });
            !(list.is_empty() && hooks_before > 0)
        });
        if groups.is_empty() && groups_before > 0 {
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
pub fn install(settings: Value, exe: &str, kind: AgentKind) -> Result<Value> {
    match kind {
        AgentKind::Claude | AgentKind::Gemini => install_events(settings, exe, kind),
        AgentKind::Agy => install_named(settings, exe, kind),
    }
}

/// Settings without paddock's hooks; everything else stays.
pub fn uninstall(mut settings: Value, kind: AgentKind) -> Value {
    match kind {
        AgentKind::Claude | AgentKind::Gemini => remove_hooks(&mut settings, is_ours),
        AgentKind::Agy => {
            if let Some(obj) = settings.as_object_mut() {
                obj.remove(AGY_HOOK_NAME);
            }
        }
    }
    settings
}

/// [`install`] in a hooks file keyed by hook name (Antigravity's): the
/// entry [`AGY_HOOK_NAME`], replaced whole, lists each event's hook. Other
/// named hooks stay as they are.
fn install_named(mut settings: Value, exe: &str, kind: AgentKind) -> Result<Value> {
    let Some(obj) = settings.as_object_mut() else {
        bail!("hooks are not a JSON object");
    };
    let entry: Map<String, Value> = events(kind)
        .iter()
        .map(|event| ((*event).to_string(), json!([hook_group(exe, kind, event)])))
        .collect();
    obj.insert(AGY_HOOK_NAME.to_string(), Value::Object(entry));
    Ok(settings)
}

/// [`install`] in settings with a `hooks` object keyed by event (Claude's
/// and Gemini's).
fn install_events(mut settings: Value, exe: &str, kind: AgentKind) -> Result<Value> {
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

/// Edit a settings file in place; a missing file counts as `{}`. The file is
/// replaced atomically.
#[cfg(test)]
fn edit_file(path: &Path, f: impl FnOnce(Value) -> Result<Value>) -> Result<()> {
    let settings = read_file(path)?.unwrap_or_else(|| json!({}));
    write_file(path, &f(settings)?)
}

/// The settings in the file at `path`, `None` if there is none.
fn read_file(path: &Path) -> Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("invalid JSON in {}", path.display()))
}

/// Replace the settings file at `path` with `settings`, atomically,
/// creating it and its directory if needed.
fn write_file(path: &Path, settings: &Value) -> Result<()> {
    let mut out = serde_json::to_string_pretty(settings)?;
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
/// whether or not `claude` is on `PATH`; Gemini when `gemini` is on
/// `PATH`, Antigravity when `agy` is.
fn by_default(kind: AgentKind, var: impl Fn(&str) -> Option<String>) -> bool {
    match kind {
        AgentKind::Claude => true,
        AgentKind::Gemini => on_path("gemini", var("PATH").as_deref().unwrap_or_default()),
        AgentKind::Agy => on_path("agy", var("PATH").as_deref().unwrap_or_default()),
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
    let action = if action == "install" {
        let exe = std::env::current_exe().context("cannot find paddock's own path")?;
        Action::Install {
            exe: exe.to_string_lossy().into_owned(),
        }
    } else {
        Action::Uninstall
    };
    // Every file is read and edited before any is written: one paddock
    // cannot read (Gemini's with comments, say) leaves them all as they
    // were. Each write is atomic, but a write failing partway leaves the
    // files written before it.
    let mut edits = Vec::new();
    for kind in kinds {
        let path = settings_path(kind, var)?;
        edits.push(plan(&action, kind, path)?);
    }
    for edit in edits {
        edit.apply()?;
    }
    Ok(())
}

/// What `paddock hooks` does to each agent's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Put paddock's hooks in, calling paddock at `exe`.
    Install { exe: String },
    /// Take paddock's hooks out.
    Uninstall,
}

/// What `paddock hooks install|uninstall` does to one agent's settings.
struct Edit {
    kind: AgentKind,
    action: Action,
    path: PathBuf,
    /// The new settings; `None` leaves the file alone: there is nothing to
    /// remove from a file that does not exist, and uninstall never makes one.
    settings: Option<Value>,
    /// Claude's settings enable the paddock plugin.
    plugin: bool,
}

/// The [`Edit`] `action` makes to `kind`'s settings at `path`. The file is
/// read and checked here, never written.
fn plan(action: &Action, kind: AgentKind, path: PathBuf) -> Result<Edit> {
    let read = read_file(&path)?;
    let plugin = kind == AgentKind::Claude && read.as_ref().is_some_and(plugin_enabled);
    let settings = match action {
        Action::Install { exe } => Some(
            install(read.unwrap_or_else(|| json!({})), exe, kind)
                .with_context(|| format!("cannot edit {}", path.display()))?,
        ),
        Action::Uninstall => read.map(|s| uninstall(s, kind)),
    };
    Ok(Edit {
        kind,
        action: action.clone(),
        path,
        settings,
        plugin,
    })
}

impl Edit {
    /// Write the file and say what was done.
    fn apply(self) -> Result<()> {
        let path = self.path.display();
        match &self.settings {
            Some(s) => {
                write_file(&self.path, s)?;
                let done = if matches!(self.action, Action::Install { .. }) {
                    "installed in"
                } else {
                    "removed from"
                };
                println!("paddock hooks {done} {path}");
            }
            None => println!("no paddock hooks to remove: {path} does not exist"),
        }
        if self.plugin && matches!(self.action, Action::Install { .. }) {
            println!(
                "note: the paddock plugin is enabled and carries these hooks too; \
                 paddock records each event once, from these. To keep only the \
                 plugin's, run `paddock hooks uninstall --agent claude`."
            );
        }
        println!("{}", restart(self.kind));
        Ok(())
    }
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
/// machine, so that stays one append). Never fails, even on arguments it
/// does not understand, and prints only the agent's [`reply`] (nothing for
/// Claude and Gemini): the agent would show the output to the model, or
/// block on a failing hook.
pub fn hook(args: &[&str]) -> ! {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| {
        if let Err(e) = record(args) {
            log_error(&e);
        }
    });
    let kind = hook_args(args).map_or(AgentKind::Claude, |(kind, _)| kind);
    let mut out = std::io::stdout();
    let _ = out
        .write_all(reply(kind).as_bytes())
        .and_then(|()| out.flush());
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
            .and_then(|p| read_file(&p).ok().flatten()) // unreadable: none
    };
    if !should_record(&text(SOURCE_VAR), settings, event) {
        return Ok(()); // settings.json's paddock hook records this one
    }
    let paths = Paths::resolve(var)?;
    // Outside paddock's terminals, an external session: kept by session
    // id, in a log of its own.
    let (log_path, pane) = match backend::hook_terminal(var, &paths.socket) {
        Some(t) => (
            log::path(&paths.state_dir, t.instance),
            t.terminal.to_string(),
        ),
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
        let s = uninstall(install(json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}}), EXE, CLAUDE).unwrap(), CLAUDE);
        assert_eq!(
            s,
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}})
        );
        assert_eq!(
            uninstall(install(json!({}), EXE, CLAUDE).unwrap(), CLAUDE),
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
        assert_eq!(uninstall(s, GEMINI), gemini_settings(), "exactly as before");
    }

    const AGY: AgentKind = AgentKind::Agy;

    /// A user's Antigravity `hooks.json`, with named hooks of their own.
    fn agy_hooks() -> Value {
        json!({
            "lint-after-command": {
                "PostToolUse": [{"matcher": "run_command", "hooks": [
                    {"type": "command", "command": "./scripts/lint.sh", "timeout": 10}
                ]}]
            },
            "reminder": {
                "enabled": false,
                "PreInvocation": [{"type": "command", "command": "./scripts/reminder.sh"}]
            }
        })
    }

    #[test]
    fn agy_hooks_are_one_named_entry_with_a_timeout_in_seconds() {
        let s = install(json!({}), EXE, AGY).unwrap();
        let mut want = Map::new();
        for event in AGY_EVENTS {
            want.insert(
                (*event).to_string(),
                json!([{
                    "type": "command",
                    "command": format!("{EXE} hook --agent agy {event}"),
                    "timeout": 10
                }]),
            );
        }
        assert_eq!(s, json!({"paddock": want}));
        for event in AGY_EVENTS {
            assert!(is_ours(&hook_command(EXE, AGY, event)));
            assert_eq!(
                hook_args(&["--agent", "agy", event]).unwrap(),
                (AGY, *event)
            );
        }
        assert_eq!(AGY_EVENTS, ["SessionStart", "PreInvocation", "Stop"]);
    }

    #[test]
    fn agy_install_and_uninstall_keep_the_users_named_hooks() {
        let s = install(agy_hooks(), EXE, AGY).unwrap();
        for name in ["lint-after-command", "reminder"] {
            assert_eq!(s[name], agy_hooks()[name], "{name}");
        }
        assert_eq!(
            install(s.clone(), "/moved/paddock", AGY).unwrap(),
            install(agy_hooks(), "/moved/paddock", AGY).unwrap(),
            "idempotent, even after paddock moved"
        );
        assert_eq!(uninstall(s, AGY), agy_hooks(), "exactly as before");
        assert_eq!(uninstall(json!({}), AGY), json!({}));
        // Only paddock's entry goes: another's hook calling paddock stays.
        let theirs = json!({"mine": {"Stop": [{"type": "command", "command": "paddock hook --agent agy Stop"}]}});
        assert_eq!(uninstall(theirs.clone(), AGY), theirs);
        assert!(install(json!([]), EXE, AGY).is_err());
    }

    #[test]
    fn only_agy_hooks_reply() {
        assert_eq!(reply(CLAUDE), "");
        assert_eq!(reply(GEMINI), "");
        assert_eq!(reply(AGY), "{}");
    }

    #[test]
    fn agys_hooks_file_is_under_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let p = settings_path(AGY, env(&[("HOME", "/h"), ("GEMINI_CLI_HOME", "/g")])).unwrap();
        assert_eq!(p, PathBuf::from("/h/.gemini/config/hooks.json"));
        assert!(settings_path(AGY, env(&[])).is_err());
    }

    #[test]
    fn hooks_act_on_agy_by_default_only_when_it_is_on_path() {
        let bin = bin_with("agy", 0o755);
        let dir = bin.path().to_str().unwrap();
        assert_eq!(kinds(&[], path_env(dir)).unwrap(), [CLAUDE, AGY]);
        let both = bin_with("gemini", 0o755);
        let path = format!("{dir}:{}", both.path().to_str().unwrap());
        assert_eq!(kinds(&[], path_env(&path)).unwrap(), [CLAUDE, GEMINI, AGY]);
        assert_eq!(kinds(&["--agent", "agy"], path_env("")).unwrap(), [AGY]);
    }

    #[test]
    fn uninstall_keeps_entries_it_did_not_empty() {
        let s = json!({"hooks": {"disabled": [], "Stop": []}});
        assert_eq!(uninstall(s.clone(), GEMINI), s);
        let theirs = json!({"hooks": {"Stop": [{"hooks": []}]}});
        assert_eq!(uninstall(theirs.clone(), CLAUDE), theirs);
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
        edit_file(&link, |s| Ok(uninstall(s, CLAUDE))).unwrap();
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
