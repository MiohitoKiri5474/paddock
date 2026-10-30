//! The running app: connect to the backend, show its spaces and their
//! tabs, and pass input to the focused terminal until the user detaches.

mod attention;
mod copy;
mod dashboard;
mod desktop;
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

use crate::backend::{self, Backend, OtherVersion, SpaceId, TerminalId};
use crate::config::Config;
use crate::git::{Details, Status};
use crate::paths::Paths;
use crate::sessions::{Agent, HookRecord};
use anyhow::{Context, Result};
use crossterm::event as ct;
use signal_hook::consts::SIGTERM;
use signal_hook::iterator::Signals;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::sync::{mpsc, Arc};

/// Everything the event loop waits on, from the backend and from the outer
/// terminal.
pub enum Event {
    /// What space `space`'s connection reported: connection `connection`
    /// ([`App::attach`] numbers each), so one replaced since is told apart.
    Backend {
        space: SpaceId,
        connection: u64,
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
    run_claimed(paths, &config)
}

/// [`run`] once this paddock is the only one.
fn run_claimed(paths: Paths, config: &Config) -> Result<()> {
    let state_dir = paths.state_dir.clone();
    let backend = Backend::new(paths);
    let notice = prepare(&backend)?;
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
        config,
        (size.width, size.height),
        tx.clone(),
        start_dir,
        state_dir.clone(),
    )?;
    if let Some(notice) = notice {
        app.notice = Some(match app.notice.take() {
            Some(others) => format!("{notice}; {others}"),
            None => notice,
        });
    }
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
    match result? {
        Some(farewell) => anyhow::bail!(farewell),
        None => Ok(()),
    }
}

/// Get the backend ready ([`Backend::prepare`]) and bring back a saved
/// layout ([`Backend::resume`]). A server that speaks another protocol
/// version is said so, with an offer to upgrade it in place to this
/// paddock (ADR 0009), or, one too old to hand over, to restart it; on
/// no, or with no terminal to ask on, paddock stops with the same words.
/// See [`upgrade`] and [`restart`] for how they go. A notice for the tab
/// bar of how an upgrade or a restart went, if one ran.
fn prepare(backend: &Backend) -> Result<Option<String>> {
    let mut notice = None;
    if let Err(e) = backend.prepare() {
        let Some(other) = e.downcast_ref::<OtherVersion>() else {
            return Err(e);
        };
        notice = if other.can_hand_over() {
            let question = format!(
                "paddock: {}.\nUpgrade it? Its programs keep running. [Y/n] ",
                other.mismatch()
            );
            if !ask(&question, true)? {
                return Err(e);
            }
            upgrade(backend, other)?
        } else {
            restart(backend, other)?
        };
        backend.prepare()?;
    }
    backend.resume()?;
    Ok(notice)
}

/// The notice after a restart: the layout's own ("restored 2 spaces")
/// follows it.
const RESTARTED: &str = "paddock server restarted: its programs ended";

/// Offer to restart the server `other`, from before upgrades (ADR 0009),
/// which cannot hand its terminals over: yes by default, as a restart
/// is the only way on. It ends the server, programs and all, and a new
/// one rebuilds its layout (ADR 0008); declined, paddock stops with
/// `other`.
fn restart(backend: &Backend, other: &OtherVersion) -> Result<Option<String>> {
    let question = format!(
        "paddock: {}.\nIt is from before upgrades, so it cannot hand its terminals over.\n\
         Restart it? Its programs and agents end; its layout comes back with a fresh \
         shell in each terminal, without scrollback or agent sessions, which it never \
         saved. [Y/n] ",
        other.mismatch()
    );
    if !ask(&question, true)? {
        return Err((*other).into());
    }
    backend.restart(other)?;
    Ok(Some(RESTARTED.to_string()))
}

/// Upgrade the server `other` to this paddock's own binary: a notice of
/// how it went, if any. A server that cannot hand over runs on as it
/// was: paddock says why and offers to restart it (ADR 0007), which ends
/// it, programs and all, so a new one rebuilds its layout; declined,
/// paddock stops with `other`. One that failed after it switched has
/// ended with its programs: paddock says so, and goes on as after a
/// restart.
fn upgrade(backend: &Backend, other: &OtherVersion) -> Result<Option<String>> {
    let exe = std::env::current_exe().context("cannot find paddock's own binary")?;
    match backend.upgrade_other(other, &exe)? {
        backend::Upgraded::Done => Ok(Some(event_loop::UPGRADED.to_string())),
        backend::Upgraded::Unchanged(why) => {
            println!("paddock: it cannot be upgraded: {why:#}");
            let question = "Restart it? Its programs end; its layout comes back. [y/N] ";
            if !ask(question, false)? {
                return Err((*other).into());
            }
            backend.restart(other)?;
            Ok(Some(RESTARTED.to_string()))
        }
        backend::Upgraded::Ended(why) => {
            println!(
                "paddock: the upgrade failed after the server switched to this paddock, so \
                 its programs have ended; its layout comes back: {why:#}"
            );
            Ok(Some(
                "paddock server upgrade failed: its programs ended".to_string(),
            ))
        }
    }
}

/// Ask `question` on the terminal: whether the answer is yes, `default`
/// for an empty one; no at the end of input, or with no terminal to ask
/// on.
fn ask(question: &str, default: bool) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("{question}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    let read = std::io::stdin().read_line(&mut answer)?;
    Ok(answered((read > 0).then_some(answer.as_str()), default))
}

/// Whether the line `answer` says yes: `default` for an empty one (Enter
/// alone), no at the end of input (`None`, as after Ctrl-D), which never
/// counts as the default.
fn answered(answer: Option<&str>, default: bool) -> bool {
    let Some(answer) = answer.map(str::trim) else {
        return false;
    };
    if answer.is_empty() {
        return default;
    }
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::answered;

    #[test]
    fn enter_takes_the_default_and_the_end_of_input_says_no() {
        assert!(answered(Some("\n"), true));
        assert!(!answered(Some("\n"), false));
        assert!(
            !answered(None, true),
            "Ctrl-D is no, even when yes is the default"
        );
        assert!(answered(Some(" Yes\n"), false));
        assert!(answered(Some("y"), false), "a last line with no newline");
        assert!(!answered(Some("n\n"), true));
        assert!(!answered(Some("maybe\n"), true));
    }
}
