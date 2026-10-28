//! An emulator's state as bytes: what the native server sends a client
//! that attaches (ADR 0007). Fed to a fresh emulator of the same size and
//! scrollback, they rebuild the scrollback, the screen (the saved normal
//! one too while a program uses the alternate one), the cursor, the pen,
//! the modes and the kitty keyboard flags of each screen.
//!
//! The scroll region is carried too, but alacritty keeps it private: the
//! caller tracks it from the output and passes it in. Without it, a
//! full-screen program reattached at the same size (no resize, so no
//! redraw) would scroll its region while the client scrolled the whole
//! screen. Not carried: the charsets (private too), the title and
//! hyperlinks.

use super::Emulator;
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
use std::fmt::Write as _;

/// Where kitty keyboard flags start in [`TermMode`]'s bits.
const KITTY_SHIFT: u32 = TermMode::DISAMBIGUATE_ESC_CODES.bits().trailing_zeros();
/// The five kitty keyboard flags.
const KITTY_MASK: u32 = 0x1f;

/// Private modes written as `CSI ? n h` when on.
const ON_MODES: &[(TermMode, u16)] = &[
    (TermMode::APP_CURSOR, 1),
    (TermMode::MOUSE_REPORT_CLICK, 1000),
    (TermMode::MOUSE_DRAG, 1002),
    (TermMode::MOUSE_MOTION, 1003),
    (TermMode::FOCUS_IN_OUT, 1004),
    (TermMode::UTF8_MOUSE, 1005),
    (TermMode::SGR_MOUSE, 1006),
    (TermMode::BRACKETED_PASTE, 2004),
];

/// Private modes on in a fresh emulator, written as `CSI ? n l` when off.
const OFF_MODES: &[(TermMode, u16)] = &[
    (TermMode::SHOW_CURSOR, 25),
    (TermMode::LINE_WRAP, 7),
    (TermMode::ALTERNATE_SCROLL, 1007),
];

impl Emulator {
    /// The bytes that rebuild this emulator in a fresh one of its size
    /// and scrollback, with `region` as its scroll region (its first and
    /// last rows, from 1; `None` for the whole screen). It takes
    /// `&mut self` only to read the normal screen while the alternate one
    /// shows, and leaves it as it was.
    pub fn snapshot(&mut self, region: Option<(u16, u16)>) -> Vec<u8> {
        let mut out = String::new();
        let mode = *self.term.mode();
        if mode.contains(TermMode::ALT_SCREEN) {
            // alacritty swaps the screens' grids and keyboard stacks; swap
            // to read the normal one, and put the alternate one back as it
            // was (swapping back from the normal screen clears it).
            let alt = self.term.grid().clone();
            self.term.swap_alt();
            write_screen(&mut out, self.term.grid(), true);
            write_kitty(&mut out, *self.term.mode());
            self.term.swap_alt();
            *self.term.grid_mut() = alt;
            out.push_str("\x1b[?1049h\x1b[H");
            write_screen(&mut out, self.term.grid(), false);
        } else {
            write_screen(&mut out, self.term.grid(), true);
        }
        write_kitty(&mut out, mode);
        write_modes(&mut out, mode);
        write_region(&mut out, self.term.grid(), mode, region);
        out.into_bytes()
    }
}

impl Emulator {
    /// The normal screen's lines as text, its scrollback first, with
    /// their colours and attributes, and no blank lines after the last
    /// written: what a restored terminal shows above its new shell (ADR
    /// 0008). Ends with the pen reset and a new line; empty if nothing
    /// was written. `&mut self` as for [`Self::snapshot`].
    pub fn history_text(&mut self) -> Vec<u8> {
        let out = if self.term.mode().contains(TermMode::ALT_SCREEN) {
            let alt = self.term.grid().clone();
            self.term.swap_alt();
            let out = lines_text(self.term.grid());
            self.term.swap_alt();
            *self.term.grid_mut() = alt;
            out
        } else {
            lines_text(self.term.grid())
        };
        out.into_bytes()
    }
}

/// `grid`'s scrollback and screen as [`Emulator::history_text`] writes
/// them.
fn lines_text(grid: &Grid<Cell>) -> String {
    let top = -i32::try_from(grid.history_size()).unwrap_or(i32::MAX);
    let bottom = i32::try_from(grid.screen_lines()).unwrap_or(i32::MAX);
    let mut out = String::new();
    // Where the last line that wrote anything ends.
    let mut kept = 0;
    let mut pen = Cell::default();
    for line in top..bottom {
        let start = out.len();
        let wrapped = write_row(&mut out, grid, Line(line), &mut pen);
        if out.len() > start {
            kept = out.len();
        }
        if !wrapped {
            if pen != Cell::default() {
                out.push_str("\x1b[0m");
                pen = Cell::default();
            }
            out.push_str("\r\n");
        }
    }
    out.truncate(kept);
    if !out.is_empty() {
        out.push_str("\x1b[0m\r\n");
    }
    out
}

/// The scroll region and origin mode, then the cursor again: DECSTBM and
/// DECOM move it home. Last, so the lines written before scroll the whole
/// screen.
fn write_region(out: &mut String, grid: &Grid<Cell>, mode: TermMode, region: Option<(u16, u16)>) {
    let origin = mode.contains(TermMode::ORIGIN);
    if region.is_none() && !origin {
        return;
    }
    if let Some((top, bottom)) = region {
        let _ = write!(out, "\x1b[{top};{bottom}r");
    }
    let mut line = grid.cursor.point.line.0 + 1;
    if origin {
        out.push_str("\x1b[?6h");
        // The cursor moves within the region now, from its top.
        line -= region.map_or(0, |(top, _)| i32::from(top) - 1);
    }
    let _ = write!(out, "\x1b[{line};{}H", grid.cursor.point.column.0 + 1);
}

/// `grid`'s lines (its scrollback first if `history`), then its cursor
/// and pen.
fn write_screen(out: &mut String, grid: &Grid<Cell>, history: bool) {
    let top = if history {
        -i32::try_from(grid.history_size()).unwrap_or(i32::MAX)
    } else {
        0
    };
    let bottom = i32::try_from(grid.screen_lines()).unwrap_or(i32::MAX);
    let mut pen = Cell::default();
    for line in top..bottom {
        let wrapped = write_row(out, grid, Line(line), &mut pen);
        if !wrapped && line + 1 < bottom {
            if pen != Cell::default() {
                // A new line takes the pen's background.
                out.push_str("\x1b[0m");
                pen = Cell::default();
            }
            out.push_str("\r\n");
        }
    }
    let cursor = &grid.cursor;
    let _ = write!(
        out,
        "\x1b[{};{}H",
        cursor.point.line.0 + 1,
        cursor.point.column.0 + 1
    );
    out.push_str(&sgr(&cursor.template));
}

/// One line's cells, with the pen changed where needed. Returns whether
/// the line wraps onto the next, which then continues it (no new line).
fn write_row(out: &mut String, grid: &Grid<Cell>, line: Line, pen: &mut Cell) -> bool {
    let cols = grid.columns();
    let row = &grid[line];
    let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    // A wrapped line is written to its end, so it wraps as it did.
    let end = if wrapped {
        cols
    } else {
        (0..cols)
            .rev()
            .find(|&c| !is_blank(&row[Column(c)]))
            .map_or(0, |c| c + 1)
    };
    for col in 0..end {
        let cell = &row[Column(col)];
        let spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        if cell.flags.intersects(spacer) {
            continue;
        }
        if !same_look(cell, pen) {
            out.push_str(&sgr(cell));
            *pen = look(cell);
        }
        out.push(cell.c);
        out.extend(cell.zerowidth().into_iter().flatten());
    }
    wrapped
}

/// A cell that a fresh line already holds.
fn is_blank(cell: &Cell) -> bool {
    cell.c == ' ' && cell.zerowidth().is_none() && same_look(cell, &Cell::default())
}

/// `cell`'s colours and attributes, without its character.
fn look(cell: &Cell) -> Cell {
    Cell {
        fg: cell.fg,
        bg: cell.bg,
        flags: cell.flags & attributes(),
        ..Cell::default()
    }
}

fn same_look(a: &Cell, b: &Cell) -> bool {
    a.fg == b.fg && a.bg == b.bg && a.flags & attributes() == b.flags & attributes()
}

/// The flags SGR sets.
fn attributes() -> Flags {
    Flags::BOLD
        | Flags::DIM
        | Flags::ITALIC
        | Flags::ALL_UNDERLINES
        | Flags::INVERSE
        | Flags::HIDDEN
        | Flags::STRIKEOUT
}

/// The SGR sequence that sets `cell`'s look from scratch.
fn sgr(cell: &Cell) -> String {
    let mut s = String::from("\x1b[0");
    let f = cell.flags;
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if f.contains(flag) {
            s.push(';');
            s.push_str(code);
        }
    }
    colour(&mut s, cell.fg, 30, 90, 38);
    colour(&mut s, cell.bg, 40, 100, 48);
    s.push('m');
    s
}

/// `c` as SGR parameters: `base` for the first eight colours, `bright`
/// for the next eight, `extended` for an index or RGB. Default colours
/// add nothing.
fn colour(s: &mut String, c: Color, base: u8, bright: u8, extended: u8) {
    let _ = match c {
        Color::Spec(rgb) => write!(s, ";{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => write!(s, ";{extended};5;{i}"),
        Color::Named(n) => {
            let i = n as usize;
            let dim = NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize;
            match i {
                0..=7 => write!(s, ";{}", usize::from(base) + i),
                8..=15 => write!(s, ";{}", usize::from(bright) + i - 8),
                _ if dim.contains(&i) => {
                    write!(s, ";{}", usize::from(base) + i - *dim.start())
                }
                _ => Ok(()),
            }
        }
    };
}

/// The kitty keyboard flags of the screen showing in `mode`, pushed.
fn write_kitty(out: &mut String, mode: TermMode) {
    let flags = (mode.bits() >> KITTY_SHIFT) & KITTY_MASK;
    if flags != 0 {
        let _ = write!(out, "\x1b[>{flags}u");
    }
}

fn write_modes(out: &mut String, mode: TermMode) {
    for &(flag, n) in ON_MODES {
        if mode.contains(flag) {
            let _ = write!(out, "\x1b[?{n}h");
        }
    }
    for &(flag, n) in OFF_MODES {
        if !mode.contains(flag) {
            let _ = write!(out, "\x1b[?{n}l");
        }
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        out.push_str("\x1b=");
    }
    if mode.contains(TermMode::INSERT) {
        out.push_str("\x1b[4h");
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out.push_str("\x1b[20h");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::key_modes::KeyModeScanner;

    const SCROLLBACK: usize = 1000;

    /// `e`'s snapshot fed to a fresh emulator of its size.
    fn rebuilt(e: &mut Emulator) -> Emulator {
        let (cols, rows) = e.size();
        let mut copy = Emulator::new(cols, rows, SCROLLBACK);
        copy.feed(&e.snapshot(None));
        copy
    }

    /// Every cell of the scrollback and the screen, with the cursor and
    /// the modes.
    fn picture(e: &Emulator) -> (Vec<String>, (u16, u16), super::super::Modes) {
        let (cols, rows) = e.size();
        let top = -i32::try_from(e.history_len()).unwrap();
        let lines = (top..i32::from(rows))
            .map(|line| {
                let cells = (0..cols).map(|c| format!("{:?}", e.cell_at(Line(line), c)));
                cells.collect::<Vec<_>>().join("")
            })
            .collect();
        (lines, e.cursor(), e.modes())
    }

    fn assert_same(a: &Emulator, b: &Emulator) {
        assert_eq!(a.history_len(), b.history_len(), "scrollback");
        assert_eq!(picture(a), picture(b));
    }

    #[test]
    fn rebuilds_scrollback_screen_colours_and_cursor() {
        let mut e = Emulator::new(12, 4, SCROLLBACK);
        for i in 0..10 {
            e.feed(format!("\x1b[3{}mline {i}\x1b[0m\r\n", i % 8).as_bytes());
        }
        e.feed("\x1b[1;4;38;2;1;2;3;48;5;200mbold\x1b[0m 漢字 ❤\u{fe0f}".as_bytes());
        e.feed(b"\x1b[2;3H");
        let copy = rebuilt(&mut e);
        assert_same(&e, &copy);
        assert_eq!(copy.cursor(), (1, 2));
    }

    #[test]
    fn wrapped_lines_stay_one_line() {
        let mut e = Emulator::new(5, 3, SCROLLBACK);
        e.feed("abcdefgh\r\nxyz 漢字漢".as_bytes());
        let mut copy = rebuilt(&mut e);
        assert_same(&e, &copy);
        // A narrower screen rewraps a wrapped line as one.
        e.resize(10, 3);
        copy.resize(10, 3);
        assert_same(&e, &copy);
    }

    #[test]
    fn a_coloured_background_to_the_line_end_is_kept_but_not_spread() {
        let mut e = Emulator::new(8, 3, SCROLLBACK);
        e.feed(b"\x1b[44mblue\x1b[K\r\nplain");
        let copy = rebuilt(&mut e);
        assert_same(&e, &copy);
    }

    #[test]
    fn rebuilds_both_screens_and_their_kitty_flags_while_the_alternate_one_shows() {
        let mut e = Emulator::new(10, 3, SCROLLBACK);
        e.feed(b"one\r\ntwo\r\nthree\r\nfour");
        e.feed(b"\x1b[>1u\x1b[?1049h\x1b[>5u\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[?25l");
        e.feed(b"\x1b[H\x1b[2Jfull\r\nscreen");
        let before = e.snapshot(None);
        let mut copy = rebuilt(&mut e);
        assert_eq!(
            e.snapshot(None),
            before,
            "a snapshot leaves the emulator as it was"
        );
        assert_same(&e, &copy);
        let mut keys = KeyModeScanner::default();
        keys.feed(&before);
        assert_eq!(keys.kitty_flags(), 5);
        // Leaving the alternate screen shows the same normal screen, with
        // its own kitty flags.
        for t in [&mut e, &mut copy] {
            t.feed(b"\x1b[?1049l\x1b[?u");
        }
        assert_same(&e, &copy);
        assert_eq!(e.take_replies(), copy.take_replies());
    }

    #[test]
    fn rebuilds_the_scroll_region_and_origin_mode() {
        let mut e = Emulator::new(10, 6, SCROLLBACK);
        e.feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
        e.feed(b"\x1b[2;4r\x1b[4;3H");
        let mut copy = Emulator::new(10, 6, SCROLLBACK);
        copy.feed(&e.snapshot(Some((2, 4))));
        let mut whole = rebuilt(&mut e);
        assert_same(&e, &copy);
        // A new line at the region's bottom scrolls only the region.
        for t in [&mut e, &mut copy, &mut whole] {
            t.feed(b"\nnew");
        }
        assert_same(&e, &copy);
        assert_eq!(copy.row_text(0), "one");
        assert_eq!(copy.row_text(4), "five");
        assert_ne!(picture(&e), picture(&whole), "the region matters here");
        // In origin mode the cursor moves within the region.
        e.feed(b"\x1b[?6h\x1b[2;2H");
        let mut copy = Emulator::new(10, 6, SCROLLBACK);
        copy.feed(&e.snapshot(Some((2, 4))));
        assert_same(&e, &copy);
        for t in [&mut e, &mut copy] {
            t.feed(b"\x1b[1;1Hx");
        }
        assert_same(&e, &copy);
        assert_eq!(copy.cursor(), (1, 1));
    }

    #[test]
    fn rebuilds_modes_that_are_off_by_default_and_on_by_default() {
        let mut e = Emulator::new(10, 3, SCROLLBACK);
        e.feed(b"\x1b[?1h\x1b[?1000h\x1b[?7l\x1b=x");
        let copy = rebuilt(&mut e);
        assert_same(&e, &copy);
        assert!(copy.modes().app_cursor);
    }

    #[test]
    fn the_answering_emulator_keeps_its_replies_and_the_other_drops_them() {
        let mut answering = Emulator::answering(10, 3, 0);
        answering.feed(b"\x1b[c\x1b[>1u\x1b[?u");
        assert_eq!(answering.take_replies(), b"\x1b[?6c\x1b[?1u");
        assert!(answering.take_replies().is_empty(), "taken once");
        let mut quiet = Emulator::new(10, 3, 0);
        quiet.feed(b"\x1b[c");
        assert!(quiet.take_replies().is_empty());
    }

    #[test]
    fn history_text_is_the_normal_screens_lines_with_their_colours() {
        let mut e = Emulator::new(5, 3, SCROLLBACK);
        e.feed(b"one\r\n\x1b[31mred\x1b[0m\r\nabcdefg\r\n\r\n");
        e.feed(b"\x1b[?1049h\x1b[Hfull");
        let text = String::from_utf8(e.history_text()).unwrap();
        assert_eq!(
            text, "one\r\n\x1b[0;31mred\x1b[0m\r\nabcdefg\x1b[0m\r\n",
            "no blank lines after; a wrapped line stays one"
        );
        assert!(e.modes().alt_screen, "left as it was");
        assert!(Emulator::new(5, 3, SCROLLBACK).history_text().is_empty());
    }
}
