//! paddock as the native server's client: one short connection per
//! request, and one long one for the attached space ([`Attached`]). Only
//! making a space starts the server; every other request finds none
//! running and says so ([`NotRunning`]), or, if it only asks, has nothing
//! for an answer. A server this paddock cannot talk to is an error.

use super::protocol::{self, Answer, Closed, Ending, Frame, Message, Notice, Refusal, Request};
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

/// No server runs (nothing listens on its socket).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotRunning;

impl std::fmt::Display for NotRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the paddock server is not running")
    }
}

impl std::error::Error for NotRunning {}

/// The server could not do what it was asked: why.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// Whether `e` says no server could answer: none runs, or it was ending
/// or went away as it was asked (the next request starts a new one).
fn no_server(e: &anyhow::Error) -> bool {
    e.downcast_ref::<NotRunning>().is_some()
        || e.downcast_ref::<Ending>().is_some()
        || e.downcast_ref::<Closed>().is_some()
}

/// A closure taking the value of answer `variant`, and giving any other
/// answer back.
macro_rules! pick {
    ($variant:path) => {
        |answer| match answer {
            $variant(value) => Ok(value),
            other => Err(other),
        }
    };
}

/// The most input one frame carries.
const INPUT_FRAME: usize = 64 << 10;
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
        let mut stream = UnixStream::connect(&socket).context(NotRunning)?;
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

    pub fn spaces(&self) -> Result<Vec<ListedSpace>> {
        let spaces = self.query(Request::Spaces, pick!(Answer::Spaces))?;
        Ok(spaces.unwrap_or_default())
    }

    pub fn new_space(&self, name: &str, cwd: &Path, size: (u16, u16)) -> Result<SpaceId> {
        let request = Request::NewSpace {
            name: name.into(),
            cwd: cwd.into(),
            size,
        };
        self.start()?;
        let answer = match self.request(request.clone()) {
            // Its last terminal had just ended: the next server takes it.
            Err(e) if no_server(&e) => {
                std::thread::sleep(POLL);
                self.start()?;
                self.request(request)?
            }
            answer => answer?,
        };
        match answer {
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

    /// The server's answer to `request`, taken by `pick`; `None` if no
    /// server runs or it could not say (no such space). An error if one
    /// runs that this paddock cannot talk to, or it answers out of turn.
    fn query<T>(
        &self,
        request: Request,
        pick: impl FnOnce(Answer) -> Result<T, Answer>,
    ) -> Result<Option<T>> {
        match self.request(request) {
            Ok(answer) => pick(answer).map(Some).map_err(|a| unexpected(&a)),
            Err(e) if no_server(&e) || e.downcast_ref::<Refused>().is_some() => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn root(&self, space: &SpaceId) -> Result<Option<String>> {
        let root = self.query(Request::Root(space.clone()), pick!(Answer::Text))?;
        Ok(root.flatten())
    }

    pub fn terminal_dir(&self, terminal: &TerminalId) -> Result<Option<String>> {
        let request = Request::TerminalDir(terminal.clone());
        Ok(self.query(request, pick!(Answer::Text))?.flatten())
    }

    pub fn structure(&self, space: &SpaceId) -> Result<Structure> {
        match self.request(Request::Structure(space.clone()))? {
            Answer::Structure(s) => Ok(s),
            other => Err(unexpected(&other)),
        }
    }

    pub fn space_terminals(&self, space: &SpaceId) -> Result<Vec<SpaceTerminal>> {
        let request = Request::SpaceTerminals(space.clone());
        Ok(self
            .query(request, pick!(Answer::Terminals))?
            .unwrap_or_default())
    }

    pub fn space_of(&self, terminal: &TerminalId) -> Result<Option<SpaceId>> {
        let request = Request::SpaceOf(terminal.clone());
        Ok(self.query(request, pick!(Answer::SpaceOf))?.flatten())
    }

    pub fn terminal_commands(&self) -> Result<Option<HashMap<TerminalId, String>>> {
        self.query(Request::TerminalCommands, pick!(Answer::Commands))
    }

    pub fn terminal_pids(&self) -> Result<HashMap<u32, TerminalId>> {
        let pids = self.query(Request::TerminalPids, pick!(Answer::Pids))?;
        Ok(pids.unwrap_or_default())
    }

    pub fn terminal_details(&self) -> Result<Vec<TerminalDetails>> {
        let details = self.query(Request::TerminalDetails, pick!(Answer::Details))?;
        Ok(details.unwrap_or_default())
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
            match protocol::read(&mut stream, protocol::FROM_SERVER)? {
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
            // The space's end is the server's notice; a connection that
            // ends without it (let go, or the server gone) is lost.
            while let Ok(Some(frame)) = protocol::read(&mut reader, protocol::FROM_SERVER) {
                let event = match frame {
                    Frame::Bytes { terminal, data } => Event::Output { terminal, data },
                    Frame::Message(Message::Notice(Notice::Changed)) => Event::Changed,
                    Frame::Message(Message::Notice(Notice::SpacesChanged)) => Event::SpacesChanged,
                    Frame::Message(Message::Notice(Notice::InputDropped { terminal, why })) => {
                        Event::InputDropped { terminal, why }
                    }
                    Frame::Message(Message::Notice(Notice::Ended)) => {
                        deliver(Event::Ended);
                        return;
                    }
                    Frame::Message(_) => continue,
                };
                if !deliver(event) {
                    return;
                }
            }
            deliver(Event::Lost);
        });
        Ok((Attached { stream }, structure, replays))
    }
}

/// The reply that comes next on `stream`.
fn read_reply(stream: &mut UnixStream) -> Result<Answer> {
    match protocol::read(stream, protocol::FROM_SERVER)? {
        Some(Frame::Message(Message::Reply(reply))) => reply.map_err(|refusal| match refusal {
            Refusal::Ending => Ending.into(),
            Refusal::Failed(why) => Refused(why).into(),
        }),
        Some(other) => bail!("expected a reply from the paddock server, got {other:?}"),
        None => Err(anyhow::Error::new(Closed).context("no reply from the paddock server")),
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
    /// Send `bytes` to `terminal`'s program, in frames of at most
    /// [`INPUT_FRAME`] bytes: a paste of any size fits the server's frame
    /// limit, and the server sees the program's progress through it.
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(INPUT_FRAME) {
            let frame = Frame::Bytes {
                terminal: terminal.clone(),
                data: chunk.to_vec(),
            };
            protocol::write(&mut self.stream, &frame)?;
        }
        Ok(())
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
