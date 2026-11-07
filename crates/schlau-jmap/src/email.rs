use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::method::JmapObject;
use crate::session::capability;
use crate::types::{Id, IdSet, UtcDate};

pub mod keyword {
    pub const SEEN: &str = "$seen";
    pub const FLAGGED: &str = "$flagged";
    pub const DRAFT: &str = "$draft";
    pub const ANSWERED: &str = "$answered";
    pub const FORWARDED: &str = "$forwarded";
    pub const JUNK: &str = "$junk";
    pub const NOT_JUNK: &str = "$notjunk";
    pub const PHISHING: &str = "$phishing";
}

pub mod property {
    /// Everything needed to render a message list row and thread metadata, no bodies.
    pub const HEADERS: &[&str] = &[
        "id",
        "blobId",
        "threadId",
        "mailboxIds",
        "keywords",
        "size",
        "receivedAt",
        "messageId",
        "inReplyTo",
        "references",
        "sender",
        "from",
        "to",
        "cc",
        "bcc",
        "replyTo",
        "subject",
        "sentAt",
        "hasAttachment",
        "preview",
    ];

    pub const BODY: &[&str] = &["bodyValues", "textBody", "htmlBody", "attachments"];

    /// The only properties of an existing Email that can change.
    pub const MUTABLE: &[&str] = &["id", "mailboxIds", "keywords"];

    pub const BODY_PART: &[&str] = &[
        "partId",
        "blobId",
        "size",
        "name",
        "type",
        "charset",
        "disposition",
        "cid",
    ];

    pub fn to_vec(props: &[&str]) -> Vec<String> {
        props.iter().map(|p| (*p).to_owned()).collect()
    }

    pub fn headers_and_body() -> Vec<String> {
        HEADERS
            .iter()
            .chain(BODY)
            .map(|p| (*p).to_owned())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct EmailAddress {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: String,
}

impl EmailAddress {
    pub fn new(name: Option<&str>, email: &str) -> Self {
        Self {
            name: name.map(str::to_owned),
            email: email.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EmailHeader {
    pub name: String,
    pub value: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailBodyValue {
    pub value: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_encoding_problem: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_truncated: bool,
}

/// A MIME part (RFC 8621 section 4.1.4). All fields optional because `bodyProperties` selects them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EmailBodyPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<EmailHeader>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_parts: Option<Vec<EmailBodyPart>>,
}

impl EmailBodyPart {
    /// A body part for `Email/set` create whose content is in `bodyValues[part_id]`.
    pub fn inline(part_id: &str, mime_type: &str) -> Self {
        Self {
            part_id: Some(part_id.to_owned()),
            mime_type: Some(mime_type.to_owned()),
            ..Self::default()
        }
    }

    /// An attachment for `Email/set` create referencing an uploaded blob.
    pub fn attachment(blob_id: Id, mime_type: &str, name: &str) -> Self {
        Self {
            blob_id: Some(blob_id),
            mime_type: Some(mime_type.to_owned()),
            name: Some(name.to_owned()),
            disposition: Some("attachment".to_owned()),
            ..Self::default()
        }
    }
}

/// Email (RFC 8621 section 4). Only `id` is always present; everything else depends on the
/// requested properties. Unmodelled properties (e.g. `header:List-Id:asText`) land in `extra`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Email {
    pub id: Id,
    pub blob_id: Option<Id>,
    pub thread_id: Option<Id>,
    pub mailbox_ids: Option<IdSet>,
    pub keywords: Option<HashMap<String, bool>>,
    pub size: Option<u64>,
    pub received_at: Option<UtcDate>,
    pub message_id: Option<Vec<String>>,
    pub in_reply_to: Option<Vec<String>>,
    pub references: Option<Vec<String>>,
    pub sender: Option<Vec<EmailAddress>>,
    pub from: Option<Vec<EmailAddress>>,
    pub to: Option<Vec<EmailAddress>>,
    pub cc: Option<Vec<EmailAddress>>,
    pub bcc: Option<Vec<EmailAddress>>,
    pub reply_to: Option<Vec<EmailAddress>>,
    pub subject: Option<String>,
    pub sent_at: Option<String>,
    pub has_attachment: Option<bool>,
    pub preview: Option<String>,
    pub body_values: Option<HashMap<String, EmailBodyValue>>,
    pub text_body: Option<Vec<EmailBodyPart>>,
    pub html_body: Option<Vec<EmailBodyPart>>,
    pub attachments: Option<Vec<EmailBodyPart>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Email {
    pub fn has_keyword(&self, keyword: &str) -> bool {
        self.keywords
            .as_ref()
            .is_some_and(|k| k.get(keyword).copied().unwrap_or(false))
    }

    pub fn is_unread(&self) -> bool {
        !self.has_keyword(keyword::SEEN)
    }

    pub fn is_in_mailbox(&self, mailbox_id: &Id) -> bool {
        self.mailbox_ids
            .as_ref()
            .is_some_and(|m| m.get(mailbox_id).copied().unwrap_or(false))
    }

    /// Concatenated fetched values of the given parts (e.g. `text_body`), in order.
    pub fn body_text(&self, parts: &[EmailBodyPart]) -> Option<String> {
        let values = self.body_values.as_ref()?;
        let texts: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.part_id.as_ref())
            .filter_map(|id| values.get(id))
            .map(|v| v.value.as_str())
            .collect();
        (!texts.is_empty()).then(|| texts.concat())
    }
}

/// Body fetching options for `Email/get` (RFC 8621 section 4.2).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailGetArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_properties: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_text_body_values: Option<bool>,
    #[serde(
        rename = "fetchHTMLBodyValues",
        skip_serializing_if = "Option::is_none"
    )]
    pub fetch_html_body_values: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_all_body_values: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_body_value_bytes: Option<u64>,
}

impl EmailGetArgs {
    /// Fetch text and HTML body values, truncated at `max_bytes` (0 means no limit).
    pub fn with_bodies(max_bytes: u64) -> Self {
        Self {
            body_properties: Some(property::to_vec(property::BODY_PART)),
            fetch_text_body_values: Some(true),
            fetch_html_body_values: Some(true),
            fetch_all_body_values: None,
            max_body_value_bytes: (max_bytes > 0).then_some(max_bytes),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailQueryArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapse_threads: Option<bool>,
}

/// An Email to create with `Email/set`, typically a draft.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailCreate {
    pub mailbox_ids: IdSet,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub keywords: HashMap<String, bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<Vec<EmailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<EmailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<EmailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<EmailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_body: Option<Vec<EmailBodyPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_body: Option<Vec<EmailBodyPart>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<EmailBodyPart>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub body_values: HashMap<String, EmailBodyValue>,
    /// Raw extra properties, e.g. `"header:X-Mailer:asText"`.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl EmailCreate {
    /// A plain-text draft in `drafts_mailbox` with `$draft` and `$seen` set.
    pub fn draft(
        drafts_mailbox: impl Into<Id>,
        from: EmailAddress,
        subject: &str,
        text: &str,
    ) -> Self {
        Self {
            mailbox_ids: HashMap::from([(drafts_mailbox.into(), true)]),
            keywords: HashMap::from([
                (keyword::DRAFT.to_owned(), true),
                (keyword::SEEN.to_owned(), true),
            ]),
            from: Some(vec![from]),
            subject: Some(subject.to_owned()),
            text_body: Some(vec![EmailBodyPart::inline("text", "text/plain")]),
            body_values: HashMap::from([(
                "text".to_owned(),
                EmailBodyValue {
                    value: text.to_owned(),
                    ..EmailBodyValue::default()
                },
            )]),
            ..Self::default()
        }
    }

    pub fn to(mut self, to: Vec<EmailAddress>) -> Self {
        self.to = Some(to);
        self
    }

    pub fn cc(mut self, cc: Vec<EmailAddress>) -> Self {
        self.cc = Some(cc);
        self
    }

    pub fn html(mut self, html: &str) -> Self {
        self.html_body = Some(vec![EmailBodyPart::inline("html", "text/html")]);
        self.body_values.insert(
            "html".to_owned(),
            EmailBodyValue {
                value: html.to_owned(),
                ..EmailBodyValue::default()
            },
        );
        self
    }

    pub fn attach(mut self, part: EmailBodyPart) -> Self {
        self.attachments.push(part);
        self
    }

    pub fn reply_to_message(mut self, message_ids: Vec<String>, references: Vec<String>) -> Self {
        self.in_reply_to = Some(message_ids);
        self.references = Some(references);
        self
    }
}

impl JmapObject for Email {
    const TYPE: &'static str = "Email";
    const USING: &'static [&'static str] = &[capability::MAIL];
    type Condition = crate::email_filter::EmailFilterCondition;
    type Create = EmailCreate;
    type GetArgs = EmailGetArgs;
    type QueryArgs = EmailQueryArgs;
    type SetArgs = crate::types::NoArgs;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::GetResponse;
    use serde_json::json;

    #[test]
    fn parses_rfc8621_email_get_with_body_values() {
        let resp: GetResponse<Email> =
            serde_json::from_str(include_str!("../fixtures/email_get_rfc8621.json")).unwrap();
        assert_eq!(resp.not_found, vec![Id::from("f123u456")]);
        let e = &resp.list[0];
        assert_eq!(e.id.as_str(), "f123u457");
        assert_eq!(e.thread_id.as_ref().unwrap().as_str(), "ef1314a");
        assert!(e.is_in_mailbox(&Id::from("f123")));
        assert_eq!(e.from.as_ref().unwrap()[0].email, "joe@example.com");
        assert_eq!(e.keywords, None);
        let html = e.html_body.as_ref().unwrap();
        assert_eq!(html[0].mime_type.as_deref(), Some("text/html"));
        assert_eq!(html[0].size, Some(283331));
        let values = e.body_values.as_ref().unwrap();
        assert!(values["1"].is_truncated);
        assert!(!values["2"].is_truncated);
        assert_eq!(
            e.extra["header:List-POST:asURLs"],
            json!(["mailto:partytime@lists.example.com"])
        );
    }

    #[test]
    fn parses_fastmail_full_email() {
        let resp: GetResponse<Email> =
            serde_json::from_str(include_str!("../fixtures/email_get_fastmail.json")).unwrap();
        let e = &resp.list[0];
        assert!(e.is_unread());
        assert!(e.has_keyword(keyword::FLAGGED));
        assert_eq!(
            e.message_id.as_ref().unwrap()[0],
            "CAF=abc123@mail.example.com"
        );
        assert_eq!(e.has_attachment, Some(true));
        let att = &e.attachments.as_ref().unwrap()[0];
        assert_eq!(att.name.as_deref(), Some("invoice-2026-10.pdf"));
        assert_eq!(att.disposition.as_deref(), Some("attachment"));
        assert_eq!(
            att.blob_id.as_ref().unwrap().as_str(),
            "G7f3e2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f"
        );
        let inline = &e.html_body.as_ref().unwrap()[1];
        assert_eq!(inline.cid.as_deref(), Some("logo@example.com"));
        assert_eq!(
            e.body_text(e.text_body.as_ref().unwrap()).unwrap(),
            "Hi Jane,\n\nYour invoice for October is attached.\n"
        );
        assert_eq!(e.reply_to, None);
        assert!(e.extra.is_empty());

        let minimal = &resp.list[1];
        assert_eq!(minimal.id.as_str(), "M2b");
        assert!(minimal.subject.is_none() && minimal.mailbox_ids.is_some());
        assert!(!minimal.is_unread());
    }

    #[test]
    fn get_args_serialize_with_rfc_names() {
        let v = serde_json::to_value(EmailGetArgs::with_bodies(65536)).unwrap();
        assert_eq!(v["fetchHTMLBodyValues"], true);
        assert_eq!(v["fetchTextBodyValues"], true);
        assert_eq!(v["maxBodyValueBytes"], 65536);
        assert_eq!(v["bodyProperties"][0], "partId");
        assert!(v.get("fetchAllBodyValues").is_none());
    }

    #[test]
    fn draft_create_serializes() {
        let draft = EmailCreate::draft(
            "drafts",
            EmailAddress::new(Some("Jane"), "jane@fastmail.com"),
            "Hello",
            "Body text",
        )
        .to(vec![EmailAddress::new(None, "bob@example.com")])
        .attach(EmailBodyPart::attachment(
            Id::from("Gblob"),
            "image/png",
            "a.png",
        ));
        let v = serde_json::to_value(draft).unwrap();
        assert_eq!(v["mailboxIds"], json!({"drafts": true}));
        assert_eq!(v["keywords"], json!({"$draft": true, "$seen": true}));
        assert_eq!(
            v["textBody"],
            json!([{"partId": "text", "type": "text/plain"}])
        );
        assert_eq!(v["bodyValues"], json!({"text": {"value": "Body text"}}));
        assert_eq!(v["to"], json!([{"name": null, "email": "bob@example.com"}]));
        assert_eq!(
            v["attachments"],
            json!([{"blobId": "Gblob", "type": "image/png", "name": "a.png", "disposition": "attachment"}])
        );
        assert!(v.get("htmlBody").is_none());
    }
}
