use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::method::JmapObject;
use crate::session::capability;
use crate::types::{Id, NoArgs};

/// Mailbox (RFC 8621 section 2). Every field but `id` may be absent in partial responses.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mailbox {
    pub id: Id,
    pub name: Option<String>,
    pub parent_id: Option<Id>,
    /// e.g. `inbox`, `archive`, `drafts`, `sent`, `trash`, `junk`.
    pub role: Option<String>,
    pub sort_order: Option<u32>,
    pub total_emails: Option<u64>,
    pub unread_emails: Option<u64>,
    pub total_threads: Option<u64>,
    pub unread_threads: Option<u64>,
    pub my_rights: Option<MailboxRights>,
    pub is_subscribed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MailboxRights {
    pub may_read_items: bool,
    pub may_add_items: bool,
    pub may_remove_items: bool,
    pub may_set_seen: bool,
    pub may_set_keywords: bool,
    pub may_create_child: bool,
    pub may_rename: bool,
    pub may_delete: bool,
    pub may_submit: bool,
}

pub mod role {
    pub const INBOX: &str = "inbox";
    pub const ARCHIVE: &str = "archive";
    pub const DRAFTS: &str = "drafts";
    pub const SENT: &str = "sent";
    pub const TRASH: &str = "trash";
    pub const JUNK: &str = "junk";
}

/// Mailbox/query filter (RFC 8621 section 2.3).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxFilterCondition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Option<Id>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_any_role: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxQueryArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_as_tree: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_as_tree: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxSetArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_destroy_remove_emails: Option<bool>,
}

impl JmapObject for Mailbox {
    const TYPE: &'static str = "Mailbox";
    const USING: &'static [&'static str] = &[capability::MAIL];
    type Condition = MailboxFilterCondition;
    type Create = Value;
    type GetArgs = NoArgs;
    type QueryArgs = MailboxQueryArgs;
    type SetArgs = MailboxSetArgs;
}

pub fn find_by_role<'a>(mailboxes: &'a [Mailbox], role: &str) -> Option<&'a Mailbox> {
    mailboxes.iter().find(|m| m.role.as_deref() == Some(role))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::{ChangesResponse, GetResponse};

    #[test]
    fn parses_rfc8621_mailbox_get() {
        let resp: GetResponse<Mailbox> =
            serde_json::from_str(include_str!("../fixtures/mailbox_get_rfc8621.json")).unwrap();
        assert_eq!(resp.state.as_str(), "78540");
        assert_eq!(resp.list.len(), 3);
        let inbox = find_by_role(&resp.list, role::INBOX).unwrap();
        assert_eq!(inbox.name.as_deref(), Some("Inbox"));
        assert_eq!(inbox.unread_emails, Some(3));
        assert!(inbox.my_rights.unwrap().may_read_items);
        assert!(!inbox.my_rights.unwrap().may_delete);
        let trash = find_by_role(&resp.list, role::TRASH).unwrap();
        assert_eq!(trash.parent_id, None);
        assert_eq!(resp.not_found, Vec::<Id>::new());
    }

    #[test]
    fn partial_mailbox_and_updated_properties() {
        let resp: ChangesResponse = serde_json::from_str(
            r#"{"accountId":"u1","oldState":"78541","newState":"78542","hasMoreChanges":false,
                "updatedProperties":["totalEmails","unreadEmails","totalThreads","unreadThreads"],
                "created":[],"updated":["MB23cfa8094c0f41e6"],"destroyed":[]}"#,
        )
        .unwrap();
        assert_eq!(resp.updated_properties.as_ref().unwrap().len(), 4);
        let partial: Mailbox =
            serde_json::from_str(r#"{"id":"MB23cfa8094c0f41e6","unreadEmails":2}"#).unwrap();
        assert_eq!(partial.unread_emails, Some(2));
        assert_eq!(partial.name, None);
    }
}
