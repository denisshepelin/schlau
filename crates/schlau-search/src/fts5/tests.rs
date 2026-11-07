use super::*;
use crate::expand::{Alternative, expand};
use crate::parse::parse;
use crate::query::Filter;
use crate::vocab::{MatchKind, Vocabulary};

fn columns(field: Field) -> &'static [&'static str] {
    match field {
        Field::Any => &[],
        Field::From => &["from_text"],
        Field::To | Field::Cc => &["to_text", "cc_text"],
        Field::Subject => &["subject"],
        Field::Body => &["body"],
        Field::Attachment => &["file names"],
    }
}

fn vocab() -> Vocabulary {
    Vocabulary::new([
        ("invoice", 100),
        ("receipt", 50),
        ("jane", 10),
        ("newsletter", 30),
        ("and", 900),
    ])
}

fn fts(input: &str) -> Fts5Match {
    to_fts5_match(&expand(&parse(input, 0), &vocab()), columns, Join::All)
}

fn expr(input: &str) -> String {
    fts(input).expression.unwrap()
}

fn terms(term: &str) -> ExpandedClause {
    ExpandedClause::Terms {
        field: Field::Any,
        source: term.into(),
        alternatives: vec![Alternative {
            term: term.into(),
            prefix: false,
            kind: MatchKind::Exact,
            distance: 0,
            weight: 1.0,
        }],
        negated: false,
    }
}

#[test]
fn quoting_escapes_quotes() {
    assert_eq!(quote_term("plain"), "\"plain\"");
    assert_eq!(quote_term("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(quote_term(""), "\"\"");
}

#[test]
fn quoting_neutralizes_syntax() {
    [
        "AND", "OR", "NOT", "NEAR", "a*", "e-mail", "x:y", "(a)", "{c}", "^a", "a+b",
    ]
    .iter()
    .for_each(|t| {
        let q = quote_term(t);
        assert!(q.starts_with('"') && q.ends_with('"'), "{t}");
        assert_eq!(&q[1..q.len() - 1], *t);
    });
}

#[test]
fn single_word() {
    assert_eq!(expr("invoice "), "\"invoice\"");
}

#[test]
fn keywords_as_words_are_quoted() {
    assert_eq!(expr("AND invoice NEAR "), "(\"invoice\") AND (\"near\")");
    assert_eq!(expr("and "), "\"and\"");
}

#[test]
fn several_words_and() {
    assert_eq!(expr("invoice receipt "), "(\"invoice\") AND (\"receipt\")");
}

#[test]
fn join_any() {
    let m = to_fts5_match(
        &expand(&parse("invoice receipt ", 0), &vocab()),
        columns,
        Join::Any,
    );
    assert_eq!(m.expression.unwrap(), "(\"invoice\") OR (\"receipt\")");
}

#[test]
fn typo_alternatives_or() {
    assert_eq!(expr("invocie "), "\"invoice\"");
    assert_eq!(expr("inv"), "\"inv\"*");
}

#[test]
fn multiple_alternatives_are_grouped() {
    let q = ExpandedQuery {
        clauses: vec![ExpandedClause::Terms {
            field: Field::Subject,
            source: "x".into(),
            alternatives: vec![
                Alternative {
                    term: "a".into(),
                    prefix: false,
                    kind: MatchKind::Exact,
                    distance: 0,
                    weight: 1.0,
                },
                Alternative {
                    term: "b".into(),
                    prefix: true,
                    kind: MatchKind::Prefix,
                    distance: 0,
                    weight: 0.9,
                },
            ],
            negated: false,
        }],
    };
    assert_eq!(
        to_fts5_match(&q, columns, Join::All).expression.unwrap(),
        "{subject} : (\"a\" OR \"b\"*)"
    );
}

#[test]
fn column_filters() {
    assert_eq!(expr("from:jane "), "{from_text} : \"jane\"");
    assert_eq!(expr("to:jane "), "{to_text cc_text} : \"jane\"");
    assert_eq!(expr("filename:invoice "), "{\"file names\"} : \"invoice\"");
}

#[test]
fn phrases() {
    assert_eq!(expr("\"Jane's invoice\""), "\"jane s invoice\"");
    assert_eq!(expr("\"jane inv"), "\"jane inv\"*");
    assert_eq!(expr("from:\"Jane Doe\""), "{from_text} : \"jane doe\"");
}

#[test]
fn hyphenated_word_is_a_phrase() {
    assert_eq!(expr("e-mail "), "\"e mail\"");
}

#[test]
fn negation_folds_into_not() {
    assert_eq!(
        expr("invoice -newsletter"),
        "\"invoice\" NOT \"newsletter\""
    );
    assert_eq!(
        expr("invoice receipt -newsletter -from:jane"),
        "(\"invoice\") AND (\"receipt\") NOT \"newsletter\" NOT {from_text} : \"jane\""
    );
}

#[test]
fn negation_only_is_excluded() {
    let m = fts("-newsletter is:unread");
    assert_eq!(m.expression, None);
    assert_eq!(m.excluded, vec!["\"newsletter\"".to_string()]);
    assert_eq!(
        m.residual,
        vec![ExpandedClause::Filter {
            filter: Filter::Unread,
            negated: false
        }]
    );
}

#[test]
fn filters_are_residual() {
    let m = fts("invoice is:unread after:2026-01-01");
    assert_eq!(m.expression.as_deref(), Some("\"invoice\""));
    assert_eq!(m.residual.len(), 2);
}

#[test]
fn or_group() {
    assert_eq!(expr("{invoice receipt}"), "(\"invoice\") OR (\"receipt\")");
    assert_eq!(
        expr("invoice OR from:jane "),
        "(\"invoice\") OR ({from_text} : \"jane\")"
    );
}

#[test]
fn and_group_inside_or() {
    assert_eq!(
        expr("(invoice jane) OR receipt "),
        "((\"invoice\") AND (\"jane\")) OR (\"receipt\")"
    );
}

#[test]
fn and_group_with_negation_inside_or() {
    assert_eq!(
        expr("(invoice -jane) OR receipt "),
        "(\"invoice\" NOT \"jane\") OR (\"receipt\")"
    );
}

#[test]
fn or_with_negation_is_residual() {
    let m = fts("invoice OR -jane");
    assert_eq!(m.expression, None);
    assert_eq!(m.residual.len(), 1);
}

#[test]
fn or_with_filter_is_residual() {
    let m = fts("receipt {invoice is:unread}");
    assert_eq!(m.expression.as_deref(), Some("\"receipt\""));
    assert!(matches!(m.residual[..], [ExpandedClause::Any(_)]));
}

#[test]
fn clause_to_fts5_per_leaf() {
    assert_eq!(
        clause_to_fts5(&terms("x"), columns).as_deref(),
        Some("\"x\"")
    );
    assert_eq!(
        clause_to_fts5(
            &ExpandedClause::Filter {
                filter: Filter::Read,
                negated: false
            },
            columns
        ),
        None
    );
    assert_eq!(
        clause_to_fts5(
            &ExpandedClause::All(vec![ExpandedClause::Phrase {
                field: Field::Any,
                terms: vec!["a".into()],
                prefix: false,
                negated: true,
            }]),
            columns
        ),
        None
    );
}

#[test]
fn empty_query() {
    assert_eq!(fts(""), Fts5Match::default());
}

#[test]
fn column_names() {
    assert_eq!(column_name("subject"), "subject");
    assert_eq!(column_name("from_text"), "from_text");
    assert_eq!(column_name("my col"), "\"my col\"");
    assert_eq!(column_name("NOT"), "\"NOT\"");
    assert_eq!(column_name("a\"b"), "\"a\"\"b\"");
}
