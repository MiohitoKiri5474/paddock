//! The hidden tmux server an older paddock left running (ADR 0001), found
//! at start on the native backend: its terminals are not moved to the
//! native server, so paddock says so, and how to close it.
//!
//! Once per such server: the notice names a server by its pid and start
//! time, kept in [`SEEN_FILE`] in the state directory. The same server
//! found again stays quiet; another one (the old server closed and a
//! tmux-backed paddock started it again) is said anew.

use super::tmux;
use crate::paths::Paths;

/// The last old server paddock told about, as `<pid> <start time>`.
const SEEN_FILE: &str = "old-tmux-server";

/// The notice for an old tmux server on `paths.socket`, if one runs and
/// was not told about yet; it is then remembered as told. Never starts a
/// server; no tmux, or one that cannot say, is no server.
pub fn old_tmux_server_notice(paths: &Paths) -> Option<String> {
    let server = tmux::Server::new(paths.clone());
    let id = server
        .run(&["display-message", "-p", "#{pid} #{start_time}"])
        .ok()?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let seen = paths.state_dir.join(SEEN_FILE);
    if std::fs::read_to_string(&seen).is_ok_and(|s| s.trim() == id) {
        return None;
    }
    // Not remembered (state directory not writable): said again next time,
    // which beats never.
    let _ = std::fs::create_dir_all(&paths.state_dir)
        .and_then(|()| std::fs::write(&seen, format!("{id}\n")));
    Some(notice(&paths.socket))
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
}
