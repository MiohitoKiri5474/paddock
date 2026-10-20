//! Scripted checks. Each feeds a real pane's `%output` into both emulator
//! crates and compares the result with what tmux itself holds.

use crate::emu::{self, capture_to_stream, Emulator};
use crate::keys::{encode_key, encode_mouse_sgr, encode_paste};
use crate::pane::{fetch_seed, wait_reply, Pane};
use crate::tmux::{self, Control, Event};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEventKind};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub const EMUS: [&str; 3] = ["alacritty", "yas", "vt100"];

pub fn report(ok: bool, name: &str, detail: &str) {
    println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
}

pub struct Harness {
    pub ctl: Control,
    rx: Receiver<Event>,
    pub panes: Vec<Pane>,
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

fn id(e: Event) -> Option<Event> {
    Some(e)
}

impl Harness {
    pub fn attach(session: &str) -> Result<Self> {
        let info = tmux::tmux(&["display", "-p", "-t", session, "#{pane_id} #{pane_width} #{pane_height}"])?;
        let f: Vec<&str> = info.split_whitespace().collect();
        let (pid, cols, rows) = (f[0].to_string(), f[1].parse()?, f[2].parse()?);
        let (tx, rx) = mpsc::channel();
        let mut ctl = Control::attach(session, tx, |e| e)?;
        ctl.send(&format!("refresh-client -C {cols}x{rows}"))?;
        wait_reply(&rx, id, &pid)?;
        let seed = fetch_seed(&mut ctl, &rx, id, &pid)?;
        let mut panes = Vec::new();
        for name in EMUS {
            let mut p = Pane::new(&pid, emu::make(name, cols, rows, 50_000));
            p.apply_seed(&seed)?;
            panes.push(p);
        }
        Ok(Self { ctl, rx, panes, id: pid, cols, rows })
    }

    /// Feed pane output for `d`. Returns bytes received.
    pub fn pump(&mut self, d: Duration) -> usize {
        let end = Instant::now() + d;
        let mut n = 0;
        while let Ok(e) = self.rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            n += self.handle(e);
        }
        n
    }

    fn handle(&mut self, e: Event) -> usize {
        match e {
            Event::Output { pane, data } if pane == self.id => {
                for p in &mut self.panes {
                    p.feed(&data);
                }
                data.len()
            }
            _ => 0,
        }
    }

    /// Pump until `pred` holds for the first emulator's screen text. Returns elapsed.
    pub fn pump_until(&mut self, timeout: Duration, pred: impl Fn(&str) -> bool) -> Option<Duration> {
        let t0 = Instant::now();
        loop {
            if pred(&self.screen_text(0)) {
                return Some(t0.elapsed());
            }
            let left = timeout.checked_sub(t0.elapsed())?;
            match self.rx.recv_timeout(left) {
                Ok(e) => {
                    self.handle(e);
                }
                Err(_) => return pred(&self.screen_text(0)).then(|| t0.elapsed()),
            }
        }
    }

    /// Wait until no output for `quiet`.
    pub fn settle(&mut self, quiet: Duration, max: Duration) {
        let t0 = Instant::now();
        while t0.elapsed() < max {
            if self.pump(quiet) == 0 {
                return;
            }
        }
    }

    pub fn screen_text(&self, i: usize) -> String {
        let e = &self.panes[i].emu;
        (0..e.size().1).map(|r| e.row_text(r)).collect::<Vec<_>>().join("\n")
    }

    pub fn send(&mut self, b: &[u8]) -> Result<()> {
        self.ctl.send_bytes(&self.id.clone(), b)
    }

    pub fn key(&mut self, code: KeyCode, m: KeyModifiers) -> Result<Vec<u8>> {
        let ev = KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE };
        let b = encode_key(&ev, self.panes[0].key_modes()).context("unencodable key")?;
        self.send(&b)?;
        Ok(b)
    }

    pub fn type_text(&mut self, s: &str) -> Result<()> {
        self.send(s.as_bytes())
    }

    pub fn detach(self) {
        self.ctl.detach();
    }

    /// Compare each emulator's visible grid with tmux's own copy of the pane.
    /// Text is compared against `capture-pane -p` (tmux's widths); cell colours
    /// against `capture-pane -p -e` re-parsed by the same crate.
    pub fn compare(&self, label: &str) -> Result<bool> {
        let plain = tmux::tmux(&["capture-pane", "-p", "-t", &self.id])?;
        let tlines: Vec<String> = plain.lines().map(|l| l.trim_end().to_string()).collect();
        let esc = tmux::tmux_bytes(&["capture-pane", "-p", "-e", "-t", &self.id])?;
        let mut all_ok = true;
        for (i, p) in self.panes.iter().enumerate() {
            let e = &p.emu;
            let mut text_bad = Vec::new();
            for r in 0..self.rows {
                let ours = e.row_text(r);
                let theirs = tlines.get(r as usize).cloned().unwrap_or_default();
                if ours != theirs {
                    text_bad.push(format!("row {r}: ours={ours:?} tmux={theirs:?}"));
                }
            }
            let mut reference = emu::make(EMUS[i], self.cols, self.rows, 0);
            reference.feed(&capture_to_stream(&esc));
            let mut color_bad = 0;
            let mut first = String::new();
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let (a, b) = (e.cell(r, c), reference.cell(r, c));
                    // Colours on blank cells past the end of a line are not in the capture.
                    if b.text.trim().is_empty() && a.text.trim().is_empty() {
                        continue;
                    }
                    if (a.fg, a.bg, a.bold, a.inverse) != (b.fg, b.bg, b.bold, b.inverse) {
                        color_bad += 1;
                        if first.is_empty() {
                            first = format!(" first at {r},{c} {:?}: ours {:?}/{:?} tmux {:?}/{:?}", a.text, a.fg, a.bg, b.fg, b.bg);
                        }
                    }
                }
            }
            let ok = text_bad.is_empty() && color_bad == 0;
            all_ok &= ok;
            let mut detail = format!("{} text-rows-differ={} attr-cells-differ={}{}", e.name(), text_bad.len(), color_bad, first);
            if let Some(f) = text_bad.first() {
                detail.push_str(&format!(" | {f}"));
            }
            report(ok, label, &detail);
        }
        Ok(all_ok)
    }
}

fn fresh(session: &str, cols: u16, rows: u16, cmd: &str) -> Result<Harness> {
    let _ = tmux::tmux(&["kill-session", "-t", session]);
    let cwd = std::env::current_dir()?;
    tmux::new_session(session, &cwd, cols, rows, Some(cmd))?;
    std::thread::sleep(Duration::from_millis(300));
    Harness::attach(session)
}

/// A pane whose program echoes our input bytes back as output: lets a test
/// write any byte sequence "from the program".
const ECHO_CMD: &str = "stty raw -echo; exec cat -u";

pub fn render() -> Result<()> {
    let mut h = fresh("render", 100, 30, ECHO_CMD)?;
    // Truecolor gradient, 256 colours, 16 colours, attributes.
    let mut s = String::from("\x1b[H\x1b[2J");
    for i in 0..96u32 {
        let v = (i * 255 / 95) as u8;
        s.push_str(&format!("\x1b[48;2;{v};{};{}m ", 255 - v, v / 2));
    }
    s.push_str("\x1b[0m\r\n");
    for i in 0..96u32 {
        s.push_str(&format!("\x1b[38;2;{};{};255mX", 255 - (i * 2) as u8, (i * 2) as u8));
    }
    s.push_str("\x1b[0m\r\n");
    for i in 0..96u32 {
        s.push_str(&format!("\x1b[38;5;{}m#", 16 + i));
    }
    s.push_str("\x1b[0m\r\n\x1b[1mbold\x1b[0m \x1b[7mrev\x1b[0m \x1b[31;42mred-on-green\x1b[0m \x1b[91mbright\x1b[0m\r\n");
    h.send(s.as_bytes())?;
    h.settle(Duration::from_millis(300), Duration::from_secs(5));
    let ok_tc = h.compare("truecolor/256/16 colours")?;
    // Spot-check a gradient cell carries the exact RGB.
    for p in &h.panes {
        let c = p.emu.cell(0, 95);
        report(c.bg == emu::Col::Rgb(255, 0, 127), "truecolor exact rgb", &format!("{} got {:?}", p.emu.name(), c.bg));
    }
    let _ = ok_tc;

    // Wide characters and emoji: compare cursor column with tmux after each sample,
    // then compare the full line text (which uses absolute positioning).
    let samples = [
        ("CJK", "漢字"),
        ("fullwidth", "ＡＢ"),
        ("emoji", "😀"),
        ("emoji+skin", "👍🏽"),
        ("VS16 heart", "❤️"),
        ("VS16 digit", "1️⃣"),
        ("ZWJ family", "👨‍👩‍👧"),
        ("flag", "🇹🇼"),
        ("combining", "é"),
        ("box/spinner", "✳⏺✻"),
    ];
    for (name, s) in samples {
        h.send(format!("\x1b[H\x1b[2J|{s}|\x1b[3;1Hx\x1b[1;20H<").as_bytes())?;
        h.send(format!("\x1b[1;1H|{s}").as_bytes())?;
        h.settle(Duration::from_millis(150), Duration::from_secs(3));
        let tx: u16 = tmux::tmux(&["display", "-p", "-t", &h.id, "#{cursor_x}"])?.trim().parse()?;
        let mut line = String::new();
        for p in &h.panes {
            let (_, cx) = p.emu.cursor();
            line.push_str(&format!("{}: col {} {} ", p.emu.name(), cx, if cx == tx { "ok" } else { "MISMATCH" }));
        }
        let ok = h.panes.iter().all(|p| p.emu.cursor().1 == tx);
        report(ok, &format!("width {name} {s:?}"), &format!("tmux col {tx}; {line}"));
    }
    h.send("\x1b[H\x1b[2J漢字 😀 👍🏽 ❤️ 🇹🇼 é ✳⏺ end\x1b[1;40H|abs\r\n".as_bytes())?;
    h.settle(Duration::from_millis(200), Duration::from_secs(3));
    h.compare("wide/emoji line with absolute positioning")?;

    // Bracketed paste arrives as one bracketed block.
    h.send(b"\x1b[H\x1b[2J\x1b[?2004h")?;
    h.settle(Duration::from_millis(150), Duration::from_secs(3));
    for p in &h.panes {
        report(p.emu.modes().bracketed_paste, "bracketed-paste mode tracked", p.emu.name());
    }
    h.detach();
    let _ = tmux::tmux(&["kill-session", "-t", "render"]);

    // A program that shows raw bytes it receives, to see the paste framing.
    let mut h = fresh("paste", 100, 10, "printf '\\033[?2004h'; stty raw -echo; exec cat -uv")?;
    h.settle(Duration::from_millis(200), Duration::from_secs(3));
    let bracketed = h.panes[0].emu.modes().bracketed_paste;
    let b = encode_paste("line one\nline two\nline three", bracketed);
    h.send(&b)?;
    h.settle(Duration::from_millis(300), Duration::from_secs(3));
    let txt = h.screen_text(0).replace('\n', "");
    let opens = txt.matches("^[[200~").count();
    let closes = txt.matches("^[[201~").count();
    let crs = txt.matches("^M").count();
    report(opens == 1 && closes == 1 && crs == 2, "paste framed once", &format!("bracketed={bracketed} opens={opens} closes={closes} CRs={crs} seen={txt:?}"));
    h.detach();
    let _ = tmux::tmux(&["kill-session", "-t", "paste"]);
    Ok(())
}

pub fn history() -> Result<()> {
    let n = 3000;
    let mut h = fresh("hist", 80, 24, "zsh -f")?;
    h.settle(Duration::from_millis(300), Duration::from_secs(5));
    h.type_text(&format!("for i in $(seq 1 {n}); do printf '\\033[3%dmline %d\\033[0m\\n' $((i%7+1)) $i; done\r"))?;
    h.pump_until(Duration::from_secs(20), |t| t.contains(&format!("line {n}")));
    h.settle(Duration::from_millis(300), Duration::from_secs(5));
    let tmux_all = tmux::tmux(&["capture-pane", "-p", "-S", "-", "-t", &h.id])?;
    let tmux_lines: Vec<String> = tmux_all.lines().map(|l| l.trim_end().to_string()).collect();
    for p in &mut h.panes {
        report(p.emu.history_len() + p.emu.size().1 as usize >= n, "live history (history + screen >= lines printed)", &format!("{} history_len={}", p.emu.name(), p.emu.history_len()));
    }
    h.compare("live screen")?;
    // Wheel on a shell (no mouse mode) scrolls our own scrollback.
    for p in &mut h.panes {
        let modes = p.emu.modes();
        let off = p.emu.scroll(3 * 10);
        let top = p.emu.row_text(0);
        let expect_idx = tmux_lines.len() as i64 - p.emu.size().1 as i64 - 30;
        let expect = tmux_lines.get(expect_idx.max(0) as usize).cloned().unwrap_or_default();
        report(
            modes.mouse == emu::MouseMode::Off && off == 30 && top == expect,
            "wheel scrolls own scrollback (10 notches)",
            &format!("{} offset={off} top={top:?} expect={expect:?}", p.emu.name()),
        );
        p.emu.scroll(-1_000_000);
    }
    h.detach();

    // Reattach: a new control client rebuilds history from capture-pane -S -.
    let mut h2 = Harness::attach("hist")?;
    h2.settle(Duration::from_millis(200), Duration::from_secs(3));
    for p in &mut h2.panes {
        let ours = p.emu.all_text();
        let (o, t) = (trim_tail(&ours), trim_tail(&tmux_lines));
        let same = o == t;
        report(same, "history after reattach", &format!("{} ours={} lines tmux={} lines all-equal={same}", p.emu.name(), o.len(), t.len()));
    }
    h2.compare("screen after reattach")?;
    for p in &h2.panes {
        let c = p.emu.cell(0, 0);
        report(matches!(c.fg, emu::Col::Idx(_)), "colours after reattach", &format!("{} row0 fg={:?} text={:?}", p.emu.name(), c.fg, p.emu.row_text(0)));
    }
    h2.detach();

    // Reattach while output is streaming: nothing may be lost or doubled.
    tmux::tmux(&["send-keys", "-t", "hist", "clear; seq 1 3000000; echo STREAM-DONE", "Enter"])?;
    std::thread::sleep(Duration::from_millis(700));
    let mid = !tmux::tmux(&["capture-pane", "-p", "-t", "hist"])?.contains("STREAM-DONE");
    let mut h3 = Harness::attach("hist")?;
    h3.pump_until(Duration::from_secs(60), |t| t.lines().any(|l| l == "STREAM-DONE"));
    h3.settle(Duration::from_millis(300), Duration::from_secs(5));
    let tmux_all = tmux::tmux(&["capture-pane", "-p", "-S", "-", "-t", "hist"])?;
    let t: Vec<String> = tmux_all.lines().map(|l| l.trim_end().to_string()).collect();
    for p in &mut h3.panes {
        let ours = p.emu.all_text();
        let (o, t) = (trim_tail(&ours), trim_tail(&t));
        let n = o.len().min(t.len()).min(40_000);
        let same = o[o.len() - n..] == t[t.len() - n..];
        let first_bad = (0..n).find(|&i| o[o.len() - n + i] != t[t.len() - n + i]).map(|i| (o[o.len() - n + i].clone(), t[t.len() - n + i].clone()));
        report(mid && same, "history after reattach mid-stream", &format!("{} attached-mid-stream={mid} last {n} lines equal={same} first-diff={first_bad:?}", p.emu.name()));
    }
    h3.compare("screen after reattach mid-stream")?;
    h3.detach();
    let _ = tmux::tmux(&["kill-session", "-t", "hist"]);
    Ok(())
}

fn trim_tail(v: &[String]) -> &[String] {
    let mut end = v.len();
    while end > 0 && v[end - 1].is_empty() {
        end -= 1;
    }
    &v[..end]
}

fn prompt_lines(screen: &str) -> Vec<String> {
    // Lines between the two horizontal rules around Claude's input box.
    let lines: Vec<&str> = screen.lines().collect();
    let rules: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.starts_with("────")).map(|(i, _)| i).collect();
    if rules.len() < 2 {
        return vec![];
    }
    let (a, b) = (rules[rules.len() - 2], rules[rules.len() - 1]);
    lines[a + 1..b].iter().map(|s| s.trim_end().to_string()).collect()
}

fn wait_claude_ready(h: &mut Harness) -> Result<()> {
    let ok = h.pump_until(Duration::from_secs(40), |t| prompt_lines(t).iter().any(|l| l.starts_with('❯')));
    anyhow::ensure!(ok.is_some(), "claude prompt did not appear:\n{}", h.screen_text(0));
    h.settle(Duration::from_millis(800), Duration::from_secs(5));
    Ok(())
}

fn clear_prompt(h: &mut Harness) -> Result<()> {
    // Ctrl+U deletes to start of line in Claude's input; repeat for multi-line input.
    for _ in 0..4 {
        h.key(KeyCode::Char('u'), KeyModifiers::CONTROL)?;
        h.key(KeyCode::Backspace, KeyModifiers::NONE)?;
    }
    h.settle(Duration::from_millis(300), Duration::from_secs(2));
    Ok(())
}

/// Try Shift+Enter encodings. Returns (name, produced newline) per variant.
fn shift_enter_variants(h: &mut Harness, tag: &str) -> Result<()> {
    let km = h.panes[0].key_modes();
    println!("INFO {tag}: key modes kitty={} mok={} (alacritty native kitty={:?}) pane_key_mode={}",
        km.kitty_flags, km.modify_other_keys, h.panes[0].emu.modes().kitty_flags_native,
        tmux::tmux(&["display", "-p", "-t", &h.id, "#{pane_key_mode}"])?.trim());
    let variants: [(&str, &[u8]); 5] = [
        ("encoder (current modes)", b""),
        ("kitty CSI 13;2u", b"\x1b[13;2u"),
        ("modifyOtherKeys CSI 27;2;13~", b"\x1b[27;2;13~"),
        ("ESC CR (meta-enter)", b"\x1b\r"),
        ("negative control: plain '-' (must NOT make a newline)", b"-"),
    ];
    for (name, bytes) in variants {
        clear_prompt(h)?;
        h.type_text("alpha")?;
        h.settle(Duration::from_millis(200), Duration::from_secs(2));
        let sent = if bytes.is_empty() { h.key(KeyCode::Enter, KeyModifiers::SHIFT)? } else { h.send(bytes).map(|_| bytes.to_vec())? };
        h.type_text("beta")?;
        h.settle(Duration::from_millis(400), Duration::from_secs(3));
        let pl = prompt_lines(&h.screen_text(0));
        let a = pl.iter().position(|l| l.contains("alpha"));
        let b = pl.iter().position(|l| l.contains("beta"));
        let newline = matches!((a, b), (Some(x), Some(y)) if y == x + 1);
        let expect = bytes != b"-";
        report(newline == expect, &format!("{tag} Shift+Enter {name}"), &format!("sent {:?} prompt={:?}", String::from_utf8_lossy(&sent), pl));
    }
    clear_prompt(h)?;
    Ok(())
}

fn esc_latency(h: &mut Harness, tag: &str, esc: Option<&[u8]>) -> Result<()> {
    clear_prompt(h)?;
    h.type_text("Print the integers from 1 to 3000, one per line, as plain text. Do not use any tools.")?;
    h.settle(Duration::from_millis(300), Duration::from_secs(2));
    h.key(KeyCode::Enter, KeyModifiers::NONE)?;
    // Busy: Claude is streaming the numbers.
    let busy = h.pump_until(Duration::from_secs(40), |t| t.lines().any(|l| l.trim() == "30"));
    if busy.is_none() {
        report(false, &format!("{tag} Esc interrupts"), "claude never showed a busy indicator");
        return Ok(());
    }
    h.pump(Duration::from_millis(500));
    let t0 = Instant::now();
    let sent = match esc {
        Some(b) => {
            h.send(b)?;
            b.to_vec()
        }
        None => h.key(KeyCode::Esc, KeyModifiers::NONE)?,
    };
    let done = h.pump_until(Duration::from_secs(10), |t| t.contains("Interrupted"));
    let ms = done.map(|_| t0.elapsed().as_millis());
    report(
        matches!(ms, Some(m) if m < 300),
        &format!("{tag} Esc interrupts"),
        &format!("sent {:?}; 'Interrupted' after {:?} ms (via control-mode output)", String::from_utf8_lossy(&sent), ms),
    );
    h.settle(Duration::from_millis(800), Duration::from_secs(5));
    Ok(())
}

pub fn claude(run_esc: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let _ = tmux::tmux(&["kill-session", "-t", "claude"]);
    // Plan mode: Claude cannot edit files even if a prompt asked it to.
    tmux::new_session("claude", &cwd, 110, 40, Some("claude --permission-mode plan"))?;
    std::thread::sleep(Duration::from_millis(300));
    let mut h = Harness::attach("claude")?;
    wait_claude_ready(&mut h)?;
    h.compare("claude UI first frame")?;
    for p in &h.panes {
        let m = p.emu.modes();
        println!("INFO {} modes: alt={} mouse={:?} sgr={} paste={} kitty(scan)={} mok={}", p.emu.name(), m.alt_screen, m.mouse, m.sgr_mouse, m.bracketed_paste, p.scanner.kitty_flags(), p.scanner.modify_other_keys);
    }

    shift_enter_variants(&mut h, "attached")?;

    // Bracketed paste of multi-line text: one paste, nothing submitted.
    clear_prompt(&mut h)?;
    let bracketed = h.panes[0].emu.modes().bracketed_paste;
    let text: String = (1..=12).map(|i| format!("pasted line {i}\n")).collect();
    h.send(&encode_paste(&text, bracketed))?;
    h.settle(Duration::from_millis(600), Duration::from_secs(4));
    let screen = h.screen_text(0);
    let pl = prompt_lines(&screen);
    let one = pl.iter().any(|l| l.contains("Pasted text")) || pl.iter().filter(|l| l.contains("pasted line")).count() == 12;
    let submitted = screen.contains("pasted line 1\n") && !pl.iter().any(|l| l.contains("pasted"));
    report(one && !submitted, "bracketed multi-line paste", &format!("bracketed={bracketed} prompt={pl:?} submitted={submitted}"));
    clear_prompt(&mut h)?;

    // A large paste (about 5 KB, many send-keys chunks) is still one paste.
    let big: String = (1..=100).map(|i| format!("big paste line {i:03} with some padding text here\n")).collect();
    h.send(&encode_paste(&big, bracketed))?;
    h.settle(Duration::from_millis(800), Duration::from_secs(6));
    let pl = prompt_lines(&h.screen_text(0));
    report(pl.len() == 1 && pl[0].contains("Pasted text") && pl[0].contains("+100 lines"), "5 KB bracketed paste",
        &format!("{} bytes, prompt={pl:?}", big.len()));
    clear_prompt(&mut h)?;

    // Emoji that Claude itself lays out (it positions with CHA, so widths matter).
    for (label, text) in [("VS16 + CJK", "⚠️ ✅ ✔️ ❤️ 漢字 x|"), ("skin tone + ZWJ", "👍🏽 👨‍👩‍👧 x|")] {
        h.type_text(text)?;
        h.settle(Duration::from_millis(600), Duration::from_secs(3));
        h.compare(&format!("claude prompt with {label} typed"))?;
        clear_prompt(&mut h)?;
        h.compare(&format!("claude prompt cleared after {label} (stale cells?)"))?;
    }

    if run_esc {
        esc_latency(&mut h, "attached kitty", None)?;
        // Mouse wheel inside Claude (it enabled 1003/1006): forward as SGR wheel.
        let before = h.screen_text(0);
        let mid = (h.cols / 2, h.rows / 3);
        for _ in 0..5 {
            let b = encode_mouse_sgr(MouseEventKind::ScrollUp, mid.0, mid.1, KeyModifiers::NONE).unwrap();
            h.send(&b)?;
        }
        h.settle(Duration::from_millis(400), Duration::from_secs(3));
        let after = h.screen_text(0);
        report(before != after, "wheel up forwarded to Claude (SGR 64)", &format!("screen changed={}", before != after));
        for _ in 0..5 {
            let b = encode_mouse_sgr(MouseEventKind::ScrollDown, mid.0, mid.1, KeyModifiers::NONE).unwrap();
            h.send(&b)?;
        }
        h.settle(Duration::from_millis(400), Duration::from_secs(3));
        h.compare("claude UI after wheel")?;
    }

    // Detach and reattach while Claude keeps running.
    h.detach();
    std::thread::sleep(Duration::from_millis(500));
    let mut h = Harness::attach("claude")?;
    h.settle(Duration::from_millis(300), Duration::from_secs(3));
    h.compare("claude UI after reattach")?;
    for p in &h.panes {
        let m = p.emu.modes();
        println!("INFO reattached {} modes: alt={} mouse={:?} sgr={} paste={} kitty(scan)={} mok={}", p.emu.name(), m.alt_screen, m.mouse, m.sgr_mouse, m.bracketed_paste, p.scanner.kitty_flags(), p.scanner.modify_other_keys);
    }
    shift_enter_variants(&mut h, "reattached")?;
    if run_esc {
        esc_latency(&mut h, "reattached bare-ESC", Some(b"\x1b"))?;
        esc_latency(&mut h, "reattached kitty CSI 27u", Some(b"\x1b[27u"))?;
    }
    // Force a redraw via resize and compare again.
    h.ctl.send(&format!("refresh-client -C {}x{}", h.cols, h.rows - 1))?;
    for p in &mut h.panes {
        p.emu.resize(h.cols, h.rows - 1);
    }
    h.rows -= 1;
    h.settle(Duration::from_millis(500), Duration::from_secs(4));
    h.compare("claude UI after resize redraw")?;
    h.detach();
    let _ = tmux::tmux(&["kill-session", "-t", "claude"]);
    Ok(())
}

/// Throughput of each crate on a large recorded stream (no tmux).
pub fn bench() -> Result<()> {
    let mut data = Vec::new();
    for i in 0..200_000u32 {
        data.extend(format!("\x1b[38;2;{};{};200mline {i} 漢字 😀 some text here\x1b[0m\r\n", i % 256, (i * 7) % 256).into_bytes());
    }
    for name in EMUS {
        let mut e: Box<dyn Emulator> = emu::make(name, 120, 40, 10_000);
        let t0 = Instant::now();
        for chunk in data.chunks(4096) {
            e.feed(chunk);
        }
        let s = t0.elapsed().as_secs_f64();
        println!("INFO bench {name}: {:.1} MB in {:.3}s = {:.1} MB/s", data.len() as f64 / 1e6, s, data.len() as f64 / 1e6 / s);
    }
    Ok(())
}
