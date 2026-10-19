//! The sidebar: a title, then one row per Claude session, label left and
//! state right, each followed by its task rows (branch left, git fields
//! right), with a border on its right edge.

use crate::app::{Row, TaskRow};
use crate::sessions::{State, TaskState};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

const TITLE: &str = "sessions";
/// Rows above the first session row (the title).
const FIRST_ROW: u16 = 1;

/// How a row (or a tab holding it) in `state` stands out, in the terminal's
/// own ANSI colours: strongly while waiting, softly while idle.
pub fn highlight(state: State) -> Style {
    match state {
        State::Waiting => Style::default().fg(Color::Black).bg(Color::Yellow),
        State::Idle => Style::default().fg(Color::Green),
        State::Running | State::Stopped | State::Unknown => Style::default(),
    }
}

/// Draw `rows` into `area`; `selected` is the focused row's pane, if focused.
pub fn render(rows: &[Row], selected: Option<&str>, area: Rect, buf: &mut Buffer) {
    if area.width < 2 || area.height == 0 {
        return;
    }
    let inner = area.width - 1;
    for y in area.top()..area.bottom() {
        buf.set_string(area.right() - 1, y, "│", Style::default());
    }
    let dim = Style::default().add_modifier(Modifier::DIM);
    buf.set_stringn(area.x + 1, area.y, TITLE, usize::from(inner - 1), dim);
    let width = usize::from(inner);
    for ((index, task), y) in lines(rows).zip(area.y + FIRST_ROW..area.bottom()) {
        let row = &rows[index];
        let (text, style) = match task {
            None if selected == Some(row.pane.as_str()) => (
                spread(1, &row.label, row.state.label(), width),
                highlight(row.state).add_modifier(Modifier::REVERSED),
            ),
            None => (
                spread(1, &row.label, row.state.label(), width),
                highlight(row.state),
            ),
            Some(t) => task_line(t, width),
        };
        buf.set_string(area.x, y, text, style);
    }
}

/// A task row: the branch and git fields, or for a subagent's task its
/// description (else the branch), git fields and state, standing out like a
/// session row while waiting and, softly, once done.
fn task_line(task: &TaskRow, width: usize) -> (String, Style) {
    let Some(sub) = &task.subagent else {
        return (
            spread(2, &task.branch, &fields(task), width),
            Style::default(),
        );
    };
    let label = sub.description.as_deref().unwrap_or(&task.branch);
    let right = [fields(task), sub.state.label().to_string()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let style = match sub.state {
        TaskState::Waiting => highlight(State::Waiting),
        TaskState::Done => highlight(State::Idle),
        TaskState::Running | TaskState::Stopped => Style::default(),
    };
    (spread(2, label, &right, width), style)
}

/// The sidebar's lines in order: each session row (by index into `rows`),
/// then its task rows.
fn lines(rows: &[Row]) -> impl Iterator<Item = (usize, Option<&TaskRow>)> {
    rows.iter().enumerate().flat_map(|(i, row)| {
        std::iter::once((i, None)).chain(row.tasks.iter().map(move |t| (i, Some(t))))
    })
}

/// The session row drawn at screen row `y` of a sidebar over `area`, if any;
/// a task row counts as its session's.
pub fn row_at(rows: &[Row], area: Rect, y: u16) -> Option<usize> {
    let first = area.y + FIRST_ROW;
    if !(first..area.bottom()).contains(&y) {
        return None;
    }
    lines(rows).nth(usize::from(y - first)).map(|(i, _)| i)
}

/// A task's git fields, `↑ahead↓behind +added −removed`, leaving out what is
/// zero or unknown.
fn fields(task: &TaskRow) -> String {
    let Some(status) = task.status else {
        return String::new();
    };
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
    [Some(arrows), changes]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Status;

    fn row(label: &str, state: State) -> Row {
        Row {
            pane: format!("%{label}"),
            terminal: format!("${label}"),
            tab: 0,
            label: label.into(),
            state,
            tasks: Vec::new(),
        }
    }

    fn task(
        branch: &str,
        ahead_behind: Option<(u32, u32)>,
        changes: Option<(u32, u32)>,
    ) -> TaskRow {
        TaskRow {
            branch: branch.into(),
            cwd: "/r".into(),
            status: Some(Status {
                ahead_behind,
                changes,
            }),
            subagent: None,
        }
    }

    fn text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    #[test]
    fn rows_show_label_and_state_under_a_title() {
        let area = Rect::new(0, 0, 16, 4);
        let mut buf = Buffer::empty(area);
        let rows = [row("api", State::Waiting), row("a-long-label", State::Idle)];
        render(&rows, Some("%api"), area, &mut buf);
        assert_eq!(text(&buf, 0), " sessions      │");
        assert_eq!(text(&buf, 1), " api   waiting │");
        assert_eq!(text(&buf, 2), " a-long-l idle │");
        assert_eq!(text(&buf, 3), "               │");
        assert!(buf[(1, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(1, 2)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn control_characters_in_labels_never_reach_the_screen() {
        // Labels come from `claude agents`, tab names and directories; an
        // escape sequence in one must not be written to the outer terminal.
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        render(
            &[row("a\x1b]52;c;eA==\x07b", State::Idle)],
            None,
            area,
            &mut buf,
        );
        let drawn = text(&buf, 1);
        assert!(!drawn.contains(char::is_control), "{drawn:?}");
    }

    #[test]
    fn clicks_find_the_row_drawn_under_them() {
        let area = Rect::new(0, 3, 16, 6);
        let mut api = row("api", State::Idle);
        api.tasks = vec![task("feat/a", None, None), task("feat/b", None, None)];
        let rows = [api, row("web", State::Idle), row("db", State::Idle)];
        assert_eq!(row_at(&rows, area, 3), None, "title");
        assert_eq!(row_at(&rows, area, 4), Some(0));
        assert_eq!(row_at(&rows, area, 5), Some(0), "a task row of api");
        assert_eq!(row_at(&rows, area, 6), Some(0));
        assert_eq!(row_at(&rows, area, 7), Some(1));
        assert_eq!(row_at(&rows, area, 8), Some(2));
        assert_eq!(row_at(&rows, area, 9), None, "below the sidebar");
        assert_eq!(row_at(&rows[1..], area, 6), None, "below the last row");
    }

    #[test]
    fn task_rows_follow_their_session_with_git_fields() {
        let area = Rect::new(0, 0, 24, 7);
        let mut buf = Buffer::empty(area);
        let mut api = row("api", State::Running);
        api.tasks = vec![
            task("feat/login", Some((2, 1)), Some((30, 144))),
            task("fix/a-very-long-branch-name", Some((0, 0)), Some((0, 0))),
            task("docs", None, Some((5, 0))),
            TaskRow {
                branch: "unread".into(),
                cwd: "/r".into(),
                status: None,
                subagent: None,
            },
        ];
        render(
            &[api, row("web", State::Idle)],
            Some("%api"),
            area,
            &mut buf,
        );
        assert_eq!(text(&buf, 1), " api           running │");
        assert_eq!(text(&buf, 2), "  feat/l ↑2↓1 +30 −144 │");
        assert_eq!(text(&buf, 3), "  fix/a-very-long-bran │");
        assert_eq!(text(&buf, 4), "  docs           +5 −0 │");
        assert_eq!(text(&buf, 5), "  unread               │");
        assert_eq!(text(&buf, 6), " web              idle │");
        assert!(buf[(1, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(2, 2)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn subagent_task_rows_show_their_description_and_state() {
        use crate::sessions::{SubagentTask, TaskState};
        let area = Rect::new(0, 0, 24, 5);
        let mut buf = Buffer::empty(area);
        let sub = |branch: &str, description: Option<&str>, state, changes| TaskRow {
            subagent: Some(SubagentTask {
                agent_id: "a1".into(),
                description: description.map(str::to_string),
                state,
            }),
            ..task(branch, None, changes)
        };
        let mut api = row("api", State::Idle);
        api.tasks = vec![
            sub(
                "fix/a",
                Some("Create fix/a branch"),
                TaskState::Running,
                None,
            ),
            sub("fix/b", None, TaskState::Done, Some((3, 1))),
            sub("fix/c", Some("Ask"), TaskState::Waiting, None),
            sub("fix/d", Some("Gone"), TaskState::Stopped, None),
        ];
        render(&[api], None, area, &mut buf);
        assert_eq!(text(&buf, 1), " api              idle │");
        assert_eq!(text(&buf, 2), "  Create fix/a running │");
        assert_eq!(text(&buf, 3), "  fix/b     +3 −1 done │");
        assert_eq!(text(&buf, 4), "  Ask          waiting │");
        assert_eq!(buf[(2, 3)].fg, Color::Green, "done stands out softly");
        assert_eq!(buf[(2, 4)].bg, Color::Yellow, "waiting stands out");
        assert_eq!(buf[(2, 2)].fg, Color::Reset);
    }

    #[test]
    fn rows_stop_at_the_bottom() {
        let area = Rect::new(0, 0, 24, 2);
        let mut buf = Buffer::empty(area);
        let mut api = row("api", State::Running);
        api.tasks = vec![task("feat/login", None, None)];
        render(&[api], None, area, &mut buf);
        assert_eq!(text(&buf, 1), " api           running │");
    }

    #[test]
    fn waiting_rows_stand_out_strongly_and_idle_rows_softly() {
        let area = Rect::new(0, 0, 16, 4);
        let mut buf = Buffer::empty(area);
        let rows = [
            row("w", State::Waiting),
            row("i", State::Idle),
            row("r", State::Running),
        ];
        render(&rows, Some("%w"), area, &mut buf);
        let (waiting, idle, running) = (&buf[(1, 1)], &buf[(1, 2)], &buf[(1, 3)]);
        assert_eq!((waiting.fg, waiting.bg), (Color::Black, Color::Yellow));
        assert!(
            waiting.modifier.contains(Modifier::REVERSED),
            "still selected"
        );
        assert_eq!((idle.fg, idle.bg), (Color::Green, Color::Reset));
        assert_eq!((running.fg, running.bg), (Color::Reset, Color::Reset));
    }
}
