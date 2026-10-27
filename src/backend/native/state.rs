//! What the native server holds: its spaces, each with one tab and one
//! terminal so far, and the one client attached to a space. It answers
//! every request that only reads or renames; making and ending terminals
//! is the server's ([`super::server`]), as it starts their threads.

use super::protocol::{Answer, Frame, Message, Notice, Request};
use super::terminal::HostedTerminal;
use super::unsupported;
use crate::backend::{
    ListedSpace, SpaceId, SpaceTerminal, Split, Structure, Tab, TabId, TerminalDetails, TerminalId,
};
use anyhow::{anyhow, bail, Result};
use ratatui::layout::Rect;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

/// How many frames wait for the attached client at most (each at most a
/// read of output, so a few MiB): one that falls this far behind has
/// stopped reading, and is let go.
pub const QUEUE: usize = 256;

pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub root: String,
    pub tab: TabId,
    pub tab_name: String,
    /// The tab is named after its directory, not by the user.
    pub auto_name: bool,
    /// Its terminals: one so far.
    pub terminals: Vec<HostedTerminal>,
}

/// The attached client: frames for it go through `tx`, in order, to a
/// thread that writes them.
pub struct Subscriber {
    /// Which connection it is.
    pub conn: u64,
    pub space: SpaceId,
    pub tx: SyncSender<Frame>,
    pub writer: JoinHandle<()>,
    /// Its connection, which only letting it go closes.
    pub stream: UnixStream,
}

impl Subscriber {
    /// Let the client go once it has read what it was sent, or after
    /// `wait`: close its connection, which also ends a writer stuck on a
    /// client that does not read.
    pub fn let_go(self, wait: Duration) {
        let Self {
            tx, writer, stream, ..
        } = self;
        drop(tx);
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = writer.join();
            let _ = done.send(());
        });
        let _ = finished.recv_timeout(wait);
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

#[derive(Default)]
pub struct State {
    next_id: u64,
    pub spaces: Vec<Space>,
    pub attached: Option<Subscriber>,
    /// A terminal ran: once none is left, the server ends.
    pub started: bool,
    /// The server is ending: it takes no new space.
    pub closing: bool,
}

impl State {
    /// A number for a new id, never used before by this server.
    pub fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn space(&self, id: &SpaceId) -> Result<&Space> {
        let found = self.spaces.iter().find(|s| s.id == *id);
        found.ok_or_else(|| anyhow!("no space {id}"))
    }

    pub fn space_mut(&mut self, id: &SpaceId) -> Result<&mut Space> {
        let found = self.spaces.iter_mut().find(|s| s.id == *id);
        found.ok_or_else(|| anyhow!("no space {id}"))
    }

    fn tab_mut(&mut self, tab: &TabId) -> Result<&mut Space> {
        let found = self.spaces.iter_mut().find(|s| s.tab == *tab);
        found.ok_or_else(|| anyhow!("no tab {tab}"))
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

    /// Queue `frame` for the attached client, if it is attached to
    /// `space` (to any space if `None`). A client whose queue is full has
    /// stopped reading, and one whose writer failed has gone: either is
    /// let go, so its frames never pile up.
    pub fn send(&mut self, space: Option<&SpaceId>, frame: Frame) {
        let Some(s) = &self.attached else { return };
        if space.is_some_and(|space| s.space != *space) {
            return;
        }
        let why = match s.tx.try_send(frame) {
            Ok(()) => return,
            Err(TrySendError::Full(_)) => "it stopped reading",
            Err(TrySendError::Disconnected(_)) => "it went away",
        };
        if let Some(s) = self.attached.take() {
            eprintln!("paddock server: letting connection {} go: {why}", s.conn);
            std::thread::spawn(move || s.let_go(Duration::ZERO));
        }
    }

    /// Tell the client attached to `space`, if any.
    pub fn notify(&mut self, space: &SpaceId, notice: Notice) {
        self.send(Some(space), Frame::Message(Message::Notice(notice)));
    }

    /// Tell the attached client a space was made or ended.
    pub fn notify_spaces(&mut self) {
        let notice = Message::Notice(Notice::SpacesChanged);
        self.send(None, Frame::Message(notice));
    }

    /// Space `id`'s tab with its terminal, filling it.
    pub fn structure(&self, id: &SpaceId) -> Result<Structure> {
        let space = self.space(id)?;
        let term = space
            .terminals
            .first()
            .ok_or_else(|| anyhow!("{id} is empty"))?;
        let (cols, rows) = term.size;
        let tab = Tab {
            id: space.tab.clone(),
            name: space.tab_name.clone(),
            auto_name: space.auto_name,
            splits: vec![Split {
                id: term.id.clone(),
                rect: Rect::new(0, 0, cols, rows),
            }],
            active: term.id.clone(),
            zoomed: false,
        };
        Ok(Structure {
            name: space.name.clone(),
            tabs: vec![tab],
            current: 0,
            floats: Vec::new(),
            float_active: false,
        })
    }

    /// The answer to a request that reads or renames.
    pub fn answer(&mut self, request: Request) -> Result<Answer> {
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
                self.space_mut(&id)?.name = name;
                self.notify(&id, Notice::Changed);
                Answer::Done
            }
            Request::SetRoot(id, dir) => {
                self.space_mut(&id)?.root = dir.to_string_lossy().into_owned();
                Answer::Done
            }
            Request::Root(id) => Answer::Text(self.space(&id).ok().map(|s| s.root.clone())),
            Request::Structure(id) => Answer::Structure(self.structure(&id)?),
            Request::SpaceTerminals(id) => Answer::Terminals(
                self.space(&id)?
                    .terminals
                    .iter()
                    .map(|h| SpaceTerminal {
                        id: h.id.clone(),
                        path: h.dir().unwrap_or_default(),
                        float: false,
                    })
                    .collect(),
            ),
            Request::RenameTab(tab, name) => {
                let space = self.tab_mut(&tab)?;
                space.tab_name = name;
                space.auto_name = false;
                let id = space.id.clone();
                self.notify(&id, Notice::Changed);
                Answer::Done
            }
            Request::Select(tab, terminal) => {
                let (_, term) = self
                    .terminals()
                    .find(|(s, _)| s.tab == tab)
                    .ok_or_else(|| anyhow!("no tab {tab}"))?;
                if terminal.is_some_and(|t| t != term.id) {
                    bail!(unsupported("more than one terminal in a tab"));
                }
                Answer::Done
            }
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
                        hook_env: Vec::new(),
                    })
                    .collect(),
            ),
            other => bail!("the server cannot answer {other:?} here"),
        })
    }
}
