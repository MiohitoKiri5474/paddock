//! Key help (`C-a ?`): an overlay listing the keys as configured. With the
//! default keys it fits an 80x24 terminal.

use crate::config::Floats;
use crate::input::keymap::{label, Keymap};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// The most of a float's command the help shows.
const FLOAT_COMMAND: usize = 16;
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
/// commands' and floats' keys in two columns, each command with its direct
/// chord in brackets, then the sidebar's and the dashboard's keys.
pub fn lines(prefix: &str, keys: &Keymap, floats: &Floats) -> Vec<String> {
    let mut out = vec![
        format!("Press {prefix}, then:   ([chord]: alone, no prefix)"),
        String::new(),
    ];
    let mut cells: Vec<String> = keys
        .commands
        .bindings()
        // Both sections are the commands' table, in its order.
        .zip(keys.direct.bindings())
        .filter(|((_, k), (_, chords))| !k.is_empty() || !chords.is_empty())
        .map(|((b, k), (_, chords))| {
            let chord = match chords {
                [] => String::new(),
                _ => format!(" [{}]", label(chords)),
            };
            format!("{:<LABEL$} {}{chord}", label(k), b.help)
        })
        .collect();
    cells.extend(floats.list.iter().map(|f| {
        let command: String = f.command.chars().take(FLOAT_COMMAND).collect();
        let cut = if f.command.chars().count() > FLOAT_COMMAND {
            "..."
        } else {
            ""
        };
        format!("{:<LABEL$} float: {command}{cut}", f.key.name())
    }));
    cells.push(format!("{prefix:<LABEL$} send {prefix} to the terminal"));
    out.extend(two_columns(&cells));
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
pub fn render(prefix: &str, keys: &Keymap, floats: &Floats, area: Rect, buf: &mut Buffer) {
    let text = lines(prefix, keys, floats);
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
        let l = lines("C-b", &Keymap::default(), &Floats::default());
        assert!(l[0].starts_with("Press C-b, then:"), "{}", l[0]);
        assert!(l
            .iter()
            .any(|x| x.contains("c      new tab in this space [C-M-c]")));
        assert!(l.iter().any(|x| x.contains("l Right focus right [C-l]")));
        assert!(l
            .iter()
            .any(|x| x.contains("|      split side by side [C-\\]")));
        assert!(l
            .iter()
            .any(|x| x.contains("-      split top and bottom [C--]")));
        assert!(l
            .iter()
            .any(|x| x.contains("x      close pane (asks if busy)")));
        assert!(l.iter().any(|x| x.contains("1...9  go to tab")));
        assert!(l.iter().any(|x| x.contains("send C-b to the terminal")));
        assert!(l.iter().any(|x| x.contains("g      float: lazygit")));
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
        let direct: Given = [
            ("focus_left".to_string(), vec![String::new()]),
            ("help".to_string(), vec!["F1".to_string()]),
        ]
        .into();
        let none = Given::new();
        let prefix = Key::parse("C-a").unwrap();
        let (keys, _) = Keymap::configured(prefix, [&commands, &direct, &none, &none]).unwrap();
        let l = lines("C-a", &keys, &Floats::default());
        assert!(l.iter().any(|x| x.contains("t      new tab in this space")));
        assert!(!l.iter().any(|x| x.contains("detach")));
        assert!(l
            .iter()
            .any(|x| x.contains("focus left") && !x.contains("[C-h]")));
        assert!(l.iter().any(|x| x.contains("this help [F1]")));
        // A command on a chord only still shows, with no key before it.
        let commands: Given = [("detach".to_string(), vec![])].into();
        let direct: Given = [("detach".to_string(), vec!["C-M-q".to_string()])].into();
        let (keys, _) = Keymap::configured(prefix, [&commands, &direct, &none, &none]).unwrap();
        let l = lines("C-a", &keys, &Floats::default());
        assert!(l
            .iter()
            .any(|x| x.contains("       detach (terminals go on) [C-M-q]")));
    }

    #[test]
    fn the_default_help_fits_80x24_whole() {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let keys = Keymap::default();
        render("C-a", &keys, &Floats::default(), area, &mut buf);
        let shown = rows(area, &buf);
        assert!(shown.iter().any(|r| r.contains("paddock keys")));
        assert!(shown.iter().any(|r| r.contains("any key closes this help")));
        for line in lines("C-a", &keys, &Floats::default())
            .iter()
            .filter(|l| !l.is_empty())
        {
            let line = line.trim_end();
            assert!(
                shown.iter().any(|r| r.contains(line)),
                "cut off: {line:?}\n{}",
                shown.join("\n")
            );
        }
    }

    #[test]
    fn long_float_commands_are_cut_and_removed_floats_left_out() {
        let floats = Floats {
            list: vec![crate::config::Float {
                key: crate::input::keymap::Key::parse("T").unwrap(),
                command: "htop --delay 10 --sort-key PERCENT_CPU".into(),
            }],
            ..Floats::default()
        };
        let l = lines("C-a", &Keymap::default(), &floats);
        assert!(
            l.iter()
                .any(|x| x.contains("T      float: htop --delay 10 ...")),
            "{l:?}"
        );
        assert!(!l.iter().any(|x| x.contains("lazygit")));
    }

    #[test]
    fn renders_on_screens_too_small_for_it() {
        let small = Rect::new(0, 0, 10, 3);
        render(
            "C-a",
            &Keymap::default(),
            &Floats::default(),
            small,
            &mut Buffer::empty(small),
        );
    }
}
