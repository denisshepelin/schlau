//! The attachment worker: fetches attachment files, extracts their text (for the word index) and
//! images, and embeds both with the multimodal model, so a search for "dog" finds a photo and
//! "blueish document" a scanned page. It runs on its own thread with its own store connection, so
//! seconds of model work never hold up sync.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use schlau_embed::{EMBEDDINGGEMMA_2, MediaEmbedder, MediaInput, ModelStore, RemoteEmbedder, Towers, vector};
use schlau_extract::{Extracted, Limits};
use schlau_jmap::Id;
use schlau_store::{Attachment, AttachmentOutcome, PendingAttachment, Store};

use crate::engine::{Connect, Notify};
use crate::index::body_index_text;
use crate::remote::DynClient;

/// Where attachment files come from.
pub trait BlobSource: Send + Sync {
    fn fetch(&self, attachment: &Attachment) -> Result<Vec<u8>, BlobError>;
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BlobError {
    /// The file is gone or refused for good; the attachment is skipped.
    #[error("{0}")]
    Missing(String),
    /// It can't be fetched right now (offline, server trouble); the attachment waits.
    #[error("{0}")]
    Unavailable(String),
}

/// Files next to a fixture mailbox: `<dir>/<attachment name>`.
pub struct FixtureBlobs(pub PathBuf);

impl BlobSource for FixtureBlobs {
    fn fetch(&self, attachment: &Attachment) -> Result<Vec<u8>, BlobError> {
        let name = attachment
            .name
            .as_deref()
            .and_then(|n| Path::new(n).file_name())
            .ok_or_else(|| BlobError::Missing("attachment without a name".into()))?;
        std::fs::read(self.0.join(name)).map_err(|e| BlobError::Missing(e.to_string()))
    }
}

/// Blobs downloaded from the JMAP account, over a connection of the worker's own.
pub struct JmapBlobs {
    connect: Connect,
    client: Mutex<Option<(DynClient, Id)>>,
}

impl JmapBlobs {
    pub fn new(connect: Connect) -> JmapBlobs {
        JmapBlobs { connect, client: Mutex::new(None) }
    }
}

impl BlobSource for JmapBlobs {
    fn fetch(&self, a: &Attachment) -> Result<Vec<u8>, BlobError> {
        let unavailable = |e: &dyn std::fmt::Display| BlobError::Unavailable(e.to_string());
        let mut slot = self.client.lock().map_err(|e| unavailable(&e))?;
        if slot.is_none() {
            let client = (self.connect)().map_err(|e| unavailable(&e))?;
            let account = client.mail_account_id().map_err(|e| unavailable(&e))?;
            *slot = Some((client, account));
        }
        let (client, account) = slot.as_ref().ok_or_else(|| unavailable(&"not connected"))?;
        let name = a.name.as_deref().unwrap_or("attachment");
        client.download(account, &Id::from(a.blob_id.as_str()), &a.mime, name).map_err(|e| match e {
            schlau_jmap::Error::Http { status: 403 | 404 | 410, .. } => BlobError::Missing(e.to_string()),
            e => {
                *slot = None;
                unavailable(&e)
            }
        })
    }
}

/// How to reach the multimodal model.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaBackend {
    /// EmbeddingGemma 2 in process, its weights in `cache` (downloaded on request).
    Local { cache: PathBuf },
    /// A server that is already running, such as llama-server with EmbeddingGemma 2.
    Remote { url: String, model: String, key: Option<String> },
}

impl MediaBackend {
    /// The weights are in the cache already, or there is nothing to download.
    pub fn is_ready(&self) -> bool {
        match self {
            MediaBackend::Local { cache } => cfg!(feature = "gemma") && ModelStore::local(cache, &EMBEDDINGGEMMA_2).is_some(),
            MediaBackend::Remote { .. } => true,
        }
    }

    fn load(&self, towers: Towers) -> Result<Arc<dyn MediaEmbedder>, String> {
        match self {
            MediaBackend::Remote { url, model, key } => {
                RemoteEmbedder::connect(url, model, key.clone()).map(|e| Arc::new(e) as Arc<dyn MediaEmbedder>).map_err(|e| e.to_string())
            }
            MediaBackend::Local { cache } => load_local(cache, towers),
        }
    }
}

#[cfg(feature = "gemma")]
fn load_local(cache: &Path, towers: Towers) -> Result<Arc<dyn MediaEmbedder>, String> {
    let paths = ModelStore::local(cache, &EMBEDDINGGEMMA_2).ok_or("the image search model is not downloaded")?;
    schlau_embed::GemmaEmbedder::load(&paths.dir, towers).map(|e| Arc::new(e) as Arc<dyn MediaEmbedder>).map_err(|e| e.to_string())
}

#[cfg(not(feature = "gemma"))]
fn load_local(_cache: &Path, _towers: Towers) -> Result<Arc<dyn MediaEmbedder>, String> {
    Err("this build has no built-in image search model; point SCHLAU_EMBED_URL at a server".into())
}

/// Loaded query models, one per backend: the text tower when the model runs in process. Message text
/// and attachments may use different backends; both read from here, and the UI embeds queries with them.
/// A backend and its loaded query model.
type Loaded = (MediaBackend, Arc<dyn MediaEmbedder>);

#[derive(Clone, Default)]
pub struct SharedMedia {
    models: Arc<RwLock<Vec<Loaded>>>,
    loading: Arc<Mutex<()>>,
}

impl SharedMedia {
    /// The query model of `backend`, if loaded.
    pub fn get(&self, backend: &MediaBackend) -> Option<Arc<dyn MediaEmbedder>> {
        self.models.read().ok()?.iter().find(|(b, _)| b == backend).map(|(_, m)| m.clone())
    }

    /// The query model of `backend`, loading it once (the weights must be on disk).
    pub fn load(&self, backend: &MediaBackend) -> Result<Arc<dyn MediaEmbedder>, String> {
        let _loading = self.loading.lock().map_err(|e| e.to_string())?;
        if let Some(model) = self.get(backend) {
            return Ok(model);
        }
        let model = backend.load(Towers::Text)?;
        if let Ok(mut models) = self.models.write() {
            models.push((backend.clone(), model.clone()));
        }
        Ok(model)
    }

    /// Unloads the models of backends no longer in use.
    pub fn retain(&self, used: &[MediaBackend]) {
        if let Ok(mut models) = self.models.write() {
            models.retain(|(b, _)| used.contains(b));
        }
    }
}

static DOWNLOADING: Mutex<()> = Mutex::new(());

/// Makes `backend` ready: downloads EmbeddingGemma 2 once when it runs in process, reporting progress.
pub(crate) fn make_ready(backend: &MediaBackend, notify: &Notify) -> Result<(), String> {
    let MediaBackend::Local { cache } = backend else { return Ok(()) };
    if !cfg!(feature = "gemma") {
        return Err("this build has no built-in EmbeddingGemma 2; choose an embeddings server".into());
    }
    let _one_at_a_time = DOWNLOADING.lock().map_err(|e| e.to_string())?;
    if backend.is_ready() {
        return Ok(());
    }
    let total: u64 = EMBEDDINGGEMMA_2.files.iter().filter_map(|f| f.size).sum();
    let mut done_files = 0u64;
    let mut current = (String::new(), 0u64);
    let result = ModelStore::ensure(cache, &EMBEDDINGGEMMA_2, |p| {
        if p.file != current.0 {
            done_files += current.1;
            current = (p.file.to_string(), 0);
        }
        current.1 = p.downloaded;
        let fraction = (done_files + p.downloaded) as f64 / total.max(1) as f64;
        notify.set_background(Some(format!("Downloading EmbeddingGemma 2, {:.0}%", fraction * 100.0)));
    });
    notify.set_background(None);
    result.map(|_| ()).map_err(|e| format!("EmbeddingGemma 2 download failed: {e}"))
}

/// What the attachment worker needs: the database, where files come from, and the model.
pub struct AttachmentWorker {
    pub path: PathBuf,
    pub blobs: Arc<dyn BlobSource>,
    pub backend: Option<MediaBackend>,
    pub shared: SharedMedia,
}

pub(crate) enum Signal {
    /// New bodies arrived; there may be new attachments.
    Kick,
    /// Embed with this model from now on (downloading it if needed), or with none.
    Configure(Option<MediaBackend>),
}

const BATCH: usize = 4;
/// Drop the in-process model this long after the last attachment, to give back its memory.
const UNLOAD_AFTER: Duration = Duration::from_secs(60);
const IDLE_CHECK: Duration = Duration::from_secs(30);
/// Characters of an attachment's text the model reads: the opening is what a search is about.
const EMBED_TEXT_CHARS: usize = 4_000;

struct Indexer {
    worker: AttachmentWorker,
    notify: Notify,
    store: Store,
    /// The backend is ready and its query model loaded.
    enabled: bool,
    /// The model for documents, loaded while there is work.
    model: Option<Arc<dyn MediaEmbedder>>,
    last_work: Instant,
}

impl AttachmentWorker {
    /// Indexes every pending attachment on this thread and returns, for tools and tests. Loads the model
    /// when the backend is ready; never downloads it.
    pub fn run_until_idle(self) -> Result<(), String> {
        let store = Store::open(&self.path).map_err(|e| e.to_string())?;
        let mut indexer = Indexer { worker: self, notify: Notify::new(|| {}), store, enabled: false, model: None, last_work: Instant::now() };
        indexer.enabled = indexer.worker.backend.as_ref().is_some_and(MediaBackend::is_ready) && indexer.load_query_model();
        while indexer.step().map_err(|e| e.to_string())? {}
        Ok(())
    }

    pub(crate) fn spawn(self, notify: Notify, inbox: Receiver<Signal>) {
        let _ = std::thread::Builder::new().name("schlau-attachments".into()).spawn(move || {
            let Ok(store) = Store::open(&self.path) else { return };
            let backend = self.backend.clone();
            let mut indexer = Indexer { worker: self, notify, store, enabled: false, model: None, last_work: Instant::now() };
            indexer.configure(backend);
            indexer.run(&inbox);
        });
    }
}

fn truncate_chars(text: &str, max: usize) -> &str {
    text.char_indices().nth(max).map_or(text, |(i, _)| &text[..i])
}

fn inputs(name: Option<&str>, extracted: &Extracted) -> Vec<MediaInput> {
    let text = extracted
        .text
        .as_deref()
        .map(|t| MediaInput::Text { title: name.map(str::to_string), text: truncate_chars(t, EMBED_TEXT_CHARS).to_string() });
    let images = extracted
        .images
        .iter()
        .map(|i| MediaInput::Image { bytes: Arc::from(i.bytes.as_slice()), mime: i.mime.to_string() });
    text.into_iter().chain(images).collect()
}

impl Indexer {
    fn run(&mut self, inbox: &Receiver<Signal>) {
        loop {
            match self.step() {
                Ok(true) => match inbox.try_recv() {
                    Ok(signal) => self.handle(signal),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                },
                Ok(false) | Err(_) => {
                    self.notify.set_background(None);
                    if self.model.is_some() && self.last_work.elapsed() > UNLOAD_AFTER {
                        self.model = None;
                    }
                    match inbox.recv_timeout(IDLE_CHECK) {
                        Ok(signal) => self.handle(signal),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
        }
    }

    fn handle(&mut self, signal: Signal) {
        match signal {
            Signal::Kick => {}
            Signal::Configure(backend) => self.configure(backend),
        }
    }

    /// Switches to `backend`: downloads and loads it, or stops embedding. Text extraction goes on either way.
    fn configure(&mut self, backend: Option<MediaBackend>) {
        if backend != self.worker.backend {
            self.model = None;
        }
        self.worker.backend = backend;
        self.enabled = match &self.worker.backend {
            None => false,
            Some(b) => match make_ready(b, &self.notify) {
                Ok(()) => self.load_query_model(),
                Err(e) => {
                    self.notify.set_background(Some(e));
                    false
                }
            },
        };
    }

    fn load_query_model(&mut self) -> bool {
        let Some(backend) = &self.worker.backend else { return false };
        match self.worker.shared.load(backend) {
            Ok(_) => {
                self.notify.changed();
                true
            }
            Err(e) => {
                self.notify.set_background(Some(format!("Attachment search unavailable: {e}")));
                false
            }
        }
    }

    /// The document model, loading it if needed. A server serves queries and documents alike.
    fn document_model(&mut self) -> Option<Arc<dyn MediaEmbedder>> {
        if !self.enabled {
            return None;
        }
        if self.model.is_none() {
            let backend = self.worker.backend.as_ref()?;
            self.model = match backend {
                MediaBackend::Remote { .. } => self.worker.shared.get(backend),
                MediaBackend::Local { .. } => backend.load(Towers::TextAndVision).ok(),
            };
        }
        self.model.clone()
    }

    /// Works on one batch. Returns whether there was anything to do.
    fn step(&mut self) -> schlau_store::Result<bool> {
        let model_id = self
            .enabled
            .then(|| self.worker.backend.as_ref().and_then(|b| self.worker.shared.get(b)))
            .flatten()
            .map(|m| m.id().to_string());
        let pending = self.store.reader()?.pending_attachments(model_id.as_deref(), BATCH)?;
        if pending.is_empty() {
            return Ok(false);
        }
        let model = model_id.as_ref().and_then(|_| self.document_model());
        let count = pending.len();
        let outcomes: Vec<(PendingAttachment, AttachmentOutcome)> =
            pending.into_iter().filter_map(|p| self.index(&p, model.as_deref()).map(|o| (p, o))).collect();
        let reader = self.store.reader()?;
        let bodies: Vec<String> = outcomes
            .iter()
            .map(|(p, _)| reader.body(&p.message_id).map(|b| b.map(|b| body_index_text(&b)).unwrap_or_default()))
            .collect::<schlau_store::Result<_>>()?;
        drop(reader);
        self.store.write(|tx| {
            outcomes.iter().zip(&bodies).try_for_each(|((p, o), body)| tx.index_attachment(p.message, p.part, o, body))
        })?;
        self.last_work = Instant::now();
        let (done, total) = self.store.reader()?.attachment_progress(model_id.as_deref())?;
        self.notify.set_background(Some(format!("Indexing attachments, {done} of {total}")));
        self.notify.changed();
        Ok(outcomes.len() == count)
    }

    /// What one attachment gives, or None when its file can't be fetched right now (offline), to try
    /// again later.
    fn index(&self, p: &PendingAttachment, model: Option<&dyn MediaEmbedder>) -> Option<AttachmentOutcome> {
        let a = &p.attachment;
        let skipped = |why: String| AttachmentOutcome { skipped: Some(why), model: model.map(|m| m.id().to_string()), ..Default::default() };
        if schlau_extract::kind(a.name.as_deref(), &a.mime).is_none() {
            return Some(skipped(format!("unsupported type {}", a.mime)));
        }
        let bytes = match self.worker.blobs.fetch(a) {
            Ok(b) => b,
            Err(BlobError::Missing(why)) => return Some(skipped(why)),
            Err(BlobError::Unavailable(_)) => return None,
        };
        let extracted = match schlau_extract::extract(a.name.as_deref(), &a.mime, &bytes, &Limits::default()) {
            Ok(x) => x,
            Err(e) => return Some(skipped(e.to_string())),
        };
        let vectors = model
            .map(|m| {
                let inputs = inputs(a.name.as_deref(), &extracted);
                m.embed(&inputs).map(|vs| vs.iter().map(|v| vector::pack_f16(v)).collect()).unwrap_or_default()
            })
            .unwrap_or_default();
        Some(AttachmentOutcome {
            text: if p.extracted { None } else { extracted.text },
            vectors,
            model: model.map(|m| m.id().to_string()),
            skipped: None,
        })
    }
}
