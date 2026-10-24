//! A space: its tabs and splits as the backend has them (ADR 0006), and the
//! one connection that carries every terminal's output and input.

use crate::backend::{
    Backend, Connection, Event, Float, SpaceId, Structure, Tab, TabId, TerminalId,
};
use crate::terminal::Terminal;
use anyhow::Result;

/// The smallest size paddock gives the backend, as `(cols, rows)`.
pub const MIN_SIZE: (u16, u16) = (2, 1);

pub struct Space {
    pub id: SpaceId,
    pub structure: Structure,
    connection: Connection,
    /// The size the connection last asked for.
    size: (u16, u16),
}

/// `(cols, rows)`, at least [`MIN_SIZE`].
pub fn clamp((cols, rows): (u16, u16)) -> (u16, u16) {
    (cols.max(MIN_SIZE.0), rows.max(MIN_SIZE.1))
}

impl Space {
    /// Attach space `id` at `size` and rebuild every terminal's screen and
    /// history. Afterwards every event, in order, goes to `deliver`.
    /// Returns the space and a terminal for each of its terminals.
    pub fn attach(
        backend: &Backend,
        id: &SpaceId,
        size: (u16, u16),
        deliver: impl FnMut(Event) -> bool + Send + 'static,
    ) -> Result<(Self, Vec<(TerminalId, Terminal)>)> {
        let size = clamp(size);
        let (connection, structure, replays) = backend.attach(id, size, deliver)?;
        let terminals = replays
            .into_iter()
            .map(|r| {
                let mut t = Terminal::new(clamp(r.size));
                t.output(&r.bytes);
                (r.terminal, t)
            })
            .collect();
        let space = Self {
            id: id.clone(),
            structure,
            connection,
            size,
        };
        Ok((space, terminals))
    }

    /// The space's name, shown as its title.
    pub fn name(&self) -> &str {
        &self.structure.name
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.structure.tabs
    }

    /// The current tab, if any.
    pub fn current_tab(&self) -> Option<&Tab> {
        self.structure.tabs.get(self.structure.current)
    }

    pub fn current_tab_mut(&mut self) -> Option<&mut Tab> {
        self.structure.tabs.get_mut(self.structure.current)
    }

    /// The index of tab `tab`, else (it closed) of the current one.
    pub fn tab_or_current(&self, tab: &str) -> Option<usize> {
        let found = self.structure.tabs.iter().position(|w| w.id == tab);
        found.or_else(|| self.current_tab().map(|_| self.structure.current))
    }

    /// The index of the tab holding terminal `terminal`.
    pub fn tab_of(&self, terminal: &str) -> Option<usize> {
        self.structure
            .tabs
            .iter()
            .position(|w| w.contains(terminal))
    }

    /// Every terminal of the space, its floats' included.
    pub fn terminals(&self) -> impl Iterator<Item = &TerminalId> {
        self.structure
            .all_tabs()
            .flat_map(|w| w.splits.iter().map(|p| &p.id))
    }

    /// Whether terminal `terminal` is in this space, in a tab or a float.
    pub fn holds(&self, terminal: &str) -> bool {
        self.terminals().any(|p| p == terminal)
    }

    /// The float on key `key` (as [`crate::input::keymap::Key::name`]
    /// writes it), if it runs.
    pub fn float(&self, key: &str) -> Option<&Float> {
        self.structure.floats.iter().find(|f| f.key == key)
    }

    /// Write raw bytes to terminal `terminal` of this space.
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        self.connection.input(terminal, bytes)
    }

    /// Show the space at `size`: every tab of it is resized.
    pub fn resize(&mut self, size: (u16, u16)) -> Result<()> {
        let size = clamp(size);
        if size == self.size {
            return Ok(());
        }
        self.size = size;
        self.connection.resize(size)
    }

    /// Show float `float` at `size`, apart from the tabs' size.
    pub fn resize_float(&mut self, float: &TabId, size: (u16, u16)) -> Result<()> {
        self.connection.resize_float(float, clamp(size))
    }

    /// Leave the space running in the backend.
    pub fn detach(self) {
        self.connection.detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_never_go_below_the_minimum() {
        assert_eq!(clamp((0, 0)), MIN_SIZE);
        assert_eq!(clamp((40, 10)), (40, 10));
    }
}
