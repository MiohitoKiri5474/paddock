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
    CloseTab,
    Help,
    Detach,
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
    ("x", "close tab (ends its terminal)"),
    ("d", "detach: quit, terminals keep running"),
    ("?", "this help"),
];

fn command(key: &KeyEvent) -> Option<Command> {
    // Shift is part of characters like `?`; any other modifier is not a command.
    if !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    Some(match c {
        'c' => Command::NewTab,
        'n' => Command::NextTab,
        'p' => Command::PreviousTab,
        '1'..='9' => Command::GoToTab(c as usize - '1' as usize),
        ',' => Command::RenameTab,
        'x' => Command::CloseTab,
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
        assert_eq!(
            after_prefix('x', none),
            KeyAction::Command(Command::CloseTab)
        );
        assert_eq!(
            after_prefix('?', KeyModifiers::SHIFT),
            KeyAction::Command(Command::Help)
        );
        assert_eq!(after_prefix('d', none), KeyAction::Command(Command::Detach));
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
