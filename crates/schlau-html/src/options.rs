use std::collections::{BTreeSet, HashMap};

/// What to do with images, backgrounds and CSS `url()` that point at remote hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemotePolicy {
    /// Load nothing from the network. Every remote reference is removed and counted.
    #[default]
    Block,
    /// Load remote content; tracking pixels are still removed.
    Allow,
    /// Load remote content except anything hosted by a known tracker or email service provider.
    AllowExceptTrackers,
}

impl RemotePolicy {
    pub fn allows_remote(self) -> bool {
        !matches!(self, RemotePolicy::Block)
    }
}

/// Sanitizer configuration.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub remote: RemotePolicy,
    /// Content-ID (without `<>` and without the `cid:` prefix) to a local URL such as
    /// `schlau-cid://<blobId>`. References to unknown Content-IDs are removed.
    pub cid_urls: HashMap<String, String>,
}

impl Options {
    pub fn with_remote(remote: RemotePolicy) -> Self {
        Options {
            remote,
            ..Options::default()
        }
    }

    pub fn with_cid_urls(mut self, cid_urls: HashMap<String, String>) -> Self {
        self.cid_urls = cid_urls;
        self
    }

    pub(crate) fn resolve_cid(&self, content_id: &str) -> Option<&str> {
        let id = content_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        self.cid_urls
            .get(id)
            .or_else(|| {
                self.cid_urls
                    .iter()
                    .find(|(key, _)| key.trim_matches(['<', '>']).eq_ignore_ascii_case(id))
                    .map(|(_, url)| url)
            })
            .map(String::as_str)
    }
}

/// What the sanitizer removed or rewrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackerReport {
    /// Tracking pixels removed (tiny, hidden, or served from a tracking endpoint).
    pub pixels_removed: usize,
    /// Tracker and email-service domains seen in pixels, blocked content and link wrappers.
    pub tracker_domains: Vec<String>,
    /// Links whose redirect wrapper was replaced by the real destination.
    pub links_unwrapped: usize,
    /// Tracking query parameters removed from links.
    pub params_stripped: usize,
    /// Remote references (images, backgrounds, CSS `url()`, srcset candidates) not loaded.
    pub remote_blocked: usize,
}

impl TrackerReport {
    pub fn is_clean(&self) -> bool {
        *self == TrackerReport::default()
    }
}

/// Mutable tally used while rewriting; turned into a [`TrackerReport`] at the end.
#[derive(Debug, Default)]
pub(crate) struct Tally {
    pub pixels_removed: usize,
    pub links_unwrapped: usize,
    pub params_stripped: usize,
    pub remote_blocked: usize,
    pub domains: BTreeSet<String>,
}

impl Tally {
    pub fn into_report(self) -> TrackerReport {
        TrackerReport {
            pixels_removed: self.pixels_removed,
            tracker_domains: self.domains.into_iter().collect(),
            links_unwrapped: self.links_unwrapped,
            params_stripped: self.params_stripped,
            remote_blocked: self.remote_blocked,
        }
    }
}

/// Where the sanitized body came from; decides the document styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Html,
    PlainText,
}

/// Safe HTML fragment plus the report of what was removed.
#[derive(Debug, Clone)]
pub struct Sanitized {
    pub html: String,
    pub report: TrackerReport,
    pub remote: RemotePolicy,
    pub kind: BodyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorScheme {
    Light,
    Dark,
    /// Follow the system appearance.
    #[default]
    Auto,
}

/// Presentation settings for [`crate::wrap_document`].
#[derive(Debug, Clone)]
pub struct Theme {
    pub color_scheme: ColorScheme,
    pub font_family: String,
    pub monospace_family: String,
    pub font_size_px: u16,
    pub text_color: String,
    pub background_color: String,
    pub link_color: String,
    pub dark_text_color: String,
    pub dark_background_color: String,
    pub dark_link_color: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            color_scheme: ColorScheme::Auto,
            font_family: "-apple-system, system-ui, sans-serif".into(),
            monospace_family: "ui-monospace, SFMono-Regular, Menlo, monospace".into(),
            font_size_px: 15,
            text_color: "#1d1d1f".into(),
            background_color: "#ffffff".into(),
            link_color: "#0a66d8".into(),
            dark_text_color: "#e6e6e6".into(),
            dark_background_color: "#1c1c1e".into(),
            dark_link_color: "#6cb4ff".into(),
        }
    }
}
