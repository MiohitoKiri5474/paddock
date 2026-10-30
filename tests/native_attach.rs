//! Ticket 64: a paddock attaching to the native server draws a terminal
//! as soon with a full scrollback as with none: its screens come first,
//! its scrollback after, the output read on while it is rebuilt, and
//! copy mode and the wheel still reach every line of it. Ticket 65: with
//! several spaces, it draws the current one before attaching the others.
//! Every test here picks the native backend in its config, so it runs on
//! either run of the suite.

mod harness;

use harness::backend::NATIVE;
use harness::stall::wait_with_progress;
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// The copy-mode line's start.
const COPY: &str = " COPY  line";
/// A full scrollback (50,000 lines) and more, each line its number and
/// a colour change every few cells, which take long to write and to
/// read; then a line to wait for.
const FILL: &str = "seq 1 60000 | awk '{s=$1; for (i = 0; i < 12; i++) \
s = s sprintf(\"\\033[3%dm-\\033[4%dm=+\", i % 8, (i + 3) % 8); print s \"\\033[0m\"}'; \
echo FIL''LED-$((1+1))";
/// What [`FILL`] ends with.
const FILLED: &str = "FILLED-2";

fn native() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.write_config(NATIVE);
    sandbox
}

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Wait until `text` shows, as long as the screen still changes: under
/// load, drawing 60,000 lines can take longer than [`harness::TIMEOUT`],
/// which then counts from the last change.
fn wait_while_streaming(paddock: &Paddock, text: &str) {
    wait_with_progress(
        || paddock.screen(),
        |screen| screen.contains(text),
        |_| {
            format!(
                "no {text}, no longer changing; screen:\n{}",
                paddock.full_screen()
            )
        },
    );
}

/// A new paddock attached to the sandbox's server (the last one gone),
/// once it draws `text`, which its terminal shows.
fn attach_again(sandbox: &Sandbox, last: Paddock, text: &str) -> Paddock {
    drop(last);
    let again = sandbox.launch();
    again.wait_for(text, |s| s.contains(text));
    again
}

/// Write one message frame holding `json`.
fn send_message(stream: &mut UnixStream, json: &str) {
    let len = u32::try_from(json.len() + 1).unwrap().to_be_bytes();
    stream.write_all(&len).unwrap();
    stream.write_all(&[0]).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
}

/// The next frame's body, its kind byte first.
fn read_frame(stream: &mut UnixStream) -> Vec<u8> {
    let mut len = [0; 4];
    stream.read_exact(&mut len).expect("a frame");
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).expect("its body");
    body
}

/// The copy-mode view after going to the top of the history (`g`): the
/// terminal's top row and the copy-mode line (`line 1/M`). `g` is
/// pressed until the top holds the whole history, as the scrollback
/// may come after copy mode starts.
fn top_of_history(p: &mut Paddock, lines: Option<&str>) -> (String, String) {
    p.command(b"[");
    p.wait_for_text(COPY);
    let wanted = |s: &str| {
        let status = s.lines().find(|l| l.contains(COPY));
        status.is_some_and(|l| {
            l.contains("line 1/") && lines.is_none_or(|n| l.contains(&format!("line 1/{n} ")))
        })
    };
    p.wait_for_poking("the top of the history", b"g", wanted);
    let rows = p.rows();
    let status = rows.iter().find(|r| r.contains(COPY)).unwrap();
    let status = status.trim().to_string();
    let top = rows[1].clone();
    p.send(b"q");
    p.wait_for("copy mode left", |s| !s.contains(COPY));
    (top, status)
}

/// `M` of a copy-mode line ` COPY  line N/M ...`.
fn total_lines(status: &str) -> &str {
    let after = status.split("line ").nth(1).unwrap();
    let after = after.split('/').nth(1).unwrap();
    after.split_whitespace().next().unwrap()
}

#[test]
fn the_screens_come_before_the_scrollback() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line(FILL);
    wait_while_streaming(&paddock, FILLED);
    let space = sandbox.state().spaces[0].id.clone();
    drop(paddock);
    // Attached as a paddock since ticket 64 attaches: each terminal's
    // screens, with no scrollback, then its whole snapshot. No clock:
    // what decides the first draw is what comes first.
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    stream.set_read_timeout(Some(harness::TIMEOUT)).unwrap();
    assert_eq!(read_frame(&mut stream)[0], 0, "the server's hello");
    send_message(&mut stream, r#"{"Hello":{"version":2,"pid":1}}"#);
    let attach = format!(
        r#"{{"Request":{{"Attach":{{"space":"{space}","size":[80,24],"scrollback_after":true}}}}}}"#
    );
    send_message(&mut stream, &attach);
    let reply = String::from_utf8_lossy(&read_frame(&mut stream)).into_owned();
    assert!(reply.contains(r#""scrollback_after":true"#), "{reply}");
    let screens = read_frame(&mut stream);
    let whole = read_frame(&mut stream);
    assert_eq!((screens[0], whole[0]), (1, 1), "two snapshot frames");
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    // The screen's last lines, but not the oldest kept ones.
    assert!(text(&screens).contains("59990"), "the screen");
    assert!(!text(&screens).contains("\n20000"), "no scrollback");
    assert!(text(&whole).contains("\n20000"), "the scrollback");
    assert!(
        screens.len() * 100 < whole.len(),
        "{} {}",
        screens.len(),
        whole.len()
    );
}

#[test]
fn a_paddock_attaching_while_output_streams_keeps_up_as_its_scrollback_comes() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line(FILL);
    wait_while_streaming(&paddock, FILLED);
    paddock.type_line("yes");
    paddock.wait_for_text("y\ny\ny\n");
    // The new paddock reads `yes` on while it rebuilds the scrollback:
    // one that waited for that first could fall behind past the grace
    // and be let go.
    let mut again = attach_again(&sandbox, paddock, "y\ny\ny\n");
    let log = sandbox.state_dir().join("server.log");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(7) {
        let said = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(!said.contains("it stopped reading"), "let go: {said}");
        assert!(!again.full_screen().contains("lost the connection"));
        std::thread::sleep(Duration::from_millis(200));
    }
    again.send(b"\x03");
    again.type_line("echo back-$((1+1))");
    wait_while_streaming(&again, "back-2");
}

#[test]
fn after_attaching_copy_mode_and_the_wheel_reach_the_whole_scrollback() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("seq 1 60000; echo FIL''LED-$((1+1))");
    wait_while_streaming(&paddock, FILLED);
    // What the paddock that saw it all keeps: 50,000 lines and more.
    let (top, status) = top_of_history(&mut paddock, None);
    let lines = total_lines(&status).to_string();
    assert!(lines.parse::<usize>().unwrap() > 50_000, "{status}");
    assert!(top.parse::<u32>().is_ok(), "a number at the top: {top:?}");

    let mut again = attach_again(&sandbox, paddock, FILLED);
    assert_eq!(top_of_history(&mut again, Some(&lines)), (top, status));
    // The wheel scrolls back past the screen, deep into the numbers.
    let live: u32 = again
        .rows()
        .iter()
        .filter_map(|r| r.parse().ok())
        .min()
        .unwrap();
    let deep = |s: &str| {
        s.lines()
            .any(|l| l.parse::<u32>().is_ok_and(|n| n + 50 < live))
    };
    let start = Instant::now();
    while !deep(&again.screen()) {
        assert!(start.elapsed() < harness::TIMEOUT, "{}", again.screen());
        again.wheel(true, 10, 10);
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A gate the attach [`hold_attach`] holds waits at, until it opens.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn open(&self) {
        let (open, changed) = &*self.0;
        *open.lock().unwrap() = true;
        changed.notify_all();
    }

    fn wait(&self) {
        let (open, changed) = &*self.0;
        let mut open = open.lock().unwrap();
        while !*open {
            open = changed.wait(open).unwrap();
        }
    }
}

/// The next frame, its length and all; `None` once the connection ends.
fn next_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0; 4];
    stream.read_exact(&mut len).ok()?;
    let mut frame = len.to_vec();
    frame.resize(4 + u32::from_be_bytes(len) as usize, 0);
    stream.read_exact(&mut frame[4..]).ok()?;
    Some(frame)
}

/// Stand between paddock and the sandbox's server: the server's socket
/// moves aside, and each connection to `server.sock` goes through to it,
/// but a request to attach space `held` waits until the gate returned
/// opens. The server never looks at its socket's path again.
fn hold_attach(sandbox: &Sandbox, held: &str) -> Gate {
    let socket = sandbox.state_dir().join("server.sock");
    let real = sandbox.state_dir().join("server-real.sock");
    std::fs::rename(&socket, &real).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    let gate = Gate::default();
    let attach = format!(r#"{{"Attach":{{"space":"{held}""#);
    let held = gate.clone();
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(mut client) = client else { return };
            let Ok(mut server) = UnixStream::connect(&real) else {
                return;
            };
            let mut back = server.try_clone().unwrap();
            let mut to_client = client.try_clone().unwrap();
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut back, &mut to_client);
                let _ = to_client.shutdown(Shutdown::Both);
            });
            let (attach, gate) = (attach.clone(), held.clone());
            std::thread::spawn(move || {
                while let Some(frame) = next_frame(&mut client) {
                    if String::from_utf8_lossy(&frame).contains(&attach) {
                        gate.wait();
                    }
                    if server.write_all(&frame).is_err() {
                        break;
                    }
                }
                let _ = server.shutdown(Shutdown::Both);
            });
        }
    });
    gate
}

/// The space names the sidebar lists, top down.
fn sidebar_spaces(p: &Paddock) -> Vec<String> {
    p.sidebar()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("space-"))
        .collect()
}

/// Open a second space and show `text` in it: space-2, current.
fn second_space(sandbox: &Sandbox, paddock: &mut Paddock, text: &str) {
    paddock.command(b"C\r");
    paddock.wait_for("the second space", |_| sandbox.state().spaces.len() == 2);
    paddock.wait_for_text(PROMPT);
    paddock.type_line(&format!("echo {text}"));
    paddock.wait_for_text(&format!("{text}\n"));
}

/// Pick space-1 in the sidebar, once it lists both spaces in order.
fn pick_space_1(p: &mut Paddock) {
    p.wait_for("both spaces", |_| {
        sidebar_spaces(p) == ["space-1", "space-2"]
    });
    p.command(b"w");
    p.send(b"k\r");
}

#[test]
fn the_current_space_is_drawn_before_the_others_attach() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo FIRST-$((0+1))");
    paddock.wait_for_text("FIRST-1\n");
    second_space(&sandbox, &mut paddock, "SECOND-2");
    let first = sandbox.state().spaces[0].id.clone();
    drop(paddock);
    // No clock: space-1's attach is held until space-2, current, shows.
    // A paddock that attached every space before drawing never would.
    let gate = hold_attach(&sandbox, &first);
    let mut again = sandbox.launch();
    again.wait_for_text("SECOND-2\n");
    assert_eq!(sidebar_spaces(&again), ["space-2"], "{}", again.screen());
    gate.open();
    // In its place, above space-2, which stays current.
    pick_space_1(&mut again);
    again.wait_for_text("FIRST-1\n");
}

#[test]
fn a_space_attached_after_the_first_draw_has_its_whole_scrollback() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("seq 1 60000; echo FIL''LED-$((1+1))");
    wait_while_streaming(&paddock, FILLED);
    let (top, status) = top_of_history(&mut paddock, None);
    let lines = total_lines(&status).to_string();
    assert!(lines.parse::<usize>().unwrap() > 50_000, "{status}");
    second_space(&sandbox, &mut paddock, "SECOND-2");

    // space-2 is current: space-1 attaches after the first draw.
    let mut again = attach_again(&sandbox, paddock, "SECOND-2\n");
    pick_space_1(&mut again);
    again.wait_for_text(FILLED);
    assert_eq!(top_of_history(&mut again, Some(&lines)), (top, status));
}
