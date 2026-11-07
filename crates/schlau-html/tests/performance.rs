mod common;

use std::time::{Duration, Instant};

use common::fixture;
use schlau_html::{Options, sanitize};

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "timing is only meaningful in release builds"
)]
fn sanitizes_a_100k_marketing_email_under_budget() {
    let html = fixture("marketing_100k.html");
    assert!(html.len() >= 100_000);
    let options = Options::default();
    (0..5).for_each(|_| drop(sanitize(&html, &options)));
    let mut samples: Vec<Duration> = (0..30)
        .map(|_| {
            let start = Instant::now();
            drop(sanitize(&html, &options));
            start.elapsed()
        })
        .collect();
    samples.sort();
    let median = samples[samples.len() / 2];
    assert!(median < Duration::from_millis(10), "median {median:?}");
}
