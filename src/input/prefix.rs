//! The prefix key (`C-a` unless configured): the key after it is a paddock
//! command instead of input for the terminal. Direct chords (`C-l`) are
//! commands without it.

use super::keymap::{Key, Keys};
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
    /// A new worktree of the focused terminal's repository, as a new space.
    NewWorktree,
    /// One of the focused terminal's repository's worktrees, as a new space.
    OpenWorktree,
    /// Delete the current space's worktree and branch, closing the space.
    DeleteWorktree,
    /// Split the focused terminal side by side.
    SplitColumns,
    /// Split the focused terminal top and bottom.
    SplitRows,
    Focus(Direction),
    Equalise,
    Help,
    Detach,
    /// Focus the sidebar on the agent in the focused terminal, if it is
    /// one, else on the current space.
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
    /// Open the finder: a box to pick a space, tab or agent by typing.
    Finder,
    /// Show or hide float N (0-based, in `[floats]` order).
    ToggleFloat(usize),
}

impl Command {
    /// Whether a float shown stays shown through this command: those that
    /// change neither the page under it nor who has the keyboard, and the
    /// finder, which draws over it and hides it only when a row is picked.
    pub fn keeps_float(self) -> bool {
        matches!(
            self,
            Command::Help
                | Command::Detach
                | Command::ToggleSidebar
                | Command::ResizeSidebar(_)
                | Command::ToggleFloat(_)
                | Command::Finder
        )
    }
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
    prefix: Key,
    commands: Keys<Command>,
    /// Commands on one chord, without the prefix.
    direct: Keys<Command>,
    /// Each float's key, in `[floats]` order.
    floats: Vec<Key>,
    armed: bool,
}

impl PrefixKeys {
    /// The prefix `prefix`, then one of `commands`' keys or a float's key
    /// from `floats`; or one of `direct`'s keys on its own.
    pub fn new(
        prefix: Key,
        commands: Keys<Command>,
        direct: Keys<Command>,
        floats: Vec<Key>,
    ) -> Self {
        Self {
            prefix,
            commands,
            direct,
            floats,
            armed: false,
        }
    }

    /// The command `key` is a direct chord of, if any. For a key the
    /// prefix did not arm: the caller decides where it would go else (a
    /// float gets every key but the prefix).
    pub fn direct(&self, key: &KeyEvent) -> Option<Command> {
        self.direct.get(key).map(|(c, i)| nth_key_command(c, i))
    }

    /// The float whose key `key` is: exactly, else, as for commands,
    /// without Shift.
    fn float(&self, key: &KeyEvent) -> Option<usize> {
        Key::of(key).lookup(self.commands.ignored(), |k| {
            self.floats.iter().position(|f| *f == k)
        })
    }

    /// Whether the next key will be read as a command.
    pub fn armed(&self) -> bool {
        self.armed
    }

    fn is_prefix(&self, k: &KeyEvent) -> bool {
        Key::of(k) == self.prefix
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
        let Some((command, i)) = self.commands.get(&key) else {
            return self.float(&key).map_or(KeyAction::None, |f| {
                KeyAction::Command(Command::ToggleFloat(f))
            });
        };
        KeyAction::Command(nth_key_command(command, i))
    }
}

/// What the `i`th key bound to `command` runs: `command` itself, except
/// that `go_to_tab`'s Nth key goes to tab N.
fn nth_key_command(command: Command, i: usize) -> Command {
    if let Command::GoToTab(_) = command {
        Command::GoToTab(i)
    } else {
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keymap::{given, COMMAND_KEYS, DIRECT_KEYS};
    use crossterm::event::{KeyCode, KeyModifiers};

    fn k(c: char, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), m)
    }

    fn ctrl_a() -> PrefixKeys {
        PrefixKeys::new(
            Key::parse("C-a").unwrap(),
            Keys::defaults(&COMMAND_KEYS),
            Keys::defaults(&DIRECT_KEYS),
            ["G", "C-y", "F5"]
                .iter()
                .map(|k| Key::parse(k).unwrap())
                .collect(),
        )
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
            after_prefix('W', KeyModifiers::SHIFT),
            KeyAction::Command(Command::NewWorktree)
        );
        assert_eq!(
            after_prefix('O', KeyModifiers::SHIFT),
            KeyAction::Command(Command::OpenWorktree)
        );
        assert_eq!(after_prefix('X', KeyModifiers::SHIFT), KeyAction::None);
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
        assert_eq!(after_prefix('g', none), KeyAction::Command(Command::Finder));
    }

    #[test]
    fn float_keys_after_the_prefix_toggle_their_float() {
        let none = KeyModifiers::NONE;
        assert_eq!(
            after_prefix('G', KeyModifiers::SHIFT),
            KeyAction::Command(Command::ToggleFloat(0))
        );
        assert_eq!(
            after_prefix('y', KeyModifiers::CONTROL),
            KeyAction::Command(Command::ToggleFloat(1))
        );
        // As for commands, Shift does not matter for a named key, but `g`
        // is another key than `G` (it opens the finder), and Ctrl matters.
        let mut p = ctrl_a();
        p.handle(k('a', KeyModifiers::CONTROL));
        assert_eq!(
            p.handle(KeyEvent::new(KeyCode::F(5), KeyModifiers::SHIFT)),
            KeyAction::Command(Command::ToggleFloat(2))
        );
        assert_eq!(after_prefix('y', none), KeyAction::None);
        // Without the prefix, a float's key is typed.
        let mut p = ctrl_a();
        assert_eq!(
            p.handle(k('G', KeyModifiers::SHIFT)),
            KeyAction::Forward(k('G', KeyModifiers::SHIFT))
        );
    }

    #[test]
    fn only_commands_that_keep_the_page_and_the_keyboard_keep_a_float() {
        for c in [
            Command::Help,
            Command::ToggleSidebar,
            Command::ToggleFloat(1),
            Command::Finder,
        ] {
            assert!(c.keeps_float(), "{c:?}");
        }
        for c in [
            Command::NextTab,
            Command::GoToTab(0),
            Command::LastPage,
            Command::NewTab,
            Command::Dashboard,
            Command::FocusSidebar,
            Command::NextAttention,
            Command::Close,
        ] {
            assert!(!c.keeps_float(), "{c:?}");
        }
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
    fn shift_arrows_after_the_prefix_move_focus_too() {
        let mut p = ctrl_a();
        p.handle(k('a', KeyModifiers::CONTROL));
        assert_eq!(
            p.handle(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)),
            KeyAction::Command(Command::Focus(Direction::Right))
        );
    }

    #[test]
    fn the_prefix_matches_however_shift_is_reported() {
        let mut p = ctrl_a();
        let ctrl_shift_a = k('A', KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(p.handle(ctrl_shift_a), KeyAction::None);
        assert!(p.armed());
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
    fn direct_chords_are_commands_without_the_prefix() {
        let p = ctrl_a();
        let ctrl = KeyModifiers::CONTROL;
        let ctrl_alt = ctrl | KeyModifiers::ALT;
        assert_eq!(
            p.direct(&k('l', ctrl)),
            Some(Command::Focus(Direction::Right))
        );
        assert_eq!(p.direct(&k('c', ctrl_alt)), Some(Command::NewTab));
        // Cycling tabs has no chord by default.
        assert_eq!(p.direct(&k(']', ctrl_alt)), None);
        // A terminal without the kitty protocol reports Ctrl+- as Ctrl+7.
        assert_eq!(p.direct(&k('7', ctrl)), Some(Command::SplitRows));
        assert_eq!(p.direct(&k('\\', ctrl)), Some(Command::SplitColumns));
        // Only the chord as bound: not the command key, not with Shift.
        assert_eq!(p.direct(&k('l', KeyModifiers::NONE)), None);
        assert_eq!(p.direct(&k('c', ctrl)), None);
        let shift_right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        let right_only = given(&[("focus_right", &["Right"])]);
        let (keys, _) =
            Keys::configured(&DIRECT_KEYS, &right_only, Key::parse("C-a").unwrap()).unwrap();
        let p = PrefixKeys::new(
            Key::parse("C-a").unwrap(),
            Keys::defaults(&COMMAND_KEYS),
            keys,
            Vec::new(),
        );
        assert_eq!(p.direct(&shift_right), None);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(p.direct(&right), Some(Command::Focus(Direction::Right)));
        // `handle` is untouched by them: it still forwards the key.
        let mut p = ctrl_a();
        assert_eq!(p.handle(k('l', ctrl)), KeyAction::Forward(k('l', ctrl)));
    }

    #[test]
    fn any_prefix_key_works() {
        let mut p = PrefixKeys::new(
            Key::parse("C-b").unwrap(),
            Keys::defaults(&COMMAND_KEYS),
            Keys::defaults(&DIRECT_KEYS),
            Vec::new(),
        );
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
