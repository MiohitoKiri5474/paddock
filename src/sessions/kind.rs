//! Agent kinds: which coding agent a session belongs to: Claude Code or
//! Gemini CLI. What differs between kinds (how a hook payload becomes
//! a record and a state, which settings file the hooks go in) matches on
//! the kind where it is used, so a new kind is an arm in each, not a
//! rewrite.

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
}

impl AgentKind {
    /// Every kind, in the order `paddock hooks` acts on them.
    pub const ALL: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Gemini];

    /// The kind's short tag: its name on the command line (`--agent`), in
    /// the hook log, on its rows, and for a tab named after its session.
    pub fn tag(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Gemini => "gemini",
        }
    }

    /// Whether the kind lists its running sessions in a snapshot that
    /// [`super::Sessions::reconcile`] folds in. Claude's is `claude agents
    /// --json` ([`super::agents::fetch`]); a kind without one is left to
    /// its hooks.
    pub fn has_snapshot(self) -> bool {
        match self {
            AgentKind::Claude => true,
            AgentKind::Gemini => false,
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
        assert_eq!(AgentKind::parse("Claude"), None);
        assert_eq!(AgentKind::parse(""), None);
        assert_eq!(AgentKind::parse("cursor"), None);
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
