//! What paddock and its server say to each other over the server's Unix
//! socket: length-prefixed frames. A frame is its length (a big-endian
//! `u32`, the kind byte included), a kind byte, and its body:
//!
//! - kind 0, a [`Message`] as JSON;
//! - kind 1, raw bytes for one terminal: the length of its id (one byte),
//!   the id, then the bytes. From the server they are the terminal's
//!   output (or, right after an attach, its snapshot);
//! - kind 2, a part of one input to a terminal, to the server: the length
//!   of its id, the id, how many bytes of the input follow this part (a
//!   big-endian `u64`), then the part's bytes. An input (a key, a paste)
//!   comes as parts of at most 64 KiB, in order, the last with 0 to
//!   follow: the server queues it once all has come, or refuses all of
//!   it, and discards one whose connection ends before its last part;
//! - kind 3, an [`Upgrade`] as JSON: the handover of the server's
//!   terminals to a new binary (ADR 0009). Unlike every other frame, it
//!   is frozen: it never changes with [`VERSION`], as it is sent exactly
//!   when the two sides may speak different versions.
//!
//! Both sides start with [`Message::Hello`], carrying [`VERSION`]. A side
//! that reads another version says so and goes no further; but a client
//! may answer the server's hello, whatever its version, with
//! [`Upgrade::Ask`] instead, and read one [`Upgrade`] back. Then the
//! client sends a [`Request`] and reads its [`Message::Reply`]; one
//! connection can ask many. After [`Request::Attach`] the connection
//! carries the space: its reply, snapshots, output and [`Notice`]s from
//! the server, and only input, [`Request::Resize`],
//! [`Request::ResizeFloat`] and [`Request::Detach`] from the client,
//! which get no reply (the server closes a connection
//! that asks anything else then).

use super::axis::Axis;
use super::layout::BorderHit;
use crate::backend::{FloatId, Restored, TerminalId};
use crate::backend::{ListedSpace, SpaceId, SpaceTerminal, Structure, TabId, TerminalDetails};
use crate::input::Direction;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;

/// The protocol's version: it changes whenever a message changes.
pub const VERSION: u32 = 2;

/// The largest frame the client reads: a snapshot of 50,000 lines of
/// scrollback, even with colours changing every few cells, fits.
pub const FROM_SERVER: usize = 256 << 20;
/// The largest frame the server reads: a request, or input (a paste).
pub const FROM_CLIENT: usize = 16 << 20;
const MESSAGE: u8 = 0;
const BYTES: u8 = 1;
const PART: u8 = 2;
const UPGRADE: u8 = 3;

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Message(Message),
    Bytes {
        terminal: TerminalId,
        data: Vec<u8>,
    },
    /// Part of one input: `rest` bytes of it follow.
    Part {
        terminal: TerminalId,
        rest: u64,
        data: Vec<u8>,
    },
    Upgrade(Upgrade),
}

/// The handover of the server's terminals to a new binary (ADR 0009), on
/// a frame of its own (kind 3). Frozen: every paddock that knows it reads
/// it the same, whatever protocol version it speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Upgrade {
    /// From a client, in place of its hello: hand every terminal to the
    /// binary `exe`, which then runs as the server.
    Ask { exe: PathBuf },
    /// From the server: `exe` read what it is handed, and the server
    /// becomes it now. `Done` or `Refused` follows, unless the new server
    /// fails before it can answer.
    Probed,
    /// From the server, once `exe` runs as it, still as pid `pid`.
    Done { pid: u32 },
    /// From the server: why it did not hand over. Before `Probed`, or
    /// after it from a failed `exec`, the server runs on unchanged; from
    /// the new server, it could not take over, and has ended.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// The first frame each side sends: its protocol version, its pid and,
    /// from the server, its instance id (a client sends 0).
    Hello {
        version: u32,
        pid: u32,
        #[serde(default)]
        instance: u32,
    },
    Request(Request),
    /// The answer to a request, or why there is none.
    Reply(Result<Answer, Refusal>),
    /// What changed in the attached space.
    Notice(Notice),
}

/// What paddock asks of its server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Spaces,
    NewSpace {
        name: String,
        cwd: PathBuf,
        size: (u16, u16),
    },
    SpaceName(SpaceId),
    RenameSpace(SpaceId, String),
    CloseSpace(SpaceId),
    SetRoot(SpaceId, PathBuf),
    Root(SpaceId),
    /// Note the space paddock shows, which `layout.json` keeps.
    SetCurrent(SpaceId),
    /// The space paddock showed last, if any (an [`Answer::SpaceOf`]).
    Current,
    /// How many spaces the server rebuilt from `layout.json` when it
    /// started; only the first to ask is told.
    TakeRestored,
    Structure(SpaceId),
    SpaceTerminals(SpaceId),
    /// A tab at the end of the space, current, its shell in `cwd`.
    NewTab {
        space: SpaceId,
        cwd: PathBuf,
    },
    RenameTab(TabId, String),
    Select(TabId, Option<TerminalId>),
    /// Swap two tabs of a space; the current one stays current.
    SwapTabs(TabId, TabId),
    /// Split a terminal (side by side if `columns`), the new shell in
    /// `cwd` taking the second half and the focus.
    Split {
        terminal: TerminalId,
        columns: bool,
        cwd: PathBuf,
    },
    FocusTowards(TerminalId, Direction),
    Spread(TerminalId),
    Zoom(TerminalId),
    /// Move a terminal's border in a direction by `cells`.
    ResizeSplit {
        terminal: TerminalId,
        dir: Direction,
        cells: u16,
    },
    /// The border at `(x, y)` of a tab, if any.
    BorderAt {
        tab: TabId,
        x: u16,
        y: u16,
    },
    /// Size the cell ending in `terminal` (a [`BorderHit`]'s) to `len`
    /// along `axis`.
    DragBorder {
        terminal: TerminalId,
        axis: Axis,
        len: u16,
    },
    CloseTerminal(TerminalId),
    /// A float on `key` in the space, running `command` in `cwd`; it
    /// closes when the command exits. At most one per key in a space.
    NewFloat {
        space: SpaceId,
        cwd: PathBuf,
        command: String,
        key: String,
    },
    CloseFloat(FloatId),
    TerminalDir(TerminalId),
    SpaceOf(TerminalId),
    TerminalCommands,
    TerminalPids,
    TerminalDetails,
    /// Take this connection as the client attached to the space, shown
    /// at `size`: any other client of that space is let go. With
    /// `scrollback_after`, each terminal's scrollback comes after its
    /// screens (see [`Answer::Attached`]); a client that never sends it
    /// is sent both at once, as before.
    Attach {
        space: SpaceId,
        size: (u16, u16),
        #[serde(default)]
        scrollback_after: bool,
    },
    /// Show the attached space at `size` (no reply).
    Resize((u16, u16)),
    /// Show a float of the attached space at `size`, apart from its
    /// tabs' (no reply).
    ResizeFloat(FloatId, (u16, u16)),
    /// Let the attached space go (no reply).
    Detach,
    /// End every terminal and the server.
    Stop,
}

/// Why the server answers a request with nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// It is ending, and makes no space: the client starts the next one.
    Ending,
    /// The request failed: why.
    Failed(String),
}

/// The server is ending, so it makes no space: the error a server
/// refuses with ([`Refusal::Ending`] on the wire), and the client reads
/// back. The client then starts the next server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending;

impl std::fmt::Display for Ending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the paddock server is ending")
    }
}

impl std::error::Error for Ending {}

/// The other side closed the connection where a frame was due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

impl std::fmt::Display for Closed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the other side closed the paddock server connection")
    }
}

impl std::error::Error for Closed {}

/// A request's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Done,
    Space(SpaceId),
    /// A terminal just started.
    Terminal(TerminalId),
    /// A float just started.
    Float(FloatId),
    Border(Option<BorderHit>),
    /// The space holding a terminal (or the current one); `None` if no
    /// space does.
    SpaceOf(Option<SpaceId>),
    Spaces(Vec<ListedSpace>),
    Text(Option<String>),
    Structure(Structure),
    Terminals(Vec<SpaceTerminal>),
    Commands(HashMap<TerminalId, String>),
    Pids(HashMap<u32, TerminalId>),
    Details(Vec<TerminalDetails>),
    /// What the server rebuilt when it started: spaces, and sessions it
    /// resumed; `None` if it rebuilt none, or another client was told.
    Restored(Option<Restored>),
    /// The space is attached: one snapshot frame follows for each
    /// terminal, in this order, at this size; then its output. With
    /// `scrollback_after` (asked for, and from a server that knows it),
    /// those snapshots leave the normal screen's scrollback out, and a
    /// second frame for each terminal, in the same order, follows them
    /// before any output: its whole snapshot, of the same moment. A
    /// client then draws the screens without waiting for the scrollback,
    /// which takes time to write and to read in proportion to its size.
    Attached {
        structure: Structure,
        terminals: Vec<(TerminalId, (u16, u16))>,
        #[serde(default)]
        scrollback_after: bool,
    },
}

/// What the server tells the attached client, in order with the output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notice {
    /// The space's structure changed (its tabs, splits, focus, zoom,
    /// sizes or names): read it again.
    Changed,
    /// A space was made or ended.
    SpacesChanged,
    /// The space's last terminal ended, or the server is ending and hangs
    /// every shell up; nothing follows.
    Ended,
    /// Input to `terminal` was refused, and `why` ("it would take what
    /// waits for vim past 64 MiB"). Once per episode (until its program
    /// has taken all that waited).
    InputRefused { terminal: TerminalId, why: String },
    /// The server hands its terminals to a new binary (ADR 0009): the
    /// connection closes next, and the new binary serves the same spaces.
    Upgrading,
}

/// Write `frame` and flush.
pub fn write(w: &mut impl Write, frame: &Frame) -> Result<()> {
    w.write_all(&encode(frame)?)?;
    w.flush().context("the paddock server connection closed")
}

/// `frame` as written: its length first.
pub fn encode(frame: &Frame) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    match frame {
        Frame::Message(m) => {
            body.push(MESSAGE);
            serde_json::to_writer(&mut body, m)?;
        }
        Frame::Bytes { terminal, data } => {
            body.push(BYTES);
            push_id(&mut body, terminal)?;
            body.extend_from_slice(data);
        }
        Frame::Part {
            terminal,
            rest,
            data,
        } => {
            body.push(PART);
            push_id(&mut body, terminal)?;
            body.extend_from_slice(&rest.to_be_bytes());
            body.extend_from_slice(data);
        }
        Frame::Upgrade(u) => {
            body.push(UPGRADE);
            serde_json::to_writer(&mut body, u)?;
        }
    }
    let len = u32::try_from(body.len()).context("frame too long")?;
    let mut out = len.to_be_bytes().to_vec();
    out.extend(body);
    Ok(out)
}

/// The next frame, of at most `max` bytes; `None` if the other side
/// closed the connection between frames (Linux reports one closed with
/// what we sent unread as reset, macOS as closed). A connection closed
/// within a frame, its length included, is an error.
pub fn read(r: &mut impl Read, max: usize) -> Result<Option<Frame>> {
    let mut len = [0; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Err(e) if got == 0 && e.kind() == ErrorKind::ConnectionReset => return Ok(None),
            Ok(0) => bail!("the paddock server connection closed within a frame's length"),
            Ok(n) => got += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > max {
        bail!("bad frame length {len} on the paddock server connection");
    }
    // Read as it comes: a false length allocates nothing up front.
    let mut body = Vec::new();
    r.take(len as u64).read_to_end(&mut body)?;
    if body.len() < len {
        bail!("the paddock server connection closed within a frame");
    }
    decode(&body).map(Some)
}

fn decode(body: &[u8]) -> Result<Frame> {
    match body.split_first() {
        Some((&MESSAGE, json)) => Ok(Frame::Message(serde_json::from_slice(json)?)),
        Some((&BYTES, body)) => {
            let (terminal, data) = take_id(body)?;
            Ok(Frame::Bytes {
                terminal,
                data: data.to_vec(),
            })
        }
        Some((&PART, body)) => {
            let (terminal, body) = take_id(body)?;
            let (rest, data) = body.split_first_chunk::<8>().context("short part frame")?;
            Ok(Frame::Part {
                terminal,
                rest: u64::from_be_bytes(*rest),
                data: data.to_vec(),
            })
        }
        Some((&UPGRADE, json)) => Ok(Frame::Upgrade(serde_json::from_slice(json)?)),
        _ => bail!("unknown frame kind"),
    }
}

/// A terminal id as frames carry it: its length (one byte), then it.
fn push_id(body: &mut Vec<u8>, terminal: &TerminalId) -> Result<()> {
    let id = terminal.as_str().as_bytes();
    body.push(u8::try_from(id.len()).context("terminal id too long")?);
    body.extend_from_slice(id);
    Ok(())
}

/// The terminal id `body` (a bytes or part frame's) starts with, and what
/// follows it.
fn take_id(body: &[u8]) -> Result<(TerminalId, &[u8])> {
    let (&len, body) = body.split_first().context("short frame")?;
    let len = usize::from(len);
    if body.len() < len {
        bail!("short frame");
    }
    let id = std::str::from_utf8(&body[..len]).context("terminal id")?;
    Ok((TerminalId::new(id), &body[len..]))
}

/// The running server speaks another protocol version: its pid and
/// version, as its hello tells. paddock offers to upgrade it, or to
/// restart it if it cannot hand over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtherVersion {
    pub pid: u32,
    pub version: u32,
}

impl OtherVersion {
    /// Which version the running server speaks, and this paddock.
    pub fn mismatch(&self) -> String {
        let Self { pid, version } = self;
        format!(
            "the running paddock server (pid {pid}) speaks protocol version {version}, \
             and this paddock speaks {VERSION}"
        )
    }
}

impl std::fmt::Display for OtherVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pid = self.pid;
        write!(
            f,
            "{}: close every terminal in it, or end it (kill {pid}), and start paddock again",
            self.mismatch()
        )
    }
}

impl std::error::Error for OtherVersion {}

/// The other side, as its hello tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub pid: u32,
    /// The server's instance id; 0 from a client.
    pub instance: u32,
}

/// Send our hello, with our `instance` id (0 from a client), and read
/// theirs; an error if it speaks another version. `server` names the
/// other side in that error. A side gone before our hello reaches it
/// (the pipe broken or reset) is [`Closed`], as one that closes unanswered.
pub fn hello(stream: &mut (impl Read + Write), server: bool, instance: u32) -> Result<Peer> {
    send_hello(stream, instance)?;
    let theirs = read(stream, FROM_CLIENT)?;
    check_hello(theirs, server)
}

/// Send our hello, with our `instance` id (0 from a client).
pub fn send_hello(stream: &mut impl Write, instance: u32) -> Result<()> {
    let ours = Message::Hello {
        version: VERSION,
        pid: std::process::id(),
        instance,
    };
    if let Err(e) = write(stream, &Frame::Message(ours)) {
        let gone = e.downcast_ref::<std::io::Error>().is_some_and(|e| {
            matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
        });
        return Err(if gone {
            anyhow::Error::new(Closed).context("no hello from the other side")
        } else {
            e
        });
    }
    Ok(())
}

/// The other side, from `frame`, the first it sent: an error unless it
/// is a hello of our version. `server` names the other side in that
/// error.
pub fn check_hello(frame: Option<Frame>, server: bool) -> Result<Peer> {
    match frame {
        Some(Frame::Message(Message::Hello {
            version,
            pid,
            instance,
        })) if version == VERSION => Ok(Peer { pid, instance }),
        Some(Frame::Message(Message::Hello { version, pid, .. })) if server => {
            Err(OtherVersion { pid, version }.into())
        }
        Some(Frame::Message(Message::Hello { version, .. })) => {
            bail!("a client speaks protocol version {version}, this server {VERSION}")
        }
        Some(_) => bail!("no hello from the other side"),
        None => Err(anyhow::Error::new(Closed).context("no hello from the other side")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn round_trip(frame: Frame) {
        let bytes = encode(&frame).unwrap();
        assert_eq!(
            read(&mut bytes.as_slice(), FROM_CLIENT).unwrap(),
            Some(frame)
        );
    }

    /// A connection the other side reset.
    struct Reset;

    impl Read for Reset {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(ErrorKind::ConnectionReset.into())
        }
    }

    /// A connection whose other side went away before our first write.
    struct Gone(ErrorKind);

    impl Read for Gone {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(self.0.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_hello_to_a_side_already_gone_is_closed() {
        for kind in [ErrorKind::BrokenPipe, ErrorKind::ConnectionReset] {
            let e = hello(&mut Gone(kind), true, 0).unwrap_err();
            assert!(e.downcast_ref::<Closed>().is_some(), "{kind:?}: {e:#}");
        }
        let e = hello(&mut Gone(ErrorKind::PermissionDenied), true, 0).unwrap_err();
        assert!(e.downcast_ref::<Closed>().is_none(), "{e:#}");
    }

    #[test]
    fn a_connection_reset_between_frames_reads_as_closed() {
        assert_eq!(read(&mut Reset, FROM_CLIENT).unwrap(), None);
        let mut cut = [0u8, 0].chain(Reset);
        assert!(read(&mut cut, FROM_CLIENT).is_err());
    }

    #[test]
    fn frames_read_back_as_written() {
        round_trip(Frame::Message(Message::Request(Request::NewSpace {
            name: "api".into(),
            cwd: "/w/api".into(),
            size: (80, 24),
        })));
        round_trip(Frame::Message(Message::Reply(Err(Refusal::Failed(
            "no".into(),
        )))));
        round_trip(Frame::Message(Message::Reply(Err(Refusal::Ending))));
        round_trip(Frame::Message(Message::Request(Request::ResizeSplit {
            terminal: TerminalId::new("%2"),
            dir: Direction::Left,
            cells: 2,
        })));
        round_trip(Frame::Message(Message::Reply(Ok(Answer::Border(Some(
            BorderHit {
                terminal: TerminalId::new("%3"),
                axis: Axis::Columns,
                start: 0,
            },
        ))))));
        round_trip(Frame::Message(Message::Request(Request::ResizeFloat(
            FloatId::new("@4"),
            (60, 20),
        ))));
        round_trip(Frame::Message(Message::Reply(Ok(Answer::Float(
            FloatId::new("@4"),
        )))));
        round_trip(Frame::Message(Message::Reply(Ok(Answer::Pids(
            HashMap::from([(42, TerminalId::new("%1"))]),
        )))));
        round_trip(Frame::Bytes {
            terminal: TerminalId::new("%1"),
            data: b"\x1b[31mred\0\xff".to_vec(),
        });
        round_trip(Frame::Part {
            terminal: TerminalId::new("%1"),
            rest: 1 << 40,
            data: b"\x1b[200~paste".to_vec(),
        });
        round_trip(Frame::Upgrade(Upgrade::Ask {
            exe: "/opt/homebrew/bin/paddock".into(),
        }));
        round_trip(Frame::Upgrade(Upgrade::Probed));
        round_trip(Frame::Upgrade(Upgrade::Done { pid: 42 }));
        round_trip(Frame::Upgrade(Upgrade::Refused("no".into())));
    }

    #[test]
    fn an_attach_without_scrollback_after_is_one_from_before_it() {
        // What a paddock from before ticket 64 sends, and its server
        // answers: the scrollback comes with the screens.
        let old = r#"{"Request":{"Attach":{"space":"$1","size":[80,24]}}}"#;
        let Message::Request(Request::Attach {
            scrollback_after, ..
        }) = serde_json::from_str(old).unwrap()
        else {
            panic!("an attach");
        };
        assert!(!scrollback_after);
        let attached = Message::Reply(Ok(Answer::Attached {
            structure: Structure::default(),
            terminals: Vec::new(),
            scrollback_after: true,
        }));
        let mut old = serde_json::to_value(&attached).unwrap();
        let fields = &mut old["Reply"]["Ok"]["Attached"];
        assert!(fields
            .as_object_mut()
            .unwrap()
            .remove("scrollback_after")
            .is_some());
        let message: Message = serde_json::from_value(old).unwrap();
        let Message::Reply(Ok(Answer::Attached {
            scrollback_after, ..
        })) = message
        else {
            panic!("attached: {message:?}");
        };
        assert!(!scrollback_after);
    }

    #[test]
    fn the_upgrade_frame_is_frozen() {
        // What every paddock since ADR 0009 writes and reads: changing it
        // breaks the handover between two versions.
        let body = |u: Upgrade| encode(&Frame::Upgrade(u)).unwrap()[4..].to_vec();
        let ask = body(Upgrade::Ask { exe: "/p".into() });
        assert_eq!(ask, b"\x03{\"Ask\":{\"exe\":\"/p\"}}");
        assert_eq!(body(Upgrade::Probed), b"\x03\"Probed\"");
        assert_eq!(
            body(Upgrade::Done { pid: 7 }),
            b"\x03{\"Done\":{\"pid\":7}}"
        );
        assert_eq!(
            body(Upgrade::Refused("x".into())),
            b"\x03{\"Refused\":\"x\"}"
        );
    }

    #[test]
    fn frames_follow_each_other_and_a_close_between_them_is_the_end() {
        let mut bytes = encode(&Frame::Message(Message::Notice(Notice::Changed))).unwrap();
        bytes.extend(encode(&Frame::Message(Message::Notice(Notice::Ended))).unwrap());
        let mut r = bytes.as_slice();
        assert!(read(&mut r, FROM_CLIENT).unwrap().is_some());
        assert!(read(&mut r, FROM_CLIENT).unwrap().is_some());
        assert_eq!(read(&mut r, FROM_CLIENT).unwrap(), None);
    }

    #[test]
    fn a_cut_or_oversized_frame_is_an_error() {
        let bytes = encode(&Frame::Message(Message::Notice(Notice::Changed))).unwrap();
        assert!(read(&mut &bytes[..bytes.len() - 1], FROM_CLIENT).is_err());
        for cut in 1..4 {
            assert!(read(&mut &bytes[..cut], FROM_CLIENT).is_err(), "{cut}");
        }
        let huge = (u32::try_from(FROM_CLIENT).unwrap() + 1).to_be_bytes();
        assert!(read(&mut huge.as_slice(), FROM_CLIENT).is_err());
        assert!(read(&mut huge.as_slice(), FROM_SERVER).is_err(), "cut");
        assert!(read(&mut [0, 0, 0, 1, 9].as_slice(), FROM_CLIENT).is_err());
    }

    #[test]
    fn hello_returns_the_other_sides_pid_or_names_its_version() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let other = std::thread::spawn(move || hello(&mut b, false, 77).unwrap());
        let ours = std::process::id();
        let server = hello(&mut a, true, 0).unwrap();
        assert_eq!((server.pid, server.instance), (ours, 77));
        let client = other.join().unwrap();
        assert_eq!((client.pid, client.instance), (ours, 0));

        let (mut a, mut b) = UnixStream::pair().unwrap();
        let old = Message::Hello {
            version: 0,
            pid: 7,
            instance: 9,
        };
        write(&mut b, &Frame::Message(old)).unwrap();
        let err = hello(&mut a, true, 0).unwrap_err().to_string();
        assert!(err.contains("pid 7") && err.contains("version 0"), "{err}");
    }
}
