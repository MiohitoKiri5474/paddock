//! What paddock keeps in the state directory (`layout.toml`) that the
//! backend does not hold: which space was current (on tmux; the native
//! server keeps it in `layout.json`, ADR 0008), and the sidebar's width.
//! Tabs, splits, sizes and focus live in the backend (ADR 0006). A file
//! from before, with its `current` tab and `tabs` layouts, still reads:
//! those fields are ignored.

use crate::backend::SpaceId;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE_NAME: &str = "layout.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The current space's id, as the backend spells it (tmux: `$N`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "item")]
    pub space: Option<SpaceId>,
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
    fn a_file_saved_as_item_still_names_the_current_space() {
        let saved: Saved = toml::from_str("item = \"$4\"").unwrap();
        assert_eq!(saved.space.as_ref().map(SpaceId::as_str), Some("$4"));
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("paddock-saved-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        let saved = Saved {
            space: Some("$3".into()),
            sidebar: Some(40),
        };
        save(&dir, &saved).unwrap();
        assert_eq!(load(&dir).unwrap(), Some(saved));
        std::fs::write(dir.join(FILE_NAME), "space = 1").unwrap();
        let err = format!("{:#}", load(&dir).unwrap_err());
        assert!(err.contains("layout.toml"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
