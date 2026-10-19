//! Pasted text as a terminal sends it.

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Newlines become CR, as typed Enter would. With bracketed paste on, the text
/// is wrapped so the program sees one paste; an embedded end marker is removed
/// so the paste cannot close itself early.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return body.into_bytes();
    }
    let body = body.replace("\x1b[201~", "");
    [PASTE_START, body.as_bytes(), PASTE_END].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_is_wrapped_once() {
        assert_eq!(
            encode_paste("a\nb\r\nc", true),
            b"\x1b[200~a\rb\rc\x1b[201~"
        );
        assert_eq!(encode_paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }

    #[test]
    fn plain_paste_only_converts_newlines() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
    }
}
