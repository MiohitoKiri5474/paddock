//! Which backend the suite runs on: `PADDOCK_TEST_BACKEND=native` runs
//! every test against paddock's own server (`[backend] kind = "native"`,
//! written into each sandbox's config), anything else or nothing against
//! tmux. The native server lives in the sandbox's state directory, socket
//! and lock included, so it never meets the user's own.
//!
//! On native, run with `--skip tmux_only --skip native_needs`: a test
//! whose name holds `native_needs_4N` needs ticket 4N's part of the native
//! backend, which is not there yet.

use std::path::Path;
use std::time::{Duration, Instant};

/// The environment variable that picks the backend.
const VAR: &str = "PADDOCK_TEST_BACKEND";

/// Whether this run is on the native backend.
pub fn native() -> bool {
    std::env::var(VAR).is_ok_and(|v| v == "native")
}

/// A config that picks the native backend whatever the run's.
pub const NATIVE: &str = "[backend]\nkind = \"native\"\n";

/// The `[backend]` table a sandbox's config `text` starts with: none if
/// it has its own (a test of one backend's).
pub fn config(text: &str) -> &'static str {
    if native() && !text.contains("[backend]") {
        "[backend]\nkind = \"native\"\n"
    } else {
        ""
    }
}

/// The pid of the native server running for `state_dir` (paddock's own
/// state directory), from its lock; `None` if none ever ran.
pub fn server_pid(state_dir: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(state_dir.join("server.lock")).ok()?;
    text.trim().parse().ok().filter(|&pid| pid > 1)
}

/// Whether a process holds the lock on `path`.
fn locked(path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    // SAFETY: flock on a descriptor `file` owns; dropping `file` lets go
    // of a lock taken here.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
}

/// Whether process `pid` runs.
pub fn alive(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// End the native server for `state_dir`, if one runs: it hangs its
/// shells up and removes its socket.
pub fn stop_server(state_dir: &Path) {
    let Some(pid) = server_pid(state_dir) else {
        return;
    };
    // The lock outlives its server, with its pid, which another process
    // may have since: only a held lock means a server runs.
    if !locked(&state_dir.join("server.lock")) {
        return;
    }
    let signal = |name: &str| {
        let _ = std::process::Command::new("kill")
            .args([name, &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
    };
    signal("-TERM");
    let start = Instant::now();
    while alive(pid) {
        if start.elapsed() > Duration::from_secs(3) {
            signal("-KILL");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
