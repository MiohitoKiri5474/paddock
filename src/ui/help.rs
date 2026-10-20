//! Key help (`C-a ?`): an overlay listing the command keys.

use crate::input::BINDINGS;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// The lines of the help, with the prefix written out (e.g. `C-a`).
pub fn lines(prefix: &str) -> Vec<String> {
    let mut out = vec![format!("Press {prefix}, then:"), String::new()];
    out.extend(
        BINDINGS
            .iter()
            .map(|(key, what)| format!("  {key:<6} {what}")),
    );
    out.push(format!("  {prefix:<6} send {prefix} to the terminal"));
    out.push(String::new());
    out.push("Any key closes this help.".into());
    out
}

/// Draw the help centred in `area`.
pub fn render(prefix: &str, area: Rect, buf: &mut Buffer) {
    let text = lines(prefix);
    let width = text
        .iter()
        .map(|l| Line::raw(l.as_str()).width())
        .max()
        .unwrap_or(0)
        + 4;
    let height = text.len() + 2;
    let width = u16::try_from(width).unwrap_or(u16::MAX).min(area.width);
    let height = u16::try_from(height).unwrap_or(u16::MAX).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    Clear.render(popup, buf);
    let body: Vec<Line> = text.into_iter().map(Line::raw).collect();
    Paragraph::new(body)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" paddock keys "),
        )
        .render(popup, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_binding_with_the_prefix() {
        let l = lines("C-b");
        assert_eq!(l[0], "Press C-b, then:");
        assert!(l.iter().any(|x| x.contains("new tab in this workspace")));
        assert!(l.iter().any(|x| x.contains("send C-b to the terminal")));
        assert_eq!(l.len(), BINDINGS.len() + 5);
    }

    #[test]
    fn renders_a_centred_box_that_fits_small_screens() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render("C-a", area, &mut buf);
        let rows: Vec<String> = (0..24)
            .map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        assert!(rows.iter().any(|r| r.contains("paddock keys")));
        assert!(rows
            .iter()
            .any(|r| r.contains("c      new tab in this workspace")));
        let small = Rect::new(0, 0, 10, 3);
        render("C-a", small, &mut Buffer::empty(small));
    }
}
