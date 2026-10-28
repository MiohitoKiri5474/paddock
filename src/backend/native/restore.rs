//! Rebuilding the layout `layout.json` holds (ADR 0008), when a server
//! starts and finds one: every space, tab and split comes back with new
//! ids, and every terminal as a fresh shell in its saved directory (in
//! `$HOME` if that is gone, as [`super::pty::spawn`] does). A terminal
//! whose saved agent session is not over, and whose directory is still
//! there, is to get that session's resume command (`claude --resume
//! <id>`, [`AgentKind::resume_command`]) typed into its shell once the
//! shell has started (the server types it, see [`Resume`]), so quitting
//! the agent leaves a prompt (ticket 48); any other agent is not
//! restarted. A file any space of which cannot be rebuilt is set aside
//! before anything is written over it.
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

/// A session to resume: the command to type into a restored terminal's
/// new shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
    pub terminal: TerminalId,
    pub agent: SavedAgent,
    /// The command line, without its Enter.
    pub command: String,
}

/// Rebuild the layout saved in the state directory `dir`, if any, in
/// `state`, starting each terminal with `start`, and add the sessions to
/// resume in them to `resumed`. A space that cannot be rebuilt (a shell
/// that does not start) is left out, and the file set aside
/// ([`saved::set_aside`]) so what is saved next never takes its place.
/// Returns how many spaces came back.
pub fn from_file(
    dir: &Path,
    state: &mut State,
    start: &mut Start<'_>,
    resumed: &mut Vec<Resume>,
) -> usize {
    let Some(saved) = saved::load(dir) else {
        return 0;
    };
    let total = saved.spaces.len();
    let restored = rebuild_layout(state, saved, start, resumed);
    if restored < total {
        let lost = total - restored;
        saved::set_aside(dir, &anyhow!("{lost} of {total} saved spaces not rebuilt"));
    }
    restored
}

/// Rebuild every space of `saved` in `state`, adding the sessions to
/// resume in them to `resumed`; one that cannot be is left out, and why
/// logged. Returns how many came back.
fn rebuild_layout(
    state: &mut State,
    saved: SavedLayout,
    start: &mut Start<'_>,
    resumed: &mut Vec<Resume>,
) -> usize {
    let mut restored = 0;
    for (i, space) in saved.spaces.into_iter().enumerate() {
        let name = space.name.clone();
        let before = resumed.len();
        match rebuild_space(state, space, start, resumed) {
            Ok(made) => {
                if saved.current == Some(i) {
                    state.current = Some(made.id.clone());
                }
                state.spaces.push(made);
                restored += 1;
            }
            Err(e) => {
                // Its terminals are hung up: nothing to resume in them.
                resumed.truncate(before);
                eprintln!("paddock server: cannot restore space {name}: {e:#}");
            }
        }
    }
    if restored > 0 {
        state.started = true;
        state.restored = Some(Restored {
            spaces: restored,
            sessions: resumed.len(),
        });
    }
    restored
}

/// Space `saved` with new ids, its shells started, and the sessions to
/// resume in them added to `resumed`; on an error, the shells already
/// started are hung up.
fn rebuild_space(
    state: &mut State,
    saved: SavedSpace,
    start: &mut Start<'_>,
    resumed: &mut Vec<Resume>,
) -> Result<Space> {
    let id = SpaceId::new(format!("${}", state.next()));
    let size = (saved.size.0.max(1), saved.size.1.max(1));
    let mut terminals = Vec::new();
    let mut tabs = Vec::new();
    for tab in saved.tabs {
        match rebuild_tab(state, &id, tab, size, start, &mut terminals, resumed) {
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
/// at `size` (laid out later) and added to `terminals`, and the sessions
/// to resume in them to `resumed`.
fn rebuild_tab(
    state: &mut State,
    space: &SpaceId,
    saved: SavedTab,
    size: (u16, u16),
    start: &mut Start<'_>,
    terminals: &mut Vec<HostedTerminal>,
    resumed: &mut Vec<Resume>,
) -> Result<Tab> {
    let id = TabId::new(format!("@{}", state.next()));
    let tree = saved.splits.map(&mut |t| {
        let terminal = TerminalId::new(format!("%{}", state.next()));
        (terminal, t)
    });
    for (terminal, saved) in tree.terminals() {
        let dir = Path::new(&saved.dir);
        terminals.push(start(state, space, terminal.clone(), dir, size)?);
        resumed.extend(resume(terminal, saved));
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

/// The session of `saved` to resume in its new terminal `terminal`;
/// nothing if it is over, its kind is not resumed, or its directory is
/// gone (the shell started in `$HOME`, where the agent would not find
/// the session).
fn resume(terminal: &TerminalId, saved: &SavedTerminal) -> Option<Resume> {
    let agent = saved.agent.as_ref().filter(|a| !a.over)?;
    if !Path::new(&saved.dir).is_dir() {
        return None;
    }
    let command = agent.kind.resume_command(&agent.session)?;
    Some(Resume {
        terminal: terminal.clone(),
        agent: agent.clone(),
        command,
    })
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

        let mut resumed = Vec::new();
        assert_eq!(
            from_file(dir.path(), &mut state, &mut start, &mut resumed),
            0
        );

        assert!(state.spaces.is_empty() && state.restored.is_none());
        assert!(resumed.is_empty());
        assert!(!dir.path().join(FILE).exists(), "nothing to write over");
        let kept: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(kept.len(), 1);
        let kept = kept[0].as_ref().unwrap().path();
        assert_eq!(std::fs::read_to_string(kept).unwrap(), text, "kept whole");
    }

    #[test]
    fn only_a_session_not_over_of_a_resumed_kind_in_a_directory_still_there_resumes() {
        use crate::sessions::AgentKind;
        let dir = tempfile::tempdir().unwrap();
        let id = "7f3c2a10-1b2c-4d5e-8f90-0123456789ab";
        let terminal = TerminalId::new("%3");
        let saved = |dir: &Path, kind, over| SavedTerminal {
            dir: dir.to_string_lossy().into_owned(),
            agent: Some(SavedAgent {
                kind,
                session: id.into(),
                over,
            }),
        };
        let live = saved(dir.path(), AgentKind::Claude, false);
        let resumed = resume(&terminal, &live).expect("resumed");
        assert_eq!(resumed.command, format!("claude --resume '{id}'"));
        assert_eq!(resumed.terminal, terminal);
        let gone = dir.path().join("gone");
        for (what, t) in [
            ("over", saved(dir.path(), AgentKind::Claude, true)),
            ("gone", saved(&gone, AgentKind::Claude, false)),
            ("gemini", saved(dir.path(), AgentKind::Gemini, false)),
        ] {
            assert_eq!(resume(&terminal, &t), None, "{what}");
        }
        let none = SavedTerminal {
            agent: None,
            ..live
        };
        assert_eq!(resume(&terminal, &none), None);
    }
}
