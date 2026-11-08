//! Blocking JMAP client: RFC 8620 core and RFC 8621 mail.
//!
//! [`Client`] holds a [`Session`] and a [`Transport`]. Low-level access goes through
//! [`RequestBuilder`], which batches typed method calls ([`Get`], [`Changes`], [`Query`],
//! [`QueryChanges`], [`Set`]) with result references. The `mail` module adds the round-trip
//! helpers a sync engine needs. Push events come from [`Client::event_source`].

mod client;
mod contact;
mod email;
mod email_filter;
mod error;
mod eventsource;
mod identity;
mod mail;
mod mailbox;
mod method;
mod patch;
mod request;
mod session;
mod submission;
mod thread;
mod transport;
mod types;

#[cfg(any(test, feature = "testing"))]
pub mod mock;

#[cfg(test)]
mod mail_tests;

pub use client::{Client, UploadResponse};
pub use contact::{
    AddressBook, ContactCard, ContactEmail, ContactName, NameComponent, group_card, individual_card,
    member_patch, require_success,
};
pub use email::{
    Email, EmailAddress, EmailBodyPart, EmailBodyValue, EmailCreate, EmailGetArgs, EmailHeader,
    EmailQueryArgs, keyword, property as email_property,
};
pub use email_filter::EmailFilterCondition;
pub use error::{Error, MethodError, MethodErrorKind, ProblemDetails, ProblemKind, Result};
pub use eventsource::{EventStream, PushEvent, SseEvent, SseParser, StateChange};
pub use identity::Identity;
pub use mail::{ObjectChanges, SendMailboxes, Submitted};
pub use mailbox::{
    Mailbox, MailboxFilterCondition, MailboxQueryArgs, MailboxRights, MailboxSetArgs, find_by_role,
    role,
};
pub use method::{
    AddedItem, Changes, ChangesResponse, Get, GetResponse, JmapObject, Method, PropertiesArg,
    Query, QueryChanges, QueryChangesResponse, QueryResponse, Set, SetResponse,
};
pub use patch::EmailPatch;
pub use request::{Call, Invocation, Request, RequestBuilder, Response};
pub use session::{Account, CoreCapabilities, FASTMAIL_SESSION_URL, Session, capability};
pub use submission::{
    Address, EmailSubmission, EmailSubmissionCreate, EmailSubmissionSetArgs, Envelope,
};
pub use thread::Thread;
pub use transport::{HttpResponse, Timeouts, Transport, UreqTransport};
pub use types::{
    Comparator, Filter, FilterOperator, Id, IdSet, IdsArg, NoArgs, NoCondition, Operator,
    PatchObject, ResultReference, SetError, SetErrorKind, State, UtcDate, id_set, pointer_segment,
};
