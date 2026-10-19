//! The layout kept in the state directory (`layout.toml`), so tabs and splits
//! come back when paddock reopens. Terminals are named by their tmux session
//! id (`$N`); tab names live in tmux (see [`super::state`]).

use super::layout::Layout;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE_NAME: &str = "layout.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// The current tab, 0-based.
    pub current: usize,
    pub tabs: Vec<Layout<String>>,
}

/// Read the saved layout; `None` if there is none yet.
pub fn load(dir: &Path) -> Result<Option<Saved>> {
    let path = dir.join(FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    toml::from_str(&text)
        .map(Some)
        .with_context(|| format!("invalid layout in {}", path.display()))
}

/// Write the layout, replacing the old file in one step.
pub fn save(dir: &Path, saved: &Saved) -> Result<()> {
    let path = dir.join(FILE_NAME);
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    let text = toml::to_string(saved).context("cannot encode the layout")?;
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// The saved layout fitted to the terminals that still run (`live`, oldest
/// first): ended ones are dropped, and live ones it does not know get a tab
/// each at the end.
pub fn restore(saved: Option<Saved>, live: &[String]) -> Saved {
    let saved = saved.unwrap_or(Saved {
        current: 0,
        tabs: Vec::new(),
    });
    let mut seen: Vec<String> = Vec::new();
    let mut tabs = Vec::new();
    let mut current = 0;
    for (i, mut tab) in saved.tabs.into_iter().enumerate() {
        if i == saved.current {
            // If this tab is dropped, the one that takes its place is current.
            current = tabs.len();
        }
        if !tab.well_formed() {
            continue;
        }
        let gone: Vec<String> = tab
            .leaves()
            .into_iter()
            .filter(|id| !live.contains(id) || seen.contains(id))
            .cloned()
            .collect();
        if gone.iter().all(|id| tab.remove(id)) {
            if !tab.contains(&tab.focus) {
                tab.focus = tab.leaves()[0].clone();
            }
            seen.extend(tab.leaves().into_iter().cloned());
            tabs.push(tab);
        }
    }
    tabs.extend(
        live.iter()
            .filter(|id| !seen.contains(id))
            .map(|id| Layout::new(id.clone())),
    );
    let current = current.min(tabs.len().saturating_sub(1));
    Saved { current, tabs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layout::Split;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn split(a: &str, b: &str) -> Layout<String> {
        let mut l = Layout::new(a.to_string());
        l.split(Split::Columns, b.to_string());
        l
    }

    fn leaves(s: &Saved) -> Vec<Vec<String>> {
        s.tabs
            .iter()
            .map(|t| t.leaves().into_iter().cloned().collect())
            .collect()
    }

    #[test]
    fn nothing_saved_gives_a_tab_per_terminal() {
        let s = restore(None, &ids(&["$1", "$2"]));
        assert_eq!(leaves(&s), [ids(&["$1"]), ids(&["$2"])]);
        assert_eq!(s.current, 0);
        assert!(restore(None, &[]).tabs.is_empty());
    }

    #[test]
    fn a_saved_layout_comes_back_as_it_was() {
        let saved = Saved {
            current: 1,
            tabs: vec![split("$1", "$2"), Layout::new("$3".into())],
        };
        assert_eq!(
            restore(Some(saved.clone()), &ids(&["$1", "$2", "$3"])),
            saved
        );
    }

    #[test]
    fn ended_terminals_are_dropped_and_new_ones_added() {
        let saved = Saved {
            current: 1,
            tabs: vec![
                split("$1", "$2"),
                Layout::new("$3".into()),
                split("$1", "$4"),
            ],
        };
        let s = restore(Some(saved), &ids(&["$2", "$4", "$5"]));
        // $3 ended, so its tab went; the tab that took its place is current.
        assert_eq!(leaves(&s), [ids(&["$2"]), ids(&["$4"]), ids(&["$5"])]);
        assert_eq!(s.tabs[0].focus, "$2");
        assert_eq!(s.current, 1);
    }

    #[test]
    fn broken_tabs_are_dropped_and_a_lost_focus_is_fixed() {
        let text = r#"
            current = 0
            [[tabs]]
            focus = "$9"
            root = { leaf = "$1" }
            [[tabs]]
            focus = "$2"
            root = { split = { split = "rows", children = [] } }
            [[tabs]]
            focus = "$3"
            root = { split = { split = "rows", children = [
                { weight = 0.0, node = { leaf = "$3" } },
            ] } }
        "#;
        let saved: Saved = toml::from_str(text).unwrap();
        let s = restore(Some(saved), &ids(&["$1", "$2", "$3"]));
        assert_eq!(leaves(&s), [ids(&["$1"]), ids(&["$2"]), ids(&["$3"])]);
        assert_eq!(s.tabs[0].focus, "$1");
    }

    #[test]
    fn a_terminal_listed_twice_is_shown_once() {
        let saved = Saved {
            current: 0,
            tabs: vec![Layout::new("$1".into()), split("$1", "$2")],
        };
        let s = restore(Some(saved), &ids(&["$1", "$2"]));
        assert_eq!(leaves(&s), [ids(&["$1"]), ids(&["$2"])]);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("paddock-saved-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        let saved = Saved {
            current: 0,
            tabs: vec![split("$1", "$2")],
        };
        save(&dir, &saved).unwrap();
        assert_eq!(load(&dir).unwrap(), Some(saved));
        std::fs::write(dir.join(FILE_NAME), "tabs = 1").unwrap();
        let err = format!("{:#}", load(&dir).unwrap_err());
        assert!(err.contains("layout.toml"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
