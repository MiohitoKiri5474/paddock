//! Background threads that feed session information into the event loop:
//! new lines of the hook event log, `claude agents --json` snapshots, and the
//! git status of task rows on screen.

use super::Event;
use crate::git;
use crate::hooks::now_ms;
use crate::sessions::{agents, log};
use crate::tmux::Server;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

/// How often to look for new hook events.
const LOG_POLL: Duration = Duration::from_millis(200);
/// How often to ask Claude which sessions run (catches what hooks miss).
const AGENTS_POLL: Duration = Duration::from_secs(3);
/// How often to re-read git for the task rows on screen.
const GIT_POLL: Duration = Duration::from_secs(3);

/// Tasks to keep the git status of, as `(directory, branch)`.
pub type Tasks = Vec<(String, String)>;

/// Start the threads. They end when the event loop goes away. Returns where
/// to send the tasks on screen: each list is read at once, then every
/// [`GIT_POLL`] until the next one.
pub fn start(server: &Server, state_dir: &Path, tx: &Sender<Event>) -> Result<Sender<Tasks>> {
    let pid: u32 = server
        .run(&["display-message", "-p", "#{pid}"])?
        .trim()
        .parse()
        .context("cannot read the tmux server's pid")?;
    let mut reader = log::Reader::new(log::path(state_dir, pid));
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let records = reader.read_new();
        if !records.is_empty() && events.send(Event::Hooks(records)).is_err() {
            break;
        }
        std::thread::sleep(LOG_POLL);
    });

    let server = server.clone();
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let taken = now_ms();
        if let Some(list) = agents::fetch(&server) {
            if events.send(Event::Agents(taken, list)).is_err() {
                break;
            }
        }
        std::thread::sleep(AGENTS_POLL);
    });

    let (tasks_tx, tasks_rx) = mpsc::channel::<Tasks>();
    let events = tx.clone();
    std::thread::spawn(move || {
        let mut tasks = Tasks::new();
        loop {
            match tasks_rx.recv_timeout(GIT_POLL) {
                // Only the latest list matters.
                Ok(list) => tasks = tasks_rx.try_iter().last().unwrap_or(list),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if tasks.is_empty() {
                continue;
            }
            let read = tasks
                .iter()
                .map(|(dir, branch)| ((dir.clone(), branch.clone()), git::status(dir, branch)))
                .collect();
            if events.send(Event::Git(read)).is_err() {
                break;
            }
        }
    });
    Ok(tasks_tx)
}
