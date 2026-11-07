use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::Result;
use crate::model::{Action, Attachment, Body, Flags, Mailbox, MessageView, OutboxItem, PageCursor, ThreadRow};

/// A read-only connection. The UI keeps one per worker; reads never wait on the sync writer.
pub struct Reader {
    pub(crate) conn: Connection,
}

const THREAD_ROW_COLUMNS: &str = "thread_id, latest_at, latest_message, subject, participants, preview, \
                                  message_count, unread_count, flagged, has_attachment";

impl Reader {
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM meta WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    pub fn mailboxes(&self) -> Result<Vec<Mailbox>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, parent_id, role, sort_order, total_emails, unread_emails, total_threads,
                    unread_threads, is_subscribed
             FROM mailboxes ORDER BY sort_order, name",
        )?;
        let rows = stmt.query_map([], mailbox_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn mailbox_by_role(&self, role: &str) -> Result<Option<Mailbox>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT id, name, parent_id, role, sort_order, total_emails, unread_emails, total_threads,
                        unread_threads, is_subscribed
                 FROM mailboxes WHERE role = ?1 LIMIT 1",
            )?
            .query_row([role], mailbox_from_row)
            .optional()?)
    }

    /// A page of a mailbox's conversations, newest first, after `cursor` (keyset paging).
    pub fn thread_page(&self, mailbox_id: &str, cursor: Option<PageCursor<'_>>, limit: usize) -> Result<Vec<ThreadRow>> {
        let limit = limit as i64;
        match cursor {
            None => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT {THREAD_ROW_COLUMNS} FROM thread_rows WHERE mailbox_id = ?1
                     ORDER BY latest_at DESC, thread_id LIMIT ?2"
                ))?;
                let rows = stmt.query_map(params![mailbox_id, limit], thread_row_from_row)?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            }
            Some(c) => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT {THREAD_ROW_COLUMNS} FROM thread_rows
                     WHERE mailbox_id = ?1 AND (latest_at < ?2 OR (latest_at = ?2 AND thread_id > ?3))
                     ORDER BY latest_at DESC, thread_id LIMIT ?4"
                ))?;
                let rows = stmt.query_map(params![mailbox_id, c.latest_at, c.thread_id, limit], thread_row_from_row)?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            }
        }
    }

    pub fn thread_count(&self, mailbox_id: &str) -> Result<i64> {
        Ok(self
            .conn
            .prepare_cached("SELECT count(*) FROM thread_rows WHERE mailbox_id = ?1")?
            .query_row([mailbox_id], |r| r.get(0))?)
    }

    /// The list rows of the given threads in a mailbox (or in any mailbox, the newest row per thread,
    /// when `mailbox_id` is None), in the order given.
    pub fn thread_rows(&self, mailbox_id: Option<&str>, thread_ids: &[String]) -> Result<Vec<ThreadRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {THREAD_ROW_COLUMNS} FROM thread_rows
             WHERE thread_id = ?1 AND (?2 IS NULL OR mailbox_id = ?2)
             ORDER BY latest_at DESC LIMIT 1"
        ))?;
        thread_ids
            .iter()
            .filter_map(|id| stmt.query_row(params![id, mailbox_id], thread_row_from_row).optional().transpose())
            .map(|r| r.map_err(Into::into))
            .collect()
    }

    pub fn thread_messages(&self, thread_id: &str) -> Result<Vec<MessageView>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.rowid, m.id, m.thread_id, m.received_at, m.subject, m.addresses, m.preview, m.flags,
                    m.has_attachment, m.body_state, m.message_id, m.refs,
                    (SELECT group_concat(mailbox_id, char(31)) FROM message_mailboxes WHERE message = m.rowid)
             FROM messages m WHERE m.thread_id = ?1 ORDER BY m.received_at, m.rowid",
        )?;
        let rows = stmt.query_map([thread_id], |r| {
            let addresses: String = r.get(5)?;
            let refs: Option<String> = r.get(11)?;
            let mailboxes: Option<String> = r.get(12)?;
            Ok(MessageView {
                rowid: r.get(0)?,
                id: r.get(1)?,
                thread_id: r.get(2)?,
                received_at: r.get(3)?,
                subject: r.get(4)?,
                addresses: serde_json::from_str(&addresses).unwrap_or_default(),
                preview: r.get(6)?,
                flags: Flags(r.get(7)?),
                has_attachment: r.get(8)?,
                body_fetched: r.get::<_, i64>(9)? != 0,
                message_id: r.get(10)?,
                references: refs.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
                mailbox_ids: mailboxes.map(|m| m.split('\u{1f}').map(str::to_string).collect()).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Message ids in a thread, for actions on a whole conversation.
    pub fn thread_message_ids(&self, thread_id: &str) -> Result<Vec<(String, Vec<String>)>> {
        Ok(self
            .thread_messages(thread_id)?
            .into_iter()
            .map(|m| (m.id, m.mailbox_ids))
            .collect())
    }

    /// The thread of each message (by rowid), in the order given; unknown rowids are skipped.
    pub fn threads_of(&self, messages: &[i64]) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached("SELECT thread_id FROM messages WHERE rowid = ?1")?;
        messages
            .iter()
            .filter_map(|rowid| stmt.query_row([rowid], |r| r.get::<_, String>(0)).optional().transpose())
            .map(|r| r.map_err(Into::into))
            .collect()
    }

    pub fn body(&self, message_id: &str) -> Result<Option<Body>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT b.text, b.html, b.attachments FROM bodies b JOIN messages m ON m.rowid = b.message
                 WHERE m.id = ?1",
            )?
            .query_row([message_id], |r| {
                let attachments: String = r.get(2)?;
                Ok(Body {
                    text: r.get(0)?,
                    html: r.get(1)?,
                    attachments: serde_json::from_str::<Vec<Attachment>>(&attachments).unwrap_or_default(),
                })
            })
            .optional()?)
    }

    /// Messages whose body hasn't been fetched yet, newest first.
    pub fn missing_bodies(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM messages WHERE body_state = 0 ORDER BY received_at DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn message_count(&self) -> Result<i64> {
        Ok(self.conn.prepare_cached("SELECT count(*) FROM messages")?.query_row([], |r| r.get(0))?)
    }

    pub fn outbox(&self) -> Result<Vec<OutboxItem>> {
        let mut stmt = self.conn.prepare_cached("SELECT id, action, attempts FROM outbox ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
        rows.map(|row| {
            let (id, action, attempts) = row?;
            let action: Action = serde_json::from_str(&action)?;
            Ok(OutboxItem { id, action, attempts })
        })
        .collect()
    }
}

fn mailbox_from_row(r: &Row<'_>) -> rusqlite::Result<Mailbox> {
    Ok(Mailbox {
        id: r.get(0)?,
        name: r.get(1)?,
        parent_id: r.get(2)?,
        role: r.get(3)?,
        sort_order: r.get(4)?,
        total_emails: r.get(5)?,
        unread_emails: r.get(6)?,
        total_threads: r.get(7)?,
        unread_threads: r.get(8)?,
        is_subscribed: r.get(9)?,
    })
}

fn thread_row_from_row(r: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        thread_id: r.get(0)?,
        latest_at: r.get(1)?,
        latest_message: r.get(2)?,
        subject: r.get(3)?,
        participants: r.get(4)?,
        preview: r.get(5)?,
        message_count: r.get(6)?,
        unread_count: r.get(7)?,
        flagged: r.get(8)?,
        has_attachment: r.get(9)?,
    })
}

/// What the embedder needs about a message.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedInput {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub body: Body,
}

impl Reader {
    /// Messages with a fetched body and no vectors from `model` yet, newest first.
    pub fn messages_without_vectors(&self, model: &str, limit: usize) -> Result<Vec<EmbedInput>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, m.subject, m.from_name, m.from_email, b.text, b.html
             FROM messages m JOIN bodies b ON b.message = m.rowid
             WHERE NOT EXISTS (SELECT 1 FROM vectors v WHERE v.model = ?1 AND v.message = m.rowid)
             ORDER BY m.received_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![model, limit as i64], |r| {
            let (name, email): (String, String) = (r.get(2)?, r.get(3)?);
            Ok(EmbedInput {
                id: r.get(0)?,
                subject: r.get(1)?,
                from: if name.is_empty() { email } else { name },
                body: Body { text: r.get(4)?, html: r.get(5)?, attachments: Vec::new() },
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every vector `model` made, with its message rowid, in rowid order.
    pub fn vectors(&self, model: &str, mut each: impl FnMut(i64, &[u8])) -> Result<()> {
        let mut stmt = self.conn.prepare_cached("SELECT message, vec FROM vectors WHERE model = ?1 ORDER BY message, chunk")?;
        let mut rows = stmt.query([model])?;
        while let Some(row) = rows.next()? {
            let blob = row.get_ref(1)?.as_blob().map_err(rusqlite::Error::from)?;
            each(row.get(0)?, blob);
        }
        Ok(())
    }

    /// Messages `model` has vectors for, and messages with a body to embed.
    pub fn vector_progress(&self, model: &str) -> Result<(i64, i64)> {
        Ok(self.conn.prepare_cached(
            "SELECT (SELECT count(DISTINCT message) FROM vectors WHERE model = ?1), (SELECT count(*) FROM bodies)",
        )?.query_row([model], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }
}
