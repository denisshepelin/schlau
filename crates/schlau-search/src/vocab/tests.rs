use super::*;

fn vocab() -> Vocabulary {
    Vocabulary::new([
        ("receive", 3000),
        ("received", 1200),
        ("receiver", 90),
        ("receipt", 800),
        ("recieve", 3),
        ("invoice", 2500),
        ("invoices", 700),
        ("voice", 400),
        ("schema", 300),
        ("schemas", 40),
        ("scheme", 120),
        ("accommodate", 15),
        ("accommodation", 60),
        ("from", 9000),
        ("form", 800),
        ("2024", 500),
        ("2025", 900),
        ("café", 12),
        ("zürich", 7),
        ("cat", 50),
        ("category", 80),
        ("meeting", 1000),
        ("meetings", 300),
    ])
}

fn terms_of(cs: &[Correction]) -> Vec<&str> {
    cs.iter().map(|c| c.term.as_str()).collect()
}

#[test]
fn build_dedups_and_sums() {
    let v = Vocabulary::new([("b", 1), ("a", 2), ("b", 3), ("", 9)]);
    assert_eq!(v.len(), 2);
    assert_eq!(v.df("b"), Some(4));
    assert_eq!(v.df("a"), Some(2));
    assert_eq!(v.df(""), None);
}

#[test]
fn empty_vocabulary() {
    let v = Vocabulary::new(Vec::<(String, u32)>::new());
    assert!(v.is_empty());
    assert!(v.correct("invoice", 5).is_empty());
    assert!(v.complete("inv", 5).is_empty());
    assert!(v.fuzzy_prefixes("invo", 5).is_empty());
}

#[test]
fn exact_comes_first() {
    let cs = vocab().correct("invoice", 5);
    assert_eq!(
        cs[0],
        Correction {
            term: "invoice".into(),
            kind: MatchKind::Exact,
            distance: 0,
            df: 2500
        }
    );
    assert_eq!(terms_of(&cs), ["invoice", "invoices"]);
}

#[test]
fn transposition_recieve() {
    let cs = vocab().correct("recieve", 5);
    assert_eq!(cs[0].kind, MatchKind::Exact);
    assert_eq!(cs[1].term, "receive");
    assert_eq!(cs[1].distance, 1);
    assert_eq!(cs[1].kind, MatchKind::Typo);
}

#[test]
fn transposition_invocie() {
    let cs = vocab().correct("invocie", 5);
    assert_eq!(terms_of(&cs), ["invoice"]);
    assert_eq!(cs[0].distance, 1);
}

#[test]
fn transposition_shcema() {
    let cs = vocab().correct("shcema", 5);
    assert_eq!(terms_of(&cs)[0], "schema");
}

#[test]
fn deletion_insertion_substitution() {
    let v = vocab();
    assert_eq!(terms_of(&v.correct("invoce", 3))[0], "invoice");
    assert_eq!(terms_of(&v.correct("invoicce", 3))[0], "invoice");
    assert_eq!(terms_of(&v.correct("invoixe", 3))[0], "invoice");
}

#[test]
fn ranked_by_distance_then_df() {
    let cs = vocab().correct("receve", 5);
    assert_eq!(terms_of(&cs), ["receive", "recieve"]);
    let cs = vocab().correct("schem", 5);
    assert_eq!(terms_of(&cs), ["schema", "scheme"]);
}

#[test]
fn two_typos_for_long_terms() {
    let cs = vocab().correct("acomodate", 5);
    assert_eq!(terms_of(&cs), ["accommodate"]);
    assert_eq!(cs[0].distance, 2);
}

#[test]
fn only_one_typo_for_short_terms() {
    assert!(vocab().correct("shmea", 5).is_empty());
    assert!(vocab().correct("reciev", 5).iter().all(|c| c.distance <= 1));
}

#[test]
fn no_typos_under_four_chars() {
    assert!(vocab().correct("fro", 5).is_empty());
    assert_eq!(terms_of(&vocab().correct("cat", 5)), ["cat"]);
}

#[test]
fn valid_words_still_list_neighbors() {
    let cs = vocab().correct("form", 5);
    assert_eq!(terms_of(&cs), ["form", "from"]);
}

#[test]
fn numbers_are_not_corrected() {
    assert_eq!(terms_of(&vocab().correct("2024", 5)), ["2024"]);
    assert!(vocab().correct("2026", 5).is_empty());
}

#[test]
fn non_ascii_terms() {
    assert_eq!(terms_of(&vocab().correct("zurich", 5)), ["zürich"]);
    assert_eq!(terms_of(&vocab().correct("zürch", 5)), ["zürich"]);
}

#[test]
fn max_suggestions_limits() {
    assert_eq!(vocab().correct("invoice", 1).len(), 1);
    assert!(vocab().correct("invoice", 0).is_empty());
}

#[test]
fn prefix_completions_by_df() {
    let cs = vocab().complete("rec", 3);
    assert_eq!(terms_of(&cs), ["receive", "received", "receipt"]);
    assert!(cs.iter().all(|c| c.kind == MatchKind::Prefix));
}

#[test]
fn prefix_that_is_a_term() {
    let cs = vocab().complete("cat", 5);
    assert_eq!(terms_of(&cs), ["category", "cat"]);
    assert_eq!(cs[1].kind, MatchKind::Exact);
}

#[test]
fn prefix_with_typo() {
    let cs = vocab().complete("invoc", 3);
    assert_eq!(terms_of(&cs), ["invoice", "invoices"]);
    assert!(cs.iter().all(|c| c.kind == MatchKind::PrefixTypo));
    let cs = vocab().complete("meetni", 3);
    assert_eq!(terms_of(&cs), ["meeting", "meetings"]);
}

#[test]
fn prefix_typo_needs_four_chars() {
    assert!(vocab().complete("ivn", 3).is_empty());
}

#[test]
fn fuzzy_prefixes_rank_by_total_df() {
    let ps = vocab().fuzzy_prefixes("rexei", 3);
    assert_eq!(ps[0].0, "recei");
    assert_eq!(ps[0].1, 3000 + 1200 + 90 + 800);
    assert!(vocab().fuzzy_prefixes("rece", 3).is_empty());
}

#[test]
fn unknown_everything() {
    assert!(vocab().correct("qqqqqqq", 5).is_empty());
    assert!(vocab().complete("qqqq", 5).is_empty());
}

#[test]
fn long_tokens_are_not_corrected_but_found() {
    let long = "a".repeat(80);
    let v = Vocabulary::new([(long.as_str(), 1)]);
    assert_eq!(v.correct(&long, 3).len(), 1);
    assert!(v.correct(&"a".repeat(79), 3).is_empty());
    assert!(v.memory_bytes() > 0);
}

#[test]
fn one_edit_variants_cover_edits() {
    let chars: Vec<char> = "abc".chars().collect();
    let vs = one_edit_variants(&chars);
    ["bc", "ac", "ab", "bac", "acb", "xbc", "abcd", "zabc"]
        .iter()
        .for_each(|v| assert!(vs.contains(&v.to_string()), "{v}"));
}
