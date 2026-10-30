//! What the native server holds: its spaces ([`Space`]), and the client
//! attached to each (paddock attaches every space). It answers every
//! request that reads, or changes a layout or a name; making and ending
//! terminals is the server's ([`super::server`]), as it starts their
//! threads. A change to an attached space's structure tells its client
//! ([`Notice::Changed`]), which reads it again.

use super::hook;
use super::limits;
use super::protocol::{self, Answer, Frame, Message, Notice, Request};
use super::queue::Queue;
use super::space::Space;
use super::tab::Tab;
use super::terminal::HostedTerminal;
use crate::backend::{
    FloatId, ListedSpace, Restored, SpaceId, SpaceTerminal, Structure, TabId, TerminalDetails,
    TerminalId,
};
use anyhow::{anyhow, bail, Result};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Bytes queued for an attached client past which it is behind: a client
/// that stays behind for [`GRACE`] has stopped reading (not just paused a
/// moment), and is let go.
pub const BEHIND: usize = 16 << 20;
const GRACE: Duration = Duration::from_secs(5);
/// Bytes queued past which the client is let go at once, so a program
/// writing fast never grows the server without bound meanwhile.
pub const MAX_QUEUED: usize = 128 << 20;

/// The client attached to a space: frames for it go through `queue`, in
/// order, to a thread that writes them.
pub struct Subscriber {
    /// Which connection it is.
    pub conn: u64,
    pub space: SpaceId,
    queue: Queue<Frame>,
    writer: JoinHandle<()>,
    /// Since when it has been [`BEHIND`].
    behind: Option<Instant>,
    /// Its connection, which only letting it go closes.
    stream: UnixStream,
}

impl Subscriber {
    /// Connection `conn` (`stream`) as the client attached to `space`,
    /// with its writer thread.
    pub fn new(conn: u64, space: SpaceId, stream: &UnixStream) -> Result<Self> {
        let mut out = stream.try_clone()?;
        // It never closes the connection: whoever lets the client go does.
        let (queue, writer) = Queue::start(size, move |frame: Frame| {
            protocol::write(&mut out, &frame).is_ok()
        });
        Ok(Self {
            conn,
            space,
            queue,
            writer,
            behind: None,
            stream: stream.try_clone()?,
        })
    }

    /// Queue `frame`; `Err` with why if the client is to be let go.
    pub fn queue(&mut self, frame: Frame) -> Result<(), &'static str> {
        let Some(queued) = self.queue.push(frame) else {
            return Err("it went away");
        };
        if queued <= limits::behind() {
            self.behind = None;
            return Ok(());
        }
        let since = *self.behind.get_or_insert_with(Instant::now);
        if queued > limits::max_queued() || since.elapsed() > GRACE {
            return Err("it stopped reading");
        }
        Ok(())
    }

    /// Let the client go once it has read what it was sent, or after
    /// `wait`: close its connection, which also ends a writer stuck on a
    /// client that does not read.
    pub fn let_go(self, wait: Duration) {
        let Self {
            queue,
            writer,
            stream,
            ..
        } = self;
        drop(queue);
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = writer.join();
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(wait);
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

/// About how many bytes `frame` takes queued.
fn size(frame: &Frame) -> usize {
    match frame {
        Frame::Bytes { data, .. } | Frame::Part { data, .. } => data.len(),
        Frame::Message(_) | Frame::Upgrade(_) => 64,
    }
}

pub struct State {
    /// The server's socket and instance id, which its terminals' hook
    /// variables name (see [`super::hook`]).
    socket: PathBuf,
    instance: u32,
    next_id: u64,
    pub spaces: Vec<Space>,
    /// The client attached to each space, one at most per space.
    pub attached: Vec<Subscriber>,
    /// A terminal ran: once none is left, the server ends.
    pub started: bool,
    /// The server is ending: it takes no new space.
    pub closing: bool,
    /// The space paddock shows, as it last said.
    pub current: Option<SpaceId>,
    /// What the server rebuilt from `layout.json` when it started (spaces,
    /// and sessions resumed), until a client takes it to say so.
    pub restored: Option<Restored>,
    /// The threads letting clients go (see [`Self::let_go`]).
    leaving: Vec<JoinHandle<()>>,
}

impl State {
    /// The state of a server listening on `socket`, with instance id
    /// `instance`, before its first space.
    pub fn new(socket: PathBuf, instance: u32) -> Self {
        Self {
            socket,
            instance,
            next_id: 0,
            spaces: Vec::new(),
            attached: Vec::new(),
            started: false,
            closing: false,
            current: None,
            restored: None,
            leaving: Vec::new(),
        }
    }

    /// The state of a server that took over from another (ADR 0009), as
    /// [`Self::new`], its new ids going on after `last_id`, the last the
    /// other gave.
    pub fn handed(socket: PathBuf, instance: u32, last_id: u64) -> Self {
        Self {
            next_id: last_id,
            ..Self::new(socket, instance)
        }
    }

    /// The last number [`Self::next`] gave.
    pub fn last_id(&self) -> u64 {
        self.next_id
    }

    /// The server's socket.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The server's instance id (see [`crate::backend::Instance`]).
    pub fn instance(&self) -> u32 {
        self.instance
    }

    /// The variables that tell a process it runs in terminal `id`.
    pub fn hook_env(&self, id: &TerminalId) -> Vec<(String, String)> {
        hook::env(&self.socket, self.instance, id)
    }

    /// A number for a new id, never used before by this server.
    pub fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn space(&self, id: &SpaceId) -> Result<&Space> {
        let found = self.spaces.iter().find(|s| s.id == *id);
        found.ok_or_else(|| anyhow!("no space {id}"))
    }

    /// An error if a space is named `name` already, in tmux's words,
    /// which paddock shows.
    pub fn check_space_name(&self, name: &str) -> Result<()> {
        if self.spaces.iter().any(|s| s.name == name) {
            bail!("duplicate session: {name}");
        }
        Ok(())
    }

    pub fn space_mut(&mut self, id: &SpaceId) -> Result<&mut Space> {
        let found = self.spaces.iter_mut().find(|s| s.id == *id);
        found.ok_or_else(|| anyhow!("no space {id}"))
    }

    /// The space holding tab `tab`, and the tab's index there.
    fn tab_mut(&mut self, tab: &TabId) -> Result<(&mut Space, usize)> {
        let found = self.spaces.iter_mut().find_map(|s| {
            let i = s.tab(tab)?;
            Some((s, i))
        });
        found.ok_or_else(|| anyhow!("no tab {tab}"))
    }

    /// The space holding `terminal`.
    pub fn space_of_mut(&mut self, terminal: &TerminalId) -> Result<&mut Space> {
        let found = self
            .spaces
            .iter_mut()
            .find(|s| s.hosted(terminal).is_some());
        found.ok_or_else(|| anyhow!("no terminal {terminal}"))
    }

    /// Every terminal, with its space.
    pub fn terminals(&self) -> impl Iterator<Item = (&Space, &HostedTerminal)> {
        self.spaces
            .iter()
            .flat_map(|s| s.terminals.iter().map(move |h| (s, h)))
    }

    fn terminal(&self, terminal: &TerminalId) -> Result<(&Space, &HostedTerminal)> {
        let found = self.terminals().find(|(_, h)| h.id == *terminal);
        found.ok_or_else(|| anyhow!("no terminal {terminal}"))
    }

    /// Queue `frame` for the client attached to `space`, if any. A client
    /// that has stopped reading (see [`BEHIND`]), or whose writer failed,
    /// is let go, so its frames never pile up.
    pub fn send(&mut self, space: &SpaceId, frame: Frame) {
        let Some(s) = self.attached.iter_mut().find(|s| s.space == *space) else {
            return;
        };
        let Err(why) = s.queue(frame) else { return };
        eprintln!("paddock server: letting connection {} go: {why}", s.conn);
        if let Some(s) = self.detach_space(space) {
            self.let_go(s, Duration::ZERO);
        }
    }

    /// Let client `s` go ([`Subscriber::let_go`]) on a thread of its own,
    /// which whoever ends the server waits for ([`Self::take_leaving`]):
    /// ending it at once would cut the client off before it reads its last
    /// frames, its space's end among them.
    pub fn let_go(&mut self, s: Subscriber, wait: Duration) {
        self.leaving.retain(|t| !t.is_finished());
        self.leaving
            .push(std::thread::spawn(move || s.let_go(wait)));
    }

    /// The threads still letting clients go, for the server's end to wait
    /// for.
    pub fn take_leaving(&mut self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut self.leaving)
    }

    /// Take out the first attached client `which` picks, if any.
    fn take(&mut self, which: impl Fn(&Subscriber) -> bool) -> Option<Subscriber> {
        let i = self.attached.iter().position(which)?;
        Some(self.attached.remove(i))
    }

    /// Take out the client of connection `conn`, if it is attached.
    pub fn detach(&mut self, conn: u64) -> Option<Subscriber> {
        self.take(|s| s.conn == conn)
    }

    /// Take out the client attached to `space`, if any.
    pub fn detach_space(&mut self, space: &SpaceId) -> Option<Subscriber> {
        self.take(|s| s.space == *space)
    }

    /// The space connection `conn` is attached to, if it is.
    pub fn attached_space(&self, conn: u64) -> Option<SpaceId> {
        let found = self.attached.iter().find(|s| s.conn == conn);
        found.map(|s| s.space.clone())
    }

    /// Tell every attached client `notice`.
    pub fn notify_all(&mut self, notice: &Notice) {
        let spaces: Vec<SpaceId> = self.attached.iter().map(|s| s.space.clone()).collect();
        for space in spaces {
            self.notify(&space, notice.clone());
        }
    }

    /// The terminal float `float` runs in, in any space.
    pub fn float_terminal(&self, float: &FloatId) -> Option<TerminalId> {
        let found = self.spaces.iter().find_map(|s| s.float(float));
        found.map(|f| f.terminal.clone())
    }

    /// Tell the client attached to `space`, if any.
    pub fn notify(&mut self, space: &SpaceId, notice: Notice) {
        self.send(space, Frame::Message(Message::Notice(notice)));
    }

    /// Tell every attached client a space was made or ended.
    pub fn notify_spaces(&mut self) {
        self.notify_all(&Notice::SpacesChanged);
    }

    pub fn structure(&self, id: &SpaceId) -> Result<Structure> {
        self.space(id).map(Space::structure)
    }

    /// Change the tab holding `terminal` with `change`, given the tab and
    /// its space's size; if that changed what it shows, size its
    /// terminals to their places and tell its client. A change that fails
    /// or does nothing tells nobody.
    fn change(
        &mut self,
        terminal: &TerminalId,
        change: impl FnOnce(&mut Tab, (u16, u16)) -> Result<()>,
    ) -> Result<Answer> {
        let space = self.space_of_mut(terminal)?;
        let i = space.tab_index(terminal)?;
        let before = space.tab_view(i);
        let size = space.size;
        let result = change(&mut space.tabs[i], size);
        if space.tab_view(i) != before {
            space.lay_out_tab(i);
            let id = space.id.clone();
            self.notify(&id, Notice::Changed);
        }
        result.map(|()| Answer::Done)
    }

    /// Change the space holding tab `tab` (its index `i` there) with
    /// `change`; if that changed its structure, size that tab's terminals
    /// and tell its client.
    fn change_tab(
        &mut self,
        tab: &TabId,
        change: impl FnOnce(&mut Space, usize) -> Result<()>,
    ) -> Result<Answer> {
        let (space, i) = self.tab_mut(tab)?;
        let before = space.structure();
        let result = change(space, i);
        if space.structure() != before {
            if let Some(i) = space.tab(tab) {
                space.lay_out_tab(i);
            }
            let id = space.id.clone();
            self.notify(&id, Notice::Changed);
        }
        result.map(|()| Answer::Done)
    }

    /// Name every tab that follows what runs in it, telling the attached
    /// client of a change.
    pub fn follow_names(&mut self) {
        let changed: Vec<SpaceId> = self
            .spaces
            .iter_mut()
            .filter_map(|s| s.follow_names().then(|| s.id.clone()))
            .collect();
        for id in changed {
            self.notify(&id, Notice::Changed);
        }
    }

    /// Every terminal of space `id`, in its tabs' order, then its
    /// floats'.
    fn space_terminals(&self, id: &SpaceId) -> Result<Vec<SpaceTerminal>> {
        let space = self.space(id)?;
        let tabs = space.tabs.iter().flat_map(Tab::terminals);
        let tabs = tabs.map(|t| (t, false));
        let floats = space.floats.iter().map(|f| (f.terminal.clone(), true));
        let listed = tabs.chain(floats).filter_map(|(t, float)| {
            let h = space.hosted(&t)?;
            Some(SpaceTerminal {
                id: h.id.clone(),
                path: h.dir().unwrap_or_default(),
                float,
            })
        });
        Ok(listed.collect())
    }

    /// The answer to a request that changes a tab or its splits, as
    /// `Ok(Ok(answer))`; any other request comes back as `Ok(Err(request))`
    /// for [`Self::answer`] to answer.
    fn answer_layout(&mut self, request: Request) -> Result<Result<Answer, Request>> {
        Ok(Ok(match request {
            Request::RenameTab(tab, name) => self.change_tab(&tab, |space, i| {
                space.tabs[i].rename(name);
                Ok(())
            })?,
            Request::Select(tab, terminal) => self.change_tab(&tab, |space, i| {
                if let Some(t) = &terminal {
                    if !space.tabs[i].contains(t) {
                        bail!("no terminal {t} in tab {tab}");
                    }
                    space.tabs[i].focus(t);
                }
                space.select(i);
                Ok(())
            })?,
            Request::SwapTabs(a, b) => self.change_tab(&a, |space, i| {
                let j = space.tab(&b);
                let j = j.ok_or_else(|| anyhow!("no tab {b} beside {a}"))?;
                space.swap(i, j);
                Ok(())
            })?,
            Request::BorderAt { tab, x, y } => {
                let (space, i) = self.tab_mut(&tab)?;
                Answer::Border(space.tabs[i].border_at(x, y, space.size))
            }
            Request::FocusTowards(t, dir) => self.change(&t, |tab, size| {
                tab.focus_towards(&t, dir, size);
                Ok(())
            })?,
            Request::Spread(t) => self.change(&t, |tab, size| tab.spread(&t, size))?,
            Request::Zoom(t) => self.change(&t, |tab, _| {
                tab.zoom(&t);
                Ok(())
            })?,
            Request::ResizeSplit {
                terminal: t,
                dir,
                cells,
            } => self.change(&t, |tab, size| tab.resize(&t, dir, cells, size))?,
            Request::DragBorder {
                terminal: t,
                axis,
                len,
            } => self.change(&t, |tab, size| tab.resize_to(&t, axis, len, size))?,
            other => return Ok(Err(other)),
        }))
    }

    /// The answer to a request that reads, or changes a layout or a name.
    pub fn answer(&mut self, request: Request) -> Result<Answer> {
        let request = match self.answer_layout(request)? {
            Ok(answer) => return Ok(answer),
            Err(request) => request,
        };
        Ok(match request {
            Request::Spaces => Answer::Spaces(
                self.spaces
                    .iter()
                    .map(|s| ListedSpace {
                        id: s.id.clone(),
                        name: s.name.clone(),
                    })
                    .collect(),
            ),
            Request::SpaceName(id) => Answer::Text(Some(self.space(&id)?.name.clone())),
            Request::RenameSpace(id, name) => {
                if self.space(&id)?.name != name {
                    self.check_space_name(&name)?;
                }
                self.space_mut(&id)?.name = name;
                self.notify(&id, Notice::Changed);
                Answer::Done
            }
            Request::SetRoot(id, dir) => {
                self.space_mut(&id)?.root = dir.to_string_lossy().into_owned();
                Answer::Done
            }
            Request::SetCurrent(id) => {
                self.space(&id)?;
                self.current = Some(id);
                Answer::Done
            }
            Request::Current => Answer::SpaceOf(self.current.clone()),
            Request::TakeRestored => Answer::Restored(self.restored.take()),
            Request::Root(id) => Answer::Text(self.space(&id).ok().map(|s| s.root.clone())),
            Request::Structure(id) => Answer::Structure(self.structure(&id)?),
            Request::SpaceTerminals(id) => Answer::Terminals(self.space_terminals(&id)?),
            Request::TerminalDir(t) => {
                Answer::Text(self.terminal(&t).ok().and_then(|(_, h)| h.dir()))
            }
            Request::SpaceOf(t) => {
                Answer::SpaceOf(self.terminal(&t).ok().map(|(space, _)| space.id.clone()))
            }
            Request::TerminalCommands => Answer::Commands(
                self.terminals()
                    .filter_map(|(_, h)| Some((h.id.clone(), h.command()?)))
                    .collect(),
            ),
            Request::TerminalPids => Answer::Pids(
                self.terminals()
                    .map(|(_, h)| (h.pid, h.id.clone()))
                    .collect(),
            ),
            Request::TerminalDetails => Answer::Details(
                self.terminals()
                    .map(|(_, h)| TerminalDetails {
                        id: h.id.clone(),
                        dir: h.dir().unwrap_or_default(),
                        command: h.command().unwrap_or_default(),
                        pid: h.pid,
                        tty: h.tty.clone(),
                        scrollback: u32::try_from(h.scrollback()).unwrap_or(u32::MAX),
                        hook_env: self.hook_env(&h.id),
                    })
                    .collect(),
            ),
            other => bail!("the server cannot answer {other:?} here"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The notices the client on `stream` reads, until none comes for a
    /// moment.
    fn notices(stream: &mut UnixStream) -> Vec<Notice> {
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut got = Vec::new();
        while let Ok(Some(frame)) = protocol::read(stream, protocol::FROM_SERVER) {
            if let Frame::Message(Message::Notice(n)) = frame {
                got.push(n);
            }
        }
        got
    }

    #[test]
    fn each_space_has_its_own_client_and_every_client_hears_of_spaces() {
        let mut state = State::new(PathBuf::from("/tmp/server.sock"), 1);
        let (one, two) = (SpaceId::new("$1"), SpaceId::new("$2"));
        let (server_a, mut client_a) = UnixStream::pair().unwrap();
        let (server_b, mut client_b) = UnixStream::pair().unwrap();
        let a = Subscriber::new(1, one.clone(), &server_a).unwrap();
        let b = Subscriber::new(2, two.clone(), &server_b).unwrap();
        state.attached.extend([a, b]);

        state.notify(&one, Notice::Changed);
        state.notify_spaces();

        assert_eq!(
            notices(&mut client_a),
            [Notice::Changed, Notice::SpacesChanged]
        );
        assert_eq!(notices(&mut client_b), [Notice::SpacesChanged]);
        assert_eq!(state.detach(2).map(|s| s.conn), Some(2));
        assert!(state.detach(2).is_none(), "taken out once");
        assert_eq!(state.detach_space(&one).map(|s| s.conn), Some(1));
        assert!(state.attached.is_empty());
    }

    #[test]
    fn a_client_let_go_has_read_its_end_once_its_thread_is_joined() {
        let mut state = State::new(PathBuf::from("/tmp/server.sock"), 1);
        let space = SpaceId::new("$1");
        let (server, mut client) = UnixStream::pair().unwrap();
        state
            .attached
            .push(Subscriber::new(1, space.clone(), &server).unwrap());
        state.notify(&space, Notice::Ended);
        let ended = state.detach_space(&space).unwrap();
        state.let_go(ended, Duration::from_secs(5));

        // What the server's end does before it exits.
        for thread in state.take_leaving() {
            thread.join().unwrap();
        }
        assert!(state.take_leaving().is_empty(), "taken once");
        // Everything was written before the connection closed.
        client.set_nonblocking(true).unwrap();
        let first = protocol::read(&mut client, protocol::FROM_SERVER).unwrap();
        assert_eq!(first, Some(Frame::Message(Message::Notice(Notice::Ended))));
        let end = protocol::read(&mut client, protocol::FROM_SERVER).unwrap();
        assert_eq!(end, None, "then the connection closed");
    }
}
