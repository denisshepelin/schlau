//! Local semantic embeddings for schlau.
//!
//! - [`Embedder`]: the backend-neutral interface the store and sync crates use.
//! - [`StaticEmbedder`]: the default backend, a Model2Vec static embedding model
//!   (token lookup + mean pooling, no neural network at inference time).
//! - [`ModelStore`]: one-time, verified download of model files from Hugging Face.
//! - [`vector`]: f16 blob packing, normalization and a brute-force [`VectorIndex`]
//!   with the score statistics hybrid ranking needs.
//! - [`text`]: turning an email into the passages that get embedded.
//! - [`MediaEmbedder`]: text and images in one space, for attachment search. Backends: EmbeddingGemma 2
//!   in process (feature `gemma`, [`GemmaEmbedder`]) or any OpenAI-compatible embeddings server such as
//!   llama-server ([`RemoteEmbedder`]).

mod embedder;
mod error;
#[cfg(feature = "gemma")]
mod gemma;
mod media;
mod remote;
mod static_model;
mod store;
pub mod text;
pub mod vector;

pub use embedder::Embedder;
pub use error::{Error, Result};
#[cfg(feature = "gemma")]
pub use gemma::GemmaEmbedder;
pub use media::{EMBEDDINGGEMMA_2, EMBEDDINGGEMMA_2_ID, MediaEmbedder, MediaInput, Towers, prompt};
pub use remote::RemoteEmbedder;
pub use static_model::{POTION_MULTILINGUAL_128M, StaticEmbedder};
pub use store::{DownloadProgress, ModelFile, ModelPaths, ModelSpec, ModelStore};
pub use text::{ChunkPolicy, email_document, email_passages};
pub use vector::{DocId, Hit, Precision, ScoreStats, VectorIndex};
