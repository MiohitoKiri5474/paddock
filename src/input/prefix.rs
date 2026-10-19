//! The prefix key (`C-a` unless configured): the key after it is a paddock
//! command instead of input for the terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What paddock does for a command key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewTab,
    NextTab,
    PreviousTab,
    /// Go to tab N (0-based).
    GoToTab(usize),
    RenameTab,
    /// End the focused terminal; its tab goes with its last terminal.
    Close,
    /// Split the focused terminal side by side.
    SplitColumns,
    /// Split the focused terminal top and bottom.
    SplitRows,
    Focus(Direction),
    Equalise,
    Help,
    Detach,
}

/// A direction to move focus in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Send this key to the terminal.
    Forward(KeyEvent),
    Command(Command),
    /// Consumed by paddock (the prefix itself, or an unbound command key).
    None,
}

/// Command keys, for both dispatch and the key help.
pub const BINDINGS: &[(&str, &str)] = &[
    ("c", "new shell tab"),
    ("n / p", "next / previous tab"),
    ("1-9", "go to tab"),
    (",", "rename tab"),
    ("|", "split side by side"),
    ("-", "split top and bottom"),
    ("hjkl", "move focus (also arrows)"),
    ("=", "equalise split sizes"),
    ("x", "close terminal (ends it)"),
    ("d", "detach: quit, terminals keep running"),
    ("?", "this help"),
];

fn command(key: &KeyEvent) -> Option<Command> {
    // Shift is part of characters like `?`; any other modifier is not a command.
    if !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    let c = match key.code {
        KeyCode::Char(c) => c,
        KeyCode::Left => 'h',
        KeyCode::Down => 'j',
        KeyCode::Up => 'k',
        KeyCode::Right => 'l',
        _ => return None,
    };
    Some(match c {
        'c' => Command::NewTab,
        'n' => Command::NextTab,
        'p' => Command::PreviousTab,
        '1'..='9' => Command::GoToTab(c as usize - '1' as usize),
        ',' => Command::RenameTab,
        'x' => Command::Close,
        '|' => Command::SplitColumns,
        '-' => Command::SplitRows,
        'h' => Command::Focus(Direction::Left),
        'j' => Command::Focus(Direction::Down),
        'k' => Command::Focus(Direction::Up),
        'l' => Command::Focus(Direction::Right),
        '=' => Command::Equalise,
        '?' => Command::Help,
        'd' => Command::Detach,
        _ => return None,
    })
}

pub struct PrefixKeys {
    prefix: KeyEvent,
    armed: bool,
}

impl PrefixKeys {
    pub fn new(prefix: KeyEvent) -> Self {
        Self {
            prefix,
            armed: false,
        }
    }

    /// Whether the next key will be read as a command.
    pub fn armed(&self) -> bool {
        self.armed
    }

    fn is_prefix(&self, k: &KeyEvent) -> bool {
        k.code == self.prefix.code && k.modifiers == self.prefix.modifiers
    }

    pub fn handle(&mut self, key: KeyEvent) -> KeyAction {
        if !self.armed {
            if self.is_prefix(&key) {
                self.armed = true;
                return KeyAction::None;
            }
            return KeyAction::Forward(key);
        }
        self.armed = false;
        if self.is_prefix(&key) {
            // Pressing the prefix twice sends it literally.
            return KeyAction::Forward(key);
        }
        command(&key).map_or(KeyAction::None, KeyAction::Command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }

    fn ctrl_a() -> PrefixKeys {
        PrefixKeys::new(k('a', KeyModifiers::CONTROL))
    }

    fn after_prefix(c: char, m: KeyModifiers) -> KeyAction {
        let mut p = ctrl_a();
        assert_eq!(p.handle(k('a', KeyModifiers::CONTROL)), KeyAction::None);
        assert!(p.armed());
        let action = p.handle(k(c, m));
        assert!(!p.armed());
        action
    }

    #[test]
    fn plain_keys_pass_through() {
        let mut p = ctrl_a();
        assert_eq!(
            p.handle(k('d', KeyModifiers::NONE)),
            KeyAction::Forward(k('d', KeyModifiers::NONE))
        );
    }

    #[test]
    fn command_keys_after_the_prefix() {
        let none = KeyModifiers::NONE;
        assert_eq!(after_prefix('c', none), KeyAction::Command(Command::NewTab));
        assert_eq!(
            after_prefix('n', none),
            KeyAction::Command(Command::NextTab)
        );
        assert_eq!(
            after_prefix('p', none),
            KeyAction::Command(Command::PreviousTab)
        );
        assert_eq!(
            after_prefix('1', none),
            KeyAction::Command(Command::GoToTab(0))
        );
        assert_eq!(
            after_prefix('9', none),
            KeyAction::Command(Command::GoToTab(8))
        );
        assert_eq!(
            after_prefix(',', none),
            KeyAction::Command(Command::RenameTab)
        );
        assert_eq!(after_prefix('x', none), KeyAction::Command(Command::Close));
        assert_eq!(
            after_prefix('|', KeyModifiers::SHIFT),
            KeyAction::Command(Command::SplitColumns)
        );
        assert_eq!(
            after_prefix('-', none),
            KeyAction::Command(Command::SplitRows)
        );
        assert_eq!(
            after_prefix('=', none),
            KeyAction::Command(Command::Equalise)
        );
        for (c, d) in [
            ('h', Direction::Left),
            ('j', Direction::Down),
            ('k', Direction::Up),
            ('l', Direction::Right),
        ] {
            assert_eq!(after_prefix(c, none), KeyAction::Command(Command::Focus(d)));
        }
        assert_eq!(
            after_prefix('?', KeyModifiers::SHIFT),
            KeyAction::Command(Command::Help)
        );
        assert_eq!(after_prefix('d', none), KeyAction::Command(Command::Detach));
    }

    #[test]
    fn arrows_after_the_prefix_move_focus() {
        let mut p = ctrl_a();
        for (code, d) in [
            (KeyCode::Left, Direction::Left),
            (KeyCode::Down, Direction::Down),
            (KeyCode::Up, Direction::Up),
            (KeyCode::Right, Direction::Right),
        ] {
            p.handle(k('a', KeyModifiers::CONTROL));
            assert_eq!(
                p.handle(KeyEvent::new(code, KeyModifiers::NONE)),
                KeyAction::Command(Command::Focus(d))
            );
        }
    }

    #[test]
    fn unbound_and_modified_keys_are_swallowed() {
        assert_eq!(after_prefix('z', KeyModifiers::NONE), KeyAction::None);
        assert_eq!(after_prefix('0', KeyModifiers::NONE), KeyAction::None);
        assert_eq!(after_prefix('c', KeyModifiers::CONTROL), KeyAction::None);
    }

    #[test]
    fn prefix_twice_sends_it_literally() {
        assert_eq!(
            after_prefix('a', KeyModifiers::CONTROL),
            KeyAction::Forward(k('a', KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn any_prefix_key_works() {
        let mut p = PrefixKeys::new(k('b', KeyModifiers::CONTROL));
        assert_eq!(
            p.handle(k('a', KeyModifiers::CONTROL)),
            KeyAction::Forward(k('a', KeyModifiers::CONTROL))
        );
        assert_eq!(p.handle(k('b', KeyModifiers::CONTROL)), KeyAction::None);
        assert_eq!(
            p.handle(k('c', KeyModifiers::NONE)),
            KeyAction::Command(Command::NewTab)
        );
    }
}
