//! Drawing paddock's screen.

pub mod dashboard;
pub mod help;
pub mod prompt;
pub mod session_view;
pub mod sidebar;
pub mod tab_bar;

/// Marks a Claude state: in the sidebar and before a tab's name.
pub const STATE_DOT: &str = "●";
