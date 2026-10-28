//! A border between two splits of a tab, as found under the mouse: which
//! way it runs, and how to move it. A backend finds one
//! ([`super::Backend::border_at`]) and moves it
//! ([`super::Backend::drag_border`]); paddock only asks where it is going.

use super::TerminalId;

/// A border between two splits, and how the backend moves it: by sizing
/// the cell of `terminal` on one side of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Border {
    /// The terminal whose cell the backend sizes to move the border.
    pub(super) terminal: TerminalId,
    columns: bool,
    anchor: Anchor,
}

/// Where the sized cell sits on the border's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Anchor {
    /// The cell before the border, starting here: the border is its end.
    Before(u16),
    /// The cell after the border, ending (exclusive) here: the border is
    /// just before its start.
    After(u16),
}

impl Border {
    pub(super) fn new(terminal: TerminalId, columns: bool, anchor: Anchor) -> Self {
        Self {
            terminal,
            columns,
            anchor,
        }
    }

    /// A border between columns (dragged sideways), else between rows.
    pub fn columns(&self) -> bool {
        self.columns
    }

    /// The size the sized cell takes for the border to sit at `at` (tab
    /// coordinates), at least one cell.
    pub fn size(&self, at: u16) -> u16 {
        let size = match self.anchor {
            Anchor::Before(start) => at.saturating_sub(start),
            Anchor::After(end) => end.saturating_sub(at).saturating_sub(1),
        };
        size.max(1)
    }
}

#[cfg(test)]
impl Border {
    /// The border at the end of a cell starting at `start`, moved by
    /// `terminal`.
    pub fn ending(terminal: &str, columns: bool, start: u16) -> Self {
        Self::new(TerminalId::new(terminal), columns, Anchor::Before(start))
    }
}
