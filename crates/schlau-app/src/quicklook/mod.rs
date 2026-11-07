//! The system Quick Look panel, on macOS. Elsewhere there is none and schlau's own viewer is used.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{attach, close, current, is_open, show, step};

#[cfg(not(target_os = "macos"))]
mod none {
    use std::path::PathBuf;

    pub fn attach(_: raw_window_handle::RawWindowHandle) {}
    pub fn show(_: &[PathBuf], _: usize) {}
    pub fn is_open() -> bool {
        false
    }
    pub fn close() {}
    pub fn step(_: isize) {}
    pub fn current() -> Option<PathBuf> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub use none::{attach, close, current, is_open, show, step};

/// Whether this platform has a system panel to use.
pub const AVAILABLE: bool = cfg!(target_os = "macos");
