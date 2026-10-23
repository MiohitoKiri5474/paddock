//! Program states: what an agent's status report (OSC 7501, read from
//! its terminal's output by `terminal::program_status`) says it is
//! doing. Kept with sessions, which record and fold them.

use serde::{Deserialize, Serialize};

/// The state a report gives, as the spec names it (written so in the
/// hook log too).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    /// Waiting on the user (for a `permission`, a `question` or `auth`).
    Blocked,
    Error,
    /// The program takes its reports back.
    Clear,
}

impl ProgramState {
    /// The state named `name`, if the spec has it.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "done" => Self::Done,
            "blocked" => Self::Blocked,
            "error" => Self::Error,
            "clear" => Self::Clear,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_parse_from_and_are_written_as_their_names() {
        for name in ["idle", "working", "done", "blocked", "error", "clear"] {
            let state = ProgramState::parse(name).unwrap();
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{name}\"")
            );
        }
        assert_eq!(ProgramState::parse("Done"), None);
        assert_eq!(ProgramState::parse("sleeping"), None);
    }
}
