//! The tmux backend: everything that talks to the hidden tmux server. A
//! space is a tmux session, a tab or a float a window, and a terminal a
//! pane (ADR 0006).

mod control;
mod hook;
mod layout;
mod protocol;
mod seed;
mod server;
mod structure;

pub use control::{attach, ControlClient};
pub use hook::terminal as hook_terminal;
pub use server::Server;
