//! A key as written in `config.toml`: `c`, `Enter`, `F1`, `C-n`, `S-Tab`.

use anyhow::{bail, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key as bound: its code and Ctrl, Alt and Shift, with Shift folded into
/// characters (`C`, `?`) and `S-Tab` (back-tab) written as Tab with Shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

/// Keys written by name; any other key is a single character or `F1`-`F12`.
const NAMED: &[(&str, KeyCode)] = &[
    ("Space", KeyCode::Char(' ')),
    ("Tab", KeyCode::Tab),
    ("Enter", KeyCode::Enter),
    ("Esc", KeyCode::Esc),
    ("Backspace", KeyCode::Backspace),
    ("Delete", KeyCode::Delete),
    ("Insert", KeyCode::Insert),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("PageUp", KeyCode::PageUp),
    ("PageDown", KeyCode::PageDown),
    ("Left", KeyCode::Left),
    ("Down", KeyCode::Down),
    ("Up", KeyCode::Up),
    ("Right", KeyCode::Right),
];

/// Modifiers as written before a key, in the order they are written.
const MODIFIERS: &[(&str, KeyModifiers)] = &[
    ("C-", KeyModifiers::CONTROL),
    ("M-", KeyModifiers::ALT),
    ("S-", KeyModifiers::SHIFT),
];

impl Key {
    /// The key `event` stands for.
    pub fn of(event: &KeyEvent) -> Self {
        let mods =
            event.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        match event.code {
            KeyCode::BackTab => Self::new(KeyCode::Tab, mods | KeyModifiers::SHIFT),
            code => Self::new(code, mods),
        }
    }

    fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        match code {
            // The character already says whether Shift was down.
            KeyCode::Char(c) if mods.contains(KeyModifiers::SHIFT) => Self {
                code: KeyCode::Char(c.to_ascii_uppercase()),
                mods: mods - KeyModifiers::SHIFT,
            },
            _ => Self { code, mods },
        }
    }

    /// Parse a key such as `c`, `|`, `Enter`, `F1`, `C-n` or `S-Tab`.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = spec;
        while let Some((m, after)) = MODIFIERS
            .iter()
            .find_map(|(p, m)| Some((*m, rest.strip_prefix(p)?)).filter(|(_, a)| !a.is_empty()))
        {
            mods |= m;
            rest = after;
        }
        let Some(code) = code_of(rest) else {
            bail!(
                "invalid key {spec:?}: expected a character, a name such as Enter, Tab, Esc, \
                 Space, Left or F1, optionally after C- (Ctrl), S- (Shift) or M- (Alt)"
            );
        };
        Ok(Self::new(code, mods))
    }

    /// How the key is written, e.g. `C-a`, `Enter`, `|`; parses back to it.
    pub fn name(&self) -> String {
        let mods: String = MODIFIERS
            .iter()
            .filter(|(_, m)| self.mods.contains(*m))
            .map(|(p, _)| *p)
            .collect();
        let base = match self.code {
            KeyCode::F(n) => format!("F{n}"),
            code => match NAMED.iter().find(|(_, c)| *c == code) {
                Some((name, _)) => (*name).to_string(),
                None => match code {
                    KeyCode::Char(c) => c.to_string(),
                    other => format!("{other:?}"),
                },
            },
        };
        format!("{mods}{base}")
    }

    pub fn uses_alt(&self) -> bool {
        self.mods.contains(KeyModifiers::ALT)
    }
}

/// The key code written `text`: a single character, a name, or `F1`-`F12`.
fn code_of(text: &str) -> Option<KeyCode> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c));
    }
    if let Some(&(_, code)) = NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(text)) {
        return Some(code);
    }
    let n: u8 = text.strip_prefix(['F', 'f'])?.parse().ok()?;
    (1..=12).contains(&n).then_some(KeyCode::F(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parses_characters_names_function_keys_and_modifiers() {
        let none = KeyModifiers::NONE;
        let cases = [
            ("c", KeyCode::Char('c'), none),
            ("|", KeyCode::Char('|'), none),
            ("$", KeyCode::Char('$'), none),
            ("{", KeyCode::Char('{'), none),
            ("-", KeyCode::Char('-'), none),
            ("Tab", KeyCode::Tab, none),
            ("enter", KeyCode::Enter, none),
            ("Esc", KeyCode::Esc, none),
            ("Space", KeyCode::Char(' '), none),
            ("Left", KeyCode::Left, none),
            ("F1", KeyCode::F(1), none),
            ("F12", KeyCode::F(12), none),
            ("C-n", KeyCode::Char('n'), KeyModifiers::CONTROL),
            ("C--", KeyCode::Char('-'), KeyModifiers::CONTROL),
            ("S-a", KeyCode::Char('A'), none),
            ("S-Tab", KeyCode::Tab, KeyModifiers::SHIFT),
            ("M-x", KeyCode::Char('x'), KeyModifiers::ALT),
            (
                "C-S-Left",
                KeyCode::Left,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        ];
        for (spec, code, mods) in cases {
            assert_eq!(Key::parse(spec).unwrap(), Key { code, mods }, "{spec}");
        }
    }

    #[test]
    fn rejects_keys_it_cannot_read() {
        for bad in ["", "ab", "C-", "Ctrl+a", "F0", "F13", "Hyper-a", "C-Foo"] {
            let err = Key::parse(bad).unwrap_err().to_string();
            assert!(err.contains("invalid key"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_key_event_matches_its_written_key() {
        let shift = KeyModifiers::SHIFT;
        let of = |c, m| Key::of(&key(c, m));
        assert_eq!(of(KeyCode::Char('C'), shift), Key::parse("C").unwrap());
        assert_eq!(of(KeyCode::Char('?'), shift), Key::parse("?").unwrap());
        assert_eq!(of(KeyCode::BackTab, shift), Key::parse("S-Tab").unwrap());
        assert_eq!(
            of(KeyCode::Char('n'), KeyModifiers::CONTROL),
            Key::parse("C-n").unwrap()
        );
    }
}
