//! Rebuilding the layout `layout.json` holds (ADR 0008), when a server
//! starts and finds one: every space, tab and split comes back with new
//! ids, and every terminal as a fresh shell in its saved directory (in
//! `$HOME` if that is gone, as [`super::pty::spawn`] does). Agents that
//! ran are not restarted: their terminals are new ones, under a new
//! instance id, so no session carries over. Each terminal's saved
//! scrollback is drawn above its new shell (see [`super::scrollback`]);
//! scrollback missing or unreadable is left out, and never stops the
//! restore. A file any space of which cannot be rebuilt is set aside
//! before anything is written over it.

use super::layout::Layout;
use super::saved::{self, SavedLayout, SavedSpace, SavedTab};
use super::scrollback;
use super::space::Space;
use super::state::State;
use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{SpaceId, TabId, TerminalId};
use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// How the server starts terminal `id`, a shell in `dir` at `size`, in
/// space `space`, with the state locked.
pub type Start<'a> =
    dyn FnMut(&State, &SpaceId, TerminalId, &Path, (u16, u16)) -> Result<HostedTerminal> + 'a;

/// Rebuild the layout saved in the state directory `dir`, if any, in
/// `state`, starting each terminal with `start`. A space that cannot be
/// rebuilt (a shell that does not start) is left out, and the file set
/// aside ([`saved::set_aside`]) so what is saved next never takes its
/// place. Returns how many spaces came back.
pub fn from_file(dir: &Path, state: &mut State, start: &mut Start<'_>) -> usize {
    let Some(saved) = saved::load(dir) else {
        return 0;
    };
    let total = saved.spaces.len();
    let restored = rebuild_layout(dir, state, saved, start);
    if restored < total {
        let lost = total - restored;
        saved::set_aside(dir, &anyhow!("{lost} of {total} saved spaces not rebuilt"));
    }
    restored
}

/// Rebuild every space of `saved` in `state`, with the scrollback saved
/// in the state directory `dir`; one that cannot be is left out, and why
/// logged. Returns how many came back.
fn rebuild_layout(
    dir: &Path,
    state: &mut State,
    saved: SavedLayout,
    start: &mut Start<'_>,
) -> usize {
    let mut restored = 0;
    for (i, space) in saved.spaces.into_iter().enumerate() {
        let name = space.name.clone();
        match rebuild_space(dir, state, space, start) {
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
fn rebuild_space(
    dir: &Path,
    state: &mut State,
    saved: SavedSpace,
    start: &mut Start<'_>,
) -> Result<Space> {
    let id = SpaceId::new(format!("${}", state.next()));
    let size = (saved.size.0.max(1), saved.size.1.max(1));
    let mut terminals = Vec::new();
    let mut tabs = Vec::new();
    for tab in saved.tabs {
        match rebuild_tab(dir, state, &id, tab, size, start, &mut terminals) {
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
/// at `size` (laid out later), under its saved scrollback, and added to
/// `terminals`.
fn rebuild_tab(
    dir: &Path,
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
        (terminal, t)
    });
    for (terminal, saved) in tree.terminals() {
        let cwd = Path::new(&saved.dir);
        let mut term = start(state, space, terminal.clone(), cwd, size)?;
        // With the state locked, so before its shell's first output.
        if let Some(key) = saved.scrollback.as_deref() {
            if let Some(text) = scrollback::load(dir, key) {
                term.seed(&text);
            }
            if scrollback::is_key(key) {
                term.scrollback = key.to_string();
            }
        }
        terminals.push(term);
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

#[cfg(test)]
mod tests {
    use super::super::layout::SavedTree;
    use super::super::saved::{SavedTerminal, FILE};
    use super::*;
    use anyhow::bail;
    use std::path::PathBuf;

    #[test]
    fn a_layout_a_space_of_which_cannot_be_rebuilt_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let text = serde_json::json!({
            "version": 1,
            "spaces": [{
                "name": "api",
                "root": "/w/api",
                "size": [80, 24],
                "current": 0,
                "tabs": [{
                    "name": "api",
                    "auto_name": true,
                    "focus": 0,
                    "splits": SavedTree::Terminal(SavedTerminal {
                        dir: "/w/api".into(),
                        ..SavedTerminal::default()
                    }),
                }],
            }],
        })
        .to_string();
        std::fs::write(dir.path().join(FILE), &text).unwrap();
        let mut state = State::new(PathBuf::from("/tmp/server.sock"), 1);
        let mut start = |_: &State, _: &SpaceId, _, _: &Path, _| -> Result<HostedTerminal> {
            bail!("no shell")
        };

        assert_eq!(from_file(dir.path(), &mut state, &mut start), 0);

        assert!(state.spaces.is_empty() && state.restored.is_none());
        assert!(!dir.path().join(FILE).exists(), "nothing to write over");
        let kept: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(kept.len(), 1);
        let kept = kept[0].as_ref().unwrap().path();
        assert_eq!(std::fs::read_to_string(kept).unwrap(), text, "kept whole");
    }
}
