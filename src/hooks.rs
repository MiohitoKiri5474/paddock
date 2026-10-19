//! Claude Code hooks: `paddock hooks install|uninstall` edits Claude's
//! `settings.json`, and `paddock hook <event>` records one event.

use crate::git;
use crate::paths::Paths;
use crate::sessions::{self, log};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The events paddock listens to.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PostToolUse",
    "PreCompact",
    "PermissionRequest",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Hooks of the tmux plugin paddock replaces; install removes them.
const LEGACY: &str = "tmux-claude-session-manager";

/// Claude's user settings: `$CLAUDE_CONFIG_DIR/settings.json`, else
/// `~/.claude/settings.json`.
pub fn settings_path(var: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    let non_empty = |k: &str| var(k).filter(|v| !v.is_empty());
    let dir = match non_empty("CLAUDE_CONFIG_DIR") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(non_empty("HOME").context("HOME is not set")?).join(".claude"),
    };
    Ok(dir.join("settings.json"))
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

/// Whether a hook command runs `paddock hook`, whatever paddock's path.
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

/// Drop every hook whose command matches, and any group or event left empty.
fn remove_hooks(settings: &mut Value, matches: impl Fn(&str) -> bool) {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
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
    }
    hooks.retain(|_, g| g.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove("hooks");
        }
    }
}

/// Settings with paddock's hooks (calling `exe`) and without old ones.
pub fn install(mut settings: Value, exe: &str) -> Result<Value> {
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
    for event in EVENTS {
        let group = json!({"hooks": [{"type": "command", "command": format!("{} hook {event}", quote(exe))}]});
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
    std::fs::write(&tmp, out).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// `paddock hooks install|uninstall`.
pub fn command(action: &str) -> Result<()> {
    let path = settings_path(|k| std::env::var(k).ok())?;
    match action {
        "install" => {
            let exe = std::env::current_exe().context("cannot find paddock's own path")?;
            let exe = exe.to_string_lossy().into_owned();
            edit_file(&path, |s| install(s, &exe))?;
            println!("paddock hooks installed in {}", path.display());
        }
        "uninstall" => {
            edit_file(&path, |s| Ok(uninstall(s)))?;
            println!("paddock hooks removed from {}", path.display());
        }
        other => bail!("unknown hooks action {other:?}: use install or uninstall"),
    }
    Ok(())
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `paddock hook <event>`: record the event if it comes from one of
/// paddock's terminals. Never prints, never fails: Claude would show the
/// output to the model, or block on a failing hook.
pub fn hook(event: &str) -> ! {
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(|| {
        if let Err(e) = record(event) {
            log_error(&e);
        }
    });
    std::process::exit(0)
}

fn record(event: &str) -> Result<()> {
    // Always drain stdin, so Claude never writes into a closed pipe.
    let mut payload = String::new();
    std::io::stdin().read_to_string(&mut payload)?;
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let paths = Paths::from_env()?;
    let Some((pid, pane)) = log::terminal(&var("TMUX"), &var("TMUX_PANE"), &paths.socket) else {
        return Ok(()); // not in paddock: an external session
    };
    let mut r = log::record(event, &pane, &payload, now_ms());
    if sessions::needs_branch(&r) {
        r.branch = r.cwd.as_deref().and_then(git::task_branch);
    }
    log::append(&log::path(&paths.state_dir, pid), &r)
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
        let s = install(supacode(), EXE).unwrap();
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
        let s = install(supacode(), EXE).unwrap();
        assert_eq!(
            commands(&s, "Notification"),
            [format!("{EXE} hook Notification")]
        );
    }

    #[test]
    fn install_is_idempotent_even_after_paddock_moved() {
        let once = install(supacode(), "/old/paddock").unwrap();
        let twice = install(install(once, EXE).unwrap(), EXE).unwrap();
        assert_eq!(twice, install(supacode(), EXE).unwrap());
        assert_eq!(commands(&twice, "Stop").len(), 2);
    }

    #[test]
    fn uninstall_removes_only_paddocks_hooks() {
        let s = uninstall(install(json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}}), EXE).unwrap());
        assert_eq!(
            s,
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}})
        );
        assert_eq!(uninstall(install(json!({}), EXE).unwrap()), json!({}));
    }

    #[test]
    fn odd_paths_are_quoted_and_still_recognised() {
        let s = install(json!({}), "/Users/me/my apps/paddock").unwrap();
        let c = &commands(&s, "Stop")[0];
        assert_eq!(c, "'/Users/me/my apps/paddock' hook Stop");
        assert!(is_ours(c));
        assert!(!is_ours("paddock-other hook Stop"));
        assert!(!is_ours("echo paddock hook Stop"));
        assert!(is_ours("paddock hook Stop"));
    }

    #[test]
    fn install_refuses_settings_it_does_not_understand() {
        assert!(install(json!([]), EXE).is_err());
        assert!(install(json!({"hooks": {"Stop": 1}}), EXE).is_err());
    }

    #[test]
    fn settings_follow_claude_config_dir_or_home() {
        let p = settings_path(|k| (k == "HOME").then(|| "/h".to_string())).unwrap();
        assert_eq!(p, PathBuf::from("/h/.claude/settings.json"));
        let p = settings_path(|k| (k == "CLAUDE_CONFIG_DIR").then(|| "/c".to_string())).unwrap();
        assert_eq!(p, PathBuf::from("/c/settings.json"));
    }

    #[test]
    fn edit_file_creates_and_rewrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude/settings.json");
        edit_file(&path, |s| install(s, EXE)).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        edit_file(&path, |s| install(s, EXE)).unwrap();
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
        assert!(edit_file(&path, |s| install(s, EXE)).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{oops",
            "left untouched"
        );
    }
}
