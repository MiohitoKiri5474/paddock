//! Which of paddock's terminals a process runs in, from the variables the
//! native server sets in every terminal, and those variables for one.

use crate::backend::{HookTerminal, TerminalId};
use std::path::Path;

/// The terminal's id.
const PANE: &str = "PADDOCK_PANE";
/// The path of the server's socket.
const SOCKET: &str = "PADDOCK_SOCKET";
/// The server's instance id, which names its hook event log.
const INSTANCE: &str = "PADDOCK_INSTANCE";

/// The terminal a process runs in, from `$PADDOCK_PANE`, `$PADDOCK_SOCKET`
/// and `$PADDOCK_INSTANCE` as `var` reads them: `None` unless all are set
/// and the socket is `socket`, the server's under paddock's state dir.
pub fn terminal(var: impl Fn(&str) -> Option<String>, socket: &Path) -> Option<HookTerminal> {
    let set = |k: &str| var(k).filter(|v| !v.is_empty());
    let (pane, path) = (set(PANE)?, set(SOCKET)?);
    let instance = set(INSTANCE)?.parse().ok()?;
    (Path::new(&path) == socket).then(|| HookTerminal {
        instance,
        terminal: TerminalId::new(pane),
    })
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

    const SOCK: &str = "/Users/me/.local/state/paddock/server.sock";

    fn read(vars: &[(&str, &str)]) -> Option<(u32, String)> {
        let var = |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        };
        terminal(var, Path::new(SOCK)).map(|t| (t.instance, t.terminal.as_str().to_string()))
    }

    #[test]
    fn a_terminal_counts_only_on_paddocks_socket_with_every_variable() {
        let ours = [(PANE, "%3"), (SOCKET, SOCK), (INSTANCE, "77")];
        assert_eq!(read(&ours), Some((77, "%3".into())));
        let other = [
            (PANE, "%3"),
            (SOCKET, "/tmp/x/server.sock"),
            (INSTANCE, "77"),
        ];
        assert_eq!(read(&other), None, "another server");
        assert_eq!(read(&[(PANE, ""), (SOCKET, SOCK), (INSTANCE, "77")]), None);
        assert_eq!(read(&[(PANE, "%3"), (SOCKET, SOCK), (INSTANCE, "x")]), None);
        assert_eq!(read(&[(PANE, "%3"), (SOCKET, SOCK)]), None, "no instance");
        assert_eq!(read(&[]), None, "outside paddock");
    }

    #[test]
    fn a_terminals_variables_read_back_as_that_terminal() {
        let id = TerminalId::new("%9");
        let vars = env(Path::new(SOCK), 4242, &id);
        let var = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let t = terminal(var, Path::new(SOCK)).expect("a paddock terminal");
        assert_eq!((t.instance, t.terminal), (4242, id));
    }
}
