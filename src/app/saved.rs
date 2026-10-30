//! What paddock keeps in the state directory (`layout.toml`) that the
//! backend does not hold: the sidebar's width. Tabs, splits, sizes, focus
//! and the current space live in the server (ADR 0008). A file from
//! before, with its `current` tab and `tabs` layouts, or the current
//! space the tmux backend kept here, still reads: those fields are
//! ignored.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE_NAME: &str = "layout.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The sidebar's width when last changed (`C-a {`, `C-a }`, dragging).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar: Option<u16>,
}

/// Read the saved state; `None` if there is none yet.
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

/// Write the state, replacing the old file in one step.
pub fn save(dir: &Path, saved: &Saved) -> Result<()> {
    let path = dir.join(FILE_NAME);
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    let text = toml::to_string(saved).context("cannot encode the layout")?;
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_from_before_items_reads_as_no_item() {
        let text = r#"
            current = 1
            [[tabs]]
            focus = "$1"
            root = { leaf = "$1" }
        "#;
        assert_eq!(toml::from_str::<Saved>(text).unwrap(), Saved::default());
    }

    #[test]
    fn a_file_with_the_tmux_backends_current_space_still_reads() {
        let saved: Saved = toml::from_str("space = \"$4\"\nsidebar = 30").unwrap();
        assert_eq!(saved.sidebar, Some(30));
        let saved: Saved = toml::from_str("item = \"$4\"").unwrap();
        assert_eq!(saved, Saved::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("paddock-saved-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        let saved = Saved { sidebar: Some(40) };
        save(&dir, &saved).unwrap();
        assert_eq!(load(&dir).unwrap(), Some(saved));
        std::fs::write(dir.join(FILE_NAME), "sidebar = \"wide\"").unwrap();
        let err = format!("{:#}", load(&dir).unwrap_err());
        assert!(err.contains("layout.toml"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
