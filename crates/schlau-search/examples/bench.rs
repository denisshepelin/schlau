//! Timing for the per-keystroke parts of search: `cargo run --release -p schlau-search --example bench`.
//!
//! Builds a synthetic 500k-term vocabulary (English-like letter frequencies, lengths 2..=18, Zipf document
//! frequencies) and times `Vocabulary::correct` and `Vocabulary::complete` on misspelled queries, `parse` +
//! `expand` + `to_fts5_match`, `fuzzy_rank` over 500 command names, and fusion over 100k vectors.

use std::hint::black_box;

use neo_frizbee::{Config, Matcher};
use std::time::{Duration, Instant};

use schlau_search::{
    Field, Join, LexicalHit, Meaning, SemanticHit, Vocabulary, expand, fuse, fuzzy_rank, parse,
    to_fts5_match,
};

const TERMS: usize = 500_000;
const LETTERS: &[u8] = b"eeeeeeeeeeeetttttttttaaaaaaaaoooooooiiiiiiinnnnnnnsssssshhhhhhrrrrrrddddllllcccuuummmwwffggyyppbbvkjxqz";

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn word(&mut self) -> String {
        let len = 2 + (0..4).map(|_| self.below(5)).sum::<usize>();
        (0..len)
            .map(|_| LETTERS[self.below(LETTERS.len())] as char)
            .collect()
    }

    fn typo(&mut self, word: &str) -> String {
        let mut cs: Vec<char> = word.chars().collect();
        let i = self.below(cs.len());
        match self.below(4) {
            0 if cs.len() > 1 => {
                cs.remove(i);
            }
            1 if i + 1 < cs.len() => cs.swap(i, i + 1),
            2 => cs[i] = LETTERS[self.below(LETTERS.len())] as char,
            _ => cs.insert(i, LETTERS[self.below(LETTERS.len())] as char),
        }
        cs.into_iter().collect()
    }
}

fn percentiles(mut samples: Vec<Duration>) -> String {
    samples.sort_unstable();
    let at = |p: f64| samples[((samples.len() - 1) as f64 * p) as usize];
    format!(
        "p50 {:>8.1?}  p99 {:>8.1?}  max {:>8.1?}",
        at(0.5),
        at(0.99),
        at(1.0)
    )
}

fn time<T>(inputs: &[T], mut f: impl FnMut(&T)) -> String {
    inputs.iter().take(50).for_each(&mut f);
    let samples = inputs
        .iter()
        .map(|input| {
            let start = Instant::now();
            f(input);
            start.elapsed()
        })
        .collect();
    percentiles(samples)
}

fn main() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut words: Vec<String> = (0..TERMS * 11 / 10).map(|_| rng.word()).collect();
    words.sort_unstable();
    words.dedup();
    words.truncate(TERMS);
    [
        "receive",
        "invoice",
        "schema",
        "meeting",
        "accommodation",
        "quarterly",
        "reimbursement",
    ]
    .iter()
    .for_each(|w| words.push((*w).to_owned()));
    let entries: Vec<(String, u32)> = words
        .iter()
        .enumerate()
        .map(|(rank, w)| (w.clone(), (1_000_000 / (rank % 50_000 + 1)) as u32))
        .collect();

    let start = Instant::now();
    let vocab = Vocabulary::new(entries.iter().map(|(t, d)| (t.as_str(), *d)));
    println!(
        "vocabulary: {} terms, built in {:.0?}, {:.1} MiB",
        vocab.len(),
        start.elapsed(),
        vocab.memory_bytes() as f64 / (1024.0 * 1024.0)
    );

    let typos: Vec<String> = (0..2000)
        .map(|_| {
            let i = rng.below(words.len());
            rng.typo(&words[i])
        })
        .collect();
    let typos4plus: Vec<String> = typos.iter().filter(|t| t.len() >= 4).cloned().collect();
    let long: Vec<String> = typos.iter().filter(|t| t.len() >= 8).cloned().collect();
    let fresh: Vec<String> = (0..2000).map(|_| rng.word()).collect();
    println!(
        "correct, typo of a vocabulary term ({}):  {}",
        typos4plus.len(),
        time(&typos4plus, |t| {
            black_box(vocab.correct(t, 8));
        })
    );
    println!(
        "correct, length >= 8, budget 2 ({}):      {}",
        long.len(),
        time(&long, |t| {
            black_box(vocab.correct(t, 8));
        })
    );
    println!(
        "correct, random word ({}):                {}",
        fresh.len(),
        time(&fresh, |t| {
            black_box(vocab.correct(t, 8));
        })
    );
    let mut by_len: Vec<Vec<&str>> = vec![Vec::new(); 64];
    words
        .iter()
        .for_each(|w| by_len[w.len().min(63)].push(w.as_str()));
    let frizbee_correct = |t: &String| {
        let k = schlau_search::typo_budget(t.len());
        let config = Config::default().max_typos(Some(k as u16));
        let mut matcher = Matcher::new(t.as_str(), &config);
        let hits: usize = (t.len().saturating_sub(k)..=(t.len() + k).min(63))
            .map(|len| matcher.match_list(&by_len[len]).len())
            .sum();
        black_box(hits);
    };
    println!(
        "baseline: neo_frizbee over length buckets ({}): {}",
        typos4plus.len(),
        time(&typos4plus, frizbee_correct)
    );
    let prefixes: Vec<String> = typos
        .iter()
        .map(|t| t.chars().take(1 + rng.below(t.chars().count())).collect())
        .collect();
    println!(
        "complete, prefix of a typo ({}):          {}",
        prefixes.len(),
        time(&prefixes, |p| {
            black_box(vocab.complete(p, 8));
        })
    );

    let queries: Vec<String> = (0..500)
        .map(|i| match i % 4 {
            0 => format!("from:jane {} is:unread", typos[i]),
            1 => format!("{} {} after:2026-01-01", typos[i], typos[i + 1]),
            2 => format!("\"{} {}\" -{}", words[i], words[i + 1], typos[i]),
            _ => format!("{} OR {} {}", typos[i], fresh[i], prefixes[i]),
        })
        .collect();
    let columns = |f: Field| -> &'static [&'static str] {
        match f {
            Field::Any => &[],
            Field::From => &["from_text"],
            Field::To | Field::Cc => &["to_text"],
            Field::Subject => &["subject"],
            Field::Body => &["body"],
            Field::Attachment => &["attachments"],
        }
    };
    println!(
        "parse + expand + to_fts5_match ({}):      {}",
        queries.len(),
        time(&queries, |q| {
            let query = parse(q, 1_767_225_600);
            black_box(to_fts5_match(&expand(&query, &vocab), columns, Join::All));
        })
    );

    let verbs = [
        "archive", "move", "label", "mark", "reply", "forward", "snooze", "open", "toggle",
        "search",
    ];
    let objects = [
        "thread", "message", "inbox", "sender", "folder", "label", "draft", "reminder", "all",
        "read",
    ];
    let commands: Vec<String> = (0..500)
        .map(|i| {
            format!(
                "{} {} {}",
                verbs[i % 10],
                objects[(i / 10) % 10],
                ["now", "later", "as unread", "to archive", "and next"][i % 5]
            )
        })
        .collect();
    let haystack: Vec<&str> = commands.iter().map(String::as_str).collect();
    let needles = [
        "arch",
        "mrk unread",
        "snoze",
        "move to",
        "r",
        "lbl thrd",
        "forward sender",
        "togle all",
        "xyz",
        "op inb",
    ];
    let needles: Vec<&str> = needles.iter().cycle().take(1000).copied().collect();
    println!(
        "fuzzy_rank, 500 commands ({}):           {}",
        needles.len(),
        time(&needles, |n| {
            black_box(fuzzy_rank(n, &haystack, 20));
        })
    );

    let semantic: Vec<SemanticHit> = (0..100_000)
        .map(|i| SemanticHit {
            doc: i as i64 / 2,
            cosine: (rng.below(1000) as f32 / 1000.0) * 0.6,
            received_at: 1_767_225_600 - rng.below(3 * 365 * 86_400) as i64,
        })
        .collect();
    let lexical: Vec<LexicalHit> = (0..1000)
        .map(|i| LexicalHit {
            doc: (i * 37) as i64,
            bm25: -(rng.below(1000) as f64) / 100.0,
            coverage: [1.0, 0.5, 0.67][i % 3],
            field: Field::Subject,
            received_at: 1_767_225_600 - rng.below(3 * 365 * 86_400) as i64,
        })
        .collect();
    let query = parse("trip to lisbon flights", 1_767_225_600);
    let runs: Vec<usize> = (0..200).collect();
    println!(
        "fuse, 1k lexical + 100k vectors ({}):      {}",
        runs.len(),
        time(&runs, |_| {
            black_box(fuse(
                &query,
                &lexical,
                Meaning::All(&semantic),
                1_767_225_600,
                50,
            ));
        })
    );
}
