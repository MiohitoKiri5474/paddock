//! The native server's PTYs: a shell started on a new one, its size, and
//! the program in its foreground (named and placed by [`super::proc`]).

use anyhow::{Context, Result};
use std::ffi::CStr;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// What runs on a new PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program<'a> {
    /// The user's shell as a login shell, as tmux starts a pane's (so
    /// macOS's `path_helper` runs).
    LoginShell,
    /// `shell -c <command>`, not a login shell, as tmux runs a window's
    /// command (a float's program).
    Command(&'a str),
}

/// A shell (or a float's program) on a PTY of its own.
pub struct Pty {
    /// The PTY's master side: the shell's output is read, and its input
    /// written, here.
    pub master: File,
    /// The path of the shell's tty.
    pub tty: String,
    pub child: Child,
}

/// Start `shell` as `program` says in `cwd`, or in `home` if `cwd` is
/// gone, on a new PTY of `(cols, rows)`, with exactly the environment
/// `env`. The server calls it holding its state lock (see `open`).
pub fn spawn(
    shell: &str,
    program: Program<'_>,
    cwd: &Path,
    home: &Path,
    size: (u16, u16),
    env: &[(String, String)],
) -> Result<Pty> {
    let (master, slave) = open(size)?;
    let tty = tty_name(slave.as_raw_fd()).unwrap_or_default();
    let name = Path::new(shell)
        .file_name()
        .map_or("sh".into(), |n| n.to_string_lossy().into_owned());
    let dir = if cwd.is_dir() { cwd } else { home };
    let mut child = Command::new(shell);
    match program {
        Program::Command(command) => child.arg0(name).arg("-c").arg(command),
        Program::LoginShell => child.arg0(format!("-{name}")),
    };
    child
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(dir)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // macOS's request is a u32, Linux's already the c_ulong ioctl takes.
    #[allow(clippy::useless_conversion)]
    let make_controlling: libc::c_ulong = libc::TIOCSCTTY.into();
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        child.pre_exec(move || {
            // A session of its own, with the PTY as its controlling tty.
            if libc::setsid() < 0 || libc::ioctl(0, make_controlling, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = child
        .spawn()
        .with_context(|| format!("cannot start {shell}"))?;
    Ok(Pty {
        master: File::from(master),
        tty,
        child,
    })
}

/// A new PTY of `(cols, rows)`: its master and slave, neither inherited by
/// other programs.
fn open((cols, rows): (u16, u16)) -> Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    let mut size = winsize(cols, rows);
    // SAFETY: openpty writes two descriptors; the name and termios are
    // optional and passed as null.
    let ok = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            // A raw pointer: macOS's openpty takes it mutable, Linux's not.
            std::ptr::from_mut(&mut size),
        )
    };
    if ok != 0 {
        return Err(std::io::Error::last_os_error()).context("cannot open a PTY");
    }
    // SAFETY: openpty succeeded, so both are open descriptors we own.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // Until this, a program started meanwhile would inherit both. Safe
    // only because the server starts programs (spawn) under its state
    // lock alone, which its caller holds.
    for fd in [&master, &slave] {
        // SAFETY: fcntl on a descriptor we own.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error()).context("cannot set up a PTY");
        }
    }
    Ok((master, slave))
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Tell the program on `master` its terminal is now `(cols, rows)`.
pub fn resize(master: &File, (cols, rows): (u16, u16)) -> Result<()> {
    let size = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ reads one winsize from the pointer.
    if unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) } < 0 {
        return Err(std::io::Error::last_os_error()).context("cannot resize a PTY");
    }
    Ok(())
}

fn tty_name(fd: RawFd) -> Option<String> {
    let mut buf = [0 as libc::c_char; 128];
    // SAFETY: ttyname_r writes at most `buf.len()` bytes, NUL included.
    if unsafe { libc::ttyname_r(fd, buf.as_mut_ptr(), buf.len()) } != 0 {
        return None;
    }
    // SAFETY: on success the buffer holds a NUL-terminated string.
    let name = unsafe { CStr::from_ptr(buf.as_ptr()) };
    Some(name.to_string_lossy().into_owned())
}

/// The process group in the foreground of the PTY `master`.
pub fn foreground(master: &File) -> Option<i32> {
    // SAFETY: tcgetpgrp only reads the descriptor.
    let group = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
    (group > 0).then_some(group)
}

pub use super::proc::{cwd, name};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_shell_runs_on_its_own_pty_where_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let env = [("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let mut pty = spawn(
            "/bin/sh",
            Program::LoginShell,
            dir.path(),
            dir.path(),
            (40, 10),
            &env,
        )
        .unwrap();
        // macOS names a PTY /dev/ttysNNN, Linux /dev/pts/N.
        assert!(
            pty.tty.starts_with("/dev/ttys") || pty.tty.starts_with("/dev/pts/"),
            "{}",
            pty.tty
        );
        let pid = i32::try_from(pty.child.id()).unwrap();
        // The shell leads its own session, in the PTY's foreground.
        let start = std::time::Instant::now();
        while foreground(&pty.master) != Some(pid) {
            assert!(start.elapsed().as_secs() < 5, "never in the foreground");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(name(pid).as_deref(), Some("sh"));
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(cwd(pid), Some(real.to_string_lossy().into_owned()));
        resize(&pty.master, (50, 12)).unwrap();
        use std::io::Write;
        pty.master
            .write_all(b"stty size; exit\n")
            .expect("write to the shell");
        // Read to the end: an exiting shell waits for its output to be read.
        let mut out = String::new();
        let mut buf = [0; 1024];
        while let Ok(n) = pty.master.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(out.contains("12 50"), "{out:?}");
        pty.child.wait().unwrap();
    }

    #[test]
    fn a_command_runs_in_a_shell_that_is_not_a_login_one_and_ends_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let env = [("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let command = Program::Command("echo \"name=$0\"; exit 3");
        let mut pty = spawn("/bin/sh", command, dir.path(), dir.path(), (40, 10), &env).unwrap();
        let mut out = String::new();
        let mut buf = [0; 1024];
        while let Ok(n) = pty.master.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(out.contains("name=sh\r\n"), "{out:?}");
        assert_eq!(pty.child.wait().unwrap().code(), Some(3));
    }
}
