//! Rebuilding the layout `layout.json` holds (ADR 0008), when a server
//! starts and finds one: every space, tab and split comes back with new
//! ids, and every terminal as a fresh shell in its saved directory (in
//! `$HOME` if that is gone, as [`super::pty::spawn`] does). A terminal
//! whose saved agent session had not ended, and whose directory is still
//! there, gets that session's resume command (`claude --resume <id>`,
//! [`AgentKind::resume_command`]) typed into its shell, so quitting the
//! agent leaves a prompt (ticket 48); any other agent is not restarted.
//! A file any space of which cannot be rebuilt is set aside before
//! anything is written over it.
//!
//! [`AgentKind::resume_command`]: crate::sessions::AgentKind::resume_command

use super::layout::Layout;
use super::saved::{self, SavedAgent, SavedLayout, SavedSpace, SavedTab, SavedTerminal};
use super::space::Space;
use super::state::State;
use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{Restored, SpaceId, TabId, TerminalId};
use anyhow::{anyhow, Context, Result};
use std::path::Path;

/// How the server starts terminal `id`, a shell in `dir` at `size`, in
/// space `space`, with the state locked.
pub type Start<'a> =
    dyn FnMut(&State, &SpaceId, TerminalId, &Path, (u16, u16)) -> Result<HostedTerminal> + 'a;

/// The sessions resumed, each in its new terminal.
pub type Resumed = Vec<(TerminalId, SavedAgent)>;

/// Rebuild the layout saved in the state directory `dir`, if any, in
/// `state`, starting each terminal with `start`. A space that cannot be
/// rebuilt (a shell that does not start) is left out, and the file set
/// aside ([`saved::set_aside`]) so what is saved next never takes its
/// place. Returns how many spaces came back, and the sessions resumed.
pub fn from_file(dir: &Path, state: &mut State, start: &mut Start<'_>) -> (usize, Resumed) {
    let Some(saved) = saved::load(dir) else {
        return (0, Vec::new());
    };
    let total = saved.spaces.len();
    let (restored, resumed) = rebuild_layout(state, saved, start);
    if restored < total {
        let lost = total - restored;
        saved::set_aside(dir, &anyhow!("{lost} of {total} saved spaces not rebuilt"));
    }
    (restored, resumed)
}

/// Rebuild every space of `saved` in `state`; one that cannot be is left
/// out, and why logged. Returns how many came back, and the sessions
/// resumed in them.
fn rebuild_layout(
    state: &mut State,
    saved: SavedLayout,
    start: &mut Start<'_>,
) -> (usize, Resumed) {
    let mut restored = 0;
    let mut resumed = Vec::new();
    for (i, space) in saved.spaces.into_iter().enumerate() {
        let name = space.name.clone();
        match rebuild_space(state, space, start) {
            Ok((made, sessions)) => {
                if saved.current == Some(i) {
                    state.current = Some(made.id.clone());
                }
                state.spaces.push(made);
                resumed.extend(sessions);
                restored += 1;
            }
            Err(e) => eprintln!("paddock server: cannot restore space {name}: {e:#}"),
        }
    }
    if restored > 0 {
        state.started = true;
        state.restored = Some(Restored {
            spaces: restored,
            sessions: resumed.len(),
        });
    }
    (restored, resumed)
}

/// Space `saved` with new ids, its shells started, and the sessions
/// resumed in them; on an error, those already started are hung up.
fn rebuild_space(
    state: &mut State,
    saved: SavedSpace,
    start: &mut Start<'_>,
) -> Result<(Space, Resumed)> {
    let id = SpaceId::new(format!("${}", state.next()));
    let size = (saved.size.0.max(1), saved.size.1.max(1));
    let mut terminals = Vec::new();
    let mut resumed = Vec::new();
    let mut tabs = Vec::new();
    for tab in saved.tabs {
        match rebuild_tab(state, &id, tab, size, start, &mut terminals) {
            Ok((tab, sessions)) => {
                tabs.push(tab);
                resumed.extend(sessions);
            }
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
    Ok((space, resumed))
}

/// Tab `saved` of space `space` with new ids, each of its shells started
/// at `size` (laid out later) and added to `terminals`, and the sessions
/// resumed in them.
fn rebuild_tab(
    state: &mut State,
    space: &SpaceId,
    saved: SavedTab,
    size: (u16, u16),
    start: &mut Start<'_>,
    terminals: &mut Vec<HostedTerminal>,
) -> Result<(Tab, Resumed)> {
    let id = TabId::new(format!("@{}", state.next()));
    let tree = saved.splits.map(&mut |t| {
        let terminal = TerminalId::new(format!("%{}", state.next()));
        (terminal, t)
    });
    let mut resumed = Vec::new();
    for (terminal, saved) in tree.terminals() {
        let dir = Path::new(&saved.dir);
        let started = start(state, space, terminal.clone(), dir, size)?;
        if let Some(agent) = resume(&started, saved) {
            resumed.push((terminal.clone(), agent));
        }
        terminals.push(started);
    }
    let tree = tree.map(&mut |(terminal, _)| terminal);
    let active = tree.terminals().get(saved.focus).map(|t| (*t).clone());
    let active = active.context("a tab focusing no terminal")?;
    let layout = Layout::restore(tree)?;
    let tab = Tab::restore(
        id,
        saved.name,
        saved.auto_name,
        layout,
        active,
        saved.zoomed,
    );
    Ok((tab, resumed))
}

/// Type the resume command of `saved`'s agent session into `terminal`'s
/// new shell, which reads it once it starts, and return the session; or
/// nothing if it had ended, its kind is not resumed, or its directory is
/// gone (the shell started in `$HOME`, where the agent would not find
/// the session).
fn resume(terminal: &HostedTerminal, saved: &SavedTerminal) -> Option<SavedAgent> {
    let agent = saved.agent.as_ref().filter(|a| !a.ended)?;
    if !Path::new(&saved.dir).is_dir() {
        return None;
    }
    let command = agent.kind.resume_command(&agent.session)?;
    terminal.input.send(format!("{command}\r").into_bytes());
    Some(agent.clone())
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
                        agent: None,
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

        assert_eq!(from_file(dir.path(), &mut state, &mut start).0, 0);

        assert!(state.spaces.is_empty() && state.restored.is_none());
        assert!(!dir.path().join(FILE).exists(), "nothing to write over");
        let kept: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(kept.len(), 1);
        let kept = kept[0].as_ref().unwrap().path();
        assert_eq!(std::fs::read_to_string(kept).unwrap(), text, "kept whole");
    }
}
