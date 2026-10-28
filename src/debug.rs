//! `paddock debug state --json`, left out of `paddock help`: the backend's
//! state as one JSON object, read through [`Backend`] alone, so tests (and
//! someone looking into a bug) see spaces, tabs, splits, floats and
//! terminals the same way whatever hosts the terminals. `version` changes
//! when a field changes meaning or goes away; a new field may come without.

use crate::backend::{Backend, Float, SpaceId, Split, Structure, Tab, TerminalDetails, TerminalId};
use crate::paths::Paths;
use crate::sessions::log;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// The version of the JSON's shape; `VERSION` in `tests/harness/state.rs`
/// reads it and changes with it.
pub const VERSION: u32 = 1;

/// One space as read from the backend.
pub struct SpaceSnapshot {
    pub id: SpaceId,
    pub root: Option<String>,
    pub structure: Structure,
}

#[derive(Serialize)]
struct State<'a> {
    version: u32,
    /// The backend's instance id (it names the hook event log); `None`
    /// when it does not run.
    instance: Option<u32>,
    /// The path of that hook event log; `None` when it does not run.
    event_log: Option<String>,
    /// Oldest first, as the sidebar lists them.
    spaces: Vec<SpaceState<'a>>,
}

#[derive(Serialize)]
struct SpaceState<'a> {
    id: &'a str,
    name: &'a str,
    root: Option<&'a str>,
    /// Index of the current tab in `tabs`.
    current: usize,
    tabs: Vec<TabState<'a>>,
    floats: Vec<FloatState<'a>>,
}

#[derive(Serialize)]
struct TabState<'a> {
    id: &'a str,
    name: &'a str,
    /// The backend still names it after what runs in it.
    auto_name: bool,
    zoomed: bool,
    splits: Vec<SplitState<'a>>,
}

#[derive(Serialize)]
struct FloatState<'a> {
    id: &'a str,
    key: &'a str,
    splits: Vec<SplitState<'a>>,
}

/// A terminal and its place; what the backend knows of the terminal is
/// `None` (or empty) if it ended between the reads.
#[derive(Serialize)]
struct SplitState<'a> {
    id: &'a str,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    /// It has the keyboard when its tab or float shows.
    focused: bool,
    dir: Option<&'a str>,
    command: Option<&'a str>,
    pid: Option<u32>,
    tty: Option<&'a str>,
    scrollback: Option<u32>,
    hook_env: BTreeMap<&'a str, &'a str>,
}

/// The JSON for `spaces` (oldest first), the backend's `instance` and
/// every terminal's `terminals` details.
pub fn state_json(
    instance: Option<(u32, &Path)>,
    spaces: &[SpaceSnapshot],
    terminals: &[TerminalDetails],
) -> Result<serde_json::Value> {
    let state = State {
        version: VERSION,
        instance: instance.map(|(id, _)| id),
        event_log: instance.map(|(_, log)| log.to_string_lossy().into_owned()),
        spaces: spaces.iter().map(|s| space(s, terminals)).collect(),
    };
    Ok(serde_json::to_value(state)?)
}

fn space<'a>(s: &'a SpaceSnapshot, terminals: &'a [TerminalDetails]) -> SpaceState<'a> {
    let tab = |t: &'a Tab| TabState {
        id: t.id.as_str(),
        name: &t.name,
        auto_name: t.auto_name,
        zoomed: t.zoomed,
        splits: splits(&t.splits, &t.active, terminals),
    };
    let float = |f: &'a Float| FloatState {
        id: f.id.as_str(),
        key: &f.key,
        splits: splits(&f.splits, &f.active, terminals),
    };
    SpaceState {
        id: s.id.as_str(),
        name: &s.structure.name,
        root: s.root.as_deref(),
        current: s.structure.current,
        tabs: s.structure.tabs.iter().map(tab).collect(),
        floats: s.structure.floats.iter().map(float).collect(),
    }
}

/// `splits` with what `terminals` knows of each; `active` is focused.
fn splits<'a>(
    splits: &'a [Split],
    active: &TerminalId,
    terminals: &'a [TerminalDetails],
) -> Vec<SplitState<'a>> {
    splits
        .iter()
        .map(|s| {
            let d = terminals.iter().find(|d| d.id == s.id);
            SplitState {
                id: s.id.as_str(),
                x: s.rect.x,
                y: s.rect.y,
                width: s.rect.width,
                height: s.rect.height,
                focused: s.id == *active,
                dir: d.map(|d| d.dir.as_str()),
                command: d.map(|d| d.command.as_str()),
                pid: d.map(|d| d.pid),
                tty: d.map(|d| d.tty.as_str()),
                scrollback: d.map(|d| d.scrollback),
                hook_env: d
                    .iter()
                    .flat_map(|d| &d.hook_env)
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
            }
        })
        .collect()
}

/// Print the backend's state as JSON on one line. It never starts the
/// backend: with none running there are no spaces. A backend running that
/// this paddock cannot talk to is an error, not an empty state.
pub fn print_state() -> Result<()> {
    let paths = Paths::from_env()?;
    let backend = Backend::configured(paths.clone())?;
    backend.check()?;
    let instance = backend.running_instance()?;
    let log = instance.map(|i| (i.id, log::path(&paths.state_dir, i)));
    // A space that ends between the list and its read is left out.
    let spaces: Vec<SpaceSnapshot> = backend
        .spaces()
        .into_iter()
        .filter_map(|s| {
            let structure = backend.structure(&s.id).ok()?;
            let root = backend.root(&s.id);
            Some(SpaceSnapshot {
                id: s.id,
                root,
                structure,
            })
        })
        .collect();
    let terminals = backend.terminal_details();
    let instance = log.as_ref().map(|(id, path)| (*id, path.as_path()));
    println!("{}", state_json(instance, &spaces, &terminals)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use serde_json::json;

    fn split(id: &str, x: u16, w: u16) -> Split {
        Split {
            id: id.into(),
            rect: Rect::new(x, 0, w, 29),
        }
    }

    #[test]
    fn the_state_names_every_space_tab_split_float_and_terminal() {
        let structure = Structure {
            name: "api".into(),
            tabs: vec![
                Tab {
                    id: "@1".into(),
                    name: "home".into(),
                    auto_name: true,
                    splits: vec![split("%1", 0, 34), split("%2", 35, 33)],
                    active: "%2".into(),
                    zoomed: true,
                },
                Tab {
                    id: "@2".into(),
                    name: "logs".into(),
                    auto_name: false,
                    splits: vec![split("%3", 0, 68)],
                    active: "%3".into(),
                    zoomed: false,
                },
            ],
            current: 1,
            floats: vec![Float {
                id: "@3".into(),
                key: "G".into(),
                splits: vec![split("%4", 0, 80)],
                active: "%4".into(),
            }],
            float_active: false,
        };
        let spaces = [SpaceSnapshot {
            id: "$1".into(),
            root: Some("/w/api".into()),
            structure,
        }];
        let terminals = [TerminalDetails {
            id: "%1".into(),
            dir: "/w/api".into(),
            command: "zsh".into(),
            pid: 100,
            tty: "/dev/ttys001".into(),
            scrollback: 12,
            hook_env: vec![("TMUX_PANE".into(), "%1".into())],
        }];
        // `%2`-`%4` ended between the reads: nothing known of them.
        let unknown = |id: &str, x: u16, width: u16, focused: bool| {
            json!({
                "id": id, "x": x, "y": 0, "width": width, "height": 29,
                "focused": focused, "dir": null, "command": null, "pid": null,
                "tty": null, "scrollback": null, "hook_env": {}
            })
        };
        assert_eq!(
            state_json(
                Some((42, Path::new("/s/events/42.jsonl"))),
                &spaces,
                &terminals
            )
            .unwrap(),
            json!({
                "version": 1,
                "instance": 42,
                "event_log": "/s/events/42.jsonl",
                "spaces": [{
                    "id": "$1",
                    "name": "api",
                    "root": "/w/api",
                    "current": 1,
                    "tabs": [
                        {
                            "id": "@1", "name": "home", "auto_name": true, "zoomed": true,
                            "splits": [
                                {
                                    "id": "%1", "x": 0, "y": 0, "width": 34, "height": 29,
                                    "focused": false, "dir": "/w/api", "command": "zsh",
                                    "pid": 100, "tty": "/dev/ttys001", "scrollback": 12,
                                    "hook_env": {"TMUX_PANE": "%1"}
                                },
                                unknown("%2", 35, 33, true),
                            ]
                        },
                        {
                            "id": "@2", "name": "logs", "auto_name": false, "zoomed": false,
                            "splits": [unknown("%3", 0, 68, true)]
                        }
                    ],
                    "floats": [{"id": "@3", "key": "G", "splits": [unknown("%4", 0, 80, true)]}]
                }]
            })
        );
    }

    #[test]
    fn with_no_backend_running_there_is_nothing_but_the_version() {
        assert_eq!(
            state_json(None, &[], &[]).unwrap(),
            json!({"version": 1, "instance": null, "event_log": null, "spaces": []})
        );
    }
}
