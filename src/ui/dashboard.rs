//! The dashboard (`C-a o`): an overlay over the session view listing every
//! session row with its task rows and their git details, beside or above a
//! live, read-only preview of the selected row's terminal.

use super::session_view;
use crate::app::{Row, RowId, TaskDetails, TaskRow};
use crate::git::{age, Details};
use crate::sessions::{State, TaskState};
use crate::terminal::emulator::Emulator;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Clear, Widget};

pub const TITLE: &str = "dashboard";
const KEYS: &str = "j/k move  Enter jump  D dismiss  x close  f fork  I interact  Esc back";
/// From this width on, the preview sits beside the list instead of below it.
const SIDE_BY_SIDE: u16 = 140;

/// What the dashboard shows.
pub struct View<'a> {
    pub rows: &'a [Row],
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

/// The dashboard's lines in order: each session row, then its task rows.
pub fn lines(rows: &[Row]) -> Vec<(&Row, Option<&TaskRow>)> {
    rows.iter()
        .flat_map(|r| std::iter::once((r, None)).chain(r.tasks.iter().map(move |t| (r, Some(t)))))
        .collect()
}

/// A task's fields: ahead/behind and uncommitted line changes, then its base
/// diff and changed files, last commit age, merged badge, and a subagent's
/// state; what is zero or unknown is left out.
pub fn fields(task: &TaskRow, details: Option<&Details>, now: u64) -> String {
    let status = task.status.unwrap_or_default();
    let arrows: String = status
        .ahead_behind
        .map(|(ahead, behind)| [("↑", ahead), ("↓", behind)])
        .into_iter()
        .flatten()
        .filter(|&(_, n)| n > 0)
        .map(|(arrow, n)| format!("{arrow}{n}"))
        .collect();
    let changes = status
        .changes
        .filter(|&c| c != (0, 0))
        .map(|(added, removed)| format!("+{added} −{removed}"));
    let base = details
        .and_then(|d| d.base_diff)
        .filter(|&(_, _, files)| files > 0);
    let base_diff = base.map(|(added, removed, _)| format!("base +{added} −{removed}"));
    let files = base.map(|(_, _, n)| match n {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    });
    let commit = details
        .and_then(|d| d.last_commit)
        .map(|t| age(now.saturating_sub(t)));
    let merged = details.filter(|d| d.merged).map(|_| "merged".to_string());
    let state = task.subagent.as_ref().map(|s| s.state.label().to_string());
    [
        Some(arrows),
        changes,
        base_diff,
        files,
        commit,
        merged,
        state,
    ]
    .into_iter()
    .flatten()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join("  ")
}

/// `left` after `indent` spaces and `right` at the end, then one space:
/// exactly `width` cells; `left` is cut to fit.
fn spread(indent: usize, left: &str, right: &str, width: usize) -> String {
    let right_len = right.chars().count();
    let min_gap = usize::from(right_len > 0);
    let left: String = left
        .chars()
        .take(width.saturating_sub(indent + right_len + min_gap + 1))
        .collect();
    let gap = width.saturating_sub(indent + left.chars().count() + right_len + 1);
    let text = format!("{}{left}{}{right} ", " ".repeat(indent), " ".repeat(gap));
    text.chars().take(width).collect()
}

/// The list's area and the preview's, with one line or column between them.
pub fn split(area: Rect) -> (Rect, Rect) {
    if area.width >= SIDE_BY_SIDE {
        let list = area.width / 2;
        let preview = Rect {
            x: area.x + list + 1,
            width: area.width - list - 1,
            ..area
        };
        return (
            Rect {
                width: list,
                ..area
            },
            preview,
        );
    }
    let list = area.height / 2;
    let preview = Rect {
        y: area.y + list,
        height: area.height - list,
        ..area
    };
    (
        Rect {
            height: list,
            ..area
        },
        preview,
    )
}

/// Draw the dashboard over `area`. Returns where the cursor goes: in the
/// previewed terminal while interactive.
pub fn render(view: &View, area: Rect, buf: &mut Buffer) -> Option<Position> {
    Clear.render(area, buf);
    let (list, preview) = split(area);
    render_list(view, list, buf);
    if preview.x > list.right() {
        for y in area.top()..area.bottom() {
            buf.set_string(list.right(), y, "│", Style::default());
        }
    }
    render_preview(view, preview, buf)
}

fn render_list(view: &View, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let width = usize::from(area.width);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let title = format!(" {TITLE}");
    buf.set_stringn(area.x, area.y, &title, width, bold);
    let keys_at = u16::try_from(title.len() + 2).unwrap_or(u16::MAX);
    if keys_at < area.width {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let room = usize::from(area.width - keys_at);
        buf.set_stringn(area.x + keys_at, area.y, KEYS, room, dim);
    }
    let lines = lines(view.rows);
    let picked = lines.iter().position(|(row, task)| {
        view.selected.is_some_and(|s| {
            s.pane == row.pane && s.branch.as_deref() == task.map(|t| t.branch.as_str())
        })
    });
    // Keep the selected line on screen.
    let room = usize::from(area.height - 1);
    let skip = picked.map_or(0, |p| (p + 1).saturating_sub(room));
    for ((i, (row, task)), y) in lines
        .iter()
        .enumerate()
        .skip(skip)
        .zip(area.y + 1..area.bottom())
    {
        let (text, style) = match task {
            None => (
                spread(1, &row.label, row.state.label(), width),
                style(row.state),
            ),
            Some(t) => {
                let label = match t.subagent.as_ref().and_then(|s| s.description.as_ref()) {
                    Some(d) => format!("{d} · {}", t.branch),
                    None => t.branch.clone(),
                };
                let right = fields(
                    t,
                    view.details.get(&(t.cwd.clone(), t.branch.clone())),
                    view.now,
                );
                let style = t
                    .subagent
                    .as_ref()
                    .map_or(Style::default(), |s| task_style(s.state));
                (spread(3, &label, &right, width), style)
            }
        };
        let style = if Some(i) == picked {
            style.add_modifier(Modifier::REVERSED)
        } else {
            style
        };
        buf.set_stringn(area.x, y, text, width, style);
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
        .map_or("", |r| r.label.as_str());
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
    use crate::git::Status;
    use crate::sessions::SubagentTask;

    fn row(label: &str, state: State, tasks: Vec<TaskRow>) -> Row {
        Row {
            pane: format!("%{label}"),
            terminal: format!("${label}"),
            tab: 0,
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
    fn fields_cover_status_base_diff_files_age_and_merged() {
        let status = Status {
            ahead_behind: Some((2, 0)),
            changes: Some((30, 144)),
        };
        let details = Details {
            base_diff: Some((12, 3, 4)),
            last_commit: Some(1_000),
            merged: true,
        };
        let t = task("feat/a", Some(status));
        assert_eq!(
            fields(&t, Some(&details), 1_000 + 7_200),
            "↑2  +30 −144  base +12 −3  4 files  2h  merged"
        );
        let one = Details {
            base_diff: Some((1, 0, 1)),
            last_commit: None,
            merged: false,
        };
        assert_eq!(
            fields(&t, Some(&one), 0),
            "↑2  +30 −144  base +1 −0  1 file"
        );
        let none = Details {
            base_diff: Some((0, 0, 0)),
            ..Details::default()
        };
        assert_eq!(fields(&task("b", None), Some(&none), 0), "");
        let sub = TaskRow {
            subagent: Some(SubagentTask {
                agent_id: "a1".into(),
                description: None,
                state: TaskState::Done,
            }),
            ..task("c", Some(Status::default()))
        };
        assert_eq!(fields(&sub, None, 0), "done");
    }

    #[test]
    fn lines_follow_rows_each_with_its_tasks() {
        let rows = [
            row("w", State::Waiting, vec![task("a", None), task("b", None)]),
            row("i", State::Idle, vec![]),
        ];
        let got: Vec<(&str, Option<&str>)> = lines(&rows)
            .into_iter()
            .map(|(r, t)| (r.label.as_str(), t.map(|t| t.branch.as_str())))
            .collect();
        assert_eq!(
            got,
            [("w", None), ("w", Some("a")), ("w", Some("b")), ("i", None)]
        );
    }

    #[test]
    fn the_preview_goes_below_the_list_unless_the_area_is_wide() {
        assert_eq!(
            split(Rect::new(24, 1, 76, 29)),
            (Rect::new(24, 1, 76, 14), Rect::new(24, 15, 76, 15))
        );
        assert_eq!(
            split(Rect::new(0, 1, 160, 40)),
            (Rect::new(0, 1, 80, 40), Rect::new(81, 1, 79, 40))
        );
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
        let area = Rect::new(0, 0, 60, 12);
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
                base_diff: Some((5, 0, 2)),
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
            rows: &rows,
            details: &details,
            selected: Some(&selected),
            preview: Some(&emu),
            interactive: false,
            now: 0,
        };
        assert_eq!(render(&view, area, &mut buf), None);
        assert!(text(&buf, 0).starts_with(" dashboard  j/k move"));
        assert_eq!(
            text(&buf, 1),
            " api                                                   idle"
        );
        assert_eq!(
            text(&buf, 2),
            "   feat/a                +3 −1  base +5 −0  2 files  merged"
        );
        assert_eq!(
            text(&buf, 3),
            "   Fix b · fix/b                                    waiting"
        );
        assert!(buf[(3, 2)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(3, 1)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buf[(3, 3)].bg, Color::Yellow);
        assert!(text(&buf, 6).starts_with("─ preview: api  I to type ──"));
        assert_eq!(text(&buf, 7), "hello preview");

        let typing = View {
            interactive: true,
            ..view
        };
        let cursor = render(&typing, area, &mut Buffer::empty(area));
        assert_eq!(cursor, Some(Position::new(13, 7)));
    }

    #[test]
    fn a_closed_rows_preview_says_it_has_no_terminal() {
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        let rows = [row("gone", State::Stopped, vec![])];
        let selected = RowId {
            pane: "%gone".into(),
            branch: None,
        };
        let view = View {
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            now: 0,
        };
        render(&view, area, &mut buf);
        assert!(text(&buf, 3).starts_with("─ gone: no terminal ─"));
    }

    #[test]
    fn the_selected_line_scrolls_into_sight() {
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        let rows: Vec<Row> = (0..6)
            .map(|i| row(&format!("r{i}"), State::Running, vec![]))
            .collect();
        let selected = RowId {
            pane: "%r5".into(),
            branch: None,
        };
        let view = View {
            rows: &rows,
            details: &TaskDetails::new(),
            selected: Some(&selected),
            preview: None,
            interactive: false,
            now: 0,
        };
        render(&view, area, &mut buf);
        // Two lines for the list below its title.
        assert!(text(&buf, 1).starts_with(" r4"));
        assert!(text(&buf, 2).starts_with(" r5"));
    }
}
