//! What the running app holds: the tabs, each with its terminals arranged as
//! splits, and what the keyboard is doing (typing into the focused terminal,
//! renaming, reading help).
//!
//! A tab's name is the tmux session name of its first terminal, so it
//! survives reattaching; when that terminal ends, the next one names the tab.

use super::layout::{terminal_size, Layout};
use super::saved;
use super::tabs::Tabs;
use super::Event;
use crate::config::{key_name, Config};
use crate::input::PrefixKeys;
use crate::sessions::{self, Sessions, State};
use crate::terminal::Terminal;
use crate::tmux::Server;
use anyhow::Result;
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
    /// The sidebar has the keyboard; holds the selected row's pane.
    Sidebar(Option<String>),
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
    /// The user hid the sidebar (`C-a b`).
    pub(super) sidebar_hidden: bool,
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

/// Session rows of the given terminals, sorted by state, then by tab.
pub fn rows<'a>(sessions: &Sessions, terminals: impl IntoIterator<Item = Placed<'a>>) -> Vec<Row> {
    let mut rows: Vec<Row> = terminals
        .into_iter()
        .filter_map(|t| {
            let s = sessions.get(t.pane)?;
            let tab_name = (!is_default_name(t.tab_name)).then_some(t.tab_name);
            Some(Row {
                pane: t.pane.to_string(),
                terminal: t.terminal.to_string(),
                tab: t.tab,
                label: sessions::label(s, tab_name),
                state: s.state,
            })
        })
        .collect();
    rows.sort_by_key(|r| (r.state.rank(), r.tab));
    rows
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
            sidebar_hidden: false,
        };
        let sessions = app.server.sessions();
        let saved = saved::load(&app.state_dir).unwrap_or_else(|e| {
            app.notice = Some(format!("{e:#}"));
            None
        });
        let live: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
        let restored = saved::restore(saved, &live);
        let area = app.area();
        for layout in restored.tabs {
            // Attach at the final size, so no shell sees a resize.
            for (id, rect) in layout.rects(area).0 {
                app.attach(&id, terminal_size(rect))?;
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
        rows(&self.sessions, placed)
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

    /// Start a shell named `terminal-N` at `size`, in the focused terminal's
    /// directory, and attach it. Returns its session id.
    pub(super) fn new_terminal(&mut self, size: (u16, u16)) -> Result<String> {
        let sessions = self.server.sessions();
        let name = next_name(sessions.iter().map(|s| s.name.as_str()));
        let dir = self
            .current()
            .and_then(|t| {
                self.server
                    .display(t.session(), "#{pane_current_path}")
                    .ok()
            })
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| self.start_dir.clone());
        let session = self.server.new_session(&name, &dir, size)?;
        self.attach(&session.id, size)?;
        Ok(session.id)
    }

    /// Start a shell in a new tab and make it current.
    pub fn new_tab(&mut self) -> Result<()> {
        let id = self.new_terminal(terminal_size(self.area()))?;
        let name = self.server.display(&id, "#{session_name}")?;
        self.tabs.push(name, Layout::new(id));
        self.tabs.select_last();
        Ok(())
    }

    /// End the focused terminal. It leaves its tab when tmux reports that the
    /// session ended, the same way as when its shell exits.
    pub fn close(&mut self) -> Result<()> {
        match self.current() {
            Some(t) => self.server.kill_session(t.session()),
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
            app.relayout()?;
            app.save()
        });
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
    fn rows_cover_terminals_with_sessions_sorted_by_state_then_tab() {
        use crate::sessions::HookRecord;
        let mut s = Sessions::default();
        for (pane, event, cwd) in [
            ("%1", "SessionEnd", "/w/a"),
            ("%2", "Stop", "/w/b"),
            ("%3", "PermissionRequest", "/w/c"),
            ("%4", "Stop", "/w/d"),
            ("%9", "Stop", "/w/gone"),
        ] {
            s.apply(&HookRecord {
                event: event.into(),
                pane: pane.into(),
                session_id: Some(pane.into()),
                cwd: Some(cwd.into()),
                ..Default::default()
            });
        }
        // Tab 3 is split: both of its terminals run Claude.
        let terminals = [
            (0, "terminal-1", "$1", "%1"),
            (1, "api", "$2", "%2"),
            (2, "terminal-3", "$3", "%3"),
            (2, "terminal-3", "$4", "%4"),
            (3, "shell", "$5", "%5"),
        ];
        let placed = terminals
            .iter()
            .map(|&(tab, tab_name, terminal, pane)| Placed {
                tab,
                tab_name,
                terminal,
                pane,
            });
        let got: Vec<(String, String, State)> = rows(&s, placed)
            .into_iter()
            .map(|r| (r.label, r.terminal, r.state))
            .collect();
        let want = [
            ("c", "$3", State::Waiting),
            ("api", "$2", State::Idle),
            ("d", "$4", State::Idle),
            ("a", "$1", State::Stopped),
        ];
        let want: Vec<(String, String, State)> = want
            .iter()
            .map(|&(l, t, s)| (l.to_string(), t.to_string(), s))
            .collect();
        assert_eq!(got, want);
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
