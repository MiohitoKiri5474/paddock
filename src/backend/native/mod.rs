//! The native backend (ADR 0007): `paddock server`, a background process
//! that owns every terminal's PTY and an emulator for each, and paddock as
//! its client over a Unix socket in the state directory. It hosts the
//! spaces, each with its tabs, splits and floats, and saves them to
//! `layout.json` to rebuild them when it next starts (ADR 0008).
//!
//! - [`client`]: what the app, the CLI and `paddock debug state` ask, and
//!   the attached space's connection. Starts the server when needed.
//! - [`server`]: the server process: its socket, lock and lifetime.
//! - [`state`]: the spaces it holds and what it answers about them.
//! - [`space`], [`tab`] and [`layout`]: a space's tabs and floats, a
//!   tab's focus and zoom, and its tree of splits.
//! - [`saved`] and [`restore`]: `layout.json`, written as the layout
//!   changes, and the layout rebuilt from it.
//! - [`scrollback`]: each terminal's scrollback, saved as the server
//!   ends and drawn again above its restored shell.
//! - [`terminal`] and [`pty`]: one terminal, its shell and its emulator.
//! - [`handover`]: what the server hands to a new binary on an upgrade
//!   (ADR 0009), so programs run on.
//! - [`proc`]: a process's name and directory, as macOS or Linux says.
//! - [`hook`]: the variables that tell a process which terminal it runs in.
//! - [`queue`]: the queues to the threads that write to a client or a
//!   program.
//! - [`protocol`]: the frames between them.

mod axis;
mod client;
mod handover;
mod hook;
mod layout;
mod limits;
mod proc;
mod protocol;
mod pty;
mod queue;
mod restore;
mod saved;
mod scrollback;
mod seen;
mod server;
mod space;
mod state;
mod tab;
mod terminal;

pub use client::{Attached, Client, NoHandover, Upgraded};
pub use handover::check as check_handover;
pub use protocol::OtherVersion;
pub use server::{serve, take_over};

use crate::paths::Paths;
use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The server's socket, in the state directory.
const SOCKET: &str = "server.sock";
/// Held by the running server, with its pid: one server per user.
const LOCK: &str = "server.lock";
/// Where the server writes what goes wrong.
const LOG: &str = "server.log";
/// The longest socket path the platform takes (`sun_path` holds 104 bytes
/// on macOS, 108 on Linux, its final NUL included).
#[cfg(target_os = "macos")]
const MAX_SOCKET_PATH: usize = 103;
#[cfg(target_os = "linux")]
const MAX_SOCKET_PATH: usize = 107;

/// The native terminal a process runs in, from its environment as `var`
/// reads it (see [`hook::terminal`]); `None` outside the terminals of the
/// server under `paths`.
pub fn hook_terminal(
    var: impl Fn(&str) -> Option<String>,
    paths: &Paths,
) -> Option<crate::backend::HookTerminal> {
    hook::terminal(var, &socket_path(paths).ok()?)
}

/// The server's socket under `paths`; an error if the path is too long for
/// a Unix socket.
fn socket_path(paths: &Paths) -> Result<PathBuf> {
    let path = paths.state_dir.join(SOCKET);
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        bail!(
            "the paddock server's socket path is {len} bytes, longer than this system allows \
             ({MAX_SOCKET_PATH}): {}; set XDG_STATE_HOME to a shorter directory",
            path.display()
        );
    }
    Ok(path)
}

/// Take the server lock in `state_dir` and write our pid in it; `None` if
/// another server holds it. Held until the returned file is dropped (or
/// the process ends).
fn lock(state_dir: &Path) -> Result<Option<File>> {
    let path = state_dir.join(LOCK);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    // SAFETY: the descriptor is valid while `file` is borrowed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(err).with_context(|| format!("cannot lock {}", path.display()));
    }
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(Some(file))
}

/// Whether a server holds the lock in `state_dir`; unlike [`lock`],
/// writes nothing (a lock taken to see is let go at once).
fn held(state_dir: &Path) -> Result<bool> {
    let path = state_dir.join(LOCK);
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("cannot open {}", path.display())),
    };
    // SAFETY: the descriptor is valid while `file` is borrowed; dropping
    // it lets go of a lock taken here.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(false);
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(true);
    }
    Err(err).with_context(|| format!("cannot lock {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(state_dir: &Path) -> Paths {
        Paths {
            state_dir: state_dir.into(),
            config_dir: state_dir.into(),
        }
    }

    #[test]
    fn the_socket_lives_in_the_state_dir_unless_its_path_is_too_long() {
        let p = socket_path(&paths(Path::new("/Users/me/.local/state/paddock"))).unwrap();
        assert_eq!(p, Path::new("/Users/me/.local/state/paddock/server.sock"));
        let long = format!("/{}", "d".repeat(100));
        let err = socket_path(&paths(Path::new(&long)))
            .unwrap_err()
            .to_string();
        assert!(err.contains("XDG_STATE_HOME"), "{err}");
    }

    #[test]
    fn one_lock_at_a_time_and_it_names_its_holder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!held(dir.path()).unwrap(), "no lock file yet");
        let taken = lock(dir.path()).unwrap().expect("the first takes it");
        assert!(lock(dir.path()).unwrap().is_none(), "the second does not");
        assert!(held(dir.path()).unwrap());
        let pid = std::fs::read_to_string(dir.path().join(LOCK)).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        drop(taken);
        // A child forked meanwhile by another test holds the descriptor
        // until it execs (it is close-on-exec): give it a moment.
        let start = std::time::Instant::now();
        while lock(dir.path()).unwrap().is_none() {
            assert!(start.elapsed().as_secs() < 2, "free once dropped");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
