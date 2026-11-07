use super::*;
use crate::query::Filter;

const NOW: i64 = 1_767_225_600 + 15 * 3600;
const DAY: i64 = 86_400;
const JAN1: i64 = 1_767_225_600;

fn q(input: &str) -> Query {
    parse(input, NOW)
}

fn word(text: &str) -> Clause {
    Clause::Word {
        field: Field::Any,
        text: text.into(),
        negated: false,
    }
}

fn not_word(text: &str) -> Clause {
    Clause::Word {
        field: Field::Any,
        text: text.into(),
        negated: true,
    }
}

fn fword(field: Field, text: &str) -> Clause {
    Clause::Word {
        field,
        text: text.into(),
        negated: false,
    }
}

fn phrase(field: Field, text: &str) -> Clause {
    Clause::Phrase {
        field,
        text: text.into(),
        negated: false,
    }
}

fn filter(filter: Filter) -> Clause {
    Clause::Filter {
        filter,
        negated: false,
    }
}

fn kinds(input: &str) -> Vec<(&str, TokenKind)> {
    tokens(input)
        .into_iter()
        .map(|t| (&input[t.range], t.kind))
        .collect()
}

#[test]
fn empty_and_whitespace() {
    assert_eq!(q(""), Query::default());
    assert_eq!(q("   "), Query::default());
    assert!(tokens("  ").is_empty());
}

#[test]
fn free_words() {
    let query = q("quarterly report");
    assert_eq!(query.clauses, vec![word("quarterly"), word("report")]);
    assert_eq!(query.semantic_text, "quarterly report");
    assert!(query.last_is_prefix);
}

#[test]
fn trailing_space_ends_prefix() {
    assert!(!q("report ").last_is_prefix);
    assert!(q("rep").last_is_prefix);
}

#[test]
fn prefix_not_set_when_last_is_filter() {
    assert!(!q("report is:unread").last_is_prefix);
    assert!(!q("report in:inb").last_is_prefix);
}

#[test]
fn prefix_set_for_field_value() {
    let query = q("from:ja");
    assert_eq!(query.clauses, vec![fword(Field::From, "ja")]);
    assert!(query.last_is_prefix);
}

#[test]
fn prefix_not_set_for_negated_word() {
    assert!(!q("report -draf").last_is_prefix);
    assert!(!q("-(a b").last_is_prefix);
}

#[test]
fn prefix_for_unterminated_phrase() {
    assert!(q("\"hello wor").last_is_prefix);
    assert!(!q("\"hello world\"").last_is_prefix);
    assert!(!q("\"hello world ").last_is_prefix);
}

#[test]
fn field_operators() {
    let query =
        q("from:jane to:bob cc:carol subject:invoice body:paid filename:pdf attachment:xls");
    assert_eq!(
        query.clauses,
        vec![
            fword(Field::From, "jane"),
            fword(Field::To, "bob"),
            fword(Field::Cc, "carol"),
            fword(Field::Subject, "invoice"),
            fword(Field::Body, "paid"),
            fword(Field::Attachment, "pdf"),
            fword(Field::Attachment, "xls"),
        ]
    );
    assert_eq!(query.semantic_text, "invoice paid");
}

#[test]
fn operator_names_ignore_case() {
    assert_eq!(q("FROM:jane").clauses, vec![fword(Field::From, "jane")]);
    assert_eq!(q("Is:Unread").clauses, vec![filter(Filter::Unread)]);
}

#[test]
fn quoted_field_values() {
    assert_eq!(
        q("from:\"Jane Doe\"").clauses,
        vec![phrase(Field::From, "Jane Doe")]
    );
    assert_eq!(
        q("in:\"Receipts/2026\"").clauses,
        vec![filter(Filter::InMailbox("Receipts/2026".into()))]
    );
}

#[test]
fn email_address_value() {
    assert_eq!(
        q("from:jane.doe@example.com").clauses,
        vec![fword(Field::From, "jane.doe@example.com")]
    );
}

#[test]
fn is_and_has_flags() {
    assert_eq!(
        q("is:unread is:read is:flagged is:starred is:draft is:answered has:attachment").clauses,
        vec![
            filter(Filter::Unread),
            filter(Filter::Read),
            filter(Filter::Flagged),
            filter(Filter::Flagged),
            filter(Filter::Draft),
            filter(Filter::Answered),
            filter(Filter::HasAttachment),
        ]
    );
}

#[test]
fn mailbox_and_label() {
    assert_eq!(
        q("in:inbox label:Work").clauses,
        vec![
            filter(Filter::InMailbox("inbox".into())),
            filter(Filter::InMailbox("Work".into()))
        ]
    );
}

#[test]
fn absolute_dates() {
    assert_eq!(
        q("after:2026-01-01 before:2026/01/02").clauses,
        vec![
            filter(Filter::After(JAN1)),
            filter(Filter::Before(JAN1 + DAY))
        ]
    );
}

#[test]
fn relative_dates() {
    assert_eq!(
        q("older_than:7d newer_than:2w after:3m before:1y").clauses,
        vec![
            filter(Filter::Before(NOW - 7 * DAY)),
            filter(Filter::After(NOW - 14 * DAY)),
            filter(Filter::After(NOW - 90 * DAY)),
            filter(Filter::Before(NOW - 365 * DAY)),
        ]
    );
}

#[test]
fn today_yesterday() {
    assert_eq!(
        q("after:yesterday before:today").clauses,
        vec![
            filter(Filter::After(JAN1 - DAY)),
            filter(Filter::Before(JAN1))
        ]
    );
}

#[test]
fn dates_in_zone() {
    let query = parse_in_zone("after:2026-01-01", NOW, 3600);
    assert_eq!(query.clauses, vec![filter(Filter::After(JAN1 - 3600))]);
}

#[test]
fn sizes() {
    assert_eq!(
        q("larger:5M smaller:100k").clauses,
        vec![
            filter(Filter::Larger(5 * 1024 * 1024)),
            filter(Filter::Smaller(100 * 1024))
        ]
    );
}

#[test]
fn negation() {
    assert_eq!(
        q("-newsletter -is:unread -from:spam").clauses,
        vec![
            not_word("newsletter"),
            Clause::Filter {
                filter: Filter::Unread,
                negated: true
            },
            Clause::Word {
                field: Field::From,
                text: "spam".into(),
                negated: true
            },
        ]
    );
    assert_eq!(q("-newsletter").semantic_text, "");
}

#[test]
fn negated_phrase() {
    assert_eq!(
        q("-\"weekly digest\"").clauses,
        vec![Clause::Phrase {
            field: Field::Any,
            text: "weekly digest".into(),
            negated: true
        }]
    );
}

#[test]
fn lone_dash_is_ignored() {
    assert_eq!(q("a - b").clauses, vec![word("a"), word("b")]);
    assert_eq!(q("-").clauses, vec![]);
}

#[test]
fn hyphenated_word_stays_one_word() {
    assert_eq!(q("e-mail").clauses, vec![word("e-mail")]);
}

#[test]
fn or_between_words() {
    assert_eq!(
        q("invoice OR receipt").clauses,
        vec![Clause::Any(vec![word("invoice"), word("receipt")])]
    );
    assert_eq!(
        q("a OR b OR c d").clauses,
        vec![
            Clause::Any(vec![word("a"), word("b"), word("c")]),
            word("d")
        ]
    );
}

#[test]
fn lowercase_or_is_a_word() {
    assert_eq!(
        q("this or that").clauses,
        vec![word("this"), word("or"), word("that")]
    );
}

#[test]
fn dangling_or_is_ignored() {
    assert_eq!(q("invoice OR").clauses, vec![word("invoice")]);
    assert_eq!(q("OR invoice").clauses, vec![word("invoice")]);
    assert_eq!(q("OR").clauses, vec![]);
    assert_eq!(q("a OR OR b").clauses, vec![word("a"), word("b")]);
}

#[test]
fn and_is_implied() {
    assert_eq!(q("a AND b").clauses, vec![word("a"), word("b")]);
}

#[test]
fn brace_group_is_or() {
    assert_eq!(
        q("{invoice receipt}").clauses,
        vec![Clause::Any(vec![word("invoice"), word("receipt")])]
    );
}

#[test]
fn paren_group_inside_or() {
    assert_eq!(
        q("(from:a subject:x) OR from:b").clauses,
        vec![Clause::Any(vec![
            Clause::All(vec![fword(Field::From, "a"), fword(Field::Subject, "x")]),
            fword(Field::From, "b"),
        ])]
    );
}

#[test]
fn top_level_paren_group_flattens() {
    assert_eq!(q("(a b) c").clauses, vec![word("a"), word("b"), word("c")]);
}

#[test]
fn negated_groups_use_de_morgan() {
    assert_eq!(q("-{a b}").clauses, vec![not_word("a"), not_word("b")]);
    assert_eq!(
        q("x -(a b)").clauses,
        vec![word("x"), Clause::Any(vec![not_word("a"), not_word("b")])]
    );
}

#[test]
fn operator_with_group_value() {
    assert_eq!(
        q("from:{alice bob}").clauses,
        vec![Clause::Any(vec![
            fword(Field::From, "alice"),
            fword(Field::From, "bob")
        ])]
    );
    assert_eq!(
        q("is:(unread flagged)").clauses,
        vec![filter(Filter::Unread), filter(Filter::Flagged)]
    );
    assert_eq!(
        q("in:{inbox \"Receipts 2026\"}").clauses,
        vec![Clause::Any(vec![
            filter(Filter::InMailbox("inbox".into())),
            filter(Filter::InMailbox("Receipts 2026".into())),
        ])]
    );
}

#[test]
fn nested_groups() {
    assert_eq!(
        q("{a (b {c d})}").clauses,
        vec![Clause::Any(vec![
            word("a"),
            Clause::All(vec![word("b"), Clause::Any(vec![word("c"), word("d")])])
        ])]
    );
}

#[test]
fn unclosed_groups_close_at_end() {
    assert_eq!(
        q("{a b").clauses,
        vec![Clause::Any(vec![word("a"), word("b")])]
    );
    assert_eq!(q("(a").clauses, vec![word("a")]);
    assert!(q("{a b").last_is_prefix);
}

#[test]
fn stray_closers_are_invalid() {
    assert_eq!(q("a) b}").clauses, vec![word("a"), word("b")]);
    assert_eq!(
        kinds("a) b"),
        vec![
            ("a", TokenKind::Word),
            (")", TokenKind::Invalid),
            ("b", TokenKind::Word)
        ]
    );
}

#[test]
fn empty_groups_vanish() {
    assert_eq!(q("() {} a").clauses, vec![word("a")]);
}

#[test]
fn unknown_operator_is_words() {
    assert_eq!(q("foo:bar").clauses, vec![word("foo:bar")]);
    assert_eq!(q("10:30").clauses, vec![word("10:30")]);
    assert_eq!(kinds("foo:bar"), vec![("foo:bar", TokenKind::Word)]);
}

#[test]
fn invalid_values_degrade_to_words() {
    assert_eq!(q("is:banana").clauses, vec![word("is:banana")]);
    assert_eq!(
        q("after:2026-13-45").clauses,
        vec![word("after:2026-13-45")]
    );
    assert_eq!(q("larger:huge").clauses, vec![word("larger:huge")]);
    assert_eq!(kinds("is:banana"), vec![("is:banana", TokenKind::Invalid)]);
}

#[test]
fn empty_operator_value_is_ignored() {
    assert_eq!(q("from:").clauses, vec![]);
    assert_eq!(q("from: jane").clauses, vec![word("jane")]);
    assert_eq!(kinds("from:"), vec![("from:", TokenKind::Operator)]);
    assert!(!q("from:").last_is_prefix);
}

#[test]
fn unterminated_phrase_runs_to_end() {
    assert_eq!(
        q("\"hello world").clauses,
        vec![phrase(Field::Any, "hello world")]
    );
    assert_eq!(q("from:\"Jane").clauses, vec![phrase(Field::From, "Jane")]);
}

#[test]
fn empty_phrases_vanish() {
    assert_eq!(q("\"\" \"  \" a").clauses, vec![word("a")]);
}

#[test]
fn words_without_terms_are_dropped() {
    assert_eq!(q("!!! ... a").clauses, vec![word("a")]);
}

#[test]
fn phrase_in_semantic_text() {
    assert_eq!(
        q("\"trip to Lisbon\" from:anna flights").semantic_text,
        "trip to Lisbon flights"
    );
}

#[test]
fn tokens_cover_syntax() {
    assert_eq!(
        kinds("-from:\"Jane Doe\" is:unread {a OR b} \"x y\""),
        vec![
            ("-", TokenKind::Negation),
            ("from:", TokenKind::Operator),
            ("\"Jane Doe\"", TokenKind::Value),
            ("is:", TokenKind::Operator),
            ("unread", TokenKind::Value),
            ("{", TokenKind::Group),
            ("a", TokenKind::Word),
            ("OR", TokenKind::Connective),
            ("b", TokenKind::Word),
            ("}", TokenKind::Group),
            ("\"x y\"", TokenKind::Phrase),
        ]
    );
}

#[test]
fn tokens_never_overlap_and_stay_in_bounds() {
    let inputs = [
        "-from:\"Jane is:{unread (a OR -\"b",
        "((((",
        "))))}}}}",
        "\"\"\"\"\"",
        "-:-:-:",
        "from:from:from:",
        ":::",
        "is:unread:x",
        "ÄÖÜ OR ß -é in:Ünïcödé",
        "a\u{200B}b \u{FEFF}",
        "OR OR OR AND",
        "-{-(-a)}",
        "after: before:x larger:",
        "\"unterminated {group (in quote",
        "🎉 from:🎉 \"🎉\"",
    ];
    inputs.iter().for_each(|input| {
        let toks = tokens(input);
        toks.windows(2)
            .for_each(|w| assert!(w[0].range.end <= w[1].range.start, "{input}: {toks:?}"));
        toks.iter().for_each(|t| {
            assert!(t.range.end <= input.len());
            assert!(input.is_char_boundary(t.range.start) && input.is_char_boundary(t.range.end));
        });
        let _ = q(input);
    });
}

#[test]
fn garbage_never_panics() {
    let alphabet = [
        'a', ' ', '-', '"', '(', ')', '{', '}', ':', 'O', 'R', 'é', '🎉', 'i', 's',
    ];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..3000).for_each(|_| {
        let len = (seed % 24) as usize;
        let input: String = (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                alphabet[(seed % alphabet.len() as u64) as usize]
            })
            .collect();
        let (query, toks) = parse_with_tokens(&input, NOW, 0);
        assert!(toks.iter().all(|t| t.range.end <= input.len()), "{input:?}");
        assert!(!query.semantic_text.starts_with(' '), "{input:?}");
    });
}

#[test]
fn deep_nesting_is_fine() {
    let input = "(".repeat(500) + "a" + &")".repeat(500);
    assert_eq!(q(&input).clauses, vec![word("a")]);
}

#[test]
fn negation_inside_or() {
    assert_eq!(
        q("a OR -b").clauses,
        vec![Clause::Any(vec![word("a"), not_word("b")])]
    );
}

#[test]
fn has_text_helper() {
    assert!(q("is:unread hello").has_text());
    assert!(!q("is:unread in:inbox").has_text());
    assert!(q("{is:unread hello}").has_text());
}
