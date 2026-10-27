//! A paddock under strain: input sent from a thread of its own, and the
//! process stopped a while, for the native server's queues.

use super::Paddock;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long paddock gets to take what [`Paddock::send_in_background`]
/// sends: pastes of many MiB (17 MiB takes about 2 s alone), while the
/// whole suite runs.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(30);

impl Paddock {
    /// Send `bytes` from a thread of its own, 4 KiB at a time (other sends
    /// get in between): for input paddock may stop taking, which must not
    /// hang the test. Wait for it with [`finished`].
    pub fn send_in_background(&self, bytes: Vec<u8>) -> JoinHandle<()> {
        let writer = Arc::clone(&self.writer);
        std::thread::spawn(move || {
            for chunk in bytes.chunks(4096) {
                let mut w = writer.lock().expect("writer lock");
                if w.write_all(chunk).and_then(|()| w.flush()).is_err() {
                    return;
                }
            }
        })
    }

    /// Stop paddock (SIGSTOP) until `done` holds, then let it go on
    /// (SIGCONT), as Ctrl-Z and `fg` would; also if `done` panics.
    pub fn stall_until(&self, done: impl Fn() -> bool) {
        let pid = self.child.process_id().expect("paddock's pid");
        signal(pid, "-STOP");
        let _resume = Resume(pid);
        while !done() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Wait for a [`Paddock::send_in_background`] to have sent everything;
/// panic saying `what` if it has not within [`SEND_TIMEOUT`] (paddock stopped
/// taking input).
pub fn finished(send: JoinHandle<()>, what: &str) {
    let start = Instant::now();
    while !send.is_finished() {
        assert!(start.elapsed() < SEND_TIMEOUT, "paddock never took {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
    send.join().expect("the sending thread");
}

fn signal(pid: u32, name: &str) {
    let status = Command::new("kill").args([name, &pid.to_string()]).status();
    assert!(status.is_ok_and(|s| s.success()), "kill {name} {pid}");
}

/// Lets a stopped paddock go on when dropped.
struct Resume(u32);

impl Drop for Resume {
    fn drop(&mut self) {
        // Not `signal`: no panic while one may be unwinding.
        let _ = Command::new("kill")
            .args(["-CONT", &self.0.to_string()])
            .status();
    }
}
