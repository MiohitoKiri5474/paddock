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
//! until its `exec` (or its exit) closes it, close-on-exec as it is. A child
//! in that gap as paddock ends (one `std` forks rather than spawns, as the
//! server's start with its `pre_exec` is) can hold the lock a moment after
//! paddock has gone, longer on a loaded machine. So paddock lets go of the
//! lock itself as it ends ([`Instance`]'s drop), which frees it whoever
//! shares it; and, for an end that skips that (a crash, a kill), a launch
//! that finds the lock held for a pid that no longer runs waits for the
//! lock instead of asking to take over from a paddock that is not there.

use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

const LOCK_FILE: &str = "paddock.pid";
/// How long a launch waits for the lock: for the old paddock to detach
/// after being asked to, or for a child of an ended one to let it go.
const TAKE_OVER_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// Proof that this is the one running paddock; held until it exits.
pub struct Instance {
    lock: File,
}

impl Drop for Instance {
    /// Let go of the lock on the open file itself, not only this
    /// descriptor: a child forked a moment ago that shares it until its
    /// `exec` no longer keeps the next launch out.
    fn drop(&mut self) {
        // SAFETY: the descriptor is valid while `self.lock` is borrowed.
        unsafe { libc::flock(self.lock.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Become the one running paddock, asking to take over if another one runs.
/// `None` when the user declines.
pub fn claim(state_dir: &Path) -> Result<Option<Instance>> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("cannot create {}", state_dir.display()))?;
    let path = state_dir.join(LOCK_FILE);
    // std opens files close-on-exec, so the server never inherits the lock.
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
    Ok(Some(Instance { lock: file }))
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
        #[cfg(test)]
        tests::WAITED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    use super::{left_behind, parse_pid, try_lock, Instance};
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Polls [`super::wait_for_lock`] found the lock held: proof a test
    /// went the waiting way. Only the test that waits reads it.
    pub(super) static WAITED: AtomicUsize = AtomicUsize::new(0);

    /// Descriptors a forked child closes, up to this many: the test
    /// binary's (other tests' pipes and locks) must not stay held in it.
    const MAX_FD: i32 = 65_536;

    /// A process forked here that holds a lock and waits: killed on drop,
    /// so a failed assert leaves none behind.
    struct Holder(i32);

    impl Drop for Holder {
        fn drop(&mut self) {
            assert!(self.0 > 1, "never kill a process group or every process");
            // SAFETY: `self.0` is a process forked by this test, which
            // waits until killed.
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }

    /// A directory of the test's own, removed on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("paddock-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The highest descriptor a forked child closes (taken before the fork:
    /// `sysconf` is not async-signal-safe).
    fn max_fd() -> i32 {
        // SAFETY: sysconf has no preconditions.
        let max = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        i32::try_from(max).unwrap_or(MAX_FD).clamp(256, MAX_FD)
    }

    /// In a forked child: close every descriptor from 3 to `max` but
    /// `keep`. Async-signal-safe.
    unsafe fn close_all_but(keep: i32, max: i32) {
        for fd in 3..max {
            if fd != keep {
                libc::close(fd);
            }
        }
    }

    /// A pipe, close-on-exec, so no program another test starts meanwhile
    /// keeps its write end open (on macOS only from the `fcntl` on).
    fn pipe() -> [i32; 2] {
        let mut fds = [0; 2];
        #[cfg(target_os = "linux")]
        // SAFETY: `fds` has room for the two descriptors.
        let made = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        #[cfg(not(target_os = "linux"))]
        // SAFETY: `fds` has room for the two descriptors, which are ours.
        let made = unsafe {
            let made = libc::pipe(fds.as_mut_ptr());
            for fd in fds {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            made
        };
        assert_eq!(made, 0, "pipe");
        fds
    }

    /// Lock `path` in a child that forks a grandchild, which shares the
    /// lock and waits, then exits: the child's pid (reaped) and the
    /// grandchild, which still holds the lock. Between fork and exit both
    /// call only async-signal-safe functions, with every other descriptor
    /// closed.
    fn lock_left_in_a_grandchild(path: &Path) -> (i32, Holder) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let max = max_fd();
        let [read_end, write_end] = pipe();
        // SAFETY: the child calls only async-signal-safe functions.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork");
        if child == 0 {
            // SAFETY: async-signal-safe calls on valid arguments only.
            unsafe {
                close_all_but(write_end, max);
                let fd = libc::open(path.as_ptr(), libc::O_RDWR);
                if fd < 0 || libc::flock(fd, libc::LOCK_EX) != 0 {
                    libc::_exit(1);
                }
                let grandchild = libc::fork();
                if grandchild < 0 {
                    libc::_exit(1);
                }
                if grandchild == 0 {
                    libc::close(write_end);
                    libc::pause();
                    libc::_exit(0);
                }
                let bytes = grandchild.to_ne_bytes();
                libc::write(write_end, bytes.as_ptr().cast(), bytes.len());
                libc::_exit(0);
            }
        }
        let mut bytes = [0u8; 4];
        // SAFETY: `bytes` has room for what is read; both descriptors are ours.
        let read = unsafe {
            libc::close(write_end);
            let read = libc::read(read_end, bytes.as_mut_ptr().cast(), 4);
            libc::close(read_end);
            read
        };
        let mut status = 0;
        // SAFETY: `child` is our child, `status` is valid.
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        assert_eq!(read, 4, "the grandchild's pid");
        let holder = i32::from_ne_bytes(bytes);
        assert!(holder > 1, "a grandchild: {holder}");
        (child, Holder(holder))
    }

    #[test]
    fn a_lock_an_ended_paddock_left_in_its_child_is_waited_for() {
        let dir = Dir::new("instance-ended");
        let path = dir.0.join("paddock.pid");
        let file = std::fs::File::create(&path).unwrap();
        let (ended, holder) = lock_left_in_a_grandchild(&path);
        assert!(!try_lock(&file).unwrap(), "the grandchild holds the lock");

        // The holder goes, as it would at its exec, only once the launch
        // is waiting for the lock.
        let waited_before = WAITED.load(Ordering::SeqCst);
        let release = std::thread::spawn(move || {
            let start = Instant::now();
            while WAITED.load(Ordering::SeqCst) == waited_before
                && start.elapsed() < Duration::from_secs(10)
            {
                std::thread::yield_now();
            }
            drop(holder);
        });
        let freed = left_behind(&file, ended).unwrap();
        release.join().unwrap();

        assert!(WAITED.load(Ordering::SeqCst) > waited_before, "it waited");
        assert!(freed, "waited for the lock, not asked");
    }

    #[test]
    fn a_lock_a_running_paddock_holds_is_not_waited_for() {
        let dir = Dir::new("instance-running");
        let path = dir.0.join("paddock.pid");
        let file = std::fs::File::create(&path).unwrap();
        let (_, holder) = lock_left_in_a_grandchild(&path);

        // The holder runs: as a paddock that runs, it is asked about.
        let waited = left_behind(&file, holder.0).unwrap();

        assert!(!waited);
    }

    #[test]
    fn an_ending_paddock_lets_go_of_a_lock_a_child_still_shares() {
        let dir = Dir::new("instance-drop");
        let path = dir.0.join("paddock.pid");
        let lock = std::fs::File::create(&path).unwrap();
        assert!(try_lock(&lock).unwrap());
        let max = max_fd();
        let fd = lock.as_raw_fd();
        // SAFETY: the child calls only async-signal-safe functions.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork");
        if child == 0 {
            // SAFETY: async-signal-safe calls only.
            unsafe {
                close_all_but(fd, max);
                libc::pause();
                libc::_exit(0);
            }
        }
        let pid = child;
        let child = Holder(child);

        drop(Instance { lock });

        let next = std::fs::File::open(&path).unwrap();
        let free = try_lock(&next).unwrap();
        drop(child);
        let mut status = 0;
        // SAFETY: `pid` is our child, killed just now; `status` is valid.
        unsafe { libc::waitpid(pid, &mut status, 0) };
        assert!(free, "free while the child still shares it");
    }

    #[test]
    fn parse_pid_accepts_only_real_process_ids() {
        assert_eq!(parse_pid("4242\n"), Some(4242));
        for bad in ["", "0", "1", "-1", "-4242", "abc"] {
            assert_eq!(parse_pid(bad), None, "{bad:?}");
        }
    }
}
