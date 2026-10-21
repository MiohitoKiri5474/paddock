//! Ticket 14: the centred prompt popup that asks for names (and, in
//! `close_fork.rs`, before a busy close).

mod harness;

use harness::{Paddock, Sandbox, PROMPT};
use std::time::Duration;

/// The dashboard's title line, as drawn while it is open.
const DASHBOARD: &str = "dashboard  j down  k up";

fn ready(sandbox: &Sandbox) -> Paddock {
    let paddock = sandbox.launch();
    paddock.wait_for_text(PROMPT);
    paddock
}

/// Window names of space-1, in order.
fn windows(sandbox: &Sandbox) -> Vec<String> {
    sandbox
        .tmux(&["list-windows", "-t", "space-1", "-F", "#{window_name}"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn wait_for_sessions(paddock: &Paddock, sandbox: &Sandbox, want: &[&str]) {
    paddock.wait_for(&format!("sessions {want:?}"), |_| {
        sandbox.session_names() == want
    });
}

#[test]
fn a_new_space_asks_for_its_name_and_enter_keeps_the_default() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    assert_eq!(sandbox.session_names(), ["space-1"], "nothing before Enter");
    paddock.send(b"\r");

    wait_for_sessions(&paddock, &sandbox, &["space-1", "space-2"]);
    paddock.wait_for_no_popup();
}

#[test]
fn a_new_space_takes_the_name_typed_and_esc_makes_none() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\x1b");
    paddock.wait_for_no_popup();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(sandbox.session_names(), ["space-1"]);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\x15api\r");

    wait_for_sessions(&paddock, &sandbox, &["space-1", "api"]);
    paddock.wait_for_text(PROMPT);
    paddock.type_line("echo in-api");
    paddock.wait_for_text("in-api\n");
}

#[test]
fn the_name_is_edited_with_arrows_home_end_delete_and_c_w() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]);

    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    // C-w deletes `home`; Home and End jump; Left, then Delete, removes `!`.
    paddock.send(b"\x17logs");
    paddock.send(b"\x1b[Hbuild ");
    paddock.send(b"\x1b[F!\x1b[D\x1b[3~");
    paddock.wait_for_popup("Rename tab", "build logs");
    paddock.send(b"\r");

    paddock.wait_for_tabs(&["1:build logs"]);
    assert_eq!(windows(&sandbox), ["build logs"]);
}

#[test]
fn pasted_text_goes_into_the_name() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"$");
    paddock.wait_for_popup("Rename space", "space-1");
    paddock.send(b"\x1b[200~-api\n\x1b[201~");
    paddock.wait_for_popup("Rename space", "space-1-api");
    paddock.send(b"\r");

    wait_for_sessions(&paddock, &sandbox, &["space-1-api"]);
}

#[test]
fn a_click_outside_the_popup_cancels_it_and_one_inside_does_not() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.wait_for_tabs(&["1:home"]);

    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    paddock.send(b"zzz");
    // The 40x3 box is centred on the 100x30 screen: columns 30-69, rows 13-15.
    paddock.click_screen(50, 14);
    std::thread::sleep(Duration::from_millis(200));
    paddock.wait_for_popup("Rename tab", "homezzz");

    paddock.click_screen(80, 25);

    paddock.wait_for_no_popup();
    assert_eq!(windows(&sandbox), ["home"]);
    paddock.type_line("echo after-click");
    paddock.wait_for_text("after-click\n");
}

#[test]
fn a_new_space_with_a_taken_name_shows_a_notice_after_the_popup_closes() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\x15space-1\r");

    // The popup closes; tmux's error shows in the tab bar (its start, as
    // much as fits).
    paddock.wait_for_no_popup();
    paddock.wait_for("error notice", |s| {
        s.lines().next().unwrap_or("").contains("tmux new-session")
    });
    assert_eq!(sandbox.session_names(), ["space-1"]);
}

#[test]
fn naming_a_new_space_from_the_sidebar_keeps_the_sidebar_focused() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"w");
    // space-1 (screen rows 1-2) is selected.
    paddock.wait_for("space-1 selected", |_| paddock.reversed(1, 5));

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\x1b");
    paddock.wait_for_no_popup();
    paddock.wait_for("space-1 still selected", |_| paddock.reversed(1, 5));

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.click_screen(80, 25);
    paddock.wait_for_no_popup();
    paddock.wait_for("space-1 selected after a click", |_| paddock.reversed(1, 5));

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\r");
    wait_for_sessions(&paddock, &sandbox, &["space-1", "space-2"]);
    paddock.wait_for_no_popup();
    paddock.wait_for("space-1 selected after Enter", |_| paddock.reversed(1, 5));
}

#[test]
fn naming_a_new_space_from_the_dashboard_keeps_the_dashboard_open() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    paddock.command(b"o");
    paddock.wait_for_text(DASHBOARD);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\x1b");
    paddock.wait_for_no_popup();
    paddock.wait_for_text(DASHBOARD);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.click_screen(80, 25);
    paddock.wait_for_no_popup();
    paddock.wait_for_text(DASHBOARD);

    paddock.command(b"C");
    paddock.wait_for_popup("New space", "space-2");
    paddock.send(b"\r");
    wait_for_sessions(&paddock, &sandbox, &["space-1", "space-2"]);
    paddock.wait_for_no_popup();
    paddock.wait_for_text(DASHBOARD);
}

#[test]
fn the_popup_border_takes_the_configured_accent() {
    let sandbox = Sandbox::new();
    sandbox.write_config("[colors]\ntab_current = 33\n");
    let mut paddock = ready(&sandbox);
    // The tab bar reads tabs by the default colours, so look for the name.
    paddock.wait_for("the tab named", |s| {
        s.lines().next().unwrap_or("").contains("home")
    });

    paddock.command(b",");
    paddock.wait_for_popup("Rename tab", "home");
    // The box's top-left corner: column 30, row 13 of the 100x30 screen.
    assert_eq!(paddock.colors(13, 30).0, vt100::Color::Idx(33));
}
