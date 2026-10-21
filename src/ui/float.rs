//! A float: a program's terminal in a box centred over the whole screen,
//! sidebar included, framed like the prompt popup (rounded border in the
//! accent colour, the command as the title).
//!
//! ```text
//! ╭────────────── lazygit ──────────────╮
//! │ the program's screen                │
//! ╰─────────────────────────────────────╯
//! ```

use super::copy::CopyView;
use super::{prompt, session_view};
use crate::terminal::emulator::Emulator;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use ratatui::widgets::{Clear, Widget};

/// The smallest box, border included: one cell inside.
const MIN_SIDE: u16 = 3;
/// Percent is out of this.
const WHOLE: u32 = 100;
/// Cells the border takes on each side.
const BORDER: u16 = 1;

/// Where the box goes on a screen `screen`: centred, `width` and `height`
/// percent of it, at least [`MIN_SIDE`] and at most the screen.
pub fn area(screen: Rect, (width, height): (u16, u16)) -> Rect {
    let side = |total: u16, percent: u16| {
        let part = u32::from(total) * u32::from(percent) / WHOLE;
        let part = u16::try_from(part).unwrap_or(total);
        part.max(MIN_SIDE).min(total)
    };
    let (w, h) = (side(screen.width, width), side(screen.height, height));
    Rect {
        x: screen.x + (screen.width - w) / 2,
        y: screen.y + (screen.height - h) / 2,
        width: w,
        height: h,
    }
}

/// The inside of a box at `area`: where the program's terminal is drawn,
/// and the size its pane gets.
pub fn inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(BORDER),
        y: area.y.saturating_add(BORDER),
        width: area.width.saturating_sub(2 * BORDER),
        height: area.height.saturating_sub(2 * BORDER),
    }
}

/// A float as drawn now.
pub struct Float<'a> {
    /// The command it runs, shown in the top border.
    pub title: &'a str,
    /// The border's colour: the accent (`[colors] tab_current`).
    pub accent: Color,
    /// The program's screen.
    pub emulator: &'a Emulator,
    /// Its copy mode, if in it.
    pub copy: Option<CopyView<'a>>,
}

impl Float<'_> {
    /// Draw the box at `area` (see [`area`]), the program's screen inside.
    /// Returns the program's cursor, if shown.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Option<Position> {
        Clear.render(area, buf);
        let title = format!(" {} ", self.title);
        prompt::frame(&title, self.accent).render(area, buf);
        let inside = inner(area);
        if inside.is_empty() {
            return None;
        }
        match &self.copy {
            Some(copy) => copy.render(self.emulator, inside, buf),
            None => session_view::render(self.emulator, inside, buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(buf: &Buffer) -> Vec<String> {
        let a = buf.area;
        (0..a.height)
            .map(|y| (0..a.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_box_is_centred_at_its_percent_of_the_screen() {
        let screen = Rect::new(0, 0, 100, 30);
        assert_eq!(area(screen, (80, 80)), Rect::new(10, 3, 80, 24));
        assert_eq!(area(screen, (100, 100)), screen);
        assert_eq!(area(screen, (50, 10)), Rect::new(25, 13, 50, 3));
        // Odd room splits with the extra column on the right.
        assert_eq!(
            area(Rect::new(0, 0, 11, 5), (80, 80)),
            Rect::new(1, 0, 8, 4)
        );
    }

    #[test]
    fn the_box_is_clamped_to_one_cell_inside_and_to_the_screen() {
        assert_eq!(
            area(Rect::new(0, 0, 100, 30), (1, 1)),
            Rect::new(48, 13, 3, 3)
        );
        assert_eq!(area(Rect::new(0, 0, 2, 1), (80, 80)), Rect::new(0, 0, 2, 1));
        assert_eq!(area(Rect::new(4, 2, 0, 0), (80, 80)), Rect::new(4, 2, 0, 0));
    }

    #[test]
    fn the_inside_is_the_box_less_its_border() {
        assert_eq!(inner(Rect::new(10, 3, 80, 24)), Rect::new(11, 4, 78, 22));
        assert_eq!(inner(Rect::new(0, 0, 1, 1)), Rect::new(1, 1, 0, 0));
    }

    #[test]
    fn draws_the_program_in_a_rounded_accent_box_titled_with_its_command() {
        let screen = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(screen);
        // Something under the box, which it hides.
        buf.set_string(0, 2, "x".repeat(20), ratatui::style::Style::default());
        let mut emu = Emulator::new(14, 2, 10);
        emu.feed(b"hello");
        let float = Float {
            title: "cat",
            accent: Color::Indexed(33),
            emulator: &emu,
            copy: None,
        };
        let box_at = Rect::new(2, 1, 16, 4);
        let cursor = float.render(box_at, &mut buf);
        let shown = rows(&buf);
        assert_eq!(shown[1], "  ╭──── cat ─────╮  ");
        assert_eq!(shown[2], "xx│hello         │xx");
        assert_eq!(shown[4], "  ╰──────────────╯  ");
        assert_eq!(buf[(2, 1)].fg, Color::Indexed(33));
        assert_eq!(cursor, Some(Position::new(8, 2)));
    }

    #[test]
    fn renders_in_boxes_too_small_for_a_terminal() {
        let emu = Emulator::new(1, 1, 0);
        for (w, h) in [(0, 0), (1, 1), (2, 2), (3, 3)] {
            let screen = Rect::new(0, 0, w, h);
            let float = Float {
                title: "lazygit",
                accent: Color::Blue,
                emulator: &emu,
                copy: None,
            };
            float.render(screen, &mut Buffer::empty(screen));
        }
    }
}
