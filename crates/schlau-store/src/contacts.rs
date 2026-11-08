//! Contact cards and groups in the replica, and the screener's reads over them: who is in the
//! screened folder, which messages a group says belong elsewhere, and how the rules have routed.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, params};

use crate::model::{ContactGroup, ContactRecord, Flags, Misrouted, RoutingStats, ScreenerSender};
use crate::read::Reader;
use crate::write::WriteTx;
use crate::{Result, now_unix};

/// The card id a queued `AddToGroup` gives the card it makes, until the server's card arrives.
pub fn pending_card_id(uid: &str) -> String {
    format!("pending:{uid}")
}

impl WriteTx<'_> {
    /// Inserts a card or replaces it, with its emails and (for a group) its members.
    pub fn upsert_contact(&self, c: &ContactRecord) -> Result<()> {
        self.put_contact(c, false)
    }

    fn put_contact(&self, c: &ContactRecord, pending: bool) -> Result<()> {
        self.tx
            .prepare_cached(
                "INSERT INTO contact_cards(id, uid, kind, name, pending) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET uid = excluded.uid, kind = excluded.kind,
                                               name = excluded.name, pending = excluded.pending",
            )?
            .execute(params![c.id, c.uid, c.kind, c.name, pending])?;
        self.tx.prepare_cached("DELETE FROM contact_emails WHERE card = ?1")?.execute([&c.id])?;
        self.tx.prepare_cached("DELETE FROM group_members WHERE grp = ?1")?.execute([&c.id])?;
        let mut email = self.tx.prepare_cached("INSERT OR IGNORE INTO contact_emails(email, card) VALUES (?1, ?2)")?;
        c.emails.iter().try_for_each(|e| email.execute(params![e.to_lowercase(), c.id]).map(|_| ()))?;
        let mut member = self.tx.prepare_cached("INSERT OR IGNORE INTO group_members(grp, uid) VALUES (?1, ?2)")?;
        c.members.iter().try_for_each(|uid| member.execute(params![c.id, uid]).map(|_| ()))?;
        Ok(())
    }

    pub fn delete_contact(&self, id: &str) -> Result<()> {
        self.tx.prepare_cached("DELETE FROM contact_cards WHERE id = ?1")?.execute([id])?;
        Ok(())
    }

    /// Replaces every card with the server's, as a full contact sync does.
    pub fn replace_contacts(&self, cards: &[ContactRecord]) -> Result<()> {
        self.tx.execute("DELETE FROM contact_cards", [])?;
        cards.iter().try_for_each(|c| self.upsert_contact(c))
    }

    /// Drops cards queued actions made, before the server's state and the queue are applied again.
    pub fn drop_pending_contacts(&self) -> Result<()> {
        self.tx.execute("DELETE FROM contact_cards WHERE pending = 1", [])?;
        Ok(())
    }

    pub(crate) fn add_to_group(&self, group_id: &str, email: &str, name: &str, uid: &str) -> Result<()> {
        let group_known: bool = self
            .tx
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM contact_cards WHERE id = ?1 AND kind = 'group')")?
            .query_row([group_id], |r| r.get(0))?;
        if !group_known {
            return Ok(());
        }
        let email = email.to_lowercase();
        let uid = match self.uid_for_email(&email)? {
            Some(existing) => existing,
            None => {
                let card = ContactRecord {
                    id: pending_card_id(uid),
                    uid: uid.to_string(),
                    kind: "individual".into(),
                    name: name.to_string(),
                    emails: vec![email],
                    members: Vec::new(),
                };
                self.put_contact(&card, true)?;
                uid.to_string()
            }
        };
        self.tx
            .prepare_cached("INSERT OR IGNORE INTO group_members(grp, uid) VALUES (?1, ?2)")?
            .execute(params![group_id, uid])?;
        Ok(())
    }

    /// Takes the sender out of the group. A card made for the decision stays until the server has
    /// deleted it and contacts sync, so the replay can still find its id.
    pub(crate) fn remove_from_group(&self, group_id: &str, email: &str) -> Result<()> {
        self.tx
            .prepare_cached(
                "DELETE FROM group_members WHERE grp = ?1 AND uid IN
                    (SELECT c.uid FROM contact_cards c JOIN contact_emails e ON e.card = c.id WHERE e.email = ?2)",
            )?
            .execute(params![group_id, email.to_lowercase()])?;
        Ok(())
    }

    fn uid_for_email(&self, email: &str) -> Result<Option<String>> {
        Ok(self
            .tx
            .prepare_cached(
                "SELECT c.uid FROM contact_cards c JOIN contact_emails e ON e.card = c.id
                 WHERE e.email = ?1 AND c.kind != 'group' ORDER BY c.pending, c.id LIMIT 1",
            )?
            .query_row([email], |r| r.get(0))
            .optional()?)
    }

    /// Notes what happened to a new message from a group member. The first note for a message
    /// stands.
    pub fn record_screener_event(&self, message_id: &str, group_id: &str, routed: bool) -> Result<()> {
        self.tx
            .prepare_cached("INSERT OR IGNORE INTO screener_events(message, grp, routed, at) VALUES (?1, ?2, ?3, ?4)")?
            .execute(params![message_id, group_id, routed, now_unix()])?;
        Ok(())
    }
}

impl Reader {
    pub fn contact_groups(&self) -> Result<Vec<ContactGroup>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id, c.name, (SELECT count(*) FROM group_members m WHERE m.grp = c.id)
             FROM contact_cards c WHERE c.kind = 'group' ORDER BY c.name",
        )?;
        let rows = stmt.query_map([], |r| Ok(ContactGroup { id: r.get(0)?, name: r.get(1)?, members: r.get(2)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn contact_count(&self) -> Result<i64> {
        Ok(self.conn.prepare_cached("SELECT count(*) FROM contact_cards")?.query_row([], |r| r.get(0))?)
    }

    /// The card holding `email`, as `(uid, pending)`: the server's card before a pending one.
    pub fn card_for_email(&self, email: &str) -> Result<Option<(String, bool)>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT c.uid, c.pending FROM contact_cards c JOIN contact_emails e ON e.card = c.id
                 WHERE e.email = ?1 AND c.kind != 'group' ORDER BY c.pending, c.id LIMIT 1",
            )?
            .query_row([email.to_lowercase()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// The groups whose members include a card holding `email`.
    pub fn groups_of(&self, email: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT m.grp FROM group_members m
             JOIN contact_cards c ON c.uid = m.uid JOIN contact_emails e ON e.card = c.id
             WHERE e.email = ?1 ORDER BY m.grp",
        )?;
        let rows = stmt.query_map([email.to_lowercase()], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The `uid`s of `group`'s members whose cards hold `email`, for patching the group.
    pub fn member_uids(&self, group_id: &str, email: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT m.uid FROM group_members m
             JOIN contact_cards c ON c.uid = m.uid JOIN contact_emails e ON e.card = c.id
             WHERE m.grp = ?1 AND e.email = ?2 ORDER BY m.uid",
        )?;
        let rows = stmt.query_map(params![group_id, email.to_lowercase()], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The card with `uid` and how many groups it is in.
    pub fn card_by_uid(&self, uid: &str) -> Result<Option<(String, i64)>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT c.id, (SELECT count(*) FROM group_members m WHERE m.uid = c.uid)
                 FROM contact_cards c WHERE c.uid = ?1 AND c.kind != 'group' ORDER BY c.pending LIMIT 1",
            )?
            .query_row([uid], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    /// The senders of the messages in `mailbox_id`, the most recent first.
    pub fn screener_senders(&self, mailbox_id: &str) -> Result<Vec<ScreenerSender>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, lower(m.from_email), m.from_name, m.subject, m.received_at, m.flags, m.thread_id
             FROM message_mailboxes mm JOIN messages m ON m.rowid = mm.message
             WHERE mm.mailbox_id = ?1 ORDER BY m.received_at DESC, m.rowid DESC",
        )?;
        let rows = stmt.query_map([mailbox_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                Flags(r.get(5)?),
                r.get::<_, String>(6)?,
            ))
        })?;
        let senders = rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().fold(
            Vec::<ScreenerSender>::new(),
            |mut acc, (id, email, name, subject, at, flags, thread)| {
                let unread = i64::from(!flags.contains(Flags::SEEN));
                match acc.iter_mut().find(|s| s.email == email) {
                    Some(s) => {
                        s.message_ids.push(id);
                        s.unread += unread;
                        if s.name.is_empty() {
                            s.name = name;
                        }
                    }
                    None => acc.push(ScreenerSender {
                        email,
                        name,
                        message_ids: vec![id],
                        unread,
                        latest_at: at,
                        latest_subject: subject,
                        latest_thread: thread,
                        ..Default::default()
                    }),
                }
                acc
            },
        );
        senders.into_iter().map(|s| self.describe_sender(s, mailbox_id)).collect()
    }

    fn describe_sender(&self, mut s: ScreenerSender, mailbox_id: &str) -> Result<ScreenerSender> {
        s.contact = self.card_for_email(&s.email)?.is_some();
        s.replied = self
            .conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM message_mailboxes mm
                    JOIN mailboxes b ON b.id = mm.mailbox_id AND b.role = 'sent'
                    JOIN messages m ON m.rowid = mm.message
                    WHERE instr(lower(m.addresses), ?1) > 0)",
            )?
            .query_row([format!("\"email\":\"{}\"", s.email)], |r| r.get(0))?;
        let mut elsewhere = self.conn.prepare_cached(
            "SELECT mm.mailbox_id, count(*) FROM messages m JOIN message_mailboxes mm ON mm.message = m.rowid
             WHERE lower(m.from_email) = ?1 AND mm.mailbox_id != ?2
             GROUP BY mm.mailbox_id ORDER BY count(*) DESC, mm.mailbox_id",
        )?;
        s.elsewhere = elsewhere
            .query_map(params![s.email, mailbox_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        s.domain_groups = match s.email.rsplit_once('@') {
            Some((_, domain)) if !domain.is_empty() => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT m.grp, count(DISTINCT e.email) FROM contact_emails e
                     JOIN contact_cards c ON c.id = e.card JOIN group_members m ON m.uid = c.uid
                     WHERE e.email LIKE ?1 ESCAPE '\\' AND e.email != ?2
                     GROUP BY m.grp ORDER BY count(DISTINCT e.email) DESC, m.grp",
                )?;
                let pattern = format!("%@{}", domain.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
                stmt.query_map(params![pattern, s.email], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            }
            _ => Vec::new(),
        };
        Ok(s)
    }

    /// Messages in `sources` whose sender is in one of `groups` (group id, the mailbox it routes to),
    /// and that are not already in that mailbox. A sender in several of these groups goes by the
    /// first.
    pub fn misrouted(&self, sources: &[String], groups: &[(String, String)]) -> Result<Vec<Misrouted>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id, mm.mailbox_id, g.grp
             FROM message_mailboxes mm JOIN messages m ON m.rowid = mm.message
             JOIN contact_emails e ON e.email = lower(m.from_email)
             JOIN contact_cards c ON c.id = e.card
             JOIN group_members g ON g.uid = c.uid
             WHERE mm.mailbox_id = ?1",
        )?;
        let found: Vec<(String, String, String)> = sources
            .iter()
            .map(|source| {
                stmt.query_map([source], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()
            })
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        let by_message: BTreeMap<String, (String, String)> =
            found.into_iter().fold(BTreeMap::new(), |mut acc, (message, mailbox, group)| {
                let rank = |g: &str| groups.iter().position(|(id, _)| id == g);
                match (rank(&group), acc.get(&message).and_then(|(_, g)| rank(g))) {
                    (Some(new), Some(old)) if new >= old => {}
                    (Some(_), _) => {
                        acc.insert(message, (mailbox, group));
                    }
                    (None, _) => {}
                }
                acc
            });
        Ok(by_message
            .into_iter()
            .filter(|(_, (mailbox, group))| groups.iter().any(|(g, target)| g == group && target != mailbox))
            .map(|(message_id, (mailbox_id, group_id))| Misrouted { message_id, mailbox_id, group_id })
            .collect())
    }

    /// For a newly arrived message: the first of `groups` its sender is in.
    pub fn group_of_message(&self, message_id: &str, groups: &[String]) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT g.grp FROM messages m
             JOIN contact_emails e ON e.email = lower(m.from_email)
             JOIN contact_cards c ON c.id = e.card
             JOIN group_members g ON g.uid = c.uid
             WHERE m.id = ?1",
        )?;
        let found: Vec<String> = stmt.query_map([message_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(groups.iter().find(|g| found.contains(g)).cloned())
    }

    /// The mailboxes a message is in.
    pub fn message_mailboxes(&self, message_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT mm.mailbox_id FROM message_mailboxes mm JOIN messages m ON m.rowid = mm.message
             WHERE m.id = ?1 ORDER BY mm.mailbox_id",
        )?;
        let rows = stmt.query_map([message_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn routing_stats(&self, group_id: &str) -> Result<RoutingStats> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT coalesce(sum(routed), 0), coalesce(sum(1 - routed), 0), max(CASE WHEN routed = 0 THEN at END)
                 FROM screener_events WHERE grp = ?1",
            )?
            .query_row([group_id], |r| Ok(RoutingStats { routed: r.get(0)?, missed: r.get(1)?, last_missed_at: r.get(2)? }))?)
    }
}
