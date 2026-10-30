//! The backend seam ([ADR 0007](../../docs/adr/0007-native-backend-with-a-server.md)):
//! everything paddock asks of whatever hosts its terminals goes through
//! [`Backend`], and everything a space's terminals say comes back as an
//! [`Event`] on its [`Connection`]. paddock's own server, in [`native`],
//! fills it in; it once had a second one, the hidden tmux server of ADR
//! 0001, removed in v0.2.

mod border;
mod env;
mod hook_vars;
mod ids;
mod native;
mod structure;

pub use border::Border;
pub use ids::{FloatId, SpaceId, TabId, TerminalId};
pub use native::{check_handover, serve, take_over, NoHandover, OtherVersion, Upgraded};
pub use structure::{Float, Split, Structure, Tab};

use crate::config::BackendKind;
use crate::input::Direction;
use crate::paths::Paths;
use crate::terminal::emulator::Emulator;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a space's connection reports, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Raw bytes a terminal's program wrote.
    Output { terminal: TerminalId, data: Vec<u8> },
    /// The space's tabs, splits, floats or names changed: read its
    /// [`Structure`] again.
    Changed,
    /// A space was made or ended somewhere (one that ended also reports
    /// its own [`Event::Ended`]).
    SpacesChanged,
    /// The connection ended: the space's last terminal closed, or paddock
    /// detached.
    Ended,
    /// The connection broke while the space may still run: the native
    /// server let this paddock go (it stopped reading), or failed. (A
    /// server that ends on purpose sends [`Event::Ended`] first.)
    Lost,
    /// The native server hands its terminals to a new binary (ADR 0009):
    /// the connection ends next ([`Event::Lost`]), and the same spaces can
    /// be attached again once the new binary serves.
    Upgrading,
    /// Input to `terminal` was refused, and `why` (native only): once,
    /// until its program has taken all that waited.
    InputRefused { terminal: TerminalId, why: String },
    /// `terminal`'s emulator rebuilt with its scrollback, from the
    /// snapshot of the moment it was attached (native only): its
    /// [`Replay`] had only its screens. Comes among its output, which
    /// [`crate::terminal::Terminal::take_scrollback`] feeds it again.
    Scrollback {
        terminal: TerminalId,
        emulator: Rebuilt,
    },
}

/// An emulator rebuilt off the event loop, for [`Event::Scrollback`]:
/// taken once. Shared, not boxed, only so [`Event`] stays `Clone` and
/// `PartialEq` (tests compare events): two compare equal only if they
/// hold the same one.
#[derive(Clone)]
pub struct Rebuilt(Arc<Mutex<Option<Emulator>>>);

impl Rebuilt {
    pub fn new(emulator: Emulator) -> Self {
        Self(Arc::new(Mutex::new(Some(emulator))))
    }

    /// The emulator, unless taken already.
    pub fn take(&self) -> Option<Emulator> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl std::fmt::Debug for Rebuilt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rebuilt")
    }
}

impl PartialEq for Rebuilt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Rebuilt {}

/// A space as listed: its id and its current name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedSpace {
    pub id: SpaceId,
    pub name: String,
}

/// One terminal of a space, floats' included, with its directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceTerminal {
    pub id: TerminalId,
    /// Its shell's directory.
    pub path: String,
    /// It is a float's, not a tab's.
    pub float: bool,
}

/// What the host knows of one terminal, for `paddock debug state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDetails {
    pub id: TerminalId,
    /// Its shell's directory.
    pub dir: String,
    /// Its foreground command.
    pub command: String,
    /// The pid of its shell.
    pub pid: u32,
    /// The path of its tty.
    pub tty: String,
    /// Lines of its scrollback, above its screen.
    pub scrollback: u32,
    /// The variables that tell a process (an agent's hook) it runs in this
    /// terminal: what [`hook_terminal`] reads.
    pub hook_env: Vec<(String, String)>,
}

/// What the host rebuilt from its saved layout when it started (ADR
/// 0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restored {
    /// Spaces that came back.
    pub spaces: usize,
    /// Agent sessions resumed in their terminals.
    pub sessions: usize,
    /// The kind of each session not over that came back as a plain
    /// shell as its id is not one to type (see
    /// [`crate::sessions::AgentKind::resume_command`]). Left out when
    /// none, and read as none from a server that never sends it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cannot_resume: Vec<crate::sessions::AgentKind>,
}

/// What a terminal held when its space was attached: the bytes that,
/// fed to a fresh emulator of `size`, rebuild its screen, scrollback,
/// cursor and modes. Output after them continues the same stream. From
/// the native server, the scrollback comes after, as an
/// [`Event::Scrollback`], so the screens draw at once.
pub struct Replay {
    pub terminal: TerminalId,
    pub size: (u16, u16),
    pub bytes: Vec<u8>,
    /// `bytes` are the screens alone: the scrollback comes after, as an
    /// [`Event::Scrollback`].
    pub scrollback_after: bool,
}

/// A running host, which names its hook event log (see
/// [`crate::sessions::log::path`]): its backend, and the id its server
/// takes at each start (a random number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instance {
    pub kind: BackendKind,
    pub id: u32,
}

/// The terminal of paddock's that a process (an agent's hook, `paddock
/// worktree`) runs in, and the instance of the backend hosting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookTerminal {
    pub instance: Instance,
    pub terminal: TerminalId,
}

/// Variables a terminal multiplexer (the user's own tmux, screen, zellij)
/// sets in its panes: a process that has one runs in that multiplexer (an
/// external session), even one started in a paddock terminal, which
/// passed its variables on.
const NESTED_VARS: &[&str] = &["TMUX", "STY", "ZELLIJ"];

/// The terminal of paddock's a process runs in, from its environment as
/// `var` reads it (an empty value counts as unset); `None` outside
/// paddock's terminals, or in those of a server other than the one under
/// `paths`. The native server's variables (`PADDOCK_PANE`,
/// `PADDOCK_SOCKET`, `PADDOCK_INSTANCE`) decide, unless a variable of
/// [`NESTED_VARS`] says the process runs in another multiplexer. paddock's
/// terminals never have those (their environment is clean). Reads only
/// the environment, but for resolving symlinks in a socket path that does
/// not match as written: every agent hook on the machine runs it.
pub fn hook_terminal(var: impl Fn(&str) -> Option<String>, paths: &Paths) -> Option<HookTerminal> {
    let var = |k: &str| var(k).filter(|v| !v.is_empty());
    if NESTED_VARS.iter().any(|k| var(k).is_some()) {
        return None;
    }
    native::hook_terminal(var, paths)
}

/// A variable lookup, as [`hook_terminal`] takes one, over `pairs`.
#[cfg(test)]
pub fn test_vars<K: AsRef<str>, V: AsRef<str>>(
    pairs: &[(K, V)],
) -> impl Fn(&str) -> Option<String> {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned()))
        .collect();
    move |k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
}

/// The process that hosts paddock's terminals, which outlive the app.
#[derive(Clone, Debug)]
pub struct Backend(native::Client);

impl Backend {
    pub fn new(paths: Paths) -> Self {
        Self(native::Client::new(paths))
    }

    /// The backend, for the commands that read nothing else of paddock's
    /// config: an error if its `[backend] kind` is not one paddock has.
    pub fn configured(paths: Paths) -> Result<Self> {
        BackendKind::load(&paths.config_dir)?;
        Ok(Self::new(paths))
    }

    /// Get ready to start the host, if it does not run yet; an error if
    /// one runs that this paddock cannot talk to ([`OtherVersion`] on
    /// native, see [`Self::upgrade_other`] and [`Self::restart`]).
    pub fn prepare(&self) -> Result<()> {
        self.0.check()
    }

    /// Start a native server that saved its layout as it ended, if none
    /// runs, so it rebuilds it (ADR 0008): only the app does, at its
    /// start. (A server started otherwise, to make a space, rebuilds it
    /// too.)
    pub fn resume(&self) -> Result<()> {
        self.0.resume()
    }

    /// End the running native server `other`, which this paddock cannot
    /// talk to, and everything in it; the next [`Self::prepare`] starts
    /// a new one, which rebuilds the layout the old one saved.
    pub fn restart(&self, other: &OtherVersion) -> Result<()> {
        self.0.restart(other)
    }

    /// Hand the running native server's terminals to the paddock binary
    /// `exe` (an absolute path; this paddock's own is
    /// [`std::env::current_exe`]), which then runs as it, whatever
    /// protocol version it speaks (ADR 0009): its pid. An error says why
    /// not. Most leave the server running unchanged, [`NoHandover`]
    /// among them (it cannot hand over at all, so [`Self::restart`] is
    /// left); but an error once `exe` ran, a refusal that says the new
    /// server could not take over, or an answer that never came, means
    /// the server has ended with every program in it.
    pub fn upgrade(&self, exe: &Path) -> Result<u32> {
        self.0.upgrade(exe)
    }

    /// [`Self::upgrade`] the running native server `other`, which this
    /// paddock cannot talk to, to `exe`, and tell how it went from what
    /// runs after it ([`Upgraded`]).
    pub fn upgrade_other(&self, other: &OtherVersion, exe: &Path) -> Result<Upgraded> {
        self.0.upgrade_other(other, exe)
    }

    /// What the host rebuilt from its saved layout when it started,
    /// once: `None` after the first ask.
    pub fn restored(&self) -> Option<Restored> {
        self.0.take_restored().ok().flatten()
    }

    /// The space paddock showed last, as the host keeps it (in
    /// `layout.json`); `None` if it cannot say.
    pub fn current_space(&self) -> Option<SpaceId> {
        self.0.current_space().ok().flatten()
    }

    /// Tell the host which space paddock shows.
    pub fn set_current_space(&self, space: &SpaceId) -> Result<()> {
        self.0.set_current_space(space)
    }

    /// An error if a host runs that this paddock cannot talk to; unlike
    /// [`Self::prepare`], changes nothing. Queries that cannot say answer
    /// with nothing, so a caller that acts on them checks this first.
    pub fn check(&self) -> Result<()> {
        self.0.check()
    }

    /// The running host's instance, as [`Self::instance`]; `None` if no
    /// host runs, and an error, unlike there, only for one that runs and
    /// cannot say (a native server whose hello gave no instance id).
    pub fn running_instance(&self) -> Result<Option<Instance>> {
        Ok(self.0.running_instance()?.map(|id| Instance {
            kind: BackendKind::Native,
            id,
        }))
    }

    /// The running host's instance, which names its hook event log (see
    /// [`crate::sessions::log::path`]); an error if it does not run.
    pub fn instance(&self) -> Result<Instance> {
        Ok(Instance {
            kind: BackendKind::Native,
            id: self.0.instance()?,
        })
    }

    /// The spaces, oldest first; none if the host does not run.
    pub fn spaces(&self) -> Vec<ListedSpace> {
        self.0.spaces().unwrap_or_default()
    }

    /// The spaces a server that has just taken over runs (ADR 0009),
    /// asked within `timeout`: none if no server runs, and an error, not
    /// none, if the connection closes unanswered, as the old binary's do
    /// until its `exec`, or gives no answer in time.
    pub fn upgraded_spaces(&self, timeout: Duration) -> Result<Vec<ListedSpace>> {
        self.0.upgraded_spaces(timeout)
    }

    /// Start a space named `name` with one shell in `cwd`, which is also
    /// its root, starting the host if needed.
    pub fn new_space(&self, name: &str, cwd: &Path, size: (u16, u16)) -> Result<SpaceId> {
        self.0.new_space(name, cwd, size)
    }

    /// Space `space`'s name as the host keeps it.
    pub fn space_name(&self, space: &SpaceId) -> Result<String> {
        self.0.space_name(space)
    }

    pub fn rename_space(&self, space: &SpaceId, name: &str) -> Result<()> {
        self.0.rename_space(space, name)
    }

    /// End a space and everything running in it.
    pub fn close_space(&self, space: &SpaceId) -> Result<()> {
        self.0.close_space(space)
    }

    /// Make `dir` space `space`'s root.
    pub fn set_root(&self, space: &SpaceId, dir: &Path) -> Result<()> {
        self.0.set_root(space, dir)
    }

    /// Space `space`'s root as stored; `None` if it has none (a space made
    /// before roots) or the host cannot say.
    pub fn root(&self, space: &SpaceId) -> Option<String> {
        self.0.root(space).unwrap_or_default()
    }

    /// Space `space`'s tabs, splits and floats.
    pub fn structure(&self, space: &SpaceId) -> Result<Structure> {
        self.0.structure(space)
    }

    /// Every terminal of space `space`, floats' too, in the host's order;
    /// none if it cannot say.
    pub fn space_terminals(&self, space: &SpaceId) -> Vec<SpaceTerminal> {
        self.0.space_terminals(space).unwrap_or_default()
    }

    /// Open a tab running the user's shell in `cwd` at the end of space
    /// `space`, and make it current. Returns its terminal.
    pub fn new_tab(&self, space: &SpaceId, cwd: &Path) -> Result<TerminalId> {
        self.0.new_tab(space, cwd)
    }

    /// Rename a tab; the host then stops naming it after its command.
    pub fn rename_tab(&self, tab: &TabId, name: &str) -> Result<()> {
        self.0.rename_tab(tab, name)
    }

    /// Make a tab its space's current one, and focus `terminal` in it.
    pub fn select(&self, tab: &TabId, terminal: Option<&TerminalId>) -> Result<()> {
        self.0.select(tab, terminal)
    }

    /// Swap tab `a` with tab `b` of the same space; `a` stays current.
    pub fn swap_tabs(&self, a: &TabId, b: &TabId) -> Result<()> {
        self.0.swap_tabs(a, b)
    }

    /// Open a float on key `key` in space `space`, running `command` in
    /// `cwd` and left in the background. It closes when the command exits.
    pub fn new_float(
        &self,
        space: &SpaceId,
        cwd: &Path,
        command: &str,
        key: &str,
    ) -> Result<FloatId> {
        self.0.new_float(space, cwd, command, key)
    }

    /// End a float and the program running in it.
    pub fn close_float(&self, float: &FloatId) -> Result<()> {
        self.0.close_float(float)
    }

    /// Split `terminal`: side by side if `columns`, else top and bottom.
    /// The new shell starts in `cwd` and takes the second half and the
    /// focus. Returns it.
    pub fn split(&self, terminal: &TerminalId, columns: bool, cwd: &Path) -> Result<TerminalId> {
        self.0.split(terminal, columns, cwd)
    }

    /// End a terminal; its tab goes with its last terminal, and the space
    /// with its last tab.
    pub fn close_terminal(&self, terminal: &TerminalId) -> Result<()> {
        self.0.close_terminal(terminal)
    }

    /// Focus the terminal next to `terminal` in `dir`, if there is one.
    pub fn focus_towards(&self, terminal: &TerminalId, dir: Direction) -> Result<()> {
        self.0.focus_towards(terminal, dir)
    }

    /// Give `terminal` and the terminals beside it the same size.
    pub fn spread(&self, terminal: &TerminalId) -> Result<()> {
        self.0.spread(terminal)
    }

    /// Zoom `terminal` to fill its tab, or unzoom its tab.
    pub fn zoom(&self, terminal: &TerminalId) -> Result<()> {
        self.0.zoom(terminal)
    }

    /// Move `terminal`'s border in `dir` by `cells`; it stops at the
    /// smallest size without an error, and unzooms the tab first.
    pub fn resize(&self, terminal: &TerminalId, dir: Direction, cells: u16) -> Result<()> {
        self.0.resize(terminal, dir, cells)
    }

    /// The border between two splits of tab `tab` at `(x, y)` in tab
    /// coordinates; `None` if there is none.
    pub fn border_at(&self, tab: &TabId, x: u16, y: u16) -> Result<Option<Border>> {
        self.0.border_at(tab, x, y)
    }

    /// Move `border` to where its sized cell is `size` cells (see
    /// [`Border::size`]).
    pub fn drag_border(&self, border: &Border, size: u16) -> Result<()> {
        self.0.drag_border(border, size)
    }

    /// `terminal`'s shell directory; `None` if the host cannot say.
    pub fn terminal_dir(&self, terminal: &TerminalId) -> Option<String> {
        self.0.terminal_dir(terminal).unwrap_or_default()
    }

    /// The space holding `terminal`; `None` if the host cannot say.
    pub fn space_of(&self, terminal: &TerminalId) -> Option<SpaceId> {
        self.0.space_of(terminal).unwrap_or_default()
    }

    /// Every terminal's foreground command; `None` if the host cannot say.
    pub fn terminal_commands(&self) -> Option<HashMap<TerminalId, String>> {
        self.0.terminal_commands().ok().flatten()
    }

    /// Every terminal by the pid of its shell; none if the host cannot say.
    pub fn terminal_pids(&self) -> HashMap<u32, TerminalId> {
        self.0.terminal_pids().unwrap_or_default()
    }

    /// Every terminal, floats' too, with what the host knows of it; none
    /// if the host does not run.
    pub fn terminal_details(&self) -> Vec<TerminalDetails> {
        self.0.terminal_details().unwrap_or_default()
    }

    /// Connect to space `space` shown at `size`, and read what each of its
    /// terminals holds. Afterwards every event, in order, goes to
    /// `deliver`, which returns false once nobody listens.
    pub fn attach(
        &self,
        space: &SpaceId,
        size: (u16, u16),
        deliver: impl FnMut(Event) -> bool + Send + 'static,
    ) -> Result<(Connection, Structure, Vec<Replay>)> {
        let (attached, structure, replays) = self.0.attach(space, size, deliver)?;
        Ok((Connection(attached), structure, replays))
    }
}

/// paddock's connection to one space: input and sizes go out through it,
/// and its [`Event`]s come back.
pub struct Connection(native::Attached);

impl Connection {
    /// Close the connection both ways: its reader sees it end.
    pub fn close(&self) {
        self.0.close()
    }

    /// Write raw bytes to `terminal`, as typed.
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        self.0.input(terminal, bytes)
    }

    /// Show the space's tabs at `(cols, rows)`.
    pub fn resize(&mut self, (cols, rows): (u16, u16)) -> Result<()> {
        self.0.resize((cols, rows))
    }

    /// Show float `float` at `(cols, rows)`, apart from the tabs' size.
    pub fn resize_float(&mut self, float: &FloatId, (cols, rows): (u16, u16)) -> Result<()> {
        self.0.resize_float(float, (cols, rows))
    }

    /// Leave the space running in the host.
    pub fn detach(self) {
        self.0.detach()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const NATIVE_SOCKET: &str = "/Users/me/.local/state/paddock/server.sock";
    const USERS_TMUX: &str = "/private/tmp/tmux-501/default,999,0";

    fn paths() -> Paths {
        Paths {
            state_dir: PathBuf::from("/Users/me/.local/state/paddock"),
            config_dir: PathBuf::from("/Users/me/.config/paddock"),
        }
    }

    fn lookup(pairs: &[(&str, &str)]) -> Option<(BackendKind, u32, String)> {
        let t = hook_terminal(test_vars(pairs), &paths())?;
        Some((t.instance.kind, t.instance.id, t.terminal.to_string()))
    }

    fn native(socket: &str) -> Vec<(&str, &str)> {
        vec![
            ("PADDOCK_PANE", "%5"),
            ("PADDOCK_SOCKET", socket),
            ("PADDOCK_INSTANCE", "77"),
        ]
    }

    fn with(
        mut vars: Vec<(&'static str, &'static str)>,
        more: &[(&'static str, &'static str)],
    ) -> Vec<(&'static str, &'static str)> {
        vars.extend_from_slice(more);
        vars
    }

    const IN_NATIVE: Option<(BackendKind, u32, &str)> = Some((BackendKind::Native, 77, "%5"));

    fn owned(t: Option<(BackendKind, u32, &str)>) -> Option<(BackendKind, u32, String)> {
        t.map(|(k, id, pane)| (k, id, pane.to_string()))
    }

    #[test]
    fn a_native_terminal_is_read_from_paddocks_variables() {
        assert_eq!(lookup(&native(NATIVE_SOCKET)), owned(IN_NATIVE));
    }

    #[test]
    fn a_tmux_started_in_a_native_terminal_runs_external_sessions() {
        let nested = with(
            native(NATIVE_SOCKET),
            &[("TMUX", USERS_TMUX), ("TMUX_PANE", "%0")],
        );
        assert_eq!(lookup(&nested), None, "the user's own tmux");
        let empty = with(native(NATIVE_SOCKET), &[("TMUX", ""), ("TMUX_PANE", "")]);
        assert_eq!(lookup(&empty), owned(IN_NATIVE), "empty is unset");
    }

    #[test]
    fn tmuxs_variables_alone_are_no_paddock_terminal() {
        assert_eq!(lookup(&[("TMUX", USERS_TMUX), ("TMUX_PANE", "%3")]), None);
    }

    #[test]
    fn screen_or_zellij_started_in_a_native_terminal_runs_external_sessions() {
        for var in ["STY", "ZELLIJ"] {
            let nested = with(native(NATIVE_SOCKET), &[(var, "1")]);
            assert_eq!(lookup(&nested), None, "{var}");
        }
    }

    #[test]
    fn another_servers_variables_fall_back_to_none() {
        assert_eq!(lookup(&native("/tmp/elsewhere/paddock/server.sock")), None);
        assert_eq!(lookup(&[]), None, "outside paddock's terminals");
    }
}
