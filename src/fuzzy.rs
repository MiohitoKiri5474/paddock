//! Fuzzy matching for the finder and the dashboard filter: a query matches
//! a text when its characters appear in the text in order. Contiguous
//! matches and matches at the start of a word score higher. The match is
//! case-insensitive unless the query has an uppercase letter.

/// Points for each character matched.
const MATCHED: i32 = 1;
/// Extra points for a character matched right after the one before.
const CONTIGUOUS: i32 = 3;
/// Extra points for a character matched at the start of the text or of a
/// word in it.
const WORD_START: i32 = 2;

/// Whether a character parts words: a space, punctuation or a separator.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || c.is_ascii_punctuation() || c == '·' || c == '›'
}

/// How well `query` matches `text`: `None` if it does not, else a score
/// where higher is better (an empty query matches everything, scoring 0).
pub fn score(query: &str, text: &str) -> Option<i32> {
    let fold = query.chars().all(|c| !c.is_uppercase());
    let lower = |c: char| {
        if fold {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let query: Vec<char> = query.chars().map(lower).collect();
    if query.is_empty() {
        return Some(0);
    }
    let text: Vec<char> = text.chars().collect();
    let word_start = |j: usize| j == 0 || is_separator(text[j - 1]);
    // `best[j]`: the best score matching the query so far with its last
    // character at `text[j]`, if it matches there.
    let mut best: Vec<Option<i32>> = vec![None; text.len()];
    for (i, &q) in query.iter().enumerate() {
        let mut next: Vec<Option<i32>> = vec![None; text.len()];
        // The best score of the query so far ending anywhere before
        // `text[j - 1]`: a match at `j` after a gap.
        let mut gapped: Option<i32> = None;
        for j in 0..text.len() {
            // Ending right at `text[j - 1]`: a contiguous match at `j`.
            let adjacent = j.checked_sub(1).and_then(|k| best[k]);
            if j > 1 {
                gapped = gapped.max(best[j - 2]);
            }
            if lower(text[j]) == q {
                let from = if i == 0 {
                    Some(0)
                } else {
                    adjacent.map(|s| s + CONTIGUOUS).max(gapped)
                };
                next[j] = from.map(|s| {
                    let bonus = if word_start(j) { WORD_START } else { 0 };
                    s + MATCHED + bonus
                });
            }
        }
        best = next;
    }
    best.into_iter().flatten().max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_must_appear_in_order() {
        assert!(score("abc", "xaxbxc").is_some());
        assert_eq!(score("abc", "cba"), None);
        assert_eq!(score("abcd", "abc"), None);
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("a", ""), None);
    }

    #[test]
    fn contiguous_beats_scattered() {
        let tight = score("api", "api-server").unwrap();
        let loose = score("api", "a-p-i").unwrap();
        assert!(tight > loose, "{tight} vs {loose}");
    }

    #[test]
    fn word_starts_score_higher() {
        let starts = score("fb", "foo bar").unwrap();
        let inside = score("fb", "afab").unwrap();
        assert!(starts > inside, "{starts} vs {inside}");
        let path = score("pf", "~/project/paddock feat/x").unwrap();
        let plain = score("pf", "xxpfxx").unwrap();
        assert!(path > plain, "{path} vs {plain}");
    }

    #[test]
    fn case_is_ignored_unless_the_query_has_an_uppercase_letter() {
        assert!(score("abc", "ABC").is_some());
        assert!(score("abc", "aBc").is_some());
        assert_eq!(score("Abc", "abc"), None);
        assert!(score("ABC", "ABC").is_some());
        assert!(score("aBc", "xaBc").is_some());
    }

    #[test]
    fn a_whole_word_scores_its_best_placement() {
        // `claude` in `paddock · claude`: a word start, all contiguous.
        let s = score("claude", "paddock · claude").unwrap();
        assert_eq!(s, MATCHED * 6 + CONTIGUOUS * 5 + WORD_START);
    }
}
