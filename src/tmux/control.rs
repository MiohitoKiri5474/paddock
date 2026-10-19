//! A tmux control-mode client (`tmux -C attach`) for one terminal.

use super::protocol::{Message, Parser};
use super::server::Server;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// `send-keys -H` arguments per command; keeps command lines short.
const SEND_CHUNK: usize = 256;
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ControlClient {
    child: Child,
    stdin: ChildStdin,
}

impl ControlClient {
    /// Attach to `session`. Messages are delivered to `tx`, wrapped by `wrap`,
    /// so they can share a channel with other events.
    pub fn attach<T: Send + 'static>(
        server: &Server,
        session: &str,
        tx: Sender<T>,
        wrap: fn(Message) -> T,
    ) -> Result<Self> {
        let mut child = server
            .command()
            .args(["-C", "attach-session", "-t", session])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start the tmux control client")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        std::thread::spawn(move || read_loop(BufReader::with_capacity(1 << 16, stdout), &tx, wrap));
        Ok(Self { child, stdin })
    }

    /// Send one command line. Several commands joined with ` ; ` run back to
    /// back, with no pane output read in between.
    pub fn send(&mut self, line: &str) -> Result<()> {
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush().context("tmux control client went away")
    }

    /// Write raw bytes to a pane. `send-keys -H` passes them through unchanged
    /// (tmux treats them as literal keys), so all encoding is ours.
    pub fn send_bytes(&mut self, pane: &str, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(SEND_CHUNK) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            self.send(&format!("send-keys -t {pane} -H {}", hex.join(" ")))?;
        }
        Ok(())
    }

    /// Tell tmux the size this client shows the session at.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.send(&format!("refresh-client -C {cols}x{rows}"))
    }

    /// Detach; the session keeps running.
    pub fn detach(mut self) {
        // The client may already be gone; nothing to do then.
        let _ = self.send("detach-client");
        let _ = self.child.wait();
    }
}

fn read_loop<T>(mut r: impl BufRead, tx: &Sender<T>, wrap: fn(Message) -> T) {
    let mut parser = Parser::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if let Some(msg) = parser.line(&line) {
            let exit = msg == Message::Exit;
            if tx.send(wrap(msg)).is_err() || exit {
                return;
            }
        }
    }
    let _ = tx.send(wrap(Message::Exit));
}

/// Wait for the next reply on `rx`, discarding everything else that arrives
/// first (including pane output). `unwrap` extracts control messages from the
/// channel's event type.
pub fn wait_reply<T>(rx: &Receiver<T>, unwrap: fn(T) -> Option<Message>) -> Result<Vec<Vec<u8>>> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = rx
            .recv_timeout(left)
            .context("timed out waiting for tmux")?;
        match unwrap(event) {
            Some(Message::Reply { ok: true, lines }) => return Ok(lines),
            Some(Message::Reply { ok: false, lines }) => {
                bail!("tmux error: {}", String::from_utf8_lossy(&lines.concat()))
            }
            Some(Message::Exit) => bail!("tmux control client exited"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn read_loop_turns_lines_into_messages_and_ends_with_exit() {
        let input: &[u8] =
            b"%begin 1 1 0\n%end 1 1 0\n%output %1 a\\015\n%begin 1 2 1\nx\n%end 1 2 1\n";
        let (tx, rx) = mpsc::channel();
        read_loop(input, &tx, |m| m);
        let got: Vec<Message> = rx.try_iter().collect();
        assert_eq!(
            got,
            vec![
                Message::Output {
                    pane: "%1".into(),
                    data: b"a\r".to_vec()
                },
                Message::Reply {
                    ok: true,
                    lines: vec![b"x".to_vec()]
                },
                Message::Exit,
            ]
        );
    }

    #[test]
    fn wait_reply_skips_output_and_reports_errors() {
        let (tx, rx) = mpsc::channel();
        tx.send(Message::Output {
            pane: "%1".into(),
            data: vec![1],
        })
        .unwrap();
        tx.send(Message::Reply {
            ok: true,
            lines: vec![b"r".to_vec()],
        })
        .unwrap();
        tx.send(Message::Reply {
            ok: false,
            lines: vec![b"bad".to_vec()],
        })
        .unwrap();
        tx.send(Message::Exit).unwrap();
        assert_eq!(wait_reply(&rx, Some).unwrap(), vec![b"r".to_vec()]);
        assert!(wait_reply(&rx, Some)
            .unwrap_err()
            .to_string()
            .contains("bad"));
        assert!(wait_reply(&rx, Some).is_err());
    }
}
