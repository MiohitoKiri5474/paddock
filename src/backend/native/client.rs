//! paddock as the native server's client: one short connection per
//! request, and one long one for the attached space ([`Attached`]). Only
//! making a space starts the server; every other request finds none
//! running and says so (or answers with nothing).

use super::protocol::{self, Answer, Frame, Message, Notice, Request};
use super::server::log_path;
use super::socket_path;
use crate::backend::env::clean_env;
use crate::backend::{
    Event, ListedSpace, Replay, SpaceId, SpaceTerminal, Structure, TabId, TerminalDetails,
    TerminalId,
};
use crate::paths::Paths;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a new server gets to start listening.
const START_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub struct Client {
    paths: Paths,
}

impl Client {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    /// A connection to the running server, past the hello; an error if
    /// none runs or it speaks another protocol version.
    fn connect(&self) -> Result<(UnixStream, u32)> {
        let socket = socket_path(&self.paths)?;
        let mut stream =
            UnixStream::connect(&socket).context("the paddock server is not running")?;
        let pid = protocol::hello(&mut stream, true)?;
        Ok((stream, pid))
    }

    fn request(&self, request: Request) -> Result<Answer> {
        let (mut stream, _) = self.connect()?;
        protocol::write(&mut stream, &Frame::Message(Message::Request(request)))?;
        read_reply(&mut stream)
    }

    /// Ok if no server runs, or one runs that speaks this paddock's
    /// protocol; else the error that says which version it speaks.
    pub fn check(&self) -> Result<()> {
        let socket = socket_path(&self.paths)?;
        match UnixStream::connect(socket) {
            // None runs (a socket left behind refuses).
            Err(_) => Ok(()),
            Ok(mut stream) => protocol::hello(&mut stream, true).map(drop),
        }
    }

    /// The server's instance id: its pid.
    pub fn instance(&self) -> Result<u32> {
        self.connect().map(|(_, pid)| pid)
    }

    /// Start the server unless one runs, and wait until it listens.
    fn start(&self) -> Result<()> {
        if self.connect().is_ok() {
            return Ok(());
        }
        self.check()?;
        let state_dir = &self.paths.state_dir;
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("cannot create {}", state_dir.display()))?;
        let log = log_path(state_dir);
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .with_context(|| format!("cannot open {}", log.display()))?;
        let exe = std::env::current_exe().context("cannot find paddock's own binary")?;
        let mut command = Command::new(exe);
        command
            .arg("server")
            .env_clear()
            .envs(clean_env(std::env::vars()))
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::from(out.try_clone()?))
            .stderr(Stdio::from(out));
        // SAFETY: setsid is async-signal-safe. A session of its own: the
        // server outlives paddock and its terminal.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().context("cannot start the paddock server")?;
        // Reap it whenever it ends, while paddock runs.
        std::thread::spawn(move || child.wait());
        let start = Instant::now();
        loop {
            match self.connect() {
                Ok(_) => return Ok(()),
                Err(e) if start.elapsed() > START_TIMEOUT => {
                    return Err(e).with_context(|| {
                        format!("the paddock server did not start (see {})", log.display())
                    })
                }
                Err(_) => std::thread::sleep(POLL),
            }
        }
    }

    pub fn spaces(&self) -> Vec<ListedSpace> {
        match self.request(Request::Spaces) {
            Ok(Answer::Spaces(spaces)) => spaces,
            _ => Vec::new(),
        }
    }

    pub fn new_space(&self, name: &str, cwd: &Path, size: (u16, u16)) -> Result<SpaceId> {
        self.start()?;
        let request = Request::NewSpace {
            name: name.into(),
            cwd: cwd.into(),
            size,
        };
        match self.request(request)? {
            Answer::Space(id) => Ok(id),
            other => Err(unexpected(&other)),
        }
    }

    pub fn space_name(&self, space: &SpaceId) -> Result<String> {
        match self.request(Request::SpaceName(space.clone()))? {
            Answer::Text(Some(name)) => Ok(name),
            other => Err(unexpected(&other)),
        }
    }

    fn done(&self, request: Request) -> Result<()> {
        match self.request(request)? {
            Answer::Done => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    pub fn rename_space(&self, space: &SpaceId, name: &str) -> Result<()> {
        self.done(Request::RenameSpace(space.clone(), name.into()))
    }

    pub fn close_space(&self, space: &SpaceId) -> Result<()> {
        self.done(Request::CloseSpace(space.clone()))
    }

    pub fn set_root(&self, space: &SpaceId, dir: &Path) -> Result<()> {
        self.done(Request::SetRoot(space.clone(), dir.into()))
    }

    pub fn rename_tab(&self, tab: &TabId, name: &str) -> Result<()> {
        self.done(Request::RenameTab(tab.clone(), name.into()))
    }

    pub fn select(&self, tab: &TabId, terminal: Option<&TerminalId>) -> Result<()> {
        self.done(Request::Select(tab.clone(), terminal.cloned()))
    }

    pub fn close_terminal(&self, terminal: &TerminalId) -> Result<()> {
        self.done(Request::CloseTerminal(terminal.clone()))
    }

    fn text(&self, request: Request) -> Option<String> {
        match self.request(request) {
            Ok(Answer::Text(text)) => text,
            _ => None,
        }
    }

    pub fn root(&self, space: &SpaceId) -> Option<String> {
        self.text(Request::Root(space.clone()))
    }

    pub fn terminal_dir(&self, terminal: &TerminalId) -> Option<String> {
        self.text(Request::TerminalDir(terminal.clone()))
    }

    pub fn structure(&self, space: &SpaceId) -> Result<Structure> {
        match self.request(Request::Structure(space.clone()))? {
            Answer::Structure(s) => Ok(s),
            other => Err(unexpected(&other)),
        }
    }

    pub fn space_terminals(&self, space: &SpaceId) -> Vec<SpaceTerminal> {
        match self.request(Request::SpaceTerminals(space.clone())) {
            Ok(Answer::Terminals(t)) => t,
            _ => Vec::new(),
        }
    }

    pub fn space_of(&self, terminal: &TerminalId) -> Option<SpaceId> {
        match self.request(Request::SpaceOf(terminal.clone())) {
            Ok(Answer::Space(id)) => Some(id),
            _ => None,
        }
    }

    pub fn terminal_commands(&self) -> Option<HashMap<TerminalId, String>> {
        match self.request(Request::TerminalCommands) {
            Ok(Answer::Commands(c)) => Some(c),
            _ => None,
        }
    }

    pub fn terminal_pids(&self) -> HashMap<u32, TerminalId> {
        match self.request(Request::TerminalPids) {
            Ok(Answer::Pids(p)) => p,
            _ => HashMap::new(),
        }
    }

    pub fn terminal_details(&self) -> Vec<TerminalDetails> {
        match self.request(Request::TerminalDetails) {
            Ok(Answer::Details(d)) => d,
            _ => Vec::new(),
        }
    }

    /// End every terminal and the server.
    #[cfg(test)]
    pub fn stop(&self) {
        let _ = self.request(Request::Stop);
    }

    /// Attach space `space` at `size`: its structure and each terminal's
    /// snapshot, then every event, in order, to `deliver`.
    pub fn attach(
        &self,
        space: &SpaceId,
        size: (u16, u16),
        mut deliver: impl FnMut(Event) -> bool + Send + 'static,
    ) -> Result<(Attached, Structure, Vec<Replay>)> {
        let (mut stream, _) = self.connect()?;
        let request = Request::Attach {
            space: space.clone(),
            size,
        };
        protocol::write(&mut stream, &Frame::Message(Message::Request(request)))?;
        let (structure, terminals) = match read_reply(&mut stream)? {
            Answer::Attached {
                structure,
                terminals,
            } => (structure, terminals),
            other => return Err(unexpected(&other)),
        };
        let mut replays = Vec::new();
        for (terminal, size) in terminals {
            match protocol::read(&mut stream)? {
                Some(Frame::Bytes { terminal: t, data }) if t == terminal => replays.push(Replay {
                    terminal,
                    size,
                    bytes: data,
                }),
                other => {
                    bail!("expected {terminal}'s snapshot from the paddock server, got {other:?}")
                }
            }
        }
        let mut reader = stream.try_clone()?;
        std::thread::spawn(move || {
            // The server's end, or the connection's (detached, let go for
            // another client, or the server gone), ends the space here.
            while let Ok(Some(frame)) = protocol::read(&mut reader) {
                let event = match frame {
                    Frame::Bytes { terminal, data } => Event::Output { terminal, data },
                    Frame::Message(Message::Notice(Notice::Changed)) => Event::Changed,
                    Frame::Message(Message::Notice(Notice::SpacesChanged)) => Event::SpacesChanged,
                    Frame::Message(Message::Notice(Notice::Ended)) => break,
                    Frame::Message(_) => continue,
                };
                if !deliver(event) {
                    return;
                }
            }
            deliver(Event::Ended);
        });
        Ok((Attached { stream }, structure, replays))
    }
}

/// The reply that comes next on `stream`.
fn read_reply(stream: &mut UnixStream) -> Result<Answer> {
    match protocol::read(stream)? {
        Some(Frame::Message(Message::Reply(reply))) => reply.map_err(|e| anyhow!(e)),
        Some(other) => bail!("expected a reply from the paddock server, got {other:?}"),
        None => bail!("the paddock server closed the connection"),
    }
}

fn unexpected(answer: &Answer) -> anyhow::Error {
    anyhow!("unexpected answer from the paddock server: {answer:?}")
}

/// paddock's connection to its attached space on the server.
pub struct Attached {
    stream: UnixStream,
}

impl Attached {
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        let frame = Frame::Bytes {
            terminal: terminal.clone(),
            data: bytes.to_vec(),
        };
        protocol::write(&mut self.stream, &frame)
    }

    pub fn resize(&mut self, size: (u16, u16)) -> Result<()> {
        let frame = Frame::Message(Message::Request(Request::Resize(size)));
        protocol::write(&mut self.stream, &frame)
    }

    /// Leave the space running on the server.
    pub fn detach(mut self) {
        let frame = Frame::Message(Message::Request(Request::Detach));
        // The server may be gone already; nothing to do then.
        let _ = protocol::write(&mut self.stream, &frame);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
