//! App-level test harness: runs the real `paddock` binary in a pseudo-terminal
//! against its own tmux socket, state, config and home directories, and reads
//! what it draws with a VT parser.
//!
//! Isolation: every [`Sandbox`] has its own `TMUX_TMPDIR`, so even a bug that
//! ignored `PADDOCK_TMUX_SOCKET` could not reach the user's tmux server, and
//! its own `XDG_STATE_HOME`, which holds the native server's socket and lock.
//! The sandbox ends its tmux server and its native server when dropped.
//!
//! Tests ask paddock what runs where (`paddock debug state`, see
//! [`State`]), so they hold for any backend; only tests of tmux-only
//! behaviour call tmux, through [`tmux_only`]. Their names start with
//! `tmux_only_`: `cargo test -- --skip tmux_only` leaves them out.
//! [`backend`] picks the backend a run is on; [`native_skip`] lists the
//! tests the native backend cannot pass yet.

#![allow(dead_code)] // Each test binary uses a different part of the harness.

pub mod alerts;
pub mod backend;
pub mod native_skip;
pub mod older_server;
mod paddock;
mod panes;
pub mod stall;
mod state;
pub mod tmux_only;

pub use paddock::Paddock;
#[allow(unused_imports)] // Each test binary uses a different part of them.
pub use panes::{cols, in_region, split_sizes, wait_for_split_sizes, wait_in_region};
#[allow(unused_imports)]
pub use state::{Space, Split, State, Tab};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize, SlavePty};
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
    /// Variables a test set (`Some`) or removed (`None`) for the next launch.
    env_changes: Mutex<Vec<(String, Option<String>)>>,
}

impl Sandbox {
    pub fn new() -> Self {
        native_skip::check();
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
        // Debian and Ubuntu's /etc/zsh/zshrc runs compinit, which asks
        // about the sandbox's directories before the first prompt.
        std::fs::write(root.path().join("home/.zshenv"), "skip_global_compinit=1\n")
            .expect("zshenv");
        write_claude_stub(root.path());
        write_copy_stub(root.path());
        write_osascript_stub(root.path());
        write_desktop_stubs(root.path());
        write_gh_stub(root.path());
        let sandbox = Self {
            root,
            env_changes: Mutex::default(),
        };
        sandbox.write_config("");
        sandbox
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// A copy of paddock's binary at `name` in the sandbox, to be run.
    /// Copied by `cp`, not by this process: a program another test
    /// starts meanwhile inherits whatever this process holds open until
    /// it runs, and Linux will not run a file open for writing ("Text
    /// file busy").
    pub fn copy_paddock(&self, name: &str) -> PathBuf {
        let to = self.path(name);
        let status = Command::new("cp")
            .arg(env!("CARGO_BIN_EXE_paddock"))
            .arg(&to)
            .status();
        assert!(status.is_ok_and(|s| s.success()), "cp paddock to {name}");
        to
    }

    /// paddock's state directory (`$XDG_STATE_HOME/paddock`), where the
    /// native server keeps its socket and lock.
    pub fn state_dir(&self) -> PathBuf {
        self.path("state/paddock")
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
    /// the backend's clean environment: its state directory and where the
    /// backend listens: tmux's socket (`PADDOCK_TMUX_SOCKET`,
    /// `TMUX_TMPDIR`), and the state directory, which holds the native
    /// server's socket. The native backend's own variables (ticket 43)
    /// join them here.
    pub fn backend_env(&self) -> Vec<(String, String)> {
        let kept = ["XDG_STATE_HOME", "PADDOCK_TMUX_SOCKET", "TMUX_TMPDIR"];
        let env = self.env().into_iter();
        env.filter(|(k, _)| kept.contains(&k.as_str())).collect()
    }

    /// Set (`Some`) or remove (`None`) variable `name` for paddock and
    /// its hooks from now on.
    pub fn set_env(&self, name: &str, value: Option<&str>) {
        self.env_changes
            .lock()
            .unwrap()
            .push((name.into(), value.map(Into::into)));
    }

    /// Variables paddock runs with, including ones that must not leak,
    /// with the test's changes (see [`Sandbox::set_env`]).
    fn env(&self) -> Vec<(String, String)> {
        let mut env = self.base_env();
        for (name, value) in self.env_changes.lock().unwrap().iter() {
            env.retain(|(k, _)| k != name);
            if let Some(value) = value {
                env.push((name.clone(), value.clone()));
            }
        }
        env
    }

    fn base_env(&self) -> Vec<(String, String)> {
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

    /// Run tmux against the sandbox's paddock server, for tmux-only tests
    /// (see [`tmux_only`]); it fails the test if tmux fails.
    fn tmux(&self, args: &[&str]) -> String {
        let out = self.tmux_tolerant(args);
        assert!(
            out.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// [`Self::tmux`] that may fail (as `kill-server` with no server
    /// running): its whole output.
    fn tmux_tolerant(&self, args: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .env("TMUX_TMPDIR", self.tmux_tmpdir())
            .env_remove("TMUX")
            .args(["-L", SOCKET])
            .args(args)
            .output()
            .expect("run tmux")
    }

    /// Start the sandbox's tmux server as an older paddock left it
    /// running. Like every tmux call here, it is on the sandbox's socket,
    /// never the user's.
    pub fn start_old_tmux_server(&self) {
        self.tmux(&["-f", "/dev/null", "new-session", "-d", "sleep 600"]);
    }

    /// End the server [`Self::start_old_tmux_server`] started.
    pub fn kill_old_tmux_server(&self) {
        self.tmux(&["kill-server"]);
    }

    /// Write paddock's `config.toml`, after the `[backend]` table the
    /// run picks (see [`backend`]) unless `text` has its own.
    pub fn write_config(&self, text: &str) {
        let dir = self.path("config/paddock");
        std::fs::create_dir_all(&dir).expect("config dir");
        let text = format!("{}{text}", backend::config(text));
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

    /// The platform's notifier in the sandbox's `bin`, first on `PATH`.
    pub fn notifier_stub(&self) -> PathBuf {
        let name = if cfg!(target_os = "macos") {
            "osascript"
        } else {
            "notify-send"
        };
        self.path("bin").join(name)
    }

    /// Calls of the stub `notify-send`, `wl-copy` and `xclip`, one
    /// `<program> <arguments>` line each, oldest first.
    pub fn desktop_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("desktop-calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim_end().to_string())
            .collect()
    }

    /// Take the stubs `names` out of the sandbox's `bin` and hide the real
    /// ones (see [`Sandbox::hide_from_path`]), so paddock finds none.
    pub fn remove_tools(&self, names: &[&str]) {
        for name in names {
            std::fs::remove_file(self.path("bin").join(name)).expect("remove stub");
        }
        self.hide_from_path(names);
    }

    /// Set paddock's `PATH` to the sandbox's `bin`, then the real `PATH`
    /// with each directory holding one of `names` shadowed: replaced by a
    /// sandbox directory of links to all it holds but those, so the rest
    /// (`cat`, `zsh`, ...) still resolves. Returns the new `PATH`.
    pub fn hide_from_path(&self, names: &[&str]) -> String {
        let real = std::env::var("PATH").unwrap_or_default();
        let mut path = vec![self.path("bin").to_string_lossy().into_owned()];
        for (i, dir) in real.split(':').filter(|d| !d.is_empty()).enumerate() {
            let dir = std::path::Path::new(dir);
            if !names.iter().any(|n| dir.join(n).exists()) {
                path.push(dir.to_string_lossy().into_owned());
                continue;
            }
            let shadow = self.path(&format!("shadow/{i}"));
            std::fs::create_dir_all(&shadow).expect("mkdir shadow");
            for entry in std::fs::read_dir(dir).expect("read PATH dir").flatten() {
                let name = entry.file_name();
                if !names.iter().any(|n| name == *n) {
                    std::os::unix::fs::symlink(entry.path(), shadow.join(&name))
                        .expect("link into shadow");
                }
            }
            path.push(shadow.to_string_lossy().into_owned());
        }
        let path = path.join(":");
        self.set_env("PATH", Some(&path));
        path
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
/// notifications. paddock finds it first on `PATH`.
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

/// The desktop tools paddock copies and notifies with on Linux, and macOS's
/// `pbcopy`, first on `PATH` so tests never reach the real ones: each
/// appends `<program> <arguments>` to `desktop-calls.log` in the sandbox.
/// `notify-send` also logs its message (its last argument) to
/// `notifications.log`, as the `osascript` stub does; the copiers write
/// what they read to `clipboard`, as the copy stub does.
fn write_desktop_stubs(root: &std::path::Path) {
    let dir = root.display();
    let log = format!("echo \"$(basename \"$0\") $*\" >> '{dir}/desktop-calls.log'\n");
    let notify = format!(
        "#!/bin/sh\n{log}for a in \"$@\"; do last=\"$a\"; done\nprintf '%s\\n' \"$last\" >> '{dir}/notifications.log'\n"
    );
    let copy = format!(
        "#!/bin/sh\n{log}cat > '{dir}/clipboard.tmp' && mv '{dir}/clipboard.tmp' '{dir}/clipboard'\n"
    );
    use std::os::unix::fs::PermissionsExt;
    let stubs = [
        ("notify-send", &notify),
        ("wl-copy", &copy),
        ("xclip", &copy),
        ("pbcopy", &copy),
    ];
    for (name, script) in stubs {
        let path = root.join("bin").join(name);
        std::fs::write(&path, script).expect("desktop stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub");
    }
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
        self.tmux_tolerant(&["kill-server"]);
        backend::stop_server(&self.state_dir());
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
