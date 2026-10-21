//! The Claude session rows: a session row per pane where Claude runs or ran,
//! each with its task rows, in display order. They feed the spaces'
//! and tabs' states, `C-a a` and the dashboard.

use crate::git::{Details, Status};
use crate::sessions::{self, Sessions, State, SubagentTask, Task, TaskState};
use std::collections::HashMap;

/// One Claude session, in the pane it runs (or ran) in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pane: String,
    /// The pane's space (tmux session id); empty once closed.
    pub space: String,
    /// The pane's tab (tmux window id); empty once closed.
    pub window: String,
    pub label: String,
    pub state: State,
    pub tasks: Vec<TaskRow>,
    /// The pane was closed: `space` and `window` are empty.
    pub closed: bool,
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
/// In a closed pane, no subagent works on.
fn task_rows(tasks: Vec<&Task>, status: &TaskStatus, session: State, closed: bool) -> Vec<TaskRow> {
    let rows = tasks
        .into_iter()
        .map(|task| TaskRow {
            branch: task.branch.clone(),
            cwd: task.cwd.clone(),
            status: status
                .get(&(task.cwd.clone(), task.branch.clone()))
                .copied(),
            subagent: task.subagent.clone().map(|mut sub| {
                if closed && matches!(sub.state, TaskState::Running | TaskState::Waiting) {
                    sub.state = TaskState::Stopped;
                }
                sub
            }),
        })
        .collect();
    sort_tasks(rows, session)
}

/// Session rows not dismissed, of the given panes and of closed ones, sorted
/// by state, then by space and tab (closed panes last), each with its task
/// rows sorted by state.
pub fn rows<'a>(
    sessions: &Sessions,
    status: &TaskStatus,
    panes: impl IntoIterator<Item = Placed<'a>>,
) -> Vec<Row> {
    let mut placed = Vec::new();
    let mut rows: Vec<((usize, usize), Row)> = panes
        .into_iter()
        .filter_map(|p| {
            placed.push(p.pane);
            let (s, tasks) = sessions.shown(p.pane)?;
            let row = Row {
                pane: p.pane.to_string(),
                space: p.space.to_string(),
                window: p.window.to_string(),
                label: sessions::label(s, p.tab_name),
                state: s.state,
                tasks: task_rows(tasks, status, s.state, false),
                closed: false,
            };
            Some((p.order, row))
        })
        .collect();
    let mut closed: Vec<&str> = sessions.panes().filter(|p| !placed.contains(p)).collect();
    closed.sort_by_key(|p| p.trim_start_matches('%').parse::<u32>().unwrap_or(u32::MAX));
    rows.extend(closed.into_iter().filter_map(|pane| {
        let (s, tasks) = sessions.shown(pane)?;
        let row = Row {
            pane: pane.to_string(),
            space: String::new(),
            window: String::new(),
            label: sessions::label(s, None),
            // Claude no longer runs there.
            state: State::Stopped,
            tasks: task_rows(tasks, status, State::Stopped, true),
            closed: true,
        };
        Some(((0, 0), row))
    }));
    rows.sort_by_key(|(order, r)| (r.state.rank(), r.closed, *order));
    rows.into_iter().map(|(_, r)| r).collect()
}

/// The most urgent state of the open rows `matching`, if any.
pub fn most_urgent<'a>(rows: &'a [Row], matching: impl Fn(&Row) -> bool + 'a) -> Option<State> {
    rows.iter()
        .filter(|r| !r.closed && matching(r))
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
        let got: Vec<String> = rows(&s, &TaskStatus::new(), [placed("$1", "@1", (0, 0), "%1")])[0]
            .tasks
            .iter()
            .map(|t| t.branch.clone())
            .collect();
        assert_eq!(got, ["feat/main", "fix/a"], "waiting before running");
    }

    #[test]
    fn rows_cover_sessions_sorted_by_state_then_item_and_tab_closed_panes_last() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        for (pane, event, cwd) in [
            ("%1", "SessionEnd", "/w/a"),
            ("%2", "Stop", "/w/b"),
            ("%3", "PermissionRequest", "/w/c"),
            ("%4", "Stop", "/w/d"),
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
        ];
        let got: Vec<(String, String, String, State, bool)> = rows(&s, &TaskStatus::new(), panes)
            .into_iter()
            .map(|r| (r.label, r.space, r.window, r.state, r.closed))
            .collect();
        // Claude no longer runs in a closed pane: its row is stopped.
        let want = [
            ("c", "$2", "@3", State::Waiting, false),
            ("api", "$2", "@2", State::Done, false),
            ("d", "$2", "@3", State::Done, false),
            ("a", "$1", "@1", State::Stopped, false),
            ("gone", "", "", State::Stopped, true),
            ("ten", "", "", State::Stopped, true),
        ];
        let want: Vec<(String, String, String, State, bool)> = want
            .iter()
            .map(|&(l, i, w, s, c)| (l.into(), i.into(), w.into(), s, c))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn the_most_urgent_open_row_gives_the_state() {
        let row = |space: &str, state, closed| Row {
            pane: "%1".into(),
            space: space.into(),
            window: "@1".into(),
            label: String::new(),
            state,
            tasks: Vec::new(),
            closed,
        };
        let rows = [
            row("$1", State::Running, false),
            row("$1", State::Idle, false),
            row("", State::Stopped, true),
            row("$2", State::Waiting, false),
        ];
        assert_eq!(most_urgent(&rows, |r| r.space == "$1"), Some(State::Idle));
        assert_eq!(most_urgent(&rows, |r| r.space.is_empty()), None);
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
        let got = &rows(&s, &status, [placed("$1", "@1", (0, 0), "%1")])[0].tasks;
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
    fn a_closed_panes_subagent_tasks_are_stopped() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        s.apply(&HookRecord {
            event: "PermissionRequest".into(),
            pane: "%1".into(),
            agent_id: Some("a1".into()),
            cwd: Some("/r/wt".into()),
            branch: Some("fix/a".into()),
            ..Default::default()
        });
        let got = &rows(&s, &TaskStatus::new(), [])[0].tasks[0];
        assert_eq!(got.subagent.as_ref().unwrap().state, TaskState::Stopped);
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
            state: State::Idle,
            tasks,
            closed: false,
        };
        let rows = [
            row("%1", vec![task("a"), sub, task("b")]),
            row("%2", vec![]),
        ];
        let id = |pane: &str| RowId { pane: pane.into() };
        assert_eq!(row_ids(&rows), [id("%1"), id("%2")]);
    }
}
