//! A key as written in `config.toml`: `c`, `Enter`, `F1`, `C-n`, `S-Tab`.
//! The prefix and every binding use this one grammar.

use anyhow::{bail, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key as bound: its code and Ctrl, Alt and Shift, with Shift folded into
/// characters (`C`, `?`), a Ctrl letter always lowercase (terminals send
/// `C-a` and `C-A` alike), and `S-Tab` (back-tab) as Tab with Shift.
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

/// Modifiers as written before a key (either case), in the order written.
const MODIFIERS: &[(char, KeyModifiers)] = &[
    ('C', KeyModifiers::CONTROL),
    ('M', KeyModifiers::ALT),
    ('S', KeyModifiers::SHIFT),
];

/// The highest function key.
const MAX_F: u8 = 12;

const SYNTAX: &str = "expected a character, a name such as Enter, Tab, Esc, Space, Left \
                      or F1, optionally after C- (Ctrl), S- (Shift) or M- (Alt)";

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

    /// The key without the modifiers in `ignored`.
    pub fn without(self, ignored: KeyModifiers) -> Self {
        Self::new(self.code, self.mods - ignored)
    }

    /// Look the key up with `find`: as it is, else without the modifiers
    /// in `ignored` (so `S-Left` reaches what `Left` is bound to).
    pub fn lookup<T>(
        self,
        ignored: KeyModifiers,
        mut find: impl FnMut(Self) -> Option<T>,
    ) -> Option<T> {
        find(self).or_else(|| find(self.without(ignored)))
    }

    fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match code {
            KeyCode::Char(c) if ctrl => Self {
                code: KeyCode::Char(c.to_ascii_lowercase()),
                mods: mods - KeyModifiers::SHIFT,
            },
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
        let (mods, rest) = modifiers(spec);
        let Some(code) = code_of(rest) else {
            bail!("invalid key {spec:?}: {SYNTAX}");
        };
        if let KeyCode::Char(c) = code {
            let (ctrl, shift) = (
                mods.contains(KeyModifiers::CONTROL),
                mods.contains(KeyModifiers::SHIFT),
            );
            if shift && ctrl {
                bail!("invalid key {spec:?}: terminals send Ctrl with Shift on a character as plain Ctrl; drop the S-");
            }
            if shift && !c.is_ascii_alphabetic() {
                bail!("invalid key {spec:?}: S- goes with a letter or a named key; write the shifted character itself (e.g. \"!\", not \"S-1\")");
            }
        }
        Ok(Self::new(code, mods))
    }

    /// Parse the prefix key: `C-` and a letter, `Space`, or one of `\ ] ^ _`.
    /// Plain keys would be unusable for typing, and paddock does not rely on
    /// Alt (design: any terminal app).
    pub fn parse_prefix(spec: &str) -> Result<Self> {
        let invalid = || {
            format!("invalid prefix key {spec:?}: expected C- followed by a letter, Space, or one of \\ ] ^ _ (for example \"C-a\")")
        };
        let key = Self::parse(spec).map_err(|_| anyhow::anyhow!(invalid()))?;
        let usable =
            matches!(key.code, KeyCode::Char(c) if c.is_ascii_lowercase() || " \\]^_".contains(c));
        if key.mods != KeyModifiers::CONTROL || !usable {
            bail!(invalid());
        }
        Ok(key)
    }

    /// How the key is written, e.g. `C-a`, `Enter`, `|`; parses back to it.
    pub fn name(&self) -> String {
        let mods: String = MODIFIERS
            .iter()
            .filter(|(_, m)| self.mods.contains(*m))
            .map(|(p, _)| format!("{p}-"))
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

/// The modifiers written before the key in `spec`, and the rest.
fn modifiers(spec: &str) -> (KeyModifiers, &str) {
    let mut mods = KeyModifiers::NONE;
    let mut rest = spec;
    loop {
        let mut chars = rest.chars();
        let (Some(p), Some('-')) = (chars.next(), chars.next()) else {
            return (mods, rest);
        };
        let found = MODIFIERS.iter().find(|(m, _)| m.eq_ignore_ascii_case(&p));
        match found {
            // `C-` alone is not a modifier: the key is missing.
            Some((_, m)) if rest.len() > 2 => {
                mods |= *m;
                rest = &rest[2..];
            }
            _ => return (mods, rest),
        }
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
    let digits = text.strip_prefix(['F', 'f'])?;
    if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u8 = digits.parse().ok()?;
    (1..=MAX_F).contains(&n).then_some(KeyCode::F(n))
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
        let ctrl = KeyModifiers::CONTROL;
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
            ("C-n", KeyCode::Char('n'), ctrl),
            ("c-n", KeyCode::Char('n'), ctrl),
            ("C-A", KeyCode::Char('a'), ctrl),
            ("C--", KeyCode::Char('-'), ctrl),
            ("S-a", KeyCode::Char('A'), none),
            ("s-a", KeyCode::Char('A'), none),
            ("S-Tab", KeyCode::Tab, KeyModifiers::SHIFT),
            ("M-x", KeyCode::Char('x'), KeyModifiers::ALT),
            ("m-x", KeyCode::Char('x'), KeyModifiers::ALT),
            ("C-S-Left", KeyCode::Left, ctrl | KeyModifiers::SHIFT),
        ];
        for (spec, code, mods) in cases {
            assert_eq!(Key::parse(spec).unwrap(), Key { code, mods }, "{spec}");
        }
    }

    #[test]
    fn rejects_keys_it_cannot_read() {
        for bad in [
            "", "ab", "C-", "Ctrl+a", "F0", "F13", "F01", "F+1", "Hyper-a", "C-Foo", "X-a",
        ] {
            let err = Key::parse(bad).unwrap_err().to_string();
            assert!(err.contains("invalid key"), "{bad}: {err}");
        }
    }

    #[test]
    fn rejects_shift_where_terminals_cannot_send_it() {
        let err = Key::parse("S-1").unwrap_err().to_string();
        assert!(err.contains("not \"S-1\""), "{err}");
        let err = Key::parse("C-S-a").unwrap_err().to_string();
        assert!(err.contains("drop the S-"), "{err}");
    }

    #[test]
    fn a_key_event_matches_its_written_key() {
        let shift = KeyModifiers::SHIFT;
        let ctrl = KeyModifiers::CONTROL;
        let of = |c, m| Key::of(&key(c, m));
        assert_eq!(of(KeyCode::Char('C'), shift), Key::parse("C").unwrap());
        assert_eq!(of(KeyCode::Char('?'), shift), Key::parse("?").unwrap());
        assert_eq!(of(KeyCode::BackTab, shift), Key::parse("S-Tab").unwrap());
        assert_eq!(of(KeyCode::Char('n'), ctrl), Key::parse("C-n").unwrap());
        assert_eq!(
            of(KeyCode::Char('N'), ctrl | shift),
            Key::parse("C-n").unwrap()
        );
    }

    #[test]
    fn without_drops_modifiers() {
        let shift_left = Key::of(&key(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(
            shift_left.without(KeyModifiers::SHIFT),
            Key::parse("Left").unwrap()
        );
        let ctrl_j = Key::parse("C-j").unwrap();
        assert_eq!(
            ctrl_j.without(KeyModifiers::CONTROL),
            Key::parse("j").unwrap()
        );
    }

    #[test]
    fn the_prefix_is_a_ctrl_key() {
        let ctrl = |c| Key {
            code: KeyCode::Char(c),
            mods: KeyModifiers::CONTROL,
        };
        assert_eq!(Key::parse_prefix("C-b").unwrap(), ctrl('b'));
        assert_eq!(Key::parse_prefix("C-A").unwrap(), ctrl('a'));
        assert_eq!(Key::parse_prefix("c-a").unwrap(), ctrl('a'));
        assert_eq!(Key::parse_prefix("C-Space").unwrap(), ctrl(' '));
        assert_eq!(Key::parse_prefix("C-]").unwrap(), ctrl(']'));
        for bad in [
            "a", "C-", "C-ab", "M-a", "C-1", "ctrl-a", "C-M-a", "C-Left", "F1",
        ] {
            let err = Key::parse_prefix(bad).unwrap_err().to_string();
            assert!(err.contains("invalid prefix key"), "{bad}: {err}");
        }
        assert_eq!(ctrl(' ').name(), "C-Space");
    }
}
