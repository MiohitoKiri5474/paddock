//! The running app: connect to the hidden server, show one terminal, and pass
//! input to it until the user detaches.

mod event_loop;
mod screen;

use crate::paths::Paths;
use crate::terminal::Terminal;
use crate::tmux::{Message, Server};
use anyhow::{Context, Result};
use crossterm::event as ct;
use std::sync::mpsc;

/// Name of the tmux session paddock creates for its first terminal.
const FIRST_SESSION: &str = "terminal-1";

/// Everything the event loop waits on, from tmux and from the outer terminal.
pub enum Event {
    Tmux(Message),
    Input(ct::Event),
}

fn wrap(m: Message) -> Event {
    Event::Tmux(m)
}

fn unwrap(e: Event) -> Option<Message> {
    match e {
        Event::Tmux(m) => Some(m),
        Event::Input(_) => None,
    }
}

pub fn run() -> Result<()> {
    let paths = Paths::from_env()?;
    let server = Server::new(paths);
    server.write_config()?;

    let (screen_guard, mut screen) = screen::ScreenGuard::enter()?;
    let size = screen.size().context("cannot read the terminal size")?;
    let size = (size.width.max(2), size.height.max(1));

    let session = match server.sessions().into_iter().next() {
        Some(existing) => existing,
        None => {
            let cwd = std::env::current_dir().context("no current directory")?;
            server.new_session(FIRST_SESSION, &cwd, size.0, size.1)?;
            FIRST_SESSION.to_string()
        }
    };

    let (tx, rx) = mpsc::channel();
    let terminal = Terminal::attach(&server, &session, size, (tx.clone(), &rx), wrap, unwrap)?;

    // Only now may another thread read stdin (ratatui has finished its queries).
    std::thread::spawn(move || {
        while let Ok(ev) = ct::read() {
            if tx.send(Event::Input(ev)).is_err() {
                break;
            }
        }
    });

    let result = event_loop::run(&mut screen, terminal, &rx);
    drop(screen_guard);
    result
}
