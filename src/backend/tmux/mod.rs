//! The tmux backend: everything that talks to the hidden tmux server. A
//! space is a tmux session, a tab or a float a window, and a terminal a
//! pane (ADR 0006).

mod control;
pub mod layout;
mod protocol;
mod seed;
mod server;
mod structure;

pub use control::{attach, ControlClient};
pub use server::Server;
