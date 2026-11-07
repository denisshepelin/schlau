//! JMAP objects to store records.

use schlau_jmap::{Email, EmailAddress, EmailBodyPart, Mailbox as JmapMailbox};
use schlau_store::{Address, Addresses, Attachment, Body, Flags, Mailbox, MessageRecord, MessageUpdate};

pub fn unix_time(date: Option<&str>) -> Option<i64> {
    date.and_then(|d| d.parse::<jiff::Timestamp>().ok()).map(|t| t.as_second())
}

fn addresses(list: &Option<Vec<EmailAddress>>) -> Vec<Address> {
    list.iter()
        .flatten()
        .map(|a| Address { name: a.name.clone().unwrap_or_default(), email: a.email.clone() })
        .collect()
}

fn keywords(email: &Email) -> (Flags, Vec<String>) {
    let on = email.keywords.iter().flatten().filter(|(_, on)| **on).map(|(k, _)| k.as_str());
    Flags::from_keywords(on)
}

fn mailbox_ids(email: &Email) -> Vec<String> {
    email.mailbox_ids.iter().flatten().filter(|(_, on)| **on).map(|(id, _)| id.as_str().to_string()).collect()
}

pub fn mailbox(m: &JmapMailbox) -> Mailbox {
    Mailbox {
        id: m.id.as_str().to_string(),
        name: m.name.clone().unwrap_or_default(),
        parent_id: m.parent_id.as_ref().map(|p| p.as_str().to_string()),
        role: m.role.as_ref().map(|r| r.to_ascii_lowercase()),
        sort_order: m.sort_order.unwrap_or(0) as i64,
        total_emails: m.total_emails.unwrap_or(0) as i64,
        unread_emails: m.unread_emails.unwrap_or(0) as i64,
        total_threads: m.total_threads.unwrap_or(0) as i64,
        unread_threads: m.unread_threads.unwrap_or(0) as i64,
        is_subscribed: m.is_subscribed.unwrap_or(true),
    }
}

/// Merges a mailbox update that may carry only some properties (count changes) into what's stored.
pub fn merge_mailbox(old: &Mailbox, m: &JmapMailbox) -> Mailbox {
    Mailbox {
        id: old.id.clone(),
        name: m.name.clone().unwrap_or_else(|| old.name.clone()),
        parent_id: m.parent_id.as_ref().map(|p| p.as_str().to_string()).or_else(|| old.parent_id.clone()),
        role: m.role.as_ref().map(|r| r.to_ascii_lowercase()).or_else(|| old.role.clone()),
        sort_order: m.sort_order.map_or(old.sort_order, i64::from),
        total_emails: m.total_emails.map_or(old.total_emails, |v| v as i64),
        unread_emails: m.unread_emails.map_or(old.unread_emails, |v| v as i64),
        total_threads: m.total_threads.map_or(old.total_threads, |v| v as i64),
        unread_threads: m.unread_threads.map_or(old.unread_threads, |v| v as i64),
        is_subscribed: m.is_subscribed.unwrap_or(old.is_subscribed),
    }
}

pub fn message(email: &Email) -> MessageRecord {
    let (flags, keywords) = keywords(email);
    MessageRecord {
        id: email.id.as_str().to_string(),
        thread_id: email.thread_id.as_ref().map_or_else(|| email.id.as_str().to_string(), |t| t.as_str().to_string()),
        blob_id: email.blob_id.as_ref().map(|b| b.as_str().to_string()),
        received_at: unix_time(email.received_at.as_deref()).unwrap_or(0),
        sent_at: unix_time(email.sent_at.as_deref()),
        size: email.size.unwrap_or(0) as i64,
        subject: email.subject.clone().unwrap_or_default(),
        addresses: Addresses {
            from: addresses(&email.from),
            sender: addresses(&email.sender),
            to: addresses(&email.to),
            cc: addresses(&email.cc),
            bcc: addresses(&email.bcc),
            reply_to: addresses(&email.reply_to),
        },
        preview: email.preview.clone().unwrap_or_default(),
        has_attachment: email.has_attachment.unwrap_or(false),
        flags,
        keywords,
        mailbox_ids: mailbox_ids(email),
        message_id: email.message_id.as_ref().and_then(|ids| ids.first().cloned()),
        in_reply_to: email.in_reply_to.as_ref().and_then(|ids| ids.first().cloned()),
        references: email.references.clone().unwrap_or_default(),
    }
}

/// The mutable part of an updated email, as `Email/changes` reports it.
pub fn update(email: &Email) -> MessageUpdate {
    let (flags, keywords) = keywords(email);
    MessageUpdate {
        id: email.id.as_str().to_string(),
        flags: email.keywords.as_ref().map(|_| flags),
        keywords: email.keywords.as_ref().map(|_| keywords),
        mailbox_ids: email.mailbox_ids.as_ref().map(|_| mailbox_ids(email)),
    }
}

fn part_text(email: &Email, parts: &Option<Vec<EmailBodyPart>>, mime: &str) -> Option<String> {
    let values = email.body_values.as_ref()?;
    let text: Vec<&str> = parts
        .iter()
        .flatten()
        .filter(|p| p.mime_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(mime)))
        .filter_map(|p| p.part_id.as_ref().and_then(|id| values.get(id)))
        .map(|v| v.value.as_str())
        .collect();
    (!text.is_empty()).then(|| text.join("\n\n"))
}

pub fn body(email: &Email) -> Body {
    let attachments = email
        .attachments
        .iter()
        .flatten()
        .filter_map(|p| {
            Some(Attachment {
                blob_id: p.blob_id.as_ref()?.as_str().to_string(),
                name: p.name.clone(),
                mime: p.mime_type.clone().unwrap_or_default(),
                size: p.size.unwrap_or(0) as i64,
                cid: p.cid.clone(),
                inline: p.disposition.as_deref() == Some("inline"),
            })
        })
        .collect();
    Body {
        text: part_text(email, &email.text_body, "text/plain"),
        html: part_text(email, &email.html_body, "text/html"),
        attachments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(json: &str) -> Email {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn headers_become_a_message_record() {
        let e = email(
            r#"{"id":"M1","threadId":"T1","mailboxIds":{"inbox":true,"x":false},"keywords":{"$seen":true,"work":true},
                "receivedAt":"2026-03-01T10:00:00Z","subject":"Hi","from":[{"name":"Jane","email":"jane@x.com"}],
                "preview":"hello","hasAttachment":true,"size":1234}"#,
        );
        let m = message(&e);
        assert_eq!(m.thread_id, "T1");
        assert_eq!(m.mailbox_ids, vec!["inbox".to_string()]);
        assert!(m.flags.contains(Flags::SEEN));
        assert_eq!(m.keywords, vec!["work".to_string()]);
        assert_eq!(m.received_at, 1_772_359_200);
        assert_eq!(m.addresses.from[0].name, "Jane");
        assert!(m.has_attachment);
    }

    #[test]
    fn an_update_without_keywords_leaves_flags_alone() {
        let u = update(&email(r#"{"id":"M1","mailboxIds":{"a":true}}"#));
        assert_eq!(u.flags, None);
        assert_eq!(u.mailbox_ids, Some(vec!["a".to_string()]));
    }

    #[test]
    fn bodies_join_their_text_and_html_parts() {
        let e = email(
            r#"{"id":"M1","bodyValues":{"1":{"value":"plain"},"2":{"value":"<p>html</p>"}},
                "textBody":[{"partId":"1","type":"text/plain"}],"htmlBody":[{"partId":"2","type":"text/html"}],
                "attachments":[{"blobId":"B1","type":"application/pdf","name":"a.pdf","size":10}]}"#,
        );
        let b = body(&e);
        assert_eq!(b.text.as_deref(), Some("plain"));
        assert_eq!(b.html.as_deref(), Some("<p>html</p>"));
        assert_eq!(b.attachments[0].name.as_deref(), Some("a.pdf"));
    }
}
