//! The dashboard (`C-a o`): an overlay over the session view listing every
//! session row with its task rows and their git details as a table on top,
//! a live preview of the selected row's terminal below, and the keys at the
//! bottom. While a filter is typed or holds text, it shows on a line under
//! the title. External sessions (Claude outside paddock's terminals) come
//! last, dim, under an `external` line; their preview has no screen.

mod table;

use super::session_view;
use crate::app::{DashboardFilter, Row, RowId, TaskDetails, TaskRow};
use crate::sessions::{State, TaskState};
use crate::terminal::emulator::Emulator;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Clear, Widget};

/// The dashboard's name, bold on the list's first line.
pub const TITLE: &str = "dashboard";
/// The list's only line without rows.
const EMPTY: &str = " no Claude sessions yet";
/// The line over the external sessions.
const EXTERNAL_GROUP: &str = " external";
/// What an external session's preview says: paddock has no terminal of it.
const NO_SCREEN: &str = "external session: no screen";
/// The list's share of the dashboard's height at most, in percent, unless
/// that is below [`MIN_LIST`].
const LIST_SHARE_PERCENT: u32 = 40;
/// The list's height at least (its heads, a few lines and its foot), while
/// the dashboard is that tall.
const MIN_LIST: u16 = 6;
/// The list's title line, its column header and a blank line, above its
/// lines (the filter line, when shown, adds one).
const LIST_HEADS: u16 = 3;
/// What the filter line starts with.
const FILTER_MARK: &str = " / ";
/// The blank line under the list's lines, above the preview.
const LIST_FOOT: u16 = 1;
/// The key line under the preview.
const KEYS_HEIGHT: u16 = 1;
/// The preview's border, on each side.
const BORDER: u16 = 1;
/// The preview's height at least: its border around one line of the
/// terminal. A preview shorter than that is given to the list.
const MIN_PREVIEW: u16 = 2 * BORDER + 1;

/// What the dashboard shows.
pub struct View<'a> {
    pub rows: &'a [Row],
    /// The keys, shown on the dashboard's last line.
    pub keys: &'a str,
    pub details: &'a TaskDetails,
    pub selected: Option<&'a RowId>,
    /// The selected row's terminal, once paddock has it.
    pub preview: Option<&'a Emulator>,
    /// Keys go to the previewed terminal.
    pub interactive: bool,
    /// The filter; its line shows while typed or holding text, with the
    /// cursor while typed.
    pub filter: &'a DashboardFilter,
    /// Seconds since the epoch, for commit ages.
    pub now: u64,
    /// The selected session block's background: the configured current
    /// item's (`[colors] current_space`), as in the sidebar.
    pub selection: Color,
}

/// How a row in `state` stands out: strongly while waiting, softly while idle.
fn style(state: State) -> Style {
    match state {
        State::Waiting => Style::default().fg(Color::Black).bg(Color::Yellow),
        State::Done => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        State::Idle => Style::default().fg(Color::Green),
        State::Running | State::Stopped | State::Unknown => Style::default(),
    }
}

/// How a subagent's state stands out, as a session's would: strongly
/// while waiting, softly once done.
fn task_style(state: TaskState) -> Style {
    match state {
        TaskState::Waiting => style(State::Waiting),
        TaskState::Done => style(State::Idle),
        TaskState::Running | TaskState::Stopped => Style::default(),
    }
}

/// A line of the list.
#[derive(Debug, Clone, Copy)]
pub enum ListLine<'a> {
    /// The blank line between two sessions.
    Blank,
    /// A session row, the start of its block.
    Session(&'a Row),
    /// One of a session row's task rows, in its block; never selected
    /// itself.
    Task(&'a Row, &'a TaskRow),
    /// The line over the external sessions (they come last).
    External,
}

/// The dashboard's lines in order: each session row, then its task rows,
/// with a blank line between two sessions, and the [`ListLine::External`]
/// line before the first external session.
pub fn lines(rows: &[Row]) -> Vec<ListLine<'_>> {
    rows.iter()
        .enumerate()
        .flat_map(|(i, r)| {
            let blank = (i > 0).then_some(ListLine::Blank);
            let first_external = r.external && (i == 0 || !rows[i - 1].external);
            let group = first_external.then_some(ListLine::External);
            let session = std::iter::once(ListLine::Session(r));
            let tasks = r.tasks.iter().map(move |t| ListLine::Task(r, t));
            blank.into_iter().chain(group).chain(session).chain(tasks)
        })
        .collect()
}

/// `style`, dimmed for an external session's lines.
fn dim_if(external: bool, style: Style) -> Style {
    if external {
        style.add_modifier(Modifier::DIM)
    } else {
        style
    }
}

/// `style` on the selection's background `bg`. A background of its own (a
/// waiting row's) becomes its text colour, so the state still shows.
fn on_selection(style: Style, bg: Color) -> Style {
    let fg = style.bg.or(style.fg);
    Style { fg, ..style }.bg(bg)
}

/// Where the dashboard's parts go, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The title, then the rows.
    pub list: Rect,
    /// The selected session's terminal in a border, its title in the top
    /// border.
    pub preview: Rect,
    /// The key line, at the bottom.
    pub keys: Rect,
}

/// The list's heads: one more with the filter line.
fn heads(filter: bool) -> u16 {
    LIST_HEADS + u16::from(filter)
}

/// The dashboard's parts in `area` for a list of `lines` lines (and the
/// filter line if `filter`): the list as tall as its lines (one line
/// saying there are none without any) but at most [`LIST_SHARE_PERCENT`]
/// of the height and at least [`MIN_LIST`], the key line at the bottom,
/// and the preview between them. On a short screen the preview goes
/// first, then the key line.
pub fn layout(area: Rect, lines: usize, filter: bool) -> Layout {
    let keys_height = if area.height > KEYS_HEIGHT {
        KEYS_HEIGHT
    } else {
        0
    };
    let room = area.height - keys_height;
    let share = u32::from(area.height) * LIST_SHARE_PERCENT / 100;
    let cap = u16::try_from(share).unwrap_or(u16::MAX).max(MIN_LIST);
    let wanted = u16::try_from(lines.max(1))
        .unwrap_or(u16::MAX)
        .saturating_add(heads(filter) + LIST_FOOT);
    let list = wanted.max(MIN_LIST).min(cap).min(room);
    // Too short to show any of the terminal: the list keeps that room.
    let list = if room - list < MIN_PREVIEW {
        room
    } else {
        list
    };
    let preview = room - list;
    let at = |y: u16, height: u16| Rect {
        y: area.y + y,
        height,
        ..area
    };
    Layout {
        list: at(0, list),
        preview: at(list, preview),
        keys: at(list + preview, keys_height),
    }
}

/// Draw the dashboard over `area`. Returns where the cursor goes: in the
/// previewed terminal while interactive.
pub fn render(view: &View, area: Rect, buf: &mut Buffer) -> Option<Position> {
    Clear.render(area, buf);
    let parts = layout(area, lines(view.rows).len(), view.filter.shown());
    let filter_cursor = render_list(view, parts.list, buf);
    render_keys(view.keys, parts.keys, buf);
    let preview_cursor = render_preview(view, parts.preview, buf);
    filter_cursor.or(preview_cursor)
}

/// The title, on the list's first line.
fn render_title(area: Rect, buf: &mut Buffer) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let title = format!(" {TITLE}");
    buf.set_stringn(area.x, area.y, &title, usize::from(area.width), bold);
}

/// The keys, dim, on the dashboard's last line.
fn render_keys(keys: &str, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    let text = format!(" {keys}");
    buf.set_stringn(area.x, area.y, text, usize::from(area.width), dim);
}

/// A list line's cells and style: a session row's, or a task row's; none
/// for a blank line.
fn cells(view: &View, line: ListLine) -> Option<(table::Cells, Style)> {
    let (row, t) = match line {
        ListLine::Blank | ListLine::External => return None,
        ListLine::Session(row) => {
            let style = dim_if(row.external, style(row.state));
            return Some((table::session(row), style));
        }
        ListLine::Task(row, t) => (row, t),
    };
    let details = view.details.get(&(t.cwd.clone(), t.branch.clone()));
    let style = t
        .subagent
        .as_ref()
        .map_or(Style::default(), |s| task_style(s.state));
    Some((
        table::task(t, details, view.now),
        dim_if(row.external, style),
    ))
}

/// The filter line under the title: ` / ` and the text typed. Returns the
/// cursor while typing it.
fn render_filter(filter: &DashboardFilter, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let x = area.x + u16::try_from(FILTER_MARK.len()).unwrap_or(u16::MAX);
    let room = area.width.saturating_sub(x - area.x);
    buf.set_stringn(
        area.x,
        area.y,
        FILTER_MARK,
        usize::from(area.width),
        Style::default(),
    );
    let (shown, col) = filter.line.window(room);
    buf.set_stringn(x, area.y, shown, usize::from(room), Style::default());
    (filter.typing && col < room).then(|| Position::new(x + col, area.y))
}

/// The title, the filter line if shown, the column header (dim), then the
/// lines in sight as a table, scrolled to keep the selected session's
/// block in sight, on the selection's background. Returns the cursor while
/// typing the filter.
fn render_list(view: &View, area: Rect, buf: &mut Buffer) -> Option<Position> {
    if area.height == 0 || area.width == 0 {
        return None;
    }
    let width = usize::from(area.width);
    render_title(area, buf);
    let heads = heads(view.filter.shown());
    let mut cursor = None;
    if view.filter.shown() && area.height > 1 {
        let line = Rect {
            y: area.y + 1,
            height: 1,
            ..area
        };
        cursor = render_filter(view.filter, line, buf);
    }
    // The header's row: under the title, or under the filter line.
    let header_y = area.y + heads - 2;
    let lines = lines(view.rows);
    let picked = lines.iter().position(|l| {
        matches!(l, ListLine::Session(row) if view.selected.is_some_and(|s| s.pane == row.pane))
    });
    // The selected session's block: its line and its task rows.
    let block = picked.map_or(0..0, |p| {
        let tasks = lines[p + 1..]
            .iter()
            .take_while(|l| matches!(l, ListLine::Task(..)));
        p..p + 1 + tasks.count()
    });
    let room = usize::from(area.height.saturating_sub(heads + LIST_FOOT));
    let skip = scroll(&lines, &block, room);
    let shown: Vec<(usize, Option<(table::Cells, Style)>)> = lines
        .iter()
        .enumerate()
        .skip(skip)
        .take(room)
        .map(|(i, &l)| (i, cells(view, l)))
        .collect();
    let header = table::header();
    let all: Vec<&table::Cells> = std::iter::once(&header)
        .chain(shown.iter().filter_map(|(_, l)| l.as_ref().map(|(c, _)| c)))
        .collect();
    let widths = table::widths(&all, width);
    if header_y < area.bottom() {
        let dim = Style::default().add_modifier(Modifier::DIM);
        buf.set_string(area.x, header_y, table::line(&header, &widths, width), dim);
    }
    if lines.is_empty() && area.height > heads {
        let y = area.y + heads;
        buf.set_stringn(area.x, y, EMPTY, width, Style::default());
    }
    for ((i, l), y) in shown.iter().zip(area.y + heads..area.bottom()) {
        if matches!(lines[*i], ListLine::External) {
            let dim = Style::default().add_modifier(Modifier::DIM | Modifier::BOLD);
            buf.set_stringn(area.x, y, EXTERNAL_GROUP, width, dim);
            continue;
        }
        let Some((cells, style)) = l else {
            continue;
        };
        let style = if block.contains(i) {
            on_selection(*style, view.selection)
        } else {
            *style
        };
        // `line` is exactly `width` cells: its padding carries the style.
        buf.set_string(area.x, y, table::line(cells, &widths, width), style);
    }
    cursor
}

/// How many of `lines` to scroll past to keep `block` in `room` lines, its
/// session line first if it does not all fit. The top line is always a
/// session's (or the external line), never a blank line or another
/// session's task rows.
fn scroll(lines: &[ListLine], block: &std::ops::Range<usize>, room: usize) -> usize {
    let least = block.end.saturating_sub(room).min(block.start);
    (least..block.start)
        .find(|&i| matches!(lines[i], ListLine::Session(_) | ListLine::External))
        .unwrap_or(block.start)
}

/// The selected row's terminal in a rounded border, its title in the top
/// border; its rows chosen so the last line with text (or the cursor) stays
/// in sight.
fn render_preview(view: &View, area: Rect, buf: &mut Buffer) -> Option<Position> {
    if area.height < MIN_PREVIEW || area.width < 2 * BORDER {
        return None;
    }
    let row = view
        .selected
        .and_then(|s| view.rows.iter().find(|r| r.pane == s.pane));
    let label = row.map(|r| r.label.as_str());
    let plain = Style::default();
    if let Some(r) = row.filter(|r| r.external) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(Span::styled(format!(" {} ", r.label), plain));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height > 0 {
            let dim = plain.add_modifier(Modifier::DIM);
            buf.set_stringn(inner.x, inner.y, NO_SCREEN, usize::from(inner.width), dim);
        }
        return None;
    }
    let (title, title_style) = match (label, view.preview, view.interactive) {
        // No rows: nothing to preview.
        (None, _, _) => (String::new(), plain),
        (Some(label), None, _) => (format!(" {label}: no terminal "), plain),
        (Some(label), Some(_), false) => (format!(" preview: {label}  I to type "), plain),
        (Some(label), Some(_), true) => (
            format!(" typing into {label}  Esc leaves "),
            plain.add_modifier(Modifier::REVERSED),
        ),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(title, title_style));
    let screen = block.inner(area);
    block.render(area, buf);
    let emu = view.preview.filter(|_| label.is_some())?;
    let first = first_row(emu, screen.height);
    session_view::cells(emu, first, screen, buf);
    let (row, col) = emu.cursor();
    let shown = view.interactive
        && emu.modes().show_cursor
        && emu.scroll_offset() == 0
        && (first..first + screen.height).contains(&row)
        && col < screen.width;
    shown.then(|| Position::new(screen.x + col, screen.y + row - first))
}

/// The first of `emu`'s rows to show in `height` rows: the last row with
/// text, or the cursor's if lower, ends up at the bottom.
pub fn first_row(emu: &Emulator, height: u16) -> u16 {
    let (cols, rows) = emu.size();
    let blank = |row: u16| {
        (0..cols).all(|x| {
            let c = emu.cell(row, x);
            c.text.trim().is_empty()
        })
    };
    let last_text = (0..rows).rev().find(|&r| !blank(r)).unwrap_or(0);
    let last = last_text.max(emu.cursor().0);
    (last + 1).saturating_sub(height)
}

#[cfg(test)]
mod tests;
