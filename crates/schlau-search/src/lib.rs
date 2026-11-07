//! Search for schlau: the query language, text normalization shared with the FTS5 index, typo-tolerant term
//! correction, fuzzy matching of short fields, and hybrid ranking of word and meaning hits.
//!
//! Pure: no I/O. `schlau-store` feeds it vocabulary and hits and executes what it produces.
//!
//! The pipeline for one keystroke:
//!
//! 1. `parse` the input into a `Query` (and `tokens` for highlighting).
//! 2. `expand` it against the `Vocabulary` (built from the FTS5 vocabulary), resolving typos and prefixes.
//! 3. `to_fts5_match` for the MATCH expression; apply `Fts5Match::excluded` and `residual` filters in SQL.
//! 4. Embed `Query::semantic_text`, score vectors, and `fuse` word and meaning hits into ranked `Hit`s.

#![warn(missing_docs)]

mod expand;
mod fts5;
mod fusion;
mod fuzzy;
mod normalize;
mod parse;
mod query;
mod vocab;

pub use expand::{
    Alternative, ExpandOptions, ExpandedClause, ExpandedQuery, LastWord, alternative_weight, expand,
    expand_with,
};
pub use fts5::{Fts5Match, Join, clause_to_fts5, quote_term, to_fts5_match};
pub use fusion::{
    DocId, FusionOptions, Hit, LexicalHit, MIN_DEVIATION, MatchReason, Meaning, MeaningScale,
    SemanticHit, fuse, fuse_with, is_lookup, recency, strong_count,
};
pub use fuzzy::{FuzzyMatch, fuzzy_rank};
pub use normalize::{is_token_char, normalize, terms, tokenize};
pub use parse::{Token, TokenKind, parse, parse_in_zone, parse_with_tokens, tokens};
pub use query::{Clause, Field, Filter, Query};
pub use vocab::{Correction, MatchKind, Typos, Vocabulary, typo_budget};
