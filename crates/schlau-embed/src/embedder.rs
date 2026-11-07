use crate::Result;

/// A text embedding backend.
///
/// Vectors are L2-normalized, so the dot product of two vectors is their cosine
/// similarity.
pub trait Embedder: Send + Sync {
    /// Stable identifier of the model and every setting that changes its output
    /// (name, revision, quantization). The store keeps it next to the vectors and
    /// re-embeds when it changes.
    fn id(&self) -> &str;

    /// Number of dimensions of every vector this embedder returns.
    fn dim(&self) -> usize;

    /// Embed passages for indexing. Output order matches input order.
    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Embed a search query.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}
