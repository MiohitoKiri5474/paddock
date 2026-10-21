//! The dashboard (`C-a o`): an overlay over the session view listing every
//! session row with its task rows and their git details as a table on top,
//! a live preview of the selected row's terminal below, and the keys at the
//! bottom.

mod table;

use super::session_view;
use crate::app::{Row, RowId, TaskDetails, TaskRow};
use crate::sessions::{State, TaskState};
use crate::terminal::emulator::Emulator;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, Widget};

pub const TITLE: &str = "dashboard";
/// The list's only line without rows.
const EMPTY: &str = " no Claude sessions yet";
/// The list's share of the dashboard's height at most, in percent, unless
/// that is below [`MIN_LIST`].
const LIST_SHARE_PERCENT: u32 = 40;
/// The list's height at least (its title, header and a few lines), while
/// the dashboard is that tall.
const MIN_LIST: u16 = 5;
/// The list's title line and its column header, above its lines.
const LIST_HEADS: u16 = 2;
/// The key line under the preview.
const KEYS_HEIGHT: u16 = 1;
/// The preview's height at least: its title and one line of the terminal.
/// A preview shorter than that is given to the list.
const MIN_PREVIEW: u16 = 2;

/// What the dashboard shows.
pub struct View<'a> {
    pub rows: &'a [Row],
    /// The keys, shown on the dashboard's last line.
    pub keys: &'a str,
    pub details: &'a TaskDetails,
    pub selected: Option<&'a RowId>,
    /// The selected row's terminal, unless it was closed.
    pub preview: Option<&'a Emulator>,
    /// Keys go to the previewed terminal.
    pub interactive: bool,
    /// Seconds since the epoch, for commit ages.
    pub now: u64,
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

fn task_style(state: TaskState) -> Style {
    match state {
        TaskState::Waiting => style(State::Waiting),
        TaskState::Done => style(State::Idle),
        TaskState::Running | TaskState::Stopped => Style::default(),
    }
}

/// A list line: a session row, or one of its task rows; `None` is the blank
/// line between two sessions.
pub type Line<'a> = Option<(&'a Row, Option<&'a TaskRow>)>;

/// The dashboard's lines in order: each session row, then its task rows,
/// with a blank line between two sessions.
pub fn lines(rows: &[Row]) -> Vec<Line<'_>> {
    rows.iter()
        .enumerate()
        .flat_map(|(i, r)| {
            let blank = (i > 0).then_some(None);
            let session = std::iter::once(Some((r, None)));
            let tasks = r.tasks.iter().map(move |t| Some((r, Some(t))));
            blank.into_iter().chain(session).chain(tasks)
        })
        .collect()
}

/// Whether `line` is a subagent's task row, which is never selected: it
/// goes with the line above it.
fn is_subagent(line: &Line) -> bool {
    line.is_some_and(|(_, task)| task.is_some_and(|t| t.subagent.is_some()))
}

/// Where the dashboard's parts go, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The title, then the rows.
    pub list: Rect,
    /// The preview's title, then the selected row's terminal.
    pub preview: Rect,
    /// The key line, at the bottom.
    pub keys: Rect,
}

/// The dashboard's parts in `area` for a list of `lines` lines: the list as
/// tall as its lines (one line saying there are none without any) but at
/// most [`LIST_SHARE_PERCENT`] of the height and at least [`MIN_LIST`], the
/// key line at the bottom, and the preview between them. On a short screen
/// the preview goes first, then the key line.
pub fn layout(area: Rect, lines: usize) -> Layout {
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
        .saturating_add(LIST_HEADS);
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
    let parts = layout(area, lines(view.rows).len());
    render_list(view, parts.list, buf);
    render_keys(view.keys, parts.keys, buf);
    render_preview(view, parts.preview, buf)
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

/// A list line's cells and style: a session row's, or a task row's.
fn cells(view: &View, row: &Row, task: Option<&TaskRow>) -> (table::Cells, Style) {
    let Some(t) = task else {
        return (table::session(row), style(row.state));
    };
    let details = view.details.get(&(t.cwd.clone(), t.branch.clone()));
    let style = t
        .subagent
        .as_ref()
        .map_or(Style::default(), |s| task_style(s.state));
    (table::task(t, details, view.now), style)
}

/// The title, the column header (dim), then the lines in sight as a table,
/// scrolled to keep the selected line in sight with the subagents under it.
fn render_list(view: &View, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let width = usize::from(area.width);
    render_title(area, buf);
    let lines = lines(view.rows);
    let picked = lines.iter().position(|l| {
        let Some((row, task)) = l else {
            return false;
        };
        view.selected.is_some_and(|s| {
            s.pane == row.pane && s.branch.as_deref() == task.map(|t| t.branch.as_str())
        }) && !is_subagent(l)
    });
    // Keep the selected line on screen with the subagents under it, the
    // selected line first if they do not all fit.
    let room = usize::from(area.height.saturating_sub(LIST_HEADS));
    let skip = picked.map_or(0, |p| {
        let subagents = lines[p + 1..].iter().take_while(|l| is_subagent(l)).count();
        (p + 1 + subagents).saturating_sub(room).min(p)
    });
    let shown: Vec<(usize, Option<(table::Cells, Style)>)> = lines
        .iter()
        .enumerate()
        .skip(skip)
        .take(room)
        .map(|(i, l)| (i, l.map(|(row, task)| cells(view, row, task))))
        .collect();
    let header = table::header();
    let all: Vec<&table::Cells> = std::iter::once(&header)
        .chain(shown.iter().filter_map(|(_, l)| l.as_ref().map(|(c, _)| c)))
        .collect();
    let widths = table::widths(&all, width);
    if area.height > 1 {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let text = table::line(&header, &widths, width);
        buf.set_stringn(area.x, area.y + 1, text, width, dim);
    }
    if lines.is_empty() && area.height > LIST_HEADS {
        let y = area.y + LIST_HEADS;
        buf.set_stringn(area.x, y, EMPTY, width, Style::default());
    }
    for ((i, l), y) in shown.iter().zip(area.y + LIST_HEADS..area.bottom()) {
        let Some((cells, style)) = l else {
            continue;
        };
        let style = if Some(*i) == picked {
            style.add_modifier(Modifier::REVERSED)
        } else {
            *style
        };
        buf.set_stringn(area.x, y, table::line(cells, &widths, width), width, style);
    }
}

/// The selected row's terminal under a title line, its rows chosen so the
/// last line with text (or the cursor) stays in sight.
fn render_preview(view: &View, area: Rect, buf: &mut Buffer) -> Option<Position> {
    if area.height < 2 || area.width == 0 {
        return None;
    }
    let width = usize::from(area.width);
    let label = view
        .selected
        .and_then(|s| view.rows.iter().find(|r| r.pane == s.pane))
        .map(|r| r.label.as_str());
    let Some(label) = label else {
        // No rows: nothing to preview.
        buf.set_stringn(area.x, area.y, "─".repeat(width), width, Style::default());
        return None;
    };
    let (title, title_style) = match (view.preview, view.interactive) {
        (None, _) => (format!("─ {label}: no terminal "), Style::default()),
        (Some(_), false) => (format!("─ preview: {label}  I to type "), Style::default()),
        (Some(_), true) => (
            format!("─ typing into {label}  Esc leaves "),
            Style::default().add_modifier(Modifier::REVERSED),
        ),
    };
    let title = format!("{title}{}", "─".repeat(width));
    buf.set_stringn(area.x, area.y, title, width, title_style);
    let emu = view.preview?;
    let screen = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
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
            closed: false,
        }
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
        let got: Vec<Option<(&str, Option<&str>)>> = lines(&rows)
            .into_iter()
            .map(|l| l.map(|(r, t)| (r.label.as_str(), t.map(|t| t.branch.as_str()))))
            .collect();
        assert_eq!(
            got,
            [
                Some(("w", None)),
                Some(("w", Some("a"))),
                Some(("w", Some("b"))),
                None,
                Some(("i", None)),
                None,
                Some(("r", None)),
            ]
        );
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
            layout(area(40), 7),
            stacked(9, 30, 1),
            "the title, the header and seven lines"
        );
        assert_eq!(layout(area(40), 50), stacked(16, 23, 1), "capped at 40%");
        assert_eq!(
            layout(area(40), 1),
            stacked(5, 34, 1),
            "a few rows at least"
        );
        assert_eq!(
            layout(area(40), 0),
            stacked(5, 34, 1),
            "the line saying there are none"
        );
    }

    #[test]
    fn a_short_dashboard_loses_its_preview_before_its_list() {
        let area = |height| Rect::new(0, 1, 80, height);
        assert_eq!(layout(area(8), 9), stacked(5, 2, 1), "the minimum wins");
        assert_eq!(
            layout(area(6), 9),
            stacked(5, 0, 1),
            "no room for a terminal line: the list keeps it"
        );
        assert_eq!(layout(area(3), 9), stacked(2, 0, 1));
        assert_eq!(layout(area(1), 9), stacked(1, 0, 0), "only the title");
        assert_eq!(layout(area(0), 9), stacked(0, 0, 0));
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
        let area = Rect::new(0, 0, 80, 12);
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
            branch: Some("feat/a".into()),
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &details,
            selected: Some(&selected),
            preview: Some(&emu),
            interactive: false,
            now: 0,
        };
        assert_eq!(render(&view, area, &mut buf), None);
        assert_eq!(text(&buf, 0), " dashboard");
        assert_eq!(
            [1, 2, 3, 4].map(|y| text(&buf, y)),
            [
                " NAME             STATE    ↑↓  CHANGES   BASE  FILES  AGE  MERGED",
                " api              idle",
                "   feat/a                        +3 −1  +5 −0      2       ✓",
                "   Fix b · fix/b  waiting",
            ]
        );
        assert!(buf[(1, 1)].modifier.contains(Modifier::DIM), "a dim header");
        assert!(buf[(3, 3)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(3, 2)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buf[(3, 4)].bg, Color::Yellow);
        assert!(text(&buf, 5).starts_with("─ preview: api  I to type ──"));
        assert_eq!(text(&buf, 6), "hello preview");
        assert_eq!(text(&buf, 11), " j down  k up", "the keys at the bottom");
        assert!(buf[(1, 11)].modifier.contains(Modifier::DIM));

        let typing = View {
            interactive: true,
            ..view
        };
        let cursor = render(&typing, area, &mut Buffer::empty(area));
        assert_eq!(cursor, Some(Position::new(13, 6)));
    }

    #[test]
    fn a_closed_rows_preview_says_it_has_no_terminal() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        let rows = [row("gone", State::Stopped, vec![])];
        let selected = RowId {
            pane: "%gone".into(),
            branch: None,
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            now: 0,
        };
        render(&view, area, &mut buf);
        assert!(text(&buf, 5).starts_with("─ gone: no terminal ─"));
    }

    #[test]
    fn without_rows_it_says_there_are_no_sessions() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        let view = View {
            keys: KEYS,
            rows: &[],
            details: &TaskDetails::new(),
            selected: None,
            preview: None,
            interactive: false,
            now: 0,
        };
        render(&view, area, &mut buf);
        assert_eq!(text(&buf, 2), " no Claude sessions yet");
        assert_eq!(text(&buf, 5), "─".repeat(40));
        assert_eq!(text(&buf, 7), " j down  k up");
    }

    #[test]
    fn the_selected_line_scrolls_into_sight() {
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let rows: Vec<Row> = (0..6)
            .map(|i| row(&format!("r{i}"), State::Running, vec![]))
            .collect();
        let selected = RowId {
            pane: "%r5".into(),
            branch: None,
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            now: 0,
        };
        render(&view, area, &mut buf);
        // The list is capped at five rows: three lines below its title and
        // header, a blank one between two sessions.
        assert!(text(&buf, 2).starts_with(" r4"));
        assert_eq!(text(&buf, 3), "");
        assert!(text(&buf, 4).starts_with(" r5"));
        assert!(text(&buf, 5).starts_with("─ r5: no terminal"));
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
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        let rows = [
            row("r0", State::Running, vec![]),
            row("r1", State::Running, vec![]),
            row("r2", State::Running, vec![subagent("s1"), subagent("s2")]),
        ];
        let selected = RowId {
            pane: "%r2".into(),
            branch: None,
        };
        let view = View {
            keys: KEYS,
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            now: 0,
        };
        render(&view, area, &mut buf);
        assert!(text(&buf, 2).starts_with(" r2"));
        assert!(text(&buf, 3).starts_with("   s1"));
        assert!(text(&buf, 4).starts_with("   s2"));
        // Only the selected line is highlighted, not its subagents.
        assert!(buf[(1, 2)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(3, 3)].modifier.contains(Modifier::REVERSED));
    }
}
