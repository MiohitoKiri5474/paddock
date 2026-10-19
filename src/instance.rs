//! One paddock at a time (ADR 0003).
//!
//! The running paddock holds an exclusive `flock` on `paddock.pid` in the
//! state directory and writes its pid there. The kernel drops the lock when
//! that process ends, so a lock left behind by a crashed paddock never blocks.
//! A second launch asks whether to take over; on yes it sends the holder
//! SIGTERM (which detaches it, leaving every terminal running) and waits for
//! the lock.

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

const LOCK_FILE: &str = "paddock.pid";
/// How long the old paddock gets to detach after being asked to.
const TAKE_OVER_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// Proof that this is the one running paddock; held until it exits.
pub struct Instance {
    _lock: File,
}

/// Become the one running paddock, asking to take over if another one runs.
/// `None` when the user declines.
pub fn claim(state_dir: &Path) -> Result<Option<Instance>> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("cannot create {}", state_dir.display()))?;
    let path = state_dir.join(LOCK_FILE);
    // std opens files close-on-exec, so tmux never inherits the lock.
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    if !try_lock(&file)? {
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let Some(pid) = parse_pid(&text) else {
            bail!("another paddock is starting ({} is locked)", path.display());
        };
        if !ask_to_take_over(pid)? {
            return Ok(None);
        }
        take_over(&file, pid)?;
    }
    file.set_len(0)?;
    file.rewind()?;
    writeln!(file, "{}", std::process::id())?;
    Ok(Some(Instance { _lock: file }))
}

fn ask_to_take_over(pid: i32) -> Result<bool> {
    print!("paddock is already running (pid {pid}). Take over? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("y") || answer.trim().eq_ignore_ascii_case("yes"))
}

/// Ask paddock `pid` to detach and wait until its lock is free.
fn take_over(file: &File, pid: i32) -> Result<()> {
    // The prompt can wait a long time; if the old paddock exited meanwhile its
    // pid may belong to another process now, so never signal it.
    if try_lock(file)? {
        return Ok(());
    }
    // SAFETY: kill has no memory-safety preconditions.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH: it is already gone and its lock about to be released.
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err).with_context(|| format!("cannot signal paddock (pid {pid})"));
        }
    }
    let start = Instant::now();
    while !try_lock(file)? {
        if start.elapsed() > TAKE_OVER_TIMEOUT {
            bail!("paddock (pid {pid}) did not close");
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

/// The pid in the lock file. Rejects 0, 1 and negatives: `kill` would signal
/// a process group, launchd, or every process the user owns.
fn parse_pid(text: &str) -> Option<i32> {
    text.trim().parse::<i32>().ok().filter(|&pid| pid > 1)
}

/// Take the exclusive lock without waiting. Returns whether it was taken.
fn try_lock(file: &File) -> Result<bool> {
    // SAFETY: the descriptor is valid for as long as `file` is borrowed.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err).context("cannot lock the paddock instance file")
    }
}

#[cfg(test)]
mod tests {
    use super::parse_pid;

    #[test]
    fn parse_pid_accepts_only_real_process_ids() {
        assert_eq!(parse_pid("4242\n"), Some(4242));
        for bad in ["", "0", "1", "-1", "-4242", "abc"] {
            assert_eq!(parse_pid(bad), None, "{bad:?}");
        }
    }
}
