//! `layout.json` in the state directory (ADR 0008): the server's spaces
//! in order, each with its name, root, size, current tab and floats, its
//! tabs with their names (and whether each is automatic), focus, zoom and
//! tree of splits with their shares, each terminal by its directory, and
//! the space paddock showed last. No terminal id is kept: a restored
//! terminal is a new one, so it inherits no agent session.
//!
//! The server writes it whenever it changes ([`Saver`]), at most once
//! every [`GAP`], by an atomic replace; a server with no space left
//! removes it, so the next starts clean. A server that starts and finds
//! it rebuilds the layout from it (see `server::restore`). Floats are
//! kept but not restarted, as no program but a shell is.

use super::layout::SavedTree;
use super::space::Space;
use super::state::State;
use crate::backend::TerminalId;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const FILE: &str = "layout.json";
/// The file's own version: a file of another is not read.
const VERSION: u32 = 1;
/// The least time between two writes.
pub const GAP: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLayout {
    pub version: u32,
    /// Which of `spaces` paddock showed last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<usize>,
    pub spaces: Vec<SavedSpace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSpace {
    pub name: String,
    pub root: String,
    /// The size its tabs were shown at.
    pub size: (u16, u16),
    /// Which of `tabs` is current.
    pub current: usize,
    pub tabs: Vec<SavedTab>,
    #[serde(default)]
    pub floats: Vec<SavedFloat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTab {
    pub name: String,
    /// Named after what runs in it: the user has not renamed it.
    pub auto_name: bool,
    #[serde(default)]
    pub zoomed: bool,
    /// Which of its terminals, in the tree's order, has the focus.
    pub focus: usize,
    pub splits: SavedTree<SavedTerminal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTerminal {
    /// Its program's directory when last saved.
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFloat {
    pub key: String,
    pub dir: String,
}

impl SavedLayout {
    /// An error unless the server can rebuild every space of it.
    fn check(&self) -> Result<()> {
        if self.version != VERSION {
            bail!("version {}, not {VERSION}", self.version);
        }
        if self.current.is_some_and(|c| c >= self.spaces.len()) {
            bail!("no current space {:?}", self.current);
        }
        let mut names = HashSet::new();
        for space in &self.spaces {
            if !names.insert(&space.name) {
                bail!("two spaces named {}", space.name);
            }
            space
                .check()
                .with_context(|| format!("space {}", space.name))?;
        }
        Ok(())
    }
}

impl SavedSpace {
    fn check(&self) -> Result<()> {
        if self.current >= self.tabs.len() {
            bail!("no current tab {} of {}", self.current, self.tabs.len());
        }
        for tab in &self.tabs {
            tab.splits.check()?;
            if tab.focus >= tab.splits.terminals().len() {
                bail!("tab {} focuses no terminal", tab.name);
            }
        }
        Ok(())
    }
}

/// What `layout.json` keeps of `state`.
pub fn snapshot(state: &State) -> SavedLayout {
    let current = state.current.as_ref();
    SavedLayout {
        version: VERSION,
        current: current.and_then(|c| state.spaces.iter().position(|s| s.id == *c)),
        spaces: state.spaces.iter().map(space).collect(),
    }
}

fn space(space: &Space) -> SavedSpace {
    // A terminal whose directory cannot be read (its shell not started
    // yet) is kept in the space's root.
    let dir = |id: &TerminalId| {
        let hosted = space.hosted(id).and_then(|t| t.dir());
        hosted.unwrap_or_else(|| space.root.clone())
    };
    let tabs = space.tabs.iter().map(|tab| {
        let terminals = tab.terminals();
        SavedTab {
            name: tab.name().to_string(),
            auto_name: tab.auto_name(),
            zoomed: tab.zoomed(),
            focus: terminals
                .iter()
                .position(|t| t == tab.active())
                .unwrap_or(0),
            splits: tab.layout().save(&mut |id| SavedTerminal { dir: dir(id) }),
        }
    });
    let floats = space.floats.iter().map(|f| SavedFloat {
        key: f.key.clone(),
        dir: dir(&f.terminal),
    });
    SavedSpace {
        name: space.name.clone(),
        root: space.root.clone(),
        size: space.size,
        current: space.current,
        tabs: tabs.collect(),
        floats: floats.collect(),
    }
}

/// The layout saved in `dir`; `None` if there is none, or the file is
/// empty. One that cannot be read, or holds no layout the server can
/// rebuild, is renamed aside (`layout.json.bad-<seconds>`), never
/// deleted, and why is logged: the server starts clean.
pub fn load(dir: &Path) -> Option<SavedLayout> {
    let path = dir.join(FILE);
    match read(&path) {
        Ok(saved) => saved,
        Err(e) => {
            let at = SystemTime::now().duration_since(UNIX_EPOCH);
            let bad = dir.join(format!("{FILE}.bad-{}", at.unwrap_or_default().as_secs()));
            match std::fs::rename(&path, &bad) {
                Ok(()) => eprintln!("paddock server: {e:#}; kept as {}", bad.display()),
                Err(r) => eprintln!("paddock server: {e:#}; cannot keep it aside: {r}"),
            }
            None
        }
    }
}

fn read(path: &Path) -> Result<Option<SavedLayout>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    let saved: SavedLayout = serde_json::from_str(&text)
        .with_context(|| format!("invalid layout in {}", path.display()))?;
    saved
        .check()
        .with_context(|| format!("cannot rebuild the layout in {}", path.display()))?;
    Ok(Some(saved))
}

/// Write `layout` to `dir`, replacing the old file in one step (a file
/// beside it, renamed over it); a layout of no space removes the file.
fn write(dir: &Path, layout: &SavedLayout) -> Result<()> {
    let path = dir.join(FILE);
    if layout.spaces.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot remove {}", path.display()))
            }
            _ => Ok(()),
        };
    }
    let tmp = dir.join(format!("{FILE}.tmp"));
    let text = serde_json::to_string_pretty(layout).context("cannot encode the layout")?;
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// Writes the layout to `layout.json` when it changes, at most once
/// every [`GAP`]: the server offers it often, and only a change is
/// written.
pub struct Saver {
    dir: PathBuf,
    /// The layout last written, and when.
    last: Option<(SavedLayout, Instant)>,
}

impl Saver {
    /// A saver into the state directory `dir`, which has written nothing
    /// yet.
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, last: None }
    }

    /// Whether `layout`, offered at `now`, is to be written: it is not
    /// the one last written, and [`GAP`] has passed since that.
    fn due(&self, layout: &SavedLayout, now: Instant) -> bool {
        match &self.last {
            None => true,
            Some((last, at)) => last != layout && now.duration_since(*at) >= GAP,
        }
    }

    /// Write `layout` if it is due; true if it was written.
    pub fn offer(&mut self, layout: SavedLayout, now: Instant) -> Result<bool> {
        if !self.due(&layout, now) {
            return Ok(false);
        }
        self.write(layout, now)?;
        Ok(true)
    }

    /// Write `layout` at once unless it is the one last written: the
    /// server ends, and nothing follows.
    pub fn flush(&mut self, layout: SavedLayout) -> Result<()> {
        if self.last.as_ref().is_some_and(|(last, _)| *last == layout) {
            return Ok(());
        }
        self.write(layout, Instant::now())
    }

    fn write(&mut self, layout: SavedLayout, now: Instant) -> Result<()> {
        write(&self.dir, &layout)?;
        self.last = Some((layout, now));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::axis::Axis;
    use super::super::layout::SavedCell;
    use super::*;

    fn terminal(dir: &str) -> SavedTree<SavedTerminal> {
        SavedTree::Terminal(SavedTerminal { dir: dir.into() })
    }

    /// A layout of one space, `name`, with a tab split in two.
    fn layout(name: &str) -> SavedLayout {
        let cell = |dir, weight| SavedCell {
            weight,
            node: terminal(dir),
        };
        SavedLayout {
            version: VERSION,
            current: Some(0),
            spaces: vec![SavedSpace {
                name: name.into(),
                root: "/w/api".into(),
                size: (80, 24),
                current: 0,
                tabs: vec![SavedTab {
                    name: "api".into(),
                    auto_name: false,
                    zoomed: true,
                    focus: 1,
                    splits: SavedTree::Split {
                        axis: Axis::Columns,
                        cells: vec![cell("/w/api", 30), cell("/w/api/src", 49)],
                    },
                }],
                floats: vec![SavedFloat {
                    key: "G".into(),
                    dir: "/w/api".into(),
                }],
            }],
        }
    }

    #[test]
    fn a_layout_reads_back_as_written() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), None, "none yet");
        write(dir.path(), &layout("api")).unwrap();
        assert_eq!(load(dir.path()), Some(layout("api")));
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "no file left beside it");
    }

    #[test]
    fn a_layout_of_no_space_removes_the_file_and_an_empty_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &layout("api")).unwrap();
        let none = SavedLayout {
            current: None,
            spaces: Vec::new(),
            ..layout("api")
        };
        write(dir.path(), &none).unwrap();
        assert!(!dir.path().join(FILE).exists());
        write(dir.path(), &none).unwrap();
        std::fs::write(dir.path().join(FILE), "\n").unwrap();
        assert_eq!(load(dir.path()), None);
        assert!(dir.path().join(FILE).exists(), "an empty file stays");
    }

    #[test]
    fn an_unreadable_layout_is_kept_aside_and_reads_as_none() {
        for text in [
            "{ not json".to_string(),
            serde_json::to_string(&SavedLayout {
                current: Some(3),
                ..layout("api")
            })
            .unwrap(),
            serde_json::to_string(&SavedLayout {
                version: 99,
                ..layout("api")
            })
            .unwrap(),
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(FILE), &text).unwrap();
            assert_eq!(load(dir.path()), None, "{text}");
            let kept: Vec<String> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(kept.len(), 1, "{kept:?}");
            assert!(kept[0].starts_with("layout.json.bad-"), "{kept:?}");
            let aside = std::fs::read_to_string(dir.path().join(&kept[0])).unwrap();
            assert_eq!(aside, text, "kept whole");
        }
    }

    #[test]
    fn a_tab_focusing_no_terminal_or_two_spaces_of_one_name_are_unreadable() {
        let mut bad = layout("api");
        bad.spaces[0].tabs[0].focus = 2;
        assert!(bad.check().is_err());
        let mut twice = layout("api");
        twice.spaces.push(twice.spaces[0].clone());
        assert!(twice.check().is_err());
        assert!(layout("api").check().is_ok());
    }

    #[test]
    fn the_saver_writes_a_change_at_most_once_every_gap() {
        let dir = tempfile::tempdir().unwrap();
        let mut saver = Saver::new(dir.path().into());
        let start = Instant::now();
        assert!(saver.offer(layout("a"), start).unwrap(), "the first");
        assert!(!saver.offer(layout("a"), start + GAP).unwrap(), "no change");
        let soon = start + GAP / 2;
        assert!(!saver.offer(layout("b"), soon).unwrap(), "too soon");
        assert_eq!(load(dir.path()), Some(layout("a")));
        assert!(saver.offer(layout("b"), start + GAP).unwrap(), "once due");
        assert_eq!(load(dir.path()), Some(layout("b")));
        assert!(!saver.offer(layout("c"), start + GAP).unwrap());
        saver.flush(layout("c")).unwrap();
        assert_eq!(load(dir.path()), Some(layout("c")), "flushed at once");
    }
}
