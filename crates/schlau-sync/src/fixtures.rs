//! Loads a mailbox written as JMAP objects on disk, through the same conversion and indexing as
//! sync, for development, screenshots and tests. The layout:
//!
//! - `mailboxes.json`: `{"mailboxes": [<JMAP Mailbox>, …]}`
//! - `emails/<stem>.json`: a JMAP Email (headers), plus `"ago": "2h"` (`m`, `h` or `d`) to place it
//!   relative to now instead of a fixed `receivedAt`
//! - `emails/<stem>.txt` and `emails/<stem>.html`: its text and HTML bodies, both optional

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use schlau_jmap::{Email, EmailBodyPart, EmailBodyValue, Mailbox as JmapMailbox};
use schlau_store::{BodyIndex, Store};
use serde_json::Value;

use crate::convert;
use crate::index::body_index_text;

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path}: {source}")]
    Json { path: String, source: serde_json::Error },
    #[error("{path}: `ago` must look like 30m, 2h or 9d")]
    Ago { path: String },
    #[error(transparent)]
    Store(#[from] schlau_store::Error),
}

fn read(path: &Path) -> Result<String, FixtureError> {
    std::fs::read_to_string(path).map_err(|source| FixtureError::Io { path: path.display().to_string(), source })
}

fn read_optional(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn json<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T, FixtureError> {
    serde_json::from_str(text).map_err(|source| FixtureError::Json { path: path.display().to_string(), source })
}

/// Seconds in an `ago` value: `30m`, `2h`, `9d`.
pub fn ago_seconds(ago: &str) -> Option<i64> {
    let (number, unit) = ago.split_at(ago.len().checked_sub(1)?);
    let n: i64 = number.parse().ok()?;
    match unit {
        "m" => Some(n * 60),
        "h" => Some(n * 3_600),
        "d" => Some(n * 86_400),
        _ => None,
    }
}

fn rfc3339(unix: i64) -> String {
    jiff::Timestamp::from_second(unix).map(|t| t.to_string()).unwrap_or_default()
}

fn part(id: &str, mime: &str) -> EmailBodyPart {
    EmailBodyPart { part_id: Some(id.into()), mime_type: Some(mime.into()), ..Default::default() }
}

fn load_email(path: &Path, now: i64) -> Result<Email, FixtureError> {
    let mut value: Value = json(path, &read(path)?)?;
    if let Some(ago) = value.get("ago").and_then(Value::as_str).map(str::to_string) {
        let seconds = ago_seconds(&ago).ok_or_else(|| FixtureError::Ago { path: path.display().to_string() })?;
        value["receivedAt"] = Value::String(rfc3339(now - seconds));
        value["sentAt"] = Value::String(rfc3339(now - seconds - 20));
    }
    let mut email: Email = serde_json::from_value(value)
        .map_err(|source| FixtureError::Json { path: path.display().to_string(), source })?;
    let text = read_optional(&path.with_extension("txt"));
    let html = read_optional(&path.with_extension("html"));
    let mut values = HashMap::new();
    if let Some(text) = text {
        values.insert("text".to_string(), EmailBodyValue { value: text, ..Default::default() });
        email.text_body = Some(vec![part("text", "text/plain")]);
    }
    if let Some(html) = html {
        values.insert("html".to_string(), EmailBodyValue { value: html, ..Default::default() });
        email.html_body = Some(vec![part("html", "text/html")]);
    }
    email.body_values = Some(values);
    Ok(email)
}

/// Replaces the store's content with the fixture mailbox in `dir`. Returns the number of emails.
pub fn load_fixtures(store: &mut Store, dir: &Path, now: i64) -> Result<usize, FixtureError> {
    #[derive(serde::Deserialize)]
    struct Mailboxes {
        mailboxes: Vec<JmapMailbox>,
    }
    let mailboxes_path = dir.join("mailboxes.json");
    let mailboxes: Mailboxes = json(&mailboxes_path, &read(&mailboxes_path)?)?;
    let emails_dir = dir.join("emails");
    let mut paths: Vec<_> = std::fs::read_dir(&emails_dir)
        .map_err(|source| FixtureError::Io { path: emails_dir.display().to_string(), source })?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let emails = paths.iter().map(|p| load_email(p, now)).collect::<Result<Vec<_>, _>>()?;
    let keep: BTreeSet<String> = emails.iter().map(|e| e.id.as_str().to_string()).collect();
    let local = store.reader()?.mailboxes()?;
    store.write(|tx| {
        local.iter().try_for_each(|m| tx.delete_mailbox(&m.id))?;
        mailboxes.mailboxes.iter().try_for_each(|m| tx.upsert_mailbox(&convert::mailbox(m)))?;
        tx.retain_messages(&keep)?;
        emails.iter().try_for_each(|e| {
            tx.upsert_message(&convert::message(e))?;
            let body = convert::body(e);
            tx.set_body(e.id.as_str(), &body, &BodyIndex { text: body_index_text(&body) }).map(|_| ())
        })
    })?;
    store.optimize()?;
    Ok(emails.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_reads_minutes_hours_and_days() {
        assert_eq!(ago_seconds("30m"), Some(1_800));
        assert_eq!(ago_seconds("2h"), Some(7_200));
        assert_eq!(ago_seconds("9d"), Some(777_600));
        assert_eq!(ago_seconds("soon"), None);
        assert_eq!(ago_seconds(""), None);
    }
}
