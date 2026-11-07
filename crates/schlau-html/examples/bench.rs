use std::hint::black_box;
use std::time::{Duration, Instant};

use schlau_html::{Options, RemotePolicy, sanitize, snippet, to_blocks, to_plain_text};

const FIXTURES: &[&str] = &[
    "marketing_100k.html",
    "mailchimp_newsletter.html",
    "outlook_safelinks.html",
    "gmail_reply.html",
    "receipt_cid.html",
    "hostile.html",
];

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn measure(iterations: usize, mut run: impl FnMut()) -> (Duration, Duration, Duration) {
    (0..iterations.min(20)).for_each(|_| run());
    let mut samples: Vec<Duration> = (0..iterations)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed()
        })
        .collect();
    samples.sort();
    let mean = samples.iter().sum::<Duration>() / samples.len() as u32;
    (mean, percentile(&samples, 0.5), percentile(&samples, 0.99))
}

fn report(
    fixture: &str,
    bytes: usize,
    label: &str,
    (mean, p50, p99): (Duration, Duration, Duration),
) {
    println!(
        "{fixture:<28} {:>7.1} KB  {label:<22} mean {:>8.3} ms  p50 {:>8.3} ms  p99 {:>8.3} ms",
        bytes as f64 / 1024.0,
        mean.as_secs_f64() * 1e3,
        p50.as_secs_f64() * 1e3,
        p99.as_secs_f64() * 1e3,
    );
}

fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(200);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let block = Options::default();
    let allow = Options::with_remote(RemotePolicy::Allow);
    for name in FIXTURES {
        let html = std::fs::read_to_string(format!("{dir}/{name}")).expect("fixture readable");
        let bytes = html.len();
        report(
            name,
            bytes,
            "sanitize (block)",
            measure(iterations, || {
                black_box(sanitize(black_box(&html), &block));
            }),
        );
        report(
            name,
            bytes,
            "sanitize (allow)",
            measure(iterations, || {
                black_box(sanitize(black_box(&html), &allow));
            }),
        );
        report(
            name,
            bytes,
            "to_plain_text",
            measure(iterations, || {
                black_box(to_plain_text(black_box(&html), true));
            }),
        );
        report(
            name,
            bytes,
            "to_blocks",
            measure(iterations, || {
                black_box(to_blocks(black_box(&html)));
            }),
        );
        let text = to_plain_text(&html, false);
        report(
            name,
            bytes,
            "snippet",
            measure(iterations, || {
                black_box(snippet(black_box(&text), 140));
            }),
        );
    }
}
