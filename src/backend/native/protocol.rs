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
//!   it, and discards one whose connection ends before its last part.
//!
//! Both sides start with [`Message::Hello`], carrying [`VERSION`]. A side
//! that reads another version says so and goes no further. Then the
//! client sends a [`Request`] and reads its [`Message::Reply`]; one
//! connection can ask many. After [`Request::Attach`] the connection
//! carries the space: its reply, snapshots, output and [`Notice`]s from
//! the server, and only input, [`Request::Resize`] and [`Request::Detach`]
//! from the client, which get no reply (the server closes a connection
//! that asks anything else then).

use crate::backend::TerminalId;
use crate::backend::{ListedSpace, SpaceId, SpaceTerminal, Structure, TabId, TerminalDetails};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;

/// The protocol's version: it changes whenever a message changes.
pub const VERSION: u32 = 1;

/// The largest frame the client reads: a snapshot of 50,000 lines of
/// scrollback, even with colours changing every few cells, fits.
pub const FROM_SERVER: usize = 256 << 20;
/// The largest frame the server reads: a request, or input (a paste).
pub const FROM_CLIENT: usize = 16 << 20;
const MESSAGE: u8 = 0;
const BYTES: u8 = 1;
const PART: u8 = 2;

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// The first frame each side sends: its protocol version and its pid.
    Hello {
        version: u32,
        pid: u32,
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
    Structure(SpaceId),
    SpaceTerminals(SpaceId),
    RenameTab(TabId, String),
    Select(TabId, Option<TerminalId>),
    CloseTerminal(TerminalId),
    TerminalDir(TerminalId),
    SpaceOf(TerminalId),
    TerminalCommands,
    TerminalPids,
    TerminalDetails,
    /// Take this connection as the one attached client of the space,
    /// shown at `size`: any other attached client is let go.
    Attach {
        space: SpaceId,
        size: (u16, u16),
    },
    /// Show the attached space at `size` (no reply).
    Resize((u16, u16)),
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
    /// The space holding a terminal; `None` if no space does.
    SpaceOf(Option<SpaceId>),
    Spaces(Vec<ListedSpace>),
    Text(Option<String>),
    Structure(Structure),
    Terminals(Vec<SpaceTerminal>),
    Commands(HashMap<TerminalId, String>),
    Pids(HashMap<u32, TerminalId>),
    Details(Vec<TerminalDetails>),
    /// The space is attached: one snapshot frame follows for each
    /// terminal, in this order, at this size; then its output.
    Attached {
        structure: Structure,
        terminals: Vec<(TerminalId, (u16, u16))>,
    },
}

/// What the server tells the attached client, in order with the output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Notice {
    /// The space's structure changed (a resize, a rename).
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
    }
    let len = u32::try_from(body.len()).context("frame too long")?;
    let mut out = len.to_be_bytes().to_vec();
    out.extend(body);
    Ok(out)
}

/// The next frame, of at most `max` bytes; `None` if the other side
/// closed the connection between frames. A connection closed within a
/// frame, its length included, is an error.
pub fn read(r: &mut impl Read, max: usize) -> Result<Option<Frame>> {
    let mut len = [0; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
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

/// The terminal id `body` starts with, and what follows it.
fn take_id(body: &[u8]) -> Result<(TerminalId, &[u8])> {
    let (&len, body) = body.split_first().context("short bytes frame")?;
    let len = usize::from(len);
    if body.len() < len {
        bail!("short bytes frame");
    }
    let id = std::str::from_utf8(&body[..len]).context("terminal id")?;
    Ok((TerminalId::new(id), &body[len..]))
}

/// Send our hello and read theirs: the other side's pid, or an error if it
/// speaks another version. `server` names the other side in that error.
pub fn hello(stream: &mut (impl Read + Write), server: bool) -> Result<u32> {
    let ours = Message::Hello {
        version: VERSION,
        pid: std::process::id(),
    };
    write(stream, &Frame::Message(ours))?;
    match read(stream, FROM_CLIENT)? {
        Some(Frame::Message(Message::Hello { version, pid })) if version == VERSION => Ok(pid),
        Some(Frame::Message(Message::Hello { version, pid })) if server => bail!(
            "the running paddock server (pid {pid}) speaks protocol version {version}, \
             and this paddock speaks {VERSION}: close every terminal in it, or end it \
             (kill {pid}), and start paddock again"
        ),
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
        let other = std::thread::spawn(move || hello(&mut b, false).unwrap());
        assert_eq!(hello(&mut a, true).unwrap(), std::process::id());
        assert_eq!(other.join().unwrap(), std::process::id());

        let (mut a, mut b) = UnixStream::pair().unwrap();
        let old = Message::Hello { version: 0, pid: 7 };
        write(&mut b, &Frame::Message(old)).unwrap();
        let err = hello(&mut a, true).unwrap_err().to_string();
        assert!(err.contains("pid 7") && err.contains("version 0"), "{err}");
    }
}
