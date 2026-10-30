//! Where paddock keeps things, with environment overrides so tests (and a
//! second copy of paddock) never share state with the real one; and how
//! paths under home are shown (`~`).
//!
//! - `XDG_STATE_HOME`: state lives in `$XDG_STATE_HOME/paddock`, default `~/.local/state/paddock`.
//! - `XDG_CONFIG_HOME`: config lives in `$XDG_CONFIG_HOME/paddock`, default `~/.config/paddock`.

use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        Self::resolve(|k| std::env::var(k).ok())
    }

    /// Resolve paths from a variable lookup (injectable for tests).
    pub fn resolve(var: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let non_empty = |k: &str| var(k).filter(|v| !v.is_empty());
        let home = || {
            non_empty("HOME")
                .map(PathBuf::from)
                .context("HOME is not set")
        };
        let state_base = match non_empty("XDG_STATE_HOME") {
            Some(d) => PathBuf::from(d),
            None => home()?.join(".local/state"),
        };
        let config_base = match non_empty("XDG_CONFIG_HOME") {
            Some(d) => PathBuf::from(d),
            None => home()?.join(".config"),
        };
        Ok(Self {
            state_dir: state_base.join("paddock"),
            config_dir: config_base.join("paddock"),
        })
    }
}

/// The home directory, for showing paths with `~`.
pub struct HomePaths {
    home: String,
    /// Directories may come with symlinks resolved (`/tmp` is
    /// `/private/tmp` on macOS), so home may be written either way.
    real_home: String,
}

impl HomePaths {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let real_home = crate::git::resolved(&home);
        Self { home, real_home }
    }

    /// `path` with home, written either way, as `~`.
    pub fn show(&self, path: &str) -> String {
        let short = tilde(path, &self.home);
        if short == path {
            tilde(path, &self.real_home)
        } else {
            short
        }
    }
}

/// `path` with the home directory `home` written `~`.
pub fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn defaults_follow_home() {
        let p = Paths::resolve(lookup(&[("HOME", "/h")])).unwrap();
        assert_eq!(p.state_dir, PathBuf::from("/h/.local/state/paddock"));
        assert_eq!(p.config_dir, PathBuf::from("/h/.config/paddock"));
    }

    #[test]
    fn overrides_win_and_empty_values_are_ignored() {
        let p = Paths::resolve(lookup(&[
            ("HOME", "/h"),
            ("XDG_STATE_HOME", "/s"),
            ("XDG_CONFIG_HOME", ""),
        ]))
        .unwrap();
        assert_eq!(p.state_dir, PathBuf::from("/s/paddock"));
        assert_eq!(p.config_dir, PathBuf::from("/h/.config/paddock"));
    }

    #[test]
    fn missing_home_is_an_error_only_when_needed() {
        assert!(Paths::resolve(lookup(&[])).is_err());
        let p = Paths::resolve(lookup(&[
            ("XDG_STATE_HOME", "/s"),
            ("XDG_CONFIG_HOME", "/c"),
        ]));
        assert!(p.is_ok());
    }

    #[test]
    fn home_is_written_as_a_tilde() {
        assert_eq!(tilde("/Users/me/project/api", "/Users/me"), "~/project/api");
        assert_eq!(tilde("/Users/me", "/Users/me"), "~");
        assert_eq!(tilde("/Users/meg/x", "/Users/me"), "/Users/meg/x");
        assert_eq!(tilde("/tmp", ""), "/tmp");
    }
}
