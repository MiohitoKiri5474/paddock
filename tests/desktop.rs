//! Ticket 53: the desktop tools. Notifications go through `osascript` on
//! macOS and `notify-send` on Linux; copying through `pbcopy` on macOS,
//! `wl-copy` on Wayland (else `xclip`) and `xclip` on X11, unless
//! `PADDOCK_COPY_COMMAND` names another. The sandbox's stubs stand in for
//! all of them (first on `PATH`), so no test notifies or copies for real.
//! A tool that cannot be run is said once in the tab bar, and paddock goes
//! on; a copy tool that runs and fails is said on each copy.

mod harness;

use harness::alerts::{hooks, ready};
use harness::{Paddock, Sandbox};

const FIRST: &str = "space-1:0";
const CANNOT_NOTIFY: &str = "cannot notify: cannot run";
const CANNOT_COPY: &str = "cannot copy: cannot run";
const COPY: &str = " COPY  line";

/// A second tab, so the first one's session notifies.
fn look_away(paddock: &mut Paddock) {
    paddock.command(b"c");
    paddock.wait_for_current_tab("2:home");
    paddock.wait_for_text(harness::PROMPT);
}

/// Print a word and copy it in copy mode.
fn copy_word(p: &mut Paddock, word: &str) {
    p.type_line(&format!("echo {word}"));
    p.wait_for("output", |s| s.lines().any(|l| l == word));
    p.command(b"[");
    p.wait_for_text(COPY);
    // From the prompt: up to the word, select it, copy.
    p.send(b"k0ve");
    p.send(b"y");
    p.wait_for("copy mode left", |s| !s.contains(COPY));
}

/// Show that paddock still runs and handles keys: the shell answers
/// `echo` with `alive-<n>` (computed, so the typed line never matches).
fn still_runs(p: &mut Paddock, n: u32) {
    p.type_line(&format!("echo alive-$(({n}+0))"));
    p.wait_for_text(&format!("alive-{n}"));
}

/// Copy twice with the copy tool missing: one notice, then none; nothing
/// copied, and paddock goes on.
fn copy_without_a_tool(sandbox: &Sandbox, p: &mut Paddock, notice: &str) {
    copy_word(p, "first");
    p.wait_for_text(notice);
    still_runs(p, 1);
    assert!(!p.full_screen().contains(notice));

    // A notice would be drawn in the frame that leaves copy mode, the tab
    // bar before the copy-mode line.
    copy_word(p, "second");
    assert!(!p.full_screen().contains(notice));
    still_runs(p, 2);
    assert_eq!(sandbox.clipboard(), None);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_notifies_through_notify_send() {
    let sandbox = Sandbox::new();
    let mut paddock = ready(&sandbox);
    look_away(&mut paddock);

    hooks(&sandbox, FIRST, &["UserPromptSubmit", "PermissionRequest"]);

    assert_eq!(sandbox.wait_for_notifications(1), ["repo is waiting"]);
    assert_eq!(
        sandbox.desktop_calls(),
        ["notify-send -- paddock repo is waiting"]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_copies_with_wl_copy_on_wayland() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    sandbox.set_env("WAYLAND_DISPLAY", Some("wayland-test"));
    let mut p = ready(&sandbox);

    copy_word(&mut p, "wayland");

    sandbox.wait_for_clipboard("wayland");
    assert_eq!(sandbox.desktop_calls(), ["wl-copy"]);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_copies_with_xclip_on_wayland_without_wl_copy() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    sandbox.set_env("WAYLAND_DISPLAY", Some("wayland-test"));
    sandbox.remove_tools(&["wl-copy"]);
    let mut p = ready(&sandbox);

    copy_word(&mut p, "xwayland");

    sandbox.wait_for_clipboard("xwayland");
    assert_eq!(sandbox.desktop_calls(), ["xclip -selection clipboard"]);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_copies_with_xclip_without_wayland() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    let mut p = ready(&sandbox);

    copy_word(&mut p, "x11");

    sandbox.wait_for_clipboard("x11");
    assert_eq!(sandbox.desktop_calls(), ["xclip -selection clipboard"]);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_without_xclip_on_its_path_says_so_once() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    sandbox.remove_tools(&["xclip"]);
    let mut p = ready(&sandbox);

    copy_without_a_tool(&sandbox, &mut p, "cannot copy: cannot run xclip");
    assert!(sandbox.desktop_calls().is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_without_notify_send_on_its_path_says_so() {
    let sandbox = Sandbox::new();
    sandbox.remove_tools(&["notify-send"]);
    let mut paddock = ready(&sandbox);
    look_away(&mut paddock);

    hooks(&sandbox, FIRST, &["UserPromptSubmit", "PermissionRequest"]);
    paddock.wait_for_text("cannot notify: cannot run notify-send");
    assert!(sandbox.notifications().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_copies_with_pbcopy() {
    let sandbox = Sandbox::new();
    sandbox.set_env("PADDOCK_COPY_COMMAND", None);
    let mut p = ready(&sandbox);

    copy_word(&mut p, "mac");

    sandbox.wait_for_clipboard("mac");
    assert_eq!(sandbox.desktop_calls(), ["pbcopy"]);
}

#[test]
fn the_copy_command_overrides_the_platform_tool() {
    let sandbox = Sandbox::new();
    sandbox.set_env("WAYLAND_DISPLAY", Some("wayland-test"));
    let mut p = ready(&sandbox);

    copy_word(&mut p, "override");

    // The harness's `PADDOCK_COPY_COMMAND`, which logs no call.
    sandbox.wait_for_clipboard("override");
    assert_eq!(sandbox.desktop_calls(), Vec::<String>::new());
}

#[test]
fn a_missing_notifier_is_said_once_and_paddock_goes_on() {
    let sandbox = Sandbox::new();
    let missing = sandbox.path("bin/missing");
    sandbox.set_env("PADDOCK_NOTIFY_COMMAND", Some(&missing.to_string_lossy()));
    let mut paddock = ready(&sandbox);
    look_away(&mut paddock);

    hooks(&sandbox, FIRST, &["UserPromptSubmit", "PermissionRequest"]);
    paddock.wait_for_text(CANNOT_NOTIFY);
    // Any key clears the notice; this one goes to the shell.
    still_runs(&mut paddock, 1);
    assert!(!paddock.full_screen().contains(CANNOT_NOTIFY));

    // The finished turn notifies, and fails, as it is marked done: a
    // notice would be drawn in that frame, the tab bar before the sidebar.
    hooks(&sandbox, FIRST, &["UserPromptSubmit", "Stop"]);
    paddock.wait_for("done", |_| paddock.full_screen().contains("done"));
    assert!(!paddock.full_screen().contains(CANNOT_NOTIFY));
    still_runs(&mut paddock, 2);
    assert!(sandbox.notifications().is_empty());
}

#[test]
fn a_missing_copy_tool_is_said_once_and_paddock_goes_on() {
    let sandbox = Sandbox::new();
    let missing = sandbox.path("bin/missing");
    sandbox.set_env("PADDOCK_COPY_COMMAND", Some(&missing.to_string_lossy()));
    let mut p = ready(&sandbox);

    copy_without_a_tool(&sandbox, &mut p, CANNOT_COPY);
}

#[test]
fn a_copy_tool_that_fails_says_why_on_each_copy() {
    let sandbox = Sandbox::new();
    // Short: the notice must fit the tab bar with the sandbox's path.
    let failing = sandbox.path("bin/c");
    std::fs::write(
        &failing,
        "#!/bin/sh\ncat >/dev/null\necho 'Error: no display' >&2\nexit 1\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
    sandbox.set_env("PADDOCK_COPY_COMMAND", Some(&failing.to_string_lossy()));
    let mut p = ready(&sandbox);
    let notice = "bin/c failed: Error: no display";

    copy_word(&mut p, "first");
    p.wait_for_text(notice);
    still_runs(&mut p, 1);
    assert!(!p.full_screen().contains(notice));

    copy_word(&mut p, "second");
    p.wait_for_text(notice);
    still_runs(&mut p, 2);
    assert_eq!(sandbox.clipboard(), None);
}
