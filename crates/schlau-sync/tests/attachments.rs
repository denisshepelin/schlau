//! Attachments of the fixture mailbox through the attachment worker: their text joins the word index
//! without any model, and with EmbeddingGemma 2 a query finds photos and scans by what they show.

use std::path::PathBuf;
use std::sync::Arc;

use schlau_search::{MeaningScale, Vocabulary, parse};
use schlau_store::{MeaningChannel, MeaningInput, SearchHit, Store};
use schlau_sync::{AttachmentWorker, FixtureBlobs, MediaBackend, SharedMedia};

const NOW: i64 = 1_800_000_000;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mailbox")
}

fn indexed(backend: Option<MediaBackend>) -> (tempfile::TempDir, Store, SharedMedia) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mail.sqlite");
    let mut store = Store::open(&path).unwrap();
    schlau_sync::load_fixtures(&mut store, &fixtures(), NOW).unwrap();
    let shared = SharedMedia::default();
    let worker = AttachmentWorker { path, blobs: Arc::new(FixtureBlobs(fixtures().join("blobs"))), backend, shared: shared.clone() };
    worker.run_until_idle().unwrap();
    (dir, store, shared)
}

fn search(store: &Store, q: &str, meaning: Option<MeaningInput<'_>>) -> Vec<SearchHit> {
    let reader = store.reader().unwrap();
    let vocab = Vocabulary::new(reader.vocabulary(1).unwrap().into_iter().map(|(t, d)| (t, d as u32)));
    reader.search(&parse(q, NOW), &vocab, meaning, NOW, 20).unwrap()
}

fn threads(hits: &[SearchHit]) -> Vec<&str> {
    hits.iter().map(|h| h.row.thread_id.as_str()).collect()
}

#[test]
fn words_inside_pdfs_find_their_conversation() {
    let (_dir, store, _) = indexed(None);
    let (done, total) = store.reader().unwrap().attachment_progress(None).unwrap();
    assert_eq!(done, total, "every attachment was looked at");
    assert_eq!(threads(&search(&store, "nachzahlung", None)), ["t-landlord"], "German PDF text");
    assert_eq!(threads(&search(&store, "2400183375", None)), ["t-flight"], "the ticket number only the e-ticket holds");
    assert!(threads(&search(&store, "lumen pro monthly plan", None)).len() >= 3, "invoice PDFs");
    assert!(search(&store, "\"Q1 Planning\"", None).is_empty(), "a scan without a text layer has no words");
}

#[cfg(feature = "gemma")]
#[test]
fn photos_and_scans_are_found_by_what_they_show() {
    let cache = PathBuf::from(std::env::var("HOME").unwrap()).join("Library/Application Support/schlau/models");
    let backend = MediaBackend::Local { cache };
    if !backend.is_ready() {
        eprintln!("EmbeddingGemma 2 is not downloaded, skipping");
        return;
    }
    found_by_what_they_show(backend);
}

/// The same through a running server: `SCHLAU_EMBED_URL=http://127.0.0.1:8080` with llama-server serving
/// EmbeddingGemma 2 (`--embeddings --pooling mean --mmproj … --image-max-tokens 140`).
#[test]
fn photos_and_scans_are_found_through_a_running_server() {
    let Ok(url) = std::env::var("SCHLAU_EMBED_URL") else {
        eprintln!("SCHLAU_EMBED_URL is not set, skipping");
        return;
    };
    found_by_what_they_show(MediaBackend::Remote { url, model: "embeddinggemma-2".into(), key: None });
}

fn found_by_what_they_show(backend: MediaBackend) {
    let (_dir, store, shared) = indexed(Some(backend));
    let model = shared.get().expect("the query model loads");
    let reader = store.reader().unwrap();
    let mut rows: Vec<(i64, Vec<u8>)> = Vec::new();
    reader.media_vectors(|id, blob| rows.push((id, blob.to_vec()))).unwrap();
    assert!(rows.len() >= 8, "{} media vectors", rows.len());
    let index = schlau_embed::VectorIndex::from_f16_blobs(
        model.dim(),
        schlau_embed::Precision::F16,
        rows.iter().map(|(id, b)| (*id, b.as_slice())),
    )
    .unwrap();
    let find = |q: &str| {
        let (hits, stats) = index.search_with_stats(&model.embed_query(q).unwrap(), 50).unwrap();
        let top: Vec<(i64, f32)> = hits.into_iter().map(|h| (h.id, h.score)).collect();
        let scale = MeaningScale { mean: stats.mean as f64, deviation: (stats.std as f64).max(0.02) };
        let meaning = MeaningInput { channels: vec![MeaningChannel { scale, top: &top, attachments: true }] };
        search(&store, q, Some(meaning)).into_iter().next().map(|h| (h.row.thread_id, h.attachment.unwrap_or_default()))
    };
    let (thread, file) = find("dog").unwrap();
    assert_eq!(thread, "t-family");
    assert!(file.starts_with("bruno-"), "{file}");
    assert_eq!(find("blueish document").unwrap().1, "planning-agenda.pdf");
    assert_eq!(find("graffiti mural").unwrap().1, "lx-factory-terrace.jpg");
    assert_eq!(find("Hund am Strand").unwrap().0, "t-family");
}
