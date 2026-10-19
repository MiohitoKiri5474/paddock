//! The tab bar: one row above the terminals.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

pub struct TabBar<'a> {
    pub names: Vec<&'a str>,
    pub current: usize,
    /// Text being typed while renaming the current tab.
    pub rename: Option<&'a str>,
    /// Shown at the right: a message, or the prefix while it is armed.
    pub status: Option<&'a str>,
}

/// Label of tab `index` (0-based) as drawn: `1:name`.
pub fn label(index: usize, name: &str) -> String {
    format!("{}:{name}", index + 1)
}

/// Tab `index` as drawn in the bar, with a space on either side.
fn cell(index: usize, name: &str) -> String {
    format!(" {} ", label(index, name))
}

/// The tab drawn at column `x` of the bar (0 = its left edge), if any.
pub fn tab_at(names: &[&str], x: u16) -> Option<usize> {
    let mut right = 0;
    names.iter().enumerate().find_map(|(i, name)| {
        right += Span::raw(cell(i, name)).width();
        (usize::from(x) < right).then_some(i)
    })
}

impl TabBar<'_> {
    /// Draw into the first row of `area`. Returns the cursor position while renaming.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        if area.height == 0 {
            return None;
        }
        let row = Rect { height: 1, ..area };
        buf.set_style(row, Style::default());
        let mut cursor = None;
        let used = match self.rename {
            Some(text) => {
                let prompt = format!(" rename tab: {text}");
                let (x, _) = buf.set_stringn(
                    row.x,
                    row.y,
                    &prompt,
                    usize::from(row.width),
                    Style::default(),
                );
                cursor = Some(Position::new(x.min(row.right().saturating_sub(1)), row.y));
                x
            }
            None => self.render_tabs(row, buf),
        };
        if let Some(status) = self.status {
            let text = format!(" {status} ");
            let width = u16::try_from(Span::raw(text.as_str()).width()).unwrap_or(u16::MAX);
            if used + width <= row.right() {
                let style = Style::default().add_modifier(Modifier::REVERSED);
                buf.set_string(row.right() - width, row.y, &text, style);
            }
        }
        cursor
    }

    fn render_tabs(&self, row: Rect, buf: &mut Buffer) -> u16 {
        let mut x = row.x;
        for (i, name) in self.names.iter().enumerate() {
            let text = cell(i, name);
            let style = if i == self.current {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
            };
            let left = usize::from(row.right().saturating_sub(x));
            if left == 0 {
                break;
            }
            (x, _) = buf.set_stringn(x, row.y, &text, left, style);
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(bar: &TabBar, width: u16) -> (Buffer, Option<Position>) {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        let cursor = bar.render(area, &mut buf);
        (buf, cursor)
    }

    fn text(buf: &Buffer) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn bar<'a>(names: Vec<&'a str>, current: usize) -> TabBar<'a> {
        TabBar {
            names,
            current,
            rename: None,
            status: None,
        }
    }

    #[test]
    fn numbers_tabs_and_highlights_the_current_one() {
        let (buf, cursor) = draw(&bar(vec!["shell", "logs"], 1), 30);
        assert_eq!(text(&buf), " 1:shell  2:logs");
        assert!(!buf[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(9, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(cursor, None);
    }

    #[test]
    fn rename_prompt_replaces_the_tabs_and_places_the_cursor() {
        let b = TabBar {
            rename: Some("new"),
            ..bar(vec!["shell"], 0)
        };
        let (buf, cursor) = draw(&b, 30);
        assert_eq!(text(&buf), " rename tab: new");
        assert_eq!(cursor, Some(Position::new(16, 0)));
    }

    #[test]
    fn status_sits_at_the_right_and_long_bars_are_cut() {
        let b = TabBar {
            status: Some("C-a"),
            ..bar(vec!["a", "b"], 0)
        };
        let (buf, _) = draw(&b, 20);
        assert_eq!(text(&buf), " 1:a  2:b       C-a");
        let (buf, _) = draw(&bar(vec!["a-very-long-tab-name"], 0), 10);
        assert_eq!(text(&buf), " 1:a-very-");
    }

    #[test]
    fn clicks_find_the_tab_drawn_under_them() {
        let names = ["shell", "漢字"];
        // " 1:shell " is columns 0-8, " 2:漢字 " 9-16.
        assert_eq!(tab_at(&names, 0), Some(0));
        assert_eq!(tab_at(&names, 8), Some(0));
        assert_eq!(tab_at(&names, 9), Some(1));
        assert_eq!(tab_at(&names, 16), Some(1));
        assert_eq!(tab_at(&names, 17), None);
    }
}
