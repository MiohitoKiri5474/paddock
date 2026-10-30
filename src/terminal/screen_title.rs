//! Strips the screen/tmux title escape `ESC k <title> ST` from pane output.
//!
//! Shells inside tmux (e.g. oh-my-zsh's termsupport with TERM=tmux-256color)
//! set the window title this way. paddock's server hands terminal output
//! over raw, and alacritty_terminal does not know `ESC k`, so it would print the
//! title into the grid. ST is `ESC \`; BEL is accepted too.

/// A title longer than this without a terminator is given up on: the filter
/// drops what it swallowed and passes bytes through again.
const MAX_TITLE: usize = 1024;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    /// Saw `ESC`; held back until the next byte says what it starts.
    Esc,
    /// Inside a title, with this many title bytes swallowed.
    Title(usize),
    /// Saw `ESC` inside a title: `\` ends it, anything else starts a new escape.
    TitleEsc,
}

/// Remembers a sequence split across chunks.
#[derive(Debug, Default)]
pub struct ScreenTitleFilter {
    state: State,
}

impl ScreenTitleFilter {
    /// `bytes` with every screen title removed.
    pub fn filter(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            self.state = match (self.state, b) {
                (State::Ground, ESC) => State::Esc,
                (State::Ground, _) => {
                    out.push(b);
                    State::Ground
                }
                (State::Esc, b'k') => State::Title(0),
                (State::Esc, ESC) => {
                    out.push(ESC);
                    State::Esc
                }
                (State::Esc, _) => {
                    out.extend_from_slice(&[ESC, b]);
                    State::Ground
                }
                (State::Title(_), BEL) => State::Ground,
                (State::Title(_), ESC) => State::TitleEsc,
                (State::Title(n), _) if n + 1 >= MAX_TITLE => State::Ground,
                (State::Title(n), _) => State::Title(n + 1),
                (State::TitleEsc, b'\\') => State::Ground,
                // Not ST: the title is over and this ESC starts something else.
                (State::TitleEsc, b'k') => State::Title(0),
                (State::TitleEsc, ESC) => State::Esc,
                (State::TitleEsc, _) => {
                    out.extend_from_slice(&[ESC, b]);
                    State::Ground
                }
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut f = ScreenTitleFilter::default();
        chunks.iter().flat_map(|c| f.filter(c)).collect()
    }

    #[test]
    fn strips_a_whole_title_in_one_chunk() {
        assert_eq!(run(&[b"a\x1bkclaude\x1b\\b"]), b"ab");
    }

    #[test]
    fn strips_a_title_split_at_every_byte_boundary() {
        let input = b"x\x1bkclaude\x1b\\y\x1b[1mz";
        for i in 0..=input.len() {
            let (a, b) = input.split_at(i);
            assert_eq!(run(&[a, b]), b"xy\x1b[1mz", "split at {i}");
        }
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(run(&bytes), b"xy\x1b[1mz", "one byte at a time");
    }

    #[test]
    fn accepts_bel_as_terminator() {
        assert_eq!(run(&[b"a\x1bkti", b"tle\x07b"]), b"ab");
    }

    #[test]
    fn leaves_other_escapes_alone() {
        let other: &[u8] =
            b"\x1b[31mred\x1b]0;osc\x07\x1b]2;st\x1b\\\x1bPdcs\x1b\\\x1b\\\x1b7\x1b\x1b[m\x1bK";
        assert_eq!(run(&[other]), other);
        let bytes: Vec<&[u8]> = other.chunks(1).collect();
        assert_eq!(run(&bytes), other);
    }

    #[test]
    fn a_trailing_esc_is_held_until_the_next_chunk() {
        let mut f = ScreenTitleFilter::default();
        assert_eq!(f.filter(b"a\x1b"), b"a");
        assert_eq!(f.filter(b"[1m"), b"\x1b[1m");
    }

    #[test]
    fn an_escape_inside_a_title_ends_it_and_starts_anew() {
        assert_eq!(run(&[b"\x1bkti\x1b[1mx"]), b"\x1b[1mx");
    }

    #[test]
    fn gives_up_on_a_runaway_title() {
        let mut input = b"\x1bk".to_vec();
        input.extend(std::iter::repeat_n(b'a', MAX_TITLE));
        input.extend_from_slice(b"after");
        let out = run(&[&input]);
        assert!(
            out.ends_with(b"after"),
            "{:?}",
            String::from_utf8_lossy(&out)
        );
        assert!(out.len() < MAX_TITLE, "the swallowed title is dropped");
    }
}
