//! Copy mode over an emulator's grid: a cursor moving over the scrollback
//! and the screen, selections between two places, and search. The view
//! stays where it is while copy mode holds the terminal's output back
//! (`Terminal::pause`), so places stay put.

use super::Emulator;
use crate::terminal::search;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell as GridCell, Flags};
use alacritty_terminal::vi_mode::{ViModeCursor, ViMotion};

/// A cell of the grid: `line` counts from the live screen's top row (0),
/// negative back into the scrollback; `col` from the left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub line: i32,
    pub col: usize,
}

impl Spot {
    pub fn new(line: i32, col: usize) -> Self {
        Self { line, col }
    }

    fn point(self) -> Point {
        Point::new(Line(self.line), Column(self.col))
    }

    fn of(p: Point) -> Self {
        Self::new(p.line.0, p.column.0)
    }
}

/// A move of the copy-mode cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Down,
    Up,
    Right,
    /// The start of the next word (`w`).
    WordNext,
    /// The start of this or the previous word (`b`).
    WordBack,
    /// The end of this or the next word (`e`).
    WordEnd,
    /// The first column (`0`).
    LineStart,
    /// The last character of the line (`$`), across wrapped rows.
    LineEnd,
    /// The first line of the scrollback (`g`).
    Top,
    /// The last line of the screen (`G`).
    Bottom,
    /// The view and the cursor this many lines back into the history
    /// (forward if negative): half pages and pages.
    Scroll(i32),
}

/// A match of a search: its first and last cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub start: Spot,
    pub end: Spot,
}

impl Found {
    /// Whether `spot` is one of the match's cells.
    pub fn contains(&self, spot: Spot) -> bool {
        self.start <= spot && spot <= self.end
    }
}

impl Emulator {
    /// Where the copy-mode cursor starts: on the terminal's cursor in the
    /// live view, else at the start of the view's last row.
    pub fn copy_start(&self) -> Spot {
        let offset = self.offset();
        if offset == 0 {
            let p = self.term.grid().cursor.point;
            let col = p.column.0.min(self.term.columns() - 1);
            return Spot::new(p.line.0, col);
        }
        let last_row = i32::from(self.size().1) - 1;
        Spot::new(last_row - offset, 0)
    }

    /// The view's top line: the history line shown on its first row.
    fn offset(&self) -> i32 {
        i32::try_from(self.scroll_offset()).unwrap_or(i32::MAX)
    }

    /// The line shown on viewport row `row`.
    pub fn view_line(&self, row: u16) -> i32 {
        i32::from(row) - self.offset()
    }

    /// The viewport (row, column) where `spot` shows, if it does.
    pub fn view_of(&self, spot: Spot) -> Option<(u16, u16)> {
        let row = u16::try_from(spot.line + self.offset()).ok()?;
        let col = u16::try_from(spot.col).ok()?;
        let (cols, rows) = self.size();
        (row < rows && col < cols).then_some((row, col))
    }

    /// `spot`'s line counted from the first line of the scrollback (1),
    /// and how many lines there are: history and screen.
    pub fn line_number(&self, spot: Spot) -> (usize, usize) {
        let history = i64::try_from(self.history_len()).unwrap_or(i64::MAX);
        let n = usize::try_from(i64::from(spot.line) + history + 1).unwrap_or(0);
        (n, self.term.total_lines())
    }

    /// `spot` moved by `motion`, the view scrolled to show it.
    pub fn motion(&mut self, spot: Spot, motion: Motion) -> Spot {
        let spot = self.clamped(spot);
        let vi = |m| Some(m);
        let by = match motion {
            Motion::Left => vi(ViMotion::Left),
            Motion::Down => vi(ViMotion::Down),
            Motion::Up => vi(ViMotion::Up),
            Motion::Right => vi(ViMotion::Right),
            Motion::WordNext => vi(ViMotion::SemanticRight),
            Motion::WordBack => vi(ViMotion::SemanticLeft),
            Motion::WordEnd => vi(ViMotion::SemanticRightEnd),
            Motion::LineStart => vi(ViMotion::First),
            Motion::LineEnd => vi(ViMotion::Last),
            Motion::Top | Motion::Bottom | Motion::Scroll(_) => None,
        };
        let point = match (by, motion) {
            (Some(m), _) => {
                ViModeCursor::new(spot.point())
                    .motion(&mut self.term, m)
                    .point
            }
            (None, Motion::Top) => Point::new(self.term.topmost_line(), Column(0)),
            (None, Motion::Bottom) => Point::new(self.term.bottommost_line(), Column(0)),
            (None, Motion::Scroll(lines)) => {
                self.term.scroll_display(Scroll::Delta(lines));
                let moved = Spot::new(spot.line.saturating_sub(lines), spot.col);
                self.clamped(moved).point()
            }
            (None, _) => spot.point(),
        };
        self.term.scroll_to_point(point);
        Spot::of(point)
    }

    /// Scroll the view just enough to show `spot`.
    pub fn reveal(&mut self, spot: Spot) {
        let spot = self.clamped(spot);
        self.term.scroll_to_point(spot.point());
    }

    /// Scroll the view by `lines` (positive = back into history), keeping
    /// `spot` in it: returns where it is now.
    pub fn wheel(&mut self, spot: Spot, lines: i32) -> Spot {
        self.term.scroll_display(Scroll::Delta(lines));
        let top = -self.offset();
        let bottom = top + i32::from(self.size().1) - 1;
        self.clamped(Spot::new(spot.line.clamp(top, bottom), spot.col))
    }

    /// `spot` inside the grid, off the second half of a wide character.
    fn clamped(&self, spot: Spot) -> Spot {
        let top = self.term.topmost_line().0;
        let bottom = self.term.bottommost_line().0;
        let mut s = Spot::new(
            spot.line.clamp(top, bottom),
            spot.col.min(self.term.columns() - 1),
        );
        if s.col > 0 && self.is_spacer(s) {
            s.col -= 1;
        }
        s
    }

    fn is_spacer(&self, spot: Spot) -> bool {
        is_spacer(&self.term.grid()[spot.point()])
    }

    /// Select from `anchor` to `cursor`, both cells included, in either
    /// direction; the lines they are on if `whole_lines`.
    pub fn select_between(&mut self, anchor: Spot, cursor: Spot, whole_lines: bool) {
        let (anchor, cursor) = (self.clamped(anchor), self.clamped(cursor));
        let selection = if whole_lines {
            let mut s = Selection::new(SelectionType::Lines, anchor.point(), Side::Left);
            s.update(cursor.point(), Side::Right);
            s
        } else {
            let (from, to) = if cursor >= anchor {
                (Side::Left, Side::Right)
            } else {
                (Side::Right, Side::Left)
            };
            let mut s = Selection::new(SelectionType::Simple, anchor.point(), from);
            s.update(cursor.point(), to);
            s
        };
        self.term.selection = Some(selection);
    }

    /// Every match of `query` in the scrollback and the screen, top to
    /// bottom. Rows wrapped into the next are searched as one line.
    pub fn find(&self, query: &str) -> Vec<Found> {
        let grid = self.term.grid();
        let last = self.term.last_column();
        let len = query.chars().count();
        let mut found = Vec::new();
        let mut text: Vec<char> = Vec::new();
        let mut spots: Vec<Spot> = Vec::new();
        for line in self.term.topmost_line().0..=self.term.bottommost_line().0 {
            let row = &grid[Line(line)];
            for col in 0..=last.0 {
                let cell = &row[Column(col)];
                if is_spacer(cell) {
                    continue;
                }
                text.push(cell.c);
                spots.push(Spot::new(line, col));
            }
            if row[last].flags.contains(Flags::WRAPLINE) {
                continue;
            }
            found.extend(search::find(&text, query).into_iter().map(|i| {
                // A match ending on a wide character ends on its second half.
                let mut end = spots[i + len - 1];
                if grid[end.point()].flags.contains(Flags::WIDE_CHAR) {
                    end.col += 1;
                }
                Found {
                    start: spots[i],
                    end,
                }
            }));
            text.clear();
            spots.clear();
        }
        found
    }
}

/// Whether `cell` is the second half of a wide character, or the empty
/// cell before one wrapped onto the next row: no character of its own.
fn is_spacer(cell: &GridCell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An emulator `cols` wide and `rows` high holding `lines` (each
    /// ended with a newline but the last), with all the history it needs.
    fn grid(cols: u16, rows: u16, lines: &[&str]) -> Emulator {
        let mut e = Emulator::new(cols, rows, 100);
        e.feed(lines.join("\r\n").as_bytes());
        e
    }

    /// The text of grid line `line`, trailing spaces removed.
    fn line_text(e: &Emulator, line: i32) -> String {
        let row = &e.term.grid()[Line(line)];
        let text: String = (0..e.term.columns()).map(|c| row[Column(c)].c).collect();
        text.trim_end().to_string()
    }

    #[test]
    fn starts_on_the_terminal_cursor_or_at_the_view_bottom() {
        let mut e = grid(10, 3, &["a", "b", "c", "d", "ef"]);
        assert_eq!(e.copy_start(), Spot::new(2, 2));
        e.scroll(2);
        assert_eq!(e.copy_start(), Spot::new(0, 0));
        assert_eq!(e.view_of(Spot::new(0, 0)), Some((2, 0)));
        assert_eq!(e.view_of(Spot::new(1, 0)), None);
        assert_eq!(e.view_line(0), -2);
    }

    #[test]
    fn moves_by_character_and_line_and_stays_in_the_grid() {
        let mut e = grid(10, 3, &["one", "two", "three"]);
        let s = Spot::new(2, 0);
        assert_eq!(e.motion(s, Motion::Left), s);
        assert_eq!(e.motion(s, Motion::Right), Spot::new(2, 1));
        assert_eq!(e.motion(s, Motion::Down), s);
        assert_eq!(e.motion(s, Motion::Up), Spot::new(1, 0));
        let top = e.motion(s, Motion::Top);
        assert_eq!(top, Spot::new(0, 0));
        assert_eq!(e.motion(top, Motion::Up), top);
    }

    #[test]
    fn moves_over_the_history_and_scrolls_the_view_with_it() {
        let lines: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut e = grid(10, 3, &lines);
        assert_eq!(e.history_len(), 7);
        let top = e.motion(Spot::new(2, 0), Motion::Top);
        assert_eq!(top, Spot::new(-7, 0));
        assert_eq!(line_text(&e, top.line), "l0");
        assert_eq!(e.scroll_offset(), 7, "the view shows the top");
        let bottom = e.motion(top, Motion::Bottom);
        assert_eq!(bottom, Spot::new(2, 0));
        assert_eq!(e.scroll_offset(), 0);
        // A page up moves the view and the cursor together.
        let up = e.motion(bottom, Motion::Scroll(3));
        assert_eq!((up, e.scroll_offset()), (Spot::new(-1, 0), 3));
        let down = e.motion(up, Motion::Scroll(-1));
        assert_eq!((down, e.scroll_offset()), (Spot::new(0, 0), 2));
        // Past either end, both stop there.
        let far = e.motion(down, Motion::Scroll(100));
        assert_eq!((far, e.scroll_offset()), (Spot::new(-7, 0), 7));
        assert_eq!(e.motion(far, Motion::Scroll(-100)), Spot::new(2, 0));
    }

    #[test]
    fn moves_by_word_and_to_line_ends() {
        let mut e = grid(20, 2, &["cargo  test --locked", "next"]);
        let s = Spot::new(0, 0);
        let w = e.motion(s, Motion::WordNext);
        assert_eq!(w, Spot::new(0, 7));
        assert_eq!(e.motion(w, Motion::WordEnd), Spot::new(0, 10));
        assert_eq!(e.motion(w, Motion::WordBack), s);
        assert_eq!(e.motion(s, Motion::WordEnd), Spot::new(0, 4));
        let end = e.motion(s, Motion::LineEnd);
        assert_eq!(end, Spot::new(0, 19));
        assert_eq!(e.motion(end, Motion::LineStart), s);
        // The last word of a line goes on to the next line's first.
        let last = Spot::new(0, 14);
        assert_eq!(e.motion(last, Motion::WordNext), Spot::new(1, 0));
    }

    #[test]
    fn line_ends_follow_wrapped_rows() {
        // 12 characters on 5 columns: rows 0 and 1 wrap into the next.
        let mut e = grid(5, 4, &["abcdefghijkl", "x"]);
        let end = e.motion(Spot::new(0, 1), Motion::LineEnd);
        assert_eq!(end, Spot::new(0, 4));
        assert_eq!(e.motion(end, Motion::LineEnd), Spot::new(2, 1));
        // `0` goes to the row's start, then again to the line's.
        let start = e.motion(Spot::new(2, 1), Motion::LineStart);
        assert_eq!(start, Spot::new(2, 0));
        assert_eq!(e.motion(start, Motion::LineStart), Spot::new(0, 0));
    }

    #[test]
    fn the_wheel_keeps_the_cursor_in_the_view() {
        let lines: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut e = grid(10, 3, &lines);
        let s = e.wheel(Spot::new(2, 1), 3);
        assert_eq!((s, e.scroll_offset()), (Spot::new(-1, 1), 3));
        let s = e.wheel(s, -1);
        assert_eq!(s, Spot::new(-1, 1), "still in sight");
    }

    #[test]
    fn selections_copy_characters_or_whole_lines_across_wrapped_rows() {
        let mut e = grid(5, 4, &["abcdefghij", "xy"]);
        // `c` on row 0 to `g` on row 1: one wrapped line, no newline.
        e.select_between(Spot::new(0, 2), Spot::new(1, 1), false);
        assert_eq!(e.selected_text().as_deref(), Some("cdefg"));
        // Backwards gives the same cells.
        e.select_between(Spot::new(1, 1), Spot::new(0, 2), false);
        assert_eq!(e.selected_text().as_deref(), Some("cdefg"));
        // Into the next line, which is not wrapped: a newline.
        e.select_between(Spot::new(1, 3), Spot::new(2, 0), false);
        assert_eq!(e.selected_text().as_deref(), Some("ij\nx"));
        // Whole lines from anywhere in them.
        e.select_between(Spot::new(2, 1), Spot::new(1, 2), true);
        assert_eq!(e.selected_text().as_deref(), Some("abcdefghij\nxy"));
        assert!(e.selection().is_some_and(|s| s.contains(0, 0)));
    }

    #[test]
    fn selections_reach_into_the_history() {
        let mut e = grid(10, 2, &["first", "second", "third", "fourth"]);
        e.select_between(Spot::new(-2, 0), Spot::new(-1, 2), false);
        assert_eq!(e.selected_text().as_deref(), Some("first\nsec"));
    }

    #[test]
    fn finds_matches_in_the_history_and_across_wrapped_rows() {
        let mut e = grid(6, 2, &["error one", "ok", "Error two", "end"]);
        // "error one" wraps: `error ` then `one`.
        let found = e.find("error");
        let starts: Vec<Spot> = found.iter().map(|f| f.start).collect();
        assert_eq!(starts, [Spot::new(-4, 0), Spot::new(-1, 0)]);
        assert_eq!(found[0].end, Spot::new(-4, 4));
        assert!(e.find("Error").len() == 1);
        let across = e.find("r one");
        assert_eq!(
            across,
            [Found {
                start: Spot::new(-4, 4),
                end: Spot::new(-3, 2)
            }]
        );
        assert!(across[0].contains(Spot::new(-3, 0)));
        assert!(!across[0].contains(Spot::new(-4, 3)));
        assert!(e.find("missing").is_empty());
        e.feed("\r\n漢err".as_bytes());
        let wide = e.find("err");
        assert_eq!(wide.last().map(|f| f.start), Some(Spot::new(1, 2)));
    }

    #[test]
    fn a_match_ending_on_a_wide_character_covers_both_its_halves() {
        let e = grid(10, 1, &["a漢字b"]);
        let found = e.find("a漢");
        assert_eq!(
            found,
            [Found {
                start: Spot::new(0, 0),
                end: Spot::new(0, 2)
            }]
        );
        let wide = e.find("字");
        assert_eq!(wide[0].start, Spot::new(0, 3));
        assert!(wide[0].contains(Spot::new(0, 4)), "its second half");
        assert!(!wide[0].contains(Spot::new(0, 5)));
    }

    #[test]
    fn line_numbers_count_from_the_top_of_the_history() {
        let e = grid(10, 2, &["a", "b", "c", "d"]);
        assert_eq!(e.line_number(Spot::new(-2, 0)), (1, 4));
        assert_eq!(e.line_number(Spot::new(1, 0)), (4, 4));
    }
}
