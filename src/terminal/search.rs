//! Plain-text search, as copy mode does it over a terminal's scrollback
//! and screen: no regex, case-insensitive unless the query has an
//! uppercase letter (the finder's rule, `crate::fuzzy`).

use crate::fuzzy::ignores_case;

/// `c` as compared: lowercased when the search ignores case. One character
/// stays one character, so match positions stay positions in the text.
fn fold(c: char, ignore: bool) -> char {
    if ignore {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

/// Where `query` occurs in `text`: the index of each match's first
/// character, left to right, matches not overlapping. An empty query
/// matches nowhere.
pub fn find(text: &[char], query: &str) -> Vec<usize> {
    let ignore = ignores_case(query);
    let query: Vec<char> = query.chars().map(|c| fold(c, ignore)).collect();
    let mut found = Vec::new();
    if query.is_empty() {
        return found;
    }
    let mut i = 0;
    while i + query.len() <= text.len() {
        let hit = text[i..i + query.len()]
            .iter()
            .zip(&query)
            .all(|(t, q)| fold(*t, ignore) == *q);
        if hit {
            found.push(i);
            i += query.len();
        } else {
            i += 1;
        }
    }
    found
}

/// Which of `starts` (in order) comes next from `from`: the first after it
/// going `forward`, else the last before it; past either end it wraps
/// around. `None` if there are none.
pub fn next<T: Ord>(starts: &[T], from: &T, forward: bool) -> Option<usize> {
    if starts.is_empty() {
        return None;
    }
    let found = if forward {
        starts.iter().position(|s| s > from).unwrap_or(0)
    } else {
        starts
            .iter()
            .rposition(|s| s < from)
            .unwrap_or(starts.len() - 1)
    };
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn finds_every_match_left_to_right_without_overlap() {
        assert_eq!(find(&chars("abcabc"), "bc"), [1, 4]);
        assert_eq!(find(&chars("aaaa"), "aa"), [0, 2]);
        assert!(find(&chars("abc"), "").is_empty());
        assert!(find(&chars("ab"), "abc").is_empty());
        // Positions count characters, not bytes.
        assert_eq!(find(&chars("漢字 error"), "err"), [3]);
    }

    #[test]
    fn case_is_ignored_unless_the_query_has_an_uppercase_letter() {
        let text = chars("Error error ERROR");
        assert_eq!(find(&text, "error"), [0, 6, 12]);
        assert_eq!(find(&text, "Error"), [0]);
        assert_eq!(find(&text, "ERROR"), [12]);
    }

    #[test]
    fn the_next_match_wraps_around_both_ways() {
        let starts = [2, 5, 9];
        assert_eq!(next(&starts, &5, true), Some(2));
        assert_eq!(next(&starts, &5, false), Some(0));
        assert_eq!(next(&starts, &9, true), Some(0), "wraps to the first");
        assert_eq!(next(&starts, &2, false), Some(2), "wraps to the last");
        assert_eq!(next(&starts, &0, true), Some(0));
        assert_eq!(next(&starts, &7, false), Some(1));
        assert_eq!(next::<i32>(&[], &0, true), None);
    }
}
