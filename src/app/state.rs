//! What the running app holds: the tabs, each with its terminals arranged as
//! splits, and what the keyboard is doing (typing into the focused terminal,
//! renaming, reading help).
//!
//! A tab's name is the tmux session name of its first terminal, so it
//! survives reattaching; when that terminal ends, the next one names the tab.

use super::attention::Attention;
use super::layout::{terminal_size, Layout};
use super::mouse::Press;
use super::saved;
use super::tabs::Tabs;
use super::Event;
use crate::config::{key_name, Config};
use crate::git::Status;
use crate::hooks::now_ms;
use crate::input::PrefixKeys;
use crate::sessions::{self, dismissed, Session, Sessions, State, SubagentTask, Task, TaskState};
use crate::terminal::Terminal;
use crate::tmux::server::quote;
use crate::tmux::Server;
use anyhow::{bail, Result};
use ratatui::layout::Rect;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// Rows above the terminals taken by the tab bar.
pub const TAB_BAR_ROWS: u16 = 1;
/// Columns the sidebar takes, its border included, while it is shown.
pub const SIDEBAR_COLS: u16 = 24;
/// New terminals are named `terminal-N` until the user renames them.
const NAME_STEM: &str = "terminal-";

pub enum Mode {
    Normal,
    /// Renaming the current tab; holds the text typed so far.
    Rename(String),
    Help,
    /// The sidebar has the keyboard; holds the selected row.
    Sidebar(Option<RowId>),
    /// Asking before closing terminal `terminal`, where Claude is `state`.
    ConfirmClose {
        terminal: String,
        state: State,
    },
}

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

pub struct App {
    pub(super) server: Server,
    pub(super) events: Sender<Event>,
    /// Each tab's layout, over tmux session ids (`$N`).
    pub(super) tabs: Tabs<Layout<String>>,
    /// Every attached terminal, by tmux session id.
    pub(super) terminals: HashMap<String, Terminal>,
    pub(super) prefix: PrefixKeys,
    pub(super) prefix_name: String,
    pub(super) mode: Mode,
    /// A message for the user, shown in the tab bar until the next key.
    pub(super) notice: Option<String>,
    /// Size of the whole outer screen.
    pub(super) screen: (u16, u16),
    /// Where new terminals start when the current one's directory is unknown.
    pub(super) start_dir: PathBuf,
    /// Where the layout is saved.
    pub(super) state_dir: PathBuf,
    pub(super) sessions: Sessions,
    pub(super) task_status: TaskStatus,
    /// The user hid the sidebar (`C-a b`).
    pub(super) sidebar_hidden: bool,
    /// Where the held mouse button went, until it is released.
    pub(super) press: Option<Press>,
    pub(super) attention: Attention,
}

/// The part of a screen of `(cols, rows)` that shows the current tab's
/// terminals, right of a sidebar `sidebar` columns wide.
pub fn session_area((cols, rows): (u16, u16), sidebar: u16) -> Rect {
    let sidebar = sidebar.min(cols);
    Rect {
        x: sidebar,
        y: TAB_BAR_ROWS.min(rows),
        width: cols - sidebar,
        height: rows.saturating_sub(TAB_BAR_ROWS),
    }
}

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

/// Session rows not dismissed, of the given terminals and of closed ones,
/// sorted by state, then by tab (closed terminals last), each with its task
/// rows.
pub fn rows<'a>(
    sessions: &Sessions,
    status: &TaskStatus,
    terminals: impl IntoIterator<Item = Placed<'a>>,
) -> Vec<Row> {
    // In a closed terminal, no subagent works on.
    let task_rows = |tasks: Vec<&Task>, closed: bool| -> Vec<TaskRow> {
        tasks
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
            .collect()
    };
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
                tasks: task_rows(tasks, false),
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
            tasks: task_rows(tasks, true),
            closed: true,
        })
    }));
    rows.sort_by_key(|r| (r.state.rank(), r.closed, r.tab));
    rows
}

/// A state file that cannot be read is moved to `<name>.bad`, so the next
/// save does not overwrite what it held. Returns the message for the user.
fn set_aside(dir: &std::path::Path, name: &str, e: &anyhow::Error) -> String {
    let path = dir.join(name);
    let bad = dir.join(format!("{name}.bad"));
    match std::fs::rename(&path, &bad) {
        Ok(()) => format!("{e:#} (kept as {})", bad.display()),
        Err(r) => format!("{e:#} (cannot keep it: {r})"),
    }
}

/// The first `terminal-N` not already taken.
pub fn next_name<'a>(taken: impl IntoIterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{NAME_STEM}{n}"))
        .find(|name| !taken.clone().into_iter().any(|t| t == name))
        .unwrap_or_else(|| NAME_STEM.to_string())
}

impl App {
    /// Show the terminals already on the hidden server in their saved layout
    /// (any the layout does not know get a tab each), or start the first one.
    pub fn start(
        server: Server,
        config: &Config,
        screen: (u16, u16),
        events: Sender<Event>,
        start_dir: PathBuf,
        state_dir: PathBuf,
    ) -> Result<Self> {
        let mut app = Self {
            server,
            events,
            tabs: Tabs::default(),
            terminals: HashMap::new(),
            prefix: PrefixKeys::new(config.prefix),
            prefix_name: key_name(&config.prefix),
            mode: Mode::Normal,
            notice: None,
            screen,
            start_dir,
            state_dir,
            sessions: Sessions::default(),
            task_status: TaskStatus::new(),
            sidebar_hidden: false,
            press: None,
            attention: Attention::from_env(),
        };
        match dismissed::load(&app.state_dir) {
            Ok(list) => {
                for d in list {
                    app.sessions.dismiss(&d.pane, d.branch.as_deref(), d.at);
                }
            }
            Err(e) => app.notice = Some(set_aside(&app.state_dir, dismissed::FILE_NAME, &e)),
        }
        let sessions = app.server.sessions();
        let saved = saved::load(&app.state_dir).unwrap_or_else(|e| {
            app.notice = Some(set_aside(&app.state_dir, saved::FILE_NAME, &e));
            None
        });
        let live: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
        let restored = saved::restore(saved, &live);
        let area = app.area();
        for mut layout in restored.tabs {
            // Attach at the final size, so no shell sees a resize. A terminal
            // that cannot be attached (it just ended) keeps paddock from
            // opening no other.
            let mut failed = Vec::new();
            for (id, rect) in layout.rects(area).0 {
                if let Err(e) = app.attach(&id, terminal_size(rect)) {
                    app.notice = Some(format!("{e:#}"));
                    failed.push(id);
                }
            }
            if !failed.iter().all(|id| layout.remove(id)) {
                continue;
            }
            let first = layout.leaves()[0];
            let name = sessions
                .iter()
                .find(|s| s.id == *first)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            app.tabs.push(name, layout);
        }
        app.tabs.select(restored.current);
        if app.tabs.is_empty() {
            app.new_tab()?;
        }
        app.report(|app| app.relayout());
        Ok(app)
    }

    /// Where the current tab's terminals are drawn.
    pub fn area(&self) -> Rect {
        session_area(self.screen, self.sidebar_width())
    }

    /// The sidebar's session rows, in display order.
    pub fn rows(&self) -> Vec<Row> {
        let placed = self.tabs.iter().enumerate().flat_map(|(tab, t)| {
            t.content.leaves().into_iter().filter_map(move |id| {
                Some(Placed {
                    tab,
                    tab_name: &t.name,
                    terminal: id,
                    pane: self.terminals.get(id)?.pane(),
                })
            })
        });
        rows(&self.sessions, &self.task_status, placed)
    }

    /// Dismiss a sidebar row: it leaves the sidebar, and stays gone after
    /// reopening, until Claude reports on it again. Git and terminals are
    /// untouched.
    pub fn dismiss(&mut self, id: &RowId) -> Result<()> {
        self.sessions
            .dismiss(&id.pane, id.branch.as_deref(), now_ms());
        let shown: Vec<(String, String)> = self
            .rows()
            .into_iter()
            .flat_map(|r| r.tasks)
            .map(|t| (t.cwd, t.branch))
            .collect();
        self.task_status.retain(|task, _| shown.contains(task));
        dismissed::save(&self.state_dir, &self.sessions.dismissals())
    }

    /// Tasks whose rows the sidebar shows now, as `(directory, branch)`:
    /// their git status is kept fresh.
    pub fn visible_tasks(&self) -> Vec<(String, String)> {
        if self.sidebar_width() == 0 {
            return Vec::new();
        }
        // Below the sidebar's title, a line per session, then one per task.
        let lines = usize::from(self.screen.1.saturating_sub(1));
        self.rows()
            .into_iter()
            .flat_map(|r| std::iter::once(None).chain(r.tasks.into_iter().map(Some)))
            .take(lines)
            .flatten()
            .map(|t| (t.cwd, t.branch))
            .collect()
    }

    /// Columns the sidebar takes now: none while hidden or without rows.
    pub fn sidebar_width(&self) -> u16 {
        let focused = matches!(self.mode, Mode::Sidebar(_));
        let shown = !self.sidebar_hidden && (focused || !self.rows().is_empty());
        if shown {
            SIDEBAR_COLS
        } else {
            0
        }
    }

    /// Bring terminal `id` to the screen: its tab becomes current and it
    /// gets the focus.
    pub fn jump_to(&mut self, tab: usize, id: &str) {
        self.tabs.select(tab);
        if let Some(t) = self.tabs.current_mut() {
            if t.content.contains(&id.to_string()) {
                t.content.focus = id.to_string();
            }
        }
    }

    /// Attach the terminal in tmux session `id` at `size`.
    fn attach(&mut self, id: &str, size: (u16, u16)) -> Result<()> {
        let events = self.events.clone();
        let session = id.to_string();
        let deliver = move |msg| {
            events
                .send(Event::Tmux {
                    session: session.clone(),
                    msg,
                })
                .is_ok()
        };
        let terminal = Terminal::attach(&self.server, id, size, deliver)?;
        self.terminals.insert(id.to_string(), terminal);
        Ok(())
    }

    /// Terminal `id`'s shell directory, if it still exists.
    fn dir_of(&self, id: &str) -> Option<PathBuf> {
        self.server
            .display(id, "#{pane_current_path}")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
    }

    /// The focused terminal's shell directory, if it still exists.
    fn current_dir(&self) -> Option<PathBuf> {
        self.dir_of(self.current()?.session())
    }

    /// Start a shell named `terminal-N` at `size`, in the focused terminal's
    /// directory, and attach it. Returns its session id.
    pub(super) fn new_terminal(&mut self, size: (u16, u16)) -> Result<String> {
        let dir = self.current_dir();
        self.new_terminal_in(size, dir)
    }

    /// Start a shell named `terminal-N` at `size` in `dir` (else the start
    /// directory) and attach it. Returns its session id.
    fn new_terminal_in(&mut self, size: (u16, u16), dir: Option<PathBuf>) -> Result<String> {
        let sessions = self.server.sessions();
        let name = next_name(sessions.iter().map(|s| s.name.as_str()));
        let dir = dir.unwrap_or_else(|| self.start_dir.clone());
        let session = self.server.new_session(&name, &dir, size)?;
        if let Err(e) = self.attach(&session.id, size) {
            // Best effort: never leave a shell running out of sight.
            let _ = self.server.kill_session(&session.id);
            return Err(e);
        }
        Ok(session.id)
    }

    /// Start a shell in a new tab and make it current.
    pub fn new_tab(&mut self) -> Result<()> {
        let dir = self.current_dir();
        self.open_tab(dir).map(drop)
    }

    /// Start a shell in `dir` in a new tab and make it current. Returns its
    /// session id.
    fn open_tab(&mut self, dir: Option<PathBuf>) -> Result<String> {
        let id = self.new_terminal_in(terminal_size(self.area()), dir)?;
        let name = self.server.display(&id, "#{session_name}")?;
        self.tabs.push(name, Layout::new(id.clone()));
        self.tabs.select_last();
        Ok(id)
    }

    /// The Claude session in terminal `id`, if any.
    fn session_in(&self, id: &str) -> Option<&Session> {
        self.sessions.get(self.terminals.get(id)?.pane())
    }

    /// End terminal `id`, first asking if Claude there is running or
    /// waiting. It leaves its tab when tmux reports that the session ended,
    /// the same way as when its shell exits.
    pub fn close(&mut self, id: &str) -> Result<()> {
        match self.session_in(id).map(|s| s.state) {
            Some(state @ (State::Running | State::Waiting)) => {
                self.mode = Mode::ConfirmClose {
                    terminal: id.to_string(),
                    state,
                };
                Ok(())
            }
            _ => self.server.kill_session(id),
        }
    }

    /// Continue a copy of the Claude session in terminal `id` in a new tab,
    /// in the session's directory: `claude --resume <id> --fork-session`,
    /// typed into the new shell so the tab stays when Claude exits.
    pub fn fork(&mut self, id: &str) -> Result<()> {
        let Some((session_id, cwd)) = self
            .session_in(id)
            .and_then(|s| Some((s.session_id.clone()?, s.cwd.clone())))
        else {
            bail!("no Claude session here");
        };
        // The id comes from a hook payload and is typed into a shell, where a
        // control character would end the line even inside quotes.
        if !is_session_id(&session_id) {
            bail!("unexpected Claude session id");
        }
        let dir = cwd
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| self.dir_of(id));
        let new = self.open_tab(dir)?;
        let command = format!("claude --resume {} --fork-session\r", quote(&session_id));
        match self.terminals.get_mut(&new) {
            Some(t) => t.input(command.as_bytes()),
            None => Ok(()),
        }
    }

    /// Forget a terminal that ended; its tab goes with its last terminal.
    /// Returns whether any tab is left.
    pub fn terminal_ended(&mut self, session: &str) -> bool {
        if let Some(t) = self.terminals.remove(session) {
            t.detach();
        }
        let session = session.to_string();
        let Some(index) = self.tabs.position(|l| l.contains(&session)) else {
            return !self.tabs.is_empty();
        };
        let mut first = None;
        match self.tabs.find_mut(|l| l.contains(&session)) {
            Some(tab) if tab.content.leaves().len() > 1 => {
                tab.content.remove(&session);
                first = Some(tab.content.leaves()[0].clone());
            }
            _ => {
                self.tabs.remove(index);
            }
        }
        self.report(|app| {
            // The tab is named after its first terminal, which may have changed.
            if let Some(first) = first {
                let name = app.server.display(&first, "#{session_name}")?;
                if let Some(tab) = app.tabs.find_mut(|l| l.contains(&first)) {
                    tab.name = name;
                }
            }
            Ok(())
        });
        self.report(|app| app.relayout());
        self.report(|app| app.save());
        !self.tabs.is_empty()
    }

    /// Rename the current tab by renaming its first terminal's tmux session,
    /// so the name survives reattaching. tmux may adjust the name (it
    /// replaces `.` and `:`).
    pub fn rename_current(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        let Some(session) = self.tabs.current().map(|t| t.content.leaves()[0].clone()) else {
            return Ok(());
        };
        if name.is_empty() {
            return Ok(());
        }
        self.server.rename_session(&session, name)?;
        let actual = self.server.display(&session, "#{session_name}")?;
        if let Some(tab) = self.tabs.current_mut() {
            tab.name = actual;
        }
        Ok(())
    }

    /// The focused terminal of the current tab.
    pub fn current(&self) -> Option<&Terminal> {
        self.tabs
            .current()
            .and_then(|t| self.terminals.get(&t.content.focus))
    }

    pub fn current_mut(&mut self) -> Option<&mut Terminal> {
        let id = &self.tabs.current()?.content.focus;
        self.terminals.get_mut(id)
    }

    /// Whether terminal `session` is on screen (in the current tab).
    pub fn shows(&self, session: &str) -> bool {
        self.tabs
            .current()
            .is_some_and(|t| t.content.leaves().iter().any(|id| *id == session))
    }

    pub fn resize(&mut self, screen: (u16, u16)) -> Result<()> {
        self.screen = screen;
        self.relayout()
    }

    /// Detach every terminal; they keep running on the hidden server. The
    /// layout is saved first.
    pub fn detach_all(self) -> Result<()> {
        let saved = self.save();
        for (_, t) in self.terminals {
            t.detach();
        }
        saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_get_the_screen_minus_the_tab_bar() {
        assert_eq!(session_area((100, 30), 0), Rect::new(0, 1, 100, 29));
        assert_eq!(session_area((1, 0), 0), Rect::new(0, 0, 1, 0));
    }

    #[test]
    fn the_sidebar_takes_columns_from_the_left() {
        assert_eq!(
            session_area((100, 30), SIDEBAR_COLS),
            Rect::new(24, 1, 76, 29)
        );
        assert_eq!(session_area((10, 5), SIDEBAR_COLS), Rect::new(10, 1, 0, 4));
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

    #[test]
    fn new_names_fill_the_first_gap() {
        assert_eq!(next_name(Vec::<&str>::new()), "terminal-1");
        assert_eq!(next_name(vec!["terminal-1", "terminal-3"]), "terminal-2");
        assert_eq!(next_name(vec!["terminal-2", "logs"]), "terminal-1");
    }
}

/// Whether `id` looks like a Claude session id (a UUID: hex digits and `-`).
fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

#[cfg(test)]
mod session_id_tests {
    use super::is_session_id;

    #[test]
    fn only_uuid_shaped_session_ids_are_typed_into_a_shell() {
        assert!(is_session_id("7f3c2a10-1b2c-4d5e-8f90-0123456789ab"));
        for bad in ["", "x\r; rm -rf ~", "a b", "abc$(id)", "abc\u{1b}"] {
            assert!(!is_session_id(bad), "{bad:?}");
        }
    }
}
