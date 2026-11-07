use super::*;
use crate::parse::parse;

const NOW: i64 = 1_767_225_600;
const DAY: i64 = 86_400;

fn lex(doc: DocId, bm25: f64, coverage: f32) -> LexicalHit {
    LexicalHit {
        doc,
        bm25,
        coverage,
        field: Field::Body,
        received_at: NOW - 100 * DAY,
    }
}

fn sem(doc: DocId, cosine: f32) -> SemanticHit {
    SemanticHit {
        doc,
        cosine,
        received_at: NOW - 100 * DAY,
    }
}

/// 200 background vectors around 0.2 (deviation about 0.05) plus the given ones.
fn corpus(extra: &[SemanticHit]) -> Vec<SemanticHit> {
    (0..200)
        .map(|i| sem(1000 + i, 0.15 + (i % 11) as f32 * 0.01))
        .chain(extra.iter().copied())
        .collect()
}

fn ids(hits: &[Hit]) -> Vec<DocId> {
    hits.iter().map(|h| h.doc).collect()
}

#[test]
fn scale_from_cosines() {
    let s = MeaningScale::from_cosines([0.1, 0.3]);
    assert!((s.mean - 0.2).abs() < 1e-6);
    assert!((s.deviation - 0.1).abs() < 1e-6);
    assert!((s.z(0.4) - 2.0).abs() < 1e-5);
    assert_eq!(
        MeaningScale::from_cosines([0.5, 0.5]).deviation,
        MIN_DEVIATION
    );
    assert_eq!(MeaningScale::from_cosines(Vec::<f32>::new()).mean, 0.0);
}

#[test]
fn words_only_ranks_by_coverage_then_bm25() {
    let q = parse("quarterly report", NOW);
    let hits = fuse(
        &q,
        &[lex(1, -2.0, 0.5), lex(2, -1.0, 1.0), lex(3, -5.0, 1.0)],
        Meaning::None,
        NOW,
        10,
    );
    assert_eq!(ids(&hits), [3, 2, 1]);
    assert!(
        hits.iter()
            .all(|h| h.why == MatchReason::Words && h.z.is_none())
    );
}

#[test]
fn coverage_gate_drops_weak_word_hits() {
    let q = parse("a b c", NOW);
    let hits = fuse(
        &q,
        &[lex(1, -2.0, 0.34), lex(2, -1.0, 0.67)],
        Meaning::None,
        NOW,
        10,
    );
    assert_eq!(ids(&hits), [2]);
}

#[test]
fn meaning_needs_to_stand_out() {
    let q = parse("trip to the sea", NOW);
    let vectors = corpus(&[sem(1, 0.6), sem(2, 0.22)]);
    let hits = fuse(&q, &[], Meaning::All(&vectors), NOW, 10);
    assert_eq!(ids(&hits), [1]);
    assert_eq!(hits[0].why, MatchReason::Meaning);
    assert!(hits[0].z.unwrap() > 2.0);
}

#[test]
fn both_beats_either() {
    let q = parse("lisbon flights", NOW);
    let vectors = corpus(&[sem(1, 0.6), sem(2, 0.6)]);
    let hits = fuse(
        &q,
        &[lex(2, -3.0, 1.0), lex(3, -3.0, 1.0)],
        Meaning::All(&vectors),
        NOW,
        10,
    );
    assert_eq!(hits[0].doc, 2);
    assert_eq!(hits[0].why, MatchReason::Both);
    let reasons: Vec<_> = hits.iter().map(|h| (h.doc, h.why)).collect();
    assert!(reasons.contains(&(1, MatchReason::Meaning)));
    assert!(reasons.contains(&(3, MatchReason::Words)));
}

#[test]
fn strong_meaning_beats_one_common_word() {
    let q = parse("trip lisbon summer", NOW);
    let vectors = corpus(&[sem(1, 0.75)]);
    let hits = fuse(&q, &[lex(2, -3.0, 0.67)], Meaning::All(&vectors), NOW, 10);
    assert_eq!(ids(&hits), [1, 2]);
}

#[test]
fn lookup_prefers_words() {
    let q = parse("INV-20391", NOW);
    assert!(is_lookup(&q));
    let vectors = corpus(&[sem(1, 0.5)]);
    let hits = fuse(&q, &[lex(2, -3.0, 1.0)], Meaning::All(&vectors), NOW, 10);
    assert_eq!(hits[0].doc, 2);
    let q = parse("summer holidays", NOW);
    assert!(!is_lookup(&q));
    let hits = fuse(&q, &[lex(2, -3.0, 1.0)], Meaning::All(&vectors), NOW, 10);
    assert_eq!(hits[0].doc, 1);
}

#[test]
fn lookup_detection() {
    let yes = [
        "K7Q2LM",
        "TRX50",
        "4815162342",
        "jane@example.com",
        "\"Jane Doe\"",
        "12345 K9X",
        "from:x 4815",
    ];
    let no = [
        "invoice",
        "24",
        "trip 2024",
        "",
        "is:unread",
        "-K7Q2LM",
        "a@b",
    ];
    yes.iter()
        .for_each(|s| assert!(is_lookup(&parse(s, NOW)), "{s}"));
    no.iter()
        .for_each(|s| assert!(!is_lookup(&parse(s, NOW)), "{s}"));
}

#[test]
fn recency_breaks_ties() {
    let q = parse("invoice", NOW);
    let old = LexicalHit {
        received_at: NOW - 400 * DAY,
        ..lex(1, -2.0, 1.0)
    };
    let new = LexicalHit {
        received_at: NOW - DAY,
        ..lex(2, -2.0, 1.0)
    };
    assert_eq!(ids(&fuse(&q, &[old, new], Meaning::None, NOW, 10)), [2, 1]);
}

#[test]
fn recency_is_gentle() {
    let q = parse("invoice", NOW);
    let better_old = LexicalHit {
        received_at: NOW - 400 * DAY,
        ..lex(1, -6.0, 1.0)
    };
    let worse_new = LexicalHit {
        received_at: NOW,
        ..lex(2, -3.0, 1.0)
    };
    assert_eq!(
        ids(&fuse(&q, &[better_old, worse_new], Meaning::None, NOW, 10)),
        [1, 2]
    );
}

#[test]
fn recency_formula() {
    let o = FusionOptions::default();
    assert!((recency(NOW, NOW, &o) - 0.15).abs() < 1e-12);
    assert!((recency(NOW, NOW - 30 * DAY, &o) - 0.075).abs() < 1e-12);
    assert!((recency(NOW, NOW + DAY, &o) - 0.15).abs() < 1e-12);
}

#[test]
fn field_bonus_for_subject() {
    let q = parse("invoice", NOW);
    let body = lex(1, -2.0, 1.0);
    let subject = LexicalHit {
        field: Field::Subject,
        ..lex(2, -2.0, 1.0)
    };
    assert_eq!(
        ids(&fuse(&q, &[body, subject], Meaning::None, NOW, 10)),
        [2, 1]
    );
}

#[test]
fn best_chunk_counts() {
    let q = parse("trip", NOW);
    let vectors = corpus(&[sem(1, 0.1), sem(1, 0.7), sem(1, 0.2)]);
    let hits = fuse(&q, &[], Meaning::All(&vectors), NOW, 10);
    assert_eq!(ids(&hits), [1]);
    assert!(hits[0].z.unwrap() > 5.0);
}

#[test]
fn summary_matches_all() {
    let q = parse("trip", NOW);
    let vectors = corpus(&[sem(1, 0.6), sem(2, 0.5)]);
    let all = fuse(&q, &[lex(3, -1.0, 1.0)], Meaning::All(&vectors), NOW, 10);
    let scale = MeaningScale::from_cosines(vectors.iter().map(|v| v.cosine));
    let top = [sem(1, 0.6), sem(2, 0.5)];
    let summary = fuse(
        &q,
        &[lex(3, -1.0, 1.0)],
        Meaning::Summary { scale, top: &top },
        NOW,
        10,
    );
    assert_eq!(all, summary);
}

#[test]
fn duplicate_lexical_hits_keep_best() {
    let q = parse("a b", NOW);
    let hits = fuse(
        &q,
        &[lex(1, -1.0, 0.5), lex(1, -1.0, 1.0)],
        Meaning::None,
        NOW,
        10,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].coverage, 1.0);
}

#[test]
fn limit_and_empty() {
    let q = parse("x", NOW);
    assert!(fuse(&q, &[], Meaning::None, NOW, 10).is_empty());
    let many: Vec<LexicalHit> = (0..50).map(|i| lex(i, -1.0, 1.0)).collect();
    assert_eq!(fuse(&q, &many, Meaning::None, NOW, 7).len(), 7);
}

#[test]
fn deterministic_tie_order() {
    let q = parse("x", NOW);
    let hits = fuse(
        &q,
        &[lex(9, -1.0, 1.0), lex(3, -1.0, 1.0), lex(5, -1.0, 1.0)],
        Meaning::None,
        NOW,
        10,
    );
    assert_eq!(ids(&hits), [3, 5, 9]);
}

#[test]
fn strong_count_ratio() {
    let h = |score| Hit {
        doc: 0,
        score,
        why: MatchReason::Words,
        z: None,
        coverage: 1.0,
    };
    assert_eq!(strong_count(&[h(4.0), h(2.5), h(2.0), h(1.0)], 0.5), 3);
    assert_eq!(strong_count(&[], 0.5), 0);
    assert_eq!(strong_count(&[h(0.0), h(0.0)], 0.5), 2);
}
