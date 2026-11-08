use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::types::Id;

pub mod capability {
    pub const CORE: &str = "urn:ietf:params:jmap:core";
    pub const MAIL: &str = "urn:ietf:params:jmap:mail";
    pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
    pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
}

pub const FASTMAIL_SESSION_URL: &str = "https://api.fastmail.com/jmap/session";

/// `urn:ietf:params:jmap:core` capability limits (RFC 8620 section 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CoreCapabilities {
    pub max_size_upload: u64,
    pub max_concurrent_upload: u64,
    pub max_size_request: u64,
    #[serde(alias = "maxConcurrentRequest")]
    pub max_concurrent_requests: u64,
    pub max_calls_in_request: u64,
    pub max_objects_in_get: u64,
    pub max_objects_in_set: u64,
    pub collation_algorithms: Vec<String>,
}

impl Default for CoreCapabilities {
    /// The minimums RFC 8620 recommends servers support; used only when the server omits them.
    fn default() -> Self {
        Self {
            max_size_upload: 50_000_000,
            max_concurrent_upload: 4,
            max_size_request: 10_000_000,
            max_concurrent_requests: 4,
            max_calls_in_request: 16,
            max_objects_in_get: 500,
            max_objects_in_set: 500,
            collation_algorithms: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub name: String,
    #[serde(default)]
    pub is_personal: bool,
    #[serde(default)]
    pub is_read_only: bool,
    #[serde(default)]
    pub account_capabilities: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub capabilities: HashMap<String, Value>,
    pub accounts: HashMap<Id, Account>,
    pub primary_accounts: HashMap<String, Id>,
    pub username: String,
    pub api_url: String,
    pub download_url: String,
    pub upload_url: String,
    pub event_source_url: String,
    pub state: String,
    #[serde(skip)]
    core: CoreCapabilities,
}

impl Session {
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let mut session: Session = serde_json::from_slice(bytes)?;
        session.core = session
            .capabilities
            .get(capability::CORE)
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .ok_or_else(|| Error::MissingCapability(capability::CORE.into()))?;
        Ok(session)
    }

    pub fn core(&self) -> &CoreCapabilities {
        &self.core
    }

    pub fn has_capability(&self, uri: &str) -> bool {
        self.capabilities.contains_key(uri)
    }

    pub fn primary_account(&self, capability: &str) -> Result<&Id> {
        self.primary_accounts
            .get(capability)
            .ok_or_else(|| Error::NoPrimaryAccount(capability.into()))
    }

    pub fn mail_account_id(&self) -> Result<&Id> {
        self.primary_account(capability::MAIL)
    }

    pub fn submission_account_id(&self) -> Result<&Id> {
        self.primary_account(capability::SUBMISSION)
    }

    pub fn download_url_for(
        &self,
        account_id: &Id,
        blob_id: &Id,
        mime_type: &str,
        name: &str,
    ) -> String {
        expand(
            &self.download_url,
            &[
                ("accountId", account_id.as_str()),
                ("blobId", blob_id.as_str()),
                ("type", mime_type),
                ("name", name),
            ],
        )
    }

    pub fn upload_url_for(&self, account_id: &Id) -> String {
        expand(&self.upload_url, &[("accountId", account_id.as_str())])
    }

    /// `types` is `*` or a comma-separated list of type names; `ping` is in seconds, 0 disables.
    pub fn event_source_url_for(&self, types: &str, close_after_state: bool, ping: u32) -> String {
        let ping = ping.to_string();
        let close_after = if close_after_state { "state" } else { "no" };
        let vars = [
            ("types", types),
            ("closeafter", close_after),
            ("ping", ping.as_str()),
        ];
        if self.event_source_url.contains('{') {
            expand(&self.event_source_url, &vars)
        } else {
            let sep = if self.event_source_url.contains('?') {
                '&'
            } else {
                '?'
            };
            let query = vars
                .iter()
                .map(|(k, v)| format!("{k}={}", encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{}{sep}{query}", self.event_source_url)
        }
    }
}

/// RFC 6570 level 1 expansion. `*` and `,` are left unescaped so `types=*` stays readable.
fn expand(template: &str, vars: &[(&str, &str)]) -> String {
    vars.iter().fold(template.to_owned(), |acc, (name, value)| {
        acc.replace(&format!("{{{name}}}"), &encode(value))
    })
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'*' | b',' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FASTMAIL: &str = include_str!("../fixtures/session_fastmail.json");
    const RFC: &str = include_str!("../fixtures/session_rfc8620.json");

    #[test]
    fn parses_fastmail_session() {
        let s = Session::from_json(FASTMAIL.as_bytes()).unwrap();
        assert_eq!(s.api_url, "https://api.fastmail.com/jmap/api/");
        assert_eq!(s.mail_account_id().unwrap().as_str(), "u1a2b3c4d");
        assert_eq!(s.submission_account_id().unwrap().as_str(), "u1a2b3c4d");
        assert_eq!(s.core().max_objects_in_get, 4096);
        assert_eq!(s.core().max_calls_in_request, 50);
        assert_eq!(s.core().max_size_request, 10_000_000);
        assert!(s.has_capability(capability::SUBMISSION));
        assert!(s.accounts[&Id::from("u1a2b3c4d")].is_personal);
    }

    #[test]
    fn parses_rfc_example_session() {
        let s = Session::from_json(RFC.as_bytes()).unwrap();
        assert_eq!(s.username, "john@example.com");
        assert_eq!(s.core().max_objects_in_get, 256);
        assert_eq!(s.mail_account_id().unwrap().as_str(), "A13824");
        assert!(matches!(
            s.submission_account_id(),
            Err(Error::NoPrimaryAccount(_))
        ));
        assert_eq!(s.accounts.len(), 2);
    }

    #[test]
    fn missing_core_capability_is_an_error() {
        let json = r#"{"capabilities":{},"accounts":{},"primaryAccounts":{},"username":"",
            "apiUrl":"","downloadUrl":"","uploadUrl":"","eventSourceUrl":"","state":""}"#;
        assert!(matches!(
            Session::from_json(json.as_bytes()),
            Err(Error::MissingCapability(_))
        ));
    }

    #[test]
    fn expands_download_and_upload_templates() {
        let s = Session::from_json(RFC.as_bytes()).unwrap();
        let url = s.download_url_for(
            &Id::from("A13824"),
            &Id::from("Gabc"),
            "application/pdf",
            "Q3 report/v2.pdf",
        );
        assert_eq!(
            url,
            "https://jmap.example.com/download/A13824/Gabc/Q3%20report%2Fv2.pdf?accept=application%2Fpdf"
        );
        assert_eq!(
            s.upload_url_for(&Id::from("A13824")),
            "https://jmap.example.com/upload/A13824/"
        );
    }

    #[test]
    fn event_source_url_with_and_without_template() {
        let rfc = Session::from_json(RFC.as_bytes()).unwrap();
        assert_eq!(
            rfc.event_source_url_for("*", false, 30),
            "https://jmap.example.com/eventsource/?types=*&closeafter=no&ping=30"
        );
        let fm = Session::from_json(FASTMAIL.as_bytes()).unwrap();
        assert_eq!(
            fm.event_source_url_for("Email,Mailbox", true, 0),
            "https://api.fastmail.com/jmap/event/?types=Email,Mailbox&closeafter=state&ping=0"
        );
    }
}
