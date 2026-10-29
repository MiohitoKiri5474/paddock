//! The clean environment every backend's host runs with (ADR 0001):
//! an allowlist of paddock's own variables, so nothing from whichever
//! terminal app launched paddock (`SUPACODE_*`, `TERM_PROGRAM`) leaks into
//! its shells.

/// Variables passed from paddock's environment to the host. Everything
/// else is dropped.
const KEPT_VARS: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "LANG",
    "TMPDIR",
    "TMUX_TMPDIR",
    // `paddock hook` in a terminal needs paddock's own state dir and socket.
    "XDG_STATE_HOME",
    "PADDOCK_TMUX_SOCKET",
    // Tests only: the native server's wait for a silent restored shell
    // (it keeps it from its shells).
    "PADDOCK_RESUME_WAIT_MS",
];

/// Set to `on` in every terminal, so Antigravity CLI reports its state
/// (OSC 7501) there.
pub const AGY_STATUS_VAR: &str = "AGY_CLI_OSC7501";

/// `TERM` the host itself, and with the native backend every terminal,
/// runs with.
pub const TERM: &str = "xterm-256color";

/// The environment the host runs with: an allowlist of the given variables,
/// sorted, then `TERM`.
pub fn clean_env(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(k, _)| KEPT_VARS.contains(&k.as_str()) || k.starts_with("LC_"))
        .collect();
    kept.sort();
    kept.push(("TERM".into(), TERM.into()));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn clean_env_keeps_only_the_allowlist() {
        let env = clean_env(vars(&[
            ("HOME", "/h"),
            ("SUPACODE_PANE", "1"),
            ("TERM_PROGRAM", "ghostty"),
            ("TERM", "xterm-ghostty"),
            ("TMUX", "/tmp/x,1,0"),
            ("LC_ALL", "C"),
            ("PATH", "/bin"),
            ("XDG_STATE_HOME", "/s"),
            ("PADDOCK_TMUX_SOCKET", "p"),
        ]));
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        // Hooks in the terminals must find the same state and socket.
        assert_eq!(
            keys,
            [
                "HOME",
                "LC_ALL",
                "PADDOCK_TMUX_SOCKET",
                "PATH",
                "XDG_STATE_HOME",
                "TERM"
            ]
        );
        assert!(env.contains(&("TERM".into(), "xterm-256color".into())));
    }
}
