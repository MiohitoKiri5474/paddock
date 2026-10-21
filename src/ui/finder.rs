//! The finder: a box centred over the whole screen, framed like the prompt
//! popup, with the line typed on its first row and the matching rows under
//! it, each with its kind as a marker and its detail dim; the selected row
//! is reversed.
//!
//! ```text
//! ╭──────────────── Finder ────────────────╮
//! │ cla█                                   │
//! │ agent  claude  waiting · api › claude  │
//! │ tab    api › claude  ~/project/api     │
//! ╰────────────────────────────────────────╯
//! ```

use super::prompt;
use crate::input::line::LineEdit;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, Widget};

/// The box's width on screens wide enough.
const WIDTH: u16 = 64;
/// Rows of matches shown at most.
const ROWS: u16 = 12;
/// Borders, the line typed and the rows.
const HEIGHT: u16 = 2 + 1 + ROWS;
/// Blank columns between the border and the text, each side.
const PAD: u16 = 1;
/// Columns the kind marker takes before the name.
const KIND: usize = 6;
/// Blank columns between the name and its detail.
const GAP: &str = "  ";
/// The only row without matches.
const EMPTY: &str = "no matches";

/// One matching row, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderLine {
    pub kind: &'static str,
    pub name: String,
    pub detail: String,
}

/// The finder as drawn now.
pub struct View<'a> {
    /// Shown in the top border, with the state filter.
    pub title: String,
    pub line: &'a LineEdit,
    /// The matches, best first.
    pub lines: Vec<FinderLine>,
    pub selected: usize,
    /// The border's colour: the accent (`[colors] tab_current`).
    pub accent: Color,
}

/// Where the box goes on a screen `screen`: centred, [`WIDTH`] by
/// [`HEIGHT`] or as much of the screen as there is.
pub fn area(screen: Rect) -> Rect {
    let width = WIDTH.min(screen.width);
    let height = HEIGHT.min(screen.height);
    Rect {
        x: screen.x + (screen.width - width) / 2,
        y: screen.y + (screen.height - height) / 2,
        width,
        height,
    }
}

/// The rows of a box at `area`: inside its border, under the line typed.
fn list(area: Rect) -> Rect {
    let inner = prompt::frame("", Color::Reset).inner(area);
    Rect {
        y: inner.y.saturating_add(1),
        height: inner.height.saturating_sub(1),
        ..inner
    }
}

/// The first of `count` matches shown in `room` rows: the selected one
/// stays in sight, at the bottom once past the room.
fn first(selected: usize, room: usize) -> usize {
    selected.saturating_sub(room.saturating_sub(1))
}

/// The index of the match on screen row `y` of a box at `area` showing
/// `count` matches with `selected` selected.
pub fn row_at(area: Rect, selected: usize, count: usize, y: u16) -> Option<usize> {
    let list = list(area);
    if !(list.y..list.bottom()).contains(&y) || list.width == 0 {
        return None;
    }
    let i = first(selected, usize::from(list.height)) + usize::from(y - list.y);
    (i < count).then_some(i)
}

impl View<'_> {
    /// Draw the box at `popup` (see [`area`]). Returns the cursor in the
    /// line typed.
    pub fn render(&self, popup: Rect, buf: &mut Buffer) -> Option<Position> {
        Clear.render(popup, buf);
        let block = prompt::frame(&self.title, self.accent);
        let inner = block.inner(popup);
        block.render(popup, buf);
        let text = Rect {
            x: inner.x.saturating_add(PAD),
            width: inner.width.saturating_sub(2 * PAD),
            ..inner
        };
        if text.is_empty() {
            return None;
        }
        let width = usize::from(text.width);
        let (shown, col) = self.line.window(text.width);
        buf.set_stringn(text.x, text.y, shown, width, Style::default());
        let cursor = Position::new(text.x + col, text.y);
        let rows = list(popup);
        let dim = Style::default().add_modifier(Modifier::DIM);
        if self.lines.is_empty() && rows.height > 0 {
            buf.set_stringn(text.x, rows.y, EMPTY, width, dim);
            return Some(cursor);
        }
        let skip = first(self.selected, usize::from(rows.height));
        for (i, y) in (skip..self.lines.len()).zip(rows.y..rows.bottom()) {
            let l = &self.lines[i];
            let selected = i == self.selected;
            let base = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            // The whole row carries the selection.
            buf.set_string(rows.x, y, " ".repeat(usize::from(rows.width)), base);
            let kind = format!("{:<KIND$}", l.kind);
            let x = buf
                .set_stringn(text.x, y, &kind, width, base.add_modifier(Modifier::DIM))
                .0;
            let room = usize::from(text.right().saturating_sub(x));
            let x = buf.set_stringn(x, y, &l.name, room, base).0;
            let room = usize::from(text.right().saturating_sub(x));
            if !l.detail.is_empty() && room > GAP.len() {
                let detail = format!("{GAP}{}", l.detail);
                buf.set_stringn(x, y, detail, room, base.patch(dim));
            }
        }
        Some(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(buf: &Buffer) -> Vec<String> {
        let a = buf.area;
        (0..a.height)
            .map(|y| (0..a.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn line(kind: &'static str, name: &str, detail: &str) -> FinderLine {
        FinderLine {
            kind,
            name: name.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn the_box_is_centred_and_never_larger_than_the_screen() {
        assert_eq!(area(Rect::new(0, 0, 100, 30)), Rect::new(18, 7, 64, 15));
        assert_eq!(area(Rect::new(0, 0, 40, 10)), Rect::new(0, 0, 40, 10));
        assert_eq!(area(Rect::new(5, 5, 0, 0)), Rect::new(5, 5, 0, 0));
    }

    #[test]
    fn draws_the_line_then_the_matches_with_the_selected_one_reversed() {
        let screen = Rect::new(0, 0, 50, 6);
        let mut buf = Buffer::empty(screen);
        let typed = LineEdit::new("cl");
        let view = View {
            title: " Finder ".into(),
            line: &typed,
            lines: vec![
                line("agent", "claude", "waiting · api › claude"),
                line("tab", "api › claude", "~/project/api"),
            ],
            selected: 1,
            accent: Color::Indexed(33),
        };
        let cursor = view.render(screen, &mut buf);
        let shown = rows(&buf);
        assert!(shown[0].contains(" Finder "), "{}", shown[0]);
        assert_eq!(shown[1], format!("│ cl{:45}│", ""));
        assert_eq!(
            shown[2],
            "│ agent claude  waiting · api › claude           │"
        );
        assert_eq!(
            shown[3],
            "│ tab   api › claude  ~/project/api              │"
        );
        assert_eq!(cursor, Some(Position::new(4, 1)));
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(33));
        assert!(buf[(2, 3)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(2, 2)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(2, 2)].modifier.contains(Modifier::DIM), "the kind");
        assert!(!buf[(8, 2)].modifier.contains(Modifier::DIM), "the name");
        assert!(buf[(17, 2)].modifier.contains(Modifier::DIM), "the detail");
    }

    #[test]
    fn without_matches_it_says_so() {
        let screen = Rect::new(0, 0, 30, 4);
        let mut buf = Buffer::empty(screen);
        let typed = LineEdit::new("zzz");
        let view = View {
            title: " Finder ".into(),
            line: &typed,
            lines: Vec::new(),
            selected: 0,
            accent: Color::Blue,
        };
        view.render(screen, &mut buf);
        assert_eq!(rows(&buf)[2], format!("│ no matches{:17}│", ""));
    }

    #[test]
    fn the_selected_match_scrolls_into_sight_and_clicks_find_it() {
        let screen = Rect::new(0, 0, 30, 5);
        let mut buf = Buffer::empty(screen);
        let typed = LineEdit::new("");
        let lines: Vec<FinderLine> = (0..5).map(|i| line("tab", &format!("t{i}"), "")).collect();
        let view = View {
            title: " Finder ".into(),
            line: &typed,
            lines,
            selected: 3,
            accent: Color::Blue,
        };
        view.render(screen, &mut buf);
        // Two rows of room: t2 and t3, the selected one at the bottom.
        assert!(rows(&buf)[2].starts_with("│ tab   t2"));
        assert!(rows(&buf)[3].starts_with("│ tab   t3"));
        assert_eq!(row_at(screen, 3, 5, 2), Some(2));
        assert_eq!(row_at(screen, 3, 5, 3), Some(3));
        assert_eq!(row_at(screen, 0, 1, 3), None, "past the matches");
        assert_eq!(row_at(screen, 0, 5, 1), None, "the line typed");
        assert_eq!(row_at(screen, 0, 5, 4), None, "the border");
    }

    #[test]
    fn renders_on_screens_too_small_for_it() {
        for (w, h) in [(0, 0), (1, 1), (3, 3), (4, 2)] {
            let screen = Rect::new(0, 0, w, h);
            let typed = LineEdit::new("abc");
            let view = View {
                title: " Finder ".into(),
                line: &typed,
                lines: vec![line("tab", "x", "y")],
                selected: 0,
                accent: Color::Blue,
            };
            view.render(area(screen), &mut Buffer::empty(screen));
        }
    }
}
