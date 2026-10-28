//! What both backends' hook variables hold, checked the same way.

/// `text` as a number, if it is only ASCII digits (no sign, no spaces).
pub fn digits(text: &str) -> Option<u32> {
    let digit = |b: u8| b.is_ascii_digit();
    let plain = !text.is_empty() && text.bytes().all(digit);
    plain.then(|| text.parse().ok()).flatten()
}

/// Whether `id` is spelled as both backends spell a terminal's id: `%`
/// and a number.
pub fn terminal_id_ok(id: &str) -> bool {
    id.strip_prefix('%').and_then(digits).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_digits_are_a_number() {
        assert_eq!(digits("77"), Some(77));
        for bad in ["", "+77", "-77", " 77", "77x", "99999999999"] {
            assert_eq!(digits(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_terminal_id_is_a_percent_and_a_number() {
        assert!(terminal_id_ok("%3"));
        for bad in ["", "%", "3", "%x", "%3 ", "%-3", "%+3"] {
            assert!(!terminal_id_ok(bad), "{bad:?}");
        }
    }
}
