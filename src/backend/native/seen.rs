//! The agent session last seen in each of the server's terminals, read
//! from its own hook event log, so `layout.json` can name it and a server
//! that restores the layout can resume it (ticket 48). The server reads
//! the log itself: the app may be closed while agents run.
//!
//! A session is saved as over once its agent reported its end. A kind
//! that reports none (Antigravity, [`AgentKind::reports_end`]) is over
//! once its terminal is seen back at its shell ([`Seen::foregrounds`],
//! ticket 49), until its agent is seen again (in the foreground, as after
//! `fg`, or in a record). One exception covers a reboot, when every
//! process is signalled at once: an agent ended from outside (a
//! `SessionEnd` with its kind's [`AgentKind::outside_end_reason`], or a
//! terminal back at the shell) within [`CLOSE_WINDOW`] before the server
//! began to close, or after, is saved as not over, so the next server
//! resumes it. (So a Gemini session quit just before the close is
//! resumed: Gemini gives its `exit` for a signal and a quit alike.)

use super::saved::SavedAgent;
use crate::backend::{Instance, TerminalId};
use crate::config::BackendKind;
use crate::sessions::{ends_conversation, is_shell, log, AgentKind, HookRecord};
use std::collections::{HashMap, HashSet};
use std::path::Path;

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
    /// A record of the session came in: its agent ran in this terminal,
    /// not only a restore seeded it there.
    heard: bool,
}

/// An agent's report of its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct End {
    /// When it was logged (ms since the Unix epoch).
    ts: u64,
    /// Ended from outside (see [`AgentKind::outside_end_reason`]), not by
    /// its user.
    outside: bool,
    /// Not reported: its terminal was seen back at the shell (see
    /// [`Seen::foregrounds`]), which seeing its agent again takes back.
    at_shell: bool,
}

impl End {
    /// An end its agent reported at `ts`, from outside or not.
    fn reported(ts: u64, outside: bool) -> Self {
        Self {
            ts,
            outside,
            at_shell: false,
        }
    }
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
        let end = agent.over.then_some(End::reported(0, false));
        let entry = Entry {
            kind: agent.kind,
            session: agent.session,
            end,
            heard: false,
        };
        self.terminals.insert(terminal, entry);
    }

    /// What ran in the foreground of each of `terminals` at `at` (ms since
    /// the Unix epoch): its program's name, `None` when it could not be
    /// read. A session of a kind that reports no end
    /// ([`AgentKind::reports_end`]) is over once its terminal is at a
    /// shell, if its agent has reported in there (a seeded one waits for
    /// its resume to start); taken as ended from outside, as a reboot may
    /// stop it before the server. Its agent in the foreground again (`fg`
    /// after `C-z`: agy is a native program named `agy`) takes that back.
    pub fn foregrounds(&mut self, terminals: &[(TerminalId, Option<String>)], at: u64) {
        for (id, program) in terminals {
            let Some(seen) = self.terminals.get_mut(id) else {
                continue;
            };
            let Some(program) = program.as_deref() else {
                continue;
            };
            if seen.kind.reports_end() {
                continue;
            }
            if is_shell(program) && seen.heard && seen.end.is_none() {
                seen.end = Some(End {
                    ts: at,
                    outside: true,
                    at_shell: true,
                });
            } else if program == seen.kind.tag() && seen.end.is_some_and(|e| e.at_shell) {
                seen.end = None;
            }
        }
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
    /// the same terminal) takes that back, not a hook landing late. An
    /// end only seen at the shell is taken back by any record of the
    /// session: its agent runs.
    fn fold(&mut self, r: &HookRecord) {
        let Some(session) = r.session_id.as_deref().filter(|s| !s.is_empty()) else {
            return;
        };
        let start = r.event == "SessionStart";
        let outside = r.kind.outside_end_reason();
        let outside = outside.is_some_and(|o| r.reason.as_deref() == Some(o));
        let end = ends_conversation(r).then_some(End::reported(r.ts, outside));
        let terminal = TerminalId::new(r.pane.clone());
        match self.terminals.get_mut(&terminal) {
            Some(seen) if seen.kind == r.kind && seen.session == session => {
                seen.heard = true;
                if end.is_some() {
                    seen.end = end;
                } else if start || seen.end.is_some_and(|e| e.at_shell) {
                    seen.end = None;
                }
            }
            Some(_) if !start => {}
            _ => {
                let entry = Entry {
                    kind: r.kind,
                    session: session.to_string(),
                    end,
                    heard: true,
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
        assert_eq!(at(&mut seen, "%1"), agent(CLAUDE, "a", false));
        assert_eq!(at(&mut seen, "%2"), agent(CLAUDE, "b", true));
        assert_eq!(at(&mut seen, "%3"), None);
    }

    #[test]
    fn a_late_hook_does_not_reopen_an_ended_session_but_a_new_start_does() {
        let mut late = seen(&[
            record("SessionEnd", "%1", "a"),
            record("PostToolUse", "%1", "a"),
        ]);
        assert_eq!(at(&mut late, "%1"), agent(CLAUDE, "a", true));
        let mut resumed = seen(&[
            record("SessionEnd", "%1", "a"),
            record("SessionStart", "%1", "a"),
        ]);
        assert_eq!(at(&mut resumed, "%1"), agent(CLAUDE, "a", false));
    }

    #[test]
    fn clear_ends_the_old_session_and_the_new_one_takes_its_place() {
        let mut seen = seen(&[
            record("SessionStart", "%1", "a"),
            end("%1", "a", "clear", 1),
            record("SessionStart", "%1", "b"),
        ]);
        assert_eq!(at(&mut seen, "%1"), agent(CLAUDE, "b", false));
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
            assert_eq!(at(&mut seen, "%1"), agent(CLAUDE, "b", false), "{late:?}");
        }
        // Over, the new session does not give way to a late hook of the
        // old one either.
        let mut over = seen(&[
            record("SessionStart", "%1", "a"),
            record("SessionStart", "%1", "b"),
            record("SessionEnd", "%1", "b"),
            record("Stop", "%1", "a"),
        ]);
        assert_eq!(at(&mut over, "%1"), agent(CLAUDE, "b", true));
    }

    #[test]
    fn a_terminal_with_no_session_takes_one_from_any_record() {
        let mut seen = seen(&[record("UserPromptSubmit", "%1", "a")]);
        assert_eq!(at(&mut seen, "%1"), agent(CLAUDE, "a", false));
    }

    /// A reboot signals the agents as it does the server: an end from
    /// outside just before the close, or after, is not saved as over; one
    /// earlier, or one by the user, is.
    #[test]
    fn an_end_from_outside_close_to_the_close_is_not_over() {
        let close = 100_000;
        let mut seen = seen(&[
            end("%1", "a", "other", close - CLOSE_WINDOW / 2),
            end("%2", "b", "other", close - CLOSE_WINDOW * 2),
            end("%3", "c", "prompt_input_exit", close - 10),
            end("%4", "d", "other", close + 10),
        ]);
        assert_eq!(
            at(&mut seen, "%1"),
            agent(CLAUDE, "a", true),
            "before the close"
        );
        seen.close(close);
        assert_eq!(at(&mut seen, "%1"), agent(CLAUDE, "a", false));
        assert_eq!(at(&mut seen, "%2"), agent(CLAUDE, "b", true), "long before");
        assert_eq!(
            at(&mut seen, "%3"),
            agent(CLAUDE, "c", true),
            "the user quit"
        );
        assert_eq!(at(&mut seen, "%4"), agent(CLAUDE, "d", false), "after it");
    }

    const CLAUDE: AgentKind = AgentKind::Claude;
    const AGY: AgentKind = AgentKind::Agy;

    fn agent(kind: AgentKind, session: &str, over: bool) -> Option<SavedAgent> {
        Some(SavedAgent {
            kind,
            session: session.into(),
            over,
        })
    }

    fn of(kind: AgentKind, r: HookRecord) -> HookRecord {
        HookRecord { kind, ..r }
    }

    /// Gemini says `exit` for a signal as for its user quitting: one close
    /// to the close is not over; `/clear` always is.
    #[test]
    fn a_gemini_exit_close_to_the_close_is_not_over() {
        let gemini = AgentKind::Gemini;
        let close = 100_000;
        let mut seen = seen(&[
            of(gemini, end("%1", "a", "exit", close - 10)),
            of(gemini, end("%2", "b", "exit", close - CLOSE_WINDOW * 2)),
            of(gemini, end("%3", "c", "clear", close - 10)),
            end("%4", "d", "exit", close - 10),
        ]);
        seen.close(close);
        assert_eq!(at(&mut seen, "%1"), agent(gemini, "a", false));
        assert_eq!(at(&mut seen, "%2"), agent(gemini, "b", true));
        assert_eq!(at(&mut seen, "%3"), agent(gemini, "c", true));
        assert_eq!(
            at(&mut seen, "%4"),
            agent(CLAUDE, "d", true),
            "Claude's own"
        );
    }

    /// `terminals`, each running `program`, as [`Seen::foregrounds`]
    /// takes them.
    fn running(terminals: &[&str], program: &str) -> Vec<(TerminalId, Option<String>)> {
        let program = Some(program.to_string());
        terminals
            .iter()
            .map(|t| (TerminalId::new(*t), program.clone()))
            .collect()
    }

    /// A log the test adds records to one at a time.
    struct Log {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        seen: Seen,
    }

    impl Log {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = log_path(dir.path(), INSTANCE);
            let seen = Seen::new(dir.path(), INSTANCE);
            Self {
                _dir: dir,
                path,
                seen,
            }
        }

        fn add(&mut self, r: HookRecord) {
            log::append(&self.path, &r).unwrap();
            self.seen.read_new();
        }
    }

    /// agy reports no end: its session is over once its terminal is back
    /// at the shell, but only one it reported in, not one a restore
    /// seeded before the resumed agy runs.
    #[test]
    fn an_agy_session_is_over_once_its_terminal_is_back_at_the_shell() {
        let mut log = Log::new();
        log.add(of(AGY, record("SessionStart", "%1", "a")));
        log.add(record("SessionStart", "%2", "b"));
        let seen = &mut log.seen;
        assert_eq!(at(seen, "%1"), agent(AGY, "a", false));
        seen.seed(TerminalId::new("%3"), agent(AGY, "c", false).unwrap());
        seen.foregrounds(&running(&["%1", "%2", "%3"], "-zsh"), 1);
        assert_eq!(at(seen, "%1"), agent(AGY, "a", true));
        assert_eq!(
            at(seen, "%2"),
            agent(CLAUDE, "b", false),
            "Claude ends itself"
        );
        assert_eq!(at(seen, "%3"), agent(AGY, "c", false), "seeded");
        // An unreadable foreground changes nothing.
        seen.foregrounds(&[(TerminalId::new("%1"), None)], 2);
        assert_eq!(at(seen, "%1"), agent(AGY, "a", true));
        // The resumed agy reports in: back at the shell, it is over.
        log.add(of(AGY, record("SessionStart", "%3", "c")));
        assert_eq!(at(&mut log.seen, "%3"), agent(AGY, "c", false));
        log.seen.foregrounds(&running(&["%3"], "bash"), 3);
        assert_eq!(at(&mut log.seen, "%3"), agent(AGY, "c", true));
        // Another program in the foreground (vim, after agy quit) does
        // not take it back.
        log.seen.foregrounds(&running(&["%3"], "vim"), 4);
        assert_eq!(at(&mut log.seen, "%3"), agent(AGY, "c", true));
    }

    /// `C-z` puts the shell in agy's place for a while: `fg` (agy in the
    /// foreground again) or any record of its session takes the end back.
    #[test]
    fn a_suspended_agy_session_is_taken_back_by_fg_or_a_record() {
        let mut log = Log::new();
        log.add(of(AGY, record("SessionStart", "%1", "a")));
        log.seen.foregrounds(&running(&["%1"], "zsh"), 1);
        assert_eq!(at(&mut log.seen, "%1"), agent(AGY, "a", true), "C-z");
        log.seen.foregrounds(&running(&["%1"], "agy"), 2);
        assert_eq!(at(&mut log.seen, "%1"), agent(AGY, "a", false), "fg");

        log.seen.foregrounds(&running(&["%1"], "zsh"), 3);
        log.add(of(AGY, record("Stop", "%1", "a")));
        assert_eq!(at(&mut log.seen, "%1"), agent(AGY, "a", false), "a record");
        // A reported end is taken back by its own start only.
        log.add(of(CLAUDE, record("SessionStart", "%2", "b")));
        log.add(of(CLAUDE, record("SessionEnd", "%2", "b")));
        log.seen.foregrounds(&running(&["%2"], "claude"), 4);
        log.add(of(CLAUDE, record("Stop", "%2", "b")));
        assert_eq!(at(&mut log.seen, "%2"), agent(CLAUDE, "b", true));
    }

    /// A reboot signals agy as it does the server: back at the shell just
    /// before the close is not over; long before, it is.
    #[test]
    fn agy_back_at_the_shell_close_to_the_close_is_not_over() {
        let close = 100_000;
        let mut seen = seen(&[
            of(AGY, record("SessionStart", "%1", "a")),
            of(AGY, record("SessionStart", "%2", "b")),
        ]);
        seen.foregrounds(&running(&["%1"], "zsh"), close - CLOSE_WINDOW * 2);
        seen.foregrounds(&running(&["%2"], "zsh"), close - 10);
        // Seen at the shell again later keeps when it was first.
        seen.foregrounds(&running(&["%1"], "zsh"), close - 10);
        seen.close(close);
        assert_eq!(at(&mut seen, "%1"), agent(AGY, "a", true));
        assert_eq!(at(&mut seen, "%2"), agent(AGY, "b", false));
    }

    #[test]
    fn a_record_without_a_session_changes_nothing_and_a_seed_counts_as_seen() {
        let mut status = record("OSC7501", "%1", "");
        status.session_id = None;
        let mut seen = seen(&[record("SessionStart", "%1", "a"), status]);
        assert_eq!(at(&mut seen, "%1"), agent(CLAUDE, "a", false));
        seen.seed(TerminalId::new("%4"), agent(CLAUDE, "c", false).unwrap());
        assert_eq!(at(&mut seen, "%4"), agent(CLAUDE, "c", false));
    }

    #[test]
    fn closed_terminals_are_forgotten() {
        let mut seen = seen(&[
            record("SessionStart", "%1", "a"),
            record("SessionStart", "%2", "b"),
        ]);
        let only = [TerminalId::new("%2")];
        assert_eq!(seen.agents(&only), [agent(CLAUDE, "b", false)]);
        assert_eq!(seen.terminals.len(), 1);
    }
}
