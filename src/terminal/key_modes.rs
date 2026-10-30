//! Keyboard-protocol requests seen in a program's raw output.
//!
//! alacritty_terminal tracks kitty flags itself but not xterm modifyOtherKeys,
//! and after a reattach the tmux backend only remembered modifyOtherKeys
//! (ADR 0005). This scanner tracks both so the key encoder has one source
//! of truth.

/// What the program asked for, as far as key encoding is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModes {
    pub app_cursor: bool,
    pub kitty_flags: u8,
    pub modify_other_keys: u8,
}

const MAX_STACK: usize = 64;
const MAX_PARAMS: usize = 32;

#[derive(Default, Clone, Debug)]
pub struct KeyModeScanner {
    kitty_stack: Vec<u8>,
    modify_other_keys: u8,
    state: State,
    params: Vec<u8>,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
}

impl KeyModeScanner {
    pub fn kitty_flags(&self) -> u8 {
        self.kitty_stack.last().copied().unwrap_or(0)
    }

    pub fn modify_other_keys(&self) -> u8 {
        self.modify_other_keys
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state = match (self.state, b) {
                (_, 0x1b) => State::Escape,
                (State::Escape, b'[') => {
                    self.params.clear();
                    State::Csi
                }
                (State::Csi, 0x40..=0x7e) => {
                    let params = std::mem::take(&mut self.params);
                    self.dispatch(&params, b);
                    State::Ground
                }
                (State::Csi, _) if self.params.len() < MAX_PARAMS => {
                    self.params.push(b);
                    State::Csi
                }
                _ => State::Ground,
            };
        }
    }

    fn dispatch(&mut self, params: &[u8], fin: u8) {
        let (lead, rest) = match params.first() {
            Some(&c @ (b'>' | b'<' | b'=')) => (c, &params[1..]),
            _ => (0, params),
        };
        let nums: Vec<u32> = std::str::from_utf8(rest)
            .unwrap_or("")
            .split(';')
            .map(|s| s.parse().unwrap_or(0))
            .collect();
        let first = nums.first().copied().unwrap_or(0);
        let flags = u8::try_from(first).unwrap_or(u8::MAX);
        match (lead, fin) {
            (b'>', b'u') if self.kitty_stack.len() < MAX_STACK => self.kitty_stack.push(flags),
            (b'<', b'u') => {
                for _ in 0..first.max(1) {
                    self.kitty_stack.pop();
                }
            }
            (b'=', b'u') => {
                let current = self.kitty_flags();
                let new = match nums.get(1).copied().unwrap_or(1) {
                    2 => current | flags,
                    3 => current & !flags,
                    _ => flags,
                };
                match self.kitty_stack.last_mut() {
                    Some(top) => *top = new,
                    None => self.kitty_stack.push(new),
                }
            }
            (b'>', b'm') if first == 4 => {
                self.modify_other_keys = nums.get(1).map_or(0, |&n| u8::try_from(n).unwrap_or(0));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_kitty_push_pop_and_set() {
        let mut s = KeyModeScanner::default();
        s.feed(b"\x1b[>5u");
        assert_eq!(s.kitty_flags(), 5);
        s.feed(b"\x1b[>1u\x1b[<u");
        assert_eq!(s.kitty_flags(), 5);
        s.feed(b"\x1b[=2;2u");
        assert_eq!(s.kitty_flags(), 7);
        s.feed(b"\x1b[=1;3u");
        assert_eq!(s.kitty_flags(), 6);
        s.feed(b"\x1b[<5u");
        assert_eq!(s.kitty_flags(), 0);
    }

    #[test]
    fn tracks_modify_other_keys() {
        let mut s = KeyModeScanner::default();
        s.feed(b"\x1b[>4;2m");
        assert_eq!(s.modify_other_keys(), 2);
        s.feed(b"\x1b[>4m");
        assert_eq!(s.modify_other_keys(), 0);
    }

    #[test]
    fn handles_sequences_split_across_chunks_and_ignores_others() {
        let mut s = KeyModeScanner::default();
        s.feed(b"text \x1b[");
        s.feed(b">3");
        s.feed(b"u \x1b[1;31m \x1b]0;title\x07");
        assert_eq!(s.kitty_flags(), 3);
        assert_eq!(s.modify_other_keys(), 0);
    }
}
