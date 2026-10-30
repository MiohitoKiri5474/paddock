//! paddock as the native server's client: one short connection per
//! request (but one kept for a border drag's many, see
//! [`Client::drag_border`]), and one long one for each attached space
//! ([`Attached`]). Only
//! making a space starts the server; every other request finds none
//! running and says so ([`NotRunning`]), or, if it only asks, has nothing
//! for an answer. A server this paddock cannot talk to is an error.

use super::axis::Axis;
use super::protocol::{
    self, Answer, Closed, Ending, Frame, Message, Notice, OtherVersion, Peer, Refusal, Request,
    Upgrade,
};
use super::server::log_path;
use super::{held, saved, socket_path, LOCK};
use crate::backend::border::{Anchor, Border};
use crate::backend::env::clean_env;
use crate::backend::{
    Event, FloatId, ListedSpace, Rebuilt, Replay, Restored, SpaceId, SpaceTerminal, Structure,
    TabId, TerminalDetails, TerminalId,
};
use crate::input::Direction;
use crate::paths::Paths;
use crate::terminal;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
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

/// The running server cannot hand over: its hello names a protocol
/// version from before ADR 0009 (`version`), or it gave no answer to an
/// upgrade (`None`): it is from before ADR 0009, or its new binary
/// failed once it ran (see `server.log`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoHandover {
    pub pid: u32,
    pub version: Option<u32>,
}

impl std::fmt::Display for NoHandover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pid = self.pid;
        match self.version {
            Some(version) => write!(
                f,
                "the paddock server (pid {pid}) speaks protocol version {version}, from a \
                 paddock that cannot hand its terminals over"
            ),
            None => write!(
                f,
                "the paddock server (pid {pid}) gave no answer to the upgrade: it may be too \
                 old to hand its terminals over (see server.log)"
            ),
        }
    }
}

impl std::error::Error for NoHandover {}

/// How an upgrade of a server that speaks another protocol version went
/// ([`Client::upgrade_other`]), told by what runs after it.
#[derive(Debug)]
pub enum Upgraded {
    /// The new binary runs as the server, its programs running on.
    Done,
    /// The server did not hand over, and runs on as it was: why. It can
    /// still be restarted.
    Unchanged(anyhow::Error),
    /// The handover failed after the server switched to the new binary:
    /// why. The server has ended with every program in it, and the next
    /// one rebuilds the layout it saved.
    Ended(anyhow::Error),
}

/// Whether `e` says no server could answer: none runs, or it was ending
/// or went away as it was asked (the next request starts a new one).
fn no_server(e: &anyhow::Error) -> bool {
    e.downcast_ref::<NotRunning>().is_some()
        || e.downcast_ref::<Ending>().is_some()
        || e.downcast_ref::<Closed>().is_some()
}

/// Whether `e` is a read that waited past its timeout.
fn timed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|e| {
        matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )
    })
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
/// How long a server asked to end for a restart gets to let its lock go.
const END_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a server gets to answer an upgrade: its probe of the new
/// binary has 10 s, and the new binary rebuilds every terminal.
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub struct Client {
    paths: Paths,
    /// The connection border drags share (see [`Self::drag_border`]).
    drag: Arc<Mutex<Option<UnixStream>>>,
}

impl Client {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            drag: Arc::default(),
        }
    }

    /// A connection to the running server, past the hello; an error if
    /// none runs or it speaks another protocol version.
    fn connect(&self) -> Result<(UnixStream, Peer)> {
        self.connect_within(None)
    }

    /// [`Self::connect`], each read on the connection within `timeout`
    /// (an error past it) if there is one.
    fn connect_within(&self, timeout: Option<Duration>) -> Result<(UnixStream, Peer)> {
        let socket = socket_path(&self.paths)?;
        let mut stream = UnixStream::connect(&socket).context(NotRunning)?;
        stream.set_read_timeout(timeout)?;
        let server = protocol::hello(&mut stream, true, 0)?;
        Ok((stream, server))
    }

    /// The running server, as its hello tells, read within `timeout`
    /// (an error past it); an error, as [`Self::connect`], if none runs
    /// or it speaks another protocol version.
    fn hello_within(&self, timeout: Duration) -> Result<Peer> {
        let socket = socket_path(&self.paths)?;
        let mut stream = UnixStream::connect(&socket).context(NotRunning)?;
        stream.set_read_timeout(Some(timeout))?;
        protocol::hello(&mut stream, true, 0)
    }

    fn request(&self, request: Request) -> Result<Answer> {
        self.request_within(request, None)
    }

    /// [`Self::request`], the hello and the reply each read within
    /// `timeout` if there is one.
    fn request_within(&self, request: Request, timeout: Option<Duration>) -> Result<Answer> {
        let (mut stream, _) = self.connect_within(timeout)?;
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
            Ok(mut stream) => match protocol::hello(&mut stream, true, 0) {
                // It ended as it was asked: none runs now.
                Err(e) if no_server(&e) => Ok(()),
                hello => hello.map(drop),
            },
        }
    }

    /// The server's instance id, new each time a server starts; an error
    /// if its hello gave none (0), which names no event log.
    pub fn instance(&self) -> Result<u32> {
        self.connect().and_then(|(_, server)| instance_of(server))
    }

    /// The running server's instance id; `None` if none runs; an error if
    /// one runs that gave none (see [`Self::instance`]) or that this
    /// paddock cannot talk to.
    pub fn running_instance(&self) -> Result<Option<u32>> {
        match self.connect() {
            Ok((_, server)) => instance_of(server).map(Some),
            Err(e) if no_server(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Start the server if none runs and `layout.json` holds a layout for
    /// it to rebuild, so paddock finds those spaces instead of making
    /// its first.
    pub fn resume(&self) -> Result<()> {
        if !self.paths.state_dir.join(saved::FILE).exists() {
            return Ok(());
        }
        self.start()
    }

    /// End the running server `other`, which speaks another protocol
    /// version, so the next one rebuilds the layout it saved as it
    /// ended. Only the process holding the server lock is signalled, and
    /// only if it is the one `other` names: a hello's pid alone could be
    /// anyone's. With no lock held, the socket is left over: removed.
    pub fn restart(&self, other: &OtherVersion) -> Result<()> {
        let state_dir = &self.paths.state_dir;
        if !held(state_dir)? {
            let _ = std::fs::remove_file(socket_path(&self.paths)?);
            return Ok(());
        }
        let path = state_dir.join(LOCK);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let pid = text.trim().parse::<i32>().ok().filter(|&p| p > 1);
        let Some(pid) = pid.filter(|&p| u32::try_from(p) == Ok(other.pid)) else {
            bail!(
                "the server lock names pid {}, not the running server's {}: \
                 end it yourself (kill {}) and start paddock again",
                text.trim(),
                other.pid,
                other.pid
            );
        };
        // SAFETY: kill has no memory-safety preconditions; the pid holds
        // the server lock, so it is the server.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err).with_context(|| format!("cannot end pid {}", other.pid));
            }
        }
        self.wait_ended(other.pid)
    }

    /// Wait until the ending server `pid` lets its lock go, then remove
    /// the socket it may leave.
    fn wait_ended(&self, pid: u32) -> Result<()> {
        let start = Instant::now();
        while held(&self.paths.state_dir)? {
            if start.elapsed() > END_TIMEOUT {
                bail!("the paddock server (pid {pid}) did not end");
            }
            std::thread::sleep(POLL);
        }
        // An ending server removes its socket; one killed leaves it.
        let _ = std::fs::remove_file(socket_path(&self.paths)?);
        Ok(())
    }

    /// [`Self::upgrade`] the running server `other`, which speaks another
    /// protocol version, to `exe`, and tell from what runs after a
    /// failure whether the server ran on unchanged (it could not hand
    /// over, from too old to know how to a refusal) or ended after the
    /// switch, every program with it: the same server (`other`'s pid) at
    /// this version means done, another server means it ended and a new
    /// one started. Once ended with none running, its lock is let go, so
    /// a new server can start. An error only if what runs cannot be told,
    /// as when the server gives no hello within [`START_TIMEOUT`].
    pub fn upgrade_other(&self, other: &OtherVersion, exe: &Path) -> Result<Upgraded> {
        let why = match self.upgrade(exe) {
            Ok(_) => return Ok(Upgraded::Done),
            Err(why) => why,
        };
        match self.hello_within(START_TIMEOUT) {
            Err(e) if e.downcast_ref::<OtherVersion>().is_some() => Ok(Upgraded::Unchanged(why)),
            // The switch happened, though its answer was lost.
            Ok(server) if server.pid == other.pid => Ok(Upgraded::Done),
            // Another server, started (from `layout.json`) since the old
            // one ended.
            Ok(_) => Ok(Upgraded::Ended(why)),
            Err(e) if no_server(&e) => {
                self.wait_ended(other.pid)?;
                Ok(Upgraded::Ended(why))
            }
            Err(e) => Err(e.context(format!(
                "the upgrade failed ({why:#}), and the server cannot be asked how it is"
            ))),
        }
    }

    /// Ask the running server, whatever protocol version it speaks, to
    /// hand every terminal to the binary `exe`, which then runs as it
    /// (ADR 0009): the server's pid, the same as before. An error says
    /// why not. A refusal from the old server, or [`NoHandover`] (it
    /// cannot hand over at all, so it has to be restarted instead), leaves
    /// it running unchanged; but once `exe` runs, a refusal from it, or
    /// the connection ending, means the server has ended, and every
    /// program in it.
    pub fn upgrade(&self, exe: &Path) -> Result<u32> {
        let socket = socket_path(&self.paths)?;
        let mut stream = UnixStream::connect(socket).context(NotRunning)?;
        // The probe and the rebuild of a big scrollback take a while.
        stream.set_read_timeout(Some(UPGRADE_TIMEOUT))?;
        let server = match protocol::read(&mut stream, protocol::FROM_SERVER)? {
            Some(Frame::Message(Message::Hello { version, pid, .. }))
                if version < protocol::HANDOVER_VERSION =>
            {
                let version = Some(version);
                return Err(NoHandover { pid, version }.into());
            }
            Some(Frame::Message(Message::Hello { pid, .. })) => pid,
            other => bail!("expected the paddock server's hello, got {other:?}"),
        };
        let ask = Upgrade::Ask { exe: exe.into() };
        protocol::write(&mut stream, &Frame::Upgrade(ask))?;
        let mut probed = false;
        loop {
            match protocol::read(&mut stream, protocol::FROM_SERVER) {
                Ok(Some(Frame::Upgrade(Upgrade::Probed))) => probed = true,
                Ok(Some(Frame::Upgrade(Upgrade::Done { pid }))) => return Ok(pid),
                Ok(Some(Frame::Upgrade(Upgrade::Refused(why)))) => return Err(Refused(why).into()),
                Ok(Some(other)) => {
                    bail!("the paddock server answered the upgrade with {other:?}")
                }
                // A server from before ADR 0009 closes the connection at a
                // frame it does not know.
                Ok(None) if !probed => {
                    return Err(NoHandover {
                        pid: server,
                        version: None,
                    }
                    .into())
                }
                Ok(None) => bail!(
                    "the paddock server (pid {server}) ended as it took over, without an \
                     answer: its programs may have ended (see server.log)"
                ),
                Err(e) if timed_out(&e) => bail!(
                    "the paddock server (pid {server}) gave no answer to the upgrade within \
                     {} s (see server.log)",
                    UPGRADE_TIMEOUT.as_secs()
                ),
                Err(e) => return Err(e.context("a broken answer to the upgrade")),
            }
        }
    }

    /// What the server rebuilt from `layout.json` when it started; `None`
    /// if nothing, or paddock was told already.
    pub fn take_restored(&self) -> Result<Option<Restored>> {
        let restored = self.query(Request::TakeRestored, pick!(Answer::Restored))?;
        Ok(restored.flatten())
    }

    /// The space paddock showed last, as the server keeps it.
    pub fn current_space(&self) -> Result<Option<SpaceId>> {
        Ok(self
            .query(Request::Current, pick!(Answer::SpaceOf))?
            .flatten())
    }

    pub fn set_current_space(&self, space: &SpaceId) -> Result<()> {
        self.done(Request::SetCurrent(space.clone()))
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

    /// Every space, as [`Self::spaces`], asked within `timeout`; none
    /// only if no server runs. A connection that closes unanswered, or
    /// gives no answer within `timeout`, is an error, not none (see
    /// `Backend::upgraded_spaces`).
    pub fn upgraded_spaces(&self, timeout: Duration) -> Result<Vec<ListedSpace>> {
        match self.ask(Request::Spaces, Some(timeout), pick!(Answer::Spaces)) {
            Err(e) if e.downcast_ref::<NotRunning>().is_some() => Ok(Vec::new()),
            asked => asked,
        }
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

    fn started(&self, request: Request) -> Result<TerminalId> {
        match self.request(request)? {
            Answer::Terminal(id) => Ok(id),
            other => Err(unexpected(&other)),
        }
    }

    pub fn new_tab(&self, space: &SpaceId, cwd: &Path) -> Result<TerminalId> {
        self.started(Request::NewTab {
            space: space.clone(),
            cwd: cwd.into(),
        })
    }

    pub fn split(&self, terminal: &TerminalId, columns: bool, cwd: &Path) -> Result<TerminalId> {
        self.started(Request::Split {
            terminal: terminal.clone(),
            columns,
            cwd: cwd.into(),
        })
    }

    pub fn new_float(
        &self,
        space: &SpaceId,
        cwd: &Path,
        command: &str,
        key: &str,
    ) -> Result<FloatId> {
        let request = Request::NewFloat {
            space: space.clone(),
            cwd: cwd.into(),
            command: command.into(),
            key: key.into(),
        };
        match self.request(request)? {
            Answer::Float(id) => Ok(id),
            other => Err(unexpected(&other)),
        }
    }

    pub fn close_float(&self, float: &FloatId) -> Result<()> {
        self.done(Request::CloseFloat(float.clone()))
    }

    pub fn swap_tabs(&self, a: &TabId, b: &TabId) -> Result<()> {
        self.done(Request::SwapTabs(a.clone(), b.clone()))
    }

    pub fn focus_towards(&self, terminal: &TerminalId, dir: Direction) -> Result<()> {
        self.done(Request::FocusTowards(terminal.clone(), dir))
    }

    pub fn spread(&self, terminal: &TerminalId) -> Result<()> {
        self.done(Request::Spread(terminal.clone()))
    }

    pub fn zoom(&self, terminal: &TerminalId) -> Result<()> {
        self.done(Request::Zoom(terminal.clone()))
    }

    pub fn resize(&self, terminal: &TerminalId, dir: Direction, cells: u16) -> Result<()> {
        self.done(Request::ResizeSplit {
            terminal: terminal.clone(),
            dir,
            cells,
        })
    }

    /// The border at `(x, y)` of tab `tab`: moved by sizing the cell
    /// before it (see [`Self::drag_border`]).
    pub fn border_at(&self, tab: &TabId, x: u16, y: u16) -> Result<Option<Border>> {
        let request = Request::BorderAt {
            tab: tab.clone(),
            x,
            y,
        };
        match self.request(request)? {
            Answer::Border(hit) => Ok(hit.map(|hit| {
                let anchor = Anchor::Before(hit.start);
                Border::new(hit.terminal, hit.axis.is_columns(), anchor)
            })),
            other => Err(unexpected(&other)),
        }
    }

    /// Move `border` to where the cell before it is `size` cells. A drag
    /// sends many of these (at most one every 40 ms, see
    /// `app::splits`): they share one connection, kept for drags.
    pub fn drag_border(&self, border: &Border, size: u16) -> Result<()> {
        let request = Request::DragBorder {
            terminal: border.terminal.clone(),
            axis: Axis::of(border.columns()),
            len: size,
        };
        match self.drag_request(&request)? {
            Answer::Done => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    /// Ask `request` on the connection kept for drags, opening it when
    /// there is none. A connection that fails (the server ended, or a new
    /// one runs) is dropped, and the request asked once more on a new one:
    /// moving a border to a size twice moves it once.
    fn drag_request(&self, request: &Request) -> Result<Answer> {
        let mut kept = self.drag.lock().unwrap_or_else(|e| e.into_inner());
        let frame = Frame::Message(Message::Request(request.clone()));
        let mut last = None;
        for _ in 0..2 {
            let stream = match kept.as_mut() {
                Some(stream) => stream,
                None => kept.insert(self.connect()?.0),
            };
            let asked = protocol::write(stream, &frame).and_then(|()| read_reply(stream));
            match asked {
                Err(e) if e.downcast_ref::<Refused>().is_none() => {
                    *kept = None;
                    last = Some(e);
                }
                answer => return answer,
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("no answer from the paddock server")))
    }

    /// The server's answer to `request`, taken by `pick`; `None` if no
    /// server runs or it could not say (no such space). An error if one
    /// runs that this paddock cannot talk to, or it answers out of turn.
    fn query<T>(
        &self,
        request: Request,
        pick: impl FnOnce(Answer) -> Result<T, Answer>,
    ) -> Result<Option<T>> {
        match self.ask(request, None, pick) {
            Ok(value) => Ok(Some(value)),
            Err(e) if no_server(&e) || e.downcast_ref::<Refused>().is_some() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The server's answer to `request`, read within `timeout` if there is
    /// one, and taken by `pick`; an error for any other answer.
    fn ask<T>(
        &self,
        request: Request,
        timeout: Option<Duration>,
        pick: impl FnOnce(Answer) -> Result<T, Answer>,
    ) -> Result<T> {
        let answer = self.request_within(request, timeout)?;
        pick(answer).map_err(|a| unexpected(&a))
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
    /// screens, then every event, in order, to `deliver`. From a server
    /// that sends the scrollback after the screens, each terminal's
    /// [`Event::Scrollback`] comes too, whenever it is rebuilt, among the
    /// output (see [`Replay::scrollback_after`]); from an older one, the
    /// replays have it.
    pub fn attach(
        &self,
        space: &SpaceId,
        size: (u16, u16),
        deliver: impl FnMut(Event) -> bool + Send + 'static,
    ) -> Result<(Attached, Structure, Vec<Replay>)> {
        let (mut stream, _) = self.connect()?;
        let request = Request::Attach {
            space: space.clone(),
            size,
            scrollback_after: true,
        };
        protocol::write(&mut stream, &Frame::Message(Message::Request(request)))?;
        let (structure, terminals, scrollback_after) = match read_reply(&mut stream)? {
            Answer::Attached {
                structure,
                terminals,
                scrollback_after,
            } => (structure, terminals, scrollback_after),
            other => return Err(unexpected(&other)),
        };
        let mut replays = Vec::new();
        for (terminal, size) in &terminals {
            let bytes = read_snapshot(&mut stream, terminal)?;
            replays.push(Replay {
                terminal: terminal.clone(),
                size: *size,
                bytes,
                scrollback_after,
            });
        }
        let later = if scrollback_after {
            terminals
        } else {
            Vec::new()
        };
        // Shared by the reader and the thread that rebuilds, behind one
        // lock: their events reach the app in one order, which the
        // terminal's replay of what it was fed before its scrollback
        // (`Terminal::take_scrollback`) depends on.
        let deliver = Arc::new(Mutex::new(deliver));
        let deliver = move |event| {
            let mut deliver = deliver.lock().unwrap_or_else(|e| e.into_inner());
            deliver(event)
        };
        let mut reader = stream.try_clone()?;
        std::thread::spawn(move || {
            // Each whole snapshot read as it comes, and rebuilt by a
            // thread of its own: the longer it is, the longer that takes,
            // and the output read on meanwhile.
            let (rebuild, snapshots) =
                std::sync::mpsc::channel::<(TerminalId, (u16, u16), Vec<u8>)>();
            let rebuilt = deliver.clone();
            std::thread::spawn(move || {
                for (terminal, size, bytes) in snapshots {
                    let emulator = Rebuilt::new(terminal::rebuild(size, &bytes));
                    if !rebuilt(Event::Scrollback { terminal, emulator }) {
                        return;
                    }
                }
            });
            for (terminal, size) in later {
                let Ok(bytes) = read_snapshot(&mut reader, &terminal) else {
                    deliver(Event::Lost);
                    return;
                };
                let _ = rebuild.send((terminal, size, bytes));
            }
            drop(rebuild);
            // The space's end is the server's notice; a connection that
            // ends without it (let go, or the server gone) is lost.
            while let Ok(Some(frame)) = protocol::read(&mut reader, protocol::FROM_SERVER) {
                let event = match frame {
                    Frame::Bytes { terminal, data } => Event::Output { terminal, data },
                    Frame::Message(Message::Notice(Notice::Changed)) => Event::Changed,
                    Frame::Message(Message::Notice(Notice::SpacesChanged)) => Event::SpacesChanged,
                    Frame::Message(Message::Notice(Notice::InputRefused { terminal, why })) => {
                        Event::InputRefused { terminal, why }
                    }
                    Frame::Message(Message::Notice(Notice::Upgrading)) => Event::Upgrading,
                    Frame::Message(Message::Notice(Notice::Ended)) => {
                        deliver(Event::Ended);
                        return;
                    }
                    // Input parts only go to the server, and an upgrade's
                    // answer only on its own connection.
                    Frame::Message(_) | Frame::Part { .. } | Frame::Upgrade(_) => continue,
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

/// `terminal`'s snapshot, next on `stream`.
fn read_snapshot(stream: &mut UnixStream, terminal: &TerminalId) -> Result<Vec<u8>> {
    match protocol::read(stream, protocol::FROM_SERVER)? {
        Some(Frame::Bytes { terminal: t, data }) if t == *terminal => Ok(data),
        other => bail!("expected {terminal}'s snapshot from the paddock server, got {other:?}"),
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
    /// Shut the connection down both ways, so its reader (which holds
    /// a clone of it) sees it end.
    pub fn close(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Send `bytes` to `terminal`'s program as one input, in parts of at
    /// most [`INPUT_FRAME`] bytes (a paste of any size fits the server's
    /// frame limit); the server queues or refuses all of it.
    pub fn input(&mut self, terminal: &TerminalId, bytes: &[u8]) -> Result<()> {
        let mut rest = bytes.len();
        for chunk in bytes.chunks(INPUT_FRAME) {
            rest -= chunk.len();
            let frame = Frame::Part {
                terminal: terminal.clone(),
                rest: rest as u64,
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

    /// Show float `float` at `size`, apart from the tabs.
    pub fn resize_float(&mut self, float: &FloatId, size: (u16, u16)) -> Result<()> {
        let request = Request::ResizeFloat(float.clone(), size);
        protocol::write(&mut self.stream, &Frame::Message(Message::Request(request)))
    }

    /// Leave the space running on the server.
    pub fn detach(mut self) {
        let frame = Frame::Message(Message::Request(Request::Detach));
        // The server may be gone already; nothing to do then.
        let _ = protocol::write(&mut self.stream, &frame);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// The instance id `server`'s hello gave; an error for none (0), which
/// names no event log.
fn instance_of(server: Peer) -> Result<u32> {
    if server.instance == 0 {
        bail!(
            "the paddock server (pid {}) gave no instance id in its hello",
            server.pid
        );
    }
    Ok(server.instance)
}

#[cfg(test)]
mod instance_tests {
    use super::*;

    #[test]
    fn a_hello_without_an_instance_id_is_an_error() {
        let peer = |instance| Peer { pid: 7, instance };
        assert_eq!(instance_of(peer(42)).unwrap(), 42);
        let err = instance_of(peer(0)).unwrap_err().to_string();
        assert!(
            err.contains("no instance id") && err.contains("pid 7"),
            "{err}"
        );
    }
}

#[cfg(test)]
mod upgraded_spaces_tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A client whose state directory is `dir` (short, for the socket).
    fn client(dir: &Path) -> Client {
        Client::new(Paths {
            socket: "paddock-unit-none".into(),
            state_dir: dir.to_path_buf(),
            config_dir: dir.join("config"),
        })
    }

    fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("pdk")
            .tempdir_in("/tmp")
            .unwrap()
    }

    /// A connection accepted and never served (the old image's, near its
    /// `exec`) is an error within the timeout, not a wait for ever.
    #[test]
    fn a_server_that_never_answers_is_an_error_in_time() {
        let dir = short_dir();
        let listener = UnixListener::bind(dir.path().join(super::super::SOCKET)).unwrap();
        // Accepted and held open, unanswered, until the client gives up.
        let held = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
        let start = Instant::now();

        let asked = client(dir.path()).upgraded_spaces(Duration::from_millis(200));

        assert!(asked.is_err(), "{asked:?}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        drop(held.join());
    }

    #[test]
    fn no_server_running_is_no_spaces() {
        let dir = short_dir();

        let asked = client(dir.path()).upgraded_spaces(Duration::from_millis(200));

        assert!(asked.unwrap().is_empty());
    }
}
