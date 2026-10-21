//! A one-line text editor for prompts: the text and a cursor between its
//! characters. The cursor moves by character; widths on screen are display
//! widths, so wide characters (`漢`) take two cells.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    /// Characters before the cursor.
    cursor: usize,
}

impl LineEdit {
    /// `text` with the cursor at its end.
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The byte offset of character `i`.
    fn byte(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// Type `text` at the cursor, leaving out control characters.
    pub fn insert(&mut self, text: &str) {
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        let at = self.byte(self.cursor);
        self.text.insert_str(at, &typed);
        self.cursor += typed.chars().count();
    }

    /// Remove characters `from..to` and put the cursor at `from`.
    fn remove(&mut self, from: usize, to: usize) {
        let range = self.byte(from)..self.byte(to);
        self.text.replace_range(range, "");
        self.cursor = from;
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// The start of the word before the cursor (spaces before it skipped).
    fn word_start(&self) -> usize {
        let before: Vec<char> = self.text.chars().take(self.cursor).collect();
        let spaces = before
            .iter()
            .rev()
            .take_while(|c| c.is_whitespace())
            .count();
        let word = before
            .iter()
            .rev()
            .skip(spaces)
            .take_while(|c| !c.is_whitespace())
            .count();
        self.cursor - spaces - word
    }

    /// Apply an editing key: arrows, `Home`, `End`, `Backspace`, `Delete`,
    /// `C-u` (clear), `C-w` (delete the word before the cursor), or a
    /// character. Returns whether the key was one of them.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            KeyCode::Backspace if self.cursor > 0 => self.remove(self.cursor - 1, self.cursor),
            KeyCode::Delete if self.cursor < self.len() => {
                self.remove(self.cursor, self.cursor + 1)
            }
            KeyCode::Backspace | KeyCode::Delete => {}
            KeyCode::Char('u') if ctrl => self.remove(0, self.len()),
            KeyCode::Char('w') if ctrl => self.remove(self.word_start(), self.cursor),
            KeyCode::Char(c) if !ctrl && !alt => self.insert(c.encode_utf8(&mut [0; 4])),
            _ => return false,
        }
        true
    }

    /// What shows in a box `width` cells wide, and the cursor's column in
    /// it. The text scrolls so that the cursor, with a cell of its own,
    /// stays in sight; a wide character that would only half show is left
    /// out.
    pub fn window(&self, width: u16) -> (String, u16) {
        let width = usize::from(width);
        let cells = |c: &char| c.width().unwrap_or(0);
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor;
        let mut used = 0;
        // Back from the cursor while there is room, keeping a cell for it.
        while start > 0 && used + cells(&chars[start - 1]) < width {
            start -= 1;
            used += cells(&chars[start]);
        }
        // Text fitting from the start shows from the start.
        let before: usize = chars[..self.cursor].iter().map(cells).sum();
        if before < width {
            start = 0;
            used = before;
        }
        let mut shown = String::new();
        let mut total = 0;
        for c in &chars[start..] {
            if total + cells(c) > width {
                break;
            }
            total += cells(c);
            shown.push(*c);
        }
        (shown, u16::try_from(used).unwrap_or(u16::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(line: &mut LineEdit, code: KeyCode) {
        assert!(line.key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn ctrl(line: &mut LineEdit, c: char) {
        assert!(line.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
    }

    fn typed(line: &mut LineEdit, text: &str) {
        for c in text.chars() {
            press(line, KeyCode::Char(c));
        }
    }

    #[test]
    fn starts_with_the_cursor_after_the_text() {
        let mut line = LineEdit::new("claude");
        typed(&mut line, "-2");
        assert_eq!(line.text(), "claude-2");
    }

    #[test]
    fn types_and_deletes_at_the_cursor() {
        let mut line = LineEdit::new("ac");
        press(&mut line, KeyCode::Left);
        typed(&mut line, "b");
        assert_eq!(line.text(), "abc");
        press(&mut line, KeyCode::Home);
        press(&mut line, KeyCode::Delete);
        assert_eq!(line.text(), "bc");
        press(&mut line, KeyCode::Backspace); // nothing before the cursor
        press(&mut line, KeyCode::End);
        press(&mut line, KeyCode::Backspace);
        press(&mut line, KeyCode::Delete); // nothing after it
        assert_eq!(line.text(), "b");
    }

    #[test]
    fn the_cursor_stays_inside_the_text() {
        let mut line = LineEdit::new("ab");
        press(&mut line, KeyCode::Right);
        typed(&mut line, "c");
        press(&mut line, KeyCode::Home);
        press(&mut line, KeyCode::Left);
        typed(&mut line, "_");
        assert_eq!(line.text(), "_abc");
    }

    #[test]
    fn moves_and_deletes_by_character_not_byte() {
        let mut line = LineEdit::new("漢字é");
        press(&mut line, KeyCode::Left);
        press(&mut line, KeyCode::Backspace);
        assert_eq!(line.text(), "漢é");
        press(&mut line, KeyCode::Delete);
        assert_eq!(line.text(), "漢");
    }

    #[test]
    fn c_u_clears_and_c_w_deletes_the_word_before_the_cursor() {
        let mut line = LineEdit::new("build logs  ");
        ctrl(&mut line, 'w');
        assert_eq!(line.text(), "build ");
        ctrl(&mut line, 'w');
        assert_eq!(line.text(), "");
        ctrl(&mut line, 'w');
        let mut line = LineEdit::new("one two three");
        for _ in 0..6 {
            press(&mut line, KeyCode::Left);
        }
        ctrl(&mut line, 'w');
        assert_eq!(line.text(), "one  three");
        ctrl(&mut line, 'u');
        assert_eq!(line.text(), "");
    }

    #[test]
    fn other_keys_are_not_editing() {
        let mut line = LineEdit::new("a");
        assert!(!line.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(!line.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)));
        assert!(!line.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)));
        assert_eq!(line.text(), "a");
    }

    #[test]
    fn pasted_text_loses_control_characters() {
        let mut line = LineEdit::new("");
        line.insert("a\nb\x1bc");
        assert_eq!(line.text(), "abc");
        assert_eq!(line.window(10), ("abc".to_string(), 3));
    }

    #[test]
    fn short_text_shows_whole_with_the_cursor_where_it_is() {
        let mut line = LineEdit::new("claude");
        assert_eq!(line.window(10), ("claude".to_string(), 6));
        press(&mut line, KeyCode::Home);
        assert_eq!(line.window(10), ("claude".to_string(), 0));
    }

    #[test]
    fn long_text_scrolls_to_keep_the_cursor_in_sight() {
        let mut line = LineEdit::new("abcdefghij");
        // The cursor takes the last of 5 cells.
        assert_eq!(line.window(5), ("ghij".to_string(), 4));
        press(&mut line, KeyCode::Home);
        assert_eq!(line.window(5), ("abcde".to_string(), 0));
        for _ in 0..7 {
            press(&mut line, KeyCode::Right);
        }
        assert_eq!(line.window(5), ("defgh".to_string(), 4));
    }

    #[test]
    fn wide_characters_scroll_by_their_width() {
        let line = LineEdit::new("漢字かな");
        // 8 cells of text; 5 hold two characters and the cursor.
        assert_eq!(line.window(5), ("かな".to_string(), 4));
        let mut line = LineEdit::new("a漢字");
        press(&mut line, KeyCode::Home);
        // `字` would only half fit after `a漢`.
        assert_eq!(line.window(4), ("a漢".to_string(), 0));
        assert_eq!(LineEdit::new("abc").window(0), (String::new(), 0));
    }
}
