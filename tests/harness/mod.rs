//! App-level test harness: runs the real `paddock` binary in a pseudo-terminal
//! against its own tmux socket, state, config and home directories, and reads
//! what it draws with a VT parser.
//!
//! Isolation: every [`Sandbox`] has its own `TMUX_TMPDIR`, so even a bug that
//! ignored `PADDOCK_TMUX_SOCKET` could not reach the user's tmux server. The
//! sandbox kills its tmux server when dropped.

#![allow(dead_code)] // Each test binary uses a different part of the harness.

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub const COLS: u16 = 100;
pub const ROWS: u16 = 30;
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The prompt of the test shell, as drawn (the `.zshrc` adds a trailing space).
pub const PROMPT: &str = "test$";
const SOCKET: &str = "paddock-test";
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

/// Directories and environment one paddock under test lives in.
pub struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    pub fn new() -> Self {
        // Short base path: unix socket paths are limited to about 100 bytes on macOS.
        let root = tempfile::Builder::new()
            .prefix("pdk")
            .tempdir_in("/tmp")
            .expect("tempdir");
        for dir in ["home", "state", "config", "tmux"] {
            std::fs::create_dir_all(root.path().join(dir)).expect("mkdir");
        }
        // A plain, predictable shell: no user rc files, a fixed prompt.
        let rc = format!("PS1='{PROMPT} '\nunsetopt PROMPT_SP\nunset HISTFILE\n");
        std::fs::write(root.path().join("home/.zshrc"), rc).expect("zshrc");
        Self { root }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    pub fn tmux_tmpdir(&self) -> PathBuf {
        self.path("tmux")
    }

    /// Variables paddock runs with, including ones that must not leak.
    fn env(&self) -> Vec<(String, String)> {
        let p = |n: &str| self.path(n).to_string_lossy().into_owned();
        vec![
            ("HOME".into(), p("home")),
            ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("LANG".into(), "en_US.UTF-8".into()),
            ("TERM".into(), "xterm-256color".into()),
            ("TMUX_TMPDIR".into(), p("tmux")),
            ("PADDOCK_TMUX_SOCKET".into(), SOCKET.into()),
            ("XDG_STATE_HOME".into(), p("state")),
            ("XDG_CONFIG_HOME".into(), p("config")),
            ("SUPACODE_TEST_LEAK".into(), "leaked".into()),
            ("TERM_PROGRAM".into(), "leaky-terminal".into()),
            // As if launched from inside someone's tmux pane.
            ("TMUX".into(), "/tmp/nope,1,0".into()),
        ]
    }

    /// Run tmux against the sandbox's paddock server.
    pub fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .env("TMUX_TMPDIR", self.tmux_tmpdir())
            .env_remove("TMUX")
            .args(["-L", SOCKET])
            .args(args)
            .output()
            .expect("run tmux");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Socket files tmux created in the sandbox.
    pub fn sockets(&self) -> Vec<String> {
        let Ok(dirs) = std::fs::read_dir(self.tmux_tmpdir()) else {
            return vec![];
        };
        dirs.flatten()
            .filter_map(|d| std::fs::read_dir(d.path()).ok())
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    pub fn launch(&self) -> Paddock {
        Paddock::launch(self)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.tmux(&["kill-server"]);
        // Shells may still be exiting and writing into the sandbox; retry
        // until the whole tree is gone (TempDir's own cleanup is then a no-op).
        let start = Instant::now();
        while std::fs::remove_dir_all(self.root.path()).is_err() && self.root.path().exists() {
            if start.elapsed() > TIMEOUT {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A running paddock in a pseudo-terminal.
pub struct Paddock {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
}

impl Paddock {
    fn launch(sandbox: &Sandbox) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_paddock"));
        cmd.env_clear();
        for (k, v) in sandbox.env() {
            cmd.env(k, v);
        }
        cmd.cwd(sandbox.path("home"));
        let child = pair.slave.spawn_command(cmd).expect("spawn paddock");
        drop(pair.slave);
        let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("writer")));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            nonzero(ROWS),
            nonzero(COLS),
            0,
        )));
        let reader = pair.master.try_clone_reader().expect("reader");
        spawn_reader(reader, Arc::clone(&screen), Arc::clone(&writer));
        let writer = Box::new(SharedWriter(writer));
        Self {
            child,
            writer,
            master: pair.master,
            screen,
        }
    }

    /// Bytes as an outer terminal would send them.
    pub fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().expect("flush pty");
    }

    pub fn type_line(&mut self, line: &str) {
        self.send(line.as_bytes());
        self.send(b"\r");
    }

    /// A wheel notch at (col, row), 0-based, as an SGR mouse report.
    pub fn wheel(&mut self, up: bool, col: u16, row: u16) {
        let button = if up { 64 } else { 65 };
        self.send(format!("\x1b[<{button};{};{}M", col + 1, row + 1).as_bytes());
    }

    /// Resize the outer terminal (sends SIGWINCH to paddock).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master.resize(size).expect("resize pty");
        self.screen
            .lock()
            .expect("screen lock")
            .screen_mut()
            .set_size(nonzero(rows), nonzero(cols));
    }

    /// `C-a d`
    pub fn detach_keys(&mut self) {
        self.send(b"\x01d");
    }

    pub fn screen(&self) -> String {
        self.screen.lock().expect("screen lock").screen().contents()
    }

    pub fn rows(&self) -> Vec<String> {
        let parser = self.screen.lock().expect("screen lock");
        parser
            .screen()
            .rows(0, COLS)
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    /// Wait until `pred` holds for the drawn screen; panic with the screen if not.
    pub fn wait_for(&self, what: &str, pred: impl Fn(&str) -> bool) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if pred(&self.screen()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}; screen:\n{}", self.screen());
    }

    pub fn wait_for_text(&self, text: &str) {
        self.wait_for(text, |s| s.contains(text));
    }

    /// Wait for paddock to exit and return whether it exited successfully.
    pub fn wait_exit(&mut self) -> bool {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status.success();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("paddock did not exit; screen:\n{}", self.screen());
    }
}

impl Drop for Paddock {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
    }
}

fn nonzero(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n).expect("non-zero size")
}

struct SharedWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("writer lock").write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().expect("writer lock").flush()
    }
}

/// Feed everything paddock draws into the VT parser, answering cursor
/// position queries the way a real terminal would.
fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 1 << 16];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut parser = screen.lock().expect("screen lock");
            parser.process(&buf[..n]);
            if buf[..n]
                .windows(CURSOR_QUERY.len())
                .any(|w| w == CURSOR_QUERY)
            {
                let (row, col) = parser.screen().cursor_position();
                let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                let _ = writer
                    .lock()
                    .expect("writer lock")
                    .write_all(reply.as_bytes());
            }
        }
    });
}
