//! The server's state as `paddock debug state --json` prints it, so
//! tests ask paddock what runs where, never the server's internals.
//!
//! Tests name a terminal by a target ([`State::terminal`]): `space` (its
//! current tab's focused terminal), `space:N` (tab N's, counting from 0),
//! `space:N.M` (split M of tab N), or a terminal's id. A space is named by
//! its name or its id.

use serde::Deserialize;
use std::collections::BTreeMap;

/// The version of the JSON's shape these types read: `VERSION` in
/// `src/debug.rs`, which writes it.
const VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct State {
    pub version: u32,
    /// The backend's instance id, which names the hook event log; `None`
    /// when it does not run.
    pub instance: Option<u32>,
    /// The path of that hook event log; `None` when the backend does not
    /// run.
    pub event_log: Option<String>,
    /// Oldest first.
    pub spaces: Vec<Space>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub root: Option<String>,
    /// Index of the current tab in `tabs`.
    pub current: usize,
    pub tabs: Vec<Tab>,
    pub floats: Vec<Float>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    pub id: String,
    pub name: String,
    pub auto_name: bool,
    pub zoomed: bool,
    pub splits: Vec<Split>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Float {
    pub id: String,
    pub key: String,
    pub splits: Vec<Split>,
}

/// A terminal and its place in its tab or float.
#[derive(Debug, Clone, Deserialize)]
pub struct Split {
    pub id: String,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub focused: bool,
    pub dir: Option<String>,
    pub command: Option<String>,
    pub pid: Option<u32>,
    pub tty: Option<String>,
    /// Lines of its scrollback, above its screen.
    pub scrollback: Option<u32>,
    /// The variables that tell `paddock hook` it runs in this terminal.
    pub hook_env: BTreeMap<String, String>,
}

impl State {
    pub fn parse(json: &str) -> Self {
        let state: Self =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("debug state: {e}: {json}"));
        assert_eq!(state.version, VERSION, "debug state's version");
        state
    }

    /// The space named, or with the id, `space`.
    pub fn space(&self, space: &str) -> Option<&Space> {
        let by = |f: fn(&Space) -> &str| self.spaces.iter().find(|s| f(s) == space);
        by(|s| &s.id).or_else(|| by(|s| &s.name))
    }

    pub fn names(&self) -> Vec<String> {
        self.spaces.iter().map(|s| s.name.clone()).collect()
    }

    /// Every terminal, the tabs' then the floats', space by space.
    pub fn terminals(&self) -> impl Iterator<Item = &Split> {
        self.spaces.iter().flat_map(Space::terminals)
    }

    /// The terminal `target` names (see the module's doc).
    pub fn terminal(&self, target: &str) -> Option<&Split> {
        if let Some(t) = self.terminals().find(|t| t.id == target) {
            return Some(t);
        }
        if let Some(space) = self.space(target) {
            return Some(space.focused());
        }
        let (space, place) = target.rsplit_once(':')?;
        let tab = self
            .space(space)?
            .tabs
            .get(place.split('.').next()?.parse::<usize>().ok()?)?;
        match place.split_once('.') {
            None => Some(tab.focused()),
            Some((_, split)) => tab.splits.get(split.parse::<usize>().ok()?),
        }
    }
}

impl Space {
    pub fn current_tab(&self) -> &Tab {
        &self.tabs[self.current]
    }

    /// The current tab's focused terminal.
    pub fn focused(&self) -> &Split {
        self.current_tab().focused()
    }

    /// Every terminal, the tabs' then the floats'.
    pub fn terminals(&self) -> impl Iterator<Item = &Split> {
        let tabs = self.tabs.iter().flat_map(|t| &t.splits);
        tabs.chain(self.floats.iter().flat_map(|f| &f.splits))
    }

    /// Tab names, in order.
    pub fn tab_names(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.name.clone()).collect()
    }
}

impl Tab {
    /// Its width: where its rightmost split ends.
    pub fn width(&self) -> u16 {
        self.splits.iter().map(|s| s.x + s.width).max().unwrap_or(0)
    }

    pub fn focused(&self) -> &Split {
        let focused = self.splits.iter().find(|s| s.focused);
        focused.unwrap_or_else(|| panic!("no focused split in {self:?}"))
    }
}

impl Split {
    /// `WxH`.
    pub fn size(&self) -> String {
        format!("{}x{}", self.width, self.height)
    }

    /// Its hook variables, as `Sandbox::run` takes an environment.
    pub fn env(&self) -> Vec<(&str, &str)> {
        let vars = self.hook_env.iter();
        vars.map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }
}
