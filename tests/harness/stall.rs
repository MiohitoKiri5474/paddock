//! A paddock under strain: input sent from a thread of its own, and the
//! process stopped a while, for the native server's queues.

use super::{Paddock, TIMEOUT};
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a client gets to write one frame to the server, which never
/// waits on a program before it reads on.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Input being sent by [`Paddock::send_in_background`].
pub struct Sending {
    thread: JoinHandle<()>,
    /// Bytes paddock has taken so far.
    sent: Arc<AtomicUsize>,
}

impl Paddock {
    /// Send `bytes` from a thread of its own, 4 KiB at a time (other sends
    /// get in between): for input paddock may stop taking, which must not
    /// hang the test. Wait for it with [`finished`].
    pub fn send_in_background(&self, bytes: Vec<u8>) -> Sending {
        let writer = Arc::clone(&self.writer);
        let sent = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&sent);
        let thread = std::thread::spawn(move || {
            for chunk in bytes.chunks(4096) {
                let mut w = writer.lock().expect("writer lock");
                if w.write_all(chunk).and_then(|()| w.flush()).is_err() {
                    return;
                }
                counted.fetch_add(chunk.len(), Ordering::Relaxed);
            }
        });
        Sending { thread, sent }
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

/// Wait for a [`Paddock::send_in_background`] to have sent everything,
/// as long as paddock keeps taking it: pastes of many MiB take a while
/// under load, so only [`TIMEOUT`] with nothing taken fails, saying
/// `what` (paddock stopped taking input).
pub fn finished(send: Sending, what: &str) {
    let mut taken = (0, Instant::now());
    while !send.thread.is_finished() {
        let now = send.sent.load(Ordering::Relaxed);
        if now != taken.0 {
            taken = (now, Instant::now());
        }
        assert!(
            taken.1.elapsed() < TIMEOUT,
            "paddock stopped taking {what} after {now} bytes"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    send.thread.join().expect("the sending thread");
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
