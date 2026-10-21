// Included twice by emu.rs: once for upstream alacritty_terminal, once for
// the yas fork. `krate` and `NAME` are defined by the including module.
use super::{Cell, Col, Emulator, Modes, MouseMode};
use krate::event::VoidListener;
use krate::grid::{Dimensions, Scroll};
use krate::index::{Column, Line};
use krate::term::cell::Flags;
use krate::term::{Config, Term, TermMode};
use krate::vte::ansi::{Color as AColor, NamedColor, Processor};

struct Size(usize, usize);
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

pub struct AlacrittyEmu {
    term: Term<VoidListener>,
    parser: Processor,
}

impl AlacrittyEmu {
    pub fn new(cols: u16, rows: u16, history: usize) -> Self {
        let config = Config { scrolling_history: history, kitty_keyboard: true, ..Config::default() };
        let term = Term::new(config, &Size(cols as usize, rows as usize), VoidListener);
        Self { term, parser: Processor::new() }
    }

    fn conv(c: AColor) -> Col {
        match c {
            AColor::Spec(rgb) => Col::Rgb(rgb.r, rgb.g, rgb.b),
            AColor::Indexed(i) => Col::Idx(i),
            AColor::Named(n) => {
                let i = n as usize;
                if i < 16 {
                    Col::Idx(i as u8)
                } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) {
                    Col::Idx((i - NamedColor::DimBlack as usize) as u8)
                } else {
                    Col::Default
                }
            }
        }
    }

    fn line_cell(&self, line: Line, col: u16) -> Cell {
        let c = &self.term.grid()[line][Column(col as usize)];
        let f = c.flags;
        let mut text = String::new();
        let spacer = f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
        if !spacer {
            text.push(c.c);
            if let Some(zw) = c.zerowidth() {
                text.extend(zw.iter());
            }
        }
        Cell {
            text,
            wide: f.contains(Flags::WIDE_CHAR),
            spacer: f.contains(Flags::WIDE_CHAR_SPACER),
            fg: Self::conv(c.fg),
            bg: Self::conv(c.bg),
            bold: f.contains(Flags::BOLD),
            italic: f.contains(Flags::ITALIC),
            underline: f.intersects(Flags::ALL_UNDERLINES),
            inverse: f.contains(Flags::INVERSE),
        }
    }
}

impl Emulator for AlacrittyEmu {
    fn name(&self) -> &'static str {
        NAME
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size(cols as usize, rows as usize));
    }
    fn size(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }
    fn cell(&self, row: u16, col: u16) -> Cell {
        let off = self.term.grid().display_offset() as i32;
        self.line_cell(Line(row as i32 - off), col)
    }
    fn cursor(&self) -> (u16, u16) {
        let p = self.term.grid().cursor.point;
        (p.line.0.max(0) as u16, p.column.0 as u16)
    }
    fn modes(&self) -> Modes {
        let m = *self.term.mode();
        let mouse = if m.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Any
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Press
        } else {
            MouseMode::Off
        };
        let k = (m & TermMode::KITTY_KEYBOARD_PROTOCOL).bits() >> 18;
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            mouse,
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            show_cursor: m.contains(TermMode::SHOW_CURSOR),
            kitty_flags_native: Some(k as u8),
        }
    }
    fn scroll(&mut self, delta: i32) -> usize {
        self.term.scroll_display(Scroll::Delta(delta));
        self.scroll_offset()
    }
    fn scroll_offset(&self) -> usize {
        self.term.grid().display_offset()
    }
    fn history_len(&self) -> usize {
        self.term.grid().history_size()
    }
    fn all_text(&mut self) -> Vec<String> {
        let h = self.history_len() as i32;
        let (cols, rows) = self.size();
        (-h..rows as i32)
            .map(|l| {
                let s: String = (0..cols)
                    .map(|c| self.line_cell(Line(l), c))
                    .filter(|c| !c.spacer)
                    .map(|c| c.text)
                    .collect();
                s.trim_end().to_string()
            })
            .collect()
    }
}

