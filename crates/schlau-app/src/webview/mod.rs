//! The secure HTML view. macOS hosts a locked-down WKWebView; elsewhere the native reader is the
//! only view for now.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{attach, hide, show};

#[cfg(not(target_os = "macos"))]
pub fn attach(_: raw_window_handle::RawWindowHandle) {}
#[cfg(not(target_os = "macos"))]
pub fn show(_: &str, _: [f32; 4]) {}
#[cfg(not(target_os = "macos"))]
pub fn hide() {}
