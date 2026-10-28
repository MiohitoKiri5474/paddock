//! The agent session last seen in each of the server's terminals, read
//! from its own hook event log, so `layout.json` can name it and a server
//! that restores the layout can resume it (ticket 48). The server reads
//! the log itself: the app may be closed while agents run.
//!
//! A session is saved as over once its agent reported its end. One
//! exception covers a reboot, when every process is signalled at once:
//! an agent ended from outside (`SessionEnd` with reason
//! [`OUTSIDE`]) within [`CLOSE_WINDOW`] before the server began to
//! close, or after, is saved as not over, so the next server resumes it.

use super::saved::SavedAgent;
use crate::backend::{Instance, TerminalId};
use crate::config::BackendKind;
use crate::sessions::{ends_conversation, log, AgentKind, HookRecord};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The `SessionEnd` reason of an agent not ended by its user: Claude's
/// catch-all, which a signal (a hang-up, a reboot) gives.
const OUTSIDE: &str = "other";
/// How long before the server's close an end from outside may come and
/// still be taken for the same shutdown (ms): a reboot signals the
/// agents and the server at about the same time.
pub const CLOSE_WINDOW: u64 = 2_000;

/// The sessions seen per terminal, kept up with the log as it grows.
pub struct Seen {
    reader: log::Reader,
    terminals: HashMap<TerminalId, Entry>,
    /// When the server began to close (ms since the Unix epoch).
    closing: Option<u64>,
}

/// A session seen in one terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: AgentKind,
    session: String,
    end: Option<End>,
}

/// An agent's report of its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct End {
    /// When it was logged (ms since the Unix epoch).
    ts: u64,
    /// Ended from outside (reason [`OUTSIDE`]), not by its user.
    outside: bool,
}

impl Seen {
    /// What the native server with instance id `instance`, whose state
    /// directory is `state_dir`, has seen: nothing yet.
    pub fn new(state_dir: &Path, instance: u32) -> Self {
        Self {
            reader: log::Reader::new(log_path(state_dir, instance)),
            terminals: HashMap::new(),
            closing: None,
        }
    }

    /// Take terminal `terminal` as running `agent`, as a restored
    /// terminal resuming it does before its agent reports anything.
    pub fn seed(&mut self, terminal: TerminalId, agent: SavedAgent) {
        let end = agent.over.then_some(End {
            ts: 0,
            outside: false,
        });
        let entry = Entry {
            kind: agent.kind,
            session: agent.session,
            end,
        };
        self.terminals.insert(terminal, entry);
    }

    /// The server began to close at `at` (ms since the Unix epoch): an
    /// end from outside since [`CLOSE_WINDOW`] before it is not saved.
    pub fn close(&mut self, at: u64) {
        self.closing = Some(at);
    }

    /// Read what hooks added to the log since last time.
    pub fn read_new(&mut self) {
        for r in self.reader.read_new() {
            self.fold(&r);
        }
    }

    /// The session last seen in each of `terminals`, in order, as
    /// `layout.json` saves it. Terminals not among them are forgotten:
    /// they are closed (or floats, which are never saved).
    pub fn agents(&mut self, terminals: &[TerminalId]) -> Vec<Option<SavedAgent>> {
        let live: HashSet<&TerminalId> = terminals.iter().collect();
        self.terminals.retain(|id, _| live.contains(id));
        let closing = self.closing;
        let saved = |e: &Entry| SavedAgent {
            kind: e.kind,
            session: e.session.clone(),
            over: e.end.is_some_and(|end| over(end, closing)),
        };
        terminals
            .iter()
            .map(|id| self.terminals.get(id).map(saved))
            .collect()
    }

    /// Fold one record in. Only a `SessionStart` puts another session in
    /// a terminal that has one (records of the old one, its end among
    /// them, may land after the new one's start, as each hook is a
    /// process of its own); a terminal with none takes any. An end marks
    /// the session over; only its own `SessionStart` again (a resume in
    /// the same terminal) takes that back, not a hook landing late.
    fn fold(&mut self, r: &HookRecord) {
        let Some(session) = r.session_id.as_deref().filter(|s| !s.is_empty()) else {
            return;
        };
        let start = r.event == "SessionStart";
        let end = ends_conversation(r).then_some(End {
            ts: r.ts,
            outside: r.reason.as_deref() == Some(OUTSIDE),
        });
        let terminal = TerminalId::new(r.pane.clone());
        match self.terminals.get_mut(&terminal) {
            Some(seen) if seen.kind == r.kind && seen.session == session => {
                if end.is_some() {
                    seen.end = end;
                } else if start {
                    seen.end = None;
                }
            }
            Some(_) if !start => {}
            _ => {
                let entry = Entry {
                    kind: r.kind,
                    session: session.to_string(),
                    end,
                };
                self.terminals.insert(terminal, entry);
            }
        }
    }
}

/// Whether a session that ended at `end` is saved as over, by a server
/// that began to close at `closing` (if it has).
fn over(end: End, closing: Option<u64>) -> bool {
    match closing {
        Some(at) if end.outside => end.ts.saturating_add(CLOSE_WINDOW) < at,
        _ => true,
    }
}

/// The hook log of the native server with instance id `instance`.
fn log_path(state_dir: &Path, instance: u32) -> std::path::PathBuf {
    let instance = Instance {
        kind: BackendKind::Native,
        id: instance,
    };
    log::path(state_dir, instance)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTANCE: u32 = 7;

    fn record(event: &str, pane: &str, session: &str) -> HookRecord {
        HookRecord {
            event: event.into(),
            pane: pane.into(),
            session_id: Some(session.into()),
            ..HookRecord::default()
        }
    }

    /// A `SessionEnd` with `reason`, logged at `ts`.
    fn end(pane: &str, session: &str, reason: &str, ts: u64) -> HookRecord {
        HookRecord {
            reason: Some(reason.into()),
            ts,
            ..record("SessionEnd", pane, session)
        }
    }

    fn seen(records: &[HookRecord]) -> Seen {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = Seen::new(dir.path(), INSTANCE);
        for r in records {
            log::append(&log_path(dir.path(), INSTANCE), r).unwrap();
        }
        seen.read_new();
        seen
    }

    fn agent(session: &str, over: bool) -> Option<SavedAgent> {
        Some(SavedAgent {
            kind: AgentKind::Claude,
            session: session.into(),
            over,
        })
    }

    fn at(seen: &mut Seen, pane: &str) -> Option<SavedAgent> {
        let terminals = ["%1", "%2", "%3", "%4"].map(TerminalId::new);
        let i = terminals.iter().position(|t| t.as_str() == pane).unwrap();
        seen.agents(&terminals).swap_remove(i)
    }

    #[test]
    fn each_terminal_keeps_the_session_last_seen_in_it_and_whether_it_is_over() {
        let mut seen = seen(&[
            record("SessionStart", "%1", "a"),
            record("UserPromptSubmit", "%1", "a"),
            record("SessionStart", "%2", "b"),
            record("SessionEnd", "%2", "b"),
        ]);
        assert_eq!(at(&mut seen, "%1"), agent("a", false));
        assert_eq!(at(&mut seen, "%2"), agent("b", true));
        assert_eq!(at(&mut seen, "%3"), None);
    }

    #[test]
    fn a_late_hook_does_not_reopen_an_ended_session_but_a_new_start_does() {
        let mut late = seen(&[
            record("SessionEnd", "%1", "a"),
            record("PostToolUse", "%1", "a"),
        ]);
        assert_eq!(at(&mut late, "%1"), agent("a", true));
        let mut resumed = seen(&[
            record("SessionEnd", "%1", "a"),
            record("SessionStart", "%1", "a"),
        ]);
        assert_eq!(at(&mut resumed, "%1"), agent("a", false));
    }

    #[test]
    fn clear_ends_the_old_session_and_the_new_one_takes_its_place() {
        let mut seen = seen(&[
            record("SessionStart", "%1", "a"),
            end("%1", "a", "clear", 1),
            record("SessionStart", "%1", "b"),
        ]);
        assert_eq!(at(&mut seen, "%1"), agent("b", false));
    }

    /// `/clear`'s end of the old session, and any late hook of it, may
    /// land after the new session's start: the new one stays.
    #[test]
    fn records_of_the_old_session_landing_after_the_new_start_change_nothing() {
        for late in [
            end("%1", "a", "clear", 1),
            record("PostToolUse", "%1", "a"),
            record("Stop", "%1", "a"),
        ] {
            let mut seen = seen(&[
                record("SessionStart", "%1", "a"),
                record("SessionStart", "%1", "b"),
                late.clone(),
            ]);
            assert_eq!(at(&mut seen, "%1"), agent("b", false), "{late:?}");
        }
        // Over, the new session does not give way to a late hook of the
        // old one either.
        let mut over = seen(&[
            record("SessionStart", "%1", "a"),
            record("SessionStart", "%1", "b"),
            record("SessionEnd", "%1", "b"),
            record("Stop", "%1", "a"),
        ]);
        assert_eq!(at(&mut over, "%1"), agent("b", true));
    }

    #[test]
    fn a_terminal_with_no_session_takes_one_from_any_record() {
        let mut seen = seen(&[record("UserPromptSubmit", "%1", "a")]);
        assert_eq!(at(&mut seen, "%1"), agent("a", false));
    }

    /// A reboot signals the agents as it does the server: an end from
    /// outside just before the close, or after, is not saved as over; one
    /// earlier, or one by the user, is.
    #[test]
    fn an_end_from_outside_close_to_the_close_is_not_over() {
        let close = 100_000;
        let mut seen = seen(&[
            end("%1", "a", OUTSIDE, close - CLOSE_WINDOW / 2),
            end("%2", "b", OUTSIDE, close - CLOSE_WINDOW * 2),
            end("%3", "c", "prompt_input_exit", close - 10),
            end("%4", "d", OUTSIDE, close + 10),
        ]);
        assert_eq!(at(&mut seen, "%1"), agent("a", true), "before the close");
        seen.close(close);
        assert_eq!(at(&mut seen, "%1"), agent("a", false));
        assert_eq!(at(&mut seen, "%2"), agent("b", true), "long before");
        assert_eq!(at(&mut seen, "%3"), agent("c", true), "the user quit");
        assert_eq!(at(&mut seen, "%4"), agent("d", false), "after it");
    }

    #[test]
    fn a_record_without_a_session_changes_nothing_and_a_seed_counts_as_seen() {
        let mut status = record("OSC7501", "%1", "");
        status.session_id = None;
        let mut seen = seen(&[record("SessionStart", "%1", "a"), status]);
        assert_eq!(at(&mut seen, "%1"), agent("a", false));
        seen.seed(TerminalId::new("%4"), agent("c", false).unwrap());
        assert_eq!(at(&mut seen, "%4"), agent("c", false));
    }

    #[test]
    fn closed_terminals_are_forgotten() {
        let mut seen = seen(&[
            record("SessionStart", "%1", "a"),
            record("SessionStart", "%2", "b"),
        ]);
        let only = [TerminalId::new("%2")];
        assert_eq!(seen.agents(&only), [agent("b", false)]);
        assert_eq!(seen.terminals.len(), 1);
    }
}
