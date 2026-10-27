//! A paddock under strain: input sent from a thread of its own, and the
//! process stopped a while, for the native server's queues.

use super::Paddock;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

impl Paddock {
    /// Send `bytes` from a thread of its own, 4 KiB at a time (other sends
    /// get in between): for input paddock may stop taking, which must not
    /// hang the test.
    pub fn send_in_background(&self, bytes: Vec<u8>) -> std::thread::JoinHandle<()> {
        let writer = Arc::clone(&self.shared);
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
    /// (SIGCONT), as Ctrl-Z and `fg` would.
    pub fn stall_until(&self, done: impl Fn() -> bool) {
        let pid = self.child.process_id().expect("paddock's pid").to_string();
        let signal = |name: &str| {
            let status = Command::new("kill").args([name, &pid]).status();
            assert!(status.is_ok_and(|s| s.success()), "kill {name}");
        };
        signal("-STOP");
        while !done() {
            std::thread::sleep(Duration::from_millis(20));
        }
        signal("-CONT");
    }
}
