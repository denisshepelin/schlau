//! Attachments for search: which files still need their text extracted or their vectors made, and
//! storing both. Extracted text joins the message's word index; vectors live apart from the message
//! vectors because they come from another model.

use rusqlite::{OptionalExtension, params};

use crate::model::{Addresses, Attachment, Body};
use crate::read::Reader;
use crate::write::WriteTx;
use crate::Result;

/// Largest attachment worth fetching for search.
pub const MAX_INDEXED_SIZE: i64 = 25_000_000;
/// Vectors per attachment part are numbered below this; a vector's id packs message, part and chunk.
const CHUNKS: i64 = 8;
const PARTS: i64 = 32;

/// An attachment waiting for extraction, or for vectors from the current model.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAttachment {
    pub message: i64,
    pub message_id: String,
    pub part: i64,
    pub attachment: Attachment,
    /// Its text was extracted before; only vectors are missing.
    pub extracted: bool,
}

/// What came out of one attachment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttachmentOutcome {
    pub text: Option<String>,
    /// f16 vectors (`vector::pack_f16`); empty without a model.
    pub vectors: Vec<Vec<u8>>,
    /// The model the attachment was run through, even when it gave no vectors, so it isn't retried.
    pub model: Option<String>,
    /// Nothing could be extracted, and why.
    pub skipped: Option<String>,
}

/// Packs a media vector's location into one id for a vector index.
pub fn media_vector_id(message: i64, part: i64, chunk: i64) -> i64 {
    (message * PARTS + part.min(PARTS - 1)) * CHUNKS + chunk.min(CHUNKS - 1)
}

/// The message and attachment part of a media vector id.
pub fn media_vector_location(id: i64) -> (i64, i64) {
    let part = id / CHUNKS;
    (part / PARTS, part % PARTS)
}

/// Attachments worth indexing: files people attached, not images placed inside the HTML.
fn indexable(a: &Attachment) -> bool {
    !(a.inline && a.cid.is_some()) && a.size <= MAX_INDEXED_SIZE
}

impl WriteTx<'_> {
    /// Notes a message's attachments for the attachment worker, keeping what an earlier pass found.
    pub(crate) fn register_attachments(&self, rowid: i64, body: &Body) -> Result<()> {
        let mut insert = self.tx.prepare_cached(
            "INSERT OR IGNORE INTO attachment_index(message, part, blob_id, name, mime, size)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        body.attachments.iter().enumerate().filter(|(_, a)| indexable(a)).try_for_each(|(part, a)| {
            insert.execute(params![rowid, part as i64, a.blob_id, a.name, a.mime, a.size]).map(|_| ())
        })?;
        Ok(())
    }

    /// Stores what the worker got from an attachment and re-indexes the message's words with it.
    /// `body_text` is the message body as the word index holds it.
    pub fn index_attachment(&self, message: i64, part: i64, outcome: &AttachmentOutcome, body_text: &str) -> Result<()> {
        let state = if outcome.skipped.is_some() { 2 } else { 1 };
        let text: Option<String> = match &outcome.text {
            Some(t) => Some(t.clone()),
            None => self
                .tx
                .prepare_cached("SELECT text FROM attachment_index WHERE message = ?1 AND part = ?2")?
                .query_row(params![message, part], |r| r.get(0))
                .optional()?
                .flatten(),
        };
        self.tx
            .prepare_cached(
                "UPDATE attachment_index SET state = ?3, model = ?4, text = ?5, error = ?6
                 WHERE message = ?1 AND part = ?2",
            )?
            .execute(params![message, part, state, outcome.model, text, outcome.skipped])?;
        self.tx.prepare_cached("DELETE FROM media_vectors WHERE message = ?1 AND part = ?2")?.execute(params![message, part])?;
        let mut insert = self.tx.prepare_cached("INSERT INTO media_vectors(message, part, chunk, vec) VALUES (?1, ?2, ?3, ?4)")?;
        outcome.vectors.iter().take(CHUNKS as usize).enumerate().try_for_each(|(chunk, v)| {
            insert.execute(params![message, part, chunk as i64, v]).map(|_| ())
        })?;
        if outcome.text.is_some() {
            self.reindex_words(message, body_text)?;
        }
        Ok(())
    }

    /// Drops every media vector, for a change of model.
    pub fn clear_media_vectors(&self) -> Result<()> {
        self.tx.execute("DELETE FROM media_vectors", [])?;
        self.tx.execute("UPDATE attachment_index SET model = NULL", [])?;
        Ok(())
    }

    fn reindex_words(&self, message: i64, body_text: &str) -> Result<()> {
        let (subject, addresses, attachments): (String, String, Option<String>) = self
            .tx
            .prepare_cached(
                "SELECT m.subject, m.addresses, b.attachments FROM messages m
                 LEFT JOIN bodies b ON b.message = m.rowid WHERE m.rowid = ?1",
            )?
            .query_row([message], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let addresses: Addresses = serde_json::from_str(&addresses)?;
        let attachments: Vec<Attachment> = attachments.and_then(|a| serde_json::from_str(&a).ok()).unwrap_or_default();
        let words = self.attachment_words(message, &attachments)?;
        self.write_fts(message, &subject, &addresses, body_text, &words)
    }

    /// Attachment names and every extracted text, for the word index's `attachments` column.
    pub(crate) fn attachment_words(&self, message: i64, attachments: &[Attachment]) -> Result<String> {
        let names = attachments.iter().filter_map(|a| a.name.as_deref());
        let mut stmt =
            self.tx.prepare_cached("SELECT text FROM attachment_index WHERE message = ?1 AND text IS NOT NULL ORDER BY part")?;
        let texts: Vec<String> = stmt.query_map([message], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(names.map(str::to_string).chain(texts).collect::<Vec<_>>().join("\n"))
    }
}

impl Reader {
    /// Attachments to work on, newest mail first: ones never extracted, then, when a model is loaded,
    /// ones whose vectors are missing or from another model.
    pub fn pending_attachments(&self, model: Option<&str>, limit: usize) -> Result<Vec<PendingAttachment>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.message, m.id, a.part, a.blob_id, a.name, a.mime, a.size, a.state
             FROM attachment_index a JOIN messages m ON m.rowid = a.message
             WHERE a.state = 0 OR (?1 IS NOT NULL AND a.state = 1 AND (a.model IS NULL OR a.model != ?1))
             ORDER BY a.state, m.received_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![model, limit as i64], |r| {
            Ok(PendingAttachment {
                message: r.get(0)?,
                message_id: r.get(1)?,
                part: r.get(2)?,
                attachment: Attachment {
                    blob_id: r.get(3)?,
                    name: r.get(4)?,
                    mime: r.get(5)?,
                    size: r.get(6)?,
                    cid: None,
                    inline: false,
                },
                extracted: r.get::<_, i64>(7)? == 1,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// How many attachments are done for `model` (or extracted, without one), of all indexable ones.
    pub fn attachment_progress(&self, model: Option<&str>) -> Result<(i64, i64)> {
        Ok(self.conn.prepare_cached(
            "SELECT count(*) FILTER (WHERE state = 2 OR (state = 1 AND (?1 IS NULL OR model = ?1))), count(*)
             FROM attachment_index",
        )?.query_row([model], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// Every media vector, with its packed id (see [`media_vector_id`]).
    pub fn media_vectors(&self, mut each: impl FnMut(i64, &[u8])) -> Result<()> {
        let mut stmt = self.conn.prepare_cached("SELECT message, part, chunk, vec FROM media_vectors ORDER BY message, part, chunk")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let blob = row.get_ref(3)?.as_blob().map_err(rusqlite::Error::from)?;
            each(media_vector_id(row.get(0)?, row.get(1)?, row.get(2)?), blob);
        }
        Ok(())
    }

    pub fn media_vector_count(&self) -> Result<i64> {
        Ok(self.conn.prepare_cached("SELECT count(*) FROM media_vectors")?.query_row([], |r| r.get(0))?)
    }

    /// An attachment's file name.
    pub fn attachment_name(&self, message: i64, part: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT name FROM attachment_index WHERE message = ?1 AND part = ?2")?
            .query_row(params![message, part], |r| r.get(0))
            .optional()?
            .flatten())
    }
}
