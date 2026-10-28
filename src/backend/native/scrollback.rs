//! Each terminal's scrollback, saved beside `layout.json` (ADR 0008) in
//! `scrollback/`, one file per terminal named by its key, which
//! `layout.json` keeps with the terminal. The server writes them as it
//! ends ([`save_all`]): the text of each terminal's normal screen, its
//! scrollback first, with its colours, up to [`MAX`] bytes. A server that
//! rebuilds the layout draws each terminal's saved text above its new
//! shell, under a dim line saying when it was saved ([`load`]).
//!
//! A restored terminal keeps its key, so its file stays until the server
//! writes it again; a file the layout no longer names (its terminal
//! closed, its space too) is removed whenever the layout is written
//! ([`prune`]). A file missing, unreadable or not as written restores
//! nothing worse than less text: what is drawn is only text and colours,
//! never a sequence the emulator would act on or answer.

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
/// The longest key: an instance and a terminal number.
const KEY_MAX: usize = 32;

/// The key of terminal `id` started by server `instance`: no other
/// server's terminal has it (but by the instance's chance).
pub fn key(instance: u32, id: &TerminalId) -> String {
    let n = id.as_str().trim_start_matches('%');
    format!("{instance:08x}-{n}")
}

/// Whether `name` is a key [`key`] makes, so names a file in [`DIR`]
/// and nothing outside it.
pub fn is_key(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= KEY_MAX
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
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
            if let Err(e) = save(state_dir, &term.scrollback, &text) {
                eprintln!("paddock server: {e:#}");
            }
        }
    }
}

/// Write `text` as `key`'s file, its newest [`MAX`] bytes, replacing the
/// old one in one step (mode 0600, renamed over it); no text removes it.
fn save(state_dir: &Path, key: &str, text: &[u8]) -> Result<()> {
    let dir = dir(state_dir);
    let path = dir.join(key);
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
    let tmp = dir.join(format!("{key}.{}.tmp", std::process::id()));
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

/// What to draw above `key`'s new shell: its saved text, made safe, and
/// the line saying when it was saved. `None` if there is none, or it
/// cannot be read (logged).
pub fn load(state_dir: &Path, key: &str) -> Option<Vec<u8>> {
    if !is_key(key) {
        eprintln!("paddock server: no scrollback named {key:?}");
        return None;
    }
    let path = dir(state_dir).join(key);
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

/// The file at `path`, its newest [`MAX`] bytes, and when it was
/// written; `None` if there is none.
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
    let max = u64::try_from(MAX).unwrap_or(u64::MAX);
    let from = meta.len().saturating_sub(max);
    file.seek(SeekFrom::Start(from)).with_context(cannot)?;
    let mut text = Vec::new();
    file.take(max).read_to_end(&mut text).with_context(cannot)?;
    if from > 0 {
        text = tail_lines(&text).to_vec();
    }
    let saved = meta.modified().unwrap_or_else(|_| SystemTime::now());
    Ok(Some((text, saved)))
}

/// `text`'s newest [`MAX`] bytes, from a line's start.
fn tail(text: &[u8]) -> &[u8] {
    if text.len() <= MAX {
        return text;
    }
    tail_lines(&text[text.len() - MAX..])
}

/// `text` from just after its first new line: a cut line is dropped.
fn tail_lines(text: &[u8]) -> &[u8] {
    text.iter()
        .position(|&b| b == b'\n')
        .map_or(&[][..], |i| &text[i + 1..])
}

/// `text` as only text and colours: printable characters, tabs, new
/// lines and SGR sequences (`ESC [ <digits ; :> m`). Every other control
/// or sequence (C1 controls, strings such as OSC, whole) is dropped and
/// bytes not UTF-8 replaced, so the emulator it is fed to neither acts
/// on nor answers it. Ends at a line's start, the pen reset.
fn clean(text: &[u8]) -> String {
    let text = String::from_utf8_lossy(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // Whether anything but blanks and colours is left.
    let mut shown = false;
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let seq = csi(&mut chars);
                    let params = seq.strip_suffix('m');
                    if params.is_some_and(|p| p.chars().all(is_sgr_param)) {
                        out.push_str("\x1b[");
                        out.push_str(&seq);
                    }
                }
                // OSC, DCS, SOS, PM and APC: a string, to its end.
                Some(']' | 'P' | 'X' | '^' | '_') => skip_string(&mut chars),
                _ => {}
            },
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

fn is_sgr_param(c: char) -> bool {
    c.is_ascii_digit() || c == ';' || c == ':'
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// A CSI sequence's parameters, intermediates and final character, read
/// after its `ESC [`; it stops early before any other character.
fn csi(chars: &mut Chars<'_>) -> String {
    let mut seq = String::new();
    while let Some(&c) = chars.peek() {
        if !('\x20'..='\x7e').contains(&c) {
            break;
        }
        chars.next();
        seq.push(c);
        if ('\x40'..='\x7e').contains(&c) {
            break;
        }
    }
    seq
}

/// Read past a control string, to its BEL or `ESC \` (or the text's end).
fn skip_string(chars: &mut Chars<'_>) {
    while let Some(c) = chars.next() {
        match c {
            '\x07' => return,
            '\x1b' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
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
/// half written too); all of it if `keep` is empty.
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
        let name = entry.file_name();
        if keep.contains(name.to_string_lossy().as_ref()) {
            continue;
        }
        let path = entry.path();
        let removed = if path.is_dir() {
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

    #[test]
    fn saved_text_reads_back_with_a_mark_and_a_missing_file_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "1-2"), None, "none yet");
        save(dir.path(), "1-2", b"one\r\n\x1b[0;31mred\x1b[0m\r\n").unwrap();
        let text = String::from_utf8(load(dir.path(), "1-2").unwrap()).unwrap();
        assert!(
            text.starts_with("one\r\n\x1b[0;31mred\x1b[0m\r\n"),
            "{text:?}"
        );
        assert!(text.contains("\x1b[2m--- scrollback saved "), "{text:?}");
        let names: Vec<_> = std::fs::read_dir(dir.path().join(DIR)).unwrap().collect();
        assert_eq!(names.len(), 1, "no file left beside it");
        save(dir.path(), "1-2", b"").unwrap();
        assert_eq!(load(dir.path(), "1-2"), None, "no text removes it");
    }

    #[test]
    fn only_text_and_colours_are_kept() {
        let text =
            b"\xff a\x1b[c\x1b[?6n\x1b]0;t\x07\x1b[1;38:2:1:2:3mb\x07\x00\xc2\x9bc\x1bPq\x1b\\d\x1b7";
        assert_eq!(
            clean(text),
            "\u{fffd} a\x1b[1;38:2:1:2:3mbcd\x1b[0m\r\n",
            "no query, OSC or C1 left"
        );
        assert_eq!(clean(b" \r\n\x1b[31m"), "", "nothing to show");
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
    fn a_name_that_is_no_key_reads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("secret"), "x").unwrap();
        for name in ["../secret", "", "a/b", &"1".repeat(40)] {
            assert!(!is_key(name), "{name}");
            assert_eq!(load(&dir.path().join(DIR), name), None);
        }
        assert_eq!(key(0xab, &TerminalId::new("%12")), "000000ab-12");
        assert!(is_key(&key(u32::MAX, &TerminalId::new("%12"))));
    }

    #[test]
    fn pruning_keeps_only_the_files_named_and_none_removes_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        for key in ["1-1", "1-2", "1-3"] {
            save(dir.path(), key, b"text").unwrap();
        }
        std::fs::write(dir.path().join(DIR).join("1-4.9.tmp"), "half").unwrap();
        prune(dir.path(), &HashSet::from(["1-2"])).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path().join(DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["1-2"]);
        prune(dir.path(), &HashSet::new()).unwrap();
        assert!(!dir.path().join(DIR).exists());
        prune(dir.path(), &HashSet::new()).unwrap();
    }
}
