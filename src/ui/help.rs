//! Key help (`C-a ?`): an overlay listing the keys as configured. With the
//! default keys it fits an 80x24 terminal.

use crate::input::keymap::{label, Keymap};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// Columns between the two columns of command keys.
const GAP: usize = 3;
/// Columns for a command's keys before its description.
const LABEL: usize = 6;
/// The widest a line of sidebar or dashboard keys gets before wrapping.
const HINT_WIDTH: usize = 69;
/// Columns the box adds around the lines: borders and a space each side.
const FRAME: usize = 4;
/// The prompt popup's keys, which are fixed (`Backspace` and `Delete`
/// too, left out to fit one line).
const PROMPT_KEYS: [&str; 5] = [
    "Enter ok",
    "Esc cancel",
    "← → Home End",
    "C-u clear",
    "C-w word",
];

/// The lines of the help, with the prefix written out (e.g. `C-a`): the
/// commands' keys in two columns, then the sidebar's and the dashboard's.
pub fn lines(prefix: &str, keys: &Keymap) -> Vec<String> {
    let mut out = vec![format!("Press {prefix}, then:"), String::new()];
    let cells: Vec<String> = keys
        .commands
        .bindings()
        .filter(|(_, k)| !k.is_empty())
        .map(|(b, k)| format!("{:<LABEL$} {}", label(k), b.help))
        .collect();
    out.extend(two_columns(&cells));
    out.push(format!("{prefix:<LABEL$} send {prefix} to the terminal"));
    out.push(String::new());
    out.extend(wrapped("Sidebar:   ", &keys.sidebar.hints()));
    out.extend(wrapped("Dashboard: ", &keys.dashboard.hints()));
    let prompt: Vec<String> = PROMPT_KEYS.iter().map(|k| k.to_string()).collect();
    out.extend(wrapped("Prompts:   ", &prompt));
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

/// `title` then `hints`, two spaces apart, on lines of at most
/// [`HINT_WIDTH`], the later ones indented under the first hint.
fn wrapped(title: &str, hints: &[String]) -> Vec<String> {
    let indent = " ".repeat(title.chars().count());
    let mut out = Vec::new();
    let mut line = title.to_string();
    let mut empty = true;
    for hint in hints {
        if !empty && line.chars().count() + 2 + hint.chars().count() > HINT_WIDTH {
            out.push(std::mem::replace(&mut line, indent.clone()));
            empty = true;
        }
        if !empty {
            line.push_str("  ");
        }
        line.push_str(hint);
        empty = false;
    }
    out.push(line);
    out
}

/// Draw the help centred in `area`.
pub fn render(prefix: &str, keys: &Keymap, area: Rect, buf: &mut Buffer) {
    let text = lines(prefix, keys);
    let width = text
        .iter()
        .map(|l| Line::raw(l.as_str()).width())
        .max()
        .unwrap_or(0)
        + FRAME;
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
                .title(" paddock keys ")
                .title_bottom(" any key closes this help "),
        )
        .render(popup, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keymap::{Given, Key};

    fn rows(area: Rect, buf: &Buffer) -> Vec<String> {
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn lists_every_binding_with_the_prefix() {
        let l = lines("C-b", &Keymap::default());
        assert_eq!(l[0], "Press C-b, then:");
        assert!(l.iter().any(|x| x.contains("c      new tab in this space")));
        assert!(l.iter().any(|x| x.contains("1...9  go to tab")));
        assert!(l.iter().any(|x| x.contains("send C-b to the terminal")));
        assert!(l.iter().any(|x| x.starts_with("Sidebar:   j down  k up")));
        assert!(l.iter().any(|x| x.contains("I interact  Esc back")));
        assert!(l
            .iter()
            .any(|x| x.starts_with("Prompts:   Enter ok  Esc cancel")));
    }

    #[test]
    fn long_key_lines_wrap_under_their_first_key() {
        let hints: Vec<String> = (0..12).map(|i| format!("k{i} action")).collect();
        let l = wrapped("Keys: ", &hints);
        assert!(l.len() > 1);
        assert!(l.iter().all(|x| x.chars().count() <= HINT_WIDTH), "{l:?}");
        assert!(l[1].starts_with("      k"), "{l:?}");
    }

    #[test]
    fn shows_remapped_keys_and_leaves_out_unbound_ones() {
        let commands: Given = [
            ("new_tab".to_string(), vec!["t".to_string()]),
            ("detach".to_string(), vec![]),
        ]
        .into();
        let none = Given::new();
        let prefix = Key::parse("C-a").unwrap();
        let (keys, _) = Keymap::configured(prefix, [&commands, &none, &none]).unwrap();
        let l = lines("C-a", &keys);
        assert!(l.iter().any(|x| x.contains("t      new tab in this space")));
        assert!(!l.iter().any(|x| x.contains("detach")));
    }

    #[test]
    fn the_default_help_fits_80x24_whole() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let keys = Keymap::default();
        render("C-a", &keys, area, &mut buf);
        let shown = rows(area, &buf);
        assert!(shown.iter().any(|r| r.contains("paddock keys")));
        assert!(shown.iter().any(|r| r.contains("any key closes this help")));
        for line in lines("C-a", &keys).iter().filter(|l| !l.is_empty()) {
            let line = line.trim_end();
            assert!(
                shown.iter().any(|r| r.contains(line)),
                "cut off: {line:?}\n{}",
                shown.join("\n")
            );
        }
    }

    #[test]
    fn renders_on_screens_too_small_for_it() {
        let small = Rect::new(0, 0, 10, 3);
        render("C-a", &Keymap::default(), small, &mut Buffer::empty(small));
    }
}
