//! Ticket 40: paddock's own server hosts one terminal end to end. Every
//! test here picks the native backend in its config, so it runs on
//! either run of the suite.

mod harness;

use harness::backend::{alive, server_pid, NATIVE};
use harness::{Paddock, Sandbox, PROMPT};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
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
fn a_server_speaking_another_protocol_version_is_a_clear_error() {
    let sandbox = native();
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
