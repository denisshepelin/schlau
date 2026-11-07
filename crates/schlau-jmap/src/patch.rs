//! Builders for `Email/set` update patches (RFC 8620 section 5.3 patch syntax).

use serde_json::Value;

use crate::types::{Id, PatchObject, pointer_segment};

/// A patch for one or many Emails, e.g. `{"keywords/$seen": true, "mailboxIds/abc": null}`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EmailPatch(PatchObject);

impl EmailPatch {
    pub fn new() -> Self {
        Self::default()
    }

    fn with(mut self, path: String, value: Value) -> Self {
        self.0.insert(path, value);
        self
    }

    /// Set (`true`) or remove (`null`) a single keyword without touching the others.
    pub fn keyword(self, keyword: &str, on: bool) -> Self {
        let value = if on { Value::Bool(true) } else { Value::Null };
        self.with(format!("keywords/{}", pointer_segment(keyword)), value)
    }

    pub fn add_mailbox(self, mailbox_id: &Id) -> Self {
        self.with(
            format!("mailboxIds/{}", pointer_segment(mailbox_id.as_str())),
            Value::Bool(true),
        )
    }

    pub fn remove_mailbox(self, mailbox_id: &Id) -> Self {
        self.with(
            format!("mailboxIds/{}", pointer_segment(mailbox_id.as_str())),
            Value::Null,
        )
    }

    /// Remove from `from` and add to `to`, keeping any other mailboxes (labels).
    pub fn move_mailbox(self, from: &Id, to: &Id) -> Self {
        self.remove_mailbox(from).add_mailbox(to)
    }

    /// Replace the whole `mailboxIds` set.
    pub fn set_mailboxes<'a>(self, mailbox_ids: impl IntoIterator<Item = &'a Id>) -> Self {
        let set = mailbox_ids
            .into_iter()
            .map(|id| (id.as_str().to_owned(), Value::Bool(true)))
            .collect();
        self.with("mailboxIds".to_owned(), Value::Object(set))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_patch(self) -> PatchObject {
        self.0
    }
}

impl From<EmailPatch> for PatchObject {
    fn from(p: EmailPatch) -> Self {
        p.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_rfc_patch_paths() {
        let p = EmailPatch::new()
            .keyword("$seen", true)
            .keyword("$flagged", false)
            .move_mailbox(&Id::from("inbox"), &Id::from("archive"));
        assert_eq!(
            Value::Object(p.into_patch()),
            json!({
                "keywords/$seen": true,
                "keywords/$flagged": null,
                "mailboxIds/inbox": null,
                "mailboxIds/archive": true
            })
        );
    }

    #[test]
    fn set_mailboxes_replaces_whole_set() {
        let ids = [Id::from("a"), Id::from("b")];
        let p = EmailPatch::new().set_mailboxes(&ids);
        assert_eq!(
            Value::Object(p.into()),
            json!({"mailboxIds": {"a": true, "b": true}})
        );
    }

    #[test]
    fn escapes_pointer_characters_in_keywords() {
        let p = EmailPatch::new().keyword("odd/key~", true);
        assert!(p.into_patch().contains_key("keywords/odd~1key~0"));
    }
}
