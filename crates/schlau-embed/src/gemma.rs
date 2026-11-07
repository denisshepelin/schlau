//! EmbeddingGemma 2 in process, on candle: Metal on Apple Silicon (bf16), the CPU elsewhere (f32).

use std::path::Path;

use candle_core::{DType, Device};
use schlau_gemma::{EmbeddingGemma2, Input, Modalities, Options};

use crate::media::{EMBEDDINGGEMMA_2_ID, MediaEmbedder, MediaInput, Towers, prompt};
use crate::{Error, Result};

const DIM: usize = 768;

/// Soft tokens per image: half the checkpoint's default, which halves the vision tower's time and still
/// tells a dog from a document.
const IMAGE_TOKENS: usize = 140;

pub struct GemmaEmbedder {
    model: EmbeddingGemma2,
    device: String,
}

fn model_error(e: impl std::fmt::Display) -> Error {
    Error::Model(e.to_string())
}

fn embedding_error(e: impl std::fmt::Display) -> Error {
    Error::Embedding(e.to_string())
}

fn best_device() -> Device {
    if candle_core::utils::metal_is_available() {
        Device::new_metal(0).unwrap_or(Device::Cpu)
    } else {
        Device::Cpu
    }
}

impl GemmaEmbedder {
    /// Loads the model from a directory laid out like the Hugging Face repository.
    pub fn load(dir: &Path, towers: Towers) -> Result<GemmaEmbedder> {
        let device = best_device();
        let dtype = if device.is_cpu() { DType::F32 } else { DType::BF16 };
        let modalities = match towers {
            Towers::Text => Modalities::TEXT_ONLY,
            Towers::TextAndVision => Modalities { vision: true, ..Modalities::TEXT_ONLY },
        };
        let options = Options { device: device.clone(), dtype, modalities, batch_size: 8, max_seq_len: 2048, image_soft_tokens: Some(IMAGE_TOKENS) };
        let model = EmbeddingGemma2::from_dir(dir, options).map_err(model_error)?;
        let device = if device.is_cpu() { "cpu" } else { "metal" }.to_string();
        Ok(GemmaEmbedder { model, device })
    }

    /// Where the model runs: `metal` or `cpu`.
    pub fn device(&self) -> &str {
        &self.device
    }

    fn encode(&self, inputs: &[Input]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        self.model
            .encode(inputs, None)
            .and_then(|t| Ok(t.to_dtype(DType::F32)?.to_vec2::<f32>()?))
            .map_err(embedding_error)
    }
}

fn input(item: &MediaInput) -> Result<Input> {
    match item {
        MediaInput::Text { title, text } => Ok(Input::text(prompt::document(title.as_deref(), text))),
        MediaInput::Image { bytes, .. } => image::load_from_memory(bytes).map(Input::image).map_err(embedding_error),
    }
}

impl MediaEmbedder for GemmaEmbedder {
    fn id(&self) -> &str {
        EMBEDDINGGEMMA_2_ID
    }

    fn dim(&self) -> usize {
        DIM
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        self.encode(&[Input::text(prompt::query(query))])?.pop().ok_or_else(|| embedding_error("no output"))
    }

    /// One input per forward pass: images of different shapes in one batch would need a padding mask,
    /// which rules out the fused attention kernel and costs more than batching saves.
    fn embed(&self, inputs: &[MediaInput]) -> Result<Vec<Vec<f32>>> {
        inputs
            .iter()
            .map(|item| self.encode(&[input(item)?])?.pop().ok_or_else(|| embedding_error("no output")))
            .collect()
    }
}
