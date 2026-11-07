//! Embeddings from a server that is already running: llama-server (`llama-server -m
//! embeddinggemma-2-Q8_0.gguf --mmproj mmproj-embeddinggemma-2-Q8_0.gguf --embeddings`), or anything else
//! that speaks the OpenAI `/v1/embeddings` shape. Images go as `{"content": [{"type": "image_url", …}]}`
//! items, the form llama-server and OpenRouter accept for vision embedding models.

use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::media::{EMBEDDINGGEMMA_2_ID, MediaEmbedder, MediaInput, prompt};
use crate::vector::l2_normalized;
use crate::{Error, Result};

pub struct RemoteEmbedder {
    url: String,
    model: String,
    key: Option<String>,
    id: String,
    dim: usize,
    /// Whether to write EmbeddingGemma's task prompts; servers pass text through as is.
    prompts: bool,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Datum>,
}

#[derive(Deserialize)]
struct Datum {
    embedding: Vec<f32>,
    #[serde(default)]
    index: Option<usize>,
}

/// `http://host:8080`, `http://host:8080/v1` and the full endpoint all mean the same server.
fn endpoint(base: &str) -> String {
    let base = base.trim_end_matches('/');
    match base {
        b if b.ends_with("/embeddings") => b.to_string(),
        b if b.ends_with("/v1") => format!("{b}/embeddings"),
        b => format!("{b}/v1/embeddings"),
    }
}

fn is_gemma(model: &str) -> bool {
    let model = model.to_ascii_lowercase().replace(['_', ' '], "-");
    model.contains("embeddinggemma-2") || model.contains("embeddinggemma2")
}

fn item(input: &MediaInput, prompts: bool) -> Value {
    match input {
        MediaInput::Text { title, text } if prompts => Value::String(prompt::document(title.as_deref(), text)),
        MediaInput::Text { text, .. } => Value::String(text.clone()),
        MediaInput::Image { bytes, mime } => {
            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
            json!({ "content": [{ "type": "image_url", "image_url": { "url": format!("data:{mime};base64,{data}") } }] })
        }
    }
}

impl RemoteEmbedder {
    /// Connects to `base` and asks for one embedding, to learn the model's size.
    pub fn connect(base: &str, model: &str, key: Option<String>) -> Result<RemoteEmbedder> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("schlau/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let prompts = is_gemma(model);
        let mut embedder = RemoteEmbedder {
            url: endpoint(base),
            model: model.to_string(),
            key,
            id: String::new(),
            dim: 0,
            prompts,
            agent,
        };
        let dim = embedder.embed_query("probe")?.len();
        embedder.dim = dim;
        embedder.id = if prompts && dim == 768 { EMBEDDINGGEMMA_2_ID.to_string() } else { format!("remote:{model}/{dim}") };
        Ok(embedder)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn post(&self, input: Vec<Value>) -> Result<Vec<Vec<f32>>> {
        let count = input.len();
        let body = json!({ "model": self.model, "input": input, "encoding_format": "float" });
        let request = self.agent.post(&self.url);
        let request = match &self.key {
            Some(key) => request.header("Authorization", &format!("Bearer {key}")),
            None => request,
        };
        let failed = |e: &dyn std::fmt::Display| Error::Embedding(format!("{}: {e}", self.url));
        let response: Response =
            request.send_json(&body).map_err(|e| failed(&e))?.body_mut().read_json().map_err(|e| failed(&e))?;
        let mut data = response.data;
        data.sort_by_key(|d| d.index.unwrap_or(0));
        if data.len() != count {
            return Err(failed(&format!("expected {count} embeddings, got {}", data.len())));
        }
        if self.dim > 0
            && let Some(d) = data.iter().find(|d| d.embedding.len() != self.dim)
        {
            return Err(Error::Dimension { expected: self.dim, actual: d.embedding.len() });
        }
        Ok(data.iter().map(|d| l2_normalized(&d.embedding)).collect())
    }
}

impl MediaEmbedder for RemoteEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let text = if self.prompts { prompt::query(query) } else { query.to_string() };
        self.post(vec![Value::String(text)])?.pop().ok_or_else(|| Error::Embedding("no output".into()))
    }

    fn embed(&self, inputs: &[MediaInput]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        self.post(inputs.iter().map(|i| item(i, self.prompts)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_spelling_of_the_server_address_reaches_the_endpoint() {
        assert_eq!(endpoint("http://localhost:8080"), "http://localhost:8080/v1/embeddings");
        assert_eq!(endpoint("http://localhost:8080/"), "http://localhost:8080/v1/embeddings");
        assert_eq!(endpoint("http://localhost:8080/v1"), "http://localhost:8080/v1/embeddings");
        assert_eq!(endpoint("https://x.example/v1/embeddings"), "https://x.example/v1/embeddings");
    }

    #[test]
    fn gemma_models_get_prompts_and_images_go_as_content_parts() {
        assert!(is_gemma("embeddinggemma-2-Q8_0.gguf"));
        assert!(is_gemma("ggml-org/EmbeddingGemma-2-GGUF"));
        assert!(!is_gemma("nomic-embed-text"));
        let image = MediaInput::Image { bytes: vec![1u8, 2, 3].into(), mime: "image/jpeg".into() };
        assert_eq!(item(&image, true)["content"][0]["image_url"]["url"], "data:image/jpeg;base64,AQID");
        let text = MediaInput::Text { title: Some("a.pdf".into()), text: "hello".into() };
        assert_eq!(item(&text, true), "title: a.pdf | text: hello");
        assert_eq!(item(&text, false), "hello");
    }
}
