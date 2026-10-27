//! `[backend]` in `config.toml`: what hosts the terminals (ADR 0007).
//!
//! ```toml
//! [backend]
//! kind = "tmux"     # the default; "native" (experimental) is paddock's own server
//! ```

use super::FILE_NAME;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// `[backend] kind`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// The hidden tmux server (ADR 0001).
    #[default]
    Tmux,
    /// paddock's own server, experimental while it is built.
    Native,
}

/// A `[backend]` table as written.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBackend {
    pub kind: Option<BackendKind>,
}

impl BackendKind {
    /// `[backend] kind` from `config.toml` in `dir`, for the commands that
    /// need only it (`paddock debug state`, `paddock worktree`): the
    /// default if the file is missing or is not TOML (paddock itself
    /// reports that), but an error for a kind it does not know.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        Self::read(&text).with_context(|| format!("invalid config in {}", path.display()))
    }

    fn read(text: &str) -> Result<Self> {
        let Ok(table) = toml::from_str::<toml::Table>(text) else {
            return Ok(Self::default());
        };
        let Some(section) = table.get("backend") else {
            return Ok(Self::default());
        };
        let raw: RawBackend = section.clone().try_into()?;
        Ok(raw.kind.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn the_backend_is_tmux_unless_set_and_unknown_kinds_are_rejected() {
        assert_eq!(Config::default().backend, BackendKind::Tmux);
        let c = Config::parse("[backend]\nkind = \"native\"\n").unwrap();
        assert_eq!(c.backend, BackendKind::Native);
        let tmux = Config::parse("[backend]\nkind = \"tmux\"\n").unwrap();
        assert_eq!(tmux.backend, BackendKind::Tmux);
        let err = format!(
            "{:#}",
            Config::parse("[backend]\nkind = \"zellij\"\n").unwrap_err()
        );
        assert!(err.contains("zellij"), "{err}");
        assert!(Config::parse("[backend]\nhost = \"x\"\n").is_err());
    }

    #[test]
    fn the_kind_reads_alone_and_leniently_but_not_an_unknown_kind() {
        assert_eq!(BackendKind::read("").unwrap(), BackendKind::Tmux);
        // Another section's mistake is paddock's to report, not this read's.
        let other = "[keys.commands]\nnew_tab = 1\n[backend]\nkind = \"native\"\n";
        assert_eq!(BackendKind::read(other).unwrap(), BackendKind::Native);
        assert_eq!(BackendKind::read("not toml [").unwrap(), BackendKind::Tmux);
        assert!(BackendKind::read("[backend]\nkind = \"zellij\"\n").is_err());
    }
}
