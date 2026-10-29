//! The desktop tools paddock runs: the one that shows a notification and
//! the one that copies to the clipboard. This is their platform seam:
//! macOS runs `osascript` and `pbcopy`; Linux runs `notify-send`, and
//! `wl-copy` on Wayland (falling back to `xclip` under XWayland) or
//! `xclip` on X11.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("paddock's desktop tools are macOS's and Linux's only");

use std::ffi::OsString;
use std::io::{Read, Seek, Write};
use std::process::{Command, Stdio};

/// Names the program run in place of the platform's notifier, with the
/// same arguments (a test hook: tests use it to make the notifier missing).
const NOTIFY_VAR: &str = "PADDOCK_NOTIFY_COMMAND";
/// Names the program that copies its stdin, in place of the platform's
/// (tests use it to keep off the real clipboard).
const COPY_VAR: &str = "PADDOCK_COPY_COMMAND";

/// The value of `var`, unless it is unset or empty.
fn program_from(var: &str) -> Option<OsString> {
    std::env::var_os(var).filter(|v| !v.is_empty())
}

/// The command that shows a notification titled `paddock` saying `message`.
pub fn notifier(message: &str) -> Command {
    let program = program_from(NOTIFY_VAR).unwrap_or_else(|| NOTIFIER.into());
    let mut command = Command::new(program);
    command.args(notify_args(message));
    command
}

/// Why copying failed, as a notice.
#[derive(Debug, PartialEq, Eq)]
pub enum CopyFailure {
    /// No copy tool could be run: missing, most likely.
    NotRun(String),
    /// One ran and failed.
    Failed(String),
}

/// Put `text` on the clipboard with the program `PADDOCK_COPY_COMMAND`
/// names, else the platform's copy tools (see [`copy_with`]).
pub fn copy(text: &str) -> Result<(), CopyFailure> {
    let commands = match program_from(COPY_VAR) {
        Some(program) => vec![Command::new(program)],
        None => copiers()
            .iter()
            .map(|(program, args)| {
                let mut command = Command::new(program);
                command.args(*args);
                command
            })
            .collect(),
    };
    copy_with(commands, text)
}

/// Put `text` on the clipboard with the first of `commands` that works.
/// If one ran, the first that ran and failed says why; else none could be
/// run, most likely missing.
fn copy_with(commands: Vec<Command>, text: &str) -> Result<(), CopyFailure> {
    let mut not_run = Vec::new();
    let mut failed = None;
    for command in commands {
        let name = command.get_program().to_string_lossy().into_owned();
        match run_copier(command, text) {
            Ok(()) => return Ok(()),
            Err(Run::NotRun(e)) => not_run.push((name, e)),
            Err(Run::Failed(why)) => {
                failed.get_or_insert_with(|| format!("cannot copy: {name} {why}"));
            }
        }
    }
    if let Some(notice) = failed {
        return Err(CopyFailure::Failed(notice));
    }
    let names: Vec<&str> = not_run.iter().map(|(n, _)| n.as_str()).collect();
    let error = not_run
        .last()
        .map(|(_, e)| e.to_string())
        .unwrap_or_default();
    Err(CopyFailure::NotRun(format!(
        "cannot copy: cannot run {}: {error}",
        names.join(" or ")
    )))
}

/// How much of a failed copy tool's stderr is read: its first line is all
/// the notice shows.
const STDERR_CAP: u64 = 4096;

/// How one copy tool's run went wrong.
enum Run {
    NotRun(std::io::Error),
    /// What to say after its name.
    Failed(String),
}

/// Run one copy tool on `text`. Its stdout is not read, and its stderr
/// goes to a file, not a pipe: `xclip` and `wl-copy` leave a child behind
/// that serves the clipboard and would hold a pipe open.
fn run_copier(mut command: Command, text: &str) -> Result<(), Run> {
    let failed = |what: &str, e: std::io::Error| Run::Failed(format!("{what}: {e}"));
    let mut stderr = tempfile::tempfile().map_err(|e| failed("has no stderr file", e))?;
    let for_child = stderr
        .try_clone()
        .map_err(|e| failed("has no stderr file", e))?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(for_child)
        .spawn()
        .map_err(Run::NotRun)?;
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(text.as_bytes()),
        None => Err(std::io::Error::other("no stdin")),
    };
    let status = child.wait().map_err(|e| failed("failed", e))?;
    if status.success() {
        return written.map_err(|e| failed("failed: writing to it", e));
    }
    let mut said = Vec::new();
    // Best effort: without its words the status still says it failed.
    let _ = stderr
        .rewind()
        .and_then(|()| (&mut stderr).take(STDERR_CAP).read_to_end(&mut said));
    let said = String::from_utf8_lossy(&said);
    let first = said.lines().map(str::trim).find(|l| !l.is_empty());
    Err(Run::Failed(match first {
        Some(line) => format!("failed: {line}"),
        None => format!("failed ({status})"),
    }))
}

#[cfg(target_os = "macos")]
const NOTIFIER: &str = "osascript";

/// The AppleScript that shows a notification; its message is the script's
/// first argument, so it needs no AppleScript quoting.
#[cfg(target_os = "macos")]
const NOTIFY_SCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 1 of argv) with title \"paddock\"",
    "end run",
];

#[cfg(target_os = "macos")]
fn notify_args(message: &str) -> Vec<String> {
    NOTIFY_SCRIPT
        .iter()
        .flat_map(|line| ["-e".to_string(), (*line).to_string()])
        .chain([message.to_string()])
        .collect()
}

/// The copy tools to try in turn, with their arguments.
#[cfg(target_os = "macos")]
fn copiers() -> &'static [(&'static str, &'static [&'static str])] {
    &[("pbcopy", &[])]
}

#[cfg(target_os = "linux")]
const NOTIFIER: &str = "notify-send";

/// `--` ends the options, so a message starting with `-` is not one. The
/// body is escaped: servers that read it as Pango markup would garble
/// `R&D` or `a<b`.
#[cfg(target_os = "linux")]
fn notify_args(message: &str) -> Vec<String> {
    vec!["--".into(), "paddock".into(), escape_markup(message)]
}

/// `message` with `&`, `<` and `>` as markup entities.
#[cfg(target_os = "linux")]
fn escape_markup(message: &str) -> String {
    message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The copy tools to try in turn, with their arguments.
#[cfg(target_os = "linux")]
fn copiers() -> &'static [(&'static str, &'static [&'static str])] {
    linux_copiers(std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()))
}

/// `wl-copy` under a Wayland compositor, then `xclip` (XWayland); only
/// `xclip` on X11.
#[cfg(target_os = "linux")]
fn linux_copiers(wayland: bool) -> &'static [(&'static str, &'static [&'static str])] {
    const XCLIP: (&str, &[&str]) = ("xclip", &["-selection", "clipboard"]);
    if wayland {
        &[("wl-copy", &[]), XCLIP]
    } else {
        &[XCLIP]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_notification_script_compiles() {
        // `osacompile` checks the AppleScript without running it.
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("notify.scpt");
        let mut cmd = Command::new("osacompile");
        for line in NOTIFY_SCRIPT {
            cmd.args(["-e", line]);
        }
        let result = cmd.arg("-o").arg(&out).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_message_is_the_scripts_argument() {
        let args = notify_args("R&D is idle");
        assert_eq!(args.last().map(String::as_str), Some("R&D is idle"));
        assert_eq!(args.len(), 2 * NOTIFY_SCRIPT.len() + 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_send_is_given_a_title_and_the_message_after_its_options() {
        assert_eq!(notify_args("-x is idle"), ["--", "paddock", "-x is idle"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notify_sends_message_is_escaped_markup() {
        assert_eq!(
            notify_args("R&D <a> is idle")[2],
            "R&amp;D &lt;a&gt; is idle"
        );
        assert_eq!(escape_markup("&amp;"), "&amp;amp;");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_tries_wl_copy_then_xclip_and_x11_only_xclip() {
        let names = |wayland| {
            linux_copiers(wayland)
                .iter()
                .map(|(p, a)| [&[*p][..], a].concat().join(" "))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(true), ["wl-copy", "xclip -selection clipboard"]);
        assert_eq!(names(false), ["xclip -selection clipboard"]);
    }

    #[test]
    fn a_copier_that_fails_gives_its_first_stderr_line() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "cat >/dev/null; echo; echo 'Error: no display' >&2; exit 1",
        ]);
        match run_copier(command, "text") {
            Err(Run::Failed(why)) => assert_eq!(why, "failed: Error: no display"),
            _ => panic!("expected a failure"),
        }
    }

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn a_copier_that_fails_lets_the_next_one_copy() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let fails = sh("cat >/dev/null; echo 'wl: no compositor' >&2; exit 1");
        let mut works = sh("cat > \"$0\"");
        works.arg(&out);
        assert_eq!(copy_with(vec![fails, works], "hi"), Ok(()));
        assert_eq!(std::fs::read_to_string(out).unwrap(), "hi");
    }

    #[test]
    fn when_every_copier_fails_the_first_says_why() {
        let first = sh("cat >/dev/null; echo 'wl: no compositor' >&2; exit 1");
        let missing = Command::new("/nonexistent/paddock-copier");
        let second = sh("cat >/dev/null; echo 'x: no display' >&2; exit 1");
        let Err(CopyFailure::Failed(notice)) = copy_with(vec![first, missing, second], "hi") else {
            panic!("expected a failure");
        };
        assert!(notice.ends_with("failed: wl: no compositor"), "{notice}");
    }

    #[test]
    fn when_no_copier_runs_each_is_named() {
        let a = Command::new("/nonexistent/a");
        let b = Command::new("/nonexistent/b");
        let Err(CopyFailure::NotRun(notice)) = copy_with(vec![a, b], "hi") else {
            panic!("expected none to run");
        };
        assert!(
            notice.starts_with("cannot copy: cannot run /nonexistent/a or /nonexistent/b: "),
            "{notice}"
        );
    }

    #[test]
    fn a_copiers_stderr_is_read_up_to_its_cap() {
        // One long line, longer than the cap, then invalid UTF-8.
        let script = format!(
            "cat >/dev/null; printf '%0{}d\\377\\n' 0 >&2; exit 1",
            STDERR_CAP * 2
        );
        let Err(Run::Failed(why)) = run_copier(sh(&script), "") else {
            panic!("expected a failure");
        };
        assert_eq!(
            why.len(),
            "failed: ".len() + usize::try_from(STDERR_CAP).unwrap()
        );
    }

    #[test]
    fn a_copier_that_cannot_run_says_so() {
        let command = Command::new("/nonexistent/paddock-copier");
        assert!(matches!(run_copier(command, "text"), Err(Run::NotRun(_))));
    }

    #[test]
    fn a_copier_that_works_takes_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat > \"$0\"", &out.to_string_lossy()]);
        assert!(run_copier(command, "hello").is_ok());
        assert_eq!(std::fs::read_to_string(out).unwrap(), "hello");
    }
}
