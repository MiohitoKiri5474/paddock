//! The sidebar: a title, then one row per Claude session, label left and
//! state right, with a border on its right edge.

use crate::app::Row;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

const TITLE: &str = "sessions";

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
    for (row, y) in rows.iter().zip(area.y + 1..area.bottom()) {
        let style = if selected == Some(row.pane.as_str()) {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        buf.set_string(area.x, y, line(row, usize::from(inner)), style);
    }
}

/// ` label   state `, exactly `width` cells; long labels are cut.
fn line(row: &Row, width: usize) -> String {
    let state = row.state.label();
    let label: String = row
        .label
        .chars()
        .take(width.saturating_sub(state.len() + 3))
        .collect();
    let gap = width.saturating_sub(label.chars().count() + state.len() + 2);
    let text = format!(" {label}{}{state} ", " ".repeat(gap));
    text.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::State;

    fn row(label: &str, state: State) -> Row {
        Row {
            pane: format!("%{label}"),
            terminal: format!("${label}"),
            tab: 0,
            label: label.into(),
            state,
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
}
