//! One JMAP account against the local replica: each step is small, writes one transaction and
//! reports whether more work remains, so the engine can interleave user commands between steps.

use std::collections::BTreeSet;
use std::sync::Arc;

use schlau_jmap::{
    Client, Comparator, Email, EmailGetArgs, EmailPatch, Id, Query, State, Transport, email_property as property,
};
use schlau_store::{Action, BodyIndex, Store};

use crate::convert;
use crate::index::body_index_text;

pub type DynClient = Client<Arc<dyn Transport>>;

const MAILBOX_STATE: &str = "jmap.mailbox_state";
const EMAIL_STATE: &str = "jmap.email_state";
const BACKFILL_POSITION: &str = "jmap.backfill_position";
const BACKFILL_DONE: &str = "jmap.backfill_done";
const ACCOUNT: &str = "jmap.account";

const PAGE: u64 = 500;
const BODY_BATCH: usize = 40;
const MAX_BODY_BYTES: u64 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Jmap(#[from] schlau_jmap::Error),
    #[error(transparent)]
    Store(#[from] schlau_store::Error),
}

pub type Result<T, E = SyncError> = std::result::Result<T, E>;

pub struct Remote {
    client: DynClient,
    account: Id,
}

fn meta(store: &Store, key: &str) -> Result<Option<String>> {
    Ok(store.reader()?.meta(key)?)
}

impl Remote {
    pub fn new(client: DynClient) -> Result<Remote> {
        let account = client.mail_account_id()?;
        Ok(Remote { client, account })
    }

    pub fn client(&self) -> &DynClient {
        &self.client
    }

    pub fn account(&self) -> &Id {
        &self.account
    }

    /// A store synced from another account starts over.
    pub fn claim(&self, store: &mut Store) -> Result<()> {
        let account = self.account.as_str();
        match meta(store, ACCOUNT)? {
            Some(a) if a == account => Ok(()),
            _ => {
                store.write(|tx| {
                    [MAILBOX_STATE, EMAIL_STATE, BACKFILL_POSITION, BACKFILL_DONE]
                        .iter()
                        .try_for_each(|k| tx.delete_meta(k))?;
                    tx.retain_messages(&BTreeSet::new())?;
                    tx.set_meta(ACCOUNT, account)
                })?;
                Ok(())
            }
        }
    }

    pub fn sync_mailboxes(&self, store: &mut Store) -> Result<bool> {
        let since = meta(store, MAILBOX_STATE)?;
        match since {
            None => self.full_mailboxes(store),
            Some(since) => match self.client.mailbox_changes(&State::new(since), None) {
                Ok(changes) => {
                    let local = store.reader()?.mailboxes()?;
                    store.write(|tx| {
                        changes.created.iter().try_for_each(|m| tx.upsert_mailbox(&convert::mailbox(m)))?;
                        changes.updated.iter().try_for_each(|m| {
                            match local.iter().find(|l| l.id == m.id.as_str()) {
                                Some(old) => tx.upsert_mailbox(&convert::merge_mailbox(old, m)),
                                None => tx.upsert_mailbox(&convert::mailbox(m)),
                            }
                        })?;
                        changes.changes.destroyed.iter().try_for_each(|id| tx.delete_mailbox(id.as_str()))?;
                        tx.set_meta(MAILBOX_STATE, changes.changes.new_state.as_str())
                    })?;
                    Ok(changes.changes.has_more_changes
                        || !(changes.created.is_empty() && changes.updated.is_empty() && changes.changes.destroyed.is_empty()))
                }
                Err(e) if e.is_cannot_calculate_changes() => self.full_mailboxes(store),
                Err(e) => Err(e.into()),
            },
        }
    }

    fn full_mailboxes(&self, store: &mut Store) -> Result<bool> {
        let all = self.client.mailboxes()?;
        let keep: BTreeSet<&str> = all.list.iter().map(|m| m.id.as_str()).collect();
        let local = store.reader()?.mailboxes()?;
        store.write(|tx| {
            all.list.iter().try_for_each(|m| tx.upsert_mailbox(&convert::mailbox(m)))?;
            local.iter().filter(|m| !keep.contains(m.id.as_str())).try_for_each(|m| tx.delete_mailbox(&m.id))?;
            tx.set_meta(MAILBOX_STATE, all.state.as_str())
        })?;
        Ok(true)
    }

    /// The first page of the newest mail, which also fixes the state delta sync starts from.
    /// Returns false once the replica has a state.
    pub fn start_emails(&self, store: &mut Store) -> Result<bool> {
        if meta(store, EMAIL_STATE)?.is_some() {
            return Ok(false);
        }
        let query = Query::<Email>::new(self.account.clone()).sort([Comparator::received_at_desc()]).limit(PAGE);
        let (q, got) = self.client.query_and_get_emails(query, Some(property::HEADERS), EmailGetArgs::default())?;
        let pending = store.reader()?.outbox()?;
        store.write(|tx| {
            got.list.iter().try_for_each(|e| tx.upsert_message(&convert::message(e)).map(|_| ()))?;
            pending.iter().try_for_each(|item| tx.apply_locally(&item.action))?;
            tx.set_meta(EMAIL_STATE, got.state.as_str())?;
            tx.set_meta(BACKFILL_POSITION, &(q.position + q.ids.len() as u64).to_string())?;
            if q.total.is_some_and(|t| q.position + (q.ids.len() as u64) >= t) || q.ids.is_empty() {
                tx.set_meta(BACKFILL_DONE, "1")?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// One page of older mail. Returns whether more remains.
    pub fn backfill(&self, store: &mut Store) -> Result<bool> {
        if meta(store, BACKFILL_DONE)?.is_some() {
            return Ok(false);
        }
        let position: u64 = meta(store, BACKFILL_POSITION)?.and_then(|p| p.parse().ok()).unwrap_or(0);
        let query = Query::<Email>::new(self.account.clone())
            .sort([Comparator::received_at_desc()])
            .position(position as i64)
            .limit(PAGE);
        let (q, got) = self.client.query_and_get_emails(query, Some(property::HEADERS), EmailGetArgs::default())?;
        let pending = store.reader()?.outbox()?;
        let done = q.ids.is_empty() || q.total.is_some_and(|t| position + q.ids.len() as u64 >= t);
        store.write(|tx| {
            got.list.iter().try_for_each(|e| tx.upsert_message(&convert::message(e)).map(|_| ()))?;
            pending.iter().try_for_each(|item| tx.apply_locally(&item.action))?;
            tx.set_meta(BACKFILL_POSITION, &(position + q.ids.len() as u64).to_string())?;
            if done {
                tx.set_meta(BACKFILL_DONE, "1")?;
            }
            Ok(())
        })?;
        Ok(!done)
    }

    /// Applies server changes since the last state. Pending local actions are applied again on
    /// top, so a change the server hasn't seen yet doesn't flicker back. Returns whether anything
    /// changed.
    pub fn sync_emails(&self, store: &mut Store) -> Result<bool> {
        let Some(mut since) = meta(store, EMAIL_STATE)? else { return Ok(false) };
        let mut changed = false;
        loop {
            let changes = match self.client.email_changes_and_get(
                &State::new(since.clone()),
                None,
                property::HEADERS,
                EmailGetArgs::default(),
            ) {
                Ok(c) => c,
                Err(e) if e.is_cannot_calculate_changes() => return self.resync(store),
                Err(e) => return Err(e.into()),
            };
            let pending = store.reader()?.outbox()?;
            let c = &changes.changes;
            changed |= !(c.created.is_empty() && c.updated.is_empty() && c.destroyed.is_empty());
            store.write(|tx| {
                changes.created.iter().try_for_each(|e| tx.upsert_message(&convert::message(e)).map(|_| ()))?;
                changes.updated.iter().try_for_each(|e| tx.update_message(&convert::update(e)).map(|_| ()))?;
                c.destroyed.iter().try_for_each(|id| tx.delete_message(id.as_str()).map(|_| ()))?;
                pending.iter().try_for_each(|item| tx.apply_locally(&item.action))?;
                tx.set_meta(EMAIL_STATE, c.new_state.as_str())
            })?;
            since = c.new_state.as_str().to_string();
            if !c.has_more_changes {
                return Ok(changed);
            }
        }
    }

    /// The server can't say what changed: forget the state and sync from the newest mail again,
    /// keeping fetched bodies of messages that still exist.
    fn resync(&self, store: &mut Store) -> Result<bool> {
        store.write(|tx| {
            [EMAIL_STATE, BACKFILL_POSITION, BACKFILL_DONE].iter().try_for_each(|k| tx.delete_meta(k))
        })?;
        self.start_emails(store)
    }

    /// Fetches the bodies of `ids`, or the newest messages without one when `ids` is empty.
    /// Returns the ids it stored.
    pub fn fetch_bodies(&self, store: &mut Store, ids: &[String]) -> Result<Vec<String>> {
        let ids: Vec<String> =
            if ids.is_empty() { store.reader()?.missing_bodies(BODY_BATCH)? } else { ids.to_vec() };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted: Vec<Id> = ids.iter().map(Id::new).collect();
        let props = ["id", "bodyValues", "textBody", "htmlBody", "attachments"];
        let args = EmailGetArgs {
            body_properties: Some(property::to_vec(property::BODY_PART)),
            fetch_text_body_values: Some(true),
            fetch_html_body_values: Some(true),
            max_body_value_bytes: Some(MAX_BODY_BYTES),
            ..Default::default()
        };
        let got = self.client.get_emails(&wanted, Some(&props), args)?;
        let stored: Vec<String> = got.list.iter().map(|e| e.id.as_str().to_string()).collect();
        store.write(|tx| {
            got.list.iter().try_for_each(|e| {
                let body = convert::body(e);
                let index = BodyIndex { text: body_index_text(&body) };
                tx.set_body(e.id.as_str(), &body, &index).map(|_| ())
            })?;
            got.not_found.iter().try_for_each(|id| tx.delete_message(id.as_str()).map(|_| ()))
        })?;
        Ok(stored)
    }

    /// Sends queued actions in order. An action the server rejects for good is dropped (the next
    /// sync restores the server's truth); a transport failure stops the replay to retry later.
    pub fn replay_outbox(&self, store: &mut Store) -> Result<usize> {
        let items = store.reader()?.outbox()?;
        let mut sent = 0;
        for item in items {
            match self.send_action(&item.action) {
                Ok(()) => store.write(|tx| tx.outbox_done(item.id))?,
                Err(SyncError::Jmap(e)) if e.is_retryable() => {
                    store.write(|tx| tx.outbox_failed(item.id, &e.to_string()))?;
                    return Err(SyncError::Jmap(e));
                }
                Err(e) => store.write(|tx| {
                    tx.outbox_failed(item.id, &e.to_string())?;
                    tx.outbox_done(item.id)
                })?,
            }
            sent += 1;
        }
        Ok(sent)
    }

    fn send_action(&self, action: &Action) -> Result<()> {
        let ids = |message_ids: &[String]| message_ids.iter().map(Id::new).collect::<Vec<_>>();
        match action {
            Action::SetFlag { message_ids, flag, on } => {
                self.client.update_emails(&ids(message_ids), EmailPatch::new().keyword(flag, *on))?;
            }
            Action::Move { message_ids, from: Some(from), to } => {
                self.client.move_emails(&ids(message_ids), &Id::new(from), &Id::new(to))?;
            }
            Action::Move { message_ids, from: None, to } => {
                let to = Id::new(to);
                self.client.update_emails(&ids(message_ids), EmailPatch::new().set_mailboxes([&to]))?;
            }
            Action::AddMailbox { message_ids, mailbox_id } => {
                self.client.add_to_mailbox(&ids(message_ids), &Id::new(mailbox_id))?;
            }
            Action::RemoveMailbox { message_ids, mailbox_id } => {
                self.client.remove_from_mailbox(&ids(message_ids), &Id::new(mailbox_id))?;
            }
            Action::Destroy { message_ids } => {
                self.client.destroy_emails(&ids(message_ids))?;
            }
        }
        Ok(())
    }
}
