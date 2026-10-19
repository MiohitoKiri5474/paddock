//! The session view: the current terminal's screen, drawn from its emulator.

use crate::terminal::emulator::{Colour, Emulator};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};

fn colour(c: Colour) -> Color {
    match c {
        Colour::Default => Color::Reset,
        Colour::Indexed(i) => Color::Indexed(i),
        Colour::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Draw the terminal into `area`. Returns where the cursor should be, if visible.
pub fn render(emu: &Emulator, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let (cols, rows) = emu.size();
    for y in 0..rows.min(area.height) {
        for x in 0..cols.min(area.width) {
            let c = emu.cell(y, x);
            if c.spacer {
                // Covered by the wide character to its left; ratatui skips it.
                continue;
            }
            let mut m = Modifier::empty();
            m.set(Modifier::BOLD, c.bold);
            m.set(Modifier::DIM, c.dim);
            m.set(Modifier::ITALIC, c.italic);
            m.set(Modifier::UNDERLINED, c.underline);
            m.set(Modifier::REVERSED, c.inverse);
            let style = Style::default()
                .fg(colour(c.fg))
                .bg(colour(c.bg))
                .add_modifier(m);
            let cell = &mut buf[(area.x + x, area.y + y)];
            cell.set_symbol(if c.text.is_empty() { " " } else { &c.text });
            cell.set_style(style);
        }
    }
    let offset = emu.scroll_offset();
    if offset > 0 {
        let label = format!("[{offset}/{}]", emu.history_len());
        let width = u16::try_from(label.len())
            .unwrap_or(area.width)
            .min(area.width);
        let x = area.right() - width;
        buf.set_string(
            x,
            area.y,
            &label,
            Style::default().add_modifier(Modifier::REVERSED),
        );
        return None;
    }
    let (row, col) = emu.cursor();
    let visible = emu.modes().show_cursor && row < area.height && col < area.width;
    visible.then(|| Position::new(area.x + col, area.y + row))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render into a fresh buffer, as a frame would.
    fn draw(emu: &Emulator, w: u16, h: u16) -> (Buffer, Option<Position>) {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let cursor = render(emu, area, &mut buf);
        (buf, cursor)
    }

    /// A row as the outer terminal shows it: cells under wide characters dropped.
    fn row(buf: &Buffer, emu: &Emulator, y: u16) -> String {
        (0..buf.area.width)
            .filter(|&x| !emu.cell(y, x).spacer)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn draws_text_colours_and_cursor() {
        let mut e = Emulator::new(12, 2, 10);
        e.feed("\x1b[1;38;2;1;2;3mhi\x1b[0m 漢字".as_bytes());
        let (buf, cursor) = draw(&e, 12, 2);
        assert_eq!(row(&buf, &e, 0), "hi 漢字");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(1, 2, 3));
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD));
        assert_eq!(cursor, Some(Position::new(7, 0)));
    }

    #[test]
    fn hidden_cursor_is_not_placed() {
        let mut e = Emulator::new(5, 1, 0);
        e.feed(b"\x1b[?25l");
        assert_eq!(draw(&e, 5, 1).1, None);
    }

    #[test]
    fn scrolled_view_shows_history_and_position() {
        let mut e = Emulator::new(20, 2, 100);
        for i in 0..6 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        e.scroll(3);
        let (buf, cursor) = draw(&e, 20, 2);
        assert_eq!(row(&buf, &e, 0), "line 2         [3/5]");
        assert_eq!(row(&buf, &e, 1), "line 3");
        assert_eq!(cursor, None);
    }
}
