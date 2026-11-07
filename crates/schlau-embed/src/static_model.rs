use std::fs::{self, File};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use model2vec_rs::model::StaticModel;

use crate::store::{DownloadProgress, ModelFile, ModelPaths, ModelSpec, ModelStore};
use crate::{Embedder, Error, Result};

/// `minishlab/potion-multilingual-128M`: Model2Vec distilled from BAAI/bge-m3,
/// 256 dimensions, ~500k token vocabulary (XLM-R sentencepiece) covering 101
/// languages. 512 MB of f32 weights on disk, 128 MB in memory after int8
/// quantization at load time.
pub const POTION_MULTILINGUAL_128M: ModelSpec = ModelSpec {
    repo: "minishlab/potion-multilingual-128M",
    revision: "73908c3438cf03b6a01bcb9611d62b23d0726f08",
    files: &[
        ModelFile {
            name: "config.json",
            sha256: Some("595e4cab2093732efd5dbe084fd5c1826b5eea693b73b4c1fd971672867d2e54"),
            size: Some(271),
        },
        ModelFile {
            name: "tokenizer.json",
            sha256: Some("19f1909063da3cfe3bd83a782381f040dccea475f4816de11116444a73e1b6a1"),
            size: Some(18_616_131),
        },
        ModelFile {
            name: "model.safetensors",
            sha256: Some("14b5eb39cb4ce5666da8ad1f3dc6be4346e9b2d601c073302fa0a31bf7943397"),
            size: Some(512_361_560),
        },
    ],
};

/// Maximum number of tokens pooled per text. Longer inputs are truncated; use
/// [`crate::email_passages`] to split long bodies before embedding.
pub const MAX_TOKENS: usize = 512;

/// Model2Vec static embeddings: tokenize, look up one vector per token, mean-pool,
/// L2-normalize. Microseconds per text on a single core.
///
/// The embedding table is quantized to int8 with one scale per row when loaded,
/// which cuts memory by 4x at a negligible quality cost.
///
/// `embed_documents` runs on the current rayon pool; run it inside
/// `ThreadPool::install` to bound background CPU use.
pub struct StaticEmbedder {
    model: StaticModel,
    id: String,
}

impl StaticEmbedder {
    /// Download the model into `cache_dir` if needed, then load it.
    pub fn from_store(
        cache_dir: &Path,
        spec: &ModelSpec,
        progress: impl FnMut(DownloadProgress<'_>),
    ) -> Result<Self> {
        let paths = ModelStore::ensure(cache_dir, spec, progress)?;
        Self::load(&paths, &format!("{}@{}", spec.repo, spec.revision))
    }

    /// Load a Model2Vec model from a directory containing `config.json`,
    /// `tokenizer.json` and `model.safetensors`. `name` identifies the model
    /// (repository and revision) in [`Embedder::id`].
    pub fn load(paths: &ModelPaths, name: &str) -> Result<Self> {
        let read = |file: &str| {
            let path = paths.file(file);
            fs::read(&path).map_err(Error::io(path))
        };
        let tokenizer = read("tokenizer.json")?;
        let config = read("config.json")?;
        let weights_path = paths.file("model.safetensors");
        let weights_file = File::open(&weights_path).map_err(Error::io(&weights_path))?;
        // SAFETY: the file lives in the model cache, is never modified after its
        // atomic rename into place, and the mapping is dropped before returning.
        let weights =
            unsafe { memmap2::Mmap::map(&weights_file) }.map_err(Error::io(&weights_path))?;
        let model = StaticModel::from_bytes(tokenizer, &weights[..], config, Some(true))
            .map_err(|e| Error::Model(format!("{e:#}")))?
            .quantize_int8();
        Ok(Self {
            id: format!("model2vec:{name}:int8:max{MAX_TOKENS}"),
            model,
        })
    }

    fn encode(&self, texts: &[&str]) -> Result<Vec<f32>> {
        let owned: Vec<String> = texts.iter().map(|text| (*text).to_owned()).collect();
        catch_unwind(AssertUnwindSafe(|| {
            self.model.encode_flat_with_args(&owned, Some(MAX_TOKENS))
        }))
        .map_err(|panic| Error::Embedding(panic_message(panic.as_ref())))
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "tokenizer panicked".to_owned())
}

impl Embedder for StaticEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dim(&self) -> usize {
        self.model.dim()
    }

    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let dim = self.dim();
        Ok(self
            .encode(texts)?
            .chunks_exact(dim)
            .map(<[f32]>::to_vec)
            .collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.encode(&[text])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::dot;

    fn cache_dir() -> std::path::PathBuf {
        std::env::var_os("SCHLAU_MODEL_CACHE")
            .map(Into::into)
            .unwrap_or_else(|| std::env::temp_dir().join("schlau-embed-models"))
    }

    #[test]
    #[ignore = "downloads a 530 MB model from Hugging Face"]
    fn real_model_ranks_related_mail_higher() {
        let embedder =
            StaticEmbedder::from_store(&cache_dir(), &POTION_MULTILINGUAL_128M, |_| {}).unwrap();
        assert_eq!(embedder.dim(), 256);
        assert!(embedder.id().contains(POTION_MULTILINGUAL_128M.revision));

        let query = embedder.embed_query("invoice for march").unwrap();
        let docs = embedder
            .embed_documents(&["Your bill for March is attached", "Team offsite photos"])
            .unwrap();
        let (bill, photos) = (dot(&query, &docs[0]), dot(&query, &docs[1]));
        eprintln!("cos(query, bill) = {bill:.4}, cos(query, photos) = {photos:.4}");
        assert!(bill > photos);

        let german = embedder
            .embed_documents(&["Ihre Rechnung für März"])
            .unwrap();
        let cross = dot(&query, &german[0]);
        eprintln!("cos(query, german bill) = {cross:.4}");
        assert!(cross > photos);

        let norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3);
    }
}
