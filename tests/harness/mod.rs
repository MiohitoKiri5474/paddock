//! App-level test harness: runs the real `paddock` binary in a pseudo-terminal
//! against its own tmux socket, state, config and home directories, and reads
//! what it draws with a VT parser.
//!
//! Isolation: every [`Sandbox`] has its own `TMUX_TMPDIR`, so even a bug that
//! ignored `PADDOCK_TMUX_SOCKET` could not reach the user's tmux server. The
//! sandbox kills its tmux server when dropped.
//!
//! Tests ask paddock what runs where (`paddock debug state`, see
//! [`State`]), so they hold for any backend; only tests of tmux-only
//! behaviour call tmux, through [`tmux_only`]. Their names start with
//! `tmux_only_`: `cargo test -- --skip tmux_only` leaves them out.

#![allow(dead_code)] // Each test binary uses a different part of the harness.

mod panes;
mod state;
pub mod tmux_only;

#[allow(unused_imports)] // Each test binary uses a different part of them.
pub use panes::{cols, in_region, split_sizes, wait_for_split_sizes, wait_in_region};
#[allow(unused_imports)]
pub use state::{Space, Split, State, Tab};

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
/// Columns the sidebar takes while shown, its border included.
pub const SIDEBAR: u16 = 32;
/// Backgrounds of a tab and of the current tab in the tab bar (paddock's
/// defaults).
const TAB: vt100::Color = vt100::Color::Idx(237);
const CURRENT_TAB: vt100::Color = vt100::Color::Idx(4);
/// Background of the current space and the current agent in the sidebar
/// (paddock's default).
pub const CURRENT_ITEM: vt100::Color = vt100::Color::Idx(236);
/// The sidebar's title, in its first row.
const SIDEBAR_TITLE: &str = " SPACES";
/// Every prompt popup title, without its padding, and the finder's.
/// Copied from the source, `close_title` and `prompt` in
/// `src/app/view.rs`, `NameTarget::title` in `src/app/state.rs` and
/// `TITLE` in `src/app/finder.rs`: change them together.
const POPUP_TITLES: &[&str] = &[
    "Rename tab",
    "Rename space",
    "New space",
    "Close tab",
    "Close pane",
    "Close space",
    "New worktree",
    "Open worktree",
    "Delete worktree",
    "Finder",
];
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
        for dir in ["home", "state", "config", "tmux", "bin"] {
            std::fs::create_dir_all(root.path().join(dir)).expect("mkdir");
        }
        // A plain, predictable shell: no user rc files, a fixed prompt, and
        // the stub `claude` first on PATH even after macOS's path_helper.
        let rc = format!(
            "PS1='{PROMPT} '\nunsetopt PROMPT_SP\nunset HISTFILE\nexport PATH='{}/bin':$PATH\n",
            root.path().display()
        );
        std::fs::write(root.path().join("home/.zshrc"), rc).expect("zshrc");
        write_claude_stub(root.path());
        write_copy_stub(root.path());
        write_osascript_stub(root.path());
        write_gh_stub(root.path());
        Self { root }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn tmux_tmpdir(&self) -> PathBuf {
        self.path("tmux")
    }

    /// Names of the socket files anywhere in the sandbox: a backend that
    /// ran left its socket there.
    pub fn sockets(&self) -> Vec<String> {
        use std::os::unix::fs::FileTypeExt;
        let mut found = Vec::new();
        let mut dirs = vec![self.root.path().to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    dirs.push(entry.path());
                } else if kind.is_socket() {
                    found.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        found
    }

    /// The sandbox's variables a process in paddock's terminals has from
    /// the backend's clean environment: its state directory and the
    /// backend's socket.
    pub fn backend_env(&self) -> Vec<(String, String)> {
        let kept = ["XDG_STATE_HOME", "PADDOCK_TMUX_SOCKET", "TMUX_TMPDIR"];
        let env = self.env().into_iter();
        env.filter(|(k, _)| kept.contains(&k.as_str())).collect()
    }

    /// Variables paddock runs with, including ones that must not leak.
    fn env(&self) -> Vec<(String, String)> {
        let p = |n: &str| self.path(n).to_string_lossy().into_owned();
        vec![
            ("HOME".into(), p("home")),
            ("PATH".into(), self.path_var()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("LANG".into(), "en_US.UTF-8".into()),
            ("TERM".into(), "xterm-256color".into()),
            ("TMUX_TMPDIR".into(), p("tmux")),
            ("PADDOCK_TMUX_SOCKET".into(), SOCKET.into()),
            ("XDG_STATE_HOME".into(), p("state")),
            ("XDG_CONFIG_HOME".into(), p("config")),
            // Never the real clipboard: copies land in the sandbox's `clipboard` file.
            ("PADDOCK_COPY_COMMAND".into(), p("bin/copy")),
            ("SUPACODE_TEST_LEAK".into(), "leaked".into()),
            ("TERM_PROGRAM".into(), "leaky-terminal".into()),
            // As if launched from inside someone's tmux pane.
            ("TMUX".into(), "/tmp/nope,1,0".into()),
            ("PADDOCK_OSASCRIPT".into(), p("bin/osascript")),
            // Never the real gh: the stub answers from the sandbox (see
            // `set_gh`); without `bin/gh`, as if gh were not installed.
            ("PADDOCK_GH".into(), p("bin/gh")),
            // Set in the user's shell, it must not reach gh (see
            // `gh_repo_seen`).
            ("GH_REPO".into(), "someone/else".into()),
        ]
    }

    /// The `GH_REPO` each call of the stub `gh` saw, `<unset>` for none.
    pub fn gh_repo_seen(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("gh-env.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Make the stub's `gh pr view` take 3 s from now on.
    pub fn slow_gh_view(&self) {
        std::fs::write(self.path("gh.view-slow"), "").expect("slow gh view");
    }

    /// What the stub `gh pr list` prints from now on; until set, it fails
    /// as a logged-out gh does.
    pub fn set_gh(&self, json: &str) {
        let tmp = self.path("gh.json.tmp");
        std::fs::write(&tmp, json).expect("write gh");
        std::fs::rename(tmp, self.path("gh.json")).expect("rename gh");
    }

    /// Make the stub `gh` hang (30 s) from now on.
    pub fn slow_gh(&self) {
        std::fs::write(self.path("gh.slow"), "").expect("slow gh");
    }

    /// Remove the stub `gh`: as if gh were not installed.
    pub fn remove_gh(&self) {
        std::fs::remove_file(self.path("bin/gh")).expect("remove gh");
    }

    /// Every call of the stub `gh` so far, one `<directory> <arguments>`
    /// line each.
    pub fn gh_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("gh-calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Wait until the stub `gh` was called at least `n` times; returns the
    /// calls.
    pub fn wait_for_gh_calls(&self, n: usize) -> Vec<String> {
        let start = Instant::now();
        loop {
            let calls = self.gh_calls();
            if calls.len() >= n {
                return calls;
            }
            assert!(
                start.elapsed() < TIMEOUT,
                "gh called {} times, not {n}: {calls:?}",
                calls.len()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Run tmux against the sandbox's paddock server: only to stop it, and
    /// for tmux-only tests (see [`tmux_only`]).
    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .env("TMUX_TMPDIR", self.tmux_tmpdir())
            .env_remove("TMUX")
            .args(["-L", SOCKET])
            .args(args)
            .output()
            .expect("run tmux");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Write paddock's `config.toml`.
    pub fn write_config(&self, text: &str) {
        let dir = self.path("config/paddock");
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::write(dir.join("config.toml"), text).expect("write config");
    }

    /// The backend's state, as `paddock debug state --json` prints it.
    pub fn state(&self) -> State {
        let out = self.run(&["debug", "state", "--json"], &[], "");
        assert!(out.status.success(), "debug state failed: {out:?}");
        State::parse(&String::from_utf8_lossy(&out.stdout))
    }

    /// The terminal `target` names (see [`State::terminal`]).
    pub fn terminal(&self, target: &str) -> Split {
        let state = self.state();
        let terminal = state.terminal(target).cloned();
        terminal.unwrap_or_else(|| panic!("no terminal {target:?} in {state:#?}"))
    }

    /// The space named, or with the id, `space`.
    pub fn space(&self, space: &str) -> Space {
        let state = self.state();
        let found = state.space(space).cloned();
        found.unwrap_or_else(|| panic!("no space {space:?} in {state:#?}"))
    }

    /// Every terminal, as `<id> <shell pid>`: the same while the same
    /// shells run.
    pub fn shells(&self) -> Vec<String> {
        let state = self.state();
        let terminals = state.terminals();
        terminals
            .map(|t| format!("{} {}", t.id, t.pid.unwrap_or_default()))
            .collect()
    }

    /// Tabs and floats, in every space.
    pub fn tabs_and_floats(&self) -> usize {
        let spaces = self.state().spaces;
        spaces.iter().map(|s| s.tabs.len() + s.floats.len()).sum()
    }

    /// End the terminal `target` names from outside paddock, as a crash
    /// would: its shell is killed.
    pub fn end_terminal(&self, target: &str) {
        let terminal = self.terminal(target);
        let pid = terminal.pid.expect("the terminal's shell pid");
        let status = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .expect("run kill");
        assert!(status.success(), "kill {pid}: {status}");
    }

    /// Space names, oldest first.
    pub fn space_names(&self) -> Vec<String> {
        self.state().names()
    }

    pub fn launch(&self) -> Paddock {
        Paddock::launch(self)
    }

    /// `PATH` with the sandbox's `bin` (holding the stub `claude`) first.
    fn path_var(&self) -> String {
        format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    /// What the stub `claude agents --json` prints from now on.
    pub fn set_agents(&self, json: &str) {
        let tmp = self.path("agents.json.tmp");
        std::fs::write(&tmp, json).expect("write agents");
        std::fs::rename(tmp, self.path("agents.json")).expect("rename agents");
    }

    /// Calls of the stub `claude` other than `agents --json`, one
    /// `<directory> <arguments>` line each; waits for at least one.
    pub fn claude_calls(&self) -> Vec<String> {
        let start = Instant::now();
        loop {
            let log = std::fs::read_to_string(self.path("claude.log")).unwrap_or_default();
            if !log.is_empty() {
                return log.lines().map(str::to_string).collect();
            }
            assert!(start.elapsed() < TIMEOUT, "claude was never run");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// What paddock last copied, if anything.
    pub fn clipboard(&self) -> Option<String> {
        std::fs::read_to_string(self.path("clipboard")).ok()
    }

    /// Wait until paddock has copied exactly `text`.
    pub fn wait_for_clipboard(&self, text: &str) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if self.clipboard().as_deref() == Some(text) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(self.clipboard().as_deref(), Some(text), "clipboard");
    }

    /// Messages of the notifications shown so far, oldest first.
    pub fn notifications(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("notifications.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Wait until `n` notifications were shown, then a little longer so a
    /// late extra one is caught too, and return them all.
    pub fn wait_for_notifications(&self, n: usize) -> Vec<String> {
        let start = Instant::now();
        while self.notifications().len() < n {
            assert!(
                start.elapsed() < TIMEOUT,
                "notifications: {:?}",
                self.notifications()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        self.notifications()
    }

    /// Run the paddock binary with these arguments, environment and stdin,
    /// outside any terminal (as Claude Code runs a hook).
    pub fn run(&self, args: &[&str], env: &[(&str, &str)], stdin: &str) -> std::process::Output {
        self.run_in(&self.path("home"), args, env, stdin)
    }

    /// [`Self::run`] in directory `dir`. Never the test's own directory:
    /// that is paddock's repository, which a `worktree` command would change.
    pub fn run_in(
        &self,
        dir: &std::path::Path,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: &str,
    ) -> std::process::Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_paddock"));
        cmd.env_clear()
            .envs(self.env().into_iter().filter(|(k, _)| k != "TMUX"))
            // Only the sandbox's git configuration (its home has none).
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(dir)
            .envs(env.iter().copied())
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().expect("spawn paddock");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait paddock")
    }

    /// `paddock hook <event>` with `payload` on stdin, as Claude Code would
    /// run it inside the terminal `terminal` names (see [`State::terminal`]).
    pub fn hook(&self, terminal: &str, event: &str, payload: &str) -> std::process::Output {
        let out = self.hook_args(terminal, &[event], payload);
        assert!(out.stdout.is_empty(), "a hook prints nothing: {out:?}");
        out
    }

    /// `paddock hook --agent <agent> <event>` with `payload` on stdin, as
    /// that agent's hooks run it inside the terminal `terminal` names. It
    /// prints nothing, but `{}` for Antigravity (`agy`), which reads a
    /// reply.
    pub fn agent_hook(
        &self,
        terminal: &str,
        agent: &str,
        event: &str,
        payload: &str,
    ) -> std::process::Output {
        let out = self.hook_args(terminal, &["--agent", agent, event], payload);
        let reply: &[u8] = if agent == "agy" { b"{}" } else { b"" };
        assert_eq!(out.stdout, reply, "{agent}'s reply: {out:?}");
        out
    }

    /// `paddock hook <args>` inside the terminal `terminal` names, with the
    /// variables the backend says a process there sees.
    fn hook_args(&self, terminal: &str, args: &[&str], payload: &str) -> std::process::Output {
        let terminal = self.terminal(terminal);
        let args: Vec<&str> = std::iter::once("hook")
            .chain(args.iter().copied())
            .collect();
        let out = self.run(&args, &terminal.env(), payload);
        assert!(out.status.success(), "hook failed: {out:?}");
        out
    }
}

/// A `claude` that answers `agents --json` from `agents.json` in the sandbox
/// (`[]` until a test sets it), so tests never see real sessions. Any other
/// call only appends `<directory> <arguments>` to `claude.log`.
fn write_claude_stub(root: &std::path::Path) {
    let dir = root.display();
    let script = format!(
        "#!/bin/sh\nif [ \"$1 $2\" != \"agents --json\" ]; then\n  echo \"$(pwd -P) $*\" >> '{dir}/claude.log'\n  exit 0\nfi\ncat '{dir}/agents.json' 2>/dev/null || echo '[]'\n"
    );
    let path = root.join("bin/claude");
    std::fs::write(&path, script).expect("claude stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// A copy command that writes what it is given to `clipboard` in the sandbox.
fn write_copy_stub(root: &std::path::Path) {
    let dir = root.display();
    let script = format!(
        "#!/bin/sh\ncat > '{dir}/clipboard.tmp' && mv '{dir}/clipboard.tmp' '{dir}/clipboard'\n"
    );
    let path = root.join("bin/copy");
    std::fs::write(&path, script).expect("copy stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// An `osascript` that appends each notification's message (its last
/// argument) to `notifications.log` in the sandbox, so tests never show real
/// notifications. paddock is pointed at it by path and finds it first on
/// `PATH` too.
fn write_osascript_stub(root: &std::path::Path) {
    let script = format!(
        "#!/bin/sh\nfor a in \"$@\"; do last=\"$a\"; done\nprintf '%s\\n' \"$last\" >> '{}/notifications.log'\n",
        root.display()
    );
    let path = root.join("bin/osascript");
    std::fs::write(&path, script).expect("osascript stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

/// A `gh` that appends `<directory> <arguments>` to `gh-calls.log` in the
/// sandbox, then: hangs if `gh.slow` exists; for `pr view` exits 0 (as if
/// a browser opened); else prints `gh.json`, or fails as a logged-out gh
/// does without it.
fn write_gh_stub(root: &std::path::Path) {
    let dir = root.display();
    let script = format!(
        "#!/bin/sh\n\
         printf '%s %s\\n' \"$(pwd -P)\" \"$*\" >> '{dir}/gh-calls.log'\n\
         printf '%s\\n' \"${{GH_REPO-<unset>}}\" >> '{dir}/gh-env.log'\n\
         [ -f '{dir}/gh.slow' ] && exec sleep 30\n\
         [ \"$1 $2\" = 'pr view' ] && [ -f '{dir}/gh.view-slow' ] && exec sleep 3\n\
         [ \"$1 $2\" = 'pr view' ] && exit 0\n\
         [ -f '{dir}/gh.json' ] && exec cat '{dir}/gh.json'\n\
         echo 'To get started with GitHub CLI, please run:  gh auth login' >&2\n\
         exit 4\n"
    );
    let path = root.join("bin/gh");
    std::fs::write(&path, script).expect("gh stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
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

    /// A wheel notch at (col, row), 0-based, as an SGR mouse report. Like
    /// every mouse event here, `col` counts from the right of the sidebar,
    /// as [`Self::rows`] does.
    pub fn wheel(&mut self, up: bool, col: u16, row: u16) {
        let button = if up { 64 } else { 65 };
        let col = col + self.left();
        self.send(format!("\x1b[<{button};{};{}M", col + 1, row + 1).as_bytes());
    }

    /// A left-button mouse event at (col, row), 0-based, as an SGR report:
    /// `code` 0 is a press, 32 a drag; `release` ends the press. Shift adds 4.
    pub fn mouse(&mut self, code: u16, col: u16, row: u16, release: bool) {
        let col = col + self.left();
        self.send_mouse(code, col, row, release);
    }

    /// A left click at screen column `col` (counting from the screen's left
    /// edge, so it can hit the sidebar) and row `row`, 0-based.
    pub fn click_screen(&mut self, col: u16, row: u16) {
        self.send_mouse(0, col, row, false);
        self.send_mouse(0, col, row, true);
    }

    /// Press at screen column `from` of `row`, drag to column `to` and
    /// release there, counting from the screen's left edge.
    pub fn drag_screen(&mut self, row: u16, from: u16, to: u16) {
        self.send_mouse(0, from, row, false);
        self.send_mouse(32, to, row, false);
        self.send_mouse(0, to, row, true);
    }

    fn send_mouse(&mut self, code: u16, col: u16, row: u16, release: bool) {
        let fin = if release { 'm' } else { 'M' };
        self.send(format!("\x1b[<{code};{};{}{fin}", col + 1, row + 1).as_bytes());
    }

    /// A left click at (col, row), 0-based.
    pub fn click(&mut self, col: u16, row: u16) {
        self.mouse(0, col, row, false);
        self.mouse(0, col, row, true);
    }

    /// Press at `from`, drag to `to` and release there, as (col, row) 0-based.
    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16), shift: bool) {
        let mods = if shift { 4 } else { 0 };
        self.mouse(mods, from.0, from.1, false);
        self.mouse(32 + mods, to.0, to.1, false);
        self.mouse(mods, to.0, to.1, true);
    }

    /// Where `text` is first drawn, as (col, row) 0-based, right of the sidebar.
    pub fn find(&self, text: &str) -> Option<(u16, u16)> {
        self.rows().iter().enumerate().find_map(|(y, r)| {
            let x = r.find(text)?;
            Some((
                u16::try_from(r[..x].chars().count()).ok()?,
                u16::try_from(y).ok()?,
            ))
        })
    }

    /// The first screen column right of the sidebar (0 while it is hidden).
    pub fn left(&self) -> u16 {
        let first = self.full_rows().into_iter().next().unwrap_or_default();
        let border = first.chars().nth(usize::from(SIDEBAR - 1));
        if first.starts_with(SIDEBAR_TITLE) && border == Some('│') {
            SIDEBAR
        } else {
            0
        }
    }

    /// The sidebar's rows, without its border, trimmed at the end; empty
    /// while it is hidden.
    pub fn sidebar(&self) -> Vec<String> {
        if self.left() == 0 {
            return Vec::new();
        }
        let parser = self.screen.lock().expect("screen lock");
        parser
            .screen()
            .rows(0, SIDEBAR - 1)
            .map(|r| r.trim_end().to_string())
            .collect()
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

    /// The prefix (`C-a`) followed by a command key.
    pub fn command(&mut self, key: &[u8]) {
        self.send(b"\x01");
        self.send(key);
    }

    /// The tab bar row.
    pub fn tab_bar(&self) -> String {
        self.rows().into_iter().next().unwrap_or_default()
    }

    /// The tabs in the tab bar, numbered as the keys `1`-`9` reach them
    /// (`1:home`), and which one is current. Tabs are the filled blocks of
    /// the bar: cells with a background of their own (the default tab and
    /// current tab colours).
    pub fn tabs(&self) -> (Vec<String>, Option<usize>) {
        let left = self.left();
        let parser = self.screen.lock().expect("screen lock");
        let screen = parser.screen();
        let width = screen.size().1.get();
        let mut tabs: Vec<(String, bool)> = Vec::new();
        let mut open = false;
        for x in left..width {
            let Some(cell) = screen.cell(0, x) else { break };
            let bg = cell.bgcolor();
            let current = bg == CURRENT_TAB;
            if bg != TAB && !current {
                open = false;
                continue;
            }
            if !open {
                tabs.push((String::new(), current));
                open = true;
            }
            if let Some(tab) = tabs.last_mut() {
                tab.0.push_str(if cell.contents().is_empty() {
                    " "
                } else {
                    cell.contents()
                });
            }
        }
        let current = tabs.iter().position(|(_, c)| *c);
        let labels = tabs
            .iter()
            .enumerate()
            .map(|(i, (text, _))| format!("{}:{}", i + 1, text.trim()))
            .collect();
        (labels, current)
    }

    /// The current tab's label (e.g. `2:home`), or empty.
    pub fn current_tab(&self) -> String {
        let (labels, current) = self.tabs();
        current.map(|i| labels[i].clone()).unwrap_or_default()
    }

    /// Wait until the tab bar shows exactly these tabs (e.g. `1:home`).
    pub fn wait_for_tabs(&self, labels: &[&str]) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if self.tabs().0 == labels {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "tabs are {:?}, not {labels:?}; screen:\n{}",
            self.tabs().0,
            self.full_screen()
        );
    }

    pub fn wait_for_current_tab(&self, label: &str) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if self.current_tab() == label {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "current tab is {:?}, not {label:?}; screen:\n{}",
            self.current_tab(),
            self.screen()
        );
    }

    /// Wait until the prompt popup shows `title` in its border and `text`
    /// at the start of its line.
    pub fn wait_for_popup(&self, title: &str, text: &str) {
        let (title, line) = (format!(" {title} "), format!("│ {text}"));
        self.wait_for(&format!("popup {title:?} with {text:?}"), |_| {
            let s = self.full_screen();
            s.contains(&title) && s.contains(&line)
        });
    }

    /// Wait until the prompt popup is gone: no popup title is drawn (the
    /// dashboard preview has a rounded box too, so corners don't tell).
    pub fn wait_for_no_popup(&self) {
        self.wait_for("the popup gone", |_| {
            let s = self.full_screen();
            !POPUP_TITLES.iter().any(|t| s.contains(&format!(" {t} ")))
        });
    }

    /// `C-a d`
    pub fn detach_keys(&mut self) {
        self.send(b"\x01d");
    }

    /// What is drawn right of the sidebar: the tab bar, then the panes.
    pub fn screen(&self) -> String {
        self.rows().join("\n")
    }

    /// The whole screen, sidebar included.
    pub fn full_screen(&self) -> String {
        self.screen.lock().expect("screen lock").screen().contents()
    }

    /// Whether the cell at screen (row, col), 0-based, is drawn reversed.
    pub fn reversed(&self, row: u16, col: u16) -> bool {
        let parser = self.screen.lock().expect("screen lock");
        parser.screen().cell(row, col).is_some_and(|c| c.inverse())
    }

    /// Whether the cell at screen (row, col), 0-based, is drawn dim.
    pub fn dim(&self, row: u16, col: u16) -> bool {
        let parser = self.screen.lock().expect("screen lock");
        parser.screen().cell(row, col).is_some_and(|c| c.dim())
    }

    /// Foreground and background colour of the cell at (row, col), 0-based.
    pub fn colors(&self, row: u16, col: u16) -> (vt100::Color, vt100::Color) {
        let parser = self.screen.lock().expect("screen lock");
        let cell = parser.screen().cell(row, col).expect("cell on screen");
        (cell.fgcolor(), cell.bgcolor())
    }

    /// Rows right of the sidebar (column 0 is the session area's left
    /// edge), trimmed at the end.
    pub fn rows(&self) -> Vec<String> {
        let left = self.left();
        let parser = self.screen.lock().expect("screen lock");
        let width = parser.screen().size().1.get();
        parser
            .screen()
            .rows(left, width.saturating_sub(left))
            .map(|r| r.trim_end().to_string())
            .collect()
    }

    /// Whole rows, sidebar included, trimmed at the end.
    pub fn full_rows(&self) -> Vec<String> {
        let parser = self.screen.lock().expect("screen lock");
        let width = parser.screen().size().1.get();
        parser
            .screen()
            .rows(0, width)
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
        panic!(
            "timed out waiting for {what}; screen:\n{}",
            self.full_screen()
        );
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
