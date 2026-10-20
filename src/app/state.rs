//! What the running app holds: the tabs, each with its terminals arranged as
//! splits, and what the keyboard is doing (typing into the focused terminal,
//! renaming, reading help).
//!
//! A tab's name is the tmux session name of its first terminal, so it
//! survives reattaching; when that terminal ends, the next one names the tab.

use super::attention::Attention;
use super::dashboard::Dashboard;
use super::layout::{terminal_size, Layout};
use super::mouse::Press;
use super::rows::{rows, Placed, Row, RowId, TaskDetails, TaskStatus};
use super::saved;
use super::tabs::Tabs;
use super::Event;
use crate::config::{key_name, Config};
use crate::hooks::now_ms;
use crate::input::PrefixKeys;
use crate::sessions::{dismissed, Session, Sessions, State};
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
pub(super) const NAME_STEM: &str = "terminal-";

pub enum Mode {
    Normal,
    /// Renaming the current tab; holds the text typed so far.
    Rename(String),
    Help,
    /// The sidebar has the keyboard; holds the selected row.
    Sidebar(Option<RowId>),
    /// Asking before closing terminal `terminal`, where Claude is `state`;
    /// `back` is the dashboard asked from, to return to.
    ConfirmClose {
        terminal: String,
        state: State,
        back: Option<Dashboard>,
    },
    /// The dashboard is open.
    Dashboard(Dashboard),
}

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
    pub(super) task_details: TaskDetails,
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
            task_details: TaskDetails::new(),
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
        self.task_details.retain(|task, _| shown.contains(task));
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
                    back: None,
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
