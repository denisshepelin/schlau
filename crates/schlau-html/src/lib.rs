//! Turns untrusted email bodies into something safe and private to display.
//!
//! - [`sanitize`] cleans HTML for a web view with JavaScript disabled, applies the remote
//!   content policy, removes tracking pixels and cleans links; [`wrap_document`] and
//!   [`content_security_policy`] produce the document and CSP around it.
//! - [`text_to_html`] renders `text/plain` bodies with the same link cleaning.
//! - [`to_plain_text`] extracts text for search and embeddings, [`snippet`] makes previews.
//! - [`to_blocks`] produces a simple block tree for a native reader.

mod blocks;
mod css;
mod document;
mod dom;
mod links;
mod options;
mod pixel;
mod plain;
mod sanitize;
mod snippet;
mod text;
mod trackers;
mod urls;

pub use blocks::{Block, ImageSource, Inline, to_blocks, to_blocks_with};
pub use document::{content_security_policy, wrap_document};
pub use links::{CleanedLink, clean_link};
pub use options::{BodyKind, ColorScheme, Options, RemotePolicy, Sanitized, Theme, TrackerReport};
pub use plain::to_plain_text;
pub use sanitize::sanitize;
pub use snippet::snippet;
pub use text::{TextFormat, text_to_html, text_to_html_with};
pub use trackers::{TrackerKind, tracker_host_count};
