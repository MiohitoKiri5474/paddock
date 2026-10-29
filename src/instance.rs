//! One paddock at a time (ADR 0003).
//!
//! The running paddock holds an exclusive `flock` on `paddock.pid` in the
//! state directory and writes its pid there. The kernel drops the lock when
//! that process ends, so a lock left behind by a crashed paddock never blocks.
//! A second launch asks whether to take over; on yes it sends the holder
//! SIGTERM (which detaches it, leaving every terminal running) and waits for
//! the lock.
//!
//! A `flock` belongs to the open file, which a child shares from its fork
//! until its `exec` (or its exit) closes it: a program paddock starts just
//! as it ends (a poll's `claude agents --json`, say) can hold the lock a
//! moment after paddock has gone, longer on a loaded machine. So a launch
//! that finds the lock held for a pid that no longer runs waits for the
//! lock instead of asking to take over from a paddock that is not there.

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
        if !left_behind(&file, pid)? {
            if !ask_to_take_over(pid)? {
                return Ok(None);
            }
            take_over(&file, pid)?;
        }
    }
    file.set_len(0)?;
    file.rewind()?;
    writeln!(file, "{}", std::process::id())?;
    Ok(Some(Instance { _lock: file }))
}

/// Whether paddock `pid`, which wrote the held lock, has ended and the
/// lock came free (taken now): then a child it started held the lock only
/// until its own `exec` or exit. Waits for the lock only while `pid` is
/// gone; held past [`TAKE_OVER_TIMEOUT`], the caller asks as for a running
/// paddock (a yes waits again, see [`take_over`]).
fn left_behind(file: &File, pid: i32) -> Result<bool> {
    // SAFETY: kill with signal 0 only checks that the process exists.
    let gone = unsafe { libc::kill(pid, 0) } != 0
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    Ok(gone && wait_for_lock(file)?)
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
    if !wait_for_lock(file)? {
        bail!("paddock (pid {pid}) did not close");
    }
    Ok(())
}

/// Take the lock once it is free, waiting at most [`TAKE_OVER_TIMEOUT`].
/// Returns whether it was taken.
fn wait_for_lock(file: &File) -> Result<bool> {
    let start = Instant::now();
    while !try_lock(file)? {
        if start.elapsed() > TAKE_OVER_TIMEOUT {
            return Ok(false);
        }
        std::thread::sleep(POLL);
    }
    Ok(true)
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
    use super::{left_behind, parse_pid, try_lock};
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    /// Lock `path` in a child that forks a grandchild, which shares the
    /// lock and waits, then exits: the child's pid (reaped) and the
    /// grandchild's, which still holds the lock. Between fork and exit
    /// both call only async-signal-safe functions.
    fn lock_left_in_a_grandchild(path: &std::path::Path) -> (i32, i32) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut pipe = [0; 2];
        // SAFETY: `pipe` has room for the two descriptors.
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        // SAFETY: the child calls only async-signal-safe functions.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork");
        if child == 0 {
            // SAFETY: async-signal-safe calls on valid arguments only.
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_RDWR);
                if fd < 0 || libc::flock(fd, libc::LOCK_EX) != 0 {
                    libc::_exit(1);
                }
                let grandchild = libc::fork();
                if grandchild == 0 {
                    libc::pause();
                    libc::_exit(0);
                }
                let bytes = grandchild.to_ne_bytes();
                libc::write(pipe[1], bytes.as_ptr().cast(), bytes.len());
                libc::_exit(0);
            }
        }
        let mut bytes = [0u8; 4];
        // SAFETY: `bytes` has room for what is read; both descriptors are ours.
        unsafe {
            libc::close(pipe[1]);
            assert_eq!(libc::read(pipe[0], bytes.as_mut_ptr().cast(), 4), 4);
            libc::close(pipe[0]);
        }
        let mut status = 0;
        // SAFETY: `child` is our child, `status` is valid.
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        (child, i32::from_ne_bytes(bytes))
    }

    #[test]
    fn a_lock_an_ended_paddock_left_in_its_child_is_waited_for() {
        let dir = std::env::temp_dir().join(format!("paddock-instance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("paddock.pid");
        let file = std::fs::File::create(&path).unwrap();
        let (ended, holder) = lock_left_in_a_grandchild(&path);
        assert!(!try_lock(&file).unwrap(), "the grandchild holds the lock");

        // The holder goes as it would exec: the lock comes free meanwhile.
        let kill = std::thread::spawn(move || {
            // SAFETY: `holder` is the grandchild forked above, still running.
            unsafe { libc::kill(holder, libc::SIGKILL) };
        });
        let freed = left_behind(&file, ended).unwrap();
        kill.join().unwrap();

        assert!(freed, "waited for the lock, not asked");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lock_a_running_paddock_holds_is_not_waited_for() {
        let dir = std::env::temp_dir().join(format!("paddock-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("paddock.pid");
        let file = std::fs::File::create(&path).unwrap();
        let (_, holder) = lock_left_in_a_grandchild(&path);

        // The holder runs: as a paddock that runs, it is asked about.
        let waited = left_behind(&file, holder).unwrap();
        // SAFETY: `holder` is the grandchild forked above, still running.
        unsafe { libc::kill(holder, libc::SIGKILL) };

        assert!(!waited);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_pid_accepts_only_real_process_ids() {
        assert_eq!(parse_pid("4242\n"), Some(4242));
        for bad in ["", "0", "1", "-1", "-4242", "abc"] {
            assert_eq!(parse_pid(bad), None, "{bad:?}");
        }
    }
}
