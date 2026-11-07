use serde::{Deserialize, Serialize};

use crate::types::{Filter, Id, UtcDate};

/// Email/query filter condition (RFC 8621 section 4.4.1). All set fields must match.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EmailFilterCondition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_mailbox: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_mailbox_other_than: Option<Vec<Id>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<UtcDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<UtcDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all_in_thread_have_keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub some_in_thread_have_keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub none_in_thread_have_keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_keyword: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `[name]` matches header presence, `[name, value]` a substring of its value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<Vec<String>>,
}

impl EmailFilterCondition {
    pub fn in_mailbox(id: impl Into<Id>) -> Self {
        Self {
            in_mailbox: Some(id.into()),
            ..Self::default()
        }
    }

    pub fn in_mailbox_other_than<I, T>(ids: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        Self {
            in_mailbox_other_than: Some(ids.into_iter().map(Into::into).collect()),
            ..Self::default()
        }
    }

    pub fn has_keyword(keyword: &str) -> Self {
        Self {
            has_keyword: Some(keyword.to_owned()),
            ..Self::default()
        }
    }

    pub fn not_keyword(keyword: &str) -> Self {
        Self {
            not_keyword: Some(keyword.to_owned()),
            ..Self::default()
        }
    }

    pub fn text(text: &str) -> Self {
        Self {
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    pub fn before(mut self, date: impl Into<UtcDate>) -> Self {
        self.before = Some(date.into());
        self
    }

    pub fn after(mut self, date: impl Into<UtcDate>) -> Self {
        self.after = Some(date.into());
        self
    }
}

impl From<EmailFilterCondition> for Filter<EmailFilterCondition> {
    fn from(c: EmailFilterCondition) -> Self {
        Self::Condition(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::keyword;
    use serde_json::json;

    #[test]
    fn filter_conditions_compose() {
        let f: Filter<EmailFilterCondition> = Filter::and([
            EmailFilterCondition::in_mailbox("inbox").after("2026-01-01T00:00:00Z"),
            EmailFilterCondition::not_keyword(keyword::SEEN),
        ]);
        assert_eq!(
            serde_json::to_value(f).unwrap(),
            json!({"operator": "AND", "conditions": [
                {"inMailbox": "inbox", "after": "2026-01-01T00:00:00Z"},
                {"notKeyword": "$seen"}
            ]})
        );
        assert_eq!(
            serde_json::to_value(EmailFilterCondition::in_mailbox_other_than([
                "trash", "junk"
            ]))
            .unwrap(),
            json!({"inMailboxOtherThan": ["trash", "junk"]})
        );
    }
}
