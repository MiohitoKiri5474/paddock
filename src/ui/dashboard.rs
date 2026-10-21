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
mod tests {
    use super::*;

    const KEYS: &str = "j down  k up";
    /// The selection's background in these tests.
    const SELECTED: Color = Color::Indexed(236);
    use crate::git::{BaseDiff, Details, Status};
    use crate::sessions::SubagentTask;

    fn row(label: &str, state: State, tasks: Vec<TaskRow>) -> Row {
        Row {
            pane: format!("%{label}"),
            space: "$1".into(),
            window: "@1".into(),
            label: label.into(),
            state,
            tasks,
            external: false,
        }
    }

    #[test]
    fn external_sessions_come_dim_under_their_line_with_no_screen() {
        let area = Rect::new(0, 0, 50, 30);
        let mut buf = Buffer::empty(area);
        let ext = Row {
            pane: "ext:e1".into(),
            space: String::new(),
            window: String::new(),
            external: true,
            ..row("web", State::Waiting, vec![task("feat/x", None)])
        };
        let rows = [row("api", State::Idle, vec![]), ext];
        let selected = RowId {
            pane: "ext:e1".into(),
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        let screen: Vec<String> = (0..30).map(|y| text(&buf, y)).collect();
        let at = |s: &str| screen.iter().position(|l| l.starts_with(s));
        let group = at(" external").unwrap_or_else(|| panic!("{screen:#?}"));
        assert!(at(" api").unwrap() < group, "{screen:#?}");
        let web = at(" web").expect("web");
        assert_eq!(web, group + 1);
        assert!(screen[web + 1].contains("feat/x"));
        let y = u16::try_from(web).unwrap();
        assert!(buf[(1, y)].modifier.contains(Modifier::DIM), "dim");
        let api = u16::try_from(at(" api").unwrap()).unwrap();
        assert!(!buf[(1, api)].modifier.contains(Modifier::DIM));
        let preview = at("╭ web ─").expect("preview titled web");
        assert!(
            screen[preview + 1].contains(NO_SCREEN),
            "{}",
            screen[preview + 1]
        );
    }

    fn task(branch: &str, status: Option<Status>) -> TaskRow {
        TaskRow {
            branch: branch.into(),
            cwd: "/r".into(),
            status,
            subagent: None,
        }
    }

    fn text(buf: &Buffer, y: u16) -> String {
        (buf.area.x..buf.area.right())
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn lines_follow_rows_each_with_its_tasks_and_a_blank_line_between() {
        let rows = [
            row("w", State::Waiting, vec![task("a", None), task("b", None)]),
            row("i", State::Idle, vec![]),
            row("r", State::Running, vec![]),
        ];
        let got: Vec<String> = lines(&rows)
            .into_iter()
            .map(|l| match l {
                ListLine::Blank => String::new(),
                ListLine::Session(r) => r.label.clone(),
                ListLine::Task(_, t) => format!("  {}", t.branch),
                ListLine::External => "external".into(),
            })
            .collect();
        assert_eq!(got, ["w", "  a", "  b", "", "i", "", "r"]);
    }

    /// A layout of `list`, `preview` and `keys` rows stacked from row 1 of
    /// an area 80 columns wide.
    fn stacked(list: u16, preview: u16, keys: u16) -> Layout {
        Layout {
            list: Rect::new(0, 1, 80, list),
            preview: Rect::new(0, 1 + list, 80, preview),
            keys: Rect::new(0, 1 + list + preview, 80, keys),
        }
    }

    #[test]
    fn the_list_sits_on_top_sized_to_its_lines_with_the_keys_at_the_bottom() {
        let area = |height| Rect::new(0, 1, 80, height);
        assert_eq!(
            layout(area(40), 7, false),
            stacked(11, 28, 1),
            "the title, the header, seven lines and blank lines around them"
        );
        assert_eq!(
            layout(area(40), 50, false),
            stacked(16, 23, 1),
            "capped at 40%"
        );
        assert_eq!(
            layout(area(40), 1, false),
            stacked(6, 33, 1),
            "a few rows at least"
        );
        assert_eq!(
            layout(area(40), 0, false),
            stacked(6, 33, 1),
            "the line saying there are none"
        );
    }

    #[test]
    fn the_filter_line_takes_a_row_under_the_title() {
        let area = Rect::new(0, 1, 80, 40);
        assert_eq!(layout(area, 7, true), stacked(12, 27, 1));
        let mut buf = Buffer::empty(area);
        let rows = [row("api", State::Idle, vec![])];
        let filter = DashboardFilter {
            line: crate::input::line::LineEdit::new("ap"),
            typing: true,
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: None,
            preview: None,
            interactive: false,
            filter: &filter,
            now: 0,
            selection: SELECTED,
        };
        let cursor = render(&view, area, &mut buf);
        assert_eq!(text(&buf, 1), " dashboard");
        assert_eq!(text(&buf, 2), " / ap");
        assert!(text(&buf, 3).starts_with(" NAME"), "the header moved down");
        assert_eq!(text(&buf, 4), "");
        assert!(text(&buf, 5).starts_with(" api"));
        assert_eq!(cursor, Some(Position::new(5, 2)), "typing the filter");
        let kept_filter = DashboardFilter {
            typing: false,
            ..filter.clone()
        };
        let kept = View {
            filter: &kept_filter,
            ..view
        };
        assert_eq!(render(&kept, area, &mut buf), None, "the filter kept");
    }

    #[test]
    fn a_short_dashboard_loses_its_preview_before_its_list() {
        let area = |height| Rect::new(0, 1, 80, height);
        assert_eq!(
            layout(area(10), 9, false),
            stacked(6, 3, 1),
            "the minimum wins"
        );
        assert_eq!(
            layout(area(8), 9, false),
            stacked(7, 0, 1),
            "no room for a bordered terminal line: the list keeps it"
        );
        assert_eq!(layout(area(3), 9, false), stacked(2, 0, 1));
        assert_eq!(
            layout(area(1), 9, false),
            stacked(1, 0, 0),
            "only the title"
        );
        assert_eq!(layout(area(0), 9, false), stacked(0, 0, 0));
    }

    #[test]
    fn the_preview_keeps_the_last_text_in_sight() {
        let mut e = Emulator::new(20, 10, 0);
        e.feed(b"one\r\ntwo\r\n");
        assert_eq!(first_row(&e, 4), 0, "fits: from the top");
        for i in 0..7 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        // Text on rows 0..=8, the cursor on row 9.
        assert_eq!(first_row(&e, 4), 6);
        let mut claude = Emulator::new(20, 10, 0);
        claude.feed(b"\x1b[3;1Hprompt\x1b[10;1Hstatus\x1b[3;7H");
        assert_eq!(first_row(&claude, 4), 6, "text below the cursor");
    }

    #[test]
    fn renders_rows_tasks_and_a_live_preview() {
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        let status = Status {
            ahead_behind: None,
            changes: Some((3, 1)),
        };
        let mut sub = task("fix/b", None);
        sub.subagent = Some(SubagentTask {
            agent_id: "a1".into(),
            description: Some("Fix b".into()),
            state: TaskState::Waiting,
        });
        let rows = [row(
            "api",
            State::Idle,
            vec![task("feat/a", Some(status)), sub],
        )];
        let details = TaskDetails::from([(
            ("/r".to_string(), "feat/a".to_string()),
            Details {
                base_diff: Some(BaseDiff {
                    added: 5,
                    removed: 0,
                    files: 2,
                }),
                last_commit: None,
                merged: true,
            },
        )]);
        let mut emu = Emulator::new(20, 4, 0);
        emu.feed(b"hello preview");
        let selected = RowId {
            pane: "%api".into(),
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &details,
            selected: Some(&selected),
            preview: Some(&emu),
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        assert_eq!(render(&view, area, &mut buf), None);
        assert_eq!(text(&buf, 0), " dashboard");
        assert_eq!(
            [1, 2, 3, 4, 5, 6].map(|y| text(&buf, y)),
            [
                " NAME             STATE    ↑↓  CHANGES   BASE  FILES  AGE  MERGED",
                "",
                " api              idle",
                "   feat/a                        +3 −1  +5 −0      2       ✓",
                "   Fix b · fix/b  waiting",
                "",
            ]
        );
        assert!(buf[(1, 1)].modifier.contains(Modifier::DIM), "a dim header");
        // The selected session's block, its task rows included, has the
        // selection's background; a waiting row keeps its yellow as text.
        assert_eq!([3, 4, 5].map(|y| buf[(3, y)].bg), [SELECTED; 3]);
        assert_eq!(buf[(3, 3)].fg, Color::Green, "idle keeps its colour");
        assert_eq!(buf[(3, 5)].fg, Color::Yellow, "waiting keeps its colour");
        assert_ne!(buf[(1, 1)].bg, SELECTED, "not the header");
        // The preview in a rounded border, its title in the top border.
        assert!(text(&buf, 7).starts_with("╭ preview: api  I to type ──"));
        assert!(text(&buf, 7).ends_with("─╮"));
        assert_eq!(
            text(&buf, 8),
            "│hello preview".to_string() + &" ".repeat(65) + "│"
        );
        assert!(text(&buf, 18).starts_with("╰──"));
        assert_eq!(text(&buf, 19), " j down  k up", "the keys at the bottom");
        assert!(buf[(1, 19)].modifier.contains(Modifier::DIM));

        let typing = View {
            interactive: true,
            ..view
        };
        let cursor = render(&typing, area, &mut Buffer::empty(area));
        assert_eq!(cursor, Some(Position::new(14, 8)), "inside the border");
    }

    #[test]
    fn only_the_selected_sessions_block_is_highlighted() {
        let area = Rect::new(0, 0, 30, 30);
        let mut buf = Buffer::empty(area);
        let rows = [
            row("r0", State::Running, vec![task("b1", None), subagent("s1")]),
            row("r1", State::Running, vec![]),
        ];
        let selected = RowId { pane: "%r0".into() };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        assert!(text(&buf, 3).starts_with(" r0"));
        assert!(text(&buf, 7).starts_with(" r1"));
        let bg = |y: u16| buf[(1, y)].bg == SELECTED;
        assert_eq!([3, 4, 5, 6, 7].map(bg), [true, true, true, false, false]);
    }

    #[test]
    fn a_rows_preview_without_its_terminal_says_so() {
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        let rows = [row("gone", State::Idle, vec![])];
        let selected = RowId {
            pane: "%gone".into(),
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        assert!(text(&buf, 6).starts_with("╭ gone: no terminal ─"));
    }

    #[test]
    fn without_rows_it_says_there_are_no_sessions() {
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        let view = View {
            keys: KEYS,
            rows: &[],
            details: &TaskDetails::new(),
            selected: None,
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        assert_eq!(text(&buf, 2), "");
        assert_eq!(text(&buf, 3), " no Claude sessions yet");
        assert_eq!(text(&buf, 6), format!("╭{}╮", "─".repeat(38)));
        assert_eq!(text(&buf, 11), " j down  k up");
    }

    #[test]
    fn the_selected_line_scrolls_into_sight() {
        let area = Rect::new(0, 0, 30, 20);
        let mut buf = Buffer::empty(area);
        let rows: Vec<Row> = (0..6)
            .map(|i| row(&format!("r{i}"), State::Running, vec![]))
            .collect();
        let selected = RowId { pane: "%r5".into() };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        // The list is capped at eight rows: four lines between its heads and
        // its foot, a blank one between two sessions. A session line is on
        // top, not the blank line above it.
        assert!(text(&buf, 3).starts_with(" r4"));
        assert_eq!(text(&buf, 4), "");
        assert!(text(&buf, 5).starts_with(" r5"));
        assert_eq!(text(&buf, 6), "");
        assert_eq!(text(&buf, 7), "", "a blank line above the preview");
        assert!(text(&buf, 8).starts_with("╭ r5: no terminal"));
    }

    /// A subagent's task row on `branch`.
    fn subagent(branch: &str) -> TaskRow {
        TaskRow {
            subagent: Some(SubagentTask {
                agent_id: branch.into(),
                description: None,
                state: TaskState::Running,
            }),
            ..task(branch, None)
        }
    }

    #[test]
    fn the_selected_line_scrolls_into_sight_with_its_subagents() {
        let area = Rect::new(0, 0, 30, 20);
        let mut buf = Buffer::empty(area);
        let rows = [
            row("r0", State::Running, vec![]),
            row("r1", State::Running, vec![]),
            row("r2", State::Running, vec![subagent("s1"), subagent("s2")]),
        ];
        let selected = RowId { pane: "%r2".into() };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        assert!(text(&buf, 3).starts_with(" r2"));
        assert!(text(&buf, 4).starts_with("   s1"));
        assert!(text(&buf, 5).starts_with("   s2"));
        // The whole block is highlighted, subagents included.
        assert_eq!([3, 4, 5].map(|y| buf[(1, y)].bg), [SELECTED; 3]);
    }

    #[test]
    fn the_list_never_starts_with_another_sessions_task_rows() {
        let area = Rect::new(0, 0, 30, 20);
        let mut buf = Buffer::empty(area);
        let rows = [
            row(
                "r0",
                State::Running,
                vec![subagent("a"), subagent("b"), subagent("c")],
            ),
            row("r1", State::Running, vec![]),
        ];
        let selected = RowId { pane: "%r1".into() };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            filter: &DashboardFilter::default(),
            now: 0,
            selection: SELECTED,
        };
        render(&view, area, &mut buf);
        // Four lines of room: r0's block does not fit whole with r1's, so
        // r1 is on top rather than r0's last task rows.
        assert!(text(&buf, 3).starts_with(" r1"));
        assert_eq!([4, 5, 6].map(|y| text(&buf, y)), ["", "", ""]);
    }
}
