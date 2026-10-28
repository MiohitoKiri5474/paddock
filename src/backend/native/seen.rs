//! The agent session last seen in each of the server's terminals, read
//! from its own hook event log, so `layout.json` can name it and a server
//! that restores the layout can resume it (ticket 48). The server reads
//! the log itself: the app may be closed while agents run.

use super::saved::SavedAgent;
use crate::backend::{Instance, TerminalId};
use crate::config::BackendKind;
use crate::sessions::{ends_conversation, log, HookRecord};
use std::collections::HashMap;
use std::path::Path;

/// The sessions seen per terminal, kept up with the log as it grows.
pub struct Seen {
    reader: log::Reader,
    terminals: HashMap<TerminalId, SavedAgent>,
}

impl Seen {
    /// What the native server with instance id `instance`, whose state
    /// directory is `state_dir`, has seen: nothing yet.
    pub fn new(state_dir: &Path, instance: u32) -> Self {
        let instance = Instance {
            kind: BackendKind::Native,
            id: instance,
        };
        Self {
            reader: log::Reader::new(log::path(state_dir, instance)),
            terminals: HashMap::new(),
        }
    }

    /// Take terminal `terminal` as running `agent`, as a restored
    /// terminal resuming it does before its agent reports anything.
    pub fn seed(&mut self, terminal: TerminalId, agent: SavedAgent) {
        self.terminals.insert(terminal, agent);
    }

    /// Read what hooks added to the log since last time.
    pub fn read_new(&mut self) {
        for r in self.reader.read_new() {
            self.fold(&r);
        }
    }

    /// The session last seen in `terminal`, if any.
    pub fn get(&self, terminal: &TerminalId) -> Option<&SavedAgent> {
        self.terminals.get(terminal)
    }

    /// Fold one record in: a session id the terminal has not seen
    /// replaces what it had; its end marks it ended, and only a new start
    /// of it (a resume in the same terminal) takes that back, not a hook
    /// landing late.
    fn fold(&mut self, r: &HookRecord) {
        let Some(session) = r.session_id.as_deref().filter(|s| !s.is_empty()) else {
            return;
        };
        let ended = ends_conversation(r);
        let terminal = TerminalId::new(r.pane.clone());
        match self.terminals.get_mut(&terminal) {
            Some(seen) if seen.kind == r.kind && seen.session == session => {
                if ended {
                    seen.ended = true;
                } else if r.event == "SessionStart" {
                    seen.ended = false;
                }
            }
            _ => {
                let agent = SavedAgent {
                    kind: r.kind,
                    session: session.to_string(),
                    ended,
                };
                self.terminals.insert(terminal, agent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::AgentKind;

    fn record(event: &str, pane: &str, session: &str) -> HookRecord {
        HookRecord {
            event: event.into(),
            pane: pane.into(),
            session_id: Some(session.into()),
            ..HookRecord::default()
        }
    }

    fn seen(records: &[HookRecord]) -> Seen {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = Seen::new(dir.path(), 7);
        for r in records {
            log::append(&log::path(dir.path(), seen_instance()), r).unwrap();
        }
        seen.read_new();
        seen
    }

    fn seen_instance() -> Instance {
        Instance {
            kind: BackendKind::Native,
            id: 7,
        }
    }

    fn agent(session: &str, ended: bool) -> Option<SavedAgent> {
        Some(SavedAgent {
            kind: AgentKind::Claude,
            session: session.into(),
            ended,
        })
    }

    fn at(seen: &Seen, pane: &str) -> Option<SavedAgent> {
        seen.get(&TerminalId::new(pane)).cloned()
    }

    #[test]
    fn each_terminal_keeps_the_session_last_seen_in_it_and_whether_it_ended() {
        let seen = seen(&[
            record("SessionStart", "%1", "a"),
            record("UserPromptSubmit", "%1", "a"),
            record("SessionStart", "%2", "b"),
            record("SessionEnd", "%2", "b"),
        ]);
        assert_eq!(at(&seen, "%1"), agent("a", false));
        assert_eq!(at(&seen, "%2"), agent("b", true));
        assert_eq!(at(&seen, "%3"), None);
    }

    #[test]
    fn a_late_hook_does_not_reopen_an_ended_session_but_a_new_start_does() {
        let late = seen(&[
            record("SessionEnd", "%1", "a"),
            record("PostToolUse", "%1", "a"),
        ]);
        assert_eq!(at(&late, "%1"), agent("a", true));
        let resumed = seen(&[
            record("SessionEnd", "%1", "a"),
            record("SessionStart", "%1", "a"),
        ]);
        assert_eq!(at(&resumed, "%1"), agent("a", false));
    }

    #[test]
    fn clear_ends_the_old_session_and_the_new_one_takes_its_place() {
        let mut end = record("SessionEnd", "%1", "a");
        end.reason = Some("clear".into());
        let seen = seen(&[
            record("SessionStart", "%1", "a"),
            end,
            record("SessionStart", "%1", "b"),
        ]);
        assert_eq!(at(&seen, "%1"), agent("b", false));
    }

    #[test]
    fn a_record_without_a_session_changes_nothing_and_a_seed_counts_as_seen() {
        let mut status = record("OSC7501", "%1", "");
        status.session_id = None;
        let mut seen = seen(&[record("SessionStart", "%1", "a"), status]);
        assert_eq!(at(&seen, "%1"), agent("a", false));
        seen.seed(TerminalId::new("%4"), agent("c", false).unwrap());
        assert_eq!(at(&seen, "%4"), agent("c", false));
    }
}
