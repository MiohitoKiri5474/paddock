//! The backend seam ([ADR 0007](../../docs/adr/0007-native-backend-with-a-server.md)):
//! everything paddock asks of whatever hosts its terminals goes through
//! [`Backend`], and everything a space's terminals say comes back as an
//! [`Event`] on its [`Connection`]. Two backends fill it in, picked by
//! `[backend] kind`: tmux (ADR 0001), whose commands, formats,
//! notifications and ids stay in [`tmux`], and paddock's own server, in
//! [`native`]. Nothing outside this module knows which one runs.

mod border;
mod env;
mod hook_vars;
mod ids;
mod native;
mod structure;
mod tmux;

pub use border::Border;
pub use ids::{FloatId, SpaceId, TabId, TerminalId};
pub use native::serve;
pub use structure::{Float, Split, Structure, Tab};

use crate::config::BackendKind;
use crate::input::Direction;
use crate::paths::Paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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
    /// Input to `terminal` was refused, and `why` (native only): once,
    /// until its program has taken all that waited.
    InputRefused { terminal: TerminalId, why: String },
}

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

/// What a terminal held when its space was attached: the bytes that,
/// fed to a fresh emulator of `size`, rebuild its screen, scrollback,
/// cursor and modes. Output after them continues the same stream.
pub struct Replay {
    pub terminal: TerminalId,
    pub size: (u16, u16),
    pub bytes: Vec<u8>,
}

/// A running host, which names its hook event log (see
/// [`crate::sessions::log::path`]): its backend, and the id that backend
/// gives each start of its server (tmux: the server's pid; native: a
/// random number). The two backends' ids may coincide, so the kind is
/// part of it.
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

/// Variables a terminal multiplexer other than tmux sets in its panes: a
/// process that has one runs in that multiplexer, even one started in a
/// native terminal (which passed its variables on).
const NESTED_VARS: &[&str] = &["STY", "ZELLIJ"];

/// The terminal of paddock's a process runs in, from its environment as
/// `var` reads it (an empty value counts as unset); `None` outside
/// paddock's terminals, or in those of a backend other than the one under
/// `paths`. A set `TMUX` decides: the process runs in a tmux pane,
/// paddock's (`TMUX_PANE`) or one of the user's own tmux (an external
/// session, even when that tmux was started in a native terminal). Else
/// the native server's variables (`PADDOCK_PANE`, `PADDOCK_SOCKET`,
/// `PADDOCK_INSTANCE`) do, unless a variable of [`NESTED_VARS`] says the
/// process runs in another multiplexer. Native terminals never have
/// `TMUX` (their environment is clean), so there theirs come first. Reads
/// only the environment, but for resolving symlinks in a socket path that
/// does not match as written: every agent hook on the machine runs it.
pub fn hook_terminal(var: impl Fn(&str) -> Option<String>, paths: &Paths) -> Option<HookTerminal> {
    let var = |k: &str| var(k).filter(|v| !v.is_empty());
    if var("TMUX").is_some() {
        return tmux::hook_terminal(var, &paths.socket);
    }
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
pub enum Backend {
    Tmux(tmux::Server),
    Native(native::Client),
}

impl Backend {
    pub fn new(paths: Paths, kind: BackendKind) -> Self {
        match kind {
            BackendKind::Tmux => Self::Tmux(tmux::Server::new(paths)),
            BackendKind::Native => Self::Native(native::Client::new(paths)),
        }
    }

    /// The backend `[backend] kind` in paddock's config picks, for the
    /// commands that read nothing else of it.
    pub fn configured(paths: Paths) -> Result<Self> {
        let kind = BackendKind::load(&paths.config_dir)?;
        Ok(Self::new(paths, kind))
    }

    /// Get ready to start the host, if it does not run yet; an error if
    /// one runs that this paddock cannot talk to.
    pub fn prepare(&self) -> Result<()> {
        match self {
            Self::Tmux(t) => t.write_config(),
            Self::Native(n) => n.check(),
        }
    }

    /// An error if a host runs that this paddock cannot talk to; unlike
    /// [`Self::prepare`], changes nothing. Queries that cannot say answer
    /// with nothing, so a caller that acts on them checks this first.
    pub fn check(&self) -> Result<()> {
        match self {
            Self::Tmux(_) => Ok(()),
            Self::Native(n) => n.check(),
        }
    }

    /// Bring a host that already runs, maybe started by an older paddock,
    /// up to this paddock's settings.
    pub fn refresh(&self) -> Result<()> {
        match self {
            Self::Tmux(t) => t.apply_config(),
            Self::Native(_) => Ok(()),
        }
    }

    /// The running host's instance, as [`Self::instance`]; `None` if no
    /// host runs, and an error, unlike there, only for one that runs and
    /// cannot say (a native server whose hello gave no instance id).
    pub fn running_instance(&self) -> Result<Option<Instance>> {
        Ok(match self {
            Self::Tmux(t) => t.pid().ok().map(|id| Instance {
                kind: BackendKind::Tmux,
                id,
            }),
            Self::Native(n) => n.running_instance()?.map(|id| Instance {
                kind: BackendKind::Native,
                id,
            }),
        })
    }

    /// The running host's instance, which names its hook event log (see
    /// [`crate::sessions::log::path`]); an error if it does not run.
    pub fn instance(&self) -> Result<Instance> {
        Ok(match self {
            Self::Tmux(t) => Instance {
                kind: BackendKind::Tmux,
                id: t.pid()?,
            },
            Self::Native(n) => Instance {
                kind: BackendKind::Native,
                id: n.instance()?,
            },
        })
    }

    /// The spaces, oldest first; none if the host does not run.
    pub fn spaces(&self) -> Vec<ListedSpace> {
        match self {
            Self::Tmux(t) => t.sessions(),
            // Here and below: none when it cannot say, as with tmux. A
            // server this paddock cannot talk to is `check`'s error.
            Self::Native(n) => n.spaces().unwrap_or_default(),
        }
    }

    /// Start a space named `name` with one shell in `cwd`, which is also
    /// its root, starting the host if needed.
    pub fn new_space(&self, name: &str, cwd: &Path, size: (u16, u16)) -> Result<SpaceId> {
        match self {
            Self::Tmux(t) => t.new_session(name, cwd, size),
            Self::Native(n) => n.new_space(name, cwd, size),
        }
    }

    /// Space `space`'s name as the host keeps it.
    pub fn space_name(&self, space: &SpaceId) -> Result<String> {
        match self {
            Self::Tmux(t) => t.session_name(space.as_str()),
            Self::Native(n) => n.space_name(space),
        }
    }

    pub fn rename_space(&self, space: &SpaceId, name: &str) -> Result<()> {
        match self {
            Self::Tmux(t) => t.rename_session(space.as_str(), name),
            Self::Native(n) => n.rename_space(space, name),
        }
    }

    /// End a space and everything running in it.
    pub fn close_space(&self, space: &SpaceId) -> Result<()> {
        match self {
            Self::Tmux(t) => t.kill_session(space.as_str()),
            Self::Native(n) => n.close_space(space),
        }
    }

    /// Make `dir` space `space`'s root.
    pub fn set_root(&self, space: &SpaceId, dir: &Path) -> Result<()> {
        match self {
            Self::Tmux(t) => t.set_root(space.as_str(), dir),
            Self::Native(n) => n.set_root(space, dir),
        }
    }

    /// Space `space`'s root as stored; `None` if it has none (a space made
    /// before roots) or the host cannot say.
    pub fn root(&self, space: &SpaceId) -> Option<String> {
        match self {
            Self::Tmux(t) => t.root(space.as_str()),
            Self::Native(n) => n.root(space).unwrap_or_default(),
        }
    }

    /// Space `space`'s tabs, splits and floats.
    pub fn structure(&self, space: &SpaceId) -> Result<Structure> {
        match self {
            Self::Tmux(t) => t.structure(space.as_str()),
            Self::Native(n) => n.structure(space),
        }
    }

    /// Every terminal of space `space`, floats' too, in the host's order;
    /// none if it cannot say.
    pub fn space_terminals(&self, space: &SpaceId) -> Vec<SpaceTerminal> {
        match self {
            Self::Tmux(t) => t.space_panes(space.as_str()),
            Self::Native(n) => n.space_terminals(space).unwrap_or_default(),
        }
    }

    /// Open a tab running the user's shell in `cwd` at the end of space
    /// `space`, and make it current. Returns its terminal.
    pub fn new_tab(&self, space: &SpaceId, cwd: &Path) -> Result<TerminalId> {
        match self {
            Self::Tmux(t) => t.new_window(space.as_str(), cwd),
            Self::Native(n) => n.new_tab(space, cwd),
        }
    }

    /// Rename a tab; the host then stops naming it after its command.
    pub fn rename_tab(&self, tab: &TabId, name: &str) -> Result<()> {
        match self {
            Self::Tmux(t) => t.rename_window(tab.as_str(), name),
            Self::Native(n) => n.rename_tab(tab, name),
        }
    }

    /// Make a tab its space's current one, and focus `terminal` in it.
    pub fn select(&self, tab: &TabId, terminal: Option<&TerminalId>) -> Result<()> {
        match self {
            Self::Tmux(t) => t.select(tab.as_str(), terminal.map(TerminalId::as_str)),
            Self::Native(n) => n.select(tab, terminal),
        }
    }

    /// Swap tab `a` with tab `b` of the same space; `a` stays current.
    pub fn swap_tabs(&self, a: &TabId, b: &TabId) -> Result<()> {
        match self {
            Self::Tmux(t) => t.swap_windows(a.as_str(), b.as_str()),
            Self::Native(n) => n.swap_tabs(a, b),
        }
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
        match self {
            Self::Tmux(t) => t.new_float(space.as_str(), cwd, command, key),
            Self::Native(n) => n.new_float(space, cwd, command, key),
        }
    }

    /// End a float and the program running in it.
    pub fn close_float(&self, float: &FloatId) -> Result<()> {
        match self {
            Self::Tmux(t) => t.kill_window(float.as_str()),
            Self::Native(n) => n.close_float(float),
        }
    }

    /// Split `terminal`: side by side if `columns`, else top and bottom.
    /// The new shell starts in `cwd` and takes the second half and the
    /// focus. Returns it.
    pub fn split(&self, terminal: &TerminalId, columns: bool, cwd: &Path) -> Result<TerminalId> {
        match self {
            Self::Tmux(t) => t.split(terminal.as_str(), columns, cwd),
            Self::Native(n) => n.split(terminal, columns, cwd),
        }
    }

    /// End a terminal; its tab goes with its last terminal, and the space
    /// with its last tab.
    pub fn close_terminal(&self, terminal: &TerminalId) -> Result<()> {
        match self {
            Self::Tmux(t) => t.kill_pane(terminal.as_str()),
            Self::Native(n) => n.close_terminal(terminal),
        }
    }

    /// Focus the terminal next to `terminal` in `dir`, if there is one.
    pub fn focus_towards(&self, terminal: &TerminalId, dir: Direction) -> Result<()> {
        match self {
            Self::Tmux(t) => t.select_towards(terminal.as_str(), dir),
            Self::Native(n) => n.focus_towards(terminal, dir),
        }
    }

    /// Give `terminal` and the terminals beside it the same size.
    pub fn spread(&self, terminal: &TerminalId) -> Result<()> {
        match self {
            Self::Tmux(t) => t.spread(terminal.as_str()),
            Self::Native(n) => n.spread(terminal),
        }
    }

    /// Zoom `terminal` to fill its tab, or unzoom its tab.
    pub fn zoom(&self, terminal: &TerminalId) -> Result<()> {
        match self {
            Self::Tmux(t) => t.zoom(terminal.as_str()),
            Self::Native(n) => n.zoom(terminal),
        }
    }

    /// Move `terminal`'s border in `dir` by `cells`; it stops at the
    /// smallest size without an error, and unzooms the tab first.
    pub fn resize(&self, terminal: &TerminalId, dir: Direction, cells: u16) -> Result<()> {
        match self {
            Self::Tmux(t) => t.resize(terminal.as_str(), dir, cells),
            Self::Native(n) => n.resize(terminal, dir, cells),
        }
    }

    /// The border between two splits of tab `tab` at `(x, y)` in tab
    /// coordinates; `None` if there is none.
    pub fn border_at(&self, tab: &TabId, x: u16, y: u16) -> Result<Option<Border>> {
        match self {
            Self::Tmux(t) => t.border_at(tab.as_str(), x, y),
            Self::Native(n) => n.border_at(tab, x, y),
        }
    }

    /// Move `border` to where its sized cell is `size` cells (see
    /// [`Border::size`]).
    pub fn drag_border(&self, border: &Border, size: u16) -> Result<()> {
        match self {
            Self::Tmux(t) => t.resize_to(border.terminal.as_str(), border.columns(), size),
            Self::Native(n) => n.drag_border(border, size),
        }
    }

    /// `terminal`'s shell directory; `None` if the host cannot say.
    pub fn terminal_dir(&self, terminal: &TerminalId) -> Option<String> {
        match self {
            Self::Tmux(t) => t.pane_path(terminal.as_str()),
            Self::Native(n) => n.terminal_dir(terminal).unwrap_or_default(),
        }
    }

    /// The space holding `terminal`; `None` if the host cannot say.
    pub fn space_of(&self, terminal: &TerminalId) -> Option<SpaceId> {
        match self {
            Self::Tmux(t) => t.pane_session(terminal.as_str()),
            Self::Native(n) => n.space_of(terminal).unwrap_or_default(),
        }
    }

    /// Every terminal's foreground command; `None` if the host cannot say.
    pub fn terminal_commands(&self) -> Option<HashMap<TerminalId, String>> {
        match self {
            Self::Tmux(t) => t.pane_commands(),
            Self::Native(n) => n.terminal_commands().ok().flatten(),
        }
    }

    /// Every terminal by the pid of its shell; none if the host cannot say.
    pub fn terminal_pids(&self) -> HashMap<u32, TerminalId> {
        match self {
            Self::Tmux(t) => t.pane_pids(),
            Self::Native(n) => n.terminal_pids().unwrap_or_default(),
        }
    }

    /// Every terminal, floats' too, with what the host knows of it; none
    /// if the host does not run.
    pub fn terminal_details(&self) -> Vec<TerminalDetails> {
        match self {
            Self::Tmux(t) => t.pane_details(),
            Self::Native(n) => n.terminal_details().unwrap_or_default(),
        }
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
        match self {
            Self::Tmux(t) => {
                let (control, structure, replays) = tmux::attach(t, space.as_str(), size, deliver)?;
                Ok((Connection::Tmux(control), structure, replays))
            }
            Self::Native(n) => {
                let (attached, structure, replays) = n.attach(space, size, deliver)?;
                Ok((Connection::Native(attached), structure, replays))
            }
        }
    }

    /// Stop the host and everything in it, and remove its socket.
    #[cfg(test)]
    pub fn stop(&self) {
        match self {
            Self::Tmux(t) => t.kill_server(),
            Self::Native(n) => n.stop(),
        }
    }
}

/// paddock's connection to one space: input and sizes go out through it,
/// and its [`Event`]s come back.
pub enum Connection {
    Tmux(tmux::ControlClient),
    Native(native::Attached),
}

impl Connection {
    /// Write raw bytes to `terminal`, as typed.
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Tmux(c) => c.send_bytes(terminal.as_str(), bytes),
            Self::Native(a) => a.input(terminal, bytes),
        }
    }

    /// Show the space's tabs at `(cols, rows)`.
    pub fn resize(&mut self, (cols, rows): (u16, u16)) -> Result<()> {
        match self {
            Self::Tmux(c) => c.resize(cols, rows),
            Self::Native(a) => a.resize((cols, rows)),
        }
    }

    /// Show float `float` at `(cols, rows)`, apart from the tabs' size.
    pub fn resize_float(&mut self, float: &FloatId, (cols, rows): (u16, u16)) -> Result<()> {
        match self {
            Self::Tmux(c) => c.resize_window(float.as_str(), cols, rows),
            Self::Native(a) => a.resize_float(float, (cols, rows)),
        }
    }

    /// Leave the space running in the host.
    pub fn detach(self) {
        match self {
            Self::Tmux(c) => c.detach(),
            Self::Native(a) => a.detach(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const NATIVE_SOCKET: &str = "/Users/me/.local/state/paddock/server.sock";
    const TMUX: &str = "/private/tmp/tmux-501/paddock,4242,0";
    const USERS_TMUX: &str = "/private/tmp/tmux-501/default,999,0";

    fn paths() -> Paths {
        Paths {
            socket: "paddock".into(),
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
    fn a_tmux_terminal_is_read_from_tmuxs_variables() {
        let tmux = [("TMUX", TMUX), ("TMUX_PANE", "%3")];
        assert_eq!(lookup(&tmux), Some((BackendKind::Tmux, 4242, "%3".into())));
    }

    #[test]
    fn a_tmux_started_in_a_native_terminal_runs_external_sessions() {
        let nested = with(
            native(NATIVE_SOCKET),
            &[("TMUX", USERS_TMUX), ("TMUX_PANE", "%0")],
        );
        assert_eq!(lookup(&nested), None, "the user's own tmux");
        let ours = with(
            native(NATIVE_SOCKET),
            &[("TMUX", TMUX), ("TMUX_PANE", "%3")],
        );
        assert_eq!(lookup(&ours), Some((BackendKind::Tmux, 4242, "%3".into())));
        let empty = with(native(NATIVE_SOCKET), &[("TMUX", ""), ("TMUX_PANE", "")]);
        assert_eq!(lookup(&empty), owned(IN_NATIVE), "empty is unset");
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
