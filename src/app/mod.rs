//! The running app: connect to the hidden server, show its sessions as
//! spaces with their windows as tabs, and pass input to the focused
//! pane until the user detaches.

mod attention;
mod dashboard;
mod event_loop;
mod floats;
mod input;
mod mouse;
mod pages;
mod rows;
mod saved;
mod screen;
mod sidebar;
mod space;
mod splits;
mod state;
mod tabs;
mod view;
mod watch;

pub use rows::{Row, RowId, TaskDetails, TaskRow};

use crate::config::Config;
use crate::git::{Details, Status};
use crate::paths::Paths;
use crate::sessions::{Agent, HookRecord};
use crate::tmux::{Message, Server};
use anyhow::{Context, Result};
use crossterm::event as ct;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use std::sync::mpsc;

/// Everything the event loop waits on, from tmux and from the outer terminal.
pub enum Event {
    /// A control message from the client of space (tmux session) `session` (`$N`).
    Tmux {
        session: String,
        msg: Message,
    },
    Input(ct::Event),
    /// Asked to detach (SIGTERM), e.g. by a second launch taking over.
    Detach,
    /// Hook events recorded since the last batch.
    Hooks(Vec<HookRecord>),
    /// A `claude agents --json` snapshot and when it was taken (ms).
    Agents(u64, Vec<Agent>),
    /// Git status just read for tasks, by `(directory, branch)`.
    Git(Vec<((String, String), Status)>),
    /// Dashboard details just read for tasks, by `(directory, branch)`.
    Details(Vec<((String, String), Details)>),
    /// Each space's directory and branch just read, by tmux session id.
    Spaces(Vec<(String, watch::Place)>),
    /// Showing a notification failed, with what the notifier said.
    NotifyFailed(String),
}

pub fn run() -> Result<()> {
    let paths = Paths::from_env()?;
    // Read the config before touching the screen, so errors print normally.
    let config = Config::load(&paths.config_dir)?;
    let Some(_instance) = crate::instance::claim(&paths.state_dir)? else {
        return Ok(());
    };
    let state_dir = paths.state_dir.clone();
    let server = Server::new(paths);
    server.write_config()?;
    let start_dir = std::env::current_dir().context("no current directory")?;

    let (tx, rx) = mpsc::channel();
    // Before taking over the screen, so SIGTERM never leaves it in raw mode.
    let mut signals = Signals::new([SIGTERM]).context("cannot handle SIGTERM")?;
    let detach = tx.clone();
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = detach.send(Event::Detach);
        }
    });

    let (screen_guard, mut screen) = screen::ScreenGuard::enter()?;
    let size = screen.size().context("cannot read the terminal size")?;
    let app = state::App::start(
        server,
        &config,
        (size.width, size.height),
        tx.clone(),
        start_dir,
        state_dir.clone(),
    )?;
    let git = watch::start(&app.server, &state_dir, &tx)?;

    // Only now may another thread read stdin (ratatui has finished its queries).
    std::thread::spawn(move || {
        while let Ok(ev) = ct::read() {
            if tx.send(Event::Input(ev)).is_err() {
                return;
            }
        }
        // No more input: nothing could ever detach, so detach now.
        let _ = tx.send(Event::Detach);
    });

    let result = event_loop::run(&mut screen, app, &rx, git);
    drop(screen_guard);
    // Shown at start too, but only until the first key: repeat them where
    // they stay.
    for warning in &config.warnings {
        eprintln!("paddock: {warning}");
    }
    result
}
