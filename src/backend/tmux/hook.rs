//! Which of paddock's terminals a process runs in, from the variables tmux
//! sets in every pane.

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
}
