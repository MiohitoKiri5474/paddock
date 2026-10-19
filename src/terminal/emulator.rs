//! A terminal emulator for one terminal's raw output (alacritty_terminal,
//! ADR 0005), with our own scrollback.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Processor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One screen cell, independent of the emulator crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    /// Second half of a double-width character; draw nothing.
    pub spacer: bool,
    pub fg: Colour,
    pub bg: Colour,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    Off,
    Press,
    Drag,
    Motion,
}

/// Modes the program set that change how paddock talks to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    pub sgr_mouse: bool,
    pub alt_screen: bool,
    pub show_cursor: bool,
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Emulator {
    term: Term<VoidListener>,
    parser: Processor,
    /// Where the current selection started.
    anchor: Option<Point>,
}

impl Emulator {
    /// `VoidListener` drops the emulator's replies to terminal queries: tmux
    /// already answers them for the program.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            kitty_keyboard: true,
            ..Config::default()
        };
        let size = Size {
            cols: cols.into(),
            rows: rows.into(),
        };
        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            anchor: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size {
            cols: cols.max(2).into(),
            rows: rows.max(1).into(),
        });
    }

    /// (columns, rows)
    pub fn size(&self) -> (u16, u16) {
        let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
        (clamp(self.term.columns()), clamp(self.term.screen_lines()))
    }

    /// The cell shown at a viewport position, taking the scroll offset into account.
    pub fn cell(&self, row: u16, col: u16) -> Cell {
        let offset = i32::try_from(self.scroll_offset()).unwrap_or(i32::MAX);
        self.cell_at(Line(i32::from(row) - offset), col)
    }

    fn cell_at(&self, line: Line, col: u16) -> Cell {
        let c = &self.term.grid()[line][Column(col.into())];
        let f = c.flags;
        let spacer = f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
        let mut text = String::new();
        if !spacer {
            text.push(c.c);
            if let Some(extra) = c.zerowidth() {
                text.extend(extra);
            }
        }
        Cell {
            text,
            spacer,
            fg: colour(c.fg),
            bg: colour(c.bg),
            bold: f.contains(Flags::BOLD),
            dim: f.contains(Flags::DIM),
            italic: f.contains(Flags::ITALIC),
            underline: f.intersects(Flags::ALL_UNDERLINES),
            inverse: f.contains(Flags::INVERSE),
        }
    }

    /// Text of a viewport row with trailing spaces removed.
    #[cfg(test)]
    pub fn row_text(&self, row: u16) -> String {
        let (cols, _) = self.size();
        let text: String = (0..cols)
            .map(|c| self.cell(row, c))
            .filter(|c| !c.spacer)
            .map(|c| c.text)
            .collect();
        text.trim_end().to_string()
    }

    /// Cursor (row, column) on the live screen.
    pub fn cursor(&self) -> (u16, u16) {
        let p = self.term.grid().cursor.point;
        (
            u16::try_from(p.line.0.max(0)).unwrap_or(0),
            u16::try_from(p.column.0).unwrap_or(0),
        )
    }

    pub fn modes(&self) -> Modes {
        let m = *self.term.mode();
        let mouse = if m.contains(TermMode::MOUSE_MOTION) {
            MouseMode::Motion
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseMode::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseMode::Press
        } else {
            MouseMode::Off
        };
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            mouse,
            sgr_mouse: m.contains(TermMode::SGR_MOUSE),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            show_cursor: m.contains(TermMode::SHOW_CURSOR),
        }
    }

    /// Scroll the view by `lines` (positive = back into history).
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn scroll_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// The grid point under a viewport position, clamped to the screen.
    fn point(&self, row: u16, col: u16) -> Point {
        let (cols, rows) = self.size();
        let offset = i32::try_from(self.scroll_offset()).unwrap_or(i32::MAX);
        Point::new(
            Line(i32::from(row.min(rows - 1)) - offset),
            Column(col.min(cols - 1).into()),
        )
    }

    /// Start selecting at a viewport position. Nothing is selected until
    /// [`select_to`](Self::select_to).
    pub fn select(&mut self, row: u16, col: u16) {
        let anchor = self.point(row, col);
        self.anchor = Some(anchor);
        self.term.selection = Some(Selection::new(SelectionType::Simple, anchor, Side::Left));
    }

    /// Select from the anchor to a viewport position, both cells included,
    /// in either direction.
    pub fn select_to(&mut self, row: u16, col: u16) {
        let Some(anchor) = self.anchor else {
            return;
        };
        let point = self.point(row, col);
        let (from, to) = if point >= anchor {
            (Side::Left, Side::Right)
        } else {
            (Side::Right, Side::Left)
        };
        let mut selection = Selection::new(SelectionType::Simple, anchor, from);
        selection.update(point, to);
        self.term.selection = Some(selection);
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
        self.term.selection = None;
    }

    /// The selected text: lines joined with `\n`, trailing spaces removed.
    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|t| !t.is_empty())
    }

    /// The selected cells, for drawing.
    pub fn selection(&self) -> Option<Selected> {
        let range = self.term.selection.as_ref()?.to_range(&self.term)?;
        Some(Selected {
            range,
            offset: i32::try_from(self.scroll_offset()).unwrap_or(i32::MAX),
        })
    }
}

/// The selected cells of an emulator's view.
pub struct Selected {
    range: SelectionRange,
    offset: i32,
}

impl Selected {
    /// Whether the cell at a viewport position is selected.
    pub fn contains(&self, row: u16, col: u16) -> bool {
        let line = Line(i32::from(row) - self.offset);
        self.range.contains(Point::new(line, Column(col.into())))
    }
}

fn colour(c: AnsiColor) -> Colour {
    match c {
        AnsiColor::Spec(rgb) => Colour::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(i) => Colour::Indexed(i),
        AnsiColor::Named(n) => {
            let i = n as usize;
            let dim = NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize;
            if i < 16 {
                Colour::Indexed(i as u8)
            } else if dim.contains(&i) {
                Colour::Indexed((i - NamedColor::DimBlack as usize) as u8)
            } else {
                Colour::Default
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_truecolor_and_wide_characters() {
        let mut e = Emulator::new(20, 3, 100);
        e.feed("\x1b[38;2;10;20;30mA\x1b[0m漢❤\u{fe0f}".as_bytes());
        assert_eq!(e.cell(0, 0).fg, Colour::Rgb(10, 20, 30));
        assert!(e.cell(0, 2).spacer);
        assert_eq!(
            e.cell(0, 3).text,
            "❤\u{fe0f}",
            "VS16 emoji is one wide cell"
        );
        assert!(e.cell(0, 4).spacer);
        assert_eq!(e.row_text(0), "A漢❤\u{fe0f}");
        assert_eq!(e.cursor(), (0, 5));
    }

    #[test]
    fn scrolls_own_history() {
        let mut e = Emulator::new(10, 3, 100);
        for i in 0..10 {
            e.feed(format!("l{i}\r\n").as_bytes());
        }
        assert_eq!(e.history_len(), 8);
        e.scroll(3);
        assert_eq!(e.scroll_offset(), 3);
        assert_eq!(e.row_text(0), "l5");
        e.scroll(1000);
        assert_eq!(e.row_text(0), "l0");
        e.scroll_to_bottom();
        assert_eq!(e.row_text(0), "l8");
    }

    #[test]
    fn reports_modes() {
        let mut e = Emulator::new(10, 3, 0);
        assert!(e.modes().show_cursor);
        e.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?1049h\x1b[?25l");
        let m = e.modes();
        assert!(m.app_cursor && m.bracketed_paste && m.sgr_mouse && m.alt_screen && !m.show_cursor);
        assert_eq!(m.mouse, MouseMode::Drag);
    }

    #[test]
    fn selections_copy_text_in_reading_order_either_way() {
        let mut e = Emulator::new(12, 3, 10);
        e.feed("ab 漢字\r\ncd   \r\nef".as_bytes());
        e.select(0, 1);
        assert_eq!(e.selected_text(), None, "a click selects nothing");
        e.select_to(1, 0);
        assert_eq!(e.selected_text().as_deref(), Some("b 漢字\nc"));
        assert!(e
            .selection()
            .is_some_and(|s| s.contains(0, 5) && !s.contains(1, 1)));
        // Backwards: the same cells, both ends included.
        e.select(2, 1);
        e.select_to(1, 1);
        assert_eq!(e.selected_text().as_deref(), Some("d\nef"));
        e.clear_selection();
        assert_eq!(e.selected_text(), None);
        assert!(e.selection().is_none());
    }

    #[test]
    fn selections_follow_the_scrolled_view() {
        let mut e = Emulator::new(10, 2, 100);
        for i in 0..6 {
            e.feed(format!("l{i}\r\n").as_bytes());
        }
        e.scroll(2);
        e.select(0, 0);
        e.select_to(1, 1);
        assert_eq!(e.selected_text().as_deref(), Some("l3\nl4"));
    }

    #[test]
    fn named_colours_map_to_palette_indexes() {
        let mut e = Emulator::new(10, 1, 0);
        e.feed(b"\x1b[31;102mx\x1b[2;33my");
        assert_eq!(e.cell(0, 0).fg, Colour::Indexed(1));
        assert_eq!(e.cell(0, 0).bg, Colour::Indexed(10));
        assert_eq!(e.cell(0, 1).fg, Colour::Indexed(3));
        assert!(e.cell(0, 1).dim);
    }
}
