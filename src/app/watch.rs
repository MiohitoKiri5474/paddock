//! Background threads that feed session information into the event loop:
//! new lines of the hook event logs (this server's and the external one),
//! `claude agents --json` snapshots, the git status of task rows on screen
//! (and their details while the dashboard is open), and each space's
//! directory and branch.

use super::Event;
use crate::backend::{Backend, SpaceId};
use crate::git;
use crate::hooks::now_ms;
use crate::paths::HomePaths;
use crate::sessions::{agents, log, HookRecord};
use crate::worktree::space_dir;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often to look for new hook events.
const LOG_POLL: Duration = Duration::from_millis(200);
/// How often to ask Claude which sessions run (catches what hooks miss).
const AGENTS_POLL: Duration = Duration::from_secs(3);
/// How often to re-read git for the task rows on screen.
const GIT_POLL: Duration = Duration::from_secs(3);
/// How often to look whether the external log needs trimming (a `stat`).
const EXTERNAL_CHECK: Duration = AGENTS_POLL;

/// Trim the external log (see [`log::compact`]), whose reader got to
/// `read`.
fn compact_external(path: &Path, read: u64) -> Result<Option<log::Compacted>> {
    log::compact(
        path,
        log::EXTERNAL_MAX_BYTES,
        now_ms(),
        log::EXTERNAL_KEEP_MS,
        read,
    )
}

/// While paddock runs: once the external log passes its cap, trim it and
/// have `reader` go on in the trimmed file. Returns the records past what
/// `reader` had read, from the bytes the trim read, so none is skipped or
/// read twice.
fn trim_external(path: &Path, reader: &mut log::Reader) -> Vec<HookRecord> {
    if !log::too_big(path, log::EXTERNAL_MAX_BYTES) {
        return Vec::new();
    }
    match compact_external(path, reader.position()) {
        Ok(Some(done)) => {
            reader.resume_at(done.kept);
            done.unread
        }
        _ => Vec::new(),
    }
}

/// Tasks to keep the git status of, as `(directory, branch)`.
pub type Tasks = Vec<(String, String)>;

/// The tasks on screen, whether the dashboard's details are wanted too,
/// and the spaces whose directory and branch to read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watched {
    pub tasks: Tasks,
    pub details: bool,
    pub spaces: Vec<SpaceId>,
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
    /// The space's root as stored (as the user made it, links kept);
    /// `None` for a space made before roots, whose root is `path`.
    pub root: Option<String>,
    /// What is checked out at the root (as `branch` at `path`); `None`
    /// outside a git checkout.
    pub root_branch: Option<String>,
}

impl Place {
    /// The space's root: as stored, else its directory.
    pub fn root(&self) -> &str {
        self.root.as_deref().unwrap_or(&self.path)
    }

    /// The directory and the branch checked out there, as the sidebar draws
    /// them: the key its pull request is read and kept by. `None` outside
    /// a repository.
    pub fn pr_key(&self) -> Option<(String, String)> {
        Some((self.path.clone(), self.branch.clone()?))
    }

    /// The root and what is checked out there, as the finder shows them
    /// (home as `~`): a branch always belongs to the directory beside it.
    pub fn root_shown(&self, home: &HomePaths) -> (String, Option<String>) {
        (home.show(self.root()), self.root_branch.clone())
    }
}

/// Space `id`'s directory, root and what is checked out in each, read
/// now; `None` if the backend cannot say.
pub fn place(backend: &Backend, id: &SpaceId) -> Option<Place> {
    let path = space_dir(backend, id)?;
    let branch = git::head(&path);
    let tree = git::working_tree(&path);
    let root = backend.root(id);
    let root_branch = match &root {
        Some(root) => git::head(root),
        None => branch.clone(),
    };
    Some(Place {
        path,
        branch,
        tree,
        root,
        root_branch,
    })
}

/// Read each of `spaces`' place.
fn places(backend: &Backend, spaces: &[SpaceId]) -> Vec<(SpaceId, Place)> {
    spaces
        .iter()
        .filter_map(|id| Some((id.clone(), place(backend, id)?)))
        .collect()
}

/// The hook event log of `backend` (see [`log::path`]).
pub fn server_log(backend: &Backend, state_dir: &Path) -> Result<PathBuf> {
    Ok(log::path(state_dir, backend.instance()?))
}

/// Start the threads, reading the server's hook events from `server_log`
/// (see [`server_log`]), and each terminal's command while `agy_running`
/// says an Antigravity session runs in one. They end when the event loop
/// goes away. Returns where to send the tasks on screen: each list is read
/// at once, then every [`GIT_POLL`] until the next one.
pub fn start(
    backend: &Backend,
    server_log: PathBuf,
    agy_running: Arc<AtomicBool>,
    state_dir: &Path,
    tx: &Sender<Event>,
) -> Result<Sender<Watched>> {
    let mut reader = log::Reader::new(server_log);
    // Every agent on the machine appends to the external log: trim it
    // before replaying it, and while paddock runs. Failing that, it is read
    // whole.
    let external_path = log::external_path(state_dir);
    let _ = compact_external(&external_path, u64::MAX);
    let mut external = log::Reader::new(external_path.clone());
    let events = tx.clone();
    std::thread::spawn(move || {
        let mut checked = Instant::now();
        loop {
            let server = reader.read_new();
            let mut outside = external.read_new();
            if checked.elapsed() >= EXTERNAL_CHECK {
                checked = Instant::now();
                outside.extend(trim_external(&external_path, &mut external));
            }
            // One batch in time order, late records included.
            let records = log::merged(server, outside);
            if !records.is_empty() && events.send(Event::Hooks(records)).is_err() {
                break;
            }
            std::thread::sleep(LOG_POLL);
        }
    });

    let agents_backend = backend.clone();
    let events = tx.clone();
    std::thread::spawn(move || loop {
        // Whether Antigravity quit back to the shell (see
        // `Sessions::end_exited`).
        let taken = now_ms();
        let commands = agy_running
            .load(Ordering::Relaxed)
            .then(|| agents_backend.terminal_commands())
            .flatten();
        if let Some(commands) = commands {
            if events.send(Event::Commands(taken, commands)).is_err() {
                break;
            }
        }
        let taken = now_ms();
        if let Some(list) = agents::fetch(&agents_backend) {
            if events.send(Event::Agents(taken, list)).is_err() {
                break;
            }
        }
        std::thread::sleep(AGENTS_POLL);
    });

    let (tasks_tx, tasks_rx) = mpsc::channel::<Watched>();
    let events = tx.clone();
    let backend = backend.clone();
    std::thread::spawn(move || {
        let mut watched = Watched::default();
        loop {
            match tasks_rx.recv_timeout(GIT_POLL) {
                // Only the latest list matters.
                Ok(list) => watched = tasks_rx.try_iter().last().unwrap_or(list),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if !read_watched(&backend, &watched, &events) {
                break;
            }
        }
    });
    Ok(tasks_tx)
}

/// Read what `watched` asks for (spaces' places, tasks' git status, and
/// their details if wanted) and send it. Returns false once nobody listens.
fn read_watched(backend: &Backend, watched: &Watched, events: &Sender<Event>) -> bool {
    let read = places(backend, &watched.spaces);
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
    fn a_space_shows_its_root_as_stored_with_what_is_checked_out_there() {
        let home = HomePaths::new();
        let place = |root: Option<&str>, root_branch: Option<&str>| Place {
            path: "/w/api".into(),
            branch: Some("main".into()),
            tree: None,
            root: root.map(str::to_string),
            root_branch: root_branch.map(str::to_string),
        };
        // Made before roots: its directory is its root.
        assert_eq!(
            place(None, Some("main")).root_shown(&home),
            ("/w/api".into(), Some("main".into()))
        );
        // Stored as made, links kept; the branch is the root's own.
        assert_eq!(
            place(Some("/tmp/x/sub"), Some("feat")).root_shown(&home),
            ("/tmp/x/sub".into(), Some("feat".into()))
        );
        assert_eq!(
            place(Some("/w/notes"), None).root_shown(&home),
            ("/w/notes".into(), None)
        );
    }
}
