//! Hybrid ranking of word hits (FTS5) and meaning hits (embeddings), ported from DigUp's `Searcher.search`.
//!
//! Meaning is put on one scale per query: every stored vector's cosine is scored anyway, so the
//! distribution comes for free, and a document's meaning score is how many standard deviations its best
//! vector stands above the mean (`z`). Only documents that stand out (`z >= z_gate`) count as meaning
//! matches. A document qualifies by meaning, or by containing at least `coverage_gate` of the query's words.
//!
//! ```text
//! score = meaning_weight * max(z, 0)
//!       + words_weight * coverage^1.5          (coverage: share of the query's words found, 0..=1)
//!       + bm25_weight * bm25 / best_bm25       (FTS5 bm25 relative to the best word hit, 0..=1)
//!       + field_bonus                          (best word match in the subject or sender)
//!       + recency_weight * 0.5^(age_days / recency_half_life_days)
//! ```
//!
//! `meaning_weight` is 1 and `words_weight` is `word_weight` (2), unless the query is a lookup
//! (`is_lookup`: IDs, numbers, email addresses, quoted names), which weighs words double and meaning half.
//! One common word can't beat a strong meaning match; all the words of an exact lookup usually do. Recency is
//! a tie-breaker: at most 0.15, a tenth of a typical z, halving every 30 days.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::query::{Clause, Field, Query};

/// A document id, as the store keys messages.
pub type DocId = i64;

/// A document found by words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LexicalHit {
    /// The document.
    pub doc: DocId,
    /// FTS5 `bm25()` of the document: negative, lower is better.
    pub bm25: f64,
    /// Share of the query's positive text clauses the document matched, `0.0..=1.0` (optionally weighted by
    /// the alternative that matched, see `Alternative::weight`).
    pub coverage: f32,
    /// The field the best match was in.
    pub field: Field,
    /// When the message was received, seconds since the Unix epoch.
    pub received_at: i64,
}

/// One scored vector. A document may have several (chunks); its best counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SemanticHit {
    /// The document the vector belongs to.
    pub doc: DocId,
    /// Cosine similarity with the query vector.
    pub cosine: f32,
    /// When the message was received, seconds since the Unix epoch.
    pub received_at: i64,
}

/// The mean and standard deviation of one query's cosines over all stored vectors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeaningScale {
    /// Mean cosine.
    pub mean: f64,
    /// Standard deviation of the cosines, at least `MIN_DEVIATION`.
    pub deviation: f64,
}

/// Floor for `MeaningScale::deviation`, so a flat distribution doesn't blow z up.
pub const MIN_DEVIATION: f64 = 0.02;

impl MeaningScale {
    /// The scale of a set of cosines (population standard deviation).
    pub fn from_cosines(cosines: impl IntoIterator<Item = f32>) -> MeaningScale {
        let (n, sum, sq) = cosines
            .into_iter()
            .fold((0.0f64, 0.0f64, 0.0f64), |(n, s, q), c| {
                let c = f64::from(c);
                (n + 1.0, s + c, q + c * c)
            });
        let mean = if n > 0.0 { sum / n } else { 0.0 };
        let var = if n > 0.0 {
            (sq / n - mean * mean).max(0.0)
        } else {
            0.0
        };
        MeaningScale {
            mean,
            deviation: var.sqrt().max(MIN_DEVIATION),
        }
    }

    /// How many deviations `cosine` stands above the mean.
    pub fn z(&self, cosine: f32) -> f64 {
        (f64::from(cosine) - self.mean) / self.deviation
    }
}

/// The meaning side of a search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Meaning<'a> {
    /// No embedding (no model, or the query has no free text).
    None,
    /// Every scored vector: the scale is computed from them.
    All(&'a [SemanticHit]),
    /// A scale computed over all vectors elsewhere, and the best vectors only.
    Summary {
        /// The scale over all vectors.
        scale: MeaningScale,
        /// The best vectors (any order).
        top: &'a [SemanticHit],
    },
}

/// Why a document is in the results, for the UI to explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchReason {
    /// It contains enough of the query's words.
    Words,
    /// Its meaning stands out for the query.
    Meaning,
    /// Both.
    Both,
}

/// A ranked result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The document.
    pub doc: DocId,
    /// Fused score; higher is better.
    pub score: f64,
    /// Why it matched.
    pub why: MatchReason,
    /// Its meaning z-score, if it has a vector that was scored.
    pub z: Option<f64>,
    /// Its word coverage (0 when not found by words).
    pub coverage: f32,
}

/// Knobs for `fuse_with`. Defaults follow DigUp's evaluated values where they exist.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionOptions {
    /// Least z for a meaning match.
    pub z_gate: f64,
    /// Least coverage for a words match.
    pub coverage_gate: f32,
    /// Weight of `coverage^1.5`.
    pub word_weight: f64,
    /// Weight of the bm25 relative to the best word hit.
    pub bm25_weight: f64,
    /// Bonus when the best word match is in the subject or sender.
    pub field_bonus: f64,
    /// Recency bonus of a message received now.
    pub recency_weight: f64,
    /// Days for the recency bonus to halve.
    pub recency_half_life_days: f64,
    /// The most a meaning score can add: below a full word match (`word_weight` plus the bonuses), so a
    /// document found only by meaning never outranks one that holds the words.
    pub meaning_cap: f64,
}

impl Default for FusionOptions {
    fn default() -> Self {
        FusionOptions {
            z_gate: 2.0,
            coverage_gate: 0.5,
            word_weight: 2.0,
            bm25_weight: 0.3,
            field_bonus: 0.25,
            recency_weight: 0.15,
            recency_half_life_days: 30.0,
            meaning_cap: 1.8,
        }
    }
}

/// `fuse_with` default options.
pub fn fuse(
    query: &Query,
    lexical: &[LexicalHit],
    meaning: Meaning<'_>,
    now_unix: i64,
    limit: usize,
) -> Vec<Hit> {
    fuse_with(
        query,
        lexical,
        meaning,
        now_unix,
        limit,
        &FusionOptions::default(),
    )
}

#[derive(Default)]
struct Doc {
    z: Option<f64>,
    lexical: Option<LexicalHit>,
    received_at: i64,
}

/// Fuses word and meaning hits into one ranking, best first, at most `limit`. Ties go to the newer message,
/// then the lower id.
pub fn fuse_with(
    query: &Query,
    lexical: &[LexicalHit],
    meaning: Meaning<'_>,
    now_unix: i64,
    limit: usize,
    options: &FusionOptions,
) -> Vec<Hit> {
    let mut docs: IdMap<Doc> =
        IdMap::with_capacity_and_hasher(lexical.len() * 2, Default::default());
    lexical.iter().for_each(|h| {
        let d = docs.entry(h.doc).or_default();
        d.received_at = h.received_at;
        if d.lexical.is_none_or(|l| {
            h.coverage > l.coverage || (h.coverage == l.coverage && h.bm25 < l.bm25)
        }) {
            d.lexical = Some(*h);
        }
    });
    let (scale, vectors) = match meaning {
        Meaning::None => (None, &[][..]),
        Meaning::All(all) => (
            Some(MeaningScale::from_cosines(all.iter().map(|h| h.cosine))),
            all,
        ),
        Meaning::Summary { scale, top } => (Some(scale), top),
    };
    if let Some(scale) = scale {
        vectors.iter().for_each(|v| {
            let z = scale.z(v.cosine);
            let keep = z >= options.z_gate;
            let entry = if keep {
                Some(docs.entry(v.doc).or_default())
            } else {
                docs.get_mut(&v.doc)
            };
            if let Some(d) = entry {
                d.received_at = d.received_at.max(v.received_at);
                d.z = Some(d.z.map_or(z, |old| old.max(z)));
            }
        });
    }

    let lookup = is_lookup(query);
    let (meaning_weight, words_weight) = if lookup {
        (0.5, options.word_weight * 2.0)
    } else {
        (1.0, options.word_weight)
    };
    let best_bm25 = lexical.iter().map(|h| h.bm25).fold(0.0f64, f64::min);

    let mut hits: Vec<(Hit, i64)> = docs
        .into_iter()
        .filter_map(|(doc, d)| {
            let coverage = d.lexical.map_or(0.0, |l| l.coverage.clamp(0.0, 1.0));
            let by_words = d.lexical.is_some() && coverage >= options.coverage_gate;
            let by_meaning = d.z.is_some_and(|z| z >= options.z_gate);
            let why = match (by_words, by_meaning) {
                (true, true) => MatchReason::Both,
                (true, false) => MatchReason::Words,
                (false, true) => MatchReason::Meaning,
                (false, false) => return None,
            };
            let words = d.lexical.map_or(0.0, |l| {
                let rel_bm25 = if best_bm25 < 0.0 {
                    (l.bm25 / best_bm25).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let field = if matches!(l.field, Field::Subject | Field::From) {
                    options.field_bonus
                } else {
                    0.0
                };
                words_weight * f64::from(coverage).powf(1.5)
                    + options.bm25_weight * rel_bm25
                    + field
            });
            let score = meaning_weight * meaning_score(d.z, options)
                + words
                + recency(now_unix, d.received_at, options);
            Some((
                Hit {
                    doc,
                    score,
                    why,
                    z: d.z,
                    coverage,
                },
                d.received_at,
            ))
        })
        .collect();
    hits.sort_unstable_by(|(a, at), (b, bt)| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| bt.cmp(at))
            .then_with(|| a.doc.cmp(&b.doc))
    });
    hits.truncate(limit);
    hits.into_iter().map(|(h, _)| h).collect()
}

/// A z-score as a bounded score: 0 at the gate's half, rising quickly over the gate and leveling off
/// towards `meaning_cap`, so standing out matters but standing out a lot can't swamp the words.
fn meaning_score(z: Option<f64>, options: &FusionOptions) -> f64 {
    let z = z.unwrap_or(0.0).max(0.0);
    options.meaning_cap * (1.0 - (-z / options.z_gate.max(f64::EPSILON)).exp())
}

/// The recency tie-breaker: `recency_weight * 0.5^(age_days / recency_half_life_days)`; messages from the
/// future count as new.
pub fn recency(now_unix: i64, received_at: i64, options: &FusionOptions) -> f64 {
    let age_days = (now_unix - received_at).max(0) as f64 / 86_400.0;
    options.recency_weight
        * 0.5f64.powf(age_days / options.recency_half_life_days.max(f64::EPSILON))
}

/// Whether the query looks up something exact rather than describing it: every free word (of any field,
/// subject or body; sender and recipient values don't count) is an ID or number (has a digit, and a letter
/// or at least 4 characters: `K7Q2LM`, `TRX50`, `4815162342`) or an email address, or the query quotes a
/// phrase. Such text means nothing to an embedding but everything to words.
pub fn is_lookup(query: &Query) -> bool {
    let mut words = Vec::new();
    let mut quoted = false;
    fn walk<'q>(c: &'q Clause, words: &mut Vec<&'q str>, quoted: &mut bool) {
        match c {
            Clause::Word {
                text,
                field: Field::Any | Field::Subject | Field::Body,
                negated: false,
            } => words.push(text),
            Clause::Phrase { negated: false, .. } => *quoted = true,
            Clause::Any(cs) | Clause::All(cs) => cs.iter().for_each(|c| walk(c, words, quoted)),
            _ => {}
        }
    }
    query
        .clauses
        .iter()
        .for_each(|c| walk(c, &mut words, &mut quoted));
    quoted || (!words.is_empty() && words.iter().all(|w| is_identifier(w) || is_email(w)))
}

fn is_identifier(word: &str) -> bool {
    word.chars().any(|c| c.is_numeric())
        && (word.chars().any(char::is_alphabetic) || word.chars().count() >= 4)
}

fn is_email(word: &str) -> bool {
    word.split_once('@')
        .is_some_and(|(user, host)| !user.is_empty() && host.contains('.') && !host.ends_with('.'))
}

/// How many of `hits` (best first) are strong: scoring at least `ratio` times the top hit. The rest pass
/// the gates but trail far behind; the UI can fold them under "weaker matches". DigUp uses 0.5.
pub fn strong_count(hits: &[Hit], ratio: f64) -> usize {
    match hits.first() {
        Some(top) if top.score > 0.0 => hits
            .iter()
            .take_while(|h| h.score >= top.score * ratio)
            .count(),
        _ => hits.len(),
    }
}

type IdMap<V> = HashMap<DocId, V, BuildHasherDefault<IdHasher>>;

#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = bytes.iter().fold(self.0, |h, &b| {
            (h.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95)
        });
    }

    fn write_i64(&mut self, n: i64) {
        self.0 = (n as u64 ^ 0x9E37_79B9_7F4A_7C15)
            .wrapping_mul(0xBF58_476D_1CE4_E5B9)
            .rotate_left(31);
    }
}

#[cfg(test)]
mod tests;
