//! `paddock server` (ADR 0007): started in the background by the first
//! paddock that needs it, one per user (its lock), listening on its socket
//! (mode 0600) until its last terminal ends.
//!
//! Every terminal has three threads: one reads its PTY, feeds its
//! emulator and passes the output on to the attached client; one writes
//! its input and query replies; one waits for its shell to exit. All
//! share one [`State`] behind one lock, so a snapshot taken under it holds
//! exactly the output before it, and everything after it reaches the
//! client after the snapshot: no byte lost or doubled.

use super::protocol::{self, Answer, Ending, Frame, Message, Notice, Refusal, Request};
use super::state::{Space, State, Subscriber};
use super::terminal::{HostedTerminal, Input};
use super::{lock, pty, socket_path, unsupported, LOG};
use crate::backend::env::{clean_env, AGY_STATUS_VAR};
use crate::backend::{SpaceId, TabId, TerminalId};
use crate::paths::Paths;
use anyhow::{bail, Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long a server that never got a terminal waits before ending (its
/// client may have died before asking for one).
const IDLE_EXIT: Duration = Duration::from_secs(15);
/// How long the attached client gets to read its last frames at the end.
const LAST_WORDS: Duration = Duration::from_secs(1);
const READ_CHUNK: usize = 64 * 1024;
/// How long an exited shell's last output gets to be read.
const DRAIN: Duration = Duration::from_millis(500);
/// How long a new server waits for an ending one's lock.
const LOCK_WAIT: Duration = Duration::from_secs(3);

struct Shared {
    state: Mutex<State>,
    socket: PathBuf,
    shell: String,
    home: PathBuf,
    /// The shells' environment.
    env: Vec<(String, String)>,
    connections: AtomicU64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Run the server until its last terminal ends. If another server holds
/// the lock, waits up to [`LOCK_WAIT`] for it to let go (an ending one
/// does), else returns without serving.
pub fn serve() -> Result<()> {
    let paths = Paths::from_env()?;
    std::fs::create_dir_all(&paths.state_dir)
        .with_context(|| format!("cannot create {}", paths.state_dir.display()))?;
    let socket = socket_path(&paths)?;
    let Some(_lock) = wait_for_lock(&paths.state_dir)? else {
        return Ok(());
    };
    let listener = bind(&socket)?;
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut env = clean_env(std::env::vars());
    env.push(("COLORTERM".into(), "truecolor".into()));
    env.push((AGY_STATUS_VAR.into(), "on".into()));
    let shared = Arc::new(Shared {
        state: Mutex::default(),
        socket,
        shell: var("SHELL").unwrap_or_else(|| "/bin/sh".into()),
        home: var("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
        env,
        connections: AtomicU64::new(0),
    });
    watch_signals(&shared)?;
    let idle = Arc::clone(&shared);
    std::thread::spawn(move || {
        std::thread::sleep(IDLE_EXIT);
        if !idle.lock().started {
            shut_down(&idle);
        }
    });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let conn = shared.connections.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = serve_connection(&shared, conn, stream) {
                eprintln!("paddock server: connection {conn}: {e:#}");
            }
            // Its writer ends; nobody reads what is left.
            drop(shared.lock().attached.take_if(|s| s.conn == conn));
        });
    }
    Ok(())
}

/// The server lock, once a server that is ending lets it go; `None` if
/// another server keeps it.
fn wait_for_lock(state_dir: &Path) -> Result<Option<std::fs::File>> {
    let start = std::time::Instant::now();
    loop {
        if let Some(lock) = lock(state_dir)? {
            return Ok(Some(lock));
        }
        if start.elapsed() > LOCK_WAIT {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Listen on `socket`, mode 0600, in place of any socket left there: the
/// lock says no server runs.
fn bind(socket: &Path) -> Result<UnixListener> {
    let _ = std::fs::remove_file(socket);
    // The mask is the whole process's: safe only because no other thread
    // runs yet to make a file meanwhile (`serve` starts them after this).
    // SAFETY: umask only changes the mask of files made from now on.
    let old = unsafe { libc::umask(0o177) };
    let listener = UnixListener::bind(socket);
    // SAFETY: as above, putting the old mask back.
    unsafe { libc::umask(old) };
    listener.with_context(|| format!("cannot listen on {}", socket.display()))
}

/// End everything when asked to (SIGTERM, SIGINT, SIGHUP).
fn watch_signals(shared: &Arc<Shared>) -> Result<()> {
    let mut signals = Signals::new([SIGTERM, SIGINT, SIGHUP]).context("cannot handle signals")?;
    let shared = Arc::clone(shared);
    std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            shut_down(&shared);
        }
    });
    Ok(())
}

/// Hang every shell up, give the attached client its last frames, remove
/// the socket and exit.
fn shut_down(shared: &Shared) -> ! {
    let attached = {
        let mut state = shared.lock();
        close(shared, &mut state);
        for (_, term) in state.terminals() {
            hang_up(term.pid);
        }
        state.attached.take()
    };
    if let Some(attached) = attached {
        attached.let_go(LAST_WORDS);
    }
    std::process::exit(0)
}

/// Take no more spaces, and stop listening: a paddock that comes now
/// starts the next server.
fn close(shared: &Shared, state: &mut State) {
    if !state.closing {
        state.closing = true;
        let _ = std::fs::remove_file(&shared.socket);
    }
}

/// Hang up the shell `pid` and its process group, as closing its terminal
/// does.
fn hang_up(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else { return };
    // SAFETY: kill has no memory-safety preconditions; `pid` > 1 leads its
    // own session (setsid), so `-pid` is its group only.
    unsafe {
        if pid > 1 {
            libc::kill(-pid, libc::SIGHUP);
            libc::kill(pid, libc::SIGHUP);
        }
    }
}

fn serve_connection(shared: &Arc<Shared>, conn: u64, mut stream: UnixStream) -> Result<()> {
    protocol::hello(&mut stream, false)?;
    let mut attached = false;
    while let Some(frame) = protocol::read(&mut stream, protocol::FROM_CLIENT)? {
        let request = match frame {
            Frame::Bytes { terminal, data } => {
                input(shared, &terminal, data);
                continue;
            }
            Frame::Message(Message::Request(request)) => request,
            Frame::Message(other) => bail!("unexpected {other:?}"),
        };
        let answer = match request {
            Request::Resize(size) => {
                resize(shared, conn, size);
                continue;
            }
            Request::Detach => {
                let mut state = shared.lock();
                // Its writer ends; the client closes the connection.
                drop(state.attached.take_if(|s| s.conn == conn));
                continue;
            }
            // Once attached, the connection is the writer's: a reply here
            // would land among its frames.
            request if attached => bail!("{request:?} after attaching"),
            Request::Attach { space, size } => match attach(shared, conn, &stream, &space, size) {
                Ok(()) => {
                    attached = true;
                    continue;
                }
                Err(e) => Err(e),
            },
            Request::Stop => {
                protocol::write(&mut stream, &reply(Ok(Answer::Done)))?;
                shut_down(shared);
            }
            Request::NewSpace { name, cwd, size } => new_space(shared, &name, &cwd, size),
            Request::CloseSpace(space) => close_space(shared, &space),
            Request::CloseTerminal(terminal) => close_terminal(shared, &terminal),
            request => shared.lock().answer(request),
        };
        protocol::write(&mut stream, &reply(answer))?;
    }
    Ok(())
}

fn reply(answer: Result<Answer>) -> Frame {
    let answer = answer.map_err(|e| match e.downcast_ref::<Ending>() {
        Some(Ending) => Refusal::Ending,
        None => Refusal::Failed(format!("{e:#}")),
    });
    Frame::Message(Message::Reply(answer))
}

/// Write `data` to `terminal`'s program, after what came before it.
/// Never waits (see [`Input`]): the connection goes on reading, so a
/// resize or detach behind input to a program that does not read gets
/// through.
fn input(shared: &Shared, terminal: &TerminalId, data: Vec<u8>) {
    let state = shared.lock();
    let found = state.terminals().find(|(_, t)| t.id == *terminal);
    if let Some((_, term)) = found {
        term.input.send(terminal, data);
    }
}

/// Make connection `conn` the attached client of `space`, shown at
/// `size`: its reply, then each terminal's snapshot, then their output,
/// all through a writer thread of its own. Any other client is let go.
fn attach(
    shared: &Shared,
    conn: u64,
    stream: &UnixStream,
    space: &SpaceId,
    size: (u16, u16),
) -> Result<()> {
    let mut subscriber = Subscriber::new(conn, space.clone(), stream)?;
    let mut state = shared.lock();
    let hosted = &mut state.space_mut(space)?.terminals;
    for term in hosted.iter_mut() {
        term.resize(size)?;
    }
    let terminals = hosted.iter().map(|t| (t.id.clone(), t.size)).collect();
    let snapshots: Vec<Frame> = hosted
        .iter_mut()
        .map(|t| Frame::Bytes {
            terminal: t.id.clone(),
            data: t.snapshot(),
        })
        .collect();
    let structure = state.structure(space)?;
    let first = std::iter::once(reply(Ok(Answer::Attached {
        structure,
        terminals,
    })));
    for frame in first.chain(snapshots) {
        // A new client: it has had no time to fall behind.
        let _ = subscriber.queue(frame);
    }
    let old = state.attached.replace(subscriber);
    drop(state);
    if let Some(old) = old {
        std::thread::spawn(move || old.let_go(LAST_WORDS));
    }
    Ok(())
}

/// Show the space connection `conn` attached at `size`.
fn resize(shared: &Shared, conn: u64, size: (u16, u16)) {
    let mut state = shared.lock();
    let Some(space) = state.attached.as_ref().filter(|s| s.conn == conn) else {
        return;
    };
    let space = space.space.clone();
    let Ok(s) = state.space_mut(&space) else {
        return;
    };
    for term in &mut s.terminals {
        if let Err(e) = term.resize(size) {
            eprintln!("paddock server: {e:#}");
        }
    }
    state.notify(&space, Notice::Changed);
}

/// Start the one space: a shell in `cwd`, at `size`.
fn new_space(shared: &Arc<Shared>, name: &str, cwd: &Path, size: (u16, u16)) -> Result<Answer> {
    let mut state = shared.lock();
    if state.closing {
        return Err(Ending.into());
    }
    if !state.spaces.is_empty() {
        bail!(unsupported("more than one space"));
    }
    let space = SpaceId::new(format!("${}", state.next()));
    let tab = TabId::new(format!("@{}", state.next()));
    let terminal = TerminalId::new(format!("%{}", state.next()));
    let term = start_terminal(shared, &space, terminal, cwd, size)?;
    let tab_name = cwd
        .file_name()
        .map_or_else(|| cwd.to_string_lossy(), |n| n.to_string_lossy())
        .into_owned();
    state.spaces.push(Space {
        id: space.clone(),
        name: name.to_string(),
        root: cwd.to_string_lossy().into_owned(),
        tab,
        tab_name,
        auto_name: true,
        terminals: vec![term],
    });
    state.started = true;
    state.notify_spaces();
    Ok(Answer::Space(space))
}

/// A shell on a new PTY in `cwd`, and its threads. Called with the state
/// locked, so its reader waits until the terminal is in it (and no other
/// shell starts meanwhile, see [`pty::spawn`]).
fn start_terminal(
    shared: &Arc<Shared>,
    space: &SpaceId,
    id: TerminalId,
    cwd: &Path,
    size: (u16, u16),
) -> Result<HostedTerminal> {
    let mut pty = pty::spawn(&shared.shell, cwd, &shared.home, size, &shared.env)?;
    let ready = (|| {
        let input = Input::start(pty.master.try_clone()?);
        let reader = pty.master.try_clone()?;
        let term = HostedTerminal::new(id.clone(), &pty, input, size)?;
        anyhow::Ok((reader, term))
    })();
    let (reader, term) = match ready {
        Ok(ready) => ready,
        Err(e) => {
            // Leave no shell behind with no terminal.
            let _ = pty.child.kill();
            let _ = pty.child.wait();
            return Err(e);
        }
    };
    let (drained, done) = mpsc::channel::<()>();
    let (s, sp, t) = (Arc::clone(shared), space.clone(), id.clone());
    std::thread::spawn(move || read_output(&s, &sp, &t, reader, drained));
    let (s, sp) = (Arc::clone(shared), space.clone());
    let child = pty.child;
    std::thread::spawn(move || wait_for_exit(&s, &sp, &id, child, &done));
    Ok(term)
}

/// Feed terminal `id`'s output to its emulator and the attached client,
/// until its PTY closes; then drop `_drained`.
fn read_output(
    shared: &Shared,
    space: &SpaceId,
    id: &TerminalId,
    mut master: std::fs::File,
    _drained: mpsc::Sender<()>,
) {
    let mut buf = vec![0; READ_CHUNK];
    loop {
        let n = match master.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let mut state = shared.lock();
        let found = state
            .spaces
            .iter_mut()
            .flat_map(|s| s.terminals.iter_mut())
            .find(|t| t.id == *id);
        let Some(term) = found else { return };
        term.output(&buf[..n]);
        let frame = Frame::Bytes {
            terminal: id.clone(),
            data: buf[..n].to_vec(),
        };
        state.send(Some(space), frame);
    }
}

/// When terminal `id`'s shell exits, and its last output is read (or after
/// [`DRAIN`], as a program left in the background may hold the PTY open),
/// forget the terminal, and its space with its last terminal; end the
/// server with the last one.
fn wait_for_exit(
    shared: &Shared,
    space: &SpaceId,
    id: &TerminalId,
    mut child: Child,
    drained: &mpsc::Receiver<()>,
) {
    let _ = child.wait();
    // The reader only drops its end.
    let _ = drained.recv_timeout(DRAIN);
    let mut state = shared.lock();
    let mut ended = None;
    if let Ok(s) = state.space_mut(space) {
        s.terminals.retain(|t| t.id != *id);
        if s.terminals.is_empty() {
            state.spaces.retain(|s| s.id != *space);
            state.notify(space, Notice::Ended);
            // Nothing follows its end.
            ended = state.attached.take_if(|s| s.space == *space);
            state.notify_spaces();
        } else {
            state.notify(space, Notice::Changed);
        }
    }
    let last = state.spaces.is_empty();
    if last {
        close(shared, &mut state);
    }
    drop(state);
    if let Some(ended) = ended {
        ended.let_go(LAST_WORDS);
    }
    if last {
        shut_down(shared);
    }
}

fn close_space(shared: &Shared, space: &SpaceId) -> Result<Answer> {
    let state = shared.lock();
    for term in &state.space(space)?.terminals {
        hang_up(term.pid);
    }
    Ok(Answer::Done)
}

fn close_terminal(shared: &Shared, terminal: &TerminalId) -> Result<Answer> {
    let state = shared.lock();
    let Some((_, term)) = state.terminals().find(|(_, t)| t.id == *terminal) else {
        bail!("no terminal {terminal}");
    };
    hang_up(term.pid);
    Ok(Answer::Done)
}

/// Where the server writes what goes wrong, under `state_dir`.
pub fn log_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LOG)
}
