//! The local replica of one JMAP account. One SQLite file per account, one writer (sync), any number
//! of readers (the UI's workers).

mod actions;
mod attachments;
mod contacts;
mod fts;
mod model;
mod read;
mod schema;
mod search;
mod threads;
mod write;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

pub use attachments::{AttachmentOutcome, MAX_INDEXED_SIZE, PendingAttachment, media_vector_id, media_vector_location};
pub use fts::FtsHit;
pub use model::Action;
pub use model::Address;
pub use model::Addresses;
pub use model::Attachment;
pub use model::Body;
pub use model::ContactGroup;
pub use model::ContactRecord;
pub use model::Misrouted;
pub use model::RoutingStats;
pub use model::ScreenerSender;
pub use model::BodyIndex;
pub use model::Flags;
pub use model::Mailbox;
pub use model::MessageRecord;
pub use model::MessageUpdate;
pub use model::MessageView;
pub use model::OutboxItem;
pub use model::PageCursor;
pub use model::ThreadRow;
pub use read::EmbedInput;
pub use read::Reader;
pub use search::{MeaningChannel, MeaningInput};
pub use search::SearchHit;
pub use write::WriteTx;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The writer. Owns the schema; hands out readers.
pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let path = path.as_ref().to_path_buf();
        let mut conn = Connection::open(&path)?;
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        Ok(Store { conn, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Opens a read-only connection to the same database.
    pub fn reader(&self) -> Result<Reader> {
        open_reader(&self.path)
    }

    /// Runs `f` in one transaction. List rows of the threads it touched are rebuilt before commit.
    pub fn write<T>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T>) -> Result<T> {
        let tx = WriteTx { tx: self.conn.transaction()?, touched: RefCell::new(Default::default()) };
        let value = f(&tx)?;
        tx.finish()?;
        Ok(value)
    }

    /// Asks SQLite to merge FTS segments; run when idle after large syncs.
    pub fn optimize(&self) -> Result<()> {
        self.conn.execute_batch("INSERT INTO messages_fts(messages_fts) VALUES ('optimize'); PRAGMA optimize;")?;
        Ok(())
    }
}

/// A reader for a database another process or thread writes.
pub fn open_reader(path: &Path) -> Result<Reader> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.execute_batch(
        "PRAGMA query_only = ON; PRAGMA mmap_size = 1073741824; PRAGMA cache_size = -32768; PRAGMA busy_timeout = 5000;",
    )?;
    conn.set_prepared_statement_cache_capacity(256);
    Ok(Reader { conn })
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests;
