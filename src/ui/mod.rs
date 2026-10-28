//! Drawing paddock's screen.

pub mod copy;
pub mod dashboard;
pub mod finder;
pub mod float;
pub mod help;
pub mod prompt;
pub mod session_view;
pub mod sidebar;
pub mod tab_bar;

use crate::git::Status;
use crate::github::PrState;
use ratatui::style::{Color, Modifier, Style};

/// Marks an agent state: in the sidebar and before a tab's name.
pub const STATE_DOT: &str = "●";

/// What goes between `+a` and `−r`, and between them and the arrows.
pub const STATUS_SEP: &str = " ";

/// A git [`Status`] as the dashboard and the sidebar write it: its line
/// changes as `+a` and `−r` (both, unless both are zero), and commits ahead
/// of and behind upstream as `↑n` and `↓n` (each left out at zero). Each
/// part apart, so a caller can colour it; [`STATUS_SEP`] between them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusText {
    /// `+a` and `−r`, if there are uncommitted changes.
    pub added_removed: Option<(String, String)>,
    /// `↑n`, then `↓n`, those not zero.
    pub arrows: String,
}

impl StatusText {
    pub fn of(status: &Status) -> Self {
        let added_removed = status
            .changes
            .filter(|&c| c != (0, 0))
            .map(|(added, removed)| (format!("+{added}"), format!("−{removed}")));
        let arrows = status
            .ahead_behind
            .map(|(ahead, behind)| [("↑", ahead), ("↓", behind)])
            .into_iter()
            .flatten()
            .filter(|&(_, n)| n > 0)
            .map(|(arrow, n)| format!("{arrow}{n}"))
            .collect();
        Self {
            added_removed,
            arrows,
        }
    }

    /// `+a −r`, if there are uncommitted changes.
    pub fn line_changes(&self) -> Option<String> {
        self.added_removed
            .as_ref()
            .map(|(a, r)| format!("{a}{STATUS_SEP}{r}"))
    }
}

/// How a PR's number stands out, on the dashboard and in the sidebar, by
/// its state: open green, draft dim, merged purple, closed red. Only its
/// colour (or dimness) changes, so the line's background stays.
pub fn pr_style(state: PrState) -> Style {
    match state {
        PrState::Open => Style::default().fg(Color::Green),
        PrState::Draft => Style::default().add_modifier(Modifier::DIM),
        PrState::Merged => Style::default().fg(Color::Magenta),
        PrState::Closed => Style::default().fg(Color::Red),
    }
}
