use serde::{Deserialize, Serialize};

/// The JMAP system keywords the UI filters and draws on, packed so a list row reads one integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Flags(pub u32);

impl Flags {
    pub const SEEN: Flags = Flags(1);
    pub const FLAGGED: Flags = Flags(1 << 1);
    pub const ANSWERED: Flags = Flags(1 << 2);
    pub const DRAFT: Flags = Flags(1 << 3);
    pub const FORWARDED: Flags = Flags(1 << 4);

    const KEYWORDS: [(&'static str, Flags); 5] = [
        ("$seen", Flags::SEEN),
        ("$flagged", Flags::FLAGGED),
        ("$answered", Flags::ANSWERED),
        ("$draft", Flags::DRAFT),
        ("$forwarded", Flags::FORWARDED),
    ];

    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn with(self, other: Flags, on: bool) -> Flags {
        if on { Flags(self.0 | other.0) } else { Flags(self.0 & !other.0) }
    }

    /// The JMAP keyword for a flag, for building `Email/set` patches.
    pub fn keyword(self) -> Option<&'static str> {
        Self::KEYWORDS.iter().find(|(_, f)| *f == self).map(|(k, _)| *k)
    }

    /// Splits JMAP keywords into system flags and the remaining custom keywords.
    pub fn from_keywords<'a>(keywords: impl IntoIterator<Item = &'a str>) -> (Flags, Vec<String>) {
        keywords.into_iter().fold((Flags::default(), Vec::new()), |(flags, mut rest), keyword| {
            let lower = keyword.to_ascii_lowercase();
            match Self::KEYWORDS.iter().find(|(k, _)| *k == lower) {
                Some((_, flag)) => (flags.with(*flag, true), rest),
                None => {
                    rest.push(keyword.to_string());
                    (flags, rest)
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Address {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub email: String,
}

impl Address {
    /// The name to show for this address: its display name, else the part of the address before `@`.
    pub fn display(&self) -> &str {
        if !self.name.trim().is_empty() {
            self.name.trim()
        } else {
            self.email.split('@').next().unwrap_or(&self.email)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Addresses {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<Address>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sender: Vec<Address>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<Address>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<Address>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<Address>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reply_to: Vec<Address>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    /// The JMAP role (`inbox`, `archive`, `drafts`, `sent`, `trash`, `junk`, …), lowercase.
    pub role: Option<String>,
    pub sort_order: i64,
    pub total_emails: i64,
    pub unread_emails: i64,
    pub total_threads: i64,
    pub unread_threads: i64,
    pub is_subscribed: bool,
}

/// A message as sync writes it: everything except the body, which arrives separately.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MessageRecord {
    pub id: String,
    pub thread_id: String,
    pub blob_id: Option<String>,
    /// Seconds since the Unix epoch.
    pub received_at: i64,
    pub sent_at: Option<i64>,
    pub size: i64,
    pub subject: String,
    pub addresses: Addresses,
    pub preview: String,
    pub has_attachment: bool,
    pub flags: Flags,
    pub keywords: Vec<String>,
    pub mailbox_ids: Vec<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

/// The mutable part of a JMAP email: its keywords and mailboxes. `None` leaves a part unchanged.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MessageUpdate {
    pub id: String,
    pub flags: Option<Flags>,
    pub keywords: Option<Vec<String>>,
    pub mailbox_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Attachment {
    pub blob_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub cid: Option<String>,
    #[serde(default)]
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Body {
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// The text the full-text index holds for a message's body: plain text (extracted from HTML when
/// there is no text part), with quoted history removed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BodyIndex {
    pub text: String,
}

/// One row of a mailbox's list: a conversation with at least one message in that mailbox.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub thread_id: String,
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

/// Keyset cursor for paging a mailbox newest-first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageCursor<'a> {
    pub latest_at: i64,
    pub thread_id: &'a str,
}

/// A message inside an opened conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageView {
    pub rowid: i64,
    pub id: String,
    pub thread_id: String,
    pub received_at: i64,
    pub subject: String,
    pub addresses: Addresses,
    pub preview: String,
    pub flags: Flags,
    pub has_attachment: bool,
    pub mailbox_ids: Vec<String>,
    pub body_fetched: bool,
    pub message_id: Option<String>,
    pub references: Vec<String>,
}

/// A queued change to replay against the server, already applied locally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    SetFlag { message_ids: Vec<String>, flag: String, on: bool },
    Move { message_ids: Vec<String>, from: Option<String>, to: String },
    AddMailbox { message_ids: Vec<String>, mailbox_id: String },
    RemoveMailbox { message_ids: Vec<String>, mailbox_id: String },
    Destroy { message_ids: Vec<String> },
    /// Adds the card holding `email` to a contact group, making the card (with `uid`) when there is
    /// none.
    AddToGroup { group_id: String, email: String, name: String, uid: String },
    /// Takes the cards holding `email` out of a group. With `delete_card`, the server also deletes
    /// the card `uid` when that leaves it in no group: the card the matching `AddToGroup` made.
    RemoveFromGroup { group_id: String, email: String, name: String, uid: String, delete_card: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxItem {
    pub id: i64,
    pub action: Action,
    pub attempts: i64,
}

/// A contact card as the replica keeps it: emails lowercased, group members by `uid`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactRecord {
    pub id: String,
    pub uid: String,
    pub kind: String,
    pub name: String,
    pub emails: Vec<String>,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactGroup {
    pub id: String,
    pub name: String,
    pub members: i64,
}

/// Everything one sender has in the screened folder, and what the replica knows about them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScreenerSender {
    /// Lowercased.
    pub email: String,
    pub name: String,
    /// Their messages in the folder, newest first.
    pub message_ids: Vec<String>,
    pub unread: i64,
    pub latest_at: i64,
    pub latest_subject: String,
    pub latest_thread: String,
    /// The address is on a contact card.
    pub contact: bool,
    /// Something in Sent was addressed to them.
    pub replied: bool,
    /// Their messages elsewhere: mailbox id and count, most first.
    pub elsewhere: Vec<(String, i64)>,
    /// Groups holding other addresses at the same domain: group id and how many, most first.
    pub domain_groups: Vec<(String, i64)>,
}

/// A message to move because its sender's group says where it belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misrouted {
    pub message_id: String,
    pub mailbox_id: String,
    pub group_id: String,
}

/// What the account's rules did with new mail from a group's members.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutingStats {
    pub routed: i64,
    pub missed: i64,
    pub last_missed_at: Option<i64>,
}
