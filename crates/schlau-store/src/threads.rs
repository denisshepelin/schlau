use std::collections::{BTreeMap, BTreeSet, HashSet};

use rusqlite::{Transaction, params};

use crate::Result;
use crate::model::{Address, Addresses, Flags};

struct ThreadMessage {
    rowid: i64,
    received_at: i64,
    subject: String,
    sender: Option<Address>,
    preview: String,
    flags: Flags,
    has_attachment: bool,
    mailboxes: Vec<String>,
}

/// Recomputes the list rows of each thread: one row per mailbox holding any of its messages.
pub(crate) fn rebuild_thread_rows(tx: &Transaction<'_>, threads: &BTreeSet<String>) -> Result<()> {
    if threads.is_empty() {
        return Ok(());
    }
    let hidden = hidden_mailboxes(tx)?;
    let mut delete = tx.prepare_cached("DELETE FROM thread_rows WHERE thread_id = ?1")?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO thread_rows(mailbox_id, thread_id, latest_at, latest_message, subject, participants,
                                 preview, message_count, unread_count, flagged, has_attachment)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for thread_id in threads {
        delete.execute([thread_id])?;
        let messages = thread_messages(tx, thread_id)?;
        for (mailbox_id, row) in rows_for_thread(&messages, &hidden) {
            insert.execute(params![
                mailbox_id,
                thread_id,
                row.latest_at,
                row.latest_message,
                row.subject,
                row.participants,
                row.preview,
                row.message_count,
                row.unread_count,
                row.flagged,
                row.has_attachment
            ])?;
        }
    }
    Ok(())
}

/// Trash and junk: a conversation's messages there don't count toward its rows elsewhere.
fn hidden_mailboxes(tx: &Transaction<'_>) -> Result<HashSet<String>> {
    let mut stmt = tx.prepare_cached("SELECT id FROM mailboxes WHERE role IN ('trash', 'junk')")?;
    let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

fn thread_messages(tx: &Transaction<'_>, thread_id: &str) -> Result<Vec<ThreadMessage>> {
    let mut stmt = tx.prepare_cached(
        "SELECT m.rowid, m.received_at, m.subject, m.addresses, m.preview, m.flags, m.has_attachment,
                (SELECT group_concat(mailbox_id, char(31)) FROM message_mailboxes WHERE message = m.rowid)
         FROM messages m WHERE m.thread_id = ?1 ORDER BY m.received_at, m.rowid",
    )?;
    let rows = stmt.query_map([thread_id], |r| {
        let addresses: String = r.get(3)?;
        let mailboxes: Option<String> = r.get(7)?;
        Ok(ThreadMessage {
            rowid: r.get(0)?,
            received_at: r.get(1)?,
            subject: r.get(2)?,
            sender: serde_json::from_str::<Addresses>(&addresses).ok().and_then(|a| a.from.into_iter().next()),
            preview: r.get(4)?,
            flags: Flags(r.get(5)?),
            has_attachment: r.get(6)?,
            mailboxes: mailboxes.map(|m| m.split('\u{1f}').map(str::to_string).collect()).unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, PartialEq)]
pub(crate) struct Row {
    pub latest_at: i64,
    pub latest_message: i64,
    pub subject: String,
    pub participants: String,
    pub preview: String,
    pub message_count: i64,
    pub unread_count: i64,
    pub flagged: bool,
    pub has_attachment: bool,
}

fn rows_for_thread(messages: &[ThreadMessage], hidden: &HashSet<String>) -> BTreeMap<String, Row> {
    let mailboxes: BTreeSet<&String> = messages.iter().flat_map(|m| &m.mailboxes).collect();
    mailboxes
        .into_iter()
        .filter_map(|mailbox| {
            let in_mailbox = |m: &&ThreadMessage| m.mailboxes.contains(mailbox);
            let visible: Vec<&ThreadMessage> = if hidden.contains(mailbox) {
                messages.iter().filter(in_mailbox).collect()
            } else {
                messages.iter().filter(|m| !m.mailboxes.iter().any(|id| hidden.contains(id))).collect()
            };
            summarize(&visible).map(|row| (mailbox.clone(), row))
        })
        .collect()
}

fn summarize(messages: &[&ThreadMessage]) -> Option<Row> {
    let latest = messages.iter().max_by_key(|m| (m.received_at, m.rowid))?;
    let first = messages.first()?;
    Some(Row {
        latest_at: latest.received_at,
        latest_message: latest.rowid,
        subject: first.subject.clone(),
        participants: participants(messages),
        preview: latest.preview.clone(),
        message_count: messages.len() as i64,
        unread_count: messages.iter().filter(|m| !m.flags.contains(Flags::SEEN)).count() as i64,
        flagged: messages.iter().any(|m| m.flags.contains(Flags::FLAGGED)),
        has_attachment: messages.iter().any(|m| m.has_attachment),
    })
}

/// Senders in the order they first wrote, as Superhuman lists them: the full name when one person
/// wrote everything, first names otherwise, at most three.
fn participants(messages: &[&ThreadMessage]) -> String {
    let senders: Vec<&Address> = messages.iter().filter_map(|m| m.sender.as_ref()).fold(Vec::new(), |mut acc, a| {
        if !acc.iter().any(|seen: &&Address| seen.email.eq_ignore_ascii_case(&a.email)) {
            acc.push(a);
        }
        acc
    });
    match senders.as_slice() {
        [] => String::new(),
        [only] => only.display().to_string(),
        many => {
            let names: Vec<&str> =
                many.iter().map(|a| a.display().split_whitespace().next().unwrap_or(a.display())).collect();
            let shown = names.iter().rev().take(3).rev().copied().collect::<Vec<_>>().join(", ");
            if names.len() > 3 { format!("…, {shown}") } else { shown }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(rowid: i64, at: i64, name: &str, email: &str, seen: bool, mailboxes: &[&str]) -> ThreadMessage {
        ThreadMessage {
            rowid,
            received_at: at,
            subject: format!("subject {rowid}"),
            sender: Some(Address { name: name.into(), email: email.into() }),
            preview: format!("preview {rowid}"),
            flags: Flags::default().with(Flags::SEEN, seen),
            has_attachment: false,
            mailboxes: mailboxes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_thread_gets_a_row_in_each_of_its_mailboxes_with_the_latest_preview() {
        let messages = [
            msg(1, 10, "Jane Doe", "jane@x.com", true, &["inbox"]),
            msg(2, 20, "Bob Stone", "bob@x.com", false, &["sent"]),
        ];
        let rows = rows_for_thread(&messages, &HashSet::new());
        assert_eq!(rows.len(), 2);
        let inbox = &rows["inbox"];
        assert_eq!(inbox.latest_at, 20);
        assert_eq!(inbox.preview, "preview 2");
        assert_eq!(inbox.subject, "subject 1");
        assert_eq!(inbox.unread_count, 1);
        assert_eq!(inbox.participants, "Jane, Bob");
    }

    #[test]
    fn trashed_messages_count_only_in_the_trash() {
        let messages = [msg(1, 10, "A", "a@x", true, &["inbox"]), msg(2, 20, "B", "b@x", false, &["trash"])];
        let hidden = HashSet::from(["trash".to_string()]);
        let rows = rows_for_thread(&messages, &hidden);
        assert_eq!(rows["inbox"].message_count, 1);
        assert_eq!(rows["inbox"].latest_at, 10);
        assert_eq!(rows["trash"].message_count, 1);
        assert_eq!(rows["trash"].latest_message, 2);
    }

    #[test]
    fn one_sender_shows_the_full_name_and_many_keep_the_last_three() {
        let one = [msg(1, 1, "Jane Doe", "jane@x", true, &["i"]), msg(2, 2, "Jane Doe", "JANE@x", true, &["i"])];
        assert_eq!(rows_for_thread(&one, &HashSet::new())["i"].participants, "Jane Doe");
        let many = [
            msg(1, 1, "Ann A", "a@x", true, &["i"]),
            msg(2, 2, "Ben B", "b@x", true, &["i"]),
            msg(3, 3, "", "carl@x", true, &["i"]),
            msg(4, 4, "Dora D", "d@x", true, &["i"]),
        ];
        assert_eq!(rows_for_thread(&many, &HashSet::new())["i"].participants, "…, Ben, carl, Dora");
    }
}
