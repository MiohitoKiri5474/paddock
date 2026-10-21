//! Background threads that feed session information into the event loop:
//! new lines of the hook event log, `claude agents --json` snapshots, and the
//! git status of task rows on screen (and their details while the dashboard
//! is open), and each space's directory and branch.

use super::Event;
use crate::git;
use crate::hooks::now_ms;
use crate::sessions::{agents, log};
use crate::tmux::structure::FLOAT_OPTION;
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

/// Space `id`'s directory: that of its first tab's first pane, else the
/// session's start directory.
fn space_dir(server: &Server, id: &str) -> Option<String> {
    let format = format!("#{{{FLOAT_OPTION}}}\x1f#{{pane_current_path}}");
    let pane = server
        .run(&["list-panes", "-s", "-t", id, "-F", &format])
        .ok()
        .and_then(|out| first_tab_dir(&out));
    pane.or_else(|| server.display(id, "#{session_path}").ok())
        .filter(|p| !p.is_empty())
}

/// The first directory in `list-panes -s` output of float marker and
/// directory, skipping floats' panes: a float is never the first tab,
/// even when tmux lists it first.
fn first_tab_dir(out: &str) -> Option<String> {
    out.lines()
        .filter_map(|l| l.split_once('\x1f'))
        .find(|(float, _)| float.is_empty())
        .map(|(_, dir)| dir.to_string())
        .filter(|dir| !dir.is_empty())
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
    let events = tx.clone();
    std::thread::spawn(move || loop {
        let records = reader.read_new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_space_s_directory_is_its_first_tab_s_never_a_float_s() {
        let out = "g\x1f/w/float\n\x1f/w/tab\n\x1f/w/second\n";
        assert_eq!(first_tab_dir(out).as_deref(), Some("/w/tab"));
        assert_eq!(
            first_tab_dir("\x1f/w/tab\ng\x1f/w/float\n").as_deref(),
            Some("/w/tab")
        );
        assert_eq!(first_tab_dir("g\x1f/w/float\n"), None);
        assert_eq!(first_tab_dir("\x1f\n"), None);
    }
}
