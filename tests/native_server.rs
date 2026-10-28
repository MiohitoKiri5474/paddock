//! Ticket 40: paddock's own server hosts one terminal end to end. Every
//! test here picks the native backend in its config, so it runs on
//! either run of the suite.

mod harness;

use harness::backend::{alive, server_pid, NATIVE};
use harness::native_skip::NATIVE_NEEDS;
use harness::stall::{finished, wait_with_progress, SEND_TIMEOUT};
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

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

/// Wait until `done` holds, or fail saying `what`.
fn wait_until(what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < harness::TIMEOUT, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait until file `path` holds `len` bytes, as long as it still grows:
/// many MiB through a terminal take a while under load, so only
/// [`harness::TIMEOUT`] without growing fails, saying `what`.
fn wait_for_len(path: &Path, len: u64, what: &str) {
    wait_with_progress(
        || std::fs::metadata(path).map_or(0, |m| m.len()),
        |&size| {
            assert!(size <= len, "{what}: {size} bytes, past {len}");
            size == len
        },
        |size| format!("{what}: {size} bytes of {len}"),
    );
}

#[test]
fn the_server_hosts_the_shell_in_the_sandbox_and_answers_debug_state() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo hosted");
    paddock.wait_for_text("hosted\n");

    let pid = server_pid(&sandbox.state_dir()).expect("a server holds the lock");
    assert!(alive(pid));
    assert_eq!(
        sandbox.sockets(),
        ["server.sock"],
        "only the server's socket"
    );
    // Its instance id and event log: tests/native_hooks.rs.
    assert!(sandbox.state().instance.is_some());
    let terminal = sandbox.terminal("space-1");
    assert!(terminal
        .tty
        .as_deref()
        .is_some_and(|t| t.starts_with("/dev/ttys")));
    assert_eq!(terminal.command.as_deref(), Some("zsh"));
    let mode = std::fs::metadata(sandbox.state_dir().join("server.sock")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(mode.permissions().mode() & 0o777, 0o600);
}

#[test]
fn shells_run_with_the_clean_environment() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("env | grep -c -E 'SUPACODE_|leaky-terminal|^TMUX='; echo env-checked");
    paddock.wait_for_text("env-checked");
    assert!(
        paddock.rows().iter().any(|l| l == "0"),
        "leaked variables:\n{}",
        paddock.screen()
    );
    paddock.type_line("echo term=$TERM agy=$AGY_CLI_OSC7501");
    paddock.wait_for_text("term=xterm-256color agy=on");
}

#[test]
fn the_server_answers_terminal_queries() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Each reply is read raw and shown without its escapes.
    let ask = |query: &str, len: usize| {
        format!(
            "stty raw -echo; printf '{query}'; dd bs=1 count={len} 2>/dev/null | tr -d '\\033'; stty sane; echo"
        )
    };
    paddock.type_line(&ask("\\033[c", 5));
    paddock.wait_for_text("[?6c");
    paddock.type_line(&ask("\\033[>1u\\033[?u", 5));
    paddock.wait_for_text("[?1u");
    let version = format!("paddock {}", env!("CARGO_PKG_VERSION"));
    paddock.type_line(&ask("\\033[>q", version.len() + 6));
    paddock.wait_for_text(&format!("P>|{version}\\"));
}

/// A full-screen program: the alternate screen, every mouse motion in SGR
/// and kitty's disambiguated keys, showing each byte it reads as `<n>`
/// until `q`, then back to the normal screen.
const FULL_SCREEN: &str =
    "printf '\\033[?1049h\\033[?1003h\\033[?1006h\\033[>1u\\033[HFULL-SCREEN\\r\\n'; \
stty raw -echo; while read -rs -k1 c && [ \"$c\" != q ]; do printf '<%d>' \"'$c\"; done; \
printf '\\033[<u\\033[?1003l\\033[?1006l\\033[?1049l'; stty sane";

#[test]
fn a_full_screen_program_survives_reattaching() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("echo before-the-program");
    paddock.wait_for_text("before-the-program\n");
    paddock.type_line(FULL_SCREEN);
    paddock.wait_for_text("FULL-SCREEN");
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let mut again = sandbox.launch();
    again.wait_for_text("FULL-SCREEN");
    assert!(
        !again.screen().contains("before-the-program"),
        "{}",
        again.screen()
    );
    // Kitty's flags came back: Esc is `CSI 27 u`, not a bare escape.
    again.send(b"\x1b");
    again.wait_for_text("<27><91><50><55><117>");
    // So did the mouse: a click is reported to the program.
    again.click(5, 3);
    again.wait_for_text("<27><91><60><48><59>");
    // Leaving the program shows the normal screen as it was.
    again.send(b"q");
    again.wait_for_text("before-the-program");
    again.type_line("echo back");
    again.wait_for_text("back\n");
}

/// A program that keeps rows 2-4 as its scroll region (as vim, less and
/// htop do), and after a key scrolls that region twice. (The quotes
/// keep the typed command from showing what the program draws.)
const SCROLL_REGION: &str = "printf '\\033[?1049h\\033[H'; \
for i in 1 2 3 4 5 6; do printf 'line-%d\\r\\n' $i; done; printf '\\033[2;4r\\033[4;1HREGION-SE''T'; \
stty raw -echo; read -rs -k1 c; printf '\\r\\nscrolled-%d\\r\\nscrolled-%d' 1 2; read -rs -k1 c; \
stty sane; printf '\\033[r\\033[?1049l'";

#[test]
fn a_scroll_region_survives_reattaching_at_the_same_size() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line(SCROLL_REGION);
    paddock.wait_for_text("REGION-SET");
    paddock.detach_keys();
    assert!(paddock.wait_exit());

    let mut again = sandbox.launch();
    again.wait_for_text("REGION-SET");
    again.send(b"x");
    again.wait_for_text("scrolled-2");
    // Only the region scrolled: the rows above and below it stay put.
    let want = [
        "line-1",
        "REGION-SET",
        "scrolled-1",
        "scrolled-2",
        "line-5",
        "line-6",
    ];
    again.wait_for("the region to scroll alone", |screen| {
        let rows: Vec<&str> = screen.lines().map(str::trim_end).collect();
        rows.windows(want.len()).any(|w| w == want)
    });
    again.send(b"x");
    again.type_line("echo after");
    again.wait_for_text("after\n");
}

/// Lines the streaming test writes.
const LINES: u32 = 400;

#[test]
fn reattaching_while_output_streams_loses_and_doubles_nothing() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Numbered lines, with pauses so the reattach lands mid-stream.
    paddock.type_line(&format!(
        "clear; for i in {{1..{LINES}}}; do echo n$i; (( i % 40 )) || sleep 0.2; done"
    ));
    paddock.wait_for_text("n80\n");
    paddock.detach_keys();
    assert!(paddock.wait_exit());
    let mut again = sandbox.launch();
    again.wait_for_text(&format!("n{LINES}\n"));

    // Read every line from the top of the scrollback down, a screen at a
    // time (screens overlap), and check they run on one by one.
    let numbers = |p: &Paddock| -> Vec<u32> {
        let rows = p.rows();
        rows.iter()
            .filter_map(|r| r.strip_prefix('n')?.parse().ok())
            .collect()
    };
    // Past the top: the view stops at the first line, `[N/N]`.
    for _ in 0..(LINES / 3 + 10) {
        again.wheel(true, 10, 10);
    }
    let at_top = |s: &str| {
        s.split('[').find_map(|part| {
            let (offset, len) = part.split(']').next()?.split_once('/')?;
            (offset == len).then(|| offset.parse::<u32>().ok())?
        })
    };
    again.wait_for("the top of the scrollback", |s| at_top(s).is_some());
    let mut offset = at_top(&again.screen()).expect("at the top");
    let mut seen = Vec::new();
    loop {
        let screen = numbers(&again);
        for pair in screen.windows(2) {
            assert_eq!(
                pair[1],
                pair[0] + 1,
                "lines out of order:\n{}",
                again.screen()
            );
        }
        seen.extend(screen);
        if offset == 0 {
            break;
        }
        // Nine notches: 27 lines, fewer than a screen.
        for _ in 0..9 {
            again.wheel(false, 10, 10);
        }
        offset = offset.saturating_sub(27);
        if offset > 0 {
            again.wait_for_text(&format!("[{offset}/"));
        } else {
            again.wait_for("the live screen", |s| !s.contains("/"));
        }
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, (1..=LINES).collect::<Vec<_>>(), "every line once");
}

#[test]
fn a_second_launch_takes_over_and_finds_the_same_shell() {
    let sandbox = native();
    let mut first = ready(&sandbox);
    first.type_line("echo marker-before-take-over");
    first.wait_for_text("marker-before-take-over\n");
    let before = sandbox.shells();

    let mut second = sandbox.launch();
    second.wait_for_text("Take over?");
    second.type_line("y");

    assert!(first.wait_exit(), "the first paddock detaches cleanly");
    second.wait_for_text("marker-before-take-over\n");
    assert_eq!(sandbox.shells(), before, "the same shell");
    second.type_line("echo after-take-over");
    second.wait_for_text("after-take-over\n");
}

#[test]
fn the_server_ends_with_its_last_shell() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    let pid = server_pid(&sandbox.state_dir()).expect("a server");

    paddock.type_line("exit");

    assert!(paddock.wait_exit());
    wait_until("the server ended", || !alive(pid));
    assert!(sandbox.sockets().is_empty(), "its socket is gone");
    // The next launch starts a new one.
    let mut again = ready(&sandbox);
    again.type_line("echo fresh");
    again.wait_for_text("fresh\n");
    assert_ne!(server_pid(&sandbox.state_dir()), Some(pid));
}

#[test]
fn a_launch_right_after_the_last_shell_exits_gets_a_new_server() {
    let sandbox = native();
    for round in 0..4 {
        let mut paddock = ready(&sandbox);
        paddock.type_line(&format!("echo round-{round}"));
        paddock.wait_for_text(&format!("round-{round}\n"));
        paddock.type_line("exit");
        assert!(paddock.wait_exit(), "round {round}");
    }
    let mut last = ready(&sandbox);
    last.type_line("echo still-fine");
    last.wait_for_text("still-fine\n");
}

/// Listen as an older server would: it reads paddock's hello and
/// answers with its own, version 999 and pid 4242.
fn fake_older_server(sandbox: &Sandbox) {
    std::fs::create_dir_all(sandbox.state_dir()).unwrap();
    let listener = UnixListener::bind(sandbox.state_dir().join("server.sock")).unwrap();
    // An older server: it reads paddock's hello and answers with its own.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut len = [0; 4];
            if stream.read_exact(&mut len).is_err() {
                continue;
            }
            let mut body = vec![0; u32::from_be_bytes(len) as usize];
            let _ = stream.read_exact(&mut body);
            let hello = br#"{"Hello":{"version":999,"pid":4242}}"#;
            let len = u32::try_from(hello.len() + 1).unwrap().to_be_bytes();
            let _ = stream.write_all(&len);
            let _ = stream.write_all(&[0]);
            let _ = stream.write_all(hello);
        }
    });
}

#[test]
fn a_server_speaking_another_protocol_version_is_a_clear_error() {
    let sandbox = native();
    fake_older_server(&sandbox);

    let mut paddock = sandbox.launch();

    paddock.wait_for_text("speaks protocol version 999");
    assert!(
        paddock.screen().contains("pid 4242"),
        "{}",
        paddock.screen()
    );
    paddock.wait_for_text("Restart it?");
    paddock.type_line("n");
    assert!(!paddock.wait_exit(), "declining fails");
    assert!(
        server_pid(&sandbox.state_dir()).is_none(),
        "no server of ours started"
    );
}

#[test]
fn paddock_worktree_stops_at_a_server_it_cannot_talk_to() {
    let sandbox = native();
    fake_older_server(&sandbox);

    let out = sandbox.run(&["worktree", "list"], &[], "");

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("speaks protocol version 999"), "{err}");
}

#[test]
fn debug_state_fails_at_a_server_it_cannot_talk_to() {
    let sandbox = native();
    fake_older_server(&sandbox);

    let out = sandbox.run(&["debug", "state", "--json"], &[], "");

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("speaks protocol version 999"), "{err}");
    assert!(out.stdout.is_empty(), "no state printed");
}

/// Write a JSON message frame (kind 0) as the protocol does.
fn send_message(stream: &mut UnixStream, json: &str) {
    let len = u32::try_from(json.len() + 1).unwrap().to_be_bytes();
    stream.write_all(&len).unwrap();
    stream.write_all(&[0]).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
}

/// The next frame's body; `None` once the server closed the connection.
fn read_frame(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut len = [0; 4];
    if let Err(e) = stream.read_exact(&mut len) {
        use std::io::ErrorKind::{TimedOut, WouldBlock};
        assert!(
            !matches!(e.kind(), WouldBlock | TimedOut),
            "no frame, no end"
        );
        return None;
    }
    let mut body = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

#[test]
fn pasting_into_a_program_that_never_reads_leaves_paddock_free_to_detach() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Raw mode: the PTY takes no more input once its queue is full.
    paddock.type_line("stty raw -echo; echo FLOOD''-READY; sleep 60");
    paddock.wait_for_text("FLOOD-READY");
    // Many pastes: far more input than the program will ever take.
    let paste = paste(16 << 10);
    // paddock takes it all: it never waits on the program.
    finished(paddock.send_in_background(paste.repeat(200)), "the paste");

    // Still answering keys: it detaches.
    finished(paddock.send_in_background(b"\x01d".to_vec()), "C-a d");
    assert!(paddock.wait_exit());
    let again = sandbox.launch();
    again.wait_for_text("FLOOD-READY");
}

/// One paste of `len` bytes of `x`.
fn paste(len: usize) -> Vec<u8> {
    [b"\x1b[200~".as_slice(), &vec![b'x'; len], b"\x1b[201~"].concat()
}

#[test]
fn a_paste_bigger_than_any_frame_reaches_a_program_that_reads_whole() {
    // Past the server's 16 MiB frame and 4 MiB input limits.
    const LEN: usize = 17 << 20;
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // Raw before the paste comes: a canonical line takes 1 KiB at most.
    paddock.type_line(&format!(
        "stty raw -echo; echo PASTE''-READY; head -c {LEN} > big.txt; stty sane; echo GOT''-IT"
    ));
    paddock.wait_for_text("PASTE-READY");
    finished(paddock.send_in_background(paste(LEN)), "the paste");
    let file = sandbox.path("home/big.txt");
    wait_for_len(&file, LEN as u64, "head to get it all");
    paddock.wait_for_text("GOT-IT");
    let got = std::fs::read(&file).unwrap();
    assert_eq!(got.len(), LEN);
    assert!(got.iter().all(|&b| b == b'x'), "every byte as pasted");
}

/// Write part `data` of an input to `terminal`, `rest` bytes of it to
/// follow (a kind 2 frame), as paddock does.
fn send_part(stream: &mut UnixStream, terminal: &str, rest: u64, data: &[u8]) {
    let mut body = vec![2, u8::try_from(terminal.len()).unwrap()];
    body.extend_from_slice(terminal.as_bytes());
    body.extend_from_slice(&rest.to_be_bytes());
    body.extend_from_slice(data);
    let len = u32::try_from(body.len()).unwrap().to_be_bytes();
    stream.write_all(&len).unwrap();
    stream.write_all(&body).unwrap();
}

/// A connection to the sandbox's server, past the hello, and its one
/// terminal, where `cat` (raw, so nothing is cooked) writes what it reads
/// to `got.txt` once `wait` seconds have passed.
fn cat_after(sandbox: &Sandbox, paddock: &mut Paddock, wait: u32) -> (UnixStream, String) {
    paddock.type_line(&format!(
        "stty raw -echo; echo CAT''-READY; sleep {wait}; cat > got.txt"
    ));
    paddock.wait_for_text("CAT-READY");
    connect(sandbox)
}

/// A connection to the sandbox's server, past the hello, and its one
/// terminal.
fn connect(sandbox: &Sandbox) -> (UnixStream, String) {
    let terminal = sandbox.terminal("space-1").id;
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    send_message(&mut stream, r#"{"Hello":{"version":2,"pid":1}}"#);
    assert!(read_frame(&mut stream).is_some(), "the server's hello");
    (stream, terminal)
}

/// Send `data` as one input in parts of at most 8 MiB, as paddock does in
/// 64 KiB ones.
fn send_input(stream: &mut UnixStream, terminal: &str, data: &[u8]) {
    let mut rest = data.len();
    for part in data.chunks(8 << 20) {
        rest -= part.len();
        send_part(stream, terminal, rest as u64, part);
    }
}

#[test]
fn an_input_in_parts_is_queued_whole_however_long_its_parts_take() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    let (mut stream, terminal) = cat_after(&sandbox, &mut paddock, 0);
    send_part(&mut stream, &terminal, 1006, b"\x1b[200~abc");
    std::thread::sleep(Duration::from_secs(1));
    let file = sandbox.path("home/got.txt");
    assert_eq!(
        std::fs::read(&file).unwrap_or_default(),
        b"",
        "not before its end"
    );
    send_part(
        &mut stream,
        &terminal,
        0,
        &[[b'y'; 1000].as_slice(), b"\x1b[201~"].concat(),
    );

    let want = format!("\x1b[200~abc{}\x1b[201~", "y".repeat(1000));
    wait_until("the whole paste", || {
        std::fs::read_to_string(&file).is_ok_and(|got| got == want)
    });
}

#[test]
fn a_key_typed_while_a_big_paste_waits_still_arrives() {
    const LEN: usize = 6 << 20;
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    let (mut stream, terminal) = cat_after(&sandbox, &mut paddock, 1);
    send_input(&mut stream, &terminal, &vec![b'x'; LEN]);
    send_input(&mut stream, &terminal, b"Z");

    let file = sandbox.path("home/got.txt");
    wait_for_len(&file, LEN as u64 + 1, "the paste and the key");
    let got = std::fs::read(&file).unwrap();
    assert!(got.ends_with(b"xZ"), "the key after the paste");
    assert!(!paddock.full_screen().contains("input refused"));
}

/// Whether the server closes `stream` (it reads to its end).
fn closed(stream: &mut UnixStream) -> bool {
    stream
        .set_read_timeout(Some(harness::TIMEOUT))
        .expect("a read timeout");
    while read_frame(stream).is_some() {}
    true
}

#[test]
fn a_client_whose_frames_lie_is_cut_off_and_its_input_never_arrives() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    let (mut lying, terminal) = cat_after(&sandbox, &mut paddock, 0);
    // 100 bytes said to follow, then 1 that says it is the last.
    send_part(&mut lying, &terminal, 100, b"abc");
    send_part(&mut lying, &terminal, 0, b"x");
    assert!(closed(&mut lying));
    // Output, which only the server sends.
    let (mut sending_output, _) = connect(&sandbox);
    let mut body = vec![1, u8::try_from(terminal.len()).unwrap()];
    body.extend_from_slice(terminal.as_bytes());
    body.extend_from_slice(b"out");
    let len = u32::try_from(body.len()).unwrap().to_be_bytes();
    sending_output.write_all(&len).unwrap();
    sending_output.write_all(&body).unwrap();
    assert!(closed(&mut sending_output));

    let (mut honest, _) = connect(&sandbox);
    send_input(&mut honest, &terminal, b"ok");
    let file = sandbox.path("home/got.txt");
    wait_until("the honest input", || {
        std::fs::read(&file).is_ok_and(|got| !got.is_empty())
    });
    assert_eq!(std::fs::read(&file).unwrap(), b"ok");
}

#[test]
fn an_input_cut_by_a_takeover_is_discarded_whole() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    let space = sandbox.state().spaces[0].id.clone();
    let attach = format!(r#"{{"Request":{{"Attach":{{"space":"{space}","size":[80,24]}}}}}}"#);
    let (mut first, terminal) = cat_after(&sandbox, &mut paddock, 0);
    send_message(&mut first, &attach);
    send_part(&mut first, &terminal, 100, b"\x1b[200~cut");
    // A second client takes over: the first is let go mid-paste.
    let (mut second, _) = connect(&sandbox);
    send_message(&mut second, &attach);
    assert!(closed(&mut first));
    send_input(&mut second, &terminal, b"after");

    let file = sandbox.path("home/got.txt");
    wait_until("the next input", || {
        std::fs::read(&file).is_ok_and(|got| !got.is_empty())
    });
    assert_eq!(std::fs::read(&file).unwrap(), b"after");
}

/// End the `sleep` the shell of space-1 runs, once it runs.
fn end_sleep(sandbox: &Sandbox) {
    let shell = sandbox.terminal("space-1").pid.expect("its shell's pid");
    let shell = shell.to_string();
    wait_until("a sleep to end", || {
        let pkill = std::process::Command::new("pkill")
            .args(["-P", &shell, "-x", "sleep"])
            .status();
        pkill.is_ok_and(|s| s.success())
    });
}

#[test]
fn an_input_past_the_cap_is_refused_whole_with_one_notice() {
    const CAP: usize = 64 << 20;
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    // `cat` reads only once the test ends the `sleep`: nothing is taken
    // while the inputs come, however long sending them takes.
    let (mut stream, terminal) = cat_after(&sandbox, &mut paddock, 600);
    // Queuing never waits on the program: a server that did would stop
    // reading, and a write would time out here.
    stream
        .set_write_timeout(Some(SEND_TIMEOUT))
        .expect("a write timeout");
    send_input(&mut stream, &terminal, &vec![b'x'; CAP - (4 << 20)]);
    // Past the cap: refused, both, with one notice.
    let paste = [b"\x1b[200~".as_slice(), &vec![b'p'; 8 << 20], b"\x1b[201~"].concat();
    send_input(&mut stream, &terminal, &paste);
    send_input(&mut stream, &terminal, &paste);
    send_input(&mut stream, &terminal, b"Z");

    paddock.wait_for_text("input refused: it would take what waits for sleep past 64 MiB");
    end_sleep(&sandbox);
    let file = sandbox.path("home/got.txt");
    wait_for_len(&file, (CAP - (4 << 20) + 1) as u64, "what was kept");
    let got = std::fs::read(&file).unwrap();
    assert!(got.ends_with(b"xZ"), "the key after what was kept");
    assert!(!got.contains(&b'p'), "nothing of the refused pastes");
    let log = std::fs::read_to_string(sandbox.state_dir().join("server.log")).unwrap();
    assert_eq!(log.matches("refusing input").count(), 1, "{log}");
}

#[test]
fn stopping_the_server_ends_paddock_without_a_lost_connection() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);

    harness::backend::stop_server(&sandbox.state_dir());

    assert!(paddock.wait_exit(), "a clean end");
    assert!(
        !paddock.full_screen().contains("lost the connection"),
        "{}",
        paddock.full_screen()
    );
}

/// Wait until `text` shows, as long as the screen still changes: under
/// load, drawing much output can take longer than [`harness::TIMEOUT`],
/// which then counts from the last change. A paddock let go, or stuck,
/// stops changing.
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

#[test]
fn a_paddock_stalled_a_moment_under_heavy_output_stays_attached() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("seq 1 1000000; echo ALL''-DONE");
    paddock.wait_for("numbers", |s| s.lines().any(|l| l.parse::<u32>().is_ok()));
    let start = Instant::now();
    paddock.stall_until(|| start.elapsed() > Duration::from_secs(2));
    wait_while_streaming(&paddock, "ALL-DONE");
    paddock.type_line("echo still-here");
    paddock.wait_for_text("still-here\n");
}

#[test]
fn a_paddock_the_server_lets_go_says_so() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.type_line("yes");
    paddock.wait_for_text("y\ny\ny\n");
    // Stopped while output piles up, until the server gives up on it.
    let log = sandbox.state_dir().join("server.log");
    let start = Instant::now();
    paddock.stall_until(|| {
        assert!(start.elapsed() < Duration::from_secs(60), "never let go");
        std::fs::read_to_string(&log).is_ok_and(|l| l.contains("it stopped reading"))
    });
    assert!(!paddock.wait_exit(), "it fails");
    paddock.wait_for("the farewell", |_| {
        paddock
            .full_screen()
            .contains("lost the connection to the paddock server")
    });
    // The shell runs on: a new paddock attaches to it.
    let mut again = sandbox.launch();
    again.send(b"\x03");
    again.type_line("echo back");
    again.wait_for_text("back\n");
}

#[test]
fn a_request_after_attaching_closes_the_connection_unanswered() {
    let sandbox = native();
    let paddock = ready(&sandbox);
    let space = sandbox.state().spaces[0].id.clone();
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(harness::TIMEOUT))
        .expect("a read timeout");
    send_message(&mut stream, r#"{"Hello":{"version":2,"pid":1}}"#);
    assert!(read_frame(&mut stream).is_some(), "the server's hello");
    let attach = format!(r#"{{"Request":{{"Attach":{{"space":"{space}","size":[80,24]}}}}}}"#);
    send_message(&mut stream, &attach);
    let reply = read_frame(&mut stream).expect("the attach reply");
    assert!(String::from_utf8_lossy(&reply).contains("Attached"));
    drop(paddock);

    send_message(&mut stream, r#"{"Request":"Spaces"}"#);
    // Whatever the stream still brings, no reply: then the end.
    while let Some(frame) = read_frame(&mut stream) {
        assert!(
            !String::from_utf8_lossy(&frame).contains("Reply"),
            "{}",
            String::from_utf8_lossy(&frame)
        );
    }
}

/// Ticket 42: a space has at most one float per key; another space has
/// its own.
#[test]
fn a_space_takes_one_float_per_key() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    paddock.command(b"C\r");
    wait_until("two spaces", || sandbox.state().spaces.len() == 2);
    let spaces: Vec<String> = sandbox.state().spaces.into_iter().map(|s| s.id).collect();
    let cwd = sandbox.path("").to_string_lossy().into_owned();
    let new_float = |space: &str| {
        let (mut stream, _) = connect(&sandbox);
        let request = format!(
            r#"{{"Request":{{"NewFloat":{{"space":"{space}","cwd":"{cwd}","command":"sleep 30","key":"G"}}}}}}"#
        );
        send_message(&mut stream, &request);
        String::from_utf8_lossy(&read_frame(&mut stream).expect("a reply")).into_owned()
    };

    assert!(new_float(&spaces[0]).contains("Float"));
    let again = new_float(&spaces[0]);
    assert!(again.contains("a float on G already"), "{again}");
    assert!(new_float(&spaces[1]).contains("Float"), "the other space's");
    let floats: Vec<usize> = sandbox
        .state()
        .spaces
        .iter()
        .map(|s| s.floats.len())
        .collect();
    assert_eq!(floats, [1, 1]);
}

/// Closing the last space ends the server at once, as its last shell
/// ending does, even with a shell that ignores the hangup.
#[test]
fn closing_the_last_space_ends_the_server_whatever_its_shells_do() {
    let sandbox = native();
    let mut paddock = ready(&sandbox);
    let pid = server_pid(&sandbox.state_dir()).expect("a server");
    paddock.type_line("trap '' HUP; echo HUP''-IGNORED");
    paddock.wait_for_text("HUP-IGNORED");
    let shell = sandbox.terminal("space-1").pid.expect("its shell's pid");
    // The shell that ignores the hangup is left to the test, pass or fail.
    let _shell = KillOnDrop(shell);
    let space = sandbox.state().spaces[0].id.clone();

    let (mut stream, _) = connect(&sandbox);
    send_message(
        &mut stream,
        &format!(r#"{{"Request":{{"CloseSpace":"{space}"}}}}"#),
    );
    let reply = read_frame(&mut stream).expect("a reply");
    assert!(String::from_utf8_lossy(&reply).contains("Done"));

    wait_until("the server ended", || !alive(pid));
    // Its end, not a lost connection: the server ends only once its
    // client has read that its space ended.
    assert!(paddock.wait_exit(), "{}", paddock.full_screen());
}

/// Kills the process with this pid (SIGKILL) when dropped.
struct KillOnDrop(u32);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-9", &self.0.to_string()])
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// The test functions in a test file's source: the `fn` after each
/// `#[test]`, past any other attributes. Tests in a nested module would
/// be named by its path, which this does not read: it refuses them.
fn tests_in(file: &str, text: &str) -> Vec<String> {
    let mut lines = text.lines().map(str::trim);
    let mut tests = Vec::new();
    while let Some(line) = lines.next() {
        let module = line.split_whitespace().any(|w| w == "mod") && line.ends_with('{');
        assert!(
            !module,
            "{file}: a nested module ({line}): name its tests by its path here"
        );
        if line != "#[test]" {
            continue;
        }
        let item = lines
            .by_ref()
            .find(|l| !l.is_empty() && !l.starts_with("#[") && !l.starts_with("//"))
            .unwrap_or_default();
        let name = item
            .strip_prefix("fn ")
            .and_then(|rest| rest.split('(').next())
            .unwrap_or_else(|| panic!("{file}: no fn after #[test]: {item}"));
        tests.push(name.to_string());
    }
    tests
}

/// Every entry of the native skip list is a test, and its `--skip` (a
/// substring match) leaves out no test it does not list.
#[test]
fn the_native_skip_list_names_tests_and_skips_only_those() {
    // native_skip::check() reads the running test's name from its thread.
    assert_eq!(
        std::thread::current().name(),
        Some("the_native_skip_list_names_tests_and_skips_only_those")
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut tests = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let file = path.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        for name in tests_in(&file, &text) {
            tests.push((file.clone(), name));
        }
    }
    let listed = |file: &str, name: &str| {
        let path = format!("{file}::{name}");
        NATIVE_NEEDS.iter().any(|(p, _)| *p == path)
    };
    for (path, tickets) in NATIVE_NEEDS {
        let (file, name) = path.split_once("::").unwrap();
        assert!(!tickets.is_empty(), "{path} needs no ticket: delete it");
        assert!(
            tests.iter().any(|(f, n)| f == file && n == name),
            "{path} is no test"
        );
        for (f, n) in &tests {
            assert!(
                listed(f, n) || !n.contains(name),
                "--skip {name} also skips {f}::{n}"
            );
        }
    }
}

#[test]
fn test_functions_are_found_past_other_attributes() {
    let text =
        "#[test]\n#[ignore = \"slow\"]\n\nfn one() {}\nfn helper() {}\n#[test]\nfn two() {}\n";
    assert_eq!(tests_in("x", text), ["one", "two"]);
}

#[test]
#[should_panic = "a nested module"]
fn a_nested_test_module_is_refused() {
    tests_in("x", "pub(crate) mod inner {\n#[test]\nfn one() {}\n}\n");
}
