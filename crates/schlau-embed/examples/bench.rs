//! Throughput benchmark.
//!
//! cargo run -p schlau-embed --release --example bench [-- <model cache dir>]
//!
//! Vector scoring always runs. Embedding runs when the model is already in the
//! cache dir (default: `$SCHLAU_MODEL_CACHE`), or downloads it when
//! `SCHLAU_BENCH_DOWNLOAD=1`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use schlau_embed::vector::{dot, l2_normalized, top_k};
use schlau_embed::{
    Embedder, ModelStore, POTION_MULTILINGUAL_128M, Precision, ScoreStats, StaticEmbedder,
    VectorIndex, email_passages,
};

const DIM: usize = 256;
const ROWS: usize = 200_000;

struct Lcg(u64);

impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }

    fn next_index(&mut self, n: usize) -> usize {
        (((self.next_f32() + 1.0) / 2.0) * n as f32) as usize % n
    }

    fn vector(&mut self, dim: usize) -> Vec<f32> {
        (0..dim).map(|_| self.next_f32()).collect()
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut last = f();
    let samples = (0..runs)
        .map(|_| {
            let start = Instant::now();
            last = f();
            start.elapsed()
        })
        .collect();
    (median(samples), last)
}

/// Vectors around 2000 topic centers, closer to real embeddings than uniform noise.
fn clustered(rng: &mut Lcg, rows: usize) -> Vec<Vec<f32>> {
    let centers: Vec<Vec<f32>> = (0..2000).map(|_| rng.vector(DIM)).collect();
    (0..rows)
        .map(|_| {
            let center = &centers[rng.next_index(centers.len())];
            let noise = rng.vector(DIM);
            l2_normalized(
                &center
                    .iter()
                    .zip(&noise)
                    .map(|(c, n)| c + 0.8 * n)
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn recall_at_10(index: &VectorIndex, vectors: &[Vec<f32>], queries: &[Vec<f32>]) -> f32 {
    let total: usize = queries
        .iter()
        .map(|q| {
            let exact: Vec<f32> = vectors.iter().map(|v| dot(q, v)).collect();
            let truth: Vec<i64> = top_k(&exact, 10)
                .into_iter()
                .map(|(i, _)| i as i64)
                .collect();
            index
                .search(q, 10)
                .unwrap()
                .iter()
                .filter(|hit| truth.contains(&hit.id))
                .count()
        })
        .sum();
    total as f32 / (queries.len() * 10) as f32
}

fn bench_vectors() {
    println!("== vector scoring: {ROWS} x {DIM} ==");
    let mut rng = Lcg(42);
    let vectors = clustered(&mut rng, ROWS);
    let queries: Vec<Vec<f32>> = (0..20).map(|i| vectors[i * 997].clone()).collect();
    for precision in [Precision::F16, Precision::I8] {
        let index = vectors.iter().enumerate().fold(
            VectorIndex::with_capacity(DIM, precision, ROWS),
            |mut index, (i, v)| {
                index.push(i as i64, v).unwrap();
                index
            },
        );
        let query = &queries[0];
        let (score_time, scores) = time(30, || index.score_all(query).unwrap());
        let (topk_time, _) = time(30, || top_k(&scores, 50));
        let (stats_time, _) = time(30, || ScoreStats::of(&scores));
        let (search_time, _) = time(30, || index.search_with_stats(query, 50).unwrap());
        let serial = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let (serial_time, _) = serial.install(|| time(10, || index.score_all(query).unwrap()));
        println!(
            "{precision:?}: {:.1} MB, score_all {:.2} ms (1 thread {:.2} ms), top_k(50) {:.2} ms, stats {:.2} ms, \
             search_with_stats {:.2} ms, recall@10 vs f32 {:.3}",
            index.memory_bytes() as f64 / 1e6,
            score_time.as_secs_f64() * 1e3,
            serial_time.as_secs_f64() * 1e3,
            topk_time.as_secs_f64() * 1e3,
            stats_time.as_secs_f64() * 1e3,
            search_time.as_secs_f64() * 1e3,
            recall_at_10(&index, &vectors, &queries),
        );
    }
}

const WORDS: &[&str] = &[
    "invoice",
    "meeting",
    "tomorrow",
    "project",
    "deadline",
    "attached",
    "please",
    "review",
    "budget",
    "flight",
    "booking",
    "confirmation",
    "Rechnung",
    "Termin",
    "morgen",
    "bitte",
    "Angebot",
    "réunion",
    "facture",
    "demain",
    "proyecto",
    "factura",
    "reunión",
    "password",
    "reset",
    "account",
    "delivery",
    "order",
    "shipped",
    "newsletter",
    "weekly",
    "update",
    "the",
    "and",
    "for",
    "with",
    "your",
    "our",
    "team",
    "release",
    "notes",
    "build",
    "failed",
];

fn synthetic_mail(rng: &mut Lcg) -> String {
    let words = 40 + rng.next_index(260);
    (0..words)
        .map(|_| WORDS[rng.next_index(WORDS.len())])
        .collect::<Vec<_>>()
        .join(" ")
}

fn bench_embedding(cache_dir: PathBuf) {
    let spec = &POTION_MULTILINGUAL_128M;
    let download = std::env::var_os("SCHLAU_BENCH_DOWNLOAD").is_some();
    if ModelStore::local(&cache_dir, spec).is_none() && !download {
        println!(
            "== embedding: skipped, model not in {} ==",
            cache_dir.display()
        );
        return;
    }
    println!("== embedding: {} ==", spec.repo);
    let start = Instant::now();
    let embedder = StaticEmbedder::from_store(&cache_dir, spec, |_| {}).unwrap();
    println!(
        "load (mmap + int8 quantize): {:.2} s",
        start.elapsed().as_secs_f64()
    );

    let mut rng = Lcg(7);
    let mails: Vec<String> = (0..20_000)
        .flat_map(|_| {
            email_passages(
                "Weekly update",
                "Alice <alice@example.com>",
                &synthetic_mail(&mut rng),
                &Default::default(),
            )
        })
        .collect();
    let texts: Vec<&str> = mails.iter().map(String::as_str).collect();
    let avg_chars = texts.iter().map(|t| t.len()).sum::<usize>() / texts.len();

    let start = Instant::now();
    let vectors = texts
        .chunks(1024)
        .flat_map(|batch| embedder.embed_documents(batch).unwrap())
        .count();
    let parallel = vectors as f64 / start.elapsed().as_secs_f64();

    let serial_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let sample = &texts[..2000];
    let start = Instant::now();
    serial_pool.install(|| embedder.embed_documents(sample).unwrap());
    let serial = sample.len() as f64 / start.elapsed().as_secs_f64();

    let queries = [
        "invoice for march",
        "flight booking confirmation",
        "Rechnung Termin morgen",
    ];
    let (query_time, _) = time(1000, || embedder.embed_query(queries[0]).unwrap());
    println!(
        "documents (avg {avg_chars} bytes): {parallel:.0} texts/s on all cores, {serial:.0} texts/s on 1 core; \
         100k mails ~ {:.1} s; query {:.1} us",
        100_000.0 / parallel,
        query_time.as_secs_f64() * 1e6
    );
}

fn main() {
    let cache_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SCHLAU_MODEL_CACHE").map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("schlau-embed-models"));
    bench_vectors();
    bench_embedding(cache_dir);
}
