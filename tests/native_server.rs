//! Ticket 40: paddock's own server hosts one terminal end to end. Every
//! test here picks the native backend in its config, so it runs on
//! either run of the suite.

mod harness;

use harness::backend::{alive, server_pid, NATIVE};
use harness::native_skip::NATIVE_NEEDS;
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
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
    let state = sandbox.state();
    assert_eq!(state.instance, Some(u32::try_from(pid).unwrap()));
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
    assert!(!paddock.wait_exit(), "paddock fails");
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
fn a_request_after_attaching_closes_the_connection_unanswered() {
    let sandbox = native();
    let paddock = ready(&sandbox);
    let space = sandbox.state().spaces[0].id.clone();
    let mut stream = UnixStream::connect(sandbox.state_dir().join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(harness::TIMEOUT))
        .expect("a read timeout");
    send_message(&mut stream, r#"{"Hello":{"version":1,"pid":1}}"#);
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

/// Every entry of the native skip list is a test, and its `--skip` (a
/// substring match) leaves out no test it does not list.
#[test]
fn the_native_skip_list_names_tests_and_skips_only_those() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut tests = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let file = path.file_stem().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        for test in text.split("#[test]\nfn ").skip(1) {
            let name = test.split('(').next().unwrap();
            tests.push((file.clone(), name.to_string()));
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
