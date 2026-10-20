//! Key help (`C-a ?`): an overlay listing the keys as configured.

use crate::input::keymap::{label, Keymap};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// Columns between the two columns of command keys.
const GAP: usize = 3;

/// The lines of the help, with the prefix written out (e.g. `C-a`): the
/// commands' keys in two columns, then the sidebar's and the dashboard's.
pub fn lines(prefix: &str, keys: &Keymap) -> Vec<String> {
    let mut out = vec![format!("Press {prefix}, then:"), String::new()];
    let cells: Vec<String> = keys
        .commands
        .bindings()
        .filter(|(_, k)| !k.is_empty())
        .map(|(b, k)| format!("{:<8} {}", label(k), b.help))
        .collect();
    out.extend(two_columns(&cells));
    out.push(format!("{prefix:<8} send {prefix} to the terminal"));
    out.push(String::new());
    out.push(format!("Sidebar:   {}", keys.sidebar.hint()));
    out.push(format!("Dashboard: {}", keys.dashboard.hint()));
    out.push(String::new());
    out.push("Any key closes this help.".into());
    out
}

/// `cells` down the left column, then down the right one.
fn two_columns(cells: &[String]) -> Vec<String> {
    let rows = cells.len().div_ceil(2);
    let width = cells.iter().map(|c| c.chars().count()).max().unwrap_or(0) + GAP;
    let (left, right) = cells.split_at(rows);
    left.iter()
        .enumerate()
        .map(|(i, l)| match right.get(i) {
            Some(r) => format!("{l:<width$}{r}"),
            None => l.clone(),
        })
        .collect()
}

/// Draw the help centred in `area`.
pub fn render(prefix: &str, keys: &Keymap, area: Rect, buf: &mut Buffer) {
    let text = lines(prefix, keys);
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
    let body: Vec<Line> = text
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
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
    use crate::input::keymap::{Given, Key};

    #[test]
    fn lists_every_binding_with_the_prefix() {
        let l = lines("C-b", &Keymap::default());
        assert_eq!(l[0], "Press C-b, then:");
        assert!(l
            .iter()
            .any(|x| x.contains("c        new tab in this space")));
        assert!(l.iter().any(|x| x.contains("1...9    go to tab")));
        assert!(l.iter().any(|x| x.contains("send C-b to the terminal")));
        assert!(l.iter().any(|x| x.starts_with("Sidebar:   j down  k up")));
        assert!(l.iter().any(|x| x.contains("I interact  Esc back")));
        assert!(l.len() <= 26, "fits a 30-row screen: {}", l.len());
    }

    #[test]
    fn shows_remapped_keys_and_leaves_out_unbound_ones() {
        let commands: Given = [
            ("new_tab".to_string(), vec!["t".to_string()]),
            ("detach".to_string(), vec![]),
        ]
        .into();
        let none = Given::new();
        let keys = Keymap::configured(
            Key::parse("C-a").unwrap(),
            [&commands, &none, &none],
            &mut vec![],
        )
        .unwrap();
        let l = lines("C-a", &keys);
        assert!(l
            .iter()
            .any(|x| x.contains("t        new tab in this space")));
        assert!(!l.iter().any(|x| x.contains("detach")));
    }

    #[test]
    fn renders_a_centred_box_that_fits_small_screens() {
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        render("C-a", &Keymap::default(), area, &mut buf);
        let rows: Vec<String> = (0..30)
            .map(|y| (0..100).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        assert!(rows.iter().any(|r| r.contains("paddock keys")));
        assert!(rows
            .iter()
            .any(|r| r.contains("c        new tab in this space")));
        assert!(rows.iter().any(|r| r.contains("Any key closes this help.")));
        let small = Rect::new(0, 0, 10, 3);
        render("C-a", &Keymap::default(), small, &mut Buffer::empty(small));
    }
}
