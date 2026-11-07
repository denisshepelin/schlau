//! What search indexes with, one enum per choice. The engine and the attachment worker follow these;
//! the app keeps them in its settings file and sends changes with `Command::Configure`.

use std::path::{Path, PathBuf};

use schlau_embed::{EMBEDDINGGEMMA_2, ModelStore, POTION_MULTILINGUAL_128M};
use schlau_search::{ExpandOptions, LastWord, Typos};
use serde::{Deserialize, Serialize};

use crate::attachments::MediaBackend;

/// Meaning search over message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextMeaning {
    /// Words only.
    #[default]
    Off,
    /// Model2Vec (potion-multilingual-128M): static embeddings, about 35,000 texts a second, 530 MB.
    #[serde(rename = "model2vec")]
    Model2Vec,
    /// EmbeddingGemma 2 in the app: a real transformer, much better at meaning, about 20 texts a second.
    EmbeddingGemma,
    /// The embeddings server in `server`. Message text leaves the app for it.
    Server,
}

impl TextMeaning {
    pub const ALL: [TextMeaning; 4] = [TextMeaning::Off, TextMeaning::Model2Vec, TextMeaning::EmbeddingGemma, TextMeaning::Server];

    pub fn label(self) -> &'static str {
        match self {
            TextMeaning::Off => "Off",
            TextMeaning::Model2Vec => "Model2Vec",
            TextMeaning::EmbeddingGemma => "EmbeddingGemma 2",
            TextMeaning::Server => "Server",
        }
    }

    /// What the choice does and costs, for a settings page.
    pub fn detail(self) -> &'static str {
        match self {
            TextMeaning::Off => "Words only.",
            TextMeaning::Model2Vec => "Static embeddings in 101 languages: indexes about 35,000 messages a second. 530 MB.",
            TextMeaning::EmbeddingGemma => {
                "A transformer, much better at meaning: indexes about 20 messages a second. 1.5 GB, shared with attachments."
            }
            TextMeaning::Server => "The embeddings server below. Message text leaves the app for it.",
        }
    }
}

/// Meaning search over attachments: photos, page images and document text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttachmentMeaning {
    /// Attachment text still joins the word index.
    #[default]
    Off,
    /// EmbeddingGemma 2 in the app.
    EmbeddingGemma,
    /// The embeddings server in `server`. Attachments leave the app for it.
    Server,
}

impl AttachmentMeaning {
    pub const ALL: [AttachmentMeaning; 3] = [AttachmentMeaning::Off, AttachmentMeaning::EmbeddingGemma, AttachmentMeaning::Server];

    pub fn label(self) -> &'static str {
        match self {
            AttachmentMeaning::Off => "Off",
            AttachmentMeaning::EmbeddingGemma => "EmbeddingGemma 2",
            AttachmentMeaning::Server => "Server",
        }
    }

    /// What the choice does and costs, for a settings page.
    pub fn detail(self) -> &'static str {
        match self {
            AttachmentMeaning::Off => "The words in attachments are still searchable.",
            AttachmentMeaning::EmbeddingGemma => "Finds photos and scans by what they show: dog, blueish document. 1.5 GB.",
            AttachmentMeaning::Server => "The embeddings server below. Attachments leave the app for it.",
        }
    }
}

/// An OpenAI-compatible embeddings endpoint, such as llama-server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub url: String,
    #[serde(default = "default_server_model")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

fn default_server_model() -> String {
    "embeddinggemma-2".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    pub typos: Typos,
    pub last_word: LastWord,
    pub text_meaning: TextMeaning,
    pub attachment_meaning: AttachmentMeaning,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<Server>,
}

impl SearchSettings {
    /// Defaults that follow what is already on disk: a model downloaded before keeps being used.
    pub fn for_models(cache: &Path) -> SearchSettings {
        SearchSettings {
            text_meaning: if ModelStore::local(cache, &POTION_MULTILINGUAL_128M).is_some() {
                TextMeaning::Model2Vec
            } else {
                TextMeaning::Off
            },
            attachment_meaning: if cfg!(feature = "gemma") && ModelStore::local(cache, &EMBEDDINGGEMMA_2).is_some() {
                AttachmentMeaning::EmbeddingGemma
            } else {
                AttachmentMeaning::Off
            },
            ..SearchSettings::default()
        }
    }

    pub fn expand_options(&self) -> ExpandOptions {
        ExpandOptions { typos: self.typos, last_word: self.last_word, ..ExpandOptions::default() }
    }

    fn server_backend(&self) -> Option<MediaBackend> {
        self.server.as_ref().map(|s| MediaBackend::Remote { url: s.url.clone(), model: s.model.clone(), key: s.key.clone() })
    }

    /// The multimodal model attachments use, if any.
    pub fn attachment_backend(&self, cache: &Path) -> Option<MediaBackend> {
        match self.attachment_meaning {
            AttachmentMeaning::Off => None,
            AttachmentMeaning::EmbeddingGemma => Some(MediaBackend::Local { cache: cache.to_path_buf() }),
            AttachmentMeaning::Server => self.server_backend(),
        }
    }

    /// The model message text uses, if any.
    pub fn text_provider(&self, cache: &Path) -> Option<TextProvider> {
        match self.text_meaning {
            TextMeaning::Off => None,
            TextMeaning::Model2Vec => Some(TextProvider::Model2Vec { cache: cache.to_path_buf() }),
            TextMeaning::EmbeddingGemma => Some(TextProvider::Media(MediaBackend::Local { cache: cache.to_path_buf() })),
            TextMeaning::Server => self.server_backend().map(TextProvider::Media),
        }
    }
}

/// How message text gets its vectors.
#[derive(Debug, Clone, PartialEq)]
pub enum TextProvider {
    Model2Vec { cache: PathBuf },
    /// The multimodal model, for text only.
    Media(MediaBackend),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_kebab_case_json_and_missing_fields_take_defaults() {
        let json = r#"{"typos": "fewer", "text_meaning": "x"}"#;
        assert!(serde_json::from_str::<SearchSettings>(json).is_err(), "unknown choices are refused");
        let parsed: SearchSettings = serde_json::from_str(r#"{"typos": "fewer", "attachment_meaning": "embedding-gemma"}"#).unwrap();
        assert_eq!(parsed.typos, Typos::Fewer);
        assert_eq!(parsed.attachment_meaning, AttachmentMeaning::EmbeddingGemma);
        assert_eq!(parsed.text_meaning, TextMeaning::Off);
        assert_eq!(parsed.last_word, LastWord::Prefix);
        let written = serde_json::to_string(&SearchSettings { text_meaning: TextMeaning::Model2Vec, ..parsed }).unwrap();
        assert!(written.contains(r#""text_meaning":"model2vec""#), "{written}");
    }

    #[test]
    fn a_server_choice_without_a_server_has_no_backend() {
        let cache = Path::new("/nonexistent");
        let settings = SearchSettings { attachment_meaning: AttachmentMeaning::Server, text_meaning: TextMeaning::Server, ..Default::default() };
        assert_eq!(settings.attachment_backend(cache), None);
        assert_eq!(settings.text_provider(cache), None);
        let with = SearchSettings { server: Some(Server { url: "http://127.0.0.1:8080".into(), model: "embeddinggemma-2".into(), key: None }), ..settings };
        assert!(matches!(with.attachment_backend(cache), Some(MediaBackend::Remote { .. })));
    }
}
