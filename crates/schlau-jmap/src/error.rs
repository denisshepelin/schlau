use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $($variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            Other(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Other(s) => s,
                }
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                match s {
                    $($text => Self::$variant,)+
                    other => Self::Other(other.to_owned()),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = std::borrow::Cow::<'de, str>::deserialize(d)?;
                Ok(Self::from(s.as_ref()))
            }
        }
    };
}

pub(crate) use string_enum;

string_enum! {
    /// Method-level error `type` values from RFC 8620 section 3.6.2 and RFC 8621.
    pub enum MethodErrorKind {
        ServerUnavailable => "serverUnavailable",
        ServerFail => "serverFail",
        ServerPartialFail => "serverPartialFail",
        UnknownMethod => "unknownMethod",
        InvalidArguments => "invalidArguments",
        InvalidResultReference => "invalidResultReference",
        Forbidden => "forbidden",
        AccountNotFound => "accountNotFound",
        AccountNotSupportedByMethod => "accountNotSupportedByMethod",
        AccountReadOnly => "accountReadOnly",
        RequestTooLarge => "requestTooLarge",
        CannotCalculateChanges => "cannotCalculateChanges",
        StateMismatch => "stateMismatch",
        AnchorNotFound => "anchorNotFound",
        UnsupportedSort => "unsupportedSort",
        UnsupportedFilter => "unsupportedFilter",
        TooManyChanges => "tooManyChanges",
        FromAccountNotFound => "fromAccountNotFound",
        FromAccountNotSupportedByMethod => "fromAccountNotSupportedByMethod",
    }
}

string_enum! {
    /// Request-level problem `type` values from RFC 8620 section 3.6.1.
    pub enum ProblemKind {
        UnknownCapability => "urn:ietf:params:jmap:error:unknownCapability",
        NotJson => "urn:ietf:params:jmap:error:notJSON",
        NotRequest => "urn:ietf:params:jmap:error:notRequest",
        Limit => "urn:ietf:params:jmap:error:limit",
    }
}

/// An `error` method response: the call failed, others in the same request may not have.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MethodError {
    #[serde(rename = "type")]
    pub kind: MethodErrorKind,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(skip)]
    pub call_id: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// RFC 7807 problem details returned for a request the server rejected as a whole.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub kind: ProblemKind,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    /// For `limit` problems: the name of the limit that was exceeded, e.g. `maxCallsInRequest`.
    #[serde(default)]
    pub limit: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("transport error: {0}")]
    Transport(String),

    #[error("HTTP {status} from {url}")]
    Http {
        status: u16,
        url: String,
        body: String,
    },

    #[error("request rejected: {} ({})", .0.kind, .0.detail.as_deref().unwrap_or("no detail"))]
    Problem(ProblemDetails),

    #[error("method error in call {}: {} ({})", .0.call_id, .0.kind, .0.description.as_deref().unwrap_or("no description"))]
    Method(MethodError),

    #[error("no response for call {call_id} ({method})")]
    MissingResponse { call_id: String, method: String },

    #[error("session has no primary account for {0}")]
    NoPrimaryAccount(String),

    #[error("server does not advertise capability {0}")]
    MissingCapability(String),

    #[error("request exceeds {limit}: {actual} > {max}")]
    LocalLimit {
        limit: &'static str,
        actual: u64,
        max: u64,
    },

    #[error("invalid {what}: {detail}")]
    Invalid { what: &'static str, detail: String },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn method_kind(&self) -> Option<&MethodErrorKind> {
        match self {
            Self::Method(e) => Some(&e.kind),
            _ => None,
        }
    }

    pub fn problem_kind(&self) -> Option<&ProblemKind> {
        match self {
            Self::Problem(p) => Some(&p.kind),
            _ => None,
        }
    }

    /// The `/changes` state is too old: the caller must resync from scratch.
    pub fn is_cannot_calculate_changes(&self) -> bool {
        self.method_kind() == Some(&MethodErrorKind::CannotCalculateChanges)
    }

    pub fn is_state_mismatch(&self) -> bool {
        self.method_kind() == Some(&MethodErrorKind::StateMismatch)
    }

    /// The session may be stale (e.g. account moved): the caller should refetch it.
    pub fn suggests_session_refresh(&self) -> bool {
        matches!(
            self.method_kind(),
            Some(MethodErrorKind::AccountNotFound | MethodErrorKind::AccountNotSupportedByMethod)
        ) || matches!(
            self,
            Self::Http {
                status: 401 | 404,
                ..
            }
        )
    }

    /// A transient failure worth retrying with backoff.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) | Self::Io(_) => true,
            Self::Http { status, .. } => *status == 429 || *status >= 500,
            Self::Method(e) => matches!(
                e.kind,
                MethodErrorKind::ServerUnavailable | MethodErrorKind::ServerPartialFail
            ),
            _ => false,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_error_kinds_round_trip() {
        let e: MethodError =
            serde_json::from_str(r#"{"type":"cannotCalculateChanges","description":"too old"}"#)
                .unwrap();
        assert_eq!(e.kind, MethodErrorKind::CannotCalculateChanges);
        assert_eq!(e.description.as_deref(), Some("too old"));

        let other: MethodError = serde_json::from_str(r#"{"type":"vendorThing","x":1}"#).unwrap();
        assert_eq!(other.kind, MethodErrorKind::Other("vendorThing".into()));
        assert_eq!(other.extra["x"], 1);
        assert_eq!(
            serde_json::to_string(&MethodErrorKind::StateMismatch).unwrap(),
            "\"stateMismatch\""
        );
    }

    #[test]
    fn problem_details_limit() {
        let p: ProblemDetails = serde_json::from_str(
            r#"{"type":"urn:ietf:params:jmap:error:limit","limit":"maxSizeRequest","status":400,"detail":"too big"}"#,
        )
        .unwrap();
        assert_eq!(p.kind, ProblemKind::Limit);
        assert_eq!(p.limit.as_deref(), Some("maxSizeRequest"));
        let err = Error::Problem(p);
        assert_eq!(err.problem_kind(), Some(&ProblemKind::Limit));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_classification() {
        let http = |status| Error::Http {
            status,
            url: String::new(),
            body: String::new(),
        };
        assert!(http(503).is_retryable());
        assert!(http(429).is_retryable());
        assert!(!http(400).is_retryable());
        assert!(http(401).suggests_session_refresh());
    }
}
