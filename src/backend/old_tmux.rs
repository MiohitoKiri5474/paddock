//! The hidden tmux server an older paddock left running (ADR 0001), found
//! at start on the native backend: its terminals are not moved to the
//! native server, so paddock says so, and how to close it.
//!
//! Once per such server: a server is named by its pid and start time,
//! kept in [`SEEN_FILE`] in the state directory once paddock has printed
//! the notice ([`OldTmuxServer::told`]). The same server found again
//! stays quiet; another one (the old server closed and a tmux-backed
//! paddock started it again) is said anew. A start that ends before
//! printing it (killed, or its terminal closed) says it again next time.

use super::tmux;
use crate::config::BackendKind;
use crate::paths::Paths;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// The last old server paddock told about, as `<pid> <start time>`.
const SEEN_FILE: &str = "old-tmux-server";
/// How long the old server may take to answer; later is no server, so a
/// stuck one never holds paddock's start.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// An old tmux server paddock has not told about yet.
#[derive(Debug)]
pub struct OldTmuxServer {
    id: String,
    socket: String,
    seen: PathBuf,
}

impl OldTmuxServer {
    /// The one on `paths.socket`, on the `kind` backend: only the native
    /// backend has one to tell about. `None` if none runs or it was told.
    pub fn find(paths: &Paths, kind: BackendKind) -> Option<Self> {
        if kind != BackendKind::Native {
            return None;
        }
        let id = probe(paths)?;
        let seen = paths.state_dir.join(SEEN_FILE);
        (!was_told(&seen, &id)).then(|| Self {
            id,
            socket: paths.socket.clone(),
            seen,
        })
    }

    /// What paddock says about it.
    pub fn notice(&self) -> String {
        notice(&self.socket)
    }

    /// Remember it as told, once the notice was printed. Not remembered
    /// (state directory not writable): said again next time, which beats
    /// never.
    pub fn told(self) {
        if let Some(dir) = self.seen.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&self.seen, format!("{}\n", self.id));
    }
}

/// The tmux server on `paths.socket` as `<pid> <start time>`, if one
/// runs and answers within [`PROBE_TIMEOUT`]. Never starts a server; no
/// tmux, or one that cannot say, is no server.
fn probe(paths: &Paths) -> Option<String> {
    let mut command = tmux::Server::new(paths.clone()).command();
    command
        .args(["display-message", "-p", "#{pid} #{start_time}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = crate::git::wait(command, PROBE_TIMEOUT).ok()?;
    let id = out.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// Whether the server `id` was told about already.
fn was_told(seen: &Path, id: &str) -> bool {
    std::fs::read_to_string(seen).is_ok_and(|s| s.trim() == id)
}

/// What paddock says about an old server on tmux socket `socket`.
fn notice(socket: &str) -> String {
    format!(
        "an older paddock's tmux server still runs; its terminals are not moved here. \
         Close it with: tmux -L {socket} kill-server"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_names_the_socket_to_close() {
        let n = notice("paddock");
        assert!(n.contains("not moved"), "{n}");
        assert!(n.ends_with("tmux -L paddock kill-server"), "{n}");
    }

    #[test]
    fn a_server_is_told_about_once_by_its_pid_and_start_time() {
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("state").join(SEEN_FILE);
        let server = |id: &str| OldTmuxServer {
            id: id.into(),
            socket: "paddock".into(),
            seen: seen.clone(),
        };
        assert!(!was_told(&seen, "1 100"));
        server("1 100").told();
        assert!(was_told(&seen, "1 100"));
        assert!(!was_told(&seen, "2 100"), "another server");
        assert!(!was_told(&seen, "1 200"), "the pid again, later");
    }

    #[test]
    fn the_tmux_backend_has_no_old_server_to_tell_about() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            socket: "paddock-unit-none".into(),
            state_dir: dir.path().join("state"),
            config_dir: dir.path().join("config"),
        };
        assert!(OldTmuxServer::find(&paths, BackendKind::Tmux).is_none());
    }
}
