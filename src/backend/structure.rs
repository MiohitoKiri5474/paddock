//! What a space holds, as paddock shows it (ADR 0006): its tabs in order,
//! and each tab's terminals (splits) with their place. Floats are kept
//! apart: they are never tabs.

use super::{TabId, TerminalId};
use ratatui::layout::Rect;

/// One terminal of a tab: its id and where it sits in the tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub id: TerminalId,
    pub rect: Rect,
}

/// One tab of a space (a float's page has the same shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub name: String,
    /// The backend still names it (the user has not renamed it).
    pub auto_name: bool,
    pub splits: Vec<Split>,
    /// The focused terminal.
    pub active: TerminalId,
    /// The focused terminal fills the tab (zoom); the others are hidden
    /// but keep running.
    pub zoomed: bool,
}

impl Tab {
    pub fn contains(&self, terminal: &str) -> bool {
        self.splits.iter().any(|p| p.id == terminal)
    }

    /// Whether `terminal` is drawn when the tab is: any of its splits, or
    /// only the focused one while it is zoomed.
    pub fn shows(&self, terminal: &str) -> bool {
        self.contains(terminal) && (!self.zoomed || self.active == terminal)
    }

    /// Focus `terminal` ahead of the backend's own report, as the backend
    /// does: focusing another terminal than the zoomed one unzooms.
    pub fn focus(&mut self, terminal: &TerminalId) {
        if self.active != *terminal {
            self.zoomed = false;
        }
        self.active = terminal.clone();
    }

    /// The splits drawn when the tab is: all of them, or only the focused
    /// one while it is zoomed.
    pub fn shown_splits(&self) -> impl Iterator<Item = &Split> {
        self.splits.iter().filter(|p| self.shows(&p.id))
    }
}

/// A float: its key and its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Float {
    /// The float's key, as written by [`crate::input::keymap::Key::name`].
    pub key: String,
    /// Its terminals and focus, as for a tab.
    pub tab: Tab,
}

/// A space's name, its tabs in order with the current one's index, and
/// its floats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Structure {
    pub name: String,
    pub tabs: Vec<Tab>,
    /// Index of the current tab in `tabs`.
    pub current: usize,
    /// The floats, in the backend's order.
    pub floats: Vec<Float>,
    /// The backend made a float its space's current page (tmux does when
    /// the current tab closes and the float is next); `current` is then
    /// the tab before it.
    pub float_active: bool,
}

impl Structure {
    /// Every page, the tabs first, then the floats'.
    pub fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter().chain(self.floats.iter().map(|f| &f.tab))
    }
}
