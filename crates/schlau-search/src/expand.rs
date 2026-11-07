//! Turns a parsed `Query` into terms the index has: each typed word becomes an OR of acceptable vocabulary
//! terms (the word itself, prefix completions, typo corrections), each with a weight.

use crate::normalize::{normalize, tokenize};
use crate::query::{Clause, Field, Filter, Query};
use crate::vocab::{MatchKind, Typos, Vocabulary};

/// A query whose words have been resolved against the vocabulary. Same shape as `Query`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExpandedQuery {
    /// All of these must hold.
    pub clauses: Vec<ExpandedClause>,
}

/// A clause with its words resolved to index terms.
#[derive(Debug, Clone, PartialEq)]
pub enum ExpandedClause {
    /// A typed word: matches if any alternative does.
    Terms {
        /// Where the word must appear.
        field: Field,
        /// The word as typed.
        source: String,
        /// Index terms to accept, best first. Never empty.
        alternatives: Vec<Alternative>,
        /// The word must not appear.
        negated: bool,
    },
    /// Consecutive terms: a quoted phrase, or a typed word that tokenizes into several terms (`jane.doe`).
    Phrase {
        /// Where the phrase must appear.
        field: Field,
        /// Normalized terms, in order. Never empty.
        terms: Vec<String>,
        /// The last term matches as a prefix (it is still being typed).
        prefix: bool,
        /// The phrase must not appear.
        negated: bool,
    },
    /// A structured filter, passed through unchanged.
    Filter {
        /// The condition.
        filter: Filter,
        /// The condition must not hold.
        negated: bool,
    },
    /// At least one holds.
    Any(Vec<ExpandedClause>),
    /// All hold.
    All(Vec<ExpandedClause>),
}

/// One index term accepted for a typed word.
#[derive(Debug, Clone, PartialEq)]
pub struct Alternative {
    /// The normalized index term (or prefix, when `prefix` is set).
    pub term: String,
    /// Match every term that starts with `term`.
    pub prefix: bool,
    /// How the term relates to the typed word.
    pub kind: MatchKind,
    /// Edits between the typed word and `term`.
    pub distance: u8,
    /// Relative confidence in `0.0..=1.0`, for weighting coverage: exact > prefix > one typo > two typos.
    pub weight: f32,
}

/// Knobs for `expand_with`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandOptions {
    /// Most alternatives per word.
    pub max_alternatives: usize,
    /// A word found in the index as typed also accepts its typo neighbors only when they are this many times
    /// more frequent (it is then more likely a misspelling that made it into a few messages: `recieve`).
    pub dominant_ratio: u32,
    /// Shortest word (in characters) that falls back to prefix matching when it is not in the index.
    pub prefix_fallback_min_len: usize,
    /// How forgiving words are with misspellings.
    pub typos: Typos,
    /// How the word still being typed matches.
    pub last_word: LastWord,
}

/// How the word still being typed matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "kebab-case"))]
pub enum LastWord {
    /// As a prefix, so results show while typing: `invo` finds "invoice".
    #[default]
    Prefix,
    /// As a whole word, like every other.
    Whole,
}

impl LastWord {
    /// Every choice, in the order a settings list shows them.
    pub const ALL: [LastWord; 2] = [LastWord::Prefix, LastWord::Whole];

    /// How a settings list names the choice.
    pub fn label(self) -> &'static str {
        match self {
            LastWord::Prefix => "Prefix",
            LastWord::Whole => "Whole word",
        }
    }

    /// What the choice does, for a settings page.
    pub fn detail(self) -> &'static str {
        match self {
            LastWord::Prefix => "The word being typed finds longer words too: inv finds invoice.",
            LastWord::Whole => "Every word matches whole, the last one too.",
        }
    }
}

impl Default for ExpandOptions {
    fn default() -> Self {
        ExpandOptions {
            max_alternatives: 6,
            dominant_ratio: 20,
            prefix_fallback_min_len: 3,
            typos: Typos::default(),
            last_word: LastWord::default(),
        }
    }
}

/// Weight of an alternative of `kind` at `distance` edits.
pub fn alternative_weight(kind: MatchKind, distance: u8) -> f32 {
    match (kind, distance) {
        (MatchKind::Exact, _) => 1.0,
        (MatchKind::Prefix, _) => 0.9,
        (MatchKind::Typo, 1) => 0.7,
        (MatchKind::PrefixTypo, _) => 0.6,
        (MatchKind::Typo, _) => 0.5,
    }
}

/// `expand_with` with default options.
pub fn expand(query: &Query, vocab: &Vocabulary) -> ExpandedQuery {
    expand_with(query, vocab, &ExpandOptions::default())
}

/// Resolves every word of `query` against `vocab`. Words are normalized and tokenized like the index;
/// negated words and phrases are kept exactly as typed.
pub fn expand_with(query: &Query, vocab: &Vocabulary, options: &ExpandOptions) -> ExpandedQuery {
    let prefix_at = (query.last_is_prefix && options.last_word == LastWord::Prefix)
        .then(|| count_text(&query.clauses))
        .and_then(|n| n.checked_sub(1));
    let mut ctx = Ctx {
        vocab,
        options,
        prefix_at,
        seen: 0,
    };
    ExpandedQuery {
        clauses: query.clauses.iter().filter_map(|c| ctx.clause(c)).collect(),
    }
}

fn count_text(clauses: &[Clause]) -> usize {
    clauses
        .iter()
        .map(|c| match c {
            Clause::Word { .. } | Clause::Phrase { .. } => 1,
            Clause::Filter { .. } => 0,
            Clause::Any(cs) | Clause::All(cs) => count_text(cs),
        })
        .sum()
}

struct Ctx<'a> {
    vocab: &'a Vocabulary,
    options: &'a ExpandOptions,
    prefix_at: Option<usize>,
    seen: usize,
}

impl Ctx<'_> {
    fn next_is_prefix(&mut self) -> bool {
        let is = self.prefix_at == Some(self.seen);
        self.seen += 1;
        is
    }

    fn clause(&mut self, clause: &Clause) -> Option<ExpandedClause> {
        match clause {
            Clause::Word {
                field,
                text,
                negated,
            } => {
                let prefix = self.next_is_prefix() && !negated;
                let terms = tokens(text);
                match terms.as_slice() {
                    [] => None,
                    [one] if *negated => Some(ExpandedClause::Terms {
                        field: *field,
                        source: text.clone(),
                        alternatives: vec![exact(one, false)],
                        negated: true,
                    }),
                    [one] => Some(ExpandedClause::Terms {
                        field: *field,
                        source: text.clone(),
                        alternatives: self.alternatives(one, prefix),
                        negated: false,
                    }),
                    _ => Some(ExpandedClause::Phrase {
                        field: *field,
                        terms,
                        prefix,
                        negated: *negated,
                    }),
                }
            }
            Clause::Phrase {
                field,
                text,
                negated,
            } => {
                let prefix = self.next_is_prefix() && !negated;
                let terms = tokens(text);
                (!terms.is_empty()).then_some(ExpandedClause::Phrase {
                    field: *field,
                    terms,
                    prefix,
                    negated: *negated,
                })
            }
            Clause::Filter { filter, negated } => Some(ExpandedClause::Filter {
                filter: filter.clone(),
                negated: *negated,
            }),
            Clause::Any(cs) => self.group(cs, ExpandedClause::Any),
            Clause::All(cs) => self.group(cs, ExpandedClause::All),
        }
    }

    fn group(
        &mut self,
        cs: &[Clause],
        wrap: fn(Vec<ExpandedClause>) -> ExpandedClause,
    ) -> Option<ExpandedClause> {
        let mut inner: Vec<ExpandedClause> = cs.iter().filter_map(|c| self.clause(c)).collect();
        match inner.len() {
            0 => None,
            1 => inner.pop(),
            _ => Some(wrap(inner)),
        }
    }

    fn alternatives(&self, term: &str, prefix: bool) -> Vec<Alternative> {
        let (vocab, opts) = (self.vocab, self.options);
        let max = opts.max_alternatives.max(1);
        let corrections = vocab.correct_with(term, max, opts.typos);
        let exact_df = corrections
            .first()
            .filter(|c| c.kind == MatchKind::Exact)
            .map(|c| c.df);
        let best_typo = corrections
            .iter()
            .filter(|c| c.kind == MatchKind::Typo)
            .map(|c| c.distance)
            .min();
        let typos = corrections
            .iter()
            .filter(|c| c.kind == MatchKind::Typo && Some(c.distance) == best_typo)
            .filter(|c| {
                exact_df.is_none_or(|df| {
                    u64::from(c.df) >= u64::from(df) * u64::from(opts.dominant_ratio)
                })
            })
            .map(|c| alt(&c.term, false, MatchKind::Typo, c.distance));

        let has_completions = vocab.has_prefix(term);
        let lead = match (prefix, exact_df, has_completions) {
            (true, Some(_), _) => vec![alt(term, true, MatchKind::Exact, 0)],
            (true, None, true) => vec![alt(term, true, MatchKind::Prefix, 0)],
            (true, None, false) => vocab
                .fuzzy_prefixes(term, max)
                .into_iter()
                .map(|(p, _)| alt(&p, true, MatchKind::PrefixTypo, 1))
                .collect(),
            (false, Some(_), _) => vec![exact(term, false)],
            (false, None, _) => Vec::new(),
        };
        let typo_list: Vec<Alternative> = typos.collect();
        let fallback = (lead.is_empty()
            && typo_list.is_empty()
            && has_completions
            && term.chars().count() >= opts.prefix_fallback_min_len)
            .then(|| alt(term, true, MatchKind::Prefix, 0));

        let mut all: Vec<Alternative> = lead.into_iter().chain(typo_list).chain(fallback).collect();
        dedup_subsumed(&mut all);
        all.truncate(max);
        if all.is_empty() {
            all.push(exact(term, false));
        }
        all
    }
}

fn tokens(text: &str) -> Vec<String> {
    tokenize(&normalize(text)).map(str::to_owned).collect()
}

fn alt(term: &str, prefix: bool, kind: MatchKind, distance: u8) -> Alternative {
    Alternative {
        term: term.to_owned(),
        prefix,
        kind,
        distance,
        weight: alternative_weight(kind, distance),
    }
}

fn exact(term: &str, prefix: bool) -> Alternative {
    alt(term, prefix, MatchKind::Exact, 0)
}

/// Drops alternatives already matched by an earlier prefix alternative.
fn dedup_subsumed(all: &mut Vec<Alternative>) {
    let mut kept: Vec<Alternative> = Vec::with_capacity(all.len());
    all.drain(..).for_each(|a| {
        let covered = kept.iter().any(|k| {
            if k.prefix {
                a.term.starts_with(&k.term)
            } else {
                !a.prefix && k.term == a.term
            }
        });
        if !covered {
            kept.push(a);
        }
    });
    *all = kept;
}

impl ExpandedQuery {
    /// The top-level clauses that match text and must be found (not negated): what "coverage" counts.
    /// The store can run each one's `clause_to_fts5` over the candidate rows and pass the share of clauses
    /// a document matched (weighted by the matching alternative, if it wishes) to `fuse` as coverage.
    pub fn positive_text_clauses(&self) -> impl Iterator<Item = &ExpandedClause> {
        self.clauses.iter().filter(|c| match c {
            ExpandedClause::Terms { negated, .. } | ExpandedClause::Phrase { negated, .. } => {
                !negated
            }
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn vocab() -> Vocabulary {
        Vocabulary::new([
            ("receive", 3000),
            ("received", 1200),
            ("recieve", 3),
            ("invoice", 2500),
            ("invoices", 700),
            ("schema", 300),
            ("from", 9000),
            ("form", 800),
            ("jane", 400),
            ("doe", 300),
            ("meeting", 1000),
            ("lisbon", 20),
        ])
    }

    fn alts(input: &str) -> Vec<(String, bool, MatchKind)> {
        match &expand(&parse(input, 0), &vocab()).clauses[..] {
            [ExpandedClause::Terms { alternatives, .. }] => alternatives
                .iter()
                .map(|a| (a.term.clone(), a.prefix, a.kind))
                .collect(),
            other => panic!("{other:?}"),
        }
    }

    fn a(term: &str, prefix: bool, kind: MatchKind) -> (String, bool, MatchKind) {
        (term.to_owned(), prefix, kind)
    }

    fn terms_with(input: &str, options: &ExpandOptions) -> Vec<String> {
        match &expand_with(&parse(input, 0), &vocab(), options).clauses[..] {
            [ExpandedClause::Terms { alternatives, .. }] => alternatives.iter().map(|a| a.term.clone()).collect(),
            [] => Vec::new(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn typo_tolerance_is_a_setting() {
        let with = |typos| ExpandOptions { typos, ..ExpandOptions::default() };
        assert_eq!(terms_with("invocie ", &with(Typos::Normal)), ["invoice"]);
        assert_eq!(terms_with("invocie ", &with(Typos::Fewer)), ["invoice"], "seven letters, one typo");
        assert_eq!(terms_with("shcema ", &with(Typos::Fewer)), ["schema"]);
        assert_eq!(terms_with("lisbn ", &with(Typos::Fewer)), ["lisbn"], "five letters get none");
        assert_eq!(terms_with("invocie ", &with(Typos::Off)), ["invocie"], "kept as typed");
    }

    #[test]
    fn the_last_word_can_match_whole() {
        let whole = ExpandOptions { last_word: LastWord::Whole, ..ExpandOptions::default() };
        let alternatives = |options: &ExpandOptions| match &expand_with(&parse("receive", 0), &vocab(), options).clauses[..] {
            [ExpandedClause::Terms { alternatives, .. }] => alternatives.iter().map(|a| (a.term.clone(), a.prefix)).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert!(alternatives(&ExpandOptions::default()).contains(&("receive".into(), true)), "still typing: received too");
        assert_eq!(alternatives(&whole), [("receive".to_string(), false)]);
    }

    #[test]
    fn exact_word() {
        assert_eq!(alts("invoice "), [a("invoice", false, MatchKind::Exact)]);
    }

    #[test]
    fn typo_word() {
        assert_eq!(alts("invocie "), [a("invoice", false, MatchKind::Typo)]);
        assert_eq!(alts("shcema "), [a("schema", false, MatchKind::Typo)]);
    }

    #[test]
    fn rare_misspelling_also_gets_dominant_correction() {
        assert_eq!(
            alts("recieve "),
            [
                a("recieve", false, MatchKind::Exact),
                a("receive", false, MatchKind::Typo)
            ]
        );
    }

    #[test]
    fn valid_word_keeps_its_meaning() {
        assert_eq!(alts("form "), [a("form", false, MatchKind::Exact)]);
    }

    #[test]
    fn last_word_is_a_prefix() {
        assert_eq!(alts("invo"), [a("invo", true, MatchKind::Prefix)]);
        assert_eq!(alts("invoice"), [a("invoice", true, MatchKind::Exact)]);
    }

    #[test]
    fn last_word_with_typo_uses_fuzzy_prefix() {
        let got = alts("invoic3x");
        assert_eq!(got, [a("invoic3x", false, MatchKind::Exact)]);
        let got = alts("meetni");
        assert_eq!(got[0], a("meeti", true, MatchKind::PrefixTypo));
    }

    #[test]
    fn misspelled_complete_word_while_typing() {
        let got = alts("invocie");
        assert_eq!(got, [a("invoice", true, MatchKind::PrefixTypo)]);
    }

    #[test]
    fn prefix_fallback_for_unknown_word() {
        assert_eq!(alts("invoi "), [a("invoi", true, MatchKind::Prefix)]);
    }

    #[test]
    fn unknown_word_stays_exact() {
        assert_eq!(alts("zzzzzz "), [a("zzzzzz", false, MatchKind::Exact)]);
    }

    #[test]
    fn negated_word_is_not_expanded() {
        let q = expand(&parse("-invocie", 0), &vocab());
        assert_eq!(
            q.clauses,
            vec![ExpandedClause::Terms {
                field: Field::Any,
                source: "invocie".into(),
                alternatives: vec![exact("invocie", false)],
                negated: true,
            }]
        );
    }

    #[test]
    fn multi_token_word_becomes_phrase() {
        let q = expand(&parse("from:jane.doe@example.com x", 0), &vocab());
        assert_eq!(
            q.clauses[0],
            ExpandedClause::Phrase {
                field: Field::From,
                terms: vec!["jane".into(), "doe".into(), "example".into(), "com".into()],
                prefix: false,
                negated: false,
            }
        );
    }

    #[test]
    fn unterminated_phrase_gets_prefix() {
        let q = expand(&parse("\"trip to lis", 0), &vocab());
        assert_eq!(
            q.clauses,
            vec![ExpandedClause::Phrase {
                field: Field::Any,
                terms: vec!["trip".into(), "to".into(), "lis".into()],
                prefix: true,
                negated: false,
            }]
        );
    }

    #[test]
    fn prefix_applies_to_last_text_clause_only() {
        let q = expand(&parse("jane {invoice meet", 0), &vocab());
        let ExpandedClause::Terms { alternatives, .. } = &q.clauses[0] else {
            panic!()
        };
        assert!(!alternatives[0].prefix);
        let ExpandedClause::Any(cs) = &q.clauses[1] else {
            panic!()
        };
        let ExpandedClause::Terms { alternatives, .. } = &cs[1] else {
            panic!()
        };
        assert!(alternatives[0].prefix);
    }

    #[test]
    fn filters_pass_through() {
        let q = expand(&parse("is:unread", 0), &vocab());
        assert_eq!(
            q.clauses,
            vec![ExpandedClause::Filter {
                filter: Filter::Unread,
                negated: false
            }]
        );
    }

    #[test]
    fn weights_order() {
        let w = |k, d| alternative_weight(k, d);
        assert!(w(MatchKind::Exact, 0) > w(MatchKind::Prefix, 0));
        assert!(w(MatchKind::Prefix, 0) > w(MatchKind::Typo, 1));
        assert!(w(MatchKind::Typo, 1) > w(MatchKind::PrefixTypo, 1));
        assert!(w(MatchKind::PrefixTypo, 1) > w(MatchKind::Typo, 2));
    }

    #[test]
    fn positive_text_clauses_skip_filters_and_negations() {
        let q = expand(&parse("invoice -form is:unread \"jane doe\"", 0), &vocab());
        assert_eq!(q.positive_text_clauses().count(), 2);
    }
}
