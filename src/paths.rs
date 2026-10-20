//! Where paddock keeps things, with environment overrides so tests (and a
//! second copy of paddock) never share state with the real one.
//!
//! - `PADDOCK_TMUX_SOCKET`: name of the hidden server's socket (`tmux -L`), default `paddock`.
//! - `XDG_STATE_HOME`: state lives in `$XDG_STATE_HOME/paddock`, default `~/.local/state/paddock`.
//! - `XDG_CONFIG_HOME`: config lives in `$XDG_CONFIG_HOME/paddock`, default `~/.config/paddock`.

use anyhow::{Context, Result};
use std::path::PathBuf;

const DEFAULT_SOCKET: &str = "paddock";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub socket: String,
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
            socket: non_empty("PADDOCK_TMUX_SOCKET").unwrap_or_else(|| DEFAULT_SOCKET.into()),
            state_dir: state_base.join("paddock"),
            config_dir: config_base.join("paddock"),
        })
    }

    /// paddock's generated tmux config for the hidden server.
    pub fn tmux_conf(&self) -> PathBuf {
        self.state_dir.join("tmux.conf")
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
        assert_eq!(p.socket, "paddock");
        assert_eq!(p.state_dir, PathBuf::from("/h/.local/state/paddock"));
        assert_eq!(p.config_dir, PathBuf::from("/h/.config/paddock"));
        assert_eq!(
            p.tmux_conf(),
            PathBuf::from("/h/.local/state/paddock/tmux.conf")
        );
    }

    #[test]
    fn overrides_win_and_empty_values_are_ignored() {
        let p = Paths::resolve(lookup(&[
            ("HOME", "/h"),
            ("PADDOCK_TMUX_SOCKET", "test-1"),
            ("XDG_STATE_HOME", "/s"),
            ("XDG_CONFIG_HOME", ""),
        ]))
        .unwrap();
        assert_eq!(p.socket, "test-1");
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
}
