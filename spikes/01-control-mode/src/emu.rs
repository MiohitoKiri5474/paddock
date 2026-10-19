//! One trait over the candidate emulator crates: alacritty_terminal (and its
//! yas fork with VS16 width fixes) and vt100.

use std::num::NonZeroU16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    /// First half of a double-width character.
    pub wide: bool,
    /// Second half of a double-width character (draw nothing).
    pub spacer: bool,
    pub fg: Col,
    pub bg: Col,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    Press,
    Drag,
    Any,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    pub sgr_mouse: bool,
    pub alt_screen: bool,
    pub show_cursor: bool,
    /// Kitty keyboard flags as tracked by the emulator itself (None = crate does not track).
    pub kitty_flags_native: Option<u8>,
}

pub trait Emulator {
    fn name(&self) -> &'static str;
    fn feed(&mut self, bytes: &[u8]);
    fn resize(&mut self, cols: u16, rows: u16);
    fn size(&self) -> (u16, u16);
    /// Cell at a viewport position (respects the scroll offset).
    fn cell(&self, row: u16, col: u16) -> Cell;
    /// Cursor position on the live screen (row, col).
    fn cursor(&self) -> (u16, u16);
    fn modes(&self) -> Modes;
    /// Positive = scroll up into history. Returns the new offset.
    fn scroll(&mut self, delta: i32) -> usize;
    fn scroll_offset(&self) -> usize;
    fn history_len(&self) -> usize;
    /// Every line, oldest history first, trailing spaces trimmed.
    fn all_text(&mut self) -> Vec<String>;

    fn row_text(&self, row: u16) -> String {
        let (cols, _) = self.size();
        let s: String = (0..cols)
            .map(|c| self.cell(row, c))
            .filter(|c| !c.spacer)
            .map(|c| c.text)
            .collect();
        s.trim_end().to_string()
    }
}

pub fn make(name: &str, cols: u16, rows: u16, history: usize) -> Box<dyn Emulator> {
    match name {
        "vt100" => Box::new(Vt100Emu::new(cols, rows, history)),
        "yas" => Box::new(yas::AlacrittyEmu::new(cols, rows, history)),
        _ => Box::new(upstream::AlacrittyEmu::new(cols, rows, history)),
    }
}

// ---------------------------------------------------------------- alacritty

pub mod upstream {
    use alacritty_terminal as krate;
    const NAME: &str = "alacritty_terminal";
    include!("alac.rs");
}

pub mod yas {
    use yas_alacritty as krate;
    const NAME: &str = "yas-alacritty-terminal";
    include!("alac.rs");
}

// ---------------------------------------------------------------- vt100

pub struct Vt100Emu {
    p: vt100::Parser,
}

impl Vt100Emu {
    pub fn new(cols: u16, rows: u16, history: usize) -> Self {
        let nz = |v: u16| NonZeroU16::new(v.max(1)).unwrap();
        Self { p: vt100::Parser::new(nz(rows), nz(cols), history) }
    }
    fn conv(c: vt100::Color) -> Col {
        match c {
            vt100::Color::Default => Col::Default,
            vt100::Color::Idx(i) => Col::Idx(i),
            vt100::Color::Rgb(r, g, b) => Col::Rgb(r, g, b),
        }
    }
}

impl Emulator for Vt100Emu {
    fn name(&self) -> &'static str {
        "vt100"
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.p.process(bytes);
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        let nz = |v: u16| NonZeroU16::new(v.max(1)).unwrap();
        self.p.set_size(nz(rows), nz(cols));
    }
    fn size(&self) -> (u16, u16) {
        let (r, c) = self.p.screen().size();
        (c.get(), r.get())
    }
    fn cell(&self, row: u16, col: u16) -> Cell {
        let Some(c) = self.p.screen().cell(row, col) else {
            return blank();
        };
        Cell {
            text: if c.is_wide_continuation() { String::new() } else if c.has_contents() { c.contents().to_string() } else { " ".into() },
            wide: c.is_wide(),
            spacer: c.is_wide_continuation(),
            fg: Self::conv(c.fgcolor()),
            bg: Self::conv(c.bgcolor()),
            bold: c.bold(),
            italic: c.italic(),
            underline: c.underline(),
            inverse: c.inverse(),
        }
    }
    fn cursor(&self) -> (u16, u16) {
        self.p.screen().cursor_position()
    }
    fn modes(&self) -> Modes {
        let s = self.p.screen();
        let mouse = match s.mouse_protocol_mode() {
            vt100::MouseProtocolMode::None => MouseMode::Off,
            vt100::MouseProtocolMode::Press | vt100::MouseProtocolMode::PressRelease => MouseMode::Press,
            vt100::MouseProtocolMode::ButtonMotion => MouseMode::Drag,
            vt100::MouseProtocolMode::AnyMotion => MouseMode::Any,
        };
        Modes {
            app_cursor: s.application_cursor(),
            bracketed_paste: s.bracketed_paste(),
            mouse,
            sgr_mouse: s.mouse_protocol_encoding() == vt100::MouseProtocolEncoding::Sgr,
            alt_screen: s.alternate_screen(),
            show_cursor: !s.hide_cursor(),
            kitty_flags_native: None,
        }
    }
    fn scroll(&mut self, delta: i32) -> usize {
        let cur = self.p.screen().scrollback() as i32;
        self.p.screen_mut().set_scrollback((cur + delta).max(0) as usize);
        self.p.screen().scrollback()
    }
    fn scroll_offset(&self) -> usize {
        self.p.screen().scrollback()
    }
    fn history_len(&self) -> usize {
        // vt100 has no direct accessor; probe by scrolling to the top.
        let mut probe = self.p.screen().clone();
        probe.set_scrollback(usize::MAX);
        probe.scrollback()
    }
    fn all_text(&mut self) -> Vec<String> {
        let saved = self.p.screen().scrollback();
        let h = self.history_len();
        let (cols, rows) = self.size();
        let mut out = Vec::new();
        // Walk the history one screen at a time from the top.
        let mut off = h;
        loop {
            self.p.screen_mut().set_scrollback(off);
            let rows_txt: Vec<String> =
                self.p.screen().rows(0, cols).map(|r| r.trim_end().to_string()).collect();
            // At offset `off`, row 0 shows history line (h - off).
            let start = h - off;
            for (i, r) in rows_txt.into_iter().enumerate() {
                if start + i == out.len() {
                    out.push(r);
                }
            }
            if off == 0 {
                break;
            }
            off = off.saturating_sub(rows as usize);
        }
        self.p.screen_mut().set_scrollback(saved);
        out
    }
}

fn blank() -> Cell {
    Cell {
        text: " ".into(),
        wide: false,
        spacer: false,
        fg: Col::Default,
        bg: Col::Default,
        bold: false,
        italic: false,
        underline: false,
        inverse: false,
    }
}

// ---------------------------------------------------------------- key modes

/// Tracks keyboard-protocol requests in the raw pane output, independent of
/// the emulator crate: kitty flag stack (`CSI > n u`, `CSI < n u`, `CSI = n ; m u`)
/// and xterm modifyOtherKeys (`CSI > 4 ; n m`). alacritty_terminal ignores the
/// latter and vt100 tracks neither.
#[derive(Default, Clone, Debug)]
pub struct KeyModeScanner {
    pub kitty_stack: Vec<u8>,
    pub modify_other_keys: u8,
    st: ScanState,
    buf: Vec<u8>,
}

#[derive(Default, Clone, Debug, PartialEq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Csi,
}

impl KeyModeScanner {
    pub fn kitty_flags(&self) -> u8 {
        self.kitty_stack.last().copied().unwrap_or(0)
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match self.st {
                ScanState::Ground => {
                    if b == 0x1b {
                        self.st = ScanState::Esc;
                    }
                }
                ScanState::Esc => {
                    self.st = if b == b'[' { ScanState::Csi } else { ScanState::Ground };
                    self.buf.clear();
                }
                ScanState::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        let params = std::mem::take(&mut self.buf);
                        self.dispatch(&params, b);
                        self.st = ScanState::Ground;
                    } else if self.buf.len() < 32 {
                        self.buf.push(b);
                    } else {
                        self.st = ScanState::Ground;
                    }
                }
            }
        }
    }

    fn dispatch(&mut self, params: &[u8], fin: u8) {
        let (lead, rest) = match params.first() {
            Some(&c @ (b'>' | b'<' | b'=' | b'?')) => (c, &params[1..]),
            _ => (0, params),
        };
        let nums: Vec<u32> = std::str::from_utf8(rest)
            .unwrap_or("")
            .split(';')
            .map(|s| s.parse().unwrap_or(0))
            .collect();
        let n0 = nums.first().copied().unwrap_or(0);
        match (lead, fin) {
            (b'>', b'u') => {
                if self.kitty_stack.len() < 64 {
                    self.kitty_stack.push(n0 as u8);
                }
            }
            (b'<', b'u') => {
                for _ in 0..n0.max(1) {
                    self.kitty_stack.pop();
                }
            }
            (b'=', b'u') => {
                let mode = nums.get(1).copied().unwrap_or(1);
                let cur = self.kitty_flags();
                let new = match mode {
                    2 => cur | n0 as u8,
                    3 => cur & !(n0 as u8),
                    _ => n0 as u8,
                };
                match self.kitty_stack.last_mut() {
                    Some(top) => *top = new,
                    None => self.kitty_stack.push(new),
                }
            }
            (b'>', b'm') if n0 == 4 => {
                self.modify_other_keys = nums.get(1).copied().unwrap_or(0) as u8;
            }
            (b'?', b'h' | b'l') if nums.contains(&1049) => {
                // Kitty: main and alternate screens keep separate stacks; for the
                // spike we keep one stack (Claude re-pushes after switching).
            }
            _ => {}
        }
    }
}

/// Convert a `capture-pane -p -e` dump into bytes that rebuild the same grid.
pub fn capture_to_stream(capture: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(capture.len() + 64);
    let lines: Vec<&[u8]> = capture.split(|&b| b == b'\n').collect();
    let n = if capture.ends_with(b"\n") { lines.len() - 1 } else { lines.len() };
    for (i, l) in lines[..n].iter().enumerate() {
        out.extend_from_slice(l);
        out.extend_from_slice(b"\x1b[0m");
        if i + 1 < n {
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both(cols: u16, rows: u16) -> Vec<Box<dyn Emulator>> {
        vec![make("alacritty", cols, rows, 1000), make("vt100", cols, rows, 1000)]
    }

    #[test]
    fn truecolor_and_wide() {
        for mut e in both(20, 3) {
            e.feed("\x1b[38;2;10;20;30mA\x1b[0m漢".as_bytes());
            assert_eq!(e.cell(0, 0).fg, Col::Rgb(10, 20, 30), "{}", e.name());
            assert!(e.cell(0, 1).wide, "{}", e.name());
            assert!(e.cell(0, 2).spacer, "{}", e.name());
            assert_eq!(e.row_text(0), "A漢");
        }
    }

    #[test]
    fn scrollback_and_all_text() {
        for mut e in both(10, 3) {
            for i in 0..10 {
                e.feed(format!("l{i}\r\n").as_bytes());
            }
            assert_eq!(e.history_len(), 8, "{}", e.name());
            let all = e.all_text();
            assert_eq!(all[0], "l0", "{}", e.name());
            assert_eq!(all[9], "l9", "{}", e.name());
            assert_eq!(e.scroll(3), 3);
            assert_eq!(e.row_text(0), "l5", "{}", e.name());
            assert_eq!(e.scroll(-100), 0);
            assert_eq!(e.row_text(0), "l8", "{}", e.name());
        }
    }

    #[test]
    fn scanner_tracks_kitty_and_mok() {
        let mut s = KeyModeScanner::default();
        s.feed(b"\x1b[>5u\x1b[>4;2m");
        assert_eq!(s.kitty_flags(), 5);
        assert_eq!(s.modify_other_keys, 2);
        s.feed(b"\x1b[<u\x1b[>4m");
        assert_eq!(s.kitty_flags(), 0);
        assert_eq!(s.modify_other_keys, 0);
        s.feed(b"\x1b[=3;1u");
        assert_eq!(s.kitty_flags(), 3);
        // split across chunks
        s.feed(b"\x1b[");
        s.feed(b">1u");
        assert_eq!(s.kitty_flags(), 1);
    }

    #[test]
    fn alacritty_tracks_kitty_natively() {
        let mut e = make("alacritty", 10, 3, 10);
        e.feed(b"\x1b[>5u");
        assert_eq!(e.modes().kitty_flags_native, Some(5));
    }
}
