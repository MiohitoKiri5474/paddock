//! The prompt popup (after LazyVim's input): a small rounded box in the
//! middle of the screen, drawn over everything, its border in the accent
//! colour and its title in the top border. It holds a line being typed
//! (a name), a question, or a list to pick from.
//!
//! ```text
//! ╭──────── Rename tab ────────╮
//! │ claude█                    │
//! ╰────────────────────────────╯
//! ```

use crate::input::line::LineEdit;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Widget};

/// The box's width on screens wide enough.
const WIDTH: u16 = 40;
/// The width of a box holding a list (paths need the room).
const LIST_WIDTH: u16 = 60;
/// The widest a question's box gets; a longer question wraps.
const MESSAGE_WIDTH: u16 = 64;
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
    /// Lines to pick one of, `selected` reversed.
    List { items: Vec<String>, selected: usize },
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

/// Where a box of one line goes on a screen `screen`: centred, [`WIDTH`]
/// wide or as wide as the screen if narrower.
pub fn area(screen: Rect) -> Rect {
    sized(screen, WIDTH, 1)
}

/// `text` in lines of at most `width` characters, broken between words
/// (a word too long for a line is cut); at least one line.
fn wrap(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let line = lines.last_mut().expect("never empty");
            let used = line.chars().count();
            let gap = usize::from(used > 0);
            if used + gap + word.len() <= width {
                if gap == 1 {
                    line.push(' ');
                }
                line.extend(word);
                break;
            }
            if used > 0 {
                lines.push(String::new());
                continue;
            }
            let rest = word.split_off(width);
            line.extend(word);
            word = rest;
            lines.push(String::new());
        }
    }
    lines
}

/// Where a box `width` wide holding `lines` lines goes on a screen
/// `screen`: centred, cut to the screen.
fn sized(screen: Rect, width: u16, lines: u16) -> Rect {
    let width = width.min(screen.width);
    let height = (HEIGHT - 1).saturating_add(lines).min(screen.height);
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
    /// Where this box goes on `screen`: one line [`WIDTH`] wide for input,
    /// as wide as its question needs for a message, and [`LIST_WIDTH`]
    /// with a line per item for a list.
    pub fn area(&self, screen: Rect) -> Rect {
        match &self.body {
            Body::Input(_) => area(screen),
            Body::Message(m) => {
                let needed = u16::try_from(m.chars().count()).unwrap_or(u16::MAX);
                let width = needed
                    .saturating_add(2 * PAD + 2)
                    .clamp(WIDTH, MESSAGE_WIDTH)
                    .min(screen.width);
                let inner = width.saturating_sub(2 * PAD + 2);
                let lines = u16::try_from(wrap(m, inner).len()).unwrap_or(u16::MAX);
                sized(screen, width, lines)
            }
            Body::List { items, .. } => {
                let lines = u16::try_from(items.len().max(1)).unwrap_or(u16::MAX);
                sized(screen, LIST_WIDTH, lines)
            }
        }
    }

    /// Draw the box at `popup` (see [`Self::area`]). Returns the cursor
    /// while typing.
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
                for (line, y) in wrap(m, text.width).iter().zip(text.y..text.bottom()) {
                    buf.set_stringn(text.x, y, line, width, Style::default());
                }
                None
            }
            Body::Input(line) => {
                let (shown, col) = line.window(text.width);
                buf.set_stringn(text.x, text.y, shown, width, Style::default());
                Some(Position::new(text.x + col, text.y))
            }
            Body::List { items, selected } => {
                for (item, y) in items.iter().zip(text.y..text.bottom()) {
                    let row = Rect::new(text.x, y, text.width, 1);
                    let style = if items.get(*selected) == Some(item) {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    buf.set_style(row, style);
                    buf.set_stringn(text.x, y, item, width, style);
                }
                None
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
    fn a_list_shows_a_line_per_item_with_the_selected_one_reversed() {
        let screen = Rect::new(0, 0, 70, 8);
        let mut buf = Buffer::empty(screen);
        let prompt = Prompt {
            title: " Open worktree ",
            body: Body::List {
                items: vec!["api/feat/x  ~/w/feat-x".into(), "api  ~/api".into()],
                selected: 1,
            },
            accent: Color::Blue,
        };
        let popup = prompt.area(screen);
        assert_eq!(popup, Rect::new(5, 2, 60, 4));
        assert_eq!(prompt.render(popup, &mut buf), None);
        let shown = rows(&buf);
        assert!(
            shown[3].contains("│ api/feat/x  ~/w/feat-x"),
            "{}",
            shown[3]
        );
        assert!(shown[4].contains("│ api  ~/api"), "{}", shown[4]);
        assert!(!buf[(7, 3)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(7, 4)].modifier.contains(Modifier::REVERSED));
        assert!(
            buf[(60, 4)].modifier.contains(Modifier::REVERSED),
            "whole line"
        );
        // A long question widens its box; an empty list still has a line.
        let long = Prompt {
            title: " x ",
            body: Body::Message("a".repeat(50)),
            accent: Color::Blue,
        };
        assert_eq!(long.area(screen).width, 54);
        // Longer still, it wraps between words in a box at most 64 wide.
        let longer = Prompt {
            title: " x ",
            body: Body::Message(format!("{} {}: y/n", "b".repeat(40), "c".repeat(30))),
            accent: Color::Blue,
        };
        let popup = longer.area(screen);
        assert_eq!((popup.width, popup.height), (64, 4));
        let mut buf = Buffer::empty(screen);
        longer.render(popup, &mut buf);
        let shown = rows(&buf);
        assert!(
            shown[3].contains(&format!("│ {} ", "b".repeat(40))),
            "{shown:#?}"
        );
        assert!(shown[4].contains(&format!("│ {}: y/n", "c".repeat(30))));
        assert_eq!(wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(wrap("", 4), [""]);
        assert_eq!(wrap("ab cd", 5), ["ab cd"]);
        let empty = Prompt {
            title: " x ",
            body: Body::List {
                items: vec![],
                selected: 0,
            },
            accent: Color::Blue,
        };
        assert_eq!(empty.area(screen).height, 3);
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
