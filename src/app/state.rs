//! What the running app holds: the sidebar items (tmux sessions, ADR 0006),
//! each with its tabs (windows) and splits (panes) as tmux has them, a
//! terminal per pane, the Claude sessions in them, and what the keyboard is
//! doing (typing into the focused pane, renaming, reading help).

use super::attention::Attention;
use super::dashboard::Dashboard;
use super::item::Item;
use super::mouse::Press;
use super::rows::{rows, Placed, Row, RowId, TaskDetails, TaskStatus};
use super::saved;
use super::sidebar::Selection;
use super::watch::Place;
use super::Event;
use crate::config::{key_name, Config};
use crate::hooks::now_ms;
use crate::input::PrefixKeys;
use crate::sessions::{dismissed, Session, Sessions, State};
use crate::terminal::Terminal;
use crate::tmux::Server;
use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// Rows above the terminals taken by the tab bar.
pub const TAB_BAR_ROWS: u16 = 1;
/// Columns the sidebar takes, its border included, while it is shown.
pub const SIDEBAR_COLS: u16 = 24;

pub enum Mode {
    Normal,
    /// Renaming the current tab or item; holds the text typed so far.
    Rename {
        target: Renaming,
        text: String,
    },
    Help,
    /// The sidebar has the keyboard; holds its selection.
    Sidebar(Selection),
    /// Asking before closing `target`, where Claude is `state`; `back` is
    /// the dashboard asked from, to return to.
    ConfirmClose {
        target: CloseTarget,
        state: State,
        back: Option<Dashboard>,
    },
    /// The dashboard is open.
    Dashboard(Dashboard),
}

/// What is being renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renaming {
    /// The current tab (tmux window).
    Tab,
    /// The current sidebar item (tmux session).
    Item,
}

/// What a close question is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseTarget {
    /// A pane (`%N`).
    Pane(String),
    /// A whole sidebar item (`$N`) with all its tabs.
    Item(String),
}

pub struct App {
    pub(super) server: Server,
    pub(super) events: Sender<Event>,
    /// The sidebar items, oldest first.
    pub(super) items: Vec<Item>,
    /// Index of the current item: the one whose tabs are shown.
    pub(super) current: usize,
    /// Every attached pane's terminal, by pane id.
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
    /// Each space's directory and branch, by tmux session id, once read.
    pub(super) places: HashMap<String, Place>,
    /// The user hid the sidebar (`C-a b`).
    pub(super) sidebar_hidden: bool,
    /// Where the held mouse button went, until it is released.
    pub(super) press: Option<Press>,
    pub(super) attention: Attention,
    /// Background of the current sidebar item.
    pub(super) current_item_bg: Color,
}

/// The part of a screen of `(cols, rows)` that shows the current tab's
/// panes, right of a sidebar `sidebar` columns wide.
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

/// The first `<stem>N` not already taken.
pub fn next_name<'a>(stem: &str, taken: impl IntoIterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{stem}{n}"))
        .find(|name| !taken.clone().into_iter().any(|t| t == name))
        .unwrap_or_else(|| stem.to_string())
}

impl App {
    /// Show the items already on the hidden server (one per tmux session),
    /// the saved one current, or start the first one.
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
            items: Vec::new(),
            current: 0,
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
            places: HashMap::new(),
            sidebar_hidden: false,
            press: None,
            attention: Attention::from_env(),
            current_item_bg: config.current_item,
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
        if !sessions.is_empty() {
            // A server from an older paddock lacks newer options.
            app.report(|app| app.server.apply_config());
        }
        let saved = saved::load(&app.state_dir).unwrap_or_else(|e| {
            app.notice = Some(set_aside(&app.state_dir, saved::FILE_NAME, &e));
            None
        });
        for s in &sessions {
            // An item that cannot be attached (it just ended) keeps paddock
            // from opening no other.
            app.report(|app| app.attach(&s.id));
        }
        let saved_item = saved.and_then(|s| s.item);
        app.current = app
            .items
            .iter()
            .position(|i| Some(&i.id) == saved_item.as_ref())
            .unwrap_or(0);
        if app.items.is_empty() {
            app.new_item()?;
        }
        Ok(app)
    }

    /// Where the current tab's panes are drawn.
    pub fn area(&self) -> Rect {
        session_area(self.screen, self.sidebar_width())
    }

    /// Every open pane with its place, for [`rows`].
    fn placed(&self) -> impl Iterator<Item = Placed<'_>> {
        self.items.iter().enumerate().flat_map(|(i, item)| {
            item.windows().iter().enumerate().flat_map(move |(w, win)| {
                win.panes.iter().map(move |p| Placed {
                    item: &item.id,
                    window: &win.id,
                    order: (i, w),
                    tab_name: (!win.auto_name).then_some(win.name.as_str()),
                    pane: &p.id,
                })
            })
        })
    }

    /// The Claude session rows, in display order.
    pub fn rows(&self) -> Vec<Row> {
        rows(&self.sessions, &self.task_status, self.placed())
    }

    /// Dismiss a row: it leaves the rows, and stays gone after reopening,
    /// until Claude reports on it again. Git and terminals are untouched.
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

    /// Tasks whose rows are on screen now, as `(directory, branch)`: their
    /// git status is kept fresh. The sidebar shows no task rows (ADR 0006).
    pub fn visible_tasks(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Columns the sidebar takes now: none while hidden.
    pub fn sidebar_width(&self) -> u16 {
        if self.sidebar_hidden {
            0
        } else {
            SIDEBAR_COLS
        }
    }

    /// Attach tmux session `id` as a new item at the end of the sidebar.
    pub(super) fn attach(&mut self, id: &str) -> Result<()> {
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
        let size = (self.area().width, self.area().height);
        let (item, terminals) = Item::attach(&self.server, id, size, deliver)?;
        self.terminals.extend(terminals);
        self.items.push(item);
        Ok(())
    }

    /// Pane `pane`'s shell directory, if it still exists.
    pub(super) fn dir_of(&self, pane: &str) -> Option<PathBuf> {
        self.server
            .display(pane, "#{pane_current_path}")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
    }

    /// Where a new shell starts: the focused pane's directory, else the
    /// directory paddock started in.
    pub(super) fn new_dir(&self) -> PathBuf {
        self.current_pane()
            .and_then(|p| self.dir_of(&p))
            .unwrap_or_else(|| self.start_dir.clone())
    }

    /// The Claude session in pane `pane`, if any.
    pub(super) fn session_in(&self, pane: &str) -> Option<&Session> {
        self.sessions.get(pane)
    }

    pub(super) fn current_item(&self) -> Option<&Item> {
        self.items.get(self.current)
    }

    pub(super) fn current_item_mut(&mut self) -> Option<&mut Item> {
        self.items.get_mut(self.current)
    }

    /// The item holding pane `pane`, by index.
    pub(super) fn item_of(&self, pane: &str) -> Option<usize> {
        self.items.iter().position(|i| i.window_of(pane).is_some())
    }

    /// The focused pane of the current tab.
    pub fn current_pane(&self) -> Option<String> {
        Some(self.current_item()?.current_window()?.active.clone())
    }

    /// Send bytes to pane `pane`'s program, returning its view to the live
    /// screen.
    pub(super) fn input(&mut self, pane: &str, bytes: &[u8]) -> Result<()> {
        if let Some(t) = self.terminals.get_mut(pane) {
            t.emulator_mut().scroll_to_bottom();
        }
        match self.item_of(pane) {
            Some(i) => self.items[i].input(pane, bytes),
            None => Ok(()),
        }
    }

    /// Whether pane `pane` is on screen (in the current tab).
    pub fn shows(&self, pane: &str) -> bool {
        self.current_item()
            .and_then(Item::current_window)
            .is_some_and(|w| w.contains(pane))
    }

    pub fn resize(&mut self, screen: (u16, u16)) -> Result<()> {
        self.screen = screen;
        self.relayout()
    }

    /// Detach every item; they keep running on the hidden server. The
    /// current item is saved first.
    pub fn detach_all(self) -> Result<()> {
        let saved = self.save();
        for item in self.items {
            item.detach();
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
        assert_eq!(next_name("w-", Vec::<&str>::new()), "w-1");
        assert_eq!(next_name("w-", vec!["w-1", "w-3"]), "w-2");
        assert_eq!(next_name("w-", vec!["w-2", "logs"]), "w-1");
    }
}

/// Whether `id` looks like a Claude session id (a UUID: hex digits and `-`).
pub(super) fn is_session_id(id: &str) -> bool {
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
