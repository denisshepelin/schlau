//! Syncs a Fastmail account into a database without the UI and reports how long each step took.
//!
//! Run with: FASTMAIL_API_TOKEN=... cargo run --release -p schlau-sync --example sync -- path/to/mail.sqlite [pages] [body batches]
//! Then indexes the attachments of the fetched bodies (their text; vectors too with SCHLAU_EMBED_URL).

use std::sync::Arc;
use std::time::Instant;

use schlau_jmap::{Client, FASTMAIL_SESSION_URL, Transport, UreqTransport};
use schlau_store::Store;
use schlau_sync::{AttachmentWorker, JmapBlobs, MediaBackend, Remote, SharedMedia};

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    println!("{label:<24} {:>8.0} ms", start.elapsed().as_secs_f64() * 1000.0);
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).ok_or("usage: sync <db> [backfill pages]")?;
    let pages: usize = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(2);
    let body_batches: usize = args.get(3).and_then(|p| p.parse().ok()).unwrap_or(1);
    let token = std::env::var("FASTMAIL_API_TOKEN").or_else(|_| std::env::var("FASTMAIL_TOKEN"))?;
    let session = std::env::var("JMAP_SESSION_URL").unwrap_or_else(|_| FASTMAIL_SESSION_URL.to_string());

    let mut store = Store::open(path)?;
    let connect = {
        let (token, session) = (token.clone(), session.clone());
        Arc::new(move || {
            let transport: Arc<dyn Transport> = Arc::new(UreqTransport::bearer(&token));
            Client::connect(transport, &session)
        }) as schlau_sync::Connect
    };
    let remote = timed("session", || connect().map_err(schlau_sync::SyncError::from).and_then(Remote::new))?;
    timed("claim", || remote.claim(&mut store))?;
    timed("mailboxes", || remote.sync_mailboxes(&mut store))?;
    timed("first page", || remote.start_emails(&mut store))?;
    (0..pages).try_for_each(|i| timed(&format!("backfill {}", i + 1), || remote.backfill(&mut store)).map(|_| ()))?;
    timed("changes", || remote.sync_emails(&mut store))?;
    let bodies = timed(&format!("bodies ({body_batches} batches)"), || {
        (0..body_batches).try_fold(Vec::new(), |mut all, _| {
            all.extend(remote.fetch_bodies(&mut store, &[])?);
            Ok::<_, schlau_sync::SyncError>(all)
        })
    })?;
    let backend = std::env::var("SCHLAU_EMBED_URL")
        .ok()
        .map(|url| MediaBackend::Remote { url, model: "embeddinggemma-2".into(), key: None });
    let worker = AttachmentWorker { path: path.into(), blobs: Arc::new(JmapBlobs::new(connect)), backend, shared: SharedMedia::default() };
    timed("attachments", || worker.run_until_idle())?;
    let (done, total) = store.reader()?.attachment_progress(None)?;
    println!("attachments indexed: {done} of {total}");

    let reader = store.reader()?;
    println!("messages: {}  bodies fetched: {}", reader.message_count()?, bodies.len());
    reader.mailboxes()?.iter().for_each(|m| println!("  {:<28} {:>6} {:>6}", m.name, m.unread_emails, m.total_emails));
    Ok(())
}
