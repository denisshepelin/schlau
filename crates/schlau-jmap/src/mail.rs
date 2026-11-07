//! High-level mail operations for the sync engine, built on [`Client::send`].

use std::collections::HashMap;

use crate::client::Client;
use crate::email::{Email, EmailCreate, EmailGetArgs, keyword};
use crate::error::{Error, Result};
use crate::identity::Identity;
use crate::mailbox::Mailbox;
use crate::method::{
    Changes, ChangesResponse, Get, GetResponse, JmapObject, PropertiesArg, Query, QueryChanges,
    QueryChangesResponse, QueryResponse, Set, SetResponse,
};
use crate::patch::EmailPatch;
use crate::request::RequestBuilder;
use crate::submission::{EmailSubmission, EmailSubmissionCreate, EmailSubmissionSetArgs, Envelope};
use crate::thread::Thread;
use crate::transport::Transport;
use crate::types::{Id, IdsArg, SetError, State};

/// A `/changes` result together with the created and updated objects, from one round trip.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectChanges<O> {
    pub changes: ChangesResponse,
    pub created: Vec<O>,
    /// For `Mailbox` with `updatedProperties` set, these hold only those properties.
    pub updated: Vec<O>,
}

/// Mailboxes involved in sending: the draft leaves `drafts` and lands in `sent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendMailboxes {
    pub drafts: Id,
    pub sent: Id,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Submitted {
    pub submission_id: Id,
    pub email_id: Id,
    pub submission: SetResponse<EmailSubmission>,
    /// The implicit `Email/set` produced by `onSuccessUpdateEmail`, if the server sent it.
    pub email_update: Option<SetResponse<Email>>,
}

fn set_failure(creation_id: &str, error: Option<&SetError>) -> Error {
    Error::Invalid {
        what: "set",
        detail: match error {
            Some(e) => format!(
                "{creation_id} not created: {} {}",
                e.kind,
                e.description.as_deref().unwrap_or("")
            ),
            None => format!("{creation_id} missing from response"),
        },
    }
}

fn created_id<O: HasId>(resp: &SetResponse<O>, creation_id: &str) -> Result<Id> {
    resp.created
        .get(creation_id)
        .map(|o| o.id().clone())
        .ok_or_else(|| set_failure(creation_id, resp.not_created.get(creation_id)))
}

trait HasId {
    fn id(&self) -> &Id;
}

impl HasId for Email {
    fn id(&self) -> &Id {
        &self.id
    }
}

impl HasId for EmailSubmission {
    fn id(&self) -> &Id {
        &self.id
    }
}

fn props(p: Option<&[&str]>) -> PropertiesArg {
    PropertiesArg::List(p.map(|p| p.iter().map(|s| (*s).to_owned()).collect()))
}

fn merge_get<O>(acc: Option<GetResponse<O>>, next: GetResponse<O>) -> GetResponse<O> {
    match acc {
        None => next,
        Some(mut acc) => {
            acc.list.extend(next.list);
            acc.not_found.extend(next.not_found);
            acc
        }
    }
}

impl<T: Transport> Client<T> {
    fn get_limit(&self) -> usize {
        self.session().core().max_objects_in_get.max(1) as usize
    }

    fn set_limit(&self) -> usize {
        self.session().core().max_objects_in_set.max(1) as usize
    }

    fn calls_limit(&self) -> usize {
        self.session().core().max_calls_in_request.max(1) as usize
    }

    fn clamp_to_get_limit(&self, requested: Option<u64>) -> u64 {
        let limit = self.get_limit() as u64;
        requested.map_or(limit, |r| r.min(limit))
    }

    /// `/get` for explicit ids, split into calls of at most `maxObjectsInGet` ids and requests
    /// of at most `maxCallsInRequest` calls. `state` is the first chunk's state, which is the
    /// safe one to resume `/changes` from if chunks straddle a change.
    pub fn get_objects<O: JmapObject>(
        &self,
        account_id: &Id,
        ids: &[Id],
        properties: Option<&[&str]>,
        args: O::GetArgs,
    ) -> Result<GetResponse<O>> {
        if ids.is_empty() {
            return self.single_get::<O>(
                account_id,
                IdsArg::list(Vec::<Id>::new()),
                properties,
                args,
            );
        }
        let chunks: Vec<&[Id]> = ids.chunks(self.get_limit()).collect();
        chunks
            .chunks(self.calls_limit())
            .try_fold(None, |acc, batch| {
                let mut req = RequestBuilder::new();
                let calls: Vec<_> = batch
                    .iter()
                    .map(|chunk| {
                        req.call(
                            Get::<O>::new(account_id, IdsArg::list(chunk.iter().cloned()))
                                .properties(props(properties))
                                .args(args.clone()),
                        )
                    })
                    .collect();
                let resp = self.send(req)?;
                calls
                    .iter()
                    .try_fold(acc, |acc, call| Ok(Some(merge_get(acc, resp.get(call)?))))
            })
            .map(|r| r.expect("ids is non-empty"))
    }

    /// `/get` with `ids: null`: every object of the type. Use only for small sets.
    pub fn get_all_objects<O: JmapObject>(
        &self,
        account_id: &Id,
        properties: Option<&[&str]>,
        args: O::GetArgs,
    ) -> Result<GetResponse<O>> {
        self.single_get::<O>(account_id, IdsArg::all(), properties, args)
    }

    fn single_get<O: JmapObject>(
        &self,
        account_id: &Id,
        ids: IdsArg,
        properties: Option<&[&str]>,
        args: O::GetArgs,
    ) -> Result<GetResponse<O>> {
        let mut req = RequestBuilder::new();
        let call = req.call(
            Get::<O>::new(account_id, ids)
                .properties(props(properties))
                .args(args),
        );
        self.send(req)?.get(&call)
    }

    pub fn changes<O: JmapObject>(
        &self,
        account_id: &Id,
        since: &State,
        max_changes: Option<u64>,
    ) -> Result<ChangesResponse> {
        let mut req = RequestBuilder::new();
        let call = req.call(Changes::<O>::new(account_id, since.clone(), max_changes));
        self.send(req)?.get(&call)
    }

    /// `/changes` then `/get` of created and updated ids in one request. `max_changes` is
    /// clamped to `maxObjectsInGet` so the back-referenced gets cannot exceed it.
    pub fn changes_and_get<O: JmapObject>(
        &self,
        account_id: &Id,
        since: &State,
        max_changes: Option<u64>,
        created_properties: Option<&[&str]>,
        updated_properties: Option<&[&str]>,
        args: O::GetArgs,
    ) -> Result<ObjectChanges<O>> {
        self.changes_and_get_with(
            account_id,
            since,
            max_changes,
            props(created_properties),
            |_| props(updated_properties),
            args,
        )
    }

    fn changes_and_get_with<O: JmapObject>(
        &self,
        account_id: &Id,
        since: &State,
        max_changes: Option<u64>,
        created_properties: PropertiesArg,
        updated_properties: impl FnOnce(&crate::request::Call<ChangesResponse>) -> PropertiesArg,
        args: O::GetArgs,
    ) -> Result<ObjectChanges<O>> {
        let max = self.clamp_to_get_limit(max_changes);
        let mut req = RequestBuilder::new();
        let changes = req.call(Changes::<O>::new(account_id, since.clone(), Some(max)));
        let created = req.call(
            Get::<O>::new(account_id, changes.ids_at("/created"))
                .properties(created_properties)
                .args(args.clone()),
        );
        let updated = req.call(
            Get::<O>::new(account_id, changes.ids_at("/updated"))
                .properties(updated_properties(&changes))
                .args(args),
        );
        let resp = self.send(req)?;
        let changes = resp.get(&changes)?;
        Ok(ObjectChanges {
            changes,
            created: resp.get(&created)?.list,
            updated: resp.get(&updated)?.list,
        })
    }

    pub fn mailboxes(&self) -> Result<GetResponse<Mailbox>> {
        self.get_all_objects::<Mailbox>(&self.mail_account_id()?, None, Default::default())
    }

    /// Mailbox changes in one round trip. Updated mailboxes are fetched with
    /// `#properties` pointing at `updatedProperties`, so count-only changes return only counts.
    pub fn mailbox_changes(
        &self,
        since: &State,
        max_changes: Option<u64>,
    ) -> Result<ObjectChanges<Mailbox>> {
        self.changes_and_get_with::<Mailbox>(
            &self.mail_account_id()?,
            since,
            max_changes,
            PropertiesArg::List(None),
            |changes| changes.properties_at("/updatedProperties"),
            Default::default(),
        )
    }

    pub fn query_emails(&self, query: Query<Email>) -> Result<QueryResponse> {
        let mut req = RequestBuilder::new();
        let call = req.call(query);
        self.send(req)?.get(&call)
    }

    /// `Email/query` then `Email/get` on its ids in one round trip. The query limit is clamped
    /// to `maxObjectsInGet`.
    pub fn query_and_get_emails(
        &self,
        query: Query<Email>,
        properties: Option<&[&str]>,
        args: EmailGetArgs,
    ) -> Result<(QueryResponse, GetResponse<Email>)> {
        let account_id = query.account_id.clone();
        let limit = self.clamp_to_get_limit(query.limit);
        let mut req = RequestBuilder::new();
        let q = req.call(query.limit(limit));
        let g = req.call(
            Get::<Email>::new(account_id, q.ids_at("/ids"))
                .properties(props(properties))
                .args(args),
        );
        let resp = self.send(req)?;
        Ok((resp.get(&q)?, resp.get(&g)?))
    }

    pub fn email_query_changes(
        &self,
        query_changes: QueryChanges<Email>,
    ) -> Result<QueryChangesResponse> {
        let mut req = RequestBuilder::new();
        let call = req.call(query_changes);
        self.send(req)?.get(&call)
    }

    pub fn get_emails(
        &self,
        ids: &[Id],
        properties: Option<&[&str]>,
        args: EmailGetArgs,
    ) -> Result<GetResponse<Email>> {
        self.get_objects::<Email>(&self.mail_account_id()?, ids, properties, args)
    }

    pub fn email_changes(
        &self,
        since: &State,
        max_changes: Option<u64>,
    ) -> Result<ChangesResponse> {
        self.changes::<Email>(&self.mail_account_id()?, since, max_changes)
    }

    /// Email changes plus new emails with `created_properties` and the mutable properties
    /// (`id`, `mailboxIds`, `keywords`) of updated ones, in one round trip.
    pub fn email_changes_and_get(
        &self,
        since: &State,
        max_changes: Option<u64>,
        created_properties: &[&str],
        args: EmailGetArgs,
    ) -> Result<ObjectChanges<Email>> {
        self.changes_and_get::<Email>(
            &self.mail_account_id()?,
            since,
            max_changes,
            Some(created_properties),
            Some(crate::email::property::MUTABLE),
            args,
        )
    }

    pub fn threads(&self, ids: &[Id]) -> Result<GetResponse<Thread>> {
        self.get_objects::<Thread>(&self.mail_account_id()?, ids, None, Default::default())
    }

    pub fn thread_changes(
        &self,
        since: &State,
        max_changes: Option<u64>,
    ) -> Result<ChangesResponse> {
        self.changes::<Thread>(&self.mail_account_id()?, since, max_changes)
    }

    pub fn set_emails(&self, set: Set<Email>) -> Result<SetResponse<Email>> {
        let mut req = RequestBuilder::new();
        let call = req.call(set);
        self.send(req)?.get(&call)
    }

    /// Apply one patch to many emails, chunked by `maxObjectsInSet`.
    pub fn update_emails(&self, ids: &[Id], patch: EmailPatch) -> Result<SetResponse<Email>> {
        let account_id = self.mail_account_id()?;
        let patch = patch.into_patch();
        self.chunked_set(ids, |chunk| {
            Set::<Email>::new(&account_id).update_all(chunk.iter().cloned(), &patch)
        })
    }

    fn chunked_set(
        &self,
        ids: &[Id],
        make: impl Fn(&[Id]) -> Set<Email>,
    ) -> Result<SetResponse<Email>> {
        if ids.is_empty() {
            return Err(Error::Invalid {
                what: "ids",
                detail: "no email ids given".into(),
            });
        }
        ids.chunks(self.set_limit())
            .try_fold(None::<SetResponse<Email>>, |acc, chunk| {
                let resp = self.set_emails(make(chunk))?;
                Ok(Some(match acc {
                    None => resp,
                    Some(acc) => acc.merge(resp),
                }))
            })
            .map(|r| r.expect("ids is non-empty"))
    }

    pub fn mark_read(&self, ids: &[Id], read: bool) -> Result<SetResponse<Email>> {
        self.update_emails(ids, EmailPatch::new().keyword(keyword::SEEN, read))
    }

    pub fn flag(&self, ids: &[Id], flagged: bool) -> Result<SetResponse<Email>> {
        self.update_emails(ids, EmailPatch::new().keyword(keyword::FLAGGED, flagged))
    }

    pub fn move_emails(&self, ids: &[Id], from: &Id, to: &Id) -> Result<SetResponse<Email>> {
        self.update_emails(ids, EmailPatch::new().move_mailbox(from, to))
    }

    pub fn add_to_mailbox(&self, ids: &[Id], mailbox_id: &Id) -> Result<SetResponse<Email>> {
        self.update_emails(ids, EmailPatch::new().add_mailbox(mailbox_id))
    }

    pub fn remove_from_mailbox(&self, ids: &[Id], mailbox_id: &Id) -> Result<SetResponse<Email>> {
        self.update_emails(ids, EmailPatch::new().remove_mailbox(mailbox_id))
    }

    /// Permanently delete. To move to Trash instead, use [`Client::move_emails`].
    pub fn destroy_emails(&self, ids: &[Id]) -> Result<SetResponse<Email>> {
        let account_id = self.mail_account_id()?;
        self.chunked_set(ids, |chunk| {
            Set::<Email>::new(&account_id).destroy(chunk.iter().cloned())
        })
    }

    /// Create a draft and return its new id.
    pub fn create_draft(&self, draft: EmailCreate) -> Result<Id> {
        let resp =
            self.set_emails(Set::<Email>::new(self.mail_account_id()?).create("draft", draft))?;
        created_id(&resp, "draft")
    }

    pub fn identities(&self) -> Result<Vec<Identity>> {
        self.get_all_objects::<Identity>(&self.submission_account_id()?, None, Default::default())
            .map(|r| r.list)
    }

    fn submission_set(
        &self,
        email_id: String,
        identity_id: &Id,
        envelope: Option<Envelope>,
        mailboxes: &SendMailboxes,
    ) -> Result<Set<EmailSubmission>> {
        let on_success = EmailPatch::new()
            .move_mailbox(&mailboxes.drafts, &mailboxes.sent)
            .keyword(keyword::DRAFT, false)
            .into_patch();
        Ok(Set::<EmailSubmission>::new(self.submission_account_id()?)
            .create(
                "send",
                EmailSubmissionCreate {
                    identity_id: identity_id.clone(),
                    email_id,
                    envelope,
                },
            )
            .args(EmailSubmissionSetArgs {
                on_success_update_email: HashMap::from([("#send".to_owned(), on_success)]),
                on_success_destroy_email: Vec::new(),
            }))
    }

    /// Send an existing draft; on success the server moves it from Drafts to Sent and clears
    /// `$draft`.
    pub fn send_draft(
        &self,
        email_id: &Id,
        identity_id: &Id,
        mailboxes: &SendMailboxes,
        envelope: Option<Envelope>,
    ) -> Result<Submitted> {
        let mut req = RequestBuilder::new();
        let sub = req.call(self.submission_set(
            email_id.to_string(),
            identity_id,
            envelope,
            mailboxes,
        )?);
        let resp = self.send(req)?;
        let submission = resp.get(&sub)?;
        Ok(Submitted {
            submission_id: created_id(&submission, "send")?,
            email_id: email_id.clone(),
            email_update: resp.try_get_named(sub.id(), "Email/set")?,
            submission,
        })
    }

    /// Create a draft and send it in a single request.
    pub fn send_new_email(
        &self,
        draft: EmailCreate,
        identity_id: &Id,
        mailboxes: &SendMailboxes,
        envelope: Option<Envelope>,
    ) -> Result<Submitted> {
        let mut req = RequestBuilder::new();
        let create = req.call(Set::<Email>::new(self.mail_account_id()?).create("draft", draft));
        let sub =
            req.call(self.submission_set("#draft".to_owned(), identity_id, envelope, mailboxes)?);
        let resp = self.send(req)?;
        let email_id = created_id(&resp.get(&create)?, "draft")?;
        let submission = resp.get(&sub)?;
        Ok(Submitted {
            submission_id: created_id(&submission, "send")?,
            email_id,
            email_update: resp.try_get_named(sub.id(), "Email/set")?,
            submission,
        })
    }
}
