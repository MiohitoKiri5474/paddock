//! Writing shell command lines.

/// `s` as one single-quoted shell word.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `s` as one shell word: as it is if it needs no quotes (letters, digits
/// and `/._-+`), else [`quote`]d.
pub fn word(s: &str) -> String {
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        s.to_string()
    } else {
        quote(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_with_a_quote_stays_one_word() {
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn a_plain_word_is_left_unquoted() {
        assert_eq!(word("/usr/local/bin/paddock"), "/usr/local/bin/paddock");
        assert_eq!(word("/Apps/My App/paddock"), "'/Apps/My App/paddock'");
    }
}
