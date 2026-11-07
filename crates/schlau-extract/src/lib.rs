//! Turns email attachments into extracted text and downscaled images for
//! full-text indexing and multimodal embedding.
//!
//! Everything here runs on untrusted input: sizes are capped, decoders have
//! allocation limits, and panics inside third-party parsers are caught and
//! reported as [`ExtractError::Malformed`].

mod image;
mod office;
mod pdf;
mod text;

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Resource bounds applied to a single extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Inputs larger than this are rejected with [`ExtractError::TooLarge`].
    pub max_bytes: usize,
    /// Extracted text is truncated to this many characters.
    pub max_text_chars: usize,
    /// Number of PDF pages rendered to images.
    pub max_pages: usize,
    /// Longest side of any produced image, in pixels.
    pub max_side: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 25 * 1024 * 1024,
            max_text_chars: 20_000,
            max_pages: 2,
            max_side: 1024,
        }
    }
}

/// An encoded raster image ready to send to an embedding model.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedImage {
    /// Encoded image data.
    pub bytes: Vec<u8>,
    /// `"image/jpeg"` or `"image/png"`.
    pub mime: &'static str,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl std::fmt::Debug for EncodedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedImage")
            .field("bytes", &self.bytes.len())
            .field("mime", &self.mime)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// What was extracted from an attachment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extracted {
    /// Whitespace-normalized text, if the attachment carries meaningful text.
    pub text: Option<String>,
    /// Downscaled images (a photo, or rendered PDF pages).
    pub images: Vec<EncodedImage>,
}

/// Why an attachment could not be extracted.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// The input exceeds [`Limits::max_bytes`].
    #[error("attachment too large: {0} bytes")]
    TooLarge(usize),
    /// The attachment type is not handled.
    #[error("unsupported attachment type: {0}")]
    Unsupported(String),
    /// The attachment could not be parsed.
    #[error("malformed attachment: {0}")]
    Malformed(String),
}

/// Broad attachment category, deciding which extractor runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Raster image (JPEG, PNG, WebP, GIF, BMP, TIFF).
    Image,
    /// PDF document.
    Pdf,
    /// Plain text, CSV, Markdown or iCalendar.
    Text,
    /// DOCX, PPTX, XLSX or ODT.
    Office,
}

fn kind_from_mime(mime: &str) -> Option<Kind> {
    match mime {
        "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/png" | "image/webp" | "image/gif"
        | "image/bmp" | "image/x-bmp" | "image/x-ms-bmp" | "image/tiff" => Some(Kind::Image),
        "application/pdf" | "application/x-pdf" => Some(Kind::Pdf),
        "text/plain" | "text/csv" | "text/markdown" | "text/x-markdown" | "text/calendar"
        | "application/ics" => Some(Kind::Text),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.oasis.opendocument.text" => Some(Kind::Office),
        _ => None,
    }
}

fn kind_from_extension(ext: &str) -> Option<Kind> {
    match ext {
        "jpg" | "jpeg" | "jpe" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff" => {
            Some(Kind::Image)
        }
        "pdf" => Some(Kind::Pdf),
        "txt" | "text" | "md" | "markdown" | "csv" | "ics" => Some(Kind::Text),
        "docx" | "pptx" | "xlsx" | "odt" => Some(Kind::Office),
        _ => None,
    }
}

fn normalized_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn extension(name: Option<&str>) -> Option<String> {
    name.and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext.trim().to_ascii_lowercase())
}

/// What `extract` would do with this attachment, from MIME type and file
/// extension (case-insensitive; the extension is used when the MIME type is
/// `application/octet-stream` or empty). `None` when unsupported.
pub fn kind(name: Option<&str>, mime: &str) -> Option<Kind> {
    let mime = normalized_mime(mime);
    match mime.as_str() {
        "" | "application/octet-stream" => extension(name).as_deref().and_then(kind_from_extension),
        "application/zip" | "application/x-zip-compressed" => extension(name)
            .as_deref()
            .and_then(kind_from_extension)
            .filter(|k| *k == Kind::Office),
        other => kind_from_mime(other),
    }
}

/// Extracts text and downscaled images from an attachment.
pub fn extract(
    name: Option<&str>,
    mime: &str,
    bytes: &[u8],
    limits: &Limits,
) -> Result<Extracted, ExtractError> {
    if bytes.len() > limits.max_bytes {
        return Err(ExtractError::TooLarge(bytes.len()));
    }
    let kind = kind(name, mime).ok_or_else(|| {
        ExtractError::Unsupported(match name {
            Some(n) => format!("{mime} ({n})"),
            None => mime.to_owned(),
        })
    })?;
    match kind {
        Kind::Image => guarded(|| image::extract(bytes, limits)),
        Kind::Pdf => guarded(|| pdf::extract(bytes, limits)),
        Kind::Office => guarded(|| office::extract(bytes, limits)),
        Kind::Text => {
            let is_calendar = normalized_mime(mime) == "text/calendar"
                || normalized_mime(mime) == "application/ics"
                || extension(name).as_deref() == Some("ics");
            Ok(text::extract(bytes, is_calendar, limits))
        }
    }
}

fn guarded<F>(f: F) -> Result<Extracted, ExtractError>
where
    F: FnOnce() -> Result<Extracted, ExtractError>,
{
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        Err(ExtractError::Malformed(format!(
            "parser panicked: {message}"
        )))
    })
}
