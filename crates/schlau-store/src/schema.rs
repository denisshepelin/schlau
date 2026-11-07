use rusqlite::Connection;

use crate::Result;

/// Connection settings every connection gets: WAL so readers never block the writer, memory-mapped
/// reads, and a statement cache big enough for every query the app prepares.
pub(crate) fn configure(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA mmap_size = 1073741824;
         PRAGMA cache_size = -65536;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    conn.set_prepared_statement_cache_capacity(256);
    Ok(())
}

const MIGRATIONS: &[&str] = &[V1, V2];

/// Brings the database to the newest schema. Each migration runs once, in a transaction, and bumps
/// `user_version`.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

const V1: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE mailboxes (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    parent_id      TEXT,
    role           TEXT,
    sort_order     INTEGER NOT NULL DEFAULT 0,
    total_emails   INTEGER NOT NULL DEFAULT 0,
    unread_emails  INTEGER NOT NULL DEFAULT 0,
    total_threads  INTEGER NOT NULL DEFAULT 0,
    unread_threads INTEGER NOT NULL DEFAULT 0,
    is_subscribed  INTEGER NOT NULL DEFAULT 1
) WITHOUT ROWID;
CREATE INDEX mailboxes_role ON mailboxes(role) WHERE role IS NOT NULL;

CREATE TABLE messages (
    rowid          INTEGER PRIMARY KEY,
    id             TEXT NOT NULL UNIQUE,
    thread_id      TEXT NOT NULL,
    blob_id        TEXT,
    received_at    INTEGER NOT NULL,
    sent_at        INTEGER,
    size           INTEGER NOT NULL DEFAULT 0,
    subject        TEXT NOT NULL DEFAULT '',
    from_name      TEXT NOT NULL DEFAULT '',
    from_email     TEXT NOT NULL DEFAULT '',
    addresses      TEXT NOT NULL DEFAULT '{}',
    preview        TEXT NOT NULL DEFAULT '',
    has_attachment INTEGER NOT NULL DEFAULT 0,
    flags          INTEGER NOT NULL DEFAULT 0,
    keywords       TEXT NOT NULL DEFAULT '[]',
    message_id     TEXT,
    in_reply_to    TEXT,
    refs           TEXT,
    body_state     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX messages_thread ON messages(thread_id, received_at);
CREATE INDEX messages_received ON messages(received_at DESC);
CREATE INDEX messages_body_state ON messages(body_state, received_at DESC) WHERE body_state = 0;

CREATE TABLE message_mailboxes (
    mailbox_id  TEXT NOT NULL,
    message     INTEGER NOT NULL REFERENCES messages(rowid) ON DELETE CASCADE,
    PRIMARY KEY (mailbox_id, message)
) WITHOUT ROWID;
CREATE INDEX message_mailboxes_message ON message_mailboxes(message);

CREATE TABLE bodies (
    message     INTEGER PRIMARY KEY REFERENCES messages(rowid) ON DELETE CASCADE,
    text        TEXT,
    html        TEXT,
    attachments TEXT NOT NULL DEFAULT '[]'
);

-- One row per (mailbox, thread): everything a list row draws, so a mailbox page is one index
-- range scan with no joins. Rebuilt for the threads a write touches.
CREATE TABLE thread_rows (
    mailbox_id     TEXT NOT NULL,
    thread_id      TEXT NOT NULL,
    latest_at      INTEGER NOT NULL,
    latest_message INTEGER NOT NULL,
    subject        TEXT NOT NULL,
    participants   TEXT NOT NULL,
    preview        TEXT NOT NULL,
    message_count  INTEGER NOT NULL,
    unread_count   INTEGER NOT NULL,
    flagged        INTEGER NOT NULL,
    has_attachment INTEGER NOT NULL,
    PRIMARY KEY (mailbox_id, thread_id)
) WITHOUT ROWID;
CREATE INDEX thread_rows_page ON thread_rows(mailbox_id, latest_at DESC, thread_id);
CREATE INDEX thread_rows_thread ON thread_rows(thread_id);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    subject, sender, recipients, body, attachments,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);
CREATE VIRTUAL TABLE messages_vocab USING fts5vocab(messages_fts, 'row');

CREATE TABLE vectors (
    message INTEGER NOT NULL REFERENCES messages(rowid) ON DELETE CASCADE,
    chunk   INTEGER NOT NULL,
    vec     BLOB NOT NULL,
    PRIMARY KEY (message, chunk)
) WITHOUT ROWID;

CREATE TABLE outbox (
    id          INTEGER PRIMARY KEY,
    created_at  INTEGER NOT NULL,
    action      TEXT NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT
);
"#;

/// Attachments as search sees them: what was extracted from each file (text for the word index) and
/// the vectors of its images and text in the multimodal model's space.
const V2: &str = r#"
CREATE TABLE attachment_index (
    message  INTEGER NOT NULL REFERENCES messages(rowid) ON DELETE CASCADE,
    part     INTEGER NOT NULL,
    blob_id  TEXT NOT NULL,
    name     TEXT,
    mime     TEXT NOT NULL,
    size     INTEGER NOT NULL,
    -- 0 waiting, 1 extracted, 2 nothing to extract (unsupported, too large, broken)
    state    INTEGER NOT NULL DEFAULT 0,
    -- The model the vectors were made with, when there are any.
    model    TEXT,
    text     TEXT,
    error    TEXT,
    PRIMARY KEY (message, part)
) WITHOUT ROWID;
CREATE INDEX attachment_index_state ON attachment_index(state, model);

CREATE TABLE media_vectors (
    message INTEGER NOT NULL REFERENCES messages(rowid) ON DELETE CASCADE,
    part    INTEGER NOT NULL,
    chunk   INTEGER NOT NULL,
    vec     BLOB NOT NULL,
    PRIMARY KEY (message, part, chunk)
) WITHOUT ROWID;
"#;
