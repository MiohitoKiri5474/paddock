//! `paddock server` (ADR 0007): started in the background by the first
//! paddock that needs it, one per user (its lock), listening on its socket
//! (mode 0600) until its last terminal ends.
//!
//! Every terminal has three threads: one reads its PTY, feeds its
//! emulator and passes the output on to the attached client; one writes
//! its input and query replies; one waits for its shell to exit. All
//! share one [`State`] behind one lock, so a snapshot taken under it holds
//! exactly the output before it, and everything after it reaches the
//! client after the snapshot: no byte lost or doubled. A reader reads
//! only with the lock held, and a waiter reaps only with it held, so with
//! it held no output is half taken and no program half reaped: the server
//! can hand everything to a new binary (ADR 0009, [`handover`]).

use super::axis::Axis;
use super::handover::{self, HandedTerminal, Held};
use super::protocol::{self, Answer, Ending, Frame, Message, Notice, Refusal, Request, Upgrade};
use super::pty::Program::{self, LoginShell};
use super::restore::Resume;
use super::saved::{self, Saver};
use super::scrollback::{self, ScrollbackName};
use super::seen::Seen;
use super::space::{HostedFloat, Space};
use super::state::{State, Subscriber};
use super::tab::Tab;
use super::terminal::{dir_name, HostedTerminal, Input, Sent, INPUT_MAX};
use super::{lock, pty, restore, socket_path, LOG};
use crate::backend::env::{clean_env, AGY_STATUS_VAR};
use crate::backend::{FloatId, SpaceId, TabId, TerminalId};
use crate::hooks::now_ms;
use crate::paths::Paths;
use anyhow::{bail, Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::convert::Infallible;
use std::fs::File;
use std::io::{BufReader, Read};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
/// How often the layout is offered to `layout.json` (written only when
/// it changed, at most once every [`saved::GAP`]).
const SAVE_POLL: Duration = Duration::from_millis(100);
/// How long an upgrade waits for every program to take its input.
const INPUT_WAIT: Duration = Duration::from_secs(2);
/// How often a restored terminal is looked at for its shell's first
/// output, before its resume command is typed.
const RESUME_POLL: Duration = Duration::from_millis(20);
/// How long a restored shell may stay silent before its resume command
/// is typed anyway.
const RESUME_WAIT: Duration = Duration::from_secs(3);

struct Shared {
    state: Mutex<State>,
    shell: String,
    home: PathBuf,
    /// The shells' environment.
    env: Vec<(String, String)>,
    connections: AtomicU64,
    /// Writes `layout.json`; locked after `state`, if both are.
    saver: Mutex<Saver>,
    state_dir: PathBuf,
    /// Its lock's and listening socket's descriptors, which an upgrade
    /// hands over.
    held: Held,
    /// The agent session last seen in each terminal, from the server's
    /// hook log, for `layout.json`; locked after `state`, if both are,
    /// and before `saver`.
    seen: Mutex<Seen>,
}

impl Shared {
    /// A server's shared parts, from its environment: holding `state`,
    /// its lock and listening socket on `held`.
    fn new(state: State, paths: &Paths, held: Held) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let mut env = clean_env(std::env::vars());
        env.push(("COLORTERM".into(), "truecolor".into()));
        env.push((AGY_STATUS_VAR.into(), "on".into()));
        // A server that took over (ADR 0009) reads its hook log again from
        // its start: the same instance, so the same log, and folding it
        // again gives what the server before it had seen.
        let seen = Seen::new(&paths.state_dir, state.instance());
        Self {
            state: Mutex::new(state),
            shell: var("SHELL").unwrap_or_else(|| "/bin/sh".into()),
            home: var("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
            env,
            connections: AtomicU64::new(0),
            saver: Mutex::new(Saver::new(paths.state_dir.clone())),
            state_dir: paths.state_dir.clone(),
            held,
            seen: Mutex::new(seen),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
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
    let Some(lock) = wait_for_lock(&paths.state_dir)? else {
        return Ok(());
    };
    let listener = bind(&socket)?;
    let state = State::new(socket, new_instance());
    let held = Held::of(&lock, &listener);
    let shared = Arc::new(Shared::new(state, &paths, held));
    // Before any signal is handled: one that ends the server mid-way
    // would save half the layout.
    let mut resumed = Vec::new();
    {
        let mut state = shared.lock();
        let mut start = |state: &State, space: &SpaceId, id, cwd: &Path, size: (u16, u16)| {
            start_terminal(&shared, state, space, id, LoginShell, cwd, size)
        };
        restore::from_file(&paths.state_dir, &mut state, &mut start, &mut resumed);
        // Saved as running until the resumed agents' hooks say otherwise,
        // so a restart before they report keeps them.
        let mut seen = shared.seen();
        for r in &resumed {
            seen.seed(r.terminal.clone(), r.agent.clone());
        }
    }
    if !resumed.is_empty() {
        let typing = Arc::clone(&shared);
        std::thread::spawn(move || type_resumes(&typing, resumed));
    }
    run(&shared, &listener, lock)
}

/// `paddock server --take-over <record> <connection>`: what the server
/// becomes when it hands over (ADR 0009), from the descriptors it passed.
/// Takes over the spaces in the record, answers on the connection, and
/// serves on as the server did; an error, which ends it (hanging up every
/// program as their PTYs close), if it cannot.
pub fn take_over(record: &str, conn: &str) -> Result<()> {
    let record: RawFd = record.parse().context("a record descriptor")?;
    let conn: RawFd = conn.parse().context("a connection descriptor")?;
    for fd in [record, conn] {
        if fd <= 2 || !handover::is_open(fd) {
            bail!("no descriptor {fd} to take over from");
        }
        handover::close_on_exec(fd, true)?;
    }
    // SAFETY: the server that became this process passed these two open
    // descriptors, named on its command line, and nothing else owns them.
    let (record, mut conn) = unsafe { (File::from_raw_fd(record), UnixStream::from_raw_fd(conn)) };
    let pid = std::process::id();
    match adopt(record) {
        Ok((shared, listener, lock)) => {
            eprintln!("paddock server: took over as pid {pid}");
            let done = Frame::Upgrade(Upgrade::Done { pid });
            if let Err(e) = protocol::write(&mut conn, &done) {
                eprintln!("paddock server: cannot answer the upgrade: {e:#}");
            }
            drop(conn);
            run(&shared, &listener, lock)
        }
        Err(e) => {
            let why = format!(
                "the new paddock server could not take over, and has ended with every \
                 program in it (the next paddock rebuilds the layout): {e:#}"
            );
            let _ = protocol::write(&mut conn, &Frame::Upgrade(Upgrade::Refused(why)));
            Err(e)
        }
    }
}

/// The server the handover record `record` describes, its terminals'
/// threads started, its listener and its lock.
fn adopt(record: File) -> Result<(Arc<Shared>, UnixListener, File)> {
    let paths = Paths::from_env()?;
    let socket = socket_path(&paths)?;
    let (record, snapshots) = handover::read(&mut BufReader::new(record))?;
    let lock = File::from(take_fd(record.lock)?);
    let listener = UnixListener::from(take_fd(record.listener)?);
    let state = State::handed(socket, record.instance, record.last_id);
    let held = Held::of(&lock, &listener);
    let shared = Arc::new(Shared::new(state, &paths, held));
    {
        let mut state = shared.lock();
        let instance = record.instance;
        let mut adopt = |space: &SpaceId, handed: HandedTerminal, snapshot: &[u8]| {
            adopt_terminal(&shared, instance, space, handed, snapshot)
        };
        handover::rebuild(&mut state, record, snapshots, &mut adopt)?;
    }
    Ok((shared, listener, lock))
}

/// The descriptor `fd`, which the record names, as this process's own,
/// closed on its next exec.
fn take_fd(fd: RawFd) -> Result<std::os::fd::OwnedFd> {
    if !handover::is_open(fd) {
        bail!("no descriptor {fd} passed over");
    }
    handover::close_on_exec(fd, true)?;
    // SAFETY: open, passed over by the server this process was, and
    // named once in the record (`handover::read` checks), so owned once.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

/// Terminal `handed` of `space`, on its PTY as passed over, its emulator
/// rebuilt from `snapshot`, and its threads (see [`start_terminal`]).
fn adopt_terminal(
    shared: &Arc<Shared>,
    instance: u32,
    space: &SpaceId,
    handed: HandedTerminal,
    snapshot: &[u8],
) -> Result<HostedTerminal> {
    let master = File::from(take_fd(handed.master)?);
    let input = Input::start(&handed.id, master.try_clone()?);
    let reader = master.try_clone()?;
    let HandedTerminal {
        id,
        pid,
        tty,
        size,
        exited,
        scrollback,
        ..
    } = handed;
    let saved_as = scrollback.as_deref().and_then(ScrollbackName::parse);
    let saved_as = saved_as.unwrap_or_else(|| ScrollbackName::new(instance, &id));
    let mut term = HostedTerminal::hosting(id.clone(), saved_as, pid, tty, master, input, size);
    term.exited = exited;
    term.replay(snapshot);
    watch(shared, space, id, reader, (!exited).then_some(pid));
    Ok(term)
}

/// Serve on `listener` until the server ends, holding its lock `_lock`.
fn run(shared: &Arc<Shared>, listener: &UnixListener, _lock: File) -> Result<()> {
    watch_signals(shared)?;
    let names = Arc::clone(shared);
    std::thread::spawn(move || follow_names(&names));
    let saving = Arc::clone(shared);
    std::thread::spawn(move || save_layout(&saving));
    let idle = Arc::clone(shared);
    std::thread::spawn(move || {
        std::thread::sleep(IDLE_EXIT);
        if !idle.lock().started {
            shut_down(&idle);
        }
    });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(shared);
        std::thread::spawn(move || {
            let conn = shared.connections.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = serve_connection(&shared, conn, stream) {
                eprintln!("paddock server: connection {conn}: {e:#}");
            }
            // Its writer ends; nobody reads what is left.
            drop(shared.lock().detach(conn));
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

/// Hang every shell up, tell each attached client its space ended (with
/// its last frames), remove the socket and exit, once every client being
/// let go has read its last frames (or [`LAST_WORDS`] has passed). Only one
/// caller ends the server: another (a float's command ending after its
/// space ended with its last tab) waits here until it exits, rather than
/// exiting while a client is still let go.
fn shut_down(shared: &Shared) -> ! {
    /// Held by the caller ending the server, through its exit.
    static ENDING: Mutex<()> = Mutex::new(());
    let _ending = ENDING.lock().unwrap_or_else(|e| e.into_inner());
    let leaving = {
        let mut state = shared.lock();
        close(shared, &mut state);
        for (_, term) in state.terminals() {
            term.hang_up();
        }
        state.notify_all(&Notice::Ended);
        for s in std::mem::take(&mut state.attached) {
            state.let_go(s, LAST_WORDS);
        }
        state.take_leaving()
    };
    for thread in leaving {
        let _ = thread.join();
    }
    std::process::exit(0)
}

/// Take no more spaces, and stop listening: a paddock that comes now
/// starts the next server. The layout is saved as it is now, and no
/// more: the shells about to end leave it as it was (and a server left
/// with no space removes it).
fn close(shared: &Shared, state: &mut State) {
    if !state.closing {
        flush_layout(shared, state);
        state.closing = true;
        let _ = std::fs::remove_file(state.socket());
    }
}

/// Write each terminal's scrollback, then the layout to `layout.json`
/// now, if it changed, and no more. The saver is held throughout, so no
/// layout written meanwhile removes scrollback just saved. Unlike the
/// layout offered while the server runs, both are written with the state
/// locked: the server is ending (see [`scrollback`]). The layout has the
/// sessions seen up to now, before the shells are hung up; an agent
/// ended from outside just before (a reboot signals it too) is saved as
/// not over ([`Seen::close`]).
fn flush_layout(shared: &Shared, state: &mut State) {
    let mut seen = shared.seen();
    seen.close(now_ms());
    seen.read_new();
    let layout = saved::snapshot(state).read_dirs(&mut seen);
    drop(seen);
    let mut saver = shared.saver.lock().unwrap_or_else(|e| e.into_inner());
    scrollback::save_all(saver.dir(), state);
    if let Err(e) = saver.flush(layout) {
        eprintln!("paddock server: {e:#}");
    }
}

/// Offer the layout to `layout.json` every [`SAVE_POLL`], until the
/// server is ending. A terminal's directory is read here too, as no
/// notice says it changed, and the hook log for the sessions seen: with
/// the state let go, as is the file. (A
/// layout taken before the server began to end is never written after
/// its last: the saver is flushed then, and takes no more.)
fn save_layout(shared: &Shared) {
    loop {
        std::thread::sleep(SAVE_POLL);
        shared.seen().read_new();
        let snapshot = {
            let state = shared.lock();
            if state.closing {
                return;
            }
            saved::snapshot(&state)
        };
        let layout = snapshot.read_dirs(&mut shared.seen());
        let mut saver = shared.saver.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = saver.offer(layout, Instant::now()) {
            eprintln!("paddock server: {e:#}");
        }
    }
}

/// Type each resume command into its restored terminal once the shell
/// there has written something (its rc files have run, so it reads the
/// line, not something they ask), or after [`RESUME_WAIT`] if it stays
/// silent; drop one whose terminal has closed.
fn type_resumes(shared: &Shared, mut pending: Vec<Resume>) {
    let started = Instant::now();
    while !pending.is_empty() {
        std::thread::sleep(RESUME_POLL);
        let waited = started.elapsed();
        let state = shared.lock();
        pending.retain(|r| {
            let mut terminals = state.terminals().map(|(_, t)| t);
            let Some(t) = terminals.find(|t| t.id == r.terminal) else {
                return false;
            };
            if !ready_to_type(t.spoke, waited) {
                return true;
            }
            t.input.send(format!("{}\r", r.command).into_bytes());
            false
        });
    }
}

/// Whether a resume command is typed now into a restored shell that has
/// (`spoke`) or has not written anything, `waited` after it started.
fn ready_to_type(spoke: bool, waited: Duration) -> bool {
    spoke || waited >= RESUME_WAIT
}

fn serve_connection(shared: &Arc<Shared>, conn: u64, mut stream: UnixStream) -> Result<()> {
    let instance = shared.lock().instance();
    protocol::send_hello(&mut stream, instance)?;
    // An upgrade comes in place of the client's hello, whatever version
    // it speaks.
    let first = protocol::read(&mut stream, protocol::FROM_CLIENT)?;
    if let Some(Frame::Upgrade(Upgrade::Ask { exe })) = first {
        return upgrade(shared, &mut stream, &exe);
    }
    protocol::check_hello(first, false)?;
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
            Frame::Upgrade(_) => bail!("an upgrade frame after the hello"),
            Frame::Message(other) => bail!("unexpected {other:?}"),
        };
        let answer = match request {
            Request::Resize(size) => {
                resize(shared, conn, size);
                continue;
            }
            Request::ResizeFloat(float, size) => {
                resize_float(shared, conn, &float, size);
                continue;
            }
            Request::Detach => {
                // Its writer ends; the client closes the connection.
                drop(shared.lock().detach(conn));
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
            Request::NewFloat {
                space,
                cwd,
                command,
                key,
            } => new_float(shared, &space, &cwd, &command, &key),
            Request::CloseFloat(float) => close_float(shared, &float),
            Request::CloseSpace(space) => {
                let closed = close_space(shared, &space);
                let last = matches!(closed, Ok(true));
                protocol::write(&mut stream, &reply(closed.map(|_| Answer::Done)))?;
                if last {
                    shut_down(shared);
                }
                continue;
            }
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

/// Hand every terminal to the binary `exe`, which this process becomes
/// (ADR 0009); it answers on `stream`. Returns only if it did not: the
/// server, unchanged, says why on `stream` (after `Probed`, if its
/// `exec` failed).
fn upgrade(shared: &Shared, stream: &mut UnixStream, exe: &Path) -> Result<()> {
    let Err(e) = hand_over(shared, stream, exe);
    let why = format!("{e:#}");
    eprintln!(
        "paddock server: not handing over to {}: {why}",
        exe.display()
    );
    protocol::write(stream, &Frame::Upgrade(Upgrade::Refused(why)))
}

/// Have `exe` check a record of what the server holds, then freeze the
/// server, write what it holds, save the layout, and become `exe`. Every
/// error leaves the server as it was. One upgrade at a time.
fn hand_over(shared: &Shared, stream: &mut UnixStream, exe: &Path) -> Result<Infallible> {
    static UPGRADING: Mutex<()> = Mutex::new(());
    let _one = UPGRADING.lock().unwrap_or_else(|e| e.into_inner());
    // The probe runs from `/`, and `exec` from the server's directory.
    if !exe.is_absolute() {
        bail!("{} is not an absolute path", exe.display());
    }
    // The probe reads the record as it is now, with the state let go:
    // the server serves on while it runs (up to 10 s).
    let mut probed = {
        let mut state = shared.lock();
        if state.closing {
            return Err(Ending.into());
        }
        let (record, snapshots) = handover::record(&mut state, shared.held);
        drop(state);
        handover::write(&shared.state_dir, &record, &snapshots)?
    };
    handover::probe(exe, &mut probed)?;
    drop(probed);
    let mut state = freeze(shared)?;
    // `layout.json` is never left half written, and holds the layout as
    // it is now, with the sessions seen: a new server that fails leaves
    // it for the next.
    let mut seen = shared.seen();
    seen.read_new();
    let mut saver = shared.saver.lock().unwrap_or_else(|e| e.into_inner());
    let (record, snapshots) = handover::record(&mut state, shared.held);
    let file = handover::write(&shared.state_dir, &record, &snapshots)?;
    let layout = saved::snapshot(&state).read_dirs(&mut seen);
    saver
        .write_now(layout)
        .context("cannot save the layout before handing over")?;
    let_go_for_upgrade(&mut state);
    protocol::write(stream, &Frame::Upgrade(Upgrade::Probed))?;
    eprintln!("paddock server: handing over to {}", exe.display());
    let fds = record.descriptors();
    Err(handover::exec(exe, &file, stream.as_raw_fd(), &fds))
}

/// Tell every attached client the server is upgrading, and let each go
/// once it has read that (or after [`LAST_WORDS`]): it attaches again to
/// the new binary, or to this server if the `exec` fails. Their writers
/// never need the state, so they are waited for with it locked.
fn let_go_for_upgrade(state: &mut State) {
    state.notify_all(&Notice::Upgrading);
    for s in std::mem::take(&mut state.attached) {
        state.let_go(s, LAST_WORDS);
    }
    for thread in state.take_leaving() {
        let _ = thread.join();
    }
}

/// The state, locked once every program has taken its input, so none is
/// lost with the process; an error if the server is ending, or some input
/// still waits after [`INPUT_WAIT`]. With it locked no reader holds output
/// it has not fed, and no waiter has reaped a program it has not marked.
fn freeze(shared: &Shared) -> Result<MutexGuard<'_, State>> {
    let start = Instant::now();
    loop {
        let state = shared.lock();
        if state.closing {
            return Err(Ending.into());
        }
        let busy = state.terminals().find(|(_, t)| !t.input.idle());
        let Some((_, term)) = busy else {
            return Ok(state);
        };
        if start.elapsed() > INPUT_WAIT {
            let program = term.command().unwrap_or_else(|| "a program".into());
            bail!("input still waits for {program} in {}", term.id);
        }
        drop(state);
        std::thread::sleep(Duration::from_millis(20));
    }
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
/// all through a writer thread of its own. Any other client of `space`
/// is let go.
fn attach(
    shared: &Shared,
    conn: u64,
    stream: &UnixStream,
    space: &SpaceId,
    size: (u16, u16),
) -> Result<()> {
    let mut subscriber = Subscriber::new(conn, space.clone(), stream)?;
    let mut state = shared.lock();
    // Once ending, the server waits only for the clients it is letting go:
    // one attached now would be cut off by its exit.
    if state.closing {
        return Err(Ending.into());
    }
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
    if let Some(old) = state.detach_space(space) {
        state.let_go(old, LAST_WORDS);
    }
    state.attached.push(subscriber);
    Ok(())
}

/// Show the space connection `conn` attached at `size`.
fn resize(shared: &Shared, conn: u64, size: (u16, u16)) {
    let mut state = shared.lock();
    let Some(space) = state.attached_space(conn) else {
        return;
    };
    let Ok(s) = state.space_mut(&space) else {
        return;
    };
    s.size = size;
    s.lay_out();
    state.notify(&space, Notice::Changed);
}

/// Show float `float` of the space connection `conn` attached at `size`.
fn resize_float(shared: &Shared, conn: u64, float: &FloatId, size: (u16, u16)) {
    let mut state = shared.lock();
    let Some(space) = state.attached_space(conn) else {
        return;
    };
    let Ok(s) = state.space_mut(&space) else {
        return;
    };
    let Some(terminal) = s.float(float).map(|f| f.terminal.clone()) else {
        return;
    };
    let Some(term) = s.hosted_mut(&terminal) else {
        return;
    };
    if term.size == size {
        return;
    }
    if let Err(e) = term.resize(size) {
        eprintln!("paddock server: {e:#}");
    }
    state.notify(&space, Notice::Changed);
}

/// A new tab named after `cwd`, holding `terminal`, with a new id.
fn new_tab_for(state: &mut State, cwd: &Path, terminal: TerminalId) -> Tab {
    let tab = TabId::new(format!("@{}", state.next()));
    Tab::new(tab, dir_name(&cwd.to_string_lossy()), terminal)
}

/// Start a space named `name`: a shell in `cwd`, at `size`.
fn new_space(shared: &Arc<Shared>, name: &str, cwd: &Path, size: (u16, u16)) -> Result<Answer> {
    let mut state = shared.lock();
    if state.closing {
        return Err(Ending.into());
    }
    state.check_space_name(name)?;
    let space = SpaceId::new(format!("${}", state.next()));
    let terminal = TerminalId::new(format!("%{}", state.next()));
    let tab = new_tab_for(&mut state, cwd, terminal.clone());
    let term = start_terminal(shared, &state, &space, terminal, LoginShell, cwd, size)?;
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
    let id = terminal.clone();
    let term = start_terminal(shared, &state, space, id, LoginShell, cwd, size)?;
    let s = state.space_mut(space)?;
    s.push_tab(tab, term);
    state.notify(space, Notice::Changed);
    Ok(Answer::Terminal(terminal))
}

/// Open a float on `key` in `space`, running `command` in `cwd` at the
/// space's size until the client sizes it to its box. It closes when the
/// command exits, which may be at once: it is still made, and never a tab.
fn new_float(
    shared: &Arc<Shared>,
    space: &SpaceId,
    cwd: &Path,
    command: &str,
    key: &str,
) -> Result<Answer> {
    let mut state = shared.lock();
    if state.closing {
        return Err(Ending.into());
    }
    let s = state.space(space)?;
    s.check_float_key(key)?;
    let size = s.size;
    let float = FloatId::new(format!("@{}", state.next()));
    let terminal = TerminalId::new(format!("%{}", state.next()));
    let id = terminal.clone();
    let program = Program::Command(command);
    let term = start_terminal(shared, &state, space, id, program, cwd, size)?;
    let made = HostedFloat {
        id: float.clone(),
        key: key.to_string(),
        terminal,
    };
    state.space_mut(space)?.push_float(made, term);
    state.notify(space, Notice::Changed);
    Ok(Answer::Float(float))
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
    let id = terminal.clone();
    let started = start_terminal(shared, &state, &space, id, LoginShell, cwd, place);
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

/// `program` on a new PTY in `cwd` (a shell, or a float's command),
/// with the hook variables of terminal `id`, and its threads. Called
/// with the state locked (`state`), so its reader waits until the
/// terminal is in it (and no other shell starts meanwhile, see
/// [`pty::spawn`]).
fn start_terminal(
    shared: &Arc<Shared>,
    state: &State,
    space: &SpaceId,
    id: TerminalId,
    program: Program<'_>,
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
    let mut pty = pty::spawn(&shared.shell, program, cwd, &shared.home, size, &env)?;
    let ready = (|| {
        let input = Input::start(&id, pty.master.try_clone()?);
        let reader = pty.master.try_clone()?;
        let name = ScrollbackName::new(state.instance(), &id);
        let term = HostedTerminal::new(id.clone(), name, &pty, input, size)?;
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
    // Reaped by its waiter, by pid: a server that took over (ADR 0009)
    // has no `Child` for it.
    watch(shared, space, id, reader, Some(term.pid));
    Ok(term)
}

/// Start terminal `id`'s reader on `reader`, its PTY, and its waiter for
/// program `pid` (`None` once reaped).
fn watch(shared: &Arc<Shared>, space: &SpaceId, id: TerminalId, reader: File, pid: Option<u32>) {
    let (drained, done) = mpsc::channel::<()>();
    let (s, sp, t) = (Arc::clone(shared), space.clone(), id.clone());
    std::thread::spawn(move || read_output(&s, &sp, &t, reader, drained));
    let (s, sp) = (Arc::clone(shared), space.clone());
    std::thread::spawn(move || wait_for_exit(&s, &sp, &id, pid, &done));
}

/// Feed terminal `id`'s output to its emulator and the attached client,
/// until its PTY closes; then drop `_drained`. It waits for output with
/// the state let go, but reads it only with the state locked, so whoever
/// holds the lock knows no output is half taken.
fn read_output(
    shared: &Shared,
    space: &SpaceId,
    id: &TerminalId,
    mut master: File,
    _drained: mpsc::Sender<()>,
) {
    let mut buf = vec![0; READ_CHUNK];
    loop {
        if !readable(&master) {
            return;
        }
        let mut state = shared.lock();
        // It has output, or has closed: this read does not wait.
        let n = match master.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
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
        state.send(space, frame);
    }
}

/// Whether `master` has output to read, or has closed, once it does
/// (waiting as long as it takes); false if it cannot be waited on.
fn readable(master: &File) -> bool {
    let mut poll = libc::pollfd {
        fd: master.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: poll reads and writes the one pollfd it is given.
        if unsafe { libc::poll(&mut poll, 1, -1) } >= 0 {
            return poll.revents & libc::POLLNVAL == 0;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// When terminal `id`'s program `pid` exits (at once if `None`: it has
/// been reaped), and its last output is read (or after [`DRAIN`], as a
/// program left in the background may hold the PTY open), forget the
/// terminal, and its space with its last terminal; end the server with
/// the last one.
fn wait_for_exit(
    shared: &Shared,
    space: &SpaceId,
    id: &TerminalId,
    pid: Option<u32>,
    drained: &mpsc::Receiver<()>,
) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        exits(pid);
        reaped(shared, space, id, pid);
    }
    // The reader only drops its end.
    let _ = drained.recv_timeout(DRAIN);
    let mut state = shared.lock();
    if let Ok(s) = state.space_mut(space) {
        if s.remove(id) {
            end_space(&mut state, space);
        } else {
            state.notify(space, Notice::Changed);
        }
    }
    let last = state.spaces.is_empty();
    if last {
        close(shared, &mut state);
    }
    drop(state);
    if last {
        shut_down(shared);
    }
}

/// Wait until child `pid` has exited, leaving it unreaped (`WNOWAIT`), so
/// it is reaped and marked under the state lock ([`reaped`]). Returns at
/// once if it is no child (any more).
fn exits(pid: i32) {
    loop {
        // SAFETY: an all-zero siginfo_t is a valid value for waitid to fill.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let options = libc::WEXITED | libc::WNOWAIT;
        // SAFETY: waitid writes one siginfo_t to the pointer.
        let id = libc::id_t::try_from(pid).unwrap_or_default();
        if unsafe { libc::waitid(libc::P_PID, id, &mut info, options) } == 0 {
            return;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Reap terminal `id`'s program `pid`, which has exited, and note it with
/// the state locked, so none is reaped and not noted when the server
/// hands over: never signal its pid again, and close a float it ran at
/// once, so its key takes a new one while its last output is still read.
fn reaped(shared: &Shared, space: &SpaceId, id: &TerminalId, pid: i32) {
    let mut state = shared.lock();
    // SAFETY: waitpid writes nothing through a null status pointer.
    unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
    let Ok(s) = state.space_mut(space) else {
        return;
    };
    if s.reaped(id) {
        state.notify(space, Notice::Changed);
    }
}

/// Take `space` out at once, as tmux's `kill-session` does, hanging up
/// what still runs in it (its floats, when its last tab has closed):
/// tell its client it ended, and every client that a space did, and let
/// its client go once it has read its last frames.
fn end_space(state: &mut State, space: &SpaceId) {
    let Some(i) = state.spaces.iter().position(|s| s.id == *space) else {
        return;
    };
    let gone = state.spaces.remove(i);
    for term in &gone.terminals {
        term.hang_up();
    }
    if state.current.as_ref() == Some(space) {
        state.current = None;
    }
    state.notify(space, Notice::Ended);
    // Nothing follows its end.
    if let Some(ended) = state.detach_space(space) {
        state.let_go(ended, LAST_WORDS);
    }
    state.notify_spaces();
}

/// End `space` at once; its shells' exits follow. True if it was the
/// last: the server takes no new space, and its caller ends it once it
/// has answered, as the last terminal ending does (not waiting for shells
/// that ignore the hangup).
fn close_space(shared: &Shared, space: &SpaceId) -> Result<bool> {
    let mut state = shared.lock();
    state.space(space)?;
    end_space(&mut state, space);
    let last = state.spaces.is_empty();
    if last {
        close(shared, &mut state);
    }
    Ok(last)
}

fn close_float(shared: &Shared, float: &FloatId) -> Result<Answer> {
    let terminal = shared.lock().float_terminal(float);
    let Some(terminal) = terminal else {
        bail!("no float {float}");
    };
    close_terminal(shared, &terminal)
}

fn close_terminal(shared: &Shared, terminal: &TerminalId) -> Result<Answer> {
    let state = shared.lock();
    let Some((_, term)) = state.terminals().find(|(_, t)| t.id == *terminal) else {
        bail!("no terminal {terminal}");
    };
    term.hang_up();
    Ok(Answer::Done)
}

/// Where the server writes what goes wrong, under `state_dir`.
pub fn log_path(state_dir: &Path) -> PathBuf {
    state_dir.join(LOG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::zombie_processes)] // Reaped by pid, below.
    fn an_exit_is_seen_without_reaping_the_program() {
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        exits(pid);
        // Still there to reap, with its status: only `reaped` reaps it.
        let mut status = 0;
        // SAFETY: waitpid writes one int to the pointer.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(libc::WEXITSTATUS(status), 3);
        // No child any more: it returns at once.
        exits(pid);
    }

    #[test]
    fn a_resume_is_typed_once_the_shell_spoke_or_after_the_wait() {
        assert!(!ready_to_type(false, Duration::ZERO));
        assert!(!ready_to_type(false, RESUME_WAIT - RESUME_POLL));
        assert!(ready_to_type(true, Duration::ZERO));
        assert!(ready_to_type(false, RESUME_WAIT));
    }
}
