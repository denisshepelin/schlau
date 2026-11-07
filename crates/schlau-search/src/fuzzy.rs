//! Typo-tolerant fuzzy ranking of short strings (sender names, addresses, subjects, mailbox names, command
//! palette entries) with `neo_frizbee`'s SIMD Smith-Waterman.

use neo_frizbee::{CaseMatching, Config, Matcher, Pattern, PatternConfig};

use crate::vocab::typo_budget;

/// A haystack entry that matched, with where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Index into the haystack slice.
    pub index: usize,
    /// Match quality; higher is better. Only comparable within one call.
    pub score: u16,
    /// Byte offsets of the matched characters in the haystack entry, ascending, for highlighting.
    pub indices: Vec<usize>,
}

/// Ranks `haystack` against `needle`, best first, at most `limit` results.
///
/// Each whitespace-separated word of the needle must match (in any order), as a fuzzy subsequence with a
/// typo budget by length (`typo_budget`: none under 4 characters, one up to 7, two from 8). Case is ignored.
/// An empty needle returns the first `limit` entries in order. Ties keep haystack order.
pub fn fuzzy_rank(needle: &str, haystack: &[&str], limit: usize) -> Vec<FuzzyMatch> {
    let words: Vec<&str> = needle.split_whitespace().collect();
    if words.is_empty() {
        return (0..haystack.len().min(limit))
            .map(|index| FuzzyMatch {
                index,
                score: 0,
                indices: Vec::new(),
            })
            .collect();
    }
    let patterns: Vec<Pattern> = words
        .iter()
        .map(|w| {
            let typos = typo_budget(w.chars().count()) as u16;
            Pattern::new(w, PatternConfig::default()).max_typos(Some(typos))
        })
        .collect();
    let config = Config::default().casing(CaseMatching::Ignore);
    let mut matcher = Matcher::from_patterns(&patterns, &config);
    let mut matches = matcher.match_list(haystack);
    matches.truncate(limit);
    matches
        .into_iter()
        .map(|m| {
            let index = m.index as usize;
            let indices = matcher
                .match_one_indices(haystack[index], m.index)
                .map(|mi| {
                    let mut ix: Vec<usize> = mi.indices.into_iter().map(|i| i as usize).collect();
                    ix.sort_unstable();
                    ix.dedup();
                    ix
                })
                .unwrap_or_default();
            FuzzyMatch {
                index,
                score: m.score,
                indices,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMANDS: &[&str] = &[
        "Archive thread",
        "Mark as unread",
        "Mark as read",
        "Move to folder",
        "Snooze until tomorrow",
        "Reply all",
        "Forward",
        "Open inbox",
        "Toggle sidebar",
        "Search mail",
    ];

    fn top(needle: &str) -> Option<&'static str> {
        fuzzy_rank(needle, COMMANDS, 5)
            .first()
            .map(|m| COMMANDS[m.index])
    }

    #[test]
    fn exact_prefix_wins() {
        assert_eq!(top("arch"), Some("Archive thread"));
        assert_eq!(top("snooze"), Some("Snooze until tomorrow"));
    }

    #[test]
    fn subsequence() {
        assert_eq!(top("mvfld"), Some("Move to folder"));
    }

    #[test]
    fn typos() {
        assert_eq!(top("snoize"), Some("Snooze until tomorrow"));
        assert_eq!(top("forwrad"), Some("Forward"));
    }

    #[test]
    fn several_words_all_match() {
        assert_eq!(top("mark unread"), Some("Mark as unread"));
        assert_eq!(top("unread mark"), Some("Mark as unread"));
        let all = fuzzy_rank("mark read", COMMANDS, 10);
        assert!(all.iter().all(|m| COMMANDS[m.index].starts_with("Mark")));
    }

    #[test]
    fn case_is_ignored() {
        assert_eq!(top("REPLY"), Some("Reply all"));
    }

    #[test]
    fn no_match() {
        assert!(fuzzy_rank("qqqq", COMMANDS, 5).is_empty());
    }

    #[test]
    fn empty_needle_keeps_order() {
        let got: Vec<usize> = fuzzy_rank("  ", COMMANDS, 3)
            .iter()
            .map(|m| m.index)
            .collect();
        assert_eq!(got, [0, 1, 2]);
    }

    #[test]
    fn limit() {
        assert_eq!(fuzzy_rank("a", COMMANDS, 2).len(), 2);
        assert!(fuzzy_rank("a", COMMANDS, 0).is_empty());
    }

    #[test]
    fn indices_point_at_matched_bytes() {
        let m = &fuzzy_rank("arch", COMMANDS, 1)[0];
        assert_eq!(m.indices, [0, 1, 2, 3]);
        let hay = ["Zoë Müller"];
        let m = &fuzzy_rank("muller", &hay, 1);
        assert_eq!(m.len(), 1);
        assert!(m[0].indices.iter().all(|&i| hay[0].is_char_boundary(i)));
    }

    #[test]
    fn indices_for_multiword() {
        let m = &fuzzy_rank("mark unread", COMMANDS, 1)[0];
        let text = COMMANDS[m.index];
        let picked: String = m
            .indices
            .iter()
            .map(|&i| text[i..].chars().next().unwrap())
            .collect();
        assert!(picked.to_lowercase().contains("mark"), "{picked}");
        assert!(picked.to_lowercase().contains("unread"), "{picked}");
    }
}
