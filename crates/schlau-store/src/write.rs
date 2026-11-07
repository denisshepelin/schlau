use std::cell::RefCell;
use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction, params};

use crate::model::{Action, Addresses, Body, BodyIndex, Flags, Mailbox, MessageRecord, MessageUpdate};
use schlau_search::normalize;

use crate::threads::rebuild_thread_rows;
use crate::{Result, now_unix};

/// A write transaction. Every change notes the threads it touches; their list rows are rebuilt once,
/// when the transaction commits.
pub struct WriteTx<'c> {
    pub(crate) tx: Transaction<'c>,
    pub(crate) touched: RefCell<BTreeSet<String>>,
}

impl WriteTx<'_> {
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.tx
            .prepare_cached("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value")?
            .execute(params![key, value])?;
        Ok(())
    }

    pub fn delete_meta(&self, key: &str) -> Result<()> {
        self.tx.prepare_cached("DELETE FROM meta WHERE key = ?1")?.execute([key])?;
        Ok(())
    }

    pub fn upsert_mailbox(&self, m: &Mailbox) -> Result<()> {
        self.tx
            .prepare_cached(
                "INSERT INTO mailboxes(id, name, parent_id, role, sort_order, total_emails, unread_emails,
                                       total_threads, unread_threads, is_subscribed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name, parent_id = excluded.parent_id, role = excluded.role,
                    sort_order = excluded.sort_order, total_emails = excluded.total_emails,
                    unread_emails = excluded.unread_emails, total_threads = excluded.total_threads,
                    unread_threads = excluded.unread_threads, is_subscribed = excluded.is_subscribed",
            )?
            .execute(params![
                m.id,
                m.name,
                m.parent_id,
                m.role,
                m.sort_order,
                m.total_emails,
                m.unread_emails,
                m.total_threads,
                m.unread_threads,
                m.is_subscribed
            ])?;
        Ok(())
    }

    pub fn delete_mailbox(&self, id: &str) -> Result<()> {
        self.tx.prepare_cached("DELETE FROM mailboxes WHERE id = ?1")?.execute([id])?;
        self.tx.prepare_cached("DELETE FROM thread_rows WHERE mailbox_id = ?1")?.execute([id])?;
        self.tx.prepare_cached("DELETE FROM message_mailboxes WHERE mailbox_id = ?1")?.execute([id])?;
        Ok(())
    }

    /// Inserts a message or replaces its headers, keywords and mailboxes. A body fetched earlier stays.
    pub fn upsert_message(&self, m: &MessageRecord) -> Result<i64> {
        let addresses = serde_json::to_string(&m.addresses)?;
        let keywords = serde_json::to_string(&m.keywords)?;
        let refs = (!m.references.is_empty()).then(|| serde_json::to_string(&m.references)).transpose()?;
        let from = m.addresses.from.first();
        let rowid: i64 = self
            .tx
            .prepare_cached(
                "INSERT INTO messages(id, thread_id, blob_id, received_at, sent_at, size, subject, from_name,
                                      from_email, addresses, preview, has_attachment, flags, keywords,
                                      message_id, in_reply_to, refs)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT(id) DO UPDATE SET
                    thread_id = excluded.thread_id, blob_id = excluded.blob_id,
                    received_at = excluded.received_at, sent_at = excluded.sent_at, size = excluded.size,
                    subject = excluded.subject, from_name = excluded.from_name,
                    from_email = excluded.from_email, addresses = excluded.addresses,
                    preview = excluded.preview, has_attachment = excluded.has_attachment,
                    flags = excluded.flags, keywords = excluded.keywords,
                    message_id = excluded.message_id, in_reply_to = excluded.in_reply_to,
                    refs = excluded.refs
                 RETURNING rowid",
            )?
            .query_row(
                params![
                    m.id,
                    m.thread_id,
                    m.blob_id,
                    m.received_at,
                    m.sent_at,
                    m.size,
                    m.subject,
                    from.map(|a| a.name.as_str()).unwrap_or(""),
                    from.map(|a| a.email.as_str()).unwrap_or(""),
                    addresses,
                    m.preview,
                    m.has_attachment,
                    m.flags.0,
                    keywords,
                    m.message_id,
                    m.in_reply_to,
                    refs,
                ],
                |r| r.get(0),
            )?;
        self.replace_mailboxes(rowid, &m.mailbox_ids)?;
        self.index_headers(rowid, &m.subject, &m.addresses)?;
        self.touch_thread(&m.thread_id);
        Ok(rowid)
    }

    /// Applies a keyword or mailbox change. Returns false when the message isn't stored.
    pub fn update_message(&self, u: &MessageUpdate) -> Result<bool> {
        let Some((rowid, thread_id)) = self.message_key(&u.id)? else { return Ok(false) };
        if let Some(flags) = u.flags {
            self.tx.prepare_cached("UPDATE messages SET flags = ?2 WHERE rowid = ?1")?.execute(params![rowid, flags.0])?;
        }
        if let Some(keywords) = &u.keywords {
            self.tx
                .prepare_cached("UPDATE messages SET keywords = ?2 WHERE rowid = ?1")?
                .execute(params![rowid, serde_json::to_string(keywords)?])?;
        }
        if let Some(mailbox_ids) = &u.mailbox_ids {
            self.replace_mailboxes(rowid, mailbox_ids)?;
        }
        self.touch_thread(&thread_id);
        Ok(true)
    }

    pub fn delete_message(&self, id: &str) -> Result<bool> {
        let Some((rowid, thread_id)) = self.message_key(id)? else { return Ok(false) };
        self.tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?.execute([rowid])?;
        self.tx.prepare_cached("DELETE FROM messages WHERE rowid = ?1")?.execute([rowid])?;
        self.touch_thread(&thread_id);
        Ok(true)
    }

    /// Stores a fetched body and re-indexes the message with it.
    pub fn set_body(&self, id: &str, body: &Body, index: &BodyIndex) -> Result<bool> {
        let Some((rowid, _)) = self.message_key(id)? else { return Ok(false) };
        self.tx
            .prepare_cached(
                "INSERT INTO bodies(message, text, html, attachments) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(message) DO UPDATE SET text = excluded.text, html = excluded.html,
                                                    attachments = excluded.attachments",
            )?
            .execute(params![rowid, body.text, body.html, serde_json::to_string(&body.attachments)?])?;
        self.tx.prepare_cached("UPDATE messages SET body_state = 1 WHERE rowid = ?1")?.execute([rowid])?;
        let (subject, addresses): (String, String) = self
            .tx
            .prepare_cached("SELECT subject, addresses FROM messages WHERE rowid = ?1")?
            .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let addresses: Addresses = serde_json::from_str(&addresses)?;
        self.register_attachments(rowid, body)?;
        let attachment_words = self.attachment_words(rowid, &body.attachments)?;
        self.write_fts(rowid, &subject, &addresses, &index.text, &attachment_words)?;
        Ok(true)
    }

    /// Stores a message's vectors from `model`, replacing that model's earlier ones.
    pub fn put_vectors(&self, model: &str, message_id: &str, chunks: &[Vec<u8>]) -> Result<bool> {
        let Some((rowid, _)) = self.message_key(message_id)? else { return Ok(false) };
        self.tx.prepare_cached("DELETE FROM vectors WHERE model = ?1 AND message = ?2")?.execute(params![model, rowid])?;
        let mut insert = self.tx.prepare_cached("INSERT INTO vectors(model, message, chunk, vec) VALUES (?1, ?2, ?3, ?4)")?;
        chunks.iter().enumerate().try_for_each(|(chunk, vec)| insert.execute(params![model, rowid, chunk as i64, vec]).map(|_| ()))?;
        Ok(true)
    }

    /// Drops what `model` indexed, to free the space of a model no longer used.
    pub fn forget_model(&self, model: &str) -> Result<()> {
        self.tx.execute("DELETE FROM vectors WHERE model = ?1", [model])?;
        self.tx.execute("DELETE FROM media_vectors WHERE model = ?1", [model])?;
        self.tx.execute("DELETE FROM attachment_embedded WHERE model = ?1", [model])?;
        Ok(())
    }

    /// Queues a change for the server. Apply it locally in the same transaction.
    pub fn enqueue(&self, action: &Action) -> Result<i64> {
        Ok(self
            .tx
            .prepare_cached("INSERT INTO outbox(created_at, action) VALUES (?1, ?2) RETURNING id")?
            .query_row(params![now_unix(), serde_json::to_string(action)?], |r| r.get(0))?)
    }

    pub fn outbox_done(&self, id: i64) -> Result<()> {
        self.tx.prepare_cached("DELETE FROM outbox WHERE id = ?1")?.execute([id])?;
        Ok(())
    }

    pub fn outbox_failed(&self, id: i64, error: &str) -> Result<()> {
        self.tx
            .prepare_cached("UPDATE outbox SET attempts = attempts + 1, last_error = ?2 WHERE id = ?1")?
            .execute(params![id, error])?;
        Ok(())
    }

    /// Removes every message (and its thread rows) that isn't in `keep`. For a full resync.
    pub fn retain_messages(&self, keep: &BTreeSet<String>) -> Result<usize> {
        let stale: Vec<String> = self
            .tx
            .prepare("SELECT id FROM messages")?
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter(|id| !keep.contains(id))
            .collect();
        stale.iter().try_for_each(|id| self.delete_message(id).map(|_| ()))?;
        Ok(stale.len())
    }

    pub(crate) fn finish(self) -> Result<()> {
        let touched = self.touched.into_inner();
        rebuild_thread_rows(&self.tx, &touched)?;
        self.tx.commit()?;
        Ok(())
    }

    fn touch_thread(&self, thread_id: &str) {
        self.touched.borrow_mut().insert(thread_id.to_string());
    }

    fn message_key(&self, id: &str) -> Result<Option<(i64, String)>> {
        Ok(self
            .tx
            .prepare_cached("SELECT rowid, thread_id FROM messages WHERE id = ?1")?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    fn replace_mailboxes(&self, rowid: i64, mailbox_ids: &[String]) -> Result<()> {
        self.tx.prepare_cached("DELETE FROM message_mailboxes WHERE message = ?1")?.execute([rowid])?;
        let mut insert =
            self.tx.prepare_cached("INSERT OR IGNORE INTO message_mailboxes(mailbox_id, message) VALUES (?1, ?2)")?;
        for mailbox_id in mailbox_ids {
            insert.execute(params![mailbox_id, rowid])?;
        }
        Ok(())
    }

    /// Indexes subject and addresses as soon as headers arrive, keeping a body indexed earlier.
    fn index_headers(&self, rowid: i64, subject: &str, addresses: &Addresses) -> Result<()> {
        let body: Option<(Option<String>, String)> = self
            .tx
            .prepare_cached("SELECT text, attachments FROM bodies WHERE message = ?1")?
            .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match body {
            Some(_) => Ok(()),
            None => self.write_fts(rowid, subject, addresses, "", ""),
        }
    }

    pub(crate) fn write_fts(&self, rowid: i64, subject: &str, a: &Addresses, body: &str, attachments: &str) -> Result<()> {
        let sender = address_text(a.from.iter().chain(&a.sender).chain(&a.reply_to));
        let recipients = address_text(a.to.iter().chain(&a.cc).chain(&a.bcc));
        self.tx.prepare_cached("DELETE FROM messages_fts WHERE rowid = ?1")?.execute([rowid])?;
        self.tx
            .prepare_cached(
                "INSERT INTO messages_fts(rowid, subject, sender, recipients, body, attachments)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(params![
                rowid,
                normalize(subject),
                normalize(&sender),
                normalize(&recipients),
                normalize(body),
                normalize(attachments)
            ])?;
        Ok(())
    }
}

/// Names and addresses as words: "Jane Doe jane.doe@example.com" also yields "jane", "doe",
/// "example" from the address, since unicode61 splits on punctuation.
fn address_text<'a>(addresses: impl Iterator<Item = &'a crate::model::Address>) -> String {
    addresses.map(|a| format!("{} {}", a.name, a.email)).collect::<Vec<_>>().join(" ")
}

/// Flags for a JMAP keyword name, for applying `SetFlag` actions locally.
pub fn flag_for_keyword(keyword: &str) -> Option<Flags> {
    let (flags, _) = Flags::from_keywords([keyword]);
    (flags.0 != 0).then_some(flags)
}
