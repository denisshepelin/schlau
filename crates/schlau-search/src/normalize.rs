//! Text normalization shared by indexing and querying.
//!
//! The store must feed `normalize(text)` to its FTS5 table (tokenizer
//! `unicode61 remove_diacritics 2`), and queries go through the same function. FTS5's own folding is then a
//! no-op on the text it receives, so `tokenize(normalize(x))` yields exactly the terms FTS5 stores and that
//! `fts5vocab` reports, which is what term correction (`Vocabulary`) relies on.

use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

/// Normalizes text for search: NFKC (ligatures, full-width forms and compatibility characters become plain
/// letters), lower case, Latin diacritics removed as SQLite's `remove_diacritics 2` removes them (plus
/// combining marks U+0300..U+036F that NFKC could not compose, and stroked letters such as `ø`, `ł`, `đ`
/// that SQLite keeps), zero-width and other format characters
/// dropped. A zero-width space becomes a space, since it separates words.
pub fn normalize(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    text.nfkc().for_each(|c| match c {
        '\u{200B}' => out.push(' '),
        c if is_dropped(c) => {}
        c => c
            .to_lowercase()
            .filter(|&l| !is_dropped(l))
            .for_each(|l| out.push(fold_stroke(remove_diacritic(l)))),
    });
    out
}

/// Splits normalized text into the terms SQLite FTS5's `unicode61` tokenizer produces: maximal runs of
/// characters in the general categories L*, N*, Co and Cn (unassigned). Everything else separates terms.
///
/// The input must already be `normalize`d; this function does not fold case or diacritics.
pub fn tokenize(normalized: &str) -> impl Iterator<Item = &str> {
    normalized
        .split(|c: char| !is_token_char(c))
        .filter(|t| !t.is_empty())
}

/// `normalize` then `tokenize`, owned. Convenient for queries; indexing should normalize once and let FTS5
/// tokenize.
pub fn terms(text: &str) -> Vec<String> {
    tokenize(&normalize(text)).map(str::to_owned).collect()
}

/// Whether `c` is part of a term for FTS5's `unicode61` tokenizer with default categories (`L* N* Co`, and
/// unassigned code points, which SQLite also treats as token characters).
pub fn is_token_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric();
    }
    use GeneralCategory as G;
    matches!(
        get_general_category(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
            | G::DecimalNumber
            | G::LetterNumber
            | G::OtherNumber
            | G::PrivateUse
            | G::Unassigned
    )
}

fn is_dropped(c: char) -> bool {
    if c.is_ascii() {
        return false;
    }
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{FE00}'..='\u{FE0F}')
        || get_general_category(c) == GeneralCategory::Format
}

/// Letters with a stroke or bar that have no Unicode decomposition, so SQLite keeps them; folding them too is
/// safe because FTS5 receives already normalized text.
fn fold_stroke(c: char) -> char {
    match c {
        'ø' => 'o',
        'ł' | 'ŀ' => 'l',
        'đ' => 'd',
        'ħ' => 'h',
        'ŧ' => 't',
        'ı' => 'i',
        'ƀ' => 'b',
        'ƶ' => 'z',
        c => c,
    }
}

/// SQLite's `fts5_remove_diacritic(c, bComplex = 1)`, ported table for table: maps a precomposed Latin
/// letter with diacritics (lower case) to its ASCII base letter.
pub(crate) fn remove_diacritic(c: char) -> char {
    const DIA: [u16; 126] = [
        0, 1797, 1848, 1859, 1891, 1928, 1940, 1995, 2024, 2040, 2060, 2110, 2168, 2206, 2264,
        2286, 2344, 2383, 2472, 2488, 2516, 2596, 2668, 2732, 2782, 2842, 2894, 2954, 2984, 3000,
        3028, 3336, 3456, 3696, 3712, 3728, 3744, 3766, 3832, 3896, 3912, 3928, 3944, 3968, 4008,
        4040, 4056, 4106, 4138, 4170, 4202, 4234, 4266, 4296, 4312, 4344, 4408, 4424, 4442, 4472,
        4488, 4504, 6148, 6198, 6264, 6280, 6360, 6429, 6505, 6529, 61448, 61468, 61512, 61534,
        61592, 61610, 61642, 61672, 61688, 61704, 61726, 61784, 61800, 61816, 61836, 61880, 61896,
        61914, 61948, 61998, 62062, 62122, 62154, 62184, 62200, 62218, 62252, 62302, 62364, 62410,
        62442, 62478, 62536, 62554, 62584, 62604, 62640, 62648, 62656, 62664, 62730, 62766, 62830,
        62890, 62924, 62974, 63032, 63050, 63082, 63118, 63182, 63242, 63274, 63310, 63368, 63390,
    ];
    const CHARS: &[u8; 126] = b"\0aceinouyyacdeeghijklnorstuuwyzouaiouuagkoojgnaaeiorusthaeoooy\
\0\0\0\0\0\0\0\0abcddeeefghhiiklllmnoprrrsstuuvwwxyzhtwyaaaaeeeioooouuuy";

    let code = c as u32;
    if !(128..65536).contains(&code) {
        return c;
    }
    let key = (code << 3) | 7;
    let idx = DIA
        .partition_point(|&d| u32::from(d) <= key)
        .saturating_sub(1);
    let start = u32::from(DIA[idx]) >> 3;
    let span = u32::from(DIA[idx]) & 7;
    match CHARS[idx] {
        0 => c,
        _ if code > start + span => c,
        base => base as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_lowercased() {
        assert_eq!(normalize("Hello World"), "hello world");
    }

    #[test]
    fn diacritics_are_removed() {
        assert_eq!(normalize("Café Ünïcödé Ñandú"), "cafe unicode nandu");
        assert_eq!(normalize("Ørsted Łódź Đorđe"), "orsted lodz dorde");
        assert_eq!(normalize("Ångström"), "angstrom");
    }

    #[test]
    fn decomposed_input_matches_precomposed() {
        assert_eq!(normalize("e\u{0301}te\u{0301}"), normalize("été"));
        assert_eq!(normalize("e\u{0301}te\u{0301}"), "ete");
    }

    #[test]
    fn nfkc_folds_ligatures_and_fullwidth() {
        assert_eq!(normalize("ﬁle"), "file");
        assert_eq!(normalize("ＡＢＣ１２３"), "abc123");
        assert_eq!(normalize("①"), "1");
    }

    #[test]
    fn format_characters_are_dropped() {
        assert_eq!(normalize("in\u{00AD}voice"), "invoice");
        assert_eq!(normalize("a\u{200D}b\u{200E}c\u{FEFF}"), "abc");
        assert_eq!(normalize("one\u{200B}two"), "one two");
    }

    #[test]
    fn non_latin_scripts_survive() {
        assert_eq!(normalize("Привет МИР"), "привет мир");
        assert_eq!(normalize("Ελληνικά"), "ελληνικά");
        assert_eq!(normalize("日本語"), "日本語");
    }

    #[test]
    fn turkish_dotted_capital_i_folds_to_i() {
        assert_eq!(normalize("İstanbul"), "istanbul");
    }

    #[test]
    fn tokenize_splits_like_unicode61() {
        let n = normalize("Re: Invoice #2024-001 (jane.doe@example.com) — paid!");
        let toks: Vec<_> = tokenize(&n).collect();
        assert_eq!(
            toks,
            [
                "re", "invoice", "2024", "001", "jane", "doe", "example", "com", "paid"
            ]
        );
    }

    #[test]
    fn tokenize_keeps_non_ascii_letters_and_drops_symbols() {
        let n = normalize("naïve straße 東京 🎉 ok");
        let toks: Vec<_> = tokenize(&n).collect();
        assert_eq!(toks, ["naive", "straße", "東京", "ok"]);
    }

    #[test]
    fn apostrophes_and_underscores_separate() {
        assert_eq!(terms("O'Brien snake_case"), ["o", "brien", "snake", "case"]);
    }

    #[test]
    fn empty_and_whitespace() {
        assert!(terms("").is_empty());
        assert!(terms("   \t\n ").is_empty());
        assert!(terms("!!! ---").is_empty());
    }

    #[test]
    fn remove_diacritic_leaves_plain_letters() {
        ('a'..='z').for_each(|c| assert_eq!(remove_diacritic(c), c));
        assert_eq!(remove_diacritic('é'), 'e');
        assert_eq!(remove_diacritic('ç'), 'c');
        assert_eq!(remove_diacritic('ß'), 'ß');
        assert_eq!(remove_diacritic('ж'), 'ж');
    }

    #[test]
    fn normalize_is_idempotent() {
        ["Café ﬁle", "İstanbul", "Ørsted", "a\u{200B}b"]
            .iter()
            .for_each(|s| assert_eq!(normalize(&normalize(s)), normalize(s)));
    }
}
