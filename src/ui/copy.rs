//! A terminal in copy mode: its frozen view with every search match shown
//! in the matches' colours, the copy-mode cursor, and a status line over
//! its last row (its first while the cursor is on the last):
//!
//! ```text
//!  COPY  line 120/5000  match 2/7          v select  y copy  / search_up
//! ```
//!
//! or, while typing a search, the prompt: ` /error`.

use super::session_view;
use crate::input::line::LineEdit;
use crate::terminal::emulator::{Emulator, Found, Spot};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

/// The copy-mode line.
pub enum Status<'a> {
    /// Where the cursor is (` COPY  line N/M`, the match), and key hints
    /// shown on the right as far as they fit.
    Info { left: String, hints: &'a [String] },
    /// The search being typed, after its key (`/` or `?`).
    Prompt { key: char, line: &'a LineEdit },
}

/// A terminal in copy mode, as drawn now.
pub struct CopyView<'a> {
    pub cursor: Spot,
    /// The matches of the last search, top to bottom.
    pub found: &'a [Found],
    pub status: Status<'a>,
}

impl CopyView<'_> {
    /// Draw `emu` into `area`. Returns where the cursor goes: in the search
    /// prompt while typing, else on the copy-mode cursor if it shows.
    pub fn render(&self, emu: &Emulator, area: Rect, buf: &mut Buffer) -> Option<Position> {
        if area.is_empty() {
            return None;
        }
        // Only the matches on screen, and those found once per cell.
        let top = emu.view_line(0);
        let bottom = emu.view_line(area.height.saturating_sub(1));
        let shown: Vec<&Found> = self
            .found
            .iter()
            .filter(|f| f.end.line >= top && f.start.line <= bottom)
            .collect();
        let marked = |row: u16, col: u16| {
            let spot = Spot::new(emu.view_line(row), usize::from(col));
            shown.iter().any(|f| f.contains(spot))
        };
        session_view::cells_marked(emu, 0, area, buf, marked);
        let cursor = emu
            .view_of(self.cursor)
            .filter(|(row, col)| *row < area.height && *col < area.width);
        // The status line covers the last row, or the first while the
        // cursor is on the last.
        let last = area.height - 1;
        let y = match cursor {
            Some((row, _)) if row == last && last > 0 => area.y,
            _ => area.y + last,
        };
        let line = Rect::new(area.x, y, area.width, 1);
        let typed = self.status_line(line, buf);
        typed.or_else(|| cursor.map(|(row, col)| Position::new(area.x + col, area.y + row)))
    }

    /// Draw the status line at `line`; returns the prompt's cursor while
    /// typing a search.
    fn status_line(&self, line: Rect, buf: &mut Buffer) -> Option<Position> {
        let style = Style::default().add_modifier(Modifier::REVERSED);
        buf.set_style(line, style);
        let width = usize::from(line.width);
        match &self.status {
            Status::Info { left, hints } => {
                let left_width = left.chars().count();
                buf.set_stringn(line.x, line.y, left, width, style);
                // Hints in order, as many as fit with a gap after the left.
                let mut shown = String::new();
                for hint in hints.iter() {
                    let next = if shown.is_empty() {
                        hint.clone()
                    } else {
                        format!("{shown}  {hint}")
                    };
                    if left_width + 2 + next.chars().count() + 1 > width {
                        break;
                    }
                    shown = next;
                }
                if !shown.is_empty() {
                    let len = u16::try_from(shown.chars().count() + 1).unwrap_or(line.width);
                    let x = line.right().saturating_sub(len);
                    buf.set_string(x, line.y, &shown, style);
                }
                None
            }
            Status::Prompt { key, line: text } => {
                let head = format!(" {key}");
                buf.set_stringn(line.x, line.y, &head, width, style);
                let used = u16::try_from(head.chars().count()).unwrap_or(line.width);
                let room = line.width.saturating_sub(used);
                if room == 0 {
                    return None;
                }
                let (shown, at) = text.window(room);
                buf.set_stringn(line.x + used, line.y, &shown, usize::from(room), style);
                Some(Position::new(line.x + used + at, line.y))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::session_view::{MATCH_BG, MATCH_FG};

    fn rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn emulator(text: &str) -> Emulator {
        let mut e = Emulator::new(30, 4, 100);
        e.feed(text.as_bytes());
        e
    }

    #[test]
    fn matches_are_coloured_and_the_status_line_fits_what_it_can() {
        let e = emulator("an error\r\nok\r\nerror again");
        let found = e.find("error");
        let hints = [
            "v select".to_string(),
            "y copy".to_string(),
            "q leave".to_string(),
        ];
        let view = CopyView {
            cursor: Spot::new(1, 1),
            found: &found,
            status: Status::Info {
                left: " COPY  line 2/4".into(),
                hints: &hints,
            },
        };
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        let at = view.render(&e, area, &mut buf);
        assert_eq!(at, Some(Position::new(1, 1)));
        let shown = rows(&buf);
        // `y copy` would not fit with a gap on either side.
        assert_eq!(shown[3], " COPY  line 2/4      v select ");
        assert!(buf[(0, 3)].modifier.contains(Modifier::REVERSED));
        for x in 3..8 {
            assert_eq!((buf[(x, 0)].fg, buf[(x, 0)].bg), (MATCH_FG, MATCH_BG));
        }
        assert_ne!(buf[(2, 0)].bg, MATCH_BG);
        assert_eq!(buf[(0, 2)].bg, MATCH_BG);
    }

    #[test]
    fn the_status_line_moves_up_off_the_cursor_and_shows_the_prompt() {
        let e = emulator("a\r\nb\r\nc\r\nd");
        let line = LineEdit::new("err");
        let view = CopyView {
            cursor: Spot::new(3, 0),
            found: &[],
            status: Status::Prompt {
                key: '/',
                line: &line,
            },
        };
        let area = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(area);
        let at = view.render(&e, area, &mut buf);
        let shown = rows(&buf);
        assert!(shown[0].starts_with(" /err"), "{shown:?}");
        assert_eq!(shown[3].trim_end(), "d");
        assert_eq!(at, Some(Position::new(5, 0)), "the prompt's cursor");
    }
}
