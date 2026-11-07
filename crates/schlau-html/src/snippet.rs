use crate::dom::is_invisible_char;

const ELLIPSIS: char = '\u{2026}';

/// A one-line preview: whitespace collapsed, invisible padding removed, cut at a word
/// boundary to at most `max_chars` characters (including the trailing ellipsis).
pub fn snippet(text: &str, max_chars: usize) -> String {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || is_invisible_char(c))
        .filter(|w| !w.is_empty())
        .collect();
    let collapsed = words.join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    if max_chars == 0 {
        return String::new();
    }
    let budget = max_chars - 1;
    let cut = collapsed
        .char_indices()
        .nth(budget)
        .map_or(collapsed.len(), |(index, _)| index);
    let head = &collapsed[..cut];
    let at_word = match head.rfind(' ') {
        _ if collapsed[cut..].starts_with(' ') => head,
        Some(space) if space * 3 >= cut * 2 => &head[..space],
        _ => head,
    };
    let mut out = at_word
        .trim_end_matches([' ', ',', ';', ':', '-'])
        .to_owned();
    out.push(ELLIPSIS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_unchanged_but_collapsed() {
        assert_eq!(snippet("  Hello\n\n world\u{200c} ", 50), "Hello world");
    }

    #[test]
    fn cuts_at_word_boundary() {
        let out = snippet("The quick brown fox jumps over the lazy dog", 20);
        assert_eq!(out, "The quick brown fox\u{2026}");
        assert!(out.chars().count() <= 20);
    }

    #[test]
    fn cuts_long_words_and_multibyte() {
        assert_eq!(snippet("ääääääääää", 5), "ääää\u{2026}");
        assert_eq!(snippet("abc", 0), "");
    }
}
