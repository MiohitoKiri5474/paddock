//! What the running app holds: the tabs, each with its terminal, and what
//! the keyboard is doing (typing into the terminal, renaming, reading help).

use super::tabs::Tabs;
use super::Event;
use crate::config::{key_name, Config};
use crate::input::PrefixKeys;
use crate::terminal::Terminal;
use crate::tmux::server::Session;
use crate::tmux::Server;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// Rows above the terminals taken by the tab bar.
pub const TAB_BAR_ROWS: u16 = 1;
/// New terminals are named `terminal-N` until the user renames them.
const NAME_STEM: &str = "terminal-";

pub enum Mode {
    Normal,
    /// Renaming the current tab; holds the text typed so far.
    Rename(String),
    Help,
}

pub struct App {
    pub(super) server: Server,
    pub(super) events: Sender<Event>,
    pub(super) tabs: Tabs<Terminal>,
    pub(super) prefix: PrefixKeys,
    pub(super) prefix_name: String,
    pub(super) mode: Mode,
    /// A message for the user, shown in the tab bar until the next key.
    pub(super) notice: Option<String>,
    /// Size of the whole outer screen.
    pub(super) screen: (u16, u16),
    /// Where new terminals start when the current one's directory is unknown.
    pub(super) start_dir: PathBuf,
}

/// The size terminals get on a screen of `(cols, rows)`.
pub fn terminal_size((cols, rows): (u16, u16)) -> (u16, u16) {
    (cols.max(2), rows.saturating_sub(TAB_BAR_ROWS).max(1))
}

/// The first `terminal-N` not already taken.
pub fn next_name<'a>(taken: impl IntoIterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{NAME_STEM}{n}"))
        .find(|name| !taken.clone().into_iter().any(|t| t == name))
        .unwrap_or_else(|| NAME_STEM.to_string())
}

impl App {
    /// Show every terminal already on the hidden server as a tab, oldest
    /// first, or start the first one.
    pub fn start(
        server: Server,
        config: &Config,
        screen: (u16, u16),
        events: Sender<Event>,
        start_dir: PathBuf,
    ) -> Result<Self> {
        let mut app = Self {
            server,
            events,
            tabs: Tabs::default(),
            prefix: PrefixKeys::new(config.prefix),
            prefix_name: key_name(&config.prefix),
            mode: Mode::Normal,
            notice: None,
            screen,
            start_dir,
        };
        let sessions = app.server.sessions();
        if sessions.is_empty() {
            app.new_tab()?;
        }
        for session in &sessions {
            app.attach(session)?;
        }
        Ok(app)
    }

    /// Attach a session as a new last tab.
    fn attach(&mut self, session: &Session) -> Result<()> {
        let events = self.events.clone();
        let id = session.id.clone();
        let deliver = move |msg| {
            events
                .send(Event::Tmux {
                    session: id.clone(),
                    msg,
                })
                .is_ok()
        };
        let terminal = Terminal::attach(
            &self.server,
            &session.id,
            terminal_size(self.screen),
            deliver,
        )?;
        self.tabs.push(session.name.clone(), terminal);
        Ok(())
    }

    /// Start a shell in a new tab and make it current. It starts in the
    /// current terminal's directory.
    pub fn new_tab(&mut self) -> Result<()> {
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
        let session = self
            .server
            .new_session(&name, &dir, terminal_size(self.screen))?;
        self.attach(&session)?;
        self.tabs.select_last();
        Ok(())
    }

    /// End the current tab's terminal. The tab goes away when tmux reports
    /// that the session ended, the same way as when its shell exits.
    pub fn close_tab(&mut self) -> Result<()> {
        match self.current() {
            Some(t) => self.server.kill_session(t.session()),
            None => Ok(()),
        }
    }

    /// Forget the tab of a session that ended. Returns whether any tab is left.
    pub fn terminal_ended(&mut self, session: &str) -> bool {
        if let Some(index) = self.tabs.position(|t| t.session() == session) {
            if let Some(tab) = self.tabs.remove(index) {
                tab.content.detach();
            }
        }
        !self.tabs.is_empty()
    }

    /// Rename the current tab by renaming its tmux session, so the name
    /// survives reattaching. tmux may adjust the name (it replaces `.` and `:`).
    pub fn rename_current(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        let Some(session) = self.current().map(|t| t.session().to_string()) else {
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

    pub fn current(&self) -> Option<&Terminal> {
        self.tabs.current().map(|t| &t.content)
    }

    pub fn current_mut(&mut self) -> Option<&mut Terminal> {
        self.tabs.current_mut().map(|t| &mut t.content)
    }

    pub fn resize(&mut self, screen: (u16, u16)) -> Result<()> {
        self.screen = screen;
        let (cols, rows) = terminal_size(screen);
        for tab in self.tabs.iter_mut() {
            tab.content.resize(cols, rows)?;
        }
        Ok(())
    }

    /// Detach every terminal; they keep running on the hidden server.
    pub fn detach_all(self) {
        for tab in self.tabs.into_iter() {
            tab.content.detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_get_the_screen_minus_the_tab_bar() {
        assert_eq!(terminal_size((100, 30)), (100, 29));
        assert_eq!(terminal_size((1, 1)), (2, 1));
    }

    #[test]
    fn new_names_fill_the_first_gap() {
        assert_eq!(next_name(Vec::<&str>::new()), "terminal-1");
        assert_eq!(next_name(vec!["terminal-1", "terminal-3"]), "terminal-2");
        assert_eq!(next_name(vec!["terminal-2", "logs"]), "terminal-1");
    }
}
