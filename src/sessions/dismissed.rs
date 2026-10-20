//! Dismissed rows, kept in the state directory (`dismissed.toml`) so they
//! stay gone when paddock reopens and replays the hook event log.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE_NAME: &str = "dismissed.toml";

/// The user dismissed a session row (`branch` unset) or one of its task rows
/// at `at` (ms since the epoch). The row stays hidden until an event about it
/// newer than that.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Dismissal {
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    dismissed: Vec<Dismissal>,
}

/// Read the dismissals; none if nothing was dismissed yet.
pub fn load(dir: &Path) -> Result<Vec<Dismissal>> {
    let path = dir.join(FILE_NAME);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let file: File =
        toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))?;
    Ok(file.dismissed)
}

/// Write the dismissals, replacing the old file in one step.
pub fn save(dir: &Path, dismissed: &[Dismissal]) -> Result<()> {
    let path = dir.join(FILE_NAME);
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    let file = File {
        dismissed: dismissed.to_vec(),
    };
    let text = toml::to_string(&file).context("cannot encode dismissed rows")?;
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), []);
        let list = [
            Dismissal {
                pane: "%1".into(),
                branch: None,
                at: 5,
            },
            Dismissal {
                pane: "%1".into(),
                branch: Some("feat/a".into()),
                at: 6,
            },
        ];
        save(dir.path(), &list).unwrap();
        assert_eq!(load(dir.path()).unwrap(), list);
        std::fs::write(dir.path().join(FILE_NAME), "dismissed = 1").unwrap();
        let err = format!("{:#}", load(dir.path()).unwrap_err());
        assert!(err.contains(FILE_NAME), "{err}");
    }
}
