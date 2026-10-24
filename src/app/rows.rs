//! The agent session rows: a session row per open pane whose session has
//! not ended, each with its task rows, in display order. They feed the
//! spaces' and tabs' states, the sidebar's agents, `C-a a` and the
//! dashboard. A session that ended (its pane closed, or its agent quit) has no
//! row.
//!
//! External sessions (agents outside paddock's terminals) come last, in
//! their own group: they have no pane, space or tab, so nothing can jump
//! to, close, fork or type into them.

use crate::git::{Details, Status};
use crate::github::Pr;
use crate::sessions::{self, AgentKind, Sessions, State, SubagentTask, Task, TaskState};
use std::collections::HashMap;

/// One agent session, in the pane it runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The pane's id; an external session's key (see
    /// [`sessions::external_key`]).
    pub pane: String,
    /// The pane's space's id; empty for an external session.
    pub space: String,
    /// The pane's tab's id; empty for an external session.
    pub window: String,
    pub label: String,
    /// The session's agent, shown as a tag on its row.
    pub kind: AgentKind,
    pub state: State,
    pub tasks: Vec<TaskRow>,
    /// The agent runs outside paddock's terminals: no screen, nothing to jump to.
    pub external: bool,
}

impl Row {
    /// The branch the main agent works on, if any: its own task row's.
    pub fn main_branch(&self) -> Option<&str> {
        self.tasks
            .iter()
            .find(|t| t.subagent.is_none())
            .map(|t| t.branch.as_str())
    }
}

/// What `Enter` on an external row says instead of jumping.
pub const EXTERNAL_OPEN: &str = "external: open it in its own terminal";
/// What closing an external row says.
pub const EXTERNAL_CLOSE: &str = "external: paddock cannot close it";
/// What forking an external row says.
pub const EXTERNAL_FORK: &str = "external: paddock cannot fork it";
/// Why an external session has no directory to start a shell in.
pub const EXTERNAL_DIR: &str = "external: no terminal directory";
/// What typing into an external row's preview says.
pub const EXTERNAL_TYPE: &str = "external: no screen to type into";

/// Refuse `key` where a terminal is needed when it is an external session's
/// (see [`sessions::is_external`]): paddock has none of it, so the error
/// says `refused`. Every path that reaches a pane (jumping to it, input, its
/// directory, close, fork) checks here, whatever handler called it.
pub fn pane_only(key: &str, refused: &str) -> anyhow::Result<()> {
    if sessions::is_external(key) {
        anyhow::bail!("{refused}");
    }
    Ok(())
}

/// Which external sessions get rows: those heard of at most `timeout` ms
/// before `now` (see [`Sessions::externals`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Externals {
    pub now: u64,
    pub timeout: u64,
}

/// A selectable line: pane `pane`'s session row. Task rows belong to
/// their session and are not selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowId {
    pub pane: String,
}

/// The lines of `rows` the dashboard can select, in display order: the
/// session rows. Task rows belong to their session and are not selected.
pub fn row_ids(rows: &[Row]) -> Vec<RowId> {
    rows.iter()
        .map(|r| RowId {
            pane: r.pane.clone(),
        })
        .collect()
}

/// One task row, under its session row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub branch: String,
    /// The directory git status is read in.
    pub cwd: String,
    /// `None` until git has been read.
    pub status: Option<Status>,
    /// Set for a subagent task: its label and state.
    pub subagent: Option<SubagentTask>,
}

/// Last known git status of each task, by `(directory, branch)`.
pub type TaskStatus = HashMap<(String, String), Status>;

/// Last known dashboard details of each task, by `(directory, branch)`.
pub type TaskDetails = HashMap<(String, String), Details>;

/// Last known pull request of each task, by `(directory, branch)`.
pub type TaskPrs = HashMap<(String, String), Pr>;

/// A pane as the rows see it: its space and window, their places in the
/// sidebar and tab bar, the tab's name if the user gave it one, and its id.
pub struct Placed<'a> {
    pub space: &'a str,
    pub window: &'a str,
    /// `(space index, window index)`, for sorting.
    pub order: (usize, usize),
    pub tab_name: Option<&'a str>,
    pub pane: &'a str,
}

/// Sort order of task rows: waiting, running, done, stopped. A main agent's
/// task row has its session's state, idle counting as done.
fn task_rank(task: &TaskRow, session: State) -> u8 {
    let state = task.subagent.as_ref().map_or(
        match session {
            State::Waiting => TaskState::Waiting,
            State::Running => TaskState::Running,
            State::Done | State::Idle => TaskState::Done,
            State::Stopped | State::Unknown => TaskState::Stopped,
        },
        |s| s.state,
    );
    match state {
        TaskState::Waiting => 0,
        TaskState::Running => 1,
        TaskState::Done => 2,
        TaskState::Stopped => 3,
    }
}

/// `tasks` sorted by state, in tree order within a state, under a session
/// in state `session`.
fn sort_tasks(mut tasks: Vec<TaskRow>, session: State) -> Vec<TaskRow> {
    // Stable: tree order breaks ties.
    tasks.sort_by_key(|t| task_rank(t, session));
    tasks
}

/// The task rows of a session in state `session`, with their git status.
fn task_rows(tasks: Vec<&Task>, status: &TaskStatus, session: State) -> Vec<TaskRow> {
    let rows = tasks
        .into_iter()
        .map(|task| TaskRow {
            branch: task.branch.clone(),
            cwd: task.cwd.clone(),
            status: status
                .get(&(task.cwd.clone(), task.branch.clone()))
                .copied(),
            subagent: task.subagent.clone(),
        })
        .collect();
    sort_tasks(rows, session)
}

/// Session rows of the given (open) panes, sorted by state, then by space
/// and tab, each with its task rows sorted by state. Sessions that ended
/// or were dismissed get none (see [`Sessions::shown`]); nor do sessions of
/// panes not given: their terminal was closed, so they ended with it.
/// Then, if `externals` is given, the external sessions' rows, sorted by
/// state, then label.
pub fn rows<'a>(
    sessions: &Sessions,
    status: &TaskStatus,
    panes: impl IntoIterator<Item = Placed<'a>>,
    externals: Option<Externals>,
) -> Vec<Row> {
    let mut rows: Vec<((usize, usize), Row)> = panes
        .into_iter()
        .filter_map(|p| {
            let (s, tasks) = sessions.shown(p.pane)?;
            let row = Row {
                pane: p.pane.to_string(),
                space: p.space.to_string(),
                window: p.window.to_string(),
                label: sessions::label(s, p.tab_name),
                kind: s.kind,
                state: s.state,
                tasks: task_rows(tasks, status, s.state),
                external: false,
            };
            Some((p.order, row))
        })
        .collect();
    rows.sort_by_key(|(order, r)| (r.state.rank(), *order));
    let mut out: Vec<Row> = rows.into_iter().map(|(_, r)| r).collect();
    if let Some(e) = externals {
        out.extend(external_rows(sessions, status, e));
    }
    out
}

/// The external sessions' rows, sorted by state, then label (then key, so
/// the order holds still).
fn external_rows(sessions: &Sessions, status: &TaskStatus, e: Externals) -> Vec<Row> {
    let mut rows: Vec<Row> = sessions
        .externals(e.now, e.timeout)
        .into_iter()
        .filter_map(|key| {
            let (s, tasks) = sessions.shown(key)?;
            Some(Row {
                pane: key.to_string(),
                space: String::new(),
                window: String::new(),
                label: sessions::label(s, None),
                kind: s.kind,
                state: s.state,
                tasks: task_rows(tasks, status, s.state),
                external: true,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.state.rank(), &a.label, &a.pane).cmp(&(b.state.rank(), &b.label, &b.pane))
    });
    rows
}

/// The most urgent state of the rows `matching`, if any.
pub fn most_urgent<'a>(rows: &'a [Row], matching: impl Fn(&Row) -> bool + 'a) -> Option<State> {
    rows.iter()
        .filter(|r| matching(r))
        .map(|r| r.state)
        .min_by_key(|s| s.rank())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed<'a>(
        space: &'a str,
        window: &'a str,
        order: (usize, usize),
        pane: &'a str,
    ) -> Placed<'a> {
        Placed {
            space,
            window,
            order,
            tab_name: None,
            pane,
        }
    }

    #[test]
    fn task_rows_sort_by_state_in_tree_order_within_a_state() {
        let task = |branch: &str, state: Option<TaskState>| TaskRow {
            branch: branch.into(),
            cwd: "/r".into(),
            status: None,
            subagent: state.map(|state| SubagentTask {
                agent_id: branch.into(),
                description: None,
                state,
            }),
        };
        let tasks = vec![
            task("stopped", Some(TaskState::Stopped)),
            task("done-1", Some(TaskState::Done)),
            task("main", None),
            task("running", Some(TaskState::Running)),
            task("done-2", Some(TaskState::Done)),
            task("waiting", Some(TaskState::Waiting)),
        ];
        let order = |session| -> Vec<String> {
            sort_tasks(tasks.clone(), session)
                .into_iter()
                .map(|t| t.branch)
                .collect()
        };
        // The main agent's task row takes its session's state; idle is done.
        assert_eq!(
            order(State::Idle),
            ["waiting", "running", "done-1", "main", "done-2", "stopped"]
        );
        assert_eq!(
            order(State::Waiting),
            ["main", "waiting", "running", "done-1", "done-2", "stopped"]
        );
        assert_eq!(
            order(State::Unknown),
            ["waiting", "running", "done-1", "done-2", "stopped", "main"]
        );
    }

    #[test]
    fn rows_sort_their_task_rows() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        let record = |event: &str, agent: Option<&str>, branch: &str| HookRecord {
            event: event.into(),
            pane: "%1".into(),
            agent_id: agent.map(str::to_string),
            cwd: Some(format!("/r/{branch}")),
            branch: Some(branch.into()),
            ..Default::default()
        };
        s.apply(&record("PostToolUse", Some("a1"), "fix/a"));
        s.apply(&record("PermissionRequest", None, "feat/main"));
        let got: Vec<String> = rows(
            &s,
            &TaskStatus::new(),
            [placed("$1", "@1", (0, 0), "%1")],
            None,
        )[0]
        .tasks
        .iter()
        .map(|t| t.branch.clone())
        .collect();
        assert_eq!(got, ["feat/main", "fix/a"], "waiting before running");
    }

    #[test]
    fn rows_cover_live_sessions_in_open_panes_sorted_by_state_then_item_and_tab() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        for (pane, event, cwd) in [
            ("%1", "SessionEnd", "/w/ended"),
            ("%2", "Stop", "/w/b"),
            ("%3", "PermissionRequest", "/w/c"),
            ("%4", "Stop", "/w/d"),
            ("%8", "UserPromptSubmit", "/w/e"),
            ("%10", "Stop", "/w/ten"),
            ("%9", "PermissionRequest", "/w/gone"),
            ("%6", "Stop", "/w/dismissed"),
            ("%7", "Stop", "/w/closed-dismissed"),
        ] {
            s.apply(&HookRecord {
                event: event.into(),
                pane: pane.into(),
                session_id: Some(pane.into()),
                cwd: Some(cwd.into()),
                ts: 1,
                ..Default::default()
            });
        }
        s.dismiss("%6", None, 2);
        s.dismiss("%7", None, 2);
        // Space $2's tab @3 is split: both of its panes run Claude.
        let panes = [
            placed("$1", "@1", (0, 0), "%1"),
            Placed {
                tab_name: Some("api"),
                ..placed("$2", "@2", (1, 0), "%2")
            },
            placed("$2", "@3", (1, 1), "%3"),
            placed("$2", "@3", (1, 1), "%4"),
            placed("$3", "@5", (2, 0), "%5"),
            placed("$3", "@6", (2, 1), "%6"),
            placed("$3", "@7", (2, 2), "%8"),
        ];
        let got: Vec<(String, String, String, State)> = rows(&s, &TaskStatus::new(), panes, None)
            .into_iter()
            .map(|r| (r.label, r.space, r.window, r.state))
            .collect();
        // Claude quit in %1, and %7, %9 and %10 were closed: they ended.
        let want = [
            ("c", "$2", "@3", State::Waiting),
            ("api", "$2", "@2", State::Done),
            ("d", "$2", "@3", State::Done),
            ("e", "$3", "@7", State::Running),
        ];
        let want: Vec<(String, String, String, State)> = want
            .iter()
            .map(|&(l, i, w, s)| (l.into(), i.into(), w.into(), s))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn a_new_claude_in_a_terminal_whose_session_ended_gets_a_row() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        let record = |event: &str, session: &str, ts| HookRecord {
            event: event.into(),
            pane: "%1".into(),
            session_id: Some(session.into()),
            cwd: Some("/w/a".into()),
            ts,
            ..Default::default()
        };
        let labels = |s: &Sessions| -> Vec<(String, State)> {
            rows(
                s,
                &TaskStatus::new(),
                [placed("$1", "@1", (0, 0), "%1")],
                None,
            )
            .into_iter()
            .map(|r| (r.label, r.state))
            .collect()
        };
        s.apply(&record("Stop", "s1", 1));
        s.apply(&record("SessionEnd", "s1", 2));
        assert_eq!(labels(&s), []);
        s.apply(&record("SessionStart", "s2", 3));
        assert_eq!(labels(&s), [("a".to_string(), State::Idle)]);
    }

    #[test]
    fn external_rows_come_last_sorted_by_state_and_only_when_asked() {
        use crate::sessions::{external_key, HookRecord};
        let mut s = Sessions::default();
        let record = |event: &str, pane: String, session: &str, cwd: &str| HookRecord {
            event: event.into(),
            pane,
            session_id: Some(session.into()),
            cwd: Some(cwd.into()),
            branch: Some("feat/x".into()),
            ts: 1,
            ..Default::default()
        };
        s.apply(&record("UserPromptSubmit", "%1".into(), "p1", "/w/mine"));
        s.apply(&record("Stop", external_key("e1"), "e1", "/w/web"));
        s.apply(&record(
            "PermissionRequest",
            external_key("e2"),
            "e2",
            "/w/api",
        ));
        let panes = || [placed("$1", "@1", (0, 0), "%1")];
        let shown = |r: Vec<Row>| -> Vec<(String, State, bool, String)> {
            r.into_iter()
                .map(|r| (r.label, r.state, r.external, r.space))
                .collect()
        };
        let externals = Externals {
            now: 2,
            timeout: 1_000,
        };
        let got = shown(rows(&s, &TaskStatus::new(), panes(), Some(externals)));
        let want = [
            ("mine", State::Running, false, "$1"),
            ("api", State::Waiting, true, ""),
            ("web", State::Done, true, ""),
        ];
        let want: Vec<(String, State, bool, String)> = want
            .iter()
            .map(|&(l, st, e, sp)| (l.into(), st, e, sp.into()))
            .collect();
        assert_eq!(got, want);
        let all = rows(&s, &TaskStatus::new(), panes(), Some(externals));
        assert_eq!(all[1].main_branch(), Some("feat/x"));
        assert_eq!(rows(&s, &TaskStatus::new(), panes(), None).len(), 1);
        let later = Externals {
            now: 5_000,
            timeout: 1_000,
        };
        assert_eq!(rows(&s, &TaskStatus::new(), panes(), Some(later)).len(), 1);
    }

    #[test]
    fn only_panes_pass_the_pane_guard() {
        assert!(pane_only("%3", EXTERNAL_CLOSE).is_ok());
        let err = pane_only(&crate::sessions::external_key("e1"), EXTERNAL_CLOSE).unwrap_err();
        assert_eq!(err.to_string(), EXTERNAL_CLOSE);
    }

    #[test]
    fn the_most_urgent_matching_row_gives_the_state() {
        let row = |space: &str, state| Row {
            pane: "%1".into(),
            space: space.into(),
            window: "@1".into(),
            label: String::new(),
            kind: AgentKind::Claude,
            state,
            tasks: Vec::new(),
            external: false,
        };
        let rows = [
            row("$1", State::Running),
            row("$1", State::Idle),
            row("$2", State::Waiting),
        ];
        assert_eq!(most_urgent(&rows, |r| r.space == "$1"), Some(State::Idle));
        assert_eq!(most_urgent(&rows, |r| r.space == "$9"), None);
    }

    #[test]
    fn task_rows_carry_the_status_read_for_their_directory() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        for (branch, cwd) in [("feat/a", "/r"), ("feat/b", "/r/wt")] {
            s.apply(&HookRecord {
                event: "PostToolUse".into(),
                pane: "%1".into(),
                cwd: Some(cwd.into()),
                branch: Some(branch.into()),
                ..Default::default()
            });
        }
        let read = Status {
            ahead_behind: Some((1, 0)),
            changes: Some((3, 2)),
        };
        let status = TaskStatus::from([
            (("/r".to_string(), "feat/a".to_string()), read),
            (("/r".to_string(), "feat/b".to_string()), Status::default()),
        ]);
        let got = &rows(&s, &status, [placed("$1", "@1", (0, 0), "%1")], None)[0].tasks;
        let want = [
            TaskRow {
                branch: "feat/a".into(),
                cwd: "/r".into(),
                status: Some(read),
                subagent: None,
            },
            TaskRow {
                branch: "feat/b".into(),
                cwd: "/r/wt".into(),
                status: None,
                subagent: None,
            },
        ];
        assert_eq!(got, &want);
    }

    #[test]
    fn row_ids_are_the_session_rows_only() {
        let task = |branch: &str| TaskRow {
            branch: branch.into(),
            cwd: "/r".into(),
            status: None,
            subagent: None,
        };
        let sub = TaskRow {
            subagent: Some(SubagentTask {
                agent_id: "a1".into(),
                description: None,
                state: TaskState::Running,
            }),
            ..task("fix/s")
        };
        let row = |pane: &str, tasks| Row {
            pane: pane.into(),
            space: String::new(),
            window: String::new(),
            label: String::new(),
            kind: AgentKind::Claude,
            state: State::Idle,
            tasks,
            external: false,
        };
        let rows = [
            row("%1", vec![task("a"), sub, task("b")]),
            row("%2", vec![]),
        ];
        let id = |pane: &str| RowId { pane: pane.into() };
        assert_eq!(row_ids(&rows), [id("%1"), id("%2")]);
    }
}
