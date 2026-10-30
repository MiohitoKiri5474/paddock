//! Fake native servers of another protocol version (999, or 1 from
//! before upgrades), listening on a sandbox's socket, and the frames they
//! speak: a big-endian `u32` length, a kind byte, then the body. Each
//! sends its hello first, as every server does.

use super::Sandbox;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

/// Write one frame of `kind` holding `json`, as the protocol does. A fake
/// server ignores an error: the client may have closed the connection
/// already, as one that only checks the hello does.
pub fn send_frame(stream: &mut UnixStream, kind: u8, json: &str) -> std::io::Result<()> {
    let len = u32::try_from(json.len() + 1).unwrap().to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(&[kind])?;
    stream.write_all(json.as_bytes())
}

/// The next frame's body, its kind first; `None` once the other side
/// closed the connection.
pub fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0; 4];
    stream.read_exact(&mut len).ok()?;
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

/// Listen on the sandbox's socket as a server of protocol version 999
/// whose pid is `pid` would. Each connection gets its hello; the server
/// then reads one frame and closes, as a server from before upgrades
/// does at an upgrade frame it does not know.
pub fn fake_older_server(sandbox: &Sandbox, pid: u32) {
    fake_server_of_version(sandbox, pid, 999);
}

/// [`fake_older_server`], speaking protocol `version`: 1 is a server
/// from before upgrades (v0.1.0-rc.1 and rc.2).
pub fn fake_server_of_version(sandbox: &Sandbox, pid: u32, version: u32) {
    std::fs::create_dir_all(sandbox.state_dir()).unwrap();
    let listener = UnixListener::bind(sandbox.state_dir().join("server.sock")).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let hello = format!(r#"{{"Hello":{{"version":{version},"pid":{pid}}}}}"#);
            if send_frame(&mut stream, 0, &hello).is_ok() {
                let _ = read_frame(&mut stream);
            }
        }
    });
}

/// Listen as a server of protocol version 999, pid 4242, that switches to
/// the new binary at an upgrade and ends there: it answers `Probed`, then
/// stops listening, its socket gone, and leaves no lock held.
pub fn fake_server_ending_at_upgrade(sandbox: &Sandbox) {
    std::fs::create_dir_all(sandbox.state_dir()).unwrap();
    let socket = sandbox.state_dir().join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let _ = send_frame(&mut stream, 0, r#"{"Hello":{"version":999,"pid":4242}}"#);
            if read_frame(&mut stream).is_some_and(|frame| frame[0] == 3) {
                let _ = send_frame(&mut stream, 3, r#""Probed""#);
                let _ = std::fs::remove_file(&socket);
                return;
            }
        }
    });
}
