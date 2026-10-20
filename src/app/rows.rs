//! The sidebar's and dashboard's rows: a session row per terminal where
//! Claude runs or ran, each with its task rows, in display order.

use super::state::NAME_STEM;
use crate::git::{Details, Status};
use crate::sessions::{self, Sessions, State, SubagentTask, Task, TaskState};
use std::collections::HashMap;

/// One session row of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub pane: String,
    /// The terminal's tmux session id.
    pub terminal: String,
    /// Index of the tab holding the terminal.
    pub tab: usize,
    pub label: String,
    pub state: State,
    pub tasks: Vec<TaskRow>,
    /// The terminal was closed: `terminal` is empty and `tab` means nothing.
    pub closed: bool,
}

/// A sidebar line: pane `pane`'s session row, or its task row on `branch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowId {
    pub pane: String,
    pub branch: Option<String>,
}

/// Every sidebar line of `rows`, in display order.
pub fn row_ids(rows: &[Row]) -> Vec<RowId> {
    rows.iter()
        .flat_map(|r| {
            let id = |branch| RowId {
                pane: r.pane.clone(),
                branch,
            };
            std::iter::once(id(None)).chain(r.tasks.iter().map(move |t| id(Some(t.branch.clone()))))
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

/// Whether a tab still has the name paddock gave it (so it says nothing).
fn is_default_name(name: &str) -> bool {
    name.strip_prefix(NAME_STEM)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A terminal as the sidebar sees it: its tab's index and name, its tmux
/// session id and its pane id.
pub struct Placed<'a> {
    pub tab: usize,
    pub tab_name: &'a str,
    pub terminal: &'a str,
    pub pane: &'a str,
}

/// Sort order of task rows: waiting, running, done, stopped. A main agent's
/// task row has its session's state, idle counting as done.
fn task_rank(task: &TaskRow, session: State) -> u8 {
    let state = task.subagent.as_ref().map_or(
        match session {
            State::Waiting => TaskState::Waiting,
            State::Running => TaskState::Running,
            State::Idle => TaskState::Done,
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
/// In a closed terminal, no subagent works on.
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

/// Session rows not dismissed, of the given terminals and of closed ones,
/// sorted by state, then by tab (closed terminals last), each with its task
/// rows sorted by state.
pub fn rows<'a>(
    sessions: &Sessions,
    status: &TaskStatus,
    terminals: impl IntoIterator<Item = Placed<'a>>,
) -> Vec<Row> {
    let mut placed = Vec::new();
    let mut rows: Vec<Row> = terminals
        .into_iter()
        .filter_map(|t| {
            placed.push(t.pane);
            let (s, tasks) = sessions.shown(t.pane)?;
            let tab_name = (!is_default_name(t.tab_name)).then_some(t.tab_name);
            Some(Row {
                pane: t.pane.to_string(),
                terminal: t.terminal.to_string(),
                tab: t.tab,
                label: sessions::label(s, tab_name),
                state: s.state,
                tasks: task_rows(tasks, status, s.state, false),
                closed: false,
            })
        })
        .collect();
    let mut closed: Vec<&str> = sessions.panes().filter(|p| !placed.contains(p)).collect();
    closed.sort_by_key(|p| p.trim_start_matches('%').parse::<u32>().unwrap_or(u32::MAX));
    rows.extend(closed.into_iter().filter_map(|pane| {
        let (s, tasks) = sessions.shown(pane)?;
        Some(Row {
            pane: pane.to_string(),
            terminal: String::new(),
            tab: 0,
            label: sessions::label(s, None),
            // Claude no longer runs there.
            state: State::Stopped,
            tasks: task_rows(tasks, status, State::Stopped, true),
            closed: true,
        })
    }));
    rows.sort_by_key(|r| (r.state.rank(), r.closed, r.tab));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let placed = [Placed {
            tab: 0,
            tab_name: "terminal-1",
            terminal: "$1",
            pane: "%1",
        }];
        let got: Vec<String> = rows(&s, &TaskStatus::new(), placed)[0]
            .tasks
            .iter()
            .map(|t| t.branch.clone())
            .collect();
        assert_eq!(got, ["feat/main", "fix/a"], "waiting before running");
    }

    #[test]
    fn rows_cover_sessions_sorted_by_state_then_tab_closed_terminals_last() {
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
        // Tab 3 is split: both of its terminals run Claude.
        let terminals = [
            (0, "terminal-1", "$1", "%1"),
            (1, "api", "$2", "%2"),
            (2, "terminal-3", "$3", "%3"),
            (2, "terminal-3", "$4", "%4"),
            (3, "shell", "$5", "%5"),
            (4, "terminal-6", "$6", "%6"),
        ];
        let placed = terminals
            .iter()
            .map(|&(tab, tab_name, terminal, pane)| Placed {
                tab,
                tab_name,
                terminal,
                pane,
            });
        let got: Vec<(String, String, State, bool)> = rows(&s, &TaskStatus::new(), placed)
            .into_iter()
            .map(|r| (r.label, r.terminal, r.state, r.closed))
            .collect();
        // Claude no longer runs in a closed terminal: its row is stopped.
        let want = [
            ("c", "$3", State::Waiting, false),
            ("api", "$2", State::Idle, false),
            ("d", "$4", State::Idle, false),
            ("a", "$1", State::Stopped, false),
            ("gone", "", State::Stopped, true),
            ("ten", "", State::Stopped, true),
        ];
        let want: Vec<(String, String, State, bool)> = want
            .iter()
            .map(|&(l, t, s, c)| (l.to_string(), t.to_string(), s, c))
            .collect();
        assert_eq!(got, want);
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
        let placed = [Placed {
            tab: 0,
            tab_name: "terminal-1",
            terminal: "$1",
            pane: "%1",
        }];
        let got = &rows(&s, &status, placed)[0].tasks;
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
    fn a_closed_terminals_subagent_tasks_are_stopped() {
        use crate::sessions::{HookRecord, TaskState};
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
    fn row_ids_follow_the_sidebar_lines() {
        let task = |branch: &str| TaskRow {
            branch: branch.into(),
            cwd: "/r".into(),
            status: None,
            subagent: None,
        };
        let row = |pane: &str, tasks| Row {
            pane: pane.into(),
            terminal: String::new(),
            tab: 0,
            label: String::new(),
            state: State::Idle,
            tasks,
            closed: false,
        };
        let rows = [row("%1", vec![task("a"), task("b")]), row("%2", vec![])];
        let id = |pane: &str, branch: Option<&str>| RowId {
            pane: pane.into(),
            branch: branch.map(str::to_string),
        };
        assert_eq!(
            row_ids(&rows),
            [
                id("%1", None),
                id("%1", Some("a")),
                id("%1", Some("b")),
                id("%2", None)
            ]
        );
    }

    #[test]
    fn only_paddocks_own_names_count_as_default() {
        assert!(is_default_name("terminal-12"));
        assert!(!is_default_name("terminal-"));
        assert!(!is_default_name("terminal-x"));
        assert!(!is_default_name("api"));
    }
}
