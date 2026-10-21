//! The prompt popup (after LazyVim's input): a small rounded box in the
//! middle of the screen, drawn over everything, its border in the accent
//! colour and its title in the top border. It holds a line being typed
//! (a name) or a question.
//!
//! ```text
//! ╭──────── Rename tab ────────╮
//! │ claude█                    │
//! ╰────────────────────────────╯
//! ```

use crate::input::line::LineEdit;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Widget};

/// The box's width on screens wide enough.
const WIDTH: u16 = 40;
/// Borders and one line.
const HEIGHT: u16 = 3;
/// Blank columns between the border and the text, each side.
const PAD: u16 = 1;

/// What the box holds.
pub enum Body<'a> {
    /// A line being typed, with its cursor.
    Input(&'a LineEdit),
    /// A question, answered by a key.
    Message(String),
}

/// The popup as drawn now: its title, what it holds, and its colour.
pub struct Prompt<'a> {
    /// Shown in the top border, e.g. ` Rename tab `.
    pub title: &'a str,
    /// The line being typed, or the question.
    pub body: Body<'a>,
    /// The border's colour: the accent (`[colors] tab_current`), as the
    /// current tab has.
    pub accent: Color,
}

/// Where the box goes on a screen `screen`: centred, [`WIDTH`] wide or
/// as wide as the screen if narrower.
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

/// The popup's border: rounded, in `accent`, with `title` centred in its
/// top. Floats are framed the same way.
pub fn frame(title: &str, accent: Color) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Line::from(title).centered())
}

impl Prompt<'_> {
    /// Draw the box at `popup` (see [`area`]). Returns the cursor while
    /// typing.
    pub fn render(&self, popup: Rect, buf: &mut Buffer) -> Option<Position> {
        Clear.render(popup, buf);
        let block = frame(self.title, self.accent);
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
        match &self.body {
            Body::Message(m) => {
                buf.set_stringn(text.x, text.y, m, width, Style::default());
                None
            }
            Body::Input(line) => {
                let (shown, col) = line.window(text.width);
                buf.set_stringn(text.x, text.y, shown, width, Style::default());
                Some(Position::new(text.x + col, text.y))
            }
        }
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

    #[test]
    fn the_box_is_centred_and_never_wider_than_the_screen() {
        assert_eq!(area(Rect::new(0, 0, 100, 30)), Rect::new(30, 13, 40, 3));
        assert_eq!(area(Rect::new(0, 0, 41, 10)), Rect::new(0, 3, 40, 3));
        assert_eq!(area(Rect::new(0, 0, 20, 2)), Rect::new(0, 0, 20, 2));
        assert_eq!(area(Rect::new(5, 5, 0, 0)), Rect::new(5, 5, 0, 0));
    }

    #[test]
    fn draws_a_rounded_accent_box_with_the_title_in_its_border() {
        let screen = Rect::new(0, 0, 50, 5);
        let mut buf = Buffer::empty(screen);
        let line = LineEdit::new("claude");
        let prompt = Prompt {
            title: " Rename tab ",
            body: Body::Input(&line),
            accent: Color::Indexed(33),
        };
        let cursor = prompt.render(area(screen), &mut buf);
        let shown = rows(&buf);
        assert_eq!(
            shown[1],
            "     ╭───────────── Rename tab ─────────────╮     "
        );
        assert_eq!(shown[2], format!("     │ claude{:31}│     ", ""));
        assert_eq!(shown[3], format!("     ╰{}╯     ", "─".repeat(38)));
        assert_eq!(cursor, Some(Position::new(13, 2)));
        assert_eq!(buf[(5, 1)].fg, Color::Indexed(33));
        assert_eq!(buf[(44, 3)].fg, Color::Indexed(33));
    }

    #[test]
    fn long_input_scrolls_inside_the_box() {
        let screen = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(screen);
        let line = LineEdit::new("abcdefghijkl");
        let prompt = Prompt {
            title: " x ",
            body: Body::Input(&line),
            accent: Color::Blue,
        };
        let cursor = prompt.render(area(screen), &mut buf);
        assert_eq!(rows(&buf)[1], "│ fghijkl  │");
        // Text 8 wide from column 2: 7 letters, then the cursor.
        assert_eq!(cursor, Some(Position::new(2 + 7, 1)));
    }

    #[test]
    fn a_question_shows_without_a_cursor() {
        let screen = Rect::new(0, 0, 40, 3);
        let mut buf = Buffer::empty(screen);
        let prompt = Prompt {
            title: " Close tab ",
            body: Body::Message("Claude is running: close? y/n".into()),
            accent: Color::Blue,
        };
        assert_eq!(prompt.render(area(screen), &mut buf), None);
        assert!(rows(&buf)[1].contains("│ Claude is running: close? y/n"));
        assert!(rows(&buf)[0].contains(" Close tab "));
    }

    #[test]
    fn renders_on_screens_too_small_for_it() {
        for (w, h) in [(0, 0), (1, 1), (3, 3), (4, 2)] {
            let screen = Rect::new(0, 0, w, h);
            let line = LineEdit::new("abc");
            let prompt = Prompt {
                title: " Rename tab ",
                body: Body::Input(&line),
                accent: Color::Blue,
            };
            prompt.render(area(screen), &mut Buffer::empty(screen));
        }
    }
}
