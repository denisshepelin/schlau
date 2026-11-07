//! Multimodal embeddings: text queries and attachments (images, document text) in one vector space,
//! so "dog" finds a photo and "blueish document" a scanned page.

use std::sync::Arc;

use crate::{ModelFile, ModelSpec, Result};

/// One thing to embed as a document.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaInput {
    /// Text of an attachment (a PDF's text layer, a document's words), with its file name as the title.
    Text { title: Option<String>, text: String },
    /// An encoded image (JPEG, PNG or WebP), already downscaled.
    Image { bytes: Arc<[u8]>, mime: String },
}

/// A backend that embeds text queries and media into one space.
///
/// Vectors are L2-normalized, so a dot product is a cosine.
pub trait MediaEmbedder: Send + Sync {
    /// The model family and output size. Vectors with the same id are comparable, whichever backend
    /// made them, so switching between the built-in model and a server keeps the index.
    fn id(&self) -> &str;

    fn dim(&self) -> usize;

    /// Embeds a search query.
    fn embed_query(&self, query: &str) -> Result<Vec<f32>>;

    /// Embeds documents. Output order matches input order.
    fn embed(&self, inputs: &[MediaInput]) -> Result<Vec<Vec<f32>>>;
}

/// The id of EmbeddingGemma 2 vectors at full size.
pub const EMBEDDINGGEMMA_2_ID: &str = "embeddinggemma-2/768";

/// EmbeddingGemma 2 (Apache 2.0), the released bf16 safetensors at a pinned revision. 1.5 GB.
pub const EMBEDDINGGEMMA_2: ModelSpec = ModelSpec {
    repo: "google/embeddinggemma-2",
    revision: "f9972ac22e696a105c482202646979e33e4b5a1e",
    files: &[
        ModelFile {
            name: "config.json",
            sha256: Some("b8f1e9931b57fbc054acdb445c41765d55b0074c58d145fa82839941ad1b5bb3"),
            size: Some(4_455),
        },
        ModelFile {
            name: "config_sentence_transformers.json",
            sha256: Some("031e56a498d33c349ab489a21885bcfe25b4fcba841149dc99e1e90d4a7c28f5"),
            size: Some(1_565),
        },
        ModelFile {
            name: "processor_config.json",
            sha256: Some("168f6a08522f3ce5dea596d94d003af2fd691742d4f41fe1f9d8cce76bfbf69c"),
            size: Some(1_788),
        },
        ModelFile {
            name: "tokenizer.json",
            sha256: Some("4d777ef5bdc1aa36227abdfb77c3e49e7b9c892d16e1b6bda41c393504828be4"),
            size: Some(32_170_510),
        },
        ModelFile {
            name: "model.safetensors",
            sha256: Some("197a32965d4b1105faf060417baa899e193fb73cd401f42ec9295234d5553d79"),
            size: Some(1_488_915_288),
        },
    ],
};

/// Which parts of the model to load. Queries need only the text tower (270M parameters); indexing
/// images needs the vision tower too (another 170M).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Towers {
    Text,
    TextAndVision,
}

/// The task prompts EmbeddingGemma 2 was trained with. Images take no prompt.
pub mod prompt {
    pub fn query(query: &str) -> String {
        format!("task: search result | query: {query}")
    }

    pub fn document(title: Option<&str>, text: &str) -> String {
        let title = title.map(str::trim).filter(|t| !t.is_empty()).unwrap_or("none");
        format!("title: {title} | text: {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::prompt;

    #[test]
    fn prompts_match_the_model_card() {
        assert_eq!(prompt::query("dog"), "task: search result | query: dog");
        assert_eq!(prompt::document(None, "Total due"), "title: none | text: Total due");
        assert_eq!(prompt::document(Some(" invoice.pdf "), "x"), "title: invoice.pdf | text: x");
        assert_eq!(prompt::document(Some(""), "x"), "title: none | text: x");
    }
}
