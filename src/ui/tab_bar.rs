//! The tab bar: one row above the panes, listing the current item's tabs.
//! Tabs are numbered and set apart by thin lines; the current one is drawn
//! reversed and bold, and a tab holding a Claude that waits or is idle gets
//! its colour and a `●`.

use super::sidebar::highlight;
use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// Drawn between two tabs.
const SEPARATOR: &str = "│";
/// Marks a tab holding a Claude that needs you.
pub const MARKER: &str = "●";

/// One tab as the bar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub name: String,
    /// The most urgent Claude state in the tab.
    pub state: State,
}

pub struct TabBar<'a> {
    pub tabs: Vec<Tab>,
    pub current: usize,
    /// While renaming: what is renamed (`tab`, `workspace`) and the text so far.
    pub rename: Option<(&'a str, &'a str)>,
    /// Shown at the right: a message, or the prefix while it is armed.
    pub status: Option<&'a str>,
}

/// Whether a tab in `state` gets the marker.
fn needs_you(state: State) -> bool {
    state.needs_you()
}

/// Tab `index` as drawn in the bar: ` 1:name ` (` 1:name ● ` if it needs you).
fn cell(index: usize, tab: &Tab) -> String {
    let marker = if needs_you(tab.state) {
        format!(" {MARKER}")
    } else {
        String::new()
    };
    format!(" {}:{}{marker} ", index + 1, tab.name)
}

/// The tab drawn at column `x` of the bar (0 = its left edge), if any. A
/// separator counts as the tab to its left.
pub fn tab_at(tabs: &[Tab], x: u16) -> Option<usize> {
    let mut right = 0;
    tabs.iter().enumerate().find_map(|(i, tab)| {
        right += Span::raw(cell(i, tab)).width() + Span::raw(SEPARATOR).width();
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
        match self.rename {
            Some((what, text)) => {
                let prompt = format!(" rename {what}: {text}");
                let (x, _) = buf.set_stringn(
                    row.x,
                    row.y,
                    &prompt,
                    usize::from(row.width),
                    Style::default(),
                );
                cursor = Some(Position::new(x.min(row.right().saturating_sub(1)), row.y));
            }
            None => self.render_tabs(row, buf),
        }
        if let Some(status) = self.status {
            let text = format!(" {status} ");
            let width = u16::try_from(Span::raw(text.as_str()).width()).unwrap_or(u16::MAX);
            // A message too long for the space left covers the tabs, cut at
            // the right edge: an error must never go unseen.
            let x = row.right().saturating_sub(width).max(row.x);
            let style = Style::default().add_modifier(Modifier::REVERSED);
            buf.set_stringn(x, row.y, &text, usize::from(row.right() - x), style);
        }
        cursor
    }

    fn render_tabs(&self, row: Rect, buf: &mut Buffer) {
        let mut x = row.x;
        let dim = Style::default().add_modifier(Modifier::DIM);
        for (i, tab) in self.tabs.iter().enumerate() {
            let mut style = highlight(tab.state);
            if i == self.current {
                style = style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
            }
            for (text, style) in [(cell(i, tab), style), (SEPARATOR.to_string(), dim)] {
                let left = usize::from(row.right().saturating_sub(x));
                if left == 0 {
                    return;
                }
                (x, _) = buf.set_stringn(x, row.y, &text, left, style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

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

    fn tabs(names: &[&str]) -> Vec<Tab> {
        names
            .iter()
            .map(|n| Tab {
                name: n.to_string(),
                state: State::Unknown,
            })
            .collect()
    }

    fn bar(names: &[&str], current: usize) -> TabBar<'static> {
        TabBar {
            tabs: tabs(names),
            current,
            rename: None,
            status: None,
        }
    }

    #[test]
    fn tabs_holding_sessions_that_need_you_stand_out_with_a_marker() {
        let mut b = bar(&["w", "i", "r"], 1);
        for (tab, state) in b
            .tabs
            .iter_mut()
            .zip([State::Waiting, State::Idle, State::Running])
        {
            tab.state = state;
        }
        let (buf, _) = draw(&b, 40);
        assert_eq!(text(&buf), " 1:w ● │ 2:i ● │ 3:r │");
        let (waiting, idle, running) = (&buf[(1, 0)], &buf[(9, 0)], &buf[(17, 0)]);
        assert_eq!((waiting.fg, waiting.bg), (Color::Black, Color::Yellow));
        assert_eq!(idle.fg, Color::Green);
        assert!(idle.modifier.contains(Modifier::REVERSED), "still current");
        assert_eq!((running.fg, running.bg), (Color::Reset, Color::Reset));
    }

    #[test]
    fn numbers_tabs_separates_them_and_highlights_the_current_one() {
        let (buf, cursor) = draw(&bar(&["shell", "logs"], 1), 30);
        assert_eq!(text(&buf), " 1:shell │ 2:logs │");
        assert!(!buf[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(10, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(10, 0)].modifier.contains(Modifier::BOLD));
        assert!(buf[(9, 0)].modifier.contains(Modifier::DIM), "separator");
        assert_eq!(cursor, None);
    }

    #[test]
    fn rename_prompt_replaces_the_tabs_and_places_the_cursor() {
        let b = TabBar {
            rename: Some(("workspace", "new")),
            ..bar(&["shell"], 0)
        };
        let (buf, cursor) = draw(&b, 30);
        assert_eq!(text(&buf), " rename workspace: new");
        assert_eq!(cursor, Some(Position::new(22, 0)));
    }

    #[test]
    fn status_sits_at_the_right_and_long_bars_are_cut() {
        let b = TabBar {
            status: Some("C-a"),
            ..bar(&["a", "b"], 0)
        };
        let (buf, _) = draw(&b, 20);
        assert_eq!(text(&buf), " 1:a │ 2:b │    C-a");
        let (buf, _) = draw(&bar(&["a-very-long-tab-name"], 0), 10);
        assert_eq!(text(&buf), " 1:a-very-");
        let b = TabBar {
            status: Some("cannot read /a/long/path"),
            ..bar(&["a", "b"], 0)
        };
        let (buf, _) = draw(&b, 20);
        assert_eq!(text(&buf), " cannot read /a/long");
    }

    #[test]
    fn clicks_find_the_tab_drawn_under_them() {
        let tabs = tabs(&["shell", "漢字"]);
        // " 1:shell │" is columns 0-9, " 2:漢字 │" 10-18.
        assert_eq!(tab_at(&tabs, 0), Some(0));
        assert_eq!(tab_at(&tabs, 9), Some(0));
        assert_eq!(tab_at(&tabs, 10), Some(1));
        assert_eq!(tab_at(&tabs, 18), Some(1));
        assert_eq!(tab_at(&tabs, 19), None);
    }
}
