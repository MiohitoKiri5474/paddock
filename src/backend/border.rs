//! A border between two splits of a tab, as found under the mouse: which
//! way it runs, and how to move it. A backend finds one
//! ([`super::Backend::border_at`]) and moves it
//! ([`super::Backend::drag_border`]); paddock only asks where it is going.

use super::TerminalId;

/// A border between two splits, and how the backend moves it: by sizing
/// the cell of `terminal`, before the border, which is its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Border {
    /// The terminal whose cell the backend sizes to move the border.
    pub(super) terminal: TerminalId,
    columns: bool,
    /// Where the sized cell starts on the border's axis.
    start: u16,
}

impl Border {
    pub(super) fn new(terminal: TerminalId, columns: bool, start: u16) -> Self {
        Self {
            terminal,
            columns,
            start,
        }
    }

    /// A border between columns (dragged sideways), else between rows.
    pub fn columns(&self) -> bool {
        self.columns
    }

    /// The size the sized cell takes for the border to sit at `at` (tab
    /// coordinates), at least one cell.
    pub fn size(&self, at: u16) -> u16 {
        at.saturating_sub(self.start).max(1)
    }
}

#[cfg(test)]
impl Border {
    /// The border at the end of a cell starting at `start`, moved by
    /// `terminal`.
    pub fn ending(terminal: &str, columns: bool, start: u16) -> Self {
        Self::new(TerminalId::new(terminal), columns, start)
    }
}
