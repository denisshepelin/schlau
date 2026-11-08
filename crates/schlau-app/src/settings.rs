//! Preferences that outlive a session, in `settings.json` next to the database. Every setting is an enum
//! of its choices; `Setting` lists the settings and `Choice` their choices, so the settings pane shows
//! a new choice by itself.

use std::path::{Path, PathBuf};

use schlau_search::{LastWord, Typos};
use schlau_sync::{AttachmentMeaning, ScreenerConfig, SearchSettings, Server, TextMeaning};
use serde::{Deserialize, Serialize};

/// Which viewer `space` opens for attachments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Previewer {
    /// schlau's own viewer: pictures and PDF pages drawn in the window, text for documents.
    #[default]
    Schlau,
    /// The system Quick Look panel, for every kind of file macOS can preview.
    Macos,
}

impl Previewer {
    pub const ALL: [Previewer; 2] = [Previewer::Schlau, Previewer::Macos];

    pub fn label(self) -> &'static str {
        match self {
            Previewer::Schlau => "schlau",
            Previewer::Macos => "macOS Quick Look",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Previewer::Schlau => "Pictures and PDF pages drawn in the window, the text of other documents.",
            Previewer::Macos => "The system panel, for every kind of file macOS can preview.",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub search: SearchSettings,
    #[serde(alias = "previewer")]
    pub preview: Previewer,
    /// The screened folder and where screened senders go.
    pub screener: ScreenerConfig,
}

/// A group of settings on the settings pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Words,
    Meaning,
    Preview,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Words => "Word search",
            Section::Meaning => "Meaning search",
            Section::Preview => "Quick look",
        }
    }
}

/// One setting, with its choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Typos,
    LastWord,
    TextMeaning,
    AttachmentMeaning,
    Preview,
}

impl Setting {
    #[cfg(test)]
    pub const ALL: [Setting; 5] = [Setting::Typos, Setting::LastWord, Setting::TextMeaning, Setting::AttachmentMeaning, Setting::Preview];

    pub fn name(self) -> &'static str {
        match self {
            Setting::Typos => "Typo tolerance",
            Setting::LastWord => "Last word typed",
            Setting::TextMeaning => "In messages",
            Setting::AttachmentMeaning => "In attachments",
            Setting::Preview => "Open with",
        }
    }

    pub fn section(self) -> Section {
        match self {
            Setting::Typos | Setting::LastWord => Section::Words,
            Setting::TextMeaning | Setting::AttachmentMeaning => Section::Meaning,
            Setting::Preview => Section::Preview,
        }
    }

    pub fn choices(self) -> Vec<Choice> {
        match self {
            Setting::Typos => Typos::ALL.into_iter().map(Choice::Typos).collect(),
            Setting::LastWord => LastWord::ALL.into_iter().map(Choice::LastWord).collect(),
            Setting::TextMeaning => TextMeaning::ALL.into_iter().map(Choice::TextMeaning).collect(),
            Setting::AttachmentMeaning => AttachmentMeaning::ALL.into_iter().map(Choice::AttachmentMeaning).collect(),
            Setting::Preview => Previewer::ALL.into_iter().map(Choice::Preview).collect(),
        }
    }

    pub fn current(self, s: &Settings) -> Choice {
        match self {
            Setting::Typos => Choice::Typos(s.search.typos),
            Setting::LastWord => Choice::LastWord(s.search.last_word),
            Setting::TextMeaning => Choice::TextMeaning(s.search.text_meaning),
            Setting::AttachmentMeaning => Choice::AttachmentMeaning(s.search.attachment_meaning),
            Setting::Preview => Choice::Preview(s.preview),
        }
    }

    /// The choice `delta` steps from the current one, clamped to the ends.
    pub fn stepped(self, s: &Settings, delta: isize) -> Choice {
        let choices = self.choices();
        let at = choices.iter().position(|&c| c == self.current(s)).unwrap_or(0) as isize;
        choices[(at + delta).clamp(0, choices.len() as isize - 1) as usize]
    }
}

/// One choice of one setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Typos(Typos),
    LastWord(LastWord),
    TextMeaning(TextMeaning),
    AttachmentMeaning(AttachmentMeaning),
    Preview(Previewer),
}

impl Choice {
    /// Every choice of every setting, setting by setting.
    #[cfg(test)]
    pub fn all() -> Vec<Choice> {
        Setting::ALL.into_iter().flat_map(Setting::choices).collect()
    }

    pub fn setting(self) -> Setting {
        match self {
            Choice::Typos(_) => Setting::Typos,
            Choice::LastWord(_) => Setting::LastWord,
            Choice::TextMeaning(_) => Setting::TextMeaning,
            Choice::AttachmentMeaning(_) => Setting::AttachmentMeaning,
            Choice::Preview(_) => Setting::Preview,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Choice::Typos(c) => c.label(),
            Choice::LastWord(c) => c.label(),
            Choice::TextMeaning(c) => c.label(),
            Choice::AttachmentMeaning(c) => c.label(),
            Choice::Preview(c) => c.label(),
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Choice::Typos(c) => c.detail(),
            Choice::LastWord(c) => c.detail(),
            Choice::TextMeaning(c) => c.detail(),
            Choice::AttachmentMeaning(c) => c.detail(),
            Choice::Preview(c) => c.detail(),
        }
    }

    /// The model the choice downloads, if any.
    pub fn model(self) -> Option<&'static schlau_embed::ModelSpec> {
        match self {
            Choice::TextMeaning(TextMeaning::Model2Vec) => Some(&schlau_embed::POTION_MULTILINGUAL_128M),
            Choice::TextMeaning(TextMeaning::EmbeddingGemma) | Choice::AttachmentMeaning(AttachmentMeaning::EmbeddingGemma) => {
                Some(&schlau_embed::EMBEDDINGGEMMA_2)
            }
            _ => None,
        }
    }

    /// The setting's name and the choice, as a toast confirms it.
    pub fn title(self) -> String {
        let setting = self.setting();
        match setting.section() {
            Section::Meaning => format!("Meaning search {}: {}", setting.name().to_lowercase(), self.label()),
            Section::Preview => format!("Quick look with {}", self.label()),
            Section::Words => format!("{}: {}", setting.name(), self.label()),
        }
    }

    pub fn is_current(self, s: &Settings) -> bool {
        self.setting().current(s) == self
    }

    pub fn apply(self, s: &mut Settings) {
        match self {
            Choice::Typos(c) => s.search.typos = c,
            Choice::LastWord(c) => s.search.last_word = c,
            Choice::TextMeaning(c) => s.search.text_meaning = c,
            Choice::AttachmentMeaning(c) => s.search.attachment_meaning = c,
            Choice::Preview(c) => s.preview = c,
        }
    }

    /// Whether the choice needs the embeddings server.
    pub fn needs_server(self) -> bool {
        matches!(self, Choice::TextMeaning(TextMeaning::Server) | Choice::AttachmentMeaning(AttachmentMeaning::Server))
    }

    /// Whether the search engine must hear about it (the model or how words match).
    pub fn changes_search(self) -> bool {
        !matches!(self, Choice::Preview(_))
    }
}

/// The settings file, and the environment that overrides it without being written back.
pub struct SettingsFile {
    path: PathBuf,
    stored: Settings,
    server_from_env: Option<Server>,
}

/// `SCHLAU_EMBED_URL` (with `SCHLAU_EMBED_MODEL`, `SCHLAU_EMBED_KEY`) names an embeddings server.
fn server_from_env() -> Option<Server> {
    let url = std::env::var("SCHLAU_EMBED_URL").ok().filter(|u| !u.trim().is_empty())?;
    Some(Server {
        url,
        model: std::env::var("SCHLAU_EMBED_MODEL").unwrap_or_else(|_| "embeddinggemma-2".into()),
        key: std::env::var("SCHLAU_EMBED_KEY").ok(),
    })
}

impl SettingsFile {
    /// The settings in `path`. Without a file, search uses the models already downloaded to `models`.
    pub fn load(path: &Path, models: &Path) -> SettingsFile {
        let stored = std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| Settings { search: SearchSettings::for_models(models), ..Settings::default() });
        SettingsFile { path: path.to_path_buf(), stored, server_from_env: server_from_env() }
    }

    /// In-memory settings, for tests and runs without a data directory.
    pub fn transient() -> SettingsFile {
        SettingsFile { path: PathBuf::new(), stored: Settings::default(), server_from_env: None }
    }

    /// The settings in force: the file's, with a server named in the environment used for attachments.
    pub fn effective(&self) -> Settings {
        let mut s = self.stored.clone();
        if let Some(server) = &self.server_from_env {
            s.search.server = Some(server.clone());
            s.search.attachment_meaning = AttachmentMeaning::Server;
        }
        s
    }

    /// The server named in the environment, which overrides the file's for this run.
    pub fn env_server(&self) -> Option<&Server> {
        self.server_from_env.as_ref()
    }

    /// Applies `choice` and writes the file.
    pub fn choose(&mut self, choice: Choice) -> std::io::Result<()> {
        choice.apply(&mut self.stored);
        self.save()
    }

    /// Sets the embeddings server (`None` forgets it) and writes the file. A key stays as it was.
    pub fn set_server(&mut self, url: &str, model: &str) -> std::io::Result<()> {
        let key = self.stored.search.server.as_ref().and_then(|s| s.key.clone());
        let model = Some(model.trim()).filter(|m| !m.is_empty()).unwrap_or("embeddinggemma-2");
        self.stored.search.server =
            Some(url.trim()).filter(|u| !u.is_empty()).map(|url| Server { url: url.to_string(), model: model.to_string(), key });
        self.save()
    }

    /// Replaces the screener's setup and writes the file.
    pub fn set_screener(&mut self, config: ScreenerConfig) -> std::io::Result<()> {
        self.stored.screener = config;
        self.save()
    }

    /// The server as the file has it.
    pub fn stored_server(&self) -> Option<&Server> {
        self.stored.search.server.as_ref()
    }

    fn save(&self) -> std::io::Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = serde_json::to_vec_pretty(&self.stored).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let (path, models) = (dir.path().join("settings.json"), dir.path().join("models"));
        assert_eq!(SettingsFile::load(&path, &models).effective(), Settings::default());
        let mut file = SettingsFile::load(&path, &models);
        file.choose(Choice::Preview(Previewer::Macos)).unwrap();
        file.choose(Choice::Typos(Typos::Fewer)).unwrap();
        let again = SettingsFile::load(&path, &models).effective();
        assert_eq!((again.preview, again.search.typos), (Previewer::Macos, Typos::Fewer));
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(json.contains(r#""preview": "macos""#) && json.contains(r#""typos": "fewer""#), "{json}");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(SettingsFile::load(&path, &models).effective(), Settings::default(), "a broken file falls back");
        std::fs::write(&path, r#"{"previewer": "macos"}"#).unwrap();
        assert_eq!(SettingsFile::load(&path, &models).effective().preview, Previewer::Macos, "the first file's name still reads");
    }

    #[test]
    fn every_choice_is_listed_once_and_applies() {
        let all = Choice::all();
        assert_eq!(all.len(), 3 + 2 + 4 + 3 + 2);
        let mut settings = Settings::default();
        all.iter().for_each(|&c| {
            c.apply(&mut settings);
            assert!(c.is_current(&settings), "{c:?}");
        });
        let titles: std::collections::HashSet<String> = all.iter().map(|c| c.title()).collect();
        assert_eq!(titles.len(), all.len(), "titles tell choices apart");
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;

    #[test]
    fn stepping_a_setting_stops_at_its_ends() {
        let s = Settings::default();
        assert_eq!(Setting::Typos.stepped(&s, 1), Choice::Typos(Typos::Normal), "defaults to normal");
        assert_eq!(Setting::Typos.stepped(&s, -1), Choice::Typos(Typos::Fewer));
        assert_eq!(Setting::Typos.stepped(&s, -9), Choice::Typos(Typos::Off));
        assert_eq!(Setting::Preview.stepped(&s, 1), Choice::Preview(Previewer::Macos));
        assert!(Setting::ALL.iter().all(|&setting| setting.choices().iter().all(|c| c.setting() == setting)));
    }

    #[test]
    fn a_server_is_set_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = SettingsFile::load(&dir.path().join("settings.json"), dir.path());
        file.set_server(" http://127.0.0.1:8080 ", "").unwrap();
        let server = file.stored_server().unwrap();
        assert_eq!((server.url.as_str(), server.model.as_str()), ("http://127.0.0.1:8080", "embeddinggemma-2"));
        file.set_server("", "x").unwrap();
        assert_eq!(file.stored_server(), None);
    }
}
