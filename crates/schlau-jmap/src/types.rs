use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::string_enum;

/// A JMAP `Id` (RFC 8620 section 1.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(String);

impl Id {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for Id {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&Id> for Id {
    fn from(id: &Id) -> Self {
        id.clone()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::borrow::Borrow<str> for Id {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// An opaque server state string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct State(String);

impl State {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for State {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// RFC 3339 `UTCDate`, kept as the server's string (lexicographic order equals time order).
pub type UtcDate = String;

/// A set of ids as JMAP encodes it: `{ id: true }`.
pub type IdSet = HashMap<Id, bool>;

pub fn id_set<I, T>(ids: I) -> IdSet
where
    I: IntoIterator<Item = T>,
    T: Into<Id>,
{
    ids.into_iter().map(|id| (id.into(), true)).collect()
}

/// A back-reference to an earlier call's result (RFC 8620 section 3.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultReference {
    pub result_of: String,
    pub name: String,
    pub path: String,
}

/// The `ids` argument of `/get` and similar: a literal list, `null` (all), or a back-reference.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum IdsArg {
    #[serde(rename = "ids")]
    List(Option<Vec<Id>>),
    #[serde(rename = "#ids")]
    Ref(ResultReference),
}

impl Default for IdsArg {
    fn default() -> Self {
        Self::List(None)
    }
}

impl IdsArg {
    pub fn all() -> Self {
        Self::List(None)
    }

    pub fn list<I, T>(ids: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        Self::List(Some(ids.into_iter().map(Into::into).collect()))
    }
}

impl From<ResultReference> for IdsArg {
    fn from(r: ResultReference) -> Self {
        Self::Ref(r)
    }
}

impl From<Vec<Id>> for IdsArg {
    fn from(ids: Vec<Id>) -> Self {
        Self::List(Some(ids))
    }
}

/// A `/set` patch: JSON-pointer paths (relative to the object) to new values; `null` removes.
pub type PatchObject = Map<String, Value>;

/// Escape a map key for use as one JSON pointer path segment (RFC 6901).
pub fn pointer_segment(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// Sort comparator (RFC 8620 section 5.5, RFC 8621 section 4.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparator {
    pub property: String,
    #[serde(default = "default_true")]
    pub is_ascending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Comparator {
    pub fn asc(property: impl Into<String>) -> Self {
        Self {
            property: property.into(),
            is_ascending: true,
            collation: None,
            keyword: None,
        }
    }

    pub fn desc(property: impl Into<String>) -> Self {
        Self {
            is_ascending: false,
            ..Self::asc(property)
        }
    }

    pub fn received_at_desc() -> Self {
        Self::desc("receivedAt")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Operator {
    And,
    Or,
    Not,
}

/// A filter tree: either a leaf condition or an operator over sub-filters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Filter<C> {
    Operator(FilterOperator<C>),
    Condition(C),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FilterOperator<C> {
    pub operator: Operator,
    pub conditions: Vec<Filter<C>>,
}

impl<C> Filter<C> {
    fn op(operator: Operator, conditions: impl IntoIterator<Item = impl Into<Filter<C>>>) -> Self {
        Self::Operator(FilterOperator {
            operator,
            conditions: conditions.into_iter().map(Into::into).collect(),
        })
    }

    pub fn and(conditions: impl IntoIterator<Item = impl Into<Filter<C>>>) -> Self {
        Self::op(Operator::And, conditions)
    }

    pub fn or(conditions: impl IntoIterator<Item = impl Into<Filter<C>>>) -> Self {
        Self::op(Operator::Or, conditions)
    }

    /// Matches when none of the conditions match.
    pub fn not(conditions: impl IntoIterator<Item = impl Into<Filter<C>>>) -> Self {
        Self::op(Operator::Not, conditions)
    }
}

/// Used for object types that are never queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NoCondition {}

/// Extra method arguments for object types that define none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct NoArgs {}

string_enum! {
    /// `SetError` types from RFC 8620 section 5.3 and RFC 8621.
    pub enum SetErrorKind {
        Forbidden => "forbidden",
        OverQuota => "overQuota",
        TooLarge => "tooLarge",
        RateLimit => "rateLimit",
        NotFound => "notFound",
        InvalidPatch => "invalidPatch",
        WillDestroy => "willDestroy",
        InvalidProperties => "invalidProperties",
        Singleton => "singleton",
        AlreadyExists => "alreadyExists",
        MailboxHasChild => "mailboxHasChild",
        MailboxHasEmail => "mailboxHasEmail",
        BlobNotFound => "blobNotFound",
        TooManyKeywords => "tooManyKeywords",
        TooManyMailboxes => "tooManyMailboxes",
        InvalidEmail => "invalidEmail",
        TooManyRecipients => "tooManyRecipients",
        NoRecipients => "noRecipients",
        InvalidRecipients => "invalidRecipients",
        ForbiddenMailFrom => "forbiddenMailFrom",
        ForbiddenFrom => "forbiddenFrom",
        ForbiddenToSend => "forbiddenToSend",
        CannotUnsend => "cannotUnsend",
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SetError {
    #[serde(rename = "type")]
    pub kind: SetErrorKind,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub properties: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Serialize)]
    struct Args {
        #[serde(flatten)]
        ids: IdsArg,
    }

    #[test]
    fn ids_arg_serializes_as_literal_null_or_reference() {
        let lit = serde_json::to_value(Args {
            ids: IdsArg::list(["a", "b"]),
        })
        .unwrap();
        assert_eq!(lit, json!({"ids": ["a", "b"]}));
        let all = serde_json::to_value(Args { ids: IdsArg::all() }).unwrap();
        assert_eq!(all, json!({"ids": null}));
        let r = serde_json::to_value(Args {
            ids: IdsArg::Ref(ResultReference {
                result_of: "t0".into(),
                name: "Email/query".into(),
                path: "/ids".into(),
            }),
        })
        .unwrap();
        assert_eq!(
            r,
            json!({"#ids": {"resultOf": "t0", "name": "Email/query", "path": "/ids"}})
        );
    }

    #[test]
    fn filter_tree_serializes_like_rfc() {
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct C {
            #[serde(rename = "hasKeyword")]
            has_keyword: String,
        }
        let f: Filter<C> = Filter::and([
            Filter::Condition(C {
                has_keyword: "$flagged".into(),
            }),
            Filter::not([Filter::Condition(C {
                has_keyword: "$seen".into(),
            })]),
        ]);
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(
            v,
            json!({"operator": "AND", "conditions": [
                {"hasKeyword": "$flagged"},
                {"operator": "NOT", "conditions": [{"hasKeyword": "$seen"}]}
            ]})
        );
        let back: Filter<C> = serde_json::from_value(v).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn comparator_and_pointer_escape() {
        assert_eq!(
            serde_json::to_value(Comparator::received_at_desc()).unwrap(),
            json!({"property": "receivedAt", "isAscending": false})
        );
        assert_eq!(pointer_segment("a/b~c"), "a~1b~0c");
    }

    #[test]
    fn set_error_parses_properties() {
        let e: SetError = serde_json::from_value(json!({
            "type": "invalidProperties", "properties": ["mailboxIds"], "description": "bad"
        }))
        .unwrap();
        assert_eq!(e.kind, SetErrorKind::InvalidProperties);
        assert_eq!(e.properties.unwrap(), vec!["mailboxIds"]);
    }
}
