//! The prefix key (`C-a`): the key after it is a paddock command instead of
//! input for the terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Send this key to the terminal.
    Forward(KeyEvent),
    /// Leave paddock; terminals keep running.
    Detach,
    /// Consumed by paddock (the prefix itself, or an unbound command key).
    None,
}

pub struct PrefixKeys {
    prefix: KeyEvent,
    armed: bool,
}

impl Default for PrefixKeys {
    fn default() -> Self {
        Self::new(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL))
    }
}

impl PrefixKeys {
    pub fn new(prefix: KeyEvent) -> Self {
        Self {
            prefix,
            armed: false,
        }
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
        match key.code {
            KeyCode::Char('d') if key.modifiers - KeyModifiers::SHIFT == KeyModifiers::NONE => {
                KeyAction::Detach
            }
            _ => KeyAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: char, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }

    #[test]
    fn plain_keys_pass_through() {
        let mut p = PrefixKeys::default();
        assert_eq!(
            p.handle(k('d', KeyModifiers::NONE)),
            KeyAction::Forward(k('d', KeyModifiers::NONE))
        );
    }

    #[test]
    fn prefix_then_d_detaches() {
        let mut p = PrefixKeys::default();
        assert_eq!(p.handle(k('a', KeyModifiers::CONTROL)), KeyAction::None);
        assert_eq!(p.handle(k('d', KeyModifiers::NONE)), KeyAction::Detach);
    }

    #[test]
    fn prefix_twice_sends_it_literally_and_unbound_keys_are_swallowed() {
        let mut p = PrefixKeys::default();
        p.handle(k('a', KeyModifiers::CONTROL));
        assert_eq!(
            p.handle(k('a', KeyModifiers::CONTROL)),
            KeyAction::Forward(k('a', KeyModifiers::CONTROL))
        );
        p.handle(k('a', KeyModifiers::CONTROL));
        assert_eq!(p.handle(k('z', KeyModifiers::NONE)), KeyAction::None);
        assert_eq!(
            p.handle(k('z', KeyModifiers::NONE)),
            KeyAction::Forward(k('z', KeyModifiers::NONE))
        );
    }
}
