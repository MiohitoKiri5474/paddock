//! Background threads that feed session information into the event loop:
//! new lines of the hook event logs (this server's and the external one),
//! `claude agents --json` snapshots, the git status of task rows on screen
//! (and their details while the dashboard is open), and each space's
//! directory and branch.

use super::Event;
use crate::git;
use crate::hooks::now_ms;
use crate::sessions::{agents, log};
use crate::tmux::Server;
use crate::worktree::space_dir;
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

/// The tasks on screen, whether the dashboard's details are wanted too,
/// and the spaces (tmux session ids) whose directory and branch to read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watched {
    pub tasks: Tasks,
    pub details: bool,
    pub spaces: Vec<String>,
}

/// A space's directory, the branch checked out there, and the working
/// tree it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: String,
    /// `None` outside a git repository.
    pub branch: Option<String>,
    /// The working tree and its repository's main checkout; `None`
    /// outside a git repository.
    pub tree: Option<git::WorkingTree>,
}

/// Read each of `spaces`' directory and branch.
fn places(server: &Server, spaces: &[String]) -> Vec<(String, Place)> {
    spaces
        .iter()
        .filter_map(|id| {
            let path = space_dir(server, id)?;
            let branch = git::head(&path);
            let tree = git::working_tree(&path);
            Some((id.clone(), Place { path, branch, tree }))
        })
        .collect()
}

/// Start the threads. They end when the event loop goes away. Returns where
/// to send the tasks on screen: each list is read at once, then every
/// [`GIT_POLL`] until the next one.
pub fn start(server: &Server, state_dir: &Path, tx: &Sender<Event>) -> Result<Sender<Watched>> {
    let pid: u32 = server
        .run(&["display-message", "-p", "#{pid}"])?
        .trim()
        .parse()
        .context("cannot read the tmux server's pid")?;
    let mut reader = log::Reader::new(log::path(state_dir, pid));
    // Every Claude on the machine appends to the external log: trim it
    // before replaying it. Failing that, it is read whole.
    let external = log::external_path(state_dir);
    let _ = log::compact(
        &external,
        log::EXTERNAL_MAX_BYTES,
        now_ms(),
        log::EXTERNAL_KEEP_MS,
    );
    let mut external = log::Reader::new(external);
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let records = log::merged(reader.read_new(), external.read_new());
        if !records.is_empty() && events.send(Event::Hooks(records)).is_err() {
            break;
        }
        std::thread::sleep(LOG_POLL);
    });

    let agents_server = server.clone();
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let taken = now_ms();
        if let Some(list) = agents::fetch(&agents_server) {
            if events.send(Event::Agents(taken, list)).is_err() {
                break;
            }
        }
        std::thread::sleep(AGENTS_POLL);
    });

    let (tasks_tx, tasks_rx) = mpsc::channel::<Watched>();
    let events = tx.clone();
    let server = server.clone();
    std::thread::spawn(move || {
        let mut watched = Watched::default();
        loop {
            match tasks_rx.recv_timeout(GIT_POLL) {
                // Only the latest list matters.
                Ok(list) => watched = tasks_rx.try_iter().last().unwrap_or(list),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if !read_watched(&server, &watched, &events) {
                break;
            }
        }
    });
    Ok(tasks_tx)
}

/// Read what `watched` asks for (spaces' places, tasks' git status, and
/// their details if wanted) and send it. Returns false once nobody listens.
fn read_watched(server: &Server, watched: &Watched, events: &Sender<Event>) -> bool {
    let read = places(server, &watched.spaces);
    if !read.is_empty() && events.send(Event::Spaces(read)).is_err() {
        return false;
    }
    let tasks = &watched.tasks;
    if tasks.is_empty() {
        return true;
    }
    let read = tasks
        .iter()
        .map(|(dir, branch)| ((dir.clone(), branch.clone()), git::status(dir, branch)))
        .collect();
    if events.send(Event::Git(read)).is_err() {
        return false;
    }
    if !watched.details {
        return true;
    }
    let read = tasks
        .iter()
        .map(|(dir, branch)| ((dir.clone(), branch.clone()), git::details(dir, branch)))
        .collect();
    events.send(Event::Details(read)).is_ok()
}
