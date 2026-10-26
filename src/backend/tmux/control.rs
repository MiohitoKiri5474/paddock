//! A tmux control-mode client (`tmux -C attach`) for one session: it
//! carries every pane's output and input.

use super::protocol::{self, Message, Parser};
use super::seed;
use super::server::Server;
use crate::backend::{Event, Replay, Structure};
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `send-keys -H` arguments per command; keeps command lines short.
const SEND_CHUNK: usize = 256;
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the reader thread delivers messages. Returns false once nobody listens.
type Sink = Box<dyn FnMut(Message) -> bool + Send>;

/// Attach to session `session` at `size` and read every pane's screen and
/// history, floats' too, so a hidden float's screen survives reopening.
/// Afterwards every message that means something to paddock goes, in
/// order, to `deliver` as an [`Event`].
pub fn attach(
    server: &Server,
    session: &str,
    (cols, rows): (u16, u16),
    mut deliver: impl FnMut(Event) -> bool + Send + 'static,
) -> Result<(ControlClient, Structure, Vec<Replay>)> {
    let (mut control, rx) = ControlClient::attach(server, session)?;
    control.resize(cols, rows)?;
    wait_reply(&rx)?;
    // Read after the resize, so the panes have their final sizes.
    let structure = server.structure(session)?;
    let panes: Vec<_> = structure
        .splits()
        .map(|s| (s.id.clone(), (s.rect.width, s.rect.height)))
        .collect();
    let ids: Vec<&str> = panes.iter().map(|(p, _)| p.as_str()).collect();
    let seeds = seed::fetch(&mut control, &rx, &ids)?;
    control.forward(&rx, move |msg| {
        protocol::event(msg).is_none_or(&mut deliver)
    });
    let replays = panes
        .into_iter()
        .zip(seeds)
        .map(|((terminal, size), seed)| {
            Ok(Replay {
                terminal,
                size,
                bytes: seed::stream(&seed)?,
            })
        })
        .collect::<Result<_>>()?;
    Ok((control, structure, replays))
}

pub struct ControlClient {
    child: Child,
    stdin: ChildStdin,
    sink: Arc<Mutex<Sink>>,
}

impl ControlClient {
    /// Attach to `session`. Messages first arrive on the returned private
    /// receiver, so the caller can wait for its own replies without consuming
    /// anything that belongs to other terminals; [`Self::forward`] then hands
    /// them on.
    pub fn attach(server: &Server, session: &str) -> Result<(Self, Receiver<Message>)> {
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
        let (tx, rx) = mpsc::channel();
        let sink: Arc<Mutex<Sink>> = Arc::new(Mutex::new(Box::new(move |m| tx.send(m).is_ok())));
        let reader_sink = Arc::clone(&sink);
        std::thread::spawn(move || {
            read_loop(BufReader::with_capacity(1 << 16, stdout), &reader_sink);
        });
        Ok((Self { child, stdin, sink }, rx))
    }

    /// From now on deliver messages to `deliver`, after first passing it
    /// everything still queued on `pending`, so nothing is lost or reordered.
    pub fn forward(
        &self,
        pending: &Receiver<Message>,
        deliver: impl FnMut(Message) -> bool + Send + 'static,
    ) {
        redirect(&self.sink, pending, Box::new(deliver));
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

    /// Give window `window` a size of its own on this client, apart from
    /// the session's (later session resizes leave it alone).
    pub fn resize_window(&mut self, window: &str, cols: u16, rows: u16) -> Result<()> {
        self.send(&format!("refresh-client -C {window}:{cols}x{rows}"))
    }

    /// Detach; the session keeps running.
    pub fn detach(mut self) {
        // The client may already be gone; nothing to do then.
        let _ = self.send("detach-client");
        let _ = self.child.wait();
    }
}

/// Swap the sink while holding its lock: the reader thread waits, so queued
/// messages are delivered before any new one.
fn redirect(sink: &Mutex<Sink>, pending: &Receiver<Message>, mut deliver: Sink) {
    let mut current = sink.lock().unwrap_or_else(|e| e.into_inner());
    for m in pending.try_iter() {
        deliver(m);
    }
    *current = deliver;
}

fn read_loop(mut r: impl BufRead, sink: &Mutex<Sink>) {
    let deliver = |m: Message| (sink.lock().unwrap_or_else(|e| e.into_inner()))(m);
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
            if !deliver(msg) || exit {
                return;
            }
        }
    }
    deliver(Message::Exit);
}

/// Wait for the next reply on `rx`, discarding everything else that arrives
/// first (including pane output).
pub fn wait_reply(rx: &Receiver<Message>) -> Result<Vec<Vec<u8>>> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = rx
            .recv_timeout(left)
            .context("timed out waiting for tmux")?;
        match msg {
            Message::Reply { ok: true, lines } => return Ok(lines),
            Message::Reply { ok: false, lines } => {
                bail!("tmux error: {}", String::from_utf8_lossy(&lines.concat()))
            }
            Message::Exit => bail!("tmux control client exited"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_loop_turns_lines_into_messages_and_ends_with_exit() {
        let input: &[u8] =
            b"%begin 1 1 0\n%end 1 1 0\n%output %1 a\\015\n%begin 1 2 1\nx\n%end 1 2 1\n";
        let (tx, rx) = mpsc::channel();
        let sink: Mutex<Sink> = Mutex::new(Box::new(move |m| tx.send(m).is_ok()));
        read_loop(input, &sink);
        drop(sink);
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
    fn redirect_delivers_queued_messages_first() {
        let (private_tx, private_rx) = mpsc::channel();
        let sink: Mutex<Sink> = Mutex::new(Box::new(move |m| private_tx.send(m).is_ok()));
        let out = |n: u8| Message::Output {
            pane: "%1".into(),
            data: vec![n],
        };
        (sink.lock().unwrap())(out(1));
        (sink.lock().unwrap())(out(2));
        let (shared_tx, shared_rx) = mpsc::channel();
        redirect(
            &sink,
            &private_rx,
            Box::new(move |m| shared_tx.send(m).is_ok()),
        );
        (sink.lock().unwrap())(out(3));
        let got: Vec<Message> = shared_rx.try_iter().collect();
        assert_eq!(got, vec![out(1), out(2), out(3)]);
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
        assert_eq!(wait_reply(&rx).unwrap(), vec![b"r".to_vec()]);
        assert!(wait_reply(&rx).unwrap_err().to_string().contains("bad"));
        assert!(wait_reply(&rx).is_err());
    }
}
