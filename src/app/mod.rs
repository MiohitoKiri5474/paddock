//! The running app: connect to the backend, show its spaces and their
//! tabs, and pass input to the focused terminal until the user detaches.

mod attention;
mod copy;
mod dashboard;
mod event_loop;
mod finder;
mod floats;
mod input;
mod mouse;
mod pages;
mod prs;
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
mod worktrees;

pub use dashboard::DashboardFilter;
pub use rows::{Row, RowId, TaskDetails, TaskPrs, TaskRow};

use crate::backend::{self, Backend, SpaceId, TerminalId};
use crate::config::Config;
use crate::git::{Details, Status};
use crate::paths::Paths;
use crate::sessions::{Agent, HookRecord};
use anyhow::{Context, Result};
use crossterm::event as ct;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};

/// Everything the event loop waits on, from the backend and from the outer
/// terminal.
pub enum Event {
    /// What space `space`'s connection reported.
    Backend {
        space: SpaceId,
        event: backend::Event,
    },
    Input(ct::Event),
    /// Asked to detach (SIGTERM), e.g. by a second launch taking over.
    Detach,
    /// Hook events recorded since the last batch.
    Hooks(Vec<HookRecord>),
    /// A `claude agents --json` snapshot and when it was taken (ms).
    Agents(u64, Vec<Agent>),
    /// Each terminal's foreground command, and when it was read (ms).
    Commands(u64, HashMap<TerminalId, String>),
    /// Git status just read for tasks, by `(directory, branch)`.
    Git(Vec<((String, String), Status)>),
    /// Dashboard details just read for tasks, by `(directory, branch)`.
    Details(Vec<((String, String), Details)>),
    /// Each space's directory and branch just read.
    Spaces(Vec<(SpaceId, watch::Place)>),
    /// Pull requests just read for the dashboard's tasks, by
    /// `(directory, branch)`; `None` for none.
    Prs(prs::PrUpdate),
    /// `p`'s `gh pr view --web` for PR number .0 is done, with how it went.
    PrOpened(u64, Result<(), String>),
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
    let backend = Backend::new(paths);
    backend.prepare()?;
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
    let mut app = state::App::start(
        backend,
        &config,
        (size.width, size.height),
        tx.clone(),
        start_dir,
        state_dir.clone(),
    )?;
    let events_log = watch::server_log(&app.backend, &state_dir)?;
    app.events_log = Some(events_log.clone());
    let git = watch::start(
        &app.backend,
        events_log,
        Arc::clone(&app.agy_running),
        &state_dir,
        &tx,
    )?;
    let prs = prs::start(config.github_refresh, state_dir.join(prs::LOG_FILE), &tx);

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

    let result = event_loop::run(&mut screen, app, &rx, git, prs);
    drop(screen_guard);
    // Shown at start too, but only until the first key: repeat them where
    // they stay.
    for warning in &config.warnings {
        eprintln!("paddock: {warning}");
    }
    result
}
