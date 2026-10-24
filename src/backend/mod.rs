//! The backend seam (ADR 0007): everything paddock asks of whatever hosts
//! its terminals goes through [`Backend`], and everything a space's
//! terminals say comes back as an [`Event`] on its [`Connection`]. tmux
//! (ADR 0001) is the only backend so far, and nothing outside this module
//! knows it is there: its commands, formats, notifications and ids stay
//! in [`tmux`].

mod ids;
mod structure;
mod tmux;

pub use ids::{SpaceId, TabId, TerminalId};
pub use structure::{Float, Split, Structure, Tab};
pub use tmux::layout::Border;

use crate::input::Direction;
use crate::paths::Paths;
use anyhow::Result;
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
}

/// A space as listed: its id and its current name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceEntry {
    pub id: SpaceId,
    pub name: String,
}

/// One terminal of a space, floats' included, with its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceTerminal {
    pub id: TerminalId,
    /// Its shell's directory.
    pub path: String,
    /// It is a float's, not a tab's.
    pub float: bool,
}

/// What a terminal held when its space was attached: the bytes that,
/// fed to a fresh emulator of `size`, rebuild its screen and history.
pub struct Replay {
    pub terminal: TerminalId,
    pub size: (u16, u16),
    pub bytes: Vec<u8>,
}

/// The process that hosts paddock's terminals, which outlive the app.
#[derive(Clone, Debug)]
pub struct Backend {
    tmux: tmux::Server,
}

impl Backend {
    pub fn new(paths: Paths) -> Self {
        Self {
            tmux: tmux::Server::new(paths),
        }
    }

    /// Get ready to start the host, if it does not run yet.
    pub fn prepare(&self) -> Result<()> {
        self.tmux.write_config()
    }

    /// Bring a host that already runs, maybe started by an older paddock,
    /// up to this paddock's settings.
    pub fn refresh(&self) -> Result<()> {
        self.tmux.apply_config()
    }

    /// The host's instance id, which names its hook event log (see
    /// [`crate::sessions::log::path`]); an error if it does not run.
    pub fn instance(&self) -> Result<u32> {
        self.tmux.pid()
    }

    /// The spaces, oldest first; none if the host does not run.
    pub fn spaces(&self) -> Vec<SpaceEntry> {
        self.tmux.sessions()
    }

    /// Start a space named `name` with one shell in `cwd`, which is also
    /// its root, starting the host if needed.
    pub fn new_space(&self, name: &str, cwd: &Path, size: (u16, u16)) -> Result<SpaceId> {
        self.tmux.new_session(name, cwd, size)
    }

    /// Space `space`'s name as the host keeps it.
    pub fn space_name(&self, space: &SpaceId) -> Result<String> {
        self.tmux.session_name(space)
    }

    pub fn rename_space(&self, space: &SpaceId, name: &str) -> Result<()> {
        self.tmux.rename_session(space, name)
    }

    /// End a space and everything running in it.
    pub fn close_space(&self, space: &SpaceId) -> Result<()> {
        self.tmux.kill_session(space)
    }

    /// Make `dir` space `space`'s root.
    pub fn set_root(&self, space: &SpaceId, dir: &Path) -> Result<()> {
        self.tmux.set_root(space, dir)
    }

    /// Space `space`'s root as stored; `None` if it has none (a space made
    /// before roots) or the host cannot say.
    pub fn root(&self, space: &SpaceId) -> Option<String> {
        self.tmux.root(space)
    }

    /// Space `space`'s tabs, splits and floats.
    pub fn structure(&self, space: &SpaceId) -> Result<Structure> {
        self.tmux.structure(space)
    }

    /// Every terminal of space `space`, floats' too, in the host's order;
    /// none if it cannot say.
    pub fn space_terminals(&self, space: &SpaceId) -> Vec<SpaceTerminal> {
        self.tmux.space_panes(space)
    }

    /// Open a tab running the user's shell in `cwd` at the end of space
    /// `space`, and make it current. Returns its terminal.
    pub fn new_tab(&self, space: &SpaceId, cwd: &Path) -> Result<TerminalId> {
        self.tmux.new_window(space, cwd)
    }

    /// Open a float on key `key` in space `space`, running `command` in
    /// `cwd` and left in the background. It closes when the command exits.
    pub fn new_float(
        &self,
        space: &SpaceId,
        cwd: &Path,
        command: &str,
        key: &str,
    ) -> Result<TabId> {
        self.tmux.new_float(space, cwd, command, key)
    }

    /// End a tab or a float and everything running in it.
    pub fn close_tab(&self, tab: &TabId) -> Result<()> {
        self.tmux.kill_window(tab)
    }

    /// Rename a tab; the host then stops naming it after its command.
    pub fn rename_tab(&self, tab: &TabId, name: &str) -> Result<()> {
        self.tmux.rename_window(tab, name)
    }

    /// Make a tab its space's current one, and focus `terminal` in it.
    pub fn select(&self, tab: &TabId, terminal: Option<&TerminalId>) -> Result<()> {
        self.tmux.select(tab, terminal.map(TerminalId::as_str))
    }

    /// Swap tab `a` with tab `b` of the same space; `a` stays current.
    pub fn swap_tabs(&self, a: &TabId, b: &TabId) -> Result<()> {
        self.tmux.swap_windows(a, b)
    }

    /// Split `terminal`: side by side if `columns`, else top and bottom.
    /// The new shell starts in `cwd` and takes the second half and the
    /// focus. Returns it.
    pub fn split(&self, terminal: &TerminalId, columns: bool, cwd: &Path) -> Result<TerminalId> {
        self.tmux.split(terminal, columns, cwd)
    }

    /// End a terminal; its tab goes with its last terminal, and the space
    /// with its last tab.
    pub fn close_terminal(&self, terminal: &TerminalId) -> Result<()> {
        self.tmux.kill_pane(terminal)
    }

    /// Focus the terminal next to `terminal` in `dir`, if there is one.
    pub fn focus_towards(&self, terminal: &TerminalId, dir: Direction) -> Result<()> {
        self.tmux.select_towards(terminal, dir)
    }

    /// Give `terminal` and the terminals beside it the same size.
    pub fn spread(&self, terminal: &TerminalId) -> Result<()> {
        self.tmux.spread(terminal)
    }

    /// Zoom `terminal` to fill its tab, or unzoom its tab.
    pub fn zoom(&self, terminal: &TerminalId) -> Result<()> {
        self.tmux.zoom(terminal)
    }

    /// Move `terminal`'s border in `dir` by `cells`; it stops at the
    /// smallest size without an error, and unzooms the tab first.
    pub fn resize(&self, terminal: &TerminalId, dir: Direction, cells: u16) -> Result<()> {
        self.tmux.resize(terminal, dir, cells)
    }

    /// Make `terminal`'s cell `size` columns wide (`columns`) or rows
    /// high, moving the border a [`Border`] with that terminal names.
    pub fn resize_to(&self, terminal: &TerminalId, columns: bool, size: u16) -> Result<()> {
        self.tmux.resize_to(terminal, columns, size)
    }

    /// The border between two splits of tab `tab` at `(x, y)` in tab
    /// coordinates, and how to move it; `None` if there is none.
    pub fn border_at(&self, tab: &TabId, x: u16, y: u16) -> Result<Option<Border>> {
        let text = self.tmux.layout(tab)?;
        Ok(tmux::layout::parse(&text).and_then(|l| l.border_at(x, y)))
    }

    /// `terminal`'s shell directory; `None` if the host cannot say.
    pub fn terminal_dir(&self, terminal: &TerminalId) -> Option<String> {
        self.tmux.pane_path(terminal)
    }

    /// The space holding `terminal`; `None` if the host cannot say.
    pub fn space_of(&self, terminal: &TerminalId) -> Option<SpaceId> {
        self.tmux.pane_session(terminal)
    }

    /// Every terminal's foreground command; `None` if the host cannot say.
    pub fn terminal_commands(&self) -> Option<HashMap<TerminalId, String>> {
        self.tmux.pane_commands()
    }

    /// Every terminal by the pid of its shell; none if the host cannot say.
    pub fn terminal_pids(&self) -> HashMap<u32, TerminalId> {
        self.tmux.pane_pids()
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
        let (control, structure, replays) = tmux::attach(&self.tmux, space, size, deliver)?;
        Ok((Connection { control }, structure, replays))
    }

    /// Stop the host and everything in it, and remove its socket.
    #[cfg(test)]
    pub fn stop(&self) {
        self.tmux.kill_server();
    }
}

/// paddock's connection to one space: input and sizes go out through it,
/// and its [`Event`]s come back.
pub struct Connection {
    control: tmux::ControlClient,
}

impl Connection {
    /// Write raw bytes to `terminal`, as typed.
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        self.control.send_bytes(terminal, bytes)
    }

    /// Show the space's tabs at `(cols, rows)`.
    pub fn resize(&mut self, (cols, rows): (u16, u16)) -> Result<()> {
        self.control.resize(cols, rows)
    }

    /// Show float `float` at `(cols, rows)`, apart from the tabs' size.
    pub fn resize_float(&mut self, float: &TabId, (cols, rows): (u16, u16)) -> Result<()> {
        self.control.resize_window(float, cols, rows)
    }

    /// Leave the space running in the host.
    pub fn detach(self) {
        self.control.detach();
    }
}
