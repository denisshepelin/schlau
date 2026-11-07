//! The engine thread. It owns the store's writer; everything else reads. The UI sends it commands
//! and learns about new data through `Notify`, which bumps a version and wakes the UI.

mod attachments;
mod convert;
mod engine;
mod fixtures;
mod index;
mod push;
mod remote;
mod semantic;

pub use attachments::{AttachmentWorker, BlobError, BlobSource, FixtureBlobs, JmapBlobs, MediaBackend, SharedMedia};
pub use engine::Command;
pub use fixtures::FixtureError;
pub use fixtures::load_fixtures;
pub use engine::Connect;
pub use engine::Engine;
pub use engine::EngineHandle;
pub use engine::Notify;
pub use engine::Status;
pub use remote::DynClient;
pub use remote::Remote;
pub use remote::SyncError;
pub use semantic::SharedEmbedder;
