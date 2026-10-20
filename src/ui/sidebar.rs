//! The sidebar: a title, then one entry per item (tmux session, ADR 0006),
//! with a border on its right edge. Each item takes two lines and a blank
//! one after it:
//!
//! ```text
//!  api-server
//!    ● waiting · 3 tabs
//! ```
//!
//! The second line is dim and shows the most urgent Claude state in the
//! item (none without Claude) and its number of tabs. The current item has
//! its own background; the selected one, while the sidebar has the keyboard,
//! is drawn reversed.

use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

const TITLE: &str = "workspaces";
/// Rows above the first item (the title).
const FIRST_ROW: u16 = 1;
/// Rows each item takes: its two lines and a blank one.
const ITEM_ROWS: u16 = 3;
/// Marks the Claude state on an item's second line.
const MARKER: &str = "●";

/// One sidebar item as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    pub name: String,
    /// The most urgent Claude state among its panes; `None` without Claude.
    pub state: Option<State>,
    pub tabs: usize,
    /// Its tabs are the ones shown.
    pub current: bool,
}

/// How an item's state (or a tab holding it) in `state` stands out, in the
/// terminal's own ANSI colours: strongly while waiting, softly while idle.
pub fn highlight(state: State) -> Style {
    match state {
        State::Waiting => Style::default().fg(Color::Black).bg(Color::Yellow),
        State::Idle => Style::default().fg(Color::Green),
        State::Running | State::Stopped | State::Unknown => Style::default(),
    }
}

/// An item's second line, after its indent: `● waiting · 3 tabs`, as the
/// state part and the rest.
fn status(item: &SidebarItem) -> (Option<String>, String) {
    let tabs = match item.tabs {
        1 => "1 tab".to_string(),
        n => format!("{n} tabs"),
    };
    match item.state {
        Some(s) => (
            Some(format!("{MARKER} {}", s.label())),
            format!(" · {tabs}"),
        ),
        None => (None, tabs),
    }
}

/// Draw `items` into `area`. `selected` is the item the keyboard is on while
/// the sidebar has it; `current_bg` the current item's background.
pub fn render(
    items: &[SidebarItem],
    selected: Option<usize>,
    current_bg: Color,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.width < 2 || area.height == 0 {
        return;
    }
    let inner = area.width - 1;
    for y in area.top()..area.bottom() {
        buf.set_string(area.right() - 1, y, "│", Style::default());
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    buf.set_stringn(area.x + 1, area.y, TITLE, usize::from(inner - 1), dim);
    for (i, item) in items.iter().enumerate() {
        let Some(top) = u16::try_from(i)
            .ok()
            .and_then(|i| i.checked_mul(ITEM_ROWS))
            .and_then(|o| (area.y + FIRST_ROW).checked_add(o))
        else {
            break;
        };
        let mut base = Style::default();
        if item.current {
            base = base.bg(current_bg);
        }
        if selected == Some(i) {
            base = base.add_modifier(Modifier::REVERSED);
        }
        for (line, y) in (0..2).zip(top..) {
            if y >= area.bottom() {
                break;
            }
            let row = Rect::new(area.x, y, inner, 1);
            buf.set_style(row, base);
            let width = usize::from(inner);
            if line == 0 {
                buf.set_stringn(area.x + 1, y, &item.name, width.saturating_sub(2), base);
                continue;
            }
            let (state, rest) = status(item);
            let mut x = area.x + 3;
            let end = area.x + inner.saturating_sub(1);
            if let (Some(text), Some(s)) = (state, item.state) {
                let style = match s {
                    State::Waiting | State::Idle => base.patch(highlight(s)),
                    _ => base.patch(dim),
                };
                let left = usize::from(end.saturating_sub(x));
                (x, _) = buf.set_stringn(x, y, &text, left, style);
            }
            let left = usize::from(end.saturating_sub(x));
            buf.set_stringn(x, y, &rest, left, base.patch(dim));
        }
    }
}

/// The item drawn at screen row `y` of a sidebar over `area`, if any; the
/// blank line after an item counts as none.
pub fn item_at(area: Rect, y: u16) -> Option<usize> {
    let first = area.y + FIRST_ROW;
    if !(first..area.bottom()).contains(&y) {
        return None;
    }
    let offset = y - first;
    (offset % ITEM_ROWS < 2).then_some(usize::from(offset / ITEM_ROWS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, state: Option<State>, tabs: usize) -> SidebarItem {
        SidebarItem {
            name: name.into(),
            state,
            tabs,
            current: false,
        }
    }

    fn text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    const GREY: Color = Color::Indexed(236);

    #[test]
    fn items_show_name_then_state_and_tabs_with_a_blank_line_between() {
        let area = Rect::new(0, 0, 24, 8);
        let mut buf = Buffer::empty(area);
        let items = [
            item("api-server", Some(State::Waiting), 3),
            item("notes", None, 1),
            item("third", Some(State::Running), 2),
        ];
        render(&items, None, GREY, area, &mut buf);
        assert_eq!(text(&buf, 0), " workspaces            │");
        assert_eq!(text(&buf, 1), " api-server            │");
        assert_eq!(text(&buf, 2), "   ● waiting · 3 tabs  │");
        assert_eq!(text(&buf, 3), "                       │");
        assert_eq!(text(&buf, 4), " notes                 │");
        assert_eq!(text(&buf, 5), "   1 tab               │");
        assert_eq!(text(&buf, 7), " third                 │");
        assert!(buf[(5, 5)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn waiting_and_idle_states_keep_their_highlight() {
        let area = Rect::new(0, 0, 24, 6);
        let mut buf = Buffer::empty(area);
        let items = [
            item("w", Some(State::Waiting), 1),
            item("i", Some(State::Idle), 1),
        ];
        render(&items, None, GREY, area, &mut buf);
        let (waiting, idle) = (&buf[(3, 2)], &buf[(3, 5)]);
        assert_eq!((waiting.fg, waiting.bg), (Color::Black, Color::Yellow));
        assert_eq!(idle.fg, Color::Green);
        assert!(buf[(13, 2)].modifier.contains(Modifier::DIM), "tab count");
    }

    #[test]
    fn the_current_item_has_its_own_background_and_the_selected_one_is_reversed() {
        let area = Rect::new(0, 0, 24, 6);
        let mut buf = Buffer::empty(area);
        let mut current = item("cur", None, 1);
        current.current = true;
        render(
            &[current, item("other", None, 2)],
            Some(1),
            GREY,
            area,
            &mut buf,
        );
        for (x, y) in [(0, 1), (10, 1), (0, 2), (22, 2)] {
            assert_eq!(buf[(x, y)].bg, GREY, "({x}, {y})");
        }
        assert_eq!(buf[(23, 1)].bg, Color::Reset, "not the border");
        assert_eq!(buf[(0, 3)].bg, Color::Reset, "not the blank line");
        assert!(buf[(1, 4)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(20, 5)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(1, 1)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn control_characters_in_names_never_reach_the_screen() {
        // Names come from tmux; an escape sequence in one must not be
        // written to the outer terminal.
        let area = Rect::new(0, 0, 30, 3);
        let mut buf = Buffer::empty(area);
        render(
            &[item("a\x1b]52;c;eA==\x07b", None, 1)],
            None,
            GREY,
            area,
            &mut buf,
        );
        let drawn = text(&buf, 1);
        assert!(!drawn.contains(char::is_control), "{drawn:?}");
    }

    #[test]
    fn long_names_are_cut_and_items_stop_at_the_bottom() {
        let area = Rect::new(0, 0, 12, 3);
        let mut buf = Buffer::empty(area);
        let items = [
            item("a-very-long-name", Some(State::Running), 12),
            item("hidden", None, 1),
        ];
        render(&items, None, GREY, area, &mut buf);
        assert_eq!(text(&buf, 1), " a-very-lo │");
        assert_eq!(text(&buf, 2), "   ● runni │");
    }

    #[test]
    fn clicks_find_the_item_drawn_under_them() {
        let area = Rect::new(0, 3, 24, 10);
        assert_eq!(item_at(area, 3), None, "title");
        assert_eq!(item_at(area, 4), Some(0));
        assert_eq!(item_at(area, 5), Some(0));
        assert_eq!(item_at(area, 6), None, "blank line");
        assert_eq!(item_at(area, 7), Some(1));
        assert_eq!(item_at(area, 10), Some(2));
        assert_eq!(item_at(area, 12), None);
        assert_eq!(item_at(area, 13), None, "below the sidebar");
    }
}
