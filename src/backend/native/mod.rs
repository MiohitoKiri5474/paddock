//! The native backend (ADR 0007): `paddock server`, a background process
//! that owns every terminal's PTY and an emulator for each, and paddock as
//! its client over a Unix socket in the state directory. So far it hosts
//! one space with one tab and one terminal; what it cannot do yet fails
//! with [`unsupported`].
//!
//! - [`client`]: what the app, the CLI and `paddock debug state` ask, and
//!   the attached space's connection. Starts the server when needed.
//! - [`server`]: the server process: its socket, lock and lifetime.
//! - [`state`]: the spaces it holds and what it answers about them.
//! - [`host`] and [`pty`]: one terminal, its shell and its emulator.
//! - [`protocol`]: the frames between them.

mod client;
mod host;
mod protocol;
mod pty;
mod server;
mod state;

pub use client::{Attached, Client};
pub use server::serve;

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
/// The longest socket path macOS takes (`sun_path` holds 104 bytes, its
/// final NUL included).
const MAX_SOCKET_PATH: usize = 103;

/// The error for what the native backend cannot do yet.
pub fn unsupported(what: &str) -> anyhow::Error {
    anyhow::anyhow!("not on the native backend yet: {what}")
}

/// The server's socket under `paths`; an error if the path is too long for
/// a Unix socket.
fn socket_path(paths: &Paths) -> Result<PathBuf> {
    let path = paths.state_dir.join(SOCKET);
    let len = path.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        bail!(
            "the paddock server's socket path is {len} bytes, longer than macOS allows \
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

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(state_dir: &Path) -> Paths {
        Paths {
            socket: "unused".into(),
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
        let held = lock(dir.path()).unwrap().expect("the first takes it");
        assert!(lock(dir.path()).unwrap().is_none(), "the second does not");
        let pid = std::fs::read_to_string(dir.path().join(LOCK)).unwrap();
        assert_eq!(pid.trim(), std::process::id().to_string());
        drop(held);
        assert!(lock(dir.path()).unwrap().is_some(), "free once dropped");
    }
}
