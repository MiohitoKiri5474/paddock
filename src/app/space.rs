//! A space: one tmux session on the hidden server (ADR 0006), with its
//! windows (tabs) and panes (splits) as tmux has them, and the one control
//! client that carries every pane's output and input.

use crate::terminal::{seed, Terminal};
use crate::tmux::structure::{Structure, Window};
use crate::tmux::{wait_reply, ControlClient, Message, Server};
use anyhow::Result;

/// The smallest size paddock gives tmux, as `(cols, rows)`.
pub const MIN_SIZE: (u16, u16) = (2, 1);

pub struct Space {
    /// The tmux session id (`$N`).
    pub id: String,
    pub structure: Structure,
    control: ControlClient,
    /// The size the client last asked tmux for.
    size: (u16, u16),
}

/// `(cols, rows)`, at least [`MIN_SIZE`].
pub fn clamp((cols, rows): (u16, u16)) -> (u16, u16) {
    (cols.max(MIN_SIZE.0), rows.max(MIN_SIZE.1))
}

impl Space {
    /// Attach to tmux session `id` at `size` and rebuild every pane's screen
    /// and history. Afterwards every control message, in order, goes to
    /// `deliver`. Returns the space and a terminal for each pane.
    pub fn attach(
        server: &Server,
        id: &str,
        size: (u16, u16),
        deliver: impl FnMut(Message) -> bool + Send + 'static,
    ) -> Result<(Self, Vec<(String, Terminal)>)> {
        let size = clamp(size);
        let (mut control, rx) = ControlClient::attach(server, id)?;
        control.resize(size.0, size.1)?;
        wait_reply(&rx)?;
        // Read after the resize, so the panes have their final sizes.
        let structure = server.structure(id)?;
        let panes: Vec<(&str, (u16, u16))> = structure
            .windows
            .iter()
            .flat_map(|w| &w.panes)
            .map(|p| (p.id.as_str(), (p.rect.width, p.rect.height)))
            .collect();
        let ids: Vec<&str> = panes.iter().map(|(p, _)| *p).collect();
        let seeds = seed::fetch(&mut control, &rx, &ids)?;
        control.forward(&rx, deliver);
        let mut terminals = Vec::new();
        for ((pane, size), seed) in panes.into_iter().zip(seeds) {
            let mut t = Terminal::new(clamp(size));
            t.output(&seed::stream(&seed)?);
            terminals.push((pane.to_string(), t));
        }
        let space = Self {
            id: id.to_string(),
            structure,
            control,
            size,
        };
        Ok((space, terminals))
    }

    /// The session name, shown as the space's title.
    pub fn name(&self) -> &str {
        &self.structure.name
    }

    pub fn windows(&self) -> &[Window] {
        &self.structure.windows
    }

    /// The current window (tab), if any.
    pub fn current_window(&self) -> Option<&Window> {
        self.structure.windows.get(self.structure.current)
    }

    pub fn current_window_mut(&mut self) -> Option<&mut Window> {
        self.structure.windows.get_mut(self.structure.current)
    }

    /// The index of the window holding pane `pane`.
    pub fn window_of(&self, pane: &str) -> Option<usize> {
        self.structure.windows.iter().position(|w| w.contains(pane))
    }

    pub fn panes(&self) -> impl Iterator<Item = &str> {
        self.structure
            .windows
            .iter()
            .flat_map(|w| w.panes.iter().map(|p| p.id.as_str()))
    }

    /// Write raw bytes to pane `pane` of this space.
    pub fn input(&mut self, pane: &str, bytes: &[u8]) -> Result<()> {
        self.control.send_bytes(pane, bytes)
    }

    /// Show the space at `size`: tmux resizes every window of the session.
    pub fn resize(&mut self, size: (u16, u16)) -> Result<()> {
        let size = clamp(size);
        if size == self.size {
            return Ok(());
        }
        self.size = size;
        self.control.resize(size.0, size.1)
    }

    /// Leave the space running on the hidden server.
    pub fn detach(self) {
        self.control.detach();
    }
}

/// Whether a control notification changes a space's windows or panes, so
/// they must be read again (ADR 0006).
pub fn changes_structure(notification: &str) -> bool {
    let kind = notification.split(' ').next().unwrap_or("");
    matches!(
        kind,
        "%layout-change"
            | "%window-add"
            | "%window-close"
            | "%window-renamed"
            | "%session-renamed"
            | "%session-window-changed"
            | "%window-pane-changed"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_notifications_are_told_apart_from_others() {
        for n in [
            "%layout-change @1 a87d,100x30,0,0,0 a87d,100x30,0,0,0 *",
            "%window-add @2",
            "%window-close @2",
            "%window-renamed @1 vim",
            "%session-renamed $0 api",
            "%session-window-changed $0 @2",
            "%window-pane-changed @1 %3",
        ] {
            assert!(changes_structure(n), "{n}");
        }
        for n in [
            "%unlinked-window-add @3",
            "%sessions-changed",
            "%pane-mode-changed %1",
        ] {
            assert!(!changes_structure(n), "{n}");
        }
    }

    #[test]
    fn sizes_never_go_below_the_minimum() {
        assert_eq!(clamp((0, 0)), MIN_SIZE);
        assert_eq!(clamp((40, 10)), (40, 10));
    }
}
