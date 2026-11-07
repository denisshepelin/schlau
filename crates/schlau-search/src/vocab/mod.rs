//! The index's vocabulary, for typo-tolerant term correction and prefix completion.
//!
//! Terms are kept sorted in one string arena (exact lookup and prefix ranges by binary search), and bucketed
//! by length in characters with a 64-bit character signature each. Correction scans only the buckets within
//! the edit budget, rejects most candidates with one XOR and popcount on the signatures, and runs a banded
//! edit distance on the rest. Measured on a synthetic 500k-term vocabulary: see `examples/bench.rs`.

mod distance;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;

use distance::{MAX_CORRECTABLE, osa_within, signature};

/// How a suggested term relates to what was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MatchKind {
    /// The term as typed.
    Exact,
    /// A term that starts with what was typed.
    Prefix,
    /// A term within the typo budget of what was typed.
    Typo,
    /// A term that starts with something one typo away from what was typed.
    PrefixTypo,
}

/// A term from the vocabulary suggested for a typed term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    /// The vocabulary term (normalized).
    pub term: String,
    /// How it relates to the typed term.
    pub kind: MatchKind,
    /// Edits between the typed term and the term (or its prefix of the typed length, for `PrefixTypo`).
    pub distance: u8,
    /// Number of documents containing the term.
    pub df: u32,
}

/// Terms of the full-text index with their document frequencies.
///
/// Build it from `fts5vocab` (`SELECT term, doc FROM fts_vocab` on a `row` vocabulary table) on a background
/// thread and swap it in; it is immutable once built. Terms must be normalized (`crate::normalize`), as FTS5
/// stores them when fed normalized text.
#[derive(Debug, Clone, Default)]
pub struct Vocabulary {
    arena: String,
    offsets: Vec<u32>,
    df: Vec<u32>,
    cumulative_df: Vec<u64>,
    buckets: Vec<Bucket>,
}

#[derive(Debug, Clone, Default)]
struct Bucket {
    ids: Vec<u32>,
    signatures: Vec<u64>,
}

/// Typos allowed for a term of `len` characters: none below 4, one up to 7, two from 8.
pub fn typo_budget(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

impl Vocabulary {
    /// Builds a vocabulary from `(term, document frequency)` pairs. Duplicate terms have their frequencies
    /// summed; empty terms are skipped.
    pub fn new<S: AsRef<str>>(entries: impl IntoIterator<Item = (S, u32)>) -> Vocabulary {
        let mut items: Vec<(String, u32)> = entries
            .into_iter()
            .filter(|(t, _)| !t.as_ref().is_empty())
            .map(|(t, df)| (t.as_ref().to_owned(), df))
            .collect();
        items.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        items.dedup_by(|next, kept| {
            let same = next.0 == kept.0;
            if same {
                kept.1 = kept.1.saturating_add(next.1);
            }
            same
        });

        let arena_len = items.iter().map(|(t, _)| t.len()).sum();
        let mut arena = String::with_capacity(arena_len);
        let mut offsets = Vec::with_capacity(items.len() + 1);
        let mut buckets = vec![Bucket::default(); MAX_CORRECTABLE + 1];
        offsets.push(0);
        items.iter().enumerate().for_each(|(id, (term, _))| {
            arena.push_str(term);
            offsets.push(arena.len() as u32);
            let len = term.chars().count();
            if let Some(bucket) = buckets.get_mut(len) {
                bucket.ids.push(id as u32);
                bucket.signatures.push(signature(term.chars()));
            }
        });
        let df: Vec<u32> = items.iter().map(|(_, df)| *df).collect();
        let cumulative_df = std::iter::once(0)
            .chain(df.iter().scan(0u64, |acc, &d| {
                *acc += u64::from(d);
                Some(*acc)
            }))
            .collect();
        buckets.iter_mut().for_each(|b| {
            b.ids.shrink_to_fit();
            b.signatures.shrink_to_fit();
        });
        Vocabulary {
            arena,
            offsets,
            df,
            cumulative_df,
            buckets,
        }
    }

    /// Number of distinct terms.
    pub fn len(&self) -> usize {
        self.df.len()
    }

    /// Whether there are no terms.
    pub fn is_empty(&self) -> bool {
        self.df.is_empty()
    }

    /// Approximate heap memory used, in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.arena.capacity()
            + self.offsets.capacity() * 4
            + self.df.capacity() * 4
            + self.cumulative_df.capacity() * 8
            + self
                .buckets
                .iter()
                .map(|b| b.ids.capacity() * 4 + b.signatures.capacity() * 8)
                .sum::<usize>()
    }

    fn term(&self, id: u32) -> &str {
        let id = id as usize;
        &self.arena[self.offsets[id] as usize..self.offsets[id + 1] as usize]
    }

    fn lookup(&self, term: &str) -> Option<u32> {
        let i = self.lower_bound(term);
        (i < self.len() && self.term(i as u32) == term).then_some(i as u32)
    }

    fn lower_bound(&self, term: &str) -> usize {
        partition(self.len(), |i| self.term(i as u32) < term)
    }

    /// Ids of the terms that start with `prefix`.
    fn prefix_range(&self, prefix: &str) -> Range<usize> {
        let start = self.lower_bound(prefix);
        let len = partition(self.len() - start, |i| {
            self.term((start + i) as u32).starts_with(prefix)
        });
        start..start + len
    }

    fn range_df(&self, range: &Range<usize>) -> u64 {
        self.cumulative_df[range.end] - self.cumulative_df[range.start]
    }

    /// Document frequency of `term`, if it is in the vocabulary.
    pub fn df(&self, term: &str) -> Option<u32> {
        self.lookup(term).map(|id| self.df[id as usize])
    }

    /// Whether any term starts with `prefix`.
    pub fn has_prefix(&self, prefix: &str) -> bool {
        !self.prefix_range(prefix).is_empty()
    }

    /// The term itself if present, plus terms within the typo budget (`typo_budget`): ranked by edit distance,
    /// then document frequency. Terms with digits are never corrected (`2024` must not become `2025`).
    ///
    /// `term` must be one normalized token (`crate::tokenize`).
    pub fn correct(&self, term: &str, max_suggestions: usize) -> Vec<Correction> {
        let exact = self.lookup(term);
        let exact_hit = exact.map(|id| self.correction(id, MatchKind::Exact, 0));
        let chars: Vec<char> = term.chars().collect();
        let budget = if chars.iter().all(|c| c.is_alphabetic()) {
            typo_budget(chars.len())
        } else {
            0
        };
        let mut typos: Vec<(u8, Reverse<u32>, u32)> = if budget == 0 {
            Vec::new()
        } else {
            self.within(&chars, budget)
                .filter(|&(_, id)| Some(id) != exact)
                .map(|(d, id)| (d, Reverse(self.df[id as usize]), id))
                .collect()
        };
        typos.sort_unstable();
        exact_hit
            .into_iter()
            .chain(
                typos
                    .into_iter()
                    .map(|(d, _, id)| self.correction(id, MatchKind::Typo, d)),
            )
            .take(max_suggestions)
            .collect()
    }

    fn within<'s>(&'s self, query: &'s [char], k: usize) -> impl Iterator<Item = (u8, u32)> + 's {
        let q_sig = signature(query.iter().copied());
        let q_bytes: Option<Vec<u8>> = query
            .iter()
            .all(char::is_ascii)
            .then(|| query.iter().map(|&c| c as u8).collect());
        let lengths = query.len().saturating_sub(k).max(1)..=(query.len() + k).min(MAX_CORRECTABLE);
        let max_bits = 2 * k as u32;
        lengths.flat_map(move |len| {
            let bucket = &self.buckets[len];
            let q_bytes = q_bytes.clone();
            bucket
                .signatures
                .iter()
                .zip(&bucket.ids)
                .filter(move |&(&sig, _)| (sig ^ q_sig).count_ones() <= max_bits)
                .filter_map(move |(_, &id)| {
                    let term = self.term(id);
                    let d = match (&q_bytes, term.is_ascii()) {
                        (Some(q), true) => osa_within(q, term.as_bytes(), k),
                        _ => {
                            let mut buf = ['\0'; MAX_CORRECTABLE];
                            let n = term
                                .chars()
                                .zip(buf.iter_mut())
                                .map(|(c, slot)| *slot = c)
                                .count();
                            osa_within(query, &buf[..n], k)
                        }
                    };
                    d.map(|d| (d, id))
                })
        })
    }

    /// Terms starting with `prefix`, most frequent first (the term itself counts, as `Exact`). When no term
    /// starts with `prefix` and it has at least 4 characters, terms starting with a one-typo variant of it
    /// (`PrefixTypo`).
    pub fn complete(&self, prefix: &str, max: usize) -> Vec<Correction> {
        let range = self.prefix_range(prefix);
        if !range.is_empty() {
            return self
                .top_by_df(std::iter::once(range), max)
                .into_iter()
                .map(|id| {
                    let kind = if self.term(id) == prefix {
                        MatchKind::Exact
                    } else {
                        MatchKind::Prefix
                    };
                    self.correction(id, kind, 0)
                })
                .collect();
        }
        let ranges = self.fuzzy_prefix_ranges(prefix).into_iter().map(|(_, r)| r);
        self.top_by_df(ranges, max)
            .into_iter()
            .map(|id| self.correction(id, MatchKind::PrefixTypo, 1))
            .collect()
    }

    /// One-typo variants of `prefix` that some term starts with, with the total document frequency of those
    /// terms, most frequent first. Empty for prefixes under 4 characters, with digits, or that already start
    /// a term.
    pub fn fuzzy_prefixes(&self, prefix: &str, max: usize) -> Vec<(String, u64)> {
        let mut found: Vec<(String, u64)> = self
            .fuzzy_prefix_ranges(prefix)
            .into_iter()
            .map(|(variant, range)| {
                let df = self.range_df(&range);
                (variant, df)
            })
            .collect();
        found.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        found.truncate(max);
        found
    }

    fn fuzzy_prefix_ranges(&self, prefix: &str) -> Vec<(String, Range<usize>)> {
        let chars: Vec<char> = prefix.chars().collect();
        if chars.len() < 4 || !chars.iter().all(|c| c.is_alphabetic()) || self.has_prefix(prefix) {
            return Vec::new();
        }
        let mut variants = one_edit_variants(&chars);
        variants.sort_unstable();
        variants.dedup();
        variants
            .into_iter()
            .filter_map(|v| {
                let range = self.prefix_range(&v);
                (!range.is_empty()).then_some((v, range))
            })
            .collect()
    }

    fn top_by_df(&self, ranges: impl Iterator<Item = Range<usize>>, max: usize) -> Vec<u32> {
        let mut heap: BinaryHeap<Reverse<(u32, Reverse<u32>)>> = BinaryHeap::with_capacity(max + 1);
        merge_ranges(ranges.collect())
            .into_iter()
            .flatten()
            .for_each(|id| {
                let item = Reverse((self.df[id], Reverse(id as u32)));
                if heap.len() < max {
                    heap.push(item);
                } else if heap.peek().is_some_and(|min| item < *min) {
                    heap.pop();
                    heap.push(item);
                }
            });
        let mut ids: Vec<(u32, Reverse<u32>)> = heap.into_iter().map(|Reverse(x)| x).collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.into_iter().map(|(_, Reverse(id))| id).collect()
    }

    fn correction(&self, id: u32, kind: MatchKind, distance: u8) -> Correction {
        Correction {
            term: self.term(id).to_owned(),
            kind,
            distance,
            df: self.df[id as usize],
        }
    }
}

/// Deletions, adjacent transpositions, substitutions and insertions of one character. Substituted and inserted
/// characters are ASCII letters and the term's own non-ASCII characters.
fn one_edit_variants(chars: &[char]) -> Vec<String> {
    let mut alphabet: Vec<char> = ('a'..='z')
        .chain(chars.iter().copied().filter(|c| !c.is_ascii()))
        .collect();
    alphabet.sort_unstable();
    alphabet.dedup();
    let n = chars.len();
    let build = |f: &dyn Fn(&mut Vec<char>)| -> String {
        let mut v = chars.to_vec();
        f(&mut v);
        v.into_iter().collect()
    };
    let deletions = (0..n).map(|i| {
        build(&|v| {
            v.remove(i);
        })
    });
    let transpositions = (0..n.saturating_sub(1)).map(|i| build(&|v| v.swap(i, i + 1)));
    let substitutions = (0..n).flat_map(|i| {
        alphabet
            .iter()
            .filter(move |&&c| c != chars[i])
            .map(move |&c| build(&|v| v[i] = c))
    });
    let insertions =
        (0..=n).flat_map(|i| alphabet.iter().map(move |&c| build(&|v| v.insert(i, c))));
    deletions
        .chain(transpositions)
        .chain(substitutions)
        .chain(insertions)
        .collect()
}

fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_unstable_by_key(|r| (r.start, Reverse(r.end)));
    ranges
        .into_iter()
        .fold(Vec::new(), |mut merged: Vec<Range<usize>>, r| {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
            merged
        })
}

/// The first index in `0..len` for which `pred` is false, given `pred` is true then false.
fn partition(len: usize, pred: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

#[cfg(test)]
mod tests;
