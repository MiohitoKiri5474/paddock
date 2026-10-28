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

use super::axis::Axis;
use super::protocol::{self, Answer, Ending, Frame, Message, Notice, Refusal, Request};
use super::space::Space;
use super::state::{State, Subscriber};
use super::tab::Tab;
use super::terminal::{dir_name, HostedTerminal, Input, Sent, INPUT_MAX};
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
/// How often tabs named after what runs in them look again.
const TAB_NAME_POLL: Duration = Duration::from_millis(300);

struct Shared {
    state: Mutex<State>,
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
        state: Mutex::new(State::new(socket, new_instance())),
        shell: var("SHELL").unwrap_or_else(|| "/bin/sh".into()),
        home: var("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
        env,
        connections: AtomicU64::new(0),
    });
    watch_signals(&shared)?;
    let names = Arc::clone(&shared);
    std::thread::spawn(move || follow_names(&names));
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

/// A new instance id: random, so no earlier server's (whose hook event
/// log, and agents still writing to it, may be left) has it but by a
/// one in four billion chance. Never 0, which a client's hello sends.
fn new_instance() -> u32 {
    loop {
        // SAFETY: arc4random has no preconditions.
        let id = unsafe { libc::arc4random() };
        if id != 0 {
            return id;
        }
    }
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

/// Hang every shell up, tell the attached client its space ended (with
/// its last frames), remove the socket and exit.
fn shut_down(shared: &Shared) -> ! {
    let attached = {
        let mut state = shared.lock();
        close(&mut state);
        for (_, term) in state.terminals() {
            hang_up(term.pid);
        }
        if let Some(space) = state.attached.as_ref().map(|s| s.space.clone()) {
            state.notify(&space, Notice::Ended);
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
fn close(state: &mut State) {
    if !state.closing {
        state.closing = true;
        let _ = std::fs::remove_file(state.socket());
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
    let instance = shared.lock().instance();
    protocol::hello(&mut stream, false, instance)?;
    let mut attached = false;
    // An input whose last parts have not come yet. Left so when the
    // connection ends, it is discarded whole.
    let mut ongoing = None;
    let ongoing = &mut ongoing;
    while let Some(frame) = protocol::read(&mut stream, protocol::FROM_CLIENT)? {
        let request = match frame {
            Frame::Part {
                terminal,
                rest,
                data,
            } => {
                input(shared, ongoing, &terminal, rest, data)?;
                continue;
            }
            Frame::Message(Message::Request(request)) => request,
            // Output is the server's to send.
            Frame::Bytes { .. } => bail!("unexpected bytes frame"),
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
            Request::NewTab { space, cwd } => new_tab(shared, &space, &cwd),
            Request::Split {
                terminal,
                columns,
                cwd,
            } => split(shared, &terminal, columns, &cwd),
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

/// An input whose parts still come on a connection: collected, to be
/// queued whole once its last part has come.
struct Ongoing {
    terminal: TerminalId,
    /// Its bytes so far; none kept if it is too big to queue.
    data: Vec<u8>,
    /// Its bytes still to come.
    due: usize,
    /// Bigger than [`INPUT_MAX`]: refused whole.
    too_big: bool,
}

/// Take part `data` of an input to `terminal`'s program, `rest` bytes of
/// it to follow; once all has come, queue it whole ([`Input::send`]), or
/// refuse it whole with a notice. Never waits: the connection goes on
/// reading, so a resize or detach behind input to a program that does not
/// read gets through. A part that does not continue the input before it
/// (another terminal, or lengths that do not add up) is an error, which
/// closes the connection: what it collects never passes the input's first
/// length, nor [`INPUT_MAX`].
fn input(
    shared: &Shared,
    ongoing: &mut Option<Ongoing>,
    terminal: &TerminalId,
    rest: u64,
    data: Vec<u8>,
) -> Result<()> {
    let rest = usize::try_from(rest).context("an input too long")?;
    let total = data.len().checked_add(rest).context("an input too long")?;
    let mut collected = match ongoing.take() {
        Some(o) if o.terminal == *terminal && o.due == total => o,
        Some(o) => bail!(
            "a part of {terminal}'s input, {total} bytes, where {} of {}'s were due",
            o.due,
            o.terminal
        ),
        None => Ongoing {
            terminal: terminal.clone(),
            data: Vec::new(),
            due: total,
            too_big: total > INPUT_MAX,
        },
    };
    collected.due = rest;
    if !collected.too_big {
        collected.data.extend_from_slice(&data);
    }
    if rest > 0 {
        *ongoing = Some(collected);
        return Ok(());
    }
    let mut state = shared.lock();
    let found = state.terminals().find(|(_, t)| t.id == *terminal);
    let Some((space, term)) = found else {
        return Ok(());
    };
    let sent = if collected.too_big {
        term.input.refuse()
    } else {
        term.input.send(collected.data)
    };
    if sent != (Sent::Refused { first: true }) {
        return Ok(());
    }
    let program = term.command().unwrap_or_else(|| "its program".into());
    let why = format!(
        "it would take what waits for {program} past {} MiB",
        INPUT_MAX >> 20
    );
    eprintln!("paddock server: refusing input to {terminal}: {why}");
    let space = space.id.clone();
    let notice = Notice::InputRefused {
        terminal: terminal.clone(),
        why,
    };
    state.notify(&space, notice);
    Ok(())
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
    let shown = state.space_mut(space)?;
    shown.size = size;
    shown.lay_out();
    let hosted = &mut shown.terminals;
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
    s.size = size;
    s.lay_out();
    state.notify(&space, Notice::Changed);
}

/// A new tab named after `cwd`, holding `terminal`, with a new id.
fn new_tab_for(state: &mut State, cwd: &Path, terminal: TerminalId) -> Tab {
    let tab = TabId::new(format!("@{}", state.next()));
    Tab::new(tab, dir_name(&cwd.to_string_lossy()), terminal)
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
    let terminal = TerminalId::new(format!("%{}", state.next()));
    let tab = new_tab_for(&mut state, cwd, terminal.clone());
    let term = start_terminal(shared, &state, &space, terminal, cwd, size)?;
    let root = cwd.to_string_lossy().into_owned();
    let made = Space::new(space.clone(), name.to_string(), root, tab, term);
    state.spaces.push(made);
    state.started = true;
    state.notify_spaces();
    Ok(Answer::Space(space))
}

/// Open a tab at the end of `space`, its shell in `cwd`, and make it
/// current.
fn new_tab(shared: &Arc<Shared>, space: &SpaceId, cwd: &Path) -> Result<Answer> {
    let mut state = shared.lock();
    let size = state.space(space)?.size;
    let terminal = TerminalId::new(format!("%{}", state.next()));
    let tab = new_tab_for(&mut state, cwd, terminal.clone());
    let term = start_terminal(shared, &state, space, terminal.clone(), cwd, size)?;
    let s = state.space_mut(space)?;
    s.push_tab(tab, term);
    state.notify(space, Notice::Changed);
    Ok(Answer::Terminal(terminal))
}

/// Split `target` (side by side if `columns`): a shell in `cwd` takes the
/// second half and the focus.
fn split(shared: &Arc<Shared>, target: &TerminalId, columns: bool, cwd: &Path) -> Result<Answer> {
    let mut state = shared.lock();
    let terminal = TerminalId::new(format!("%{}", state.next()));
    let s = state.space_of_mut(target)?;
    let (space, size) = (s.id.clone(), s.size);
    let i = s.tab_index(target)?;
    // To put back as it was, zoom and shares, if the shell does not start.
    let before = s.tabs[i].clone();
    let tab = &mut s.tabs[i];
    tab.split(target, terminal.clone(), Axis::of(columns), size)?;
    let place = tab.splits(size).into_iter().find(|p| p.id == terminal);
    let place = place.map_or(size, |p| (p.rect.width, p.rect.height));
    let started = start_terminal(shared, &state, &space, terminal.clone(), cwd, place);
    let s = state.space_mut(&space)?;
    match started {
        Ok(term) => s.terminals.push(term),
        Err(e) => {
            if let Some(tab) = s.tabs.get_mut(i) {
                *tab = before;
            }
            return Err(e);
        }
    }
    s.lay_out_tab(i);
    state.notify(&space, Notice::Changed);
    Ok(Answer::Terminal(terminal))
}

/// Name the tabs that follow what runs in them, every [`TAB_NAME_POLL`].
fn follow_names(shared: &Shared) {
    loop {
        std::thread::sleep(TAB_NAME_POLL);
        shared.lock().follow_names();
    }
}

/// A shell on a new PTY in `cwd`, with the hook variables of terminal
/// `id`, and its threads. Called with the state locked (`state`), so its
/// reader waits until the terminal is in it (and no other shell starts
/// meanwhile, see [`pty::spawn`]).
fn start_terminal(
    shared: &Arc<Shared>,
    state: &State,
    space: &SpaceId,
    id: TerminalId,
    cwd: &Path,
    size: (u16, u16),
) -> Result<HostedTerminal> {
    // Its hook variables: they tell an agent's hook which terminal it runs in.
    let env: Vec<_> = shared
        .env
        .iter()
        .cloned()
        .chain(state.hook_env(&id))
        .collect();
    let mut pty = pty::spawn(&shared.shell, cwd, &shared.home, size, &env)?;
    let ready = (|| {
        let input = Input::start(&id, pty.master.try_clone()?);
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
        let empty = s.remove(id);
        if empty {
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
        close(&mut state);
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
