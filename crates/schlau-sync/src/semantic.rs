//! Embeddings for meaning search: the model is shared between the engine (which embeds messages
//! as their bodies arrive) and the UI's search workers (which embed queries).

use std::path::Path;
use std::sync::{Arc, RwLock};

use schlau_embed::{ChunkPolicy, Embedder, ModelStore, POTION_MULTILINGUAL_128M, StaticEmbedder, email_passages, vector};
use schlau_store::Store;

use crate::index::body_index_text;

const MODEL_META: &str = "embed.model";
const BATCH: usize = 256;

/// The loaded model, once there is one.
#[derive(Clone, Default)]
pub struct SharedEmbedder(Arc<RwLock<Option<Arc<dyn Embedder>>>>);

impl SharedEmbedder {
    pub fn get(&self) -> Option<Arc<dyn Embedder>> {
        self.0.read().ok().and_then(|e| e.clone())
    }

    fn set(&self, embedder: Arc<dyn Embedder>) {
        if let Ok(mut slot) = self.0.write() {
            *slot = Some(embedder);
        }
    }
}

/// Loads the model if it was downloaded before. Never touches the network.
pub fn load_local(cache: &Path, shared: &SharedEmbedder) -> bool {
    let Some(paths) = ModelStore::local(cache, &POTION_MULTILINGUAL_128M) else { return false };
    match StaticEmbedder::load(&paths, POTION_MULTILINGUAL_128M.repo) {
        Ok(model) => {
            shared.set(Arc::new(model));
            true
        }
        Err(_) => false,
    }
}

/// Downloads (once) and loads the model, reporting progress as a fraction.
pub fn download(cache: &Path, shared: &SharedEmbedder, mut progress: impl FnMut(f64)) -> Result<(), String> {
    let total: u64 = POTION_MULTILINGUAL_128M.files.iter().filter_map(|f| f.size).sum();
    let mut done_files: u64 = 0;
    let mut current = (String::new(), 0u64);
    let model = StaticEmbedder::from_store(cache, &POTION_MULTILINGUAL_128M, |p| {
        if p.file != current.0 {
            done_files += current.1;
            current = (p.file.to_string(), 0);
        }
        current.1 = p.downloaded;
        progress((done_files + p.downloaded) as f64 / total.max(1) as f64);
    })
    .map_err(|e| e.to_string())?;
    shared.set(Arc::new(model));
    Ok(())
}

/// Embeds up to one batch of messages that have bodies but no vectors. Returns how many it embedded.
/// A different model than the stored vectors were made with clears them first.
pub fn embed_batch(store: &mut Store, embedder: &dyn Embedder) -> schlau_store::Result<usize> {
    if store.reader()?.meta(MODEL_META)?.as_deref() != Some(embedder.id()) {
        store.write(|tx| {
            tx.clear_vectors()?;
            tx.set_meta(MODEL_META, embedder.id())
        })?;
    }
    let inputs = store.reader()?.messages_without_vectors(BATCH)?;
    if inputs.is_empty() {
        return Ok(0);
    }
    let policy = ChunkPolicy::default();
    let passages: Vec<(String, Vec<String>)> = inputs
        .iter()
        .map(|m| (m.id.clone(), email_passages(&m.subject, &m.from, &body_index_text(&m.body), &policy)))
        .collect();
    let texts: Vec<&str> = passages.iter().flat_map(|(_, p)| p.iter().map(String::as_str)).collect();
    let Ok(vectors) = embedder.embed_documents(&texts) else { return Ok(0) };
    let mut vectors = vectors.into_iter();
    store.write(|tx| {
        passages.iter().try_for_each(|(id, p)| {
            let blobs: Vec<Vec<u8>> = vectors.by_ref().take(p.len().max(1)).map(|v| vector::pack_f16(&v)).collect();
            tx.put_vectors(id, &blobs).map(|_| ())
        })
    })?;
    Ok(inputs.len())
}
