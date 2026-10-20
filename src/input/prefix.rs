//! The prefix key (`C-a` unless configured): the key after it is a paddock
//! command instead of input for the terminal.

use super::keymap::Keys;
use crossterm::event::KeyEvent;

/// What paddock does for a command key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// A new tab (tmux window) in the current space.
    NewTab,
    /// A new space (tmux session).
    NewSpace,
    NextTab,
    PreviousTab,
    /// Go to tab N (0-based).
    GoToTab(usize),
    RenameTab,
    RenameSpace,
    /// End the focused pane; its tab goes with its last pane, its space with
    /// its last tab.
    Close,
    /// Continue a copy of the focused session in a new tab of its space.
    Fork,
    /// Split the focused terminal side by side.
    SplitColumns,
    /// Split the focused terminal top and bottom.
    SplitRows,
    Focus(Direction),
    Equalise,
    Help,
    Detach,
    FocusSidebar,
    /// Focus the sidebar on the current space.
    FocusSpaces,
    /// Move the current tab one place left (`-1`) or right (`1`).
    MoveTab(isize),
    /// Make the sidebar this many columns wider (narrower if negative).
    ResizeSidebar(i16),
    /// Show the space and tab shown before the current ones.
    LastPage,
    ToggleSidebar,
    /// Jump to the next session that is waiting or idle.
    NextAttention,
    /// Open or close the dashboard.
    Dashboard,
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

/// Columns `C-a {` and `C-a }` take away or add.
pub const SIDEBAR_STEP: i16 = 2;

pub struct PrefixKeys {
    prefix: KeyEvent,
    commands: Keys<Command>,
    armed: bool,
}

impl PrefixKeys {
    pub fn new(prefix: KeyEvent, commands: Keys<Command>) -> Self {
        Self {
            prefix,
            commands,
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
        self.commands
            .get(&key)
            .map_or(KeyAction::None, KeyAction::Command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keymap::COMMANDS;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn k(c: char, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }

    fn ctrl_a() -> PrefixKeys {
        PrefixKeys::new(k('a', KeyModifiers::CONTROL), Keys::defaults(COMMANDS))
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
            after_prefix('C', KeyModifiers::SHIFT),
            KeyAction::Command(Command::NewSpace)
        );
        assert_eq!(
            after_prefix('$', KeyModifiers::SHIFT),
            KeyAction::Command(Command::RenameSpace)
        );
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
        assert_eq!(after_prefix('f', none), KeyAction::Command(Command::Fork));
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
        assert_eq!(
            after_prefix('s', none),
            KeyAction::Command(Command::FocusSidebar)
        );
        assert_eq!(
            after_prefix('<', KeyModifiers::SHIFT),
            KeyAction::Command(Command::MoveTab(-1))
        );
        assert_eq!(
            after_prefix('>', KeyModifiers::SHIFT),
            KeyAction::Command(Command::MoveTab(1))
        );
        assert_eq!(
            after_prefix('{', KeyModifiers::SHIFT),
            KeyAction::Command(Command::ResizeSidebar(-SIDEBAR_STEP))
        );
        assert_eq!(
            after_prefix('}', KeyModifiers::SHIFT),
            KeyAction::Command(Command::ResizeSidebar(SIDEBAR_STEP))
        );
        assert_eq!(
            after_prefix('w', none),
            KeyAction::Command(Command::FocusSpaces)
        );
        assert_eq!(
            after_prefix('b', none),
            KeyAction::Command(Command::ToggleSidebar)
        );
        assert_eq!(
            after_prefix('a', none),
            KeyAction::Command(Command::NextAttention)
        );
        assert_eq!(
            after_prefix('o', none),
            KeyAction::Command(Command::Dashboard)
        );
    }

    #[test]
    fn tab_after_the_prefix_shows_the_last_page() {
        let mut p = ctrl_a();
        p.handle(k('a', KeyModifiers::CONTROL));
        assert_eq!(
            p.handle(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            KeyAction::Command(Command::LastPage)
        );
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
        let mut p = PrefixKeys::new(k('b', KeyModifiers::CONTROL), Keys::defaults(COMMANDS));
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
