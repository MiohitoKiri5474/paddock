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

use crate::github::PrState;
use ratatui::style::{Color, Modifier, Style};

/// Marks an agent state: in the sidebar and before a tab's name.
pub const STATE_DOT: &str = "●";

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
