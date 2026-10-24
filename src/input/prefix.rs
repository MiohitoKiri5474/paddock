//! The prefix key (`C-a` unless configured): the key after it is a paddock
//! command instead of input for the terminal. Direct chords (`C-l`) are
//! commands without it.

use super::keymap::{Key, Keys};
use crossterm::event::KeyEvent;
use std::time::{Duration, Instant};

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
    /// Make the focused terminal's directory the current space's root.
    SetRoot,
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
    /// Move the focused split's border this way by [`RESIZE_STEP`]. Its
    /// keys work again without the prefix for a while (`[keys] repeat_ms`).
    Resize(Direction),
    /// Make the focused split fill its tab, or give the others their
    /// places back.
    Zoom,
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
    /// Copy mode in the terminal typing goes to (the float shown, else the
    /// focused pane): move, select and copy over its history by keyboard.
    CopyMode,
    /// Copy mode with the search prompt open.
    Search,
}

impl Command {
    /// Whether this command's keys work again without the prefix for a
    /// while after it ran from the prefix, like tmux's `bind -r`.
    pub fn repeats(self) -> bool {
        matches!(self, Command::Resize(_))
    }

    /// Whether a float shown stays shown through this command: those that
    /// change neither the page under it nor who has the keyboard, and the
    /// finder, which draws over it and hides it only when a row is picked,
    /// and copy mode, which works in the float's pane.
    pub fn keeps_float(self) -> bool {
        matches!(
            self,
            Command::Help
                | Command::Detach
                | Command::ToggleSidebar
                | Command::ResizeSidebar(_)
                | Command::ToggleFloat(_)
                | Command::Finder
                | Command::CopyMode
                | Command::Search
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

/// Columns (or rows) `C-a H/J/K/L` move a split's border by.
pub const RESIZE_STEP: u16 = 2;

pub struct PrefixKeys {
    prefix: Key,
    commands: Keys<Command>,
    /// Commands on one chord, without the prefix.
    direct: Keys<Command>,
    /// Each float's key, in `[floats]` order.
    floats: Vec<Key>,
    armed: bool,
    /// How long a repeating command's keys work without the prefix after
    /// it ran; zero turns that off.
    repeat: Duration,
    /// Until when they do, after the last one.
    repeating_until: Option<Instant>,
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
            repeat: Duration::ZERO,
            repeating_until: None,
        }
    }

    /// Let a repeating command's keys work without the prefix for
    /// `repeat` after it ran (never, by default).
    pub fn with_repeat(self, repeat: Duration) -> Self {
        Self { repeat, ..self }
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
        self.handle_at(key, Instant::now())
    }

    /// [`Self::handle`] for a key read at `now`. Within the repeat window,
    /// a repeating command's key runs it again and keeps the window open;
    /// any other key closes the window and is read as usual.
    fn handle_at(&mut self, key: KeyEvent, now: Instant) -> KeyAction {
        if let Some(until) = self.repeating_until.take() {
            let again = self.commands.get(&key).map(|(c, i)| nth_key_command(c, i));
            if let Some(command) = again.filter(|c| now < until && c.repeats()) {
                self.repeating_until = Some(now + self.repeat);
                return KeyAction::Command(command);
            }
        }
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
        let command = nth_key_command(command, i);
        if command.repeats() && !self.repeat.is_zero() {
            self.repeating_until = Some(now + self.repeat);
        }
        KeyAction::Command(command)
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
        assert_eq!(after_prefix('z', none), KeyAction::Command(Command::Zoom));
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
    fn shifted_h_j_k_l_after_the_prefix_resize_the_split() {
        for (c, d) in [
            ('H', Direction::Left),
            ('J', Direction::Down),
            ('K', Direction::Up),
            ('L', Direction::Right),
        ] {
            assert_eq!(
                after_prefix(c, KeyModifiers::SHIFT),
                KeyAction::Command(Command::Resize(d))
            );
        }
        assert!(Command::Resize(Direction::Up).repeats());
        assert!(!Command::Focus(Direction::Up).repeats());
        assert!(!Command::Resize(Direction::Up).keeps_float());
    }

    /// `C-a` then `key` with a 500 ms repeat window, at `now`.
    fn repeating(now: Instant, key: KeyEvent) -> (PrefixKeys, KeyAction) {
        let mut p = ctrl_a().with_repeat(Duration::from_millis(500));
        assert_eq!(
            p.handle_at(k('a', KeyModifiers::CONTROL), now),
            KeyAction::None
        );
        let action = p.handle_at(key, now);
        (p, action)
    }

    #[test]
    fn resize_keys_repeat_without_the_prefix_within_the_window() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let shift = KeyModifiers::SHIFT;
        let left = KeyAction::Command(Command::Resize(Direction::Left));
        let (mut p, first) = repeating(t0, k('H', shift));
        assert_eq!(first, left);
        assert!(!p.armed());
        assert_eq!(p.handle_at(k('H', shift), t0 + ms(400)), left);
        // Each repeat opens the window again, and any resize key repeats.
        assert_eq!(
            p.handle_at(k('K', shift), t0 + ms(800)),
            KeyAction::Command(Command::Resize(Direction::Up))
        );
        // Past the window, the key is typed again.
        assert_eq!(
            p.handle_at(k('H', shift), t0 + ms(1400)),
            KeyAction::Forward(k('H', shift))
        );
        assert_eq!(
            p.handle_at(k('H', shift), t0 + ms(1500)),
            KeyAction::Forward(k('H', shift))
        );
    }

    #[test]
    fn another_key_in_the_repeat_window_ends_it_and_is_read_as_usual() {
        let t0 = Instant::now();
        let shift = KeyModifiers::SHIFT;
        // `l` is a command after the prefix, but typed here.
        let (mut p, _) = repeating(t0, k('L', shift));
        let l = k('l', KeyModifiers::NONE);
        assert_eq!(p.handle_at(l, t0), KeyAction::Forward(l));
        assert_eq!(
            p.handle_at(k('L', shift), t0),
            KeyAction::Forward(k('L', shift))
        );
        // The prefix arms as usual.
        let (mut p, _) = repeating(t0, k('L', shift));
        assert_eq!(
            p.handle_at(k('a', KeyModifiers::CONTROL), t0),
            KeyAction::None
        );
        assert!(p.armed());
        assert_eq!(
            p.handle_at(k('H', shift), t0),
            KeyAction::Command(Command::Resize(Direction::Left))
        );
        // Commands that do not repeat open no window.
        let (mut p, _) = repeating(t0, k('h', KeyModifiers::NONE));
        assert_eq!(
            p.handle_at(k('H', shift), t0),
            KeyAction::Forward(k('H', shift))
        );
    }

    #[test]
    fn a_zero_repeat_window_or_a_direct_chord_never_repeats() {
        let t0 = Instant::now();
        let shift = KeyModifiers::SHIFT;
        let mut p = ctrl_a();
        p.handle_at(k('a', KeyModifiers::CONTROL), t0);
        assert_eq!(
            p.handle_at(k('H', shift), t0),
            KeyAction::Command(Command::Resize(Direction::Left))
        );
        assert_eq!(
            p.handle_at(k('H', shift), t0),
            KeyAction::Forward(k('H', shift))
        );
        // A direct chord is looked up apart from `handle`, so it opens no
        // window either.
        let resize_chord = given(&[("resize_left", &["C-M-h"])]);
        let (direct, _) =
            Keys::configured(&DIRECT_KEYS, &resize_chord, Key::parse("C-a").unwrap()).unwrap();
        let mut p = PrefixKeys::new(
            Key::parse("C-a").unwrap(),
            Keys::defaults(&COMMAND_KEYS),
            direct,
            Vec::new(),
        )
        .with_repeat(Duration::from_millis(500));
        let chord = k('h', KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert_eq!(p.direct(&chord), Some(Command::Resize(Direction::Left)));
        assert_eq!(
            p.handle_at(k('H', shift), t0),
            KeyAction::Forward(k('H', shift))
        );
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
            Command::CopyMode,
            Command::Search,
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
        assert_eq!(after_prefix('m', KeyModifiers::NONE), KeyAction::None);
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
