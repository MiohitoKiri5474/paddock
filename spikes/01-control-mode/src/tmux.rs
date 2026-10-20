//! Private tmux server + control-mode client.
//!
//! Throwaway spike code (ticket 01). Everything goes through `tmux -L <socket>`,
//! never the user's default server.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

pub const SOCKET: &str = "paddock-spike";

/// Environment variables that must not leak from the launching terminal app.
const STRIPPED_PREFIXES: &[&str] = &["SUPACODE_", "TERM_PROGRAM", "TMUX", "GHOSTTY_", "ITERM_", "KITTY_", "WEZTERM_"];

pub fn conf_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tmux.conf")
}

/// A one-shot tmux CLI call on the private socket.
pub fn tmux(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .arg("-L")
        .arg(SOCKET)
        .args(args)
        .output()
        .context("spawn tmux")?;
    if !out.status.success() {
        bail!("tmux {:?}: {}", args, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn tmux_bytes(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("tmux").arg("-L").arg(SOCKET).args(args).output()?;
    if !out.status.success() {
        bail!("tmux {:?}: {}", args, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

pub fn has_session(name: &str) -> bool {
    tmux(&["has-session", "-t", name]).is_ok()
}

/// Create a session on the private server with a clean environment.
/// Starts the server (with our own config) if it is not running.
pub fn new_session(name: &str, cwd: &Path, cols: u16, rows: u16, cmd: Option<&str>) -> Result<()> {
    let mut c = Command::new("tmux");
    c.env_clear();
    for (k, v) in std::env::vars() {
        if STRIPPED_PREFIXES.iter().any(|p| k.starts_with(p)) {
            continue;
        }
        if matches!(k.as_str(), "HOME" | "PATH" | "SHELL" | "USER" | "LOGNAME" | "LANG" | "LC_ALL" | "LC_CTYPE" | "TMPDIR") {
            c.env(k, v);
        }
    }
    c.env("TERM", "xterm-256color");
    c.args(["-L", SOCKET, "-f"]).arg(conf_path());
    c.args(["new-session", "-d", "-s", name, "-x", &cols.to_string(), "-y", &rows.to_string(), "-c"]);
    c.arg(cwd);
    if let Some(cmd) = cmd {
        c.arg(cmd);
    }
    let out = c.output()?;
    if !out.status.success() {
        bail!("new-session: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn kill_server() {
    let _ = tmux(&["kill-server"]);
}

/// Decode tmux control-mode `%output` escaping: `\ooo` octal for bytes < 32 and `\\`.
pub fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' && i + 4 <= data.len() {
            let d = &data[i + 1..i + 4];
            if d.iter().all(|b| (b'0'..=b'7').contains(b)) {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
                continue;
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

#[derive(Debug)]
pub enum Event {
    Output { pane: String, data: Vec<u8> },
    /// Reply to a command this client sent (`%begin ... 1` blocks).
    Reply { ok: bool, lines: Vec<Vec<u8>> },
    Notify(String),
    Exit,
}

pub struct Control {
    child: Child,
    stdin: ChildStdin,
}

impl Control {
    /// Attach a control-mode client. Events are sent to `tx` wrapped by `wrap`.
    pub fn attach<T: Send + 'static>(
        session: &str,
        tx: Sender<T>,
        wrap: fn(Event) -> T,
    ) -> Result<Self> {
        let mut child = Command::new("tmux")
            .args(["-L", SOCKET, "-C", "attach-session", "-t", session])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn control client")?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        thread::spawn(move || read_loop(BufReader::with_capacity(1 << 16, stdout), tx, wrap));
        Ok(Self { child, stdin })
    }

    pub fn send(&mut self, cmd: &str) -> Result<()> {
        self.stdin.write_all(cmd.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Send raw bytes to a pane with `send-keys -H` (tmux writes them as literal keys).
    pub fn send_bytes(&mut self, pane: &str, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(256) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            self.send(&format!("send-keys -t {pane} -H {}", hex.join(" ")))?;
        }
        Ok(())
    }

    pub fn detach(mut self) {
        let _ = self.send("detach-client");
        let _ = self.child.wait();
    }
}

fn read_loop<T>(mut r: impl BufRead, tx: Sender<T>, wrap: fn(Event) -> T) {
    let mut line = Vec::new();
    // Some(flags) while inside a %begin/%end block.
    let mut block: Option<(bool, Vec<Vec<u8>>)> = None;
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if let Some((mine, lines)) = block.as_mut() {
            let end = line.starts_with(b"%end ");
            let err = line.starts_with(b"%error ");
            if end || err {
                let mine = *mine;
                let lines = std::mem::take(lines);
                block = None;
                if mine && tx.send(wrap(Event::Reply { ok: end, lines })).is_err() {
                    return;
                }
            } else {
                lines.push(line.clone());
            }
            continue;
        }
        let ev = if let Some(rest) = line.strip_prefix(b"%output ") {
            let sp = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
            let pane = String::from_utf8_lossy(&rest[..sp]).into_owned();
            let data = unescape(rest.get(sp + 1..).unwrap_or(&[]));
            Event::Output { pane, data }
        } else if line.starts_with(b"%begin ") {
            // %begin <time> <number> <flags>; flags 1 = command from this client.
            let mine = line.ends_with(b" 1");
            block = Some((mine, Vec::new()));
            continue;
        } else if line.starts_with(b"%exit") {
            Event::Exit
        } else {
            Event::Notify(String::from_utf8_lossy(&line).into_owned())
        };
        let exit = matches!(ev, Event::Exit);
        if tx.send(wrap(ev)).is_err() || exit {
            return;
        }
    }
    let _ = tx.send(wrap(Event::Exit));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_octal_and_passthrough() {
        assert_eq!(unescape(br"a\033[1mb\015\012"), b"a\x1b[1mb\r\n");
        assert_eq!(unescape(br"\134"), b"\\");
        assert_eq!(unescape("✳".as_bytes()), "✳".as_bytes());
        assert_eq!(unescape(br"\03"), br"\03");
    }
}
