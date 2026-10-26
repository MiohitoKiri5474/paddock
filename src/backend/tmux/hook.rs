//! Which of paddock's terminals a process runs in, from the variables tmux
//! sets in every pane, and those variables for a pane.

use crate::backend::{HookTerminal, TerminalId};
use std::path::Path;

/// The terminal a process runs in, from `$TMUX` (`socket,pid,index`) and
/// `$TMUX_PANE`, as `var` reads them: `None` unless the socket is paddock's
/// (`socket` is its name). The server's pid is its instance id.
pub fn terminal(var: impl Fn(&str) -> Option<String>, socket: &str) -> Option<HookTerminal> {
    let (tmux, pane) = (var("TMUX")?, var("TMUX_PANE")?);
    let mut parts = tmux.rsplitn(3, ',');
    let _index = parts.next()?;
    let instance = parts.next()?.parse().ok()?;
    let path = parts.next()?;
    let ours = Path::new(path).file_name()? == socket;
    let pane_ok = pane.starts_with('%') && pane.len() > 1;
    (ours && pane_ok).then(|| HookTerminal {
        instance,
        terminal: TerminalId::new(pane),
    })
}

/// The variables tmux sets in pane `pane` of the server whose socket is
/// `socket_path` and whose pid is `instance`: what [`terminal`] reads.
pub fn env(socket_path: &str, instance: u32, pane: &TerminalId) -> Vec<(String, String)> {
    vec![
        ("TMUX".into(), format!("{socket_path},{instance},0")),
        ("TMUX_PANE".into(), pane.as_str().into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(tmux: &str, pane: &str) -> Option<(u32, String)> {
        let var = |k: &str| match k {
            "TMUX" => Some(tmux.to_string()),
            "TMUX_PANE" => Some(pane.to_string()),
            _ => None,
        };
        terminal(var, "paddock").map(|t| (t.instance, t.terminal.as_str().to_string()))
    }

    #[test]
    fn a_terminal_counts_only_on_paddocks_socket() {
        let t = "/private/tmp/tmux-501/paddock,4242,0";
        assert_eq!(read(t, "%3"), Some((4242, "%3".into())));
        assert_eq!(read("/tmp/tmux-501/default,1,0", "%3"), None);
        assert_eq!(read(t, ""), None);
        assert_eq!(read("", "%3"), None);
        assert_eq!(read("garbage", "%3"), None);
        assert_eq!(terminal(|_| None, "paddock"), None, "outside tmux");
    }

    #[test]
    fn a_panes_variables_read_back_as_that_pane() {
        let pane = TerminalId::new("%7");
        let vars = env("/private/tmp/tmux-501/paddock", 4242, &pane);
        let var = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let t = terminal(var, "paddock").expect("a paddock terminal");
        assert_eq!((t.instance, t.terminal), (4242, pane));
    }
}
