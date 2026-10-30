//! `[backend]` in `config.toml`: what hosts the terminals (ADR 0007).
//!
//! ```toml
//! [backend]
//! kind = "native"   # the default, and the only one: paddock's own server
//! ```
//!
//! `"tmux"`, the hidden tmux server of ADR 0001, was removed in v0.2: it
//! is an error that says so.

use super::FILE_NAME;
use anyhow::{Context, Result};
use serde::de::{Deserializer, Error as _};
use serde::Deserialize;
use std::path::Path;

/// `[backend] kind`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BackendKind {
    /// paddock's own server (ADR 0007).
    #[default]
    Native,
}

/// The error for `kind = "tmux"`.
const TMUX_REMOVED: &str =
    "the tmux backend was removed in v0.2: delete `kind = \"tmux\"` from [backend]";

impl<'de> Deserialize<'de> for BackendKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "native" => Ok(Self::Native),
            "tmux" => Err(D::Error::custom(TMUX_REMOVED)),
            other => Err(D::Error::unknown_variant(other, &["native"])),
        }
    }
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
    fn native_is_accepted_and_unknown_kinds_are_rejected() {
        assert!(Config::parse("[backend]\nkind = \"native\"\n").is_ok());
        let err = format!(
            "{:#}",
            Config::parse("[backend]\nkind = \"zellij\"\n").unwrap_err()
        );
        assert!(err.contains("zellij"), "{err}");
        assert!(Config::parse("[backend]\nhost = \"x\"\n").is_err());
    }

    #[test]
    fn the_kind_reads_alone_and_leniently_but_not_an_unknown_kind() {
        assert_eq!(BackendKind::read("").unwrap(), BackendKind::Native);
        let native = "[backend]\nkind = \"native\"\n";
        assert_eq!(BackendKind::read(native).unwrap(), BackendKind::Native);
        // Another section's mistake is paddock's to report, not this read's.
        let other = "[keys.commands]\nnew_tab = 1\n[backend]\nkind = \"native\"\n";
        assert_eq!(BackendKind::read(other).unwrap(), BackendKind::Native);
        assert_eq!(
            BackendKind::read("not toml [").unwrap(),
            BackendKind::Native
        );
        assert!(BackendKind::read("[backend]\nkind = \"zellij\"\n").is_err());
    }

    #[test]
    fn the_tmux_backend_is_an_error_that_says_it_was_removed() {
        let tmux = "[backend]\nkind = \"tmux\"\n";
        let err = format!("{:#}", Config::parse(tmux).unwrap_err());
        assert!(err.contains(TMUX_REMOVED), "{err}");
        // The commands that read only the kind say the same.
        let err = format!("{:#}", BackendKind::read(tmux).unwrap_err());
        assert!(err.contains(TMUX_REMOVED), "{err}");
    }
}
