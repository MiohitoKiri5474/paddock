//! The tab bar: one row above the panes, listing the current space's tabs
//! (after herdr's look). The row has a darker panel colour; each tab is a
//! filled block with its name centred, at least [`MIN_TAB`] cells wide, one
//! blank column apart. The current tab has the accent colour; a name the
//! user gave is bold; a tab holding a Claude that needs you gets a coloured
//! `●`. A `+` after the last tab opens a new one. When the tabs do not fit,
//! `<` and `>` scroll and `…` marks hidden tabs.
//!
//! Drawing and clicks both go through [`layout`].

use super::sidebar::highlight;
use crate::sessions::State;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Marks a tab holding a Claude that needs you.
pub const MARKER: &str = "●";
/// The narrowest a tab is drawn.
pub const MIN_TAB: u16 = 8;
/// Cells around a tab's name: two on either side.
const PADDING: u16 = 4;
/// The width of the `<`, `>` and `+` buttons.
const BUTTON: u16 = 3;
/// Blank columns between two parts of the bar.
const GAP: u16 = 1;
/// The width of `…`.
const MORE: u16 = 1;

/// One tab as the bar shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub name: String,
    /// The most urgent Claude state in the tab.
    pub state: State,
    /// The user named it (drawn bold).
    pub custom: bool,
}

/// The bar's colours (`[colors]` in config.toml).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabColors {
    /// The row behind the tabs.
    pub bar: Color,
    /// A tab that is not current.
    pub tab: Color,
    /// The current tab.
    pub current: Color,
}

impl Default for TabColors {
    fn default() -> Self {
        Self {
            bar: Color::Indexed(235),
            tab: Color::Indexed(237),
            current: Color::Blue,
        }
    }
}

/// What a part of the bar is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Tab(usize),
    /// `<`: scroll left; enabled if tabs are hidden on the left.
    Left(bool),
    /// `>`: scroll right; enabled if tabs are hidden on the right.
    Right(bool),
    /// `…`: tabs hidden here.
    More,
    /// `+`: a new tab.
    Plus,
}

/// Where each part of the bar goes: `(part, x, width)`, left to right, `x`
/// counted from the bar's left edge.
pub type Layout = Vec<(Part, u16, u16)>;

/// What tab `tab` shows: the marker if it needs you, then its name.
fn label(tab: &Tab) -> String {
    if tab.state.needs_you() {
        format!("{MARKER} {}", tab.name)
    } else {
        tab.name.clone()
    }
}

/// Tab `tab`'s width: its label's plus padding, at least [`MIN_TAB`].
pub fn tab_width(tab: &Tab) -> u16 {
    let text = u16::try_from(Span::raw(label(tab)).width()).unwrap_or(u16::MAX);
    text.saturating_add(PADDING).max(MIN_TAB)
}

/// The bar's parts for `tabs` in `width` cells, showing tabs from `first`
/// on when they do not all fit.
pub fn layout(tabs: &[Tab], width: u16, first: usize) -> Layout {
    let widths: Vec<u16> = tabs.iter().map(tab_width).collect();
    let all: u16 = widths.iter().map(|w| w + GAP).sum::<u16>() + BUTTON;
    let mut out = Layout::new();
    let mut x = 0;
    let place = |out: &mut Layout, part, w: u16, x: &mut u16| {
        out.push((part, *x, w));
        *x += w + GAP;
    };
    if all <= width {
        for (i, w) in widths.iter().enumerate() {
            place(&mut out, Part::Tab(i), *w, &mut x);
        }
        place(&mut out, Part::Plus, BUTTON, &mut x);
        return out;
    }
    let first = first.min(tabs.len().saturating_sub(1));
    place(&mut out, Part::Left(first > 0), BUTTON, &mut x);
    if first > 0 {
        place(&mut out, Part::More, MORE, &mut x);
    }
    // `>` and `+` always fit after the tabs; `…` too if any is left out.
    let tail = BUTTON + GAP + BUTTON;
    let fits = |x: u16, w: u16, more: bool| {
        let end = x + w + GAP + if more { MORE + GAP } else { 0 } + tail;
        end <= width
    };
    let mut last = first;
    for (i, w) in widths.iter().enumerate().skip(first) {
        let more = i + 1 < tabs.len();
        // The first shown tab is drawn even if cut, so the bar is never empty.
        if i > first && !fits(x, *w, more) {
            break;
        }
        place(&mut out, Part::Tab(i), *w, &mut x);
        last = i;
    }
    let hidden_after = last + 1 < tabs.len();
    if hidden_after {
        place(&mut out, Part::More, MORE, &mut x);
    }
    place(&mut out, Part::Right(hidden_after), BUTTON, &mut x);
    place(&mut out, Part::Plus, BUTTON, &mut x);
    out
}

/// Whether tab `i` is shown in `layout`.
fn shown(layout: &Layout, i: usize) -> bool {
    layout.iter().any(|(p, _, _)| *p == Part::Tab(i))
}

/// The first tab to show so that tab `current` is in sight: `first` if it
/// already is, else a `first` that puts `current` about in the middle (or
/// shows as many tabs as fit up to the last one).
pub fn scroll(tabs: &[Tab], width: u16, first: usize, current: usize) -> usize {
    if tabs.is_empty() || shown(&layout(tabs, width, first), current) {
        return first.min(tabs.len().saturating_sub(1));
    }
    let mut first = current.min(tabs.len() - 1);
    while first > 0 {
        let l = layout(tabs, width, first - 1);
        let last = l
            .iter()
            .filter_map(|(p, _, _)| match p {
                Part::Tab(i) => Some(*i),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let (before, after) = (current - (first - 1), last.saturating_sub(current));
        // Centre only while tabs are hidden on both sides.
        let hidden_after = last + 1 < tabs.len();
        if !shown(&l, current) || (before > after && hidden_after) {
            break;
        }
        first -= 1;
    }
    first
}

/// The part of the bar at column `x` (0 = its left edge), if any.
pub fn part_at(layout: &Layout, x: u16) -> Option<Part> {
    layout
        .iter()
        .find(|(_, at, w)| (*at..at + w).contains(&x))
        .map(|(p, _, _)| *p)
}

pub struct TabBar<'a> {
    pub tabs: Vec<Tab>,
    pub current: usize,
    /// The first tab shown when they do not all fit.
    pub first: usize,
    pub colors: TabColors,
    /// While renaming: what is renamed (`tab`, `space`) and the text so far.
    pub rename: Option<(&'a str, &'a str)>,
    /// Shown at the right: a message, or the prefix while it is armed.
    pub status: Option<&'a str>,
}

impl TabBar<'_> {
    /// Draw into the first row of `area`. Returns the cursor position while renaming.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        if area.height == 0 {
            return None;
        }
        let row = Rect { height: 1, ..area };
        let bar = Style::default().bg(self.colors.bar);
        buf.set_style(row, bar);
        let mut cursor = None;
        match self.rename {
            Some((what, text)) => {
                let prompt = format!(" rename {what}: {text}");
                let (x, _) = buf.set_stringn(row.x, row.y, &prompt, usize::from(row.width), bar);
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
        let bar = Style::default().bg(self.colors.bar);
        let dim = bar.add_modifier(Modifier::DIM);
        for (part, at, w) in layout(&self.tabs, row.width, self.first) {
            let x = row.x + at;
            let room = usize::from(row.right().saturating_sub(x).min(w));
            if room == 0 {
                break;
            }
            let (text, style) = match part {
                Part::Tab(i) => {
                    self.draw_tab(
                        i,
                        Rect::new(x, row.y, u16::try_from(room).unwrap_or(w), 1),
                        buf,
                    );
                    continue;
                }
                Part::Left(on) => (" < ", if on { bar } else { dim }),
                Part::Right(on) => (" > ", if on { bar } else { dim }),
                Part::More => ("…", dim),
                Part::Plus => (" + ", bar),
            };
            buf.set_stringn(x, row.y, text, room, style);
        }
    }

    /// Tab `i` as a filled block over `cell`, its label centred.
    fn draw_tab(&self, i: usize, cell: Rect, buf: &mut Buffer) {
        let tab = &self.tabs[i];
        let mut style = if i == self.current {
            Style::default().bg(self.colors.current).fg(Color::Black)
        } else {
            Style::default()
                .bg(self.colors.tab)
                .add_modifier(Modifier::DIM)
        };
        if tab.custom {
            style = style.add_modifier(Modifier::BOLD);
        }
        buf.set_style(cell, style);
        let text = label(tab);
        let width = u16::try_from(Span::raw(text.as_str()).width()).unwrap_or(u16::MAX);
        let x = cell.x + tab_width(tab).saturating_sub(width) / 2;
        let room = usize::from(cell.right().saturating_sub(x));
        if !tab.state.needs_you() {
            buf.set_stringn(x, cell.y, &text, room, style);
            return;
        }
        // The marker in the state's colour, on the tab's background.
        let marker = Style {
            bg: style.bg,
            ..highlight(tab.state)
        };
        let marker = if tab.state == State::Waiting {
            marker.fg(Color::Yellow)
        } else {
            marker
        };
        let (after, _) = buf.set_stringn(x, cell.y, MARKER, room, marker);
        let room = usize::from(cell.right().saturating_sub(after));
        buf.set_stringn(after, cell.y, format!(" {}", tab.name), room, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str) -> Tab {
        Tab {
            name: name.into(),
            state: State::Unknown,
            custom: false,
        }
    }

    fn tabs(names: &[&str]) -> Vec<Tab> {
        names.iter().map(|n| tab(n)).collect()
    }

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

    fn bar(names: &[&str], current: usize) -> TabBar<'static> {
        TabBar {
            tabs: tabs(names),
            current,
            first: 0,
            colors: TabColors::default(),
            rename: None,
            status: None,
        }
    }

    #[test]
    fn tabs_are_padded_blocks_at_least_eight_wide() {
        assert_eq!(tab_width(&tab("a")), 8);
        assert_eq!(tab_width(&tab("claude")), 10);
        assert_eq!(tab_width(&tab("漢字")), 8);
        let marked = Tab {
            state: State::Waiting,
            ..tab("claude")
        };
        assert_eq!(tab_width(&marked), 12, "the marker and a space");
        let l = layout(&tabs(&["a", "claude"]), 40, 0);
        assert_eq!(
            l,
            [
                (Part::Tab(0), 0, 8),
                (Part::Tab(1), 9, 10),
                (Part::Plus, 20, 3)
            ]
        );
    }

    #[test]
    fn names_are_centred_on_filled_blocks_the_current_one_accented() {
        let mut b = bar(&["a", "claude"], 1);
        b.tabs[0].custom = true;
        let (buf, cursor) = draw(&b, 30);
        assert_eq!(text(&buf), "   a       claude    +");
        let c = TabColors::default();
        assert_eq!(buf[(0, 0)].bg, c.tab, "tab filled from its edge");
        assert!(
            buf[(3, 0)].modifier.contains(Modifier::BOLD),
            "named by the user"
        );
        assert!(buf[(3, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(buf[(8, 0)].bg, c.bar, "a gap");
        assert_eq!(
            (buf[(11, 0)].bg, buf[(11, 0)].fg),
            (c.current, Color::Black)
        );
        assert!(!buf[(11, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(29, 0)].bg, c.bar, "the rest of the row");
        assert_eq!(cursor, None);
    }

    #[test]
    fn tabs_holding_sessions_that_need_you_get_a_coloured_marker() {
        let mut b = bar(&["w", "i", "r"], 2);
        for (t, s) in b
            .tabs
            .iter_mut()
            .zip([State::Waiting, State::Idle, State::Running])
        {
            t.state = s;
        }
        let (buf, _) = draw(&b, 40);
        assert_eq!(text(&buf), "  ● w      ● i       r      +");
        assert_eq!(buf[(2, 0)].fg, Color::Yellow);
        assert_eq!(buf[(2, 0)].bg, TabColors::default().tab);
        assert_eq!(buf[(11, 0)].fg, Color::Green);
    }

    #[test]
    fn overflow_shows_arrows_and_ellipses_and_scrolls_to_the_current_tab() {
        let names: Vec<String> = (0..10).map(|i| format!("tab{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let t = tabs(&names);
        // Each tab is 8 wide; 40 cells hold `<`, three tabs, `…`, `>`, `+`.
        let l = layout(&t, 40, 0);
        let parts: Vec<Part> = l.iter().map(|p| p.0).collect();
        assert_eq!(
            parts,
            [
                Part::Left(false),
                Part::Tab(0),
                Part::Tab(1),
                Part::Tab(2),
                Part::More,
                Part::Right(true),
                Part::Plus
            ]
        );
        assert!(l.iter().all(|(_, x, w)| x + w <= 40));
        let l = layout(&t, 40, 4);
        assert_eq!(l[0].0, Part::Left(true));
        assert_eq!(l[1].0, Part::More);
        assert_eq!(l[2].0, Part::Tab(4));
        let end = layout(&t, 40, 9);
        assert!(end.contains(&(Part::Right(false), end[3].1, 3)));
        // In sight: kept. Out of sight: about in the middle.
        assert_eq!(scroll(&t, 40, 0, 2), 0);
        let first = scroll(&t, 40, 0, 7);
        let l = layout(&t, 40, first);
        assert!(shown(&l, 7) && first <= 7, "{l:?}");
        assert_eq!(scroll(&t, 40, 0, 9), 7, "at the end");
        assert_eq!(scroll(&[], 40, 3, 0), 0);
    }

    #[test]
    fn clicks_find_the_part_drawn_under_them() {
        let l = layout(&tabs(&["shell", "漢字"]), 40, 0);
        // `shell` is 0-8, a gap, `漢字` 10-17, a gap, `+` 19-21.
        assert_eq!(part_at(&l, 0), Some(Part::Tab(0)));
        assert_eq!(part_at(&l, 8), Some(Part::Tab(0)));
        assert_eq!(part_at(&l, 9), None, "gap");
        assert_eq!(part_at(&l, 10), Some(Part::Tab(1)));
        assert_eq!(part_at(&l, 20), Some(Part::Plus));
        assert_eq!(part_at(&l, 30), None);
    }

    #[test]
    fn rename_prompt_replaces_the_tabs_and_places_the_cursor() {
        let b = TabBar {
            rename: Some(("space", "new")),
            ..bar(&["shell"], 0)
        };
        let (buf, cursor) = draw(&b, 30);
        assert_eq!(text(&buf), " rename space: new");
        assert_eq!(cursor, Some(Position::new(18, 0)));
    }

    #[test]
    fn status_sits_at_the_right_and_long_messages_cover_the_tabs() {
        let b = TabBar {
            status: Some("C-a"),
            ..bar(&["a", "b"], 0)
        };
        let (buf, _) = draw(&b, 30);
        assert_eq!(text(&buf), "   a        b      +      C-a");
        let b = TabBar {
            status: Some("cannot read /a/long/path"),
            ..bar(&["a", "b"], 0)
        };
        let (buf, _) = draw(&b, 20);
        assert_eq!(text(&buf), " cannot read /a/long");
    }
}
