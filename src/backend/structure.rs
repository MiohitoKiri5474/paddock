//! What a space holds, as paddock shows it (ADR 0006): its tabs in order,
//! each tab's terminals (splits) with their place, and its floats, which
//! are never tabs.

use super::{FloatId, TabId, TerminalId};
use ratatui::layout::Rect;

/// One terminal and where it sits in its tab or float.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub id: TerminalId,
    pub rect: Rect,
}

/// One tab of a space.
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
    pub fn contains(&self, terminal: &TerminalId) -> bool {
        self.splits.iter().any(|s| s.id == *terminal)
    }

    /// Whether `terminal` is drawn when the tab is: any of its splits, or
    /// only the focused one while it is zoomed.
    pub fn shows(&self, terminal: &TerminalId) -> bool {
        self.contains(terminal) && (!self.zoomed || self.active == *terminal)
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
        self.splits.iter().filter(|s| self.shows(&s.id))
    }
}

/// A float: a program in a box over the screen, on its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Float {
    pub id: FloatId,
    /// The float's key, as written by [`crate::input::keymap::Key::name`].
    pub key: String,
    /// Its terminals (one, unless split from outside paddock).
    pub splits: Vec<Split>,
    /// The terminal that has the keyboard while it shows.
    pub active: TerminalId,
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
    /// The backend made a float its space's current page; `current` is
    /// then the tab before it, which paddock makes current again.
    pub float_active: bool,
}

impl Structure {
    /// Every terminal of the space with its place, the tabs' first, then
    /// the floats'.
    pub fn splits(&self) -> impl Iterator<Item = &Split> {
        let tabs = self.tabs.iter().flat_map(|t| &t.splits);
        tabs.chain(self.floats.iter().flat_map(|f| &f.splits))
    }
}
