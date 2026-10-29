//! The record a server hands to the binary it becomes on an upgrade
//! (ADR 0009): everything it holds, with the descriptors that pass the
//! `exec` (its lock, its listening socket and each terminal's PTY) named
//! by number. The record has its own [`FORMAT`], apart from the client
//! protocol's version, so two paddocks speaking different protocols can
//! hand over as long as the new one reads the old one's format.
//!
//! On disk, in a file removed from the state directory as soon as it is
//! open: the format (a big-endian `u32`), the JSON header's length (a
//! big-endian `u64`), the header ([`Record`]), then each terminal's
//! snapshot, in the header's order, as raw bytes of the length the
//! header gives, and then the input held in each terminal that holds
//! some ([`HandedHold`]), in the same order. A paddock from before
//! held input reads no `hold`: given held bytes, it finds more than the
//! header gives and refuses the record (the old server runs on); given
//! a hold with none, it reads the record and drops that resume.
//!
//! Before it `exec`s, the server runs the new binary as a probe
//! ([`probe`], `paddock server --check-handover`) on the record: one
//! that cannot read it refuses, and the old server runs on unchanged.

use super::layout::{Layout, SavedTree};
use super::space::{HostedFloat, Space};
use super::state::State;
use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{FloatId, Restored, SpaceId, TabId, TerminalId};
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The record's format: a new binary refuses a record of another. A
/// field added to the header gets `#[serde(default)]` and keeps it; a
/// change that cannot bumps it, keeping a reader for the one before, so
/// every later paddock can take over from every earlier one.
pub const FORMAT: u32 = 1;
/// What the probe prints, alone on a line, once it has read the record.
pub const PROBE_OK: &str = "paddock handover ok";
/// How long the probe gets.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// The most of each of the probe's outputs kept.
const PROBE_OUTPUT: usize = 64 << 10;
/// The record, in the state directory only until it is open.
const FILE: &str = "handover.tmp";
/// The longest header read: far more than any server holds.
const MAX_HEADER: u64 = 64 << 20;

/// What a server hands over, but its snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Its instance id, which its terminals' hook variables and event
    /// log name: the new server keeps it.
    pub instance: u32,
    /// The last number it gave an id: new ids go on after it.
    pub last_id: u64,
    pub started: bool,
    pub current: Option<SpaceId>,
    pub restored: Option<Restored>,
    /// The descriptor holding `server.lock`.
    pub lock: RawFd,
    /// The descriptor listening on `server.sock`.
    pub listener: RawFd,
    pub spaces: Vec<HandedSpace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedSpace {
    pub id: SpaceId,
    pub name: String,
    pub root: String,
    pub size: (u16, u16),
    pub current: usize,
    /// The tabs current before, the latest last.
    pub previous: Vec<TabId>,
    pub tabs: Vec<HandedTab>,
    pub floats: Vec<HandedFloat>,
    /// Every terminal of its tabs and floats.
    pub terminals: Vec<HandedTerminal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedTab {
    pub id: TabId,
    pub name: String,
    pub auto_name: bool,
    pub zoomed: bool,
    pub active: TerminalId,
    /// Its terminals, the one focused last at the end.
    pub focus_order: Vec<TerminalId>,
    pub splits: SavedTree<TerminalId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedFloat {
    pub id: FloatId,
    pub key: String,
    pub terminal: TerminalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedTerminal {
    pub id: TerminalId,
    /// Its program's pid: still a child of the server, as the `exec`
    /// keeps the process.
    pub pid: u32,
    pub tty: String,
    pub size: (u16, u16),
    /// Its program has been reaped: never signal its pid again.
    pub exited: bool,
    /// The descriptor of its PTY's master side.
    pub master: RawFd,
    /// The name its scrollback is saved under as the server ends; a
    /// record without one names it after the instance and the terminal.
    #[serde(default)]
    pub scrollback: Option<String>,
    /// How many bytes its snapshot takes, after the header.
    pub snapshot: usize,
    /// Its resume command, not typed yet, and the input held behind it
    /// (ticket 59); a record without one holds none.
    #[serde(default)]
    pub hold: Option<HandedHold>,
}

/// A restored terminal's resume command still to type, and the input
/// held behind it ([`super::terminal::Input::hold_behind`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandedHold {
    /// The command, with its Enter.
    pub first: Vec<u8>,
    /// How many bytes of input are held, after the snapshots.
    pub held: usize,
    /// Its shell has written something: the command may be typed.
    pub spoke: bool,
    /// The held input, read after the snapshots.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// The descriptors a server holds as long as it runs, which a handover
/// passes on: its lock and its listening socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub lock: RawFd,
    pub listener: RawFd,
}

impl Held {
    pub fn of(lock: &impl AsRawFd, listener: &impl AsRawFd) -> Self {
        Self {
            lock: lock.as_raw_fd(),
            listener: listener.as_raw_fd(),
        }
    }
}

/// What `state` hands over, with it locked, holding `held`; each
/// terminal's snapshot taken now, in the record's order, then the input
/// held in each terminal that holds some.
pub fn record(state: &mut State, held: Held) -> (Record, Vec<Vec<u8>>) {
    let Held { lock, listener } = held;
    let mut snapshots = Vec::new();
    let spaces = state.spaces.iter_mut();
    let spaces = spaces.map(|s| handed_space(s, &mut snapshots)).collect();
    let record = Record {
        instance: state.instance(),
        last_id: state.last_id(),
        started: state.started,
        current: state.current.clone(),
        restored: state.restored.clone(),
        lock,
        listener,
        spaces,
    };
    let holds = record.terminals().filter_map(|t| t.hold.as_ref());
    snapshots.extend(holds.map(|h| h.bytes.clone()));
    (record, snapshots)
}

fn handed_space(space: &mut Space, snapshots: &mut Vec<Vec<u8>>) -> HandedSpace {
    let tabs = space.tabs.iter().map(|tab| HandedTab {
        id: tab.id.clone(),
        name: tab.name().to_string(),
        auto_name: tab.auto_name(),
        zoomed: tab.zoomed(),
        active: tab.active().clone(),
        focus_order: tab.focus_order().to_vec(),
        splits: tab.layout().save(&mut TerminalId::clone),
    });
    let tabs = tabs.collect();
    let floats = space.floats.iter().map(|f| HandedFloat {
        id: f.id.clone(),
        key: f.key.clone(),
        terminal: f.terminal.clone(),
    });
    let floats = floats.collect();
    let previous = space.previous().to_vec();
    let terminals = space.terminals.iter_mut().map(|t| {
        let snapshot = t.snapshot();
        let handed = HandedTerminal {
            id: t.id.clone(),
            pid: t.pid,
            tty: t.tty.clone(),
            size: t.size,
            exited: t.exited,
            master: t.master.as_raw_fd(),
            scrollback: Some(t.saved_as.as_str().to_string()),
            snapshot: snapshot.len(),
            hold: t.input.held().map(|(first, bytes)| HandedHold {
                first,
                held: bytes.len(),
                spoke: t.spoke,
                bytes,
            }),
        };
        snapshots.push(snapshot);
        handed
    });
    HandedSpace {
        id: space.id.clone(),
        name: space.name.clone(),
        root: space.root.clone(),
        size: space.size,
        current: space.current,
        previous,
        tabs,
        floats,
        terminals: terminals.collect(),
    }
}

impl Record {
    /// The descriptors it names, which must pass the `exec`.
    pub fn descriptors(&self) -> Vec<RawFd> {
        let masters = self.terminals().map(|t| t.master);
        [self.lock, self.listener]
            .into_iter()
            .chain(masters)
            .collect()
    }

    fn terminals(&self) -> impl Iterator<Item = &HandedTerminal> {
        self.spaces.iter().flat_map(|s| &s.terminals)
    }

    /// An error unless a server can take it over: ids and descriptors
    /// each once, every tab's tree one a layout holds, and every terminal
    /// in exactly one tab or float of its space.
    fn check(&self) -> Result<()> {
        let mut fds = HashSet::new();
        for fd in self.descriptors() {
            ensure!(fd > 2, "descriptor {fd} is stdio");
            ensure!(fds.insert(fd), "descriptor {fd} twice");
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for space in &self.spaces {
            ensure!(ids.insert(space.id.to_string()), "id {} twice", space.id);
            ensure!(names.insert(&space.name), "two spaces named {}", space.name);
            space
                .check(&mut ids)
                .with_context(|| format!("space {}", space.name))?;
        }
        if let Some(current) = &self.current {
            ensure!(
                self.spaces.iter().any(|s| s.id == *current),
                "no current space {current}"
            );
        }
        Ok(())
    }
}

impl HandedSpace {
    fn check(&self, ids: &mut HashSet<String>) -> Result<()> {
        ensure!(self.current < self.tabs.len(), "no current tab");
        let mut placed = Vec::new();
        for tab in &self.tabs {
            ensure!(ids.insert(tab.id.to_string()), "id {} twice", tab.id);
            tab.splits.check()?;
            let terminals = tab.splits.terminals();
            ensure!(
                terminals.contains(&&tab.active),
                "tab {} focuses none",
                tab.id
            );
            placed.extend(terminals.into_iter().cloned());
        }
        for float in &self.floats {
            ensure!(ids.insert(float.id.to_string()), "id {} twice", float.id);
            placed.push(float.terminal.clone());
        }
        let mut hosted: Vec<_> = self.terminals.iter().map(|t| t.id.clone()).collect();
        for id in &hosted {
            ensure!(ids.insert(id.to_string()), "id {id} twice");
        }
        placed.sort();
        hosted.sort();
        ensure!(placed == hosted, "its tabs and floats hold other terminals");
        Ok(())
    }
}

/// Write `record` and its `snapshots` to a file in `dir`, removed from it
/// as soon as it is open (mode 0600); the file, at its start.
pub fn write(dir: &Path, record: &Record, snapshots: &[Vec<u8>]) -> Result<File> {
    let path = dir.join(FILE);
    let _ = std::fs::remove_file(&path);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
    let header = serde_json::to_vec(record)?;
    let mut out = BufWriter::new(&mut file);
    out.write_all(&FORMAT.to_be_bytes())?;
    out.write_all(&u64::try_from(header.len())?.to_be_bytes())?;
    out.write_all(&header)?;
    for snapshot in snapshots {
        out.write_all(snapshot)?;
    }
    out.flush().context("cannot write the handover record")?;
    drop(out);
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

/// The record `r` holds, and its snapshots; an error if it is of another
/// format, cut, followed by more, or no record a server can take over.
pub fn read(r: &mut impl Read) -> Result<(Record, Vec<Vec<u8>>)> {
    let mut format = [0; 4];
    r.read_exact(&mut format).context("no handover record")?;
    let format = u32::from_be_bytes(format);
    if format != FORMAT {
        bail!("the handover record is of format {format}, and this paddock reads {FORMAT}");
    }
    let mut len = [0; 8];
    r.read_exact(&mut len).context("a cut handover record")?;
    let len = u64::from_be_bytes(len);
    ensure!(len <= MAX_HEADER, "a handover header of {len} bytes");
    let record: Record = serde_json::from_slice(&take(r, len)?).context("the handover record")?;
    record.check().context("the handover record")?;
    let snapshots = record.terminals().map(|t| take(r, t.snapshot as u64));
    let snapshots = snapshots.collect::<Result<Vec<_>>>()?;
    let mut record = record;
    let holds = record.spaces.iter_mut().flat_map(|s| &mut s.terminals);
    for hold in holds.filter_map(|t| t.hold.as_mut()) {
        hold.bytes = take(r, hold.held as u64)?;
    }
    ensure!(
        r.read(&mut [0])? == 0,
        "more in the handover record than it holds"
    );
    Ok((record, snapshots))
}

/// The next `len` bytes of `r`, read as they come; an error if fewer.
fn take(r: &mut impl Read, len: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    r.take(len).read_to_end(&mut out)?;
    ensure!(out.len() as u64 == len, "a cut handover record");
    Ok(out)
}

/// How the server adopts a terminal of a space, handed over with its
/// snapshot, starting its threads.
pub type Adopt<'a> = dyn FnMut(&SpaceId, HandedTerminal, &[u8]) -> Result<HostedTerminal> + 'a;

/// The spaces of `record` in `state`, its ids going on, each terminal
/// adopted by `adopt` with its snapshot.
pub fn rebuild(
    state: &mut State,
    record: Record,
    snapshots: Vec<Vec<u8>>,
    adopt: &mut Adopt<'_>,
) -> Result<()> {
    state.started = record.started;
    state.current = record.current;
    state.restored = record.restored;
    let mut snapshots = snapshots.into_iter();
    for space in record.spaces {
        let mut terminals = Vec::new();
        for handed in space.terminals {
            let snapshot = snapshots.next().context("a snapshot missing")?;
            terminals.push(adopt(&space.id, handed, &snapshot)?);
        }
        let tabs = space.tabs.into_iter().map(|tab| {
            let layout = Layout::restore(tab.splits)?;
            let made = Tab::restore(
                tab.id,
                tab.name,
                tab.auto_name,
                layout,
                tab.active,
                tab.zoomed,
            );
            Ok(made.with_focus_order(tab.focus_order))
        });
        let tabs = tabs.collect::<Result<Vec<_>>>()?;
        let made = Space::restore(
            space.id,
            space.name,
            space.root,
            space.size,
            tabs,
            space.current,
            terminals,
        );
        let mut made = made.with_previous(space.previous);
        made.floats = space
            .floats
            .into_iter()
            .map(|f| HostedFloat {
                id: f.id,
                key: f.key,
                terminal: f.terminal,
            })
            .collect();
        state.spaces.push(made);
    }
    Ok(())
}

/// Run `exe server --check-handover` with `record` on its stdin, for at
/// most [`PROBE_TIMEOUT`]: Ok only if it exits 0 having printed
/// [`PROBE_OK`] (an older paddock prints its usage and fails). Leaves no
/// probe running, and `record` at its start.
pub fn probe(exe: &Path, record: &mut File) -> Result<()> {
    record.seek(SeekFrom::Start(0))?;
    let mut child = Command::new(exe)
        .args(["server", "--check-handover"])
        .current_dir("/")
        .stdin(Stdio::from(record.try_clone()?))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {}", exe.display()))?;
    // Read here, without waiting, and closed with the probe's end: a
    // program it left running with them open keeps nothing of ours.
    let mut out = Output::new(child.stdout.take().map(OwnedFd::from))?;
    let mut err = Output::new(child.stderr.take().map(OwnedFd::from))?;
    let start = Instant::now();
    let status = loop {
        out.drain();
        err.drain();
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > PROBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} did not answer within {} s",
                exe.display(),
                PROBE_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    record.seek(SeekFrom::Start(0))?;
    // What it wrote before it exited.
    let (out, err) = (out.finish(), err.finish());
    if status.success() && out.lines().any(|l| l == PROBE_OK) {
        return Ok(());
    }
    let why = err.lines().map(str::trim).find(|l| !l.is_empty());
    let why = why.map_or_else(
        || status.to_string(),
        |l| l.trim_start_matches("Error: ").into(),
    );
    bail!("{} cannot take over: {why}", exe.display())
}

/// One of the probe's output pipes, read without waiting: the first
/// [`PROBE_OUTPUT`] bytes kept, the rest read and dropped.
struct Output {
    pipe: Option<File>,
    text: Vec<u8>,
}

impl Output {
    fn new(pipe: Option<OwnedFd>) -> Result<Self> {
        if let Some(fd) = &pipe {
            let fd = fd.as_raw_fd();
            // SAFETY: fcntl with F_GETFL and F_SETFL only reads and sets
            // the descriptor's status flags.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(std::io::Error::last_os_error()).context("cannot read the probe");
            }
        }
        Ok(Self {
            pipe: pipe.map(File::from),
            text: Vec::new(),
        })
    }

    /// Read what waits in the pipe, until it closes or holds no more now.
    fn drain(&mut self) {
        let Some(pipe) = &mut self.pipe else {
            return;
        };
        let mut buf = [0; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break self.pipe = None,
                Ok(n) => {
                    let room = PROBE_OUTPUT.saturating_sub(self.text.len());
                    self.text.extend_from_slice(&buf[..n.min(room)]);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break self.pipe = None,
            }
        }
    }

    /// What it gave, its last bytes read; the pipe closed.
    fn finish(mut self) -> String {
        self.drain();
        String::from_utf8_lossy(&self.text).into_owned()
    }
}

/// Become `exe`, the server that takes `record` over and answers on the
/// connection `conn`, passing it `fds` (the record's descriptors). Returns
/// only if it could not: why, every descriptor closed on exec again.
pub fn exec(exe: &Path, record: &File, conn: RawFd, fds: &[RawFd]) -> anyhow::Error {
    let mut all = fds.to_vec();
    all.extend([record.as_raw_fd(), conn]);
    let passed = all.iter().try_for_each(|&fd| close_on_exec(fd, false));
    let err = match passed {
        Ok(()) => Command::new(exe)
            .args(["server", "--take-over"])
            .arg(record.as_raw_fd().to_string())
            .arg(conn.to_string())
            .exec(),
        Err(e) => e,
    };
    for &fd in &all {
        let _ = close_on_exec(fd, true);
    }
    anyhow::Error::new(err).context(format!("cannot run {}", exe.display()))
}

/// Close `fd` when this process `exec`s (`on`), or pass it on.
pub fn close_on_exec(fd: RawFd, on: bool) -> std::io::Result<()> {
    let flags = if on { libc::FD_CLOEXEC } else { 0 };
    // SAFETY: fcntl with F_SETFD only changes the descriptor's flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Close every descriptor this process holds but the standard three and
/// `keep`. Run by the new image before it starts a thread: on macOS,
/// `accept` and `openpty` set close-on-exec only once they return, so a
/// descriptor the old image opened as it `exec`ed passes over unnamed. One
/// such is a client's connection, accepted but never served: left open,
/// that client would wait for a hello until its read times out (for ever
/// if it reads with none), where a closed one makes it ask again.
pub fn close_strays(keep: &[RawFd]) -> Result<()> {
    // Read whole first: the listing's own descriptor is closed once read.
    let open: Vec<RawFd> = std::fs::read_dir("/dev/fd")
        .context("cannot list this process's descriptors")?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    for fd in open.into_iter().filter(|fd| *fd > 2 && !keep.contains(fd)) {
        if is_open(fd) {
            // SAFETY: nothing in this process owns `fd`: it is none of
            // those it was handed (`keep`), and no thread has started yet.
            unsafe { libc::close(fd) };
        }
    }
    Ok(())
}

/// Whether `fd` is an open descriptor.
pub fn is_open(fd: RawFd) -> bool {
    // SAFETY: fcntl with F_GETFD only reads the descriptor's flags.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

/// `paddock server --check-handover`: read the record on stdin, as a
/// server about to take it over would, and say so ([`PROBE_OK`]); an
/// error if it cannot.
pub fn check() -> Result<()> {
    read(&mut std::io::stdin().lock())?;
    println!("{PROBE_OK}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str) -> TerminalId {
        TerminalId::new(id)
    }

    fn terminal(id: &str, master: RawFd, snapshot: usize) -> HandedTerminal {
        HandedTerminal {
            id: t(id),
            pid: 100,
            tty: "/dev/ttys001".into(),
            size: (80, 24),
            exited: false,
            master,
            scrollback: Some("00000007-2".into()),
            snapshot,
            hold: None,
        }
    }

    fn tab(id: &str, terminal: &str) -> HandedTab {
        HandedTab {
            id: TabId::new(id),
            name: "api".into(),
            auto_name: true,
            zoomed: false,
            active: t(terminal),
            focus_order: vec![t(terminal)],
            splits: SavedTree::Terminal(t(terminal)),
        }
    }

    /// A space of two tabs, and a float.
    fn record() -> (Record, Vec<Vec<u8>>) {
        let space = HandedSpace {
            id: SpaceId::new("$1"),
            name: "api".into(),
            root: "/w/api".into(),
            size: (80, 24),
            current: 1,
            previous: vec![TabId::new("@4")],
            tabs: vec![tab("@4", "%2"), tab("@7", "%3")],
            floats: vec![HandedFloat {
                id: FloatId::new("@5"),
                key: "g".into(),
                terminal: t("%6"),
            }],
            terminals: vec![
                terminal("%2", 10, 3),
                terminal("%3", 11, 0),
                terminal("%6", 12, 2),
            ],
        };
        let record = Record {
            instance: 7,
            last_id: 7,
            started: true,
            current: Some(SpaceId::new("$1")),
            restored: Some(Restored {
                spaces: 1,
                sessions: 1,
                cannot_resume: Vec::new(),
            }),
            lock: 3,
            listener: 4,
            spaces: vec![space],
        };
        (record, vec![b"abc".to_vec(), Vec::new(), b"xy".to_vec()])
    }

    fn bytes(record: &Record, snapshots: &[Vec<u8>]) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let mut file = write(dir.path(), record, snapshots).unwrap();
        assert!(!dir.path().join(FILE).exists(), "gone once open");
        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn a_record_reads_back_as_written() {
        let (record, snapshots) = record();
        let written = bytes(&record, &snapshots);
        let read_back = read(&mut written.as_slice()).unwrap();
        assert_eq!(read_back, (record.clone(), snapshots));
        assert_eq!(record.descriptors(), vec![3, 4, 10, 11, 12]);
    }

    /// The sessions a restore could not resume are handed over with its
    /// counts, so the notice still names them after an upgrade; a record
    /// from a server that never wrote them reads as none (FORMAT 1).
    #[test]
    fn a_record_hands_over_the_kinds_a_restore_could_not_resume() {
        use crate::sessions::AgentKind;
        let (mut record, snapshots) = record();
        let cannot = vec![AgentKind::Gemini, AgentKind::Agy];
        record.restored.as_mut().unwrap().cannot_resume = cannot.clone();
        let written = bytes(&record, &snapshots);
        let (read_back, _) = read(&mut written.as_slice()).unwrap();
        assert_eq!(read_back.restored.unwrap().cannot_resume, cannot);

        let mut json = serde_json::to_value(&record).unwrap();
        let restored = json["restored"].as_object_mut().unwrap();
        assert!(restored.remove("cannot_resume").is_some());
        let older: Record = serde_json::from_value(json).unwrap();
        assert_eq!(older.restored.unwrap().cannot_resume, []);
    }

    /// A resume not typed yet, and the input held behind it, are handed
    /// over after the snapshots (ticket 59); a record from a server that
    /// never wrote them holds none.
    #[test]
    fn a_record_hands_over_a_pending_resume_and_the_input_held_behind_it() {
        let (mut record, mut raw) = record();
        let terminals = &mut record.spaces[0].terminals;
        for (i, held) in [(0, b"ec".to_vec()), (2, Vec::new())] {
            terminals[i].hold = Some(HandedHold {
                first: b"claude --resume x\r".to_vec(),
                held: held.len(),
                spoke: i == 0,
                bytes: held.clone(),
            });
            raw.push(held);
        }
        let written = bytes(&record, &raw);
        let read_back = read(&mut written.as_slice()).unwrap();
        assert_eq!(read_back, (record.clone(), raw[..3].to_vec()));
        let hold = read_back.0.spaces[0].terminals[0].hold.clone().unwrap();
        assert_eq!((hold.bytes, hold.spoke), (b"ec".to_vec(), true));

        let mut json = serde_json::to_value(&record).unwrap();
        let first = json["spaces"][0]["terminals"][0].as_object_mut().unwrap();
        assert!(first.remove("hold").is_some());
        let older: Record = serde_json::from_value(json).unwrap();
        assert_eq!(older.spaces[0].terminals[0].hold, None);
    }

    #[test]
    fn a_record_of_another_format_is_refused_by_its_number() {
        let (record, snapshots) = record();
        let mut written = bytes(&record, &snapshots);
        written[..4].copy_from_slice(&2u32.to_be_bytes());
        let err = read(&mut written.as_slice()).unwrap_err().to_string();
        assert!(err.contains("format 2") && err.contains("reads 1"), "{err}");
    }

    #[test]
    fn a_cut_or_padded_record_is_refused() {
        let (record, snapshots) = record();
        let written = bytes(&record, &snapshots);
        assert!(read(&mut &written[..written.len() - 1]).is_err(), "cut");
        let mut padded = written.clone();
        padded.push(0);
        assert!(read(&mut padded.as_slice()).is_err(), "padded");
    }

    #[test]
    fn a_record_no_server_could_take_over_is_refused() {
        let broken: [fn(&mut Record); 5] = [
            |r| r.spaces[0].terminals[1].master = 10,
            |r| r.spaces[0].terminals[2].id = t("%9"),
            |r| r.spaces[0].tabs[0].active = t("%6"),
            |r| r.spaces[0].current = 2,
            |r| r.current = Some(SpaceId::new("$9")),
        ];
        for (i, breaks) in broken.into_iter().enumerate() {
            let (mut record, snapshots) = record();
            breaks(&mut record);
            let written = bytes(&record, &snapshots);
            assert!(read(&mut written.as_slice()).is_err(), "{i}");
        }
    }
}
