//! Each terminal's scrollback, saved beside `layout.json` (ADR 0008) in
//! `scrollback/`, one file per terminal named by its scrollback name
//! ([`ScrollbackName`]), which `layout.json` keeps with the terminal. The
//! server writes them as it ends ([`save_all`]): the text of each
//! terminal's normal screen, its scrollback first, with its colours, up
//! to [`MAX`] bytes. A server that rebuilds the layout draws each
//! terminal's saved text above its new shell, under a dim line saying
//! when it was saved ([`load`]).
//!
//! Unlike `layout.json`, which is written with the state let go, these
//! are taken and written with it locked: the text is the emulators',
//! read under the lock, and the server is ending, its shells hung up
//! under the same lock right after, so nothing is kept waiting.
//!
//! A restored terminal keeps its name, so its file stays until the server
//! writes it again; a file the layout no longer names (its terminal
//! closed, its space too) is removed whenever the layout is written
//! ([`prune`]). A file missing, unreadable or not as written restores
//! nothing worse than less text: what is drawn is only text and colours
//! (see [`clean`]).

use super::saved::write_synced;
use super::state::State;
use crate::backend::TerminalId;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The directory, in the state directory.
pub const DIR: &str = "scrollback";
/// The most of a terminal's text that is saved, or read back: its newest.
pub const MAX: usize = 1 << 20;
/// The longest scrollback name: an instance and a terminal number.
const NAME_MAX: usize = 32;

/// The name a terminal's scrollback is saved under, a file in [`DIR`]:
/// only letters, digits and `-`, so it names nothing outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollbackName(String);

impl ScrollbackName {
    /// The name of terminal `id` started by server `instance`: no other
    /// server's terminal has it (but by the instance's chance).
    pub fn new(instance: u32, id: &TerminalId) -> Self {
        let n = id.as_str().trim_start_matches('%');
        Self(format!("{instance:08x}-{n}"))
    }

    /// `name` as a scrollback name; `None` if it is not one.
    pub fn parse(name: &str) -> Option<Self> {
        let ok = !name.is_empty()
            && name.len() <= NAME_MAX
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        ok.then(|| Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn dir(state_dir: &Path) -> PathBuf {
    state_dir.join(DIR)
}

/// Write each tab terminal's text to its file (floats are not saved, as
/// in the layout), logging what cannot be: the server ends.
pub fn save_all(state_dir: &Path, state: &mut State) {
    for space in &mut state.spaces {
        let ids: Vec<TerminalId> = space.tabs.iter().flat_map(|t| t.terminals()).collect();
        for id in ids {
            let Some(term) = space.hosted_mut(&id) else {
                continue;
            };
            let text = term.history_text();
            if let Err(e) = save(state_dir, &term.saved_as, &text) {
                eprintln!("paddock server: {e:#}");
            }
        }
    }
    // The renames on disk too: a reboot is one reason to restore.
    let dir = dir(state_dir);
    match std::fs::File::open(&dir).and_then(|d| d.sync_all()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            eprintln!("paddock server: cannot sync {}: {e}", dir.display());
        }
        _ => {}
    }
}

/// Write `text` as `name`'s file, its newest [`MAX`] bytes, replacing
/// the old one (or a directory in its place) in one step (mode 0600,
/// renamed over it); no text removes it.
fn save(state_dir: &Path, name: &ScrollbackName, text: &[u8]) -> Result<()> {
    let dir = dir(state_dir);
    let path = dir.join(name.as_str());
    if path.is_dir() {
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("cannot remove {}", path.display()))?;
    }
    if text.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot remove {}", path.display()))
            }
            _ => Ok(()),
        };
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    let tmp = dir.join(format!("{}.{}.tmp", name.as_str(), std::process::id()));
    let written = write_synced(&tmp, tail(text))
        .with_context(|| format!("cannot write {}", tmp.display()))
        .and_then(|()| {
            std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// What to draw above the new shell of the terminal saved as `name`: its
/// saved text, made safe, and the line saying when it was saved. `None`
/// if there is none, or it cannot be read (logged).
pub fn load(state_dir: &Path, name: &ScrollbackName) -> Option<Vec<u8>> {
    let path = dir(state_dir).join(name.as_str());
    match read(&path) {
        Ok(Some((text, saved))) => {
            let mut shown = clean(&text);
            if shown.is_empty() {
                return None;
            }
            shown.push_str(&mark(saved));
            Some(shown.into_bytes())
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("paddock server: {e:#}");
            None
        }
    }
}

/// The file at `path`, its newest [`MAX`] bytes (as [`tail`] cuts them),
/// and when it was written; `None` if there is none.
fn read(path: &Path) -> Result<Option<(Vec<u8>, SystemTime)>> {
    let cannot = || format!("cannot read {}", path.display());
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(cannot),
    };
    let meta = file.metadata().with_context(cannot)?;
    if !meta.is_file() {
        anyhow::bail!("{} is not a file", path.display());
    }
    // One byte more than kept, to tell whether the cut is at a line's start.
    let max = u64::try_from(MAX + 1).unwrap_or(u64::MAX);
    let from = meta.len().saturating_sub(max);
    file.seek(SeekFrom::Start(from)).with_context(cannot)?;
    let mut text = Vec::new();
    file.take(max).read_to_end(&mut text).with_context(cannot)?;
    let text = tail(&text).to_vec();
    let saved = meta.modified().unwrap_or_else(|_| SystemTime::now());
    Ok(Some((text, saved)))
}

/// `text`'s newest [`MAX`] bytes, from the first line that starts in
/// them; with none (one long line), from the first whole character.
fn tail(text: &[u8]) -> &[u8] {
    if text.len() <= MAX {
        return text;
    }
    let cut = text.len() - MAX;
    if text[cut - 1] == b'\n' {
        return &text[cut..];
    }
    let rest = &text[cut..];
    if let Some(i) = rest.iter().position(|&b| b == b'\n') {
        return &rest[i + 1..];
    }
    // UTF-8 continuation bytes are 0b10xx_xxxx.
    let start = rest.iter().position(|&b| b & 0xc0 != 0x80);
    &rest[start.unwrap_or(rest.len())..]
}

/// `text` as only text and colours, so the emulator it is fed to
/// neither acts on nor answers anything in it. Kept: printable
/// characters, tabs, new lines, and SGR sequences (`ESC [` or C1 CSI,
/// then `<digits ; :> m`). Dropped whole: other CSI sequences, other
/// escape sequences (`ESC 7`, `ESC ( B`), and control strings (OSC, DCS,
/// SOS, PM, APC, by `ESC` or C1) to their end. Other controls are
/// dropped, and bytes not UTF-8 replaced. A control inside a sequence
/// ends it there, as in a terminal, and is taken on its own (an `ESC`
/// then a new line keeps the new line). Ends at a line's start, the pen
/// reset.
fn clean(text: &[u8]) -> String {
    let text = String::from_utf8_lossy(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // Whether anything but blanks and colours is left.
    let mut shown = false;
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => escape(&mut chars, &mut out),
            '\u{9b}' => sgr_only(&mut chars, &mut out),
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            '\r' | '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => {
                shown |= !c.is_whitespace();
                out.push(c);
            }
        }
    }
    if !shown {
        return String::new();
    }
    out.push_str("\x1b[0m");
    if !out.ends_with("\r\n\x1b[0m") {
        out.push_str("\r\n");
    }
    out
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Read the sequence after an `ESC`, pushing it on `out` only if it is
/// SGR.
fn escape(chars: &mut Chars<'_>, out: &mut String) {
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            sgr_only(chars, out);
        }
        Some(']' | 'P' | 'X' | '^' | '_') => {
            chars.next();
            skip_string(chars);
        }
        // Intermediates, then a final character.
        Some('\x20'..='\x2f') => {
            while chars.next_if(|c| ('\x20'..='\x2f').contains(c)).is_some() {}
            chars.next_if(|c| ('\x30'..='\x7e').contains(c));
        }
        Some('\x30'..='\x7e') => {
            chars.next();
        }
        // A control (taken on its own), anything else, or the end.
        _ => {}
    }
}

/// Read a CSI sequence after its introducer, pushing it on `out` only if
/// it is SGR.
fn sgr_only(chars: &mut Chars<'_>, out: &mut String) {
    let seq = csi(chars);
    let params = seq.strip_suffix('m');
    if params.is_some_and(|p| p.chars().all(is_sgr_param)) {
        out.push_str("\x1b[");
        out.push_str(&seq);
    }
}

fn is_sgr_param(c: char) -> bool {
    c.is_ascii_digit() || c == ';' || c == ':'
}

/// A CSI sequence's parameters, intermediates and final character, read
/// after its introducer; it stops early before any other character.
fn csi(chars: &mut Chars<'_>) -> String {
    let mut seq = String::new();
    while let Some(c) = chars.next_if(|c| ('\x20'..='\x7e').contains(c)) {
        seq.push(c);
        if ('\x40'..='\x7e').contains(&c) {
            break;
        }
    }
    seq
}

/// Read past a control string, to its BEL, `ESC \` or ST (or the text's
/// end).
fn skip_string(chars: &mut Chars<'_>) {
    while let Some(c) = chars.next() {
        match c {
            '\x07' | '\u{9c}' => return,
            '\x1b' => {
                chars.next_if_eq(&'\\');
                return;
            }
            _ => {}
        }
    }
}

/// The dim line under the saved text: when it was saved, local time.
fn mark(saved: SystemTime) -> String {
    let secs = saved
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "\x1b[2m--- scrollback saved {} ---\x1b[0m\r\n",
        local_time(secs)
    )
}

/// `secs` since the epoch as local `YYYY-MM-DD HH:MM`.
fn local_time(secs: u64) -> String {
    let t = libc::time_t::try_from(secs).unwrap_or(0);
    // SAFETY: an all-zero tm is valid (its zone pointer null).
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r reads only `t` and writes only `tm`.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return "at an unknown time".into();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

/// Remove every file in [`DIR`] but those `keep` names (a file left
/// half written too), and any directory, so each name kept can be saved
/// again; all of it if `keep` is empty.
pub fn prune(state_dir: &Path, keep: &HashSet<&str>) -> Result<()> {
    let dir = dir(state_dir);
    if keep.is_empty() {
        return match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot remove {}", dir.display()))
            }
            _ => Ok(()),
        };
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if !is_dir && keep.contains(entry.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        let removed = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = removed {
            eprintln!("paddock server: cannot remove {}: {e}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> ScrollbackName {
        ScrollbackName::parse(s).unwrap()
    }

    #[test]
    fn saved_text_reads_back_with_a_mark_and_a_missing_file_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), &name("1-2")), None, "none yet");
        save(dir.path(), &name("1-2"), b"one\r\n\x1b[0;31mred\x1b[0m\r\n").unwrap();
        let text = String::from_utf8(load(dir.path(), &name("1-2")).unwrap()).unwrap();
        assert!(
            text.starts_with("one\r\n\x1b[0;31mred\x1b[0m\r\n"),
            "{text:?}"
        );
        assert!(text.contains("\x1b[2m--- scrollback saved "), "{text:?}");
        let names: Vec<_> = std::fs::read_dir(dir.path().join(DIR)).unwrap().collect();
        assert_eq!(names.len(), 1, "no file left beside it");
        save(dir.path(), &name("1-2"), b"").unwrap();
        assert_eq!(load(dir.path(), &name("1-2")), None, "no text removes it");
    }

    #[test]
    fn a_directory_in_a_files_place_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(DIR).join("1-2").join("x")).unwrap();
        assert_eq!(load(dir.path(), &name("1-2")), None, "not a file");
        save(dir.path(), &name("1-2"), b"text").unwrap();
        assert!(load(dir.path(), &name("1-2")).is_some());
    }

    #[test]
    fn only_text_and_colours_are_kept() {
        let text =
            b"\xff a\x1b[c\x1b[?6n\x1b]0;t\x07\x1b[1;38:2:1:2:3mb\x07\x00\xc2\x9bc\x1bPq\x1b\\d\x1b7";
        assert_eq!(
            clean(text),
            "\u{fffd} a\x1b[1;38:2:1:2:3mbd\x1b[0m\r\n",
            "no query, OSC or C1 left"
        );
        assert_eq!(clean(b" \r\n\x1b[31m"), "", "nothing to show");
    }

    #[test]
    fn escape_sequences_and_c1_strings_go_whole_and_a_control_after_esc_stays() {
        let text = "a\x1b(Bb\x1b#8c\u{9b}32md\u{9b}6ne\u{9d}0;title\u{9c}f\u{90}q\x1b\\g\x1b\nh";
        assert_eq!(clean(text.as_bytes()), "abc\x1b[32mdefg\nh\x1b[0m\r\n");
    }

    #[test]
    fn only_the_newest_text_is_kept_from_a_lines_start() {
        let line = "x".repeat(99) + "\n";
        let text = line.repeat(MAX / 100 + 5);
        let kept = tail(text.as_bytes());
        assert!(kept.len() <= MAX && kept.len() > MAX - 100);
        assert!(kept.starts_with(line.as_bytes()));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DIR);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("1-2"), &text).unwrap();
        let (read, _) = read(&path.join("1-2")).unwrap().unwrap();
        assert_eq!(read, kept, "a long file is read from its end");
    }

    #[test]
    fn a_cut_at_a_lines_start_keeps_that_line() {
        let text = "x".repeat(10) + "\n" + &"y".repeat(MAX - 1) + "\n";
        assert_eq!(tail(text.as_bytes()), &text.as_bytes()[11..]);
    }

    #[test]
    fn one_long_line_is_cut_at_a_character() {
        // Three-byte characters, so the cut falls inside one.
        let text = "漢".repeat(MAX / 3 + 10);
        let kept = tail(text.as_bytes());
        assert!(!kept.is_empty() && kept.len() <= MAX);
        assert!(std::str::from_utf8(kept).is_ok(), "whole characters");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DIR);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("1-2"), &text).unwrap();
        let (read, _) = read(&path.join("1-2")).unwrap().unwrap();
        assert_eq!(read, kept);
    }

    #[test]
    fn only_letters_digits_and_dashes_make_a_name() {
        for bad in ["../secret", "", "a/b", ".", &"1".repeat(40)] {
            assert_eq!(ScrollbackName::parse(bad), None, "{bad}");
        }
        let made = ScrollbackName::new(0xab, &TerminalId::new("%12"));
        assert_eq!(made.as_str(), "000000ab-12");
        let longest = ScrollbackName::new(u32::MAX, &TerminalId::new("%12"));
        assert_eq!(ScrollbackName::parse(longest.as_str()), Some(longest));
    }

    #[test]
    fn pruning_keeps_only_the_files_named_and_none_removes_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["1-1", "1-2", "1-3"] {
            save(dir.path(), &name(n), b"text").unwrap();
        }
        std::fs::write(dir.path().join(DIR).join("1-4.9.tmp"), "half").unwrap();
        std::fs::create_dir(dir.path().join(DIR).join("1-5")).unwrap();
        prune(dir.path(), &HashSet::from(["1-2", "1-5"])).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path().join(DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["1-2"], "a directory goes, named or not");
        prune(dir.path(), &HashSet::new()).unwrap();
        assert!(!dir.path().join(DIR).exists());
        prune(dir.path(), &HashSet::new()).unwrap();
    }
}
