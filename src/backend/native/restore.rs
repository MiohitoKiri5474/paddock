//! Rebuilding the layout `layout.json` holds (ADR 0008), when a server
//! starts and finds one: every space, tab and split comes back with new
//! ids, and every terminal as a fresh shell in its saved directory (in
//! `$HOME` if that is gone, as [`super::pty::spawn`] does). Agents that
//! ran are not restarted: their terminals are new ones, under a new
//! instance id, so no session carries over. Floats are not restarted.

use super::layout::Layout;
use super::saved::{SavedLayout, SavedSpace, SavedTab};
use super::space::Space;
use super::state::State;
use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{SpaceId, TabId, TerminalId};
use anyhow::{Context, Result};
use std::path::Path;

/// How the server starts terminal `id`, a shell in `dir` at `size`, in
/// space `space`, with the state locked.
pub type Start<'a> =
    dyn FnMut(&State, &SpaceId, TerminalId, &Path, (u16, u16)) -> Result<HostedTerminal> + 'a;

/// Rebuild every space of `saved` in `state`, starting each terminal
/// with `start`. A space that cannot be rebuilt (a shell that does not
/// start) is left out, and why logged. Returns how many came back.
pub fn spaces(state: &mut State, saved: SavedLayout, start: &mut Start<'_>) -> usize {
    let mut restored = 0;
    for (i, space) in saved.spaces.into_iter().enumerate() {
        let name = space.name.clone();
        match self::space(state, space, start) {
            Ok(made) => {
                if saved.current == Some(i) {
                    state.current = Some(made.id.clone());
                }
                state.spaces.push(made);
                restored += 1;
            }
            Err(e) => eprintln!("paddock server: cannot restore space {name}: {e:#}"),
        }
    }
    if restored > 0 {
        state.started = true;
        state.restored = Some(restored);
    }
    restored
}

/// Space `saved` with new ids, its shells started; on an error, those
/// already started are hung up.
fn space(state: &mut State, saved: SavedSpace, start: &mut Start<'_>) -> Result<Space> {
    let id = SpaceId::new(format!("${}", state.next()));
    let size = (saved.size.0.max(1), saved.size.1.max(1));
    let mut terminals = Vec::new();
    let mut tabs = Vec::new();
    for tab in saved.tabs {
        match self::tab(state, &id, tab, size, start, &mut terminals) {
            Ok(tab) => tabs.push(tab),
            Err(e) => {
                for t in &terminals {
                    t.hang_up();
                }
                return Err(e);
            }
        }
    }
    let mut space = Space::restore(
        id,
        saved.name,
        saved.root,
        size,
        tabs,
        saved.current,
        terminals,
    );
    space.lay_out();
    Ok(space)
}

/// Tab `saved` of space `space` with new ids, each of its shells started
/// at `size` (laid out later) and added to `terminals`.
fn tab(
    state: &mut State,
    space: &SpaceId,
    saved: SavedTab,
    size: (u16, u16),
    start: &mut Start<'_>,
    terminals: &mut Vec<HostedTerminal>,
) -> Result<Tab> {
    let id = TabId::new(format!("@{}", state.next()));
    let tree = saved.splits.map(&mut |t| {
        let terminal = TerminalId::new(format!("%{}", state.next()));
        (terminal, t.dir)
    });
    for (terminal, dir) in tree.terminals() {
        terminals.push(start(state, space, terminal.clone(), Path::new(dir), size)?);
    }
    let tree = tree.map(&mut |(terminal, _)| terminal);
    let active = tree.terminals().get(saved.focus).map(|t| (*t).clone());
    let active = active.context("a tab focusing no terminal")?;
    let layout = Layout::restore(tree)?;
    Ok(Tab::restore(
        id,
        saved.name,
        saved.auto_name,
        layout,
        active,
        saved.zoomed,
    ))
}
