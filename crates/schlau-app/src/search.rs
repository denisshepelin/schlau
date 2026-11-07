//! Search runs on `rune::Latest` workers so typing never waits on SQLite, and only the newest
//! query's answer lands. The vocabulary (for typo correction) and the vector index (for meaning)
//! are built once and rebuilt only after the replica changed, at most every few seconds.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use schlau_embed::{Embedder, MediaEmbedder, Precision, ScoreStats, VectorIndex};
use schlau_search::{MatchReason, MeaningScale, Vocabulary, parse_in_zone};
use schlau_store::{MeaningChannel, MeaningInput, Reader, ThreadRow};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    pub query: String,
    pub rows: Vec<ThreadRow>,
    pub reasons: Vec<MatchReason>,
    /// Per row, the attachment whose meaning matched, when that is why it is here.
    pub attachments: Vec<Option<String>>,
    pub elapsed_ms: f64,
    pub semantic: bool,
}

pub struct SearchRequest {
    pub path: PathBuf,
    pub query: String,
    /// The engine's data version: caches built at an older version are rebuilt.
    pub version: u64,
    pub embedder: Option<Arc<dyn Embedder>>,
    pub media: Option<Arc<dyn MediaEmbedder>>,
}

struct Cache {
    path: PathBuf,
    reader: Reader,
    vocabulary: Arc<Vocabulary>,
    vectors: Option<Arc<VectorIndex>>,
    media_vectors: Option<Arc<VectorIndex>>,
    /// Built while the multimodal model was loaded: without it there was no way to read media vectors.
    with_media: bool,
    version: u64,
    built: Instant,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

const REBUILD_AFTER: Duration = Duration::from_secs(5);
const MEANING_TOP: usize = 300;
const LIMIT: usize = 200;

fn vocabulary(reader: &Reader) -> Vocabulary {
    let terms = reader.vocabulary(1).unwrap_or_default();
    Vocabulary::new(terms.into_iter().map(|(t, d)| (t, d.min(u32::MAX as i64) as u32)))
}

fn index_of(rows: Vec<(i64, Vec<u8>)>, dim: usize) -> Option<VectorIndex> {
    if rows.is_empty() {
        return None;
    }
    VectorIndex::from_f16_blobs(dim, Precision::F16, rows.iter().map(|(id, b)| (*id, b.as_slice()))).ok()
}

fn vector_index(reader: &Reader, dim: usize) -> Option<VectorIndex> {
    let mut rows: Vec<(i64, Vec<u8>)> = Vec::new();
    reader.vectors(|id, blob| rows.push((id, blob.to_vec()))).ok()?;
    index_of(rows, dim)
}

fn media_vector_index(reader: &Reader, dim: usize) -> Option<VectorIndex> {
    let mut rows: Vec<(i64, Vec<u8>)> = Vec::new();
    reader.media_vectors(|id, blob| rows.push((id, blob.to_vec()))).ok()?;
    index_of(rows, dim)
}

/// The cached reader, vocabulary and vectors, rebuilt when stale.
fn with_cache<T>(request: &SearchRequest, f: impl FnOnce(&Cache) -> T) -> Option<T> {
    let mut slot = CACHE.lock().ok()?;
    let stale = slot.as_ref().is_none_or(|c| {
        c.path != request.path
            || (c.version != request.version && c.built.elapsed() > REBUILD_AFTER)
            || (c.vectors.is_none() && request.embedder.is_some() && c.version != request.version)
            || (request.media.is_some() && !c.with_media)
            || (c.media_vectors.is_none() && request.media.is_some() && c.version != request.version)
    });
    if stale {
        let reader = schlau_store::open_reader(&request.path).ok()?;
        let vectors = request.embedder.as_ref().and_then(|e| vector_index(&reader, e.dim())).map(Arc::new);
        let media_vectors = request.media.as_ref().and_then(|e| media_vector_index(&reader, e.dim())).map(Arc::new);
        *slot = Some(Cache {
            path: request.path.clone(),
            vocabulary: Arc::new(vocabulary(&reader)),
            reader,
            vectors,
            media_vectors,
            with_media: request.media.is_some(),
            version: request.version,
            built: Instant::now(),
        });
    }
    slot.as_ref().map(f)
}

fn utc_offset() -> i64 {
    jiff::Zoned::now().offset().seconds() as i64
}

/// The query without a last word of one character still being typed: as a prefix it matches a
/// large share of the index and says little about what the person is looking for.
pub fn settled(input: &str) -> &str {
    if input.ends_with(char::is_whitespace) {
        return input;
    }
    let start = input.rfind(char::is_whitespace).map_or(0, |i| i + 1);
    let last = &input[start..];
    let value = last.trim_start_matches('-').rsplit(':').next().unwrap_or("").trim_matches('"');
    if value.chars().count() <= 1 { input[..start].trim_end() } else { input }
}

pub fn run(request: SearchRequest) -> SearchResults {
    let started = Instant::now();
    let now = crate::format::now_unix();
    let query = parse_in_zone(settled(&request.query), now, utc_offset());
    let found = with_cache(&request, |cache| {
        let text = query.semantic_text.trim();
        let wanted = !text.is_empty();
        let words = match (&request.embedder, &cache.vectors) {
            (Some(embedder), Some(vectors)) if wanted => top_hits(vectors, embedder.embed_query(text).ok()),
            _ => None,
        };
        let media = match (&request.media, &cache.media_vectors) {
            (Some(embedder), Some(vectors)) if wanted => top_hits(vectors, embedder.embed_query(text).ok()),
            _ => None,
        };
        let channels: Vec<MeaningChannel<'_>> = [(&words, false), (&media, true)]
            .into_iter()
            .filter_map(|(hits, attachments)| hits.as_ref().map(|(top, scale)| MeaningChannel { scale: *scale, top, attachments }))
            .collect();
        let semantic = !channels.is_empty();
        let input = semantic.then_some(MeaningInput { channels });
        (cache.reader.search(&query, &cache.vocabulary, input, now, LIMIT).unwrap_or_default(), semantic)
    });
    let (hits, semantic) = found.unwrap_or_default();
    SearchResults {
        query: request.query,
        reasons: hits.iter().map(|h| h.why).collect(),
        attachments: hits.iter().map(|h| h.attachment.clone()).collect(),
        rows: hits.into_iter().map(|h| h.row).collect(),
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        semantic,
    }
}

/// The query's best vectors in one index, and the scale of its scores over all of them.
fn top_hits(vectors: &VectorIndex, query: Option<Vec<f32>>) -> Option<(Vec<(i64, f32)>, MeaningScale)> {
    let (hits, stats) = vectors.search_with_stats(&query?, MEANING_TOP).ok()?;
    Some((hits.into_iter().map(|h| (h.id, h.score)).collect(), scale(stats)))
}

fn scale(stats: ScoreStats) -> MeaningScale {
    MeaningScale { mean: stats.mean as f64, deviation: (stats.std as f64).max(schlau_search::MIN_DEVIATION) }
}

/// Warms the cache in the background so the first search after launch is fast.
pub fn warm(path: &Path, version: u64, embedder: Option<Arc<dyn Embedder>>) {
    let request = SearchRequest { path: path.to_path_buf(), query: String::new(), version, embedder, media: None };
    let _ = std::thread::Builder::new().name("schlau-search-warm".into()).spawn(move || with_cache(&request, |_| ()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Types a set of queries one keystroke at a time and reports latency percentiles, as the
    /// search box sees them. Run in release: `SCHLAU_TEST_DB=… cargo test --release -p schlau-app
    /// keystroke_latency -- --ignored --nocapture`.
    #[test]
    fn a_one_letter_word_being_typed_waits_for_the_next() {
        assert_eq!(settled("w"), "");
        assert_eq!(settled("weekly d"), "weekly");
        assert_eq!(settled("from:s"), "");
        assert_eq!(settled("from:st"), "from:st");
        assert_eq!(settled("weekly d "), "weekly d ");
        assert_eq!(settled("is:unread"), "is:unread");
    }

    #[test]
    #[ignore = "needs SCHLAU_TEST_DB pointing at a demo database"]
    fn keystroke_latency() {
        let path = PathBuf::from(std::env::var("SCHLAU_TEST_DB").unwrap());
        let queries = [
            "weekly dgest",
            "from:stripe receipt",
            "kangaroo budget",
            "is:unread in:inbox",
            "invocie october",
            "deploymnt pipeline flaky",
            "has:attachment after:2026-06-01 photos",
            "-from:github lunch",
            "\"design review\"",
            "contract renewal annual billing",
        ];
        let mut warm = SearchRequest { path: path.clone(), query: "x".into(), version: 1, embedder: None, media: None };
        run(SearchRequest { query: "warm".into(), ..warm });
        warm = SearchRequest { path, query: String::new(), version: 1, embedder: None, media: None };
        let mut times: Vec<f64> = queries
            .iter()
            .flat_map(|q| (1..=q.len()).filter(|&i| q.is_char_boundary(i)).map(move |i| q[..i].to_string()))
            .map(|q| {
                let r = run(SearchRequest { query: q.clone(), path: warm.path.clone(), version: 1, embedder: None, media: None });
                if r.elapsed_ms > 100.0 {
                    eprintln!("slow {q:?}: {:.0} ms, {} rows", r.elapsed_ms, r.rows.len());
                }
                r.elapsed_ms
            })
            .collect();
        times.sort_by(f64::total_cmp);
        let at = |p: f64| times[((times.len() - 1) as f64 * p) as usize];
        eprintln!("{} keystrokes: p50 {:.1} ms, p90 {:.1} ms, p99 {:.1} ms, max {:.1} ms", times.len(), at(0.5), at(0.9), at(0.99), at(1.0));
    }
}

#[cfg(test)]
mod explain {
    use super::*;

    /// Prints the top results of `SCHLAU_EXPLAIN` against `SCHLAU_TEST_DB`, with the image model when it is
    /// downloaded: `SCHLAU_TEST_DB=… SCHLAU_EXPLAIN=tesla cargo test --release -p schlau-app explain -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs SCHLAU_TEST_DB and SCHLAU_EXPLAIN"]
    fn explain() {
        let path = PathBuf::from(std::env::var("SCHLAU_TEST_DB").unwrap());
        let query = std::env::var("SCHLAU_EXPLAIN").unwrap();
        let models = PathBuf::from(std::env::var("HOME").unwrap()).join("Library/Application Support/schlau/models");
        let media = schlau_embed::ModelStore::local(&models, &schlau_embed::EMBEDDINGGEMMA_2)
            .and_then(|p| schlau_embed::GemmaEmbedder::load(&p.dir, schlau_embed::Towers::Text).ok())
            .map(|m| Arc::new(m) as Arc<dyn MediaEmbedder>);
        let now = crate::format::now_unix();
        let parsed = parse_in_zone(&query, now, utc_offset());
        let request = SearchRequest { path, query: query.clone(), version: 1, embedder: None, media };
        with_cache(&request, |cache| {
            let media = match (&request.media, &cache.media_vectors) {
                (Some(e), Some(v)) => top_hits(v, e.embed_query(&query).ok()),
                _ => None,
            };
            let channels = media.iter().map(|(top, scale)| MeaningChannel { scale: *scale, top, attachments: true }).collect();
            let hits = cache.reader.search(&parsed, &cache.vocabulary, Some(MeaningInput { channels }), now, 8).unwrap();
            hits.iter().for_each(|h| {
                let subject: String = h.row.subject.chars().take(48).collect();
                eprintln!("{:6.2} {:?} {subject:<48} {}", h.score, h.why, h.attachment.clone().unwrap_or_default());
            });
        });
    }
}
