//! Which of paddock's terminals a process runs in, from the variables the
//! native server sets in every terminal, and those variables for one.

use crate::backend::hook_vars::{digits, terminal_id_ok};
use crate::backend::{HookTerminal, Instance, TerminalId};
use crate::config::BackendKind;
use std::path::{Path, PathBuf};

/// The terminal's id.
const PANE: &str = "PADDOCK_PANE";
/// The path of the server's socket.
const SOCKET: &str = "PADDOCK_SOCKET";
/// The server's instance id, which names its hook event log.
const INSTANCE: &str = "PADDOCK_INSTANCE";

/// The terminal a process runs in, from `$PADDOCK_PANE`, `$PADDOCK_SOCKET`
/// and `$PADDOCK_INSTANCE` as `var` reads them: `None` unless all are set
/// and well formed (a terminal id `%N`, an instance id above 0 in plain
/// digits), and the socket is `socket`, the server's under paddock's state
/// dir, written either way (symlinks resolved or not).
pub fn terminal(var: impl Fn(&str) -> Option<String>, socket: &Path) -> Option<HookTerminal> {
    let (pane, path) = (var(PANE)?, var(SOCKET)?);
    let id = digits(&var(INSTANCE)?).filter(|&id| id > 0)?;
    let pane_ok = terminal_id_ok(&pane);
    (pane_ok && same_socket(Path::new(&path), socket)).then(|| HookTerminal {
        instance: Instance {
            kind: BackendKind::Native,
            id,
        },
        terminal: TerminalId::new(pane),
    })
}

/// Whether `a` and `b` name the same socket: as written (no system call:
/// the usual case), else with the symlinks in their directories resolved
/// (`/var` and `/private/var`).
fn same_socket(a: &Path, b: &Path) -> bool {
    a == b || resolved(a).is_some_and(|a| resolved(b) == Some(a))
}

/// `path` with its directory's symlinks resolved; `None` if that fails.
fn resolved(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?.canonicalize().ok()?;
    Some(dir.join(path.file_name()?))
}

/// The variables of terminal `terminal` in the server listening on
/// `socket` whose instance id is `instance`: what [`terminal`] reads.
pub fn env(socket: &Path, instance: u32, terminal: &TerminalId) -> Vec<(String, String)> {
    vec![
        (PANE.into(), terminal.as_str().into()),
        (SOCKET.into(), socket.to_string_lossy().into_owned()),
        (INSTANCE.into(), instance.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::test_vars as vars;

    const SOCK: &str = "/Users/me/.local/state/paddock/server.sock";

    fn read(pairs: &[(&str, &str)]) -> Option<(u32, String)> {
        let t = terminal(vars(pairs), Path::new(SOCK))?;
        assert_eq!(t.instance.kind, BackendKind::Native);
        Some((t.instance.id, t.terminal.as_str().to_string()))
    }

    fn with(pane: &str, socket: &str, instance: &str) -> Option<(u32, String)> {
        read(&[(PANE, pane), (SOCKET, socket), (INSTANCE, instance)])
    }

    #[test]
    fn a_terminal_counts_only_on_paddocks_socket_with_every_variable() {
        assert_eq!(with("%3", SOCK, "77"), Some((77, "%3".into())));
        assert_eq!(with("%3", "/tmp/x/server.sock", "77"), None, "another");
        assert_eq!(read(&[(PANE, "%3"), (SOCKET, SOCK)]), None, "no instance");
        assert_eq!(read(&[]), None, "outside paddock");
    }

    #[test]
    fn a_malformed_terminal_or_instance_id_counts_as_none() {
        for pane in ["", "%", "3", "%x", "%3 ", "%-3"] {
            assert_eq!(with(pane, SOCK, "77"), None, "pane {pane:?}");
        }
        for instance in ["", "0", "+77", "-77", " 77", "77x", "99999999999"] {
            assert_eq!(with("%3", SOCK, instance), None, "instance {instance:?}");
        }
    }

    #[test]
    fn a_socket_path_through_a_symlink_is_the_same_socket() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let ours = link.join("server.sock");
        let seen = real.join("server.sock").to_string_lossy().into_owned();
        let var = vars(&[(PANE, "%3"), (SOCKET, &seen), (INSTANCE, "77")]);
        assert!(terminal(&var, &ours).is_some(), "the link's target");
        let other = dir.path().join("other.sock");
        assert!(terminal(&var, &other).is_none(), "another socket there");
    }

    #[test]
    fn a_terminals_variables_read_back_as_that_terminal() {
        let id = TerminalId::new("%9");
        let var = vars(&env(Path::new(SOCK), 4242, &id));
        let t = terminal(var, Path::new(SOCK)).expect("a paddock terminal");
        assert_eq!((t.instance.id, t.terminal), (4242, id));
    }
}
