//! Agent kinds: which coding agent a session belongs to: Claude Code,
//! Gemini CLI or Antigravity CLI. What differs between kinds (how a hook payload becomes
//! a record and a state, which settings file the hooks go in) matches on
//! the kind where it is used, so a new kind is an arm in each, not a
//! rewrite.

use crate::shell::quote;
use serde::{Deserialize, Serialize};

/// The coding agent a session belongs to. A hook record without one (every
/// log line written before kinds existed) is Claude's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// Claude Code (`claude`).
    #[default]
    Claude,
    /// Gemini CLI (`gemini`).
    Gemini,
    /// Antigravity CLI (`agy`).
    Agy,
}

impl AgentKind {
    /// Every kind, in the order `paddock hooks` acts on them.
    pub const ALL: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Gemini, AgentKind::Agy];

    /// The kind's short tag: its name on the command line (`--agent`), in
    /// the hook log, on its rows, and for a tab named after its session.
    pub fn tag(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Gemini => "gemini",
            AgentKind::Agy => "agy",
        }
    }

    /// Whether the kind lists its running sessions in a snapshot that
    /// [`super::Sessions::reconcile`] folds in. Claude's is `claude agents
    /// --json` ([`super::agents::fetch`]); a kind without one is left to
    /// its hooks.
    pub fn has_snapshot(self) -> bool {
        match self {
            AgentKind::Claude => true,
            AgentKind::Gemini | AgentKind::Agy => false,
        }
    }

    /// The kind tagged `tag` (see [`AgentKind::tag`]).
    pub fn parse(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.tag() == tag)
    }

    /// Whether this is Claude, the kind a record without one has: such
    /// records leave the kind out of the log, so Claude's lines are written
    /// as before kinds existed. By reference, as serde's
    /// `skip_serializing_if` calls it.
    pub fn is_claude(&self) -> bool {
        *self == AgentKind::Claude
    }

    /// The `SessionEnd` reason a signal (a hang-up, a reboot) gives a
    /// session of this kind: Claude's catch-all `other`; Gemini's `exit`,
    /// which its user quitting gives too. `None` for Antigravity, which
    /// sends no `SessionEnd` at all (see [`Self::reports_end`]).
    pub fn outside_end_reason(self) -> Option<&'static str> {
        match self {
            AgentKind::Claude => Some("other"),
            AgentKind::Gemini => Some("exit"),
            AgentKind::Agy => None,
        }
    }

    /// Whether the kind reports its session's end (a `SessionEnd`); one
    /// that does not ends when its terminal is back at the shell.
    pub fn reports_end(self) -> bool {
        self.outside_end_reason().is_some()
    }

    /// The shell command that continues session `id` of this kind where
    /// it was left, typed into a terminal paddock restored in the
    /// session's directory; `None` for an id not safe to type into a
    /// shell (see [`is_session_id`]). Every kind's hooks name a session
    /// by a UUID: Claude's `session_id`; Gemini CLI's, a `randomUUID()`
    /// that `gemini --resume <id>` takes (its sessions are kept per
    /// project directory); Antigravity's `conversationId`, which `agy
    /// --conversation <id>` takes.
    pub fn resume_command(self, id: &str) -> Option<String> {
        if !is_session_id(id) {
            return None;
        }
        let command = match self {
            AgentKind::Claude => "claude --resume",
            AgentKind::Gemini => "gemini --resume",
            AgentKind::Agy => "agy --conversation",
        };
        Some(format!("{command} {}", quote(id)))
    }
}

/// Whether `id` is shaped as a session id, a UUID (groups of 8, 4, 4, 4
/// and 12 hex digits joined by `-`): one that comes from a hook payload
/// or a file and is typed into a shell, where a control character would
/// end the line even inside quotes, and a leading `-` would read as a
/// flag.
pub fn is_session_id(id: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == GROUPS.len()
        && groups
            .iter()
            .zip(GROUPS)
            .all(|(g, n)| g.len() == n && g.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_parse_from_their_tags() {
        for kind in AgentKind::ALL {
            assert_eq!(AgentKind::parse(kind.tag()), Some(kind));
        }
        assert_eq!(AgentKind::parse("claude"), Some(AgentKind::Claude));
        assert_eq!(AgentKind::parse("gemini"), Some(AgentKind::Gemini));
        assert_eq!(AgentKind::parse("agy"), Some(AgentKind::Agy));
        assert_eq!(AgentKind::parse("Claude"), None);
        assert_eq!(AgentKind::parse(""), None);
        assert_eq!(AgentKind::parse("cursor"), None);
    }

    #[test]
    fn only_uuid_shaped_session_ids_are_typed_into_a_shell() {
        assert!(is_session_id("7f3c2a10-1b2c-4d5e-8f90-0123456789ab"));
        assert!(is_session_id("7F3C2A10-1B2C-4D5E-8F90-0123456789AB"));
        for bad in [
            "",
            "x\r; rm -rf ~",
            "a b",
            "abc$(id)",
            "abc\u{1b}",
            "--",
            "-7f3c2a10-1b2c-4d5e-8f90-0123456789ab",
            "abcdef",
            "7f3c2a10-1b2c-4d5e-8f90-0123456789a",
            "7f3c2a10-1b2c-4d5e-8f90-0123456789abc",
            "7f3c2a101b2c-4d5e-8f90-0123456789ab",
            "7f3c2a10-1b2c-4d5e-8f90-0123456789ab-",
            "7g3c2a10-1b2c-4d5e-8f90-0123456789ab",
        ] {
            assert!(!is_session_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn each_kind_resumes_only_a_uuid_shaped_id() {
        let id = "7f3c2a10-1b2c-4d5e-8f90-0123456789ab";
        assert_eq!(
            AgentKind::Claude.resume_command(id).as_deref(),
            Some("claude --resume '7f3c2a10-1b2c-4d5e-8f90-0123456789ab'")
        );
        assert_eq!(
            AgentKind::Gemini.resume_command(id).as_deref(),
            Some("gemini --resume '7f3c2a10-1b2c-4d5e-8f90-0123456789ab'")
        );
        assert_eq!(
            AgentKind::Agy.resume_command(id).as_deref(),
            Some("agy --conversation '7f3c2a10-1b2c-4d5e-8f90-0123456789ab'")
        );
        for kind in AgentKind::ALL {
            for bad in [
                "",
                "x\r; rm -rf ~",
                "a b",
                "abc$(id)",
                "abc\u{1b}",
                "latest",
                "1",
            ] {
                assert_eq!(kind.resume_command(bad), None, "{kind:?} {bad:?}");
            }
        }
    }

    #[test]
    fn a_kind_is_written_as_its_tag() {
        for kind in AgentKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.tag()));
            assert_eq!(serde_json::from_str::<AgentKind>(&json).unwrap(), kind);
        }
    }
}
