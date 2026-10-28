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

    /// The shell command that continues session `id` of this kind where
    /// it was left, typed into a terminal paddock restored; `None` for a
    /// kind paddock does not resume, or an id not safe to type into a
    /// shell (see [`is_session_id`]).
    pub fn resume_command(self, id: &str) -> Option<String> {
        if !is_session_id(id) {
            return None;
        }
        match self {
            AgentKind::Claude => Some(format!("claude --resume {}", quote(id))),
            AgentKind::Gemini | AgentKind::Agy => None,
        }
    }
}

/// Whether `id` looks like a session id (a UUID: hex digits and `-`): one
/// that comes from a hook payload or a file and is typed into a shell,
/// where a control character would end the line even inside quotes.
pub fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
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
        for bad in ["", "x\r; rm -rf ~", "a b", "abc$(id)", "abc\u{1b}"] {
            assert!(!is_session_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_claude_resumes_and_only_a_uuid_shaped_id() {
        let id = "7f3c2a10-1b2c-4d5e-8f90-0123456789ab";
        assert_eq!(
            AgentKind::Claude.resume_command(id).as_deref(),
            Some("claude --resume '7f3c2a10-1b2c-4d5e-8f90-0123456789ab'")
        );
        assert_eq!(AgentKind::Gemini.resume_command(id), None);
        assert_eq!(AgentKind::Agy.resume_command(id), None);
        for bad in ["", "x\r; rm -rf ~", "a b", "abc$(id)", "abc\u{1b}"] {
            assert_eq!(AgentKind::Claude.resume_command(bad), None, "{bad:?}");
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
